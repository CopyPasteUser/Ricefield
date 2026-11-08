//! Explicit remote-to-local downloads. Remote GIO operations never receive
//! write access: data arrives through `gio cat` into a private local staging
//! directory, then is published with an atomic no-overwrite rename.
//! A crash may leave a hidden staging directory but never a partial file
//! under its final download name.

use crate::{filesystem, location::Location, volume};
use std::{
    collections::{HashSet, VecDeque},
    ffi::OsString,
    fs::{self, DirBuilder, File, OpenOptions},
    io::{self, Read, Write},
    os::unix::{
        ffi::OsStringExt,
        fs::{DirBuilderExt, MetadataExt},
    },
    path::{Component, Path, PathBuf},
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
const MAX_FOLDER_ENTRIES: usize = 4096;
const MAX_FOLDER_DEPTH: usize = 16;
const MAX_FOLDER_DIRECTORIES: usize = 512;
pub const MAX_WAITING_DOWNLOADS: usize = 16;
const MAX_STAGING_SCAN: usize = 8192;
const MAX_STAGING_RESULTS: usize = 64;

/// Read-only hints, not authenticated recovery records. Another process may
/// still be writing to a matching directory; never delete or replay these.
pub struct StagingInspection {
    pub directories: Vec<PathBuf>,
    pub truncated: bool,
}

/// Find privately owned download staging directories left after a possible
/// crash. This is intentionally opt-in and never examines their contents.
pub fn inspect_staging(directory: &Path) -> io::Result<StagingInspection> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(StagingInspection {
                directories: Vec::new(),
                truncated: false,
            });
        }
        Err(error) => return Err(error),
    };
    let mut directories = Vec::new();
    let mut truncated = false;
    for (index, entry) in entries.enumerate() {
        if index >= MAX_STAGING_SCAN {
            return Err(io::Error::other(
                "Download folder has too many entries to inspect safely",
            ));
        }
        let entry = entry?;
        let name = entry.file_name();
        let Some(suffix) = name
            .to_str()
            .and_then(|name| name.strip_prefix(".ricefield-download-"))
        else {
            continue;
        };
        let mut fields = suffix.split('-');
        if !matches!((fields.next(), fields.next(), fields.next(), fields.next()),
            (Some(pid), Some(nonce), Some(attempt), None)
            if !pid.is_empty() && pid.bytes().all(|byte| byte.is_ascii_digit())
                && !nonce.is_empty() && nonce.len() <= 40 && nonce.bytes().all(|byte| byte.is_ascii_digit())
                && !attempt.is_empty() && attempt.len() <= 2 && attempt.bytes().all(|byte| byte.is_ascii_digit()))
        {
            continue;
        }
        let metadata = match fs::symlink_metadata(entry.path()) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        if !metadata.is_dir()
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o077 != 0
        {
            continue;
        }
        if directories.len() < MAX_STAGING_RESULTS {
            directories.push(entry.path());
        } else {
            truncated = true;
        }
    }
    directories.sort();
    Ok(StagingInspection {
        directories,
        truncated,
    })
}

/// Only explicitly selected network downloads enter this in-memory FIFO.
/// Enqueued paths are snapshots, never reconstructed from later UI state.
/// Jobs are intentionally not replayed automatically after a process restart.
pub struct DownloadRequest {
    pub uri: String,
    pub encoded_name: String,
    pub directory: PathBuf,
    pub is_directory: bool,
}

#[derive(Default)]
pub struct DownloadQueue {
    pending: VecDeque<DownloadRequest>,
}

impl DownloadQueue {
    pub fn enqueue(&mut self, job: DownloadRequest) -> Result<usize, &'static str> {
        if self.pending.len() >= MAX_WAITING_DOWNLOADS {
            return Err("Network download queue is full (16 waiting jobs)");
        }
        if self
            .pending
            .iter()
            .any(|waiting| waiting.uri == job.uri && waiting.directory == job.directory)
        {
            return Err("This network entry is already waiting for download");
        }
        self.pending.push_back(job);
        Ok(self.pending.len())
    }

    pub fn dequeue(&mut self) -> Option<DownloadRequest> {
        self.pending.pop_front()
    }

    pub fn remove(&mut self, index: usize) -> bool {
        self.pending.remove(index).is_some()
    }

    pub fn len(&self) -> usize {
        self.pending.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &DownloadRequest> {
        self.pending.iter()
    }
}

