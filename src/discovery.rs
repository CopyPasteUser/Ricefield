//! Passive, read-only network discovery through the user's existing desktop
//! services. No port scans, privileged operations, or automatic logins.

use crate::{
    location::{Location, RemoteScheme},
    volume,
};
use std::{
    collections::HashSet,
    process::Command,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

const MAX_DISCOVERY_BYTES: u64 = 512 * 1024;
const MAX_RESULTS: usize = 64;
const MAX_ALIASES: usize = 6;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NetworkHost {
    pub label: String,
    pub uri: String,
    pub source: &'static str,
}

fn add_host(hosts: &mut Vec<NetworkHost>, label: &str, uri: &str, source: &'static str) {
    if hosts.len() >= MAX_RESULTS || uri.len() > 2048 {
        return;
    }
    if !matches!(Location::parse(uri.into()), Ok(Location::Remote(remote)) if remote.scheme == RemoteScheme::Smb)
    {
        return;
    }
    let label = label.trim();
    if label.is_empty() || label.len() > 128 || label.chars().any(char::is_control) {
        return;
    }
    hosts.push(NetworkHost {
        label: label.to_owned(),
        uri: uri.to_owned(),
        source,
    });
}

fn parse_gio(hosts: &mut Vec<NetworkHost>, bytes: &[u8], aliases: &mut Vec<String>) -> Vec<String> {
    let mut groups = Vec::new();
    let text = String::from_utf8_lossy(bytes);
    for line in text.lines().take(1024) {
        // With `gio list -l -a standard::target-uri -u`, the first token is
        // the entry URI; a later attribute can be its actual SMB target.
        for part in line.split_ascii_whitespace() {
            let uri = part
                .find("smb://")
                .map(|start| &part[start..])
                .unwrap_or(part);
            if let Some(group) = uri.strip_prefix("smb:///") {
                let group = group.trim_matches('/');
                if let Some((_, server)) = group.split_once('/') {
                    if !server.is_empty() && !server.contains('/') {
                        add_host(hosts, server, &format!("smb://{server}/"), "SMB workgroup");
                    }
                } else if !group.is_empty()
                    && !group.contains('/')
                    && group.len() < 100
                    && group.bytes().all(|byte| {
                        byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.')
                    })
                    && groups.len() < 4
                {
                    let group_uri = format!("smb:///{group}");
                    if !groups.contains(&group_uri) {
                        groups.push(group_uri);
                    }
                }
            } else if let Some(rest) = uri.strip_prefix("smb://") {
                let host = rest.split('/').next().unwrap_or("");
                add_host(hosts, host, &format!("smb://{host}/"), "SMB / GVfs");
            } else if let Some(name) = uri.strip_prefix("network:///")
                && aliases.len() < MAX_ALIASES
                && !name.is_empty()
                && name.len() <= 160
                && name.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'%')
                })
                && !matches!(name, "." | "..")
            {
                let alias = format!("network:///{name}");
                if !aliases.contains(&alias) {
                    aliases.push(alias);
                }
            }
        }
    }
    groups
}

fn parse_avahi(hosts: &mut Vec<NetworkHost>, bytes: &[u8]) {
    let text = String::from_utf8_lossy(bytes);
    for line in text.lines().take(1024) {
        let fields: Vec<_> = line.split(';').collect();
        if fields.len() < 9 || fields[0] != "=" || fields[4] != "_smb._tcp" {
            continue;
        }
        // Resolved DNS-SD records: =;interface;protocol;name;type;domain;
        // host;address;port;TXT. A resolved .local hostname is preferable to
        // an interface-scoped IPv6 address or a potentially changing IPv4 IP.
        let host = fields[6].trim_end_matches('.');
        let port = match fields[8].parse::<u16>() {
            Ok(1..) => fields[8],
            _ => continue,
        };
        let suffix = if port == "445" {
            "".to_owned()
        } else {
            format!(":{port}")
        };
        let uri = format!("smb://{host}{suffix}/");
        add_host(hosts, fields[3], &uri, "mDNS / Avahi");
    }
}

