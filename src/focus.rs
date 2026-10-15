//! Bring a terminal tab to the front, given its tty.
//!
//! - Terminal.app and iTerm2 expose each tab's `tty` to AppleScript, so we select it directly.
//! - Ghostty's AppleScript has no tty, so we briefly report a unique marker working directory on
//!   that tty (OSC 7), ask Ghostty which terminal has it, restore the real one, and focus it.
//! - tmux: select the pane's window and pane, then focus the terminal its client is attached to.

use std::{collections::HashMap, fs::OpenOptions, io::Write, process::Command};

/// Which app owns each tty (from the terminal scan), used to follow a tmux client to its window.
pub type AppByTty = HashMap<String, String>;

fn osascript(script: &str) -> Result<String, String> {
    let out = Command::new("osascript").arg("-e").arg(script).output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        // "Not authorized to send Apple events" is the usual one: macOS Automation permission
        if err.contains("-1743") || err.contains("Not authorized") {
            Err("macOS blocked it: allow whosup's terminal under System Settings › Privacy & Security › Automation".into())
        } else {
            Err(err.lines().last().unwrap_or("osascript failed").trim().to_string())
        }
    }
}

/// AppleScript string literal.
fn quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Report a working directory for a tty with OSC 7 (`file://localhost/path`, percent-encoded).
fn set_cwd(dev: &str, path: &str) -> Result<(), String> {
    let enc: String = path
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect();
    let mut f = OpenOptions::new().write(true).open(dev).map_err(|e| format!("can't write to {dev}: {e}"))?;
    f.write_all(format!("\x1b]7;file://localhost{enc}\x07").as_bytes()).map_err(|e| e.to_string())
}

fn ghostty(dev: &str) -> Result<(), String> {
    let id = ghostty_terminal(dev)?;
    osascript(&format!(
        "tell application \"Ghostty\"\nfocus (first terminal whose id is {})\nactivate\nend tell",
        quote(&id)
    ))?;
    Ok(())
}

