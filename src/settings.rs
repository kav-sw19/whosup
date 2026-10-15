//! What whosup remembers between runs, under `~/.config/whosup/` (or `$XDG_CONFIG_HOME/whosup/`):
//!
//! - `config`: `theme=light|dark`, `notify=on|off`
//! - `hosts.json`: the app each session last ran in (`{"<transcript path>": "Ghostty"}`), so a
//!   closed session can be resumed where it was.

use std::{collections::HashMap, fs, path::PathBuf};

pub fn dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".config"))
        .join("whosup")
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Settings {
    pub light: bool,
    pub notify: bool,
}

impl Settings {
    pub fn load() -> Self {
        let mut s = Settings::default();
        match fs::read_to_string(dir().join("config")) {
            Ok(text) => {
                for line in text.lines() {
                    match line.split_once('=').map(|(k, v)| (k.trim(), v.trim())) {
                        Some(("theme", v)) => s.light = v == "light",
                        Some(("notify", v)) => s.notify = v == "on",
                        _ => {}
                    }
                }
            }
            // earlier versions kept only the theme, in `theme`
            Err(_) => s.light = fs::read_to_string(dir().join("theme")).map(|t| t.trim() == "light").unwrap_or(false),
        }
        s
    }

    pub fn save(&self) -> std::io::Result<()> {
        fs::create_dir_all(dir())?;
        fs::write(
            dir().join("config"),
            format!(
                "theme={}\nnotify={}\n",
                if self.light { "light" } else { "dark" },
                if self.notify { "on" } else { "off" }
            ),
        )
    }
}

/// The app each session last ran in, keyed by transcript path.
#[derive(Default)]
pub struct Hosts {
    map: HashMap<String, String>,
    dirty: bool,
}

impl Hosts {
    pub fn load() -> Self {
        let map = fs::read_to_string(dir().join("hosts.json"))
            .ok()
            .and_then(|t| serde_json::from_str::<HashMap<String, String>>(&t).ok())
            .unwrap_or_default();
        Hosts { map, dirty: false }
    }

    pub fn get(&self, key: &str) -> Option<&String> {
        self.map.get(key)
    }

    pub fn record(&mut self, key: &str, app: &str) {
        if self.map.get(key).map(String::as_str) != Some(app) {
            self.map.insert(key.to_string(), app.to_string());
            self.dirty = true;
        }
    }

    /// Write only when something changed.
    pub fn flush(&mut self) {
        if !self.dirty {
            return;
        }
        self.dirty = false;
        if fs::create_dir_all(dir()).is_ok() {
            if let Ok(json) = serde_json::to_string_pretty(&self.map) {
                let _ = fs::write(dir().join("hosts.json"), json);
            }
        }
    }
}
