//! GIO volume actions run outside the egui thread. The system's GIO/GVfs
//! handles device permissions and Polkit; RiceExplorer never invokes sudo.

use std::{
    ffi::OsString,
    io::{Read, Write},
    path::PathBuf,
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};

const MAX_REMOTE_LIST_BYTES: u64 = 4 * 1024 * 1024;
const MAX_REMOTE_ENTRIES: usize = 10_000;
const MAX_REMOTE_TEXT_BYTES: u64 = 32 * 1024;

#[derive(Clone, Debug)]
pub struct RemoteEntry {
    pub uri: String,
    pub name: String,
    pub is_directory: bool,
}

/// Enumerate URIs rather than unescaped names: newline and tab in a remote
/// filename are encoded in the URI, so each output line stays unambiguous.
/// GIO creates/drives its own GLib context in a worker process.
pub fn list_remote(
    uri: &str,
    show_hidden: bool,
    cancelled: &AtomicBool,
) -> Result<Vec<RemoteEntry>, String> {
    let mut command = Command::new("gio");
    command.args(["list", "-l", "-a", "standard::type", "-u"]);
    if show_hidden {
        command.arg("--hidden");
    }
    command.arg(uri).env("LC_ALL", "C");
    let bytes = capture_gio(
        command,
        MAX_REMOTE_LIST_BYTES,
        Duration::from_secs(20),
        cancelled,
        "Network listing",
    )?;
    parse_remote_listing(uri, &bytes)
}

/// Reading a remote file never grants a POSIX path or a mutating capability.
/// Only bounded plain-text previews are supported until native GIO is ready.
pub fn preview_remote_text(uri: &str, cancelled: &AtomicBool) -> Result<String, String> {
    if !matches!(
        crate::location::Location::parse(uri.into()),
        Ok(crate::location::Location::Remote(_))
    ) {
        return Err("Invalid network preview address".into());
    }
    let mut command = Command::new("gio");
    command.args(["cat", uri]);
    let bytes = capture_gio(
        command,
        MAX_REMOTE_TEXT_BYTES,
        Duration::from_secs(12),
        cancelled,
        "Network preview",
    )?;
    decode_remote_text(&bytes)
}

fn capture_gio(
    mut command: Command,
    limit: u64,
    timeout: Duration,
    cancelled: &AtomicBool,
    operation: &str,
) -> Result<Vec<u8>, String> {
    let mut child = command
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|error| format!("Cannot start {operation}: {error}"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| format!("{operation} output unavailable"))?;
    let reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout
            .take(limit + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        if cancelled.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = reader.join();
            return Err(format!("{operation} cancelled"));
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(50)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                return Err(format!(
                    "{operation} timed out after {} seconds",
                    timeout.as_secs()
                ));
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                return Err(format!("{operation} failed: {error}"));
            }
        }
    };
    let bytes = reader
        .join()
        .map_err(|_| format!("{operation} reader panicked"))?
        .map_err(|error| format!("Cannot read {operation}: {error}"))?;
    if bytes.len() as u64 > limit {
        return Err(format!(
            "{operation} exceeds the {} KiB limit",
            limit / 1024
        ));
    }
    if !status.success() {
        return Err(format!(
            "{operation} failed. Check connection and permissions."
        ));
    }
    Ok(bytes)
}

fn decode_remote_text(bytes: &[u8]) -> Result<String, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "Cannot preview binary or non-UTF-8 data")?;
    if text
        .chars()
        .any(|ch| ch.is_control() && !matches!(ch, '\n' | '\r' | '\t'))
    {
        return Err("Cannot preview binary or control-character data".into());
    }
    Ok(text.to_owned())
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
    ConnectAuthenticated(String, MountCredentials),
    Unmount(PathBuf),
    Eject(PathBuf),
}

/// Credentials are deliberately ephemeral: never embed them in a URI or
/// process arguments, and never include them in a Debug/status message.
#[derive(Clone)]
pub struct MountCredentials {
    pub username: String,
    pub domain: String,
    pub password: String,
}

