//! Explicit remote-to-local downloads. Remote GIO operations never receive
//! write access: data arrives through `gio cat` into a private local staging
//! directory, then is published with an atomic no-overwrite rename.
//! A crash may leave a hidden staging directory but never a partial file
//! under its final download name.

use crate::{filesystem, location::Location};
use std::{
    ffi::OsString,
    fs::{self, DirBuilder, File, OpenOptions},
    io::{Read, Write},
    os::unix::{ffi::OsStringExt, fs::DirBuilderExt},
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const MAX_DOWNLOAD_BYTES: u64 = 8 * 1024 * 1024 * 1024;
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(20 * 60);

fn file_name(encoded: &str) -> Result<OsString, String> {
    let input = encoded.as_bytes();
    let mut decoded = Vec::with_capacity(input.len());
    let mut index = 0;
    while index < input.len() {
        if input[index] == b'%' {
            let hex = input
                .get(index + 1..index + 3)
                .ok_or("Incomplete percent-encoded network filename")?;
            let digit = |byte: u8| match byte {
                b'0'..=b'9' => Some(byte - b'0'),
                b'a'..=b'f' => Some(byte - b'a' + 10),
                b'A'..=b'F' => Some(byte - b'A' + 10),
                _ => None,
            };
            decoded.push(
                (digit(hex[0]).ok_or("Invalid encoded filename")? << 4)
                    | digit(hex[1]).ok_or("Invalid encoded filename")?,
            );
            index += 3;
        } else {
            decoded.push(input[index]);
            index += 1;
        }
    }
    if decoded.is_empty()
        || decoded.len() > 255
        || decoded == b"."
        || decoded == b".."
        || decoded
            .iter()
            .any(|byte| matches!(*byte, b'/' | b'\\' | 0..=31 | 127))
    {
        return Err("Unsafe network filename; download was not started".into());
    }
    Ok(OsString::from_vec(decoded))
}

fn checked_download_name(uri: &str, encoded_name: &str) -> Result<OsString, String> {
    if !matches!(Location::parse(uri.into()), Ok(Location::Remote(_)))
        || uri.rsplit('/').next() != Some(encoded_name)
    {
        return Err("Download needs a validated remote file URI".into());
    }
    file_name(encoded_name)
}

struct StagingDirectory(PathBuf);

impl StagingDirectory {
    fn create(destination: &Path) -> Result<Self, String> {
        for attempt in 0..32 {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|error| error.to_string())?
                .as_nanos();
            let path = destination.join(format!(
                ".ricefield-download-{}-{nonce}-{attempt}",
                std::process::id()
            ));
            match DirBuilder::new().mode(0o700).create(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    return Err(format!("Cannot create private download staging: {error}"));
                }
            }
        }
        Err("Cannot allocate unique download staging directory".into())
    }
}

impl Drop for StagingDirectory {
    fn drop(&mut self) {
        // Only our exclusively created directory is cleaned up. An abrupt
        // process exit leaves the incomplete data there for manual inspection.
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn stream_to_staging(
    mut command: Command,
    staging_file: &Path,
    cancelled: &AtomicBool,
    bytes: &Arc<AtomicU64>,
    timeout: Duration,
) -> Result<(), String> {
    let destination = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(staging_file)
        .map_err(|error| format!("Cannot prepare download: {error}"))?;
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("Cannot start network download: {error}"))?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or("Network download output is unavailable")?;
    let bytes = Arc::clone(bytes);
    let reader = thread::spawn(move || -> Result<(), String> {
        let mut output = destination;
        let mut buffer = [0; 128 * 1024];
        let mut transferred = 0u64;
        loop {
            let count = stdout
                .read(&mut buffer)
                .map_err(|error| format!("Cannot read network data: {error}"))?;
            if count == 0 {
                break;
            }
            transferred += count as u64;
            if transferred > MAX_DOWNLOAD_BYTES {
                return Err("Download exceeds the 8 GiB safety limit".into());
            }
            output
                .write_all(&buffer[..count])
                .map_err(|error| format!("Cannot write download: {error}"))?;
            bytes.store(transferred, Ordering::Relaxed);
        }
        output
            .sync_all()
            .map_err(|error| format!("Cannot sync downloaded file: {error}"))
    });

    let deadline = Instant::now() + timeout;
    let mut reader = Some(reader);
    let mut read_result = None;
    let mut exit_status: Option<ExitStatus> = None;
    loop {
        if let Some(handle) = reader.as_ref()
            && handle.is_finished()
        {
            read_result = Some(match reader.take().expect("reader exists").join() {
                Ok(result) => result,
                Err(_) => Err("Network download reader panicked".to_owned()),
            });
            if read_result.as_ref().is_some_and(Result::is_err) {
                let _ = child.kill();
                let _ = child.wait();
                return read_result.expect("reader finished");
            }
        }
        if exit_status.is_none() {
            exit_status = match child.try_wait() {
                Ok(status) => status,
                Err(error) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    if let Some(handle) = reader.take() {
                        let _ = handle.join();
                    }
                    return Err(format!("Cannot monitor network download: {error}"));
                }
            };
        }
        if exit_status.is_some() && read_result.is_some() {
            break;
        }
        if cancelled.load(Ordering::Relaxed) || Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            if let Some(handle) = reader.take() {
                let _ = handle.join();
            }
            return Err(if cancelled.load(Ordering::Relaxed) {
                "Network download cancelled; no final file was published".into()
            } else {
                "Network download timed out; no final file was published".into()
            });
        }
        thread::sleep(Duration::from_millis(50));
    }
    read_result.expect("reader finished")?;
    if !exit_status.expect("child exited").success() {
        return Err("Network download failed; check connection and credentials".into());
    }
    if cancelled.load(Ordering::Relaxed) {
        return Err("Network download cancelled; no final file was published".into());
    }
    Ok(())
}

