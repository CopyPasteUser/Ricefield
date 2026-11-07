//! GIO volume actions run outside the egui thread. The system's GIO/GVfs
//! handles device permissions and Polkit; RiceField never invokes sudo.

use std::{
    collections::{HashSet, VecDeque},
    ffi::OsString,
    io::{Read, Write},
    path::PathBuf,
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

const MAX_REMOTE_LIST_BYTES: u64 = 4 * 1024 * 1024;
const MAX_REMOTE_ENTRIES: usize = 10_000;
const MAX_REMOTE_TEXT_BYTES: u64 = 32 * 1024;
const MAX_REMOTE_SEARCH_DIRS: usize = 64;
const MAX_REMOTE_SEARCH_ENTRIES: usize = 16_384;
const MAX_REMOTE_SEARCH_MATCHES: usize = 2048;
const MAX_REMOTE_SEARCH_DEPTH: usize = 8;
const REMOTE_SEARCH_BUDGET: Duration = Duration::from_secs(12);

#[derive(Clone, Debug)]
pub struct RemoteEntry {
    pub uri: String,
    pub name: String,
    pub is_directory: bool,
    pub is_regular_file: bool,
}

pub struct RemoteSearch {
    pub entries: Vec<RemoteEntry>,
    pub notice: Option<String>,
}

/// Enumerate URIs rather than unescaped names: newline and tab in a remote
/// filename are encoded in the URI, so each output line stays unambiguous.
/// GIO creates/drives its own GLib context in a worker process.
pub fn list_remote(
    uri: &str,
    show_hidden: bool,
    cancelled: &AtomicBool,
) -> Result<Vec<RemoteEntry>, String> {
    list_remote_with_timeout(uri, show_hidden, cancelled, Duration::from_secs(20))
}

pub(crate) fn list_remote_with_timeout(
    uri: &str,
    show_hidden: bool,
    cancelled: &AtomicBool,
    timeout: Duration,
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
        timeout,
        cancelled,
        "Network listing",
    )?;
    parse_remote_listing(uri, &bytes)
}

/// Explicit, read-only network filename search. Never read file bodies,
/// index, mutate a remote location or enumerate indefinitely. The GIO backend
/// resolves metadata; bounds still apply if it exposes linked directories.
/// Each GIO request respects the shared overall deadline.
pub fn search_remote(
    uri: &str,
    query: &str,
    recursive: bool,
    show_hidden: bool,
    cancelled: &AtomicBool,
) -> Result<RemoteSearch, String> {
    search_remote_with(uri, query, recursive, cancelled, |folder, remaining| {
        list_remote_with_timeout(folder, show_hidden, cancelled, remaining)
    })
}

