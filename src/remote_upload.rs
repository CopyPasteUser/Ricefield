//! Explicit, source-preserving local-to-GIO upload. Never expose an unfinished
//! file under its requested remote name: copy to an unpredictable sibling,
//! read every byte back, then publish with a native no-overwrite GIO rename.
//! Unsupported backends fail closed; a failed upload can leave a hidden
//! staging object for manual inspection, never delete remote data blindly.

use crate::gio_native;
use crate::location::Location;
use std::{
    fs::{self, File, OpenOptions},
    io::Read,
    os::unix::{ffi::OsStrExt, fs::OpenOptionsExt},
    path::Path,
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

const MAX_UPLOAD_BYTES: u64 = 8 * 1024 * 1024 * 1024;
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(20 * 60);

#[cfg(unix)]
fn same_snapshot(before: &fs::Metadata, after: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    before.dev() == after.dev()
        && before.ino() == after.ino()
        && before.mode() == after.mode()
        && before.uid() == after.uid()
        && before.gid() == after.gid()
        && before.len() == after.len()
        && before.mtime() == after.mtime()
        && before.mtime_nsec() == after.mtime_nsec()
        && before.ctime() == after.ctime()
        && before.ctime_nsec() == after.ctime_nsec()
}

fn open_local_regular(source: &Path) -> Result<(File, fs::Metadata), String> {
    if !source.is_absolute() {
        return Err("Upload source must be an absolute local path".into());
    }
    let initial = fs::symlink_metadata(source)
        .map_err(|error| format!("Cannot inspect upload source: {error}"))?;
    if !initial.is_file() || initial.len() > MAX_UPLOAD_BYTES {
        return Err("Upload requires a regular file of at most 8 GiB; symlinks are refused".into());
    }
    let input = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(source)
        .map_err(|error| format!("Cannot open upload source: {error}"))?;
    let snapshot = input.metadata().map_err(|error| error.to_string())?;
    if !snapshot.is_file() || !same_snapshot(&initial, &snapshot) {
        return Err("Upload source changed while opening".into());
    }
    Ok((input, snapshot))
}

fn encode_name(bytes: &[u8]) -> Result<String, String> {
    if bytes.is_empty()
        || bytes.len() > 255
        || bytes == b"."
        || bytes == b".."
        || bytes.iter().any(|byte| *byte == 0 || *byte == b'/')
    {
        return Err("Invalid upload filename".into());
    }
    let mut encoded = String::with_capacity(bytes.len() * 3);
    for byte in bytes {
        if byte.is_ascii_alphanumeric() || matches!(*byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(char::from(*byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    Ok(encoded)
}

fn destination_uri(folder: &str, source: &Path) -> Result<String, String> {
    if !matches!(Location::parse(folder.into()), Ok(Location::Remote(_))) {
        return Err("Upload destination must be a validated SMB, SFTP, WebDAV or MTP URI".into());
    }
    let basename = source.file_name().ok_or("Upload source has no filename")?;
    Ok(format!(
        "{}/{}",
        folder.trim_end_matches('/'),
        encode_name(basename.as_bytes())?
    ))
}

fn check_source(source: &Path, opened: &File, initial: &fs::Metadata) -> Result<(), String> {
    let at_name =
        fs::symlink_metadata(source).map_err(|error| format!("Cannot recheck source: {error}"))?;
    let descriptor = opened.metadata().map_err(|error| error.to_string())?;
    if !same_snapshot(initial, &at_name) || !same_snapshot(initial, &descriptor) {
        return Err(
            "Upload source changed during transfer; remote staging was not published".into(),
        );
    }
    Ok(())
}

fn stopped(cancelled: &AtomicBool, deadline: Instant) -> Result<(), String> {
    if cancelled.load(Ordering::Relaxed) {
        Err("Upload cancelled; remote staging may need manual cleanup".into())
    } else if Instant::now() >= deadline {
        Err("Upload exceeded 20-minute deadline; remote staging may need manual cleanup".into())
    } else {
        Ok(())
    }
}

fn terminate(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// Compare the staged network bytes with the pinned local file descriptor.
/// A streaming read-back keeps RAM bounded and detects truncated/corrupted
/// uploads before any final filename is published.
fn verify_staging(
    stage_uri: &str,
    mut local: File,
    cancelled: &AtomicBool,
    bytes: &Arc<AtomicU64>,
    deadline: Instant,
) -> Result<(), String> {
    stopped(cancelled, deadline)?;
    let mut child = Command::new("gio")
        .args(["cat", stage_uri])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("Cannot verify staged remote content: {error}"))?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or("GIO verification stream unavailable")?;
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    let bytes = Arc::clone(bytes);
    thread::spawn(move || {
        let result = (|| {
            let mut network = [0u8; 128 * 1024];
            let mut reference = [0u8; 128 * 1024];
            let mut total = 0u64;
            loop {
                let count = stdout
                    .read(&mut network)
                    .map_err(|error| error.to_string())?;
                if count == 0 {
                    break;
                }
                total = total
                    .checked_add(count as u64)
                    .ok_or("Remote upload size overflow")?;
                if total > MAX_UPLOAD_BYTES {
                    return Err("Remote staged file exceeds 8 GiB".into());
                }
                local
                    .read_exact(&mut reference[..count])
                    .map_err(|_| "Remote upload exceeds source content")?;
                if network[..count] != reference[..count] {
                    return Err("Remote staged file differs from the source".into());
                }
                bytes.store(total, Ordering::Relaxed);
            }
            if local
                .read(&mut reference[..1])
                .map_err(|error| error.to_string())?
                != 0
            {
                return Err("Remote staged file ended before the source".into());
            }
            Ok::<_, String>(())
        })();
        let _ = sender.send(result);
    });
    loop {
        if let Ok(result) = receiver.try_recv() {
            if result.is_err() {
                terminate(&mut child);
                return result;
            }
            loop {
                match child.try_wait() {
                    Ok(Some(status)) if status.success() => return result,
                    Ok(Some(status)) => {
                        return Err(format!("GIO read-back exited unsuccessfully ({status})"));
                    }
                    Ok(None) => {}
                    Err(error) => {
                        terminate(&mut child);
                        return Err(format!("Cannot monitor upload verification: {error}"));
                    }
                }
                if let Err(error) = stopped(cancelled, deadline) {
                    terminate(&mut child);
                    return Err(error);
                }
                thread::sleep(Duration::from_millis(40));
            }
        }
        if let Err(error) = stopped(cancelled, deadline) {
            terminate(&mut child);
            return Err(error);
        }
        // If gio fails before a pipe reader gets EOF, killing it below is
        // still handled by the timeout. Never block egui on child.wait().
        thread::sleep(Duration::from_millis(40));
    }
}

/// Explicit file-only upload. The source is never moved or removed, even on
/// a successful upload. No automatic cleanup occurs on remote failure: a
/// crash or disconnect can leave a hidden stage needing manual inspection.
pub fn upload(
    source: &Path,
    remote_folder: &str,
    cancelled: &AtomicBool,
    bytes: &Arc<AtomicU64>,
) -> Result<String, String> {
    let final_uri = destination_uri(remote_folder, source)?;
    upload_to_uri(source, &final_uri, cancelled, bytes)
}

fn upload_to_uri(
    source: &Path,
    final_uri: &str,
    cancelled: &AtomicBool,
    bytes: &Arc<AtomicU64>,
) -> Result<String, String> {
    let (opened, snapshot) = open_local_regular(source)?;
    stopped(cancelled, Instant::now() + UPLOAD_TIMEOUT)?;
    bytes.store(0, Ordering::Relaxed);
    let parent = final_uri
        .rsplit_once('/')
        .ok_or("Remote target has no parent URI")?
        .0;
    // Native GIO copy refuses an existing stage. Cryptographic randomness
    // also makes a malicious pre-created filename impractical to predict.
    let mut nonce = [0u8; 16];
    File::open("/dev/urandom")
        .and_then(|mut input| input.read_exact(&mut nonce))
        .map_err(|error| format!("Cannot allocate private upload name: {error}"))?;
    let suffix = nonce
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let staged_uri = format!("{parent}/.ricefield-upload-{suffix}");
    let deadline = Instant::now() + UPLOAD_TIMEOUT;
    let staged_label = format!("Remote stage (may require manual cleanup): {staged_uri}");
    let result = (|| {
        gio_native::stage_copy(source, &staged_uri, cancelled, deadline)?;
        check_source(source, &opened, &snapshot)?;
        verify_staging(
            &staged_uri,
            opened.try_clone().map_err(|error| error.to_string())?,
            cancelled,
            bytes,
            deadline,
        )?;
        check_source(source, &opened, &snapshot)?;
        gio_native::publish_no_replace(&staged_uri, final_uri, cancelled, deadline)
    })();
    match result {
        Ok(()) => Ok(final_uri.into()),
        Err(error) => Err(format!(
            "{error}. {staged_label}; the original local file was retained"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn rejects_non_remote_parent_symlinks_and_untrusted_names() {
        assert!(destination_uri("file:///tmp", Path::new("/tmp/a")).is_err());
        assert_eq!(
            destination_uri("smb://host/share", Path::new("/tmp/a b.txt")).unwrap(),
            "smb://host/share/a%20b.txt"
        );
        assert_eq!(encode_name(&[0xff, b'#', b'z']).unwrap(), "%FF%23z");
        assert!(encode_name(b"..").is_err());
        assert!(encode_name(b"/").is_err());
    }

    #[test]
    fn local_backend_roundtrip_keeps_source_and_rejects_collisions() {
        let root = std::env::temp_dir().join(format!(
            "ricefield-upload-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        let source = root.join("source.txt");
        let remote = root.join("destination.txt");
        fs::write(&source, b"upload verification bytes\n").unwrap();
        let destination = format!("file://{}", remote.display());
        let cancel = AtomicBool::new(false);
        let bytes = Arc::new(AtomicU64::new(0));
        let done = upload_to_uri(&source, &destination, &cancel, &bytes).unwrap();
        assert_eq!(done, destination);
        assert_eq!(fs::read(&remote).unwrap(), fs::read(&source).unwrap());
        assert_eq!(bytes.load(Ordering::Relaxed), 26);
        fs::write(&source, b"a newer source must not overwrite the target").unwrap();
        assert!(upload_to_uri(&source, &destination, &cancel, &bytes).is_err());
        assert_eq!(fs::read(&remote).unwrap(), b"upload verification bytes\n");
        assert_eq!(
            fs::read(&source).unwrap(),
            b"a newer source must not overwrite the target"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_local_source_symlink_before_any_remote_mutation() {
        use std::os::unix::fs::symlink;
        let root = std::env::temp_dir().join(format!(
            "ricefield-upload-link-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        let original = root.join("original");
        let symlink_path = root.join("link");
        fs::write(&original, b"untouched").unwrap();
        symlink(&original, &symlink_path).unwrap();
        assert!(open_local_regular(&symlink_path).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn native_publish_refuses_existing_symlink_target() {
        use std::os::unix::fs::symlink;
        let root = std::env::temp_dir().join(format!(
            "ricefield-upload-symlink-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        let source = root.join("source");
        let sentinel = root.join("sentinel");
        let destination = root.join("destination");
        fs::write(&source, b"new content").unwrap();
        fs::write(&sentinel, b"must remain unchanged").unwrap();
        symlink(&sentinel, &destination).unwrap();
        let cancelled = AtomicBool::new(false);
        let bytes = Arc::new(AtomicU64::new(0));
        assert!(
            upload_to_uri(
                &source,
                &format!("file://{}", destination.display()),
                &cancelled,
                &bytes
            )
            .is_err()
        );
        assert!(
            fs::symlink_metadata(&destination)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read(&sentinel).unwrap(), b"must remain unchanged");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn competing_native_publications_publish_only_one_source() {
        let root = std::env::temp_dir().join(format!(
            "ricefield-upload-race-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        let source_a = root.join("a");
        let source_b = root.join("b");
        let destination = format!("file://{}", root.join("destination").display());
        fs::write(&source_a, b"AAAAAAAA").unwrap();
        fs::write(&source_b, b"BBBBBBBB").unwrap();
        let result = thread::scope(|scope| {
            let a = destination.clone();
            let b = destination.clone();
            let first_source = &source_a;
            let second_source = &source_b;
            let first = scope.spawn(move || {
                upload_to_uri(
                    first_source,
                    &a,
                    &AtomicBool::new(false),
                    &Arc::new(AtomicU64::new(0)),
                )
            });
            let second = scope.spawn(move || {
                upload_to_uri(
                    second_source,
                    &b,
                    &AtomicBool::new(false),
                    &Arc::new(AtomicU64::new(0)),
                )
            });
            (first.join().unwrap(), second.join().unwrap())
        });
        assert_ne!(
            result.0.is_ok(),
            result.1.is_ok(),
            "exactly one publisher must succeed: {result:?}"
        );
        let content = fs::read(root.join("destination")).unwrap();
        assert!(content == b"AAAAAAAA" || content == b"BBBBBBBB");
        assert_eq!(fs::read(source_a).unwrap(), b"AAAAAAAA");
        assert_eq!(fs::read(source_b).unwrap(), b"BBBBBBBB");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cancelled_upload_never_creates_destination() {
        let root = std::env::temp_dir().join(format!(
            "ricefield-upload-cancel-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        let source = root.join("source");
        let destination = root.join("destination");
        fs::write(&source, b"original").unwrap();
        let cancelled = AtomicBool::new(true);
        let bytes = Arc::new(AtomicU64::new(0));
        assert!(
            upload_to_uri(
                &source,
                &format!("file://{}", destination.display()),
                &cancelled,
                &bytes
            )
            .is_err()
        );
        assert!(!destination.exists());
        assert_eq!(fs::read(source).unwrap(), b"original");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn verification_refuses_truncated_or_modified_remote_stage() {
        let root = std::env::temp_dir().join(format!(
            "ricefield-upload-verify-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        let source = root.join("source");
        let stage = root.join(".ricefield-upload-stage");
        fs::write(&source, b"correct original bytes").unwrap();
        let bytes = Arc::new(AtomicU64::new(0));
        for damaged in [b"corrupt original bytes".as_slice(), b"short".as_slice()] {
            fs::write(&stage, damaged).unwrap();
            let result = verify_staging(
                &format!("file://{}", stage.display()),
                File::open(&source).unwrap(),
                &AtomicBool::new(false),
                &bytes,
                Instant::now() + Duration::from_secs(10),
            );
            assert!(result.is_err(), "modified stage must never verify");
        }
        assert_eq!(fs::read(source).unwrap(), b"correct original bytes");
        fs::remove_dir_all(root).unwrap();
    }
}
