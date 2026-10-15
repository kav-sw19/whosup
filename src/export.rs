//! Copying things to the clipboard (`y`) and exporting a conversation to Markdown (`e`).

use crate::convo::{Entry, Item};
use crate::data::Session;
use std::process::{Command, Stdio};

pub fn copy(text: &str) -> Result<(), String> {
    use std::io::Write;
    let mut child = Command::new("pbcopy").stdin(Stdio::piped()).spawn().map_err(|e| e.to_string())?;
    child.stdin.as_mut().ok_or("pbcopy failed")?.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
    child.wait().map_err(|e| e.to_string())?;
    Ok(())
}

/// Seconds east of UTC for local time (from `date +%z`), read once.
fn local_offset_secs() -> i64 {
    static OFF: std::sync::OnceLock<i64> = std::sync::OnceLock::new();
    *OFF.get_or_init(|| {
        let out = Command::new("date").arg("+%z").output().ok();
        let z = out.map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
        let sign = if z.starts_with('-') { -1 } else { 1 };
        let digits: String = z.chars().filter(|c| c.is_ascii_digit()).collect();
        if digits.len() == 4 {
            sign * (digits[..2].parse::<i64>().unwrap_or(0) * 3600 + digits[2..].parse::<i64>().unwrap_or(0) * 60)
        } else {
            0
        }
    })
}

/// `2026-10-15 14:03` in local time.
pub fn local_time(ms: i64) -> String {
    format_time(ms, local_offset_secs())
}

pub fn format_time(ms: i64, offset_secs: i64) -> String {
    let secs = ms.div_euclid(1000) + offset_secs;
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // civil from days (Howard Hinnant)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}", rem / 3600, (rem % 3600) / 60)
}

/// The conversation as Markdown.
pub fn markdown(s: &Session, items: &[Item], time: impl Fn(i64) -> String) -> String {
    let mut o = format!("# {}\n\n", s.title());
    let mut facts = vec![format!("**Agent:** {}", s.tool.label())];
    if let Some(m) = &s.t.model {
        facts.push(format!("**Model:** {m}"));
    }
    if !s.t.cwd.is_empty() {
        facts.push(format!("**Folder:** `{}`", s.t.cwd));
    }
    if let Some(at) = s.t.started {
        facts.push(format!("**Started:** {}", time(at)));
    }
    if s.t.cost > 0.0 {
        facts.push(format!("**Cost:** {}${:.2}", if s.t.cost_estimated { "≈" } else { "" }, s.t.cost));
    }
    facts.push(format!("**Turns:** {} · **Tool calls:** {} · **Tokens:** {}", s.t.turns, s.t.tool_calls, s.t.tokens));
    o += &facts.join("  \n");
    o += "\n";
    let mut last = "";
    for item in items {
        let when = item.at.map(|t| format!(" · {}", time(t))).unwrap_or_default();
        match &item.entry {
            Entry::User(t) => {
                o += &format!("\n## You{when}\n\n{}\n", t.trim());
                last = "user";
            }
            Entry::Reply(t) => {
                if last != "reply" {
                    o += &format!("\n## {}{when}\n\n", s.tool.label());
                }
                o += &format!("{}\n", t.trim());
                last = "reply";
            }
            Entry::Call(name, arg) => {
                if last == "reply" || last == "user" {
                    o += "\n";
                }
                if arg.is_empty() {
                    o += &format!("- ⚙ **{name}**\n");
                } else {
                    o += &format!("- ⚙ **{name}** `{}`\n", arg.replace('`', "'"));
                }
                last = "tool";
            }
            Entry::Result(out, err) => {
                let first = out.lines().next().unwrap_or("").replace('`', "'");
                if !first.is_empty() {
                    o += &format!("  - {}`{first}`\n", if *err { "error: " } else { "" });
                }
                last = "tool";
            }
        }
    }
    o
}

/// `whosup-fix-login-redirect-2026-10-15.md`
pub fn file_name(title: &str, ms: i64) -> String {
    let slug: String = title
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|p| !p.is_empty())
        .take(8)
        .collect::<Vec<_>>()
        .join("-");
    format!("whosup-{}-{}.md", if slug.is_empty() { "session" } else { &slug }, &local_time(ms)[..10])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_and_names() {
        assert_eq!(format_time(0, 0), "1970-01-01 00:00");
        // 2026-10-15T12:04:21Z in BST (+1h)
        assert_eq!(format_time(1_792_065_861_000, 3600), "2026-10-15 13:04");
        assert!(file_name("Fix: the login redirect!", 0).starts_with("whosup-fix-the-login-redirect-"));
        assert!(file_name("???", 0).starts_with("whosup-session-"));
    }
}
