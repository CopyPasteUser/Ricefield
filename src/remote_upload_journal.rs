//! Durable, private evidence for uncertain WebDAV, SFTP and SMB publications.
//!
//! Create and sync a record *before* any remote staging PUT/mkdir. Records never
//! contain credentials and never authorize remote deletion or upload replay.

use serde::{Deserialize, Serialize};
use std::{
    env,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    os::{
        fd::AsRawFd,
        unix::fs::{MetadataExt, OpenOptionsExt},
    },
    path::{Component, Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

const VERSION: u32 = 1;
const MAX_RECORDS: usize = 256;
const MAX_RECORD_BYTES: u64 = 8 * 1024;
const MAX_URI_BYTES: usize = 4096;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Record {
    pub id: String,
    pub stage_uri: String,
    pub destination_uri: String,
    pub created_unix_seconds: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredRecord {
    version: u32,
    id: String,
    stage_uri: String,
    destination_uri: String,
    created_unix_seconds: u64,
}

/// Evidence for a server capability test that may have left private probe
/// objects behind. It never authorizes retrying or deleting those objects.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProbeRecord {
    pub id: String,
    pub prefix_uri: String,
    pub created_unix_seconds: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredProbeRecord {
    version: u32,
    id: String,
    prefix_uri: String,
    created_unix_seconds: u64,
}

impl ProbeRecord {
    fn valid(&self) -> bool {
        let Some((parent, suffix)) = self.prefix_uri.rsplit_once("/.ricefield-safety-") else {
            return false;
        };
        self.id.len() == 32
            && self.id.bytes().all(|byte| byte.is_ascii_hexdigit())
            && suffix == self.id
            && (parent.starts_with("http://")
                || parent.starts_with("https://")
                || parent.starts_with("sftp://")
                || parent.starts_with("smb://"))
            && parent
                .split_once("://")
                .and_then(|(_, rest)| rest.split('/').next())
                .is_some_and(|authority| !authority.is_empty() && !authority.contains('@'))
            && self.prefix_uri.len() <= MAX_URI_BYTES
            && !self.prefix_uri.chars().any(char::is_control)
            && self.created_unix_seconds > 0
            && self.created_unix_seconds <= 32_503_680_000
    }
}

impl Record {
    fn valid(&self) -> bool {
        let Some((parent, suffix)) = self.stage_uri.rsplit_once("/.ricefield-upload-") else {
            return false;
        };
        let Some((destination_parent, destination_name)) = self.destination_uri.rsplit_once('/')
        else {
            return false;
        };
        self.id.len() == 32
            && self.id.bytes().all(|byte| byte.is_ascii_hexdigit())
            && suffix == self.id
            && parent == destination_parent
            && (parent.starts_with("http://")
                || parent.starts_with("https://")
                || parent.starts_with("sftp://")
                || parent.starts_with("smb://"))
            && !parent
                .split_once("://")
                .and_then(|(_, rest)| rest.split('/').next())
                .is_some_and(|authority| authority.is_empty() || authority.contains('@'))
            && !destination_name.is_empty()
            && self.stage_uri.len() <= MAX_URI_BYTES
            && self.destination_uri.len() <= MAX_URI_BYTES
            && !self
                .stage_uri
                .chars()
                .chain(self.destination_uri.chars())
                .any(char::is_control)
            && self.created_unix_seconds > 0
            && self.created_unix_seconds <= 32_503_680_000
    }
}

pub struct PendingUpload {
    record: Record,
    directory: PathBuf,
}

impl PendingUpload {
    /// Returns only after the new journal has been durably published. A
    /// failure here must prevent the caller from creating a remote stage.
    pub fn begin(stage_uri: &str, destination_uri: &str) -> io::Result<Self> {
        Self::begin_in(&journal_directory()?, stage_uri, destination_uri)
    }

    pub(crate) fn begin_in(
        directory: &Path,
        stage_uri: &str,
        destination_uri: &str,
    ) -> io::Result<Self> {
        let id = stage_uri
            .rsplit_once("/.ricefield-upload-")
            .map(|(_, suffix)| suffix.to_owned())
            .ok_or_else(|| invalid_data("Invalid WebDAV staging URI"))?;
        let record = Record {
            id,
            stage_uri: stage_uri.to_owned(),
            destination_uri: destination_uri.to_owned(),
            created_unix_seconds: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        };
        if !record.valid() {
            return Err(invalid_data("Invalid WebDAV journal fields"));
        }
        prepare_private_directory(directory)?;
        let _lock = lock_directory(directory)?;
        let existing = scan_in(directory)?;
        if existing.len() + scan_probes_in(directory)?.len() >= MAX_RECORDS {
            return Err(io::Error::other(
                "Too many unfinished WebDAV uploads; inspect saved journal evidence",
            ));
        }
        let path = record_path(directory, &record.id)?;
        let temporary = directory.join(format!(".{}.tmp", record.id));
        let stored = StoredRecord {
            version: VERSION,
            id: record.id.clone(),
            stage_uri: record.stage_uri.clone(),
            destination_uri: record.destination_uri.clone(),
            created_unix_seconds: record.created_unix_seconds,
        };
        let bytes = toml::to_string(&stored).map_err(io::Error::other)?;
        if bytes.len() as u64 > MAX_RECORD_BYTES {
            return Err(invalid_data("WebDAV journal record exceeds size limit"));
        }
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&temporary)?;
        let result = (|| {
            file.write_all(bytes.as_bytes())?;
            file.sync_all()?;
            // Atomic, no-overwrite link of the fully synced record. If killed
            // before this step no remote write has happened.
            fs::hard_link(&temporary, &path)?;
            File::open(directory)?.sync_all()
        })();
        if let Err(error) = fs::remove_file(&temporary) {
            eprintln!("RiceField: WebDAV journal temporary cleanup failed: {error}");
        }
        result?;
        Ok(Self {
            record,
            directory: directory.to_path_buf(),
        })
    }

    /// A successfully acknowledged remote MOVE retires only our local
    /// evidence. A failed retirement is logged but must not retry the MOVE.
    pub fn finish(self) {
        if let Err(error) = retire_in(&self.directory, &self.record) {
            eprintln!("RiceField: remote upload published but journal retirement failed: {error}");
        }
    }
}

pub struct PendingProbe {
    record: ProbeRecord,
    directory: PathBuf,
}

impl PendingProbe {
    /// Persist the complete probe prefix before the first remote PUT. A crash
    /// can then leave probe objects, but never untraceable probe objects.
    pub fn begin(prefix_uri: &str) -> io::Result<Self> {
        Self::begin_in(&journal_directory()?, prefix_uri)
    }

    pub(crate) fn begin_in(directory: &Path, prefix_uri: &str) -> io::Result<Self> {
        let id = prefix_uri
            .rsplit_once("/.ricefield-safety-")
            .map(|(_, suffix)| suffix.to_owned())
            .ok_or_else(|| invalid_data("Invalid WebDAV safety-probe URI"))?;
        let record = ProbeRecord {
            id,
            prefix_uri: prefix_uri.to_owned(),
            created_unix_seconds: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        };
        if !record.valid() {
            return Err(invalid_data("Invalid WebDAV safety-probe fields"));
        }
        prepare_private_directory(directory)?;
        let _lock = lock_directory(directory)?;
        if scan_in(directory)?.len() + scan_probes_in(directory)?.len() >= MAX_RECORDS {
            return Err(io::Error::other(
                "Too many unfinished WebDAV operations; inspect saved journal evidence",
            ));
        }
        let path = probe_path(directory, &record.id)?;
        let temporary = directory.join(format!(".{}.probe.tmp", record.id));
        let stored = StoredProbeRecord {
            version: VERSION,
            id: record.id.clone(),
            prefix_uri: record.prefix_uri.clone(),
            created_unix_seconds: record.created_unix_seconds,
        };
        let bytes = toml::to_string(&stored).map_err(io::Error::other)?;
        if bytes.len() as u64 > MAX_RECORD_BYTES {
            return Err(invalid_data(
                "WebDAV safety-probe record exceeds size limit",
            ));
        }
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&temporary)?;
        let result = (|| {
            file.write_all(bytes.as_bytes())?;
            file.sync_all()?;
            fs::hard_link(&temporary, &path)?;
            File::open(directory)?.sync_all()
        })();
        if let Err(error) = fs::remove_file(&temporary) {
            eprintln!("RiceField: WebDAV probe journal temporary cleanup failed: {error}");
        }
        result?;
        Ok(Self {
            record,
            directory: directory.to_path_buf(),
        })
    }

    /// Only a completed and cleaned-up capability test may retire evidence.
    pub fn finish(self) {
        if let Err(error) = retire_probe_in(&self.directory, &self.record) {
            eprintln!("RiceField: remote probe finished but journal retirement failed: {error}");
        }
    }
}

pub fn inspect_incomplete() -> io::Result<Vec<Record>> {
    let directory = journal_directory()?;
    match fs::symlink_metadata(&directory) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error),
        Ok(_) => scan_in(&directory),
    }
}

