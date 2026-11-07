//! Bounded, off-thread previews. Never decode arbitrarily large images in the UI thread.
use std::{
    fmt::Write as _,
    fs,
    io::{self, Cursor, Read},
    path::Path,
    process::Command,
    sync::atomic::AtomicBool,
    time::Duration,
};

const MAX_IMAGE_BYTES: u64 = 32 * 1024 * 1024;
const MAX_IMAGE_PIXELS: u64 = 16_000_000;
const MAX_THUMBNAIL_BYTES: u64 = 12 * 1024 * 1024;
const MAX_PDF_BYTES: u64 = 64 * 1024 * 1024;
const MAX_MEDIA_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const MAX_HEX_PREVIEW_BYTES: usize = 256;

pub struct Thumbnail {
    pub rgba: Vec<u8>,
    pub width: usize,
    pub height: usize,
}

pub enum Preview {
    Image {
        rgba: Vec<u8>,
        width: usize,
        height: usize,
    },
    Text(String),
    Info(String),
}

fn image_extension(extension: &str) -> bool {
    matches!(extension, "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico")
}

fn decode_image(bytes: &[u8], max_width: u32, max_height: u32) -> io::Result<Thumbnail> {
    let dimensions = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()?
        .into_dimensions()
        .map_err(io::Error::other)?;
    if u64::from(dimensions.0) * u64::from(dimensions.1) > MAX_IMAGE_PIXELS {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Image exceeds the 16 MP safety limit",
        ));
    }
    let image = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()?
        .decode()
        .map_err(io::Error::other)?
        .thumbnail(dimensions.0.min(max_width), dimensions.1.min(max_height))
        .to_rgba8();
    let (width, height) = image.dimensions();
    Ok(Thumbnail {
        rgba: image.into_raw(),
        width: width as usize,
        height: height as usize,
    })
}

