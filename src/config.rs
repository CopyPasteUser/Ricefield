use crate::location::Location;
use serde::{Deserialize, Serialize};
use std::{
    env, fs,
    fs::OpenOptions,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, SyncSender, TrySendError},
    },
    thread,
    time::Duration,
    time::{SystemTime, UNIX_EPOCH},
};

/// Coalesce preference changes without doing filesystem I/O on the GUI thread.
/// One writer owns the output sequence, so an older snapshot cannot publish
/// after a newer one. The pending slot holds at most one complete snapshot.
pub struct SaveWorker {
    pending: Arc<Mutex<Option<Config>>>,
    wakeup: Option<SyncSender<()>>,
    errors: Receiver<String>,
    finished: Receiver<()>,
}

impl SaveWorker {
    pub fn spawn() -> Self {
        Self::spawn_with_write(|config| config.save())
    }

    fn spawn_with_write(write: impl Fn(Config) -> io::Result<()> + Send + 'static) -> Self {
        let pending = Arc::new(Mutex::new(None));
        let worker_pending = Arc::clone(&pending);
        let (wakeup, requests) = mpsc::sync_channel(1);
        let (error_sender, errors) = mpsc::channel();
        let (finished_sender, finished) = mpsc::channel();
        thread::spawn(move || {
            while requests.recv().is_ok() {
                let config = worker_pending.lock().ok().and_then(|mut slot| slot.take());
                if let Some(config) = config
                    && let Err(error) = write(config)
                {
                    let _ = error_sender.send(error.to_string());
                }
            }
            let _ = finished_sender.send(());
        });
        Self {
            pending,
            wakeup: Some(wakeup),
            errors,
            finished,
        }
    }

    pub fn request(&self, config: &Config) -> Result<(), String> {
        let mut slot = self.pending.lock().map_err(|error| error.to_string())?;
        *slot = Some(config.clone());
        drop(slot);
        let Some(wakeup) = &self.wakeup else {
            return Err("Preference writer has stopped".into());
        };
        match wakeup.try_send(()) {
            Ok(()) | Err(TrySendError::Full(())) => Ok(()),
            Err(TrySendError::Disconnected(())) => Err("Preference writer disconnected".into()),
        }
    }

    pub fn next_error(&self) -> Option<String> {
        self.errors.try_recv().ok()
    }

    /// Give the final snapshot a bounded chance to finish on normal exit.
    /// A stalled config mount must not indefinitely block app shutdown.
    pub fn finish(&mut self) {
        self.wakeup.take();
        let _ = self.finished.recv_timeout(Duration::from_secs(2));
    }
}

pub const MAX_SAVED_TABS: usize = 32;
pub const MAX_SAVED_HISTORY: usize = 40;
pub const MAX_SAVED_CLOSED_TABS: usize = 16;
pub const MAX_SAVED_SEARCHES: usize = 16;
pub type SessionHistory = (Vec<Location>, Vec<Location>);
pub type SessionNavigation = (Location, Vec<Location>, Vec<Location>);

#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct StoredHistory {
    pub back: Vec<String>,
    pub forward: Vec<String>,
}

#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct StoredClosedTab {
    pub location: String,
    pub history: StoredHistory,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedSearch {
    pub folder: String,
    pub query: String,
    pub subfolders: bool,
    pub contents: bool,
}