/// Ghostty's id for the terminal on this tty (see `ghostty` for how).
fn ghostty_terminal(dev: &str) -> Result<String, String> {
    // Ghostty's AppleScript has no tty, but it does report each terminal's working directory,
    // which programs set with OSC 7. Briefly report a unique marker directory on this tty, ask
    // Ghostty which terminal has it, then put the real directory back. (Titles don't work as a
    // marker: Ghostty applies a title only once it has been stable for ~100ms, and agents that
    // animate their title rewrite it faster than that.)
    let listing = osascript(
        r#"tell application "Ghostty"
set out to ""
repeat with t in terminals
set out to out & (id of t) & (ASCII character 9) & (working directory of t) & linefeed
end repeat
return out
end tell"#,
    )?;
    let dirs: HashMap<String, String> = listing
        .lines()
        .filter_map(|l| l.split_once('\t'))
        .map(|(id, d)| (id.to_string(), d.to_string()))
        .collect();

    let marker = format!("/tmp/whosup-focus-{}-{}", std::process::id(), crate::data::now_ms());
    set_cwd(dev, &marker)?;
    let found = osascript(&format!(
        r#"set marker to {}
repeat 30 times
tell application "Ghostty"
try
return id of (first terminal whose working directory is marker)
end try
end tell
delay 0.03
end repeat
return """#,
        quote(&marker)
    ));
    let id = found.ok().filter(|id| !id.is_empty());
    // restore the directory the terminal reported before (home if it had none)
    let old = id
        .as_ref()
        .and_then(|id| dirs.get(id))
        .filter(|d| d.starts_with('/'))
        .cloned()
        .unwrap_or_else(|| std::env::var("HOME").unwrap_or_else(|_| "/".into()));
    let _ = set_cwd(dev, &old);

    id.ok_or_else(|| "Ghostty didn't report a terminal for this tty".to_string())
}

/// Press Esc in the terminal on `tty`: what you'd do to interrupt an agent's turn.
pub fn send_escape(tty: &str, app: &str) -> Result<String, String> {
    let dev = format!("/dev/{tty}");
    match app {
        "Ghostty" => {
            let id = ghostty_terminal(&dev)?;
            osascript(&format!("tell application \"Ghostty\" to send key \"escape\" to (first terminal whose id is {})", quote(&id)))?;
        }
        "iTerm" | "iTerm2" => {
            let r = osascript(&format!(
                r#"tell application "iTerm2"
repeat with w in windows
repeat with t in tabs of w
repeat with s in sessions of t
if tty of s is {dev} then
tell s to write text (ASCII character 27) newline NO
return "ok"
end if
end repeat
end repeat
end repeat
end tell
return "missing""#,
                dev = quote(&dev)
            ))?;
            if r != "ok" {
                return Err("iTerm2 has no session on this tty".into());
            }
        }
        other => return Err(format!("whosup can't press keys in {other}; switch to it with ↵ and press Esc")),
    }
    Ok(format!("Pressed Esc in {}", tty.trim_start_matches("tty")))
}

/// End a process politely (SIGTERM) and report whether it's gone after a moment.
pub fn terminate(pid: u32) -> Result<String, String> {
    let ok = std::process::Command::new("kill").args(["-TERM", &pid.to_string()]).status().map_err(|e| e.to_string())?;
    if !ok.success() {
        return Err(format!("Couldn't stop pid {pid}"));
    }
    for _ in 0..30 {
        std::thread::sleep(std::time::Duration::from_millis(100));
        // gone, or a zombie waiting to be collected by its parent: either way it has ended
        let stat = std::process::Command::new("ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();
        if stat.is_empty() || stat.starts_with('Z') {
            return Ok(format!("Ended pid {pid}"));
        }
    }
    Err(format!("pid {pid} is still running after 3s (it may be finishing up)"))
}

fn terminal_app(dev: &str) -> Result<(), String> {
    let r = osascript(&format!(
        r#"tell application "Terminal"
repeat with w in windows
repeat with t in tabs of w
if tty of t is {dev} then
set selected tab of w to t
set index of w to 1
activate
return "ok"
end if
end repeat
end repeat
end tell
return "missing""#,
        dev = quote(dev)
    ))?;
    if r == "ok" { Ok(()) } else { Err("Terminal has no tab on this tty".into()) }
}

fn iterm(dev: &str) -> Result<(), String> {
    let r = osascript(&format!(
        r#"tell application "iTerm2"
repeat with w in windows
repeat with t in tabs of w
repeat with s in sessions of t
if tty of s is {dev} then
select w
tell t to select
tell s to select
activate
return "ok"
end if
end repeat
end repeat
end repeat
end tell
return "missing""#,
        dev = quote(dev)
    ))?;
    if r == "ok" { Ok(()) } else { Err("iTerm2 has no session on this tty".into()) }
}

fn tmux(dev: &str, apps: &AppByTty, depth: u8) -> Result<String, String> {
    let run = |args: &[&str]| -> Result<String, String> {
        let out = Command::new("tmux").args(args).output().map_err(|e| e.to_string())?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).to_string())
        } else {
            Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
        }
    };
    let panes = run(&["list-panes", "-a", "-F", "#{pane_tty} #{session_name} #{window_id} #{pane_id}"])?;
    let (session, window, pane) = panes
        .lines()
        .find_map(|l| {
            let f: Vec<&str> = l.split(' ').collect();
            (f.len() == 4 && f[0] == dev).then(|| (f[1].to_string(), f[2].to_string(), f[3].to_string()))
        })
        .ok_or("tmux has no pane on this tty")?;
    run(&["select-window", "-t", &window])?;
    run(&["select-pane", "-t", &pane])?;
    // then raise the terminal the session's client is shown in
    let clients = run(&["list-clients", "-F", "#{client_tty} #{session_name}"])?;
    if let Some(client_tty) = clients.lines().find_map(|l| l.split_once(' ').filter(|(_, s)| *s == session).map(|(t, _)| t)) {
        let short = client_tty.trim_start_matches("/dev/");
        if let Some(app) = apps.get(short) {
            if depth == 0 {
                return focus_with_depth(short, app, apps, 1);
            }
        }
    }
    Ok(String::new())
}

/// Bring an app to the front (editors, desktop apps).
pub fn activate_app(app: &str) -> Result<String, String> {
    osascript(&format!("tell application {} to activate", quote(app)))?;
    Ok(format!("Brought {app} to the front"))
}

/// Focus the tab on `tty` (`ttys003`) owned by `app` (as named by the terminal scan).
/// Returns a short message saying what happened.
pub fn focus(tty: &str, app: &str, apps: &AppByTty) -> Result<String, String> {
    focus_with_depth(tty, app, apps, 0)
}

fn focus_with_depth(tty: &str, app: &str, apps: &AppByTty, depth: u8) -> Result<String, String> {
    let dev = format!("/dev/{tty}");
    let short = tty.trim_start_matches("tty");
    let done = |r: Result<(), String>| r.map(|_| format!("Switched to {short} in {app}"));
    match app {
        "Ghostty" => done(ghostty(&dev)),
        "Terminal" => done(terminal_app(&dev)),
        "iTerm" | "iTerm2" => done(iterm(&dev)),
        "tmux" => tmux(&dev, apps, depth).map(|m| if m.is_empty() { format!("Switched to the tmux pane on {short}") } else { m }),
        "" | "terminal" => Err(format!("No terminal app found for {short}")),
        "ssh" | "screen" | "zellij" => Err(format!("{short} is inside {app}; whosup can't switch to it")),
        // editors (VS Code, Cursor, Zed, JetBrains, Atom, …) don't let other apps pick a
        // terminal panel, so bring the editor's window forward
        editor => activate_app(editor).map(|_| format!("Brought {editor} to the front (its terminal is {short})")),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn terminate_ends_a_process() {
        let mut child = std::process::Command::new("/bin/sleep").arg("30").spawn().unwrap();
        let pid = child.id();
        assert_eq!(super::terminate(pid), Ok(format!("Ended pid {pid}")));
        child.wait().ok();
    }
}
