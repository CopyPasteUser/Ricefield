use crate::journal::{Journal, State};
use std::{
    ffi::OsString,
    fs::{self, File, FileTimes, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant, SystemTime},
};

/// A snapshot of a file operation; callbacks run on the worker thread.
#[derive(Clone, Debug)]
pub struct Progress {
    pub transferred: u64,
    pub total: u64,
    pub current: PathBuf,
    pub phase: &'static str,
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
            match self
                .path
                .extension()
                .and_then(|extension| extension.to_str())
            {
                Some("png" | "jpg" | "jpeg" | "gif" | "webp" | "svg") => "Image",
                Some("mp3" | "flac" | "wav" | "ogg" | "m4a") => "Audio",
                Some("mkv" | "mp4" | "avi" | "webm" | "mov") => "Video",
                Some("zip" | "7z" | "tar" | "gz" | "rar") => "Archive",
                Some("rs" | "py" | "js" | "ts" | "html" | "css" | "sh") => "Code",
                Some("pdf") => "PDF",
                Some("txt" | "md") => "Document",
                _ => "File",
            }
        }
    }
}

pub fn scan(path: &Path) -> io::Result<Vec<Entry>> {
    let mut entries = Vec::new();
    for result in fs::read_dir(path)? {
        let item = result?;
        // Metadata is gathered on a worker thread: large/slow folders never freeze the UI.
        let metadata = match fs::symlink_metadata(item.path()) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        entries.push(Entry {
            name: item.file_name().to_string_lossy().into_owned(),
            path: item.path(),
            is_dir: metadata.is_dir(),
            is_symlink: metadata.file_type().is_symlink(),
            size: metadata.len(),
            modified: metadata.modified().ok(),
        });
    }
    Ok(entries)
}

/// Bounded recursive filename search. Symlink directories are never followed.
pub fn search(root: &Path, query: &str, show_hidden: bool) -> io::Result<Vec<Entry>> {
    let mut results = Vec::new();
    let mut pending = std::collections::VecDeque::from([(root.to_path_buf(), 0usize)]);
    let needle = query.to_lowercase();
    let mut visited = 0usize;
    while let Some((directory, depth)) = pending.pop_front() {
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(_) if directory.as_path() != root => {
                // An inaccessible child must not hide all the accessible results.
                continue;
            }
            Err(error) => return Err(error),
        };
        for entry in entries {
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
            visited += 1;
            if visited >= 250_000 || results.len() >= 20_000 {
                return Ok(results);
            }
            if metadata.is_dir() && depth < 48 {
                pending.push_back((entry.path(), depth + 1));
            }
            if name.to_lowercase().contains(&needle) {
                results.push(Entry {
                    name,
                    path: entry.path(),
                    is_dir: metadata.is_dir(),
                    is_symlink: metadata.file_type().is_symlink(),
                    size: metadata.len(),
                    modified: metadata.modified().ok(),
                });
            }
        }
    }
    Ok(results)
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
    Copy {
        sources: Vec<PathBuf>,
        destination: PathBuf,
    },
    Move {
        sources: Vec<PathBuf>,
        destination: PathBuf,
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
    },
}

