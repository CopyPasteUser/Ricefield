//! Archive operations isolated from normal file mutations.
//!
//! libarchive is loaded at runtime so minimal systems can still launch the
//! explorer. No archive pathname is ever trusted as a destination path.
//! Extraction happens in a private staging directory, with only regular files
//! and directories permitted. Publication never overwrites an existing entry.

use libloading::Library;
use std::{
    collections::BTreeSet,
    ffi::{CStr, CString, OsStr},
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    os::fd::AsRawFd,
    os::unix::{
        ffi::OsStrExt,
        fs::{DirBuilderExt, OpenOptionsExt},
    },
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

const OK: i32 = 0;
const EOF: i32 = 1;
const REGULAR: u32 = 0o100000;
const DIRECTORY: u32 = 0o040000;
const MAX_ENTRIES: usize = 20_000;
const MAX_ENTRY_BYTES: u64 = 512 * 1024 * 1024;
const MAX_TOTAL_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_PATH_BYTES: usize = 4096;
const MAX_PATH_DEPTH: usize = 64;
const IO_BLOCK: usize = 64 * 1024;
const MAX_PASSWORD_BYTES: usize = 4096;

type Ptr = *mut std::ffi::c_void;
type New = unsafe extern "C" fn() -> Ptr;
type Free = unsafe extern "C" fn(Ptr) -> i32;
type Config = unsafe extern "C" fn(Ptr) -> i32;
type Next = unsafe extern "C" fn(Ptr, *mut Ptr) -> i32;
type ReadData = unsafe extern "C" fn(Ptr, Ptr, usize) -> isize;
type WriteData = unsafe extern "C" fn(Ptr, *const std::ffi::c_void, usize) -> isize;
type Header = unsafe extern "C" fn(Ptr, Ptr) -> i32;

struct Api {
    _library: Library,
    read_new: New,
    read_free: Free,
    read_filter_all: Config,
    read_format_all: Config,
    read_open_fd: unsafe extern "C" fn(Ptr, i32, usize) -> i32,
    read_add_passphrase: unsafe extern "C" fn(Ptr, *const libc::c_char) -> i32,
    read_next: Next,
    read_data: ReadData,
    write_new: New,
    write_free: Free,
    write_close: Config,
    write_zip: Config,
    write_tar: Config,
    write_7z: Config,
    write_gzip: Config,
    write_xz: Config,
    write_zstd: Config,
    write_open: unsafe extern "C" fn(Ptr, *const libc::c_char) -> i32,
    write_set_options: unsafe extern "C" fn(Ptr, *const libc::c_char) -> i32,
    write_set_passphrase: unsafe extern "C" fn(Ptr, *const libc::c_char) -> i32,
    write_header: Header,
    write_data: WriteData,
    entry_new: New,
    entry_free: unsafe extern "C" fn(Ptr),
    entry_path: unsafe extern "C" fn(Ptr) -> *const libc::c_char,
    entry_size: unsafe extern "C" fn(Ptr) -> i64,
    entry_type: unsafe extern "C" fn(Ptr) -> u32,
    entry_hardlink: unsafe extern "C" fn(Ptr) -> *const libc::c_char,
    entry_set_path: unsafe extern "C" fn(Ptr, *const libc::c_char),
    entry_set_type: unsafe extern "C" fn(Ptr, u32),
    entry_set_size: unsafe extern "C" fn(Ptr, i64),
    entry_set_perm: unsafe extern "C" fn(Ptr, i32),
    error_string: unsafe extern "C" fn(Ptr) -> *const libc::c_char,
}

impl Api {
    fn load() -> io::Result<Self> {
        // All symbols have the libarchive C ABI. The library remains loaded
        // for the entire lifetime of the function pointers.
        let library = unsafe { Library::new("libarchive.so.13") }
            .or_else(|_| unsafe { Library::new("libarchive.so") })
            .map_err(|error| invalid(format!("Archive support needs libarchive: {error}")))?;
        macro_rules! sym {
            ($name:literal) => {{
                // SAFETY: libarchive exports this exact function and ABI.
                unsafe { library.get(concat!($name, "\0").as_bytes()) }
                    .map(|symbol: libloading::Symbol<'_, _>| *symbol)
                    .map_err(|error| invalid(format!("libarchive {}: {error}", $name)))?
            }};
        }
        Ok(Self {
            read_new: sym!("archive_read_new"),
            read_free: sym!("archive_read_free"),
            read_filter_all: sym!("archive_read_support_filter_all"),
            read_format_all: sym!("archive_read_support_format_all"),
            read_open_fd: sym!("archive_read_open_fd"),
            read_add_passphrase: sym!("archive_read_add_passphrase"),
            read_next: sym!("archive_read_next_header"),
            read_data: sym!("archive_read_data"),
            write_new: sym!("archive_write_new"),
            write_free: sym!("archive_write_free"),
            write_close: sym!("archive_write_close"),
            write_zip: sym!("archive_write_set_format_zip"),
            write_tar: sym!("archive_write_set_format_pax_restricted"),
            write_7z: sym!("archive_write_set_format_7zip"),
            write_gzip: sym!("archive_write_add_filter_gzip"),
            write_xz: sym!("archive_write_add_filter_xz"),
            write_zstd: sym!("archive_write_add_filter_zstd"),
            write_open: sym!("archive_write_open_filename"),
            write_set_options: sym!("archive_write_set_options"),
            write_set_passphrase: sym!("archive_write_set_passphrase"),
            write_header: sym!("archive_write_header"),
            write_data: sym!("archive_write_data"),
            entry_new: sym!("archive_entry_new"),
            entry_free: sym!("archive_entry_free"),
            entry_path: sym!("archive_entry_pathname"),
            entry_size: sym!("archive_entry_size"),
            entry_type: sym!("archive_entry_filetype"),
            entry_hardlink: sym!("archive_entry_hardlink"),
            entry_set_path: sym!("archive_entry_set_pathname"),
            entry_set_type: sym!("archive_entry_set_filetype"),
            entry_set_size: sym!("archive_entry_set_size"),
            entry_set_perm: sym!("archive_entry_set_perm"),
            error_string: sym!("archive_error_string"),
            _library: library,
        })
    }

    fn check(&self, handle: Ptr, status: i32) -> io::Result<()> {
        if status == OK {
            Ok(())
        } else {
            // SAFETY: error_string is valid until the next libarchive call.
            let raw = unsafe { (self.error_string)(handle) };
            let detail = if raw.is_null() {
                format!("libarchive status {status}")
            } else {
                unsafe { CStr::from_ptr(raw) }
                    .to_string_lossy()
                    .into_owned()
            };
            Err(invalid(detail))
        }
    }
}

struct Reader<'a> {
    api: &'a Api,
    handle: Ptr,
    file: File,
}

impl<'a> Reader<'a> {
    fn open(api: &'a Api, path: &Path, password: Option<&str>) -> io::Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)?;
        if !file.metadata()?.is_file() {
            return Err(invalid("Archive must be a regular file"));
        }
        let handle = unsafe { (api.read_new)() };
        if handle.is_null() {
            return Err(invalid("Cannot allocate archive reader"));
        }
        let reader = Self { api, handle, file };
        api.check(handle, unsafe { (api.read_filter_all)(handle) })?;
        api.check(handle, unsafe { (api.read_format_all)(handle) })?;
        if let Some(password) = password {
            let passphrase = validate_password(password)?;
            api.check(handle, unsafe {
                (api.read_add_passphrase)(handle, passphrase.as_ptr())
            })?;
        }
        // libarchive borrows archive_read_open_fd's descriptor; the Reader
        // owns it until archive_read_free, so path replacement cannot redirect
        // a later open to a symlink or a special file.
        api.check(handle, unsafe {
            (api.read_open_fd)(handle, reader.file.as_raw_fd(), IO_BLOCK)
        })?;
        Ok(reader)
    }

    fn next(&self) -> io::Result<Option<Ptr>> {
        let mut entry = std::ptr::null_mut();
        let status = unsafe { (self.api.read_next)(self.handle, &mut entry) };
        if status == EOF {
            Ok(None)
        } else {
            self.api.check(self.handle, status)?;
            Ok(Some(entry))
        }
    }

    fn data(&self, buffer: &mut [u8]) -> io::Result<usize> {
        let bytes =
            unsafe { (self.api.read_data)(self.handle, buffer.as_mut_ptr().cast(), buffer.len()) };
        if bytes < 0 {
            self.api.check(self.handle, bytes as i32)?;
            Err(invalid("Archive read failed"))
        } else {
            Ok(bytes as usize)
        }
    }
}

impl Drop for Reader<'_> {
    fn drop(&mut self) {
        unsafe { (self.api.read_free)(self.handle) };
    }
}

