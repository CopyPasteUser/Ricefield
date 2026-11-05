//! Shared, theme-independent file ordering and filtering policies.

use crate::filesystem::Entry;
use std::{
    cmp::Ordering,
    time::{Duration, SystemTime},
};

/// Natural comparison without integer parsing/overflow (file2 < file10).
/// Inputs are already folded for case-insensitive comparisons by the scanner.
pub fn natural_cmp(left: &str, right: &str) -> Ordering {
    let a = left.as_bytes();
    let b = right.as_bytes();
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        if a[i].is_ascii_digit() && b[j].is_ascii_digit() {
            let start_a = i;
            let start_b = j;
            while i < a.len() && a[i].is_ascii_digit() {
                i += 1;
            }
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            let digits_a = &a[start_a..i];
            let digits_b = &b[start_b..j];
            let short_a = digits_a
                .iter()
                .position(|ch| *ch != b'0')
                .unwrap_or(digits_a.len());
            let short_b = digits_b
                .iter()
                .position(|ch| *ch != b'0')
                .unwrap_or(digits_b.len());
            let number_a = &digits_a[short_a..];
            let number_b = &digits_b[short_b..];
            let compared = number_a
                .len()
                .cmp(&number_b.len())
                .then_with(|| number_a.cmp(number_b));
            if !compared.is_eq() {
                return compared;
            }
            let zeros = digits_a.len().cmp(&digits_b.len());
            if !zeros.is_eq() {
                return zeros;
            }
        } else {
            let compared = a[i].cmp(&b[j]);
            if !compared.is_eq() {
                return compared;
            }
            i += 1;
            j += 1;
        }
    }
    a.len().saturating_sub(i).cmp(&b.len().saturating_sub(j))
}

pub const FILTERS: &[(&str, &str)] = &[
    ("all", "All items"),
    ("folders", "Folders"),
    ("files", "Files"),
    ("images", "Images"),
    ("documents", "Documents / PDFs"),
    ("audio", "Audio"),
    ("video", "Video"),
    ("archives", "Archives"),
    ("code", "Code"),
];

pub const SIZE_FILTERS: &[(&str, &str)] = &[
    ("any", "Any size"),
    ("small", "Under 1 MiB"),
    ("medium", "1–100 MiB"),
    ("large", "100 MiB or more"),
];

pub const MODIFIED_FILTERS: &[(&str, &str)] = &[
    ("any", "Any date"),
    ("day", "Past 24 hours"),
    ("week", "Past 7 days"),
    ("month", "Past 30 days"),
];

pub fn matches_size(entry: &Entry, filter: &str) -> bool {
    const MIB: u64 = 1024 * 1024;
    let regular = !entry.is_dir && !entry.is_symlink;
    match filter {
        // Directory entry sizes and symlink lengths are not file contents.
        "small" => regular && entry.size < MIB,
        "medium" => regular && (MIB..100 * MIB).contains(&entry.size),
        "large" => regular && entry.size >= 100 * MIB,
        _ => true,
    }
}

pub fn matches_modified(entry: &Entry, filter: &str, now: SystemTime) -> bool {
    let max_age = match filter {
        "day" => Duration::from_secs(24 * 3600),
        "week" => Duration::from_secs(7 * 24 * 3600),
        "month" => Duration::from_secs(30 * 24 * 3600),
        _ => return true,
    };
    entry.modified.is_some_and(|modified| {
        // Future timestamps can occur from clock skew; count as recent.
        now.duration_since(modified).unwrap_or_default() <= max_age
    })
}

pub fn matches_filter(entry: &Entry, filter: &str) -> bool {
    match filter {
        "folders" => entry.is_dir,
        "files" => !entry.is_dir,
        "images" => entry.kind() == "Image",
        "documents" => matches!(entry.kind(), "Document" | "PDF"),
        "audio" => entry.kind() == "Audio",
        "video" => entry.kind() == "Video",
        "archives" => entry.kind() == "Archive",
        "code" => entry.kind() == "Code",
        _ => true,
    }
}

/// Small, case-insensitive filename glob. `*` matches any run of characters,
/// `?` matches one Unicode scalar. Deliberately no regex engine or filesystem IO.
#[cfg(test)]
pub fn matches_glob(name: &str, pattern: &str) -> bool {
    let compiled: Vec<_> = pattern.to_lowercase().chars().take(120).collect();
    matches_compiled_glob(name, &compiled)
}

/// Precompile once for a folder refresh instead of allocating the pattern
/// again for every file in a potentially 100k-entry directory.
pub fn matches_compiled_glob(name: &str, glob: &[char]) -> bool {
    if glob.is_empty() {
        return true;
    }
    let text: Vec<_> = name.to_lowercase().chars().collect();
    let (mut ni, mut gi, mut backtrack) = (0usize, 0usize, None);
    let mut after_star = 0usize;
    while ni < text.len() {
        if gi < glob.len() && (glob[gi] == '?' || glob[gi] == text[ni]) {
            ni += 1;
            gi += 1;
        } else if gi < glob.len() && glob[gi] == '*' {
            backtrack = Some(gi);
            gi += 1;
            after_star = ni;
        } else if let Some(star) = backtrack {
            after_star += 1;
            ni = after_star;
            gi = star + 1;
        } else {
            return false;
        }
    }
    while gi < glob.len() && glob[gi] == '*' {
        gi += 1;
    }
    gi == glob.len()
}

