//! Durable, fail-closed records for cross-filesystem moves.
//!
//! Each transition atomically replaces a private TOML file and syncs both the
//! file and its containing directory. Recovery is explicitly user-initiated:
//! an already published copy can be confirmed and its journal retired after
//! fresh checks. A record never authorizes deletion of source or staging data.

use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    env,
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

const VERSION: u32 = 3;
const MAX_RECORD_BYTES: u64 = 64 * 1024;
const MAX_RECORDS: usize = 4096;
// Recovery comparisons are explicitly requested, never run during startup.
// Limits make a remote or very large tree a deliberate manual-verification case.
const MAX_COMPARE_ENTRIES: u64 = 10_000;
const MAX_COMPARE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_COMPARE_DEPTH: usize = 128;
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
    pub source_identity: Option<ObjectIdentity>,
    pub staged_identity: Option<ObjectIdentity>,
    pub destination_identity: Option<ObjectIdentity>,
}

/// An observation of one inode, not an authenticated proof of ownership or
/// contents. Device/inode numbers can be reused after deletion or reboot.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ObjectIdentity {
    dev: u64,
    ino: u64,
    mode: u32,
    uid: u32,
    gid: u32,
    len: u64,
    mtime: i64,
    mtime_nsec: i64,
    ctime: i64,
    ctime_nsec: i64,
}

#[cfg(unix)]
fn identity_of(metadata: &fs::Metadata) -> ObjectIdentity {
    use std::os::unix::fs::MetadataExt;

    ObjectIdentity {
        dev: metadata.dev(),
        ino: metadata.ino(),
        mode: metadata.mode(),
        uid: metadata.uid(),
        gid: metadata.gid(),
        len: metadata.len(),
        mtime: metadata.mtime(),
        mtime_nsec: metadata.mtime_nsec(),
        ctime: metadata.ctime(),
        ctime_nsec: metadata.ctime_nsec(),
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IdentityStatus {
    NotRecorded,
    Missing,
    Unreadable(String),
    SameSnapshot,
    Changed,
    Replaced,
}

impl IdentityStatus {
    /// Human-readable observation only; never enough to authorize cleanup.
    pub fn label(&self) -> String {
        match self {
            Self::NotRecorded => "No saved inode evidence for this path".into(),
            Self::Missing => "Recorded path is missing".into(),
            Self::Unreadable(error) => format!("Cannot inspect saved identity: {error}"),
            Self::SameSnapshot => {
                "Saved inode and metadata still match (contents not proven)".into()
            }
            Self::Changed => "Same inode, but recorded metadata has changed".into(),
            Self::Replaced => "Different inode or object type now occupies this path".into(),
        }
    }
}

fn inspect_identity(path: &Path, recorded: Option<&ObjectIdentity>) -> IdentityStatus {
    let Some(recorded) = recorded else {
        return IdentityStatus::NotRecorded;
    };
    #[cfg(unix)]
    {
        match fs::symlink_metadata(path) {
            Ok(current) => {
                let current = identity_of(&current);
                if current.dev != recorded.dev
                    || current.ino != recorded.ino
                    || (current.mode & libc::S_IFMT) != (recorded.mode & libc::S_IFMT)
                {
                    IdentityStatus::Replaced
                } else if &current == recorded {
                    IdentityStatus::SameSnapshot
                } else {
                    IdentityStatus::Changed
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => IdentityStatus::Missing,
            Err(error) => IdentityStatus::Unreadable(error.to_string()),
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (path, recorded);
        IdentityStatus::NotRecorded
    }
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
    pub source_identity: IdentityStatus,
    pub destination_identity: IdentityStatus,
    pub publication_evidence: PublicationEvidence,
}

/// A persisted staging inode can be compared to the destination inode after
/// restart, even if the process died between rename and the Published record.
/// An inode match alone never proves that file contents are still unchanged.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PublicationEvidence {
    NotRecorded,
    DestinationMissing,
    SameInode,
    DifferentObject,
    Unreadable(String),
}

impl PublicationEvidence {
    pub fn label(&self) -> String {
        match self {
            Self::NotRecorded => "No staging-to-publication inode evidence".into(),
            Self::DestinationMissing => "Recorded staged object is not at the destination".into(),
            Self::SameInode => {
                "Published inode matches the staged inode (contents not proven)".into()
            }
            Self::DifferentObject => "Destination is not the recorded staged inode".into(),
            Self::Unreadable(error) => format!("Cannot verify staged inode: {error}"),
        }
    }
}

fn inspect_publication(record: &Record) -> PublicationEvidence {
    let Some(staged) = &record.staged_identity else {
        return PublicationEvidence::NotRecorded;
    };
    #[cfg(unix)]
    {
        match fs::symlink_metadata(&record.destination) {
            Ok(current) => {
                let current = identity_of(&current);
                if staged.dev == current.dev
                    && staged.ino == current.ino
                    && staged.mode & libc::S_IFMT == current.mode & libc::S_IFMT
                {
                    PublicationEvidence::SameInode
                } else {
                    PublicationEvidence::DifferentObject
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                PublicationEvidence::DestinationMissing
            }
            Err(error) => PublicationEvidence::Unreadable(error.to_string()),
        }
    }
    #[cfg(not(unix))]
    {
        let _ = staged;
        PublicationEvidence::NotRecorded
    }
}

impl Inspection {
    /// Only offer an explicit, separate Copy preview when the recorded source
    /// is still a supported object and its saved identity matches. The worker
    /// still validates paths before copying; this never authorizes deletion.
    pub fn can_offer_fresh_copy(&self) -> bool {
        matches!(
            self.source,
            PathStatus::File | PathStatus::Directory | PathStatus::Symlink
        ) && matches!(self.source_identity, IdentityStatus::SameSnapshot)
    }

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
        } else if matches!(self.source_identity, IdentityStatus::Replaced)
            || matches!(self.destination_identity, IdentityStatus::Replaced)
            || matches!(
                self.publication_evidence,
                PublicationEvidence::DifferentObject
            )
        {
            "A recorded path now names a different filesystem object. Treat recovery as unsafe; do not remove or overwrite either side."
        } else if matches!(self.source_identity, IdentityStatus::Changed)
            || matches!(self.destination_identity, IdentityStatus::Changed)
        {
            "A recorded inode has changed metadata since the move. This may reflect partial cleanup or another writer; manual verification is required."
        } else if matches!(self.source_identity, IdentityStatus::Unreadable(_))
            || matches!(self.destination_identity, IdentityStatus::Unreadable(_))
        {
            "Recorded filesystem identity cannot be checked. Verify mounts and permissions before touching either path."
        } else if !self.source.present() && !self.destination.present() {
            "Neither recorded path is visible. Check mounts and backups; do not discard this record."
        } else if self.source.present() && self.destination.present() {
            if matches!(self.source_identity, IdentityStatus::SameSnapshot)
                && matches!(self.destination_identity, IdentityStatus::SameSnapshot)
            {
                "Both recorded inodes still match their observed metadata, but this does not prove their contents or authorize deletion."
            } else {
                "Both paths exist. Compare their actual contents before deciding what to keep; neither is removed automatically."
            }
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
        publication_evidence: inspect_publication(&record),
        source: path_status(&record.source),
        destination: path_status(&record.destination),
        staging: record.staging.as_ref().map(|path| path_status(path)),
        source_identity: inspect_identity(&record.source, record.source_identity.as_ref()),
        destination_identity: inspect_identity(
            &record.destination,
            record.destination_identity.as_ref(),
        ),
        record,
    }
}

/// Inspect saved records and their current paths off the GUI thread.
/// This remains strictly read-only and never implies that files match.
pub fn inspect_incomplete() -> io::Result<Vec<Inspection>> {
    Ok(scan_incomplete()?.into_iter().map(inspect_record).collect())
}

/// A read-only, bounded comparison of two currently visible trees. A match is
/// an observation, NOT permission to delete either tree: files may change
/// immediately afterwards, and journal state does not establish provenance.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Comparison {
    Identical { entries: u64, bytes: u64 },
    Different(String),
    Inconclusive(String),
}

impl Comparison {
    pub fn label(&self) -> String {
        match self {
            Self::Identical { entries, bytes } => format!(
                "Contents matched at inspection time: {entries} entries, {bytes} bytes. This is not authorization to delete the source."
            ),
            Self::Different(reason) => format!("Contents differ: {reason}"),
            Self::Inconclusive(reason) => format!("Could not verify safely: {reason}"),
        }
    }
}

#[derive(Default)]
struct ComparisonBudget {
    entries: u64,
    bytes: u64,
}

pub fn compare_for_recovery(
    source: &Path,
    destination: &Path,
    cancelled: &AtomicBool,
) -> Comparison {
    let mut budget = ComparisonBudget::default();
    match compare_node(source, destination, 0, cancelled, &mut budget) {
        Ok(Some(reason)) => Comparison::Different(reason),
        Ok(None) => Comparison::Identical {
            entries: budget.entries,
            bytes: budget.bytes,
        },
        Err(error) => Comparison::Inconclusive(error.to_string()),
    }
}

fn compare_node(
    source: &Path,
    destination: &Path,
    depth: usize,
    cancelled: &AtomicBool,
    budget: &mut ComparisonBudget,
) -> io::Result<Option<String>> {
    if cancelled.load(Ordering::Relaxed) {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "Comparison cancelled",
        ));
    }
    if depth > MAX_COMPARE_DEPTH || budget.entries >= MAX_COMPARE_ENTRIES {
        return Err(io::Error::other(
            "Comparison exceeded entry or nesting limit; verify manually",
        ));
    }
    budget.entries += 1;
    let left = fs::symlink_metadata(source)?;
    let right = fs::symlink_metadata(destination)?;
    if !same_kind(&left, &right) {
        return Ok(Some(format!(
            "Different item types at {}",
            source.display()
        )));
    }
    if left.file_type().is_symlink() {
        if fs::read_link(source)? != fs::read_link(destination)? {
            return Ok(Some(format!(
                "Different symbolic-link targets at {}",
                source.display()
            )));
        }
    } else if left.is_dir() {
        let left_names = bounded_names(source, cancelled, budget)?;
        let right_names = bounded_names(destination, cancelled, budget)?;
        if left_names != right_names {
            return Ok(Some(format!(
                "Different directory entries at {}",
                source.display()
            )));
        }
        for name in left_names {
            if let Some(reason) = compare_node(
                &source.join(&name),
                &destination.join(&name),
                depth + 1,
                cancelled,
                budget,
            )? {
                return Ok(Some(reason));
            }
        }
    } else if left.is_file() {
        if left.len() != right.len() {
            return Ok(Some(format!(
                "Different file sizes at {}",
                source.display()
            )));
        }
        budget.bytes = budget
            .bytes
            .checked_add(left.len())
            .ok_or_else(|| io::Error::other("Comparison size overflow"))?;
        if budget.bytes > MAX_COMPARE_BYTES {
            return Err(io::Error::other(
                "Comparison exceeds 512 MiB; verify manually",
            ));
        }
        if !compare_regular_files(source, destination, &left, &right, cancelled)? {
            return Ok(Some(format!(
                "Different file bytes at {}",
                source.display()
            )));
        }
    } else {
        return Err(io::Error::other(format!(
            "Special files cannot be compared: {}",
            source.display()
        )));
    }
    // Reject comparisons made while any item was being replaced or modified.
    if !same_snapshot(&left, &fs::symlink_metadata(source)?)
        || !same_snapshot(&right, &fs::symlink_metadata(destination)?)
    {
        return Err(io::Error::other(
            "An item changed during comparison; try again",
        ));
    }
    Ok(None)
}

fn bounded_names(
    path: &Path,
    cancelled: &AtomicBool,
    budget: &ComparisonBudget,
) -> io::Result<BTreeSet<OsString>> {
    let mut names = BTreeSet::new();
    for entry in fs::read_dir(path)? {
        if cancelled.load(Ordering::Relaxed) {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "Comparison cancelled",
            ));
        }
        names.insert(entry?.file_name());
        if names.len() as u64 + budget.entries > MAX_COMPARE_ENTRIES {
            return Err(io::Error::other(
                "Directory has too many entries to compare safely",
            ));
        }
    }
    Ok(names)
}