struct Writer<'a> {
    api: &'a Api,
    handle: Ptr,
    closed: bool,
}

impl<'a> Writer<'a> {
    fn open(api: &'a Api, path: &Path, format: Format, password: Option<&str>) -> io::Result<Self> {
        let name = os_cstring(path.as_os_str())?;
        let handle = unsafe { (api.write_new)() };
        if handle.is_null() {
            return Err(invalid("Cannot allocate archive writer"));
        }
        let writer = Self {
            api,
            handle,
            closed: false,
        };
        let configure = match format {
            Format::Zip => api.write_zip,
            Format::SevenZip => api.write_7z,
            _ => api.write_tar,
        };
        api.check(handle, unsafe { configure(handle) })?;
        if let Some(password) = password {
            if format != Format::Zip {
                return Err(invalid("Encryption is only supported for ZIP creation"));
            }
            let passphrase = validate_password(password)?;
            api.check(handle, unsafe {
                (api.write_set_options)(handle, c"zip:encryption=aes256".as_ptr())
            })?;
            api.check(handle, unsafe {
                (api.write_set_passphrase)(handle, passphrase.as_ptr())
            })?;
        }
        let filter = match format {
            Format::TarGz => Some(api.write_gzip),
            Format::TarXz => Some(api.write_xz),
            Format::TarZst => Some(api.write_zstd),
            _ => None,
        };
        if let Some(filter) = filter {
            api.check(handle, unsafe { filter(handle) })?;
        }
        api.check(handle, unsafe { (api.write_open)(handle, name.as_ptr()) })?;
        Ok(writer)
    }