impl SavedSearch {
    fn valid(&self) -> bool {
        !self.query.trim().is_empty()
            && self.query.len() <= 120
            && self.folder.len() <= 4096
            && Path::new(&self.folder).is_absolute()
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub theme: String,
    pub custom_theme_file: Option<PathBuf>,
    pub show_hidden: bool,
    pub show_sidebar: bool,
    pub view: String,
    pub sort_by: String,
    pub descending: bool,
    pub filter_kind: String,
    pub filter_size: String,
    pub filter_modified: String,
    pub filter_name_glob: String,
    pub filter_name_regex: String,
    pub filter_min_size_mib: Option<u64>,
    pub filter_max_size_mib: Option<u64>,
    pub filter_newer_days: Option<u64>,
    pub filter_older_days: Option<u64>,
    pub saved_searches: Vec<SavedSearch>,
    pub bookmarks: Vec<PathBuf>,
    pub remote_favorites: Vec<String>,
    pub remote_download_directory: Option<PathBuf>,
    pub restore_session: bool,
    // Local absolute paths and validated network URIs share this legacy TOML
    // string field, so existing saved sessions remain readable.
    pub session_tabs: Vec<String>,
    pub session_active: usize,
    pub session_histories: Vec<StoredHistory>,
    pub session_closed: Vec<StoredClosedTab>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            theme: "win7".to_owned(),
            custom_theme_file: None,
            show_hidden: false,
            show_sidebar: true,
            view: "list".to_owned(),
            sort_by: "name".to_owned(),
            descending: false,
            filter_kind: "all".to_owned(),
            filter_size: "any".to_owned(),
            filter_modified: "any".to_owned(),
            filter_name_glob: String::new(),
            filter_name_regex: String::new(),
            filter_min_size_mib: None,
            filter_max_size_mib: None,
            filter_newer_days: None,
            filter_older_days: None,
            saved_searches: Vec::new(),
            bookmarks: Vec::new(),
            remote_favorites: Vec::new(),
            remote_download_directory: None,
            restore_session: true,
            session_tabs: Vec::new(),
            session_active: 0,
            session_histories: Vec::new(),
            session_closed: Vec::new(),
        }
    }
}

pub fn home() -> PathBuf {
    env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from("/")))
}

/// XDG user-dirs may be localized or moved (e.g. ~/Dokumente instead of
/// ~/Documents). Parse only literal absolute paths and $HOME expansions:
/// user-dirs.dirs is configuration data, not a shell script to execute.
fn xdg_user_dir(home: &Path, config: &str, key: &str) -> Option<PathBuf> {
    let value = config.lines().find_map(|line| {
        let (name, value) = line.trim().split_once('=')?;
        (name.trim() == key).then_some(value.trim())
    })?;
    let value = value.strip_prefix('"')?.strip_suffix('"')?;
    let path = if value == "$HOME" {
        home.to_path_buf()
    } else if let Some(relative) = value.strip_prefix("$HOME/") {
        home.join(relative)
    } else if value.starts_with('/') {
        PathBuf::from(value)
    } else {
        return None;
    };
    // Reject unexpanded shell syntax rather than ever interpreting it.
    (!value.contains(['`', '\\', '"']) && !value.trim_start_matches("$HOME").contains('$'))
        .then_some(path)
}

fn sidebar_places_from_config(home: &Path, user_dirs: &str) -> Vec<(&'static str, PathBuf)> {
    let mut places = vec![("Home", home.to_path_buf())];
    for (label, key, fallback) in [
        ("Desktop", "XDG_DESKTOP_DIR", "Desktop"),
        ("Documents", "XDG_DOCUMENTS_DIR", "Documents"),
        ("Downloads", "XDG_DOWNLOAD_DIR", "Downloads"),
        ("Pictures", "XDG_PICTURES_DIR", "Pictures"),
        ("Music", "XDG_MUSIC_DIR", "Music"),
        ("Videos", "XDG_VIDEOS_DIR", "Videos"),
    ] {
        let configured = xdg_user_dir(home, user_dirs, key);
        let path = configured
            .filter(|path| path.is_dir())
            .unwrap_or_else(|| home.join(fallback));
        if path.is_dir() && !places.iter().any(|(_, existing)| existing == &path) {
            places.push((label, path));
        }
    }
    places.push(("Filesystem", PathBuf::from("/")));
    places
}

/// Called by the existing background inventory worker, never by UI painting.
pub fn sidebar_places() -> Vec<(&'static str, PathBuf)> {
    let home = home();
    let config_dir = env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".config"));
    let contents = fs::read_to_string(config_dir.join("user-dirs.dirs")).unwrap_or_default();
    sidebar_places_from_config(&home, &contents)
}

