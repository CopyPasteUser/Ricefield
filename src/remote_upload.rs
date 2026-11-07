//! Server-conditioned WebDAV uploads. Generic GIO move/copy commands are NOT
//! safe no-replace primitives (see docs/REMOTE-WRITE-SAFETY.md). Keep other
//! backends closed until they have independently verified publish protocols.

use crate::{
    keyring,
    location::{Location, RemoteScheme},
    volume::MountCredentials,
};
use std::{
    fs::{self, DirBuilder, File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};

const MAX_UPLOAD_BYTES: u64 = 8 * 1024 * 1024 * 1024;
const CURL_TIMEOUT: Duration = Duration::from_secs(20 * 60);
const HTTP_NEW: u16 = 201;
const HTTP_PRECONDITION_FAILED: u16 = 412;

pub struct UploadRequest {
    pub source: PathBuf,
    pub remote_directory: String,
    pub credentials: MountCredentials,
    pub allow_insecure_http: bool,
}

#[derive(Debug)]
pub struct UploadResult {
    pub uri: String,
    pub bytes: u64,
}

fn webdav_url(uri: &str) -> Result<String, String> {
    let scheme = match Location::parse(uri.into()) {
        Ok(Location::Remote(remote)) => remote.scheme,
        _ => return Err("Upload destination must be a validated network URI".into()),
    };
    let prefix =
        match scheme {
            RemoteScheme::Dav => "http",
            RemoteScheme::Davs => "https",
            _ => return Err(
                "This server has no verified safe-write backend; only WebDAV upload is supported"
                    .into(),
            ),
        };
    Ok(format!("{prefix}://{}", uri.split_once("://").unwrap().1))
}

fn escape_filename(name: &std::ffi::OsStr) -> Result<String, String> {
    use std::os::unix::ffi::OsStrExt;
    let name = name.as_bytes();
    if name.is_empty()
        || name.len() > 255
        || name == b"."
        || name == b".."
        || name.iter().any(|byte| *byte == b'/' || *byte == 0)
    {
        return Err("Invalid local filename for a remote upload".into());
    }
    let mut out = String::with_capacity(name.len() * 3);
    for &byte in name {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    Ok(out)
}

fn random_suffix() -> io::Result<String> {
    let mut seed = [0_u8; 16];
    File::open("/dev/urandom")?.read_exact(&mut seed)?;
    Ok(seed.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// Once a remote stage might exist, report its exact address for later
/// inspection. Network timeouts are ambiguous: neither an automatic DELETE
/// nor an automatic retry is safe without fresh server-side verification.
fn staged_upload_error(stage: &str, reason: &str) -> String {
    format!(
        "{reason}. A private WebDAV stage may remain at {stage}. Check both the final destination and this stage before manual cleanup; RiceField has not deleted or retried either object"
    )
}

/// Do not use shell quoting or command-line credentials: curl reads a short,
/// private config from stdin. Disable ~/.curlrc, redirects and automatic
/// retries so the destination host never receives an unapproved replay.
fn curl_config(credentials: &MountCredentials) -> Result<String, String> {
    if credentials.username.contains(['\n', '\r', ':', '\0'])
        || credentials.password.contains(['\n', '\r', '\0'])
        || credentials.domain.contains(['\n', '\r', '\0'])
    {
        return Err("Unsupported control character in network credentials".into());
    }
    let username = if credentials.domain.is_empty() {
        credentials.username.clone()
    } else {
        format!("{}\\{}", credentials.domain, credentials.username)
    };
    let mut userpass = format!("{username}:{}", credentials.password);
    let mut escaped = userpass.replace('\\', "\\\\").replace('"', "\\\"");
    let config = format!("user = \"{escaped}\"\nanyauth\n");
    crate::volume::clear_password(&mut userpass);
    crate::volume::clear_password(&mut escaped);
    Ok(config)
}

fn curl_command(credentials: &MountCredentials) -> Result<(Command, String), String> {
    let mut command = Command::new("curl");
    command.args([
        "--disable",
        "--config",
        "-",
        "--silent",
        "--show-error",
        "--proto",
        "=http,https",
        "--max-time",
        "1200",
        "--connect-timeout",
        "15",
        "--retry",
        "0",
    ]);
    command.stdin(Stdio::piped()).stderr(Stdio::null());
    Ok((command, curl_config(credentials)?))
}

fn launch(mut command: Command, mut config: String) -> Result<std::process::Child, String> {
    let mut child = command
        .spawn()
        .map_err(|error| format!("Cannot start curl: {error}"))?;
    let write_result = child
        .stdin
        .take()
        .ok_or("Missing curl credential input")?
        .write_all(config.as_bytes());
    crate::volume::clear_password(&mut config);
    if let Err(error) = write_result {
        let _ = child.kill();
        let _ = child.wait();
        return Err(format!("Cannot deliver curl credentials: {error}"));
    }
    Ok(child)
}

fn http_action(
    credentials: &MountCredentials,
    method: &str,
    uri: &str,
    headers: &[String],
    upload_file: Option<&Path>,
    cancelled: &AtomicBool,
) -> Result<u16, String> {
    let (mut command, config) = curl_command(credentials)?;
    command.args([
        "--request",
        method,
        "--output",
        "/dev/null",
        "--write-out",
        "%{http_code}",
    ]);
    for header in headers {
        if header.contains(['\r', '\n']) {
            return Err("Invalid HTTP header".into());
        }
        command.arg("--header").arg(header);
    }
    if let Some(file) = upload_file {
        command.arg("--upload-file").arg(file);
    }
    command.arg("--").arg(uri).stdout(Stdio::piped());
    let mut child = launch(command, config)?;
    let started = Instant::now();
    loop {
        if cancelled.load(Ordering::Relaxed) || started.elapsed() > CURL_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            return Err(if cancelled.load(Ordering::Relaxed) {
                "Upload cancelled"
            } else {
                "Remote request timed out"
            }
            .into());
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut result = String::new();
                child
                    .stdout
                    .take()
                    .ok_or("Missing curl response")?
                    .take(16)
                    .read_to_string(&mut result)
                    .map_err(|error| format!("Cannot read HTTP status: {error}"))?;
                if !status.success() {
                    return Err(format!("Network request failed (curl exit {status})"));
                }
                return result
                    .trim()
                    .parse::<u16>()
                    .map_err(|_| "Invalid WebDAV response status".into());
            }
            Ok(None) => thread::sleep(Duration::from_millis(40)),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("Cannot wait for WebDAV request: {error}"));
            }
        }
    }
}

