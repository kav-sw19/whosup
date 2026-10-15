//! Resuming a closed session: the right command for its agent, opened where it last ran.
//!
//! Where, in order: the app whosup saw it running in (remembered in `hosts.json`), what the
//! transcript says it was started from (Claude Code's `entrypoint`, Codex's originator/source),
//! and otherwise your default terminal, meaning the one whosup itself runs in.

use crate::data::Session;
use crate::providers::Tool;
use std::process::{Command, Stdio};

/// Terminal apps whosup can open a new tab in.
const TERMINALS: &[&str] = &["Ghostty", "Terminal", "iTerm", "iTerm2", "tmux"];
/// Editors: they can't be told to run a command in their terminal, so the folder is opened and
/// the resume command copied for you to paste.
const EDITORS: &[&str] = &[
    "Visual Studio Code",
    "Visual Studio Code - Insiders",
    "Cursor",
    "Windsurf",
    "Zed",
    "Atom",
    "Sublime Text",
    "Nova",
    "IntelliJ IDEA",
    "PyCharm",
    "WebStorm",
    "GoLand",
    "RustRover",
    "Android Studio",
];

#[derive(Clone, Debug, PartialEq)]
pub enum Place {
    /// a new tab in this terminal app
    Terminal(String),
    /// open the folder in this editor and copy the command
    Editor(String),
    /// the Codex desktop app
    CodexApp,
}

