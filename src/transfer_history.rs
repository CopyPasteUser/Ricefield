//! Private, bounded transfer-result history. Historical records are display
//! evidence only: they never authorize replay, deletion, recovery or Undo.

use serde::{Deserialize, Serialize};
use std::{
    env,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    os::{
        fd::AsRawFd,
        unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    },
    path::{Component, Path, PathBuf},
    sync::mpsc::{self, Receiver, SyncSender},
    thread,
    time::{SystemTime, UNIX_EPOCH},
};

const VERSION: u32 = 1;
pub const MAX_RECORDS: usize = 64;
const MAX_BYTES: u64 = 256 * 1024;
const MAX_LABEL_BYTES: usize = 200;
const MAX_SUMMARY_BYTES: usize = 2048;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Succeeded,
    Cancelled,
    Failed,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct Counts {
    pub completed: usize,
    pub retained: usize,
    pub skipped: usize,
    pub failed: usize,
    pub not_started: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Record {
    pub id: u64,
    pub label: String,
    pub summary: String,
    pub outcome: Outcome,
    pub finished_unix_seconds: u64,
    pub elapsed_millis: u64,
    pub counts: Counts,
}

impl Record {
    fn valid(&self) -> bool {
        self.id > 0
            && !self.label.is_empty()
            && self.label.len() <= MAX_LABEL_BYTES
            && self.summary.len() <= MAX_SUMMARY_BYTES
            && self.finished_unix_seconds > 0
            && self.finished_unix_seconds <= 32_503_680_000
            && self.elapsed_millis <= 31_536_000_000
    }

    /// Messages can contain filenames and external errors. Bound their size
    /// without splitting a Unicode scalar, preserving a useful short summary.
    pub fn bounded(mut self) -> Self {
        self.label = truncate_utf8(&self.label, MAX_LABEL_BYTES);
        self.summary = truncate_utf8(&self.summary, MAX_SUMMARY_BYTES);
        self
    }
}

fn truncate_utf8(text: &str, maximum: usize) -> String {
    if text.len() <= maximum {
        return text.to_owned();
    }
    let mut end = maximum.saturating_sub('…'.len_utf8());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

#[derive(Serialize, Deserialize)]
struct Ledger {
    version: u32,
    records: Vec<Record>,
}

fn state_file() -> io::Result<PathBuf> {
    let root = env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::config::home().join(".local/state"));
    if !root.is_absolute()
        || root
            .components()
            .any(|part| matches!(part, Component::ParentDir))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "XDG_STATE_HOME must be an absolute path without '..'",
        ));
    }
    Ok(root.join("rice-explorer").join("transfer-history.toml"))
}

fn load_from(path: &Path) -> io::Result<Vec<Record>> {
    let initial = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    if !initial.is_file() || initial.len() > MAX_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Transfer history must be a regular file within the size limit",
        ));
    }
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let before = file.metadata()?;
    if !before.is_file()
        || before.len() > MAX_BYTES
        || before.ino() != initial.ino()
        || before.dev() != initial.dev()
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Transfer history changed while opening",
        ));
    }
    let mut data = Vec::new();
    file.take(MAX_BYTES + 1).read_to_end(&mut data)?;
    let after = fs::symlink_metadata(path)?;
    if data.len() as u64 > MAX_BYTES
        || !after.is_file()
        || after.ino() != before.ino()
        || after.dev() != before.dev()
        || after.len() != before.len()
        || after.mtime() != before.mtime()
        || after.mtime_nsec() != before.mtime_nsec()
        || after.ctime() != before.ctime()
        || after.ctime_nsec() != before.ctime_nsec()
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Transfer history changed while reading",
        ));
    }
    let contents = std::str::from_utf8(&data).map_err(io::Error::other)?;
    let ledger: Ledger = toml::from_str(contents).map_err(io::Error::other)?;
    if ledger.version != VERSION
        || ledger.records.len() > MAX_RECORDS
        || !ledger.records.iter().all(Record::valid)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Unsupported or invalid transfer history",
        ));
    }
    Ok(ledger.records)
}

fn save_to(path: &Path, records: &[Record]) -> io::Result<()> {
    if records.len() > MAX_RECORDS || !records.iter().all(Record::valid) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Invalid transfer-history records",
        ));
    }
    let folder = path.parent().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "History needs a parent folder")
    })?;
    prepare_private_folder(folder)?;
    match fs::symlink_metadata(path) {
        Ok(existing) if !existing.is_file() => {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Refusing to replace non-regular transfer history",
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let encoded = toml::to_string(&Ledger {
        version: VERSION,
        records: records.to_vec(),
    })
    .map_err(io::Error::other)?;
    if encoded.len() as u64 > MAX_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Transfer history size limit exceeded",
        ));
    }
    for attempt in 0..16 {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let temporary = folder.join(format!(
            ".transfer-history-{}-{nonce}-{attempt}.tmp",
            std::process::id()
        ));
        let mut output = match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
        {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        };
        let result = (|| {
            output.write_all(encoded.as_bytes())?;
            output.sync_all()?;
            fs::rename(&temporary, path)?;
            File::open(folder)?.sync_all()
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        return result;
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "Cannot allocate private history staging file",
    ))
}