/// Generate tiny in-memory thumbnails from an already-open regular file.
/// The caller schedules only visible cells and bounds the resident cache.
pub fn thumbnail(path: &Path) -> io::Result<Option<Thumbnail>> {
    let extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !image_extension(&extension) {
        return Ok(None);
    }
    // Reject sockets, devices and oversized symlink targets before opening.
    // The post-open check below still protects against a path replacement.
    let candidate = fs::metadata(path)?;
    if !candidate.is_file() || candidate.len() > MAX_THUMBNAIL_BYTES {
        return Ok(None);
    }
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let mut file = options.open(path)?;
    let opened = file.metadata()?;
    if !opened.is_file() || opened.len() > MAX_THUMBNAIL_BYTES {
        return Ok(None);
    }
    let mut bytes = Vec::new();
    (&mut file)
        .take(MAX_THUMBNAIL_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_THUMBNAIL_BYTES {
        return Ok(None);
    }
    decode_image(&bytes, 108, 108).map(Some)
}

fn bounded_external(
    command: Command,
    bytes: u64,
    seconds: u64,
    label: &str,
) -> io::Result<Vec<u8>> {
    crate::volume::capture_gio(
        command,
        bytes,
        Duration::from_secs(seconds),
        &AtomicBool::new(false),
        label,
    )
    .map_err(io::Error::other)
}

fn pdf_preview(path: &Path) -> io::Result<Preview> {
    let mut command = Command::new("pdftoppm");
    // Without an output prefix pdftoppm writes the first page to stdout;
    // the worker never creates a temporary PDF or output image on disk.
    command.args([
        "-f",
        "1",
        "-l",
        "1",
        "-singlefile",
        "-scale-to",
        "900",
        "-png",
        "-q",
    ]);
    command.arg(path);
    let png = bounded_external(command, 12 * 1024 * 1024, 8, "PDF preview")?;
    let image = decode_image(&png, 900, 900)?;
    Ok(Preview::Image {
        rgba: image.rgba,
        width: image.width,
        height: image.height,
    })
}

fn media_preview(path: &Path) -> io::Result<Preview> {
    let mut command = Command::new("ffprobe");
    command.args([
        "-v",
        "error",
        "-show_entries",
        "format=duration,format_name:stream=codec_type,codec_name,width,height,channels",
        "-of",
        "default=noprint_wrappers=1",
    ]);
    command.arg(path);
    let output = bounded_external(command, 16 * 1024, 7, "Media metadata")?;
    let details = std::str::from_utf8(&output).map_err(io::Error::other)?;
    let safe = details
        .lines()
        .filter(|line| line.len() <= 180 && line.chars().all(|ch| !ch.is_control()))
        .take(25)
        .collect::<Vec<_>>()
        .join("\n");
    if safe.is_empty() {
        Ok(Preview::Info(
            "No media metadata available; use Open to play in your default app".into(),
        ))
    } else {
        Ok(Preview::Info(format!(
            "Media details (read-only):\n{safe}\n\nUse Open for playback."
        )))
    }
}

fn archive_preview(path: &Path) -> io::Result<Preview> {
    let entries = crate::archive::inspect_with_password(path, None, &AtomicBool::new(false))?;
    let total = entries.len();
    let sample = entries
        .into_iter()
        .take(35)
        .map(|item| {
            format!(
                "{} {}",
                if item.directory { "[Folder]" } else { "[File]" },
                item.path.display()
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    Ok(Preview::Info(format!(
        "Archive · {total} entries\n{sample}{}\n\nUse Archive → Browse / extract for more.",
        if total > 35 { "\n…" } else { "" }
    )))
}

/// Inspect only a small prefix. Untrusted binary content is never interpreted
/// as markup or escape sequences and cannot make the UI allocate unbounded text.
pub(crate) fn binary_hex_text(bytes: &[u8], total_size: Option<u64>) -> String {
    let sample = &bytes[..bytes.len().min(MAX_HEX_PREVIEW_BYTES)];
    let total = total_size.map_or_else(
        || "unknown total size".into(),
        |size| format!("{size} bytes"),
    );
    let mut output = format!(
        "Binary file · first {} of {total} (hex + ASCII, read-only)\n\n",
        sample.len()
    );
    for (row, chunk) in sample.chunks(16).enumerate() {
        write!(&mut output, "{:08x}  ", row * 16).unwrap();
        for column in 0..16 {
            if let Some(byte) = chunk.get(column) {
                write!(&mut output, "{byte:02x} ").unwrap();
            } else {
                output.push_str("   ");
            }
        }
        output.push(' ');
        for byte in chunk {
            output.push(if (0x20..=0x7e).contains(byte) {
                char::from(*byte)
            } else {
                '.'
            });
        }
        output.push('\n');
    }
    if total_size.is_none_or(|size| size > sample.len() as u64) {
        output.push_str("\nPreview truncated; use a hex editor for the full file.");
    }
    output
}

fn binary_preview(bytes: &[u8], size: u64) -> Preview {
    Preview::Text(binary_hex_text(bytes, Some(size)))
}

pub fn load(path: &Path) -> io::Result<Preview> {
    // Follow links for *target* size/type checks. lstat reports only the
    // symlink's tiny path length and could bypass the image byte limit.
    let metadata = fs::metadata(path)?;
    if metadata.is_dir() {
        return Ok(Preview::Info("Folder — open to browse its contents".into()));
    }
    if !metadata.is_file() {
        return Ok(Preview::Info(
            "Special file — preview disabled to avoid blocking on a device or pipe".into(),
        ));
    }
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // A regular path swapped to a FIFO after stat must not block a worker.
        options.custom_flags(libc::O_NONBLOCK);
    }
    let mut file = options.open(path)?;
    let opened = file.metadata()?;
    if !opened.is_file() {
        return Ok(Preview::Info(
            "Special file — preview disabled to avoid blocking on a device or pipe".into(),
        ));
    }
    let extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if image_extension(&extension) {
        if opened.len() > MAX_IMAGE_BYTES {
            return Ok(Preview::Info(
                "Image too large for in-app preview (32 MiB limit)".into(),
            ));
        }
        // Read the same open descriptor just once with an absolute byte cap.
        // Decoding from memory avoids a second path-open race (e.g. a FIFO
        // substituted between dimension detection and image::open).
        let mut bytes = Vec::new();
        (&mut file)
            .take(MAX_IMAGE_BYTES + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_IMAGE_BYTES {
            return Ok(Preview::Info(
                "Image too large for in-app preview (32 MiB limit)".into(),
            ));
        }
        return match decode_image(&bytes, 900, 800) {
            Ok(image) => Ok(Preview::Image {
                rgba: image.rgba,
                width: image.width,
                height: image.height,
            }),
            Err(error) if error.to_string().contains("16 MP") => Ok(Preview::Info(
                "Image dimensions exceed preview limit (16 MP)".into(),
            )),
            Err(error) => Ok(Preview::Info(format!(
                "Image preview unavailable: {error}. Open the file with an image viewer."
            ))),
        };
    }
    if extension == "pdf" {
        if opened.len() > MAX_PDF_BYTES {
            return Ok(Preview::Info(
                "PDF too large for the 64 MiB preview limit".into(),
            ));
        }
        return Ok(pdf_preview(path).unwrap_or_else(|error| {
            Preview::Info(format!(
                "PDF rendering unavailable: {error}. Open the document with a PDF viewer (pdftoppm is optional)."
            ))
        }));
    }
    if matches!(
        extension.as_str(),
        "mp3" | "flac" | "wav" | "ogg" | "m4a" | "mkv" | "mp4" | "avi" | "webm" | "mov"
    ) {
        if opened.len() > MAX_MEDIA_BYTES {
            return Ok(Preview::Info(
                "Media exceeds the 4 GiB metadata probe limit".into(),
            ));
        }
        return Ok(media_preview(path).unwrap_or_else(|error| {
            Preview::Info(format!(
                "Media metadata unavailable: {error}. Open the file with a media player (ffprobe is optional)."
            ))
        }));
    }
    if crate::archive::Format::is_archive(path) {
        if opened.len() > 64 * 1024 * 1024 {
            return Ok(Preview::Info(
                "Large archive — use the bounded Archive browser to inspect entries".into(),
            ));
        }
        return Ok(archive_preview(path).unwrap_or_else(|error| {
            Preview::Info(format!(
                "Archive listing unavailable: {error}. Open it with an archive manager to inspect the contents."
            ))
        }));
    }
    let mut bytes = vec![0u8; 32 * 1024];
    let count = file.read(&mut bytes)?;
    bytes.truncate(count);
    if std::str::from_utf8(&bytes).is_err()
        || bytes.contains(&0)
        || bytes
            .iter()
            .any(|b| (*b < 0x20 && !matches!(*b, b'\n' | b'\r' | b'\t')) || *b == 0x7f)
    {
        return Ok(binary_preview(&bytes, opened.len()));
    }
    let text = String::from_utf8(bytes).expect("validated UTF-8");
    if text.is_empty() {
        Ok(Preview::Info("Empty file".into()))
    } else {
        Ok(Preview::Text(text))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[cfg(unix)]
    fn special_file_fixture(path: &Path) -> Option<std::os::unix::net::UnixListener> {
        use std::{ffi::CString, os::unix::ffi::OsStrExt, os::unix::net::UnixListener};

        match UnixListener::bind(path) {
            Ok(listener) => Some(listener),
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
                // Exercise the same special-file refusal with a FIFO when
                // the execution environment blocks AF_UNIX socket creation.
                let name = CString::new(path.as_os_str().as_bytes()).unwrap();
                assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
                None
            }
            Err(error) => panic!("Cannot create special-file test fixture: {error}"),
        }
    }

    fn test_root() -> std::path::PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("rice-preview-test-{}-{nonce}", std::process::id()));
        fs::create_dir(&root).unwrap();
        root
    }

    #[test]
    fn recognizes_text_and_binary_without_loading_whole_file() {
        let root = test_root();
        let text = root.join("hello.txt");
        fs::write(&text, b"hello world").unwrap();
        assert!(matches!(load(&text).unwrap(), Preview::Text(ref value) if value == "hello world"));
        let binary = root.join("binary.dat");
        fs::write(&binary, [0, 1, 2, 3]).unwrap();
        assert!(matches!(load(&binary).unwrap(), Preview::Text(ref text)
            if text.contains("00 01 02 03") && text.contains("00000000")));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn binary_hex_preview_is_bounded_and_never_renders_control_sequences() {
        let root = test_root();
        let path = root.join("untrusted.bin");
        let mut bytes = vec![0x80; 1024];
        bytes[..8].copy_from_slice(b"\x1b[31m\0\xff\x80");
        fs::write(&path, &bytes).unwrap();
        let Preview::Text(text) = load(&path).unwrap() else {
            panic!("binary bytes must receive a hex preview");
        };
        assert!(text.contains("1b 5b 33 31 6d 00 ff 80"));
        assert!(text.contains("first 256 of 1024 bytes"));
        assert!(!text.contains('\x1b'));
        assert!(text.len() < 2000);
        assert_eq!(fs::read(&path).unwrap(), bytes);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn bounded_image_reader_decodes_a_small_png() {
        let root = test_root();
        let path = root.join("icon.png");
        fs::write(&path, include_bytes!("../assets/ricefield.png")).unwrap();
        assert!(matches!(
            load(&path).unwrap(),
            Preview::Image {
                width: 256,
                height: 256,
                ..
            }
        ));
        let thumb = thumbnail(&path).unwrap().unwrap();
        assert!(thumb.width <= 108 && thumb.height <= 108);
        assert_eq!(thumb.rgba.len(), thumb.width * thumb.height * 4);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn common_raster_extensions_use_the_image_decoder() {
        assert!(image_extension("bmp"));
        assert!(image_extension("ico"));
        assert!(!image_extension("svg")); // Render untrusted SVG as safe text.
        let root = test_root();
        let path = root.join("picture.bmp");
        fs::write(&path, include_bytes!("../assets/ricefield.png")).unwrap();
        assert!(matches!(load(&path).unwrap(), Preview::Image { .. }));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn oversized_thumbnail_is_skipped_without_decoding() {
        let root = test_root();
        let path = root.join("oversized.jpg");
        fs::File::create(&path)
            .unwrap()
            .set_len(MAX_THUMBNAIL_BYTES + 1)
            .unwrap();
        assert!(thumbnail(&path).unwrap().is_none());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn malformed_images_never_panic_in_the_decoder() {
        let valid = include_bytes!("../assets/ricefield.png");
        for length in [0, 1, 8, 24, 32, 128, 1024] {
            let _ = std::panic::catch_unwind(|| decode_image(&valid[..length], 108, 108))
                .expect("a truncated image must not crash the preview worker");
        }
        for offset in [0, 1, 8, 16, 20, 24, 29, 64, 128] {
            let mut corrupted = valid.to_vec();
            corrupted[offset] ^= 0xff;
            let _ = std::panic::catch_unwind(|| decode_image(&corrupted, 108, 108))
                .expect("a corrupt image must not crash the preview worker");
        }
    }

    #[cfg(unix)]
    #[test]
    fn thumbnails_reject_special_files_and_oversized_symlink_targets() {
        use std::os::unix::fs::symlink;

        let root = test_root();
        let socket = root.join("socket.png");
        let _listener = special_file_fixture(&socket);
        assert!(thumbnail(&socket).unwrap().is_none());

        let huge = root.join("huge.dat");
        fs::File::create(&huge)
            .unwrap()
            .set_len(MAX_THUMBNAIL_BYTES + 1)
            .unwrap();
        let link = root.join("linked.png");
        symlink(&huge, &link).unwrap();
        assert!(thumbnail(&link).unwrap().is_none());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_control_characters_in_apparently_text_files() {
        let root = test_root();
        let path = root.join("escape.txt");
        fs::write(&path, b"hi\x1b[31m").unwrap();
        assert!(matches!(load(&path).unwrap(), Preview::Text(ref text)
            if text.contains("68 69 1b 5b 33 31 6d") && !text.contains('\x1b')));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn oversized_pdf_and_media_do_not_invoke_external_decoders() {
        let root = test_root();
        let pdf = root.join("oversized.pdf");
        fs::File::create(&pdf)
            .unwrap()
            .set_len(MAX_PDF_BYTES + 1)
            .unwrap();
        assert!(matches!(load(&pdf).unwrap(), Preview::Info(text) if text.contains("64 MiB")));
        let video = root.join("oversized.mkv");
        fs::File::create(&video)
            .unwrap()
            .set_len(MAX_MEDIA_BYTES + 1)
            .unwrap();
        assert!(matches!(load(&video).unwrap(), Preview::Info(text) if text.contains("4 GiB")));
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn symlink_to_oversized_image_is_rejected_before_decode() {
        use std::os::unix::fs::symlink;

        let root = test_root();
        let large = root.join("large.png");
        fs::File::create(&large)
            .unwrap()
            .set_len(33 * 1024 * 1024)
            .unwrap();
        let link = root.join("preview.png");
        symlink(&large, &link).unwrap();
        assert!(matches!(
            load(&link).unwrap(),
            Preview::Info(message) if message.contains("32 MiB")
        ));
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn special_file_is_not_opened_for_preview() {
        let root = test_root();
        let path = root.join("socket.txt");
        let _listener = special_file_fixture(&path);
        assert!(matches!(
            load(&path).unwrap(),
            Preview::Info(message) if message.contains("Special file")
        ));
        fs::remove_dir_all(root).unwrap();
    }
}