pub fn download(
    uri: &str,
    encoded_name: &str,
    directory: &Path,
    cancelled: &AtomicBool,
    bytes: &Arc<AtomicU64>,
) -> Result<PathBuf, String> {
    let name = checked_download_name(uri, encoded_name)?;
    let mut command = Command::new("gio");
    command.args(["cat", uri]);
    download_with_command(name, directory, cancelled, bytes, command)
}

fn download_with_command(
    name: OsString,
    directory: &Path,
    cancelled: &AtomicBool,
    bytes: &Arc<AtomicU64>,
    command: Command,
) -> Result<PathBuf, String> {
    if cancelled.load(Ordering::Relaxed) {
        return Err("Network download cancelled".into());
    }
    fs::create_dir_all(directory)
        .map_err(|error| format!("Cannot create download directory: {error}"))?;
    if !fs::metadata(directory).is_ok_and(|metadata| metadata.is_dir()) {
        return Err("Download destination is not a directory".into());
    }
    let final_path = directory.join(name);
    match fs::symlink_metadata(&final_path) {
        Ok(_) => {
            return Err("A file with this name already exists; nothing was overwritten".into());
        }
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            return Err(format!("Cannot inspect download destination: {error}"));
        }
        Err(_) => {}
    }
    let stage = StagingDirectory::create(directory)?;
    let staged_file = stage.0.join("partial");
    stream_to_staging(command, &staged_file, cancelled, bytes, DOWNLOAD_TIMEOUT)?;
    if cancelled.load(Ordering::Relaxed) {
        return Err("Network download cancelled; no final file was published".into());
    }
    // renameat2(RENAME_NOREPLACE) is atomic on Linux, including a competing
    // file/symlink created after preflight; it also works on filesystems
    // without hard-link support.
    filesystem::rename_without_overwrite(&staged_file, &final_path).map_err(|error| {
        format!("Cannot publish download without overwriting an existing file: {error}")
    })?;
    File::open(directory)
        .and_then(|folder| folder.sync_all())
        .map_err(|error| {
            format!(
                "Download is present at {} but directory sync failed: {error}",
                final_path.display()
            )
        })?;
    Ok(final_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_traversal_and_encoded_separators() {
        for name in [
            "", ".", "..", "%2E%2E", "%2fetc", "a%2Fb", "a%5cb", "x%00", "x%0a", "x%7f", "%ZZ",
        ] {
            assert!(file_name(name).is_err(), "accepted {name}");
        }
        assert_eq!(
            file_name("My%20Photo%C3%A4.jpg").unwrap(),
            OsString::from("My Photoä.jpg")
        );
        assert!(checked_download_name("smb://nas/share/one", "two").is_err());
        assert!(checked_download_name("file:///etc/passwd", "passwd").is_err());
        assert!(checked_download_name("smb://nas/share/a%2Fb", "a%2Fb").is_err());
    }

    #[test]
    fn streaming_captures_bytes_and_avoids_partial_final_files() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let folder = std::env::temp_dir().join(format!(
            "ricefield-remote-download-test-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir(&folder).unwrap();
        let cancelled = AtomicBool::new(false);
        let progress = Arc::new(AtomicU64::new(0));
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "printf 'network bytes'"]);
        let final_path =
            download_with_command("new.txt".into(), &folder, &cancelled, &progress, cmd).unwrap();
        assert_eq!(progress.load(Ordering::Relaxed), 13);
        assert_eq!(fs::read(&final_path).unwrap(), b"network bytes");
        assert!(fs::read_dir(&folder).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".ricefield-download-")
        }));
        let existing = folder.join("existing");
        fs::write(&existing, "keep").unwrap();
        let mut competing = Command::new("sh");
        competing.args(["-c", "printf changed"]);
        assert!(
            download_with_command("existing".into(), &folder, &cancelled, &progress, competing)
                .is_err()
        );
        assert_eq!(fs::read(&existing).unwrap(), b"keep");
        // A competing file created after the destination preflight must not
        // be replaced by the staged remote content.
        let raced = folder.join("raced");
        let mut race = Command::new("sh");
        race.args([
            "-c",
            "printf intruder > \"$RICEFIELD_TEST_RACE\"; printf remote",
        ])
        .env("RICEFIELD_TEST_RACE", &raced);
        assert!(
            download_with_command("raced".into(), &folder, &cancelled, &progress, race).is_err()
        );
        assert_eq!(fs::read(&raced).unwrap(), b"intruder");
        fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn subprocess_failure_and_pre_cancel_do_not_publish() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let folder = std::env::temp_dir().join(format!(
            "ricefield-download-cancel-test-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir(&folder).unwrap();
        let progress = Arc::new(AtomicU64::new(0));
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "printf partial; exit 12"]);
        assert!(
            download_with_command(
                "complete".into(),
                &folder,
                &AtomicBool::new(false),
                &progress,
                cmd
            )
            .is_err()
        );
        assert!(!folder.join("complete").exists());
        assert!(fs::read_dir(&folder).unwrap().next().is_none());
        assert!(
            download(
                "smb://nas/share/file",
                "file",
                &folder,
                &AtomicBool::new(true),
                &progress
            )
            .is_err()
        );
        assert!(!folder.join("file").exists());
        fs::remove_dir_all(folder).unwrap();
    }
}