/// A user-selected target must never silently resolve against the app's
/// working directory. The directory is created only after the user clicks
/// Download, and only the selected basename will be published there.
pub fn download_directory(input: &str) -> Result<PathBuf, &'static str> {
    if input.is_empty() || input.len() > 4096 || input.chars().any(char::is_control) {
        return Err("Choose a local folder path of at most 4096 characters");
    }
    let path = PathBuf::from(input);
    if !path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir))
    {
        return Err("Download folder must be an absolute path without '..'");
    }
    Ok(path)
}

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
    starting_bytes: u64,
    remaining_bytes: u64,
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
            if transferred > remaining_bytes {
                return Err("Download exceeds the 8 GiB operation safety limit".into());
            }
            output
                .write_all(&buffer[..count])
                .map_err(|error| format!("Cannot write download: {error}"))?;
            bytes.store(starting_bytes + transferred, Ordering::Relaxed);
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
    let (final_path, stage) = prepare_target(name, directory, cancelled)?;
    let staged_file = stage.0.join("partial");
    stream_to_staging(
        command,
        &staged_file,
        cancelled,
        bytes,
        DOWNLOAD_TIMEOUT,
        0,
        MAX_DOWNLOAD_BYTES,
    )?;
    publish(&staged_file, &final_path, directory, cancelled)?;
    Ok(final_path)
}

fn prepare_target(
    name: OsString,
    directory: &Path,
    cancelled: &AtomicBool,
) -> Result<(PathBuf, StagingDirectory), String> {
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
    Ok((final_path, stage))
}

fn publish(
    staged_file: &Path,
    final_path: &Path,
    directory: &Path,
    cancelled: &AtomicBool,
) -> Result<(), String> {
    publish_with_sync(staged_file, final_path, directory, cancelled, &mut |path| {
        File::open(path)?.sync_all()
    })
}

#[cfg(test)]
fn crash_at(checkpoint: &str) {
    if std::env::var("RICEFIELD_TEST_DOWNLOAD_CRASH_AT").as_deref() == Ok(checkpoint) {
        // SAFETY: only the isolated test child sets this checkpoint. The
        // parent verifies the surviving staging and published objects.
        unsafe { libc::raise(libc::SIGKILL) };
        unreachable!("SIGKILL cannot be ignored");
    }
}

/// The staging file has already been synced. Persist its directory entry
/// before publication, and both directories affected by the rename after it.
/// A failure after rename is explicitly ambiguous, never an invitation to
/// retry a download over its possibly committed destination.
fn publish_with_sync(
    staged_file: &Path,
    final_path: &Path,
    directory: &Path,
    cancelled: &AtomicBool,
    sync: &mut dyn FnMut(&Path) -> io::Result<()>,
) -> Result<(), String> {
    if cancelled.load(Ordering::Relaxed) {
        return Err("Network download cancelled; no final file was published".into());
    }
    let stage_parent = staged_file
        .parent()
        .ok_or("Download staging directory is missing")?;
    sync(stage_parent)
        .map_err(|error| format!("Cannot sync staged download before publication: {error}"))?;
    if cancelled.load(Ordering::Relaxed) {
        return Err("Network download cancelled; no final file was published".into());
    }
    #[cfg(test)]
    crash_at("before-rename");
    // renameat2(RENAME_NOREPLACE) is atomic on Linux, including a competing
    // file/symlink created after preflight; it also works on filesystems
    // without hard-link support.
    filesystem::rename_without_overwrite(staged_file, final_path).map_err(|error| {
        format!("Cannot publish download without overwriting an existing file: {error}")
    })?;
    #[cfg(test)]
    crash_at("after-rename");
    sync(stage_parent).map_err(|error| {
        format!(
            "Download may be published at {} but staging directory sync failed: {error}; inspect the destination before retrying",
            final_path.display()
        )
    })?;
    #[cfg(test)]
    crash_at("after-stage-sync");
    sync(directory).map_err(|error| {
        format!(
            "Download may be published at {} but destination directory sync failed: {error}; inspect the destination before retrying",
            final_path.display()
        )
    })?;
    #[cfg(test)]
    crash_at("after-destination-sync");
    Ok(())
}

