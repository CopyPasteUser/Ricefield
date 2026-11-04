//! Durable, intentionally non-recovering records for cross-filesystem moves.
//!
//! Each transition atomically replaces a private TOML file and syncs both the
//! file and its containing directory. Recovery is inspection-only: a journal
//! record never authorizes automatic deletion of a source or staging path.

use serde::{Deserialize, Serialize};
use std::{
    env,
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

const VERSION: u32 = 1;
const MAX_RECORD_BYTES: u64 = 64 * 1024;
const MAX_RECORDS: usize = 4096;
static NEXT_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Prepared,
    StagedVerified,
    Published,
    Completed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Record {
    pub id: String,
    pub state: State,
    pub source: PathBuf,
    pub destination: PathBuf,
    pub staging: Option<PathBuf>,
}

/// Read-only observation; presence is not proof of matching contents or of
/// a recoverable move. In particular, never follow a selected symlink.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PathStatus {
    Missing,
    File,
    Directory,
    Symlink,
    Other,
    Inaccessible(String),
}

impl PathStatus {
    pub fn label(&self) -> String {
        match self {
            Self::Missing => "Missing".into(),
            Self::File => "Present (file)".into(),
            Self::Directory => "Present (directory)".into(),
            Self::Symlink => "Present (symlink)".into(),
            Self::Other => "Present (special file)".into(),
            Self::Inaccessible(error) => format!("Cannot inspect: {error}"),
        }
    }

    fn present(&self) -> bool {
        matches!(
            self,
            Self::File | Self::Directory | Self::Symlink | Self::Other
        )
    }
}

#[derive(Clone, Debug)]
pub struct Inspection {
    pub record: Record,
    pub source: PathStatus,
    pub destination: PathStatus,
    pub staging: Option<PathStatus>,
}

impl Inspection {
    /// Conservative triage only: metadata presence cannot authorize an undo,
    /// source deletion, or dismissal of a recovery record.
    pub fn guidance(&self) -> &'static str {
        if matches!(&self.source, PathStatus::Inaccessible(_))
            || matches!(&self.destination, PathStatus::Inaccessible(_))
            || self
                .staging
                .as_ref()
                .is_some_and(|status| matches!(status, PathStatus::Inaccessible(_)))
        {
            "At least one path cannot be inspected. Check mounts and permissions before taking action."
        } else if !self.source.present() && !self.destination.present() {
            "Neither recorded path is visible. Check mounts and backups; do not discard this record."
        } else if self.source.present() && self.destination.present() {
            "Both paths exist. Compare their actual contents before deciding what to keep; neither is removed automatically."
        } else if self.source.present() {
            "The original is visible, but the destination is missing. Inspect the staging path and verify mount availability before retrying."
        } else {
            "The destination is visible, but the original is missing. Verify destination integrity and mounts before resolving this record."
        }
    }
}

fn path_status(path: &Path) -> PathStatus {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => PathStatus::Symlink,
        Ok(meta) if meta.is_dir() => PathStatus::Directory,
        Ok(meta) if meta.is_file() => PathStatus::File,
        Ok(_) => PathStatus::Other,
        Err(error) if error.kind() == io::ErrorKind::NotFound => PathStatus::Missing,
        Err(error) => PathStatus::Inaccessible(error.to_string()),
    }
}

fn inspect_record(record: Record) -> Inspection {
    Inspection {
        source: path_status(&record.source),
        destination: path_status(&record.destination),
        staging: record.staging.as_ref().map(|path| path_status(path)),
        record,
    }
}

