//! Conditional SMB2/3 single-file writes. The only publication primitive is
//! `smbclient hardlink`, which must refuse an occupied final name on the server.
//! There is deliberately no rename, GIO or check-then-PUT fallback.

use crate::{
    location::{Location, RemoteScheme},
    remote_upload::{self, Snapshot, UploadRequest, UploadResult},
    remote_upload_journal,
};
use std::{
    ffi::OsStr,
    fs::{self, File},
    io::{Read, Write},
    os::unix::process::CommandExt,
    path::Path,
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(180);

#[cfg(test)]
fn kill_test_process_at(checkpoint: &str) {
    if std::env::var("RICEFIELD_TEST_SMB_CRASH_AT").as_deref() == Ok(checkpoint) {
        // SAFETY: test-only checkpoint. The parent process independently
        // inspects the Samba share and persisted recovery records.
        unsafe { libc::raise(libc::SIGKILL) };
        unreachable!("SIGKILL cannot be ignored");
    }
}

struct Target {
    host: String,
    share: String,
    port: Option<u16>,
    path: String,
    uri: String,
    username: String,
    password: String,
    domain: String,
}

fn safe_path(path: &str) -> Result<(), String> {
    if path.len() > 2048
        || !path
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'/' | b'.' | b'-' | b'_' | b' '))
        || path
            .split('/')
            .any(|segment| segment == "." || segment == "..")
    {
        return Err("SMB path supports only ASCII letters, numbers, space, '.', '-' and '_', without traversal or command metacharacters".into());
    }
    Ok(())
}

fn decode_path(path: &str) -> Result<String, String> {
    // Encoded separators must not turn an apparent share name into a
    // different share plus directory after the security and recovery checks.
    if path.to_ascii_lowercase().contains("%2f") {
        return Err("Encoded SMB path separators are unsupported".into());
    }
    let bytes = path.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut offset = 0;
    while offset < bytes.len() {
        if bytes[offset] == b'%' {
            let hex = std::str::from_utf8(
                bytes
                    .get(offset + 1..offset + 3)
                    .ok_or("Bad SMB percent escape")?,
            )
            .map_err(|_| "Bad SMB percent escape")?;
            decoded.push(u8::from_str_radix(hex, 16).map_err(|_| "Bad SMB percent escape")?);
            offset += 3;
        } else {
            decoded.push(bytes[offset]);
            offset += 1;
        }
    }
    let path = String::from_utf8(decoded).map_err(|_| "SMB path must be ASCII")?;
    safe_path(&path)?;
    Ok(path)
}

fn basename(name: &OsStr) -> Result<&str, String> {
    let name = name.to_str().ok_or("SMB file name must be UTF-8")?;
    if name.is_empty() || name == "." || name == ".." || name.len() > 255 || name.contains('/') {
        return Err("Invalid SMB filename".into());
    }
    safe_path(name)?;
    Ok(name)
}

fn plain_token(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && !value.starts_with('-')
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
}

fn credential(value: &str, limit: usize) -> bool {
    value.len() <= limit && !value.bytes().any(|c| c.is_ascii_control())
}

impl Target {
    fn parse(
        uri: &str,
        username: String,
        password: String,
        domain: String,
    ) -> Result<Self, String> {
        if !matches!(Location::parse(uri.into()),
            Ok(Location::Remote(remote)) if remote.scheme == RemoteScheme::Smb)
        {
            return Err("SMB upload needs a validated smb://host/share/folder URI".into());
        }
        if !credential(&username, 128)
            || username.is_empty()
            || !credential(&password, 1024)
            || !credential(&domain, 128)
        {
            return Err("SMB username is required and authentication fields must not contain control characters".into());
        }
        let remainder = uri.split_once("://").ok_or("Invalid SMB URI")?.1;
        let (authority, path) = remainder
            .split_once('/')
            .ok_or("SMB URI must include a share name")?;
        if authority.starts_with('[') {
            return Err("IPv6 literal SMB upload hosts are not supported".into());
        }
        let (host, port) = authority.split_once(':').unwrap_or((authority, ""));
        if !plain_token(host) {
            return Err("Invalid SMB server name".into());
        }
        let port = if port.is_empty() {
            None
        } else {
            Some(port.parse::<u16>().map_err(|_| "Invalid SMB port")?)
        };
        if port == Some(0) {
            return Err("Invalid SMB port".into());
        }
        let decoded = decode_path(path)?;
        let (share, relative) = decoded.split_once('/').unwrap_or((decoded.as_str(), ""));
        if !plain_token(share) {
            return Err("Invalid SMB share name".into());
        }
        let relative = relative.trim_matches('/');
        safe_path(relative)?;
        let uri = format!("smb://{}", remainder.trim_end_matches('/'));
        Ok(Self {
            host: host.into(),
            share: share.into(),
            port,
            path: relative.into(),
            uri,
            username,
            password,
            domain,
        })
    }