pub fn inspect_incomplete_probes() -> io::Result<Vec<ProbeRecord>> {
    let directory = journal_directory()?;
    match fs::symlink_metadata(&directory) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error),
        Ok(_) => scan_probes_in(&directory),
    }
}

pub fn inspect_evidence() -> io::Result<(Vec<Record>, Vec<ProbeRecord>)> {
    Ok((inspect_incomplete()?, inspect_incomplete_probes()?))
}

#[cfg(test)]
pub(crate) fn inspect_evidence_in(directory: &Path) -> io::Result<(Vec<Record>, Vec<ProbeRecord>)> {
    Ok((scan_in(directory)?, scan_probes_in(directory)?))
}

/// Explicitly discard private evidence only after external verification.
/// Never interacts with the remote server.
pub fn acknowledge(record: &Record) -> io::Result<()> {
    retire_in(&journal_directory()?, record)
}

pub fn acknowledge_probe(record: &ProbeRecord) -> io::Result<()> {
    retire_probe_in(&journal_directory()?, record)
}

fn retire_probe_in(directory: &Path, record: &ProbeRecord) -> io::Result<()> {
    if !record.valid() {
        return Err(invalid_data("Invalid WebDAV safety-probe record"));
    }
    check_private_directory(directory)?;
    let _lock = lock_directory(directory)?;
    let path = probe_path(directory, &record.id)?;
    if read_probe_record(&path)? != *record {
        return Err(invalid_data("WebDAV probe evidence changed since review"));
    }
    fs::remove_file(&path)?;
    File::open(directory)?.sync_all()
}

