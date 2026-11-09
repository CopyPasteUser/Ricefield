//! Key-authenticated SFTP uploads with private staging and server-atomic
//! `hardlink@openssh.com` publication. Never fall back to SFTP rename.

use crate::{
    location::{Location, RemoteScheme},
    remote_upload::{self, Snapshot, UploadRequest, UploadResult},
    remote_upload_journal,
};
use std::{
    ffi::OsStr,
    fs::{self, File},
    io::{Read, Write},
    os::unix::fs::OpenOptionsExt,
    os::unix::process::{CommandExt, ExitStatusExt},
    path::Path,
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};

#[cfg(test)]
use std::os::unix::fs::PermissionsExt;
#[cfg(test)]
use std::path::PathBuf;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(180);

#[cfg(test)]
fn kill_test_process_at(checkpoint: &str) {
    if std::env::var("RICEFIELD_TEST_SFTP_CRASH_AT").as_deref() == Ok(checkpoint) {
        // SAFETY: this test-only checkpoint terminates the current child
        // process; the parent verifies the surviving journal and remote data.
        unsafe { libc::raise(libc::SIGKILL) };
        unreachable!("SIGKILL cannot be ignored");
    }
}

#[derive(Clone)]
enum Connection {
    Ssh {
        host: String,
        port: Option<u16>,
        username: Option<String>,
    },
    #[cfg(test)]
    LocalServer(PathBuf),
}

#[derive(Clone)]
struct Target {
    connection: Connection,
    path: String,
    uri: String,
}

fn safe_ascii_path(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 2048
        || !value.starts_with('/')
        || !value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'.' | b'-' | b'_' | b' ')
        })
        || value.split('/').any(|part| part == "." || part == "..")
    {
        return Err("SFTP upload requires an absolute path with letters, digits, spaces, '.', '-' or '_' only; no traversal or wildcards".into());
    }
    Ok(())
}

fn decode_remote_path(encoded: &str) -> Result<String, String> {
    let bytes = encoded.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let text = std::str::from_utf8(
                bytes
                    .get(index + 1..index + 3)
                    .ok_or("Invalid SFTP URI escape")?,
            )
            .map_err(|_| "Invalid SFTP URI escape")?;
            out.push(u8::from_str_radix(text, 16).map_err(|_| "Invalid SFTP URI escape")?);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    let decoded = String::from_utf8(out).map_err(|_| "SFTP path must use ASCII characters")?;
    safe_ascii_path(&decoded)?;
    Ok(decoded)
}

fn safe_basename(name: &OsStr) -> Result<&str, String> {
    let name = name.to_str().ok_or("SFTP filename must be UTF-8")?;
    if name.is_empty()
        || name.len() > 255
        || name == "."
        || name == ".."
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_' | b' '))
    {
        return Err("SFTP upload filenames currently support ASCII letters, digits, spaces, '.', '-' and '_' only".into());
    }
    Ok(name)
}

