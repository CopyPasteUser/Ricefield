//! Address parsing shared by the command line and the desktop address bar.
//! Remote locations are modeled distinctly; filesystem operations must never
//! accidentally interpret an unsupported network URI as a relative path.

use std::{ffi::OsString, path::PathBuf};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Location {
    Local(PathBuf),
    Remote(RemoteLocation),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteLocation {
    pub scheme: RemoteScheme,
    /// An opaque, validated URI: keep encoded path bytes intact for GIO.
    pub uri: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RemoteScheme {
    Smb,
    Sftp,
    Dav,
    Davs,
    Mtp,
}

impl Location {
    pub fn parse(argument: OsString) -> Result<Self, &'static str> {
        let Some(text) = argument.to_str() else {
            // Unix filenames need not contain valid UTF-8.
            return Ok(Self::Local(PathBuf::from(argument)));
        };
        if text
            .get(..7)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("file://"))
        {
            return Ok(Self::Local(local_file_uri(text)?));
        }
        if !looks_like_uri(text) {
            return Ok(Self::Local(PathBuf::from(argument)));
        }
        let (scheme, remainder) = text.split_once("://").ok_or("Unsupported URI scheme")?;
        let scheme = match scheme.to_ascii_lowercase().as_str() {
            "smb" => RemoteScheme::Smb,
            "sftp" => RemoteScheme::Sftp,
            "dav" => RemoteScheme::Dav,
            "davs" => RemoteScheme::Davs,
            "mtp" => RemoteScheme::Mtp,
            _ => return Err("Unsupported URI scheme"),
        };
        let authority = remainder.split('/').next().unwrap_or_default();
        if authority.is_empty()
            || authority.contains(['@', '?', '#', '\\', '%'])
            || authority.chars().any(char::is_whitespace)
            || authority.chars().any(char::is_control)
        {
            return Err("Invalid remote URI authority; credentials belong in a secure prompt");
        }
        validate_remote_host(authority)?;
        if remainder.contains(['?', '#', '\\'])
            || remainder
                .chars()
                .any(|ch| ch.is_whitespace() || ch.is_control())
            || !valid_percent_escapes(remainder)
        {
            return Err("Invalid remote URI: use percent-encoded path characters");
        }
        Ok(Self::Remote(RemoteLocation {
            scheme,
            uri: text.to_owned(),
        }))
    }

    /// Only local locations can be passed to the current POSIX file engine.
    pub fn into_local_path(self) -> Result<PathBuf, &'static str> {
        match self {
            Self::Local(path) => Ok(path),
            Self::Remote(_) => Err(
                "Remote locations require the upcoming GIO backend; open a mounted path for now",
            ),
        }
    }
}

fn validate_remote_host(authority: &str) -> Result<(), &'static str> {
    let suffix = if let Some(bracketed) = authority.strip_prefix('[') {
        let (host, suffix) = bracketed
            .split_once(']')
            .ok_or("Invalid bracketed remote host")?;
        if host.is_empty() {
            return Err("Invalid bracketed remote host");
        }
        suffix
    } else {
        let (host, suffix) = authority.split_once(':').unwrap_or((authority, ""));
        if host.is_empty() || host.contains(['[', ']']) {
            return Err("Invalid remote host");
        }
        if authority.contains(':') {
            return validate_port(suffix);
        }
        ""
    };
    if suffix.is_empty() {
        Ok(())
    } else if let Some(port) = suffix.strip_prefix(':') {
        validate_port(port)
    } else {
        Err("Invalid remote host port")
    }
}

fn validate_port(port: &str) -> Result<(), &'static str> {
    if port.is_empty() || !port.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("Invalid remote port");
    }
    match port.parse::<u16>() {
        Ok(1..) => Ok(()),
        _ => Err("Invalid remote port"),
    }
}

fn valid_percent_escapes(value: &str) -> bool {
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if bytes
                .get(index + 1)
                .and_then(|byte| (*byte as char).to_digit(16))
                .is_none()
                || bytes
                    .get(index + 2)
                    .and_then(|byte| (*byte as char).to_digit(16))
                    .is_none()
            {
                return false;
            }
            // Reject NUL even when encoded, including in a remote path.
            if u8::from_str_radix(&value[index + 1..index + 3], 16).ok() == Some(0) {
                return false;
            }
            index += 3;
        } else {
            index += 1;
        }
    }
    true
}