fn retire_in(directory: &Path, record: &Record) -> io::Result<()> {
    if !record.valid() {
        return Err(invalid_data("Invalid WebDAV journal record"));
    }
    check_private_directory(directory)?;
    let _lock = lock_directory(directory)?;
    let path = record_path(directory, &record.id)?;
    if read_record(&path)? != *record {
        return Err(invalid_data("WebDAV journal evidence changed since review"));
    }
    fs::remove_file(&path)?;
    File::open(directory)?.sync_all()
}

fn scan_in(directory: &Path) -> io::Result<Vec<Record>> {
    check_private_directory(directory)?;
    let mut records = Vec::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            return Err(invalid_data("Invalid filename in WebDAV journal directory"));
        };
        if !name.ends_with(".toml") {
            continue;
        }
        let id = name.trim_end_matches(".toml");
        if id.len() != 32 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(invalid_data("Unexpected WebDAV journal filename"));
        }
        if records.len() >= MAX_RECORDS {
            return Err(invalid_data("WebDAV journal capacity exceeded"));
        }
        let record = read_record(&entry.path())?;
        if record.id != id {
            return Err(invalid_data("WebDAV journal filename and record differ"));
        }
        records.push(record);
    }
    records.sort_by(|left, right| {
        right
            .created_unix_seconds
            .cmp(&left.created_unix_seconds)
            .then_with(|| left.id.cmp(&right.id))
    });
    Ok(records)
}

