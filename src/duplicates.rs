//! Read-only duplicate candidates for the current visible local folder.
//! Results never authorize automatic deletion: files can change after hashing.

use crate::{filesystem::Entry, metadata};
use std::{
    collections::BTreeMap,
    io,
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

const MAX_FILES: usize = 256;
const MAX_FILE_BYTES: u64 = 128 * 1024 * 1024;
const MAX_TOTAL_BYTES: u64 = 512 * 1024 * 1024;
const MAX_DURATION: Duration = Duration::from_secs(45);

#[derive(Debug)]
pub struct Report {
    pub groups: Vec<Vec<PathBuf>>,
    pub hashed: usize,
    pub skipped: usize,
    pub partial: bool,
}

/// Only files of equal size need hashing. The scanner refuses symlinks and
/// special files, limits work, and never operates on any results itself.
pub fn find(entries: &[Entry], cancelled: &AtomicBool) -> io::Result<Report> {
    let mut by_size = BTreeMap::<u64, Vec<PathBuf>>::new();
    let mut skipped = 0usize;
    for entry in entries {
        if entry.is_dir || entry.is_symlink {
            continue;
        }
        if entry.size > MAX_FILE_BYTES {
            skipped += 1;
            continue;
        }
        by_size
            .entry(entry.size)
            .or_default()
            .push(entry.path.clone());
    }
    let mut results = BTreeMap::<(u64, String), Vec<PathBuf>>::new();
    let mut hashed = 0usize;
    let mut total_bytes = 0u64;
    let mut partial = skipped > 0;
    let started = Instant::now();
    'sizes: for (size, files) in by_size {
        if files.len() < 2 {
            continue;
        }
        for path in files {
            if cancelled.load(Ordering::Relaxed) {
                return Err(io::Error::new(io::ErrorKind::Interrupted, "Scan cancelled"));
            }
            if hashed >= MAX_FILES
                || total_bytes.saturating_add(size) > MAX_TOTAL_BYTES
                || started.elapsed() >= MAX_DURATION
            {
                partial = true;
                break 'sizes;
            }
            hashed += 1;
            let remaining = MAX_DURATION.saturating_sub(started.elapsed());
            if remaining.is_zero() {
                partial = true;
                break 'sizes;
            }
            match metadata::sha256_limited(
                &path,
                MAX_FILE_BYTES,
                remaining.min(Duration::from_secs(30)),
                cancelled,
            ) {
                Ok(hash) => {
                    total_bytes = total_bytes.saturating_add(size);
                    results.entry((size, hash)).or_default().push(path);
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => return Err(error),
                Err(_) => {
                    skipped += 1;
                    partial = true;
                }
            }
        }
    }
    let groups = results
        .into_values()
        .filter(|paths| paths.len() > 1)
        .collect();
    Ok(Report {
        groups,
        hashed,
        skipped,
        partial,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, time::SystemTime};

    #[test]
    fn equal_size_is_not_enough_to_claim_a_duplicate() {
        let root = std::env::temp_dir().join(format!(
            "rice-duplicates-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        let mut entries = Vec::new();
        for (name, contents) in [
            ("a.txt", b"identical".as_slice()),
            ("b.txt", b"identical".as_slice()),
            ("c.txt", b"different".as_slice()),
        ] {
            let path = root.join(name);
            fs::write(&path, contents).unwrap();
            entries.push(Entry {
                path,
                name: name.into(),
                is_dir: false,
                is_symlink: false,
                size: contents.len() as u64,
                modified: None,
            });
        }
        let report = find(&entries, &AtomicBool::new(false)).unwrap();
        assert_eq!(report.groups.len(), 1);
        assert_eq!(report.groups[0].len(), 2);
        assert!(!report.partial);
        assert_eq!(report.hashed, 3);
        assert_eq!(fs::read(root.join("a.txt")).unwrap(), b"identical");
        assert!(find(&entries, &AtomicBool::new(true)).is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