/// Inspect saved records and their current paths off the GUI thread.
/// This remains strictly read-only and never implies that files match.
pub fn inspect_incomplete() -> io::Result<Vec<Inspection>> {
    Ok(scan_incomplete()?.into_iter().map(inspect_record).collect())
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredRecord {
    version: u32,
    id: String,
    state: State,
    source_hex: String,
    destination_hex: String,
    staging_hex: Option<String>,
}

/// One writer owns the journal for a single move; it is not a recovery handle.
pub struct Journal {
    path: PathBuf,
    record: Record,
}

impl Journal {
    /// Persist Prepared before creating or copying any staging payload.
    pub fn begin(source: &Path, destination: &Path) -> io::Result<Self> {
        Self::begin_in(&journal_directory()?, source, destination)
    }

    pub(crate) fn begin_in(
        directory: &Path,
        source: &Path,
        destination: &Path,
    ) -> io::Result<Self> {
        ensure_private_directory(directory)?;
        let source = absolute_path(source)?;
        let destination = absolute_path(destination)?;
        if source == destination {
            return Err(invalid_input("Journal source and destination must differ"));
        }
        for _ in 0..128 {
            let id = next_id();
            let path = directory.join(format!("{id}.toml"));
            let record = Record {
                id,
                state: State::Prepared,
                source: source.clone(),
                destination: destination.clone(),
                staging: None,
            };
            match write_record(&path, &record, true) {
                Ok(()) => return Ok(Self { path, record }),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "Cannot allocate a unique operation journal",
        ))
    }

    #[cfg(test)]
    pub fn record(&self) -> &Record {
        &self.record
    }

    /// Record the reserved staging directory before copying data into it.
    pub fn set_staging(&mut self, staging: &Path) -> io::Result<()> {
        if self.record.state != State::Prepared || self.record.staging.is_some() {
            return Err(invalid_input(
                "Staging can only be assigned once while prepared",
            ));
        }
        let staging = absolute_path(staging)?;
        if staging == self.record.source || staging == self.record.destination {
            return Err(invalid_input("Staging cannot equal source or destination"));
        }
        let mut updated = self.record.clone();
        updated.staging = Some(staging);
        self.persist(updated)
    }

    /// Valid sequence: Prepared -> StagedVerified -> Published -> Completed.
    /// In particular, persist Published successfully *before* deleting any
    /// original source entry; a failed transition must stop source deletion.
    pub fn advance(&mut self, next: State) -> io::Result<()> {
        let permitted = matches!(
            (self.record.state, next),
            (State::Prepared, State::StagedVerified)
                | (State::StagedVerified, State::Published)
                | (State::Published, State::Completed)
        );
        if !permitted {
            return Err(invalid_input("Invalid journal state transition"));
        }
        if self.record.staging.is_none() {
            return Err(invalid_input("Staging path was not recorded"));
        }
        let mut updated = self.record.clone();
        updated.state = next;
        self.persist(updated)?;
        if next == State::Completed {
            // Completed transfers need no recovery. Retire their records so
            // the bounded startup scan cannot fill up after normal use.
            // Failure to retire is non-destructive: a Completed record is
            // ignored by recovery until it can be cleaned up later.
            if let Some(parent) = self.path.parent() {
                let cleanup =
                    fs::remove_file(&self.path).and_then(|()| File::open(parent)?.sync_all());
                if let Err(error) = cleanup {
                    eprintln!("RiceExplorer: completed journal cleanup failed: {error}");
                }
            }
        }
        Ok(())
    }

    fn persist(&mut self, updated: Record) -> io::Result<()> {
        write_record(&self.path, &updated, false)?;
        self.record = updated;
        Ok(())
    }
}

/// Inspect incomplete moves without modifying any files or triggering recovery.
/// Malformed/oversized records abort the scan instead of silently hiding risk.
pub fn scan_incomplete() -> io::Result<Vec<Record>> {
    scan_incomplete_in(&journal_directory()?)
}

fn scan_incomplete_in(directory: &Path) -> io::Result<Vec<Record>> {
    let metadata = match fs::symlink_metadata(directory) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    check_private_directory(&metadata)?;
    let mut records = Vec::new();
    let mut seen = 0usize;
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_none_or(|extension| extension != "toml") {
            continue;
        }
        seen += 1;
        if seen > MAX_RECORDS {
            return Err(invalid_data("Too many operation journal records"));
        }
        let metadata = fs::symlink_metadata(&path)?;
        if !metadata.is_file() || metadata.len() > MAX_RECORD_BYTES {
            return Err(invalid_data("Journal record is not a bounded regular file"));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if metadata.permissions().mode() & 0o077 != 0 {
                return Err(invalid_data("Journal record permissions are too broad"));
            }
        }
        let mut content = Vec::new();
        File::open(&path)?
            .take(MAX_RECORD_BYTES + 1)
            .read_to_end(&mut content)?;
        if content.len() as u64 > MAX_RECORD_BYTES {
            return Err(invalid_data("Operation journal record is oversized"));
        }
        let stored: StoredRecord = toml::from_str(
            std::str::from_utf8(&content).map_err(|_| invalid_data("Invalid journal UTF-8"))?,
        )
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        let record = from_stored(stored)?;
        if path.file_stem().and_then(|stem| stem.to_str()) != Some(record.id.as_str()) {
            return Err(invalid_data("Journal ID and filename disagree"));
        }
        if record.state != State::Completed {
            records.push(record);
        }
    }
    records.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(records)
}

