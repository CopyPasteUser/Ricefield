//! Dynamic removable-volume discovery and explicit mounting through UDisks2.
//!
//! UDisks2 owns authorization (Polkit), mount policy and filesystem helpers.
//! RiceField neither mounts as root nor constructs privileged shell commands.
//! An absent system bus/daemon is a supported configuration: the already
//! mounted volumes and GVfs network locations continue to work unchanged.

use std::{
    collections::HashMap,
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
const BLOCK_INTERFACE: &str = "org.freedesktop.UDisks2.Block";
const FS_INTERFACE: &str = "org.freedesktop.UDisks2.Filesystem";
const DRIVE_INTERFACE: &str = "org.freedesktop.UDisks2.Drive";
const MAX_DEVICE_COUNT: usize = 256;
const CACHE_LIFETIME: Duration = Duration::from_secs(15);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnmountedVolume {
    pub object_path: String,
    pub label: String,
    pub device: String,
    pub removable: bool,
}

struct InventoryCache {
    refreshed: Instant,
    volumes: Vec<UnmountedVolume>,
}

static INVENTORY: OnceLock<Mutex<InventoryCache>> = OnceLock::new();

/// Discovery runs only on a background worker. Throttle daemon queries even
/// when a system bus is unavailable, without caching indefinitely after a
/// device is plugged in or removed.
pub fn unmounted_volumes() -> Vec<UnmountedVolume> {
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

fn discover() -> Result<Vec<UnmountedVolume>, zbus::Error> {
    let connection = Connection::system()?;
    let proxy = Proxy::new(
        &connection,
        SERVICE,
        ROOT,
        "org.freedesktop.DBus.ObjectManager",
    )?;
    let objects: ManagedObjects = proxy.call("GetManagedObjects", &())?;
    Ok(project_unmounted(&objects))
}

fn block_path_is_valid(path: &str) -> bool {
    path.strip_prefix(BLOCK_PREFIX).is_some_and(|name| {
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

fn project_unmounted(objects: &ManagedObjects) -> Vec<UnmountedVolume> {
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
        // Do not offer a second mount button for an already-mounted partition.
        // A failed-to-decode property fails closed rather than misrepresenting
        // a mounted filesystem as an unmounted target.
        let Some(points) = property::<Vec<Vec<u8>>>(filesystem, "MountPoints") else {
            continue;
        };
        if !points.is_empty() {
            continue;
        }
        let Some(drive_path) = property::<zbus::zvariant::OwnedObjectPath>(block, "Drive") else {
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
        result.push(UnmountedVolume {
            object_path: path.to_string(),
            label,
            device,
            removable,
        });
    }
    result.sort_unstable_by(|a, b| a.device.cmp(&b.device));
    result
}

/// UDisks2 validates mount state, authorization and device access. Only an
/// explicitly selected block-device D-Bus object can be targeted. Mount is
/// never initiated by passive volume discovery.
pub fn mount(object_path: &str) -> Result<(), String> {
    if !block_path_is_valid(object_path) {
        return Err("Invalid UDisks2 block-device identifier".into());
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
        }
        assert!(block_path_is_valid(
            "/org/freedesktop/UDisks2/block_devices/sdb1"
        ));
    }

    #[test]
    fn rejects_untrusted_or_incomplete_inventory() {
        let objects = ManagedObjects::new();
        assert!(project_unmounted(&objects).is_empty());
        // A D-Bus peer cannot force arbitrary mounts merely by publishing a
        // Filesystem interface without the matching Block and Drive metadata.
        let mut malformed = objects;
        let path = "/org/freedesktop/UDisks2/block_devices/sdc1"
            .try_into()
            .unwrap();
        malformed.insert(path, HashMap::new());
        assert!(project_unmounted(&malformed).is_empty());
    }

    #[test]
    fn lists_only_unmounted_removable_filesystems() {
        let mut objects = removable_fixture();
        assert_eq!(
            project_unmounted(&objects),
            vec![UnmountedVolume {
                object_path: "/org/freedesktop/UDisks2/block_devices/sdc1".into(),
                label: "Rice USB".into(),
                device: "/dev/sdc1".into(),
                removable: true,
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
        assert!(project_unmounted(&objects).is_empty());
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
        assert!(project_unmounted(&objects).is_empty());
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
        assert!(project_unmounted(&objects).is_empty());
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
        assert!(project_unmounted(&objects).is_empty());
    }
}