struct Snapshot {
    directory: PathBuf,
    path: PathBuf,
    file: File,
    bytes: u64,
}

impl Snapshot {
    fn create(source: &Path) -> Result<Self, String> {
        let mut original = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(source)
            .map_err(|error| format!("Cannot open selected file: {error}"))?;
        let before = original.metadata().map_err(|error| error.to_string())?;
        if !before.is_file() || before.len() > MAX_UPLOAD_BYTES {
            return Err("Upload requires one regular file of at most 8 GiB".into());
        }
        let directory = (0..16)
            .find_map(|_| {
                let suffix = random_suffix().ok()?;
                let directory = std::env::temp_dir().join(format!("ricefield-upload-{suffix}"));
                match DirBuilder::new().mode(0o700).create(&directory) {
                    Ok(()) => Some(directory),
                    _ => None,
                }
            })
            .ok_or("Cannot create private upload snapshot directory")?;
        let path = directory.join("source.snapshot");
        let result = (|| {
            let mut output = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&path)
                .map_err(|error| error.to_string())?;
            let bytes = io::copy(&mut (&mut original).take(MAX_UPLOAD_BYTES + 1), &mut output)
                .map_err(|error| error.to_string())?;
            let after = original.metadata().map_err(|error| error.to_string())?;
            if bytes != before.len()
                || before.len() != after.len()
                || before.mtime() != after.mtime()
                || before.mtime_nsec() != after.mtime_nsec()
                || before.ctime() != after.ctime()
                || before.ctime_nsec() != after.ctime_nsec()
            {
                return Err("Selected source changed during snapshot; upload refused".into());
            }
            output.sync_all().map_err(|error| error.to_string())?;
            let file = File::open(&path).map_err(|error| error.to_string())?;
            Ok((file, bytes))
        })();
        match result {
            Ok((file, bytes)) => Ok(Self {
                directory,
                path,
                file,
                bytes,
            }),
            Err(error) => {
                let _ = fs::remove_file(&path);
                let _ = fs::remove_dir(&directory);
                Err(error)
            }
        }
    }
}

