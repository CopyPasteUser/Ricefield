//! Shared, theme-independent file ordering and filtering policies.

use crate::filesystem::Entry;
use std::{
    cmp::Ordering,
    collections::HashMap,
    ops::Index,
    time::{Duration, SystemTime},
};

/// Case-folded filenames share one allocation instead of retaining one
/// `String` per directory entry. Normally offsets need only 32 bits; a wide
/// index is retained as a lossless fallback for indexes exceeding 4 GiB.
/// The immutable result can be shared through `Arc` with view workers.
#[derive(Default)]
pub struct FoldedNames {
    data: String,
    ends: NameEnds,
}

enum NameEnds {
    Compact(Vec<u32>),
    Wide(Vec<usize>),
}

impl Default for NameEnds {
    fn default() -> Self {
        Self::Compact(Vec::new())
    }
}

impl NameEnds {
    fn with_capacity(count: usize) -> Self {
        Self::Compact(Vec::with_capacity(count))
    }

    fn push(&mut self, end: usize) {
        if let Self::Compact(ends) = self {
            if let Ok(compact) = u32::try_from(end) {
                ends.push(compact);
                return;
            }
            let mut wide = Vec::with_capacity(ends.capacity().max(ends.len() + 1));
            wide.extend(ends.iter().copied().map(|value| value as usize));
            *self = Self::Wide(wide);
        }
        if let Self::Wide(ends) = self {
            ends.push(end);
        }
    }

    fn len(&self) -> usize {
        match self {
            Self::Compact(ends) => ends.len(),
            Self::Wide(ends) => ends.len(),
        }
    }

    fn get(&self, index: usize) -> usize {
        match self {
            Self::Compact(ends) => ends[index] as usize,
            Self::Wide(ends) => ends[index],
        }
    }

    fn capacity_bytes(&self) -> usize {
        match self {
            Self::Compact(ends) => ends.capacity() * std::mem::size_of::<u32>(),
            Self::Wide(ends) => ends.capacity() * std::mem::size_of::<usize>(),
        }
    }
}

impl FoldedNames {
    pub fn from_entries(entries: &[Entry]) -> Self {
        let mut data = String::with_capacity(entries.iter().map(|entry| entry.name().len()).sum());
        let mut ends = NameEnds::with_capacity(entries.len());
        for entry in entries {
            // Use full Unicode string folding. Lowercasing individual chars
            // would change context-sensitive mappings (e.g. Greek sigma).
            data.push_str(&entry.name().to_lowercase());
            ends.push(data.len());
        }
        data.shrink_to_fit();
        Self { data, ends }
    }

    pub fn len(&self) -> usize {
        self.ends.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ends.len() == 0
    }

    /// Exact owned heap footprint, excluding allocator metadata.
    pub fn heap_capacity_bytes(&self) -> usize {
        self.data.capacity() + self.ends.capacity_bytes()
    }
}

impl Index<usize> for FoldedNames {
    type Output = str;

    fn index(&self, index: usize) -> &str {
        let end = self.ends.get(index);
        let start = if index == 0 {
            0
        } else {
            self.ends.get(index - 1)
        };
        &self.data[start..end]
    }
}

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

/// A linear-time regex, compiled once per refresh, with explicit memory caps.
/// Invalid input is an error, never an implicit match-all filter.
pub fn filename_regex(pattern: &str) -> Result<Option<regex::Regex>, String> {
    if pattern.is_empty() {
        return Ok(None);
    }
    if pattern.len() > 512 {
        return Err("Filename regex is limited to 512 bytes".into());
    }
    regex::RegexBuilder::new(pattern)
        .case_insensitive(true)
        .size_limit(1024 * 1024)
        .dfa_size_limit(1024 * 1024)
        .build()
        .map(Some)
        .map_err(|error| format!("Invalid filename regex: {error}"))
}

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
    CompiledGlob::new(pattern).matches(name)
}

/// Bit-parallel, bounded glob matcher: each input character takes constant
/// time, even for pathological patterns. Compile once per folder refresh.
pub struct CompiledGlob {
    len: usize,
    single: u128,
    stars: u128,
    literals: HashMap<char, u128>,
}