fn scan_probes_in(directory: &Path) -> io::Result<Vec<ProbeRecord>> {
    check_private_directory(directory)?;
    let mut records = Vec::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            return Err(invalid_data("Invalid filename in WebDAV journal directory"));
        };
        let Some(id) = name.strip_suffix(".probe") else {
            continue;
        };
        if id.len() != 32 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(invalid_data("Unexpected WebDAV probe journal filename"));
        }
        if records.len() >= MAX_RECORDS {
            return Err(invalid_data("WebDAV probe journal capacity exceeded"));
        }
        let record = read_probe_record(&entry.path())?;
        if record.id != id {
            return Err(invalid_data(
                "WebDAV probe journal filename and record differ",
            ));
        }
        records.push(record);
    }
    records.sort_by(|left, right| {
        right
            .created_unix_seconds
            .cmp(&left.created_unix_seconds)
            .then_with(|| left.id.cmp(&right.id))
    });
    Ok(records)
}

fn read_record(path: &Path) -> io::Result<Record> {
    let before = fs::symlink_metadata(path)?;
    if !before.is_file()
        || before.len() > MAX_RECORD_BYTES
        || before.mode() & 0o077 != 0
        || before.uid() != unsafe { libc::geteuid() }
    {
        return Err(invalid_data(
            "WebDAV journal file is not private and regular",
        ));
    }
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)?;
    let current = file.metadata()?;
    if current.dev() != before.dev() || current.ino() != before.ino() {
        return Err(invalid_data("WebDAV journal file was replaced during open"));
    }
    let mut bytes = Vec::new();
    file.take(MAX_RECORD_BYTES + 1).read_to_end(&mut bytes)?;
    let after = fs::symlink_metadata(path)?;
    if bytes.len() as u64 > MAX_RECORD_BYTES
        || after.dev() != before.dev()
        || after.ino() != before.ino()
        || after.len() != before.len()
        || after.mtime() != before.mtime()
        || after.mtime_nsec() != before.mtime_nsec()
        || after.ctime() != before.ctime()
        || after.ctime_nsec() != before.ctime_nsec()
    {
        return Err(invalid_data("WebDAV journal file changed during read"));
    }
    let text = std::str::from_utf8(&bytes).map_err(io::Error::other)?;
    let stored: StoredRecord = toml::from_str(text).map_err(io::Error::other)?;
    let record = Record {
        id: stored.id,
        stage_uri: stored.stage_uri,
        destination_uri: stored.destination_uri,
        created_unix_seconds: stored.created_unix_seconds,
    };
    if stored.version != VERSION || !record.valid() {
        return Err(invalid_data("Unsupported or invalid WebDAV journal record"));
    }
    Ok(record)
}

fn read_probe_record(path: &Path) -> io::Result<ProbeRecord> {
    let before = fs::symlink_metadata(path)?;
    if !before.is_file()
        || before.len() > MAX_RECORD_BYTES
        || before.mode() & 0o077 != 0
        || before.uid() != unsafe { libc::geteuid() }
    {
        return Err(invalid_data(
            "WebDAV probe journal file is not private and regular",
        ));
    }
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)?;
    let current = file.metadata()?;
    if current.dev() != before.dev() || current.ino() != before.ino() {
        return Err(invalid_data(
            "WebDAV probe journal was replaced during open",
        ));
    }
    let mut bytes = Vec::new();
    file.take(MAX_RECORD_BYTES + 1).read_to_end(&mut bytes)?;
    let after = fs::symlink_metadata(path)?;
    if bytes.len() as u64 > MAX_RECORD_BYTES
        || after.dev() != before.dev()
        || after.ino() != before.ino()
        || after.len() != before.len()
        || after.mtime() != before.mtime()
        || after.mtime_nsec() != before.mtime_nsec()
        || after.ctime() != before.ctime()
        || after.ctime_nsec() != before.ctime_nsec()
    {
        return Err(invalid_data("WebDAV probe journal changed during read"));
    }
    let text = std::str::from_utf8(&bytes).map_err(io::Error::other)?;
    let stored: StoredProbeRecord = toml::from_str(text).map_err(io::Error::other)?;
    let record = ProbeRecord {
        id: stored.id,
        prefix_uri: stored.prefix_uri,
        created_unix_seconds: stored.created_unix_seconds,
    };
    if stored.version != VERSION || !record.valid() {
        return Err(invalid_data("Unsupported or invalid WebDAV probe journal"));
    }
    Ok(record)
}

fn record_path(directory: &Path, id: &str) -> io::Result<PathBuf> {
    if id.len() != 32 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(invalid_data("Invalid WebDAV journal ID"));
    }
    Ok(directory.join(format!("{id}.toml")))
}

