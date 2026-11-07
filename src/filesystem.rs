use crate::journal::{Comparison, Journal, State, compare_for_recovery};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::{OsStr, OsString},
    fs::{self, File, FileTimes, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant, SystemTime},
};
#[cfg(target_os = "linux")]
use std::{ffi::CString, os::unix::ffi::OsStrExt};

/// A snapshot of a file operation; callbacks run on the worker thread.
#[derive(Clone, Debug)]
pub struct Progress {
    pub transferred: u64,
    pub total: u64,
    pub current: PathBuf,
    pub phase: &'static str,
}

/// Immutable per-source outcomes; unattempted items are kept visible after failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemState {
    NotStarted,
    Completed,
    /// A cross-filesystem cut was copied and verified, but the source is
    /// intentionally untouched until the user removes it separately.
    CopiedSourceRetained,
    Skipped,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug)]
pub struct ItemResult {
    pub source: PathBuf,
    pub target: Option<PathBuf>,
    pub state: ItemState,
    pub detail: Option<String>,
}

pub struct OperationReport {
    pub result: io::Result<String>,
    pub items: Vec<ItemResult>,
    pub rename_undo: Option<Box<RenameUndo>>,
    pub batch_rename_undo: Option<Box<BatchRenameUndo>>,
}

/// Session-only rename reversal. The original name is preserved as raw OS
/// bytes, and the moved entry must still match its post-rename snapshot.
/// It never authorizes an overwrite, content removal or crash replay.
#[derive(Clone)]
pub struct RenameUndo {
    pub current: PathBuf,
    pub original: PathBuf,
    pub snapshot: fs::Metadata,
}

impl RenameUndo {
    pub fn action(&self) -> Action {
        Action::UndoRename {
            current: self.current.clone(),
            original: self.original.clone(),
            snapshot: self.snapshot.clone(),
        }
    }
}

/// Session-only reversal of an entire successfully completed batch. Every
/// current name retains the post-operation metadata snapshot, so an external
/// replacement or content change blocks the whole Undo before any renames.
#[derive(Clone)]
pub struct BatchRenameUndo {
    pub items: Vec<(PathBuf, OsString, fs::Metadata)>,
}

impl BatchRenameUndo {
    pub fn action(&self) -> Action {
        Action::UndoBatchRename {
            items: self.items.clone(),
        }
    }
}

struct ItemUpdate {
    index: usize,
    target: Option<PathBuf>,
    state: ItemState,
    detail: Option<String>,
}

impl ItemUpdate {
    fn new(index: usize, state: ItemState, target: Option<PathBuf>) -> Self {
        Self {
            index,
            target,
            state,
            detail: None,
        }
    }

    fn error(index: usize, target: Option<PathBuf>, error: &io::Error) -> Self {
        Self {
            index,
            target,
            state: if error.kind() == io::ErrorKind::Interrupted {
                ItemState::Cancelled
            } else {
                ItemState::Failed
            },
            detail: Some(error.to_string()),
        }
    }
}

struct Transfer<'a> {
    cancelled: &'a AtomicBool,
    notify: &'a mut dyn FnMut(Progress),
    transferred: u64,
    total: u64,
    last_report: Instant,
    last_reported_bytes: u64,
    phase: &'static str,
}

impl Transfer<'_> {
    fn check(&self) -> io::Result<()> {
        if self.cancelled.load(Ordering::Relaxed) {
            Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "Operation cancelled",
            ))
        } else {
            Ok(())
        }
    }

    fn advance(&mut self, bytes: u64, path: &Path) {
        self.transferred = self.transferred.saturating_add(bytes);
        if self.last_report.elapsed() >= Duration::from_millis(80)
            || self.transferred.saturating_sub(self.last_reported_bytes) >= 4 * 1024 * 1024
            || self.transferred >= self.total
        {
            (self.notify)(Progress {
                transferred: self.transferred,
                total: self.total,
                current: path.to_path_buf(),
                phase: self.phase,
            });
            self.last_report = Instant::now();
            self.last_reported_bytes = self.transferred;
        }
    }

    fn report(&mut self, path: &Path) {
        (self.notify)(Progress {
            transferred: self.transferred,
            total: self.total,
            current: path.to_path_buf(),
            phase: self.phase,
        });
    }
}

#[derive(Clone)]
pub struct Entry {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
}

impl Entry {
    pub fn kind(&self) -> &'static str {
        if self.is_symlink {
            "Link"
        } else if self.is_dir {
            "Folder"
        } else {
            file_kind_from_name(&self.name)
        }
    }
}

/// Shared local/remote file badges; classification never opens an untrusted file.
pub fn file_kind_from_name(name: &str) -> &'static str {
    let extension = Path::new(name)
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match extension.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "bmp" | "ico" | "tif" | "tiff" => "Image",
        "mp3" | "flac" | "wav" | "ogg" | "m4a" | "opus" | "aac" => "Audio",
        "mkv" | "mp4" | "avi" | "webm" | "mov" | "m4v" | "mpeg" | "mpg" => "Video",
        "zip" | "7z" | "tar" | "gz" | "xz" | "zst" | "tgz" | "txz" | "tzst" | "rar" | "bz2" => "Archive",
        "rs" | "py" | "js" | "ts" | "tsx" | "jsx" | "html" | "css" | "sh" | "go" | "c" | "cpp" | "h" | "hpp" | "java" | "json" | "yaml" | "yml" | "toml" | "xml" | "sql" => "Code",
        "pdf" => "PDF",
        "csv" | "tsv" | "xls" | "xlsx" | "ods" => "Spreadsheet",
        "ppt" | "pptx" | "odp" => "Presentation",
        "txt" | "md" | "rtf" | "doc" | "docx" | "odt" => "Document",
        _ => "File",
    }
}

fn check_scan_cancel(cancelled: &AtomicBool) -> io::Result<()> {
    if cancelled.load(Ordering::Relaxed) {
        Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "Folder scan cancelled",
        ))
    } else {
        Ok(())
    }
}

pub struct FolderScan {
    pub entries: Vec<Entry>,
    pub unreadable: usize,
    pub first_error: Option<String>,
    pub notice: Option<String>,
}

/// Resolve address/desktop requests on a worker, including symlinked folders.
/// A file opens its containing folder without launching the file itself.
pub fn navigation_target(path: &Path) -> io::Result<(PathBuf, Option<PathBuf>)> {
    let absolute;
    let path = if path.is_absolute() {
        path
    } else {
        absolute = std::env::current_dir()?.join(path);
        absolute.as_path()
    };
    let metadata = fs::metadata(path).or_else(|error| match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => Ok(meta),
        _ => Err(error),
    })?;
    if metadata.is_dir() {
        Ok((path.to_path_buf(), None))
    } else {
        Ok((
            path.parent().unwrap_or(Path::new("/")).to_path_buf(),
            Some(path.to_path_buf()),
        ))
    }
}

/// Preserve readable siblings when an unplugged or stale mount refuses a
/// metadata lookup. A failure to open the *folder itself* still returns Err.
pub fn scan_cancellable(path: &Path, cancelled: &AtomicBool) -> io::Result<FolderScan> {
    scan_with_metadata(path, cancelled, |entry| fs::symlink_metadata(entry))
}

fn scan_with_metadata(
    path: &Path,
    cancelled: &AtomicBool,
    metadata_for: impl Fn(&Path) -> io::Result<fs::Metadata>,
) -> io::Result<FolderScan> {
    check_scan_cancel(cancelled)?;
    let mut entries = Vec::new();
    let mut unreadable = 0;
    let mut first_error = None;
    for result in fs::read_dir(path)? {
        check_scan_cancel(cancelled)?;
        let item = match result {
            Ok(item) => item,
            Err(error) => {
                unreadable += 1;
                first_error.get_or_insert_with(|| error.to_string());
                continue;
            }
        };
        let entry_path = item.path();
        // Metadata is gathered on a worker thread: large/slow folders never freeze the UI.
        let metadata = match metadata_for(&entry_path) {
            Ok(metadata) => Some(metadata),
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => {
                unreadable += 1;
                first_error.get_or_insert_with(|| format!("{}: {error}", entry_path.display()));
                None
            }
        };
        // Linux dirent often retains the file type even when an underlying
        // FUSE/device mount has stopped answering stat. Keep the entry visible
        // and navigable; all actual operations still revalidate the path.
        let fallback_type = if metadata.is_none() {
            item.file_type().ok()
        } else {
            None
        };
        let linked_directory = metadata
            .as_ref()
            .is_some_and(|meta| meta.file_type().is_symlink())
            && fs::metadata(&entry_path).is_ok_and(|meta| meta.is_dir());
        entries.push(Entry {
            name: item.file_name().to_string_lossy().into_owned(),
            path: entry_path,
            is_dir: linked_directory
                || metadata.as_ref().is_some_and(fs::Metadata::is_dir)
                || fallback_type.as_ref().is_some_and(fs::FileType::is_dir),
            is_symlink: metadata
                .as_ref()
                .is_some_and(|value| value.file_type().is_symlink())
                || fallback_type.as_ref().is_some_and(fs::FileType::is_symlink),
            size: metadata.as_ref().map_or(0, fs::Metadata::len),
            modified: metadata.and_then(|value| value.modified().ok()),
        });
    }
    check_scan_cancel(cancelled)?;
    Ok(FolderScan {
        entries,
        unreadable,
        first_error,
        notice: None,
    })
}

pub fn folder_error_message(path: &Path, error: &io::Error) -> String {
    if matches!(
        error.raw_os_error(),
        Some(libc::ENODEV | libc::ENOTCONN | libc::ESTALE)
    ) {
        format!(
            "Mount/device unavailable at {}: {error}. Check the mount in Linux and retry.",
            path.display()
        )
    } else {
        format!("{}: {error}", path.display())
    }
}

/// Bounded recursive filename search. Symlink directories are never followed.
#[cfg(test)]
pub fn search(root: &Path, query: &str, show_hidden: bool) -> io::Result<Vec<Entry>> {
    search_cancellable(root, query, show_hidden, &AtomicBool::new(false))
}

/// A cancelled search returns Interrupted without exposing incomplete results.
#[cfg(test)]
pub fn search_cancellable(
    root: &Path,
    query: &str,
    show_hidden: bool,
    cancelled: &AtomicBool,
) -> io::Result<Vec<Entry>> {
    search_with_options(root, query, show_hidden, true, false, cancelled).map(|scan| scan.entries)
}

const MAX_CONTENT_FILE_BYTES: u64 = 2 * 1024 * 1024;
const MAX_CONTENT_TOTAL_BYTES: u64 = 64 * 1024 * 1024;
const MAX_CONTENT_FILES: usize = 4_096;

/// Inspect only an already-open regular file. Never follow symlinks or open
/// potentially blocking pipes as part of content search.
fn text_contains(path: &Path, needle: &str, cancelled: &AtomicBool) -> io::Result<(bool, u64)> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW);
    }
    let mut file = options.open(path)?;
    let opened = file.metadata()?;
    if !opened.is_file() || opened.len() > MAX_CONTENT_FILE_BYTES {
        return Ok((false, 0));
    }
    let mut bytes = Vec::new();
    (&mut file)
        .take(MAX_CONTENT_FILE_BYTES + 1)
        .read_to_end(&mut bytes)?;
    check_scan_cancel(cancelled)?;
    if bytes.len() as u64 > MAX_CONTENT_FILE_BYTES {
        return Ok((false, bytes.len() as u64));
    }
    let found = !bytes.contains(&0)
        && std::str::from_utf8(&bytes).is_ok_and(|text| text.to_lowercase().contains(needle));
    Ok((found, bytes.len() as u64))
}

/// Bounded local name/content search. A notice marks incomplete results;
/// no index, secret storage or file mutation is involved.
pub fn search_with_options(
    root: &Path,
    query: &str,
    show_hidden: bool,
    recursive: bool,
    contents: bool,
    cancelled: &AtomicBool,
) -> io::Result<FolderScan> {
    check_scan_cancel(cancelled)?;
    let mut results = Vec::new();
    let mut pending = std::collections::VecDeque::from([(root.to_path_buf(), 0usize)]);
    let needle = query.to_lowercase();
    let mut visited = 0usize;
    let mut inspected_files = 0usize;
    let mut inspected_bytes = 0u64;
    let mut skipped_large = 0usize;
    let mut budget_hit = false;
    let started = Instant::now();
    #[cfg(unix)]
    let root_device = {
        use std::os::unix::fs::MetadataExt;
        fs::metadata(root)?.dev()
    };
    'folders: while let Some((directory, depth)) = pending.pop_front() {
        check_scan_cancel(cancelled)?;
        if contents && started.elapsed() >= Duration::from_secs(12) {
            budget_hit = true;
            break;
        }
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(_) if directory.as_path() != root => {
                // An inaccessible child must not hide all the accessible results.
                continue;
            }
            Err(error) => return Err(error),
        };
        for entry in entries {
            check_scan_cancel(cancelled)?;
            if contents && started.elapsed() >= Duration::from_secs(12) {
                budget_hit = true;
                break 'folders;
            }
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => continue,
            };
            let name = entry.file_name().to_string_lossy().into_owned();
            if !show_hidden && name.starts_with('.') {
                continue;
            }
            let metadata = match fs::symlink_metadata(entry.path()) {
                Ok(metadata) => metadata,
                Err(_) => continue,
            };
            #[cfg(unix)]
            let on_root_device = {
                use std::os::unix::fs::MetadataExt;
                metadata.dev() == root_device
            };
            visited += 1;
            if visited >= 250_000 || results.len() >= 20_000 {
                budget_hit = true;
                break 'folders;
            }
            let descend = recursive && metadata.is_dir() && depth < 48;
            #[cfg(unix)]
            let descend = descend && (!contents || on_root_device);
            if descend {
                pending.push_back((entry.path(), depth + 1));
            }
            let name_match = name.to_lowercase().contains(&needle);
            let mut content_match = false;
            #[cfg(unix)]
            let inspect_contents = on_root_device;
            #[cfg(not(unix))]
            let inspect_contents = true;
            if contents
                && !name_match
                && metadata.is_file()
                && inspect_contents
                && !needle.is_empty()
            {
                if metadata.len() > MAX_CONTENT_FILE_BYTES {
                    skipped_large += 1;
                } else if inspected_files >= MAX_CONTENT_FILES
                    || inspected_bytes.saturating_add(metadata.len()) > MAX_CONTENT_TOTAL_BYTES
                    || started.elapsed() >= Duration::from_secs(12)
                {
                    budget_hit = true;
                } else {
                    inspected_files += 1;
                    if let Ok((found, bytes)) = text_contains(&entry.path(), &needle, cancelled) {
                        inspected_bytes = inspected_bytes.saturating_add(bytes);
                        content_match = found;
                    }
                }
            }
            if name_match || content_match {
                results.push(Entry {
                    name,
                    path: entry.path(),
                    is_dir: metadata.is_dir()
                        || (metadata.file_type().is_symlink()
                            && fs::metadata(entry.path()).is_ok_and(|meta| meta.is_dir())),
                    is_symlink: metadata.file_type().is_symlink(),
                    size: metadata.len(),
                    modified: metadata.modified().ok(),
                });
            }
        }
    }
    check_scan_cancel(cancelled)?;
    let notice = if contents && (budget_hit || skipped_large > 0) {
        Some(format!(
            "Content search is partial: {} files over 2 MiB skipped{} (at most 4,096 files, 64 MiB and 12 seconds per search)",
            skipped_large,
            if budget_hit {
                "; scan budget reached"
            } else {
                ""
            }
        ))
    } else if budget_hit {
        Some("Filename search reached its 250,000-entry / 20,000-match limit".into())
    } else {
        None
    };
    Ok(FolderScan {
        entries: results,
        unreadable: 0,
        first_error: None,
        notice,
    })
}

/// A destination collision never permits overwriting existing data.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ConflictPolicy {
    #[default]
    KeepBoth,
    SkipExisting,
    /// Stop the entire batch before any source or destination is modified.
    AbortOnConflict,
    /// Skip occupied destinations only after a bounded, read-only comparison
    /// confirms matching contents. Mismatches and inconclusive checks abort
    /// the entire preflight without touching any path.
    SkipIfIdentical,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlannedDisposition {
    Transfer,
    Skip,
}

#[derive(Clone, Debug)]
pub struct PlannedItem {
    pub source: PathBuf,
    pub target: PathBuf,
    pub disposition: PlannedDisposition,
    pub collision: bool,
    pub renamed: bool,
    snapshot: fs::Metadata,
}

#[derive(Clone, Debug)]
pub struct TransferPlan {
    pub items: Vec<PlannedItem>,
    pub collisions: usize,
    pub renamed: usize,
    pub skipped: usize,
}

struct PlanFailure {
    index: Option<usize>,
    error: io::Error,
}

impl PlanFailure {
    fn at(index: usize, error: io::Error) -> Self {
        Self {
            index: Some(index),
            error,
        }
    }

    fn global(error: io::Error) -> Self {
        Self { index: None, error }
    }
}

/// Inspect all requested deletions before touching any file. Canonicalizing
/// the parent (not the final component) recognizes aliases without following
/// a selected symlink into its target.
fn plan_removal_batch(
    paths: &[PathBuf],
    cancelled: &AtomicBool,
) -> Result<Vec<fs::Metadata>, PlanFailure> {
    let mut resolved_paths: Vec<PathBuf> = Vec::with_capacity(paths.len());
    let mut snapshots = Vec::with_capacity(paths.len());
    for (index, path) in paths.iter().enumerate() {
        if cancelled.load(Ordering::Relaxed) {
            return Err(PlanFailure::at(
                index,
                io::Error::new(io::ErrorKind::Interrupted, "Operation cancelled"),
            ));
        }
        let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
            return Err(PlanFailure::at(index, invalid_name()));
        };
        if !path.is_absolute() {
            return Err(PlanFailure::at(
                index,
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Removal paths must be absolute",
                ),
            ));
        }
        let normalized = fs::canonicalize(parent)
            .map(|parent| parent.join(name))
            .map_err(|error| PlanFailure::at(index, error))?;
        for (other, resolved) in paths[..index].iter().zip(&resolved_paths) {
            if path.starts_with(other)
                || other.starts_with(path)
                || normalized.starts_with(resolved)
                || resolved.starts_with(&normalized)
            {
                return Err(PlanFailure::at(
                    index,
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "Duplicate or overlapping removal paths; select each item only once",
                    ),
                ));
            }
        }
        let metadata = fs::symlink_metadata(path).map_err(|error| PlanFailure::at(index, error))?;
        resolved_paths.push(normalized);
        snapshots.push(metadata);
    }
    Ok(snapshots)
}

#[derive(Clone)]
pub enum Action {
    NewFolder {
        parent: PathBuf,
        name: String,
    },
    NewFile {
        parent: PathBuf,
        name: String,
    },
    Rename {
        source: PathBuf,
        name: String,
    },
    UndoRename {
        current: PathBuf,
        original: PathBuf,
        snapshot: fs::Metadata,
    },
    BatchRename {
        changes: Vec<(PathBuf, OsString)>,
    },
    UndoBatchRename {
        items: Vec<(PathBuf, OsString, fs::Metadata)>,
    },
    Copy {
        sources: Vec<PathBuf>,
        destination: PathBuf,
        conflict: ConflictPolicy,
    },
    Move {
        sources: Vec<PathBuf>,
        destination: PathBuf,
        conflict: ConflictPolicy,
    },
    Trash {
        paths: Vec<PathBuf>,
    },
    Delete {
        paths: Vec<PathBuf>,
    },
    Link {
        sources: Vec<PathBuf>,
        destination: PathBuf,
    },
    SetMode {
        path: PathBuf,
        mode: u32,
        expected: fs::Metadata,
    },
    SetOwnership {
        path: PathBuf,
        uid: u32,
        gid: u32,
        expected: fs::Metadata,
    },
    CreateUserAttribute {
        path: PathBuf,
        name: String,
        value: String,
        expected: fs::Metadata,
    },
}

pub fn valid_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains('/') && !name.contains('\0')
}

/// chmod must operate on the opened inode, not a pathname that can be swapped
/// for a symlink between a preflight check and the write.
#[cfg(target_os = "linux")]
fn apply_mode_to_open_file(file: &File, mode: u32, expected: &fs::Metadata) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let metadata = file.metadata()?;
    ensure_same_source_metadata(expected, &metadata)?;
    if !metadata.is_file() && !metadata.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Permissions can only be changed on ordinary files and directories",
        ));
    }
    file.set_permissions(fs::Permissions::from_mode(mode))
}

#[cfg(target_os = "linux")]
fn set_mode_guarded(path: &Path, mode: u32, expected: &fs::Metadata) -> io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;

    if mode > 0o777 {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "Invalid mode"));
    }
    let mut open = OpenOptions::new();
    open.read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC);
    match open.open(path) {
        Ok(file) => apply_mode_to_open_file(&file, mode, expected),
        // Owners may chmod a file even when it has *no read permission*.
        // A Linux O_PATH handle plus fchmodat2(AT_EMPTY_PATH) preserves that
        // behavior without a pathname-based write or symlink following.
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            set_mode_unreadable_inode(path, mode, expected)
        }
        Err(error) => Err(error),
    }
}

#[cfg(target_os = "linux")]
fn set_mode_unreadable_inode(path: &Path, mode: u32, expected: &fs::Metadata) -> io::Result<()> {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

    let name = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "Invalid filename"))?;
    // SAFETY: `name` is a valid NUL-terminated pointer, and O_PATH does not
    // read or write file data. The returned descriptor is immediately owned.
    let raw = unsafe {
        libc::open(
            name.as_ptr(),
            libc::O_PATH | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if raw < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `raw` is a freshly opened descriptor owned solely by this scope.
    let fd = File::from(unsafe { OwnedFd::from_raw_fd(raw) });
    let metadata = fd.metadata()?;
    ensure_same_source_metadata(expected, &metadata)?;
    if !metadata.is_file() && !metadata.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Permissions can only be changed on ordinary files and directories",
        ));
    }
    // SAFETY: fchmodat2 with AT_EMPTY_PATH modifies only the inode held by
    // `fd`; no attacker-controlled pathname is dereferenced for the write.
    let result = unsafe {
        libc::syscall(
            libc::SYS_fchmodat2,
            fd.as_raw_fd(),
            c"".as_ptr(),
            mode as libc::mode_t,
            libc::AT_EMPTY_PATH,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Change owner/group only on an inode-pinned O_PATH descriptor. Linux
/// enforces the caller's group/UID permissions: no elevation or sudo occurs.
#[cfg(target_os = "linux")]
fn set_ownership_guarded(
    path: &Path,
    uid: u32,
    gid: u32,
    expected: &fs::Metadata,
) -> io::Result<()> {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

    // (uid_t)-1 and (gid_t)-1 mean "leave unchanged" to the kernel. They are
    // not valid explicit owner/group selections and must not silently succeed.
    if uid == u32::MAX || gid == u32::MAX {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Invalid owner/group identifier",
        ));
    }

    let name = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "Invalid filename"))?;
    // SAFETY: NUL-terminated path; O_NOFOLLOW pins the entry without reading
    // file contents or resolving a symlink target.
    let raw = unsafe {
        libc::open(
            name.as_ptr(),
            libc::O_PATH | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if raw < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the fresh descriptor is owned by this File and closed on drop.
    let file = File::from(unsafe { OwnedFd::from_raw_fd(raw) });
    let metadata = file.metadata()?;
    ensure_same_source_metadata(expected, &metadata)?;
    if !metadata.is_file() && !metadata.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Owner/group changes require an ordinary file or directory",
        ));
    }
    // SAFETY: AT_EMPTY_PATH directs fchownat at the pinned descriptor only;
    // the kernel enforces ownership/group membership and never dereferences
    // the original (potentially replaced) pathname.
    let result = unsafe {
        libc::fchownat(
            file.as_raw_fd(),
            c"".as_ptr(),
            uid as libc::uid_t,
            gid as libc::gid_t,
            libc::AT_EMPTY_PATH,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Create a small text attribute without ever replacing an existing value.
/// A pinned O_NOFOLLOW descriptor prevents path and symlink substitution;
/// XATTR_CREATE is the kernel-enforced no-overwrite condition.
#[cfg(target_os = "linux")]
fn create_user_attribute_guarded(
    path: &Path,
    name: &str,
    value: &str,
    expected: &fs::Metadata,
) -> io::Result<()> {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

    if !name.starts_with("user.")
        || name.len() <= "user.".len()
        || name.len() > 255
        || name.as_bytes().contains(&0)
        || value.len() > 4096
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Expected a user.* attribute name (max 255 bytes) and text value (max 4 KiB)",
        ));
    }
    let attribute = CString::new(name)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "Invalid attribute name"))?;
    let source = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "Invalid filename"))?;
    // SAFETY: valid NUL-terminated pathname; opening nofollow with read access
    // pins the inode without resolving the final symlink. The kernel still
    // enforces ordinary permissions to modify attributes.
    let raw = unsafe {
        libc::open(
            source.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
        )
    };
    if raw < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `open` succeeded; OwnedFd closes the descriptor on drop.
    let file = File::from(unsafe { OwnedFd::from_raw_fd(raw) });
    let metadata = file.metadata()?;
    ensure_same_source_metadata(expected, &metadata)?;
    if !metadata.is_file() && !metadata.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Attributes require an ordinary file or directory",
        ));
    }
    // SAFETY: name and value have valid storage for the supplied lengths.
    // XATTR_CREATE atomically refuses collisions, including concurrent writes.
    let result = unsafe {
        libc::fsetxattr(
            file.as_raw_fd(),
            attribute.as_ptr(),
            value.as_ptr().cast(),
            value.len(),
            libc::XATTR_CREATE,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

pub const MAX_BATCH_RENAME: usize = 256;

pub struct BatchRenameOptions<'a> {
    pub prefix: &'a str,
    pub suffix: &'a str,
    pub find: &'a str,
    pub replace: &'a str,
    pub regex: bool,
    pub numbering: bool,
    pub number_start: u32,
    pub number_width: usize,
}

fn valid_os_name(name: &OsStr) -> bool {
    !name.is_empty()
        && name != OsStr::new(".")
        && name != OsStr::new("..")
        && !name.as_encoded_bytes().contains(&b'/')
        && !name.as_encoded_bytes().contains(&0)
}

/// Build a side-effect-free preview from snapshotted selection paths.
/// OsString preserves arbitrary Linux filename bytes, including non-UTF-8.
#[cfg(test)]
pub fn plan_batch_rename(
    paths: &[PathBuf],
    prefix: &str,
    suffix: &str,
) -> io::Result<Vec<(PathBuf, OsString)>> {
    plan_batch_rename_with_options(
        paths,
        &BatchRenameOptions {
            prefix,
            suffix,
            find: "",
            replace: "",
            regex: false,
            numbering: false,
            number_start: 1,
            number_width: 3,
        },
    )
}

pub fn plan_batch_rename_with_options(
    paths: &[PathBuf],
    options: &BatchRenameOptions<'_>,
) -> io::Result<Vec<(PathBuf, OsString)>> {
    if paths.is_empty() || paths.len() > MAX_BATCH_RENAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("Select 1–{MAX_BATCH_RENAME} items for batch rename"),
        ));
    }
    if options.prefix.is_empty()
        && options.suffix.is_empty()
        && options.find.is_empty()
        && !options.numbering
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Enter a prefix, suffix, search text or enable numbering",
        ));
    }
    if !options.prefix.is_empty() && !valid_name(options.prefix)
        || !options.suffix.is_empty() && !valid_name(options.suffix)
        || options.find.contains('/')
        || options.find.contains('\0')
        || options.replace.contains('/')
        || options.replace.contains('\0')
        || (options.find.is_empty() && !options.replace.is_empty())
        || (options.regex && options.find.is_empty())
        || options.find.len() > 256
        || options.replace.len() > 256
    {
        return Err(invalid_name());
    }
    if options.numbering
        && (options.number_start == 0
            || !(1..=8).contains(&options.number_width)
            || u64::from(options.number_start) + paths.len() as u64 > 100_000_000)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Numbering needs a start from 1 and width 1–8 (max number 99,999,999)",
        ));
    }
    // Compile once per preview, not once per filename. The regex crate uses
    // a bounded, non-backtracking engine; the selection is capped at 256.
    let pattern = if options.regex {
        Some(
            regex::RegexBuilder::new(options.find)
                .size_limit(2 * 1024 * 1024)
                .build()
                .map_err(|error| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!("Invalid batch rename regex: {error}"),
                    )
                })?,
        )
    } else {
        None
    };
    let mut destinations = BTreeSet::new();
    let mut changes = Vec::with_capacity(paths.len());
    for (index, path) in paths.iter().enumerate() {
        let original = path.file_name().ok_or_else(invalid_name)?;
        let replacement = if options.find.is_empty() {
            None
        } else {
            let utf8 = original.to_str().ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Search/replace requires UTF-8 filenames; prefix/suffix and numbering preserve arbitrary filename bytes",
                )
            })?;
            Some(if let Some(pattern) = &pattern {
                pattern.replace_all(utf8, options.replace).into_owned()
            } else {
                utf8.replace(options.find, options.replace)
            })
        };
        let mut name = OsString::from(options.prefix);
        if options.numbering {
            name.push(format!(
                "{:0width$}-",
                options.number_start as usize + index,
                width = options.number_width
            ));
        }
        if let Some(replaced) = replacement {
            name.push(replaced);
        } else {
            name.push(original);
        }
        name.push(options.suffix);
        if !valid_os_name(&name)
            || name == original
            || !destinations.insert(path.with_file_name(&name))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Invalid, unchanged or duplicate name in batch rename preview",
            ));
        }
        changes.push((path.clone(), name));
    }
    Ok(changes)
}

