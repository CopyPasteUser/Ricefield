//! File clipboard interoperability with GTK file managers on X11 and Wayland.
//! The internal clipboard remains available when xclip/wl-clipboard is absent.
use std::{
    ffi::OsString,
    fs::File,
    io::{self, Read, Seek, Write},
    os::fd::{AsRawFd, FromRawFd},
    path::PathBuf,
    process::{Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant},
};

const MAX_DESKTOP_CLIPBOARD_BYTES: u64 = 4 * 1024 * 1024;
const CLIPBOARD_HELPER_TIMEOUT: Duration = Duration::from_secs(3);

// Give helpers an anonymous, seekable file instead of a pipe. A misbehaving
// clipboard provider can otherwise block write_all/read_to_end indefinitely,
// even after the UI abandons its pending clipboard request. The output seal
// also prevents a provider from growing memory past the clipboard limit.
fn clipboard_file() -> io::Result<File> {
    let fd = unsafe {
        libc::memfd_create(
            c"ricefield-clipboard".as_ptr(),
            libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING,
        )
    };
    if fd < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(unsafe { File::from_raw_fd(fd) })
    }
}

fn run_helper(
    mut command: Command,
    input: Option<&[u8]>,
    timeout: Duration,
) -> io::Result<(ExitStatus, Vec<u8>)> {
    let mut data = clipboard_file()?;
    if let Some(input) = input {
        data.write_all(input)?;
        data.rewind()?;
        command.stdin(Stdio::from(data.try_clone()?));
        command.stdout(Stdio::null());
    } else {
        data.set_len(MAX_DESKTOP_CLIPBOARD_BYTES + 1)?;
        if unsafe { libc::fcntl(data.as_raw_fd(), libc::F_ADD_SEALS, libc::F_SEAL_GROW) } < 0 {
            return Err(io::Error::last_os_error());
        }
        command.stdin(Stdio::null());
        command.stdout(Stdio::from(data.try_clone()?));
    }
    let mut child = command.stderr(Stdio::null()).spawn()?;
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "Clipboard helper timed out",
                ));
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
        }
    };
    if input.is_some() {
        return Ok((status, Vec::new()));
    }
    let length = data.stream_position()?;
    if length > MAX_DESKTOP_CLIPBOARD_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Clipboard payload is too large",
        ));
    }
    data.rewind()?;
    let mut output = Vec::with_capacity(length as usize);
    data.take(length).read_to_end(&mut output)?;
    Ok((status, output))
}

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
        let mut command = Command::new(program);
        command.args(args);
        if run_helper(command, Some(data.as_bytes()), CLIPBOARD_HELPER_TIMEOUT)
            .is_ok_and(|(status, _)| status.success())
        {
            return Ok(());
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
        let mut command = Command::new(program);
        command.args(args);
        let Ok((status, bytes)) = run_helper(command, None, CLIPBOARD_HELPER_TIMEOUT) else {
            continue;
        };
        if !status.success() {
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

    #[test]
    fn desktop_clipboard_capture_and_input_are_bounded_without_pipes() {
        let mut output = Command::new("sh");
        output.args(["-c", "printf 'cut\\nfile:///tmp/example\\n'"]);
        let (status, bytes) = run_helper(output, None, Duration::from_secs(2)).unwrap();
        assert!(status.success());
        assert_eq!(
            parse_files(std::str::from_utf8(&bytes).unwrap())
                .unwrap()
                .paths,
            vec![PathBuf::from("/tmp/example")]
        );

        let mut input = Command::new("sh");
        input.args(["-c", "test \"$(cat)\" = 'copy'"]);
        assert!(
            run_helper(input, Some(b"copy\n"), Duration::from_secs(2))
                .unwrap()
                .0
                .success()
        );
    }

    #[test]
    fn unresponsive_clipboard_helper_is_killed_before_ui_deadline() {
        let mut command = Command::new("sleep");
        command.arg("10");
        let started = Instant::now();
        assert_eq!(
            run_helper(command, None, Duration::from_millis(100))
                .unwrap_err()
                .kind(),
            io::ErrorKind::TimedOut
        );
        assert!(started.elapsed() < Duration::from_secs(2));

        let mut blocked_writer = Command::new("sleep");
        blocked_writer.arg("10");
        assert_eq!(
            run_helper(
                blocked_writer,
                Some(&vec![b'x'; 2 * 1024 * 1024]),
                Duration::from_millis(100),
            )
            .unwrap_err()
            .kind(),
            io::ErrorKind::TimedOut
        );
    }

    #[test]
    fn desktop_clipboard_output_cannot_exceed_memory_budget() {
        let mut command = Command::new("head");
        command.args(["-c", "4194306", "/dev/zero"]);
        match run_helper(command, None, Duration::from_secs(2)) {
            Ok((status, output)) => {
                assert!(!status.success(), "An oversized output must fail");
                assert!(output.len() as u64 <= MAX_DESKTOP_CLIPBOARD_BYTES);
            }
            Err(error) => assert_eq!(error.kind(), io::ErrorKind::InvalidData),
        }
    }
}
