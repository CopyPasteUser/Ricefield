//! Local MIME handler discovery through the desktop's existing GIO registry.
//! Never parse Exec= lines or invoke a shell; `gio launch` resolves desktop IDs.

use crate::volume;
use std::{io, path::Path, process::Command, sync::atomic::AtomicBool, thread, time::Duration};

fn valid_desktop_id(id: &str) -> bool {
    id.len() >= 9
        && id.len() <= 180
        && !id.starts_with('-')
        && id.ends_with(".desktop")
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
}

fn parse_mime(text: &str) -> Option<&str> {
    text.lines().find_map(|line| {
        let mime = line.trim().strip_prefix("standard::content-type:")?.trim();
        let (kind, subtype) = mime.split_once('/')?;
        (mime.len() <= 128
            && !kind.is_empty()
            && !subtype.is_empty()
            && !subtype.contains('/')
            && kind != "."
            && kind != ".."
            && subtype != "."
            && subtype != ".."
            && mime.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'.' | b'+' | b'-' | b'_')
            }))
        .then_some(mime)
    })
}

fn parse_handlers(text: &str) -> Vec<String> {
    let mut apps = Vec::new();
    let mut listing = false;
    for line in text.lines() {
        let clean = line.trim();
        if clean == "Registered applications:" {
            listing = true;
            continue;
        }
        if clean == "Recommended applications:" {
            listing = true;
            continue;
        }
        let candidate = if line.starts_with("Default application for") {
            line.rsplit_once(':').map(|(_, app)| app.trim())
        } else if listing {
            Some(clean)
        } else {
            None
        };
        if let Some(id) = candidate
            && valid_desktop_id(id)
            && !apps.iter().any(|existing| existing == id)
        {
            apps.push(id.to_owned());
        }
        if apps.len() >= 32 {
            break;
        }
    }
    apps
}

pub fn applications(path: &Path, cancelled: &AtomicBool) -> Result<Vec<String>, String> {
    if !path.is_absolute() {
        return Err("Open With requires a local absolute file path".into());
    }
    let mut info = Command::new("gio");
    info.args(["info", "-a", "standard::content-type"])
        .arg(path)
        .env("LC_ALL", "C");
    let output = volume::capture_gio(
        info,
        32 * 1024,
        Duration::from_secs(8),
        cancelled,
        "File MIME type",
    )?;
    let output = std::str::from_utf8(&output).map_err(|error| error.to_string())?;
    let mime = parse_mime(output).ok_or("Desktop could not determine this file's MIME type")?;

    let mut query = Command::new("gio");
    query.args(["mime", mime]).env("LC_ALL", "C");
    let output = volume::capture_gio(
        query,
        32 * 1024,
        Duration::from_secs(8),
        cancelled,
        "Registered applications",
    )?;
    let output = std::str::from_utf8(&output).map_err(|error| error.to_string())?;
    let apps = parse_handlers(output);
    if apps.is_empty() {
        return Err(format!("No registered desktop application for {mime}"));
    }
    Ok(apps)
}

pub fn launch(desktop_id: &str, path: &Path) -> io::Result<()> {
    if !valid_desktop_id(desktop_id) || !path.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Invalid Open With application or file path",
        ));
    }
    let mut child = Command::new("gio")
        .arg("launch")
        .arg(desktop_id)
        .arg(path)
        .spawn()?;
    thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_gio_mime_registry_without_shell_or_untrusted_desktop_paths() {
        assert_eq!(
            parse_mime("display name: image.png\n  standard::content-type: image/png\n"),
            Some("image/png")
        );
        assert_eq!(parse_mime("standard::content-type: x/../../evil"), None);
        let data = "Default application for ‘image/png’: viewer.desktop\n\
                    Registered applications:\n\
                    \tviewer.desktop\n\
                    \torg.example.Photos.desktop\n\
                    \t../../evil.desktop\n\
                    \t--flag.desktop\n\
                    Recommended applications:\n\
                    \tother.desktop\n";
        assert_eq!(
            parse_handlers(data),
            vec![
                "viewer.desktop",
                "org.example.Photos.desktop",
                "other.desktop"
            ]
        );
        assert!(launch("../../evil.desktop", Path::new("/tmp/test")).is_err());
    }
}