    fn at(&self, file: &str) -> String {
        if self.path.is_empty() {
            file.to_owned()
        } else {
            format!("{}/{file}", self.path)
        }
    }

    fn run(&self, command: &str, cancelled: &AtomicBool) -> Result<(), String> {
        self.run_bounded(command, cancelled, None)
    }

    fn run_bounded(
        &self,
        command: &str,
        cancelled: &AtomicBool,
        max_download_bytes: Option<u64>,
    ) -> Result<(), String> {
        if cancelled.load(Ordering::Relaxed) {
            return Err("SMB upload cancelled".into());
        }
        let mut cmd = Command::new("smbclient");
        cmd.arg(format!("//{}/{}", self.host, self.share))
            .args([
                "--max-protocol=SMB3",
                "--option=client min protocol=SMB2_10",
            ])
            .arg("--client-protection=sign")
            .arg("--authentication-file=/dev/stdin")
            .arg("--command")
            .arg(command);
        if let Some(port) = self.port {
            cmd.arg(format!("--port={port}"));
        }
        // A crashed external client must not dump in-memory passwords. For
        // GET, also bound file output even if a peer sends oversized data.
        let download_cap = max_download_bytes.map(|bytes| bytes.saturating_add(1) as libc::rlim_t);
        unsafe {
            cmd.pre_exec(move || {
                let no_core = libc::rlimit {
                    rlim_cur: 0,
                    rlim_max: 0,
                };
                if libc::setrlimit(libc::RLIMIT_CORE, &no_core) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if let Some(cap) = download_cap {
                    let limit = libc::rlimit {
                        rlim_cur: cap,
                        rlim_max: cap,
                    };
                    if libc::setrlimit(libc::RLIMIT_FSIZE, &limit) != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                }
                Ok(())
            });
        }
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut child = cmd
            .spawn()
            .map_err(|error| format!("Cannot start SMB client: {error}"))?;
        // smbclient's authentication file is a kernel pipe, never disk,
        // command-line arguments or the inherited process environment.
        // Control bytes are rejected above to forbid extra auth-file fields.
        let auth = format!(
            "username = {}\npassword = {}\ndomain = {}\n",
            self.username, self.password, self.domain
        );
        let wrote = child
            .stdin
            .take()
            .ok_or("SMB authentication pipe unavailable")?
            .write_all(auth.as_bytes());
        if let Err(error) = wrote {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("Cannot provide SMB authentication: {error}"));
        }
        let started = Instant::now();
        loop {
            if cancelled.load(Ordering::Relaxed) || started.elapsed() > REQUEST_TIMEOUT {
                let _ = child.kill();
                let _ = child.wait();
                return Err(if cancelled.load(Ordering::Relaxed) {
                    "SMB upload cancelled"
                } else {
                    "SMB operation timed out"
                }
                .into());
            }
            match child.try_wait() {
                Ok(Some(status)) => {
                    return if status.success() {
                        Ok(())
                    } else {
                        Err(format!("SMB server refused operation (exit {status})"))
                    };
                }
                Ok(None) => thread::sleep(Duration::from_millis(30)),
                Err(error) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!("SMB process failed: {error}"));
                }
            }
        }
    }
}

fn quoted(path: &str) -> String {
    // All remote paths and snapshot paths are validated before reaching here.
    format!("\"{path}\"")
}

fn put(target: &Target, local: &Path, remote: &str, cancelled: &AtomicBool) -> Result<(), String> {
    let local = local.to_str().ok_or("Invalid snapshot path")?;
    safe_path(local)?;
    target.run(
        &format!("put {} {}", quoted(local), quoted(remote)),
        cancelled,
    )
}

