//! What whosup remembers between runs, under `~/.config/whosup/` (or `$XDG_CONFIG_HOME/whosup/`):
//!
//! - `config`, one `key=value` per line (`#` starts a comment):
//!   - `theme=light|dark`, `notify=on|off` (also set from inside whosup with `t` and `n`)
//!   - `refresh=1000`: milliseconds between refreshes (at least 250)
//!   - `hide=aider,opencode`: agents to leave out
//!   - `terminal=Ghostty|Terminal|iTerm|tmux`: where to resume or start sessions by default
//!   - `recent_hours=24`: how far back "recent" reaches (`--recent` overrides it)
//!   - `stuck_minutes=10`: one step running longer than this counts as stuck
//!   - `price.<model>=in,out[,cache_read,cache_write]`: USD per million tokens for cost
//!     estimates, matched against the model name (`price.opus=15,75,1.5,18.75`)
//! - `hosts.json`: the app each session last ran in (`{"<transcript path>": "Ghostty"}`), so a
//!   closed session can be resumed where it was.

use std::{collections::HashMap, fs, path::PathBuf};

pub fn dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".config"))
        .join("whosup")
}

/// Price per million tokens, USD.
#[derive(Clone, Debug, PartialEq)]
pub struct Price {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub light: bool,
    pub notify: bool,
    pub refresh_ms: u64,
    pub hide: Vec<String>,
    pub terminal: Option<String>,
    pub recent_hours: f64,
    pub stuck_minutes: f64,
    /// (model-name fragment, price), in file order; the first match wins
    pub prices: Vec<(String, Price)>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { light: false, notify: false, refresh_ms: 1000, hide: vec![], terminal: None, recent_hours: 24.0, stuck_minutes: 10.0, prices: vec![] }
    }
}

impl Settings {
    pub fn load() -> Self {
        match fs::read_to_string(dir().join("config")) {
            Ok(text) => Settings::parse(&text),
            // earlier versions kept only the theme, in `theme`
            Err(_) => Settings {
                light: fs::read_to_string(dir().join("theme")).map(|t| t.trim() == "light").unwrap_or(false),
                ..Settings::default()
            },
        }
    }

    pub fn parse(text: &str) -> Self {
        let mut s = Settings::default();
        for line in text.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            let Some((k, v)) = line.split_once('=').map(|(k, v)| (k.trim(), v.trim())) else { continue };
            match k {
                "theme" => s.light = v == "light",
                "notify" => s.notify = v == "on",
                "refresh" => s.refresh_ms = v.parse::<u64>().map(|n| n.max(250)).unwrap_or(s.refresh_ms),
                "hide" => s.hide = v.split(',').map(|x| x.trim().to_lowercase()).filter(|x| !x.is_empty()).collect(),
                "terminal" if !v.is_empty() => s.terminal = Some(v.to_string()),
                "recent_hours" => s.recent_hours = v.parse::<f64>().ok().filter(|h| *h > 0.0).unwrap_or(s.recent_hours),
                "stuck_minutes" => s.stuck_minutes = v.parse::<f64>().ok().filter(|m| *m > 0.0).unwrap_or(s.stuck_minutes),
                k if k.starts_with("price.") => {
                    let n: Vec<f64> = v.split(',').filter_map(|x| x.trim().parse().ok()).collect();
                    if n.len() >= 2 {
                        s.prices.push((
                            k["price.".len()..].to_lowercase(),
                            Price {
                                input: n[0],
                                output: n[1],
                                cache_read: n.get(2).copied().unwrap_or(n[0] * 0.1),
                                cache_write: n.get(3).copied().unwrap_or(n[0] * 1.25),
                            },
                        ));
                    }
                }
                _ => {}
            }
        }
        s
    }

    /// `existing` with the theme and notify lines updated (or added); every other line is kept.
    pub fn render(&self, existing: &str) -> String {
        let theme = format!("theme={}", if self.light { "light" } else { "dark" });
        let notify = format!("notify={}", if self.notify { "on" } else { "off" });
        let (mut has_theme, mut has_notify) = (false, false);
        let mut out: Vec<String> = existing
            .lines()
            .map(|l| match l.split_once('=').map(|(k, _)| k.trim()) {
                Some("theme") => {
                    has_theme = true;
                    theme.clone()
                }
                Some("notify") => {
                    has_notify = true;
                    notify.clone()
                }
                _ => l.to_string(),
            })
            .collect();
        if !has_theme {
            out.push(theme);
        }
        if !has_notify {
            out.push(notify);
        }
        out.join("\n") + "\n"
    }

    pub fn save(&self) -> std::io::Result<()> {
        fs::create_dir_all(dir())?;
        let path = dir().join("config");
        let existing = fs::read_to_string(&path).unwrap_or_default();
        fs::write(path, self.render(&existing))
    }

    pub fn hides(&self, label: &str) -> bool {
        self.hide.iter().any(|h| h == label)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_key() {
        let s = Settings::parse(
            "# whosup\ntheme=light\nnotify=on\nrefresh=100\nhide=Aider, opencode\nterminal=iTerm\nrecent_hours=72\nprice.opus=15,75\nprice.gpt-6=1.25,10,0.125,0\n",
        );
        assert!(s.light && s.notify);
        assert_eq!(s.refresh_ms, 250, "clamped to the minimum");
        assert_eq!(s.hide, vec!["aider", "opencode"]);
        assert_eq!(s.terminal.as_deref(), Some("iTerm"));
        assert_eq!(s.recent_hours, 72.0);
        assert_eq!(s.prices[0], ("opus".into(), Price { input: 15.0, output: 75.0, cache_read: 1.5, cache_write: 18.75 }));
        assert_eq!(s.prices[1].1.cache_write, 0.0);
    }

    #[test]
    fn saving_keeps_your_other_settings() {
        let s = Settings { light: true, notify: false, ..Settings::default() };
        let out = s.render("refresh=2000\ntheme=dark\n# mine\nhide=aider\n");
        assert_eq!(out, "refresh=2000\ntheme=light\n# mine\nhide=aider\nnotify=off\n");
        assert_eq!(Settings::parse(&out).refresh_ms, 2000);
    }
}