impl CompiledGlob {
    pub fn new(pattern: &str) -> Self {
        let mut result = Self {
            len: 0,
            single: 0,
            stars: 0,
            literals: HashMap::new(),
        };
        let mut previous_star = false;
        for ch in pattern.to_lowercase().chars().take(120) {
            if ch == '*' && previous_star {
                continue;
            }
            let bit = 1u128 << result.len;
            match ch {
                '*' => result.stars |= bit,
                '?' => result.single |= bit,
                literal => *result.literals.entry(literal).or_default() |= bit,
            }
            result.len += 1;
            previous_star = ch == '*';
        }
        result
    }

    fn closure(&self, states: u128) -> u128 {
        // Consecutive '*' are collapsed during compilation, so one epsilon
        // transition per state is sufficient.
        states | ((states & self.stars) << 1)
    }

    pub fn matches(&self, name: &str) -> bool {
        if self.len == 0 {
            return true;
        }
        self.matches_folded(&name.to_lowercase())
    }

    /// The directory scanner already owns a compact case-folded name index.
    /// Reuse it during a glob projection instead of allocating per entry.
    pub fn matches_folded(&self, folded_name: &str) -> bool {
        if self.len == 0 {
            return true;
        }
        let mut states = self.closure(1);
        for ch in folded_name.chars() {
            let matching = self.single | self.literals.get(&ch).copied().unwrap_or_default();
            states = self.closure(((states & matching) << 1) | (states & self.stars));
            if states == 0 {
                return false;
            }
        }
        states & (1u128 << self.len) != 0
    }
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
            || older_days.is_some_and(|min| age < Duration::from_secs(min.saturating_mul(86_400)))
        {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::{
        CompiledGlob, FoldedNames, matches_custom_ranges, matches_filter, matches_glob,
        matches_modified, matches_size, natural_cmp,
    };
    use crate::filesystem::Entry;
    use std::{
        cmp::Ordering,
        path::PathBuf,
        time::{Duration, SystemTime},
    };

    #[test]
    fn compact_folded_names_preserve_unicode_and_reclaim_per_entry_allocations() {
        let mut entries = ["", "Résumé.JPG", "ΣΟΣ", "File10", "file2"]
            .into_iter()
            .map(|name| Entry {
                path: PathBuf::from(name),
                is_dir: false,
                is_symlink: false,
                size: 0,
                modified: None,
            })
            .collect::<Vec<_>>();
        let names = FoldedNames::from_entries(&entries);
        assert_eq!(names.len(), entries.len());
        for (index, entry) in entries.iter().enumerate() {
            assert_eq!(&names[index], entry.name().to_lowercase().as_str());
        }
        assert!(FoldedNames::default().is_empty());

        for index in 0..100_000 {
            entries.push(Entry {
                path: PathBuf::from(format!("file-{index}.txt")),
                is_dir: false,
                is_symlink: false,
                size: 0,
                modified: None,
            });
        }
        let names = FoldedNames::from_entries(&entries);
        let legacy = entries
            .iter()
            .map(|entry| entry.name().to_lowercase())
            .collect::<Vec<_>>();
        let legacy_bytes = legacy.capacity() * std::mem::size_of::<String>()
            + legacy.iter().map(String::capacity).sum::<usize>();
        let compact_bytes = names.heap_capacity_bytes();
        eprintln!("100k folded names: compact={compact_bytes} B legacy={legacy_bytes} B");
        assert!(compact_bytes < legacy_bytes * 3 / 4);
        assert_eq!(&names[100_004], "file-99999.txt");
    }

    #[test]
    fn glob_can_match_existing_folded_names_without_allocating_again() {
        let glob = CompiledGlob::new("*ΣΟΣ?.TXT");
        for name in ["helloΣΟΣ1.txt", "helloςος1.TXT", "no-match.png", "ΣΟΣ2.txt"] {
            assert_eq!(glob.matches(name), glob.matches_folded(&name.to_lowercase()));
        }
    }

    #[cfg(target_pointer_width = "64")]
    #[test]
    fn name_offsets_promote_losslessly_past_four_gibibytes() {
        let mut ends = super::NameEnds::with_capacity(3);
        ends.push(17);
        ends.push(u32::MAX as usize);
        ends.push(u32::MAX as usize + 1);
        assert_eq!(ends.len(), 3);
        assert_eq!(ends.get(0), 17);
        assert_eq!(ends.get(1), u32::MAX as usize);
        assert_eq!(ends.get(2), u32::MAX as usize + 1);
        assert!(matches!(ends, super::NameEnds::Wide(_)));
    }

    #[test]
    fn filename_regex_handles_unicode_errors_and_pathological_input() {
        use super::filename_regex;
        let regex = filename_regex(r"^bericht-(\d+|final)\.pdf$")
            .unwrap()
            .unwrap();
        assert!(regex.is_match("BERICHT-42.PDF"));
        assert!(!regex.is_match("report-42.pdf"));
        assert!(
            filename_regex(r"^\p{Greek}+$")
                .unwrap()
                .unwrap()
                .is_match("αβγ")
        );
        assert!(filename_regex("").unwrap().is_none());
        assert!(filename_regex("[").is_err());
        assert!(filename_regex(&"x".repeat(513)).is_err());
        assert!(filename_regex("a{999999999}").is_err());
        assert!(
            !filename_regex("^(a+)+b$")
                .unwrap()
                .unwrap()
                .is_match(&"a".repeat(100_000))
        );
    }

    #[test]
    fn size_and_date_filters_are_bounded_and_ignore_unknown_presets() {
        let now = SystemTime::now();
        let mut entry = Entry {
            path: PathBuf::from("movie.mkv"),
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
    fn hundred_thousand_virtual_entries_sort_and_glob_without_a_filesystem() {
        // Deterministic permutation, not a presorted case that TimSort can
        // optimize away. Exercises the same natural comparator and compiled
        // glob used by all seven RiceField profiles, without creating any
        // files or pretending this establishes real HDD/GUI benchmarks.
        const COUNT: usize = 100_000;
        let mut names = (0..COUNT)
            .map(|index| format!("file{:06}.txt", (index * 79_919) % COUNT))
            .collect::<Vec<_>>();
        let started = std::time::Instant::now();
        names.sort_unstable_by(|left, right| natural_cmp(left, right));
        assert_eq!(names.first().map(String::as_str), Some("file000000.txt"));
        assert_eq!(names[COUNT / 2], "file050000.txt");
        assert_eq!(names.last().map(String::as_str), Some("file099999.txt"));
        let glob = CompiledGlob::new("file*9.txt");
        assert_eq!(
            names.iter().filter(|name| glob.matches(name)).count(),
            10_000
        );
        eprintln!(
            "100k synthetic natural sort + glob: {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn filters_never_treat_folders_as_media() {
        let entry = Entry {
            path: PathBuf::from("/tmp/photo.jpg"),
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
        assert!(matches_glob(
            "abcdefghijklmnop.pdf",
            "*a*b*c*d*e*f*g*h*i*j*k*l*m*n*o*p.pdf"
        ));
        assert!(matches_glob("acccb", "a**?b"));
        assert!(!matches_glob("ab", "a**??b"));
        assert!(matches_glob("", "***"));
        assert!(!matches_glob("", "?"));
    }

    #[test]
    fn custom_size_and_age_ranges_fail_closed_on_unknown_metadata() {
        let now = SystemTime::now();
        let mut entry = Entry {
            path: PathBuf::from("paper.pdf"),
            is_dir: false,
            is_symlink: false,
            size: 3 * 1024 * 1024,
            modified: Some(now - Duration::from_secs(5 * 86_400)),
        };
        assert!(matches_custom_ranges(
            &entry,
            Some(2),
            Some(4),
            Some(6),
            Some(4),
            now
        ));
        assert!(!matches_custom_ranges(
            &entry,
            Some(4),
            None,
            None,
            None,
            now
        ));
        assert!(!matches_custom_ranges(
            &entry,
            None,
            Some(2),
            None,
            None,
            now
        ));
        assert!(!matches_custom_ranges(
            &entry,
            None,
            None,
            Some(3),
            None,
            now
        ));
        assert!(!matches_custom_ranges(
            &entry,
            None,
            None,
            None,
            Some(8),
            now
        ));
        entry.modified = None;
        assert!(!matches_custom_ranges(
            &entry,
            None,
            None,
            Some(30),
            None,
            now
        ));
        entry.is_dir = true;
        assert!(!matches_custom_ranges(
            &entry,
            Some(1),
            None,
            None,
            None,
            now
        ));
    }
}