fn journal_directory() -> io::Result<PathBuf> {
    let state_home = match env::var_os("XDG_STATE_HOME") {
        Some(path) if !path.is_empty() => {
            let path = PathBuf::from(path);
            if !path.is_absolute() {
                return Err(invalid_input("XDG_STATE_HOME must be an absolute path"));
            }
            path
        }
        _ => {
            let home = env::var_os("HOME")
                .filter(|home| !home.is_empty())
                .ok_or_else(|| invalid_input("HOME is not set"))?;
            PathBuf::from(home).join(".local/state")
        }
    };
    Ok(state_home.join("rice-explorer/operations"))
}

fn ensure_private_directory(directory: &Path) -> io::Result<()> {
    let parent = directory
        .parent()
        .ok_or_else(|| invalid_input("Invalid journal directory"))?;
    fs::create_dir_all(parent)?;
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    match builder.create(directory) {
        Ok(()) => {
            File::open(parent)?.sync_all()?;
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    let metadata = fs::symlink_metadata(directory)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(invalid_data("Journal directory must be a real directory"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
        }
    }
    check_private_directory(&fs::symlink_metadata(directory)?)
}

fn check_private_directory(metadata: &fs::Metadata) -> io::Result<()> {
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(invalid_data("Journal directory is not a real directory"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(invalid_data("Journal directory permissions are too broad"));
        }
    }
    Ok(())
}

fn write_record(path: &Path, record: &Record, new_record: bool) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| invalid_input("Invalid journal path"))?;
    let encoded = toml::to_string(&to_stored(record)?).map_err(io::Error::other)?;
    if encoded.len() as u64 > MAX_RECORD_BYTES {
        return Err(invalid_input("Operation journal record is oversized"));
    }
    let temporary = parent.join(format!(".{}.{}.tmp", record.id, next_id()));
    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        file.write_all(encoded.as_bytes())?;
        file.sync_all()?;
        drop(file);
        if new_record {
            publish_new_record(&temporary, path)?;
        } else {
            // Atomic replacement of our private journal record, not a user file.
            fs::rename(&temporary, path)?;
        }
        File::open(parent)?.sync_all()
    })();
    if result.is_err() {
        // A crash can still leave a .tmp file; scans intentionally ignore it.
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn publish_new_record(temporary: &Path, destination: &Path) -> io::Result<()> {
    // A same-directory hard link publishes the fully synced record atomically
    // without the overwrite behavior of std::fs::rename. Subsequent updates
    // use atomic rename to replace only this journal's own record.
    fs::hard_link(temporary, destination)?;
    if let Err(error) = fs::remove_file(temporary) {
        eprintln!("RiceExplorer: journal temp cleanup after publication failed: {error}");
    }
    Ok(())
}

fn to_stored(record: &Record) -> io::Result<StoredRecord> {
    Ok(StoredRecord {
        version: VERSION,
        id: record.id.clone(),
        state: record.state,
        source_hex: hex_path(&record.source)?,
        destination_hex: hex_path(&record.destination)?,
        staging_hex: record
            .staging
            .as_ref()
            .map(|path| hex_path(path.as_path()))
            .transpose()?,
    })
}

fn from_stored(stored: StoredRecord) -> io::Result<Record> {
    if stored.version != VERSION || !valid_id(&stored.id) {
        return Err(invalid_data("Unsupported or invalid journal record"));
    }
    let source = path_from_hex(&stored.source_hex)?;
    let destination = path_from_hex(&stored.destination_hex)?;
    let staging = stored
        .staging_hex
        .as_deref()
        .map(path_from_hex)
        .transpose()?;
    if !source.is_absolute()
        || !destination.is_absolute()
        || staging.as_ref().is_some_and(|path| !path.is_absolute())
    {
        return Err(invalid_data("Journal paths must be absolute"));
    }
    if source == destination
        || staging
            .as_ref()
            .is_some_and(|p| p == &source || p == &destination)
    {
        return Err(invalid_data("Journal paths overlap"));
    }
    if stored.state != State::Prepared && staging.is_none() {
        return Err(invalid_data("Published operation has no staging path"));
    }
    Ok(Record {
        id: stored.id,
        state: stored.state,
        source,
        destination,
        staging,
    })
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 128 && id.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-')
}

fn absolute_path(path: &Path) -> io::Result<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(env::current_dir()?.join(path))
    }
}