fn probe_path(directory: &Path, id: &str) -> io::Result<PathBuf> {
    if id.len() != 32 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(invalid_data("Invalid WebDAV safety-probe ID"));
    }
    Ok(directory.join(format!("{id}.probe")))
}

fn journal_directory() -> io::Result<PathBuf> {
    let base = env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::config::home().join(".local/state"));
    if !base.is_absolute()
        || base
            .components()
            .any(|part| matches!(part, Component::ParentDir))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "XDG_STATE_HOME must be absolute without parent traversal",
        ));
    }
    Ok(base.join("ricefield").join("webdav-journals"))
}

fn prepare_private_directory(directory: &Path) -> io::Result<()> {
    fs::create_dir_all(directory)?;
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(directory)?;
    if file.metadata()?.uid() != unsafe { libc::geteuid() } {
        return Err(invalid_data("WebDAV journal directory has another owner"));
    }
    if unsafe { libc::fchmod(file.as_raw_fd(), 0o700) } != 0 {
        return Err(io::Error::last_os_error());
    }
    check_private_directory(directory)
}

fn check_private_directory(directory: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(directory)?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
    {
        return Err(invalid_data(
            "WebDAV journal directory must be owned and private",
        ));
    }
    Ok(())
}

fn lock_directory(directory: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(directory.join(".lock"))?;
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
    {
        return Err(invalid_data(
            "WebDAV journal lock is not private and regular",
        ));
    }
    // Several independent uploads can complete probes simultaneously. This
    // bounded wait is only for local evidence, never remote publication.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            break;
        }
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::EAGAIN) || std::time::Instant::now() >= deadline {
            return Err(error);
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    Ok(file)
}

