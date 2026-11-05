//! Opt-in FileManager1 service. D-Bus handlers only validate and enqueue;
//! filesystem probes and all UI work stay outside the bus executor.

use crate::location::Location;
use eframe::egui;
use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver, SyncSender},
};
use zbus::{blocking::Connection, fdo};

pub const BUS_NAME: &str = "org.freedesktop.FileManager1";
const MAX_LOCATIONS: usize = 32;

#[derive(Debug, PartialEq, Eq)]
pub enum Request {
    Folders(Vec<Location>),
    Items(Vec<PathBuf>),
    Properties(Vec<PathBuf>),
}

fn locations(uris: &[String]) -> fdo::Result<Vec<Location>> {
    if uris.is_empty() || uris.len() > MAX_LOCATIONS {
        return Err(fdo::Error::InvalidArgs("Expected 1–32 URIs".into()));
    }
    uris.iter()
        .map(|uri| {
            if uri.len() > 4096 || !uri.contains("://") {
                return Err(fdo::Error::InvalidArgs("Expected an absolute URI".into()));
            }
            Location::parse(uri.clone().into())
                .map_err(|error| fdo::Error::InvalidArgs(error.into()))
        })
        .collect()
}

fn local_items(uris: &[String]) -> fdo::Result<Vec<PathBuf>> {
    locations(uris)?
        .into_iter()
        .map(|location| match location {
            Location::Local(path) if path.is_absolute() => Ok(path),
            _ => Err(fdo::Error::NotSupported(
                "Item selection and properties require local file URIs".into(),
            )),
        })
        .collect()
}

struct FileManager {
    sender: SyncSender<Request>,
    context: egui::Context,
}

impl FileManager {
    fn send(&self, request: Request) -> fdo::Result<()> {
        self.sender
            .try_send(request)
            .map_err(|_| fdo::Error::LimitsExceeded("RiceExplorer request queue is busy".into()))?;
        self.context.request_repaint();
        Ok(())
    }
}

#[zbus::interface(name = "org.freedesktop.FileManager1")]
impl FileManager {
    fn show_folders(&self, uris: Vec<String>, startup_id: &str) -> fdo::Result<()> {
        let _ = startup_id;
        self.send(Request::Folders(locations(&uris)?))
    }

    fn show_items(&self, uris: Vec<String>, startup_id: &str) -> fdo::Result<()> {
        let _ = startup_id;
        self.send(Request::Items(local_items(&uris)?))
    }

    fn show_item_properties(&self, uris: Vec<String>, startup_id: &str) -> fdo::Result<()> {
        let _ = startup_id;
        self.send(Request::Properties(local_items(&uris)?))
    }
}

pub struct Service {
    pub requests: Receiver<Request>,
    pub ready: Receiver<Result<Connection, String>>,
    // Keep ownership until the Explorer window closes.
    pub connection: Option<Connection>,
}

impl Service {
    pub fn start(context: &egui::Context) -> Self {
        let (sender, requests) = mpsc::sync_channel(16);
        let (ready_sender, ready) = mpsc::channel();
        let context = context.clone();
        std::thread::spawn(move || {
            let result = (|| {
                let connection = zbus::blocking::connection::Builder::session()?
                    .serve_at(
                        "/org/freedesktop/FileManager1",
                        FileManager {
                            sender,
                            context: context.clone(),
                        },
                    )?
                    .build()?;
                connection
                    .request_name_with_flags(BUS_NAME, fdo::RequestNameFlags::DoNotQueue.into())?;
                Ok(connection)
            })();
            let _ = ready_sender.send(result.map_err(|error: zbus::Error| error.to_string()));
            context.request_repaint();
        });
        Self {
            requests,
            ready,
            connection: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_preserve_encoded_paths_and_reject_credentials_and_overload() {
        assert_eq!(
            local_items(&["file:///tmp/a%20b%23c".into()]).unwrap(),
            vec![PathBuf::from("/tmp/a b#c")]
        );
        for uri in [
            "/tmp/file",
            "smb://user:secret@host/share",
            "file:///tmp/%00",
        ] {
            assert!(locations(&[uri.into()]).is_err());
        }
        assert!(local_items(&["smb://server/share/file".into()]).is_err());
        assert!(locations(&[]).is_err());
        assert!(locations(&vec!["file:///tmp".into(); 33]).is_err());
        let (sender, receiver) = mpsc::sync_channel(1);
        let manager = FileManager {
            sender,
            context: egui::Context::default(),
        };
        manager
            .show_folders(vec!["file:///tmp".into()], "")
            .unwrap();
        assert!(
            manager
                .show_folders(vec!["file:///tmp".into()], "")
                .is_err()
        );
        assert!(matches!(receiver.recv().unwrap(), Request::Folders(_)));
    }

    #[test]
    #[ignore = "requires a private dbus-run-session"]
    fn private_bus_roundtrip_and_existing_owner_are_respected() {
        // The dedicated CI smoke starts this under dbus-run-session. Ordinary
        // unit tests must never acquire a name on the developer's desktop bus.
        assert!(std::env::var_os("RICE_EXPLORER_TEST_BUS").is_some());
        let mut service = Service::start(&egui::Context::default());
        let connection = service
            .ready
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap()
            .unwrap();
        let client = Connection::session().unwrap();
        let proxy = zbus::blocking::Proxy::new(
            &client,
            BUS_NAME,
            "/org/freedesktop/FileManager1",
            BUS_NAME,
        )
        .unwrap();
        proxy
            .call::<_, _, ()>("ShowItems", &(vec!["file:///tmp/a%20b"], "test-startup"))
            .unwrap();
        assert_eq!(
            service.requests.recv().unwrap(),
            Request::Items(vec!["/tmp/a b".into()])
        );
        assert!(
            proxy
                .call::<_, _, ()>("ShowItems", &(vec!["smb://server/share"], ""))
                .is_err()
        );
        service.connection = Some(connection);
        let second = Service::start(&egui::Context::default());
        assert!(
            second
                .ready
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap()
                .is_err()
        );
    }
}