/// Additional, opt-in numeric bounds. Values are measured in MiB and whole
/// days; folders/symlinks do not represent file content sizes.
pub fn matches_custom_ranges(
    entry: &Entry,
    min_size_mib: Option<u64>,
    max_size_mib: Option<u64>,
    newer_days: Option<u64>,
    older_days: Option<u64>,
    now: SystemTime,
) -> bool {
    let mib = 1024 * 1024;
    if min_size_mib.is_some() || max_size_mib.is_some() {
        if entry.is_dir || entry.is_symlink {
            return false;
        }
        if min_size_mib.is_some_and(|min| entry.size < min.saturating_mul(mib))
            || max_size_mib.is_some_and(|max| entry.size > max.saturating_mul(mib))
        {
            return false;
        }
    }
    if newer_days.is_some() || older_days.is_some() {
        let Some(modified) = entry.modified else {
            return false;
        };
        let age = now.duration_since(modified).unwrap_or_default();
        if newer_days.is_some_and(|max| age > Duration::from_secs(max.saturating_mul(86_400)))
            || older_days.is_some_and(|min| {
                age < Duration::from_secs(min.saturating_mul(86_400))
            })
        {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::{
        matches_custom_ranges, matches_filter, matches_glob, matches_modified, matches_size,
        natural_cmp,
    };
    use crate::filesystem::Entry;
    use std::{
        cmp::Ordering,
        path::PathBuf,
        time::{Duration, SystemTime},
    };

    #[test]
    fn size_and_date_filters_are_bounded_and_ignore_unknown_presets() {
        let now = SystemTime::now();
        let mut entry = Entry {
            path: PathBuf::from("movie.mkv"),
            name: "movie.mkv".into(),
            is_dir: false,
            is_symlink: false,
            size: 100 * 1024 * 1024,
            modified: Some(now - Duration::from_secs(8 * 24 * 3600)),
        };
        assert!(matches_size(&entry, "large"));
        assert!(!matches_size(&entry, "medium"));
        assert!(matches_modified(&entry, "month", now));
        assert!(!matches_modified(&entry, "week", now));
        assert!(matches_size(&entry, "unknown"));
        assert!(matches_modified(&entry, "unknown", now));
        entry.size = 1024 * 1024;
        assert!(!matches_size(&entry, "small"));
        assert!(matches_size(&entry, "medium"));
        entry.modified = None;
        assert!(!matches_modified(&entry, "day", now));
        entry.is_dir = true;
        assert!(!matches_size(&entry, "small"));
        assert!(!matches_size(&entry, "large"));
        entry.is_dir = false;
        entry.is_symlink = true;
        assert!(!matches_size(&entry, "medium"));
    }

    #[test]
    fn natural_sort_preserves_numeric_magnitude_without_overflow() {
        assert_eq!(natural_cmp("file2", "file10"), Ordering::Less);
        assert_eq!(natural_cmp("x9", "x010"), Ordering::Less);
        assert_eq!(natural_cmp("file1", "file01"), Ordering::Less);
        assert_eq!(
            natural_cmp("x999999999999999999999999", "x1000000000000000000000000"),
            Ordering::Less
        );
        assert_eq!(natural_cmp("abc", "abcd"), Ordering::Less);
    }

    #[test]
    fn filters_never_treat_folders_as_media() {
        let entry = Entry {
            path: PathBuf::from("/tmp/photo.jpg"),
            name: "photo.jpg".into(),
            is_dir: true,
            is_symlink: false,
            size: 0,
            modified: None,
        };
        assert!(matches_filter(&entry, "folders"));
        assert!(!matches_filter(&entry, "images"));
        assert!(!matches_filter(&entry, "files"));
    }

    #[test]
    fn filename_globs_match_unicode_without_path_traversal_or_regex() {
        assert!(matches_glob("Résumé (2).pdf", "r?sum*PDF"));
        assert!(matches_glob("file10.txt", "file*.txt"));
        assert!(!matches_glob("file.txt", "file?.txt"));
        assert!(!matches_glob("folder/file.txt", "*.pdf"));
        assert!(matches_glob("anything", ""));
    }

    #[test]
    fn custom_size_and_age_ranges_fail_closed_on_unknown_metadata() {
        let now = SystemTime::now();
        let mut entry = Entry {
            path: PathBuf::from("paper.pdf"),
            name: "paper.pdf".into(),
            is_dir: false,
            is_symlink: false,
            size: 3 * 1024 * 1024,
            modified: Some(now - Duration::from_secs(5 * 86_400)),
        };
        assert!(matches_custom_ranges(&entry, Some(2), Some(4), Some(6), Some(4), now));
        assert!(!matches_custom_ranges(&entry, Some(4), None, None, None, now));
        assert!(!matches_custom_ranges(&entry, None, Some(2), None, None, now));
        assert!(!matches_custom_ranges(&entry, None, None, Some(3), None, now));
        assert!(!matches_custom_ranges(&entry, None, None, None, Some(8), now));
        entry.modified = None;
        assert!(!matches_custom_ranges(&entry, None, None, Some(30), None, now));
        entry.is_dir = true;
        assert!(!matches_custom_ranges(&entry, Some(1), None, None, None, now));
    }
}