impl std::fmt::Debug for MountCredentials {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("MountCredentials([REDACTED])")
    }
}

impl Drop for MountCredentials {
    fn drop(&mut self) {
        clear_password(&mut self.password);
    }
}

pub fn clear_password(password: &mut String) {
    // Best-effort wipe of our owned buffer, including when the sign-in dialog
    // is dismissed. External libraries and the server may keep other copies.
    let mut bytes = std::mem::take(password).into_bytes();
    bytes.fill(0);
    std::hint::black_box(&bytes);
}

impl VolumeAction {
    fn arguments(&self) -> Vec<OsString> {
        match self {
            Self::Connect(uri) | Self::ConnectAuthenticated(uri, _) => vec![OsString::from(uri)],
            Self::Unmount(path) => vec![OsString::from("-u"), path.as_os_str().to_owned()],
            Self::Eject(path) => vec![OsString::from("-e"), path.as_os_str().to_owned()],
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Connect(_) | Self::ConnectAuthenticated(_, _) => "Connect",
            Self::Unmount(_) => "Unmount",
            Self::Eject(_) => "Eject",
        }
    }
}

/// A bounded operation: offline servers and unresponsive device backends must
/// not keep a worker alive forever. Never use a shell or forward credentials.
pub fn execute(action: &VolumeAction) -> Result<(), String> {
    let mut command = Command::new("gio");
    command.arg("mount").args(action.arguments()).env("LC_ALL", "C");
    if let VolumeAction::ConnectAuthenticated(_, credentials) = action {
        return execute_authenticated(command, credentials.clone(), Duration::from_secs(30));
    }
    let mut child = command
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

/// GIO's console mount operation asks for User, Domain and Password in the
/// order required by the backend. Answer only recognized prompts; piping a
/// blind list of credentials could send a password to a username prompt.
/// No TTY or external password dialog is required, and secrets never appear
/// in process arguments or the filesystem.
fn answer_mount_prompts(
    mut stdout: impl Read,
    mut stdin: impl Write,
    credentials: MountCredentials,
) -> Result<(), String> {
    let mut buffer = Vec::new();
    let mut one = [0_u8; 1];
    let mut requests = 0;
    let mut password_attempted = false;
    loop {
        match stdout.read(&mut one) {
            Ok(0) => return Ok(()),
            Ok(_) => {}
            Err(error) => return Err(format!("Cannot read GIO authentication prompt: {error}")),
        }
        buffer.push(one[0]);
        if buffer.len() > 8192 {
            return Err("GIO authentication prompt exceeded the safety limit".into());
        }
        if one[0] != b' ' || !buffer.ends_with(b": ") {
            continue;
        }
        let prompt = String::from_utf8_lossy(&buffer);
        let prompt = prompt.rsplit('\n').next().unwrap_or("");
        let value = if prompt.starts_with("User") {
            &credentials.username
        } else if prompt.starts_with("Domain") {
            &credentials.domain
        } else if prompt.starts_with("Password") {
            if password_attempted {
                return Err("NAS requested the password again. Check credentials before retrying".into());
            }
            password_attempted = true;
            &credentials.password
        } else {
            // Unexpected requests must not accidentally receive a password.
            return Err("GIO requested an unsupported authentication field".into());
        };
        requests += 1;
        if requests > 6 || value.contains(['\r', '\n', '\0']) {
            return Err("Unsafe or repeated GIO authentication request".into());
        }
        stdin
            .write_all(value.as_bytes())
            .and_then(|()| stdin.write_all(b"\n"))
            .and_then(|()| stdin.flush())
            .map_err(|error| format!("Cannot submit GIO authentication: {error}"))?;
        buffer.clear();
    }
}

fn execute_authenticated(
    mut command: Command,
    credentials: MountCredentials,
    timeout: Duration,
) -> Result<(), String> {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("Cannot start authenticated GIO mount: {error}"))?;
    let stdout = child.stdout.take().ok_or("GIO output unavailable")?;
    let stdin = child.stdin.take().ok_or("GIO input unavailable")?;
    let reader = thread::spawn(move || answer_mount_prompts(stdout, stdin, credentials));
    let deadline = Instant::now() + timeout;
    let result = loop {
        match child.try_wait() {
            Ok(Some(status)) => break if status.success() { Ok(()) } else {
                Err("Connect failed. Check NAS credentials, GVfs services and the session D-Bus.".into())
            },
            Ok(None) if reader.is_finished() => {
                break Err("GIO authentication ended before the mount finished".into());
            }
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(50)),
            Ok(None) => break Err(format!("Authenticated connect timed out after {} seconds", timeout.as_secs())),
            Err(error) => break Err(format!("Cannot check GIO connection: {error}")),
        }
    };
    if result.is_err() {
        let _ = child.kill();
    }
    let _ = child.wait();
    let prompt_result = reader.join().map_err(|_| "GIO authentication reader panicked")?;
    prompt_result?;
    result
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
        let auth = VolumeAction::ConnectAuthenticated(
            "smb://host/share".into(),
            MountCredentials {
                username: "admin".into(),
                domain: "WORKGROUP".into(),
                password: "secret123".into(),
            },
        );
        assert_eq!(auth.arguments(), vec![OsString::from("smb://host/share")]);
        assert!(!format!("{auth:?}").contains("secret123"));
    }

    #[test]
    fn authenticated_mount_answers_prompts_in_backend_order() {
        let mut command = Command::new("sh");
        command.arg("-c").arg(
            "printf 'Password: '; IFS= read -r pass; \
             printf 'User [guest]: '; IFS= read -r user; \
             printf 'Domain [LOCAL]: '; IFS= read -r domain; \
             test \"$pass\" = 'test-password' && \
             test \"$user\" = 'alice' && test \"$domain\" = 'WORKGROUP'",
        );
        let credentials = MountCredentials {
            username: "alice".into(),
            domain: "WORKGROUP".into(),
            password: "test-password".into(),
        };
        assert!(execute_authenticated(command, credentials, Duration::from_secs(2)).is_ok());
    }

    #[test]
    fn authenticated_mount_does_not_reply_to_unrecognized_prompts() {
        let mut command = Command::new("sh");
        command.arg("-c").arg("printf 'Security question: '; IFS= read -r ignored");
        let credentials = MountCredentials {
            username: "alice".into(),
            domain: String::new(),
            password: "test-password".into(),
        };
        let error = execute_authenticated(command, credentials, Duration::from_secs(2)).unwrap_err();
        assert!(error.contains("unsupported authentication field"));
    }

    #[test]
    fn authenticated_mount_does_not_retry_bad_password_automatically() {
        let mut command = Command::new("sh");
        command.arg("-c").arg("printf 'Password: '; IFS= read -r one; printf 'Password: '; IFS= read -r two");
        let credentials = MountCredentials {
            username: String::new(),
            domain: String::new(),
            password: "wrong-password".into(),
        };
        let error = execute_authenticated(command, credentials, Duration::from_secs(2)).unwrap_err();
        assert!(error.contains("requested the password again"));
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

    #[test]
    fn remote_preview_rejects_local_uris_and_binary_data() {
        assert!(preview_remote_text("file:///etc/passwd", &AtomicBool::new(false)).is_err());
        assert_eq!(
            decode_remote_text(b"a\n\tUnicode: \xe2\x9c\x93").unwrap(),
            "a\n\tUnicode: ✓"
        );
        assert!(decode_remote_text(b"text\0hidden").is_err());
        assert!(decode_remote_text(&[0xff]).is_err());
        assert!(decode_remote_text(b"\x1b[31mcolored").is_err());
    }

    #[test]
    fn bounded_capture_rejects_output_above_budget() {
        let mut command = Command::new("printf");
        command.arg("abcdef");
        let result = capture_gio(
            command,
            3,
            Duration::from_secs(2),
            &AtomicBool::new(false),
            "Test output",
        );
        assert!(result.unwrap_err().contains("limit"));
    }
}