    fn add(
        &self,
        path: &Path,
        file: Option<&mut File>,
        size: u64,
        cancelled: &AtomicBool,
        progress: &mut impl FnMut(ArchiveProgress),
    ) -> io::Result<()> {
        let entry = unsafe { (self.api.entry_new)() };
        if entry.is_null() {
            return Err(invalid("Cannot allocate archive entry"));
        }
        struct EntryGuard<'a> {
            api: &'a Api,
            ptr: Ptr,
        }
        impl Drop for EntryGuard<'_> {
            fn drop(&mut self) {
                unsafe { (self.api.entry_free)(self.ptr) }
            }
        }
        let _guard = EntryGuard {
            api: self.api,
            ptr: entry,
        };
        let name = os_cstring(path.as_os_str())?;
        unsafe {
            (self.api.entry_set_path)(entry, name.as_ptr());
            (self.api.entry_set_type)(entry, if file.is_some() { REGULAR } else { DIRECTORY });
            (self.api.entry_set_perm)(entry, if file.is_some() { 0o644 } else { 0o755 });
            (self.api.entry_set_size)(entry, size as i64);
        }
        self.api.check(self.handle, unsafe {
            (self.api.write_header)(self.handle, entry)
        })?;
        if let Some(file) = file {
            let mut remaining = size;
            let mut buffer = [0u8; IO_BLOCK];
            while remaining > 0 {
                check_cancel(cancelled)?;
                let count = file.read(&mut buffer[..remaining.min(IO_BLOCK as u64) as usize])?;
                if count == 0 {
                    return Err(invalid("Source file changed during archive creation"));
                }
                let mut sent = 0;
                while sent < count {
                    let n = unsafe {
                        (self.api.write_data)(
                            self.handle,
                            buffer[sent..count].as_ptr().cast(),
                            count - sent,
                        )
                    };
                    if n <= 0 {
                        return Err(invalid("Unable to write archive payload"));
                    }
                    sent += n as usize;
                }
                remaining -= count as u64;
                progress(ArchiveProgress {
                    path: path.to_path_buf(),
                    bytes: count as u64,
                });
            }
        }
        Ok(())
    }

    fn finish(mut self) -> io::Result<()> {
        let result = self
            .api
            .check(self.handle, unsafe { (self.api.write_close)(self.handle) });
        self.closed = true;
        result
    }
}

impl Drop for Writer<'_> {
    fn drop(&mut self) {
        if !self.closed {
            unsafe { (self.api.write_close)(self.handle) };
        }
        unsafe { (self.api.write_free)(self.handle) };
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Zip,
    TarGz,
    TarXz,
    TarZst,
    SevenZip,
}

impl Format {
    pub const ALL: [Self; 5] = [
        Self::Zip,
        Self::TarGz,
        Self::TarXz,
        Self::TarZst,
        Self::SevenZip,
    ];

    pub fn extension(self) -> &'static str {
        match self {
            Self::Zip => "zip",
            Self::TarGz => "tar.gz",
            Self::TarXz => "tar.xz",
            Self::TarZst => "tar.zst",
            Self::SevenZip => "7z",
        }
    }

    pub fn is_archive(path: &Path) -> bool {
        let value = path
            .file_name()
            .and_then(OsStr::to_str)
            .unwrap_or("")
            .to_ascii_lowercase();
        [
            ".zip", ".tar", ".tar.gz", ".tgz", ".tar.xz", ".txz", ".tar.zst", ".tzst", ".7z",
            ".rar",
        ]
        .iter()
        .any(|suffix| value.ends_with(suffix))
    }
}

#[derive(Clone, Debug)]
pub struct ArchiveEntry {
    pub path: PathBuf,
    pub bytes: u64,
    pub directory: bool,
}

#[derive(Clone, Debug)]
pub struct ArchiveProgress {
    pub path: PathBuf,
    pub bytes: u64,
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

fn validate_password(password: &str) -> io::Result<CString> {
    if password.is_empty() || password.len() > MAX_PASSWORD_BYTES {
        return Err(invalid("Password must be between 1 and 4096 bytes"));
    }
    CString::new(password).map_err(|_| invalid("Password contains a NUL byte"))
}

fn os_cstring(value: &OsStr) -> io::Result<CString> {
    CString::new(value.as_bytes()).map_err(|_| invalid("NUL in file path"))
}

fn check_cancel(cancelled: &AtomicBool) -> io::Result<()> {
    if cancelled.load(Ordering::Relaxed) {
        Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "Archive operation cancelled",
        ))
    } else {
        Ok(())
    }
}

/// Convert a hostile archive pathname into a lexical, relative Linux path.
/// Reject foreign separator syntax to avoid creating surprising Windows paths.
fn safe_path(raw: &[u8]) -> io::Result<PathBuf> {
    if raw.is_empty() || raw.len() > MAX_PATH_BYTES || raw.contains(&b'\\') || raw.contains(&0) {
        return Err(invalid("Unsafe archive entry path"));
    }
    // ZIP readers may normalize `C:\\foo` into `C:/foo` before this point.
    // Reject Windows drive prefixes even after that normalization.
    if raw.len() >= 2 && raw[0].is_ascii_alphabetic() && raw[1] == b':' {
        return Err(invalid("Archive entry contains a drive prefix"));
    }
    let mut output = PathBuf::new();
    let mut depth = 0;
    for component in Path::new(OsStr::from_bytes(raw)).components() {
        match component {
            Component::CurDir => {}
            Component::Normal(name) => {
                depth += 1;
                if depth > MAX_PATH_DEPTH {
                    return Err(invalid("Archive path too deep"));
                }
                output.push(name);
            }
            _ => return Err(invalid("Archive path attempts to escape the destination")),
        }
    }
    if output.as_os_str().is_empty() {
        return Err(invalid("Empty archive entry path"));
    }
    Ok(output)
}

