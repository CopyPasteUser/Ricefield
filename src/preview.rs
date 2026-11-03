//! Bounded, off-thread previews. Never decode arbitrarily large images in the UI thread.
use std::{fs::{self, File}, io::{self, Read}, path::Path};

pub enum Preview {
    Image { rgba: Vec<u8>, width: usize, height: usize },
    Text(String),
    Info(String),
}

pub fn load(path: &Path) -> io::Result<Preview> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.is_dir() {
        return Ok(Preview::Info("Folder — open to browse its contents".into()));
    }
    let extension = path.extension().and_then(|ext| ext.to_str()).unwrap_or("").to_ascii_lowercase();
    if matches!(extension.as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp") {
        if metadata.len() > 32 * 1024 * 1024 {
            return Ok(Preview::Info("Image too large for in-app preview (32 MiB limit)".into()));
        }
        let (width, height) = image::image_dimensions(path).map_err(io::Error::other)?;
        if u64::from(width) * u64::from(height) > 16_000_000 {
            return Ok(Preview::Info("Image dimensions exceed preview limit (16 MP)".into()));
        }
        let image = image::open(path).map_err(io::Error::other)?.thumbnail(900, 800).to_rgba8();
        let (width, height) = image.dimensions();
        return Ok(Preview::Image {
            rgba: image.into_raw(),
            width: width as usize,
            height: height as usize,
        });
    }
    let mut bytes = vec![0u8; 32 * 1024];
    let count = File::open(path)?.read(&mut bytes)?;
    bytes.truncate(count);
    if bytes.contains(&0) {
        return Ok(Preview::Info("Binary file — open it in its associated application".into()));
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

    #[test]
    fn recognizes_text_and_binary_without_loading_whole_file() {
        let root = std::env::temp_dir().join(format!("rice-preview-test-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let text = root.join("hello.txt");
        fs::write(&text, b"hello world").unwrap();
        assert!(matches!(load(&text).unwrap(), Preview::Text(ref value) if value == "hello world"));
        let binary = root.join("binary.dat");
        fs::write(&binary, [0, 1, 2, 3]).unwrap();
        assert!(matches!(load(&binary).unwrap(), Preview::Info(_)));
        fs::remove_dir_all(root).unwrap();
    }
}
