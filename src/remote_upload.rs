//! Server-conditioned WebDAV uploads. Generic GIO move/copy commands are NOT
//! safe no-replace primitives (see docs/REMOTE-WRITE-SAFETY.md). SFTP uses
//! a separately verified hardlink backend; SMB/MTP remain read-only.

use crate::{
    keyring,
    location::{Location, RemoteScheme},
    remote_upload_journal,
    volume::MountCredentials,
};
use std::{
    fs::{self, DirBuilder, File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    os::fd::AsRawFd,
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

pub(crate) fn escape_filename(name: &std::ffi::OsStr) -> Result<String, String> {
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

pub(crate) fn random_suffix() -> io::Result<String> {
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

/// A blocking curl stdout read would otherwise prevent Cancel from taking
/// effect until the remote server sends more bytes or curl times out.
fn read_with_cancel(
    reader: &mut (impl Read + AsRawFd),
    buffer: &mut [u8],
    cancelled: &AtomicBool,
) -> io::Result<usize> {
    loop {
        if cancelled.load(Ordering::Relaxed) {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "Upload cancelled",
            ));
        }
        let mut ready = libc::pollfd {
            fd: reader.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: poll borrows one valid file descriptor; the reader owns it
        // throughout this call. No other thread reads from this pipe.
        let count = unsafe { libc::poll(&mut ready, 1, 100) };
        if count > 0 {
            if cancelled.load(Ordering::Relaxed) {
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "Upload cancelled",
                ));
            }
            return reader.read(buffer);
        }
        if count < 0 {
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
    }
}

/// Snapshotting an 8 GiB source may take minutes on a slow drive. Do not
/// postpone Cancel until after the entire local copy and its disk sync.
fn copy_snapshot_with_cancel(
    source: &mut impl Read,
    destination: &mut impl Write,
    cancelled: &AtomicBool,
) -> io::Result<u64> {
    let mut buffer = [0_u8; 256 * 1024];
    let mut copied = 0_u64;
    loop {
        if cancelled.load(Ordering::Relaxed) {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "Upload cancelled during local snapshot",
            ));
        }
        let size = match source.read(&mut buffer) {
            Ok(size) => size,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        if size == 0 {
            return Ok(copied);
        }
        if cancelled.load(Ordering::Relaxed) {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "Upload cancelled during local snapshot",
            ));
        }
        destination.write_all(&buffer[..size])?;
        copied = copied
            .checked_add(size as u64)
            .ok_or_else(|| io::Error::other("Upload snapshot size overflow"))?;
    }
}

pub(crate) struct Snapshot {
    directory: PathBuf,
    path: PathBuf,
    file: File,
    pub(crate) bytes: u64,
}

