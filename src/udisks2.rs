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
    let connection = Connection::system()?;
    let proxy = Proxy::new(
        &connection,
        SERVICE,
        ROOT,
        "org.freedesktop.DBus.ObjectManager",
    )?;
    let objects: ManagedObjects = proxy.call("GetManagedObjects", &())?;
    Ok(project_volumes(&objects))
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
        let Some(filesystem) = interfaces
            .iter()
            .find(|(name, _)| name.as_str() == FS_INTERFACE)
            .map(|(_, values)| values)
        else {
            continue;
        };
        if property::<bool>(block, "HintIgnore").unwrap_or(true)
            || property::<String>(block, "IdUsage").as_deref() != Some("filesystem")
        {
            continue;
        }
        // Preserve raw Unix mount-point bytes. An invalid byte-array must not
        // cause a mounted partition to be displayed as an unmounted target.
        let Some(points) = property::<Vec<Vec<u8>>>(filesystem, "MountPoints") else {
            continue;
        };
        let mount_paths: Option<Vec<_>> = points.iter().map(|p| parse_mount_path(p)).collect();
        let Some(mount_paths) = mount_paths else {
            continue;
        };
        if mount_paths
            .iter()
            .any(|path| path == std::path::Path::new("/"))
        {
            continue;
        }
        let Some(drive_path) = property::<zbus::zvariant::OwnedObjectPath>(block, "Drive") else {
            continue;
        };
        if !drive_path_is_valid(drive_path.as_str()) {
            continue;
        }
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
            mount_paths,
        });
    }
    result.sort_unstable_by(|a, b| a.device.cmp(&b.device));
    result
}

fn is_current_unmounted(volumes: &[DeviceVolume], object_path: &str) -> bool {
    volumes
        .iter()
        .any(|volume| volume.object_path == object_path && volume.mount_paths.is_empty())
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
}
