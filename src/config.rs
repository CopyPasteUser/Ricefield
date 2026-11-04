use crate::location::Location;
use serde::{Deserialize, Serialize};
use std::{env, fs, io, path::PathBuf};

pub const MAX_SAVED_TABS: usize = 32;

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub theme: String,
    pub show_hidden: bool,
    pub show_sidebar: bool,
    pub view: String,
    pub sort_by: String,
    pub descending: bool,
    pub bookmarks: Vec<PathBuf>,
    pub remote_favorites: Vec<String>,
    pub restore_session: bool,
    // Local absolute paths and validated network URIs share this legacy TOML
    // string field, so existing saved sessions remain readable.
    pub session_tabs: Vec<String>,
    pub session_active: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            theme: "win7".to_owned(),
            show_hidden: false,
            show_sidebar: true,
            view: "list".to_owned(),
            sort_by: "name".to_owned(),
            descending: false,
            bookmarks: Vec::new(),
            remote_favorites: Vec::new(),
            restore_session: true,
            session_tabs: Vec::new(),
            session_active: 0,
        }
    }
}

pub fn home() -> PathBuf {
    env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from("/")))
}

pub fn config_file() -> PathBuf {
    let root = env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".config"));
    root.join("rice-explorer").join("config.toml")
}

impl Config {
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

    pub fn load() -> Self {
        fs::read_to_string(config_file())
            .ok()
            .and_then(|contents| toml::from_str(&contents).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> io::Result<()> {
        let file = config_file();
        if let Some(parent) = file.parent() {
            fs::create_dir_all(parent)?;
        }
        let encoded = toml::to_string_pretty(self).map_err(io::Error::other)?;
        let temporary = file.with_extension("toml.tmp");
        fs::write(&temporary, encoded)?;
        fs::rename(temporary, file)
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn older_preferences_default_to_session_restore_without_requiring_new_keys() {
        let config: Config = toml::from_str("theme = 'win7'\nshow_hidden = true\n").unwrap();
        assert!(config.restore_session);
        assert!(config.session_tabs.is_empty());
        assert!(config.show_hidden);
        assert!(config.remote_favorites.is_empty());
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
}