fn invalid_name() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, "Invalid file or folder name")
}

/// Batch operations are not transactional: report exactly where an error
/// stopped work instead of suggesting that an entire batch was rolled back.
fn batch_failure(
    verb: &str,
    index: usize,
    total: usize,
    path: &Path,
    error: io::Error,
) -> io::Error {
    batch_failure_with_skips(verb, index, total, path, 0, error)
}

fn batch_failure_with_skips(
    verb: &str,
    index: usize,
    total: usize,
    path: &Path,
    skipped: usize,
    error: io::Error,
) -> io::Error {
    io::Error::new(
        error.kind(),
        format!(
            "{verb} stopped at item {} of {total} ({}): {error}. {} earlier item(s) completed, {skipped} skipped; inspect the current item and both locations before retrying",
            index + 1,
            path.display(),
            index.saturating_sub(skipped)
        ),
    )
}

fn occupied(path: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

#[cfg(target_os = "linux")]
pub(crate) fn rename_without_overwrite(source: &Path, destination: &Path) -> io::Result<()> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};
    let source = CString::new(source.as_os_str().as_bytes()).map_err(|_| invalid_name())?;
    let destination =
        CString::new(destination.as_os_str().as_bytes()).map_err(|_| invalid_name())?;
    // Linux renameat2 RENAME_NOREPLACE is atomic, including broken symlinks.
    let result = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            source.as_ptr(),
            libc::AT_FDCWD,
            destination.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Exchange two existing directory entries atomically. Unlike temporary-name
/// staging, a crash cannot strand a selected item under a hidden name.
#[cfg(target_os = "linux")]
fn exchange_names(left: &Path, right: &Path) -> io::Result<()> {
    let left = CString::new(left.as_os_str().as_bytes()).map_err(|_| invalid_name())?;
    let right = CString::new(right.as_os_str().as_bytes()).map_err(|_| invalid_name())?;
    // SAFETY: both pointers refer to NUL-terminated paths owned by CString.
    if unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            left.as_ptr(),
            libc::AT_FDCWD,
            right.as_ptr(),
            libc::RENAME_EXCHANGE,
        )
    } == 0
    {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(not(target_os = "linux"))]
fn exchange_names(_left: &Path, _right: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "Atomic rename cycles require Linux renameat2",
    ))
}

enum RenameStep {
    Moved {
        owner: usize,
        from: PathBuf,
        to: PathBuf,
    },
    Exchanged {
        left: usize,
        right: usize,
    },
}

/// A rename updates the ctime of an inode also visible through other selected
/// hardlinks. Refresh only those aliases: accepting ctime drift is safe here
/// only if identity, content-relevant metadata and ownership still match.
fn refresh_selected_hardlinks(
    positions: &[PathBuf],
    snapshots: &mut [fs::Metadata],
    touched: &[usize],
) -> io::Result<()> {
    #[cfg(unix)]
    use std::os::unix::fs::MetadataExt;

    for index in 0..positions.len() {
        if touched.contains(&index)
            || !touched
                .iter()
                .any(|&other| same_inode_and_kind(&snapshots[index], &snapshots[other]))
        {
            continue;
        }
        let current = fs::symlink_metadata(&positions[index])?;
        if !same_inode_and_kind(&snapshots[index], &current)
            || snapshots[index].len() != current.len()
            || snapshots[index].modified().ok() != current.modified().ok()
            || snapshots[index].permissions() != current.permissions()
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "A selected hardlink changed unexpectedly during batch rename",
            ));
        }
        #[cfg(unix)]
        if snapshots[index].uid() != current.uid() || snapshots[index].gid() != current.gid() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Selected hardlink ownership changed during batch rename",
            ));
        }
        snapshots[index] = current;
    }
    Ok(())
}

/// Apply a complete batch without ever replacing an unrelated entry. Moves
/// into empty names use RENAME_NOREPLACE; cycles use RENAME_EXCHANGE. Every
/// step preserves both entries across a crash. A normal error attempts a
/// guarded reverse traversal; a concurrent external writer can prevent that
/// rollback, in which case the partial positions are reported honestly.
fn apply_batch_rename(
    changes: &[(PathBuf, OsString)],
    expected: Option<&[fs::Metadata]>,
    transfer: &mut Transfer<'_>,
    on_item: &mut dyn FnMut(ItemUpdate),
) -> io::Result<String> {
    if changes.is_empty() || changes.len() > MAX_BATCH_RENAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Batch rename requires between 1 and 256 items",
        ));
    }
    if expected.is_some_and(|items| items.len() != changes.len()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Batch undo source evidence does not match the requested operation",
        ));
    }
    let parent = changes[0].0.parent();
    let mut originals = BTreeMap::new();
    let mut targets = BTreeSet::new();
    let mut destinations = Vec::with_capacity(changes.len());
    let mut snapshots = Vec::with_capacity(changes.len());
    for (index, (source, name)) in changes.iter().enumerate() {
        transfer.check()?;
        let target = source.with_file_name(name);
        if !valid_os_name(name)
            || source == &target
            || source.parent() != parent
            || originals.insert(source.clone(), index).is_some()
            || !targets.insert(target.clone())
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Batch rename has duplicate, unchanged or invalid paths",
            ));
        }
        destinations.push(target);
        let snapshot = fs::symlink_metadata(source)?;
        if let Some(saved) = expected {
            ensure_same_source(source, &saved[index])?;
        }
        snapshots.push(snapshot);
    }
    for target in &destinations {
        if !originals.contains_key(target) && occupied(target)? {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("Batch rename target already exists: {}", target.display()),
            ));
        }
    }

    transfer.phase = "Renaming";
    transfer.total = changes.len() as u64;
    let mut positions: Vec<_> = changes.iter().map(|(source, _)| source.clone()).collect();
    let mut owners = originals;
    let mut completed = vec![false; changes.len()];
    let mut steps = Vec::with_capacity(changes.len());
    let mut count = 0usize;
    let mut uncertain = false;
    while count < changes.len() {
        let candidate = (0..changes.len())
            .find(|&index| !completed[index] && !owners.contains_key(&destinations[index]));
        let index = candidate.unwrap_or_else(|| {
            completed
                .iter()
                .position(|done| !*done)
                .expect("unfinished batch")
        });
        let source = positions[index].clone();
        let target = &destinations[index];
        let result = (|| {
            transfer.check()?;
            ensure_same_source(&source, &snapshots[index])?;
            if let Some(&other) = owners.get(target) {
                ensure_same_source(target, &snapshots[other])?;
                exchange_names(&source, target)?;
                positions[index] = target.clone();
                positions[other] = source.clone();
                owners.insert(source.clone(), other);
                owners.insert(target.clone(), index);
                steps.push(RenameStep::Exchanged {
                    left: index,
                    right: other,
                });
                // Verify both original entries reached the expected paths.
                let moved = fs::symlink_metadata(target).inspect_err(|_| {
                    uncertain = true;
                })?;
                let swapped = fs::symlink_metadata(&source).inspect_err(|_| {
                    uncertain = true;
                })?;
                if !same_inode_and_kind(&snapshots[index], &moved)
                    || !same_inode_and_kind(&snapshots[other], &swapped)
                {
                    uncertain = true;
                    return Err(io::Error::other(
                        "Concurrent replacement during name exchange; inspect both entries",
                    ));
                }
                snapshots[index] = moved;
                snapshots[other] = swapped;
                refresh_selected_hardlinks(&positions, &mut snapshots, &[index, other])
                    .inspect_err(|_| {
                        uncertain = true;
                    })?;
            } else {
                rename_without_overwrite(&source, target)?;
                positions[index] = target.clone();
                owners.remove(&source);
                owners.insert(target.clone(), index);
                steps.push(RenameStep::Moved {
                    owner: index,
                    from: source,
                    to: target.clone(),
                });
                let moved = fs::symlink_metadata(target).inspect_err(|_| {
                    uncertain = true;
                })?;
                if !same_inode_and_kind(&snapshots[index], &moved) {
                    uncertain = true;
                    return Err(io::Error::other(
                        "Concurrent replacement during rename; inspect destination",
                    ));
                }
                snapshots[index] = moved;
                refresh_selected_hardlinks(&positions, &mut snapshots, &[index]).inspect_err(
                    |_| {
                        uncertain = true;
                    },
                )?;
            }
            Ok(())
        })();
        if let Err(error) = result {
            let mut rollback_error = uncertain
                .then(|| io::Error::other("A changed entry prevents an automatic rollback"));
            for step in steps.into_iter().rev() {
                if rollback_error.is_some() {
                    break;
                }
                let result = (|| {
                    match step {
                        RenameStep::Moved { owner, from, to } => {
                            ensure_same_source(&to, &snapshots[owner])?;
                            rename_without_overwrite(&to, &from)?;
                            positions[owner] = from.clone();
                            owners.remove(&to);
                            owners.insert(from.clone(), owner);
                            snapshots[owner] = fs::symlink_metadata(&from)?;
                            refresh_selected_hardlinks(&positions, &mut snapshots, &[owner])?;
                        }
                        RenameStep::Exchanged { left, right } => {
                            let at_left = positions[left].clone();
                            let at_right = positions[right].clone();
                            ensure_same_source(&at_left, &snapshots[left])?;
                            ensure_same_source(&at_right, &snapshots[right])?;
                            exchange_names(&at_left, &at_right)?;
                            positions.swap(left, right);
                            snapshots[left] = fs::symlink_metadata(&positions[left])?;
                            snapshots[right] = fs::symlink_metadata(&positions[right])?;
                            owners.insert(positions[left].clone(), left);
                            owners.insert(positions[right].clone(), right);
                            refresh_selected_hardlinks(&positions, &mut snapshots, &[left, right])?;
                        }
                    }
                    Ok::<_, io::Error>(())
                })();
                if let Err(rollback) = result {
                    rollback_error = Some(rollback);
                    break;
                }
            }
            if rollback_error.is_none() {
                for prior in 0..changes.len() {
                    on_item(ItemUpdate::new(prior, ItemState::NotStarted, None));
                }
            } else {
                for prior in 0..changes.len() {
                    if positions[prior] != changes[prior].0 {
                        on_item(ItemUpdate {
                            index: prior,
                            target: Some(positions[prior].clone()),
                            state: ItemState::Failed,
                            detail: Some("Moved to this position before the operation stopped; inspect manually".into()),
                        });
                    }
                }
            }
            on_item(ItemUpdate::error(
                index,
                Some(destinations[index].clone()),
                &error,
            ));
            let details = if let Some(rollback) = rollback_error {
                format!(
                    "Batch rename stopped: {error}. Guarded rollback also stopped: {rollback}. No data was intentionally discarded; inspect the partially renamed entries before retrying"
                )
            } else {
                format!("Batch rename stopped: {error}. Earlier name changes were rolled back")
            };
            return Err(io::Error::new(error.kind(), details));
        }
        for owner in 0..changes.len() {
            if !completed[owner] && positions[owner] == destinations[owner] {
                completed[owner] = true;
                count += 1;
                transfer.transferred = count as u64;
                transfer.report(&destinations[owner]);
                on_item(ItemUpdate::new(
                    owner,
                    ItemState::Completed,
                    Some(destinations[owner].clone()),
                ));
            }
        }
    }
    Ok(format!("Renamed {} item(s)", changes.len()))
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn rename_without_overwrite(source: &Path, destination: &Path) -> io::Result<()> {
    if occupied(destination)? {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "Target already exists",
        ));
    }
    fs::rename(source, destination)
}

#[cfg(test)]
pub fn perform(action: &Action) -> io::Result<String> {
    let cancelled = AtomicBool::new(false);
    perform_with_progress(action, &cancelled, &mut |_| {})
}

/// No source is removed before a cross-device copy completes successfully.
#[cfg(test)]
pub fn perform_with_progress(
    action: &Action,
    cancelled: &AtomicBool,
    notify: &mut dyn FnMut(Progress),
) -> io::Result<String> {
    perform_with_progress_and_items(action, cancelled, notify, None, &mut |_| {})
}

/// Returns one entry per requested source, including items never started
/// because an earlier source failed. Does not change the operation's behavior.
pub fn perform_with_report(
    action: &Action,
    cancelled: &AtomicBool,
    notify: &mut dyn FnMut(Progress),
) -> OperationReport {
    let before_rename = match action {
        Action::Rename { source, .. } => fs::symlink_metadata(source).ok(),
        _ => None,
    };
    let before_batch = match action {
        Action::BatchRename { changes } => changes
            .iter()
            .map(|(source, _)| fs::symlink_metadata(source).ok())
            .collect::<Option<Vec<_>>>(),
        _ => None,
    };
    let sources: Vec<PathBuf> = match action {
        Action::Copy { sources, .. }
        | Action::Move { sources, .. }
        | Action::Link { sources, .. } => sources.clone(),
        Action::BatchRename { changes } => {
            changes.iter().map(|(source, _)| source.clone()).collect()
        }
        Action::UndoBatchRename { items } => {
            items.iter().map(|(source, _, _)| source.clone()).collect()
        }
        Action::Trash { paths } | Action::Delete { paths } => paths.clone(),
        _ => Vec::new(),
    };
    let mut items: Vec<ItemResult> = sources
        .into_iter()
        .map(|source| ItemResult {
            source,
            target: None,
            state: ItemState::NotStarted,
            detail: None,
        })
        .collect();
    let result = perform_with_progress_and_items(action, cancelled, notify, None, &mut |update| {
        if let Some(item) = items.get_mut(update.index) {
            item.state = update.state;
            item.target = update.target;
            item.detail = update.detail;
        }
    });
    let rename_undo = if result.is_ok() {
        match (action, before_rename) {
            (Action::Rename { source, name }, Some(before)) => {
                let current = source.with_file_name(name);
                if &current == source {
                    None
                } else {
                    fs::symlink_metadata(&current)
                        .ok()
                        .filter(|after| same_inode_and_kind(&before, after))
                        .map(|snapshot| {
                            Box::new(RenameUndo {
                                current,
                                original: source.clone(),
                                snapshot,
                            })
                        })
                }
            }
            _ => None,
        }
    } else {
        None
    };
    let batch_rename_undo = if result.is_ok() {
        match (action, before_batch) {
            (Action::BatchRename { changes }, Some(before)) if before.len() == changes.len() => {
                changes
                    .iter()
                    .zip(before)
                    .map(|((source, name), original_snapshot)| {
                        let current = source.with_file_name(name);
                        let original_name = source.file_name()?.to_os_string();
                        let after = fs::symlink_metadata(&current).ok()?;
                        same_inode_and_kind(&original_snapshot, &after).then_some((
                            current,
                            original_name,
                            after,
                        ))
                    })
                    .collect::<Option<Vec<_>>>()
                    .map(|items| Box::new(BatchRenameUndo { items }))
            }
            _ => None,
        }
    } else {
        None
    };
    OperationReport {
        result,
        items,
        rename_undo,
        batch_rename_undo,
    }
}

#[cfg(test)]
fn perform_with_progress_in(
    action: &Action,
    cancelled: &AtomicBool,
    notify: &mut dyn FnMut(Progress),
    journal_directory: Option<&Path>,
) -> io::Result<String> {
    perform_with_progress_and_items(action, cancelled, notify, journal_directory, &mut |_| {})
}

/// The historical injected journal path remains in the test adapter so tests
/// can assert that new source-preserving cross-volume copies create no journal.
fn perform_with_progress_and_items(
    action: &Action,
    cancelled: &AtomicBool,
    notify: &mut dyn FnMut(Progress),
    _journal_directory: Option<&Path>,
    on_item: &mut dyn FnMut(ItemUpdate),
) -> io::Result<String> {
    let sources: &[PathBuf] = match action {
        Action::Copy { sources, .. }
        | Action::Move { sources, .. }
        | Action::Link { sources, .. } => sources,
        Action::Trash { paths } | Action::Delete { paths } => paths,
        _ => &[],
    };
    // Validate the *entire* Copy/Move batch before any destination is staged
    // or any source is renamed. UI previews are advisory; this worker-side
    // preflight runs again with current filesystem state.
    let plan = match action {
        Action::Copy {
            sources,
            destination,
            conflict,
        }
        | Action::Move {
            sources,
            destination,
            conflict,
        } if !sources.is_empty() => {
            let moving = matches!(action, Action::Move { .. });
            match plan_transfer(sources, destination, *conflict, moving, cancelled) {
                Ok(plan) => Some(plan),
                Err(failure) => {
                    let index = failure.index.unwrap_or(0);
                    on_item(ItemUpdate::error(index, None, &failure.error));
                    return Err(io::Error::new(
                        failure.error.kind(),
                        format!(
                            "Transfer preflight failed{}: {}. No items were processed",
                            failure.index.map_or_else(String::new, |index| format!(
                                " at item {} of {} ({})",
                                index + 1,
                                sources.len(),
                                sources[index].display()
                            )),
                            failure.error
                        ),
                    ));
                }
            }
        }
        _ => None,
    };
    let removal_snapshots = match action {
        Action::Trash { paths } | Action::Delete { paths } => {
            match plan_removal_batch(paths, cancelled) {
                Ok(snapshots) => Some(snapshots),
                Err(failure) => {
                    let index = failure.index.unwrap_or(0);
                    on_item(ItemUpdate::error(index, None, &failure.error));
                    return Err(io::Error::new(
                        failure.error.kind(),
                        format!(
                            "Removal preflight failed at item {} of {} ({}): {}. No items were removed",
                            index + 1,
                            paths.len(),
                            paths[index].display(),
                            failure.error
                        ),
                    ));
                }
            }
        }
        _ => None,
    };
    let mut estimated: Vec<u64> = Vec::with_capacity(sources.len());
    for (index, source) in sources.iter().enumerate() {
        let estimate =
            match action {
                Action::Copy { .. } => estimate_bytes(source, cancelled, 0),
                Action::Move { .. } => fs::symlink_metadata(source).map(|metadata| {
                    if metadata.is_dir() { 0 } else { metadata.len() }
                }),
                _ => break,
            };
        match estimate {
            Ok(bytes) => estimated.push(bytes),
            Err(error) => {
                on_item(ItemUpdate::error(index, None, &error));
                return Err(io::Error::new(
                    error.kind(),
                    format!(
                        "Transfer preflight failed at item {} of {} ({}): {error}. No items were processed",
                        index + 1,
                        sources.len(),
                        source.display()
                    ),
                ));
            }
        }
    }
    let total = estimated
        .iter()
        .fold(0u64, |sum, bytes| sum.saturating_add(*bytes));
    let mut transfer = Transfer {
        cancelled,
        notify,
        transferred: 0,
        total: match action {
            Action::Trash { paths } | Action::Delete { paths } => paths.len() as u64,
            Action::Link { sources, .. } => sources.len() as u64,
            _ => total,
        },
        last_report: Instant::now(),
        last_reported_bytes: 0,
        phase: match action {
            Action::Trash { .. } => "Trashing",
            Action::Delete { .. } => "Deleting",
            Action::Link { .. } => "Linking",
            _ => "Copying",
        },
    };
    transfer.report(
        sources
            .first()
            .map(PathBuf::as_path)
            .unwrap_or(Path::new("")),
    );
    match action {
        Action::NewFolder { parent, name } => {
            transfer.check()?;
            if !valid_name(name) {
                return Err(invalid_name());
            }
            fs::create_dir(parent.join(name))?;
            Ok(format!("Created folder {name}"))
        }
        Action::NewFile { parent, name } => {
            transfer.check()?;
            if !valid_name(name) {
                return Err(invalid_name());
            }
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(parent.join(name))?;
            Ok(format!("Created file {name}"))
        }
        Action::Rename { source, name } => {
            transfer.check()?;
            if !valid_name(name) {
                return Err(invalid_name());
            }
            let target = source.with_file_name(name);
            if source == &target {
                return Ok("Name unchanged".into());
            }
            rename_without_overwrite(source, &target)?;
            Ok(format!("Renamed to {name}"))
        }
        Action::UndoRename {
            current,
            original,
            snapshot,
        } => {
            transfer.check()?;
            if current == original
                || current.parent() != original.parent()
                || original.file_name().is_none()
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Undo rename requires distinct names in the same folder",
                ));
            }
            ensure_same_source(current, snapshot)?;
            rename_without_overwrite(current, original)?;
            Ok(format!("Restored original name {}", original.display()))
        }
        Action::BatchRename { changes } => {
            apply_batch_rename(changes, None, &mut transfer, on_item)
        }
        Action::UndoBatchRename { items } => {
            let changes: Vec<_> = items
                .iter()
                .map(|(source, original, _)| (source.clone(), original.clone()))
                .collect();
            let evidence: Vec<_> = items.iter().map(|(_, _, saved)| saved.clone()).collect();
            apply_batch_rename(&changes, Some(&evidence), &mut transfer, on_item)
                .map(|_| format!("Restored {} original names", items.len()))
        }
        Action::Copy { sources, .. } => {
            if sources.is_empty() {
                return Ok("Nothing to copy".into());
            }
            let mut skipped = 0usize;
            let plan = plan.as_ref().expect("nonempty Copy has a validated plan");
            for (index, (source, bytes)) in sources.iter().zip(estimated.iter()).enumerate() {
                let planned = &plan.items[index];
                let mut target = None;
                let mut skipped_item = false;
                let result = (|| {
                    transfer.check()?;
                    target = Some(planned.target.clone());
                    if planned.disposition == PlannedDisposition::Skip {
                        skipped += 1;
                        skipped_item = true;
                        transfer.advance(*bytes, source);
                        return Ok(());
                    }
                    ensure_same_source(source, &planned.snapshot)?;
                    stage_and_publish(source, &planned.target, false, &mut transfer)
                })();
                match result {
                    Ok(()) => on_item(ItemUpdate::new(
                        index,
                        if skipped_item {
                            ItemState::Skipped
                        } else {
                            ItemState::Completed
                        },
                        target,
                    )),
                    Err(error) => {
                        on_item(ItemUpdate::error(index, target, &error));
                        return Err(batch_failure_with_skips(
                            "Copy",
                            index,
                            sources.len(),
                            source,
                            skipped,
                            error,
                        ));
                    }
                }
            }
            if skipped > 0 {
                Ok(format!(
                    "Copied {} item(s); skipped {skipped} existing",
                    sources.len() - skipped
                ))
            } else {
                Ok(format!("Copied {} item(s)", sources.len()))
            }
        }
        Action::Move { sources, .. } => {
            if sources.is_empty() {
                return Ok("Nothing to move".into());
            }
            let mut skipped = 0usize;
            let mut retained = 0usize;
            let plan = plan.as_ref().expect("nonempty Move has a validated plan");
            for (index, (source, bytes)) in sources.iter().zip(estimated.iter()).enumerate() {
                let planned = &plan.items[index];
                let mut target = None;
                let mut skipped_item = false;
                let mut retained_item = false;
                let result = (|| {
                    transfer.check()?;
                    target = Some(planned.target.clone());
                    if planned.disposition == PlannedDisposition::Skip {
                        skipped += 1;
                        skipped_item = true;
                        transfer.advance(*bytes, source);
                        return Ok(());
                    }
                    ensure_same_source(source, &planned.snapshot)?;
                    ensure_outside_source(source, &planned.target)?;
                    match rename_without_overwrite(source, &planned.target) {
                        Ok(()) => transfer.advance(*bytes, &planned.target),
                        Err(error) if error.raw_os_error() == Some(libc::EXDEV) => {
                            // Only traverse a directory when the atomic rename is impossible.
                            let actual_bytes = estimate_bytes(source, cancelled, 0)?;
                            transfer.total = transfer
                                .total
                                .saturating_add(actual_bytes.saturating_sub(*bytes));
                            transfer.report(source);
                            // The source is never deleted after cross-device publication:
                            // Linux has no conditional unlink that atomically checks a
                            // previously verified inode against a concurrent writer.
                            // Stage and publish a verified independent copy instead.
                            let mut source_manifest = BTreeMap::new();
                            capture_source_manifest(
                                source,
                                0,
                                &mut source_manifest,
                                &mut transfer,
                            )?;
                            ensure_hardlink_scope_preserved(&source_manifest)?;
                            stage_and_publish(source, &planned.target, true, &mut transfer)?;
                            retained_item = true;
                            retained += 1;
                            transfer.phase = "Copying";
                        }
                        Err(error) => return Err(error),
                    }
                    Ok(())
                })();
                match result {
                    Ok(()) => {
                        let mut update = ItemUpdate::new(
                            index,
                            if skipped_item {
                                ItemState::Skipped
                            } else if retained_item {
                                ItemState::CopiedSourceRetained
                            } else {
                                ItemState::Completed
                            },
                            target,
                        );
                        if retained_item {
                            update.detail = Some(
                                "Cross-device copy verified. Original source was kept; review the copy and remove the original manually."
                                    .into(),
                            );
                        }
                        on_item(update);
                    }
                    Err(error) => {
                        on_item(ItemUpdate::error(index, target, &error));
                        return Err(batch_failure_with_skips(
                            "Move",
                            index,
                            sources.len(),
                            source,
                            skipped,
                            error,
                        ));
                    }
                }
            }
            if retained > 0 {
                Ok(format!(
                    "Moved {} item(s); copied {retained} across filesystems — originals kept for manual cleanup; skipped {skipped}",
                    sources.len() - skipped - retained
                ))
            } else if skipped > 0 {
                Ok(format!(
                    "Moved {} item(s); skipped {skipped} existing",
                    sources.len() - skipped
                ))
            } else {
                Ok(format!("Moved {} item(s)", sources.len()))
            }
        }
        Action::Trash { paths } => {
            let snapshots = removal_snapshots.as_ref().expect("Trash has preflight");
            for (index, (path, snapshot)) in paths.iter().zip(snapshots).enumerate() {
                let result = (|| {
                    transfer.check()?;
                    ensure_same_source(path, snapshot)?;
                    trash::delete(path).map_err(io::Error::other)
                })();
                match result {
                    Ok(()) => on_item(ItemUpdate::new(index, ItemState::Completed, None)),
                    Err(error) => {
                        on_item(ItemUpdate::error(index, None, &error));
                        return Err(batch_failure("Trash", index, paths.len(), path, error));
                    }
                }
                transfer.transferred = (index + 1) as u64;
                transfer.report(path);
            }
            Ok(format!("Moved {} item(s) to Trash", paths.len()))
        }
        Action::Delete { paths } => {
            let snapshots = removal_snapshots.as_ref().expect("Delete has preflight");
            for (index, (path, snapshot)) in paths.iter().zip(snapshots).enumerate() {
                let result = (|| {
                    transfer.check()?;
                    ensure_same_source(path, snapshot)?;
                    remove_selected(path)
                })();
                match result {
                    Ok(()) => on_item(ItemUpdate::new(index, ItemState::Completed, None)),
                    Err(error) => {
                        on_item(ItemUpdate::error(index, None, &error));
                        return Err(batch_failure("Delete", index, paths.len(), path, error));
                    }
                }
                transfer.transferred = (index + 1) as u64;
                transfer.report(path);
            }
            Ok(format!("Permanently deleted {} item(s)", paths.len()))
        }
        Action::Link {
            sources,
            destination,
        } => {
            if !fs::metadata(destination)?.is_dir() {
                return Err(invalid_name());
            }
            // Reserve destinations for the entire batch first. Two sources
            // sharing a basename must not race each other for the same name.
            // This preflight does not alter filesystem state or follow a
            // source symlink (dangling link targets are intentional).
            let mut reserved = BTreeSet::new();
            let mut planned = Vec::with_capacity(sources.len());
            for (index, source) in sources.iter().enumerate() {
                let result = (|| {
                    transfer.check()?;
                    let target = free_destination_reserved(
                        destination,
                        source.file_name().ok_or_else(invalid_name)?,
                        &reserved,
                    )?;
                    let absolute = if source.is_absolute() {
                        source.clone()
                    } else {
                        std::env::current_dir()?.join(source)
                    };
                    Ok((absolute, target))
                })();
                match result {
                    Ok((absolute, target)) => {
                        reserved.insert(target.clone());
                        planned.push((absolute, target));
                    }
                    Err(error) => {
                        on_item(ItemUpdate::error(index, None, &error));
                        return Err(io::Error::new(
                            error.kind(),
                            format!(
                                "Link preflight failed at item {} of {} ({}): {error}. No links were created",
                                index + 1,
                                sources.len(),
                                source.display()
                            ),
                        ));
                    }
                }
            }
            for (index, (absolute, target)) in planned.iter().enumerate() {
                let result = (|| {
                    transfer.check()?;
                    #[cfg(unix)]
                    {
                        std::os::unix::fs::symlink(absolute, target)
                    }
                    #[cfg(not(unix))]
                    {
                        let _ = (absolute, target);
                        Err(io::Error::new(
                            io::ErrorKind::Unsupported,
                            "Unix links only",
                        ))
                    }
                })();
                if let Err(error) = result {
                    on_item(ItemUpdate::error(index, Some(target.clone()), &error));
                    return Err(batch_failure(
                        "Link",
                        index,
                        sources.len(),
                        &sources[index],
                        error,
                    ));
                }
                on_item(ItemUpdate::new(
                    index,
                    ItemState::Completed,
                    Some(target.clone()),
                ));
                transfer.transferred = (index + 1) as u64;
                transfer.report(target);
            }
            Ok(format!("Created {} link(s)", sources.len()))
        }
        Action::SetMode {
            path,
            mode,
            expected,
        } => {
            transfer.check()?;
            #[cfg(target_os = "linux")]
            {
                set_mode_guarded(path, *mode, expected)?;
                Ok(format!("Updated permissions to {mode:03o}"))
            }
            #[cfg(not(target_os = "linux"))]
            {
                Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "Unix permissions only",
                ))
            }
        }
        Action::SetOwnership {
            path,
            uid,
            gid,
            expected,
        } => {
            transfer.check()?;
            #[cfg(target_os = "linux")]
            {
                set_ownership_guarded(path, *uid, *gid, expected)?;
                Ok(format!("Updated owner/group to {uid}:{gid}"))
            }
            #[cfg(not(target_os = "linux"))]
            {
                Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "Owner/group changes require Linux",
                ))
            }
        }
        Action::CreateUserAttribute {
            path,
            name,
            value,
            expected,
        } => {
            transfer.check()?;
            #[cfg(target_os = "linux")]
            {
                create_user_attribute_guarded(path, name, value, expected)?;
                Ok(format!("Created attribute {name}"))
            }
            #[cfg(not(target_os = "linux"))]
            {
                Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "Extended attribute editing requires Linux",
                ))
            }
        }
    }
}

