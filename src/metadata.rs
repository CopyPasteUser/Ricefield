//! Read-only, budgeted metadata queries. Deep traversal runs only on request,
//! in a worker: a slow or disconnected mount must never block the UI thread.
use std::{
    collections::VecDeque,
    fs,
    io::{self, ErrorKind},
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

const MAX_VISITED: usize = 100_000;
const MAX_DURATION: Duration = Duration::from_secs(12);
const MAX_HASH_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// Listing attribute *names* never reads secret attribute values. ACLs use
/// the same kernel namespace and are identified by their standard xattr key.
#[cfg(target_os = "linux")]
pub fn xattr_rows(path: &Path) -> Vec<(String, String)> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};
    let Ok(name) = CString::new(path.as_os_str().as_bytes()) else {
        return vec![("Extended attributes".into(), "Invalid pathname".into())];
    };
    let mut buffer = [0u8; 16 * 1024];
    // SAFETY: both arguments are valid pointers for the supplied lengths;
    // llistxattr does not follow a symlink to another file's attributes.
    let count =
        unsafe { libc::llistxattr(name.as_ptr(), buffer.as_mut_ptr().cast(), buffer.len()) };
    if count < 0 {
        let error = io::Error::last_os_error();
        if matches!(error.raw_os_error(), Some(libc::ENOTSUP | libc::ENODATA)) {
            return vec![("Extended attributes".into(), "Not supported".into())];
        }
        return vec![(
            "Extended attributes".into(),
            format!("Unavailable ({error})"),
        )];
    }
    if count as usize > buffer.len() {
        return vec![(
            "Extended attributes".into(),
            "Too many to display safely".into(),
        )];
    }
    let attrs: Vec<_> = buffer[..count as usize]
        .split(|byte| *byte == 0)
        .filter(|part| !part.is_empty())
        .collect();
    let has_acl = attrs
        .iter()
        .any(|part| *part == b"system.posix_acl_access" || *part == b"system.posix_acl_default");
    let mut rows = vec![(
        "Extended attributes".into(),
        format!("{} attribute names (values not read)", attrs.len()),
    )];
    rows.push((
        "POSIX ACL".into(),
        if has_acl {
            "Present"
        } else {
            "No extended ACL detected"
        }
        .into(),
    ));
    if !attrs.is_empty() {
        let names = attrs
            .iter()
            .take(12)
            .map(|part| String::from_utf8_lossy(part).into_owned())
            .collect::<Vec<_>>()
            .join(", ");
        rows.push((
            "Attribute names".into(),
            format!("{names}{}", if attrs.len() > 12 { ", …" } else { "" }),
        ));
    }
    rows
}

#[cfg(not(target_os = "linux"))]
pub fn xattr_rows(_path: &Path) -> Vec<(String, String)> {
    vec![("Extended attributes".into(), "Supported on Linux".into())]
}

fn cancelled() -> io::Error {
    io::Error::new(ErrorKind::Interrupted, "Analysis cancelled")
}

fn folder_size(path: &Path, stop: &AtomicBool) -> io::Result<String> {
    let mut pending = VecDeque::from([PathBuf::from(path)]);
    let mut files = 0usize;
    let mut folders = 0usize;
    let mut total = 0u64;
    let mut visited = 0usize;
    let start = Instant::now();
    let mut limited = false;
    #[cfg(unix)]
    let root_device = {
        use std::os::unix::fs::MetadataExt;
        fs::symlink_metadata(path)?.dev()
    };
    while let Some(folder) = pending.pop_front() {
        if stop.load(Ordering::Relaxed) {
            return Err(cancelled());
        }
        if visited >= MAX_VISITED || start.elapsed() >= MAX_DURATION {
            limited = true;
            break;
        }
        folders += 1;
        for entry in fs::read_dir(&folder)? {
            if stop.load(Ordering::Relaxed) {
                return Err(cancelled());
            }
            if visited >= MAX_VISITED || start.elapsed() >= MAX_DURATION {
                limited = true;
                break;
            }
            visited += 1;
            let entry = entry?;
            let meta = entry.path().symlink_metadata()?;
            if meta.file_type().is_symlink() {
                continue;
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                if meta.dev() != root_device {
                    continue;
                }
            }
            if meta.is_dir() {
                pending.push_back(entry.path());
            } else if meta.is_file() {
                files += 1;
                total = total.saturating_add(meta.len());
            }
        }
        if limited {
            break;
        }
    }
    Ok(format!(
        "{total} bytes across {files} files / {folders} folders{} (symlinks and other volumes excluded)",
        if limited {
            " · partial result: scan budget reached"
        } else {
            ""
        }
    ))
}