fn prepare_private_folder(folder: &Path) -> io::Result<()> {
    fs::create_dir_all(folder)?;
    let metadata = fs::symlink_metadata(folder)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Transfer history directory is not private",
        ));
    }
    fs::set_permissions(folder, fs::Permissions::from_mode(0o700))
}

/// One writer per state directory. Another RiceField process may inspect
/// history, but must never overwrite snapshots produced by the first.
fn acquire_lock(path: &Path) -> io::Result<File> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "History needs a parent"))?;
    prepare_private_folder(parent)?;
    let handle = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(parent.join("transfer-history.lock"))?;
    if !handle.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "History lock is not a regular file",
        ));
    }
    // SAFETY: flock receives a valid open FD. The returned File owns that FD
    // and releases the advisory lock when the worker exits or crashes.
    if unsafe { libc::flock(handle.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(handle)
}

pub enum Event {
    Loaded(Result<Vec<Record>, String>),
    SaveError(String),
}

/// Dedicated worker serializes reads and writes, keeping blocked state-home
/// I/O entirely off egui's render thread. Only completed results are sent;
/// pending jobs or destructive replay instructions are never serialized.
pub fn spawn_worker() -> (SyncSender<Record>, Receiver<Event>, Receiver<()>) {
    let (sender, receiver, done, _) = spawn_worker_with_path(state_file());
    (sender, receiver, done)
}

fn spawn_worker_with_path(
    path: io::Result<PathBuf>,
) -> (
    SyncSender<Record>,
    Receiver<Event>,
    Receiver<()>,
    thread::JoinHandle<()>,
) {
    let (sender, receiver) = mpsc::sync_channel(64);
    let (event_sender, event_receiver) = mpsc::channel();
    let (done_sender, done_receiver) = mpsc::channel();
    let handle = thread::spawn(move || {
        let lock = path
            .as_ref()
            .map_err(|error| error.to_string())
            .and_then(|file| acquire_lock(file).map_err(|error| error.to_string()));
        let loaded = path
            .as_ref()
            .map_err(|error| error.to_string())
            .and_then(|file| load_from(file).map_err(|error| error.to_string()));
        let writable = loaded.is_ok() && lock.is_ok();
        let mut records = loaded.as_ref().cloned().unwrap_or_default();
        let mut last_persisted = records.clone();
        let _ = event_sender.send(Event::Loaded(loaded));
        if let Err(error) = &lock {
            let _ = event_sender.send(Event::SaveError(format!(
                "Transfer history is read-only (another instance or invalid lock): {error}"
            )));
        }
        let _lock_guard = lock.ok();
        let mut previously_failed = false;
        while let Ok(record) = receiver.recv() {
            let record = record.bounded();
            if !record.valid() {
                continue;
            }
            records.insert(0, record);
            records.truncate(MAX_RECORDS);
            // An unreadable/unknown-version ledger belongs to the user. A
            // later transfer must never silently replace that evidence.
            if !writable {
                continue;
            }
            let saved = path.as_ref().map_err(|error| error.to_string()).and_then(|file| {
                let current = load_from(file).map_err(|error| error.to_string())?;
                if current != last_persisted {
                    return Err("Transfer history changed outside this writer; preserving the existing file".into());
                }
                save_to(file, &records).map_err(|error| error.to_string())
            });
            if let Err(error) = saved {
                if !previously_failed {
                    let _ = event_sender.send(Event::SaveError(error));
                }
                previously_failed = true;
            } else {
                last_persisted = records.clone();
                previously_failed = false;
            }
        }
        let _ = done_sender.send(());
    });
    (sender, event_receiver, done_receiver, handle)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = env::temp_dir().join(format!(
            "ricefield-history-test-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap();
        path
    }

    fn sample() -> Record {
        Record {
            id: 7,
            label: "Copy 2 items".into(),
            summary: "2 completed".into(),
            outcome: Outcome::Succeeded,
            finished_unix_seconds: 1_784_000_000,
            elapsed_millis: 456,
            counts: Counts {
                completed: 2,
                ..Counts::default()
            },
        }
    }

    #[test]
    fn history_roundtrip_preserves_results_but_not_actions() {
        let root = fixture();
        let path = root.join("state").join("transfer-history.toml");
        assert!(load_from(&path).unwrap().is_empty());
        save_to(&path, &[sample()]).unwrap();
        assert_eq!(load_from(&path).unwrap(), vec![sample()]);
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn refuses_symlinks_oversized_and_corrupted_records() {
        use std::os::unix::fs::symlink;
        let root = fixture();
        let outside = root.join("do-not-replace");
        fs::write(&outside, b"owner data").unwrap();
        let target = root.join("transfer-history.toml");
        symlink(&outside, &target).unwrap();
        assert!(load_from(&target).is_err());
        assert!(save_to(&target, &[sample()]).is_err());
        assert_eq!(fs::read(&outside).unwrap(), b"owner data");
        fs::remove_file(&target).unwrap();
        fs::write(&target, vec![b'x'; MAX_BYTES as usize + 1]).unwrap();
        assert!(load_from(&target).is_err());
        fs::write(&target, b"version = 99\nrecords = []\n").unwrap();
        assert!(load_from(&target).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn invalid_and_overlong_history_is_never_persisted() {
        let root = fixture();
        let target = root.join("transfer-history.toml");
        assert!(save_to(&target, &[Record { id: 0, ..sample() }]).is_err());
        assert!(!target.exists());
        let sample = Record {
            summary: "ö".repeat(MAX_SUMMARY_BYTES),
            ..sample()
        }
        .bounded();
        assert!(sample.summary.is_char_boundary(sample.summary.len()));
        assert!(sample.summary.len() <= MAX_SUMMARY_BYTES + 3);
        save_to(&target, &[sample]).unwrap();
        assert_eq!(load_from(&target).unwrap().len(), 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn background_worker_saves_finished_results_in_order() {
        let root = fixture();
        let path = root.join("state/transfer-history.toml");
        let (sender, events, done, worker) = spawn_worker_with_path(Ok(path.clone()));
        assert!(matches!(events.recv().unwrap(), Event::Loaded(Ok(records)) if records.is_empty()));
        let first = sample();
        let second = Record {
            id: 8,
            outcome: Outcome::Failed,
            ..sample()
        };
        sender.send(first.clone()).unwrap();
        sender.send(second.clone()).unwrap();
        drop(sender);
        done.recv_timeout(std::time::Duration::from_secs(3))
            .unwrap();
        worker.join().unwrap();
        assert_eq!(load_from(&path).unwrap(), vec![second, first]);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn worker_does_not_overwrite_unreadable_history() {
        let root = fixture();
        let path = root.join("transfer-history.toml");
        let original = b"version = 999\nrecords = []\n";
        fs::write(&path, original).unwrap();
        let (sender, events, done, worker) = spawn_worker_with_path(Ok(path.clone()));
        assert!(matches!(events.recv().unwrap(), Event::Loaded(Err(_))));
        sender.send(sample()).unwrap();
        drop(sender);
        done.recv_timeout(std::time::Duration::from_secs(3))
            .unwrap();
        worker.join().unwrap();
        assert_eq!(fs::read(&path).unwrap(), original);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn competing_worker_cannot_replace_first_writers_history() {
        let root = fixture();
        let path = root.join("state/transfer-history.toml");
        let (first, first_events, first_done, first_worker) =
            spawn_worker_with_path(Ok(path.clone()));
        assert!(matches!(first_events.recv().unwrap(), Event::Loaded(Ok(_))));
        let (second, second_events, second_done, second_worker) =
            spawn_worker_with_path(Ok(path.clone()));
        assert!(matches!(
            second_events.recv().unwrap(),
            Event::Loaded(Ok(_))
        ));
        assert!(matches!(second_events.recv().unwrap(), Event::SaveError(_)));
        first.send(sample()).unwrap();
        second.send(Record { id: 8, ..sample() }).unwrap();
        drop(second);
        drop(first);
        second_done
            .recv_timeout(std::time::Duration::from_secs(3))
            .unwrap();
        first_done
            .recv_timeout(std::time::Duration::from_secs(3))
            .unwrap();
        second_worker.join().unwrap();
        first_worker.join().unwrap();
        assert_eq!(load_from(&path).unwrap(), vec![sample()]);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn external_history_change_after_startup_is_preserved() {
        let root = fixture();
        let path = root.join("state/transfer-history.toml");
        save_to(&path, &[sample()]).unwrap();
        let (sender, events, done, worker) = spawn_worker_with_path(Ok(path.clone()));
        assert!(matches!(events.recv().unwrap(), Event::Loaded(Ok(records)) if records.len() == 1));
        let external = b"version = 777\nrecords = []\n";
        fs::write(&path, external).unwrap();
        sender.send(Record { id: 8, ..sample() }).unwrap();
        drop(sender);
        done.recv_timeout(std::time::Duration::from_secs(3))
            .unwrap();
        worker.join().unwrap();
        assert!(matches!(events.recv().unwrap(), Event::SaveError(_)));
        assert_eq!(fs::read(&path).unwrap(), external);
        fs::remove_dir_all(root).unwrap();
    }
}