fn search_remote_with(
    uri: &str,
    query: &str,
    recursive: bool,
    cancelled: &AtomicBool,
    mut list: impl FnMut(&str, Duration) -> Result<Vec<RemoteEntry>, String>,
) -> Result<RemoteSearch, String> {
    if query.is_empty() || query.len() > 120 || query.chars().any(char::is_control) {
        return Err("Network search needs a filename query of 1–120 characters".into());
    }
    if !matches!(
        crate::location::Location::parse(uri.into()),
        Ok(crate::location::Location::Remote(_))
    ) {
        return Err("Network search requires a validated remote URI".into());
    }

    let needle = query.to_lowercase();
    let deadline = Instant::now() + REMOTE_SEARCH_BUDGET;
    let mut queue = VecDeque::from([(uri.to_owned(), 0usize)]);
    let mut seen = HashSet::new();
    let mut entries = Vec::new();
    let mut inspected = 0usize;
    let mut skipped = 0usize;
    let mut partial = false;
    while let Some((folder, depth)) = queue.pop_front() {
        if cancelled.load(Ordering::Relaxed) {
            return Err("Network search cancelled".into());
        }
        if seen.len() >= MAX_REMOTE_SEARCH_DIRS || Instant::now() >= deadline {
            partial = true;
            break;
        }
        if !seen.insert(folder.clone()) {
            continue;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        let listed = match list(&folder, remaining) {
            Ok(listed) => listed,
            Err(error) if depth == 0 => return Err(error),
            Err(_) => {
                skipped += 1;
                continue;
            }
        };
        for item in listed {
            if cancelled.load(Ordering::Relaxed) {
                return Err("Network search cancelled".into());
            }
            inspected += 1;
            if inspected > MAX_REMOTE_SEARCH_ENTRIES {
                partial = true;
                break;
            }
            if remote_name_matches(&item.name, &needle) {
                entries.push(item.clone());
                if entries.len() >= MAX_REMOTE_SEARCH_MATCHES {
                    partial = true;
                    break;
                }
            }
            if recursive && item.is_directory {
                if depth < MAX_REMOTE_SEARCH_DEPTH {
                    queue.push_back((item.uri, depth + 1));
                } else {
                    partial = true;
                }
            }
        }
        if partial {
            break;
        }
    }
    if !queue.is_empty() {
        partial = true;
    }
    let notice = (partial || skipped > 0).then(|| {
        format!(
            "Partial network filename search: {} folders inspected, {inspected} entries checked, {skipped} folders unavailable; limits: 64 folders / 16,384 entries / 2,048 matches / 12 seconds",
            seen.len()
        )
    });
    Ok(RemoteSearch { entries, notice })
}

/// GIO URI-list output percent-encodes spaces and Unicode. Match what the
/// person types, while keeping the original encoded URI as the only action key.
fn remote_name_matches(encoded: &str, lower_needle: &str) -> bool {
    if encoded.to_lowercase().contains(lower_needle) {
        return true;
    }
    decode_remote_label(encoded).is_some_and(|name| name.to_lowercase().contains(lower_needle))
}

/// Render percent-encoded GIO URI segments as human-readable names without
/// changing the original URI used for navigation, preview and download.
/// Invalid UTF-8 or embedded controls remain visibly escaped.
pub fn display_remote_name(encoded: &str) -> String {
    decode_remote_label(encoded).unwrap_or_else(|| encoded.to_owned())
}

fn decode_remote_label(encoded: &str) -> Option<String> {
    if !encoded.contains('%') {
        return Some(encoded.to_owned());
    }
    let raw = encoded.as_bytes();
    let mut decoded = Vec::with_capacity(raw.len());
    let mut index = 0;
    while index < raw.len() {
        if raw[index] == b'%'
            && index + 2 < raw.len()
            && let (Some(hi), Some(lo)) = (hex(raw[index + 1]), hex(raw[index + 2]))
        {
            decoded.push((hi << 4) | lo);
            index += 3;
        } else {
            decoded.push(raw[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded)
        .ok()
        .filter(|name| !name.chars().any(char::is_control) && !name.contains(['/', '\\']))
}

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Reading a remote file never grants a POSIX path or a mutating capability.
/// Only a bounded prefix is read: large files do not need to be downloaded.
pub fn preview_remote_text(uri: &str, cancelled: &AtomicBool) -> Result<String, String> {
    if !matches!(
        crate::location::Location::parse(uri.into()),
        Ok(crate::location::Location::Remote(_))
    ) {
        return Err("Invalid network preview address".into());
    }
    let mut command = Command::new("gio");
    command.args(["cat", uri]);
    let (bytes, truncated) = capture_gio_prefix(
        command,
        MAX_REMOTE_TEXT_BYTES,
        Duration::from_secs(12),
        cancelled,
        "Network preview",
    )?;
    preview_remote_bytes(&bytes, truncated)
}

fn preview_remote_bytes(bytes: &[u8], truncated: bool) -> Result<String, String> {
    // A valid UTF-8 character can straddle the artificial prefix boundary.
    // Do not mislabel that particular case as binary.
    let end = match std::str::from_utf8(bytes) {
        Err(error) if truncated && error.error_len().is_none() => error.valid_up_to(),
        _ => bytes.len(),
    };
    match decode_remote_text(&bytes[..end]) {
        Ok(mut text) => {
            if truncated {
                text.push_str("\n\n[Preview limited to the first 32 KiB]");
            }
            Ok(text)
        }
        Err(_) => Ok(crate::preview::binary_hex_text(
            bytes,
            (!truncated).then_some(bytes.len() as u64),
        )),
    }
}

pub(crate) fn capture_gio(
    command: Command,
    limit: u64,
    timeout: Duration,
    cancelled: &AtomicBool,
    operation: &str,
) -> Result<Vec<u8>, String> {
    capture_command(command, limit, timeout, cancelled, operation, false, false)
        .map(|(bytes, _)| bytes)
}

fn capture_gio_prefix(
    command: Command,
    limit: u64,
    timeout: Duration,
    cancelled: &AtomicBool,
    operation: &str,
) -> Result<(Vec<u8>, bool), String> {
    capture_command(command, limit, timeout, cancelled, operation, false, true)
}

/// A caller-supplied, already-open stdin is needed for hashing the exact file
/// descriptor validated by the worker. Ordinary GIO commands get null stdin.
pub(crate) fn capture_gio_with_stdin(
    command: Command,
    limit: u64,
    timeout: Duration,
    cancelled: &AtomicBool,
    operation: &str,
) -> Result<Vec<u8>, String> {
    capture_command(command, limit, timeout, cancelled, operation, true, false)
        .map(|(bytes, _)| bytes)
}

fn capture_command(
    mut command: Command,
    limit: u64,
    timeout: Duration,
    cancelled: &AtomicBool,
    operation: &str,
    supplied_stdin: bool,
    allow_prefix: bool,
) -> Result<(Vec<u8>, bool), String> {
    if !supplied_stdin {
        command.stdin(Stdio::null());
    }
    let mut child = command
        .stderr(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|error| format!("Cannot start {operation}: {error}"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| format!("{operation} output unavailable"))?;
    let prefix_ready = Arc::new(AtomicBool::new(false));
    let reader_ready = Arc::clone(&prefix_ready);
    let reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.take(limit + 1).read_to_end(&mut bytes).map(|_| {
            if bytes.len() as u64 > limit {
                reader_ready.store(true, Ordering::Release);
            }
            bytes
        })
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        if cancelled.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = reader.join();
            return Err(format!("{operation} cancelled"));
        }
        if allow_prefix && prefix_ready.load(Ordering::Acquire) {
            // We asked only for a preview prefix. Kill a writer that might
            // otherwise wait on a slow server or a full/closed stdout pipe.
            let _ = child.kill();
            let _ = child.wait();
            let mut bytes = reader
                .join()
                .map_err(|_| format!("{operation} reader panicked"))?
                .map_err(|error| format!("Cannot read {operation}: {error}"))?;
            bytes.truncate(limit as usize);
            return Ok((bytes, true));
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
    let mut bytes = reader
        .join()
        .map_err(|_| format!("{operation} reader panicked"))?
        .map_err(|error| format!("Cannot read {operation}: {error}"))?;
    let truncated = bytes.len() as u64 > limit;
    if truncated && !allow_prefix {
        return Err(format!(
            "{operation} exceeds the {} KiB limit",
            limit / 1024
        ));
    }
    if !status.success() && !truncated {
        return Err(format!(
            "{operation} failed. Check connection and permissions."
        ));
    }
    if truncated {
        bytes.truncate(limit as usize);
    }
    Ok((bytes, truncated))
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
            is_regular_file: kind == "(regular)",
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
    MountDevice(String),
    UnmountDevice {
        object_path: String,
        mount_point: PathBuf,
    },
    EjectDrive(String),
    PowerOffDrive(String),
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
            Self::MountDevice(_) => Vec::new(),
            Self::UnmountDevice { .. } | Self::EjectDrive(_) | Self::PowerOffDrive(_) => Vec::new(),
            Self::Unmount(path) => vec![OsString::from("-u"), path.as_os_str().to_owned()],
            Self::Eject(path) => vec![OsString::from("-e"), path.as_os_str().to_owned()],
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Connect(_) | Self::ConnectAuthenticated(_, _) => "Connect",
            Self::MountDevice(_) => "Mount",
            Self::UnmountDevice { .. } => "Unmount",
            Self::EjectDrive(_) => "Eject",
            Self::PowerOffDrive(_) => "Power off USB drive",
            Self::Unmount(_) => "Unmount",
            Self::Eject(_) => "Eject",
        }
    }
}

/// A bounded operation: offline servers and unresponsive device backends must
/// not keep a worker alive forever. Never use a shell or forward credentials.
pub fn execute(action: &VolumeAction) -> Result<(), String> {
    if let VolumeAction::MountDevice(object_path) = action {
        return crate::udisks2::mount(object_path);
    }
    if let VolumeAction::UnmountDevice {
        object_path,
        mount_point,
    } = action
    {
        return crate::udisks2::unmount(object_path, mount_point);
    }
    if let VolumeAction::EjectDrive(drive_path) = action {
        return crate::udisks2::eject(drive_path);
    }
    if let VolumeAction::PowerOffDrive(drive_path) = action {
        return crate::udisks2::power_off(drive_path);
    }
    let mut command = Command::new("gio");
    command
        .arg("mount")
        .args(action.arguments())
        .env("LC_ALL", "C");
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
                return Err(
                    "NAS requested the password again. Check credentials before retrying".into(),
                );
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
            Ok(Some(status)) => {
                break if status.success() {
                    Ok(())
                } else {
                    Err("Connect failed. Check NAS credentials, GVfs services and the session D-Bus.".into())
                };
            }
            Ok(None) if reader.is_finished() => {
                break Err("GIO authentication ended before the mount finished".into());
            }
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(50)),
            Ok(None) => {
                break Err(format!(
                    "Authenticated connect timed out after {} seconds",
                    timeout.as_secs()
                ));
            }
            Err(error) => break Err(format!("Cannot check GIO connection: {error}")),
        }
    };
    if result.is_err() {
        let _ = child.kill();
    }
    let _ = child.wait();
    let prompt_result = reader
        .join()
        .map_err(|_| "GIO authentication reader panicked")?;
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
        command
            .arg("-c")
            .arg("printf 'Security question: '; IFS= read -r ignored");
        let credentials = MountCredentials {
            username: "alice".into(),
            domain: String::new(),
            password: "test-password".into(),
        };
        let error =
            execute_authenticated(command, credentials, Duration::from_secs(2)).unwrap_err();
        assert!(error.contains("unsupported authentication field"));
    }

    #[test]
    fn authenticated_mount_does_not_retry_bad_password_automatically() {
        let mut command = Command::new("sh");
        command
            .arg("-c")
            .arg("printf 'Password: '; IFS= read -r one; printf 'Password: '; IFS= read -r two");
        let credentials = MountCredentials {
            username: String::new(),
            domain: String::new(),
            password: "wrong-password".into(),
        };
        let error =
            execute_authenticated(command, credentials, Duration::from_secs(2)).unwrap_err();
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
        assert!(entries[1].is_regular_file);
        let special = parse_remote_listing(
            uri,
            b"smb://nas/share/Linked%20Folder\t0\t(symbolic link)\n",
        )
        .unwrap();
        assert!(!special[0].is_directory);
        assert!(!special[0].is_regular_file);
        assert!(parse_remote_listing(uri, b"file:///tmp/secret\t0\t(directory)\n").is_err());
        assert!(parse_remote_listing(uri, b"smb://nas/share/sub/file\t0\t(directory)\n").is_err());
        assert!(parse_remote_listing(uri, b"smb://nas/share/no-metadata\n").is_err());
        assert!(
            parse_remote_listing(uri, &vec![b'a'; MAX_REMOTE_LIST_BYTES as usize + 1]).is_err()
        );
    }

    #[test]
    fn bounded_network_filename_search_handles_subfolders_and_partial_errors() {
        let cancelled = AtomicBool::new(false);
        let root = "smb://nas/share";
        let entries_for = |folder: &str, _: Duration| -> Result<Vec<RemoteEntry>, String> {
            match folder {
                "smb://nas/share" => Ok(vec![
                    RemoteEntry {
                        uri: "smb://nas/share/Music".into(),
                        name: "Music".into(),
                        is_directory: true,
                        is_regular_file: false,
                    },
                    RemoteEntry {
                        uri: "smb://nas/share/Offline".into(),
                        name: "Offline".into(),
                        is_directory: true,
                        is_regular_file: false,
                    },
                    RemoteEntry {
                        uri: "smb://nas/share/Report.txt".into(),
                        name: "Report.txt".into(),
                        is_directory: false,
                        is_regular_file: true,
                    },
                ]),
                "smb://nas/share/Music" => Ok(vec![RemoteEntry {
                    uri: "smb://nas/share/Music/REPORT.MP3".into(),
                    name: "REPORT.MP3".into(),
                    is_directory: false,
                    is_regular_file: true,
                }]),
                _ => Err("Share unplugged".into()),
            }
        };
        let direct = search_remote_with(root, "report", false, &cancelled, entries_for).unwrap();
        assert_eq!(direct.entries.len(), 1);
        assert!(direct.notice.is_none());
        let nested = search_remote_with(root, "report", true, &cancelled, entries_for).unwrap();
        assert_eq!(nested.entries.len(), 2);
        assert!(nested.notice.unwrap().contains("Partial"));
        assert!(search_remote_with(root, "\n", true, &cancelled, entries_for).is_err());
    }

    #[test]
    fn network_search_checks_cancel_before_any_gio_call() {
        let cancelled = AtomicBool::new(true);
        let result = search_remote_with("smb://nas/share", "file", true, &cancelled, |_, _| {
            panic!("cancelled search must not enumerate a folder")
        });
        assert!(result.is_err());
    }

    #[test]
    fn network_search_matches_human_readable_uri_names() {
        assert!(remote_name_matches(
            "Season%201%20Finale.mkv",
            "season 1 finale"
        ));
        assert!(remote_name_matches("%E6%97%A5%E6%9C%AC.mp3", "日本"));
        assert!(!remote_name_matches("%FF.dat", "test"));
    }

    #[test]
    fn remote_labels_decode_display_only_and_escape_unsafe_bytes() {
        assert_eq!(
            display_remote_name("Season%201%20Finale.mkv"),
            "Season 1 Finale.mkv"
        );
        assert_eq!(display_remote_name("%E6%97%A5%E6%9C%AC.mp3"), "日本.mp3");
        assert_eq!(display_remote_name("100%25.txt"), "100%.txt");
        for invalid in ["%FF.dat", "line%0Afeed", "a%2Fb", "a%5Cb"] {
            assert_eq!(display_remote_name(invalid), invalid);
        }
        assert_eq!(display_remote_name("plain.txt"), "plain.txt");
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
        let binary = preview_remote_bytes(b"\x1b[31m\x00", false).unwrap();
        assert!(binary.contains("1b 5b 33 31 6d 00"));
        assert!(!binary.contains('\x1b'));
        let text = preview_remote_bytes(b"abc\xc3", true).unwrap();
        assert!(text.starts_with("abc"));
        assert!(text.contains("limited to the first 32 KiB"));
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

    #[test]
    fn remote_prefix_capture_stops_large_stream_without_downloading_it() {
        let mut command = Command::new("head");
        command.args(["-c", "1048576", "/dev/zero"]);
        let started = Instant::now();
        let (bytes, truncated) = capture_gio_prefix(
            command,
            1024,
            Duration::from_secs(6),
            &AtomicBool::new(false),
            "Test prefix",
        )
        .unwrap();
        assert!(truncated);
        assert_eq!(bytes.len(), 1024);
        assert!(bytes.iter().all(|byte| *byte == 0));
        assert!(started.elapsed() < Duration::from_secs(3));

        let mut failing = Command::new("sh");
        failing.args(["-c", "printf partial; exit 7"]);
        let failure = capture_gio_prefix(
            failing,
            1024,
            Duration::from_secs(2),
            &AtomicBool::new(false),
            "Broken remote stream",
        );
        assert!(failure.unwrap_err().contains("failed"));
    }
}