fn parse_entry(api: &Api, raw: Ptr) -> io::Result<ArchiveEntry> {
    let name = unsafe { (api.entry_path)(raw) };
    if name.is_null() {
        return Err(invalid("Archive entry has no name"));
    }
    let path = safe_path(unsafe { CStr::from_ptr(name) }.to_bytes())?;
    if !unsafe { (api.entry_hardlink)(raw) }.is_null() {
        return Err(invalid("Archive hardlinks are disabled"));
    }
    let kind = unsafe { (api.entry_type)(raw) };
    if kind != REGULAR && kind != DIRECTORY {
        return Err(invalid("Archive contains a symlink or special file"));
    }
    let size = unsafe { (api.entry_size)(raw) };
    if size < 0 || size as u64 > MAX_ENTRY_BYTES {
        return Err(invalid("Archive entry exceeds the 512 MiB safety limit"));
    }
    Ok(ArchiveEntry {
        path,
        bytes: size as u64,
        directory: kind == DIRECTORY,
    })
}

fn scan(
    reader: &Reader<'_>,
    cancelled: &AtomicBool,
    mut on_entry: impl FnMut(&Reader<'_>, &ArchiveEntry) -> io::Result<()>,
) -> io::Result<Vec<ArchiveEntry>> {
    let mut entries = Vec::new();
    let mut seen = BTreeSet::new();
    let mut total = 0u64;
    while let Some(raw) = reader.next()? {
        check_cancel(cancelled)?;
        let entry = parse_entry(reader.api, raw)?;
        if entries.len() >= MAX_ENTRIES {
            return Err(invalid("Archive exceeds 20,000 entries"));
        }
        if !seen.insert(entry.path.clone()) {
            return Err(invalid("Archive contains duplicate entry paths"));
        }
        total = total
            .checked_add(entry.bytes)
            .ok_or_else(|| invalid("Archive size overflow"))?;
        if total > MAX_TOTAL_BYTES {
            return Err(invalid("Archive exceeds the 2 GiB safety limit"));
        }
        on_entry(reader, &entry)?;
        entries.push(entry);
    }
    Ok(entries)
}

#[cfg(test)]
pub fn inspect(path: &Path, cancelled: &AtomicBool) -> io::Result<Vec<ArchiveEntry>> {
    inspect_with_password(path, None, cancelled)
}

pub fn inspect_with_password(
    path: &Path,
    password: Option<&str>,
    cancelled: &AtomicBool,
) -> io::Result<Vec<ArchiveEntry>> {
    if !Format::is_archive(path) {
        return Err(invalid("Unsupported archive filename"));
    }
    let api = Api::load()?;
    let reader = Reader::open(&api, path, password)?;
    scan(&reader, cancelled, |_, _| Ok(()))
}

/// Read every regular file and ask libarchive to validate checksums/streams.
/// The operation does not create or modify any filesystem object.
#[cfg(test)]
pub fn verify(
    path: &Path,
    cancelled: &AtomicBool,
    mut progress: impl FnMut(ArchiveProgress),
) -> io::Result<usize> {
    verify_with_password(path, None, cancelled, &mut progress)
}

pub fn verify_with_password(
    path: &Path,
    password: Option<&str>,
    cancelled: &AtomicBool,
    mut progress: impl FnMut(ArchiveProgress),
) -> io::Result<usize> {
    let api = Api::load()?;
    let reader = Reader::open(&api, path, password)?;
    let mut total_read = 0u64;
    let entries = scan(&reader, cancelled, |reader, entry| {
        if entry.directory {
            return Ok(());
        }
        stream_entry(reader, entry, cancelled, &mut total_read, |data| {
            progress(ArchiveProgress {
                path: entry.path.clone(),
                bytes: data.len() as u64,
            });
            Ok(())
        })
    })?;
    Ok(entries.len())
}

fn stream_entry(
    reader: &Reader<'_>,
    entry: &ArchiveEntry,
    cancelled: &AtomicBool,
    total: &mut u64,
    mut output: impl FnMut(&[u8]) -> io::Result<()>,
) -> io::Result<()> {
    let mut size = 0u64;
    let mut buffer = [0u8; IO_BLOCK];
    loop {
        check_cancel(cancelled)?;
        let count = reader.data(&mut buffer)?;
        if count == 0 {
            break;
        }
        size = size
            .checked_add(count as u64)
            .ok_or_else(|| invalid("Size overflow"))?;
        *total = total
            .checked_add(count as u64)
            .ok_or_else(|| invalid("Size overflow"))?;
        if size > MAX_ENTRY_BYTES || *total > MAX_TOTAL_BYTES {
            return Err(invalid("Decompressed archive exceeds safety limits"));
        }
        output(&buffer[..count])?;
    }
    if size != entry.bytes {
        return Err(invalid("Archive entry size disagrees with decoded data"));
    }
    Ok(())
}

struct Stage(PathBuf);

impl Stage {
    fn new(parent: &Path) -> io::Result<Self> {
        let metadata = fs::symlink_metadata(parent)?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(invalid("Archive destination must be a real directory"));
        }
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        for index in 0..32 {
            let path = parent.join(format!(
                ".rice-explorer-archive-{}-{nanos}-{index}",
                std::process::id()
            ));
            match fs::DirBuilder::new().mode(0o700).create(&path) {
                Ok(()) => {
                    return Ok(Self(path));
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
        Err(invalid("Cannot create a unique archive staging directory"))
    }
}

impl Drop for Stage {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn publish_no_replace(from: &Path, to: &Path) -> io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        let source = os_cstring(from.as_os_str())?;
        let target = os_cstring(to.as_os_str())?;
        let result = unsafe {
            libc::syscall(
                libc::SYS_renameat2,
                libc::AT_FDCWD,
                source.as_ptr(),
                libc::AT_FDCWD,
                target.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };
        if result != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (from, to);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Linux renameat2 required",
        ))
    }
}

fn valid_output_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && name.len() <= 180
        && !name.contains(['/', '\\', '\0'])
        && !name.starts_with('.')
}