fn same_kind(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    left.file_type().is_symlink() == right.file_type().is_symlink()
        && left.is_dir() == right.is_dir()
        && left.is_file() == right.is_file()
}

fn same_snapshot(before: &fs::Metadata, after: &fs::Metadata) -> bool {
    if !same_kind(before, after)
        || before.len() != after.len()
        || before.modified().ok() != after.modified().ok()
    {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if before.dev() != after.dev()
            || before.ino() != after.ino()
            || before.ctime() != after.ctime()
            || before.ctime_nsec() != after.ctime_nsec()
            || before.mode() != after.mode()
        {
            return false;
        }
    }
    true
}

fn open_regular_no_follow(path: &Path, snapshot: &fs::Metadata) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // Prevent substitution of the final component with a link, FIFO, etc.
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_file() || !same_snapshot(snapshot, &file.metadata()?) {
        return Err(io::Error::other(
            "A file changed before it could be compared",
        ));
    }
    Ok(file)
}

fn compare_regular_files(
    source: &Path,
    destination: &Path,
    left: &fs::Metadata,
    right: &fs::Metadata,
    cancelled: &AtomicBool,
) -> io::Result<bool> {
    let mut a = open_regular_no_follow(source, left)?;
    let mut b = open_regular_no_follow(destination, right)?;
    let mut buf_a = [0u8; 256 * 1024];
    let mut buf_b = [0u8; 256 * 1024];
    let mut remaining = left.len();
    let mut identical = true;
    while remaining > 0 {
        if cancelled.load(Ordering::Relaxed) {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "Comparison cancelled",
            ));
        }
        let n = remaining.min(buf_a.len() as u64) as usize;
        a.read_exact(&mut buf_a[..n])?;
        b.read_exact(&mut buf_b[..n])?;
        if buf_a[..n] != buf_b[..n] {
            identical = false;
        }
        remaining -= n as u64;
    }
    if !same_snapshot(left, &a.metadata()?) || !same_snapshot(right, &b.metadata()?) {
        return Err(io::Error::other("A file was modified during comparison"));
    }
    Ok(identical)
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source_identity: Option<ObjectIdentity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    staged_identity: Option<ObjectIdentity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    destination_identity: Option<ObjectIdentity>,
}

