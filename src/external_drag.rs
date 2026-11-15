//! On-demand bridge to a native Linux drag source.
//!
//! eframe 0.37 uses winit 0.30, which cannot initiate an outbound desktop
//! data transfer. A separately installed ripdrag/dragon helper owns that
//! native interaction on X11/Wayland without altering our event loop. This
//! is explicitly invoked from the file context menu, never during an internal
//! drag, so the built-in safe copy-by-drag behavior remains unchanged.

use std::{
    io,
    path::PathBuf,
    process::{Child, Command, ExitStatus, Stdio},
    sync::mpsc::{self, Receiver},
    thread,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Helper {
    Ripdrag,
    Dragon,
}

impl Helper {
    fn candidates() -> [Self; 2] {
        [Self::Ripdrag, Self::Dragon]
    }

    fn executable(self) -> &'static str {
        match self {
            Self::Ripdrag => "ripdrag",
            Self::Dragon => "dragon",
        }
    }

    fn configure(self, paths: &[PathBuf]) -> Command {
        let mut command = Command::new(self.executable());
        match self {
            Self::Ripdrag => {
                command.arg("--and-exit");
            }
            Self::Dragon => {
                command.arg("-x");
            }
        }
        // Absolute paths cannot be interpreted as option flags. Passing them
        // as individual OsStr arguments also avoids all shell expansion.
        command
            .args(paths)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        command
    }
}

fn validate_paths(paths: &[PathBuf]) -> Result<(), &'static str> {
    if paths.is_empty() || paths.len() > 128 {
        return Err("Select between 1 and 128 local files or folders to drag");
    }
    if paths.iter().any(|path| !path.is_absolute()) {
        return Err("External drag requires absolute local filesystem paths");
    }
    Ok(())
}

pub struct DragSession {
    pub helper: &'static str,
    pub completed: Receiver<io::Result<ExitStatus>>,
}

fn launch_with<F>(paths: &[PathBuf], mut start: F) -> Result<DragSession, String>
where
    F: FnMut(Helper, &[PathBuf]) -> io::Result<Child>,
{
    validate_paths(paths).map_err(str::to_owned)?;
    for helper in Helper::candidates() {
        match start(helper, paths) {
            Ok(mut child) => {
                // Reap off-thread and report helper failures to the UI.
                let (sender, completed) = mpsc::channel();
                thread::spawn(move || {
                    let _ = sender.send(child.wait());
                });
                return Ok(DragSession {
                    helper: helper.executable(),
                    completed,
                });
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(format!("Cannot launch {}: {error}", helper.executable())),
        }
    }
    Err("External drag needs ripdrag or dragon (X11/Wayland). No files were changed; use Ctrl+C and paste into the destination app instead.".into())
}

pub fn launch(paths: &[PathBuf]) -> Result<DragSession, String> {
    launch_with(paths, |helper, paths| helper.configure(paths).spawn())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outbound_drag_only_accepts_absolute_local_paths() {
        assert!(validate_paths(&[]).is_err());
        assert!(validate_paths(&[PathBuf::from("relative")]).is_err());
        assert!(validate_paths(&[PathBuf::from("/tmp/a b\n --untrusted")]).is_ok());
        assert!(validate_paths(&vec![PathBuf::from("/tmp/file"); 129]).is_err());
    }

    #[test]
    fn failed_helper_exit_is_reported_without_blocking_gui() {
        use std::time::Duration;
        let files = [PathBuf::from("/tmp/ricefield-drag-source")];
        let session = launch_with(&files, |_, _| Command::new("false").spawn()).unwrap();
        let status = session
            .completed
            .recv_timeout(Duration::from_secs(3))
            .unwrap()
            .unwrap();
        assert!(!status.success());
        assert_eq!(session.helper, "ripdrag");
    }

    #[test]
    fn bridge_does_not_invoke_shell() {
        let paths = [PathBuf::from("/tmp/a file; $(false)")];
        let command = Helper::Ripdrag.configure(&paths);
        assert_eq!(command.get_program(), "ripdrag");
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            vec!["--and-exit", "/tmp/a file; $(false)"]
        );
    }
}
