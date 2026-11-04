//! File clipboard interoperability with GTK file managers on X11 and Wayland.
//! The internal clipboard remains available when xclip/wl-clipboard is absent.
use std::{
    ffi::OsString,
    io::{self, Read, Write},
    path::PathBuf,
    process::{Command, Stdio},
};

const MAX_DESKTOP_CLIPBOARD_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Clone)]
pub struct Files {
    pub paths: Vec<PathBuf>,
    pub cut: bool,
}

fn percent_encode(path: &std::path::Path) -> String {
    #[cfg(unix)]
    use std::os::unix::ffi::OsStrExt;
    #[cfg(unix)]
    let bytes = path.as_os_str().as_bytes().to_vec();
    #[cfg(not(unix))]
    let bytes = path.to_string_lossy().as_bytes().to_vec();
    let mut result = String::from("file://");
    for byte in bytes {
        if byte.is_ascii_alphanumeric() || b"/-._~".contains(&byte) {
            result.push(char::from(byte));
        } else {
            result.push_str(&format!("%{byte:02X}"));
        }
    }
    result
}

fn decode_uri(uri: &str) -> Option<PathBuf> {
    let bytes = uri.strip_prefix("file://")?.strip_prefix('/')?;
    let mut decoded = vec![b'/'];
    let bytes = bytes.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let value = bytes.get(index + 1..index + 3)?;
            let hex = std::str::from_utf8(value).ok()?;
            decoded.push(u8::from_str_radix(hex, 16).ok()?);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    if decoded.contains(&0) {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        Some(PathBuf::from(OsString::from_vec(decoded)))
    }
    #[cfg(not(unix))]
    {
        String::from_utf8(decoded).ok().map(PathBuf::from)
    }
}

fn handlers() -> Vec<(&'static str, Vec<&'static str>, Vec<&'static str>)> {
    let wayland = (
        "wl-copy",
        vec!["--type", "x-special/gnome-copied-files"],
        vec![
            "wl-paste",
            "--no-newline",
            "--type",
            "x-special/gnome-copied-files",
        ],
    );
    let x11 = (
        "xclip",
        vec![
            "-selection",
            "clipboard",
            "-t",
            "x-special/gnome-copied-files",
            "-i",
        ],
        vec![
            "xclip",
            "-selection",
            "clipboard",
            "-t",
            "x-special/gnome-copied-files",
            "-o",
        ],
    );
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        vec![wayland, x11]
    } else {
        vec![x11, wayland]
    }
}

pub fn copy_files(paths: &[PathBuf], cut: bool) -> io::Result<()> {
    if paths.is_empty() {
        return Ok(());
    }
    let mut data = String::from(if cut { "cut\n" } else { "copy\n" });
    for path in paths {
        let absolute = if path.is_absolute() {
            path.clone()
        } else {
            std::env::current_dir()?.join(path)
        };
        data.push_str(&percent_encode(&absolute));
        data.push('\n');
        if data.len() as u64 > MAX_DESKTOP_CLIPBOARD_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "File selection is too large for the desktop clipboard",
            ));
        }
    }
    for (program, args, _) in handlers() {
        if let Ok(mut child) = Command::new(program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            let wrote = child
                .stdin
                .take()
                .ok_or_else(|| io::Error::other("Clipboard process lacks stdin"))?
                .write_all(data.as_bytes());
            let exited = child.wait()?;
            if wrote.is_ok() && exited.success() {
                return Ok(());
            }
        }
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "Install xclip (X11) or wl-clipboard (Wayland) for desktop clipboard integration",
    ))
}

pub fn paste_files() -> Option<Files> {
    for (_, _, command) in handlers() {
        let (program, args) = command.split_first()?;
        let Ok(mut child) = Command::new(program)
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        else {
            continue;
        };
        let mut bytes = Vec::new();
        let read = child
            .stdout
            .take()?
            .take(MAX_DESKTOP_CLIPBOARD_BYTES + 1)
            .read_to_end(&mut bytes);
        if read.is_err() || bytes.len() as u64 > MAX_DESKTOP_CLIPBOARD_BYTES {
            let _ = child.kill();
            let _ = child.wait();
            continue;
        }
        if !child.wait().is_ok_and(|status| status.success()) {
            continue;
        }
        if let Ok(text) = String::from_utf8(bytes)
            && let Some(files) = parse_files(&text)
        {
            return Some(files);
        }
    }
    None
}

fn parse_files(text: &str) -> Option<Files> {
    let mut lines = text.lines();
    let first = lines.next()?;
    let (cut, urls): (bool, Vec<&str>) = match first {
        "cut" => (true, lines.collect()),
        "copy" => (false, lines.collect()),
        _ => (
            false,
            text.lines().filter(|line| !line.starts_with('#')).collect(),
        ),
    };
    let paths: Vec<_> = urls.into_iter().filter_map(decode_uri).collect();
    if paths.is_empty() {
        None
    } else {
        Some(Files { paths, cut })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uri_roundtrip_with_spaces_and_unicode() {
        let file = PathBuf::from("/tmp/Äpfel & Ordner/one#.txt");
        assert_eq!(decode_uri(&percent_encode(&file)), Some(file));
    }

    #[test]
    fn oversized_file_clipboard_never_launches_a_desktop_helper() {
        let path = PathBuf::from("x".repeat(MAX_DESKTOP_CLIPBOARD_BYTES as usize));
        assert_eq!(
            copy_files(&[path], false).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
    }

    #[test]
    fn rejects_remote_and_malformed_file_uris() {
        assert!(decode_uri("file://otherhost/tmp/file").is_none());
        assert!(decode_uri("file:///tmp/%00bad").is_none());
        assert!(decode_uri("file:///tmp/%nothex").is_none());
    }

    #[test]
    fn parses_gnome_cut_and_uri_list() {
        assert!(parse_files("cut\nfile:///tmp/a%20b\n").unwrap().cut);
        assert_eq!(
            parse_files("# comment\nfile:///tmp/a\n").unwrap().paths,
            vec![PathBuf::from("/tmp/a")]
        );
    }
}
