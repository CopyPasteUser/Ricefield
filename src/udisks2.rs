//! Dynamic removable-volume discovery and explicit mounting through UDisks2.
//!
//! UDisks2 owns authorization (Polkit), mount policy and filesystem helpers.
//! RiceField neither mounts as root nor constructs privileged shell commands.
//! An absent system bus/daemon is a supported configuration: the already
//! mounted volumes and GVfs network locations continue to work unchanged.

use std::{
    collections::HashMap,
    ffi::OsStr,
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

use zbus::{
    blocking::{Connection, Proxy},
    fdo::ManagedObjects,
    zvariant::{OwnedValue, Value},
};

const SERVICE: &str = "org.freedesktop.UDisks2";
const ROOT: &str = "/org/freedesktop/UDisks2";
const BLOCK_PREFIX: &str = "/org/freedesktop/UDisks2/block_devices/";
const DRIVE_PREFIX: &str = "/org/freedesktop/UDisks2/drives/";
const BLOCK_INTERFACE: &str = "org.freedesktop.UDisks2.Block";
const FS_INTERFACE: &str = "org.freedesktop.UDisks2.Filesystem";
const ENCRYPTED_INTERFACE: &str = "org.freedesktop.UDisks2.Encrypted";
const DRIVE_INTERFACE: &str = "org.freedesktop.UDisks2.Drive";
const MAX_DEVICE_COUNT: usize = 256;
const CACHE_LIFETIME: Duration = Duration::from_secs(15);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceVolume {
    pub object_path: String,
    pub drive_path: String,
    pub label: String,
    pub device: String,
    pub removable: bool,
    pub ejectable: bool,
    pub can_power_off: bool,
    pub encrypted: bool,
    pub unlocked: bool,
    pub mount_paths: Vec<PathBuf>,
}

struct InventoryCache {
    refreshed: Instant,
    volumes: Vec<DeviceVolume>,
}

static INVENTORY: OnceLock<Mutex<InventoryCache>> = OnceLock::new();

/// Discovery runs only on a background worker. Throttle daemon queries even
/// when a system bus is unavailable, without caching indefinitely after a
/// device is plugged in or removed.
pub fn volumes() -> Vec<DeviceVolume> {
    let cache = INVENTORY.get_or_init(|| {
        Mutex::new(InventoryCache {
            refreshed: Instant::now() - CACHE_LIFETIME,
            volumes: Vec::new(),
        })
    });
    let Ok(mut snapshot) = cache.lock() else {
        return Vec::new();
    };
    if snapshot.refreshed.elapsed() >= CACHE_LIFETIME {
        snapshot.volumes = discover().unwrap_or_default();
        snapshot.refreshed = Instant::now();
    }
    snapshot.volumes.clone()
}

pub fn invalidate_cache() {
    if let Some(cache) = INVENTORY.get()
        && let Ok(mut snapshot) = cache.lock()
    {
        snapshot.refreshed = Instant::now() - CACHE_LIFETIME;
    }
}

fn discover() -> Result<Vec<DeviceVolume>, zbus::Error> {
    Ok(project_volumes(&discover_objects()?))
}

fn discover_objects() -> Result<ManagedObjects, zbus::Error> {
    let connection = Connection::system()?;
    let proxy = Proxy::new(
        &connection,
        SERVICE,
        ROOT,
        "org.freedesktop.DBus.ObjectManager",
    )?;
    proxy.call("GetManagedObjects", &())
}

fn block_path_is_valid(path: &str) -> bool {
    object_path_is_valid(path, BLOCK_PREFIX)
}

fn drive_path_is_valid(path: &str) -> bool {
    object_path_is_valid(path, DRIVE_PREFIX)
}

fn object_path_is_valid(path: &str, prefix: &str) -> bool {
    path.strip_prefix(prefix).is_some_and(|name| {
        !name.is_empty()
            && name.len() <= 128
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    })
}

fn property<T>(values: &HashMap<String, OwnedValue>, key: &str) -> Option<T>
where
    T: TryFrom<OwnedValue>,
{
    T::try_from(values.get(key)?.try_clone().ok()?).ok()
}

fn parse_mount_path(bytes: &[u8]) -> Option<PathBuf> {
    let raw = bytes.strip_suffix(&[0])?;
    if raw.len() > 4096 || !raw.starts_with(b"/") || raw.contains(&0) {
        return None;
    }
    Some(PathBuf::from(OsStr::from_bytes(raw)))
}

/// A decrypted dm block may expose Drive="/". Associate only a mapping
/// explicitly identified by another block's Encrypted.CleartextDevice with
/// that removable parent; never guess by /dev name or label.
fn encrypted_backing_drive(
    objects: &ManagedObjects,
    cleartext_path: &str,
) -> Option<zbus::zvariant::OwnedObjectPath> {
    for (path, interfaces) in objects {
        if !block_path_is_valid(path.as_str()) {
            continue;
        }
        let (Some(block), Some(encrypted)) = (
            interfaces.get(BLOCK_INTERFACE),
            interfaces.get(ENCRYPTED_INTERFACE),
        ) else {
            continue;
        };
        if property::<bool>(block, "HintIgnore").unwrap_or(true)
            || property::<String>(block, "IdUsage").as_deref() != Some("crypto")
            || property::<zbus::zvariant::OwnedObjectPath>(encrypted, "CleartextDevice")
                .as_ref()
                .map(|path| path.as_str())
                != Some(cleartext_path)
        {
            continue;
        }
        if let Some(drive) = property::<zbus::zvariant::OwnedObjectPath>(block, "Drive")
            && drive_path_is_valid(drive.as_str())
        {
            return Some(drive);
        }
    }
    None
}

fn project_volumes(objects: &ManagedObjects) -> Vec<DeviceVolume> {
    if objects.len() > MAX_DEVICE_COUNT {
        return Vec::new();
    }
    let mut result = Vec::new();
    for (path, interfaces) in objects {
        if !block_path_is_valid(path.as_str()) {
            continue;
        }
        let Some(block) = interfaces
            .iter()
            .find(|(name, _)| name.as_str() == BLOCK_INTERFACE)
            .map(|(_, values)| values)
        else {
            continue;
        };
        if property::<bool>(block, "HintIgnore").unwrap_or(true) {
            continue;
        }
        let (encrypted, unlocked, mount_paths) =
            match property::<String>(block, "IdUsage").as_deref() {
                Some("filesystem") => {
                    let Some(filesystem) = interfaces.get(FS_INTERFACE) else {
                        continue;
                    };
                    // Preserve raw Unix mount-point bytes. An invalid byte
                    // array must not hide the fact that a partition is used.
                    let Some(points) = property::<Vec<Vec<u8>>>(filesystem, "MountPoints") else {
                        continue;
                    };
                    let Some(mount_paths) = points
                        .iter()
                        .map(|path| parse_mount_path(path))
                        .collect::<Option<Vec<_>>>()
                    else {
                        continue;
                    };
                    (false, false, mount_paths)
                }
                Some("crypto") => {
                    let Some(encrypted) = interfaces.get(ENCRYPTED_INTERFACE) else {
                        continue;
                    };
                    let Some(cleartext) =
                        property::<zbus::zvariant::OwnedObjectPath>(encrypted, "CleartextDevice")
                    else {
                        continue;
                    };
                    if cleartext.as_str() != "/" && !block_path_is_valid(cleartext.as_str()) {
                        continue;
                    }
                    (true, cleartext.as_str() != "/", Vec::new())
                }
                _ => continue,
            };
        if mount_paths
            .iter()
            .any(|path| path == std::path::Path::new("/"))
        {
            continue;
        }
        let Some(drive_path) = property::<zbus::zvariant::OwnedObjectPath>(block, "Drive")
            .filter(|drive| drive_path_is_valid(drive.as_str()))
            .or_else(|| encrypted_backing_drive(objects, path.as_str()))
        else {
            continue;
        };
        let Some(drive) = objects.get(&drive_path).and_then(|interfaces| {
            interfaces
                .iter()
                .find(|(name, _)| name.as_str() == DRIVE_INTERFACE)
                .map(|(_, values)| values)
        }) else {
            continue;
        };
        let removable = property::<bool>(drive, "Removable").unwrap_or(false);
        let bus = property::<String>(drive, "ConnectionBus").unwrap_or_default();
        let ejectable = property::<bool>(drive, "Ejectable").unwrap_or(false);
        let can_power_off = bus == "usb" && property::<bool>(drive, "CanPowerOff") == Some(true);
        if !removable && !ejectable && bus != "usb" {
            continue;
        }
        let label = property::<String>(block, "IdLabel")
            .filter(|value| {
                !value.is_empty() && value.len() <= 100 && !value.chars().any(char::is_control)
            })
            .unwrap_or_else(|| "Unnamed volume".to_owned());
        let device = property::<Vec<u8>>(block, "PreferredDevice")
            .and_then(|bytes| {
                String::from_utf8(bytes.strip_suffix(&[0]).unwrap_or(&bytes).to_vec()).ok()
            })
            .filter(|device| {
                device.starts_with("/dev/")
                    && device.len() <= 128
                    && !device.chars().any(char::is_control)
            })
            .unwrap_or_else(|| "Device".into());
        result.push(DeviceVolume {
            object_path: path.to_string(),
            drive_path: drive_path.to_string(),
            label,
            device,
            removable,
            ejectable,
            can_power_off,
            encrypted,
            unlocked,
            mount_paths,
        });
    }
    result.sort_unstable_by(|a, b| a.device.cmp(&b.device));
    result
}

fn is_current_unmounted(volumes: &[DeviceVolume], object_path: &str) -> bool {
    volumes.iter().any(|volume| {
        volume.object_path == object_path && !volume.encrypted && volume.mount_paths.is_empty()
    })
}

fn has_encrypted_state(volumes: &[DeviceVolume], object_path: &str, unlocked: bool) -> bool {
    volumes.iter().any(|volume| {
        volume.object_path == object_path && volume.encrypted && volume.unlocked == unlocked
    })
}

fn is_current_mount(volumes: &[DeviceVolume], object_path: &str, expected_mount: &Path) -> bool {
    volumes.iter().any(|volume| {
        volume.object_path == object_path
            && volume
                .mount_paths
                .iter()
                .any(|mount| mount == expected_mount)
    })
}

fn is_current_ejectable(volumes: &[DeviceVolume], drive_path: &str) -> bool {
    volumes
        .iter()
        .any(|volume| volume.drive_path == drive_path && volume.ejectable)
}

/// Power off is deliberately more conservative than eject: the daemon must
/// advertise USB PowerOff and *every* filesystem on the drive must currently
/// be unmounted, including hidden/ignored partitions not shown in the sidebar.
fn can_power_off_safely(objects: &ManagedObjects, drive_path: &str) -> bool {
    if !drive_path_is_valid(drive_path) {
        return false;
    }
    let Some(drive) = objects
        .iter()
        .find(|(path, _)| path.as_str() == drive_path)
        .and_then(|(_, interfaces)| interfaces.get(DRIVE_INTERFACE))
    else {
        return false;
    };
    if property::<bool>(drive, "CanPowerOff") != Some(true)
        || property::<String>(drive, "ConnectionBus").as_deref() != Some("usb")
    {
        return false;
    }

    let mut inspected = false;
    for interfaces in objects.values() {
        let Some(block) = interfaces.get(BLOCK_INTERFACE) else {
            continue;
        };
        let Some(parent) = property::<zbus::zvariant::OwnedObjectPath>(block, "Drive") else {
            // Ignore unrelated blocks with no drive pointer. UDisks2 checks
            // global device usage during PowerOff as a final safety net.
            continue;
        };
        if parent.as_str() != drive_path {
            continue;
        }
        inspected = true;
        if let Some(fs) = interfaces.get(FS_INTERFACE) {
            let Some(mounts) = property::<Vec<Vec<u8>>>(fs, "MountPoints") else {
                return false;
            };
            if !mounts.is_empty() || mounts.iter().any(|path| parse_mount_path(path).is_none()) {
                return false;
            }
        } else if property::<String>(block, "IdUsage").is_some_and(|usage| !usage.is_empty()) {
            // Unknown active uses (swap, crypto mapping, etc.) are not safe
            // candidates for unattended disk power-off.
            return false;
        }
    }
    inspected
}

/// UDisks2 validates mount state, authorization and device access. Only an
/// explicitly selected block-device D-Bus object can be targeted. Mount is
/// never initiated by passive volume discovery.
pub fn mount(object_path: &str) -> Result<(), String> {
    if !block_path_is_valid(object_path) {
        return Err("Invalid UDisks2 block-device identifier".into());
    }
    let current = discover().map_err(|error| format!("Cannot verify volume state: {error}"))?;
    if !is_current_unmounted(&current, object_path) {
        return Err("Volume changed since the last refresh; retry after refreshing".into());
    }
    let connection =
        Connection::system().map_err(|error| format!("System bus unavailable: {error}"))?;
    let proxy = Proxy::new(&connection, SERVICE, object_path, FS_INTERFACE)
        .map_err(|error| format!("Cannot contact removable volume: {error}"))?;
    let options = HashMap::<&str, Value<'_>>::new();
    let _: String = proxy
        .call("Mount", &(options,))
        .map_err(|error| format!("Cannot mount volume: {error}"))?;
    invalidate_cache();
    Ok(())
}

/// Only removable locked crypto blocks are eligible. Never invoke cryptsetup,
/// construct privileged commands or load a passphrase from configuration.
/// The ordinary UDisks2/Polkit path owns unlock policy and mapping creation.
pub fn unlock(object_path: &str, passphrase: &str) -> Result<(), String> {
    if !block_path_is_valid(object_path) {
        return Err("Invalid UDisks2 block-device identifier".into());
    }
    if passphrase.is_empty() || passphrase.len() > 4096 || passphrase.contains('\0') {
        return Err("Enter a valid encrypted-volume passphrase (maximum 4096 bytes)".into());
    }
    let current = discover().map_err(|error| format!("Cannot verify volume state: {error}"))?;
    if !has_encrypted_state(&current, object_path, false) {
        return Err("Encrypted device changed or is already unlocked; refresh volumes".into());
    }
    let connection =
        Connection::system().map_err(|error| format!("System bus unavailable: {error}"))?;
    let proxy = Proxy::new(&connection, SERVICE, object_path, ENCRYPTED_INTERFACE)
        .map_err(|error| format!("Cannot contact encrypted volume: {error}"))?;
    let options = HashMap::<&str, Value<'_>>::new();
    let cleartext: zbus::zvariant::OwnedObjectPath =
        proxy.call("Unlock", &(passphrase, options)).map_err(|_| {
            "Unlock failed. Check the passphrase and Polkit authorization".to_owned()
        })?;
    invalidate_cache();
    if !block_path_is_valid(cleartext.as_str()) {
        return Err("Unlock returned an unexpected mapped-device identifier; inspect volumes before retrying".into());
    }
    Ok(())
}

/// A lock never forcibly unmounts or closes a busy mapping. Refresh the live
/// daemon state and let UDisks2 refuse active users of the cleartext device.
pub fn lock(object_path: &str) -> Result<(), String> {
    if !block_path_is_valid(object_path) {
        return Err("Invalid UDisks2 block-device identifier".into());
    }
    let current = discover().map_err(|error| format!("Cannot verify volume state: {error}"))?;
    if !has_encrypted_state(&current, object_path, true) {
        return Err("Encrypted device changed or is already locked; refresh volumes".into());
    }
    let connection =
        Connection::system().map_err(|error| format!("System bus unavailable: {error}"))?;
    let proxy = Proxy::new(&connection, SERVICE, object_path, ENCRYPTED_INTERFACE)
        .map_err(|error| format!("Cannot contact encrypted volume: {error}"))?;
    let options = HashMap::<&str, Value<'_>>::new();
    let _: () = proxy.call("Lock", &(options,)).map_err(|error| {
        format!("Cannot lock volume: {error}. Unmount all cleartext filesystems first")
    })?;
    invalidate_cache();
    Ok(())
}

/// Ask the UDisks2 filesystem service for a normal (never forced) unmount.
/// Busy volumes stay mounted and the device-level error is shown to the user.
pub fn unmount(object_path: &str, expected_mount: &Path) -> Result<(), String> {
    if !block_path_is_valid(object_path) {
        return Err("Invalid UDisks2 block-device identifier".into());
    }
    if !expected_mount.is_absolute() || expected_mount == Path::new("/") {
        return Err("Invalid unmount destination".into());
    }
    // Refresh the device state before unmounting, not the 15-second UI cache.
    // This detects common unplug/remount changes before any mount mutation.
    let current = discover().map_err(|error| format!("Cannot verify volume state: {error}"))?;
    if !is_current_mount(&current, object_path, expected_mount) {
        return Err("Volume changed since the last refresh; retry after refreshing".into());
    }
    let connection =
        Connection::system().map_err(|error| format!("System bus unavailable: {error}"))?;
    let proxy = Proxy::new(&connection, SERVICE, object_path, FS_INTERFACE)
        .map_err(|error| format!("Cannot contact mounted volume: {error}"))?;
    let options = HashMap::<&str, Value<'_>>::new();
    let _: () = proxy
        .call("Unmount", &(options,))
        .map_err(|error| format!("Cannot unmount volume: {error}"))?;
    invalidate_cache();
    Ok(())
}

/// Ejectable drives are handled by UDisks2/Polkit after an explicit user
/// confirmation. Non-ejectable USB hardware is *not* powered off implicitly.
pub fn eject(drive_path: &str) -> Result<(), String> {
    if !drive_path_is_valid(drive_path) {
        return Err("Invalid UDisks2 drive identifier".into());
    }
    let current = discover().map_err(|error| format!("Cannot verify drive state: {error}"))?;
    if !is_current_ejectable(&current, drive_path) {
        return Err("Drive changed since the last refresh; retry after refreshing".into());
    }
    let connection =
        Connection::system().map_err(|error| format!("System bus unavailable: {error}"))?;
    let proxy = Proxy::new(&connection, SERVICE, drive_path, DRIVE_INTERFACE)
        .map_err(|error| format!("Cannot contact removable drive: {error}"))?;
    let options = HashMap::<&str, Value<'_>>::new();
    let _: () = proxy
        .call("Eject", &(options,))
        .map_err(|error| format!("Cannot eject drive: {error}"))?;
    invalidate_cache();
    Ok(())
}

/// Explicitly request UDisks2/Polkit to flush and power off a removable USB
/// drive only after every known partition has been unmounted. A fresh daemon
/// inventory is consulted rather than trusting the potentially stale sidebar.
pub fn power_off(drive_path: &str) -> Result<(), String> {
    if !drive_path_is_valid(drive_path) {
        return Err("Invalid UDisks2 drive identifier".into());
    }
    let current =
        discover_objects().map_err(|error| format!("Cannot verify drive state: {error}"))?;
    if !can_power_off_safely(&current, drive_path) {
        return Err("USB drive is not ready for power-off: unmount all volumes and refresh".into());
    }
    let connection =
        Connection::system().map_err(|error| format!("System bus unavailable: {error}"))?;
    let proxy = Proxy::new(&connection, SERVICE, drive_path, DRIVE_INTERFACE)
        .map_err(|error| format!("Cannot contact removable USB drive: {error}"))?;
    let options = HashMap::<&str, Value<'_>>::new();
    let _: () = proxy
        .call("PowerOff", &(options,))
        .map_err(|error| format!("Cannot safely power off USB drive: {error}"))?;
    invalidate_cache();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use zbus::zvariant::{OwnedObjectPath, Value};

    fn insert_prop<T: Into<Value<'static>>>(
        props: &mut HashMap<String, OwnedValue>,
        key: &str,
        value: T,
    ) {
        props.insert(key.to_owned(), OwnedValue::try_from(value.into()).unwrap());
    }

    fn removable_fixture() -> ManagedObjects {
        let disk: OwnedObjectPath = "/org/freedesktop/UDisks2/block_devices/sdc1"
            .try_into()
            .unwrap();
        let drive: OwnedObjectPath = "/org/freedesktop/UDisks2/drives/usb_drive"
            .try_into()
            .unwrap();
        let mut block = HashMap::new();
        insert_prop(&mut block, "IdUsage", "filesystem");
        insert_prop(&mut block, "IdLabel", "Rice USB");
        insert_prop(&mut block, "HintIgnore", false);
        insert_prop(&mut block, "Drive", drive.clone());
        insert_prop(
            &mut block,
            "PreferredDevice",
            vec![b'/', b'd', b'e', b'v', b'/', b's', b'd', b'c', b'1', 0],
        );
        let mut fs = HashMap::new();
        insert_prop(&mut fs, "MountPoints", Vec::<Vec<u8>>::new());
        let mut drive_props = HashMap::new();
        insert_prop(&mut drive_props, "Removable", true);
        insert_prop(&mut drive_props, "ConnectionBus", "usb");
        insert_prop(&mut drive_props, "CanPowerOff", true);
        let mut objects = ManagedObjects::new();
        objects.insert(
            disk,
            HashMap::from([
                (BLOCK_INTERFACE.try_into().unwrap(), block),
                (FS_INTERFACE.try_into().unwrap(), fs),
            ]),
        );
        objects.insert(
            drive,
            HashMap::from([(DRIVE_INTERFACE.try_into().unwrap(), drive_props)]),
        );
        objects
    }

    #[test]
    fn rejects_untrusted_object_paths_before_system_bus_access() {
        for path in [
            "",
            "/org/freedesktop/UDisks2/block_devices/",
            "/org/freedesktop/UDisks2/drives/disk",
            "/org/freedesktop/UDisks2/block_devices/disk/evil",
            "/org/freedesktop/UDisks2/block_devices/../drives/evil",
            "/org/freedesktop/UDisks2/block_devices/disk:evil",
        ] {
            assert!(!block_path_is_valid(path), "accepted {path}");
            assert_eq!(
                mount(path).unwrap_err(),
                "Invalid UDisks2 block-device identifier"
            );
            assert_eq!(
                unmount(path, Path::new("/media/test")).unwrap_err(),
                "Invalid UDisks2 block-device identifier"
            );
            assert_eq!(
                unlock(path, "secret").unwrap_err(),
                "Invalid UDisks2 block-device identifier"
            );
            assert_eq!(
                lock(path).unwrap_err(),
                "Invalid UDisks2 block-device identifier"
            );
        }
        assert!(block_path_is_valid(
            "/org/freedesktop/UDisks2/block_devices/sdb1"
        ));
        for path in [
            "/org/freedesktop/UDisks2/drives/",
            "/org/freedesktop/UDisks2/block_devices/sdb1",
            "/org/freedesktop/UDisks2/drives/foo/bar",
        ] {
            assert_eq!(eject(path).unwrap_err(), "Invalid UDisks2 drive identifier");
            assert_eq!(
                power_off(path).unwrap_err(),
                "Invalid UDisks2 drive identifier"
            );
        }
        assert!(drive_path_is_valid("/org/freedesktop/UDisks2/drives/usb_1"));
    }

    #[test]
    fn raw_mount_points_reject_relative_embedded_null_and_unterminated_input() {
        for invalid in [
            b"relative\0".as_slice(),
            b"/media/usb",
            b"/media/usb\0garbage",
            b"\0",
        ] {
            assert!(parse_mount_path(invalid).is_none());
        }
        assert_eq!(
            parse_mount_path(b"/media/usb\xff\0"),
            Some(PathBuf::from(OsStr::from_bytes(b"/media/usb\xff")))
        );
    }

    #[test]
    fn rejects_untrusted_or_incomplete_inventory() {
        let objects = ManagedObjects::new();
        assert!(project_volumes(&objects).is_empty());
        // A D-Bus peer cannot force arbitrary mounts merely by publishing a
        // Filesystem interface without the matching Block and Drive metadata.
        let mut malformed = objects;
        let path = "/org/freedesktop/UDisks2/block_devices/sdc1"
            .try_into()
            .unwrap();
        malformed.insert(path, HashMap::new());
        assert!(project_volumes(&malformed).is_empty());
    }

    #[test]
    fn lists_removable_filesystems_and_preserves_mount_state() {
        let mut objects = removable_fixture();
        assert_eq!(
            project_volumes(&objects),
            vec![DeviceVolume {
                object_path: "/org/freedesktop/UDisks2/block_devices/sdc1".into(),
                drive_path: "/org/freedesktop/UDisks2/drives/usb_drive".into(),
                label: "Rice USB".into(),
                device: "/dev/sdc1".into(),
                removable: true,
                ejectable: false,
                can_power_off: true,
                encrypted: false,
                unlocked: false,
                mount_paths: Vec::new(),
            }]
        );

        let disk: OwnedObjectPath = "/org/freedesktop/UDisks2/block_devices/sdc1"
            .try_into()
            .unwrap();
        insert_prop(
            objects
                .get_mut(&disk)
                .unwrap()
                .get_mut(FS_INTERFACE)
                .unwrap(),
            "MountPoints",
            vec![b"/media/user/USB\0".to_vec()],
        );
        assert_eq!(
            project_volumes(&objects)[0].mount_paths,
            vec![PathBuf::from("/media/user/USB")]
        );
        insert_prop(
            objects
                .get_mut(&disk)
                .unwrap()
                .get_mut(FS_INTERFACE)
                .unwrap(),
            "MountPoints",
            Vec::<Vec<u8>>::new(),
        );
        let props = objects
            .get_mut(&disk)
            .unwrap()
            .get_mut(BLOCK_INTERFACE)
            .unwrap();
        insert_prop(props, "HintIgnore", true);
        assert!(project_volumes(&objects).is_empty());
    }

    #[test]
    fn unsupported_drive_or_invalid_mountpoints_fail_closed() {
        let mut objects = removable_fixture();
        let disk: OwnedObjectPath = "/org/freedesktop/UDisks2/block_devices/sdc1"
            .try_into()
            .unwrap();
        objects
            .get_mut(&disk)
            .unwrap()
            .get_mut(FS_INTERFACE)
            .unwrap()
            .remove("MountPoints");
        assert!(project_volumes(&objects).is_empty());
        insert_prop(
            objects
                .get_mut(&disk)
                .unwrap()
                .get_mut(FS_INTERFACE)
                .unwrap(),
            "MountPoints",
            Vec::<Vec<u8>>::new(),
        );
        let drive: OwnedObjectPath = "/org/freedesktop/UDisks2/drives/usb_drive"
            .try_into()
            .unwrap();
        let drive_props = objects
            .get_mut(&drive)
            .unwrap()
            .get_mut(DRIVE_INTERFACE)
            .unwrap();
        insert_prop(drive_props, "Removable", false);
        insert_prop(drive_props, "ConnectionBus", "sata");
        assert!(project_volumes(&objects).is_empty());
    }

    #[test]
    fn stale_device_operations_fail_closed_against_live_inventory() {
        let mut volumes = project_volumes(&removable_fixture());
        let block = "/org/freedesktop/UDisks2/block_devices/sdc1";
        let drive = "/org/freedesktop/UDisks2/drives/usb_drive";
        assert!(is_current_unmounted(&volumes, block));
        assert!(!is_current_mount(&volumes, block, Path::new("/media/USB")));
        assert!(!is_current_ejectable(&volumes, drive));

        volumes[0].mount_paths.push(PathBuf::from("/media/USB"));
        assert!(!is_current_unmounted(&volumes, block));
        assert!(is_current_mount(&volumes, block, Path::new("/media/USB")));
        assert!(!is_current_mount(
            &volumes,
            block,
            Path::new("/media/other")
        ));
        volumes[0].ejectable = true;
        assert!(is_current_ejectable(&volumes, drive));
        volumes.clear();
        assert!(!is_current_mount(&volumes, block, Path::new("/media/USB")));
        assert!(!is_current_ejectable(&volumes, drive));
    }

    #[test]
    fn encrypted_removable_discovery_only_unlocks_a_verified_locked_block() {
        let mut objects = removable_fixture();
        let disk: OwnedObjectPath = "/org/freedesktop/UDisks2/block_devices/sdc1"
            .try_into()
            .unwrap();
        let block = objects.get_mut(&disk).unwrap();
        block.remove(FS_INTERFACE);
        insert_prop(block.get_mut(BLOCK_INTERFACE).unwrap(), "IdUsage", "crypto");
        let mut encrypted = HashMap::new();
        insert_prop(
            &mut encrypted,
            "CleartextDevice",
            OwnedObjectPath::try_from("/").unwrap(),
        );
        block.insert(ENCRYPTED_INTERFACE.try_into().unwrap(), encrypted);

        let locked = project_volumes(&objects);
        assert_eq!(locked.len(), 1);
        assert!(locked[0].encrypted);
        assert!(!locked[0].unlocked);
        assert!(locked[0].mount_paths.is_empty());
        assert!(!is_current_unmounted(&locked, disk.as_str()));
        assert!(has_encrypted_state(&locked, disk.as_str(), false));
        assert!(!has_encrypted_state(&locked, disk.as_str(), true));

        let encrypted = objects
            .get_mut(&disk)
            .unwrap()
            .get_mut(ENCRYPTED_INTERFACE)
            .unwrap();
        insert_prop(
            encrypted,
            "CleartextDevice",
            OwnedObjectPath::try_from("/org/freedesktop/UDisks2/block_devices/dm_0").unwrap(),
        );
        let unlocked = project_volumes(&objects);
        assert_eq!(unlocked.len(), 1);
        assert!(unlocked[0].unlocked);
        assert!(has_encrypted_state(&unlocked, disk.as_str(), true));
        assert!(!has_encrypted_state(&unlocked, disk.as_str(), false));

        // A device mapper has no direct Drive property. Only the exact
        // CleartextDevice link authorizes inheriting the USB drive metadata.
        let cleartext: OwnedObjectPath = "/org/freedesktop/UDisks2/block_devices/dm_0"
            .try_into()
            .unwrap();
        let mut mapped_block = HashMap::new();
        insert_prop(&mut mapped_block, "IdUsage", "filesystem");
        insert_prop(&mut mapped_block, "HintIgnore", false);
        insert_prop(
            &mut mapped_block,
            "Drive",
            OwnedObjectPath::try_from("/").unwrap(),
        );
        insert_prop(&mut mapped_block, "IdLabel", "Decrypted files");
        insert_prop(
            &mut mapped_block,
            "PreferredDevice",
            b"/dev/mapper/test\0".to_vec(),
        );
        let mut mapped_fs = HashMap::new();
        insert_prop(&mut mapped_fs, "MountPoints", Vec::<Vec<u8>>::new());
        objects.insert(
            cleartext.clone(),
            HashMap::from([
                (BLOCK_INTERFACE.try_into().unwrap(), mapped_block),
                (FS_INTERFACE.try_into().unwrap(), mapped_fs),
            ]),
        );
        let with_mapping = project_volumes(&objects);
        assert_eq!(with_mapping.len(), 2);
        let mapped = with_mapping
            .iter()
            .find(|volume| volume.object_path == cleartext.as_str())
            .unwrap();
        assert!(!mapped.encrypted);
        assert!(is_current_unmounted(&with_mapping, cleartext.as_str()));
        assert_eq!(
            mapped.drive_path,
            "/org/freedesktop/UDisks2/drives/usb_drive"
        );

        // A forged or unrelated mapping must not inherit removable-drive
        // permissions from the encrypted parent.
        insert_prop(
            objects
                .get_mut(&disk)
                .unwrap()
                .get_mut(ENCRYPTED_INTERFACE)
                .unwrap(),
            "CleartextDevice",
            OwnedObjectPath::try_from("/").unwrap(),
        );
        assert!(
            project_volumes(&objects)
                .iter()
                .all(|volume| volume.object_path != cleartext.as_str())
        );

        insert_prop(
            objects
                .get_mut(&disk)
                .unwrap()
                .get_mut(ENCRYPTED_INTERFACE)
                .unwrap(),
            "CleartextDevice",
            OwnedObjectPath::try_from("/org/freedesktop/UDisks2/drives/no").unwrap(),
        );
        assert!(project_volumes(&objects).is_empty());

        objects.get_mut(&disk).unwrap().remove(ENCRYPTED_INTERFACE);
        assert!(project_volumes(&objects).is_empty());
    }

    #[test]
    fn encrypted_passphrase_validation_runs_before_system_bus_access() {
        let path = "/org/freedesktop/UDisks2/block_devices/sdc1";
        for invalid in ["", "abc\0def"] {
            assert!(
                unlock(path, invalid)
                    .unwrap_err()
                    .contains("valid encrypted-volume")
            );
        }
        assert!(
            unlock(path, &"z".repeat(4097))
                .unwrap_err()
                .contains("maximum 4096")
        );
    }

    #[test]
    fn poweroff_requires_usb_capability_and_every_sibling_unmounted() {
        let mut objects = removable_fixture();
        let drive = "/org/freedesktop/UDisks2/drives/usb_drive";
        assert!(can_power_off_safely(&objects, drive));

        let sibling: OwnedObjectPath = "/org/freedesktop/UDisks2/block_devices/sdc2"
            .try_into()
            .unwrap();
        let mut sibling_block = HashMap::new();
        insert_prop(
            &mut sibling_block,
            "Drive",
            OwnedObjectPath::try_from(drive).unwrap(),
        );
        insert_prop(&mut sibling_block, "IdUsage", "filesystem");
        insert_prop(&mut sibling_block, "HintIgnore", true);
        let mut sibling_fs = HashMap::new();
        insert_prop(
            &mut sibling_fs,
            "MountPoints",
            vec![b"/media/secret\0".to_vec()],
        );
        objects.insert(
            sibling.clone(),
            HashMap::from([
                (BLOCK_INTERFACE.try_into().unwrap(), sibling_block),
                (FS_INTERFACE.try_into().unwrap(), sibling_fs),
            ]),
        );
        assert!(!can_power_off_safely(&objects, drive));

        insert_prop(
            objects
                .get_mut(&sibling)
                .unwrap()
                .get_mut(FS_INTERFACE)
                .unwrap(),
            "MountPoints",
            Vec::<Vec<u8>>::new(),
        );
        assert!(can_power_off_safely(&objects, drive));
        objects
            .get_mut(&sibling)
            .unwrap()
            .get_mut(FS_INTERFACE)
            .unwrap()
            .remove("MountPoints");
        assert!(!can_power_off_safely(&objects, drive));

        let root_drive: OwnedObjectPath = drive.try_into().unwrap();
        insert_prop(
            objects
                .get_mut(&root_drive)
                .unwrap()
                .get_mut(DRIVE_INTERFACE)
                .unwrap(),
            "ConnectionBus",
            "sata",
        );
        assert!(!can_power_off_safely(&objects, drive));
        assert!(!can_power_off_safely(
            &objects,
            "/org/freedesktop/UDisks2/drives/../evil"
        ));
    }
}