/// One writer owns the journal for a single move; it is not a recovery handle.
pub struct Journal {
    path: PathBuf,
    record: Record,
}

impl Journal {
    /// Durable evidence for a verified cross-device copy that retains its
    /// original source. The record never authorizes source deletion or replay.
    pub fn begin(source: &Path, destination: &Path) -> io::Result<Self> {
        Self::begin_in_with_limit(&journal_directory()?, source, destination, MAX_RECORDS)
    }

    /// Isolated staging and fault-injection fixtures must never touch real
    /// user state, even when run from the developer's desktop session.
    #[cfg(test)]
    pub(crate) fn begin_in(
        directory: &Path,
        source: &Path,
        destination: &Path,
    ) -> io::Result<Self> {
        Self::begin_in_with_limit(directory, source, destination, MAX_RECORDS)
    }

    fn begin_in_with_limit(
        directory: &Path,
        source: &Path,
        destination: &Path,
        limit: usize,
    ) -> io::Result<Self> {
        ensure_private_directory(directory)?;
        // Hold an advisory cross-process lock over both the capacity check
        // and journal publication. Concurrent RiceField writers cannot
        // each see the last free slot and exceed the recovery scan limit.
        #[cfg(unix)]
        let _lock = lock_journal_directory(directory)?;
        ensure_capacity(directory, limit)?;
        let source = absolute_path(source)?;
        let destination = absolute_path(destination)?;
        if source == destination {
            return Err(invalid_input("Journal source and destination must differ"));
        }
        #[cfg(unix)]
        let source_identity = Some(identity_of(&fs::symlink_metadata(&source)?));
        #[cfg(not(unix))]
        let source_identity = None;
        for _ in 0..128 {
            let id = next_id();
            let path = directory.join(format!("{id}.toml"));
            let record = Record {
                id,
                state: State::Prepared,
                source: source.clone(),
                destination: destination.clone(),
                staging: None,
                source_identity: source_identity.clone(),
                staged_identity: None,
                destination_identity: None,
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

    /// Once a fully verified stage is durably recorded, preserve it on an
    /// ordinary publication error for explicit user review after restart.
    /// Prepared (possibly partial) stages are still disposable.
    pub(crate) fn retain_verified_stage(&self) -> bool {
        matches!(self.record.state, State::StagedVerified | State::Published)
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
        if next == State::StagedVerified {
            #[cfg(unix)]
            {
                let staged = self
                    .record
                    .staging
                    .as_ref()
                    .ok_or_else(|| invalid_input("Staging path was not recorded"))?
                    .join("payload");
                updated.staged_identity = Some(identity_of(&fs::symlink_metadata(staged)?));
            }
        }
        if next == State::Published {
            #[cfg(unix)]
            {
                let current = identity_of(&fs::symlink_metadata(&self.record.destination)?);
                let staged = updated.staged_identity.as_ref().ok_or_else(|| {
                    invalid_data("Cannot publish a move without staged inode evidence")
                })?;
                if current.dev != staged.dev
                    || current.ino != staged.ino
                    || current.mode & libc::S_IFMT != staged.mode & libc::S_IFMT
                {
                    return Err(invalid_data(
                        "Published destination is not the recorded staged object",
                    ));
                }
                updated.destination_identity = Some(current);
            }
        }
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
                    eprintln!("RiceField: completed journal cleanup failed: {error}");
                }
            }
        }
        Ok(())
    }

    fn persist(&mut self, updated: Record) -> io::Result<()> {
        // A recovery finalizer and an active copy worker must never publish
        // divergent states from stale in-memory copies of the same journal.
        let directory = self
            .path
            .parent()
            .ok_or_else(|| invalid_input("Invalid journal path"))?;
        #[cfg(unix)]
        let _lock = lock_journal_directory(directory)?;
        let (current, _) = read_record(&self.path)?;
        if current != self.record {
            return Err(invalid_data(
                "Journal record changed since the last transition",
            ));
        }
        write_record(&self.path, &updated, false)?;
        self.record = updated;
        Ok(())
    }
}

/// Resolve only an already-published, byte-verified independent copy after
/// restart. This operation changes private journal state, never either file.
/// A user must explicitly request it from the recovery review UI.
#[cfg(unix)]
pub fn resolve_verified_copy(id: &str, cancelled: &AtomicBool) -> io::Result<()> {
    resolve_verified_copy_in(&journal_directory()?, id, cancelled)
}

#[cfg(unix)]
fn resolve_verified_copy_in(directory: &Path, id: &str, cancelled: &AtomicBool) -> io::Result<()> {
    if !valid_id(id) {
        return Err(invalid_input("Invalid recovery record ID"));
    }
    check_private_directory(&fs::symlink_metadata(directory)?)?;
    let path = directory.join(format!("{id}.toml"));
    let (record, _) = read_record(&path)?;
    verify_resolvable_publication(&record)?;
    if cancelled.load(Ordering::Relaxed) {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "Recovery verification cancelled",
        ));
    }
    let comparison = compare_for_recovery(&record.source, &record.destination, cancelled);
    if cancelled.load(Ordering::Relaxed) {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "Recovery verification cancelled",
        ));
    }
    if !matches!(comparison, Comparison::Identical { .. }) {
        return Err(invalid_data(
            "Source and destination contents are not verified identical",
        ));
    }

    // File comparisons can be expensive. Lock only the final journal update,
    // then reject any intervening writer transition or metadata drift.
    let _lock = lock_journal_directory(directory)?;
    let (current, _) = read_record(&path)?;
    if current != record {
        return Err(invalid_data("Recovery record changed during verification"));
    }
    verify_resolvable_publication(&current)?;
    if cancelled.load(Ordering::Relaxed) {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "Recovery verification cancelled",
        ));
    }

    let mut completed = current.clone();
    if current.state == State::StagedVerified {
        let published_identity = identity_of(&fs::symlink_metadata(&current.destination)?);
        completed.state = State::Published;
        completed.destination_identity = Some(published_identity);
        write_record(&path, &completed, false)?;
    }
    completed.state = State::Completed;
    write_record(&path, &completed, false)?;
    // Same retirement semantics as ordinary successful copies: a crash or
    // failed unlink leaves only a harmless terminal record, not lost files.
    if let Err(error) = fs::remove_file(&path).and_then(|()| File::open(directory)?.sync_all()) {
        eprintln!("RiceField: resolved journal retirement failed: {error}");
    }
    Ok(())
}

