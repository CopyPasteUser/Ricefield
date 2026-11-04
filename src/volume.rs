//! GIO volume actions run outside the egui thread. The system's GIO/GVfs
//! handles device permissions and Polkit; RiceExplorer never invokes sudo.

use std::{
    ffi::OsString,
    path::PathBuf,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

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
}