fn invalid_data(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn isolated_directory() -> PathBuf {
        let mut nonce = [0u8; 16];
        File::open("/dev/urandom")
            .unwrap()
            .read_exact(&mut nonce)
            .unwrap();
        let suffix: String = nonce.iter().map(|byte| format!("{byte:02x}")).collect();
        env::temp_dir().join(format!("ricefield-webdav-journal-test-{suffix}"))
    }

    fn urls() -> (String, String) {
        (
            "https://dav.example.test/share/.ricefield-upload-0123456789abcdef0123456789abcdef"
                .into(),
            "https://dav.example.test/share/final.txt".into(),
        )
    }

    #[test]
    fn persists_before_remote_mutation_and_retains_uncertain_attempts() {
        let directory = isolated_directory();
        let (stage, destination) = urls();
        let handle = PendingUpload::begin_in(&directory, &stage, &destination).unwrap();
        let records = scan_in(&directory).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].stage_uri, stage);
        assert_eq!(records[0].destination_uri, destination);
        drop(handle);
        assert_eq!(scan_in(&directory).unwrap().len(), 1);
        retire_in(&directory, &records[0]).unwrap();
        assert!(scan_in(&directory).unwrap().is_empty());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn sftp_upload_and_probe_journals_are_private_and_restart_visible() {
        let directory = isolated_directory();
        let id = "0123456789abcdef0123456789abcdef";
        let stage = format!("sftp://host.example/share/.ricefield-upload-{id}");
        let destination = "sftp://host.example/share/document.txt";
        let probe = format!("sftp://host.example/share/.ricefield-safety-{id}");
        let upload = PendingUpload::begin_in(&directory, &stage, destination).unwrap();
        let safety = PendingProbe::begin_in(&directory, &probe).unwrap();
        drop(upload);
        drop(safety);
        assert_eq!(scan_in(&directory).unwrap()[0].stage_uri, stage);
        assert_eq!(scan_probes_in(&directory).unwrap()[0].prefix_uri, probe);
        let upload_record = scan_in(&directory).unwrap().pop().unwrap();
        let probe_record = scan_probes_in(&directory).unwrap().pop().unwrap();
        retire_in(&directory, &upload_record).unwrap();
        retire_probe_in(&directory, &probe_record).unwrap();
        assert!(scan_in(&directory).unwrap().is_empty());
        assert!(scan_probes_in(&directory).unwrap().is_empty());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn smb_upload_and_probe_evidence_survive_restart_without_secrets() {
        let directory = isolated_directory();
        let id = "0123456789abcdef0123456789abcdef";
        let stage = format!("smb://nas.example/media/.ricefield-upload-{id}");
        let destination = "smb://nas.example/media/file.txt";
        let probe = format!("smb://nas.example/media/.ricefield-safety-{id}");
        let upload = PendingUpload::begin_in(&directory, &stage, destination).unwrap();
        let safety = PendingProbe::begin_in(&directory, &probe).unwrap();
        drop(upload);
        drop(safety);
        let (uploads, probes) = inspect_evidence_in(&directory).unwrap();
        assert_eq!(uploads.len(), 1);
        assert_eq!(uploads[0].destination_uri, destination);
        assert_eq!(probes.len(), 1);
        assert_eq!(probes[0].prefix_uri, probe);
        retire_in(&directory, &uploads[0]).unwrap();
        retire_probe_in(&directory, &probes[0]).unwrap();
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn acknowledged_success_only_removes_own_record() {
        let directory = isolated_directory();
        let (stage, destination) = urls();
        let handle = PendingUpload::begin_in(&directory, &stage, &destination).unwrap();
        handle.finish();
        assert!(scan_in(&directory).unwrap().is_empty());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn safety_probe_evidence_is_durable_before_network_mutation() {
        let directory = isolated_directory();
        let prefix =
            "https://dav.example.test/share/.ricefield-safety-0123456789abcdef0123456789abcdef";
        let handle = PendingProbe::begin_in(&directory, prefix).unwrap();
        let records = scan_probes_in(&directory).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].prefix_uri, prefix);
        let metadata = fs::metadata(directory.join(format!("{}.probe", records[0].id))).unwrap();
        assert_eq!(metadata.mode() & 0o077, 0);
        drop(handle); // Simulated interruption: evidence must survive.
        assert_eq!(scan_probes_in(&directory).unwrap(), records);
        retire_probe_in(&directory, &records[0]).unwrap();
        assert!(scan_probes_in(&directory).unwrap().is_empty());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn safety_probe_acknowledgement_rejects_stale_and_unsafe_records() {
        let directory = isolated_directory();
        let prefix =
            "https://dav.example.test/share/.ricefield-safety-0123456789abcdef0123456789abcdef";
        let handle = PendingProbe::begin_in(&directory, prefix).unwrap();
        let mut stale = scan_probes_in(&directory).unwrap().remove(0);
        stale.prefix_uri.push_str("-changed");
        assert!(retire_probe_in(&directory, &stale).is_err());
        drop(handle);
        let path = directory.join("0123456789abcdef0123456789abcdef.probe");
        fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink("/etc/passwd", &path).unwrap();
        assert!(scan_probes_in(&directory).is_err());
        fs::remove_dir_all(directory).unwrap();

        let directory = isolated_directory();
        assert!(PendingProbe::begin_in(
            &directory,
            "https://person:password@dav.example.test/share/.ricefield-safety-0123456789abcdef0123456789abcdef",
        )
        .is_err());
        assert!(!directory.exists());
    }

    #[test]
    fn refuses_stale_acknowledgement_symlinks_and_malformed_records() {
        let directory = isolated_directory();
        let (stage, destination) = urls();
        let handle = PendingUpload::begin_in(&directory, &stage, &destination).unwrap();
        let mut stale = scan_in(&directory).unwrap().remove(0);
        stale.destination_uri.push('x');
        assert!(retire_in(&directory, &stale).is_err());
        drop(handle);
        let record_path = directory.join("0123456789abcdef0123456789abcdef.toml");
        fs::remove_file(&record_path).unwrap();
        std::os::unix::fs::symlink("/etc/passwd", &record_path).unwrap();
        assert!(scan_in(&directory).is_err());
        fs::remove_dir_all(directory).unwrap();

        let directory = isolated_directory();
        let (stage, destination) = urls();
        assert!(
            PendingUpload::begin_in(
                &directory,
                &stage.replace("https://", "http://name:password@"),
                &destination.replace("https://", "http://name:password@"),
            )
            .is_err()
        );
        assert!(!directory.exists());
    }
}