/// Extract only selected entry paths (directories also include descendants).
/// Passing an empty selection extracts the complete archive.
#[cfg(test)]
pub fn extract(
    path: &Path,
    destination: &Path,
    folder_name: &str,
    selected: &[PathBuf],
    cancelled: &AtomicBool,
    mut progress: impl FnMut(ArchiveProgress),
) -> io::Result<PathBuf> {
    extract_with_password(
        path,
        destination,
        folder_name,
        selected,
        None,
        cancelled,
        &mut progress,
    )
}

pub fn extract_with_password(
    path: &Path,
    destination: &Path,
    folder_name: &str,
    selected: &[PathBuf],
    password: Option<&str>,
    cancelled: &AtomicBool,
    mut progress: impl FnMut(ArchiveProgress),
) -> io::Result<PathBuf> {
    if !valid_output_name(folder_name) {
        return Err(invalid("Invalid extraction folder name"));
    }
    let selected = selected.iter().cloned().collect::<BTreeSet<_>>();
    // Read-only preflight rejects all unsafe paths before any output exists.
    let entries = inspect_with_password(path, password, cancelled)?;
    if !selected.is_empty()
        && !selected
            .iter()
            .all(|name| entries.iter().any(|e| &e.path == name))
    {
        return Err(invalid("Selection no longer matches the archive"));
    }
    let stage = Stage::new(destination)?;
    let root = stage.0.join("unpacked");
    fs::create_dir(&root)?;
    let api = Api::load()?;
    let reader = Reader::open(&api, path, password)?;
    let mut total_read = 0;
    scan(&reader, cancelled, |reader, entry| {
        // Selected directories include children, without an O(N²) scan when
        // the archive browser has thousands of checked entries.
        let included = selected.is_empty()
            || selected.contains(&entry.path)
            || entry
                .path
                .ancestors()
                .skip(1)
                .any(|ancestor| selected.contains(ancestor));
        if !included {
            return Ok(());
        }
        let target = root.join(&entry.path);
        if entry.directory {
            fs::create_dir_all(&target)?;
        } else {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&target)?;
            stream_entry(reader, entry, cancelled, &mut total_read, |bytes| {
                file.write_all(bytes)?;
                progress(ArchiveProgress {
                    path: entry.path.clone(),
                    bytes: bytes.len() as u64,
                });
                Ok(())
            })?;
            file.sync_all()?;
        }
        Ok(())
    })?;
    check_cancel(cancelled)?;
    let output = destination.join(folder_name);
    publish_no_replace(&root, &output)?;
    Ok(output)
}

#[cfg(test)]
pub fn create(
    sources: &[PathBuf],
    destination: &Path,
    stem: &str,
    format: Format,
    cancelled: &AtomicBool,
    mut progress: impl FnMut(ArchiveProgress),
) -> io::Result<PathBuf> {
    create_with_password(
        sources,
        destination,
        stem,
        format,
        None,
        cancelled,
        &mut progress,
    )
}

pub fn create_with_password(
    sources: &[PathBuf],
    destination: &Path,
    stem: &str,
    format: Format,
    password: Option<&str>,
    cancelled: &AtomicBool,
    mut progress: impl FnMut(ArchiveProgress),
) -> io::Result<PathBuf> {
    if sources.is_empty() || !valid_output_name(stem) {
        return Err(invalid("Select files and a safe archive name"));
    }
    // Reject repeated selection roots before trying to archive them.
    let mut names = BTreeSet::new();
    for source in sources {
        if fs::symlink_metadata(source)?.is_dir() && destination.starts_with(source) {
            return Err(invalid("Cannot archive a directory into itself"));
        }
        if !names.insert(
            source
                .file_name()
                .ok_or_else(|| invalid("Invalid source"))?
                .to_os_string(),
        ) {
            return Err(invalid("Selected sources contain duplicate names"));
        }
    }
    let stage = Stage::new(destination)?;
    let filename = format!("{stem}.{}", format.extension());
    let temporary = stage.0.join(&filename);
    let api = Api::load()?;
    let writer = Writer::open(&api, &temporary, format, password)?;
    let mut count = 0usize;
    let mut total = 0u64;
    for source in sources {
        write_path(
            &writer,
            source,
            &PathBuf::from(source.file_name().unwrap()),
            cancelled,
            &mut count,
            &mut total,
            &mut progress,
        )?;
    }
    check_cancel(cancelled)?;
    writer.finish()?;
    let output = destination.join(filename);
    publish_no_replace(&temporary, &output)?;
    Ok(output)
}