pub fn config_file() -> PathBuf {
    let root = env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".config"));
    root.join("ricefield").join("config.toml")
}

impl Config {
    /// Only deliberately saved local searches survive restart. Never check
    /// mount availability while restoring config or save transient queries.
    pub fn saved_searches(&self) -> Vec<SavedSearch> {
        self.saved_searches
            .iter()
            .filter(|search| search.valid())
            .take(MAX_SAVED_SEARCHES)
            .cloned()
            .collect()
    }

    pub fn save_search(&mut self, search: SavedSearch) -> bool {
        if !search.valid() || self.saved_searches.contains(&search) {
            return false;
        }
        self.saved_searches.retain(SavedSearch::valid);
        if self.saved_searches.len() >= MAX_SAVED_SEARCHES {
            self.saved_searches.remove(0);
        }
        self.saved_searches.push(search);
        true
    }

    pub fn active_filter_count(&self) -> usize {
        usize::from(self.filter_kind != "all")
            + usize::from(self.filter_size != "any")
            + usize::from(self.filter_modified != "any")
            + usize::from(!self.filter_name_glob.is_empty())
            + usize::from(!self.filter_name_regex.is_empty())
            + usize::from(self.filter_min_size_mib.is_some())
            + usize::from(self.filter_max_size_mib.is_some())
            + usize::from(self.filter_newer_days.is_some())
            + usize::from(self.filter_older_days.is_some())
    }

    pub fn clear_filters(&mut self) {
        self.filter_kind = "all".into();
        self.filter_size = "any".into();
        self.filter_modified = "any".into();
        self.filter_name_glob.clear();
        self.filter_name_regex.clear();
        self.filter_min_size_mib = None;
        self.filter_max_size_mib = None;
        self.filter_newer_days = None;
        self.filter_older_days = None;
    }

    /// Restore only validated local paths and network URIs. Never stat paths or
    /// contact remote servers here: unavailable mounts must not block startup.
    pub fn restored_session(&self) -> (Vec<Location>, usize) {
        if !self.restore_session {
            return (Vec::new(), 0);
        }
        let mut locations = Vec::new();
        let mut active = 0;
        for (index, text) in self.session_tabs.iter().enumerate() {
            if locations.len() >= MAX_SAVED_TABS {
                break;
            }
            let Some(location) = validated_session_location(text) else {
                continue;
            };
            if index == self.session_active {
                active = locations.len();
            }
            locations.push(location);
        }
        (locations, active)
    }

    /// A private user setting, never a background replay of file mutations.
    /// Non-UTF-8 paths remain navigable but are excluded from TOML snapshots.
    pub fn remember_session(&mut self, locations: &[Location], active_index: usize) -> bool {
        let (remembered, active) = if self.restore_session {
            let mut remembered = Vec::new();
            let mut active = 0;
            for (index, location) in locations.iter().enumerate() {
                if remembered.len() >= MAX_SAVED_TABS {
                    break;
                }
                let text = match location {
                    Location::Local(path) => path.to_str().map(str::to_owned),
                    Location::Remote(remote) => Some(remote.uri.clone()),
                };
                let Some(text) = text.filter(|text| validated_session_location(text).is_some())
                else {
                    continue;
                };
                if index == active_index {
                    active = remembered.len();
                }
                remembered.push(text);
            }
            (remembered, active)
        } else {
            (Vec::new(), 0)
        };
        if remembered == self.session_tabs && active == self.session_active {
            return false;
        }
        self.session_tabs = remembered;
        self.session_active = active;
        true
    }