fn user_token(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && !value.starts_with('-')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

impl Target {
    fn parse(uri: &str, username: &str, password: &str, domain: &str) -> Result<Self, String> {
        match Location::parse(uri.into()) {
            Ok(Location::Remote(remote)) if remote.scheme == RemoteScheme::Sftp => {}
            _ => return Err("Upload destination must be a validated sftp:// URI".into()),
        }
        if !password.is_empty() || !domain.is_empty() {
            return Err("SFTP safe uploads use your configured SSH key or agent; passwords and domains are not passed to external commands".into());
        }
        if !username.is_empty() && !user_token(username) {
            return Err("Invalid SFTP SSH username".into());
        }
        let remainder = uri.split_once("://").unwrap().1;
        let (authority, raw_path) = remainder
            .split_once('/')
            .map(|(authority, path)| (authority, format!("/{path}")))
            .unwrap_or((remainder, "/".into()));
        if authority.starts_with('[') {
            return Err("SFTP safe upload does not yet support IPv6 literal hosts".into());
        }
        let (host, port) = authority.split_once(':').unwrap_or((authority, ""));
        if !user_token(host) {
            return Err("Invalid SFTP hostname".into());
        }
        let port = if port.is_empty() {
            None
        } else {
            Some(port.parse::<u16>().map_err(|_| "Invalid SFTP port")?)
        };
        if port == Some(0) {
            return Err("Invalid SFTP port".into());
        }
        let path = decode_remote_path(&raw_path)?;
        Ok(Self {
            connection: Connection::Ssh {
                host: host.into(),
                port,
                username: (!username.is_empty()).then(|| username.into()),
            },
            path: path.trim_end_matches('/').to_owned(),
            uri: format!("sftp://{}", remainder.trim_end_matches('/')),
        })
    }

    fn run(&self, batch: &str, cancelled: &AtomicBool) -> Result<(), String> {
        self.run_bounded(batch, cancelled, None)
    }

    fn run_bounded(
        &self,
        batch: &str,
        cancelled: &AtomicBool,
        max_download_bytes: Option<u64>,
    ) -> Result<(), String> {
        if cancelled.load(Ordering::Relaxed) {
            return Err("SFTP upload cancelled".into());
        }
        let mut cmd = Command::new("sftp");
        cmd.args(["-b", "-"]);
        // The server controls the staged file's reported size. Bound the
        // verification download in the child, before it can fill local disk.
        if let Some(bytes) = max_download_bytes {
            let cap = bytes.saturating_add(1) as libc::rlim_t;
            unsafe {
                cmd.pre_exec(move || {
                    let no_core = libc::rlimit {
                        rlim_cur: 0,
                        rlim_max: 0,
                    };
                    if libc::setrlimit(libc::RLIMIT_CORE, &no_core) != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    let limit = libc::rlimit {
                        rlim_cur: cap,
                        rlim_max: cap,
                    };
                    if libc::setrlimit(libc::RLIMIT_FSIZE, &limit) != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
        }
        match &self.connection {
            Connection::Ssh {
                host,
                port,
                username,
            } => {
                cmd.args([
                    "-oBatchMode=yes",
                    "-oStrictHostKeyChecking=yes",
                    "-oNumberOfPasswordPrompts=0",
                    "-oConnectionAttempts=1",
                    "-oConnectTimeout=15",
                    "-oServerAliveInterval=5",
                    "-oServerAliveCountMax=2",
                    "-oForwardAgent=no",
                    "-oClearAllForwardings=yes",
                ]);
                if let Some(port) = port {
                    cmd.args(["-P", &port.to_string()]);
                }
                if let Some(username) = username {
                    cmd.arg(format!("-oUser={username}"));
                }
                cmd.arg(host);
            }
            #[cfg(test)]
            Connection::LocalServer(server) => {
                cmd.args(["-D", server.to_str().ok_or("Invalid test server path")?]);
            }
        }
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut child = cmd
            .spawn()
            .map_err(|error| format!("Cannot start SFTP: {error}"))?;
        if let Err(error) = child
            .stdin
            .take()
            .ok_or("Missing SFTP batch input")?
            .write_all(batch.as_bytes())
        {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("Cannot deliver SFTP command: {error}"));
        }
        let started = Instant::now();
        loop {
            if cancelled.load(Ordering::Relaxed) || started.elapsed() > REQUEST_TIMEOUT {
                let _ = child.kill();
                let _ = child.wait();
                return Err(if cancelled.load(Ordering::Relaxed) {
                    "SFTP upload cancelled"
                } else {
                    "SFTP server timed out"
                }
                .into());
            }
            match child.try_wait() {
                Ok(Some(status)) => {
                    return if status.success() {
                        Ok(())
                    } else if max_download_bytes.is_some() && status.signal() == Some(libc::SIGXFSZ)
                    {
                        Err("SFTP verification download exceeded its size limit".into())
                    } else {
                        Err(format!("SFTP server refused operation (exit {status})"))
                    };
                }
                Ok(None) => thread::sleep(Duration::from_millis(30)),
                Err(error) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!("SFTP process failed: {error}"));
                }
            }
        }
    }
}
fn quoted(path: &str) -> String {
    // Every input is validated ASCII without quotes, backslashes, glob
    // metacharacters, newlines or SFTP batch separators.
    format!("\"{path}\"")
}

fn put(client: &Target, source: &Path, target: &str, cancelled: &AtomicBool) -> Result<(), String> {
    let local = source.to_str().ok_or("Invalid local snapshot path")?;
    safe_ascii_path(local)?;
    client.run(
        &format!("put {} {}\n", quoted(local), quoted(target)),
        cancelled,
    )
}

fn get(
    client: &Target,
    source: &str,
    target: &Path,
    max_bytes: u64,
    cancelled: &AtomicBool,
) -> Result<(), String> {
    let local = target.to_str().ok_or("Invalid local verification path")?;
    safe_ascii_path(local)?;
    client.run_bounded(
        &format!("get {} {}\n", quoted(source), quoted(local)),
        cancelled,
        Some(max_bytes),
    )
}

fn compare_files(source: &Path, copy: &Path, cancelled: &AtomicBool) -> Result<(), String> {
    let mut source = File::open(source).map_err(|error| error.to_string())?;
    let mut copy = File::open(copy).map_err(|error| error.to_string())?;
    let mut source_buffer = [0_u8; 64 * 1024];
    let mut copy_buffer = [0_u8; 64 * 1024];
    loop {
        if cancelled.load(Ordering::Relaxed) {
            return Err("SFTP verification cancelled".into());
        }
        let count = source
            .read(&mut source_buffer)
            .map_err(|error| error.to_string())?;
        copy.read_exact(&mut copy_buffer[..count])
            .map_err(|error| format!("SFTP staged response is shorter than the source: {error}"))?;
        if source_buffer[..count] != copy_buffer[..count] {
            return Err("SFTP staged bytes differ from the pinned local snapshot".into());
        }
        if count == 0 {
            let mut tail = [0_u8; 1];
            return if copy.read(&mut tail).map_err(|error| error.to_string())? == 0 {
                Ok(())
            } else {
                Err("SFTP staged response exceeds the source length".into())
            };
        }
    }
}

fn private_directory(client: &Target, path: &str, cancelled: &AtomicBool) -> Result<(), String> {
    client.run(
        &format!("mkdir {}\nchmod 700 {}\n", quoted(path), quoted(path)),
        cancelled,
    )
}

fn remove_private_directory(
    client: &Target,
    dir: &str,
    names: &[&str],
    cancelled: &AtomicBool,
) -> Result<(), String> {
    let mut batch = String::new();
    for name in names {
        batch.push_str(&format!("rm {}\n", quoted(&format!("{dir}/{name}"))));
    }
    batch.push_str(&format!("rmdir {}\n", quoted(dir)));
    client.run(&batch, cancelled)
}

fn write_private_payload(path: &Path, contents: &[u8]) -> Result<(), String> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true).mode(0o600);
    let mut file = options.open(path).map_err(|error| error.to_string())?;
    file.write_all(contents)
        .map_err(|error| error.to_string())?;
    file.sync_all().map_err(|error| error.to_string())
}