pub fn property_lines(path: &Path) -> io::Result<Vec<(String, String)>> {
    let metadata = fs::symlink_metadata(path)?;
    let kind = if metadata.file_type().is_symlink() {
        "Symbolic link"
    } else if metadata.is_dir() {
        "Folder"
    } else if metadata.is_file() {
        "File"
    } else {
        "Special device"
    };
    let mut rows = vec![
        ("Location".into(), path.display().to_string()),
        ("Type".into(), kind.into()),
        (
            "Size".into(),
            format!("{} ({} bytes)", human_size(metadata.len()), metadata.len()),
        ),
        ("Modified".into(), age(metadata.modified().ok())),
        (
            "Read-only".into(),
            metadata.permissions().readonly().to_string(),
        ),
    ];
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        rows.push((
            "Permissions".into(),
            format!("{:04o}", metadata.permissions().mode() & 0o7777),
        ));
        rows.push(("Owner UID".into(), metadata.uid().to_string()));
        rows.push(("Group GID".into(), metadata.gid().to_string()));
    }
    if metadata.file_type().is_symlink() {
        rows.push((
            "Link target".into(),
            fs::read_link(path)?.display().to_string(),
        ));
    }
    Ok(rows)
}

/// Decode the octal escapes in Linux mountinfo paths without changing non-UTF-8 bytes.
/// Invalid escapes (including NUL) are rejected instead of guessing a different path.
#[cfg(target_os = "linux")]
fn decode_mountinfo_path(raw: &[u8]) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStringExt;

    let mut decoded = Vec::with_capacity(raw.len());
    let mut index = 0;
    while index < raw.len() {
        if raw[index] != b'\\' {
            decoded.push(raw[index]);
            index += 1;
            continue;
        }
        let digits = raw.get(index + 1..index + 4)?;
        if !digits.iter().all(|byte| (b'0'..=b'7').contains(byte)) {
            return None;
        }
        let value = u16::from(digits[0] - b'0') * 64
            + u16::from(digits[1] - b'0') * 8
            + u16::from(digits[2] - b'0');
        if value == 0 || value > u16::from(u8::MAX) {
            return None;
        }
        decoded.push(value as u8);
        index += 4;
    }
    Some(PathBuf::from(OsString::from_vec(decoded)))
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MountedVolume {
    pub path: PathBuf,
    pub network: bool,
}

#[cfg(target_os = "linux")]
fn is_desktop_mount_path(mount: &[u8]) -> bool {
    mount == b"/mnt"
        || mount.starts_with(b"/mnt/")
        || mount.starts_with(b"/media/")
        || mount.starts_with(b"/run/media/")
}

/// Mount points useful to a desktop user, without presenting /proc and other pseudo-filesystems.
/// Enumeration is performed by the refresh worker, never by the render thread.
pub fn mounted_volumes() -> Vec<MountedVolume> {
    let mut paths = std::collections::BTreeSet::new();
    paths.insert((PathBuf::from("/"), false));
    #[cfg(target_os = "linux")]
    if let Ok(contents) = fs::read("/proc/self/mountinfo") {
        for line in contents.split(|byte| *byte == b'\n') {
            let Some(separator) = line.windows(3).position(|bytes| bytes == b" - ") else {
                continue;
            };
            let Some(mount) = line[..separator].split(|byte| *byte == b' ').nth(4) else {
                continue;
            };
            let fs_type = line[separator + 3..]
                .split(|byte| *byte == b' ')
                .next()
                .and_then(|bytes| std::str::from_utf8(bytes).ok())
                .unwrap_or("");
            let desktop_path = is_desktop_mount_path(mount);
            let remote = matches!(
                fs_type,
                "nfs" | "nfs4" | "cifs" | "fuse.sshfs" | "fuse.rclone" | "fuse.gvfsd-fuse"
            );
            if (desktop_path || remote) && !mount.is_empty() {
                let Some(path) = decode_mountinfo_path(mount) else {
                    continue;
                };
                // /proc/self/mountinfo is the source of truth. stat can fail
                // on a disconnected mount, which must remain visible for
                // diagnosis instead of silently disappearing from Volumes.
                if fs_type == "fuse.gvfsd-fuse"
                    && let Ok(entries) = fs::read_dir(&path)
                {
                    for entry in entries.flatten() {
                        if entry.path().is_dir() {
                            paths.insert((entry.path(), true));
                        }
                    }
                }
                paths.insert((path, remote));
            }
        }
    }
    paths
        .into_iter()
        .map(|(path, network)| MountedVolume { path, network })
        .collect()
}

fn free_destination_reserved(
    parent: &Path,
    original: &OsStr,
    reserved: &BTreeSet<PathBuf>,
) -> io::Result<PathBuf> {
    let first = parent.join(original);
    if !reserved.contains(&first) && !occupied(&first)? {
        return Ok(first);
    }
    let name = Path::new(original);
    let stem = name.file_stem().unwrap_or(original);
    let extension = name.extension();
    for index in 1..10_000 {
        let suffix = if index == 1 {
            " (copy)".to_owned()
        } else {
            format!(" (copy {index})")
        };
        let mut candidate = OsString::from(stem);
        candidate.push(suffix);
        if let Some(extension) = extension {
            candidate.push(".");
            candidate.push(extension);
        }
        let candidate = parent.join(candidate);
        if !reserved.contains(&candidate) && !occupied(&candidate)? {
            return Ok(candidate);
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "Too many duplicate names",
    ))
}

/// A read-only, best-effort preview. No name reservation or overwrite occurs;
/// the worker re-runs the plan immediately before executing a batch. The
/// no-replace publication/rename is still the authority under concurrent I/O.
pub fn preview_transfer(
    sources: &[PathBuf],
    destination: &Path,
    conflict: ConflictPolicy,
    moving: bool,
) -> io::Result<TransferPlan> {
    let cancelled = AtomicBool::new(false);
    plan_transfer(sources, destination, conflict, moving, &cancelled).map_err(|failure| {
        io::Error::new(
            failure.error.kind(),
            match failure.index {
                Some(index) => format!(
                    "Cannot plan item {} of {} ({}): {}",
                    index + 1,
                    sources.len(),
                    sources[index].display(),
                    failure.error
                ),
                None => format!("Cannot plan transfer: {}", failure.error),
            },
        )
    })
}

fn plan_transfer(
    sources: &[PathBuf],
    destination: &Path,
    conflict: ConflictPolicy,
    moving: bool,
    cancelled: &AtomicBool,
) -> Result<TransferPlan, PlanFailure> {
    let destination_meta = fs::metadata(destination).map_err(PlanFailure::global)?;
    if !destination_meta.is_dir() {
        return Err(PlanFailure::global(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "Transfer destination is not a folder: {}",
                destination.display()
            ),
        )));
    }
    let canonical_destination = fs::canonicalize(destination).map_err(PlanFailure::global)?;

    // Resolve parent directories, but not the final entry: a selected symlink
    // is its own source, not an alias for the link target.
    let mut snapshots = Vec::with_capacity(sources.len());
    let mut selected = BTreeSet::new();
    let mut selected_directories = BTreeSet::new();
    for (index, source) in sources.iter().enumerate() {
        if cancelled.load(Ordering::Relaxed) {
            return Err(PlanFailure::at(
                index,
                io::Error::new(
                    io::ErrorKind::Interrupted,
                    "Operation cancelled during preflight",
                ),
            ));
        }
        let name = source
            .file_name()
            .ok_or_else(|| PlanFailure::at(index, invalid_name()))?;
        let snapshot = fs::symlink_metadata(source).map_err(|e| PlanFailure::at(index, e))?;
        let parent_path = source
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let parent = fs::canonicalize(parent_path).map_err(|e| PlanFailure::at(index, e))?;
        let normalized = parent.join(name);
        if !selected.insert(normalized.clone()) {
            return Err(PlanFailure::at(
                index,
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("Source was selected twice: {}", source.display()),
                ),
            ));
        }
        if snapshot.is_dir() {
            selected_directories.insert(normalized.clone());
        }
        snapshots.push((normalized, snapshot));
    }
    for (index, (normalized, _)) in snapshots.iter().enumerate() {
        if normalized
            .ancestors()
            .skip(1)
            .any(|ancestor| selected_directories.contains(ancestor))
        {
            return Err(PlanFailure::at(
                index,
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "An item is also inside another selected folder; split the transfer to avoid overlapping sources",
                ),
            ));
        }
    }

    let mut reserved = BTreeSet::new();
    let mut plan = TransferPlan {
        items: Vec::with_capacity(sources.len()),
        collisions: 0,
        renamed: 0,
        skipped: 0,
    };
    for (index, (source, (normalized, snapshot))) in sources.iter().zip(snapshots).enumerate() {
        if cancelled.load(Ordering::Relaxed) {
            return Err(PlanFailure::at(
                index,
                io::Error::new(
                    io::ErrorKind::Interrupted,
                    "Operation cancelled during preflight",
                ),
            ));
        }
        let name = source
            .file_name()
            .ok_or_else(|| PlanFailure::at(index, invalid_name()))?;
        let intended = destination.join(name);
        // A move into its own parent is a deliberate no-op, regardless of the
        // collision policy; never rename the original to a "copy" name.
        let same_parent = moving && normalized.parent() == Some(canonical_destination.as_path());
        if !same_parent {
            ensure_outside_source(source, &intended).map_err(|e| PlanFailure::at(index, e))?;
        }
        let collision = reserved.contains(&intended)
            || occupied(&intended).map_err(|e| PlanFailure::at(index, e))?;
        if collision {
            plan.collisions += 1;
        }
        let (target, disposition) = if same_parent {
            (source.clone(), PlannedDisposition::Skip)
        } else if collision && conflict == ConflictPolicy::SkipExisting {
            (intended.clone(), PlannedDisposition::Skip)
        } else if collision && conflict == ConflictPolicy::AbortOnConflict {
            return Err(PlanFailure::at(
                index,
                io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!("Destination already exists: {}", intended.display()),
                ),
            ));
        } else if collision && conflict == ConflictPolicy::SkipIfIdentical {
            if reserved.contains(&intended) {
                return Err(PlanFailure::at(
                    index,
                    io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        "Multiple selected sources need the same destination; choose Keep both",
                    ),
                ));
            }
            match compare_for_recovery(source, &intended, cancelled) {
                Comparison::Identical { .. } => (intended.clone(), PlannedDisposition::Skip),
                Comparison::Different(reason) => {
                    return Err(PlanFailure::at(
                        index,
                        io::Error::new(
                            io::ErrorKind::AlreadyExists,
                            format!(
                                "Existing destination differs from source: {reason}. Choose Keep both or Skip existing"
                            ),
                        ),
                    ));
                }
                Comparison::Inconclusive(reason) => {
                    return Err(PlanFailure::at(
                        index,
                        io::Error::other(format!(
                            "Cannot verify existing destination safely: {reason}"
                        )),
                    ));
                }
            }
        } else {
            let target = free_destination_reserved(destination, name, &reserved)
                .map_err(|e| PlanFailure::at(index, e))?;
            reserved.insert(target.clone());
            (target, PlannedDisposition::Transfer)
        };
        if disposition == PlannedDisposition::Skip {
            plan.skipped += 1;
        }
        let renamed = disposition == PlannedDisposition::Transfer && target != intended;
        if renamed {
            plan.renamed += 1;
        }
        plan.items.push(PlannedItem {
            source: source.clone(),
            target,
            disposition,
            collision,
            renamed,
            snapshot,
        });
    }
    Ok(plan)
}

fn estimate_bytes(path: &Path, cancelled: &AtomicBool, depth: usize) -> io::Result<u64> {
    if cancelled.load(Ordering::Relaxed) {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "Operation cancelled",
        ));
    }
    if depth > 128 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Folder nesting is too deep",
        ));
    }
    let metadata = fs::symlink_metadata(path)?;
    if metadata.is_dir() {
        let mut total = 0u64;
        for item in fs::read_dir(path)? {
            total = total.saturating_add(estimate_bytes(&item?.path(), cancelled, depth + 1)?);
        }
        Ok(total)
    } else {
        Ok(metadata.len())
    }
}

fn ensure_outside_source(source: &Path, target: &Path) -> io::Result<()> {
    if fs::symlink_metadata(source)?.is_dir() {
        let canonical = fs::canonicalize(source)?;
        let parent = fs::canonicalize(target.parent().ok_or_else(invalid_name)?)?;
        if parent.starts_with(canonical) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Cannot copy or move a folder inside itself",
            ));
        }
    }
    Ok(())
}

fn remove_created(path: &Path) -> io::Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if metadata.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}

/// Unlike cleanup of staging paths, user-requested deletion must report
/// missing inputs rather than silently claiming that they were deleted.
fn remove_selected(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}

/// Delete only source entries that have a matching published destination.
/// Unlike remove_dir_all, a concurrent writer adding an unexpected entry
/// prevents the final rmdir and leaves that entry available for recovery.
const MAX_MOVE_MANIFEST_ENTRIES: usize = 250_000;

fn capture_source_manifest(
    path: &Path,
    depth: usize,
    manifest: &mut BTreeMap<PathBuf, fs::Metadata>,
    transfer: &mut Transfer<'_>,
) -> io::Result<()> {
    transfer.check()?;
    if depth > 128 || manifest.len() >= MAX_MOVE_MANIFEST_ENTRIES {
        return Err(io::Error::other(
            "Move has too many entries or nested directories to track safely",
        ));
    }
    let metadata = fs::symlink_metadata(path)?;
    if metadata.is_dir() {
        for item in fs::read_dir(path)? {
            capture_source_manifest(&item?.path(), depth + 1, manifest, transfer)?;
        }
        ensure_same_source(path, &metadata)?;
    }
    manifest.insert(path.to_path_buf(), metadata);
    Ok(())
}

/// A cross-device Move cannot retain the identity of a hardlink whose other
/// names live outside the single staged source tree. A Copy may intentionally
/// create an independent inode, but deleting the original during Move would
/// silently break that external relationship. Refuse the Move before staging.
fn ensure_hardlink_scope_preserved(manifest: &BTreeMap<PathBuf, fs::Metadata>) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        let mut copied_names = BTreeMap::<(u64, u64), u64>::new();
        for metadata in manifest.values().filter(|meta| meta.is_file()) {
            *copied_names
                .entry((metadata.dev(), metadata.ino()))
                .or_default() += 1;
        }
        for (path, metadata) in manifest {
            if metadata.file_type().is_symlink() && metadata.nlink() > 1 {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    format!(
                        "Cannot preserve a hardlinked symlink during cross-device Move: {}",
                        path.display()
                    ),
                ));
            }
            if metadata.is_file()
                && copied_names
                    .get(&(metadata.dev(), metadata.ino()))
                    .copied()
                    .unwrap_or_default()
                    != metadata.nlink()
            {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    format!(
                        "Cross-device Move would break hardlinks outside the selected source tree: {}",
                        path.display()
                    ),
                ));
            }
        }
    }
    #[cfg(not(unix))]
    let _ = manifest;
    Ok(())
}

#[cfg(test)]
fn verify_source_manifest(
    manifest: &BTreeMap<PathBuf, fs::Metadata>,
    transfer: &mut Transfer<'_>,
) -> io::Result<()> {
    for (path, snapshot) in manifest {
        transfer.check()?;
        ensure_same_source(path, snapshot)?;
    }
    Ok(())
}

/// Unlinking an earlier name of a hardlinked inode changes the inode's ctime
/// and link count for its remaining names. Accept only that *known* effect
/// of our own earlier unlinks; require the same inode, bytes, mode and mtime.
#[cfg(unix)]
#[cfg(test)]
fn ensure_cleanup_source(
    original: &fs::Metadata,
    current: &fs::Metadata,
    removed_links: &BTreeSet<(u64, u64)>,
) -> io::Result<()> {
    use std::os::unix::fs::MetadataExt;

    if ensure_same_source_metadata(original, current).is_ok() {
        return Ok(());
    }
    let key = (original.dev(), original.ino());
    if original.is_file()
        && current.is_file()
        && removed_links.contains(&key)
        && original.nlink() > current.nlink()
        && current.nlink() > 0
        && original.size() == current.size()
        && original.mode() == current.mode()
        && original.mtime() == current.mtime()
        && original.mtime_nsec() == current.mtime_nsec()
        && original.dev() == current.dev()
        && original.ino() == current.ino()
    {
        return Ok(());
    }
    ensure_same_source_metadata(original, current)
}

#[cfg(test)]
fn remove_verified_tree(
    source: &Path,
    destination: &Path,
    depth: usize,
    manifest: &BTreeMap<PathBuf, fs::Metadata>,
    removed_links: &mut BTreeSet<(u64, u64)>,
    transfer: &mut Transfer<'_>,
) -> io::Result<()> {
    transfer.check()?;
    if depth > 128 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Folder nesting is too deep",
        ));
    }
    let metadata = fs::symlink_metadata(source)?;
    let original = manifest.get(source).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "Source was not in the original move",
        )
    })?;
    #[cfg(unix)]
    ensure_cleanup_source(original, &metadata, removed_links)?;
    #[cfg(not(unix))]
    ensure_same_source_metadata(original, &metadata)?;
    if metadata.is_dir() {
        if !fs::symlink_metadata(destination)?.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Published destination differs from source",
            ));
        }
        for entry in fs::read_dir(destination)? {
            let entry = entry?;
            remove_verified_tree(
                &source.join(entry.file_name()),
                &entry.path(),
                depth + 1,
                manifest,
                removed_links,
                transfer,
            )?;
        }
        fs::remove_dir(source)
    } else {
        // Reverify each file immediately before unlinking it. The final
        // source path still may be mutated by another process; crash-safe
        // identity tracking and journaling remain required for a 1.0 gate.
        verify_tree(source, destination, depth, transfer)?;
        fs::remove_file(source)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if original.is_file() && original.nlink() > 1 {
                removed_links.insert((original.dev(), original.ino()));
            }
        }
        Ok(())
    }
}

fn create_staging_dir(parent: &Path) -> io::Result<PathBuf> {
    use std::time::UNIX_EPOCH;

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    for attempt in 0..128 {
        let path = parent.join(format!(
            ".ricefield-staging-{}-{nonce}-{attempt}",
            std::process::id()
        ));
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        match builder.create(&path) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "Unable to reserve a unique staging directory",
    ))
}

/// Copy into a private, hidden directory on the destination filesystem, then publish
/// only a complete payload with an atomic, no-overwrite rename. A killed process may
/// leave an abandoned staging directory. The staging and destination parent
/// directories are synced to make their entries durable where supported.
fn stage_and_publish(
    source: &Path,
    destination: &Path,
    verify: bool,
    transfer: &mut Transfer<'_>,
) -> io::Result<()> {
    stage_and_publish_with_journal(source, destination, verify, transfer, None)
}

fn stage_and_publish_with_journal(
    source: &Path,
    destination: &Path,
    verify: bool,
    transfer: &mut Transfer<'_>,
    journal: Option<&mut Journal>,
) -> io::Result<()> {
    stage_and_publish_with_sync(
        source,
        destination,
        verify,
        transfer,
        journal,
        &mut sync_directory,
    )
}

fn sync_directory(path: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()
}

/// Separating sync barriers makes post-publication failure injection
/// deterministic without global test state or modifying user files.
fn stage_and_publish_with_sync(
    source: &Path,
    destination: &Path,
    verify: bool,
    transfer: &mut Transfer<'_>,
    mut journal: Option<&mut Journal>,
    sync: &mut dyn FnMut(&Path) -> io::Result<()>,
) -> io::Result<()> {
    transfer.check()?;
    ensure_outside_source(source, destination)?;
    let staging_dir = create_staging_dir(destination.parent().ok_or_else(invalid_name)?)?;
    if let Some(record) = journal.as_mut()
        && let Err(error) = record.set_staging(&staging_dir)
    {
        if let Err(cleanup_error) = fs::remove_dir(&staging_dir) {
            eprintln!("RiceField: cannot clean staging directory: {cleanup_error}");
        }
        return Err(error);
    }
    let staged = staging_dir.join("payload");
    let result = (|| {
        let original = fs::symlink_metadata(source)?;
        transfer.phase = "Copying";
        copy_tree(source, &staged, 0, transfer, &mut CopyContext::default())?;
        if verify {
            transfer.phase = "Verifying";
            transfer.report(source);
            verify_tree(source, &staged, 0, transfer)?;
        }
        // copy_tree has synced regular files and copied subdirectories.
        // Persist the staging entry before a journal can claim it was verified.
        sync(&staging_dir)?;
        if verify && let Some(record) = journal.as_mut() {
            record.advance(State::StagedVerified)?;
        }
        ensure_same_source(source, &original)?;
        transfer.check()?;
        transfer.phase = "Publishing";
        transfer.report(destination);
        rename_without_overwrite(&staged, destination)?;
        // A successful rename alone is not a power-loss durability barrier.
        // Report uncertainty if the parent cannot be synced; most importantly,
        // a cross-device Move must not delete its source in this situation.
        sync(destination.parent().ok_or_else(invalid_name)?).map_err(|error| {
            io::Error::new(
                error.kind(),
                format!(
                    "Destination was published, but directory sync failed: {error}. Verify both paths before retrying; source was not removed"
                ),
            )
        })?;
        if let Some(record) = journal.as_mut() {
            // No source entry may be removed before this transition is durable.
            record.advance(State::Published)?;
        }
        Ok(())
    })();
    match result {
        Ok(()) => {
            // Publication is committed; a failed cleanup must not report it as failed.
            if let Err(error) = fs::remove_dir(&staging_dir) {
                eprintln!(
                    "RiceField: published copy, but cannot remove staging directory: {error}"
                );
            }
            Ok(())
        }
        Err(error) => {
            if let Err(cleanup_error) = remove_created(&staging_dir) {
                eprintln!("RiceField: cannot clean staging directory: {cleanup_error}");
            }
            Err(error)
        }
    }
}

/// Check every copied byte before a cross-filesystem move deletes its source.
/// This intentionally costs extra I/O for moves across drives.
fn verify_tree(
    source: &Path,
    destination: &Path,
    depth: usize,
    transfer: &mut Transfer<'_>,
) -> io::Result<()> {
    transfer.check()?;
    if depth > 128 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Folder nesting is too deep",
        ));
    }
    let mismatch = || {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "Move verification failed; source is untouched",
        )
    };
    let source_meta = fs::symlink_metadata(source)?;
    let target_meta = fs::symlink_metadata(destination).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            mismatch()
        } else {
            error
        }
    })?;
    // Identical bytes are not sufficient to authorize source deletion: a
    // cross-device Move must not silently replace ownership, permissions,
    // timestamps or extended metadata with a lossy copy.
    verify_copied_metadata(source, destination, &source_meta, &target_meta)?;
    if source_meta.file_type().is_symlink() {
        if !target_meta.file_type().is_symlink()
            || fs::read_link(source)? != fs::read_link(destination)?
        {
            return Err(mismatch());
        }
        ensure_same_source(source, &source_meta)?;
        ensure_same_source(destination, &target_meta)?;
    } else if source_meta.is_dir() {
        if !target_meta.is_dir() {
            return Err(mismatch());
        }
        let mut count = 0usize;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            count += 1;
            verify_tree(
                &entry.path(),
                &destination.join(entry.file_name()),
                depth + 1,
                transfer,
            )?;
        }
        if count != fs::read_dir(destination)?.count() {
            return Err(mismatch());
        }
        ensure_same_source(source, &source_meta)?;
        ensure_same_source(destination, &target_meta)?;
    } else if source_meta.is_file() {
        if !target_meta.is_file() || source_meta.len() != target_meta.len() {
            return Err(mismatch());
        }
        // Verify the descriptor as well as the path: a file exchanged for a
        // symlink or FIFO after lstat must never be followed or block the UI.
        let mut source_reader = open_verified_regular(source, &source_meta)?;
        let mut target_reader = open_verified_regular(destination, &target_meta)?;
        let mut a = [0u8; 256 * 1024];
        let mut b = [0u8; 256 * 1024];
        loop {
            transfer.check()?;
            let count = source_reader.read(&mut a)?;
            if count == 0 {
                break;
            }
            target_reader.read_exact(&mut b[..count])?;
            if a[..count] != b[..count] {
                return Err(mismatch());
            }
        }
        ensure_same_source_metadata(&source_meta, &source_reader.metadata()?)?;
        ensure_same_source_metadata(&target_meta, &target_reader.metadata()?)?;
        ensure_same_source(source, &source_meta)?;
        ensure_same_source(destination, &target_meta)?;
    } else {
        return Err(mismatch());
    }
    Ok(())
}