fn get(
    target: &Target,
    remote: &str,
    local: &Path,
    max_bytes: u64,
    cancelled: &AtomicBool,
) -> Result<(), String> {
    let local = local.to_str().ok_or("Invalid verification path")?;
    safe_path(local)?;
    target.run_bounded(
        &format!("get {} {}", quoted(remote), quoted(local)),
        cancelled,
        Some(max_bytes),
    )
}

fn compare(source: &Path, copy: &Path, cancelled: &AtomicBool) -> Result<(), String> {
    let mut source = File::open(source).map_err(|error| error.to_string())?;
    let mut copy = File::open(copy).map_err(|error| error.to_string())?;
    let mut src = [0u8; 64 * 1024];
    let mut dst = [0u8; 64 * 1024];
    loop {
        if cancelled.load(Ordering::Relaxed) {
            return Err("SMB byte verification cancelled".into());
        }
        let count = source.read(&mut src).map_err(|error| error.to_string())?;
        copy.read_exact(&mut dst[..count])
            .map_err(|error| format!("SMB staged response truncated: {error}"))?;
        if src[..count] != dst[..count] {
            return Err("SMB staged bytes differ from the local snapshot".into());
        }
        if count == 0 {
            let mut tail = [0u8; 1];
            return if copy.read(&mut tail).map_err(|error| error.to_string())? == 0 {
                Ok(())
            } else {
                Err("SMB staged response has extra bytes".into())
            };
        }
    }
}

fn private_payload(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| error.to_string())?;
    output.write_all(bytes).map_err(|error| error.to_string())?;
    output.sync_all().map_err(|error| error.to_string())
}

fn cleanup(
    target: &Target,
    directory: &str,
    names: &[&str],
    cancelled: &AtomicBool,
) -> Result<(), String> {
    for name in names {
        target.run(
            &format!("del {}", quoted(&format!("{directory}/{name}"))),
            cancelled,
        )?;
    }
    target.run(&format!("rmdir {}", quoted(directory)), cancelled)
}

fn safety_probe(
    target: &Target,
    snapshot: &Snapshot,
    cancelled: &AtomicBool,
    journal_override: Option<&Path>,
) -> Result<(), String> {
    let id = remote_upload::random_suffix().map_err(|error| error.to_string())?;
    let dir = target.at(&format!(".ricefield-safety-{id}"));
    let uri = format!("{}/.ricefield-safety-{id}", target.uri);
    let pending = match journal_override {
        Some(path) => remote_upload_journal::PendingProbe::begin_in(path, &uri),
        None => remote_upload_journal::PendingProbe::begin(&uri),
    }
    .map_err(|error| format!("Cannot durably record SMB probe before remote writes: {error}"))?;
    let result = (|| {
        target.run(&format!("mkdir {}", quoted(&dir)), cancelled)?;
        let local_a = snapshot.private_file("smb-probe-a.payload");
        let local_b = snapshot.private_file("smb-probe-b.payload");
        private_payload(&local_a, b"ricefield-smb-safe-a")?;
        private_payload(&local_b, b"ricefield-smb-safe-b")?;
        put(target, &local_a, &format!("{dir}/a"), cancelled)?;
        #[cfg(test)]
        kill_test_process_at("probe-first-put");
        put(target, &local_b, &format!("{dir}/b"), cancelled)?;
        for round in 0..4 {
            let destination = format!("{dir}/target-{round}");
            let link_a = format!(
                "hardlink {} {}",
                quoted(&format!("{dir}/a")),
                quoted(&destination)
            );
            let link_b = format!(
                "hardlink {} {}",
                quoted(&format!("{dir}/b")),
                quoted(&destination)
            );
            let (left, right) = if round == 0 {
                (
                    target.run(&link_a, cancelled),
                    target.run(&link_b, cancelled),
                )
            } else {
                thread::scope(|scope| {
                    let a = scope.spawn(|| target.run(&link_a, cancelled));
                    let b = scope.spawn(|| target.run(&link_b, cancelled));
                    Ok::<_, String>((
                        a.join()
                            .map_err(|_| "SMB probe worker panicked".to_owned())?,
                        b.join()
                            .map_err(|_| "SMB probe worker panicked".to_owned())?,
                    ))
                })?
            };
            if left.is_ok() == right.is_ok() {
                return Err(format!(
                    "SMB atomic hardlink safety failed in round {round}: {left:?}, {right:?}"
                ));
            }
            let local = snapshot.private_file("smb-verify.payload");
            get(target, &destination, &local, 64, cancelled)?;
            let actual = fs::read(&local).map_err(|error| error.to_string())?;
            let expected: &[u8] = if left.is_ok() {
                b"ricefield-smb-safe-a"
            } else {
                b"ricefield-smb-safe-b"
            };
            if actual != expected {
                return Err("SMB hardlink target differs from the winning stage".into());
            }
            fs::remove_file(&local).map_err(|error| error.to_string())?;
        }
        cleanup(
            target,
            &dir,
            &["target-0", "target-1", "target-2", "target-3", "a", "b"],
            cancelled,
        )
    })();
    result.map_err(|error| format!("{error}. SMB probe objects might remain under {uri}; inspect the recovery journal and server before cleanup"))?;
    pending.finish();
    Ok(())
}

