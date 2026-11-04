//! Bounded, off-thread previews. Never decode arbitrarily large images in the UI thread.
use std::{
    fs,
    io::{self, Cursor, Read},
    path::Path,
};

const MAX_IMAGE_BYTES: u64 = 32 * 1024 * 1024;
const MAX_IMAGE_PIXELS: u64 = 16_000_000;

pub enum Preview {
    Image {
        rgba: Vec<u8>,
        width: usize,
        height: usize,
    },
    Text(String),
    Info(String),
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
    if matches!(extension.as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp") {
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
        let dimensions = image::ImageReader::new(Cursor::new(bytes.as_slice()))
            .with_guessed_format()?
            .into_dimensions()
            .map_err(io::Error::other)?;
        if u64::from(dimensions.0) * u64::from(dimensions.1) > MAX_IMAGE_PIXELS {
            return Ok(Preview::Info(
                "Image dimensions exceed preview limit (16 MP)".into(),
            ));
        }
        let image = image::ImageReader::new(Cursor::new(bytes.as_slice()))
            .with_guessed_format()?
            .decode()
            .map_err(io::Error::other)?
            // Never upscale tiny images just to fit a larger preview pane.
            .thumbnail(dimensions.0.min(900), dimensions.1.min(800))
            .to_rgba8();
        let (width, height) = image.dimensions();
        return Ok(Preview::Image {
            rgba: image.into_raw(),
            width: width as usize,
            height: height as usize,
        });
    }
    let mut bytes = vec![0u8; 32 * 1024];
    let count = file.read(&mut bytes)?;
    bytes.truncate(count);
    if bytes.contains(&0) {
        return Ok(Preview::Info(
            "Binary file — open it in its associated application".into(),
        ));
    }
    let text = String::from_utf8_lossy(&bytes).into_owned();
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
        assert!(matches!(load(&binary).unwrap(), Preview::Info(_)));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn bounded_image_reader_decodes_a_small_png() {
        let root = test_root();
        let path = root.join("icon.png");
        fs::write(&path, include_bytes!("../assets/rice-explorer.png")).unwrap();
        assert!(matches!(
            load(&path).unwrap(),
            Preview::Image {
                width: 256,
                height: 256,
                ..
            }
        ));
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
        use std::os::unix::net::UnixListener;

        let root = test_root();
        let path = root.join("socket.txt");
        let _listener = UnixListener::bind(&path).unwrap();
        assert!(matches!(
            load(&path).unwrap(),
            Preview::Info(message) if message.contains("Special file")
        ));
        fs::remove_dir_all(root).unwrap();
    }
}