fn write_path(
    writer: &Writer<'_>,
    source: &Path,
    relative: &Path,
    cancelled: &AtomicBool,
    count: &mut usize,
    total: &mut u64,
    progress: &mut impl FnMut(ArchiveProgress),
) -> io::Result<()> {
    check_cancel(cancelled)?;
    safe_path(relative.as_os_str().as_bytes())?;
    *count += 1;
    if *count > MAX_ENTRIES {
        return Err(invalid("Archive source exceeds 20,000 entries"));
    }
    let metadata = fs::symlink_metadata(source)?;
    if metadata.file_type().is_symlink() {
        return Err(invalid("Archiving symlinks is disabled"));
    }
    if metadata.is_dir() {
        writer.add(relative, None, 0, cancelled, progress)?;
        for child in fs::read_dir(source)? {
            let child = child?;
            write_path(
                writer,
                &child.path(),
                &relative.join(child.file_name()),
                cancelled,
                count,
                total,
                progress,
            )?;
        }
    } else if metadata.is_file() {
        if metadata.len() > MAX_ENTRY_BYTES {
            return Err(invalid("Source exceeds 512 MiB limit"));
        }
        *total = total
            .checked_add(metadata.len())
            .ok_or_else(|| invalid("Size overflow"))?;
        if *total > MAX_TOTAL_BYTES {
            return Err(invalid("Archive input exceeds 2 GiB limit"));
        }
        let mut file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(source)?;
        if !file.metadata()?.is_file() {
            return Err(invalid("Non-regular source file"));
        }
        writer.add(
            relative,
            Some(&mut file),
            metadata.len(),
            cancelled,
            progress,
        )?;
    } else {
        return Err(invalid(
            "Device files, sockets and pipes cannot be archived",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let id = SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let p =
                std::env::temp_dir().join(format!("rice-archive-test-{}-{id}", std::process::id()));
            fs::create_dir(&p).unwrap();
            Self(p)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn rejects_archive_escape_paths() {
        for raw in [
            b"../evil".as_slice(),
            b"/tmp/evil",
            b"folder/../../evil",
            b"C:\\evil",
            b"C:/evil",
            b".",
        ] {
            assert!(safe_path(raw).is_err(), "accepted {raw:?}");
        }
        assert_eq!(
            safe_path(b"./nested/file.txt").unwrap(),
            PathBuf::from("nested/file.txt")
        );
    }

    #[test]
    fn all_formats_roundtrip_and_selected_extraction() {
        let fixture = Fixture::new();
        let input = fixture.0.join("hello.txt");
        fs::write(&input, "archive content\n").unwrap();
        let cancel = AtomicBool::new(false);
        for format in Format::ALL {
            let stem = format!("bundle-{}", format.extension().replace('.', "-"));
            let path = create(
                std::slice::from_ref(&input),
                &fixture.0,
                &stem,
                format,
                &cancel,
                |_| {},
            )
            .unwrap();
            let entries = inspect(&path, &cancel).unwrap();
            assert_eq!(entries.len(), 1, "{format:?}");
            assert_eq!(entries[0].path, PathBuf::from("hello.txt"));
            assert_eq!(verify(&path, &cancel, |_| {}).unwrap(), 1);
            let target = extract(
                &path,
                &fixture.0,
                &stem,
                &[PathBuf::from("hello.txt")],
                &cancel,
                |_| {},
            )
            .unwrap();
            assert_eq!(
                fs::read_to_string(target.join("hello.txt")).unwrap(),
                "archive content\n"
            );
        }
    }

    #[test]
    fn existing_destination_and_cancel_leave_no_new_files() {
        let fixture = Fixture::new();
        let input = fixture.0.join("input.txt");
        fs::write(&input, "hello").unwrap();
        let cancel = AtomicBool::new(false);
        let archive = create(&[input], &fixture.0, "bundle", Format::Zip, &cancel, |_| {}).unwrap();
        fs::create_dir(fixture.0.join("unpacked")).unwrap();
        assert!(extract(&archive, &fixture.0, "unpacked", &[], &cancel, |_| {}).is_err());
        assert!(
            fs::read_dir(fixture.0.join("unpacked"))
                .unwrap()
                .next()
                .is_none()
        );
        cancel.store(true, Ordering::Relaxed);
        assert_eq!(
            verify(&archive, &cancel, |_| {}).unwrap_err().kind(),
            io::ErrorKind::Interrupted
        );
        assert!(!fixture.0.join("cancelled").exists());
    }

    #[test]
    fn malicious_archive_path_is_never_extracted() {
        let fixture = Fixture::new();
        let payload = fixture.0.join("payload.txt");
        fs::write(&payload, "must not escape").unwrap();
        let path = fixture.0.join("hostile.zip");
        let api = Api::load().unwrap();
        let writer = Writer::open(&api, &path, Format::Zip, None).unwrap();
        let cancel = AtomicBool::new(false);
        writer
            .add(
                Path::new("../outside.txt"),
                Some(&mut File::open(&payload).unwrap()),
                15,
                &cancel,
                &mut |_| {},
            )
            .unwrap();
        writer.finish().unwrap();
        assert!(inspect(&path, &cancel).is_err());
        assert!(extract(&path, &fixture.0, "unpacked", &[], &cancel, |_| {}).is_err());
        assert!(!fixture.0.join("outside.txt").exists());
        assert!(!fixture.0.join("unpacked").exists());
    }

    #[test]
    fn symlink_sources_are_rejected_without_publishing_archive() {
        use std::os::unix::fs::symlink;
        let fixture = Fixture::new();
        let original = fixture.0.join("original.txt");
        let link = fixture.0.join("linked.txt");
        fs::write(&original, "hello").unwrap();
        symlink(&original, &link).unwrap();
        let cancel = AtomicBool::new(false);
        assert!(create(&[link], &fixture.0, "linked", Format::Zip, &cancel, |_| {}).is_err());
        assert!(!fixture.0.join("linked.zip").exists());
    }

    #[test]
    fn selected_directory_extraction_omits_unselected_files() {
        let fixture = Fixture::new();
        let folder = fixture.0.join("folder");
        fs::create_dir(&folder).unwrap();
        fs::write(folder.join("inside.txt"), "inside").unwrap();
        let extra = fixture.0.join("outside.txt");
        fs::write(&extra, "outside").unwrap();
        let cancel = AtomicBool::new(false);
        let archive = create(
            &[folder, extra],
            &fixture.0,
            "selected",
            Format::TarGz,
            &cancel,
            |_| {},
        )
        .unwrap();
        let out = extract(
            &archive,
            &fixture.0,
            "selected-unpacked",
            &[PathBuf::from("folder")],
            &cancel,
            |_| {},
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(out.join("folder/inside.txt")).unwrap(),
            "inside"
        );
        assert!(!out.join("outside.txt").exists());
    }

    #[test]
    fn cancellation_discards_staged_compression() {
        let fixture = Fixture::new();
        let source = fixture.0.join("large.txt");
        fs::write(&source, vec![b'x'; IO_BLOCK * 3]).unwrap();
        let cancel = AtomicBool::new(false);
        let result = create(
            &[source],
            &fixture.0,
            "cancelled",
            Format::Zip,
            &cancel,
            |_| {
                cancel.store(true, Ordering::Relaxed);
            },
        );
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::Interrupted);
        assert!(!fixture.0.join("cancelled.zip").exists());
        assert!(!fs::read_dir(&fixture.0).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".rice-explorer-archive-")
        }));
    }

    #[test]
    fn encrypted_zip_requires_correct_password_and_never_publishes_on_failure() {
        let fixture = Fixture::new();
        let source = fixture.0.join("secret.txt");
        fs::write(&source, "private archive payload").unwrap();
        let cancel = AtomicBool::new(false);
        let path = create_with_password(
            &[source],
            &fixture.0,
            "locked",
            Format::Zip,
            Some("correct horse"),
            &cancel,
            |_| {},
        )
        .unwrap();
        // ZIP member names may be visible even when their data is encrypted.
        assert_eq!(inspect(&path, &cancel).unwrap().len(), 1);
        assert!(verify(&path, &cancel, |_| {}).is_err());
        assert!(verify_with_password(&path, Some("incorrect"), &cancel, |_| {}).is_err());
        assert_eq!(
            verify_with_password(&path, Some("correct horse"), &cancel, |_| {}).unwrap(),
            1
        );
        for password in [None, Some("incorrect")] {
            assert!(
                extract_with_password(
                    &path,
                    &fixture.0,
                    "no-output",
                    &[],
                    password,
                    &cancel,
                    |_| {},
                )
                .is_err()
            );
            assert!(!fixture.0.join("no-output").exists());
        }
        let output = extract_with_password(
            &path,
            &fixture.0,
            "unlocked",
            &[],
            Some("correct horse"),
            &cancel,
            |_| {},
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(output.join("secret.txt")).unwrap(),
            "private archive payload"
        );
        assert!(!fs::read_dir(&fixture.0).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".rice-explorer-archive-")
        }));
    }

    #[test]
    fn password_validation_and_unsupported_encrypted_creation_fail_closed() {
        assert!(validate_password("").is_err());
        assert!(validate_password("a\0b").is_err());
        assert!(validate_password(&"a".repeat(MAX_PASSWORD_BYTES + 1)).is_err());
        let fixture = Fixture::new();
        let source = fixture.0.join("source.txt");
        fs::write(&source, "data").unwrap();
        let cancel = AtomicBool::new(false);
        assert!(
            create_with_password(
                &[source],
                &fixture.0,
                "unsupported",
                Format::SevenZip,
                Some("secret"),
                &cancel,
                |_| {},
            )
            .is_err()
        );
        assert!(!fixture.0.join("unsupported.7z").exists());
    }

    #[test]
    fn duplicate_archive_entries_are_rejected_before_extraction() {
        let fixture = Fixture::new();
        let payload = fixture.0.join("payload");
        fs::write(&payload, "content").unwrap();
        let archive = fixture.0.join("duplicate.zip");
        let api = Api::load().unwrap();
        let writer = Writer::open(&api, &archive, Format::Zip, None).unwrap();
        let cancel = AtomicBool::new(false);
        for _ in 0..2 {
            writer
                .add(
                    Path::new("same.txt"),
                    Some(&mut File::open(&payload).unwrap()),
                    7,
                    &cancel,
                    &mut |_| {},
                )
                .unwrap();
        }
        writer.finish().unwrap();
        assert!(inspect(&archive, &cancel).is_err());
        assert!(extract(&archive, &fixture.0, "no-output", &[], &cancel, |_| {}).is_err());
        assert!(!fixture.0.join("no-output").exists());
    }

    #[test]
    fn truncated_archive_cannot_publish_any_extracted_files() {
        let fixture = Fixture::new();
        let source = fixture.0.join("source.txt");
        // Use a substantially incompressible payload and cut through its
        // actual data stream. Some libarchive versions can validly recover
        // local ZIP members after a damaged central-directory trailer; a
        // partial compressed member must fail on every supported backend.
        let mut state = 0x1234_5678u32;
        let payload = (0..IO_BLOCK * 4)
            .map(|_| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (state >> 24) as u8
            })
            .collect::<Vec<_>>();
        fs::write(&source, &payload).unwrap();
        let cancel = AtomicBool::new(false);
        let archive = create(
            &[source],
            &fixture.0,
            "truncated",
            Format::Zip,
            &cancel,
            |_| {},
        )
        .unwrap();
        let file = OpenOptions::new().write(true).open(&archive).unwrap();
        let original = file.metadata().unwrap().len();
        assert!(original > payload.len() as u64 / 2);
        file.set_len(payload.len() as u64 / 2).unwrap();
        assert!(verify(&archive, &cancel, |_| {}).is_err());
        assert!(extract(&archive, &fixture.0, "unpacked", &[], &cancel, |_| {}).is_err());
        assert!(!fixture.0.join("unpacked").exists());
    }

    #[test]
    fn cancellation_during_extraction_cleans_private_stage() {
        let fixture = Fixture::new();
        let source = fixture.0.join("large.dat");
        fs::write(&source, vec![b'q'; IO_BLOCK * 4]).unwrap();
        let cancel = AtomicBool::new(false);
        let archive = create(&[source], &fixture.0, "large", Format::Zip, &cancel, |_| {}).unwrap();
        let result = extract(&archive, &fixture.0, "unpacked", &[], &cancel, |_| {
            cancel.store(true, Ordering::Relaxed);
        });
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::Interrupted);
        assert!(!fixture.0.join("unpacked").exists());
        assert!(!fs::read_dir(&fixture.0).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".rice-explorer-archive-")
        }));
    }

    #[test]
    fn hostile_tar_symlinks_and_device_nodes_are_rejected() {
        // Write deliberately unsafe entries via libarchive's low-level test
        // path, bypassing the production Writer::add regular-file constraint.
        for (kind, name) in [(0o120000, "linked"), (0o020000, "device")] {
            let fixture = Fixture::new();
            let archive = fixture.0.join("hostile.tar.gz");
            let api = Api::load().unwrap();
            let writer = Writer::open(&api, &archive, Format::TarGz, None).unwrap();
            let entry = unsafe { (api.entry_new)() };
            assert!(!entry.is_null());
            let name = CString::new(name).unwrap();
            unsafe {
                (api.entry_set_path)(entry, name.as_ptr());
                (api.entry_set_type)(entry, kind);
                (api.entry_set_size)(entry, 0);
                (api.entry_set_perm)(entry, 0o777);
            }
            let result = api.check(writer.handle, unsafe {
                (api.write_header)(writer.handle, entry)
            });
            unsafe { (api.entry_free)(entry) };
            result.unwrap();
            writer.finish().unwrap();
            let cancel = AtomicBool::new(false);
            assert!(inspect(&archive, &cancel).is_err(), "accepted {name:?}");
            assert!(extract(&archive, &fixture.0, "unpacked", &[], &cancel, |_| {}).is_err());
            assert!(!fixture.0.join("unpacked").exists());
        }
    }

    #[test]
    fn hostile_zip_names_cannot_escape_or_alias_another_path() {
        for name in [
            "/absolute.txt",
            "dir/../../escape.txt",
            "folder\\escape.txt",
            "C:\\drive.txt",
            "./../escape.txt",
        ] {
            let fixture = Fixture::new();
            let archive = fixture.0.join("hostile.zip");
            let api = Api::load().unwrap();
            let writer = Writer::open(&api, &archive, Format::Zip, None).unwrap();
            let cancel = AtomicBool::new(false);
            writer
                .add(
                    Path::new(name),
                    Some(
                        &mut File::open({
                            let source = fixture.0.join("payload");
                            fs::write(&source, b"X").unwrap();
                            source
                        })
                        .unwrap(),
                    ),
                    1,
                    &cancel,
                    &mut |_| {},
                )
                .unwrap();
            writer.finish().unwrap();
            if name == "folder\\escape.txt" {
                // libarchive canonicalizes Windows separators in ZIP names.
                // Confirm the resulting relative path remains confined to the
                // newly created extraction directory.
                let entries = inspect(&archive, &cancel).unwrap();
                assert_eq!(entries[0].path, PathBuf::from("folder/escape.txt"));
                let output =
                    extract(&archive, &fixture.0, "unpacked", &[], &cancel, |_| {}).unwrap();
                assert_eq!(fs::read(output.join("folder/escape.txt")).unwrap(), b"X");
                assert!(!fixture.0.join("escape.txt").exists());
            } else {
                assert!(inspect(&archive, &cancel).is_err(), "accepted {name:?}");
                assert!(extract(&archive, &fixture.0, "unpacked", &[], &cancel, |_| {}).is_err());
                assert!(!fixture.0.join("unpacked").exists());
            }
        }
    }

    #[test]
    fn forged_zip_inflated_size_is_rejected_at_preflight() {
        let fixture = Fixture::new();
        let source = fixture.0.join("small.txt");
        fs::write(&source, b"small").unwrap();
        let cancel = AtomicBool::new(false);
        let archive = create(
            &[source],
            &fixture.0,
            "forged",
            Format::Zip,
            &cancel,
            |_| {},
        )
        .unwrap();
        let mut bytes = fs::read(&archive).unwrap();
        // PK\x01\x02 is the ZIP central directory header; its uncompressed
        // 32-bit member size sits at byte offset 24. Claim 4 GiB, not 5 B.
        let central = bytes
            .windows(4)
            .position(|part| part == b"PK\x01\x02")
            .unwrap();
        bytes[central + 24..central + 28].copy_from_slice(&u32::MAX.to_le_bytes());
        fs::write(&archive, bytes).unwrap();
        assert!(inspect(&archive, &cancel).is_err());
        assert!(extract(&archive, &fixture.0, "unpacked", &[], &cancel, |_| {}).is_err());
        assert!(!fixture.0.join("unpacked").exists());
    }
}