fn verify_copied_metadata(
    source: &Path,
    destination: &Path,
    source_meta: &fs::Metadata,
    target_meta: &fs::Metadata,
) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        if source_meta.mode() != target_meta.mode()
            || source_meta.uid() != target_meta.uid()
            || source_meta.gid() != target_meta.gid()
            || source_meta.mtime() != target_meta.mtime()
            || source_meta.mtime_nsec() != target_meta.mtime_nsec()
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Move verification failed: ownership, permissions or modification time differs; source is untouched",
            ));
        }
    }
    #[cfg(target_os = "linux")]
    verify_copied_xattrs(source, destination)?;
    #[cfg(not(target_os = "linux"))]
    let _ = (source, destination, source_meta, target_meta);
    Ok(())
}

/// Compare all visible namespaces, not only the user.* attributes we can
/// currently copy. A source ACL, capability or security label that would be
/// lost must stop a destructive Move even when the file bytes are identical.
#[cfg(target_os = "linux")]
fn verify_copied_xattrs(source: &Path, destination: &Path) -> io::Result<()> {
    let source_names = list_link_xattrs(source)?;
    let target_names = list_link_xattrs(destination)?;
    if source_names != target_names {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Move verification failed: extended-attribute names differ; source is untouched",
        ));
    }
    for name in source_names {
        if read_link_xattr(source, &name)? != read_link_xattr(destination, &name)? {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Move verification failed: extended-attribute values differ; source is untouched",
            ));
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn list_link_xattrs(path: &Path) -> io::Result<BTreeSet<Vec<u8>>> {
    const MAX_NAMES: usize = 64 * 1024;
    let path = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "NUL in path"))?;
    let count = unsafe { libc::llistxattr(path.as_ptr(), std::ptr::null_mut(), 0) };
    if count < 0 {
        return Err(io::Error::last_os_error());
    }
    if count as usize > MAX_NAMES {
        return Err(io::Error::other(
            "Too many extended-attribute names to verify",
        ));
    }
    if count == 0 {
        return Ok(BTreeSet::new());
    }
    let mut names = vec![0u8; count as usize];
    let actual = unsafe { libc::llistxattr(path.as_ptr(), names.as_mut_ptr().cast(), names.len()) };
    if actual < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(names[..actual as usize]
        .split(|byte| *byte == 0)
        .filter(|name| !name.is_empty())
        .map(Vec::from)
        .collect())
}

#[cfg(target_os = "linux")]
fn read_link_xattr(path: &Path, name: &[u8]) -> io::Result<Vec<u8>> {
    const MAX_VALUE: usize = 1024 * 1024;
    let path = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "NUL in path"))?;
    let name = CString::new(name)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "Invalid xattr name"))?;
    let count = unsafe { libc::lgetxattr(path.as_ptr(), name.as_ptr(), std::ptr::null_mut(), 0) };
    if count < 0 {
        return Err(io::Error::last_os_error());
    }
    if count as usize > MAX_VALUE {
        return Err(io::Error::other(
            "Extended-attribute value is too large to verify",
        ));
    }
    let mut value = vec![0u8; count as usize];
    let actual = unsafe {
        libc::lgetxattr(
            path.as_ptr(),
            name.as_ptr(),
            value.as_mut_ptr().cast(),
            value.len(),
        )
    };
    if actual < 0 {
        return Err(io::Error::last_os_error());
    }
    if actual != count {
        return Err(io::Error::other(
            "Extended attribute changed during verification",
        ));
    }
    Ok(value)
}

/// Open only the regular inode observed at lstat time. O_NONBLOCK prevents a
/// replaced FIFO/device from hanging a verification worker before fstat.
fn open_verified_regular(path: &Path, snapshot: &fs::Metadata) -> io::Result<File> {
    if !snapshot.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Verification expects a regular file",
        ));
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    ensure_same_source_metadata(snapshot, &file.metadata()?)?;
    Ok(file)
}

/// Tracks links within one source tree. Separate top-level paste items are
/// independently published and cannot share this staging map.
#[derive(Default)]
struct CopyContext {
    #[cfg(unix)]
    hardlinks: BTreeMap<(u64, u64), (PathBuf, fs::Metadata)>,
}

fn copy_tree(
    source: &Path,
    destination: &Path,
    depth: usize,
    transfer: &mut Transfer<'_>,
    context: &mut CopyContext,
) -> io::Result<()> {
    transfer.check()?;
    if depth > 128 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Folder nesting is too deep",
        ));
    }
    let metadata = fs::symlink_metadata(source)?;
    if metadata.file_type().is_symlink() {
        #[cfg(unix)]
        {
            let target = fs::read_link(source)?;
            ensure_same_source(source, &metadata)?;
            std::os::unix::fs::symlink(target, destination)?;
            #[cfg(target_os = "linux")]
            if let Err(error) = preserve_available_owner(destination, &metadata) {
                let _ = fs::remove_file(destination);
                return Err(error);
            }
            #[cfg(target_os = "linux")]
            if let Err(error) = copy_user_xattrs(source, destination, false) {
                let _ = fs::remove_file(destination);
                return Err(error);
            }
            #[cfg(target_os = "linux")]
            if let Err(error) = preserve_symlink_times(destination, &metadata) {
                let _ = fs::remove_file(destination);
                return Err(error);
            }
            ensure_same_source(source, &metadata)?;
            transfer.advance(metadata.len(), source);
            return Ok(());
        }
        #[cfg(not(unix))]
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Symlink copy unavailable",
        ));
    }
    if metadata.is_dir() {
        ensure_outside_source(source, destination)?;
        fs::create_dir(destination)?;
        let result = (|| {
            for item in fs::read_dir(source)? {
                let item = item?;
                copy_tree(
                    &item.path(),
                    &destination.join(item.file_name()),
                    depth + 1,
                    transfer,
                    context,
                )?;
            }
            ensure_same_source(source, &metadata)?;
            let output = File::open(destination)?;
            #[cfg(target_os = "linux")]
            preserve_available_owner(destination, &metadata)?;
            #[cfg(target_os = "linux")]
            copy_user_xattrs(source, destination, true)?;
            // Installing read-only source permissions first could prevent us
            // from writing otherwise accessible user.* metadata.
            output.set_permissions(metadata.permissions())?;
            #[cfg(target_os = "linux")]
            copy_posix_acls(source, destination)?;
            output.set_times(source_times(&metadata))?;
            ensure_same_source(source, &metadata)?;
            output.sync_all()?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_dir_all(destination);
        }
        result
    } else if metadata.is_file() {
        #[cfg(unix)]
        let hardlink_key = {
            use std::os::unix::fs::MetadataExt;
            (metadata.nlink() > 1).then_some((metadata.dev(), metadata.ino()))
        };
        #[cfg(unix)]
        if let Some((existing, first_snapshot)) =
            hardlink_key.and_then(|key| context.hardlinks.get(&key))
        {
            // Keep a shared inode within the copied tree. Only use links
            // created under our private staging directory. If the inode was
            // modified since its first name was copied, linking the old
            // staged payload under another name would silently copy stale data.
            ensure_same_source_metadata(first_snapshot, &metadata)?;
            ensure_same_source(source, &metadata)?;
            fs::hard_link(existing, destination)?;
            transfer.advance(metadata.len(), source);
            return Ok(());
        }
        // Also refuse a FIFO/device swapped in between lstat and open; a
        // blocking open here would otherwise freeze a transfer worker.
        let mut input = open_verified_regular(source, &metadata)?;
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(destination)?;
        let result = (|| {
            let mut buffer = [0u8; 256 * 1024];
            loop {
                transfer.check()?;
                let n = input.read(&mut buffer)?;
                if n == 0 {
                    break;
                }
                output.write_all(&buffer[..n])?;
                transfer.advance(n as u64, source);
            }
            ensure_same_source_metadata(&metadata, &input.metadata()?)?;
            ensure_same_source(source, &metadata)?;
            #[cfg(target_os = "linux")]
            preserve_available_owner(destination, &metadata)?;
            #[cfg(target_os = "linux")]
            copy_user_xattrs(source, destination, true)?;
            output.set_permissions(metadata.permissions())?;
            #[cfg(target_os = "linux")]
            copy_posix_acls(source, destination)?;
            output.set_times(source_times(&metadata))?;
            ensure_same_source(source, &metadata)?;
            output.sync_all()?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(destination);
        } else {
            #[cfg(unix)]
            if let Some(key) = hardlink_key {
                context
                    .hardlinks
                    .insert(key, (destination.to_path_buf(), metadata));
            }
        }
        result
    } else {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Special device files cannot be copied",
        ))
    }
}

/// Try to retain the original uid/gid when the process has permission to do
/// so. A plain Copy remains allowed for an unprivileged reader of a file
/// owned by somebody else; a cross-device Move additionally runs strict
/// metadata verification and refuses to delete that source on mismatch.
#[cfg(target_os = "linux")]
fn preserve_available_owner(destination: &Path, source: &fs::Metadata) -> io::Result<()> {
    use std::os::unix::fs::MetadataExt;

    let existing = fs::symlink_metadata(destination)?;
    if existing.uid() == source.uid() && existing.gid() == source.gid() {
        return Ok(());
    }
    let destination = CString::new(destination.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "NUL in destination path"))?;
    let status = unsafe {
        libc::fchownat(
            libc::AT_FDCWD,
            destination.as_ptr(),
            source.uid(),
            source.gid(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if status == 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    if matches!(error.raw_os_error(), Some(libc::EPERM) | Some(libc::EACCES)) {
        return Ok(());
    }
    Err(error)
}

/// POSIX ACLs are stored in the system namespace, unlike security labels or
/// capabilities (which must never be blindly replayed). Apply ACLs *after*
/// chmod because chmod may update the ACL mask, but *before* restoring mtime.
/// Also discard any default/access ACL inherited from the staging directory
/// when the original has no matching ACL.
#[cfg(target_os = "linux")]
fn copy_posix_acls(source: &Path, destination: &Path) -> io::Result<()> {
    const ACL_NAMES: [&[u8]; 2] = [b"system.posix_acl_access", b"system.posix_acl_default"];
    // Filesystems with no xattr support cannot store POSIX ACLs either.
    // Match copy_user_xattrs' existing behavior for those mounts rather than
    // rejecting otherwise valid plain copies.
    let list_names = |path| match list_link_xattrs(path) {
        Err(error) if error.raw_os_error() == Some(libc::EOPNOTSUPP) => Ok(BTreeSet::new()),
        result => result,
    };
    let source_names = list_names(source)?;
    let destination_names = list_names(destination)?;
    let path = CString::new(destination.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "NUL in destination path"))?;

    for name in ACL_NAMES {
        let key = CString::new(name).expect("static ACL name has no NUL");
        if source_names.contains(name) {
            let value = read_link_xattr(source, name)?;
            let status = unsafe {
                libc::setxattr(
                    path.as_ptr(),
                    key.as_ptr(),
                    value.as_ptr().cast(),
                    value.len(),
                    0,
                )
            };
            if status != 0 {
                return Err(io::Error::last_os_error());
            }
        } else if destination_names.contains(name) {
            let status = unsafe { libc::removexattr(path.as_ptr(), key.as_ptr()) };
            if status != 0 {
                return Err(io::Error::last_os_error());
            }
        }
    }
    Ok(())
}

/// utimensat with AT_SYMLINK_NOFOLLOW applies the original timestamps to the
/// link inode rather than accidentally mutating its target (which may be absent).
#[cfg(target_os = "linux")]
fn preserve_symlink_times(destination: &Path, metadata: &fs::Metadata) -> io::Result<()> {
    use std::os::unix::fs::MetadataExt;

    let destination = CString::new(destination.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "NUL in link path"))?;
    let times = [
        libc::timespec {
            tv_sec: metadata.atime(),
            tv_nsec: metadata.atime_nsec(),
        },
        libc::timespec {
            tv_sec: metadata.mtime(),
            tv_nsec: metadata.mtime_nsec(),
        },
    ];
    let status = unsafe {
        libc::utimensat(
            libc::AT_FDCWD,
            destination.as_ptr(),
            times.as_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if status == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Copy the unprivileged Linux user namespace without silently stripping
/// user metadata on a cross-device Move. Unsupported destination xattrs fail
/// the staged copy, leaving the source untouched. ACL/security namespaces
/// still require a separate, privilege-aware implementation.
#[cfg(target_os = "linux")]
fn copy_user_xattrs(source: &Path, destination: &Path, follow: bool) -> io::Result<()> {
    const MAX_NAMES: usize = 64 * 1024;
    const MAX_VALUE: usize = 1024 * 1024;
    let left = CString::new(source.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "NUL in source path"))?;
    let right = CString::new(destination.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "NUL in destination path"))?;
    let count = unsafe {
        if follow {
            libc::listxattr(left.as_ptr(), std::ptr::null_mut(), 0)
        } else {
            libc::llistxattr(left.as_ptr(), std::ptr::null_mut(), 0)
        }
    };
    if count < 0 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::EOPNOTSUPP) {
            return Ok(());
        }
        return Err(error);
    }
    if count as usize > MAX_NAMES {
        return Err(io::Error::other(
            "Too many extended-attribute names to copy safely",
        ));
    }
    if count == 0 {
        return Ok(());
    }
    let mut names = vec![0u8; count as usize];
    let actual = unsafe {
        if follow {
            libc::listxattr(left.as_ptr(), names.as_mut_ptr().cast(), names.len())
        } else {
            libc::llistxattr(left.as_ptr(), names.as_mut_ptr().cast(), names.len())
        }
    };
    if actual < 0 {
        return Err(io::Error::last_os_error());
    }
    for name in names[..actual as usize].split(|byte| *byte == 0) {
        if !name.starts_with(b"user.") {
            continue;
        }
        let key = CString::new(name)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "Invalid xattr name"))?;
        let length = unsafe {
            if follow {
                libc::getxattr(left.as_ptr(), key.as_ptr(), std::ptr::null_mut(), 0)
            } else {
                libc::lgetxattr(left.as_ptr(), key.as_ptr(), std::ptr::null_mut(), 0)
            }
        };
        if length < 0 {
            return Err(io::Error::last_os_error());
        }
        if length as usize > MAX_VALUE {
            return Err(io::Error::other("Extended-attribute value is too large"));
        }
        let mut value = vec![0u8; length as usize];
        let read = unsafe {
            if follow {
                libc::getxattr(
                    left.as_ptr(),
                    key.as_ptr(),
                    value.as_mut_ptr().cast(),
                    value.len(),
                )
            } else {
                libc::lgetxattr(
                    left.as_ptr(),
                    key.as_ptr(),
                    value.as_mut_ptr().cast(),
                    value.len(),
                )
            }
        };
        if read < 0 {
            return Err(io::Error::last_os_error());
        }
        if read != length {
            return Err(io::Error::other("Extended attribute changed during copy"));
        }
        let written = unsafe {
            if follow {
                libc::setxattr(
                    right.as_ptr(),
                    key.as_ptr(),
                    value.as_ptr().cast(),
                    value.len(),
                    0,
                )
            } else {
                libc::lsetxattr(
                    right.as_ptr(),
                    key.as_ptr(),
                    value.as_ptr().cast(),
                    value.len(),
                    0,
                )
            }
        };
        if written != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

/// Reject a replaced or modified source before exposing a staged copy.
/// This is a best-effort non-snapshot check, not an atomic lock on other writers.
fn ensure_same_source(path: &Path, initial: &fs::Metadata) -> io::Result<()> {
    ensure_same_source_metadata(initial, &fs::symlink_metadata(path)?)
}

fn same_inode_and_kind(before: &fs::Metadata, after: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        before.dev() == after.dev()
            && before.ino() == after.ino()
            && before.mode() & libc::S_IFMT == after.mode() & libc::S_IFMT
    }
    #[cfg(not(unix))]
    {
        let _ = (before, after);
        false
    }
}

fn ensure_same_source_metadata(initial: &fs::Metadata, current: &fs::Metadata) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if initial.dev() != current.dev()
            || initial.ino() != current.ino()
            || initial.mtime() != current.mtime()
            || initial.mtime_nsec() != current.mtime_nsec()
            || initial.ctime() != current.ctime()
            || initial.ctime_nsec() != current.ctime_nsec()
            || initial.size() != current.size()
            || initial.mode() != current.mode()
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Selected source changed since preflight; refusing to process it",
            ));
        }
    }
    #[cfg(not(unix))]
    if initial.len() != current.len() || initial.modified().ok() != current.modified().ok() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Selected source changed since preflight; refusing to process it",
        ));
    }
    Ok(())
}

fn source_times(metadata: &fs::Metadata) -> FileTimes {
    let mut times = FileTimes::new();
    if let Ok(modified) = metadata.modified() {
        times = times.set_modified(modified);
    }
    if let Ok(accessed) = metadata.accessed() {
        times = times.set_accessed(accessed);
    }
    times
}

pub fn human_size(bytes: u64) -> String {
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = "B";
    for next in ["KiB", "MiB", "GiB", "TiB"] {
        value /= 1024.0;
        unit = next;
        if value < 1024.0 {
            break;
        }
    }
    format!("{value:.1} {unit}")
}

