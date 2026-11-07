//! Optional XDG desktop portal integration. A portal request is dispatched
//! only after the user explicitly asks the OS to choose an application.
//! This does not modify MIME associations or construct shell commands.

use std::{
    collections::HashMap,
    fs::{File, OpenOptions},
    os::unix::fs::OpenOptionsExt,
    path::Path,
};
use zbus::{
    blocking::{Connection, Proxy},
    zvariant::{Fd, OwnedObjectPath, Value},
};

const PORTAL_BUS: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
const OPEN_URI: &str = "org.freedesktop.portal.OpenURI";

/// Open the selected inode without blocking on FIFO devices. The descriptor,
/// not a potentially changed filename, is sent to the portal. The portal is
/// responsible for asking the user which application receives read access.
fn local_target(path: &Path) -> Result<File, String> {
    if !path.is_absolute() {
        return Err("The system app chooser needs an absolute local path".into());
    }
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)
        .map_err(|error| format!("Cannot open selected file for the system chooser: {error}"))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("Cannot inspect the selected file: {error}"))?;
    if !metadata.is_file() && !metadata.is_dir() {
        return Err("The system chooser only accepts ordinary files and folders".into());
    }
    Ok(file)
}

/// Returns when the portal accepts the request. The actual application
/// selection and possible user cancellation happen asynchronously in the OS.
/// OpenFile and its `ask=true` option require OpenURI interface version 3.
pub fn choose_application(path: &Path) -> Result<(), String> {
    let file = local_target(path)?;
    let connection = Connection::session()
        .map_err(|error| format!("Desktop session bus unavailable: {error}"))?;
    let proxy = Proxy::new(&connection, PORTAL_BUS, PORTAL_PATH, OPEN_URI)
        .map_err(|error| format!("Cannot contact desktop portal: {error}"))?;
    let version: u32 = proxy
        .get_property("version")
        .map_err(|error| format!("Cannot read desktop portal version: {error}"))?;
    if version < 3 {
        return Err("The installed desktop portal does not support choosing an application".into());
    }
    let mut options = HashMap::<&str, Value<'_>>::new();
    options.insert("ask", Value::Bool(true));
    let fd = Fd::from(&file);
    let _: OwnedObjectPath = proxy
        .call("OpenFile", &("", fd, options))
        .map_err(|error| format!("The desktop portal refused the app chooser: {error}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;

    #[test]
    fn chooser_only_accepts_pinned_local_regular_files_and_directories() {
        let root = std::env::temp_dir().join(format!(
            "ricefield-portal-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let file = root.join("file.txt");
        std::fs::write(&file, b"portal data").unwrap();
        assert!(local_target(&file).unwrap().metadata().unwrap().is_file());
        assert!(local_target(&root).unwrap().metadata().unwrap().is_dir());
        assert!(local_target(Path::new("relative.txt")).is_err());
        let socket = root.join("socket");
        let _listener = UnixListener::bind(&socket).unwrap();
        assert!(local_target(&socket).is_err());
        let fifo = root.join("pipe");
        let name = std::ffi::CString::new(std::os::unix::ffi::OsStrExt::as_bytes(fifo.as_os_str()))
            .unwrap();
        // SAFETY: `name` is a NUL-terminated pathname within our temp dir.
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        assert!(local_target(&fifo).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