fn next_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let count = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    format!("{nanos:x}-{:x}-{count:x}", std::process::id())
}

#[cfg(unix)]
fn hex_path(path: &Path) -> io::Result<String> {
    use std::os::unix::ffi::OsStrExt;
    let bytes = path.as_os_str().as_bytes();
    let mut hex = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        write!(hex, "{byte:02x}").map_err(io::Error::other)?;
    }
    Ok(hex)
}

#[cfg(unix)]
fn path_from_hex(hex: &str) -> io::Result<PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    if !hex.len().is_multiple_of(2) || hex.is_empty() {
        return Err(invalid_data("Invalid journal path encoding"));
    }
    let mut bytes = Vec::with_capacity(hex.len() / 2);
    for pair in hex.as_bytes().as_chunks::<2>().0 {
        let high = (pair[0] as char)
            .to_digit(16)
            .ok_or_else(|| invalid_data("Invalid journal path hex"))?;
        let low = (pair[1] as char)
            .to_digit(16)
            .ok_or_else(|| invalid_data("Invalid journal path hex"))?;
        let byte = ((high << 4) | low) as u8;
        if byte == 0 {
            return Err(invalid_data("NUL in journal path"));
        }
        bytes.push(byte);
    }
    Ok(PathBuf::from(OsString::from_vec(bytes)))
}

#[cfg(not(unix))]
fn hex_path(path: &Path) -> io::Result<String> {
    let text = path
        .to_str()
        .ok_or_else(|| invalid_input("Non-Unicode path"))?;
    let mut hex = String::new();
    for byte in text.bytes() {
        use std::fmt::Write as _;
        write!(hex, "{byte:02x}").map_err(io::Error::other)?;
    }
    Ok(hex)
}

#[cfg(not(unix))]
fn path_from_hex(hex: &str) -> io::Result<PathBuf> {
    if !hex.len().is_multiple_of(2) || hex.is_empty() {
        return Err(invalid_data("Invalid journal path encoding"));
    }
    let mut bytes = Vec::new();
    for pair in hex.as_bytes().as_chunks::<2>().0 {
        let text = std::str::from_utf8(pair).map_err(|_| invalid_data("Invalid path hex"))?;
        bytes.push(u8::from_str_radix(text, 16).map_err(|_| invalid_data("Invalid path hex"))?);
    }
    if bytes.contains(&0) {
        return Err(invalid_data("NUL in journal path"));
    }
    Ok(PathBuf::from(
        String::from_utf8(bytes).map_err(|_| invalid_data("Invalid journal path UTF-8"))?,
    ))
}