impl Place {
    pub fn describe(&self) -> String {
        match self {
            Place::Terminal(t) if t == "tmux" => "a new tmux window".into(),
            Place::Terminal(t) => format!("a new {t} tab"),
            Place::Editor(e) => e.clone(),
            Place::CodexApp => "the Codex app".into(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Plan {
    pub command: String,
    pub cwd: String,
    pub place: Place,
    /// why this place: "where it last ran", "where it was started", "your default terminal"
    pub because: &'static str,
}

/// Single-quote for sh.
fn sh(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

fn on_path(bin: &str) -> bool {
    Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {bin}"))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn app_installed(name: &str) -> bool {
    ["/Applications", "/System/Applications/Utilities"]
        .iter()
        .map(|d| std::path::PathBuf::from(d).join(format!("{name}.app")))
        .chain(std::env::var("HOME").ok().map(|h| std::path::PathBuf::from(h).join("Applications").join(format!("{name}.app"))))
        .any(|p| p.exists())
}

/// The terminal whosup runs in, from `$TERM_PROGRAM`; Terminal.app if it's not one we can drive.
pub fn default_terminal() -> String {
    match std::env::var("TERM_PROGRAM").unwrap_or_default().as_str() {
        "ghostty" => "Ghostty".into(),
        "iTerm.app" => "iTerm".into(),
        "tmux" => "tmux".into(),
        _ => "Terminal".into(),
    }
}

fn place_for_app(app: &str) -> Option<Place> {
    if TERMINALS.contains(&app) {
        Some(Place::Terminal(app.to_string()))
    } else if EDITORS.contains(&app) {
        Some(Place::Editor(app.to_string()))
    } else if app == "Codex" {
        Some(Place::CodexApp)
    } else {
        None
    }
}

/// What the transcript says about where the session was started.
fn started_in(s: &Session) -> Option<Place> {
    let entry = s.t.entrypoint.as_deref().unwrap_or("");
    let origin = s.t.origin.as_deref().unwrap_or("");
    match s.tool {
        Tool::Claude if entry.contains("vscode") => Some(Place::Editor("Visual Studio Code".into())),
        Tool::Codex if origin.contains("desktop") => Some(Place::CodexApp),
        Tool::Codex if entry == "vscode" || origin.contains("vscode") => Some(Place::Editor("Visual Studio Code".into())),
        _ => None,
    }
}

/// How to resume `s`, or why it can't be.
pub fn plan(s: &Session, last_host: Option<&str>) -> Result<Plan, String> {
    if s.t.cwd.is_empty() {
        return Err("No folder recorded for this session, so it can't be resumed".into());
    }
    let command = match s.tool {
        Tool::Pi => format!("pi --session {}", sh(&s.path.display().to_string())),
        Tool::Claude if !s.t.id.is_empty() => format!("claude --resume {}", sh(&s.t.id)),
        Tool::Codex if !s.t.id.is_empty() => format!("codex resume {}", sh(&s.t.id)),
        Tool::Other(n) => return Err(format!("whosup can't resume {n} sessions")),
        _ => return Err("No session id recorded, so it can't be resumed".into()),
    };
    let (mut place, because) = match last_host.and_then(place_for_app) {
        Some(p) => (p, "where it last ran"),
        None => match started_in(s) {
            Some(p) => (p, "where it was started"),
            None => (Place::Terminal(default_terminal()), "your default terminal"),
        },
    };
    let mut because = because;
    // Codex desktop threads reopen in the app; without the app, use the CLI in a terminal
    if place == Place::CodexApp && !app_installed("Codex") {
        place = Place::Terminal(default_terminal());
        because = "your default terminal";
    }
    if s.tool == Tool::Codex && matches!(place, Place::Terminal(_)) && !on_path("codex") {
        return Err("Install the Codex CLI (`codex`) to resume Codex threads in a terminal".into());
    }
    Ok(Plan { command, cwd: s.t.cwd.clone(), place, because })
}

fn osascript(script: &str) -> Result<(), String> {
    let out = Command::new("osascript").arg("-e").arg(script).output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        if err.contains("-1743") || err.contains("Not authorized") {
            Err("macOS blocked it: allow whosup's terminal under System Settings › Privacy & Security › Automation".into())
        } else {
            Err(err.lines().last().unwrap_or("osascript failed").trim().to_string())
        }
    }
}

/// AppleScript string literal.
fn q(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Carry out a plan. Returns a short message saying what happened.
pub fn launch(p: &Plan) -> Result<String, String> {
    let line = format!("cd {} && {}", sh(&p.cwd), p.command);
    match &p.place {
        // Typed into a fresh shell, so quitting the agent leaves you in that shell.
        Place::Terminal(t) if t == "Ghostty" => osascript(&format!(
            r#"tell application "Ghostty"
set cfg to new surface configuration
set initial working directory of cfg to {cwd}
set initial input of cfg to {cmd} & (ASCII character 10)
if (count of windows) > 0 then
new tab in front window with configuration cfg
else
new window with configuration cfg
end if
activate
end tell"#,
            cwd = q(&p.cwd),
            cmd = q(&p.command)
        ))
        .map(|_| format!("Resumed in a new Ghostty tab: {}", p.command)),
        Place::Terminal(t) if t == "Terminal" => osascript(&format!(
            "tell application \"Terminal\"\ndo script {}\nactivate\nend tell",
            q(&line)
        ))
        .map(|_| format!("Resumed in a new Terminal window: {}", p.command)),
        Place::Terminal(t) if t == "iTerm" || t == "iTerm2" => osascript(&format!(
            r#"tell application "iTerm"
if (count of windows) = 0 then
create window with default profile
else
tell current window to create tab with default profile
end if
tell current session of current window to write text {}
activate
end tell"#,
            q(&line)
        ))
        .map(|_| format!("Resumed in a new iTerm tab: {}", p.command)),
        Place::Terminal(_) => {
            // tmux: a new window in the current server, then type the command into it
            let out = Command::new("tmux")
                .args(["new-window", "-P", "-F", "#{pane_id}", "-c", &p.cwd])
                .output()
                .map_err(|e| e.to_string())?;
            if !out.status.success() {
                return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
            }
            let pane = String::from_utf8_lossy(&out.stdout).trim().to_string();
            Command::new("tmux").args(["send-keys", "-t", &pane, &p.command, "Enter"]).status().map_err(|e| e.to_string())?;
            Ok(format!("Resumed in a new tmux window: {}", p.command))
        }
        Place::Editor(e) => {
            copy(&line)?;
            let status = Command::new("open").args(["-a", e, &p.cwd]).status().map_err(|err| err.to_string())?;
            if !status.success() {
                return Err(format!("Couldn't open {e}"));
            }
            Ok(format!("Opened the folder in {e} and copied the resume command: paste it into its terminal"))
        }
        Place::CodexApp => {
            let status = Command::new("open").args(["-a", "Codex"]).status().map_err(|e| e.to_string())?;
            if !status.success() {
                return Err("Couldn't open the Codex app".into());
            }
            Ok("Opened the Codex app; the thread is in its history".into())
        }
    }
}

fn copy(text: &str) -> Result<(), String> {
    use std::io::Write;
    let mut child = Command::new("pbcopy").stdin(Stdio::piped()).spawn().map_err(|e| e.to_string())?;
    child.stdin.as_mut().ok_or("pbcopy failed")?.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
    child.wait().map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::sh;

    #[test]
    fn quotes_for_sh() {
        assert_eq!(sh("/a b/c"), "'/a b/c'");
        assert_eq!(sh("it's"), r"'it'\''s'");
    }
}