pub fn valid_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains('/') && !name.contains('\0')
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
    io::Error::new(
        error.kind(),
        format!(
            "{verb} stopped at item {} of {total} ({}): {error}. {} earlier item(s) completed; inspect the current item and both locations before retrying",
            index + 1,
            path.display(),
            index
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
fn rename_without_overwrite(source: &Path, destination: &Path) -> io::Result<()> {
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

#[cfg(not(target_os = "linux"))]
fn rename_without_overwrite(source: &Path, destination: &Path) -> io::Result<()> {
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
pub fn perform_with_progress(
    action: &Action,
    cancelled: &AtomicBool,
    notify: &mut dyn FnMut(Progress),
) -> io::Result<String> {
    perform_with_progress_in(action, cancelled, notify, None)
}

/// Supplying a journal directory lets failure-injection tests keep their
/// deliberately incomplete records inside disposable fixtures.
fn perform_with_progress_in(
    action: &Action,
    cancelled: &AtomicBool,
    notify: &mut dyn FnMut(Progress),
    journal_directory: Option<&Path>,
) -> io::Result<String> {
    let sources: &[PathBuf] = match action {
        Action::Copy { sources, .. } | Action::Move { sources, .. } => sources,
        Action::Trash { paths } | Action::Delete { paths } => paths,
        _ => &[],
    };
    let estimated: Vec<u64> =
        match action {
            Action::Copy { .. } => sources
                .iter()
                .map(|path| estimate_bytes(path, cancelled, 0))
                .collect::<io::Result<_>>()?,
            Action::Move { .. } => sources
                .iter()
                .map(|path| {
                    fs::symlink_metadata(path).map(|metadata| {
                        if metadata.is_dir() { 0 } else { metadata.len() }
                    })
                })
                .collect::<io::Result<_>>()?,
            _ => Vec::new(),
        };
    let total = estimated
        .iter()
        .fold(0u64, |sum, bytes| sum.saturating_add(*bytes));
    let mut transfer = Transfer {
        cancelled,
        notify,
        transferred: 0,
        total,
        last_report: Instant::now(),
        last_reported_bytes: 0,
        phase: "Copying",
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
        Action::Copy {
            sources,
            destination,
        } => {
            if sources.is_empty() {
                return Ok("Nothing to copy".into());
            }
            for (index, source) in sources.iter().enumerate() {
                let result = (|| {
                    transfer.check()?;
                    let target = free_destination(
                        destination,
                        source.file_name().ok_or_else(invalid_name)?,
                    )?;
                    stage_and_publish(source, &target, false, &mut transfer)
                })();
                if let Err(error) = result {
                    return Err(batch_failure("Copy", index, sources.len(), source, error));
                }
            }
            Ok(format!("Copied {} item(s)", sources.len()))
        }
        Action::Move {
            sources,
            destination,
        } => {
            if sources.is_empty() {
                return Ok("Nothing to move".into());
            }
            for (index, (source, bytes)) in sources.iter().zip(estimated.iter()).enumerate() {
                let result = (|| {
                    transfer.check()?;
                    if source.parent() == Some(destination.as_path()) {
                        transfer.advance(*bytes, source);
                        return Ok(());
                    }
                    let target = free_destination(
                        destination,
                        source.file_name().ok_or_else(invalid_name)?,
                    )?;
                    ensure_outside_source(source, &target)?;
                    match rename_without_overwrite(source, &target) {
                        Ok(()) => transfer.advance(*bytes, &target),
                        Err(error) if error.raw_os_error() == Some(libc::EXDEV) => {
                            // Only traverse a directory when the atomic rename is impossible.
                            let actual_bytes = estimate_bytes(source, cancelled, 0)?;
                            transfer.total = transfer
                                .total
                                .saturating_add(actual_bytes.saturating_sub(*bytes));
                            transfer.report(source);
                            // A durable record must exist before copying. Never delete
                            // the source if publishing the journal state fails.
                            let original_source = fs::symlink_metadata(source)?;
                            let mut journal = match journal_directory {
                                Some(directory) => Journal::begin_in(directory, source, &target)?,
                                None => Journal::begin(source, &target)?,
                            };
                            stage_and_publish_with_journal(
                                source,
                                &target,
                                true,
                                &mut transfer,
                                Some(&mut journal),
                            )?;
                            transfer.check()?;
                            transfer.phase = "Finishing";
                            transfer.report(source);
                            // Another process may have modified the source while it was
                            // copied. Recheck after publication before unlinking anything.
                            // If deletion fails, the published destination remains for
                            // recovery; no unverified new directory entries are removed.
                            ensure_same_source(source, &original_source)?;
                            verify_tree(source, &target, 0, &mut transfer)?;
                            ensure_same_source(source, &original_source)?;
                            remove_verified_tree(source, &target, 0, &mut transfer)?;
                            journal.advance(State::Completed)?;
                            transfer.phase = "Copying";
                        }
                        Err(error) => return Err(error),
                    }
                    Ok(())
                })();
                if let Err(error) = result {
                    return Err(batch_failure("Move", index, sources.len(), source, error));
                }
            }
            Ok(format!("Moved {} item(s)", sources.len()))
        }
        Action::Trash { paths } => {
            for path in paths {
                transfer.check()?;
                trash::delete(path).map_err(io::Error::other)?;
            }
            Ok(format!("Moved {} item(s) to Trash", paths.len()))
        }
        Action::Delete { paths } => {
            for path in paths {
                transfer.check()?;
                remove_created(path)?;
            }
            Ok(format!("Permanently deleted {} item(s)", paths.len()))
        }
        Action::Link {
            sources,
            destination,
        } => {
            for source in sources {
                transfer.check()?;
                let target =
                    free_destination(destination, source.file_name().ok_or_else(invalid_name)?)?;
                #[cfg(unix)]
                {
                    let absolute = if source.is_absolute() {
                        source.clone()
                    } else {
                        std::env::current_dir()?.join(source)
                    };
                    std::os::unix::fs::symlink(absolute, target)?;
                }
                #[cfg(not(unix))]
                {
                    let _ = target;
                    return Err(io::Error::new(
                        io::ErrorKind::Unsupported,
                        "Unix links only",
                    ));
                }
            }
            Ok(format!("Created {} link(s)", sources.len()))
        }
        Action::SetMode { path, mode } => {
            transfer.check()?;
            if *mode > 0o777 || fs::symlink_metadata(path)?.file_type().is_symlink() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Invalid permissions or symlink",
                ));
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(path, fs::Permissions::from_mode(*mode))?;
                Ok(format!("Updated permissions to {mode:03o}"))
            }
            #[cfg(not(unix))]
            {
                Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "Unix permissions only",
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

/// Mount points useful to a desktop user, without presenting /proc and other pseudo-filesystems.
pub fn mounted_volumes() -> Vec<PathBuf> {
    let mut paths = std::collections::BTreeSet::new();
    paths.insert(PathBuf::from("/"));
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
            let desktop_path = mount.starts_with(b"/media/")
                || mount.starts_with(b"/run/media/")
                || mount.starts_with(b"/mnt/");
            let remote = matches!(
                fs_type,
                "nfs" | "nfs4" | "cifs" | "fuse.sshfs" | "fuse.rclone" | "fuse.gvfsd-fuse"
            );
            if (desktop_path || remote) && !mount.is_empty() {
                let Some(path) = decode_mountinfo_path(mount) else {
                    continue;
                };
                if path.is_dir() {
                    if fs_type == "fuse.gvfsd-fuse"
                        && let Ok(entries) = fs::read_dir(&path)
                    {
                        for entry in entries.flatten() {
                            if entry.path().is_dir() {
                                paths.insert(entry.path());
                            }
                        }
                    }
                    paths.insert(path);
                }
            }
        }
    }
    paths.into_iter().collect()
}

fn free_destination(parent: &Path, original: &std::ffi::OsStr) -> io::Result<PathBuf> {
    let first = parent.join(original);
    if !occupied(&first)? {
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
        if !occupied(&candidate)? {
            return Ok(candidate);
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "Too many duplicate names",
    ))
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

/// Delete only source entries that have a matching published destination.
/// Unlike remove_dir_all, a concurrent writer adding an unexpected entry
/// prevents the final rmdir and leaves that entry available for recovery.
fn remove_verified_tree(
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
    let metadata = fs::symlink_metadata(source)?;
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
                transfer,
            )?;
        }
        fs::remove_dir(source)
    } else {
        // Reverify each file immediately before unlinking it. The final
        // source path still may be mutated by another process; crash-safe
        // identity tracking and journaling remain required for a 1.0 gate.
        verify_tree(source, destination, depth, transfer)?;
        fs::remove_file(source)
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
            ".rice-explorer-staging-{}-{nonce}-{attempt}",
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
/// leave an abandoned staging directory; a durable recovery journal and directory
/// fsync are still needed for power-loss guarantees.
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
    mut journal: Option<&mut Journal>,
) -> io::Result<()> {
    transfer.check()?;
    ensure_outside_source(source, destination)?;
    let staging_dir = create_staging_dir(destination.parent().ok_or_else(invalid_name)?)?;
    if let Some(record) = journal.as_mut()
        && let Err(error) = record.set_staging(&staging_dir)
    {
        if let Err(cleanup_error) = fs::remove_dir(&staging_dir) {
            eprintln!("RiceExplorer: cannot clean staging directory: {cleanup_error}");
        }
        return Err(error);
    }
    let staged = staging_dir.join("payload");
    let result = (|| {
        let original = fs::symlink_metadata(source)?;
        transfer.phase = "Copying";
        copy_tree(source, &staged, 0, transfer)?;
        if verify {
            transfer.phase = "Verifying";
            transfer.report(source);
            verify_tree(source, &staged, 0, transfer)?;
            if let Some(record) = journal.as_mut() {
                record.advance(State::StagedVerified)?;
            }
        }
        ensure_same_source(source, &original)?;
        transfer.check()?;
        transfer.phase = "Publishing";
        transfer.report(destination);
        rename_without_overwrite(&staged, destination)?;
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
                    "RiceExplorer: published copy, but cannot remove staging directory: {error}"
                );
            }
            Ok(())
        }
        Err(error) => {
            if let Err(cleanup_error) = remove_created(&staging_dir) {
                eprintln!("RiceExplorer: cannot clean staging directory: {cleanup_error}");
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
    if source_meta.file_type().is_symlink() {
        if !target_meta.file_type().is_symlink()
            || fs::read_link(source)? != fs::read_link(destination)?
        {
            return Err(mismatch());
        }
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
    } else if source_meta.is_file() {
        if !target_meta.is_file() || source_meta.len() != target_meta.len() {
            return Err(mismatch());
        }
        let mut source_reader = File::open(source)?;
        let mut target_reader = File::open(destination)?;
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
        let after = fs::metadata(source)?;
        if source_meta.len() != after.len() || source_meta.modified().ok() != after.modified().ok()
        {
            return Err(mismatch());
        }
    } else {
        return Err(mismatch());
    }
    Ok(())
}

fn copy_tree(
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
    let metadata = fs::symlink_metadata(source)?;
    if metadata.file_type().is_symlink() {
        #[cfg(unix)]
        {
            let target = fs::read_link(source)?;
            ensure_same_source(source, &metadata)?;
            std::os::unix::fs::symlink(target, destination)?;
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
                )?;
            }
            ensure_same_source(source, &metadata)?;
            let output = File::open(destination)?;
            output.set_permissions(metadata.permissions())?;
            output.set_times(source_times(&metadata))?;
            output.sync_all()?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_dir_all(destination);
        }
        result
    } else if metadata.is_file() {
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            // Refuse a symlink swapped in between lstat and open.
            options.custom_flags(libc::O_NOFOLLOW);
        }
        let mut input = options.open(source)?;
        ensure_same_source_metadata(&metadata, &input.metadata()?)?;
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
            output.set_permissions(metadata.permissions())?;
            output.set_times(source_times(&metadata))?;
            output.sync_all()?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(destination);
        }
        result
    } else {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Special device files cannot be copied",
        ))
    }
}

