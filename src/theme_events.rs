//! Optional Linux file events for RiceBowl's atomically replaced RiceProfile.
//!
//! Only the parent directory is watched: watching the profile inode itself
//! would silently stop working after RiceBowl's atomic rename. All filesystem
//! setup and waiting happen off the egui thread; periodic polling remains the
//! fallback for unsupported filesystems or a failed watch.

use eframe::egui;
use std::{
    ffi::CString,
    os::{fd::FromRawFd, unix::ffi::OsStrExt},
    path::Path,
    sync::{Arc, atomic::{AtomicBool, Ordering}, mpsc::{self, Receiver}},
};

pub struct Watcher {
    events: Receiver<()>,
    stop: Arc<AtomicBool>,
}

impl Watcher {
    pub fn start(path: &Path, context: &egui::Context) -> Option<Self> {
        let parent = CString::new(path.parent()?.as_os_str().as_bytes()).ok()?;
        let filename = path.file_name()?.as_bytes().to_vec();
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let context = context.clone();
        let (sender, events) = mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let fd = unsafe { libc::inotify_init1(libc::IN_CLOEXEC | libc::IN_NONBLOCK) };
            if fd < 0 {
                return;
            }
            // SAFETY: inotify_init1 returned a descriptor owned by this thread.
            let descriptor = unsafe { std::os::fd::OwnedFd::from_raw_fd(fd) };
            let mask = libc::IN_CLOSE_WRITE
                | libc::IN_MOVED_TO
                | libc::IN_CREATE
                | libc::IN_DELETE
                | libc::IN_ATTRIB
                | libc::IN_DELETE_SELF
                | libc::IN_MOVE_SELF;
            if unsafe { libc::inotify_add_watch(fd, parent.as_ptr(), mask) } < 0 {
                return;
            }
            let mut buffer = [0u8; 8192];
            while !thread_stop.load(Ordering::Relaxed) {
                let mut pollfd = libc::pollfd {
                    fd,
                    events: libc::POLLIN,
                    revents: 0,
                };
                let result = unsafe { libc::poll(&mut pollfd, 1, 500) };
                if result <= 0 {
                    continue;
                }
                let length = unsafe {
                    libc::read(fd, buffer.as_mut_ptr().cast(), buffer.len())
                };
                if length <= 0 {
                    continue;
                }
                if contains_profile_event(&buffer[..length as usize], &filename) {
                    let _ = sender.try_send(());
                    context.request_repaint();
                }
            }
            drop(descriptor);
        });
        Some(Self { events, stop })
    }

    pub fn changed(&self) -> bool {
        self.events.try_recv().is_ok()
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn contains_profile_event(buffer: &[u8], filename: &[u8]) -> bool {
    let header = std::mem::size_of::<libc::inotify_event>();
    let mut offset = 0;
    while let Some(bytes) = buffer.get(offset..offset + header) {
        // SAFETY: bounds checked; read_unaligned handles the byte buffer.
        let event = unsafe { bytes.as_ptr().cast::<libc::inotify_event>().read_unaligned() };
        let end = offset.saturating_add(header).saturating_add(event.len as usize);
        if end > buffer.len() {
            break;
        }
        if event.mask & (libc::IN_Q_OVERFLOW | libc::IN_IGNORED | libc::IN_DELETE_SELF | libc::IN_MOVE_SELF) != 0 {
            return true;
        }
        let name = &buffer[offset + header..end];
        if name.split(|byte| *byte == 0).next() == Some(filename) {
            return true;
        }
        offset = end;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, time::{Duration, Instant, SystemTime}};

    #[test]
    fn only_target_profile_events_trigger_reload() {
        fn event(name: &[u8], mask: u32) -> Vec<u8> {
            let padded = (name.len() + 1 + 3) & !3;
            let raw = libc::inotify_event { wd: 1, mask, cookie: 0, len: padded as u32 };
            let mut result = vec![0; std::mem::size_of::<libc::inotify_event>() + padded];
            unsafe { result.as_mut_ptr().cast::<libc::inotify_event>().write_unaligned(raw) };
            result[std::mem::size_of::<libc::inotify_event>()..][..name.len()].copy_from_slice(name);
            result
        }
        let target = b"ricefield.riceprofile.toml";
        assert!(contains_profile_event(&event(target, libc::IN_MOVED_TO), target));
        assert!(contains_profile_event(&event(target, libc::IN_CLOSE_WRITE), target));
        assert!(!contains_profile_event(&event(b"taskotto.ricepack.toml", libc::IN_MOVED_TO), target));
        assert!(contains_profile_event(&event(b"", libc::IN_Q_OVERFLOW), target));
        assert!(!contains_profile_event(&[0; 3], target));
    }

    #[test]
    fn atomic_ricebowl_profile_replacement_notifies_egui() {
        let unique = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "ricefield-profile-watch-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir(&directory).unwrap();
        let target = directory.join("ricefield.riceprofile.toml");
        let temporary = directory.join(".riceprofile-test");
        let context = egui::Context::default();
        let watcher = Watcher::start(&target, &context).unwrap();
        // Registration is asynchronous so retry a private atomic replace;
        // this also exercises the kernel event/worker channel end to end.
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut observed = false;
        while Instant::now() < deadline {
            fs::write(&temporary, b"format_version = 1\n").unwrap();
            fs::rename(&temporary, &target).unwrap();
            if watcher.changed() {
                observed = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(40));
        }
        drop(watcher);
        fs::remove_file(target).unwrap();
        fs::remove_dir(directory).unwrap();
        assert!(observed, "atomic profile rename was not observed");
    }
}
