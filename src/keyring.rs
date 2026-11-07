//! Persist explicitly opted-in network logins in the desktop Secret Service.
//! No credentials are written to RiceField config, argv or log messages.
//! `secret-tool` is an optional libsecret helper; there is no plaintext fallback.

use crate::{
    location::Location,
    volume::{MountCredentials, clear_password},
};
use serde::{Deserialize, Serialize};
use std::{
    io::{self, Read, Write},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

const SECRET_LIMIT: usize = 4096;
const KEYRING_TIMEOUT: Duration = Duration::from_secs(25);

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredLogin {
    version: u8,
    username: String,
    domain: String,
    password: String,
}

impl Drop for StoredLogin {
    fn drop(&mut self) {
        clear_password(&mut self.password);
    }
}

fn valid_location(uri: &str) -> Result<(), String> {
    match Location::parse(uri.into()) {
        Ok(Location::Remote(remote)) if remote.uri == uri && uri.len() <= 2048 => Ok(()),
        _ => Err("Keyring requires a validated network location".into()),
    }
}

#[derive(Clone, Copy)]
enum Action {
    Store,
    Lookup,
    Clear,
}

fn command(action: Action, uri: &str) -> Command {
    let mut command = Command::new("secret-tool");
    match action {
        Action::Store => {
            command.args(["store", "--label=RiceField network login"]);
        }
        Action::Lookup => {
            command.arg("lookup");
        }
        Action::Clear => {
            command.arg("clear");
        }
    }
    command.args(["application", "ricefield", "location", uri]);
    command
}

fn run(
    mut command: Command,
    input: Option<&[u8]>,
    read_output: bool,
) -> Result<(bool, Vec<u8>), String> {
    let mut child = command
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(if read_output {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| {
            "Linux keyring unavailable (install libsecret-tools and unlock the user keyring)"
                .to_owned()
        })?;
    let reader = child.stdout.take().map(|stdout| {
        thread::spawn(move || {
            let mut bytes = Vec::new();
            stdout
                .take((SECRET_LIMIT + 1) as u64)
                .read_to_end(&mut bytes)?;
            Ok::<_, io::Error>(bytes)
        })
    });
    if let Some(bytes) = input {
        let write_result = child
            .stdin
            .take()
            .ok_or("Linux keyring input unavailable")?
            .write_all(bytes);
        if write_result.is_err() {
            let _ = child.kill();
            let _ = child.wait();
            if let Some(reader) = reader {
                let _ = reader.join();
            }
            return Err("Could not send login to the Linux keyring".into());
        }
    }
    let deadline = Instant::now() + KEYRING_TIMEOUT;
    let result = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status.success()),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(50)),
            Ok(None) => break Err("Linux keyring operation timed out".to_owned()),
            Err(_) => break Err("Linux keyring operation failed".to_owned()),
        }
    };
    if result.is_err() {
        let _ = child.kill();
    }
    let _ = child.wait();
    let bytes = if let Some(reader) = reader {
        reader
            .join()
            .map_err(|_| "Linux keyring reader failed")?
            .map_err(|_| "Cannot read Linux keyring response")?
    } else {
        Vec::new()
    };
    if bytes.len() > SECRET_LIMIT {
        return Err("Saved network login is larger than the safety limit".into());
    }
    result.map(|status| (status, bytes))
}

pub fn save(uri: &str, credentials: &MountCredentials) -> Result<(), String> {
    valid_location(uri)?;
    if credentials.password.is_empty() {
        return Err("Cannot save a network login without a password".into());
    }
    let login = StoredLogin {
        version: 1,
        username: credentials.username.clone(),
        domain: credentials.domain.clone(),
        password: credentials.password.clone(),
    };
    let mut serialized = toml::to_string(&login).map_err(|_| "Cannot encode network login")?;
    if serialized.len() > SECRET_LIMIT {
        clear_password(&mut serialized);
        return Err("Network login is too long for the Linux keyring".into());
    }
    let result = run(
        command(Action::Store, uri),
        Some(serialized.as_bytes()),
        false,
    );
    clear_password(&mut serialized);
    match result {
        Ok((true, _)) => Ok(()),
        Ok((false, _)) => Err("Linux keyring refused to save the network login".into()),
        Err(error) => Err(error),
    }
}

pub fn lookup(uri: &str) -> Result<Option<MountCredentials>, String> {
    valid_location(uri)?;
    let (success, bytes) = run(command(Action::Lookup, uri), None, true)?;
    if !success || bytes.is_empty() {
        return Ok(None);
    }
    let mut plaintext =
        String::from_utf8(bytes).map_err(|_| "Linux keyring returned an invalid network login")?;
    let decoded = toml::from_str::<StoredLogin>(&plaintext);
    clear_password(&mut plaintext);
    let login = decoded.map_err(|_| "Linux keyring contains an unsupported network login")?;
    if login.version != 1 {
        return Err("Linux keyring network login version is unsupported".into());
    }
    Ok(Some(MountCredentials {
        username: login.username.clone(),
        domain: login.domain.clone(),
        password: login.password.clone(),
    }))
}

pub fn forget(uri: &str) -> Result<(), String> {
    valid_location(uri)?;
    match run(command(Action::Clear, uri), None, false)? {
        (true, _) => Ok(()),
        _ => Err("Linux keyring could not remove the saved login".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoding_is_not_plaintext_config_and_supports_special_passwords() {
        let input = StoredLogin {
            version: 1,
            username: "simon".into(),
            domain: "HOME".into(),
            password: "quote=\"\npass".into(),
        };
        let encoded = toml::to_string(&input).unwrap();
        let decoded: StoredLogin = toml::from_str(&encoded).unwrap();
        assert_eq!(decoded.password, input.password);
        assert!(valid_location("smb://nas.local/Media").is_ok());
        assert!(valid_location("smb://user:pass@nas.local/Media").is_err());
    }

    #[test]
    fn keyring_arguments_never_contain_login_secrets() {
        let creds = MountCredentials {
            username: "alice".into(),
            domain: "workgroup".into(),
            password: "pass-private".into(),
        };
        let cmd = command(Action::Store, "smb://nas.local/Share");
        let args = cmd
            .get_args()
            .map(|value| value.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(!args.contains(&creds.password));
        assert!(!args.contains(&creds.username));
        assert_eq!(cmd.get_program(), "secret-tool");
    }

    #[test]
    fn subprocess_io_is_bounded_and_shell_free() {
        let mut command = Command::new("sh");
        command
            .arg("-c")
            .arg("read line; test \"$line\" = payload; printf saved");
        let (status, bytes) = run(command, Some(b"payload\n"), true).unwrap();
        assert!(status);
        assert_eq!(bytes, b"saved");
    }
}