    /// Navigation is a bounded location-only session snapshot, never a replay
    /// of file operations, searches, credentials or clipboard contents.
    pub fn remember_navigation(
        &mut self,
        tabs: &[SessionNavigation],
        closed: &[SessionNavigation],
    ) -> bool {
        let (histories, closed_tabs) = if self.restore_session {
            let histories = tabs
                .iter()
                .filter(|(location, _, _)| session_location_text(location).is_some())
                .take(MAX_SAVED_TABS)
                .map(|(_, back, forward)| StoredHistory {
                    back: encode_history(back),
                    forward: encode_history(forward),
                })
                .collect();
            let closed_tabs = closed
                .iter()
                .filter_map(|(location, back, forward)| {
                    Some(StoredClosedTab {
                        location: session_location_text(location)?,
                        history: StoredHistory {
                            back: encode_history(back),
                            forward: encode_history(forward),
                        },
                    })
                })
                .rev()
                .take(MAX_SAVED_CLOSED_TABS)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            (histories, closed_tabs)
        } else {
            (Vec::new(), Vec::new())
        };
        if histories == self.session_histories && closed_tabs == self.session_closed {
            return false;
        }
        self.session_histories = histories;
        self.session_closed = closed_tabs;
        true
    }

    /// Maps saved history to *validated* restored tabs, even if a corrupted or
    /// legacy config contains invalid session locations between valid ones.
    pub fn restored_navigation(&self) -> (Vec<SessionHistory>, Vec<SessionNavigation>) {
        if !self.restore_session {
            return (Vec::new(), Vec::new());
        }
        let histories = self
            .session_tabs
            .iter()
            .enumerate()
            .filter(|(_, text)| validated_session_location(text).is_some())
            .take(MAX_SAVED_TABS)
            .map(|(index, _)| {
                let saved = self
                    .session_histories
                    .get(index)
                    .cloned()
                    .unwrap_or_default();
                (decode_history(&saved.back), decode_history(&saved.forward))
            })
            .collect();
        let closed = self
            .session_closed
            .iter()
            .rev()
            .filter_map(|saved| {
                Some((
                    validated_session_location(&saved.location)?,
                    decode_history(&saved.history.back),
                    decode_history(&saved.history.forward),
                ))
            })
            .take(MAX_SAVED_CLOSED_TABS)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        (histories, closed)
    }

    pub fn load() -> Self {
        fs::read_to_string(config_file())
            .ok()
            .and_then(|contents| toml::from_str(&contents).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> io::Result<()> {
        let file = config_file();
        let encoded = toml::to_string_pretty(self).map_err(io::Error::other)?;
        write_private_config(&file, encoded.as_bytes())
    }
}

/// Keep the saved UI configuration (including deliberately saved search terms)
/// private and prevent a predictable `.tmp` symlink from replacing another file.
fn write_private_config(file: &Path, contents: &[u8]) -> io::Result<()> {
    if let Some(parent) = file.parent() {
        fs::create_dir_all(parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if fs::symlink_metadata(parent)?.file_type().is_symlink() {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "RiceField config directory must not be a symlink",
                ));
            }
            fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
        }
    }
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    for attempt in 0..8 {
        let temporary =
            file.with_extension(format!("toml.tmp-{}-{nonce}-{attempt}", std::process::id()));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut handle = match options.open(&temporary) {
            Ok(handle) => handle,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        };
        let result = handle
            .write_all(contents)
            .and_then(|()| handle.flush())
            .and_then(|()| fs::rename(&temporary, file));
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        return result;
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "Could not reserve a private config staging file",
    ))
}

fn validated_session_location(text: &str) -> Option<Location> {
    if text.len() > 4096 {
        return None;
    }
    match Location::parse(text.into()).ok()? {
        Location::Local(path) if path.is_absolute() => Some(Location::Local(path)),
        Location::Remote(remote) => Some(Location::Remote(remote)),
        _ => None,
    }
}

fn session_location_text(location: &Location) -> Option<String> {
    let text = match location {
        Location::Local(path) => path.to_str()?.to_owned(),
        Location::Remote(remote) => remote.uri.clone(),
    };
    validated_session_location(&text).map(|_| text)
}