fn sha256(path: &Path, stop: &AtomicBool) -> io::Result<String> {
    sha256_limited(path, MAX_HASH_BYTES, Duration::from_secs(30), stop)
}

pub(crate) fn sha256_limited(
    path: &Path,
    max_bytes: u64,
    timeout: Duration,
    stop: &AtomicBool,
) -> io::Result<String> {
    if stop.load(Ordering::Relaxed) {
        return Err(cancelled());
    }
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW);
    }
    let file = options.open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            "Only regular files can be hashed",
        ));
    }
    if meta.len() > max_bytes.min(MAX_HASH_BYTES) {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            format!(
                "Hash limited to {} MiB to bound resource use",
                max_bytes.min(MAX_HASH_BYTES) / (1024 * 1024)
            ),
        ));
    }
    let mut command = Command::new("sha256sum");
    command.stdin(file);
    // `sha256sum` with no file arguments hashes the supplied file descriptor.
    // Its stdout is 68 bytes for a SHA-256 digest and it never sees a pathname.
    let result = crate::volume::capture_gio_with_stdin(
        command,
        128,
        timeout,
        stop,
        "SHA-256",
    )
    .map_err(io::Error::other)?;
    let digest = std::str::from_utf8(&result).map_err(io::Error::other)?;
    let hex = digest.split_whitespace().next().unwrap_or("");
    if hex.len() != 64 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            "Invalid digest from sha256sum",
        ));
    }
    Ok(hex.into())
}

pub fn deep_analysis(path: &Path, stop: &AtomicBool) -> io::Result<String> {
    let meta = fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            "Symlinks are excluded from deep analysis",
        ));
    }
    if meta.is_dir() {
        folder_size(path, stop)
    } else if meta.is_file() {
        sha256(path, stop)
    } else {
        Err(io::Error::new(
            ErrorKind::InvalidInput,
            "Special files are excluded from deep analysis",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn tiny_folder_size_and_symlink_are_safe() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("rice-meta-{}-{nonce}", std::process::id()));
        fs::create_dir(&root).unwrap();
        fs::write(root.join("tiny.txt"), b"hello").unwrap();
        let output = deep_analysis(&root, &AtomicBool::new(false)).unwrap();
        assert!(output.contains("5 bytes across 1 files"));
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&root, root.join("loop")).unwrap();
            let output = deep_analysis(&root, &AtomicBool::new(false)).unwrap();
            assert!(output.contains("5 bytes across 1 files"));
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cancelled_folder_scan_stops_early() {
        let root = std::env::temp_dir();
        let flag = AtomicBool::new(true);
        assert_eq!(
            deep_analysis(&root, &flag).unwrap_err().kind(),
            ErrorKind::Interrupted
        );
    }

    #[test]
    fn hashes_small_regular_file_using_its_open_descriptor() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("rice-hash-{}-{nonce}", std::process::id()));
        fs::write(&path, b"hello").unwrap();
        let hash = deep_analysis(&path, &AtomicBool::new(false)).unwrap();
        assert_eq!(
            hash,
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
        fs::remove_file(path).unwrap();
    }
}