fn invalid_input(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

fn invalid_data(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_directory() -> PathBuf {
        let path = env::temp_dir().join(format!("rice-explorer-journal-{}", next_id()));
        fs::create_dir(&path).unwrap();
        path
    }

    #[test]
    fn states_are_durable_and_scan_ignores_completed() {
        let directory = test_directory();
        let source = directory.join("from");
        let destination = directory.join("to");
        let mut journal = Journal::begin_in(&directory, &source, &destination).unwrap();
        assert_eq!(journal.record().state, State::Prepared);
        assert_eq!(scan_incomplete_in(&directory).unwrap().len(), 1);

        journal.set_staging(&directory.join("stage")).unwrap();
        journal.advance(State::StagedVerified).unwrap();
        journal.advance(State::Published).unwrap();
        let incomplete = scan_incomplete_in(&directory).unwrap();
        assert_eq!(incomplete.len(), 1);
        assert_eq!(incomplete[0].state, State::Published);
        assert_eq!(incomplete[0].source, source);
        assert_eq!(incomplete[0].destination, destination);
        assert_eq!(incomplete[0].staging, Some(directory.join("stage")));
        journal.advance(State::Completed).unwrap();
        assert!(scan_incomplete_in(&directory).unwrap().is_empty());
        assert!(
            !directory
                .join(format!("{}.toml", journal.record().id))
                .exists()
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn inspection_distinguishes_missing_and_present_paths_without_mutation() {
        let directory = test_directory();
        let source = directory.join("original");
        let destination = directory.join("published");
        let staging = directory.join("staging");
        fs::write(&source, b"original data").unwrap();
        fs::create_dir(&staging).unwrap();
        let record = Record {
            id: "test".into(),
            state: State::StagedVerified,
            source: source.clone(),
            destination: destination.clone(),
            staging: Some(staging.clone()),
        };
        let first = inspect_record(record.clone());
        assert_eq!(first.source, PathStatus::File);
        assert_eq!(first.destination, PathStatus::Missing);
        assert_eq!(first.staging, Some(PathStatus::Directory));
        assert!(first.guidance().contains("original is visible"));
        fs::write(&destination, b"published data").unwrap();
        let second = inspect_record(record);
        assert_eq!(second.destination, PathStatus::File);
        assert!(second.guidance().contains("Both paths exist"));
        assert_eq!(fs::read(source).unwrap(), b"original data");
        assert_eq!(fs::read(destination).unwrap(), b"published data");
        assert!(staging.is_dir());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn recovery_guidance_never_treats_missing_or_unreadable_paths_as_safe() {
        let mut inspection = Inspection {
            record: Record {
                id: "missing".into(),
                state: State::Published,
                source: PathBuf::from("/missing-source"),
                destination: PathBuf::from("/missing-destination"),
                staging: None,
            },
            source: PathStatus::Missing,
            destination: PathStatus::Missing,
            staging: None,
        };
        assert!(
            inspection
                .guidance()
                .contains("Neither recorded path is visible")
        );
        inspection.destination = PathStatus::Inaccessible("device unavailable".into());
        assert!(
            inspection
                .guidance()
                .contains("Check mounts and permissions")
        );
        inspection.destination = PathStatus::File;
        assert!(inspection.guidance().contains("destination is visible"));
    }

    #[cfg(unix)]
    #[test]
    fn inspection_recognizes_symlinks_without_following_target() {
        use std::os::unix::fs::symlink;
        let directory = test_directory();
        let source = directory.join("source-link");
        let destination = directory.join("destination");
        symlink(directory.join("missing-target"), &source).unwrap();
        let inspection = inspect_record(Record {
            id: "link".into(),
            state: State::Published,
            source: source.clone(),
            destination: destination.clone(),
            staging: Some(directory.join("staging")),
        });
        assert_eq!(inspection.source, PathStatus::Symlink);
        assert_eq!(inspection.destination, PathStatus::Missing);
        assert!(inspection.guidance().contains("original is visible"));
        assert!(
            fs::symlink_metadata(source)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn invalid_transitions_do_not_change_disk() {
        let directory = test_directory();
        let mut journal =
            Journal::begin_in(&directory, Path::new("from"), Path::new("to")).unwrap();
        assert_eq!(
            journal.advance(State::Published).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(
            journal.advance(State::StagedVerified).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        journal.set_staging(Path::new("stage")).unwrap();
        assert_eq!(
            journal.set_staging(Path::new("stage2")).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(
            scan_incomplete_in(&directory).unwrap()[0].state,
            State::Prepared
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn preserves_non_utf8_paths_and_private_modes() {
        use std::os::unix::{ffi::OsStringExt, fs::PermissionsExt};
        let directory = test_directory();
        let source = directory.join(OsString::from_vec(b"source-\xff".to_vec()));
        let destination = directory.join(OsString::from_vec(b"destination-\xfe".to_vec()));
        let journal = Journal::begin_in(&directory, &source, &destination).unwrap();
        let loaded = scan_incomplete_in(&directory).unwrap();
        assert_eq!(loaded[0].source, source);
        assert_eq!(loaded[0].destination, destination);
        assert_eq!(
            fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(directory.join(format!("{}.toml", journal.record().id)))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn corrupted_and_oversized_records_fail_closed() {
        let directory = test_directory();
        let journal = Journal::begin_in(&directory, Path::new("from"), Path::new("to")).unwrap();
        let path = directory.join(format!("{}.toml", journal.record().id));
        fs::write(&path, "version = 999\n").unwrap();
        assert_eq!(
            scan_incomplete_in(&directory).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        fs::write(&path, vec![b'x'; (MAX_RECORD_BYTES + 1) as usize]).unwrap();
        assert_eq!(
            scan_incomplete_in(&directory).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn refuses_symlinked_journal_directory() {
        use std::os::unix::fs::symlink;
        let root = test_directory();
        let private = root.join("private");
        let link = root.join("shortcut");
        fs::create_dir(&private).unwrap();
        symlink(&private, &link).unwrap();
        assert!(Journal::begin_in(&link, Path::new("from"), Path::new("to")).is_err());
        assert!(scan_incomplete_in(&link).is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
