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
    pub session_tabs: Vec<PathBuf>,
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
    /// Restore only bounded, absolute, UTF-8 paths. Do not stat paths here:
    /// checking an unavailable remote mount could freeze the UI at startup.
    pub fn restored_session(&self) -> (Vec<PathBuf>, usize) {
        if !self.restore_session {
            return (Vec::new(), 0);
        }
        let mut paths = Vec::new();
        let mut active = 0;
        for (index, path) in self.session_tabs.iter().enumerate() {
            if paths.len() >= MAX_SAVED_TABS {
                break;
            }
            if !path.is_absolute() || path.to_str().is_none() {
                continue;
            }
            if index == self.session_active {
                active = paths.len();
            }
            paths.push(path.clone());
        }
        (paths, active)
    }

    /// A private user setting, never a background replay of file mutations.
    /// Non-UTF-8 paths remain navigable but are excluded from TOML snapshots.
    pub fn remember_session(&mut self, paths: &[PathBuf], active_index: usize) -> bool {
        let (remembered, active) = if self.restore_session {
            let mut remembered = Vec::new();
            let mut active = 0;
            for (index, path) in paths.iter().enumerate() {
                if remembered.len() >= MAX_SAVED_TABS {
                    break;
                }
                if !path.is_absolute() || path.to_str().is_none() {
                    continue;
                }
                if index == active_index {
                    active = remembered.len();
                }
                remembered.push(path.clone());
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
        let paths = vec![PathBuf::from("/one"), PathBuf::from("/two")];
        assert!(config.remember_session(&paths, 1));
        assert!(!config.remember_session(&paths, 1));
        let encoded = toml::to_string(&config).unwrap();
        let reloaded: Config = toml::from_str(&encoded).unwrap();
        assert_eq!(reloaded.restored_session(), (paths, 1));
    }

    #[test]
    fn session_restore_is_bounded_and_never_checks_filesystem_state() {
        let mut config = Config::default();
        let mut paths = vec![PathBuf::from("relative"), PathBuf::from("/offline/share")];
        paths.extend((0..40).map(|index| PathBuf::from(format!("/folder-{index}"))));
        assert!(config.remember_session(&paths, 1));
        let (restored, active) = config.restored_session();
        assert_eq!(restored.len(), MAX_SAVED_TABS);
        assert_eq!(restored[0], PathBuf::from("/offline/share"));
        assert_eq!(active, 0);
    }

    #[test]
    fn opting_out_discards_saved_tab_paths() {
        let mut config = Config::default();
        config.remember_session(&[PathBuf::from("/private")], 0);
        config.restore_session = false;
        assert!(config.remember_session(&[PathBuf::from("/another")], 0));
        assert!(config.session_tabs.is_empty());
        assert_eq!(config.restored_session(), (Vec::new(), 0));
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_tab_path_does_not_break_preference_serialization() {
        use std::os::unix::ffi::OsStringExt;
        let mut config = Config::default();
        let raw = PathBuf::from(std::ffi::OsString::from_vec(b"/dir-\xff".to_vec()));
        assert!(config.remember_session(&[raw, PathBuf::from("/visible")], 0));
        assert_eq!(
            config.restored_session(),
            (vec![PathBuf::from("/visible")], 0)
        );
        assert!(toml::to_string(&config).is_ok());
    }
}
