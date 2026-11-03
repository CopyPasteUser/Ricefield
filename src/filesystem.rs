use std::{
    ffi::OsString,
    fs::{self, File, OpenOptions},
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
}

struct Transfer<'a> {
    cancelled: &'a AtomicBool,
    notify: &'a mut dyn FnMut(Progress),
    transferred: u64,
    total: u64,
    last_report: Instant,
    last_reported_bytes: u64,
}

impl Transfer<'_> {
    fn check(&self) -> io::Result<()> {
        if self.cancelled.load(Ordering::Relaxed) {
            Err(io::Error::new(io::ErrorKind::Interrupted, "Operation cancelled"))
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
            });
            self.last_report = Instant::now();
            self.last_reported_bytes = self.transferred;
        }
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
    let sources: &[PathBuf] = match action {
        Action::Copy { sources, .. } | Action::Move { sources, .. } => sources,
        Action::Trash { paths } | Action::Delete { paths } => paths,
        _ => &[],
    };
    let estimated: Vec<u64> = if matches!(action, Action::Copy { .. } | Action::Move { .. }) {
        sources
            .iter()
            .map(|path| estimate_bytes(path, cancelled, 0))
            .collect::<io::Result<_>>()?
    } else {
        Vec::new()
    };
    let total = estimated.iter().fold(0u64, |sum, bytes| sum.saturating_add(*bytes));
    let mut transfer = Transfer {
        cancelled,
        notify,
        transferred: 0,
        total,
        last_report: Instant::now(),
        last_reported_bytes: 0,
    };
    (transfer.notify)(Progress {
        transferred: 0,
        total,
        current: sources.first().cloned().unwrap_or_default(),
    });
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
            for source in sources {
                transfer.check()?;
                let target =
                    free_destination(destination, source.file_name().ok_or_else(invalid_name)?)?;
                copy_tree(source, &target, 0, &mut transfer)?;
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
            for (source, bytes) in sources.iter().zip(estimated.iter()) {
                transfer.check()?;
                if source.parent() == Some(destination.as_path()) {
                    transfer.advance(*bytes, source);
                    continue;
                }
                let target =
                    free_destination(destination, source.file_name().ok_or_else(invalid_name)?)?;
                ensure_outside_source(source, &target)?;
                match rename_without_overwrite(source, &target) {
                    Ok(()) => transfer.advance(*bytes, &target),
                    Err(error) if error.raw_os_error() == Some(libc::EXDEV) => {
                        copy_tree(source, &target, 0, &mut transfer)?;
                        if let Err(error) = transfer.check() {
                            remove_created(&target)?;
                            return Err(error);
                        }
                        // If deletion fails the copy is deliberately kept for recovery.
                        remove_source(source)?;
                    }
                    Err(error) => return Err(error),
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
        Action::Link { sources, destination } => {
            for source in sources {
                transfer.check()?;
                let target = free_destination(destination, source.file_name().ok_or_else(invalid_name)?)?;
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
                    return Err(io::Error::new(io::ErrorKind::Unsupported, "Unix links only"));
                }
            }
            Ok(format!("Created {} link(s)", sources.len()))
        }
        Action::SetMode { path, mode } => {
            transfer.check()?;
            if *mode > 0o777 || fs::symlink_metadata(path)?.file_type().is_symlink() {
                return Err(io::Error::new(io::ErrorKind::InvalidInput, "Invalid permissions or symlink"));
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(path, fs::Permissions::from_mode(*mode))?;
                Ok(format!("Updated permissions to {mode:03o}"))
            }
            #[cfg(not(unix))]
            {
                Err(io::Error::new(io::ErrorKind::Unsupported, "Unix permissions only"))
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
        ("Size".into(), format!("{} ({} bytes)", human_size(metadata.len()), metadata.len())),
        ("Modified".into(), age(metadata.modified().ok())),
        ("Read-only".into(), metadata.permissions().readonly().to_string()),
    ];
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        rows.push(("Permissions".into(), format!("{:04o}", metadata.permissions().mode() & 0o7777)));
        rows.push(("Owner UID".into(), metadata.uid().to_string()));
        rows.push(("Group GID".into(), metadata.gid().to_string()));
    }
    if metadata.file_type().is_symlink() {
        rows.push(("Link target".into(), fs::read_link(path)?.display().to_string()));
    }
    Ok(rows)
}

/// Mount points useful to a desktop user, without presenting /proc and other pseudo-filesystems.
pub fn mounted_volumes() -> Vec<PathBuf> {
    let mut paths = std::collections::BTreeSet::new();
    paths.insert(PathBuf::from("/"));
    #[cfg(target_os = "linux")]
    if let Ok(contents) = fs::read_to_string("/proc/self/mountinfo") {
        for line in contents.lines() {
            let mut parts = line.split(" - ");
            let before = parts.next().unwrap_or("");
            let after = parts.next().unwrap_or("");
            let mount = before.split_whitespace().nth(4).unwrap_or("");
            let fs_type = after.split_whitespace().next().unwrap_or("");
            let desktop_path = mount.starts_with("/media/")
                || mount.starts_with("/run/media/")
                || mount.starts_with("/mnt/");
            let remote = matches!(fs_type, "nfs" | "nfs4" | "cifs" | "fuse.sshfs" | "fuse.rclone");
            if (desktop_path || remote) && !mount.is_empty() {
                let path = PathBuf::from(mount.replace("\\040", " ").replace("\\134", "\\"));
                if path.is_dir() {
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
    let stem = name.file_stem().unwrap_or(original).to_string_lossy();
    let extension = name.extension().map(|value| value.to_string_lossy());
    for index in 1..10_000 {
        let suffix = if index == 1 {
            " (copy)".to_owned()
        } else {
            format!(" (copy {index})")
        };
        let mut candidate = OsString::from(format!("{stem}{suffix}"));
        if let Some(extension) = &extension {
            candidate.push(".");
            candidate.push(extension.as_ref());
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
        return Err(io::Error::new(io::ErrorKind::Interrupted, "Operation cancelled"));
    }
    if depth > 128 {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "Folder nesting is too deep"));
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

fn remove_source(path: &Path) -> io::Result<()> {
    remove_created(path)
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
            std::os::unix::fs::symlink(fs::read_link(source)?, destination)?;
            transfer.advance(metadata.len(), source);
            return Ok(());
        }
        #[cfg(not(unix))]
        return Err(io::Error::new(io::ErrorKind::Unsupported, "Symlink copy unavailable"));
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
            fs::set_permissions(destination, metadata.permissions())?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_dir_all(destination);
        }
        result
    } else if metadata.is_file() {
        let mut input = File::open(source)?;
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
            output.sync_all()?;
            fs::set_permissions(destination, metadata.permissions())?;
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
        }).unwrap();
        assert_eq!(fs::read_link(output.join("broken")).unwrap(), PathBuf::from("../missing-target"));
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
            &Action::Copy { sources: vec![original.clone()], destination: dest.clone() },
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
        perform(&Action::SetMode { path: item.clone(), mode: 0o640 }).unwrap();
        assert_eq!(fs::metadata(&item).unwrap().permissions().mode() & 0o777, 0o640);
        let link = root.join("link");
        symlink(&item, &link).unwrap();
        assert!(perform(&Action::SetMode { path: link, mode: 0o777 }).is_err());
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
        }).unwrap();
        let link = root.join("real-file (copy)");
        assert!(fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
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
        if !shared.is_dir() || fs::metadata(&root).unwrap().dev() == fs::metadata(shared).unwrap().dev() {
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
        perform(&Action::Move { sources: vec![source.clone()], destination: target.clone() }).unwrap();
        assert!(!source.exists());
        assert_eq!(fs::read(target.join("move.txt")).unwrap(), b"cross-device");
        fs::remove_dir_all(&target).unwrap();
        fs::remove_dir_all(root).unwrap();
    }
}
