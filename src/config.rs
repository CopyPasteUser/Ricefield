use serde::{Deserialize, Serialize};
use std::{env, fs, io, path::PathBuf};

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