fn probe_atomic_hardlink(
    client: &Target,
    snapshot: &Snapshot,
    cancelled: &AtomicBool,
    journal_override: Option<&Path>,
) -> Result<(), String> {
    let id = remote_upload::random_suffix().map_err(|error| error.to_string())?;
    let dir = format!("{}/.ricefield-safety-{id}", client.path);
    let uri = format!("{}/.ricefield-safety-{id}", client.uri);
    let pending = match journal_override {
        Some(path) => remote_upload_journal::PendingProbe::begin_in(path, &uri),
        None => remote_upload_journal::PendingProbe::begin(&uri),
    }
    .map_err(|error| format!("Cannot durably record SFTP safety probe before writing: {error}"))?;
    let error_with_path = |message: String| {
        format!(
            "{message}. Private SFTP probe objects may remain under {uri}. Review server contents and the local recovery record before manual cleanup"
        )
    };
    let result = (|| {
        private_directory(client, &dir, cancelled)?;
        let local_a = snapshot.private_file("probe-a.payload");
        let local_b = snapshot.private_file("probe-b.payload");
        let a = b"ricefield-sftp-safe-a";
        let b = b"ricefield-sftp-safe-b";
        write_private_payload(&local_a, a)?;
        write_private_payload(&local_b, b)?;
        put(client, &local_a, &format!("{dir}/a"), cancelled)?;
        #[cfg(test)]
        kill_test_process_at("probe-a");
        put(client, &local_b, &format!("{dir}/b"), cancelled)?;
        for round in 0..4 {
            let target = format!("{dir}/target-{round}");
            let link_a = format!("ln {} {}\n", quoted(&format!("{dir}/a")), quoted(&target));
            let link_b = format!("ln {} {}\n", quoted(&format!("{dir}/b")), quoted(&target));
            let (result_a, result_b) = if round == 0 {
                (
                    client.run(&link_a, cancelled),
                    client.run(&link_b, cancelled),
                )
            } else {
                thread::scope(|scope| {
                    let a = scope.spawn(|| client.run(&link_a, cancelled));
                    let b = scope.spawn(|| client.run(&link_b, cancelled));
                    Ok::<_, String>((
                        a.join()
                            .map_err(|_| "SFTP probe worker panicked".to_owned())?,
                        b.join()
                            .map_err(|_| "SFTP probe worker panicked".to_owned())?,
                    ))
                })?
            };
            if result_a.is_ok() == result_b.is_ok() {
                return Err(format!(
                    "SFTP atomic hardlink collision failed in round {round}: {result_a:?}, {result_b:?}"
                ));
            }
            if cancelled.load(Ordering::Relaxed) {
                return Err("SFTP probe cancelled".into());
            }
            let verify_path = snapshot.private_file("sftp-verify.payload");
            get(client, &target, &verify_path, 64, cancelled)?;
            let verified = fs::read(&verify_path).map_err(|error| error.to_string())?;
            let expected = if result_a.is_ok() { a } else { b };
            if verified != expected {
                return Err("SFTP hardlink target contains unexpected bytes".into());
            }
            fs::remove_file(verify_path).map_err(|error| error.to_string())?;
        }
        remove_private_directory(
            client,
            &dir,
            &["target-0", "target-1", "target-2", "target-3", "a", "b"],
            cancelled,
        )
    })();
    result.map_err(error_with_path)?;
    pending.finish();
    Ok(())
}