pub fn age(modified: Option<SystemTime>) -> String {
    let Some(time) = modified else {
        return "—".into();
    };
    let Ok(span) = SystemTime::now().duration_since(time) else {
        return "Just now".into();
    };
    let secs = span.as_secs();
    match secs {
        0..=59 => "Just now".into(),
        60..=3599 => format!("{}m ago", secs / 60),
        3600..=86399 => format!("{}h ago", secs / 3600),
        86400..=604799 => format!("{}d ago", secs / 86400),
        _ => format!("{}w ago", secs / 604800),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::UNIX_EPOCH;

    #[test]
    fn file_type_badges_cover_common_formats_case_insensitively() {
        for (name, kind) in [
            ("photo.BMP", "Image"),
            ("recording.OPUS", "Audio"),
            ("movie.MP4", "Video"),
            ("bundle.7Z", "Archive"),
            ("main.RS", "Code"),
            ("report.PDF", "PDF"),
            ("budget.XLSX", "Spreadsheet"),
            ("slides.PPTX", "Presentation"),
            ("letter.DOCX", "Document"),
            ("mystery", "File"),
        ] {
            assert_eq!(file_kind_from_name(name), kind, "{name}");
        }
    }

    #[cfg(unix)]
    fn special_file_fixture(path: &Path) -> Option<std::os::unix::net::UnixListener> {
        use std::{ffi::CString, os::unix::ffi::OsStrExt, os::unix::net::UnixListener};

        match UnixListener::bind(path) {
            Ok(listener) => Some(listener),
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
                // Some CI containers prohibit AF_UNIX sockets with seccomp,
                // while still allowing FIFOs. Both must be rejected by the
                // regular-file transfer engine, so keep the assertion active.
                let name = CString::new(path.as_os_str().as_bytes()).unwrap();
                assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
                None
            }
            Err(error) => panic!("Cannot create special-file test fixture: {error}"),
        }
    }

    #[test]
    fn mounted_volumes_include_local_system_root() {
        assert!(
            mounted_volumes()
                .iter()
                .any(|volume| { volume.path == Path::new("/") && !volume.network })
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn volume_discovery_includes_mount_at_mnt_itself() {
        assert!(is_desktop_mount_path(b"/mnt"));
        assert!(is_desktop_mount_path(b"/mnt/data"));
        assert!(is_desktop_mount_path(b"/media/usb"));
        assert!(is_desktop_mount_path(b"/run/media/user/usb"));
        assert!(!is_desktop_mount_path(b"/mnt-old"));
        assert!(!is_desktop_mount_path(b"/proc"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn enodev_folder_errors_identify_unavailable_mounts() {
        let error = io::Error::from_raw_os_error(libc::ENODEV);
        let message = folder_error_message(Path::new("/mnt"), &error);
        assert!(message.contains("Mount/device unavailable at /mnt"));
        assert!(message.contains("os error 19"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn mountinfo_paths_decode_spaces_backslashes_tabs_and_newlines() {
        assert_eq!(
            decode_mountinfo_path(b"/media/My\\040Drive"),
            Some(PathBuf::from("/media/My Drive"))
        );
        assert_eq!(
            decode_mountinfo_path(b"/mnt/a\\134b\\011tab\\012line"),
            Some(PathBuf::from("/mnt/a\\b\ttab\nline"))
        );
        // Escapes are decoded exactly once: a literal backslash followed by 040 stays so.
        assert_eq!(
            decode_mountinfo_path(b"/mnt/a\\134040"),
            Some(PathBuf::from("/mnt/a\\040"))
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn mountinfo_paths_keep_non_utf8_bytes() {
        use std::os::unix::ffi::OsStringExt;
        let expected = PathBuf::from(OsString::from_vec(b"/media/\xff".to_vec()));
        assert_eq!(
            decode_mountinfo_path(b"/media/\xff"),
            Some(expected.clone())
        );
        assert_eq!(decode_mountinfo_path(b"/media/\\377"), Some(expected));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn mountinfo_paths_reject_malformed_or_nul_escapes() {
        for malformed in [
            b"/mnt/bad\\".as_slice(),
            b"/mnt/bad\\04",
            b"/mnt/bad\\08x",
            b"/mnt/bad\\777",
            b"/mnt/bad\\000",
        ] {
            assert_eq!(decode_mountinfo_path(malformed), None);
        }
    }

    fn test_root() -> PathBuf {
        static NEXT_TEST_ROOT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        for _ in 0..128 {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let sequence = NEXT_TEST_ROOT.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!(
                "ricefield-test-{}-{nonce}-{sequence}",
                std::process::id()
            ));
            match fs::create_dir(&dir) {
                Ok(()) => return dir,
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("Cannot create isolated test root: {error}"),
            }
        }
        panic!("Cannot allocate a unique isolated test root")
    }

    #[test]
    fn navigation_resolves_files_without_opening_them_and_handles_links() {
        let root = test_root();
        let file = root.join("file.txt");
        fs::write(&file, "content").unwrap();
        assert_eq!(navigation_target(&root).unwrap(), (root.clone(), None));
        assert_eq!(
            navigation_target(&file).unwrap(),
            (root.clone(), Some(file.clone()))
        );
        assert!(navigation_target(&root.join("missing")).is_err());
        assert!(navigation_target(Path::new(".")).unwrap().0.is_absolute());
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let link = root.join("directory-link");
            symlink(&root, &link).unwrap();
            assert_eq!(navigation_target(&link).unwrap(), (link.clone(), None));
            symlink(&file, root.join("file-link")).unwrap();
            let dangling = root.join("dangling");
            symlink(root.join("missing"), &dangling).unwrap();
            assert_eq!(
                navigation_target(&dangling).unwrap(),
                (root.clone(), Some(dangling))
            );
            let scan = scan_cancellable(&root, &AtomicBool::new(false)).unwrap();
            let linked_folder = scan
                .entries
                .iter()
                .find(|entry| entry.path == link)
                .unwrap();
            assert!(linked_folder.is_dir && linked_folder.is_symlink);
            let linked_file = scan
                .entries
                .iter()
                .find(|entry| entry.name == "file-link")
                .unwrap();
            assert!(!linked_file.is_dir && linked_file.is_symlink);
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn journaled_publication_keeps_source_until_explicit_completion() {
        let root = test_root();
        let source = root.join("source.txt");
        let destination = root.join("target.txt");
        fs::write(&source, b"verified content").unwrap();
        let mut journal = Journal::begin_in(&root.join("journal"), &source, &destination).unwrap();
        let cancelled = AtomicBool::new(false);
        let mut no_op = |_: Progress| {};
        let mut transfer = Transfer {
            cancelled: &cancelled,
            notify: &mut no_op,
            transferred: 0,
            total: 16,
            last_report: Instant::now(),
            last_reported_bytes: 0,
            phase: "Copying",
        };
        stage_and_publish_with_journal(
            &source,
            &destination,
            true,
            &mut transfer,
            Some(&mut journal),
        )
        .unwrap();
        assert_eq!(journal.record().state, State::Published);
        assert_eq!(fs::read(&source).unwrap(), b"verified content");
        assert_eq!(fs::read(&destination).unwrap(), b"verified content");
        let saved = fs::read_to_string(
            root.join("journal")
                .join(format!("{}.toml", journal.record().id)),
        )
        .unwrap();
        assert!(saved.contains("state = \"published\""));
        fs::remove_file(&source).unwrap();
        journal.advance(State::Completed).unwrap();
        assert_eq!(journal.record().state, State::Completed);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn journal_publication_failure_never_deletes_source() {
        let root = test_root();
        let source = root.join("source.txt");
        let destination = root.join("target.txt");
        fs::write(&source, b"keep original").unwrap();
        let journal_dir = root.join("journal");
        let mut journal = Journal::begin_in(&journal_dir, &source, &destination).unwrap();
        let journal_path = journal_dir.join(format!("{}.toml", journal.record().id));
        let cancelled = AtomicBool::new(false);
        let mut sabotaged = false;
        let mut sabotage = |progress: Progress| {
            if progress.phase == "Publishing" && !sabotaged {
                // Simulate journal storage failing during Published persistence,
                // immediately after the destination has been atomically renamed.
                fs::remove_file(&journal_path).unwrap();
                fs::create_dir(&journal_path).unwrap();
                sabotaged = true;
            }
        };
        let mut transfer = Transfer {
            cancelled: &cancelled,
            notify: &mut sabotage,
            transferred: 0,
            total: 13,
            last_report: Instant::now(),
            last_reported_bytes: 0,
            phase: "Copying",
        };
        let result = stage_and_publish_with_journal(
            &source,
            &destination,
            true,
            &mut transfer,
            Some(&mut journal),
        );
        assert!(sabotaged, "test did not reach publication");
        assert!(result.is_err());
        assert_eq!(journal.record().state, State::StagedVerified);
        assert_eq!(fs::read(&source).unwrap(), b"keep original");
        assert_eq!(fs::read(&destination).unwrap(), b"keep original");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn staging_directory_sync_failure_prevents_publication() {
        let root = test_root();
        let source = root.join("original");
        let destination = root.join("target");
        fs::write(&source, b"keep original").unwrap();
        let mut journal = Journal::begin_in(&root.join("journal"), &source, &destination).unwrap();
        let cancelled = AtomicBool::new(false);
        let mut no_op = |_: Progress| {};
        let mut transfer = Transfer {
            cancelled: &cancelled,
            notify: &mut no_op,
            transferred: 0,
            total: 13,
            last_report: Instant::now(),
            last_reported_bytes: 0,
            phase: "Copying",
        };
        let mut sync = |_: &Path| Err(io::Error::other("injected staging directory sync error"));
        let error = stage_and_publish_with_sync(
            &source,
            &destination,
            true,
            &mut transfer,
            Some(&mut journal),
            &mut sync,
        )
        .unwrap_err();
        assert!(error.to_string().contains("injected staging"));
        assert_eq!(journal.record().state, State::Prepared);
        assert_eq!(fs::read(&source).unwrap(), b"keep original");
        assert!(!destination.exists());
        assert!(
            journal
                .record()
                .staging
                .as_ref()
                .is_some_and(|path| !path.exists())
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn competing_destination_created_before_publication_is_never_overwritten() {
        let root = test_root();
        let source = root.join("source.txt");
        let destination = root.join("target.txt");
        fs::write(&source, b"source bytes").unwrap();
        let cancelled = AtomicBool::new(false);
        let mut no_op = |_: Progress| {};
        let mut transfer = Transfer {
            cancelled: &cancelled,
            notify: &mut no_op,
            transferred: 0,
            total: 12,
            last_report: Instant::now(),
            last_reported_bytes: 0,
            phase: "Copying",
        };
        let mut sync_count = 0;
        let mut sync = |path: &Path| {
            sync_count += 1;
            if sync_count == 1 {
                // Simulate an unrelated writer winning the destination name
                // after complete staging but before the atomic rename.
                fs::write(&destination, b"other writer")?;
            }
            sync_directory(path)
        };

        let error = stage_and_publish_with_sync(
            &source,
            &destination,
            false,
            &mut transfer,
            None,
            &mut sync,
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(sync_count, 1);
        assert_eq!(fs::read(&source).unwrap(), b"source bytes");
        assert_eq!(fs::read(&destination).unwrap(), b"other writer");
        assert_eq!(fs::read_dir(&root).unwrap().count(), 2);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn changed_source_after_verified_staging_is_not_published_or_deleted() {
        let root = test_root();
        let source = root.join("source.txt");
        let destination = root.join("target.txt");
        fs::write(&source, b"before").unwrap();
        let mut journal = Journal::begin_in(&root.join("journal"), &source, &destination).unwrap();
        let cancelled = AtomicBool::new(false);
        let mut no_op = |_: Progress| {};
        let mut transfer = Transfer {
            cancelled: &cancelled,
            notify: &mut no_op,
            transferred: 0,
            total: 6,
            last_report: Instant::now(),
            last_reported_bytes: 0,
            phase: "Copying",
        };
        let mut sync_count = 0;
        let mut sync = |path: &Path| {
            sync_count += 1;
            if sync_count == 1 {
                fs::write(&source, b"source changed during publication")?;
            }
            sync_directory(path)
        };

        let error = stage_and_publish_with_sync(
            &source,
            &destination,
            true,
            &mut transfer,
            Some(&mut journal),
            &mut sync,
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert_eq!(sync_count, 1);
        assert_eq!(journal.record().state, State::StagedVerified);
        assert_eq!(
            fs::read(&source).unwrap(),
            b"source changed during publication"
        );
        assert!(!destination.exists());
        assert!(
            journal
                .record()
                .staging
                .as_ref()
                .is_some_and(|path| !path.exists())
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn destination_directory_sync_failure_retains_source_and_recovery_record() {
        let root = test_root();
        let source = root.join("original");
        let destination = root.join("target");
        fs::write(&source, b"keep original").unwrap();
        let directory = root.join("journal");
        let mut journal = Journal::begin_in(&directory, &source, &destination).unwrap();
        let cancelled = AtomicBool::new(false);
        let mut no_op = |_: Progress| {};
        let mut transfer = Transfer {
            cancelled: &cancelled,
            notify: &mut no_op,
            transferred: 0,
            total: 13,
            last_report: Instant::now(),
            last_reported_bytes: 0,
            phase: "Copying",
        };
        let mut barriers = 0usize;
        let mut sync = |path: &Path| {
            barriers += 1;
            if barriers == 2 {
                Err(io::Error::other(
                    "injected destination directory sync error",
                ))
            } else {
                sync_directory(path)
            }
        };
        let error = stage_and_publish_with_sync(
            &source,
            &destination,
            true,
            &mut transfer,
            Some(&mut journal),
            &mut sync,
        )
        .unwrap_err();
        assert_eq!(barriers, 2);
        assert!(error.to_string().contains("Destination was published"));
        assert_eq!(fs::read(&source).unwrap(), b"keep original");
        assert_eq!(fs::read(&destination).unwrap(), b"keep original");
        assert_eq!(journal.record().state, State::StagedVerified);
        let persisted =
            fs::read_to_string(directory.join(format!("{}.toml", journal.record().id))).unwrap();
        assert!(persisted.contains("state = \"staged_verified\""));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn copy_preserves_source_and_avoids_overwrite() {
        let root = test_root();
        let source = root.join("note.txt");
        fs::write(&source, b"original").unwrap();
        let dst = root.join("out");
        fs::create_dir(&dst).unwrap();
        fs::write(dst.join("note.txt"), b"existing").unwrap();
        perform(&Action::Copy {
            sources: vec![source.clone()],
            destination: dst.clone(),
            conflict: ConflictPolicy::KeepBoth,
        })
        .unwrap();
        assert_eq!(fs::read(dst.join("note.txt")).unwrap(), b"existing");
        assert_eq!(fs::read(dst.join("note (copy).txt")).unwrap(), b"original");
        assert_eq!(fs::read(source).unwrap(), b"original");
        assert_eq!(fs::read_dir(&dst).unwrap().count(), 2);
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn copying_directory_preserves_internal_hardlink_relationships() {
        use std::os::unix::fs::MetadataExt;

        let root = test_root();
        let original = root.join("original");
        let nested = original.join("nested");
        fs::create_dir(&original).unwrap();
        fs::create_dir(&nested).unwrap();
        fs::write(original.join("first.txt"), b"shared content").unwrap();
        fs::hard_link(original.join("first.txt"), nested.join("second.txt")).unwrap();
        let output = root.join("out");
        fs::create_dir(&output).unwrap();

        perform(&Action::Copy {
            sources: vec![original.clone()],
            destination: output.clone(),
            conflict: ConflictPolicy::KeepBoth,
        })
        .unwrap();

        let copied = output.join("original");
        let first = fs::metadata(copied.join("first.txt")).unwrap();
        let second = fs::metadata(copied.join("nested/second.txt")).unwrap();
        assert_eq!(first.ino(), second.ino());
        assert_eq!(first.nlink(), 2);
        assert_eq!(
            fs::read(copied.join("nested/second.txt")).unwrap(),
            b"shared content"
        );
        assert_ne!(
            first.ino(),
            fs::metadata(original.join("first.txt")).unwrap().ino()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn cross_device_move_refuses_hardlinks_outside_selected_tree() {
        let root = test_root();
        let source = root.join("original");
        fs::create_dir(&source).unwrap();
        let inside = source.join("inside.txt");
        let outside = root.join("outside.txt");
        fs::write(&inside, b"shared bytes").unwrap();
        fs::hard_link(&inside, &outside).unwrap();

        let mut manifest = BTreeMap::new();
        manifest.insert(source.clone(), fs::symlink_metadata(&source).unwrap());
        manifest.insert(inside.clone(), fs::symlink_metadata(&inside).unwrap());
        let error = ensure_hardlink_scope_preserved(&manifest).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Unsupported);
        assert_eq!(fs::read(&inside).unwrap(), b"shared bytes");
        assert_eq!(fs::read(&outside).unwrap(), b"shared bytes");
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn cross_device_move_accepts_hardlinks_fully_within_selected_tree() {
        let root = test_root();
        let source = root.join("original");
        fs::create_dir(&source).unwrap();
        let first = source.join("first.txt");
        let second = source.join("second.txt");
        fs::write(&first, b"shared bytes").unwrap();
        fs::hard_link(&first, &second).unwrap();

        let mut manifest = BTreeMap::new();
        manifest.insert(source.clone(), fs::symlink_metadata(&source).unwrap());
        manifest.insert(first.clone(), fs::symlink_metadata(&first).unwrap());
        manifest.insert(second.clone(), fs::symlink_metadata(&second).unwrap());
        ensure_hardlink_scope_preserved(&manifest).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn cross_device_move_refuses_partially_selected_hardlink_group() {
        let root = test_root();
        let source = root.join("original");
        fs::create_dir(&source).unwrap();
        let first = source.join("first.txt");
        let second = source.join("second.txt");
        let outside = root.join("outside.txt");
        fs::write(&first, b"shared bytes").unwrap();
        fs::hard_link(&first, &second).unwrap();
        fs::hard_link(&first, &outside).unwrap();

        let mut manifest = BTreeMap::new();
        manifest.insert(source.clone(), fs::symlink_metadata(&source).unwrap());
        manifest.insert(first.clone(), fs::symlink_metadata(&first).unwrap());
        manifest.insert(second.clone(), fs::symlink_metadata(&second).unwrap());
        assert_eq!(
            ensure_hardlink_scope_preserved(&manifest)
                .unwrap_err()
                .kind(),
            io::ErrorKind::Unsupported
        );
        assert_eq!(fs::read(outside).unwrap(), b"shared bytes");
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn cross_device_move_refuses_hardlinked_symlink() {
        use std::os::unix::fs::symlink;

        let root = test_root();
        let source = root.join("original");
        fs::create_dir(&source).unwrap();
        let first = source.join("link1");
        let second = source.join("link2");
        symlink("target", &first).unwrap();
        fs::hard_link(&first, &second).unwrap();

        let mut manifest = BTreeMap::new();
        manifest.insert(source.clone(), fs::symlink_metadata(&source).unwrap());
        manifest.insert(first.clone(), fs::symlink_metadata(&first).unwrap());
        manifest.insert(second.clone(), fs::symlink_metadata(&second).unwrap());
        assert_eq!(
            ensure_hardlink_scope_preserved(&manifest)
                .unwrap_err()
                .kind(),
            io::ErrorKind::Unsupported
        );
        assert_eq!(fs::read_link(first).unwrap(), Path::new("target"));
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn hardlink_copy_rejects_mutation_after_first_name_was_staged() {
        let root = test_root();
        let first = root.join("first");
        let second = root.join("second");
        let staged_first = root.join("staged-first");
        let staged_second = root.join("staged-second");
        fs::write(&first, b"original bytes").unwrap();
        fs::hard_link(&first, &second).unwrap();

        let cancelled = AtomicBool::new(false);
        let mut no_op = |_: Progress| {};
        let mut transfer = Transfer {
            cancelled: &cancelled,
            notify: &mut no_op,
            transferred: 0,
            total: 28,
            last_report: Instant::now(),
            last_reported_bytes: 0,
            phase: "Copying",
        };
        let mut context = CopyContext::default();
        copy_tree(&first, &staged_first, 0, &mut transfer, &mut context).unwrap();
        fs::write(&second, b"different data").unwrap();
        let error = copy_tree(&second, &staged_second, 0, &mut transfer, &mut context).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert_eq!(fs::read(&staged_first).unwrap(), b"original bytes");
        assert!(!staged_second.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn move_cleanup_rejects_replaced_source_even_with_identical_bytes() {
        let root = test_root();
        let source = root.join("original.txt");
        let retired = root.join("retired.txt");
        let destination = root.join("destination.txt");
        fs::write(&source, b"same bytes").unwrap();
        fs::write(&destination, b"same bytes").unwrap();
        let cancelled = AtomicBool::new(false);
        let mut no_op = |_: Progress| {};
        let mut transfer = Transfer {
            cancelled: &cancelled,
            notify: &mut no_op,
            transferred: 0,
            total: 10,
            last_report: Instant::now(),
            last_reported_bytes: 0,
            phase: "Verifying",
        };
        let mut manifest = BTreeMap::new();
        capture_source_manifest(&source, 0, &mut manifest, &mut transfer).unwrap();
        fs::rename(&source, &retired).unwrap();
        fs::write(&source, b"same bytes").unwrap();

        assert_eq!(
            verify_source_manifest(&manifest, &mut transfer)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(
            remove_verified_tree(
                &source,
                &destination,
                0,
                &manifest,
                &mut BTreeSet::new(),
                &mut transfer,
            )
            .unwrap_err()
            .kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(fs::read(source).unwrap(), b"same bytes");
        assert_eq!(fs::read(retired).unwrap(), b"same bytes");
        assert_eq!(fs::read(destination).unwrap(), b"same bytes");
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn cleanup_allows_own_hardlink_unlinks_without_losing_other_entries() {
        let root = test_root();
        let source = root.join("original");
        let destination = root.join("published");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("a.txt"), b"shared").unwrap();
        fs::hard_link(source.join("a.txt"), source.join("b.txt")).unwrap();
        let cancelled = AtomicBool::new(false);
        let mut no_op = |_: Progress| {};
        let mut transfer = Transfer {
            cancelled: &cancelled,
            notify: &mut no_op,
            transferred: 0,
            total: 12,
            last_report: Instant::now(),
            last_reported_bytes: 0,
            phase: "Verifying",
        };
        let mut manifest = BTreeMap::new();
        capture_source_manifest(&source, 0, &mut manifest, &mut transfer).unwrap();
        stage_and_publish(&source, &destination, true, &mut transfer).unwrap();
        verify_source_manifest(&manifest, &mut transfer).unwrap();
        remove_verified_tree(
            &source,
            &destination,
            0,
            &manifest,
            &mut BTreeSet::new(),
            &mut transfer,
        )
        .unwrap();
        assert!(!source.exists());
        assert_eq!(fs::read(destination.join("a.txt")).unwrap(), b"shared");
        assert_eq!(fs::read(destination.join("b.txt")).unwrap(), b"shared");
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn copying_tree_preserves_user_extended_attributes() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};

        let root = test_root();
        let original = root.join("original");
        let file = original.join("note.txt");
        let output = root.join("output");
        fs::create_dir(&original).unwrap();
        fs::create_dir(&output).unwrap();
        fs::write(&file, b"body").unwrap();
        let key = CString::new("user.ricefield_test").unwrap();
        let value = b"valuable metadata";
        for path in [&original, &file] {
            let name = CString::new(path.as_os_str().as_bytes()).unwrap();
            let result = unsafe {
                libc::setxattr(
                    name.as_ptr(),
                    key.as_ptr(),
                    value.as_ptr().cast(),
                    value.len(),
                    0,
                )
            };
            if result != 0 {
                let error = io::Error::last_os_error();
                if matches!(
                    error.raw_os_error(),
                    Some(libc::EOPNOTSUPP) | Some(libc::EPERM)
                ) {
                    fs::remove_dir_all(root).unwrap();
                    return;
                }
                panic!("Cannot prepare xattr fixture: {error}");
            }
        }
        fs::set_permissions(&file, fs::Permissions::from_mode(0o444)).unwrap();
        perform(&Action::Copy {
            sources: vec![original.clone()],
            destination: output.clone(),
            conflict: ConflictPolicy::KeepBoth,
        })
        .unwrap();

        for path in [output.join("original"), output.join("original/note.txt")] {
            let name = CString::new(path.as_os_str().as_bytes()).unwrap();
            let mut found = [0u8; 64];
            let size = unsafe {
                libc::getxattr(
                    name.as_ptr(),
                    key.as_ptr(),
                    found.as_mut_ptr().cast(),
                    found.len(),
                )
            };
            assert_eq!(size as usize, value.len());
            assert_eq!(&found[..value.len()], value);
        }
        assert_eq!(
            fs::metadata(output.join("original/note.txt"))
                .unwrap()
                .mode()
                & 0o777,
            0o444
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(target_os = "linux")]
    fn test_acl_value(owner: u16, group: u16, mask: u16, other: u16) -> Vec<u8> {
        // Linux POSIX ACL xattr v2: little-endian header followed by
        // (tag, permission, uid/gid) entries. A named user makes the ACL
        // nontrivial so the kernel retains it as an extended attribute.
        let mut value = 2u32.to_le_bytes().to_vec();
        // Use a mapped uid even inside single-id user namespaces; 65534
        // is not guaranteed to be valid in an isolated test container.
        let named_user = unsafe { libc::geteuid() };
        for (tag, permission, id) in [
            (1u16, owner, u32::MAX),
            (2u16, 4, named_user),
            (4u16, group, u32::MAX),
            (16u16, mask, u32::MAX),
            (32u16, other, u32::MAX),
        ] {
            value.extend(tag.to_le_bytes());
            value.extend(permission.to_le_bytes());
            value.extend(id.to_le_bytes());
        }
        value
    }

    #[cfg(target_os = "linux")]
    fn set_test_acl(path: &Path, name: &[u8], value: &[u8]) -> io::Result<()> {
        let path = CString::new(path.as_os_str().as_bytes()).unwrap();
        let name = CString::new(name).unwrap();
        let result = unsafe {
            libc::setxattr(
                path.as_ptr(),
                name.as_ptr(),
                value.as_ptr().cast(),
                value.len(),
                0,
            )
        };
        if result != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn copying_tree_retains_posix_access_and_default_acls() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};

        let root = test_root();
        let source = root.join("source");
        let output = root.join("output");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&output).unwrap();
        let file = source.join("note.txt");
        fs::write(&file, b"access control").unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o640)).unwrap();
        fs::set_permissions(&source, fs::Permissions::from_mode(0o750)).unwrap();

        let access = test_acl_value(6, 4, 4, 0);
        let default = test_acl_value(7, 5, 5, 0);
        for (path, name, value) in [
            (
                file.as_path(),
                b"system.posix_acl_access".as_slice(),
                &access,
            ),
            (
                source.as_path(),
                b"system.posix_acl_default".as_slice(),
                &default,
            ),
        ] {
            if let Err(error) = set_test_acl(path, name, value) {
                if matches!(
                    error.raw_os_error(),
                    Some(libc::EOPNOTSUPP) | Some(libc::EPERM)
                ) {
                    fs::remove_dir_all(root).unwrap();
                    return;
                }
                panic!("Cannot prepare ACL fixture: {error}");
            }
        }

        perform(&Action::Copy {
            sources: vec![source.clone()],
            destination: output.clone(),
            conflict: ConflictPolicy::KeepBoth,
        })
        .unwrap();
        for (left, right, name) in [
            (
                file,
                output.join("source/note.txt"),
                b"system.posix_acl_access".as_slice(),
            ),
            (
                source,
                output.join("source"),
                b"system.posix_acl_default".as_slice(),
            ),
        ] {
            assert_eq!(
                read_link_xattr(&left, name).unwrap(),
                read_link_xattr(&right, name).unwrap()
            );
            assert_eq!(
                fs::metadata(left).unwrap().mode() & 0o7777,
                fs::metadata(right).unwrap().mode() & 0o7777
            );
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn copying_file_removes_acl_inherited_from_staging_parent() {
        let root = test_root();
        let source = root.join("plain.txt");
        let output = root.join("output");
        fs::write(&source, b"without ACL").unwrap();
        fs::create_dir(&output).unwrap();
        let inherited_acl = test_acl_value(7, 5, 5, 0);
        if let Err(error) = set_test_acl(&output, b"system.posix_acl_default", &inherited_acl) {
            if matches!(
                error.raw_os_error(),
                Some(libc::EOPNOTSUPP) | Some(libc::EPERM)
            ) {
                fs::remove_dir_all(root).unwrap();
                return;
            }
            panic!("Cannot prepare inherited ACL fixture: {error}");
        }
        assert!(
            !list_link_xattrs(&source)
                .unwrap()
                .contains(b"system.posix_acl_access".as_slice())
        );
        perform(&Action::Copy {
            sources: vec![source.clone()],
            destination: output.clone(),
            conflict: ConflictPolicy::KeepBoth,
        })
        .unwrap();
        let copied = output.join("plain.txt");
        assert_eq!(fs::read(&copied).unwrap(), b"without ACL");
        assert!(
            !list_link_xattrs(&copied)
                .unwrap()
                .contains(b"system.posix_acl_access".as_slice())
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn verified_move_staging_accepts_preserved_posix_acl() {
        let root = test_root();
        let source = root.join("source.txt");
        let destination = root.join("published.txt");
        fs::write(&source, b"acl-verified data").unwrap();
        let access = test_acl_value(6, 4, 4, 0);
        if let Err(error) = set_test_acl(&source, b"system.posix_acl_access", &access) {
            if matches!(
                error.raw_os_error(),
                Some(libc::EOPNOTSUPP) | Some(libc::EPERM)
            ) {
                fs::remove_dir_all(root).unwrap();
                return;
            }
            panic!("Cannot prepare ACL fixture: {error}");
        }
        let cancelled = AtomicBool::new(false);
        let mut callback = |_: Progress| {};
        let mut transfer = Transfer {
            cancelled: &cancelled,
            notify: &mut callback,
            transferred: 0,
            total: 17,
            last_report: Instant::now(),
            last_reported_bytes: 0,
            phase: "Copying",
        };
        stage_and_publish(&source, &destination, true, &mut transfer).unwrap();
        assert_eq!(fs::read(&source).unwrap(), fs::read(&destination).unwrap());
        assert_eq!(
            read_link_xattr(&source, b"system.posix_acl_access").unwrap(),
            read_link_xattr(&destination, b"system.posix_acl_access").unwrap()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn privileged_copy_retains_noncurrent_owner_if_permitted() {
        use std::os::unix::fs::MetadataExt;

        if unsafe { libc::geteuid() } != 0 {
            return;
        }
        let root = test_root();
        let source = root.join("source.txt");
        let output = root.join("output");
        fs::write(&source, b"owned by nobody").unwrap();
        fs::create_dir(&output).unwrap();
        let path = CString::new(source.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::chown(path.as_ptr(), 65534, 65534) }, 0);
        perform(&Action::Copy {
            sources: vec![source.clone()],
            destination: output.clone(),
            conflict: ConflictPolicy::KeepBoth,
        })
        .unwrap();
        let copied = fs::symlink_metadata(output.join("source.txt")).unwrap();
        assert_eq!((copied.uid(), copied.gid()), (65534, 65534));
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn copying_broken_symlink_preserves_link_timestamps_without_touching_target() {
        use std::os::unix::fs::{MetadataExt, symlink};

        let root = test_root();
        let source = root.join("broken-link");
        let output = root.join("output");
        let timestamp_fixture = root.join("timestamp-fixture");
        fs::create_dir(&output).unwrap();
        fs::write(&timestamp_fixture, b"fixture").unwrap();
        let desired = UNIX_EPOCH + Duration::from_secs(1_600_000_000);
        File::open(&timestamp_fixture)
            .unwrap()
            .set_times(FileTimes::new().set_accessed(desired).set_modified(desired))
            .unwrap();
        symlink("missing-target", &source).unwrap();
        preserve_symlink_times(&source, &fs::metadata(&timestamp_fixture).unwrap()).unwrap();
        let before = fs::symlink_metadata(&source).unwrap();

        perform(&Action::Copy {
            sources: vec![source.clone()],
            destination: output.clone(),
            conflict: ConflictPolicy::KeepBoth,
        })
        .unwrap();

        let copied = output.join("broken-link");
        let after = fs::symlink_metadata(&copied).unwrap();
        assert!(after.file_type().is_symlink());
        assert_eq!(
            fs::read_link(&copied).unwrap(),
            PathBuf::from("missing-target")
        );
        assert_eq!(
            (after.mtime(), after.mtime_nsec()),
            (before.mtime(), before.mtime_nsec())
        );
        assert_eq!(
            (after.atime(), after.atime_nsec()),
            (before.atime(), before.atime_nsec())
        );
        assert!(!root.join("missing-target").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn verification_open_rejects_symlink_and_same_size_inode_replacements() {
        use std::os::unix::fs::symlink;

        let root = test_root();
        let source = root.join("source.txt");
        let saved = root.join("saved.txt");
        fs::write(&source, b"original").unwrap();
        let snapshot = fs::symlink_metadata(&source).unwrap();
        fs::rename(&source, &saved).unwrap();
        symlink(&saved, &source).unwrap();
        assert!(open_verified_regular(&source, &snapshot).is_err());

        fs::remove_file(&source).unwrap();
        fs::write(&source, b"original").unwrap();
        assert_eq!(
            open_verified_regular(&source, &snapshot)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(fs::read(saved).unwrap(), b"original");
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn verification_open_rejects_replacement_fifo_without_blocking() {
        let root = test_root();
        let source = root.join("source.txt");
        fs::write(&source, b"content").unwrap();
        let snapshot = fs::symlink_metadata(&source).unwrap();
        fs::remove_file(&source).unwrap();
        let name = CString::new(source.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);

        assert_eq!(
            open_verified_regular(&source, &snapshot)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn copy_skip_existing_preserves_collisions_and_copies_other_items() {
        let root = test_root();
        let destination = root.join("out");
        fs::create_dir(&destination).unwrap();
        let existing = root.join("existing.txt");
        let fresh = root.join("fresh.txt");
        fs::write(&existing, b"incoming").unwrap();
        fs::write(&fresh, b"fresh payload").unwrap();
        fs::write(destination.join("existing.txt"), b"keep destination").unwrap();
        let result = perform(&Action::Copy {
            sources: vec![existing.clone(), fresh.clone()],
            destination: destination.clone(),
            conflict: ConflictPolicy::SkipExisting,
        })
        .unwrap();
        assert!(result.contains("Copied 1 item(s); skipped 1"), "{result}");
        assert_eq!(
            fs::read(destination.join("existing.txt")).unwrap(),
            b"keep destination"
        );
        assert_eq!(
            fs::read(destination.join("fresh.txt")).unwrap(),
            b"fresh payload"
        );
        assert!(!destination.join("existing (copy).txt").exists());
        assert_eq!(fs::read(existing).unwrap(), b"incoming");
        assert_eq!(fs::read(fresh).unwrap(), b"fresh payload");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn move_skip_existing_never_removes_skipped_sources() {
        let root = test_root();
        let destination = root.join("out");
        fs::create_dir(&destination).unwrap();
        let existing = root.join("existing.txt");
        let fresh = root.join("fresh.txt");
        fs::write(&existing, b"incoming").unwrap();
        fs::write(&fresh, b"move me").unwrap();
        fs::write(destination.join("existing.txt"), b"keep destination").unwrap();
        let result = perform(&Action::Move {
            sources: vec![existing.clone(), fresh.clone()],
            destination: destination.clone(),
            conflict: ConflictPolicy::SkipExisting,
        })
        .unwrap();
        assert!(result.contains("Moved 1 item(s); skipped 1"), "{result}");
        assert_eq!(
            fs::read(destination.join("existing.txt")).unwrap(),
            b"keep destination"
        );
        assert_eq!(fs::read(existing).unwrap(), b"incoming");
        assert_eq!(fs::read(destination.join("fresh.txt")).unwrap(), b"move me");
        assert!(!fresh.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn skip_existing_recognizes_dangling_destination_symlinks() {
        use std::os::unix::fs::symlink;
        let root = test_root();
        let destination = root.join("out");
        fs::create_dir(&destination).unwrap();
        let source = root.join("link");
        fs::write(&source, b"incoming").unwrap();
        symlink("absent-target", destination.join("link")).unwrap();
        let result = perform(&Action::Copy {
            sources: vec![source.clone()],
            destination: destination.clone(),
            conflict: ConflictPolicy::SkipExisting,
        })
        .unwrap();
        assert!(result.contains("skipped 1"), "{result}");
        assert_eq!(
            fs::read_link(destination.join("link")).unwrap(),
            PathBuf::from("absent-target")
        );
        assert_eq!(fs::read_dir(&destination).unwrap().count(), 1);
        assert_eq!(fs::read(source).unwrap(), b"incoming");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn transfer_report_records_exact_completed_and_skipped_targets() {
        let root = test_root();
        let destination = root.join("out");
        fs::create_dir(&destination).unwrap();
        let existing = root.join("existing.txt");
        let fresh = root.join("fresh.txt");
        fs::write(&existing, b"source").unwrap();
        fs::write(&fresh, b"new file").unwrap();
        fs::write(destination.join("existing.txt"), b"destination").unwrap();
        let report = perform_with_report(
            &Action::Copy {
                sources: vec![existing.clone(), fresh.clone()],
                destination: destination.clone(),
                conflict: ConflictPolicy::SkipExisting,
            },
            &AtomicBool::new(false),
            &mut |_| {},
        );
        assert!(report.result.is_ok());
        assert_eq!(report.items.len(), 2);
        assert_eq!(report.items[0].state, ItemState::Skipped);
        assert_eq!(report.items[0].source, existing);
        assert_eq!(
            report.items[0].target,
            Some(destination.join("existing.txt"))
        );
        assert_eq!(report.items[1].state, ItemState::Completed);
        assert_eq!(report.items[1].source, fresh);
        assert_eq!(report.items[1].target, Some(destination.join("fresh.txt")));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn move_report_retains_skipped_sources_and_exposes_moved_target() {
        let root = test_root();
        let destination = root.join("out");
        fs::create_dir(&destination).unwrap();
        let existing = root.join("existing.txt");
        let fresh = root.join("fresh.txt");
        fs::write(&existing, b"source").unwrap();
        fs::write(&fresh, b"fresh").unwrap();
        fs::write(destination.join("existing.txt"), b"destination").unwrap();
        let report = perform_with_report(
            &Action::Move {
                sources: vec![existing.clone(), fresh.clone()],
                destination: destination.clone(),
                conflict: ConflictPolicy::SkipExisting,
            },
            &AtomicBool::new(false),
            &mut |_| {},
        );
        assert!(report.result.is_ok());
        assert_eq!(report.items[0].state, ItemState::Skipped);
        assert_eq!(report.items[1].state, ItemState::Completed);
        assert_eq!(report.items[1].target, Some(destination.join("fresh.txt")));
        assert_eq!(fs::read(existing).unwrap(), b"source");
        assert!(!fresh.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn transfer_report_preserves_partial_failure_and_unstarted_items() {
        let root = test_root();
        let destination = root.join("out");
        fs::create_dir(&destination).unwrap();
        let first = root.join("first.txt");
        let last = root.join("last.txt");
        fs::write(&first, b"completed").unwrap();
        fs::write(&last, b"not started").unwrap();
        let socket = root.join("socket");
        let _listener = special_file_fixture(&socket);
        let report = perform_with_report(
            &Action::Copy {
                sources: vec![first.clone(), socket.clone(), last.clone()],
                destination: destination.clone(),
                conflict: ConflictPolicy::KeepBoth,
            },
            &AtomicBool::new(false),
            &mut |_| {},
        );
        assert_eq!(
            report.result.unwrap_err().kind(),
            io::ErrorKind::Unsupported
        );
        assert_eq!(report.items.len(), 3);
        assert_eq!(report.items[0].state, ItemState::Completed);
        assert_eq!(report.items[0].target, Some(destination.join("first.txt")));
        assert_eq!(report.items[1].state, ItemState::Failed);
        assert!(
            report.items[1]
                .detail
                .as_ref()
                .is_some_and(|text| !text.is_empty())
        );
        assert_eq!(report.items[2].state, ItemState::NotStarted);
        assert_eq!(
            fs::read(destination.join("first.txt")).unwrap(),
            b"completed"
        );
        assert!(!destination.join("last.txt").exists());
        assert_eq!(fs::read(last).unwrap(), b"not started");
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn batch_failure_counts_skips_separately_from_completed_items() {
        let root = test_root();
        let destination = root.join("out");
        fs::create_dir(&destination).unwrap();
        let existing = root.join("existing.txt");
        fs::write(&existing, b"source").unwrap();
        fs::write(destination.join("existing.txt"), b"keep").unwrap();
        let socket = root.join("socket");
        let _listener = special_file_fixture(&socket);
        let report = perform_with_report(
            &Action::Copy {
                sources: vec![existing.clone(), socket],
                destination: destination.clone(),
                conflict: ConflictPolicy::SkipExisting,
            },
            &AtomicBool::new(false),
            &mut |_| {},
        );
        let error = report.result.unwrap_err();
        assert!(
            error
                .to_string()
                .contains("0 earlier item(s) completed, 1 skipped")
        );
        assert_eq!(report.items[0].state, ItemState::Skipped);
        assert_eq!(report.items[1].state, ItemState::Failed);
        assert_eq!(fs::read(existing).unwrap(), b"source");
        assert_eq!(fs::read_dir(&destination).unwrap().count(), 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn transfer_report_preflight_failure_does_not_touch_any_source() {
        let root = test_root();
        let destination = root.join("out");
        fs::create_dir(&destination).unwrap();
        let first = root.join("first.txt");
        fs::write(&first, b"safe").unwrap();
        let missing = root.join("missing.txt");
        let report = perform_with_report(
            &Action::Copy {
                sources: vec![first.clone(), missing],
                destination: destination.clone(),
                conflict: ConflictPolicy::KeepBoth,
            },
            &AtomicBool::new(false),
            &mut |_| {},
        );
        assert_eq!(report.result.unwrap_err().kind(), io::ErrorKind::NotFound);
        assert_eq!(report.items[0].state, ItemState::NotStarted);
        assert_eq!(report.items[1].state, ItemState::Failed);
        assert_eq!(fs::read(first).unwrap(), b"safe");
        assert_eq!(fs::read_dir(&destination).unwrap().count(), 0);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn preview_reserves_distinct_names_for_same_basename_sources() {
        let root = test_root();
        let first_parent = root.join("first");
        let second_parent = root.join("second");
        let destination = root.join("out");
        for dir in [&first_parent, &second_parent, &destination] {
            fs::create_dir(dir).unwrap();
        }
        let first = first_parent.join("same.txt");
        let second = second_parent.join("same.txt");
        fs::write(&first, b"first").unwrap();
        fs::write(&second, b"second").unwrap();
        fs::write(destination.join("same.txt"), b"existing").unwrap();
        let plan = preview_transfer(
            &[first.clone(), second.clone()],
            &destination,
            ConflictPolicy::KeepBoth,
            false,
        )
        .unwrap();
        assert_eq!(plan.collisions, 2);
        assert_eq!(plan.renamed, 2);
        assert_eq!(plan.items[0].target, destination.join("same (copy).txt"));
        assert_eq!(plan.items[1].target, destination.join("same (copy 2).txt"));
        // Planning must not create, rename or remove any file.
        assert_eq!(fs::read_dir(&destination).unwrap().count(), 1);
        let report = perform_with_report(
            &Action::Copy {
                sources: vec![first.clone(), second.clone()],
                destination: destination.clone(),
                conflict: ConflictPolicy::KeepBoth,
            },
            &AtomicBool::new(false),
            &mut |_| {},
        );
        assert!(report.result.is_ok());
        assert_eq!(report.items[0].target, Some(plan.items[0].target.clone()));
        assert_eq!(report.items[1].target, Some(plan.items[1].target.clone()));
        assert_eq!(fs::read(destination.join("same.txt")).unwrap(), b"existing");
        assert_eq!(fs::read(plan.items[0].target.clone()).unwrap(), b"first");
        assert_eq!(fs::read(plan.items[1].target.clone()).unwrap(), b"second");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn skip_existing_also_skips_collision_with_earlier_planned_item() {
        let root = test_root();
        let first_parent = root.join("first");
        let second_parent = root.join("second");
        let destination = root.join("out");
        for dir in [&first_parent, &second_parent, &destination] {
            fs::create_dir(dir).unwrap();
        }
        let first = first_parent.join("same.txt");
        let second = second_parent.join("same.txt");
        fs::write(&first, b"first").unwrap();
        fs::write(&second, b"second").unwrap();
        let report = perform_with_report(
            &Action::Copy {
                sources: vec![first, second.clone()],
                destination: destination.clone(),
                conflict: ConflictPolicy::SkipExisting,
            },
            &AtomicBool::new(false),
            &mut |_| {},
        );
        assert!(report.result.is_ok());
        assert_eq!(report.items[0].state, ItemState::Completed);
        assert_eq!(report.items[1].state, ItemState::Skipped);
        assert_eq!(fs::read(destination.join("same.txt")).unwrap(), b"first");
        assert_eq!(fs::read(second).unwrap(), b"second");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn abort_on_collision_rejects_entire_batch_before_first_copy() {
        let root = test_root();
        let destination = root.join("out");
        fs::create_dir(&destination).unwrap();
        let fresh = root.join("fresh.txt");
        let existing = root.join("existing.txt");
        fs::write(&fresh, b"fresh").unwrap();
        fs::write(&existing, b"incoming").unwrap();
        fs::write(destination.join("existing.txt"), b"original").unwrap();
        let report = perform_with_report(
            &Action::Copy {
                sources: vec![fresh.clone(), existing.clone()],
                destination: destination.clone(),
                conflict: ConflictPolicy::AbortOnConflict,
            },
            &AtomicBool::new(false),
            &mut |_| {},
        );
        assert_eq!(
            report.result.unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(report.items[0].state, ItemState::NotStarted);
        assert_eq!(report.items[1].state, ItemState::Failed);
        assert!(!destination.join("fresh.txt").exists());
        assert_eq!(
            fs::read(destination.join("existing.txt")).unwrap(),
            b"original"
        );
        assert_eq!(fs::read(fresh).unwrap(), b"fresh");
        assert_eq!(fs::read(existing).unwrap(), b"incoming");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn skip_if_identical_skips_equal_contents_without_mutating_either_side() {
        let root = test_root();
        let destination = root.join("out");
        fs::create_dir(&destination).unwrap();
        let source = root.join("equal.txt");
        let target = destination.join("equal.txt");
        fs::write(&source, b"identical content").unwrap();
        fs::write(&target, b"identical content").unwrap();

        let plan = preview_transfer(
            std::slice::from_ref(&source),
            &destination,
            ConflictPolicy::SkipIfIdentical,
            false,
        )
        .unwrap();
        assert_eq!(plan.collisions, 1);
        assert_eq!(plan.skipped, 1);
        assert_eq!(plan.renamed, 0);
        assert_eq!(plan.items[0].disposition, PlannedDisposition::Skip);

        // The same policy must never delete a cut source on a verified skip.
        let report = perform_with_report(
            &Action::Move {
                sources: vec![source.clone()],
                destination: destination.clone(),
                conflict: ConflictPolicy::SkipIfIdentical,
            },
            &AtomicBool::new(false),
            &mut |_| {},
        );
        assert!(report.result.is_ok());
        assert_eq!(report.items[0].state, ItemState::Skipped);
        assert_eq!(fs::read(&source).unwrap(), b"identical content");
        assert_eq!(fs::read(&target).unwrap(), b"identical content");
        assert_eq!(fs::read_dir(&destination).unwrap().count(), 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn skip_if_identical_rejects_different_destination_before_batch_mutation() {
        let root = test_root();
        let destination = root.join("out");
        fs::create_dir(&destination).unwrap();
        let fresh = root.join("fresh.txt");
        let conflicting = root.join("conflicting.txt");
        fs::write(&fresh, b"fresh").unwrap();
        fs::write(&conflicting, b"incoming").unwrap();
        fs::write(destination.join("conflicting.txt"), b"different").unwrap();

        let report = perform_with_report(
            &Action::Copy {
                sources: vec![fresh.clone(), conflicting.clone()],
                destination: destination.clone(),
                conflict: ConflictPolicy::SkipIfIdentical,
            },
            &AtomicBool::new(false),
            &mut |_| {},
        );
        assert_eq!(
            report.result.unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(report.items[0].state, ItemState::NotStarted);
        assert_eq!(report.items[1].state, ItemState::Failed);
        assert!(!destination.join("fresh.txt").exists());
        assert_eq!(fs::read(fresh).unwrap(), b"fresh");
        assert_eq!(fs::read(conflicting).unwrap(), b"incoming");
        assert_eq!(
            fs::read(destination.join("conflicting.txt")).unwrap(),
            b"different"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn skip_if_identical_rechecks_after_preview_and_keeps_changed_cut_source() {
        let root = test_root();
        let destination = root.join("out");
        fs::create_dir(&destination).unwrap();
        let source = root.join("changing.txt");
        let target = destination.join("changing.txt");
        fs::write(&source, b"first").unwrap();
        fs::write(&target, b"first").unwrap();
        assert_eq!(
            preview_transfer(
                std::slice::from_ref(&source),
                &destination,
                ConflictPolicy::SkipIfIdentical,
                true,
            )
            .unwrap()
            .skipped,
            1
        );
        fs::write(&target, b"other").unwrap();

        let report = perform_with_report(
            &Action::Move {
                sources: vec![source.clone()],
                destination: destination.clone(),
                conflict: ConflictPolicy::SkipIfIdentical,
            },
            &AtomicBool::new(false),
            &mut |_| {},
        );
        assert_eq!(
            report.result.unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(report.items[0].state, ItemState::Failed);
        assert_eq!(fs::read(source).unwrap(), b"first");
        assert_eq!(fs::read(target).unwrap(), b"other");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn skip_if_identical_compares_nested_directories_without_removing_source() {
        let root = test_root();
        let destination = root.join("out");
        let source = root.join("folder");
        let target = destination.join("folder");
        fs::create_dir(&destination).unwrap();
        fs::create_dir(&source).unwrap();
        fs::create_dir(&target).unwrap();
        fs::write(source.join("nested.txt"), b"same").unwrap();
        fs::write(target.join("nested.txt"), b"same").unwrap();

        let report = perform_with_report(
            &Action::Move {
                sources: vec![source.clone()],
                destination: destination.clone(),
                conflict: ConflictPolicy::SkipIfIdentical,
            },
            &AtomicBool::new(false),
            &mut |_| {},
        );
        assert!(report.result.is_ok());
        assert_eq!(report.items[0].state, ItemState::Skipped);
        assert_eq!(fs::read(source.join("nested.txt")).unwrap(), b"same");
        assert_eq!(fs::read(target.join("nested.txt")).unwrap(), b"same");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn skip_if_identical_rejects_unverifiable_large_file_without_writing() {
        let root = test_root();
        let destination = root.join("out");
        fs::create_dir(&destination).unwrap();
        let source = root.join("large.bin");
        let target = destination.join("large.bin");
        for path in [&source, &target] {
            File::create(path)
                .unwrap()
                .set_len(513 * 1024 * 1024)
                .unwrap();
        }

        let error = preview_transfer(
            std::slice::from_ref(&source),
            &destination,
            ConflictPolicy::SkipIfIdentical,
            false,
        )
        .unwrap_err();
        assert!(error.to_string().contains("512 MiB"));
        assert_eq!(fs::metadata(&source).unwrap().len(), 513 * 1024 * 1024);
        assert_eq!(fs::metadata(&target).unwrap().len(), 513 * 1024 * 1024);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn skip_if_identical_does_not_compare_to_a_reserved_future_destination() {
        let root = test_root();
        let first_parent = root.join("a");
        let second_parent = root.join("b");
        let destination = root.join("out");
        for dir in [&first_parent, &second_parent, &destination] {
            fs::create_dir(dir).unwrap();
        }
        let first = first_parent.join("name.txt");
        let second = second_parent.join("name.txt");
        fs::write(&first, b"equal").unwrap();
        fs::write(&second, b"equal").unwrap();

        let error = preview_transfer(
            &[first.clone(), second.clone()],
            &destination,
            ConflictPolicy::SkipIfIdentical,
            false,
        )
        .unwrap_err();
        assert!(error.to_string().contains("Multiple selected sources"));
        assert_eq!(fs::read_dir(&destination).unwrap().count(), 0);
        assert_eq!(fs::read(first).unwrap(), b"equal");
        assert_eq!(fs::read(second).unwrap(), b"equal");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn preflight_rejects_nested_selected_sources_and_self_copy() {
        let root = test_root();
        let parent = root.join("folder");
        let destination = root.join("out");
        fs::create_dir(&parent).unwrap();
        fs::create_dir(&destination).unwrap();
        let child = parent.join("child.txt");
        fs::write(&child, b"safe").unwrap();
        let err = preview_transfer(
            &[parent.clone(), child.clone()],
            &destination,
            ConflictPolicy::KeepBoth,
            true,
        )
        .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        let inside = parent.join("inside");
        fs::create_dir(&inside).unwrap();
        let err = preview_transfer(
            std::slice::from_ref(&parent),
            &inside,
            ConflictPolicy::KeepBoth,
            false,
        )
        .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(fs::read(child).unwrap(), b"safe");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn source_changed_after_preflight_stops_before_changing_second_item() {
        let root = test_root();
        let destination = root.join("out");
        fs::create_dir(&destination).unwrap();
        let first = root.join("first.txt");
        let second = root.join("second.txt");
        fs::write(&first, b"first").unwrap();
        fs::write(&second, b"original").unwrap();
        let mut changed = false;
        let report = perform_with_report(
            &Action::Copy {
                sources: vec![first.clone(), second.clone()],
                destination: destination.clone(),
                conflict: ConflictPolicy::KeepBoth,
            },
            &AtomicBool::new(false),
            &mut |_| {
                if !changed {
                    fs::write(&second, b"changed after preflight with different length").unwrap();
                    changed = true;
                }
            },
        );
        assert!(changed);
        assert_eq!(
            report.result.unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(report.items[0].state, ItemState::Completed);
        assert_eq!(report.items[1].state, ItemState::Failed);
        assert!(destination.join("first.txt").exists());
        assert!(!destination.join("second.txt").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cancelled_report_does_not_mark_unstarted_items_as_completed() {
        let root = test_root();
        let destination = root.join("out");
        fs::create_dir(&destination).unwrap();
        let source = root.join("sample.txt");
        fs::write(&source, b"untouched").unwrap();
        let report = perform_with_report(
            &Action::Copy {
                sources: vec![source.clone()],
                destination: destination.clone(),
                conflict: ConflictPolicy::KeepBoth,
            },
            &AtomicBool::new(true),
            &mut |_| {},
        );
        assert_eq!(
            report.result.unwrap_err().kind(),
            io::ErrorKind::Interrupted
        );
        assert_eq!(report.items[0].state, ItemState::Cancelled);
        assert!(!destination.join("sample.txt").exists());
        assert_eq!(fs::read(source).unwrap(), b"untouched");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn staged_copy_publishes_files_and_directories_without_residue() {
        let root = test_root();
        let folder = root.join("folder");
        fs::create_dir(&folder).unwrap();
        fs::write(folder.join("nested.txt"), b"nested").unwrap();
        let file = root.join("file.txt");
        fs::write(&file, b"original").unwrap();
        let output = root.join("output");
        fs::create_dir(&output).unwrap();

        perform(&Action::Copy {
            sources: vec![folder, file],
            destination: output.clone(),
            conflict: ConflictPolicy::KeepBoth,
        })
        .unwrap();

        assert_eq!(fs::read(output.join("file.txt")).unwrap(), b"original");
        assert_eq!(
            fs::read(output.join("folder/nested.txt")).unwrap(),
            b"nested"
        );
        assert_eq!(fs::read_dir(&output).unwrap().count(), 2);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn copied_files_and_directories_preserve_modification_time() {
        let root = test_root();
        let folder = root.join("folder");
        fs::create_dir(&folder).unwrap();
        let file = folder.join("note.txt");
        fs::write(&file, b"unchanged").unwrap();
        let expected_file_time = UNIX_EPOCH + Duration::from_secs(946_684_800);
        let expected_folder_time = UNIX_EPOCH + Duration::from_secs(978_307_200);
        File::open(&file)
            .unwrap()
            .set_modified(expected_file_time)
            .unwrap();
        File::open(&folder)
            .unwrap()
            .set_modified(expected_folder_time)
            .unwrap();
        let output = root.join("output");
        fs::create_dir(&output).unwrap();

        perform(&Action::Copy {
            sources: vec![folder],
            destination: output.clone(),
            conflict: ConflictPolicy::KeepBoth,
        })
        .unwrap();

        assert_eq!(
            fs::metadata(output.join("folder/note.txt"))
                .unwrap()
                .modified()
                .unwrap(),
            expected_file_time
        );
        assert_eq!(
            fs::metadata(output.join("folder"))
                .unwrap()
                .modified()
                .unwrap(),
            expected_folder_time
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn source_modified_during_copy_is_not_published() {
        let root = test_root();
        let source = root.join("large.bin");
        fs::write(&source, vec![42u8; 8 * 1024 * 1024]).unwrap();
        let output = root.join("output");
        fs::create_dir(&output).unwrap();
        let cancelled = AtomicBool::new(false);
        let mut modified = false;
        let result = perform_with_progress(
            &Action::Copy {
                sources: vec![source.clone()],
                destination: output.clone(),
                conflict: ConflictPolicy::KeepBoth,
            },
            &cancelled,
            &mut |progress| {
                if !modified && progress.transferred >= 4 * 1024 * 1024 {
                    fs::write(&source, vec![99u8; 8 * 1024 * 1024]).unwrap();
                    modified = true;
                }
            },
        );
        assert!(modified);
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::InvalidData);
        assert_eq!(fs::read_dir(&output).unwrap().count(), 0);
        assert_eq!(fs::read(&source).unwrap(), vec![99u8; 8 * 1024 * 1024]);
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn partial_copy_batch_reports_failed_item_and_prior_successes() {
        let root = test_root();
        let valid = root.join("valid.txt");
        fs::write(&valid, b"already committed").unwrap();
        let socket = root.join("unsupported-socket");
        let _listener = special_file_fixture(&socket);
        let output = root.join("output");
        fs::create_dir(&output).unwrap();

        let error = perform(&Action::Copy {
            sources: vec![valid.clone(), socket.clone()],
            destination: output.clone(),
            conflict: ConflictPolicy::KeepBoth,
        })
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Unsupported);
        assert!(error.to_string().contains("item 2 of 2"));
        assert!(error.to_string().contains("1 earlier item(s) completed"));
        assert_eq!(
            fs::read(output.join("valid.txt")).unwrap(),
            b"already committed"
        );
        assert_eq!(fs::read(valid).unwrap(), b"already committed");
        assert!(fs::symlink_metadata(socket).is_ok());
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn replacement_source_path_is_not_removed_or_published() {
        use std::os::unix::fs::symlink;

        let root = test_root();
        let source = root.join("large.bin");
        fs::write(&source, vec![42u8; 8 * 1024 * 1024]).unwrap();
        let output = root.join("output");
        fs::create_dir(&output).unwrap();
        let original = root.join("original.bin");
        let unrelated = root.join("unrelated.txt");
        fs::write(&unrelated, b"must remain private").unwrap();
        let cancelled = AtomicBool::new(false);
        let mut swapped = false;
        let result = perform_with_progress(
            &Action::Copy {
                sources: vec![source.clone()],
                destination: output.clone(),
                conflict: ConflictPolicy::KeepBoth,
            },
            &cancelled,
            &mut |progress| {
                if !swapped && progress.transferred >= 4 * 1024 * 1024 {
                    fs::rename(&source, &original).unwrap();
                    symlink(&unrelated, &source).unwrap();
                    swapped = true;
                }
            },
        );
        assert!(swapped);
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::InvalidData);
        assert_eq!(fs::read_dir(&output).unwrap().count(), 0);
        assert_eq!(fs::read(&unrelated).unwrap(), b"must remain private");
        assert!(
            fs::symlink_metadata(&source)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::metadata(&original).unwrap().len(), 8 * 1024 * 1024);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn publication_never_overwrites_file_created_during_copy() {
        let root = test_root();
        let source = root.join("large.bin");
        fs::write(&source, vec![42u8; 8 * 1024 * 1024]).unwrap();
        let output = root.join("output");
        fs::create_dir(&output).unwrap();
        let cancelled = AtomicBool::new(false);
        let mut inserted = false;
        let result = perform_with_progress(
            &Action::Copy {
                sources: vec![source.clone()],
                destination: output.clone(),
                conflict: ConflictPolicy::KeepBoth,
            },
            &cancelled,
            &mut |progress| {
                if !inserted && progress.transferred >= 4 * 1024 * 1024 {
                    fs::write(output.join("large.bin"), b"other writer").unwrap();
                    inserted = true;
                }
            },
        );
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(output.join("large.bin")).unwrap(), b"other writer");
        assert_eq!(fs::metadata(source).unwrap().len(), 8 * 1024 * 1024);
        assert_eq!(fs::read_dir(&output).unwrap().count(), 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn failed_copy_cleans_staging_and_preserves_unsupported_source() {
        let root = test_root();
        let socket = root.join("unix-socket");
        let _listener = special_file_fixture(&socket);
        let output = root.join("output");
        fs::create_dir(&output).unwrap();

        let error = perform(&Action::Copy {
            sources: vec![socket.clone()],
            destination: output.clone(),
            conflict: ConflictPolicy::KeepBoth,
        })
        .unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::Unsupported);
        assert!(fs::symlink_metadata(socket).is_ok());
        assert_eq!(fs::read_dir(&output).unwrap().count(), 0);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failed_verification_never_publishes_and_keeps_source() {
        let root = test_root();
        let source = root.join("source.txt");
        fs::write(&source, b"ORIGINAL").unwrap();
        let output = root.join("output");
        fs::create_dir(&output).unwrap();
        let cancelled = AtomicBool::new(false);
        let mut corrupted = false;
        let mut callback = |progress: Progress| {
            if progress.phase == "Verifying" && !corrupted {
                let staging_dir = fs::read_dir(&output)
                    .unwrap()
                    .next()
                    .unwrap()
                    .unwrap()
                    .path();
                fs::write(staging_dir.join("payload"), b"CORRUPTX").unwrap();
                corrupted = true;
            }
        };
        let mut transfer = Transfer {
            cancelled: &cancelled,
            notify: &mut callback,
            transferred: 0,
            total: 8,
            last_report: Instant::now(),
            last_reported_bytes: 0,
            phase: "Copying",
        };

        let result = stage_and_publish(&source, &output.join("source.txt"), true, &mut transfer);
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::InvalidData);
        assert!(corrupted);
        assert_eq!(fs::read(source).unwrap(), b"ORIGINAL");
        assert_eq!(fs::read_dir(&output).unwrap().count(), 0);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_self_copy_and_invalid_names() {
        let root = test_root();
        let folder = root.join("folder");
        fs::create_dir(&folder).unwrap();
        let err = perform(&Action::Copy {
            sources: vec![folder.clone()],
            destination: folder.clone(),
            conflict: ConflictPolicy::KeepBoth,
        })
        .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert!(!valid_name("../test"));
        assert!(!valid_name("."));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn renaming_never_replaces_an_existing_file() {
        let root = test_root();
        fs::write(root.join("a"), b"a").unwrap();
        fs::write(root.join("b"), b"b").unwrap();
        let result = perform(&Action::Rename {
            source: root.join("a"),
            name: "b".into(),
        });
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(root.join("b")).unwrap(), b"b");
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn successful_rename_offers_session_undo_without_overwriting_existing_names() {
        let root = test_root();
        let original = root.join("original.txt");
        let current = root.join("changed.txt");
        fs::write(&original, b"preserve me").unwrap();
        let report = perform_with_report(
            &Action::Rename {
                source: original.clone(),
                name: "changed.txt".into(),
            },
            &AtomicBool::new(false),
            &mut |_| {},
        );
        assert!(report.result.is_ok());
        let undo = report
            .rename_undo
            .expect("successful rename should offer Undo");
        assert_eq!(undo.current, current);
        assert_eq!(undo.original, original);

        fs::write(&original, b"other writer").unwrap();
        let error = perform(&undo.action()).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&current).unwrap(), b"preserve me");
        assert_eq!(fs::read(&original).unwrap(), b"other writer");
        fs::remove_file(&original).unwrap();

        assert!(perform(&undo.action()).is_ok());
        assert!(!current.exists());
        assert_eq!(fs::read(&original).unwrap(), b"preserve me");
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn rename_undo_refuses_modified_or_replaced_target() {
        let root = test_root();
        let original = root.join("original.txt");
        let current = root.join("changed.txt");
        fs::write(&original, b"original").unwrap();
        let rename = || {
            perform_with_report(
                &Action::Rename {
                    source: original.clone(),
                    name: "changed.txt".into(),
                },
                &AtomicBool::new(false),
                &mut |_| {},
            )
        };
        let undo = rename().rename_undo.unwrap();
        fs::write(&current, b"modified").unwrap();
        assert_eq!(
            perform(&undo.action()).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        assert!(!original.exists());
        assert_eq!(fs::read(&current).unwrap(), b"modified");

        // A replacement inode with identical bytes must not be renamed back.
        let held = File::open(&current).unwrap();
        fs::remove_file(&current).unwrap();
        fs::write(&current, b"original").unwrap();
        assert_eq!(
            perform(&undo.action()).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(fs::read(&current).unwrap(), b"original");
        assert!(!original.exists());
        drop(held);
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn rename_undo_restores_original_non_utf8_filename_bytes() {
        use std::os::unix::ffi::OsStringExt;

        let root = test_root();
        let raw = OsString::from_vec(vec![b'n', 0xff, b'm', b'e']);
        let original = root.join(raw);
        fs::write(&original, b"non-UTF8 source").unwrap();
        let rename = perform_with_report(
            &Action::Rename {
                source: original.clone(),
                name: "renamed.txt".into(),
            },
            &AtomicBool::new(false),
            &mut |_| {},
        );
        let undo = rename.rename_undo.unwrap();
        assert!(perform(&undo.action()).is_ok());
        assert_eq!(fs::read(&original).unwrap(), b"non-UTF8 source");
        assert!(!root.join("renamed.txt").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn batch_rename_undo_restores_swaps_and_preserves_both_payloads() {
        let root = test_root();
        let a = root.join("a");
        let b = root.join("b");
        fs::write(&a, b"first content").unwrap();
        fs::write(&b, b"second content").unwrap();
        let report = perform_with_report(
            &Action::BatchRename {
                changes: vec![(a.clone(), "b".into()), (b.clone(), "a".into())],
            },
            &AtomicBool::new(false),
            &mut |_| {},
        );
        assert!(report.result.is_ok());
        assert_eq!(fs::read(&a).unwrap(), b"second content");
        let undo = report
            .batch_rename_undo
            .expect("successful batch offers Undo");
        assert!(perform(&undo.action()).is_ok());
        assert_eq!(fs::read(&a).unwrap(), b"first content");
        assert_eq!(fs::read(&b).unwrap(), b"second content");
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn batch_rename_undo_refuses_any_stale_inode_before_changing_names() {
        let root = test_root();
        let a = root.join("a");
        let b = root.join("b");
        let changed_a = root.join("changed-a");
        let changed_b = root.join("changed-b");
        fs::write(&a, b"first").unwrap();
        fs::write(&b, b"second").unwrap();
        let rename = || {
            perform_with_report(
                &Action::BatchRename {
                    changes: vec![
                        (a.clone(), "changed-a".into()),
                        (b.clone(), "changed-b".into()),
                    ],
                },
                &AtomicBool::new(false),
                &mut |_| {},
            )
        };
        let report = rename();
        assert!(report.result.is_ok());
        let undo = report.batch_rename_undo.unwrap();
        fs::write(&changed_b, b"modified after batch").unwrap();
        assert_eq!(
            perform(&undo.action()).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        assert!(!a.exists() && !b.exists());
        assert_eq!(fs::read(&changed_a).unwrap(), b"first");
        assert_eq!(fs::read(&changed_b).unwrap(), b"modified after batch");
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn batch_rename_undo_never_replaces_original_name_taken_by_another_writer() {
        let root = test_root();
        let a = root.join("a");
        let b = root.join("b");
        let changed_a = root.join("changed-a");
        let changed_b = root.join("changed-b");
        fs::write(&a, b"first").unwrap();
        fs::write(&b, b"second").unwrap();
        let report = perform_with_report(
            &Action::BatchRename {
                changes: vec![
                    (a.clone(), "changed-a".into()),
                    (b.clone(), "changed-b".into()),
                ],
            },
            &AtomicBool::new(false),
            &mut |_| {},
        );
        assert!(report.result.is_ok());
        let undo = report.batch_rename_undo.unwrap();
        fs::write(&b, b"another writer").unwrap();
        assert_eq!(
            perform(&undo.action()).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(fs::read(&b).unwrap(), b"another writer");
        assert_eq!(fs::read(&changed_a).unwrap(), b"first");
        assert_eq!(fs::read(&changed_b).unwrap(), b"second");
        assert!(!a.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn batch_undo_restores_non_utf8_names_and_selected_hardlink_aliases() {
        use std::os::unix::ffi::OsStringExt;
        let root = test_root();
        let raw = OsString::from_vec(vec![b'o', 0xff, b'd']);
        let source = root.join(&raw);
        let alias = root.join("alias");
        fs::write(&source, b"linked payload").unwrap();
        fs::hard_link(&source, &alias).unwrap();
        let report = perform_with_report(
            &Action::BatchRename {
                changes: vec![
                    (source.clone(), "new".into()),
                    (alias.clone(), "other".into()),
                ],
            },
            &AtomicBool::new(false),
            &mut |_| {},
        );
        assert!(report.result.is_ok());
        let undo = report
            .batch_rename_undo
            .expect("batch Undo offered for hardlinks");
        assert!(perform(&undo.action()).is_ok());
        let first = fs::symlink_metadata(&source).unwrap();
        let second = fs::symlink_metadata(&alias).unwrap();
        assert!(same_inode_and_kind(&first, &second));
        assert_eq!(fs::read(source).unwrap(), b"linked payload");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn batch_rename_preview_is_executed_without_losing_contents() {
        let root = test_root();
        let first = root.join("alpha.txt");
        let second = root.join("beta.txt");
        fs::write(&first, b"alpha payload").unwrap();
        fs::write(&second, b"beta payload").unwrap();
        let changes =
            plan_batch_rename(&[first.clone(), second.clone()], "photo-", "-old").unwrap();
        assert_eq!(changes[0].1, OsString::from("photo-alpha.txt-old"));
        perform(&Action::BatchRename { changes }).unwrap();
        assert_eq!(
            fs::read(root.join("photo-alpha.txt-old")).unwrap(),
            b"alpha payload"
        );
        assert_eq!(
            fs::read(root.join("photo-beta.txt-old")).unwrap(),
            b"beta payload"
        );
        assert!(!first.exists());
        assert!(!second.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn batch_rename_numbering_and_literal_replacement_are_predictable() {
        let root = test_root();
        let first = root.join("old-alpha.txt");
        let second = root.join("old-beta.txt");
        fs::write(&first, b"alpha").unwrap();
        fs::write(&second, b"beta").unwrap();
        let changes = plan_batch_rename_with_options(
            &[first.clone(), second.clone()],
            &BatchRenameOptions {
                prefix: "image-",
                suffix: "-backup",
                find: "old-",
                replace: "",
                regex: false,
                numbering: true,
                number_start: 7,
                number_width: 3,
            },
        )
        .unwrap();
        assert_eq!(changes[0].1, OsString::from("image-007-alpha.txt-backup"));
        assert_eq!(changes[1].1, OsString::from("image-008-beta.txt-backup"));
        perform(&Action::BatchRename { changes }).unwrap();
        assert_eq!(
            fs::read(root.join("image-007-alpha.txt-backup")).unwrap(),
            b"alpha"
        );
        assert_eq!(
            fs::read(root.join("image-008-beta.txt-backup")).unwrap(),
            b"beta"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn batch_rename_regex_captures_preview_and_execute_without_overwrite() {
        let root = test_root();
        let old = root.join("image-004.jpeg");
        fs::write(&old, b"original payload").unwrap();
        let options = BatchRenameOptions {
            prefix: "",
            suffix: "",
            find: r"^image-(\d+)(\.jpeg)$",
            replace: "picture_${1}${2}",
            regex: true,
            numbering: false,
            number_start: 1,
            number_width: 3,
        };
        let changes = plan_batch_rename_with_options(std::slice::from_ref(&old), &options)
            .expect("regex capture preview");
        assert_eq!(changes[0].1, OsString::from("picture_004.jpeg"));
        perform(&Action::BatchRename { changes }).unwrap();
        assert_eq!(
            fs::read(root.join("picture_004.jpeg")).unwrap(),
            b"original payload"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn batch_rename_regex_rejects_bad_patterns_and_unsafe_generated_names() {
        let root = test_root();
        let source = root.join("image.jpeg");
        fs::write(&source, b"kept").unwrap();
        let options = BatchRenameOptions {
            prefix: "",
            suffix: "",
            find: "(",
            replace: "image",
            regex: true,
            numbering: false,
            number_start: 1,
            number_width: 3,
        };
        assert!(
            plan_batch_rename_with_options(std::slice::from_ref(&source), &options)
                .unwrap_err()
                .to_string()
                .contains("Invalid batch rename regex")
        );
        let unsafe_output = BatchRenameOptions {
            find: "^.*$",
            replace: "..",
            ..options
        };
        assert!(
            plan_batch_rename_with_options(std::slice::from_ref(&source), &unsafe_output).is_err()
        );
        assert_eq!(fs::read(source).unwrap(), b"kept");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn batch_rename_rejects_unchanged_invalid_and_duplicate_name_plans() {
        let root = test_root();
        let source = root.join("one.txt");
        fs::write(&source, b"original").unwrap();
        let default = BatchRenameOptions {
            prefix: "",
            suffix: "",
            find: "absent",
            replace: "new",
            regex: false,
            numbering: false,
            number_start: 1,
            number_width: 3,
        };
        assert!(plan_batch_rename_with_options(std::slice::from_ref(&source), &default).is_err());
        let invalid = BatchRenameOptions {
            find: "one",
            replace: "../unsafe",
            ..default
        };
        assert!(plan_batch_rename_with_options(std::slice::from_ref(&source), &invalid).is_err());
        let duplicate = BatchRenameOptions {
            prefix: "",
            suffix: "",
            find: "one",
            replace: "two",
            regex: false,
            numbering: false,
            number_start: 1,
            number_width: 3,
        };
        assert!(
            plan_batch_rename_with_options(&[source.clone(), source.clone()], &duplicate).is_err()
        );
        assert_eq!(fs::read(source).unwrap(), b"original");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn batch_rename_preflight_rejects_collision_before_any_change() {
        let root = test_root();
        let first = root.join("one");
        let second = root.join("two");
        fs::write(&first, b"one").unwrap();
        fs::write(&second, b"two").unwrap();
        fs::write(root.join("X-two"), b"unrelated").unwrap();
        let changes = plan_batch_rename(&[first.clone(), second.clone()], "X-", "").unwrap();
        let error = perform(&Action::BatchRename { changes }).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(first).unwrap(), b"one");
        assert_eq!(fs::read(second).unwrap(), b"two");
        assert_eq!(fs::read(root.join("X-two")).unwrap(), b"unrelated");
        assert!(!root.join("X-one").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn batch_rename_rolls_back_if_target_appears_mid_batch() {
        let root = test_root();
        let first = root.join("one");
        let second = root.join("two");
        fs::write(&first, b"one").unwrap();
        fs::write(&second, b"two").unwrap();
        let changes = plan_batch_rename(&[first.clone(), second.clone()], "X-", "").unwrap();
        let mut inserted = false;
        let result = perform_with_progress(
            &Action::BatchRename { changes },
            &AtomicBool::new(false),
            &mut |progress| {
                if progress.phase == "Renaming" && progress.transferred == 1 && !inserted {
                    fs::write(root.join("X-two"), b"external writer").unwrap();
                    inserted = true;
                }
            },
        );
        let error = result.unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert!(error.to_string().contains("rolled back"));
        assert!(inserted);
        assert_eq!(fs::read(first).unwrap(), b"one");
        assert_eq!(fs::read(second).unwrap(), b"two");
        assert_eq!(fs::read(root.join("X-two")).unwrap(), b"external writer");
        assert!(!root.join("X-one").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn batch_rename_atomic_exchange_swaps_files_and_preserves_hardlinks() {
        use std::os::unix::fs::MetadataExt;
        let root = test_root();
        let a = root.join("ab");
        let b = root.join("ba");
        let hardlink = root.join("outside-hardlink");
        fs::write(&a, b"first").unwrap();
        fs::write(&b, b"second").unwrap();
        fs::hard_link(&a, &hardlink).unwrap();
        let changes = vec![
            (a.clone(), OsString::from("ba")),
            (b.clone(), OsString::from("ab")),
        ];
        let report = perform_with_report(
            &Action::BatchRename { changes },
            &AtomicBool::new(false),
            &mut |_| {},
        );
        assert!(report.result.is_ok(), "{:?}", report.result);
        assert!(
            report
                .items
                .iter()
                .all(|item| item.state == ItemState::Completed)
        );
        assert_eq!(fs::read(&a).unwrap(), b"second");
        assert_eq!(fs::read(&b).unwrap(), b"first");
        assert_eq!(
            fs::metadata(&hardlink).unwrap().ino(),
            fs::metadata(&b).unwrap().ino()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn batch_rename_handles_two_selected_names_of_one_hardlinked_inode() {
        let root = test_root();
        let a = root.join("a");
        let b = root.join("b");
        let c = root.join("c");
        fs::write(&a, b"linked").unwrap();
        fs::write(&b, b"other").unwrap();
        fs::hard_link(&a, &c).unwrap();
        let result = perform(&Action::BatchRename {
            changes: vec![
                (a.clone(), OsString::from("b")),
                (b.clone(), OsString::from("c")),
                (c.clone(), OsString::from("a")),
            ],
        });
        assert!(result.is_ok(), "{result:?}");
        assert_eq!(fs::read(&a).unwrap(), b"linked");
        assert_eq!(fs::read(&b).unwrap(), b"linked");
        assert_eq!(fs::read(&c).unwrap(), b"other");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn batch_rename_guarded_rollback_refreshes_selected_hardlink_aliases() {
        let root = test_root();
        fs::write(root.join("a"), b"linked").unwrap();
        fs::hard_link(root.join("a"), root.join("c")).unwrap();
        fs::write(root.join("b"), b"second").unwrap();
        fs::write(root.join("d"), b"fourth").unwrap();
        let cancelled = AtomicBool::new(false);
        let result = perform_with_report(
            &Action::BatchRename {
                changes: vec![
                    (root.join("a"), OsString::from("b")),
                    (root.join("b"), OsString::from("c")),
                    (root.join("c"), OsString::from("d")),
                    (root.join("d"), OsString::from("a")),
                ],
            },
            &cancelled,
            &mut |progress| {
                if progress.transferred == 2 {
                    cancelled.store(true, Ordering::Relaxed);
                }
            },
        );
        let error = result.result.unwrap_err();
        assert!(error.to_string().contains("rolled back"), "{error}");
        assert_eq!(fs::read(root.join("a")).unwrap(), b"linked");
        assert_eq!(fs::read(root.join("b")).unwrap(), b"second");
        assert_eq!(fs::read(root.join("c")).unwrap(), b"linked");
        assert_eq!(fs::read(root.join("d")).unwrap(), b"fourth");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn batch_rename_rotates_three_names_and_moves_chains_without_staging() {
        let root = test_root();
        for (name, content) in [("a", b"a"), ("b", b"b"), ("c", b"c"), ("d", b"d")] {
            fs::write(root.join(name), content).unwrap();
        }
        perform(&Action::BatchRename {
            changes: vec![
                (root.join("a"), OsString::from("b")),
                (root.join("b"), OsString::from("c")),
                (root.join("c"), OsString::from("a")),
            ],
        })
        .unwrap();
        assert_eq!(fs::read(root.join("a")).unwrap(), b"c");
        assert_eq!(fs::read(root.join("b")).unwrap(), b"a");
        assert_eq!(fs::read(root.join("c")).unwrap(), b"b");
        perform(&Action::BatchRename {
            changes: vec![
                (root.join("c"), OsString::from("d2")),
                (root.join("b"), OsString::from("c")),
                (root.join("a"), OsString::from("b")),
            ],
        })
        .unwrap();
        assert_eq!(fs::read(root.join("d2")).unwrap(), b"b");
        assert_eq!(fs::read(root.join("c")).unwrap(), b"a");
        assert_eq!(fs::read(root.join("b")).unwrap(), b"c");
        assert_eq!(fs::read(root.join("d")).unwrap(), b"d");
        assert!(!root.join("a").exists());
        assert_eq!(fs::read_dir(&root).unwrap().count(), 4);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn batch_rename_cycle_conflict_with_unselected_entry_never_mutates() {
        let root = test_root();
        for name in ["a", "b", "unselected"] {
            fs::write(root.join(name), name.as_bytes()).unwrap();
        }
        let report = perform_with_report(
            &Action::BatchRename {
                changes: vec![
                    (root.join("a"), OsString::from("b")),
                    (root.join("b"), OsString::from("unselected")),
                ],
            },
            &AtomicBool::new(false),
            &mut |_| {},
        );
        assert_eq!(
            report.result.unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        for name in ["a", "b", "unselected"] {
            assert_eq!(fs::read(root.join(name)).unwrap(), name.as_bytes());
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn batch_rename_cancel_reverses_completed_exchange_steps() {
        let root = test_root();
        for name in ["a", "b", "c"] {
            fs::write(root.join(name), name.as_bytes()).unwrap();
        }
        let cancelled = AtomicBool::new(false);
        let result = perform_with_report(
            &Action::BatchRename {
                changes: vec![
                    (root.join("a"), OsString::from("b")),
                    (root.join("b"), OsString::from("c")),
                    (root.join("c"), OsString::from("a")),
                ],
            },
            &cancelled,
            &mut |progress| {
                if progress.transferred >= 1 {
                    cancelled.store(true, Ordering::Relaxed);
                }
            },
        );
        assert_eq!(
            result.result.unwrap_err().kind(),
            io::ErrorKind::Interrupted
        );
        for name in ["a", "b", "c"] {
            assert_eq!(fs::read(root.join(name)).unwrap(), name.as_bytes());
        }
        assert!(
            result
                .items
                .iter()
                .all(|item| item.state != ItemState::Completed)
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn batch_rename_never_rolls_back_over_a_changed_exchanged_entry() {
        let root = test_root();
        for name in ["a", "b", "c"] {
            fs::write(root.join(name), name.as_bytes()).unwrap();
        }
        let cancelled = AtomicBool::new(false);
        let result = perform_with_report(
            &Action::BatchRename {
                changes: vec![
                    (root.join("a"), OsString::from("b")),
                    (root.join("b"), OsString::from("c")),
                    (root.join("c"), OsString::from("a")),
                ],
            },
            &cancelled,
            &mut |progress| {
                if progress.transferred == 1 {
                    fs::write(root.join("b"), b"external change").unwrap();
                    cancelled.store(true, Ordering::Relaxed);
                }
            },
        );
        let error = result.result.unwrap_err();
        assert!(error.to_string().contains("rollback also stopped"));
        assert_eq!(fs::read(root.join("b")).unwrap(), b"external change");
        assert_eq!(fs::read(root.join("a")).unwrap(), b"b");
        assert_eq!(fs::read(root.join("c")).unwrap(), b"c");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn delete_batch_preflight_refuses_missing_later_item_without_deletion() {
        let root = test_root();
        let first = root.join("first");
        let missing = root.join("missing");
        let last = root.join("last");
        fs::write(&first, b"first").unwrap();
        fs::write(&last, b"last").unwrap();
        let report = perform_with_report(
            &Action::Delete {
                paths: vec![first.clone(), missing, last.clone()],
            },
            &AtomicBool::new(false),
            &mut |_| {},
        );
        assert!(
            report
                .result
                .unwrap_err()
                .to_string()
                .contains("No items were removed")
        );
        assert_eq!(report.items[0].state, ItemState::NotStarted);
        assert_eq!(report.items[1].state, ItemState::Failed);
        assert_eq!(report.items[2].state, ItemState::NotStarted);
        assert_eq!(fs::read(first).unwrap(), b"first");
        assert_eq!(fs::read(last).unwrap(), b"last");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn destructive_batch_rejects_duplicate_and_nested_paths_before_changes() {
        let root = test_root();
        let parent = root.join("folder");
        fs::create_dir(&parent).unwrap();
        let child = parent.join("file");
        fs::write(&child, b"safe").unwrap();
        for paths in [
            vec![parent.clone(), parent.clone()],
            vec![child.clone(), parent.clone()],
            vec![parent.clone(), child.clone()],
        ] {
            let result = perform(&Action::Delete { paths }).unwrap_err();
            assert_eq!(result.kind(), io::ErrorKind::InvalidInput);
            assert!(parent.is_dir());
            assert_eq!(fs::read(&child).unwrap(), b"safe");
        }
        let result = perform(&Action::Trash {
            paths: vec![parent.clone(), parent.clone()],
        })
        .unwrap_err();
        assert_eq!(result.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(fs::read(&child).unwrap(), b"safe");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn delete_batch_reports_success_and_stops_on_later_source_mutation() {
        let root = test_root();
        let first = root.join("first");
        let second = root.join("second");
        fs::write(&first, b"first").unwrap();
        fs::write(&second, b"second").unwrap();
        let mut changed = false;
        let report = perform_with_report(
            &Action::Delete {
                paths: vec![first.clone(), second.clone()],
            },
            &AtomicBool::new(false),
            &mut |progress| {
                if progress.phase == "Deleting" && progress.transferred == 1 && !changed {
                    fs::write(&second, b"changed-by-another-process").unwrap();
                    changed = true;
                }
            },
        );
        assert!(changed);
        assert_eq!(
            report.result.unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(report.items[0].state, ItemState::Completed);
        assert_eq!(report.items[1].state, ItemState::Failed);
        assert!(!first.exists());
        assert_eq!(fs::read(second).unwrap(), b"changed-by-another-process");
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn source_identity_rejects_same_size_rewrite_with_restored_mtime() {
        let root = test_root();
        let source = root.join("source");
        fs::write(&source, b"before").unwrap();
        let initial = fs::symlink_metadata(&source).unwrap();
        std::thread::sleep(Duration::from_millis(3));
        fs::write(&source, b"after!").unwrap();
        File::options()
            .write(true)
            .open(&source)
            .unwrap()
            .set_times(FileTimes::new().set_modified(initial.modified().unwrap()))
            .unwrap();
        let current = fs::symlink_metadata(&source).unwrap();
        assert_eq!(initial.len(), current.len());
        assert_eq!(initial.modified().unwrap(), current.modified().unwrap());
        assert_eq!(
            ensure_same_source(&source, &initial).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn batch_rename_keeps_non_utf8_name_bytes_and_rejects_duplicate_targets() {
        use std::os::unix::ffi::OsStringExt;
        let root = test_root();
        let source = root.join(OsString::from_vec(b"raw-\xfe".to_vec()));
        fs::write(&source, b"payload").unwrap();
        assert!(plan_batch_rename(&[source.clone(), source.clone()], "x-", "").is_err());
        let changes = plan_batch_rename(std::slice::from_ref(&source), "x-", "-end").unwrap();
        perform(&Action::BatchRename { changes }).unwrap();
        let target = root.join(OsString::from_vec(b"x-raw-\xfe-end".to_vec()));
        assert_eq!(fs::read(target).unwrap(), b"payload");
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn batch_numbering_keeps_non_utf8_bytes_but_literal_replacement_refuses_them() {
        use std::os::unix::ffi::OsStringExt;
        let root = test_root();
        let source = root.join(OsString::from_vec(b"raw-\xfe".to_vec()));
        fs::write(&source, b"payload").unwrap();
        let numbered = BatchRenameOptions {
            prefix: "",
            suffix: "",
            find: "",
            replace: "",
            regex: false,
            numbering: true,
            number_start: 4,
            number_width: 2,
        };
        let changes =
            plan_batch_rename_with_options(std::slice::from_ref(&source), &numbered).unwrap();
        assert_eq!(changes[0].1, OsString::from_vec(b"04-raw-\xfe".to_vec()));
        let find = BatchRenameOptions {
            find: "raw",
            replace: "new",
            ..numbered
        };
        assert!(plan_batch_rename_with_options(std::slice::from_ref(&source), &find).is_err());
        assert_eq!(fs::read(source).unwrap(), b"payload");
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn broken_symlink_is_not_overwritten() {
        use std::os::unix::fs::symlink;
        let root = test_root();
        fs::write(root.join("original"), b"content").unwrap();
        symlink(root.join("absent"), root.join("link")).unwrap();
        let result = perform(&Action::Rename {
            source: root.join("original"),
            name: "link".into(),
        });
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(
            fs::read_link(root.join("link")).unwrap(),
            root.join("absent")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn colliding_non_utf8_filename_keeps_original_bytes() {
        use std::os::unix::ffi::OsStringExt;
        let root = test_root();
        let name = OsString::from_vec(b"report-\xff.txt".to_vec());
        let source = root.join(&name);
        fs::write(&source, b"original").unwrap();
        let output = root.join("output");
        fs::create_dir(&output).unwrap();
        fs::write(output.join(&name), b"existing").unwrap();

        perform(&Action::Copy {
            sources: vec![source.clone()],
            destination: output.clone(),
            conflict: ConflictPolicy::KeepBoth,
        })
        .unwrap();

        assert_eq!(fs::read(output.join(&name)).unwrap(), b"existing");
        let duplicate = OsString::from_vec(b"report-\xff (copy).txt".to_vec());
        assert_eq!(fs::read(output.join(duplicate)).unwrap(), b"original");
        assert_eq!(fs::read(source).unwrap(), b"original");
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn colliding_broken_symlink_preserves_name_and_both_targets() {
        use std::{os::unix::ffi::OsStringExt, os::unix::fs::symlink};
        let root = test_root();
        let name = OsString::from_vec(b"link-\xfe".to_vec());
        let source = root.join(&name);
        symlink("missing-original", &source).unwrap();
        let output = root.join("output");
        fs::create_dir(&output).unwrap();
        symlink("missing-existing", output.join(&name)).unwrap();

        perform(&Action::Copy {
            sources: vec![source.clone()],
            destination: output.clone(),
            conflict: ConflictPolicy::KeepBoth,
        })
        .unwrap();

        assert_eq!(
            fs::read_link(output.join(&name)).unwrap(),
            PathBuf::from("missing-existing")
        );
        let duplicate = OsString::from_vec(b"link-\xfe (copy)".to_vec());
        assert_eq!(
            fs::read_link(output.join(duplicate)).unwrap(),
            PathBuf::from("missing-original")
        );
        assert_eq!(
            fs::read_link(source).unwrap(),
            PathBuf::from("missing-original")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn copies_symlink_without_following_it() {
        use std::os::unix::fs::symlink;
        let root = test_root();
        let source = root.join("original");
        let output = root.join("output");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&output).unwrap();
        symlink("../missing-target", source.join("broken")).unwrap();
        perform(&Action::Copy {
            sources: vec![source.join("broken")],
            destination: output.clone(),
            conflict: ConflictPolicy::KeepBoth,
        })
        .unwrap();
        assert_eq!(
            fs::read_link(output.join("broken")).unwrap(),
            PathBuf::from("../missing-target")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cancellation_keeps_source_and_cleans_partial_copy() {
        let root = test_root();
        let original = root.join("large.bin");
        fs::write(&original, vec![42u8; 12 * 1024 * 1024]).unwrap();
        let dest = root.join("target");
        fs::create_dir(&dest).unwrap();
        let cancelled = AtomicBool::new(false);
        let result = perform_with_progress(
            &Action::Copy {
                sources: vec![original.clone()],
                destination: dest.clone(),
                conflict: ConflictPolicy::KeepBoth,
            },
            &cancelled,
            &mut |progress| {
                if progress.transferred >= 4 * 1024 * 1024 {
                    cancelled.store(true, Ordering::Relaxed);
                }
            },
        );
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::Interrupted);
        assert!(!dest.join("large.bin").exists());
        assert_eq!(fs::metadata(original).unwrap().len(), 12 * 1024 * 1024);
        assert_eq!(fs::read_dir(&dest).unwrap().count(), 0);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn recursive_search_avoids_hidden_entries() {
        let root = test_root();
        fs::create_dir(root.join("inside")).unwrap();
        fs::write(root.join("inside").join("needle.md"), b"found").unwrap();
        fs::write(root.join(".needle-secret"), b"hidden").unwrap();
        let shown = search(&root, "needle", false).unwrap();
        assert_eq!(shown.len(), 1);
        assert!(shown[0].path.ends_with("needle.md"));
        assert_eq!(search(&root, "needle", true).unwrap().len(), 2);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn content_search_respects_scope_binary_input_and_scan_budgets() {
        let root = test_root();
        fs::create_dir(root.join("nested")).unwrap();
        fs::write(root.join("notes.txt"), b"an unusual NeEdLe in a note").unwrap();
        fs::write(root.join("binary.dat"), b"needle\0ignored").unwrap();
        fs::write(
            root.join("nested").join("deep.txt"),
            b"needle in another folder",
        )
        .unwrap();
        let cancel = AtomicBool::new(false);
        let local = search_with_options(&root, "needle", false, false, true, &cancel).unwrap();
        assert_eq!(local.entries.len(), 1);
        assert!(local.entries[0].path.ends_with("notes.txt"));
        assert!(local.notice.is_none());

        let recursive = search_with_options(&root, "needle", false, true, true, &cancel).unwrap();
        assert_eq!(recursive.entries.len(), 2);
        assert!(
            recursive
                .entries
                .iter()
                .any(|entry| entry.path.ends_with("deep.txt"))
        );

        let large = root.join("large.txt");
        fs::File::create(&large)
            .unwrap()
            .set_len(MAX_CONTENT_FILE_BYTES + 1)
            .unwrap();
        let partial = search_with_options(&root, "needle", false, false, true, &cancel).unwrap();
        assert!(partial.notice.unwrap().contains("partial"));
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn content_search_does_not_follow_symlinks() {
        use std::os::unix::fs::symlink;
        let root = test_root();
        let external = test_root();
        fs::write(external.join("target.txt"), b"hidden needle").unwrap();
        symlink(external.join("target.txt"), root.join("shortcut.txt")).unwrap();
        symlink(&external, root.join("folder-link")).unwrap();
        let results =
            search_with_options(&root, "needle", false, true, true, &AtomicBool::new(false))
                .unwrap();
        assert!(results.entries.is_empty());
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(external).unwrap();
    }

    #[test]
    fn cancelled_folder_scan_stops_before_reading_entries() {
        let root = test_root();
        fs::write(root.join("sample.txt"), b"present").unwrap();
        let cancelled = AtomicBool::new(true);
        let result = scan_cancellable(&root, &cancelled);
        assert_eq!(result.err().unwrap().kind(), io::ErrorKind::Interrupted);
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn inaccessible_mount_child_does_not_hide_readable_siblings() {
        let root = test_root();
        fs::create_dir(root.join("offline-mount")).unwrap();
        fs::write(root.join("readable.txt"), b"still visible").unwrap();
        let cancelled = AtomicBool::new(false);
        let scan = scan_with_metadata(&root, &cancelled, |path| {
            if path.file_name() == Some(OsStr::new("offline-mount")) {
                Err(io::Error::from_raw_os_error(libc::ENODEV))
            } else {
                fs::symlink_metadata(path)
            }
        })
        .unwrap();
        assert_eq!(scan.entries.len(), 2);
        assert_eq!(scan.unreadable, 1);
        assert!(scan.first_error.unwrap().contains("offline-mount"));
        assert!(
            scan.entries
                .iter()
                .any(|entry| entry.name == "readable.txt")
        );
        assert!(
            scan.entries
                .iter()
                .any(|entry| entry.name == "offline-mount" && entry.is_dir)
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unreadable_directory_itself_is_never_reported_as_empty() {
        let root = test_root();
        let not_a_folder = root.join("not-a-folder.txt");
        fs::write(&not_a_folder, b"present").unwrap();
        let result = scan_cancellable(&not_a_folder, &AtomicBool::new(false));
        assert!(result.is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cancelled_recursive_search_stops_before_reading_entries() {
        let root = test_root();
        fs::create_dir(root.join("nested")).unwrap();
        fs::write(root.join("nested").join("sample.txt"), b"present").unwrap();
        let cancelled = AtomicBool::new(true);
        let result = search_cancellable(&root, "sample", false, &cancelled);
        assert_eq!(result.err().unwrap().kind(), io::ErrorKind::Interrupted);
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn chmod_rejects_symlinks() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let root = test_root();
        let item = root.join("ordinary");
        fs::write(&item, b"content").unwrap();
        perform(&Action::SetMode {
            path: item.clone(),
            mode: 0o640,
            expected: fs::metadata(&item).unwrap(),
        })
        .unwrap();
        assert_eq!(
            fs::metadata(&item).unwrap().permissions().mode() & 0o777,
            0o640
        );
        let link = root.join("link");
        symlink(&item, &link).unwrap();
        assert!(
            perform(&Action::SetMode {
                expected: fs::symlink_metadata(&link).unwrap(),
                path: link,
                mode: 0o777,
            })
            .is_err()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn chmod_pins_the_open_inode_when_a_name_becomes_a_symlink() {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt, symlink};

        let root = test_root();
        let selected = root.join("selected");
        let moved = root.join("moved-original");
        let unrelated = root.join("unrelated");
        fs::write(&selected, b"selected inode").unwrap();
        fs::write(&unrelated, b"unrelated inode").unwrap();
        let original_unrelated = fs::metadata(&unrelated).unwrap().permissions().mode() & 0o777;

        let mut options = OpenOptions::new();
        options
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        let pinned = options.open(&selected).unwrap();
        fs::rename(&selected, &moved).unwrap();
        symlink(&unrelated, &selected).unwrap();

        let expected_pinned = pinned.metadata().unwrap();
        apply_mode_to_open_file(&pinned, 0o640, &expected_pinned).unwrap();
        assert_eq!(
            fs::metadata(&moved).unwrap().permissions().mode() & 0o777,
            0o640
        );
        assert_eq!(
            fs::metadata(&unrelated).unwrap().permissions().mode() & 0o777,
            original_unrelated
        );
        assert!(
            set_mode_guarded(&selected, 0o777, &fs::symlink_metadata(&selected).unwrap()).is_err()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn chmod_refuses_a_replaced_file_since_properties_were_opened() {
        use std::os::unix::fs::PermissionsExt;

        let root = test_root();
        let selected = root.join("selected");
        let renamed = root.join("old-selected");
        fs::write(&selected, b"file seen in properties").unwrap();
        let snapshot = fs::metadata(&selected).unwrap();
        fs::rename(&selected, &renamed).unwrap();
        fs::write(&selected, b"a different file").unwrap();
        let replacement_mode = fs::metadata(&selected).unwrap().permissions().mode() & 0o777;

        let error = perform(&Action::SetMode {
            path: selected.clone(),
            mode: 0o777,
            expected: snapshot,
        })
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert_eq!(
            fs::metadata(&selected).unwrap().permissions().mode() & 0o777,
            replacement_mode
        );
        assert_eq!(fs::read(&renamed).unwrap(), b"file seen in properties");
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn chmod_supports_unreadable_files_with_pinned_o_path_fd() {
        use std::os::unix::fs::PermissionsExt;

        let root = test_root();
        let selected = root.join("unreadable");
        fs::write(&selected, b"content").unwrap();
        fs::set_permissions(&selected, fs::Permissions::from_mode(0o000)).unwrap();
        let expected = fs::metadata(&selected).unwrap();
        let result = set_mode_unreadable_inode(&selected, 0o600, &expected);
        if result.as_ref().err().and_then(io::Error::raw_os_error) != Some(libc::ENOSYS) {
            result.unwrap();
            assert_eq!(
                fs::metadata(&selected).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn ownership_edits_are_inode_pinned_and_reject_invalid_ids() {
        use std::os::unix::fs::{MetadataExt, symlink};

        let root = test_root();
        let selected = root.join("selected");
        fs::write(&selected, b"owned file").unwrap();
        let initial = fs::metadata(&selected).unwrap();
        perform(&Action::SetOwnership {
            path: selected.clone(),
            uid: initial.uid(),
            gid: initial.gid(),
            expected: initial,
        })
        .unwrap();

        let current = fs::metadata(&selected).unwrap();
        assert_eq!(
            set_ownership_guarded(&selected, u32::MAX, current.gid(), &current)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(
            set_ownership_guarded(&selected, current.uid(), u32::MAX, &current)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );

        let link = root.join("link");
        symlink(&selected, &link).unwrap();
        let linked = fs::symlink_metadata(&link).unwrap();
        assert!(set_ownership_guarded(&link, current.uid(), current.gid(), &linked).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn ownership_refuses_a_replaced_file_since_properties_were_opened() {
        use std::os::unix::fs::MetadataExt;

        let root = test_root();
        let selected = root.join("selected");
        let old = root.join("old");
        fs::write(&selected, b"original owner target").unwrap();
        let expected = fs::metadata(&selected).unwrap();
        fs::rename(&selected, &old).unwrap();
        fs::write(&selected, b"other inode").unwrap();
        let current = fs::metadata(&selected).unwrap();

        let error = perform(&Action::SetOwnership {
            path: selected.clone(),
            uid: expected.uid(),
            gid: expected.gid(),
            expected,
        })
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        let after = fs::metadata(&selected).unwrap();
        assert_eq!((after.uid(), after.gid()), (current.uid(), current.gid()));
        assert_eq!(fs::read(&old).unwrap(), b"original owner target");
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn creating_user_attribute_never_overwrites_an_existing_value() {
        let root = test_root();
        let target = root.join("file");
        fs::write(&target, b"data").unwrap();
        let snapshot = fs::metadata(&target).unwrap();
        perform(&Action::CreateUserAttribute {
            path: target.clone(),
            name: "user.ricefield_test".into(),
            value: "first value".into(),
            expected: snapshot,
        })
        .unwrap();
        assert_eq!(
            read_link_xattr(&target, b"user.ricefield_test").unwrap(),
            b"first value"
        );

        let current = fs::metadata(&target).unwrap();
        let error = create_user_attribute_guarded(
            &target,
            "user.ricefield_test",
            "must not replace",
            &current,
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(
            read_link_xattr(&target, b"user.ricefield_test").unwrap(),
            b"first value"
        );
        assert_eq!(fs::read(&target).unwrap(), b"data");
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn creating_user_attribute_rejects_symlinks_stale_inodes_and_invalid_input() {
        use std::os::unix::fs::symlink;

        let root = test_root();
        let selected = root.join("selected");
        fs::write(&selected, b"original").unwrap();
        let expected = fs::metadata(&selected).unwrap();
        for name in ["system.posix_acl_access", "user.", "user.bad\0name"] {
            assert_eq!(
                create_user_attribute_guarded(&selected, name, "data", &expected)
                    .unwrap_err()
                    .kind(),
                io::ErrorKind::InvalidInput
            );
        }
        assert_eq!(
            create_user_attribute_guarded(&selected, "user.test", &"x".repeat(4097), &expected)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );

        let link = root.join("link");
        symlink(&selected, &link).unwrap();
        assert!(
            create_user_attribute_guarded(
                &link,
                "user.test",
                "data",
                &fs::symlink_metadata(&link).unwrap()
            )
            .is_err()
        );

        let old = root.join("old");
        fs::rename(&selected, &old).unwrap();
        fs::write(&selected, b"replacement").unwrap();
        assert_eq!(
            create_user_attribute_guarded(&selected, "user.test", "data", &expected)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
        assert!(read_link_xattr(&selected, b"user.test").is_err());
        assert!(read_link_xattr(&old, b"user.test").is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn delete_link_never_deletes_its_target() {
        let root = test_root();
        let original = root.join("real-file");
        fs::write(&original, b"keep").unwrap();
        perform(&Action::Link {
            sources: vec![original.clone()],
            destination: root.clone(),
        })
        .unwrap();
        let link = root.join("real-file (copy)");
        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        perform(&Action::Delete { paths: vec![link] }).unwrap();
        assert_eq!(fs::read(original).unwrap(), b"keep");
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn link_batch_reports_each_created_target_and_never_overwrites() {
        let root = test_root();
        let source_a = root.join("a").join("same.txt");
        let source_b = root.join("b").join("same.txt");
        let output = root.join("out");
        fs::create_dir_all(source_a.parent().unwrap()).unwrap();
        fs::create_dir_all(source_b.parent().unwrap()).unwrap();
        fs::create_dir(&output).unwrap();
        fs::write(&source_a, b"first").unwrap();
        fs::write(&source_b, b"second").unwrap();
        fs::write(output.join("same.txt"), b"existing").unwrap();
        let report = perform_with_report(
            &Action::Link {
                sources: vec![source_a.clone(), source_b.clone()],
                destination: output.clone(),
            },
            &AtomicBool::new(false),
            &mut |_| {},
        );
        assert!(report.result.is_ok());
        assert_eq!(report.items.len(), 2);
        assert_eq!(report.items[0].state, ItemState::Completed);
        assert_eq!(report.items[1].state, ItemState::Completed);
        assert_eq!(report.items[0].target, Some(output.join("same (copy).txt")));
        assert_eq!(
            report.items[1].target,
            Some(output.join("same (copy 2).txt"))
        );
        assert_eq!(fs::read(output.join("same.txt")).unwrap(), b"existing");
        assert_eq!(fs::read(output.join("same (copy).txt")).unwrap(), b"first");
        assert_eq!(
            fs::read(output.join("same (copy 2).txt")).unwrap(),
            b"second"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn link_preflight_rejects_bad_later_source_without_creating_earlier_link() {
        let root = test_root();
        let first = root.join("first");
        let output = root.join("out");
        fs::create_dir(&output).unwrap();
        fs::write(&first, b"keep").unwrap();
        let report = perform_with_report(
            &Action::Link {
                sources: vec![first.clone(), PathBuf::from("/")],
                destination: output.clone(),
            },
            &AtomicBool::new(false),
            &mut |_| {},
        );
        assert_eq!(
            report.result.unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(report.items[0].state, ItemState::NotStarted);
        assert_eq!(report.items[1].state, ItemState::Failed);
        assert_eq!(fs::read_dir(&output).unwrap().count(), 0);
        assert_eq!(fs::read(&first).unwrap(), b"keep");
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn link_batch_racing_destination_reports_partial_success_without_overwrite() {
        let root = test_root();
        let first = root.join("first");
        let second = root.join("second");
        let output = root.join("out");
        fs::create_dir(&output).unwrap();
        fs::write(&first, b"first").unwrap();
        fs::write(&second, b"second").unwrap();
        let mut raced = false;
        let report = perform_with_report(
            &Action::Link {
                sources: vec![first.clone(), second.clone()],
                destination: output.clone(),
            },
            &AtomicBool::new(false),
            &mut |progress| {
                if progress.phase == "Linking" && progress.transferred == 1 && !raced {
                    fs::write(output.join("second"), b"competing writer").unwrap();
                    raced = true;
                }
            },
        );
        assert!(raced);
        assert_eq!(
            report.result.unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(report.items[0].state, ItemState::Completed);
        assert_eq!(report.items[0].target, Some(output.join("first")));
        assert_eq!(report.items[1].state, ItemState::Failed);
        assert_eq!(report.items[1].target, Some(output.join("second")));
        assert_eq!(fs::read(output.join("first")).unwrap(), b"first");
        assert_eq!(
            fs::read(output.join("second")).unwrap(),
            b"competing writer"
        );
        assert_eq!(fs::read(second).unwrap(), b"second");
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn cancelling_link_batch_preserves_unstarted_items() {
        let root = test_root();
        let first = root.join("first");
        let second = root.join("second");
        let output = root.join("out");
        fs::create_dir(&output).unwrap();
        fs::write(&first, b"first").unwrap();
        fs::write(&second, b"second").unwrap();
        let cancelled = AtomicBool::new(false);
        let report = perform_with_report(
            &Action::Link {
                sources: vec![first.clone(), second.clone()],
                destination: output.clone(),
            },
            &cancelled,
            &mut |progress| {
                if progress.phase == "Linking" && progress.transferred == 1 {
                    cancelled.store(true, Ordering::Relaxed);
                }
            },
        );
        assert_eq!(
            report.result.unwrap_err().kind(),
            io::ErrorKind::Interrupted
        );
        assert_eq!(report.items[0].state, ItemState::Completed);
        assert_eq!(report.items[1].state, ItemState::Cancelled);
        assert!(output.join("first").exists());
        assert!(!output.join("second").exists());
        assert_eq!(fs::read(second).unwrap(), b"second");
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn cross_device_move_preserves_contents_when_available() {
        use std::os::unix::fs::MetadataExt;
        let root = test_root();
        let shared = Path::new("/dev/shm");
        if !shared.is_dir()
            || fs::metadata(&root).unwrap().dev() == fs::metadata(shared).unwrap().dev()
        {
            fs::remove_dir_all(root).unwrap();
            return;
        }
        let target = shared.join(format!("ricefield-test-{}", std::process::id()));
        if fs::create_dir(&target).is_err() {
            fs::remove_dir_all(root).unwrap();
            return;
        }
        let source = root.join("move.txt");
        fs::write(&source, b"cross-device").unwrap();
        let cancelled = AtomicBool::new(false);
        let summary = perform_with_progress_in(
            &Action::Move {
                sources: vec![source.clone()],
                destination: target.clone(),
                conflict: ConflictPolicy::KeepBoth,
            },
            &cancelled,
            &mut |_| {},
            Some(&root.join("test-journals")),
        )
        .unwrap();
        assert!(
            summary.contains("originals kept for manual cleanup"),
            "{summary}"
        );
        assert_eq!(fs::read(&source).unwrap(), b"cross-device");
        assert_eq!(fs::read(target.join("move.txt")).unwrap(), b"cross-device");
        assert_eq!(fs::read_dir(&target).unwrap().count(), 1);
        assert!(!root.join("test-journals").exists());
        fs::remove_dir_all(&target).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn cross_device_move_reports_copied_source_retained() {
        use std::os::unix::fs::MetadataExt;

        let root = test_root();
        let shared = Path::new("/dev/shm");
        if !shared.is_dir()
            || fs::metadata(&root).unwrap().dev() == fs::metadata(shared).unwrap().dev()
        {
            fs::remove_dir_all(root).unwrap();
            return;
        }
        let destination = shared.join(format!(
            "ricefield-retained-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        if fs::create_dir(&destination).is_err() {
            fs::remove_dir_all(root).unwrap();
            return;
        }
        let source = root.join("keep.txt");
        fs::write(&source, b"leave source intact").unwrap();

        let report = perform_with_report(
            &Action::Move {
                sources: vec![source.clone()],
                destination: destination.clone(),
                conflict: ConflictPolicy::KeepBoth,
            },
            &AtomicBool::new(false),
            &mut |_| {},
        );
        assert!(report.result.unwrap().contains("originals kept"));
        assert_eq!(report.items[0].state, ItemState::CopiedSourceRetained);
        assert!(
            report.items[0]
                .detail
                .as_deref()
                .unwrap()
                .contains("remove the original manually")
        );
        assert_eq!(fs::read(source).unwrap(), b"leave source intact");
        assert_eq!(
            fs::read(destination.join("keep.txt")).unwrap(),
            b"leave source intact"
        );
        fs::remove_dir_all(destination).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn cross_device_move_leaves_external_hardlinks_and_source_untouched() {
        use std::os::unix::fs::MetadataExt;

        let root = test_root();
        let shared = Path::new("/dev/shm");
        if !shared.is_dir()
            || fs::metadata(&root).unwrap().dev() == fs::metadata(shared).unwrap().dev()
        {
            fs::remove_dir_all(root).unwrap();
            return;
        }
        let destination = shared.join(format!(
            "ricefield-hardlink-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        if fs::create_dir(&destination).is_err() {
            fs::remove_dir_all(root).unwrap();
            return;
        }

        let source = root.join("original.txt");
        let outside = root.join("outside.txt");
        fs::write(&source, b"shared bytes").unwrap();
        fs::hard_link(&source, &outside).unwrap();
        let journal_directory = root.join("test-journals");
        let error = perform_with_progress_in(
            &Action::Move {
                sources: vec![source.clone()],
                destination: destination.clone(),
                conflict: ConflictPolicy::KeepBoth,
            },
            &AtomicBool::new(false),
            &mut |_| {},
            Some(&journal_directory),
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Unsupported);
        assert_eq!(fs::read(source).unwrap(), b"shared bytes");
        assert_eq!(fs::read(outside).unwrap(), b"shared bytes");
        assert_eq!(fs::read_dir(&destination).unwrap().count(), 0);
        assert!(!journal_directory.exists());
        fs::remove_dir_all(destination).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn cross_device_move_keeps_new_source_files_created_after_publication() {
        use std::os::unix::fs::MetadataExt;
        let root = test_root();
        let shared = Path::new("/dev/shm");
        if !shared.is_dir()
            || fs::metadata(&root).unwrap().dev() == fs::metadata(shared).unwrap().dev()
        {
            fs::remove_dir_all(root).unwrap();
            return;
        }
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let target = shared.join(format!("ricefield-race-{}-{nonce}", std::process::id()));
        if fs::create_dir(&target).is_err() {
            fs::remove_dir_all(root).unwrap();
            return;
        }
        let source = root.join("source-dir");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("original.txt"), b"before").unwrap();
        let cancelled = AtomicBool::new(false);
        let mut injected = false;
        let result = perform_with_progress_in(
            &Action::Move {
                sources: vec![source.clone()],
                destination: target.clone(),
                conflict: ConflictPolicy::KeepBoth,
            },
            &cancelled,
            &mut |progress: Progress| {
                if progress.phase == "Publishing" && !injected {
                    fs::write(source.join("newly-created.txt"), b"after").unwrap();
                    injected = true;
                }
            },
            Some(&root.join("test-journals")),
        );
        assert!(injected, "test did not reach cross-device publication");
        assert!(
            result
                .unwrap()
                .contains("originals kept for manual cleanup")
        );
        assert_eq!(fs::read(source.join("original.txt")).unwrap(), b"before");
        assert_eq!(
            fs::read(source.join("newly-created.txt")).unwrap(),
            b"after"
        );
        assert_eq!(
            fs::read(target.join("source-dir/original.txt")).unwrap(),
            b"before"
        );
        assert!(!target.join("source-dir/newly-created.txt").exists());
        assert!(!root.join("test-journals").exists());
        fs::remove_dir_all(target).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn verification_detects_equal_size_corruption() {
        let root = test_root();
        let original = root.join("source");
        let copy = root.join("copy");
        fs::write(&original, b"ABCDEF").unwrap();
        fs::write(&copy, b"ABCDEX").unwrap();
        let cancelled = AtomicBool::new(false);
        let mut callback = |_progress: Progress| {};
        let mut transfer = Transfer {
            cancelled: &cancelled,
            notify: &mut callback,
            transferred: 0,
            total: 6,
            last_report: Instant::now(),
            last_reported_bytes: 0,
            phase: "Verifying",
        };
        assert_eq!(
            verify_tree(&original, &copy, 0, &mut transfer)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(fs::read(&original).unwrap(), b"ABCDEF");
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn move_verification_rejects_lost_permissions_and_keeps_source() {
        use std::os::unix::fs::PermissionsExt;

        let root = test_root();
        let source = root.join("source.txt");
        let destination = root.join("destination.txt");
        fs::write(&source, b"same bytes").unwrap();
        fs::write(&destination, b"same bytes").unwrap();
        fs::set_permissions(&source, fs::Permissions::from_mode(0o640)).unwrap();
        fs::set_permissions(&destination, fs::Permissions::from_mode(0o600)).unwrap();
        let cancelled = AtomicBool::new(false);
        let mut callback = |_: Progress| {};
        let mut transfer = Transfer {
            cancelled: &cancelled,
            notify: &mut callback,
            transferred: 0,
            total: 10,
            last_report: Instant::now(),
            last_reported_bytes: 0,
            phase: "Verifying",
        };
        let error = verify_tree(&source, &destination, 0, &mut transfer).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("permissions"));
        assert_eq!(fs::read(&source).unwrap(), b"same bytes");
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn staged_move_rejects_metadata_loss_before_publication() {
        use std::os::unix::fs::PermissionsExt;

        let root = test_root();
        let source = root.join("source.txt");
        let destination_dir = root.join("destination-dir");
        let destination = destination_dir.join("source.txt");
        fs::create_dir(&destination_dir).unwrap();
        fs::write(&source, b"important bytes").unwrap();
        fs::set_permissions(&source, fs::Permissions::from_mode(0o640)).unwrap();
        let cancelled = AtomicBool::new(false);
        let mut injected = false;
        let mut callback = |progress: Progress| {
            if progress.phase == "Verifying" && !injected {
                let staging = fs::read_dir(&destination_dir)
                    .unwrap()
                    .map(|entry| entry.unwrap().path())
                    .find(|path| {
                        path.file_name()
                            .unwrap()
                            .to_string_lossy()
                            .starts_with(".ricefield-staging-")
                    })
                    .unwrap();
                fs::set_permissions(staging.join("payload"), fs::Permissions::from_mode(0o600))
                    .unwrap();
                injected = true;
            }
        };
        let mut transfer = Transfer {
            cancelled: &cancelled,
            notify: &mut callback,
            transferred: 0,
            total: 15,
            last_report: Instant::now(),
            last_reported_bytes: 0,
            phase: "Copying",
        };
        let error = stage_and_publish(&source, &destination, true, &mut transfer).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(injected, "test never modified staged metadata");
        assert_eq!(fs::read(&source).unwrap(), b"important bytes");
        assert!(!destination.exists());
        assert_eq!(fs::read_dir(&destination_dir).unwrap().count(), 0);
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn move_verification_rejects_lost_xattrs_and_keeps_source() {
        let root = test_root();
        let source = root.join("source.txt");
        let destination = root.join("destination.txt");
        fs::write(&source, b"same bytes").unwrap();
        fs::write(&destination, b"same bytes").unwrap();
        // Metadata parity is checked before xattrs. Ensure the fixture has
        // identical mtimes so this test reaches the attribute comparison.
        File::open(&destination)
            .unwrap()
            .set_times(source_times(&fs::symlink_metadata(&source).unwrap()))
            .unwrap();
        let path = CString::new(source.as_os_str().as_bytes()).unwrap();
        let key = CString::new("user.ricefield_verification").unwrap();
        let value = b"must survive";
        let status = unsafe {
            libc::lsetxattr(
                path.as_ptr(),
                key.as_ptr(),
                value.as_ptr().cast(),
                value.len(),
                0,
            )
        };
        if status != 0 {
            let error = io::Error::last_os_error();
            if matches!(
                error.raw_os_error(),
                Some(libc::EOPNOTSUPP) | Some(libc::EPERM)
            ) {
                fs::remove_dir_all(root).unwrap();
                return;
            }
            panic!("Cannot prepare xattr fixture: {error}");
        }
        let cancelled = AtomicBool::new(false);
        let mut callback = |_: Progress| {};
        let mut transfer = Transfer {
            cancelled: &cancelled,
            notify: &mut callback,
            transferred: 0,
            total: 10,
            last_report: Instant::now(),
            last_reported_bytes: 0,
            phase: "Verifying",
        };
        let error = verify_tree(&source, &destination, 0, &mut transfer).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("extended-attribute"), "{error}");
        assert_eq!(fs::read(&source).unwrap(), b"same bytes");
        fs::remove_dir_all(root).unwrap();
    }
}
