//! `whosup --watch`: no window, just notifications (and remembering where sessions ran), and a
//! launchd agent to run it at login. While a watcher runs, the TUI leaves notifying to it.

use crate::App;
use std::{fs, path::PathBuf, process::Command, time::Duration};

const LABEL: &str = "com.github.kav-sw19.whosup.watch";

fn pid_file() -> PathBuf {
    crate::settings::dir().join("watch.pid")
}

fn alive(pid: u32) -> bool {
    Command::new("kill").args(["-0", &pid.to_string()]).stderr(std::process::Stdio::null()).status().map(|s| s.success()).unwrap_or(false)
}

/// Is a `--watch` process running (other than this one)?
pub fn running() -> bool {
    fs::read_to_string(pid_file())
        .ok()
        .and_then(|p| p.trim().parse::<u32>().ok())
        .is_some_and(|pid| pid != std::process::id() && alive(pid))
}

pub fn run(mut app: App) -> std::io::Result<()> {
    if running() {
        eprintln!("whosup is already watching (pid {})", fs::read_to_string(pid_file()).unwrap_or_default().trim());
        std::process::exit(1);
    }
    fs::create_dir_all(crate::settings::dir())?;
    fs::write(pid_file(), std::process::id().to_string())?;
    app.watching = true;
    let every = Duration::from_millis(app.settings.refresh_ms.max(1000));
    eprintln!("whosup: watching for sessions that need you (every {}s)", every.as_secs());
    loop {
        app.refresh();
        std::thread::sleep(every);
    }
}

fn plist_path() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default()).join("Library/LaunchAgents").join(format!("{LABEL}.plist"))
}

/// The launchd agent: run `<binary> --watch` at login, keep it running, log to ~/Library/Logs.
pub fn plist(binary: &str, home: &str) -> String {
    let esc = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{LABEL}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{bin}</string>
    <string>--watch</string>
  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>
  <key>ProcessType</key>
  <string>Background</string>
  <key>StandardErrorPath</key>
  <string>{home}/Library/Logs/whosup-watch.log</string>
  <key>StandardOutPath</key>
  <string>{home}/Library/Logs/whosup-watch.log</string>
</dict>
</plist>
"#,
        bin = esc(binary),
        home = esc(home)
    )
}

fn uid() -> String {
    Command::new("id").arg("-u").output().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default()
}

pub fn install() -> Result<String, String> {
    let binary = std::env::current_exe().map_err(|e| e.to_string())?.canonicalize().map_err(|e| e.to_string())?;
    let home = std::env::var("HOME").unwrap_or_default();
    let path = plist_path();
    fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    // replace an existing one
    let _ = Command::new("launchctl").args(["bootout", &format!("gui/{}", uid()), &path.display().to_string()]).output();
    fs::write(&path, plist(&binary.display().to_string(), &home)).map_err(|e| e.to_string())?;
    let out = Command::new("launchctl")
        .args(["bootstrap", &format!("gui/{}", uid()), &path.display().to_string()])
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!("launchctl bootstrap failed: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(format!(
        "Installed: whosup --watch now runs at login and is running now.\n  {}\n  log: ~/Library/Logs/whosup-watch.log\n  remove with: whosup --uninstall-service",
        crate::ui::home_rel(&path.display().to_string())
    ))
}

pub fn uninstall() -> Result<String, String> {
    let path = plist_path();
    let _ = Command::new("launchctl").args(["bootout", &format!("gui/{}", uid()), &path.display().to_string()]).output();
    match fs::remove_file(&path) {
        Ok(()) => Ok("Removed: whosup --watch no longer runs at login.".into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok("Nothing to remove: the service wasn't installed.".into()),
        Err(e) => Err(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn plist_runs_watch_at_login() {
        let p = super::plist("/Users/you/.cargo/bin/whosup", "/Users/you");
        assert!(p.contains("<string>/Users/you/.cargo/bin/whosup</string>\n    <string>--watch</string>"));
        assert!(p.contains("<key>RunAtLoad</key>\n  <true/>"));
        assert!(p.contains("/Users/you/Library/Logs/whosup-watch.log"));
    }
}