impl Snapshot {
    pub(crate) fn create(source: &Path, cancelled: &AtomicBool) -> Result<Self, String> {
        if cancelled.load(Ordering::Relaxed) {
            return Err("Upload cancelled before local snapshot".into());
        }
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
            let bytes = copy_snapshot_with_cancel(
                &mut (&mut original).take(MAX_UPLOAD_BYTES + 1),
                &mut output,
                cancelled,
            )
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
            if cancelled.load(Ordering::Relaxed) {
                return Err("Upload cancelled before snapshot disk sync".into());
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

impl Snapshot {
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn private_file(&self, filename: &str) -> PathBuf {
        self.directory.join(filename)
    }
}

impl Drop for Snapshot {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
        let _ = fs::remove_file(self.directory.join("probe-a.payload"));
        let _ = fs::remove_file(self.directory.join("probe-b.payload"));
        let _ = fs::remove_file(self.directory.join("sftp-verify.payload"));
        let _ = fs::remove_file(self.directory.join("smb-probe-a.payload"));
        let _ = fs::remove_file(self.directory.join("smb-probe-b.payload"));
        let _ = fs::remove_file(self.directory.join("smb-verify.payload"));
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
            let size = read_with_cancel(&mut remote, &mut network_buffer, cancelled)
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
    let mut remote = child.stdout.take().ok_or("Missing WebDAV probe response")?;
    let mut content = Vec::new();
    let result = (|| -> Result<(), String> {
        let mut buffer = [0_u8; 129];
        while content.len() < buffer.len() {
            let size = read_with_cancel(&mut remote, &mut buffer[..129 - content.len()], cancelled)
                .map_err(|error| format!("Cannot verify WebDAV probe contents: {error}"))?;
            if size == 0 {
                break;
            }
            content.extend_from_slice(&buffer[..size]);
        }
        Ok(())
    })();
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
    journal_override: Option<&Path>,
) -> Result<(), String> {
    let prefix = format!(
        "{base}/.ricefield-safety-{}",
        random_suffix().map_err(|error| error.to_string())?
    );
    let pending = match journal_override {
        Some(directory) => remote_upload_journal::PendingProbe::begin_in(directory, &prefix),
        None => remote_upload_journal::PendingProbe::begin(&prefix),
    }
    .map_err(|error| {
        format!(
            "Cannot durably record WebDAV safety probe before PUT: {error}. No remote probe was started"
        )
    })?;
    run_probe_rounds(&prefix, credentials, snapshot, cancelled).map_err(|error| {
        format!(
            "{error}. Private WebDAV safety-probe objects may remain under {prefix}- (rounds 0–3; suffixes -a, -b and -destination). Inspect those objects and the target before manual cleanup; RiceField will not retry an uncertain probe"
        )
    })?;
    pending.finish();
    Ok(())
}

fn run_probe_rounds(
    prefix: &str,
    credentials: &MountCredentials,
    snapshot: &mut Snapshot,
    cancelled: &AtomicBool,
) -> Result<(), String> {
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
    upload_with_journal_directory(request, cancelled, None)
}

fn upload_with_journal_directory(
    request: UploadRequest,
    cancelled: &AtomicBool,
    journal_override: Option<&Path>,
) -> Result<UploadResult, String> {
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
    let mut snapshot = Snapshot::create(&request.source, cancelled)?;
    probe_no_replace(
        &base,
        &credentials,
        &mut snapshot,
        cancelled,
        journal_override,
    )?;
    let stage = format!(
        "{base}/.ricefield-upload-{}",
        random_suffix().map_err(|error| error.to_string())?
    );
    // Refuse a remote write until the stage and intended destination are
    // durably recorded. Timeout and crash outcomes must survive restarts.
    let pending = match journal_override {
        Some(directory) => remote_upload_journal::PendingUpload::begin_in(directory, &stage, &uri),
        None => remote_upload_journal::PendingUpload::begin(&stage, &uri),
    }
    .map_err(|error| {
        format!("Cannot durably record WebDAV upload before PUT: {error}. No data was uploaded")
    })?;
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
    // A confirmed server acknowledgement retires only local evidence. If
    // disk retirement fails, the record remains for explicit later review.
    pending.finish();
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
        os::unix::net::UnixStream,
        sync::{Arc, Mutex},
    };

    struct MockDav {
        url: String,
        state: Arc<Mutex<HashMap<String, Vec<u8>>>>,
        running: Arc<AtomicBool>,
        stall_stage_response: Arc<AtomicBool>,
        stage_put_seen: Arc<AtomicBool>,
        stall_probe_response: Arc<AtomicBool>,
        probe_put_seen: Arc<AtomicBool>,
        worker: Option<thread::JoinHandle<()>>,
    }

    struct StallSignals<'a> {
        stage_response: &'a AtomicBool,
        stage_seen: &'a AtomicBool,
        probe_response: &'a AtomicBool,
        probe_seen: &'a AtomicBool,
    }

    impl MockDav {
        fn start(replace_on_move: bool, replace_on_put: bool, require_auth: bool) -> Self {
            Self::start_with_stage_stall(replace_on_move, replace_on_put, require_auth, false)
        }

        fn start_with_stage_stall(
            replace_on_move: bool,
            replace_on_put: bool,
            require_auth: bool,
            stall_stage: bool,
        ) -> Self {
            Self::start_with_stalls(
                replace_on_move,
                replace_on_put,
                require_auth,
                stall_stage,
                false,
            )
        }

        fn start_with_probe_stall() -> Self {
            Self::start_with_stalls(false, false, false, false, true)
        }

        fn start_with_stalls(
            replace_on_move: bool,
            replace_on_put: bool,
            require_auth: bool,
            stall_stage: bool,
            stall_probe: bool,
        ) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let address = listener.local_addr().unwrap();
            let state = Arc::new(Mutex::new(HashMap::new()));
            let running = Arc::new(AtomicBool::new(true));
            let stall_stage_response = Arc::new(AtomicBool::new(stall_stage));
            let stage_put_seen = Arc::new(AtomicBool::new(false));
            let stall_probe_response = Arc::new(AtomicBool::new(stall_probe));
            let probe_put_seen = Arc::new(AtomicBool::new(false));
            let files = Arc::clone(&state);
            let alive = Arc::clone(&running);
            let stage_stall = Arc::clone(&stall_stage_response);
            let put_seen = Arc::clone(&stage_put_seen);
            let probe_stall = Arc::clone(&stall_probe_response);
            let probe_seen = Arc::clone(&probe_put_seen);
            let worker = thread::spawn(move || {
                while alive.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            let files = Arc::clone(&files);
                            let stage_stall = Arc::clone(&stage_stall);
                            let put_seen = Arc::clone(&put_seen);
                            let probe_stall = Arc::clone(&probe_stall);
                            let probe_seen = Arc::clone(&probe_seen);
                            thread::spawn(move || {
                                handle_dav(
                                    stream,
                                    &files,
                                    replace_on_move,
                                    replace_on_put,
                                    require_auth,
                                    StallSignals {
                                        stage_response: stage_stall.as_ref(),
                                        stage_seen: put_seen.as_ref(),
                                        probe_response: probe_stall.as_ref(),
                                        probe_seen: probe_seen.as_ref(),
                                    },
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
                stall_stage_response,
                stage_put_seen,
                stall_probe_response,
                probe_put_seen,
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
            self.stall_stage_response.store(false, Ordering::Release);
            self.stall_probe_response.store(false, Ordering::Release);
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
        stalls: StallSignals<'_>,
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
        // An actual child process can be killed after the server has stored
        // the private stage, while its HTTP request is still incomplete.
        if method == "PUT" && path.contains("/.ricefield-upload-") && status == 201 {
            stalls.stage_seen.store(true, Ordering::Release);
            while stalls.stage_response.load(Ordering::Acquire) {
                thread::sleep(Duration::from_millis(5));
            }
        }
        if method == "PUT" && path.contains("/.ricefield-safety-") && status == 201 {
            stalls.probe_seen.store(true, Ordering::Release);
            while stalls.probe_response.load(Ordering::Acquire) {
                thread::sleep(Duration::from_millis(5));
            }
        }
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

    // Mock server tests never write into a developer's real XDG state.
    fn upload(request: UploadRequest, cancelled: &AtomicBool) -> Result<UploadResult, String> {
        let directory = std::env::temp_dir().join(format!(
            "ricefield-webdav-mock-journals-{}",
            random_suffix().unwrap()
        ));
        let result = super::upload_with_journal_directory(request, cancelled, Some(&directory));
        if directory.exists() {
            fs::remove_dir_all(directory).unwrap();
        }
        result
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
            collision.contains(&format!(
                "{}/.ricefield-upload-",
                webdav_url(&server.url).unwrap()
            )),
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
    fn uncertain_publication_retains_private_evidence_and_success_retires_it() {
        let server = MockDav::start(false, false, false);
        let (folder, file) = fixture_file("evidence.txt");
        let journals = folder.join("journals");
        super::upload_with_journal_directory(
            request(file.clone(), &server),
            &AtomicBool::new(false),
            Some(&journals),
        )
        .unwrap();
        assert_eq!(
            fs::read_dir(&journals)
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| {
                    entry
                        .path()
                        .extension()
                        .is_some_and(|ext| ext == "toml" || ext == "probe")
                })
                .count(),
            0,
        );
        let error = super::upload_with_journal_directory(
            request(file.clone(), &server),
            &AtomicBool::new(false),
            Some(&journals),
        )
        .unwrap_err();
        assert!(error.contains("HTTP 412"), "{error}");
        let mut records: Vec<_> = fs::read_dir(&journals)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "toml"))
            .collect();
        assert_eq!(records.len(), 1);
        let persisted = fs::read_to_string(records.pop().unwrap().path()).unwrap();
        assert!(persisted.contains(".ricefield-upload-"));
        assert!(persisted.contains("evidence.txt"));
        assert!(!persisted.contains("password"));
        assert!(!persisted.contains("secret"));
        fs::remove_dir_all(journals).unwrap();
        fs::remove_file(file).unwrap();
        fs::remove_dir(folder).unwrap();
    }

    #[test]
    fn unwritable_journal_fails_closed_before_remote_stage_put() {
        let server = MockDav::start(false, false, false);
        let (folder, file) = fixture_file("unwritten.txt");
        let not_a_directory = folder.join("journal-blocker");
        fs::write(&not_a_directory, b"already a file").unwrap();
        let error = super::upload_with_journal_directory(
            request(file.clone(), &server),
            &AtomicBool::new(false),
            Some(&not_a_directory),
        )
        .unwrap_err();
        assert!(error.contains("Cannot durably record"), "{error}");
        assert!(server.path("unwritten.txt").is_empty());
        assert!(
            !server
                .state
                .lock()
                .unwrap()
                .keys()
                .any(|path| path.contains(".ricefield-upload-")
                    || path.contains(".ricefield-safety-"))
        );
        fs::remove_file(not_a_directory).unwrap();
        fs::remove_file(file).unwrap();
        fs::remove_dir(folder).unwrap();
    }

    #[test]
    fn abrupt_process_kill_during_stage_put_preserves_restart_evidence() {
        const CHILD_SERVER: &str = "RICEFIELD_TEST_CRASH_DAV_SERVER";
        const CHILD_SOURCE: &str = "RICEFIELD_TEST_CRASH_DAV_SOURCE";
        const CHILD_JOURNAL: &str = "RICEFIELD_TEST_CRASH_DAV_JOURNAL";
        if let Ok(url) = std::env::var(CHILD_SERVER) {
            let request = UploadRequest {
                source: PathBuf::from(std::env::var_os(CHILD_SOURCE).unwrap()),
                remote_directory: url,
                credentials: MountCredentials {
                    username: "tester".into(),
                    domain: String::new(),
                    password: "secret".into(),
                },
                allow_insecure_http: false,
            };
            let directory = PathBuf::from(std::env::var_os(CHILD_JOURNAL).unwrap());
            let _ = super::upload_with_journal_directory(
                request,
                &AtomicBool::new(false),
                Some(&directory),
            );
            panic!("crash fixture must be killed while stage response is withheld");
        }

        let server = MockDav::start_with_stage_stall(false, false, false, true);
        let (folder, file) = fixture_file("crash.txt");
        let journals = folder.join("state");
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("remote_upload::tests::abrupt_process_kill_during_stage_put_preserves_restart_evidence")
            .arg("--exact")
            .env(CHILD_SERVER, &server.url)
            .env(CHILD_SOURCE, &file)
            .env(CHILD_JOURNAL, &journals)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        while !server.stage_put_seen.load(Ordering::Acquire) && Instant::now() < deadline {
            if let Some(status) = child.try_wait().unwrap() {
                panic!("upload fixture exited before private stage PUT: {status}");
            }
            thread::sleep(Duration::from_millis(10));
        }
        if !server.stage_put_seen.load(Ordering::Acquire) {
            let _ = child.kill();
            let _ = child.wait();
            panic!("upload fixture did not reach the private stage PUT");
        }
        child.kill().unwrap();
        assert!(!child.wait().unwrap().success());
        server.stall_stage_response.store(false, Ordering::Release);

        let entries: Vec<_> = fs::read_dir(&journals)
            .unwrap()
            .map(Result::unwrap)
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "toml"))
            .collect();
        assert_eq!(entries.len(), 1, "crashed process lost durable evidence");
        let saved = fs::read_to_string(entries[0].path()).unwrap();
        let stages: Vec<_> = server
            .state
            .lock()
            .unwrap()
            .keys()
            .filter(|path| path.contains("/.ricefield-upload-"))
            .cloned()
            .collect();
        assert_eq!(stages.len(), 1, "mock server lost private stage");
        assert!(saved.contains(&stages[0]));
        assert!(saved.contains("crash.txt"));
        assert!(!saved.contains("secret"));
        assert!(server.path("crash.txt").is_empty());
        assert_eq!(fs::read(&file).unwrap(), b"verified upload bytes");
        fs::remove_dir_all(journals).unwrap();
        fs::remove_file(file).unwrap();
        fs::remove_dir(folder).unwrap();
    }

    #[test]
    fn abrupt_process_kill_during_probe_put_preserves_restart_evidence() {
        const CHILD_SERVER: &str = "RICEFIELD_TEST_CRASH_PROBE_SERVER";
        const CHILD_SOURCE: &str = "RICEFIELD_TEST_CRASH_PROBE_SOURCE";
        const CHILD_JOURNAL: &str = "RICEFIELD_TEST_CRASH_PROBE_JOURNAL";
        if let Ok(url) = std::env::var(CHILD_SERVER) {
            let request = UploadRequest {
                source: PathBuf::from(std::env::var_os(CHILD_SOURCE).unwrap()),
                remote_directory: url,
                credentials: MountCredentials {
                    username: "tester".into(),
                    domain: String::new(),
                    password: "secret".into(),
                },
                allow_insecure_http: false,
            };
            let directory = PathBuf::from(std::env::var_os(CHILD_JOURNAL).unwrap());
            let _ = super::upload_with_journal_directory(
                request,
                &AtomicBool::new(false),
                Some(&directory),
            );
            panic!("probe crash fixture must be killed while the response is withheld");
        }

        let server = MockDav::start_with_probe_stall();
        let (folder, file) = fixture_file("crash-probe.txt");
        let journals = folder.join("probe-state");
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("remote_upload::tests::abrupt_process_kill_during_probe_put_preserves_restart_evidence")
            .arg("--exact")
            .env(CHILD_SERVER, &server.url)
            .env(CHILD_SOURCE, &file)
            .env(CHILD_JOURNAL, &journals)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        while !server.probe_put_seen.load(Ordering::Acquire) && Instant::now() < deadline {
            if let Some(status) = child.try_wait().unwrap() {
                panic!("probe fixture exited before its first PUT: {status}");
            }
            thread::sleep(Duration::from_millis(10));
        }
        if !server.probe_put_seen.load(Ordering::Acquire) {
            let _ = child.kill();
            let _ = child.wait();
            panic!("probe fixture did not reach its first PUT");
        }
        child.kill().unwrap();
        assert!(!child.wait().unwrap().success());
        server.stall_probe_response.store(false, Ordering::Release);

        let entries: Vec<_> = fs::read_dir(&journals)
            .unwrap()
            .map(Result::unwrap)
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "probe"))
            .collect();
        assert_eq!(entries.len(), 1, "crashed probe lost its durable evidence");
        let saved = fs::read_to_string(entries[0].path()).unwrap();
        let probes: Vec<_> = server
            .state
            .lock()
            .unwrap()
            .keys()
            .filter(|path| path.contains("/.ricefield-safety-"))
            .cloned()
            .collect();
        assert_eq!(
            probes.len(),
            1,
            "server did not retain the interrupted probe"
        );
        let prefix_path = probes[0].trim_end_matches("-0-a");
        assert!(saved.contains(prefix_path), "journal lost the probe prefix");
        assert!(!saved.contains("secret"));
        assert!(server.path("crash-probe.txt").is_empty());
        assert_eq!(fs::read(&file).unwrap(), b"verified upload bytes");
        fs::remove_dir_all(journals).unwrap();
        fs::remove_file(file).unwrap();
        fs::remove_dir(folder).unwrap();
    }

    #[test]
    fn server_that_ignores_move_preconditions_is_blocked_before_publication() {
        let server = MockDav::start(true, false, false);
        let (folder, file) = fixture_file("critical.txt");
        let journals = folder.join("safety-journals");
        let result = super::upload_with_journal_directory(
            request(file.clone(), &server),
            &AtomicBool::new(false),
            Some(&journals),
        )
        .unwrap_err();
        assert!(result.contains("unsafe"), "{result}");
        assert!(result.contains(".ricefield-safety-"), "{result}");
        assert!(server.path("critical.txt").is_empty());
        let evidence: Vec<_> = fs::read_dir(&journals)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "probe"))
            .collect();
        assert_eq!(
            evidence.len(),
            1,
            "failed probe must retain restart evidence"
        );
        let saved = fs::read_to_string(evidence[0].path()).unwrap();
        assert!(saved.contains(".ricefield-safety-"));
        assert!(!saved.contains("secret"));
        assert_eq!(fs::read(&file).unwrap(), b"verified upload bytes");
        fs::remove_dir_all(journals).unwrap();
        fs::remove_file(file).unwrap();
        fs::remove_dir(folder).unwrap();
    }

    #[test]
    fn server_that_ignores_put_preconditions_is_blocked_before_publication() {
        let server = MockDav::start(false, true, false);
        let (folder, file) = fixture_file("private.txt");
        let result = upload(request(file.clone(), &server), &AtomicBool::new(false)).unwrap_err();
        assert!(result.contains("conditional PUT is unsafe"), "{result}");
        assert!(result.contains(".ricefield-safety-"), "{result}");
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
        assert!(Snapshot::create(&link, &AtomicBool::new(false)).is_err());
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
    fn cancelled_local_snapshot_stops_before_network_or_source_mutation() {
        let (folder, file) = fixture_file("local-cancel.txt");
        let cancelled = AtomicBool::new(true);
        let error = Snapshot::create(&file, &cancelled).err().unwrap();
        assert!(error.contains("cancelled"), "{error}");
        assert_eq!(fs::read(&file).unwrap(), b"verified upload bytes");
        fs::remove_file(file).unwrap();
        fs::remove_dir(folder).unwrap();
    }

    #[test]
    fn local_snapshot_copy_honors_cancellation_during_a_chunk() {
        struct CancelDuringRead<'a> {
            cancelled: &'a AtomicBool,
        }
        impl Read for CancelDuringRead<'_> {
            fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
                buffer.fill(7);
                self.cancelled.store(true, Ordering::Release);
                Ok(buffer.len())
            }
        }
        let cancelled = AtomicBool::new(false);
        let mut source = CancelDuringRead {
            cancelled: &cancelled,
        };
        let mut output = Vec::new();
        let error = copy_snapshot_with_cancel(&mut source, &mut output, &cancelled).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        assert!(output.is_empty(), "cancelled bytes should never be written");
    }

    #[test]
    fn cancelled_network_read_does_not_wait_for_server_timeout() {
        let (mut stalled, _server) = UnixStream::pair().unwrap();
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancelled);
        let (started, ready) = std::sync::mpsc::channel();
        let worker = thread::spawn(move || {
            let mut buffer = [0; 8];
            started.send(()).unwrap();
            read_with_cancel(&mut stalled, &mut buffer, &worker_cancel)
        });
        ready.recv_timeout(Duration::from_secs(2)).unwrap();
        thread::sleep(Duration::from_millis(120));
        let cancelled_at = Instant::now();
        cancelled.store(true, Ordering::Release);
        let error = worker.join().unwrap().unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        assert!(cancelled_at.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn webdav_probe_cancels_while_server_stalls_mid_response() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/share/stall", listener.local_addr().unwrap());
        let (sent, received) = std::sync::mpsc::channel();
        let (release, released) = std::sync::mpsc::channel();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                    break;
                }
            }
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 32\r\nConnection: close\r\n\r\nab")
                .unwrap();
            sent.send(()).unwrap();
            let _ = released.recv_timeout(Duration::from_secs(5));
        });
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancelled);
        let worker = thread::spawn(move || {
            verify_probe_contents(
                &url,
                &MountCredentials {
                    username: String::new(),
                    domain: String::new(),
                    password: String::new(),
                },
                b"ab",
                &worker_cancel,
            )
        });
        received.recv_timeout(Duration::from_secs(5)).unwrap();
        thread::sleep(Duration::from_millis(120));
        let cancelled_at = Instant::now();
        cancelled.store(true, Ordering::Release);
        let error = worker.join().unwrap().unwrap_err();
        assert!(error.contains("cancelled"), "{error}");
        assert!(cancelled_at.elapsed() < Duration::from_secs(2));
        release.send(()).unwrap();
        server.join().unwrap();
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