#[cfg(unix)]
fn verify_resolvable_publication(record: &Record) -> io::Result<()> {
    // Directory root identities do not cover metadata or external hardlinks
    // of nested entries. Keep their journals available for manual inspection.
    let source_meta = fs::symlink_metadata(&record.source)?;
    if source_meta.is_dir() || !(source_meta.is_file() || source_meta.file_type().is_symlink()) {
        return Err(invalid_data(
            "Directory and special-file recovery require manual inspection",
        ));
    }
    let destination = identity_of(&fs::symlink_metadata(&record.destination)?);
    let staged = record
        .staged_identity
        .as_ref()
        .ok_or_else(|| invalid_data("Missing staged publication identity"))?;
    // Rename may update ctime, but verified content length, permissions,
    // ownership and modification time must remain unchanged from staging.
    if destination.mode != staged.mode
        || destination.uid != staged.uid
        || destination.gid != staged.gid
        || destination.len != staged.len
        || destination.mtime != staged.mtime
        || destination.mtime_nsec != staged.mtime_nsec
    {
        return Err(invalid_data(
            "Published file metadata differs from verified staging",
        ));
    }
    if !matches!(record.state, State::Published | State::StagedVerified)
        || !matches!(
            inspect_identity(&record.source, record.source_identity.as_ref()),
            IdentityStatus::SameSnapshot
        )
        || !matches!(inspect_publication(record), PublicationEvidence::SameInode)
        || (record.state == State::Published
            && !matches!(
                inspect_identity(&record.destination, record.destination_identity.as_ref()),
                IdentityStatus::SameSnapshot
            ))
    {
        return Err(invalid_data(
            "Publication cannot be safely resolved from recorded evidence",
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn lock_journal_directory(directory: &Path) -> io::Result<File> {
    use std::os::{fd::AsRawFd, unix::fs::OpenOptionsExt};

    let mut options = OpenOptions::new();
    options
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW);
    let file = options.open(directory)?;
    check_private_directory(&file.metadata()?)?;
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(file)
}

/// Reserve room for another move while holding the directory lock. Reclaim
/// only fully validated Completed records if the capacity would otherwise be
/// exhausted; malformed, changed and unfinished records are never purged.
fn ensure_capacity(directory: &Path, limit: usize) -> io::Result<()> {
    let mut paths = Vec::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        if entry
            .path()
            .extension()
            .is_some_and(|extension| extension == "toml")
        {
            paths.push(entry.path());
            if paths.len() > MAX_RECORDS {
                return Err(invalid_data("Too many operation journal records"));
            }
        }
    }
    if paths.len() >= limit {
        let mut reclaimed = false;
        for path in paths.iter() {
            // Invalid or concurrently changing records must be retained.
            let Ok((record, snapshot)) = read_record(path) else {
                continue;
            };
            if record.state != State::Completed {
                continue;
            }
            if fs::symlink_metadata(path).is_ok_and(|current| same_snapshot(&snapshot, &current)) {
                fs::remove_file(path)?;
                reclaimed = true;
            }
        }
        if reclaimed {
            File::open(directory)?.sync_all()?;
        }
        // Recount under the same lock: only terminal records are eligible
        // for removal, and any remaining records still consume scan slots.
        let remaining = fs::read_dir(directory)?.try_fold(0usize, |count, entry| {
            let path = entry?.path();
            Ok::<_, io::Error>(
                count + usize::from(path.extension().is_some_and(|ext| ext == "toml")),
            )
        })?;
        if remaining >= limit {
            return Err(io::Error::other(
                "Move journal capacity reached; inspect existing recovery records before starting another cross-device Move",
            ));
        }
    }
    Ok(())
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
        let (record, _) = read_record(&path)?;
        if record.state != State::Completed {
            records.push(record);
        }
    }
    records.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(records)
}

/// The same bounded, no-follow validation is used for recovery inspection and
/// terminal-record retirement. A filename or a `state` string alone never
/// authorizes cleanup of journal evidence.
fn read_record(path: &Path) -> io::Result<(Record, fs::Metadata)> {
    let metadata = fs::symlink_metadata(path)?;
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
    // Pin the regular inode observed by lstat. A record replaced by a symlink
    // or FIFO must not redirect or stall the recovery scanner.
    let mut file = open_regular_no_follow(path, &metadata)?;
    let mut content = Vec::new();
    (&mut file)
        .take(MAX_RECORD_BYTES + 1)
        .read_to_end(&mut content)?;
    if content.len() as u64 > MAX_RECORD_BYTES {
        return Err(invalid_data("Operation journal record is oversized"));
    }
    if !same_snapshot(&metadata, &file.metadata()?)
        || !same_snapshot(&metadata, &fs::symlink_metadata(path)?)
    {
        return Err(invalid_data("Operation journal changed while being read"));
    }
    let stored: StoredRecord = toml::from_str(
        std::str::from_utf8(&content).map_err(|_| invalid_data("Invalid journal UTF-8"))?,
    )
    .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    let record = from_stored(stored)?;
    if path.file_stem().and_then(|stem| stem.to_str()) != Some(record.id.as_str()) {
        return Err(invalid_data("Journal ID and filename disagree"));
    }
    Ok((record, metadata))
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
    Ok(state_home.join("ricefield/operations"))
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
        eprintln!("RiceField: journal temp cleanup after publication failed: {error}");
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
        source_identity: record.source_identity.clone(),
        staged_identity: record.staged_identity.clone(),
        destination_identity: record.destination_identity.clone(),
    })
}

