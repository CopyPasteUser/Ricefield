//! GIO volume actions run outside the egui thread. The system's GIO/GVfs
//! handles device permissions and Polkit; RiceExplorer never invokes sudo.

use std::{
    ffi::OsString,
    io::Read,
    path::PathBuf,
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};

const MAX_REMOTE_LIST_BYTES: u64 = 4 * 1024 * 1024;
const MAX_REMOTE_ENTRIES: usize = 10_000;

#[derive(Clone, Debug)]
pub struct RemoteEntry {
    pub uri: String,
    pub name: String,
    pub is_directory: bool,
}

/// Enumerate URIs rather than unescaped names: newline and tab in a remote
/// filename are encoded in the URI, so each output line stays unambiguous.
/// GIO creates/drives its own GLib context in a worker process.
pub fn list_remote(uri: &str, cancelled: &AtomicBool) -> Result<Vec<RemoteEntry>, String> {
    let mut child = Command::new("gio")
        .args(["list", "-l", "-a", "standard::type", "-u", uri])
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|error| format!("Cannot start GIO listing: {error}"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or("GIO listing output unavailable")?;
    let reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout
            .take(MAX_REMOTE_LIST_BYTES + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let deadline = Instant::now() + Duration::from_secs(20);
    let status = loop {
        if cancelled.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = reader.join();
            return Err("Network listing cancelled".into());
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(50)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                return Err("Network listing timed out after 20 seconds".into());
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                return Err(format!("GIO listing failed: {error}"));
            }
        }
    };
    let bytes = reader
        .join()
        .map_err(|_| "GIO listing reader panicked")?
        .map_err(|error| format!("Cannot read GIO listing: {error}"))?;
    if !status.success() {
        return Err("Cannot browse network location. Check connection and permissions.".into());
    }
    parse_remote_listing(uri, &bytes)
}

fn parse_remote_listing(parent: &str, bytes: &[u8]) -> Result<Vec<RemoteEntry>, String> {
    if bytes.len() > MAX_REMOTE_LIST_BYTES as usize {
        return Err("Network directory listing exceeds the 4 MiB safety limit".into());
    }
    let text = std::str::from_utf8(bytes).map_err(|_| "Invalid GIO URI encoding")?;
    let prefix = format!("{}/", parent.trim_end_matches('/'));
    let mut entries = Vec::new();
    for line in text.lines().filter(|line| !line.is_empty()) {
        // Never trust GIO stdout as a local path or permit an unrelated URI.
        let (entry_uri, metadata) = line.split_once('\t').ok_or("Malformed GIO listing")?;
        let (_, kind) = metadata
            .rsplit_once('\t')
            .ok_or("Malformed GIO entry metadata")?;
        let name = entry_uri
            .strip_prefix(&prefix)
            .filter(|name| !name.is_empty() && !name.contains('/'))
            .ok_or("Unexpected URI in GIO listing")?;
        if !matches!(
            crate::location::Location::parse(entry_uri.into()),
            Ok(crate::location::Location::Remote(_))
        ) {
            return Err("Invalid remote entry URI".into());
        }
        entries.push(RemoteEntry {
            uri: entry_uri.to_owned(),
            name: name.to_owned(),
            is_directory: kind == "(directory)",
        });
        if entries.len() > MAX_REMOTE_ENTRIES {
            return Err("Network directory has more than 10,000 entries".into());
        }
    }
    Ok(entries)
}

#[derive(Clone, Debug)]
pub enum VolumeAction {
    Connect(String),
    Unmount(PathBuf),
    Eject(PathBuf),
}

impl VolumeAction {
    fn arguments(&self) -> Vec<OsString> {
        match self {
            Self::Connect(uri) => vec![OsString::from(uri)],
            Self::Unmount(path) => vec![OsString::from("-u"), path.as_os_str().to_owned()],
            Self::Eject(path) => vec![OsString::from("-e"), path.as_os_str().to_owned()],
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Connect(_) => "Connect",
            Self::Unmount(_) => "Unmount",
            Self::Eject(_) => "Eject",
        }
    }
}

/// A bounded operation: offline servers and unresponsive device backends must
/// not keep a worker alive forever. Never use a shell or forward credentials.
pub fn execute(action: &VolumeAction) -> Result<(), String> {
    let mut child = Command::new("gio")
        .arg("mount")
        .args(action.arguments())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("Cannot start GIO: {error}"))?;
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(_)) => {
                return Err(format!(
                    "{} failed. Check the device, GVfs backend and authentication in the system settings.",
                    action.label()
                ));
            }
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(100)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{} timed out after 30 seconds", action.label()));
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("Cannot check GIO operation: {error}"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_never_interpret_paths_as_shell_code() {
        let path = PathBuf::from("/media/USB ; $(touch /tmp/unsafe)");
        assert_eq!(
            VolumeAction::Unmount(path.clone()).arguments(),
            vec![OsString::from("-u"), path.as_os_str().to_owned()]
        );
        assert_eq!(
            VolumeAction::Eject(path.clone()).arguments(),
            vec![OsString::from("-e"), path.as_os_str().to_owned()]
        );
        assert_eq!(
            VolumeAction::Connect("smb://host/share".into()).arguments(),
            vec![OsString::from("smb://host/share")]
        );
    }

    #[test]
    fn remote_listing_rejects_unrelated_uris_and_large_results() {
        let uri = "smb://nas/share";
        let entries = parse_remote_listing(
            uri,
            b"smb://nas/share/A%0Afile\t0\t(directory)\nsmb://nas/share/B\t123\t(regular)\n",
        )
        .unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "A%0Afile");
        assert!(entries[0].is_directory);
        assert!(!entries[1].is_directory);
        assert!(parse_remote_listing(uri, b"file:///tmp/secret\t0\t(directory)\n").is_err());
        assert!(parse_remote_listing(uri, b"smb://nas/share/sub/file\t0\t(directory)\n").is_err());
        assert!(parse_remote_listing(uri, b"smb://nas/share/no-metadata\n").is_err());
        assert!(
            parse_remote_listing(uri, &vec![b'a'; MAX_REMOTE_LIST_BYTES as usize + 1]).is_err()
        );
    }
}
