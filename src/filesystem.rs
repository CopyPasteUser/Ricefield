use std::{
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io,
    path::{Path, PathBuf},
    time::SystemTime,
};

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

pub fn perform(action: &Action) -> io::Result<String> {
    match action {
        Action::NewFolder { parent, name } => {
            if !valid_name(name) {
                return Err(invalid_name());
            }
            fs::create_dir(parent.join(name))?;
            Ok(format!("Created folder {name}"))
        }
        Action::NewFile { parent, name } => {
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
                let target =
                    free_destination(destination, source.file_name().ok_or_else(invalid_name)?)?;
                copy_tree(source, &target, 0)?;
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
            for source in sources {
                if source.parent() == Some(destination.as_path()) {
                    continue;
                }
                let target =
                    free_destination(destination, source.file_name().ok_or_else(invalid_name)?)?;
                if source.is_dir() && target.starts_with(source) {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "Cannot move a folder into itself",
                    ));
                }
                // Deliberately do not emulate cross-volume moves with copy+delete.
                // A failed rename leaves the original intact.
                rename_without_overwrite(source, &target)?;
            }
            Ok(format!("Moved {} item(s)", sources.len()))
        }
        Action::Trash { paths } => {
            for path in paths {
                trash::delete(path).map_err(io::Error::other)?;
            }
            Ok(format!("Moved {} item(s) to Trash", paths.len()))
        }
    }
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

fn copy_tree(source: &Path, destination: &Path, depth: usize) -> io::Result<()> {
    if depth > 128 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Folder nesting is too deep",
        ));
    }
    let metadata = fs::symlink_metadata(source)?;
    if metadata.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Copying symlinks is not supported yet",
        ));
    }
    if metadata.is_dir() {
        let canonical = fs::canonicalize(source)?;
        let canonical_parent = fs::canonicalize(destination.parent().ok_or_else(invalid_name)?)?;
        if canonical_parent.starts_with(&canonical) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Cannot copy a folder inside itself",
            ));
        }
        fs::create_dir(destination)?;
        let result = (|| {
            for item in fs::read_dir(source)? {
                let item = item?;
                copy_tree(&item.path(), &destination.join(item.file_name()), depth + 1)?;
            }
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
        let result = io::copy(&mut input, &mut output);
        if result.is_err() {
            let _ = fs::remove_file(destination);
        }
        result.map(|_| ())
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
}