pub fn discover(cancelled: &AtomicBool) -> Result<Vec<NetworkHost>, String> {
    let mut hosts = Vec::new();
    let mut any_backend = false;
    let mut group_uris = Vec::new();
    let mut aliases = Vec::new();
    for root in ["smb:///", "network:///"] {
        if cancelled.load(Ordering::Relaxed) {
            return Err("Discovery cancelled".into());
        }
        let mut command = Command::new("gio");
        command
            .args(["list", "-l", "-a", "standard::target-uri", "-u", root])
            .env("LC_ALL", "C");
        if let Ok(bytes) = volume::capture_gio(
            command,
            MAX_DISCOVERY_BYTES,
            Duration::from_secs(5),
            cancelled,
            "SMB network discovery",
        ) {
            any_backend = true;
            group_uris.extend(parse_gio(&mut hosts, &bytes, &mut aliases));
        }
    }
    // The network:/// backend often exposes Samba servers as aliases such
    // as network:///smb-server-example rather than as direct SMB URIs.
    // Resolve only advertised aliases; never probe IP ranges or guessed hosts.
    for alias in aliases {
        if cancelled.load(Ordering::Relaxed) {
            return Err("Discovery cancelled".into());
        }
        let mut command = Command::new("gio");
        command
            .args(["info", "-a", "standard::target-uri", &alias])
            .env("LC_ALL", "C");
        if let Ok(bytes) = volume::capture_gio(
            command,
            16 * 1024,
            Duration::from_secs(3),
            cancelled,
            "Network alias lookup",
        ) {
            parse_gio(&mut hosts, &bytes, &mut Vec::new());
        }
    }
    for group in group_uris.into_iter().take(4) {
        if cancelled.load(Ordering::Relaxed) {
            return Err("Discovery cancelled".into());
        }
        let mut command = Command::new("gio");
        command.args(["list", "-u", &group]).env("LC_ALL", "C");
        if let Ok(bytes) = volume::capture_gio(
            command,
            MAX_DISCOVERY_BYTES,
            Duration::from_secs(3),
            cancelled,
            "SMB workgroup discovery",
        ) {
            parse_gio(&mut hosts, &bytes, &mut Vec::new());
        }
    }
    if !cancelled.load(Ordering::Relaxed) {
        let mut command = Command::new("avahi-browse");
        command.args(["-r", "-t", "-p", "-k", "_smb._tcp"]);
        if let Ok(bytes) = volume::capture_gio(
            command,
            MAX_DISCOVERY_BYTES,
            Duration::from_secs(5),
            cancelled,
            "mDNS service discovery",
        ) {
            any_backend = true;
            parse_avahi(&mut hosts, &bytes);
        }
    }
    if cancelled.load(Ordering::Relaxed) {
        return Err("Discovery cancelled".into());
    }
    let mut seen = HashSet::new();
    hosts.retain(|item| seen.insert(item.uri.to_ascii_lowercase()));
    hosts.sort_by_key(|item| item.label.to_ascii_lowercase());
    if !any_backend {
        return Err("No network discovery backend answered. Install gvfs-backends or avahi-utils, or connect to a known address manually.".into());
    }
    Ok(hosts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn avahi_only_offers_valid_smb_records() {
        let mut hosts = Vec::new();
        parse_avahi(&mut hosts, b"=;eth0;IPv4;My NAS;_smb._tcp;local;nas.local;192.168.1.42;445;\n=;eth0;IPv4;Bad NAS;_smb._tcp;local;user@invalid;192.168.1.3;445;\n=;eth0;IPv4;Printer;_printer._tcp;local;printer.local;192.168.1.5;445;\n");
        assert_eq!(
            hosts,
            vec![NetworkHost {
                label: "My NAS".into(),
                uri: "smb://nas.local/".into(),
                source: "mDNS / Avahi"
            }]
        );
    }

    #[test]
    fn gio_workgroup_and_server_entries_are_separate() {
        let mut hosts = Vec::new();
        let mut aliases = Vec::new();
        let groups = parse_gio(
            &mut hosts,
            b"smb:///WORKGROUP\nsmb://storage.local/Share\nnetwork:///smb-server-nas\n",
            &mut aliases,
        );
        assert_eq!(groups, vec!["smb:///WORKGROUP"]);
        assert_eq!(aliases, vec!["network:///smb-server-nas"]);
        assert_eq!(hosts[0].uri, "smb://storage.local/");
        assert_eq!(hosts.len(), 1);
    }

    #[test]
    fn network_aliases_resolve_to_smb_targets_without_fetching_other_schemes() {
        let mut hosts = Vec::new();
        let mut aliases = Vec::new();
        parse_gio(&mut hosts, b"standard::target-uri=smb://synology.local/\nstandard::target-uri: ftp://not-our-backend/\n", &mut aliases);
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].uri, "smb://synology.local/");
    }

    #[test]
    fn workgroup_children_are_validated_as_servers() {
        let mut hosts = Vec::new();
        parse_gio(
            &mut hosts,
            b"smb:///WORKGROUP/NAS\nsmb:///WORKGROUP/user@bad\n",
            &mut Vec::new(),
        );
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].uri, "smb://NAS/");
    }
}