impl Drop for Snapshot {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
        let _ = fs::remove_file(self.directory.join("probe-a.payload"));
        let _ = fs::remove_file(self.directory.join("probe-b.payload"));
        let _ = fs::remove_dir(&self.directory);
    }
}

fn verify_remote_bytes(
    uri: &str,
    credentials: &MountCredentials,
    snapshot: &mut Snapshot,
    cancelled: &AtomicBool,
) -> Result<(), String> {
    let (mut command, config) = curl_command(credentials)?;
    command.args([
        "--request",
        "GET",
        "--fail",
        "--speed-limit",
        "1",
        "--speed-time",
        "30",
    ]);
    command.arg("--").arg(uri).stdout(Stdio::piped());
    let mut child = launch(command, config)?;
    snapshot
        .file
        .seek(SeekFrom::Start(0))
        .map_err(|error| error.to_string())?;
    let result = (|| -> Result<(), String> {
        let mut remote = child.stdout.take().ok_or("Missing network response")?;
        let mut received = 0_u64;
        let mut network_buffer = [0_u8; 32 * 1024];
        let mut local_buffer = [0_u8; 32 * 1024];
        loop {
            if cancelled.load(Ordering::Relaxed) {
                return Err("Upload cancelled during remote verification".into());
            }
            let size = remote
                .read(&mut network_buffer)
                .map_err(|error| format!("Cannot read staged remote file: {error}"))?;
            if size == 0 {
                break;
            }
            received = received
                .checked_add(size as u64)
                .ok_or("Remote stage exceeds the size limit")?;
            if received > snapshot.bytes {
                return Err("Remote stage contains unexpected additional bytes".into());
            }
            snapshot
                .file
                .read_exact(&mut local_buffer[..size])
                .map_err(|error| format!("Snapshot changed during verification: {error}"))?;
            if network_buffer[..size] != local_buffer[..size] {
                return Err("Remote staged bytes differ from the pinned local snapshot".into());
            }
        }
        if received != snapshot.bytes {
            return Err("Remote stage is incomplete".into());
        }
        let mut tail = [0_u8; 1];
        if snapshot
            .file
            .read(&mut tail)
            .map_err(|error| error.to_string())?
            != 0
        {
            return Err("Local snapshot changed during verification".into());
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = child.kill();
    }
    let finished = child.wait().map_err(|error| error.to_string())?;
    result?;
    if !finished.success() {
        return Err("GET verification failed on remote server".into());
    }
    Ok(())
}

fn verify_probe_contents(
    uri: &str,
    credentials: &MountCredentials,
    expected: &[u8],
    cancelled: &AtomicBool,
) -> Result<(), String> {
    let (mut command, config) = curl_command(credentials)?;
    command.args([
        "--request",
        "GET",
        "--fail",
        "--speed-limit",
        "1",
        "--speed-time",
        "15",
    ]);
    command.arg("--").arg(uri).stdout(Stdio::piped());
    let mut child = launch(command, config)?;
    let mut content = Vec::new();
    let result = child
        .stdout
        .take()
        .ok_or("Missing WebDAV probe response")?
        .take(129)
        .read_to_end(&mut content)
        .map_err(|error| format!("Cannot verify WebDAV probe contents: {error}"));
    if result.is_err() || content.len() > 128 {
        let _ = child.kill();
    }
    let status = child.wait().map_err(|error| error.to_string())?;
    result?;
    if !status.success() || content != expected {
        return Err(
            "WebDAV server returned inconsistent bytes after conditional MOVE; upload refused"
                .into(),
        );
    }
    if cancelled.load(Ordering::Relaxed) {
        return Err("Upload cancelled during probe verification".into());
    }
    Ok(())
}

/// A disposable, user-owned capability probe. Every name is random and private
/// to this attempt. The second MOVE must be rejected at the server, and the
/// first payload must remain intact. Failure leaves evidence, not a fallback.
fn probe_no_replace(
    base: &str,
    credentials: &MountCredentials,
    snapshot: &mut Snapshot,
    cancelled: &AtomicBool,
) -> Result<(), String> {
    let prefix = format!(
        "{base}/.ricefield-safety-{}",
        random_suffix().map_err(|error| error.to_string())?
    );
    let probe_a = b"ricefield-safe-publish-probe-a";
    let probe_b = b"ricefield-safe-publish-probe-b";
    let local_a = snapshot.directory.join("probe-a.payload");
    let local_b = snapshot.directory.join("probe-b.payload");
    fs::write(&local_a, probe_a)
        .map_err(|error| format!("Cannot write private safety probe: {error}"))?;
    fs::write(&local_b, probe_b)
        .map_err(|error| format!("Cannot write private safety probe: {error}"))?;
    let move_header =
        |destination: &str| vec![format!("Destination: {destination}"), "Overwrite: F".into()];
    for round in 0..4 {
        let stage_a = format!("{prefix}-{round}-a");
        let stage_b = format!("{prefix}-{round}-b");
        let target = format!("{prefix}-{round}-destination");
        for (stage, source) in [(&stage_a, &local_a), (&stage_b, &local_b)] {
            let status = http_action(
                credentials,
                "PUT",
                stage,
                &["If-None-Match: *".into()],
                Some(source),
                cancelled,
            )?;
            if status != HTTP_NEW {
                return Err(format!(
                    "WebDAV server cannot conditionally create private stages (HTTP {status})"
                ));
            }
        }
        let stage_collision = http_action(
            credentials,
            "PUT",
            &stage_a,
            &["If-None-Match: *".into()],
            Some(&local_a),
            cancelled,
        )?;
        if stage_collision != HTTP_PRECONDITION_FAILED {
            return Err(format!(
                "WebDAV conditional PUT is unsafe (existing stage returned HTTP {stage_collision}); uploads disabled"
            ));
        }
        let (first, second) = if round == 0 {
            // Sequential collision catches servers that ignore Overwrite:F.
            (
                http_action(
                    credentials,
                    "MOVE",
                    &stage_a,
                    &move_header(&target),
                    None,
                    cancelled,
                ),
                http_action(
                    credentials,
                    "MOVE",
                    &stage_b,
                    &move_header(&target),
                    None,
                    cancelled,
                ),
            )
        } else {
            // Separate curl clients start together against the same target.
            // A server performing check-then-rename may let *both* succeed.
            thread::scope(|scope| {
                let first = scope.spawn(|| {
                    http_action(
                        credentials,
                        "MOVE",
                        &stage_a,
                        &move_header(&target),
                        None,
                        cancelled,
                    )
                });
                let second = scope.spawn(|| {
                    http_action(
                        credentials,
                        "MOVE",
                        &stage_b,
                        &move_header(&target),
                        None,
                        cancelled,
                    )
                });
                Ok::<_, String>((
                    first.join().map_err(|_| "WebDAV safety probe panicked")?,
                    second.join().map_err(|_| "WebDAV safety probe panicked")?,
                ))
            })?
        };
        let (first, second) = (first?, second?);
        let winner_a = first == HTTP_NEW && second == HTTP_PRECONDITION_FAILED;
        let winner_b = second == HTTP_NEW && first == HTTP_PRECONDITION_FAILED;
        if !winner_a && !winner_b {
            return Err(format!(
                "WebDAV conditional MOVE collision was unsafe (HTTP {first}, HTTP {second}); remote writes disabled"
            ));
        }
        verify_probe_contents(
            &target,
            credentials,
            if winner_a { probe_a } else { probe_b },
            cancelled,
        )?;
        // Only clean up private, unpredictable probe names after verifying
        // the server refused the losing publisher, never client data.
        let loser = if winner_a { &stage_b } else { &stage_a };
        for uri in [loser, &target] {
            let status = http_action(credentials, "DELETE", uri, &[], None, cancelled)?;
            if !matches!(status, 200 | 204) {
                return Err(format!(
                    "Safety-probe cleanup failed (HTTP {status}); upload refused"
                ));
            }
        }
    }
    Ok(())
}

pub fn upload(request: UploadRequest, cancelled: &AtomicBool) -> Result<UploadResult, String> {
    let base = webdav_url(request.remote_directory.trim_end_matches('/'))?;
    let authority = base
        .split_once("://")
        .and_then(|(_, host)| host.split('/').next())
        .ok_or("WebDAV address has no host")?;
    let is_loopback = matches!(authority, "localhost" | "127.0.0.1" | "[::1]")
        || authority.starts_with("127.0.0.1:")
        || authority.starts_with("[::1]:");
    if base.starts_with("http://") && !is_loopback && !request.allow_insecure_http {
        return Err("Unencrypted WebDAV may expose passwords and file contents. Use davs:// or explicitly allow plain HTTP in the upload dialog".into());
    }
    let filename = request
        .source
        .file_name()
        .ok_or("Select an existing regular file to upload")?;
    let name = escape_filename(filename)?;
    let uri = format!("{base}/{name}");
    let credentials =
        if request.credentials.username.is_empty() && request.credentials.password.is_empty() {
            keyring::lookup(&request.remote_directory)
                .map_err(|error| format!("Cannot access saved WebDAV credentials: {error}"))?
                .ok_or("No saved login for this directory; supply a WebDAV username and password")?
        } else {
            request.credentials
        };
    let mut snapshot = Snapshot::create(&request.source)?;
    probe_no_replace(&base, &credentials, &mut snapshot, cancelled)?;
    let stage = format!(
        "{base}/.ricefield-upload-{}",
        random_suffix().map_err(|error| error.to_string())?
    );
    let created = http_action(
        &credentials,
        "PUT",
        &stage,
        &["If-None-Match: *".into()],
        Some(&snapshot.path),
        cancelled,
    )
    .map_err(|error| staged_upload_error(&stage, &error))?;
    if created != HTTP_NEW {
        return Err(staged_upload_error(
            &stage,
            &format!("Server refused exclusive stage creation (HTTP {created})"),
        ));
    }
    verify_remote_bytes(&stage, &credentials, &mut snapshot, cancelled)
        .map_err(|error| staged_upload_error(&stage, &error))?;
    if cancelled.load(Ordering::Relaxed) {
        return Err(staged_upload_error(
            &stage,
            "Upload cancelled before publication",
        ));
    }
    // Once publication starts, cancelling can produce an ambiguous outcome:
    // the server may commit before the client notices the broken connection.
    // Let this critical request finish or time out, and report its response.
    let publishing = AtomicBool::new(false);
    let committed = http_action(
        &credentials,
        "MOVE",
        &stage,
        &[format!("Destination: {uri}"), "Overwrite: F".into()],
        None,
        &publishing,
    )
    .map_err(|error| staged_upload_error(&stage, &error))?;
    if committed != HTTP_NEW {
        return Err(staged_upload_error(
            &stage,
            &format!(
                "Remote destination was not acknowledged as published (HTTP {committed}); pre-existing data was not intentionally replaced"
            ),
        ));
    }
    Ok(UploadResult {
        uri: request.remote_directory.trim_end_matches('/').to_owned() + "/" + &name,
        bytes: snapshot.bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        collections::HashMap,
        ffi::OsStr,
        io::{BufRead, BufReader},
        net::{TcpListener, TcpStream},
        sync::{Arc, Mutex},
    };

    struct MockDav {
        url: String,
        state: Arc<Mutex<HashMap<String, Vec<u8>>>>,
        running: Arc<AtomicBool>,
        worker: Option<thread::JoinHandle<()>>,
    }

    impl MockDav {
        fn start(replace_on_move: bool, replace_on_put: bool, require_auth: bool) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let address = listener.local_addr().unwrap();
            let state = Arc::new(Mutex::new(HashMap::new()));
            let running = Arc::new(AtomicBool::new(true));
            let files = Arc::clone(&state);
            let alive = Arc::clone(&running);
            let worker = thread::spawn(move || {
                while alive.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            let files = Arc::clone(&files);
                            thread::spawn(move || {
                                handle_dav(
                                    stream,
                                    &files,
                                    replace_on_move,
                                    replace_on_put,
                                    require_auth,
                                )
                            });
                        }
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(5));
                        }
                        Err(error) => panic!("mock WebDAV accept failed: {error}"),
                    }
                }
            });
            Self {
                url: format!("dav://{address}/share"),
                state,
                running,
                worker: Some(worker),
            }
        }

        fn path(&self, name: &str) -> Vec<u8> {
            self.state
                .lock()
                .unwrap()
                .get(&format!("/share/{name}"))
                .cloned()
                .unwrap_or_default()
        }
    }

    impl Drop for MockDav {
        fn drop(&mut self) {
            self.running.store(false, Ordering::Relaxed);
            if let Some(worker) = self.worker.take() {
                worker.join().unwrap();
            }
        }
    }

    fn handle_dav(
        mut stream: TcpStream,
        files: &Arc<Mutex<HashMap<String, Vec<u8>>>>,
        weak_move: bool,
        weak_put: bool,
        require_auth: bool,
    ) {
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut request_line = String::new();
        if reader.read_line(&mut request_line).is_err() {
            return;
        }
        let mut parts = request_line.split_whitespace();
        let method = parts.next().unwrap_or("");
        let path = parts.next().unwrap_or("");
        let mut headers = HashMap::<String, String>::new();
        loop {
            let mut header = String::new();
            if reader.read_line(&mut header).unwrap_or(0) == 0 || header == "\r\n" {
                break;
            }
            if let Some((name, value)) = header.split_once(':') {
                headers.insert(name.to_ascii_lowercase(), value.trim().to_owned());
            }
        }
        if require_auth
            && headers.get("authorization").map(String::as_str)
                != Some("Basic dGVzdGVyOnNlY3JldA==")
        {
            let _ = stream.write_all(b"HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Basic realm=\"RiceField fixture\"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            return;
        }
        let length: usize = headers
            .get("content-length")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        if length > 1024 * 1024 {
            return;
        }
        let mut body = vec![0; length];
        if reader.read_exact(&mut body).is_err() {
            return;
        }
        let mut entries = files.lock().unwrap();
        let (status, response) = match method {
            "PUT" => {
                if !weak_put
                    && headers
                        .get("if-none-match")
                        .is_some_and(|value| value == "*")
                    && entries.contains_key(path)
                {
                    (412, Vec::new())
                } else {
                    entries.insert(path.into(), body);
                    (201, Vec::new())
                }
            }
            "MOVE" => {
                let destination = headers
                    .get("destination")
                    .and_then(|value| value.split_once("://"))
                    .and_then(|(_, suffix)| suffix.split_once('/'))
                    .map(|(_, suffix)| format!("/{suffix}"));
                match destination {
                    Some(dest)
                        if !weak_move
                            && headers.get("overwrite").is_some_and(|h| h == "F")
                            && entries.contains_key(&dest) =>
                    {
                        (412, Vec::new())
                    }
                    Some(dest) => match entries.remove(path) {
                        Some(bytes) => {
                            entries.insert(dest, bytes);
                            (201, Vec::new())
                        }
                        None => (404, Vec::new()),
                    },
                    None => (400, Vec::new()),
                }
            }
            "GET" => entries
                .get(path)
                .map(|bytes| (200, bytes.clone()))
                .unwrap_or((404, Vec::new())),
            "DELETE" => {
                if entries.remove(path).is_some() {
                    (204, Vec::new())
                } else {
                    (404, Vec::new())
                }
            }
            _ => (405, Vec::new()),
        };
        drop(entries);
        let text = format!(
            "HTTP/1.1 {status} Mock\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            response.len()
        );
        let _ = stream.write_all(text.as_bytes());
        let _ = stream.write_all(&response);
    }

    fn fixture_file(name: &str) -> (PathBuf, PathBuf) {
        let path = std::env::temp_dir().join(format!(
            "ricefield-webdav-test-{}",
            random_suffix().unwrap()
        ));
        fs::create_dir(&path).unwrap();
        let file = path.join(name);
        fs::write(&file, b"verified upload bytes").unwrap();
        (path, file)
    }

    fn request(file: PathBuf, server: &MockDav) -> UploadRequest {
        UploadRequest {
            source: file,
            remote_directory: server.url.clone(),
            allow_insecure_http: false,
            credentials: MountCredentials {
                username: "tester".into(),
                domain: String::new(),
                password: "secret".into(),
            },
        }
    }

    #[test]
    fn webdav_publish_verifies_private_stage_and_never_replaces_existing() {
        let server = MockDav::start(false, false, true);
        let (folder, file) = fixture_file("My Notes.txt");
        let report = upload(request(file.clone(), &server), &AtomicBool::new(false)).unwrap();
        assert_eq!(report.bytes, 21);
        assert_eq!(report.uri, format!("{}/My%20Notes.txt", server.url));
        assert_eq!(server.path("My%20Notes.txt"), b"verified upload bytes");
        assert_eq!(fs::read(&file).unwrap(), b"verified upload bytes");
        let collision =
            upload(request(file.clone(), &server), &AtomicBool::new(false)).unwrap_err();
        assert!(collision.contains("HTTP 412"), "{collision}");
        assert!(
            collision.contains(&format!("{}/.ricefield-upload-", server.url)),
            "{collision}"
        );
        assert!(
            collision.contains("Check both the final destination"),
            "{collision}"
        );
        assert_eq!(server.path("My%20Notes.txt"), b"verified upload bytes");
        fs::remove_file(file).unwrap();
        fs::remove_dir(folder).unwrap();
    }

    #[test]
    fn server_that_ignores_move_preconditions_is_blocked_before_publication() {
        let server = MockDav::start(true, false, false);
        let (folder, file) = fixture_file("critical.txt");
        let result = upload(request(file.clone(), &server), &AtomicBool::new(false)).unwrap_err();
        assert!(result.contains("unsafe"), "{result}");
        assert!(server.path("critical.txt").is_empty());
        assert_eq!(fs::read(&file).unwrap(), b"verified upload bytes");
        fs::remove_file(file).unwrap();
        fs::remove_dir(folder).unwrap();
    }

    #[test]
    fn server_that_ignores_put_preconditions_is_blocked_before_publication() {
        let server = MockDav::start(false, true, false);
        let (folder, file) = fixture_file("private.txt");
        let result = upload(request(file.clone(), &server), &AtomicBool::new(false)).unwrap_err();
        assert!(result.contains("conditional PUT is unsafe"), "{result}");
        assert!(server.path("private.txt").is_empty());
        assert_eq!(fs::read(&file).unwrap(), b"verified upload bytes");
        fs::remove_file(file).unwrap();
        fs::remove_dir(folder).unwrap();
    }

    #[test]
    fn non_webdav_backends_are_fail_closed() {
        assert!(webdav_url("smb://host/share").is_err());
        assert!(webdav_url("sftp://host/home").is_err());
        assert!(webdav_url("mtp://host/root").is_err());
        assert_eq!(
            webdav_url("davs://host:443/path").unwrap(),
            "https://host:443/path"
        );
        assert_eq!(
            webdav_url("dav://host:8080/path").unwrap(),
            "http://host:8080/path"
        );
    }

    #[test]
    fn plaintext_network_login_requires_explicit_opt_in() {
        let request = UploadRequest {
            source: PathBuf::from("/nonexistent-plain-http-file"),
            remote_directory: "dav://192.0.2.24/share".into(),
            credentials: MountCredentials {
                username: "user".into(),
                domain: String::new(),
                password: "secret".into(),
            },
            allow_insecure_http: false,
        };
        let result = upload(request, &AtomicBool::new(false)).unwrap_err();
        assert!(result.contains("Unencrypted WebDAV"), "{result}");
    }

    #[test]
    fn remote_names_are_encoded_without_path_interpolation() {
        assert_eq!(
            escape_filename(OsStr::new("a b#%.txt")).unwrap(),
            "a%20b%23%25.txt"
        );
        assert!(escape_filename(OsStr::new("..")).is_err());
    }

    #[test]
    fn local_source_symlinks_are_refused_without_touching_their_targets() {
        let (folder, source) = fixture_file("source.txt");
        let link = folder.join("link.txt");
        std::os::unix::fs::symlink(&source, &link).unwrap();
        assert!(Snapshot::create(&link).is_err());
        assert_eq!(fs::read(&source).unwrap(), b"verified upload bytes");
        fs::remove_file(link).unwrap();
        fs::remove_file(source).unwrap();
        fs::remove_dir(folder).unwrap();
    }

    #[test]
    fn cancellation_before_network_publication_leaves_source_untouched() {
        let server = MockDav::start(false, false, false);
        let (folder, file) = fixture_file("offline.txt");
        let cancelled = AtomicBool::new(true);
        let result = upload(request(file.clone(), &server), &cancelled).unwrap_err();
        assert!(result.contains("cancelled"), "{result}");
        assert!(server.path("offline.txt").is_empty());
        assert_eq!(fs::read(&file).unwrap(), b"verified upload bytes");
        fs::remove_file(file).unwrap();
        fs::remove_dir(folder).unwrap();
    }

    #[test]
    fn password_never_enters_curl_arguments_or_unescaped_config_lines() {
        let credentials = MountCredentials {
            username: "name".into(),
            domain: "domain".into(),
            password: "p\"\\ss".into(),
        };
        let (command, config) = curl_command(&credentials).unwrap();
        assert!(!format!("{command:?}").contains("p\"\\ss"));
        assert_eq!(config, "user = \"domain\\\\name:p\\\"\\\\ss\"\nanyauth\n");
    }
}