pub fn upload(request: UploadRequest, cancelled: &AtomicBool) -> Result<UploadResult, String> {
    let target = Target::parse(
        &request.remote_directory,
        request.credentials.username.clone(),
        request.credentials.password.clone(),
        request.credentials.domain.clone(),
    )?;
    upload_with_target(request, cancelled, &target, None)
}

fn upload_with_target(
    request: UploadRequest,
    cancelled: &AtomicBool,
    target: &Target,
    journal_override: Option<&Path>,
) -> Result<UploadResult, String> {
    let filename = request.source.file_name().ok_or("Select one local file")?;
    let name = basename(filename)?;
    let encoded = remote_upload::escape_filename(filename)?;
    let destination = target.at(name);
    let destination_uri = format!("{}/{}", target.uri, encoded);
    let snapshot = Snapshot::create(&request.source, cancelled)?;
    safety_probe(target, &snapshot, cancelled, journal_override)?;

    let id = remote_upload::random_suffix().map_err(|error| error.to_string())?;
    let dir = target.at(&format!(".ricefield-upload-{id}"));
    let stage_uri = format!("{}/.ricefield-upload-{id}", target.uri);
    let pending = match journal_override {
        Some(path) => {
            remote_upload_journal::PendingUpload::begin_in(path, &stage_uri, &destination_uri)
        }
        None => remote_upload_journal::PendingUpload::begin(&stage_uri, &destination_uri),
    }
    .map_err(|error| format!("Cannot durably record SMB stage before remote writes: {error}"))?;
    let result = (|| {
        target.run(&format!("mkdir {}", quoted(&dir)), cancelled)?;
        let stage = format!("{dir}/payload");
        put(target, snapshot.path(), &stage, cancelled)?;
        #[cfg(test)]
        kill_test_process_at("stage-put");
        let verify = snapshot.private_file("smb-verify.payload");
        get(target, &stage, &verify, snapshot.bytes, cancelled)?;
        compare(snapshot.path(), &verify, cancelled)?;
        fs::remove_file(&verify).map_err(|error| error.to_string())?;
        if cancelled.load(Ordering::Relaxed) {
            return Err("SMB upload cancelled before publication".into());
        }
        // Once publication might have reached the server, do not cancel,
        // retry or resolve an ambiguous answer by checking destination state.
        let commit_guard = AtomicBool::new(false);
        target.run(
            &format!("hardlink {} {}", quoted(&stage), quoted(&destination)),
            &commit_guard,
        )?;
        cleanup(target, &dir, &["payload"], &commit_guard)
    })();
    result.map_err(|error| format!("{error}. SMB publication may be ambiguous; inspect {stage_uri}/payload and {destination_uri} before discarding local evidence"))?;
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
    use std::{os::unix::process::ExitStatusExt, path::PathBuf};

    fn samba_root() -> PathBuf {
        PathBuf::from(
            std::env::var_os("RICEFIELD_TEST_SMB_ROOT")
                .expect("Run through scripts/test_smb_atomicity.sh with the Rust integration flag"),
        )
    }

    fn samba_target() -> Target {
        Target::parse(
            "smb://127.0.0.1:14445/authenticated",
            "nobody".into(),
            "ricefield-test-secret".into(),
            "WORKGROUP".into(),
        )
        .unwrap()
    }

    fn samba_request(root: &Path, name: &str) -> UploadRequest {
        UploadRequest {
            source: root.join(name),
            remote_directory: "smb://127.0.0.1:14445/authenticated".into(),
            credentials: MountCredentials {
                username: "nobody".into(),
                password: "ricefield-test-secret".into(),
                domain: "WORKGROUP".into(),
            },
            allow_insecure_http: false,
        }
    }

    #[test]
    #[ignore = "requires disposable signed Samba on port 14445"]
    fn real_samba_rust_uploader_publishes_and_refuses_existing_target() {
        let root = samba_root();
        let name = format!("rust-smb-{}.txt", remote_upload::random_suffix().unwrap());
        let source = root.join(&name);
        fs::write(&source, b"signed SMB Rust uploader snapshot\n").unwrap();
        let target = samba_target();
        let journal = root.join("rust-smb-uploader-evidence");
        let request = samba_request(&root, &name);
        let uploaded =
            upload_with_target(request, &AtomicBool::new(false), &target, Some(&journal)).unwrap();
        assert_eq!(
            uploaded.bytes,
            b"signed SMB Rust uploader snapshot\n".len() as u64
        );
        assert_eq!(
            fs::read(root.join("share").join(&name)).unwrap(),
            fs::read(&source).unwrap()
        );
        let (uploads, probes) = remote_upload_journal::inspect_evidence_in(&journal).unwrap();
        assert!(uploads.is_empty() && probes.is_empty());

        // The Samba daemon owns the server file as `nobody`; the unprivileged
        // test runner cannot assume permission to rewrite it directly. Make
        // the second local source different instead, then verify that the
        // already-published server bytes remain byte-for-byte unchanged.
        fs::write(&source, b"different second attempt").unwrap();
        let error = upload_with_target(
            samba_request(&root, &name),
            &AtomicBool::new(false),
            &target,
            Some(&journal),
        )
        .unwrap_err();
        assert!(
            error.contains("SMB publication may be ambiguous"),
            "{error}"
        );
        assert_eq!(
            fs::read(root.join("share").join(&name)).unwrap(),
            b"signed SMB Rust uploader snapshot\n"
        );
        assert_eq!(fs::read(&source).unwrap(), b"different second attempt");
        let (uploads, probes) = remote_upload_journal::inspect_evidence_in(&journal).unwrap();
        assert_eq!(
            uploads.len(),
            1,
            "collision retains manual recovery evidence"
        );
        assert!(probes.is_empty());
    }

    fn run_samba_crash_child_if_requested() {
        let Ok(case) = std::env::var("RICEFIELD_TEST_SMB_CRASH_CASE") else {
            return;
        };
        let root = samba_root();
        let name = format!("rust-smb-crash-{case}.txt");
        let journal = root.join(format!("rust-smb-crash-journal-{case}"));
        let result = upload_with_target(
            samba_request(&root, &name),
            &AtomicBool::new(false),
            &samba_target(),
            Some(&journal),
        );
        panic!("SMB child-process crash checkpoint not reached: {result:?}");
    }

    #[test]
    #[ignore = "requires disposable signed Samba on port 14445"]
    fn real_samba_rust_uploader_crash_keeps_server_and_journal_evidence() {
        run_samba_crash_child_if_requested();
        let root = samba_root();
        for (case, checkpoint) in [("probe", "probe-first-put"), ("stage", "stage-put")] {
            let name = format!("rust-smb-crash-{case}.txt");
            let source = root.join(&name);
            fs::write(&source, b"signed SMB crash source retained\n").unwrap();
            let mut child = Command::new(std::env::current_exe().unwrap())
                .arg("remote_upload_smb::tests::real_samba_rust_uploader_crash_keeps_server_and_journal_evidence")
                .arg("--exact")
                .arg("--ignored")
                .env("RICEFIELD_TEST_SMB_CRASH_CASE", case)
                .env("RICEFIELD_TEST_SMB_CRASH_AT", checkpoint)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            let deadline = Instant::now() + Duration::from_secs(80);
            let status = loop {
                if let Some(status) = child.try_wait().unwrap() {
                    break status;
                }
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!("SMB crash fixture never reached {checkpoint}");
                }
                thread::sleep(Duration::from_millis(30));
            };
            assert_eq!(
                status.signal(),
                Some(libc::SIGKILL),
                "{checkpoint}: {status}"
            );
            let journal = root.join(format!("rust-smb-crash-journal-{case}"));
            let (uploads, probes) = remote_upload_journal::inspect_evidence_in(&journal).unwrap();
            if case == "probe" {
                assert!(uploads.is_empty());
                assert_eq!(probes.len(), 1);
                let stage = root
                    .join("share")
                    .join(format!(".ricefield-safety-{}", probes[0].id));
                assert_eq!(fs::read(stage.join("a")).unwrap(), b"ricefield-smb-safe-a");
            } else {
                assert!(probes.is_empty());
                assert_eq!(uploads.len(), 1);
                let stage = root
                    .join("share")
                    .join(format!(".ricefield-upload-{}", uploads[0].id));
                assert_eq!(
                    fs::read(stage.join("payload")).unwrap(),
                    b"signed SMB crash source retained\n"
                );
            }
            assert!(!root.join("share").join(&name).exists());
            assert_eq!(
                fs::read(&source).unwrap(),
                b"signed SMB crash source retained\n"
            );
        }
    }

    #[test]
    fn smb_uri_paths_and_credentials_fail_closed() {
        assert!(
            Target::parse(
                "smb://nas.example/media",
                "user".into(),
                "secret".into(),
                "".into()
            )
            .is_ok()
        );
        assert!(
            Target::parse(
                "smb://nas.example/media/a%20b",
                "user".into(),
                "secret".into(),
                "".into()
            )
            .is_ok()
        );
        for uri in [
            "smb://nas.example",
            "smb://nas.example/media/../file",
            "smb://nas.example/media/%0Als",
            "smb://nas.example/media/a%3Bdel",
            "smb://evil.example@nas.example/share",
            "smb://[::1]/share",
            "smb://nas.example/media/a%22b",
            "smb://nas.example/media/a%5Cb",
            "smb://nas.example/media%2Fprivate",
        ] {
            assert!(
                Target::parse(uri, "user".into(), "secret".into(), "".into()).is_err(),
                "{uri}"
            );
        }
        assert!(
            Target::parse(
                "smb://nas.example/media",
                "".into(),
                "secret".into(),
                "".into()
            )
            .is_err()
        );
        assert!(
            Target::parse(
                "smb://nas.example/media",
                "user\npassword = hacked".into(),
                "secret".into(),
                "".into()
            )
            .is_err()
        );
        assert!(
            Target::parse(
                "smb://nas.example/media",
                "user".into(),
                "secret\nuser = hacked".into(),
                "".into()
            )
            .is_err()
        );
        assert!(basename(OsStr::new("file;rm.txt")).is_err());
    }

    #[test]
    fn smb_verified_stage_requires_exact_bytes_and_length() {
        let directory = std::env::temp_dir().join(format!(
            "ricefield-smb-verify-{}",
            remote_upload::random_suffix().unwrap()
        ));
        fs::create_dir(&directory).unwrap();
        let source = directory.join("source");
        let copy = directory.join("copy");
        let cancelled = AtomicBool::new(false);
        fs::write(&source, b"exact payload").unwrap();
        fs::write(&copy, b"exact payload").unwrap();
        assert!(compare(&source, &copy, &cancelled).is_ok());
        fs::write(&copy, b"exact payload plus").unwrap();
        assert!(compare(&source, &copy, &cancelled).is_err());
        fs::write(&copy, b"short").unwrap();
        assert!(compare(&source, &copy, &cancelled).is_err());
        fs::write(&copy, b"wrong payload").unwrap();
        assert!(compare(&source, &copy, &cancelled).is_err());
        cancelled.store(true, Ordering::Relaxed);
        fs::write(&copy, b"exact payload").unwrap();
        assert!(compare(&source, &copy, &cancelled).is_err());
        fs::remove_dir_all(directory).unwrap();
    }
}