/// Reject a replaced or modified source before exposing a staged copy.
/// This is a best-effort non-snapshot check, not an atomic lock on other writers.
fn ensure_same_source(path: &Path, initial: &fs::Metadata) -> io::Result<()> {
    ensure_same_source_metadata(initial, &fs::symlink_metadata(path)?)
}

fn ensure_same_source_metadata(initial: &fs::Metadata, current: &fs::Metadata) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if initial.dev() != current.dev()
            || initial.ino() != current.ino()
            || initial.mtime() != current.mtime()
            || initial.mtime_nsec() != current.mtime_nsec()
            || initial.size() != current.size()
            || initial.mode() != current.mode()
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Source changed during copy; destination was not published",
            ));
        }
    }
    #[cfg(not(unix))]
    if initial.len() != current.len() || initial.modified().ok() != current.modified().ok() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Source changed during copy; destination was not published",
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
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("rice-explorer-test-{}-{nonce}", std::process::id()));
        fs::create_dir(&dir).unwrap();
        dir
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
        })
        .unwrap();
        assert_eq!(fs::read(dst.join("note.txt")).unwrap(), b"existing");
        assert_eq!(fs::read(dst.join("note (copy).txt")).unwrap(), b"original");
        assert_eq!(fs::read(source).unwrap(), b"original");
        assert_eq!(fs::read_dir(&dst).unwrap().count(), 2);
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
        use std::os::unix::net::UnixListener;

        let root = test_root();
        let valid = root.join("valid.txt");
        fs::write(&valid, b"already committed").unwrap();
        let socket = root.join("unsupported-socket");
        let _listener = UnixListener::bind(&socket).unwrap();
        let output = root.join("output");
        fs::create_dir(&output).unwrap();

        let error = perform(&Action::Copy {
            sources: vec![valid.clone(), socket.clone()],
            destination: output.clone(),
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
        use std::os::unix::net::UnixListener;

        let root = test_root();
        let socket = root.join("unix-socket");
        let _listener = UnixListener::bind(&socket).unwrap();
        let output = root.join("output");
        fs::create_dir(&output).unwrap();

        let error = perform(&Action::Copy {
            sources: vec![socket.clone()],
            destination: output.clone(),
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
                path: link,
                mode: 0o777
            })
            .is_err()
        );
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
        let target = shared.join(format!("rice-explorer-test-{}", std::process::id()));
        if fs::create_dir(&target).is_err() {
            fs::remove_dir_all(root).unwrap();
            return;
        }
        let source = root.join("move.txt");
        fs::write(&source, b"cross-device").unwrap();
        let cancelled = AtomicBool::new(false);
        perform_with_progress_in(
            &Action::Move {
                sources: vec![source.clone()],
                destination: target.clone(),
            },
            &cancelled,
            &mut |_| {},
            Some(&root.join("test-journals")),
        )
        .unwrap();
        assert!(!source.exists());
        assert_eq!(fs::read(target.join("move.txt")).unwrap(), b"cross-device");
        assert_eq!(fs::read_dir(&target).unwrap().count(), 1);
        fs::remove_dir_all(&target).unwrap();
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
        let target = shared.join(format!("rice-explorer-race-{}-{nonce}", std::process::id()));
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
            },
            &cancelled,
            &mut |progress: Progress| {
                if progress.phase == "Finishing" && !injected {
                    fs::write(source.join("newly-created.txt"), b"after").unwrap();
                    injected = true;
                }
            },
            Some(&root.join("test-journals")),
        );
        assert!(injected, "test did not reach the EXDEV finishing phase");
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::InvalidData);
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
}
