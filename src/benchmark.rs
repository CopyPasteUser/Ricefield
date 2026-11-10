//! Explicit disposable lab benchmark. Never scans or mutates user directories.
use crate::{app, config, file_view, filesystem};
use std::{
    fs::{self, File},
    io::{self, Read, Write},
    path::PathBuf,
    sync::atomic::AtomicBool,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

struct Fixture(PathBuf);

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub fn run() -> io::Result<()> {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("ricefield-bench-{}-{nonce}", std::process::id()));
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(&root)?;
    let fixture = Fixture(root);
    let folder = fixture.0.join("entries");
    fs::create_dir(&folder)?;
    let started = Instant::now();
    for index in 0..100_000 {
        File::create(folder.join(format!("file-{index}.txt")))?;
    }
    let fixture_ms = started.elapsed().as_secs_f64() * 1000.0;
    let cancelled = AtomicBool::new(false);
    let started = Instant::now();
    let mut scan = filesystem::scan_cancellable(&folder, &cancelled)?;
    let scan_ms = started.elapsed().as_secs_f64() * 1000.0;
    if scan.entries.len() != 100_000 || scan.unreadable != 0 {
        return Err(io::Error::other("Benchmark directory scan was incomplete"));
    }
    // Match the scan worker's compact-before-handoff behavior. Report only
    // retained vector storage, not a fictitious process-wide RSS reduction.
    let spare_entry_bytes_before =
        (scan.entries.capacity() - scan.entries.len()) * std::mem::size_of::<filesystem::Entry>();
    scan.compact();
    let spare_entry_bytes_after =
        (scan.entries.capacity() - scan.entries.len()) * std::mem::size_of::<filesystem::Entry>();
    let started = Instant::now();
    let classified = (0..10)
        .flat_map(|_| scan.entries.iter())
        .filter(|entry| std::hint::black_box(entry.kind()) == "Document")
        .count();
    let kind_ms = started.elapsed().as_secs_f64() * 1000.0;
    if classified != 1_000_000 {
        return Err(io::Error::other(
            "Benchmark file classifications were incorrect",
        ));
    }
    let compact_names = file_view::FoldedNames::from_entries(&scan.entries);
    let folded_name_index_bytes = compact_names.heap_capacity_bytes();
    let started = Instant::now();
    let visible = app::project_local_view(
        &scan.entries,
        &compact_names,
        "",
        false,
        &config::Config::default(),
        &cancelled,
    )
    .ok_or_else(|| io::Error::other("Benchmark view projection was cancelled"))?;
    let view_projection_ms = started.elapsed().as_secs_f64() * 1000.0;
    if visible.len() != 100_000
        || scan.entries[visible[0]].name() != "file-0.txt"
        || scan.entries[visible[99_999]].name() != "file-99999.txt"
    {
        return Err(io::Error::other("Benchmark view ordering was incorrect"));
    }
    let retained_view_index_bytes = visible.capacity() * std::mem::size_of::<usize>();
    let mut names: Vec<_> = scan
        .entries
        .iter()
        .map(|entry| entry.name().to_lowercase())
        .collect();
    // Compare exact owned heap capacities against the former representation.
    // These figures are for the filename index, not full-process RSS.
    let legacy_folded_names_bytes = names.capacity() * std::mem::size_of::<String>()
        + names.iter().map(String::capacity).sum::<usize>();
    let started = Instant::now();
    names.sort_by(|left, right| file_view::natural_cmp(left, right));
    let sort_ms = started.elapsed().as_secs_f64() * 1000.0;
    if names[2] != "file-2.txt" || names[10] != "file-10.txt" {
        return Err(io::Error::other("Benchmark natural ordering failed"));
    }
    let glob = file_view::CompiledGlob::new("file-9*.txt");
    let started = Instant::now();
    let matches = names.iter().filter(|name| glob.matches(name)).count();
    let filter_ms = started.elapsed().as_secs_f64() * 1000.0;
    if matches != 11_111 {
        return Err(io::Error::other(
            "Benchmark filter produced incorrect results",
        ));
    }
    let started = Instant::now();
    let search =
        filesystem::search_with_options(&folder, "file-9999", false, false, false, &cancelled)?;
    let search_ms = started.elapsed().as_secs_f64() * 1000.0;
    // Search is intentionally budgeted; report truncation instead of claiming
    // to have searched all entries when its visit/time cap was reached.
    let source = fixture.0.join("payload.bin");
    let destination = fixture.0.join("destination");
    fs::create_dir(&destination)?;
    let mut file = File::create(&source)?;
    let chunk: Vec<u8> = (0..64 * 1024).map(|index| (index % 251) as u8).collect();
    for _ in 0..1024 {
        file.write_all(&chunk)?;
    }
    file.sync_all()?;
    drop(file);
    let started = Instant::now();
    filesystem::perform_with_report(
        &filesystem::Action::Copy {
            sources: vec![source.clone()],
            destination: destination.clone(),
            conflict: filesystem::ConflictPolicy::AbortOnConflict,
        },
        &cancelled,
        &mut |_| {},
    )
    .result?;
    let copy_ms = started.elapsed().as_secs_f64() * 1000.0;
    let mut copied = File::open(destination.join("payload.bin"))?;
    let mut buffer = vec![0u8; chunk.len()];
    for _ in 0..1024 {
        copied.read_exact(&mut buffer)?;
        if buffer != chunk {
            return Err(io::Error::other("Benchmark copy verification failed"));
        }
    }
    if copied.read(&mut [0])? != 0 || fs::metadata(source)?.len() != 64 * 1024 * 1024 {
        return Err(io::Error::other(
            "Benchmark copy length/source preservation failed",
        ));
    }
    println!(
        "version = {:?}\nentries = 100000\nfixture_ms = {fixture_ms:.3}\nscan_ms = {scan_ms:.3}\nspare_entry_bytes_before = {spare_entry_bytes_before}\nspare_entry_bytes_after = {spare_entry_bytes_after}\nfile_kind_calls = 1000000\nfile_kind_ms = {kind_ms:.3}\nfolded_name_index_bytes = {folded_name_index_bytes}\nlegacy_folded_names_bytes = {legacy_folded_names_bytes}\nview_projection_ms = {view_projection_ms:.3}\nretained_view_index_bytes = {retained_view_index_bytes}\nnatural_sort_ms = {sort_ms:.3}\nglob_ms = {filter_ms:.3}\nglob_matches = {matches}\nsearch_ms = {search_ms:.3}\nsearch_matches = {}\nsearch_incomplete = {}\ncopy_bytes = 67108864\ncopy_ms = {copy_ms:.3}\ncopy_verified = true\nsource_retained = true",
        env!("CARGO_PKG_VERSION"),
        search.entries.len(),
        search.notice.is_some(),
    );
    let entry_struct_bytes = std::mem::size_of::<filesystem::Entry>();
    let retained_entry_headers_bytes = scan.entries.len() * entry_struct_bytes;
    let duplicated_filename_bytes_avoided: usize =
        scan.entries.iter().map(|entry| entry.name().len()).sum();
    println!(
        "entry_struct_bytes = {entry_struct_bytes}\nretained_entry_headers_bytes = {retained_entry_headers_bytes}\nduplicated_filename_bytes_avoided = {duplicated_filename_bytes_avoided}"
    );
    Ok(())
}