fn looks_like_uri(text: &str) -> bool {
    let Some((scheme, suffix)) = text.split_once(':') else {
        return false;
    };
    let mut bytes = scheme.bytes();
    bytes.next().is_some_and(|byte| byte.is_ascii_alphabetic())
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'-' | b'.'))
        && (suffix.starts_with('/') || matches!(scheme, "trash" | "recent" | "network"))
}

fn local_file_uri(uri: &str) -> Result<PathBuf, &'static str> {
    let rest = uri.get(7..).ok_or("Invalid file URI")?;
    let absolute = if rest.starts_with('/') {
        rest.to_owned()
    } else {
        let (host, path) = rest.split_once('/').ok_or("Invalid file URI")?;
        if !host.eq_ignore_ascii_case("localhost") {
            return Err("Only local file:// URIs are supported");
        }
        format!("/{path}")
    };
    if absolute.contains(&['?', '#'][..]) {
        return Err("Invalid file URI: query and fragment are not supported");
    }
    let source = absolute.as_bytes();
    let mut decoded = Vec::with_capacity(source.len());
    let mut index = 0;
    while index < source.len() {
        if source[index] == b'%' {
            let encoded = source
                .get(index + 1..index + 3)
                .ok_or("Invalid file URI escape")?;
            let hex = std::str::from_utf8(encoded).map_err(|_| "Invalid file URI escape")?;
            decoded.push(u8::from_str_radix(hex, 16).map_err(|_| "Invalid file URI escape")?);
            index += 3;
        } else {
            decoded.push(source[index]);
            index += 1;
        }
    }
    if decoded.contains(&0) {
        return Err("Invalid file URI: NUL byte");
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        Ok(PathBuf::from(OsString::from_vec(decoded)))
    }
    #[cfg(not(unix))]
    {
        let path = String::from_utf8(decoded).map_err(|_| "Invalid file URI encoding")?;
        Ok(PathBuf::from(path))
    }
}

#[cfg(test)]
mod tests {
    use super::{Location, RemoteScheme};

    #[test]
    fn keeps_local_and_file_uri_paths_exact() {
        assert_eq!(
            Location::parse("file:///tmp/a%20b%23c".into()).unwrap(),
            Location::Local("/tmp/a b#c".into())
        );
        assert_eq!(
            Location::parse("file://LOCALHOST/home/user".into()).unwrap(),
            Location::Local("/home/user".into())
        );
        assert_eq!(
            Location::parse("notes:2026".into()).unwrap(),
            Location::Local("notes:2026".into())
        );
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_local_paths_remain_native() {
        use std::{ffi::OsString, os::unix::ffi::OsStringExt};

        let raw = OsString::from_vec(vec![b'/', b't', b'm', b'p', b'/', 0xff]);
        assert_eq!(
            Location::parse(raw.clone()).unwrap(),
            Location::Local(raw.into())
        );
    }

    #[test]
    fn remote_schemes_are_distinct_from_local_paths() {
        for (uri, scheme) in [
            ("smb://nas.local/shared/My%20Files", RemoteScheme::Smb),
            ("sftp://[2001:db8::1]:22/home", RemoteScheme::Sftp),
            ("davs://cloud.example:443/dav", RemoteScheme::Davs),
            ("dav://host/dav", RemoteScheme::Dav),
            ("mtp://[usb:001,008]/", RemoteScheme::Mtp),
        ] {
            let location = Location::parse(uri.into()).unwrap();
            assert_eq!(
                location,
                Location::Remote(super::RemoteLocation {
                    scheme,
                    uri: uri.into()
                })
            );
            assert!(location.into_local_path().is_err());
        }
    }

    #[test]
    fn invalid_remote_input_is_rejected_before_backend_use() {
        for uri in [
            "smb://user:password@host/share",
            "sftp://host:abc/home",
            "sftp://host:70000/home",
            "smb:///share",
            "smb://host/file%ZZ",
            "smb://host/file%00",
            "smb://host/name#fragment",
            "smb://host/name?query",
            "smb://host\\evil/share",
            "smb://host/unencoded space",
            "sftp://host/line\nfeed",
            "ftp://host/path",
            "trash:///",
            "sftp://host:/home",
            "sftp://[]/home",
        ] {
            assert!(Location::parse(uri.into()).is_err(), "accepted: {uri}");
        }
    }
}