pub fn upload(request: UploadRequest, cancelled: &AtomicBool) -> Result<UploadResult, String> {
    let client = Target::parse(
        &request.remote_directory,
        &request.credentials.username,
        &request.credentials.password,
        &request.credentials.domain,
    )?;
    upload_with_client(request, cancelled, &client, None)
}

fn upload_with_client(
    request: UploadRequest,
    cancelled: &AtomicBool,
    client: &Target,
    journal_override: Option<&Path>,
) -> Result<UploadResult, String> {
    let filename = request
        .source
        .file_name()
        .ok_or("Select a regular local file")?;
    let basename = safe_basename(filename)?;
    let encoded = remote_upload::escape_filename(filename)?;
    let destination = format!("{}/{}", client.path, basename);
    let destination_uri = format!("{}/{}", client.uri, encoded);
    let snapshot = Snapshot::create(&request.source, cancelled)?;
    probe_atomic_hardlink(client, &snapshot, cancelled, journal_override)?;

    let id = remote_upload::random_suffix().map_err(|error| error.to_string())?;
    let directory = format!("{}/.ricefield-upload-{id}", client.path);
    let stage_uri = format!("{}/.ricefield-upload-{id}", client.uri);
    let pending = match journal_override {
        Some(path) => {
            remote_upload_journal::PendingUpload::begin_in(path, &stage_uri, &destination_uri)
        }
        None => remote_upload_journal::PendingUpload::begin(&stage_uri, &destination_uri),
    }
    .map_err(|error| format!("Cannot durably record SFTP stage before writing: {error}"))?;
    let error_with_path = |message: String| {
        format!(
            "{message}. SFTP publication may be ambiguous; inspect {stage_uri}/payload and {destination_uri} before remote cleanup or local journal dismissal"
        )
    };
    let result = (|| {
        private_directory(client, &directory, cancelled)?;
        let stage = format!("{directory}/payload");
        put(client, snapshot.path(), &stage, cancelled)?;
        #[cfg(test)]
        kill_test_process_at("stage");
        let verify_path = snapshot.private_file("sftp-verify.payload");
        get(client, &stage, &verify_path, snapshot.bytes, cancelled)?;
        compare_files(snapshot.path(), &verify_path, cancelled)?;
        fs::remove_file(verify_path).map_err(|error| error.to_string())?;
        if cancelled.load(Ordering::Relaxed) {
            return Err("SFTP upload cancelled before publication".into());
        }
        // An uncertain link response must not be retried. Ignore cancellation
        // once the server might have committed the final directory entry.
        let publishing = AtomicBool::new(false);
        client.run(
            &format!("ln {} {}\n", quoted(&stage), quoted(&destination)),
            &publishing,
        )?;
        #[cfg(test)]
        kill_test_process_at("post-publish");
        #[cfg(test)]
        if std::env::var_os("RICEFIELD_TEST_SFTP_DENY_CLEANUP").is_some() {
            // Real OpenSSH sftp-server must reject unlink in a non-writable
            // directory after the hardlink has already been published.
            fs::set_permissions(&directory, fs::Permissions::from_mode(0o500))
                .map_err(|error| error.to_string())?;
        }
        remove_private_directory(client, &directory, &["payload"], &publishing)
    })();
    result.map_err(error_with_path)?;
    pending.finish();
    Ok(UploadResult {
        uri: destination_uri,
        bytes: snapshot.bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::volume::MountCredentials;
    use std::{os::unix::fs::DirBuilderExt, sync::Arc};

    fn server() -> PathBuf {
        let executable = PathBuf::from("/usr/lib/openssh/sftp-server");
        assert!(
            executable.is_file(),
            "install openssh-sftp-server for real SFTP protocol tests"
        );
        executable
    }

    struct Fixture {
        directory: PathBuf,
        source: PathBuf,
        journal: PathBuf,
        client: Target,
    }

    impl Fixture {
        fn new(server: PathBuf) -> Self {
            let directory = std::env::temp_dir().join(format!(
                "ricefield-sftp-test-{}",
                remote_upload::random_suffix().unwrap()
            ));
            fs::DirBuilder::new()
                .mode(0o700)
                .create(&directory)
                .unwrap();
            let source = directory.join("example.txt");
            let journal = directory.join("evidence");
            let remote = directory.join("remote");
            fs::DirBuilder::new().mode(0o700).create(&remote).unwrap();
            fs::write(&source, b"pinned-source-content\n").unwrap();
            Self {
                client: Target {
                    connection: Connection::LocalServer(server),
                    path: remote.to_str().unwrap().to_owned(),
                    uri: format!("sftp://localhost{}", remote.display()),
                },
                source,
                journal,
                directory,
            }
        }

        fn request(&self) -> UploadRequest {
            UploadRequest {
                source: self.source.clone(),
                remote_directory: self.client.uri.clone(),
                credentials: MountCredentials {
                    username: String::new(),
                    password: String::new(),
                    domain: String::new(),
                },
                allow_insecure_http: false,
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.directory);
        }
    }

    fn run_crash_child_if_requested() -> bool {
        let Some(root) = std::env::var_os("RICEFIELD_TEST_SFTP_CRASH_ROOT") else {
            return false;
        };
        let root = PathBuf::from(root);
        let remote = root.join("remote");
        let client = Target {
            connection: Connection::LocalServer(server()),
            path: remote.to_str().unwrap().to_owned(),
            uri: format!("sftp://localhost{}", remote.display()),
        };
        let request = UploadRequest {
            source: root.join("example.txt"),
            remote_directory: client.uri.clone(),
            credentials: MountCredentials {
                username: String::new(),
                password: String::new(),
                domain: String::new(),
            },
            allow_insecure_http: false,
        };
        let result = upload_with_client(
            request,
            &AtomicBool::new(false),
            &client,
            Some(&root.join("evidence")),
        );
        if std::env::var_os("RICEFIELD_TEST_SFTP_DENY_CLEANUP").is_some() {
            let error = result.expect_err("OpenSSH cleanup must fail without write permission");
            assert!(error.contains("SFTP"), "{error}");
            return true;
        }
        panic!("SFTP crash checkpoint was not reached: {result:?}");
    }

    fn process_crash_fixture(test_name: &str, checkpoint: &str) -> Fixture {
        let fixture = Fixture::new(server());
        let mut child = Command::new(std::env::current_exe().unwrap())
            .arg(test_name)
            .arg("--exact")
            .env("RICEFIELD_TEST_SFTP_CRASH_ROOT", &fixture.directory)
            .env("RICEFIELD_TEST_SFTP_CRASH_AT", checkpoint)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("SFTP crash fixture never reached checkpoint {checkpoint}");
            }
            thread::sleep(Duration::from_millis(20));
        };
        assert_eq!(
            status.signal(),
            Some(libc::SIGKILL),
            "child did not crash at {checkpoint}: {status}"
        );
        fixture
    }

    #[test]
    fn sftp_path_and_connection_validation_fails_closed() {
        for input in [
            "/ok/../bad",
            "/bad;rm",
            "/bad\nmkdir /oops",
            "/bad\\escape",
            "relative",
        ] {
            assert!(safe_ascii_path(input).is_err(), "{input:?}");
        }
        assert!(Target::parse("sftp://localhost/tmp/folder", "user", "password", "").is_err());
        assert!(Target::parse("sftp://localhost/tmp/folder", "-oInject", "", "").is_err());
        assert!(Target::parse("sftp://localhost/tmp/a%20b", "", "", "").is_ok());
        assert!(Target::parse("sftp://localhost/tmp/%2E%2E/file", "", "", "").is_err());
        assert!(Target::parse("sftp://localhost/tmp/a%0Aecho", "", "", "").is_err());
    }

    #[test]
    fn real_sftp_private_stage_hardlink_publish_and_collision() {
        let fixture = Fixture::new(server());
        let cancel = AtomicBool::new(false);
        let result = upload_with_client(
            fixture.request(),
            &cancel,
            &fixture.client,
            Some(&fixture.journal),
        )
        .unwrap();
        assert_eq!(result.bytes, b"pinned-source-content\n".len() as u64);
        assert_eq!(
            fs::read(format!("{}/example.txt", fixture.client.path)).unwrap(),
            b"pinned-source-content\n"
        );
        assert_eq!(
            remote_upload_journal::inspect_evidence_in(&fixture.journal)
                .unwrap()
                .0
                .len(),
            0
        );
        assert_eq!(
            remote_upload_journal::inspect_evidence_in(&fixture.journal)
                .unwrap()
                .1
                .len(),
            0
        );

        // The same target name is deliberately occupied. A second upload is
        // denied by the server-side hardlink, and the existing bytes survive.
        fs::write(
            format!("{}/example.txt", fixture.client.path),
            b"external-winner",
        )
        .unwrap();
        let error = upload_with_client(
            fixture.request(),
            &cancel,
            &fixture.client,
            Some(&fixture.journal),
        )
        .unwrap_err();
        assert!(error.contains("SFTP"));
        assert_eq!(
            fs::read(format!("{}/example.txt", fixture.client.path)).unwrap(),
            b"external-winner"
        );
        let (uploads, probes) =
            remote_upload_journal::inspect_evidence_in(&fixture.journal).unwrap();
        assert_eq!(uploads.len(), 1, "uncertain staged upload requires review");
        assert!(probes.is_empty(), "successful probe is retired");
    }

    #[test]
    fn two_complete_sftp_uploaders_compete_without_replacing_the_winner() {
        let fixture = Fixture::new(server());
        let second_directory = fixture.directory.join("second-client");
        fs::create_dir(&second_directory).unwrap();
        let second_source = second_directory.join("example.txt");
        fs::write(&second_source, b"second-independent-client\n").unwrap();
        let first_request = fixture.request();
        let mut second_request = fixture.request();
        second_request.source = second_source.clone();
        let cancelled = AtomicBool::new(false);

        // These are independent real OpenSSH-SFTP sessions performing the
        // entire probe/stage/verify/publish path, not only raw hardlink calls.
        let (first, second) = thread::scope(|scope| {
            let first = scope.spawn(|| {
                upload_with_client(
                    first_request,
                    &cancelled,
                    &fixture.client,
                    Some(&fixture.journal),
                )
            });
            let second = scope.spawn(|| {
                upload_with_client(
                    second_request,
                    &cancelled,
                    &fixture.client,
                    Some(&fixture.journal),
                )
            });
            (first.join().unwrap(), second.join().unwrap())
        });
        assert_ne!(first.is_ok(), second.is_ok(), "exactly one upload may publish");
        let expected = if first.is_ok() {
            b"pinned-source-content\n".as_slice()
        } else {
            b"second-independent-client\n".as_slice()
        };
        assert_eq!(
            fs::read(format!("{}/example.txt", fixture.client.path)).unwrap(),
            expected
        );
        assert_eq!(fs::read(&fixture.source).unwrap(), b"pinned-source-content\n");
        assert_eq!(fs::read(&second_source).unwrap(), b"second-independent-client\n");
        let (uploads, probes) =
            remote_upload_journal::inspect_evidence_in(&fixture.journal).unwrap();
        assert_eq!(uploads.len(), 1, "the losing attempt must retain evidence");
        assert!(probes.is_empty());
    }

    #[test]
    fn real_sftp_refuses_a_preexisting_directory_destination() {
        let fixture = Fixture::new(server());
        let destination = PathBuf::from(&fixture.client.path).join("example.txt");
        fs::create_dir(&destination).unwrap();
        let error = upload_with_client(
            fixture.request(),
            &AtomicBool::new(false),
            &fixture.client,
            Some(&fixture.journal),
        )
        .unwrap_err();
        assert!(error.contains("SFTP publication may be ambiguous"));
        assert!(destination.is_dir(), "a directory collision must survive");
        assert_eq!(fs::read(&fixture.source).unwrap(), b"pinned-source-content\n");
        let (uploads, probes) =
            remote_upload_journal::inspect_evidence_in(&fixture.journal).unwrap();
        assert_eq!(uploads.len(), 1);
        assert!(probes.is_empty());
    }

    #[test]
    fn real_sftp_verification_download_is_bounded() {
        let fixture = Fixture::new(server());
        let remote = PathBuf::from(&fixture.client.path).join("oversized.payload");
        let local = fixture.directory.join("verification.payload");
        let cancelled = AtomicBool::new(false);
        fs::write(&remote, b"exact").unwrap();
        get(
            &fixture.client,
            remote.to_str().unwrap(),
            &local,
            5,
            &cancelled,
        )
        .unwrap();
        assert_eq!(fs::read(&local).unwrap(), b"exact");
        fs::remove_file(&local).unwrap();

        fs::write(
            &remote,
            b"untrusted server response is longer than the source",
        )
        .unwrap();
        let error = get(
            &fixture.client,
            remote.to_str().unwrap(),
            &local,
            5,
            &cancelled,
        )
        .unwrap_err();
        assert!(error.contains("size limit"), "{error}");
        assert!(fs::metadata(&local).map(|file| file.len()).unwrap_or(0) <= 6);
        assert_eq!(
            fs::read(&remote).unwrap(),
            b"untrusted server response is longer than the source"
        );
    }

    #[test]
    fn real_sftp_process_crash_during_probe_preserves_recovery_evidence() {
        run_crash_child_if_requested();
        let fixture = process_crash_fixture(
            "remote_upload_sftp::tests::real_sftp_process_crash_during_probe_preserves_recovery_evidence",
            "probe-a",
        );
        let (uploads, probes) =
            remote_upload_journal::inspect_evidence_in(&fixture.journal).unwrap();
        assert!(uploads.is_empty());
        assert_eq!(probes.len(), 1);
        let directory =
            PathBuf::from(&fixture.client.path).join(format!(".ricefield-safety-{}", probes[0].id));
        assert_eq!(
            fs::read(directory.join("a")).unwrap(),
            b"ricefield-sftp-safe-a"
        );
        assert!(
            probes[0]
                .prefix_uri
                .ends_with(&format!(".ricefield-safety-{}", probes[0].id))
        );
        assert!(!Path::new(&format!("{}/example.txt", fixture.client.path)).exists());
        assert_eq!(
            fs::read(&fixture.source).unwrap(),
            b"pinned-source-content\n"
        );
    }

    #[test]
    fn real_sftp_process_crash_after_stage_put_preserves_recovery_evidence() {
        run_crash_child_if_requested();
        let fixture = process_crash_fixture(
            "remote_upload_sftp::tests::real_sftp_process_crash_after_stage_put_preserves_recovery_evidence",
            "stage",
        );
        let (uploads, probes) =
            remote_upload_journal::inspect_evidence_in(&fixture.journal).unwrap();
        assert!(
            probes.is_empty(),
            "completed probes must retire their records"
        );
        assert_eq!(uploads.len(), 1);
        let stage = PathBuf::from(&fixture.client.path)
            .join(format!(".ricefield-upload-{}", uploads[0].id));
        assert_eq!(
            fs::read(stage.join("payload")).unwrap(),
            b"pinned-source-content\n"
        );
        assert!(uploads[0].destination_uri.ends_with("/example.txt"));
        assert!(!Path::new(&format!("{}/example.txt", fixture.client.path)).exists());
        assert_eq!(
            fs::read(&fixture.source).unwrap(),
            b"pinned-source-content\n"
        );
    }

    #[test]
    fn real_sftp_crash_after_publication_never_hides_the_published_destination() {
        run_crash_child_if_requested();
        let fixture = process_crash_fixture(
            "remote_upload_sftp::tests::real_sftp_crash_after_publication_never_hides_the_published_destination",
            "post-publish",
        );
        let (uploads, probes) =
            remote_upload_journal::inspect_evidence_in(&fixture.journal).unwrap();
        assert!(probes.is_empty());
        assert_eq!(uploads.len(), 1, "unacknowledged cleanup keeps evidence");
        let stage = PathBuf::from(&fixture.client.path)
            .join(format!(".ricefield-upload-{}", uploads[0].id));
        assert_eq!(
            fs::read(stage.join("payload")).unwrap(),
            b"pinned-source-content\n"
        );
        assert_eq!(
            fs::read(format!("{}/example.txt", fixture.client.path)).unwrap(),
            b"pinned-source-content\n"
        );
        assert_eq!(
            fs::read(&fixture.source).unwrap(),
            b"pinned-source-content\n"
        );
        assert!(uploads[0].destination_uri.ends_with("/example.txt"));
        let retry_error = upload_with_client(
            fixture.request(),
            &AtomicBool::new(false),
            &fixture.client,
            Some(&fixture.journal),
        )
        .unwrap_err();
        assert!(retry_error.contains("SFTP publication may be ambiguous"));
        assert_eq!(
            fs::read(format!("{}/example.txt", fixture.client.path)).unwrap(),
            b"pinned-source-content\n"
        );
        let (new_uploads, _) =
            remote_upload_journal::inspect_evidence_in(&fixture.journal).unwrap();
        assert_eq!(
            new_uploads.len(),
            2,
            "reconnect collision keeps both attempts"
        );
    }

    #[test]
    fn real_sftp_denied_cleanup_after_publication_retains_recovery_evidence() {
        if run_crash_child_if_requested() {
            return;
        }
        let fixture = Fixture::new(server());
        let status = Command::new(std::env::current_exe().unwrap())
            .arg("remote_upload_sftp::tests::real_sftp_denied_cleanup_after_publication_retains_recovery_evidence")
            .arg("--exact")
            .env("RICEFIELD_TEST_SFTP_CRASH_ROOT", &fixture.directory)
            .env("RICEFIELD_TEST_SFTP_DENY_CLEANUP", "1")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(
            status.success(),
            "child must observe a denied remote cleanup"
        );
        let (uploads, probes) =
            remote_upload_journal::inspect_evidence_in(&fixture.journal).unwrap();
        assert!(probes.is_empty());
        assert_eq!(uploads.len(), 1);
        let stage = PathBuf::from(&fixture.client.path)
            .join(format!(".ricefield-upload-{}", uploads[0].id));
        assert_eq!(
            fs::read(stage.join("payload")).unwrap(),
            b"pinned-source-content\n"
        );
        assert_eq!(
            fs::read(format!("{}/example.txt", fixture.client.path)).unwrap(),
            b"pinned-source-content\n"
        );
        assert_eq!(
            fs::read(&fixture.source).unwrap(),
            b"pinned-source-content\n"
        );
        // Restore the disposable test fixture; the application itself never
        // performs cleanup when publication or its aftermath is uncertain.
        fs::set_permissions(&stage, fs::Permissions::from_mode(0o700)).unwrap();
    }

    #[test]
    fn two_real_sftp_clients_never_replace_a_competitor() {
        let fixture = Fixture::new(server());
        let client = Arc::new(fixture.client.clone());
        let cancel = AtomicBool::new(false);
        let local_a = fixture.directory.join("first.payload");
        let local_b = fixture.directory.join("second.payload");
        fs::write(&local_a, b"first").unwrap();
        fs::write(&local_b, b"second").unwrap();
        let winner = format!("{}/winner", client.path);
        let left = format!(
            "ln {} {}\n",
            quoted(local_a.to_str().unwrap()),
            quoted(&winner)
        );
        let right = format!(
            "ln {} {}\n",
            quoted(local_b.to_str().unwrap()),
            quoted(&winner)
        );
        let (a, b) = thread::scope(|scope| {
            let a = scope.spawn(|| client.run(&left, &cancel));
            let b = scope.spawn(|| client.run(&right, &cancel));
            (a.join().unwrap(), b.join().unwrap())
        });
        assert_ne!(a.is_ok(), b.is_ok());
        let expected = if a.is_ok() {
            b"first".as_slice()
        } else {
            b"second".as_slice()
        };
        assert_eq!(fs::read(&winner).unwrap(), expected);
        fs::remove_file(winner).unwrap();
        fs::remove_file(local_a).unwrap();
        fs::remove_file(local_b).unwrap();
    }

    #[test]
    fn unavailable_sftp_server_never_publishes_and_keeps_probe_evidence() {
        let fixture = Fixture::new(PathBuf::from("/bin/false"));
        let cancelled = AtomicBool::new(false);
        let error = upload_with_client(
            fixture.request(),
            &cancelled,
            &fixture.client,
            Some(&fixture.journal),
        )
        .unwrap_err();
        assert!(error.contains("Private SFTP probe objects"));
        assert!(!Path::new(&format!("{}/example.txt", fixture.client.path)).exists());
        let (stages, probes) =
            remote_upload_journal::inspect_evidence_in(&fixture.journal).unwrap();
        assert!(stages.is_empty());
        assert_eq!(
            probes.len(),
            1,
            "failed probe must remain visible after restart"
        );
    }

    #[test]
    fn real_sftp_refuses_a_preexisting_symlink_destination() {
        let fixture = Fixture::new(server());
        let destination = PathBuf::from(format!("{}/example.txt", fixture.client.path));
        std::os::unix::fs::symlink(&fixture.source, &destination).unwrap();
        let error = upload_with_client(
            fixture.request(),
            &AtomicBool::new(false),
            &fixture.client,
            Some(&fixture.journal),
        )
        .unwrap_err();
        assert!(error.contains("SFTP"));
        assert!(
            fs::symlink_metadata(&destination)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            fs::read(&fixture.source).unwrap(),
            b"pinned-source-content\n"
        );
    }
}
