//! Shared, theme-independent file ordering and filtering policies.

use crate::filesystem::Entry;
use std::cmp::Ordering;

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

#[cfg(test)]
mod tests {
    use super::{matches_filter, natural_cmp};
    use crate::filesystem::Entry;
    use std::{cmp::Ordering, path::PathBuf};

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
}