/// Copy an entire read-only remote directory into a private local tree.
/// The tree becomes visible only after every entry finishes and its file data
/// and directory entries have been synced. Network metadata is not recreated:
/// this is a content download, not a lossless backup of server permissions.
pub fn download_folder(
    uri: &str,
    encoded_name: &str,
    directory: &Path,
    cancelled: &AtomicBool,
    bytes: &Arc<AtomicU64>,
) -> Result<PathBuf, String> {
    download_folder_with(
        uri,
        encoded_name,
        directory,
        cancelled,
        bytes,
        |folder, timeout| volume::list_remote_with_timeout(folder, true, cancelled, timeout),
        |source, target, base, remaining, timeout| {
            let mut command = Command::new("gio");
            command.args(["cat", source]);
            stream_to_staging(command, target, cancelled, bytes, timeout, base, remaining)?;
            Ok(bytes.load(Ordering::Relaxed) - base)
        },
    )
}

fn download_folder_with<L, F>(
    uri: &str,
    encoded_name: &str,
    directory: &Path,
    cancelled: &AtomicBool,
    bytes: &Arc<AtomicU64>,
    mut list: L,
    mut fetch: F,
) -> Result<PathBuf, String>
where
    L: FnMut(&str, Duration) -> Result<Vec<volume::RemoteEntry>, String>,
    F: FnMut(&str, &Path, u64, u64, Duration) -> Result<u64, String>,
{
    let name = checked_download_name(uri, encoded_name)?;
    let (final_path, stage) = prepare_target(name, directory, cancelled)?;
    let staged_root = stage.0.join("folder");
    fs::create_dir(&staged_root)
        .map_err(|error| format!("Cannot stage network directory: {error}"))?;
    let started = Instant::now();
    let deadline = started + DOWNLOAD_TIMEOUT;
    let mut visited = HashSet::new();
    let mut queue = VecDeque::from([(uri.to_owned(), staged_root.clone(), 0usize)]);
    let mut directories = vec![staged_root.clone()];
    let mut count = 0usize;
    let mut copied = 0u64;
    while let Some((remote, local, depth)) = queue.pop_front() {
        if cancelled.load(Ordering::Relaxed) {
            return Err("Network folder download cancelled; no final folder was published".into());
        }
        let remaining_time = deadline.saturating_duration_since(Instant::now());
        if remaining_time.is_zero() {
            return Err("Network folder download exceeded the 20-minute limit".into());
        }
        if !visited.insert(remote.clone()) {
            return Err("Remote folder traversal repeats a network URI".into());
        }
        let prefix = format!("{}/", remote.trim_end_matches('/'));
        for entry in list(&remote, remaining_time)? {
            if cancelled.load(Ordering::Relaxed) {
                return Err("Network folder download cancelled".into());
            }
            count += 1;
            if count > MAX_FOLDER_ENTRIES {
                return Err("Network folder download exceeds 4,096 entries".into());
            }
            if entry.uri.strip_prefix(&prefix) != Some(entry.name.as_str()) {
                return Err("Network listing contained an unrelated or nested URI".into());
            }
            if !entry.is_directory && !entry.is_regular_file {
                return Err(
                    "Network folder contains a symlink or unsupported file type; refusing to follow it"
                        .into(),
                );
            }
            let basename = checked_download_name(&entry.uri, &entry.name)?;
            let destination = local.join(basename);
            if entry.is_directory {
                if depth >= MAX_FOLDER_DEPTH || directories.len() >= MAX_FOLDER_DIRECTORIES {
                    return Err("Network folder depth/directory safety limit exceeded".into());
                }
                fs::create_dir(&destination)
                    .map_err(|error| format!("Cannot create staged folder: {error}"))?;
                directories.push(destination.clone());
                queue.push_back((entry.uri, destination, depth + 1));
            } else {
                let remaining_bytes = MAX_DOWNLOAD_BYTES - copied;
                let remaining_time = deadline.saturating_duration_since(Instant::now());
                if remaining_bytes == 0 || remaining_time.is_zero() {
                    return Err("Network folder download reached its size or time limit".into());
                }
                let size = fetch(
                    &entry.uri,
                    &destination,
                    copied,
                    remaining_bytes,
                    remaining_time,
                )?;
                if size > remaining_bytes {
                    return Err("Network download exceeded the cumulative 8 GiB limit".into());
                }
                copied += size;
                bytes.store(copied, Ordering::Relaxed);
            }
        }
    }
    if cancelled.load(Ordering::Relaxed) {
        return Err("Network folder download cancelled".into());
    }
    for folder in directories.iter().rev() {
        File::open(folder)
            .and_then(|opened| opened.sync_all())
            .map_err(|error| format!("Cannot sync staged network folder: {error}"))?;
    }
    publish(&staged_root, &final_path, directory, cancelled)?;
    Ok(final_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;

    fn request(id: usize) -> DownloadRequest {
        DownloadRequest {
            uri: format!("smb://nas/share/file-{id}.txt"),
            encoded_name: format!("file-{id}.txt"),
            directory: PathBuf::from("/tmp/ricefield-test-downloads"),
            is_directory: false,
        }
    }

    #[test]
    fn network_download_queue_is_bounded_fifo_and_removable() {
        let mut queue = DownloadQueue::default();
        for id in 0..MAX_WAITING_DOWNLOADS {
            assert_eq!(queue.enqueue(request(id)).unwrap(), id + 1);
        }
        assert!(queue.enqueue(request(MAX_WAITING_DOWNLOADS)).is_err());
        assert!(queue.enqueue(request(0)).is_err());
        assert!(queue.remove(1));
        assert_eq!(queue.len(), MAX_WAITING_DOWNLOADS - 1);
        assert_eq!(queue.dequeue().unwrap().encoded_name, "file-0.txt");
        assert_eq!(queue.dequeue().unwrap().encoded_name, "file-2.txt");
        assert_eq!(
            queue.enqueue(request(50)).unwrap(),
            MAX_WAITING_DOWNLOADS - 2
        );
        assert!(!queue.is_empty());
        assert!(!queue.remove(100));
        // Pending jobs are deliberately session-only and never reloaded.
        assert!(DownloadQueue::default().is_empty());
    }

    #[test]
    fn network_queue_duplicate_uses_original_target_snapshot() {
        let mut queue = DownloadQueue::default();
        assert_eq!(queue.enqueue(request(5)).unwrap(), 1);
        assert!(queue.enqueue(request(5)).is_err());
        let mut separate_target = request(5);
        separate_target.directory = PathBuf::from("/tmp/another-local-folder");
        assert_eq!(queue.enqueue(separate_target).unwrap(), 2);
    }

    #[test]
    fn staging_inspection_finds_only_private_owned_directories_without_mutation() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let target = private_fixture("staging-inspect");
        let abandoned = StagingDirectory::create(&target).unwrap();
        let expected = abandoned.0.clone();
        fs::write(expected.join("partial"), b"retain after crash").unwrap();
        std::mem::forget(abandoned); // Simulate a process killed before Drop.

        let public = target.join(".ricefield-download-2-3-4");
        fs::create_dir(&public).unwrap();
        fs::set_permissions(&public, fs::Permissions::from_mode(0o755)).unwrap();
        symlink(&expected, target.join(".ricefield-download-5-6-7")).unwrap();
        fs::write(target.join(".ricefield-download-8-9-1"), b"not a directory").unwrap();
        fs::create_dir(target.join(".ricefield-download-malice")).unwrap();

        let result = inspect_staging(&target).unwrap();
        assert_eq!(result.directories, vec![expected.clone()]);
        assert!(!result.truncated);
        assert_eq!(
            fs::read(expected.join("partial")).unwrap(),
            b"retain after crash"
        );
        let missing = inspect_staging(&target.join("not-created")).unwrap();
        assert!(missing.directories.is_empty());
        fs::remove_dir_all(target).unwrap();
    }

    fn private_fixture(label: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory =
            std::env::temp_dir().join(format!("ricefield-{label}-{}-{unique}", std::process::id()));
        fs::create_dir(&directory).unwrap();
        directory
    }

    fn crash_child_if_requested() {
        let Some(folder) = std::env::var_os("RICEFIELD_TEST_DOWNLOAD_CRASH_ROOT") else {
            return;
        };
        let folder = PathBuf::from(folder);
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "printf 'remote snapshot kept across crash'"]);
        let outcome = download_with_command(
            OsString::from("received.txt"),
            &folder,
            &AtomicBool::new(false),
            &Arc::new(AtomicU64::new(0)),
            cmd,
        );
        panic!("Download process-crash checkpoint not reached: {outcome:?}");
    }

    fn crash_fixture(test_name: &str, checkpoint: &str) -> PathBuf {
        let folder = private_fixture("download-process-crash");
        let mut child = Command::new(std::env::current_exe().unwrap())
            .arg(test_name)
            .arg("--exact")
            .env("RICEFIELD_TEST_DOWNLOAD_CRASH_ROOT", &folder)
            .env("RICEFIELD_TEST_DOWNLOAD_CRASH_AT", checkpoint)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("Download did not reach process-crash checkpoint {checkpoint}");
            }
            thread::sleep(Duration::from_millis(20));
        };
        assert_eq!(
            status.signal(),
            Some(libc::SIGKILL),
            "{checkpoint}: {status}"
        );
        folder
    }

    #[test]
    fn download_crash_before_publication_keeps_private_staging() {
        crash_child_if_requested();
        let folder = crash_fixture(
            "remote_download::tests::download_crash_before_publication_keeps_private_staging",
            "before-rename",
        );
        assert!(!folder.join("received.txt").exists());
        let stages = inspect_staging(&folder).unwrap();
        assert!(!stages.truncated);
        assert_eq!(stages.directories.len(), 1);
        assert_eq!(
            fs::read(stages.directories[0].join("partial")).unwrap(),
            b"remote snapshot kept across crash"
        );
        fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn download_crash_after_rename_never_exposes_partial_target() {
        crash_child_if_requested();
        for checkpoint in ["after-rename", "after-stage-sync", "after-destination-sync"] {
            let folder = crash_fixture(
                "remote_download::tests::download_crash_after_rename_never_exposes_partial_target",
                checkpoint,
            );
            assert_eq!(
                fs::read(folder.join("received.txt")).unwrap(),
                b"remote snapshot kept across crash",
                "{checkpoint}"
            );
            // A concurrent/restarted download must refuse this committed
            // filename even if the previous process never returned success.
            let mut cmd = Command::new("sh");
            cmd.args(["-c", "printf 'competing bytes'"]);
            assert!(
                download_with_command(
                    OsString::from("received.txt"),
                    &folder,
                    &AtomicBool::new(false),
                    &Arc::new(AtomicU64::new(0)),
                    cmd,
                )
                .is_err()
            );
            assert_eq!(
                fs::read(folder.join("received.txt")).unwrap(),
                b"remote snapshot kept across crash"
            );
            fs::remove_dir_all(folder).unwrap();
        }
    }

    #[test]
    fn download_two_directory_sync_errors_distinguish_unpublished_and_ambiguous() {
        let folder = private_fixture("download-sync-errors");
        let stage = StagingDirectory::create(&folder).unwrap();
        let source = stage.0.join("partial");
        fs::write(&source, b"full download").unwrap();
        File::open(&source).unwrap().sync_all().unwrap();
        let destination = folder.join("received.txt");
        let mut calls = 0;
        let error = publish_with_sync(
            &source,
            &destination,
            &folder,
            &AtomicBool::new(false),
            &mut |_| {
                calls += 1;
                Err(io::Error::other("injected prepublication sync failure"))
            },
        )
        .unwrap_err();
        assert!(error.contains("before publication"), "{error}");
        assert_eq!(calls, 1);
        assert!(!destination.exists());
        assert_eq!(fs::read(&source).unwrap(), b"full download");

        let mut calls = 0;
        let error = publish_with_sync(
            &source,
            &destination,
            &folder,
            &AtomicBool::new(false),
            &mut |directory| {
                calls += 1;
                if calls == 2 {
                    Err(io::Error::other("injected stage removal sync failure"))
                } else {
                    File::open(directory)?.sync_all()
                }
            },
        )
        .unwrap_err();
        assert!(error.contains("staging directory sync failed"), "{error}");
        assert!(error.contains("inspect the destination"));
        assert_eq!(calls, 2);
        assert_eq!(fs::read(&destination).unwrap(), b"full download");
        assert!(!source.exists());
        fs::remove_dir_all(folder).unwrap();
    }

    fn remote_entry(uri: &str, name: &str, is_directory: bool) -> volume::RemoteEntry {
        volume::RemoteEntry {
            uri: uri.into(),
            name: name.into(),
            is_directory,
            is_regular_file: !is_directory,
            reported_size: None,
        }
    }

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
    fn download_directory_rejects_relative_or_parent_traversal() {
        for invalid in ["", "Downloads", "~/Downloads", "/tmp/../etc", "/tmp/\nnew"] {
            assert!(download_directory(invalid).is_err(), "accepted {invalid:?}");
        }
        assert_eq!(
            download_directory("/home/user/My Downloads").unwrap(),
            PathBuf::from("/home/user/My Downloads")
        );
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

    #[test]
    fn remote_folder_download_preserves_tree_and_publishes_once() {
        let target = private_fixture("folder-success");
        let root = "smb://nas/share/My%20Album";
        let sub = format!("{root}/Disc%201");
        let progress = Arc::new(AtomicU64::new(0));
        let cancelled = AtomicBool::new(false);
        let result = download_folder_with(
            root,
            "My%20Album",
            &target,
            &cancelled,
            &progress,
            |uri, _| match uri {
                value if value == root => Ok(vec![
                    remote_entry(&sub, "Disc%201", true),
                    remote_entry(&format!("{root}/cover.txt"), "cover.txt", false),
                ]),
                value if value == sub => Ok(vec![remote_entry(
                    &format!("{sub}/Track%20One.txt"),
                    "Track%20One.txt",
                    false,
                )]),
                _ => Err("Unexpected remote folder".into()),
            },
            |uri, path, base, remaining, _| {
                let contents: &[u8] = if uri.ends_with("cover.txt") {
                    b"cover"
                } else {
                    b"track one"
                };
                assert!(contents.len() as u64 <= remaining);
                fs::write(path, contents).unwrap();
                progress.store(base + contents.len() as u64, Ordering::Relaxed);
                Ok(contents.len() as u64)
            },
        )
        .unwrap();
        assert_eq!(result, target.join("My Album"));
        assert_eq!(fs::read(result.join("cover.txt")).unwrap(), b"cover");
        assert_eq!(
            fs::read(result.join("Disc 1/Track One.txt")).unwrap(),
            b"track one"
        );
        assert_eq!(progress.load(Ordering::Relaxed), 14);
        assert_eq!(fs::read_dir(&target).unwrap().count(), 1);
        fs::remove_dir_all(target).unwrap();
    }

    #[test]
    fn remote_folder_failure_does_not_publish_partial_tree() {
        let target = private_fixture("folder-failure");
        let root = "sftp://host/shared/Album";
        let cancelled = AtomicBool::new(false);
        let bytes = Arc::new(AtomicU64::new(0));
        let result = download_folder_with(
            root,
            "Album",
            &target,
            &cancelled,
            &bytes,
            |_, _| {
                Ok(vec![
                    remote_entry(&format!("{root}/first"), "first", false),
                    remote_entry(&format!("{root}/second"), "second", false),
                ])
            },
            |uri, path, base, _, _| {
                fs::write(path, b"partial").unwrap();
                if uri.ends_with("second") {
                    return Err("Network disconnected".into());
                }
                bytes.store(base + 7, Ordering::Relaxed);
                Ok(7)
            },
        );
        assert!(result.unwrap_err().contains("Network disconnected"));
        assert!(fs::read_dir(&target).unwrap().next().is_none());

        let traversal = download_folder_with(
            root,
            "Album",
            &target,
            &cancelled,
            &bytes,
            |_, _| {
                Ok(vec![remote_entry(
                    &format!("{root}/..%2Fescape"),
                    "..%2Fescape",
                    false,
                )])
            },
            |_, _, _, _, _| panic!("Unsafe filename must fail before download"),
        );
        assert!(traversal.is_err());
        assert!(fs::read_dir(&target).unwrap().next().is_none());
        fs::remove_dir_all(target).unwrap();
    }

    #[test]
    fn remote_folder_rejects_symlinks_before_following_them() {
        let target = private_fixture("folder-symlink");
        let root = "sftp://server/share/Backup";
        let cancelled = AtomicBool::new(false);
        let bytes = Arc::new(AtomicU64::new(0));
        let result = download_folder_with(
            root,
            "Backup",
            &target,
            &cancelled,
            &bytes,
            |_, _| {
                Ok(vec![volume::RemoteEntry {
                    uri: format!("{root}/link"),
                    name: "link".into(),
                    is_directory: false,
                    is_regular_file: false,
                    reported_size: None,
                }])
            },
            |_, _, _, _, _| panic!("Must not follow remote symlinks or special files"),
        );
        assert!(result.unwrap_err().contains("unsupported file type"));
        assert_eq!(bytes.load(Ordering::Relaxed), 0);
        assert!(fs::read_dir(&target).unwrap().next().is_none());
        fs::remove_dir_all(target).unwrap();
    }

    #[test]
    fn remote_folder_cancellation_and_collision_leave_target_untouched() {
        let target = private_fixture("folder-cancel");
        let root = "smb://nas/share/Photos";
        let cancelled = AtomicBool::new(false);
        let bytes = Arc::new(AtomicU64::new(0));
        let result = download_folder_with(
            root,
            "Photos",
            &target,
            &cancelled,
            &bytes,
            |_, _| {
                Ok(vec![remote_entry(
                    &format!("{root}/first.jpg"),
                    "first.jpg",
                    false,
                )])
            },
            |_, path, _, _, _| {
                fs::write(path, b"incomplete").unwrap();
                cancelled.store(true, Ordering::Relaxed);
                Ok(10)
            },
        );
        assert!(result.unwrap_err().contains("cancelled"));
        assert!(fs::read_dir(&target).unwrap().next().is_none());

        cancelled.store(false, Ordering::Relaxed);
        let existing = target.join("Photos");
        fs::create_dir(&existing).unwrap();
        fs::write(existing.join("keep.txt"), b"owner data").unwrap();
        let collision = download_folder_with(
            root,
            "Photos",
            &target,
            &cancelled,
            &bytes,
            |_, _| panic!("Must not enumerate on a destination collision"),
            |_, _, _, _, _| panic!("Must not fetch on a destination collision"),
        );
        assert!(collision.unwrap_err().contains("already exists"));
        assert_eq!(fs::read(existing.join("keep.txt")).unwrap(), b"owner data");
        fs::remove_dir_all(target).unwrap();
    }

    #[test]
    fn streaming_rejects_per_operation_byte_overflow() {
        let target = private_fixture("byte-overflow");
        let bytes = Arc::new(AtomicU64::new(0));
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "printf 'more than four'"]);
        assert!(
            stream_to_staging(
                cmd,
                &target.join("partial"),
                &AtomicBool::new(false),
                &bytes,
                Duration::from_secs(5),
                2,
                4,
            )
            .is_err()
        );
        fs::remove_dir_all(target).unwrap();
    }
}