fn from_stored(stored: StoredRecord) -> io::Result<Record> {
    if !matches!(stored.version, 1 | 2 | VERSION) || !valid_id(&stored.id) {
        return Err(invalid_data("Unsupported or invalid journal record"));
    }
    if stored.version >= 2 {
        #[cfg(unix)]
        if stored.source_identity.is_none()
            || (matches!(stored.state, State::Published | State::Completed)
                && stored.destination_identity.is_none())
            || (stored.version >= 3
                && stored.state != State::Prepared
                && stored.staged_identity.is_none())
        {
            return Err(invalid_data("Journal identity evidence is missing"));
        }
    }
    for evidence in [
        stored.source_identity.as_ref(),
        stored.staged_identity.as_ref(),
        stored.destination_identity.as_ref(),
    ]
    .into_iter()
    .flatten()
    {
        if !(0..1_000_000_000).contains(&evidence.mtime_nsec)
            || !(0..1_000_000_000).contains(&evidence.ctime_nsec)
        {
            return Err(invalid_data("Journal identity timestamp is invalid"));
        }
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
        source_identity: stored.source_identity,
        staged_identity: stored.staged_identity,
        destination_identity: stored.destination_identity,
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
        let path = env::temp_dir().join(format!("ricefield-journal-{}", next_id()));
        fs::create_dir(&path).unwrap();
        path
    }

    fn publish_test_staging(journal: &mut Journal, destination: &Path, bytes: &[u8]) {
        let staging = journal.record().staging.as_ref().unwrap();
        fs::create_dir(staging).unwrap();
        let payload = staging.join("payload");
        fs::write(&payload, bytes).unwrap();
        journal.advance(State::StagedVerified).unwrap();
        fs::rename(payload, destination).unwrap();
        journal.advance(State::Published).unwrap();
    }

    #[test]
    fn states_are_durable_and_scan_ignores_completed() {
        let directory = test_directory();
        let source = directory.join("from");
        let destination = directory.join("to");
        fs::write(&source, b"original").unwrap();
        let mut journal = Journal::begin_in(&directory, &source, &destination).unwrap();
        assert_eq!(journal.record().state, State::Prepared);
        assert!(journal.record().source_identity.is_some());
        assert_eq!(scan_incomplete_in(&directory).unwrap().len(), 1);

        journal.set_staging(&directory.join("stage")).unwrap();
        publish_test_staging(&mut journal, &destination, b"original");
        let incomplete = scan_incomplete_in(&directory).unwrap();
        assert_eq!(incomplete.len(), 1);
        assert_eq!(incomplete[0].state, State::Published);
        assert_eq!(incomplete[0].source, source);
        assert_eq!(incomplete[0].destination, destination);
        assert_eq!(incomplete[0].staging, Some(directory.join("stage")));
        assert!(incomplete[0].destination_identity.is_some());
        assert!(incomplete[0].staged_identity.is_some());
        assert_eq!(
            inspect_record(incomplete[0].clone()).publication_evidence,
            PublicationEvidence::SameInode
        );
        journal.advance(State::Completed).unwrap();
        assert!(scan_incomplete_in(&directory).unwrap().is_empty());
        assert!(
            !directory
                .join(format!("{}.toml", journal.record().id))
                .exists()
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn recovery_can_retire_a_verified_published_copy_without_removing_files() {
        let directory = test_directory();
        let source = directory.join("source");
        let destination = directory.join("destination");
        fs::write(&source, b"unchanged content").unwrap();
        let mut journal = Journal::begin_in(&directory, &source, &destination).unwrap();
        journal.set_staging(&directory.join("stage")).unwrap();
        publish_test_staging(&mut journal, &destination, b"unchanged content");
        let id = journal.record().id.clone();

        resolve_verified_copy_in(&directory, &id, &AtomicBool::new(false)).unwrap();
        assert!(scan_incomplete_in(&directory).unwrap().is_empty());
        assert_eq!(fs::read(&source).unwrap(), b"unchanged content");
        assert_eq!(fs::read(&destination).unwrap(), b"unchanged content");
        assert!(!directory.join(format!("{id}.toml")).exists());
        // An old writer handle cannot resurrect the retired journal.
        assert!(journal.advance(State::Completed).is_err());
        assert!(!directory.join(format!("{id}.toml")).exists());
        fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn recovery_recognizes_publish_before_state_update_but_not_a_collision() {
        let directory = test_directory();
        let source = directory.join("source");
        let destination = directory.join("destination");
        let staging = directory.join("stage");
        fs::write(&source, b"original").unwrap();
        let mut journal = Journal::begin_in(&directory, &source, &destination).unwrap();
        journal.set_staging(&staging).unwrap();
        fs::create_dir(&staging).unwrap();
        fs::write(staging.join("payload"), b"original").unwrap();
        journal.advance(State::StagedVerified).unwrap();
        fs::rename(staging.join("payload"), &destination).unwrap();
        let id = journal.record().id.clone();
        resolve_verified_copy_in(&directory, &id, &AtomicBool::new(false)).unwrap();
        assert_eq!(fs::read(&source).unwrap(), b"original");
        assert_eq!(fs::read(&destination).unwrap(), b"original");

        let destination2 = directory.join("unrelated");
        let mut second = Journal::begin_in(&directory, &source, &destination2).unwrap();
        let staging2 = directory.join("stage2");
        second.set_staging(&staging2).unwrap();
        fs::create_dir(&staging2).unwrap();
        fs::write(staging2.join("payload"), b"original").unwrap();
        second.advance(State::StagedVerified).unwrap();
        fs::write(&destination2, b"original").unwrap();
        let second_id = second.record().id.clone();
        assert!(resolve_verified_copy_in(&directory, &second_id, &AtomicBool::new(false)).is_err());
        assert_eq!(scan_incomplete_in(&directory).unwrap().len(), 1);
        assert_eq!(fs::read(destination2).unwrap(), b"original");
        fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn recovery_refuses_modified_source_and_different_destination_bytes() {
        let directory = test_directory();
        let source = directory.join("source");
        let destination = directory.join("destination");
        fs::write(&source, b"original").unwrap();
        let mut journal = Journal::begin_in(&directory, &source, &destination).unwrap();
        journal.set_staging(&directory.join("stage")).unwrap();
        publish_test_staging(&mut journal, &destination, b"original");
        let id = journal.record().id.clone();
        fs::write(&destination, b"changed!").unwrap();
        assert!(resolve_verified_copy_in(&directory, &id, &AtomicBool::new(false)).is_err());
        fs::write(&destination, b"original").unwrap();
        fs::write(&source, b"changed!").unwrap();
        assert!(resolve_verified_copy_in(&directory, &id, &AtomicBool::new(false)).is_err());
        assert_eq!(scan_incomplete_in(&directory).unwrap().len(), 1);
        fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn cancelled_recovery_keeps_the_journal_and_both_files() {
        let directory = test_directory();
        let source = directory.join("source");
        let destination = directory.join("destination");
        fs::write(&source, b"safe copy").unwrap();
        let mut journal = Journal::begin_in(&directory, &source, &destination).unwrap();
        journal.set_staging(&directory.join("stage")).unwrap();
        publish_test_staging(&mut journal, &destination, b"safe copy");
        let cancelled = AtomicBool::new(true);
        assert_eq!(
            resolve_verified_copy_in(&directory, &journal.record().id, &cancelled)
                .unwrap_err()
                .kind(),
            io::ErrorKind::Interrupted
        );
        assert_eq!(scan_incomplete_in(&directory).unwrap().len(), 1);
        assert_eq!(fs::read(source).unwrap(), b"safe copy");
        assert_eq!(fs::read(destination).unwrap(), b"safe copy");
        fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn directory_recovery_keeps_evidence_for_nested_metadata_review() {
        let directory = test_directory();
        let source = directory.join("source");
        let destination = directory.join("destination");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("child"), b"contents").unwrap();
        let mut journal = Journal::begin_in(&directory, &source, &destination).unwrap();
        let staging = directory.join("stage");
        journal.set_staging(&staging).unwrap();
        fs::create_dir(&staging).unwrap();
        fs::create_dir(staging.join("payload")).unwrap();
        fs::write(staging.join("payload/child"), b"contents").unwrap();
        journal.advance(State::StagedVerified).unwrap();
        fs::rename(staging.join("payload"), &destination).unwrap();
        journal.advance(State::Published).unwrap();
        assert!(
            resolve_verified_copy_in(&directory, &journal.record().id, &AtomicBool::new(false))
                .is_err()
        );
        assert_eq!(scan_incomplete_in(&directory).unwrap().len(), 1);
        assert_eq!(fs::read(destination.join("child")).unwrap(), b"contents");
        fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn recovery_identity_detects_same_inode_drift_and_replaced_source() {
        let directory = test_directory();
        let source = directory.join("original");
        let destination = directory.join("published");
        fs::write(&source, b"same bytes").unwrap();
        let mut journal = Journal::begin_in(&directory, &source, &destination).unwrap();
        journal.set_staging(&directory.join("stage")).unwrap();
        publish_test_staging(&mut journal, &destination, b"same bytes");

        let record = scan_incomplete_in(&directory).unwrap().remove(0);
        let first = inspect_record(record.clone());
        assert_eq!(first.source_identity, IdentityStatus::SameSnapshot);
        assert_eq!(first.destination_identity, IdentityStatus::SameSnapshot);
        assert!(first.guidance().contains("recorded inodes"));

        // A same-sized mutation must not be confused with the captured state.
        fs::write(&destination, b"other data").unwrap();
        let changed = inspect_record(record.clone());
        assert_eq!(changed.destination_identity, IdentityStatus::Changed);
        assert!(changed.guidance().contains("changed metadata"));

        // Keep the old inode open to prevent immediate reuse of its number.
        let old_inode = File::open(&source).unwrap();
        fs::remove_file(&source).unwrap();
        fs::write(&source, b"same bytes").unwrap();
        let replaced = inspect_record(record);
        assert_eq!(replaced.source_identity, IdentityStatus::Replaced);
        assert!(replaced.guidance().contains("different filesystem object"));
        drop(old_inode);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn legacy_records_remain_inspectable_but_new_records_require_evidence() {
        let directory = test_directory();
        let source = directory.join("original");
        fs::write(&source, b"original").unwrap();
        let journal = Journal::begin_in(&directory, &source, &directory.join("target")).unwrap();
        let path = directory.join(format!("{}.toml", journal.record().id));
        let mut stored = to_stored(journal.record()).unwrap();
        stored.version = 1;
        stored.source_identity = None;
        fs::write(&path, toml::to_string(&stored).unwrap()).unwrap();
        let record = scan_incomplete_in(&directory).unwrap().remove(0);
        assert_eq!(
            inspect_record(record).source_identity,
            IdentityStatus::NotRecorded
        );

        // A persisted v2 record predates staged inode provenance but retains
        // source/destination evidence. Keep it readable after the v3 upgrade.
        stored.version = 2;
        stored.source_identity = journal.record().source_identity.clone();
        fs::write(&path, toml::to_string(&stored).unwrap()).unwrap();
        assert_eq!(scan_incomplete_in(&directory).unwrap().len(), 1);

        stored.version = VERSION;
        stored.source_identity = None;
        fs::write(&path, toml::to_string(&stored).unwrap()).unwrap();
        #[cfg(unix)]
        assert_eq!(
            scan_incomplete_in(&directory).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn publication_refuses_destination_not_derived_from_verified_staging() {
        let directory = test_directory();
        let source = directory.join("source");
        let destination = directory.join("destination");
        let stage = directory.join("stage");
        fs::write(&source, b"source bytes").unwrap();
        let mut journal = Journal::begin_in(&directory, &source, &destination).unwrap();
        fs::create_dir(&stage).unwrap();
        journal.set_staging(&stage).unwrap();
        fs::write(stage.join("payload"), b"source bytes").unwrap();
        journal.advance(State::StagedVerified).unwrap();
        // The same bytes written under another inode cannot masquerade as
        // the already verified object, even if the journal state is stale.
        fs::write(&destination, b"source bytes").unwrap();
        assert_eq!(
            journal.advance(State::Published).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        let record = scan_incomplete_in(&directory).unwrap().remove(0);
        assert_eq!(record.state, State::StagedVerified);
        assert_eq!(
            inspect_record(record).publication_evidence,
            PublicationEvidence::DifferentObject
        );
        assert_eq!(fs::read(source).unwrap(), b"source bytes");
        assert_eq!(fs::read(destination).unwrap(), b"source bytes");
        fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn older_published_records_remain_readable_but_cannot_claim_v3_provenance() {
        let directory = test_directory();
        let source = directory.join("source");
        let destination = directory.join("destination");
        fs::write(&source, b"source bytes").unwrap();
        let mut journal = Journal::begin_in(&directory, &source, &destination).unwrap();
        journal.set_staging(&directory.join("stage")).unwrap();
        publish_test_staging(&mut journal, &destination, b"source bytes");

        let mut stored = to_stored(journal.record()).unwrap();
        stored.staged_identity = None;
        fs::write(&journal.path, toml::to_string(&stored).unwrap()).unwrap();
        assert_eq!(
            scan_incomplete_in(&directory).unwrap_err().kind(),
            io::ErrorKind::InvalidData,
            "v3 Published must retain staged provenance"
        );
        stored.version = 2;
        fs::write(&journal.path, toml::to_string(&stored).unwrap()).unwrap();
        let legacy = scan_incomplete_in(&directory).unwrap().remove(0);
        let inspection = inspect_record(legacy);
        assert_eq!(
            inspection.publication_evidence,
            PublicationEvidence::NotRecorded
        );
        assert_eq!(
            inspection.destination_identity,
            IdentityStatus::SameSnapshot
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn journal_capacity_refuses_new_moves_without_purging_records() {
        let directory = test_directory();
        fs::write(directory.join("a.toml"), b"test").unwrap();
        fs::write(directory.join("not-a-record.tmp"), b"ignored").unwrap();
        ensure_capacity(&directory, 2).unwrap();
        fs::write(directory.join("b.toml"), b"test").unwrap();
        assert!(
            ensure_capacity(&directory, 2)
                .unwrap_err()
                .to_string()
                .contains("capacity reached")
        );
        assert!(directory.join("a.toml").exists());
        assert!(directory.join("b.toml").exists());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn capacity_reclaims_only_valid_completed_records() {
        let directory = test_directory();
        let original = directory.join("original");
        let published = directory.join("published");
        fs::write(&original, b"source").unwrap();
        let mut complete = Journal::begin_in(&directory, &original, &published).unwrap();
        complete.set_staging(&directory.join("staging")).unwrap();
        publish_test_staging(&mut complete, &published, b"source");

        // Simulate a crash after Completed is durable but before the
        // normally immediate unlink of the journal record.
        let mut completed = complete.record().clone();
        completed.state = State::Completed;
        write_record(&complete.path, &completed, false).unwrap();

        let other = directory.join("another-source");
        fs::write(&other, b"source").unwrap();
        let next =
            Journal::begin_in_with_limit(&directory, &other, &directory.join("next"), 1).unwrap();
        assert!(!complete.path.exists());
        assert!(next.path.exists());
        assert_eq!(scan_incomplete_in(&directory).unwrap().len(), 1);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn capacity_preserves_incomplete_and_malformed_evidence() {
        let directory = test_directory();
        let original = directory.join("original");
        let published = directory.join("published");
        fs::write(&original, b"source").unwrap();
        let mut journal = Journal::begin_in(&directory, &original, &published).unwrap();
        journal.set_staging(&directory.join("staging")).unwrap();
        publish_test_staging(&mut journal, &published, b"source");

        let invalid = directory.join("unexpected.toml");
        let mut masquerading = journal.record().clone();
        masquerading.state = State::Completed;
        // A Completed state without a matching filename is not enough to
        // remove untrusted evidence from the journal.
        fs::write(
            &invalid,
            toml::to_string(&to_stored(&masquerading).unwrap()).unwrap(),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&invalid, fs::Permissions::from_mode(0o600)).unwrap();
        }
        let next_source = directory.join("next-source");
        fs::write(&next_source, b"next").unwrap();
        let error = Journal::begin_in_with_limit(
            &directory,
            &next_source,
            &directory.join("next-target"),
            2,
        )
        .err()
        .unwrap();
        assert!(error.to_string().contains("capacity reached"));
        assert!(journal.path.exists());
        assert!(invalid.exists());
        fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn concurrent_writers_cannot_reserve_the_same_last_journal_slot() {
        use std::sync::{Arc, Barrier};

        let directory = test_directory();
        for index in 0..2 {
            fs::write(directory.join(format!("source-{index}")), b"source").unwrap();
        }
        let barrier = Arc::new(Barrier::new(3));
        let threads: Vec<_> = (0..2)
            .map(|index| {
                let directory = directory.clone();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    Journal::begin_in_with_limit(
                        &directory,
                        &directory.join(format!("source-{index}")),
                        &directory.join(format!("destination-{index}")),
                        1,
                    )
                    .map(|journal| journal.record().id.clone())
                })
            })
            .collect();
        barrier.wait();
        let results: Vec<_> = threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect();
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|result| result
                    .as_ref()
                    .is_err_and(|error| { error.to_string().contains("capacity reached") }))
                .count(),
            1
        );
        assert_eq!(scan_incomplete_in(&directory).unwrap().len(), 1);
        fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn restart_inspection_never_infers_publication_from_a_staged_record() {
        let directory = test_directory();
        let source = directory.join("source");
        let destination = directory.join("published");
        fs::write(&source, b"source bytes").unwrap();
        let mut journal = Journal::begin_in(&directory, &source, &destination).unwrap();
        journal.set_staging(&directory.join("staging")).unwrap();
        fs::create_dir(directory.join("staging")).unwrap();
        fs::write(directory.join("staging/payload"), b"source bytes").unwrap();
        journal.advance(State::StagedVerified).unwrap();
        // Simulate a process dying after atomic publication but before it
        // could durably advance the journal to Published.
        fs::rename(directory.join("staging/payload"), &destination).unwrap();
        drop(journal);

        let restored = scan_incomplete_in(&directory).unwrap().remove(0);
        assert_eq!(restored.state, State::StagedVerified);
        assert!(restored.destination_identity.is_none());
        let inspection = inspect_record(restored);
        assert_eq!(
            inspection.publication_evidence,
            PublicationEvidence::SameInode
        );
        assert_eq!(inspection.source_identity, IdentityStatus::SameSnapshot);
        assert_eq!(inspection.destination_identity, IdentityStatus::NotRecorded);
        assert!(
            inspection
                .guidance()
                .contains("neither is removed automatically")
        );
        assert!(source.exists());
        assert!(destination.exists());
        fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn abrupt_process_exit_preserves_each_durable_move_checkpoint() {
        const PHASE: &str = "RICEFIELD_TEST_CRASH_PHASE";
        const ROOT: &str = "RICEFIELD_TEST_CRASH_ROOT";
        if let Ok(phase) = env::var(PHASE) {
            let directory = PathBuf::from(env::var_os(ROOT).expect("Fixture path provided"));
            let source = directory.join("source");
            let destination = directory.join("destination");
            let stage = directory.join("stage");
            let mut journal = Journal::begin_in(&directory, &source, &destination).unwrap();
            if phase != "prepared" {
                journal.set_staging(&stage).unwrap();
                fs::create_dir(&stage).unwrap();
                fs::write(stage.join("payload"), b"source bytes").unwrap();
                File::open(stage.join("payload"))
                    .unwrap()
                    .sync_all()
                    .unwrap();
                File::open(&stage).unwrap().sync_all().unwrap();
                journal.advance(State::StagedVerified).unwrap();
            }
            if matches!(phase.as_str(), "renamed" | "published") {
                fs::rename(stage.join("payload"), &destination).unwrap();
                File::open(&directory).unwrap().sync_all().unwrap();
            }
            if phase == "published" {
                journal.advance(State::Published).unwrap();
            }
            // Bypass Drop and the test harness just as an abruptly terminated
            // file manager would. This is a process-crash test, not a simulated
            // kernel power failure or a permission to replay the operation.
            std::process::exit(47);
        }

        for (phase, expected_state, published) in [
            ("prepared", State::Prepared, false),
            ("staged", State::StagedVerified, false),
            ("renamed", State::StagedVerified, true),
            ("published", State::Published, true),
        ] {
            let directory = test_directory();
            let source = directory.join("source");
            let destination = directory.join("destination");
            fs::write(&source, b"source bytes").unwrap();
            let status = std::process::Command::new(env::current_exe().unwrap())
                .arg("journal::tests::abrupt_process_exit_preserves_each_durable_move_checkpoint")
                .arg("--exact")
                .env(PHASE, phase)
                .env(ROOT, &directory)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .unwrap();
            assert_eq!(status.code(), Some(47), "crash child did not run: {phase}");

            let records = scan_incomplete_in(&directory).unwrap();
            assert_eq!(records.len(), 1, "missing recovery evidence: {phase}");
            assert_eq!(records[0].state, expected_state, "phase: {phase}");
            let inspection = inspect_record(records[0].clone());
            assert_eq!(inspection.source_identity, IdentityStatus::SameSnapshot);
            assert_eq!(inspection.destination.present(), published);
            if published {
                assert_eq!(fs::read(&destination).unwrap(), b"source bytes");
                assert_eq!(
                    inspection.publication_evidence,
                    PublicationEvidence::SameInode
                );
            } else if phase == "staged" {
                assert_eq!(
                    inspection.publication_evidence,
                    PublicationEvidence::DestinationMissing
                );
            }
            assert_eq!(fs::read(&source).unwrap(), b"source bytes");
            fs::remove_dir_all(directory).unwrap();
        }
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
            source_identity: None,
            staged_identity: None,
            destination_identity: None,
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
                source_identity: None,
                staged_identity: None,
                destination_identity: None,
            },
            source: PathStatus::Missing,
            destination: PathStatus::Missing,
            staging: None,
            source_identity: IdentityStatus::NotRecorded,
            destination_identity: IdentityStatus::NotRecorded,
            publication_evidence: PublicationEvidence::NotRecorded,
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

    #[test]
    fn recovery_identity_labels_distinguish_evidence_without_authorizing_cleanup() {
        assert!(IdentityStatus::NotRecorded.label().contains("No saved"));
        assert!(IdentityStatus::Missing.label().contains("missing"));
        assert!(
            IdentityStatus::Unreadable("mount offline".into())
                .label()
                .contains("mount offline")
        );
        assert!(
            IdentityStatus::Changed
                .label()
                .contains("metadata has changed")
        );
        assert!(IdentityStatus::Replaced.label().contains("Different inode"));
        let matched = IdentityStatus::SameSnapshot.label();
        assert!(matched.contains("contents not proven"));
        assert!(!matched.contains("safe to delete"));
    }

    #[test]
    fn recovery_fresh_copy_offer_requires_visible_source_and_unchanged_identity() {
        let mut inspection = Inspection {
            record: Record {
                id: "copy-check".into(),
                state: State::Published,
                source: PathBuf::from("/source"),
                destination: PathBuf::from("/target"),
                staging: None,
                source_identity: None,
                staged_identity: None,
                destination_identity: None,
            },
            source: PathStatus::File,
            destination: PathStatus::File,
            staging: None,
            source_identity: IdentityStatus::SameSnapshot,
            destination_identity: IdentityStatus::Changed,
            publication_evidence: PublicationEvidence::NotRecorded,
        };
        assert!(inspection.can_offer_fresh_copy());
        inspection.source = PathStatus::Directory;
        assert!(inspection.can_offer_fresh_copy());
        inspection.source = PathStatus::Symlink;
        assert!(inspection.can_offer_fresh_copy());
        inspection.source = PathStatus::Missing;
        assert!(!inspection.can_offer_fresh_copy());
        inspection.source = PathStatus::File;
        for identity in [
            IdentityStatus::Changed,
            IdentityStatus::Replaced,
            IdentityStatus::NotRecorded,
            IdentityStatus::Unreadable("offline".into()),
            IdentityStatus::Missing,
        ] {
            inspection.source_identity = identity;
            assert!(!inspection.can_offer_fresh_copy());
        }
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
            source_identity: None,
            staged_identity: None,
            destination_identity: None,
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
    fn recovery_comparison_checks_nested_bytes_and_extra_entries() {
        let root = test_directory();
        let source = root.join("original");
        let destination = root.join("published");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&destination).unwrap();
        fs::create_dir(source.join("nested")).unwrap();
        fs::create_dir(destination.join("nested")).unwrap();
        fs::write(source.join("nested/data"), b"abcdef").unwrap();
        fs::write(destination.join("nested/data"), b"abcdef").unwrap();
        let cancelled = AtomicBool::new(false);
        assert_eq!(
            compare_for_recovery(&source, &destination, &cancelled),
            Comparison::Identical {
                entries: 3,
                bytes: 6
            }
        );
        fs::write(destination.join("nested/data"), b"abcdeg").unwrap();
        assert!(matches!(
            compare_for_recovery(&source, &destination, &cancelled),
            Comparison::Different(reason) if reason.contains("file bytes")
        ));
        fs::write(destination.join("nested/data"), b"abcdef").unwrap();
        fs::write(destination.join("extra"), b"unexpected").unwrap();
        assert!(matches!(
            compare_for_recovery(&source, &destination, &cancelled),
            Comparison::Different(reason) if reason.contains("directory entries")
        ));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn recovery_comparison_never_treats_missing_or_cancelled_as_match() {
        let root = test_directory();
        let source = root.join("original");
        let destination = root.join("missing");
        fs::write(&source, b"hello").unwrap();
        let cancelled = AtomicBool::new(false);
        assert!(matches!(
            compare_for_recovery(&source, &destination, &cancelled),
            Comparison::Inconclusive(_)
        ));
        cancelled.store(true, Ordering::Relaxed);
        assert!(matches!(
            compare_for_recovery(&source, &source, &cancelled),
            Comparison::Inconclusive(reason) if reason.contains("cancelled")
        ));
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn recovery_comparison_does_not_follow_symlinked_file() {
        use std::os::unix::fs::symlink;
        let root = test_directory();
        let original = root.join("original");
        let published = root.join("published");
        symlink("safe-target", &original).unwrap();
        symlink("safe-target", &published).unwrap();
        let cancelled = AtomicBool::new(false);
        assert_eq!(
            compare_for_recovery(&original, &published, &cancelled),
            Comparison::Identical {
                entries: 1,
                bytes: 0
            }
        );
        fs::remove_file(&published).unwrap();
        symlink("different-target", &published).unwrap();
        assert!(matches!(
            compare_for_recovery(&original, &published, &cancelled),
            Comparison::Different(reason) if reason.contains("symbolic-link")
        ));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn recovery_comparison_rejects_large_files_without_reading_them() {
        let root = test_directory();
        let source = root.join("original");
        let destination = root.join("published");
        let file = File::create(&source).unwrap();
        file.set_len(MAX_COMPARE_BYTES + 1).unwrap();
        let file = File::create(&destination).unwrap();
        file.set_len(MAX_COMPARE_BYTES + 1).unwrap();
        assert!(matches!(
            compare_for_recovery(&source, &destination, &AtomicBool::new(false)),
            Comparison::Inconclusive(reason) if reason.contains("512 MiB")
        ));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn invalid_transitions_do_not_change_disk() {
        let directory = test_directory();
        let source = directory.join("from");
        fs::write(&source, b"from").unwrap();
        let mut journal = Journal::begin_in(&directory, &source, &directory.join("to")).unwrap();
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
        fs::write(&source, b"non utf8 source").unwrap();
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
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 2);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn corrupted_and_oversized_records_fail_closed() {
        let directory = test_directory();
        let source = directory.join("from");
        fs::write(&source, b"from").unwrap();
        let journal = Journal::begin_in(&directory, &source, &directory.join("to")).unwrap();
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
    fn journal_reader_rejects_replaced_symlink_and_fifo_without_following() {
        use std::os::unix::fs::symlink;

        let directory = test_directory();
        let source = directory.join("from");
        fs::write(&source, b"from").unwrap();
        let journal = Journal::begin_in(&directory, &source, &directory.join("to")).unwrap();
        let path = directory.join(format!("{}.toml", journal.record().id));
        let snapshot = fs::symlink_metadata(&path).unwrap();
        let sensitive = directory.join("unrelated");
        fs::write(&sensitive, "should not be read").unwrap();
        fs::remove_file(&path).unwrap();
        symlink(&sensitive, &path).unwrap();
        assert!(open_regular_no_follow(&path, &snapshot).is_err());
        assert!(scan_incomplete_in(&directory).is_err());

        fs::remove_file(&path).unwrap();
        use std::os::unix::ffi::OsStrExt;
        let name = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        assert!(open_regular_no_follow(&path, &snapshot).is_err());
        assert!(scan_incomplete_in(&directory).is_err());
        assert_eq!(fs::read_to_string(sensitive).unwrap(), "should not be read");
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