fn encode_history(history: &[Location]) -> Vec<String> {
    history
        .iter()
        .rev()
        .filter_map(session_location_text)
        .take(MAX_SAVED_HISTORY)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

fn decode_history(history: &[String]) -> Vec<Location> {
    history
        .iter()
        .rev()
        .filter_map(|text| validated_session_location(text))
        .take(MAX_SAVED_HISTORY)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_places_follow_localized_xdg_user_directories() {
        let home = std::env::temp_dir().join(format!(
            "ricefield-places-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(home.join("Schreibtisch")).unwrap();
        fs::create_dir_all(home.join("Dokumente")).unwrap();
        fs::create_dir_all(home.join("Downloads")).unwrap();
        let configured = concat!(
            "XDG_DESKTOP_DIR=\"$HOME/Schreibtisch\"\n",
            "XDG_DOCUMENTS_DIR=\"$HOME/Dokumente\"\n",
            "XDG_DOWNLOAD_DIR=\"$HOME/Downloads\"\n",
            "XDG_MUSIC_DIR=\"$HOME\"\n",
            "XDG_PICTURES_DIR=\"$(invalid command)\"\n",
        );
        let places = sidebar_places_from_config(&home, configured);
        assert_eq!(places[0], ("Home", home.clone()));
        assert!(places.contains(&("Desktop", home.join("Schreibtisch"))));
        assert!(places.contains(&("Documents", home.join("Dokumente"))));
        assert!(places.contains(&("Downloads", home.join("Downloads"))));
        assert!(
            sidebar_places_from_config(&home, "").contains(&("Downloads", home.join("Downloads")))
        );
        assert!(!places.iter().any(|(name, _)| *name == "Music"));
        assert!(!places.iter().any(|(name, _)| *name == "Pictures"));
        assert_eq!(places.last().unwrap(), &("Filesystem", PathBuf::from("/")));
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn background_preference_writer_is_bounded_and_keeps_the_latest_snapshot() {
        let (entered_sender, entered_receiver) = mpsc::channel();
        let (release_sender, release_receiver) = mpsc::channel();
        let (saved_sender, saved_receiver) = mpsc::channel();
        let mut writer = SaveWorker::spawn_with_write(move |config| {
            if config.theme == "first" {
                entered_sender.send(()).unwrap();
                release_receiver.recv().unwrap();
            }
            saved_sender.send(config.theme).unwrap();
            Ok(())
        });
        writer
            .request(&Config {
                theme: "first".into(),
                ..Config::default()
            })
            .unwrap();
        entered_receiver
            .recv_timeout(Duration::from_secs(2))
            .unwrap();
        // These requests complete immediately even while the writer is stalled.
        // They replace one pending slot instead of creating an unbounded queue.
        for n in 0..100 {
            writer
                .request(&Config {
                    theme: format!("revision-{n}"),
                    ..Config::default()
                })
                .unwrap();
        }
        release_sender.send(()).unwrap();
        writer.finish();
        assert_eq!(
            saved_receiver.try_iter().collect::<Vec<_>>(),
            ["first", "revision-99"]
        );
    }

    #[test]
    fn background_preference_writer_reports_storage_failures() {
        let mut writer =
            SaveWorker::spawn_with_write(|_| Err(io::Error::other("disk unavailable")));
        writer.request(&Config::default()).unwrap();
        writer.finish();
        assert_eq!(writer.next_error().as_deref(), Some("disk unavailable"));
    }

    #[test]
    fn older_preferences_default_to_session_restore_without_requiring_new_keys() {
        let config: Config = toml::from_str("theme = 'win7'\nshow_hidden = true\n").unwrap();
        assert!(config.restore_session);
        assert!(config.session_tabs.is_empty());
        assert!(config.show_hidden);
        assert!(config.remote_favorites.is_empty());
        assert!(config.remote_download_directory.is_none());
        assert_eq!(config.active_filter_count(), 0);
    }

    #[test]
    fn configured_remote_download_folder_survives_restart() {
        let config = Config {
            remote_download_directory: Some("/mnt/usb/My Downloads".into()),
            ..Config::default()
        };
        let saved = toml::to_string(&config).unwrap();
        let loaded: Config = toml::from_str(&saved).unwrap();
        assert_eq!(
            loaded.remote_download_directory,
            config.remote_download_directory
        );
    }

    #[test]
    fn custom_filters_roundtrip_and_clear_without_losing_other_preferences() {
        let config = Config {
            theme: "xp-luna".into(),
            filter_kind: "documents".into(),
            filter_name_glob: "draft-*.pdf".into(),
            filter_name_regex: "^draft-[0-9]+".into(),
            filter_min_size_mib: Some(2),
            filter_max_size_mib: Some(7),
            filter_older_days: Some(14),
            ..Config::default()
        };
        assert_eq!(config.active_filter_count(), 6);
        let serialized = toml::to_string(&config).unwrap();
        let mut loaded: Config = toml::from_str(&serialized).unwrap();
        assert_eq!(loaded.filter_name_glob, "draft-*.pdf");
        assert_eq!(loaded.filter_name_regex, "^draft-[0-9]+");
        assert_eq!(loaded.filter_max_size_mib, Some(7));
        loaded.clear_filters();
        assert_eq!(loaded.active_filter_count(), 0);
        assert_eq!(loaded.theme, "xp-luna");
    }

    #[test]
    fn saved_searches_are_opt_in_local_bounded_and_roundtrip() {
        let mut config = Config::default();
        assert!(config.saved_searches().is_empty());
        let search = SavedSearch {
            folder: "/home/test/Documents".into(),
            query: "invoices".into(),
            subfolders: true,
            contents: false,
        };
        assert!(config.save_search(search.clone()));
        assert!(!config.save_search(search.clone()));
        assert!(!config.save_search(SavedSearch {
            folder: "smb://private/documents".into(),
            ..search.clone()
        }));
        for index in 0..(MAX_SAVED_SEARCHES + 4) {
            assert!(config.save_search(SavedSearch {
                query: format!("keyword{index}"),
                ..search.clone()
            }));
        }
        assert_eq!(config.saved_searches().len(), MAX_SAVED_SEARCHES);
        let decoded: Config = toml::from_str(&toml::to_string(&config).unwrap()).unwrap();
        assert_eq!(decoded.saved_searches(), config.saved_searches());
        assert!(
            decoded
                .saved_searches()
                .last()
                .unwrap()
                .query
                .starts_with("keyword")
        );
    }

    #[test]
    fn config_save_is_atomic_private_and_ignores_legacy_predictable_tmp() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = env::temp_dir().join(format!(
            "ricefield-config-test-{}-{unique}",
            std::process::id()
        ));
        let file = root.join("ricefield").join("config.toml");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        let decoy = file.with_extension("toml.tmp");
        fs::write(&decoy, b"unrelated").unwrap();
        write_private_config(&file, b"old").unwrap();
        write_private_config(&file, b"new").unwrap();
        assert_eq!(fs::read(&file).unwrap(), b"new");
        assert_eq!(fs::read(&decoy).unwrap(), b"unrelated");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(&file).unwrap().permissions().mode() & 0o077, 0);
            assert_eq!(
                fs::metadata(file.parent().unwrap())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o077,
                0
            );
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn session_snapshots_preserve_tab_order_and_selected_index() {
        let mut config = Config::default();
        let paths = vec![
            Location::Local("/one".into()),
            Location::Local("/two".into()),
        ];
        assert!(config.remember_session(&paths, 1));
        assert!(!config.remember_session(&paths, 1));
        let encoded = toml::to_string(&config).unwrap();
        let reloaded: Config = toml::from_str(&encoded).unwrap();
        assert_eq!(reloaded.restored_session(), (paths, 1));
    }

    #[test]
    fn session_restore_is_bounded_and_never_checks_filesystem_state() {
        let mut config = Config::default();
        let mut paths = vec![
            Location::Local("relative".into()),
            Location::Local("/offline/share".into()),
        ];
        paths.extend((0..40).map(|index| Location::Local(format!("/folder-{index}").into())));
        assert!(config.remember_session(&paths, 1));
        let (restored, active) = config.restored_session();
        assert_eq!(restored.len(), MAX_SAVED_TABS);
        assert_eq!(restored[0], Location::Local("/offline/share".into()));
        assert_eq!(active, 0);
    }

    #[test]
    fn opting_out_discards_saved_tab_paths() {
        let mut config = Config::default();
        config.remember_session(&[Location::Local("/private".into())], 0);
        config.restore_session = false;
        assert!(config.remember_session(&[Location::Local("/another".into())], 0));
        assert!(config.session_tabs.is_empty());
        assert_eq!(config.restored_session(), (Vec::new(), 0));
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_tab_path_does_not_break_preference_serialization() {
        use std::os::unix::ffi::OsStringExt;
        let mut config = Config::default();
        let raw = PathBuf::from(std::ffi::OsString::from_vec(b"/dir-\xff".to_vec()));
        assert!(config.remember_session(
            &[Location::Local(raw), Location::Local("/visible".into())],
            0
        ));
        assert_eq!(
            config.restored_session(),
            (vec![Location::Local("/visible".into())], 0)
        );
        assert!(toml::to_string(&config).is_ok());
    }

    #[test]
    fn restores_remote_tabs_without_persisting_unsafe_uris() {
        let remote = Location::parse("smb://nas/share/Projects".into()).unwrap();
        let locations = vec![Location::Local("/home/user".into()), remote.clone()];
        let mut config = Config::default();
        assert!(config.remember_session(&locations, 1));
        let decoded: Config = toml::from_str(&toml::to_string(&config).unwrap()).unwrap();
        assert_eq!(decoded.restored_session(), (locations, 1));

        config.session_tabs = vec![
            "sftp://user:password@server/path".into(),
            "relative/path".into(),
            "smb://nas/share".into(),
        ];
        config.session_active = 2;
        assert_eq!(
            config.restored_session(),
            (vec![Location::parse("smb://nas/share".into()).unwrap()], 0)
        );

        // Existing configs serialized local PathBuf values as TOML strings.
        let legacy: Config =
            toml::from_str("session_tabs = ['/old/home']\nsession_active = 0").unwrap();
        assert_eq!(
            legacy.restored_session().0,
            vec![Location::Local("/old/home".into())]
        );
    }

    #[test]
    fn restores_bounded_navigation_and_closed_tabs_across_restart() {
        let mut config = Config::default();
        let tabs = vec![
            (
                Location::Local("/one".into()),
                vec![Location::Local("/before".into())],
                vec![],
            ),
            (
                Location::Local("/two".into()),
                vec![],
                vec![Location::Local("/next".into())],
            ),
        ];
        let closed = vec![(
            Location::Local("/closed".into()),
            (0..50)
                .map(|n| Location::Local(format!("/history/{n}").into()))
                .collect(),
            vec![],
        )];
        config.remember_session(&tabs.iter().map(|tab| tab.0.clone()).collect::<Vec<_>>(), 1);
        assert!(config.remember_navigation(&tabs, &closed));
        let reloaded: Config = toml::from_str(&toml::to_string(&config).unwrap()).unwrap();
        let (history, restored_closed) = reloaded.restored_navigation();
        assert_eq!(history[0].0, vec![Location::Local("/before".into())]);
        assert_eq!(history[1].1, vec![Location::Local("/next".into())]);
        assert_eq!(restored_closed[0].0, Location::Local("/closed".into()));
        assert_eq!(restored_closed[0].1.len(), MAX_SAVED_HISTORY);
        assert_eq!(
            restored_closed[0].1[0],
            Location::Local("/history/10".into())
        );
    }

    #[test]
    fn navigation_state_never_persists_unsafe_remote_credentials() {
        let mut config = Config::default();
        let home = Location::Local("/home".into());
        config.remember_navigation(
            &[(home.clone(), vec![home.clone()], vec![])],
            &[(home, vec![], vec![])],
        );
        config.session_histories[0]
            .back
            .push("sftp://user:pass@host/folder".into());
        config.session_closed[0].location = "sftp://user:pass@host/folder".into();
        assert_eq!(config.restored_navigation().1.len(), 0);
        config.restore_session = false;
        config.remember_navigation(&[], &[]);
        assert!(config.session_closed.is_empty());
        assert!(config.session_histories.is_empty());
    }
}
