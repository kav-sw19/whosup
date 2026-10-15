//! Gemini CLI and aider, from their own records (formats from their documentation and source;
//! neither is installed on the machine whosup was built on, so these are checked against sample
//! files only).
//!
//! - Gemini CLI: `~/.gemini/tmp/<project hash>/chats/session-*.jsonl` (one record per line:
//!   metadata, messages, `$set` / `$rewindTo` updates), or a single `session-*.json` in older
//!   versions. The project folder is stored only as a SHA-256 hash, so whosup hashes folders it
//!   knows (every other session's, and running processes') to find it.
//! - aider: `.aider.chat.history.md` in the project. Each `# aider chat started at …` block is a
//!   session; `#### ` lines are your prompts, `> ` lines aider's own output (model, tokens,
//!   cost, applied edits), the rest its replies.

use crate::convo::{key_arg, Entry, Item};
use crate::data::{parse_ts, Transcript};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub fn sha256_hex(s: &str) -> String {
    Sha256::digest(s.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

// ── Gemini CLI ────────────────────────────────────────────────────────────────

pub fn gemini_dir() -> PathBuf {
    std::env::var_os("GEMINI_CLI_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".gemini"))
}

/// Every session file: `<gemini dir>/tmp/*/chats/session-*.json{,l}`.
pub fn gemini_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(projects) = std::fs::read_dir(dir.join("tmp")) else { return out };
    for p in projects.flatten() {
        let Ok(chats) = std::fs::read_dir(p.path().join("chats")) else { continue };
        for f in chats.flatten() {
            let path = f.path();
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.starts_with("session-") && (name.ends_with(".jsonl") || name.ends_with(".json")) {
                out.push(path);
            }
        }
    }
    out
}

fn gemini_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts.iter().filter_map(|p| p.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join("\n"),
        _ => String::new(),
    }
}

/// The records of a session file, in order: (metadata, messages). Handles both layouts and the
/// JSONL control records (`$set` merges metadata, `$rewindTo` drops messages after an id, a
/// message with a known id replaces it).
fn gemini_records(text: &str) -> (Value, Vec<Value>) {
    let mut meta = serde_json::Map::new();
    let mut msgs: Vec<Value> = Vec::new();
    let take_message = |m: Value, msgs: &mut Vec<Value>| {
        let id = m.get("id").and_then(Value::as_str).map(String::from);
        match id.and_then(|id| msgs.iter().position(|x| x.get("id").and_then(Value::as_str) == Some(id.as_str()))) {
            Some(i) => msgs[i] = m,
            None => msgs.push(m),
        }
    };
    if let Ok(Value::Object(whole)) = serde_json::from_str::<Value>(text) {
        // the older single-object layout
        for (k, v) in whole {
            if k == "messages" {
                for m in v.as_array().cloned().unwrap_or_default() {
                    take_message(m, &mut msgs);
                }
            } else {
                meta.insert(k, v);
            }
        }
        return (Value::Object(meta), msgs);
    }
    for line in text.lines() {
        let Ok(Value::Object(rec)) = serde_json::from_str::<Value>(line) else { continue };
        if let Some(Value::Object(set)) = rec.get("$set") {
            meta.extend(set.clone());
        } else if let Some(id) = rec.get("$rewindTo").and_then(Value::as_str) {
            if let Some(i) = msgs.iter().position(|m| m.get("id").and_then(Value::as_str) == Some(id)) {
                msgs.truncate(i + 1);
            }
        } else if rec.contains_key("type") && rec.contains_key("id") {
            take_message(Value::Object(rec), &mut msgs);
        } else if rec.contains_key("sessionId") {
            meta.extend(rec);
        }
    }
    (Value::Object(meta), msgs)
}

/// A Gemini CLI session as a Transcript. `folders` maps project hashes to folders.
pub fn gemini_transcript(text: &str, folders: &HashMap<String, String>) -> Option<Transcript> {
    let (meta, msgs) = gemini_records(text);
    let s = |k: &str| meta.get(k).and_then(Value::as_str).map(String::from);
    if meta.get("kind").and_then(Value::as_str) == Some("subagent") {
        return None;
    }
    let mut t = Transcript {
        id: s("sessionId")?,
        started: s("startTime").and_then(|x| parse_ts(&x)),
        name: s("summary"),
        ..Transcript::default()
    };
    t.cwd = meta
        .get("directories")
        .and_then(Value::as_array)
        .and_then(|d| d.first())
        .and_then(Value::as_str)
        .map(String::from)
        .or_else(|| s("projectHash").and_then(|h| folders.get(&h).cloned()))
        .unwrap_or_default();
    for m in &msgs {
        let at = m.get("timestamp").and_then(Value::as_str).and_then(parse_ts);
        if at.is_some() {
            t.last_ts = at;
        }
        match m.get("type").and_then(Value::as_str) {
            Some("user") => {
                let text = gemini_text(m.get("content").unwrap_or(&Value::Null));
                t.user_msgs += 1;
                t.messages += 1;
                if t.first_user.is_none() {
                    t.first_user = Some(text.clone());
                }
                t.last_user = Some(text);
                t.last_role = Some("user".into());
                t.last_stop = None;
            }
            Some("gemini") => {
                t.messages += 1;
                t.turns += 1;
                if let Some(model) = m.get("model").and_then(Value::as_str) {
                    t.model = Some(model.into());
                }
                if let Some(tok) = m.get("tokens") {
                    let g = |k: &str| tok.get(k).and_then(Value::as_u64).unwrap_or(0);
                    t.tokens += g("total");
                    t.in_tokens += g("input").saturating_sub(g("cached"));
                    t.cache_read_tokens += g("cached");
                    t.out_tokens += g("output") + g("thoughts");
                    t.context_tokens = g("input") + g("output");
                }
                let text = gemini_text(m.get("content").unwrap_or(&Value::Null));
                if !text.trim().is_empty() {
                    t.last_text = Some(text);
                }
                t.last_role = Some("assistant".into());
                t.last_stop = Some("stop".into());
                t.approval_requested = false;
                for c in m.get("toolCalls").and_then(Value::as_array).into_iter().flatten() {
                    t.tool_calls += 1;
                    let name = c.get("name").and_then(Value::as_str).unwrap_or("tool");
                    t.note_call(name, c.get("args").unwrap_or(&Value::Null));
                    match c.get("status").and_then(Value::as_str) {
                        Some("executing") | Some("scheduled") | Some("validating") => {
                            t.last_stop = Some("toolUse".into());
                            t.last_tool = Some(name.into());
                        }
                        Some("awaiting_approval") => {
                            t.last_stop = Some("toolUse".into());
                            t.last_tool = Some(name.into());
                            t.approval_requested = true;
                        }
                        _ => {}
                    }
                }
            }
            Some("error") => {
                t.last_role = Some("assistant".into());
                t.last_stop = Some("error".into());
            }
            _ => {}
        }
    }
    if t.last_ts.is_none() {
        t.last_ts = s("lastUpdated").and_then(|x| parse_ts(&x)).or(t.started);
    }
    Some(t)
}

pub fn gemini_conversation(text: &str) -> Vec<Item> {
    let (_, msgs) = gemini_records(text);
    let mut out = Vec::new();
    for m in msgs {
        let at = m.get("timestamp").and_then(Value::as_str).and_then(parse_ts);
        let text = gemini_text(m.get("content").unwrap_or(&Value::Null));
        match m.get("type").and_then(Value::as_str) {
            Some("user") if !text.trim().is_empty() => out.push(Item { at, entry: Entry::User(text) }),
            Some("gemini") => {
                if !text.trim().is_empty() {
                    out.push(Item { at, entry: Entry::Reply(text) });
                }
                for c in m.get("toolCalls").and_then(Value::as_array).into_iter().flatten() {
                    let name = c.get("name").and_then(Value::as_str).unwrap_or("tool").to_string();
                    out.push(Item { at, entry: Entry::Call(name, key_arg(c.get("args").unwrap_or(&Value::Null))) });
                    let result = match c.get("result") {
                        Some(Value::Array(parts)) => parts
                            .iter()
                            .filter_map(|p| {
                                p.get("text").and_then(Value::as_str).map(String::from).or_else(|| {
                                    p.pointer("/functionResponse/response/output").and_then(Value::as_str).map(String::from)
                                })
                            })
                            .collect::<Vec<_>>()
                            .join("\n"),
                        Some(Value::String(x)) => x.clone(),
                        _ => String::new(),
                    };
                    if !result.trim().is_empty() {
                        let first: Vec<&str> = result.lines().filter(|l| !l.trim().is_empty()).take(3).collect();
                        out.push(Item { at, entry: Entry::Result(first.join("\n"), c.get("status").and_then(Value::as_str) == Some("error")) });
                    }
                }
            }
            Some("error") if !text.trim().is_empty() => out.push(Item { at, entry: Entry::Result(text, true) }),
            _ => {}
        }
    }
    out
}

// ── aider ─────────────────────────────────────────────────────────────────────

pub const AIDER_HISTORY: &str = ".aider.chat.history.md";

/// One `# aider chat started at …` block.
#[derive(Clone, Debug)]
pub struct AiderBlock {
    pub started: i64,
    pub lines: Vec<String>,
}

/// Local `YYYY-MM-DD HH:MM:SS` to ms, given the local offset.
fn local_to_ms(s: &str, offset_secs: i64) -> Option<i64> {
    parse_ts(&s.replace(' ', "T")).map(|ms| ms - offset_secs * 1000)
}

pub fn aider_blocks(text: &str, offset_secs: i64) -> Vec<AiderBlock> {
    let mut out: Vec<AiderBlock> = Vec::new();
    for line in text.lines() {
        if let Some(when) = line.strip_prefix("# aider chat started at ") {
            out.push(AiderBlock { started: local_to_ms(when.trim(), offset_secs).unwrap_or(0), lines: vec![] });
        } else if let Some(b) = out.last_mut() {
            b.lines.push(line.to_string());
        }
    }
    out
}

/// `12k` / `1.2M` / `950` -> tokens
fn tok(s: &str) -> u64 {
    let s = s.trim().trim_end_matches(',');
    let (num, mult) = match s.chars().last() {
        Some('k') => (&s[..s.len() - 1], 1_000.0),
        Some('M') => (&s[..s.len() - 1], 1_000_000.0),
        _ => (s, 1.0),
    };
    (num.parse::<f64>().unwrap_or(0.0) * mult) as u64
}

pub fn aider_transcript(b: &AiderBlock, cwd: &str, mtime: i64, is_last: bool) -> Transcript {
    let mut t = Transcript { id: format!("aider-{}", b.started), cwd: cwd.to_string(), started: Some(b.started), ..Transcript::default() };
    let mut reply = String::new();
    let flush = |reply: &mut String, t: &mut Transcript| {
        if !reply.trim().is_empty() {
            t.last_text = Some(reply.trim().to_string());
            t.turns += 1;
            t.messages += 1;
            t.last_role = Some("assistant".into());
            t.last_stop = Some("stop".into());
        }
        reply.clear();
    };
    for line in &b.lines {
        if let Some(prompt) = line.strip_prefix("#### ") {
            flush(&mut reply, &mut t);
            t.user_msgs += 1;
            t.messages += 1;
            if t.first_user.is_none() {
                t.first_user = Some(prompt.to_string());
            }
            t.last_user = Some(prompt.to_string());
            t.last_role = Some("user".into());
            t.last_stop = None;
        } else if let Some(info) = line.strip_prefix("> ") {
            if let Some(m) = info.strip_prefix("Main model: ").or(info.strip_prefix("Model: ")) {
                t.model = m.split_whitespace().next().map(String::from);
            } else if let Some(f) = info.strip_prefix("Applied edit to ") {
                t.tool_calls += 1;
                t.note_call("edit", &serde_json::json!({ "path": f.trim() }));
            } else if let Some(rest) = info.strip_prefix("Tokens: ") {
                // "Tokens: 12k sent, 1.2k received. Cost: $0.02 message, $0.10 session."
                let parts: Vec<&str> = rest.split_whitespace().collect();
                if parts.len() >= 4 {
                    t.in_tokens += tok(parts[0]);
                    t.out_tokens += tok(parts[2]);
                    t.tokens = t.in_tokens + t.out_tokens;
                    t.context_tokens = tok(parts[0]) + tok(parts[2]);
                }
                if let Some(i) = rest.find("message, $") {
                    let after = &rest[i + "message, $".len()..];
                    if let Some(cost) = after.split_whitespace().next().and_then(|c| c.parse::<f64>().ok()) {
                        t.cost = cost;
                    }
                }
            }
        } else {
            if !reply.is_empty() || !line.trim().is_empty() {
                reply.push_str(line);
                reply.push('\n');
            }
        }
    }
    flush(&mut reply, &mut t);
    t.last_ts = Some(if is_last { mtime } else { b.started });
    t
}

pub fn aider_conversation(b: &AiderBlock) -> Vec<Item> {
    let mut out = Vec::new();
    let mut reply = String::new();
    let flush = |reply: &mut String, out: &mut Vec<Item>| {
        if !reply.trim().is_empty() {
            out.push(Item { at: None, entry: Entry::Reply(reply.trim().to_string()) });
        }
        reply.clear();
    };
    for line in &b.lines {
        if let Some(p) = line.strip_prefix("#### ") {
            flush(&mut reply, &mut out);
            out.push(Item { at: None, entry: Entry::User(p.to_string()) });
        } else if let Some(info) = line.strip_prefix("> ") {
            if let Some(f) = info.strip_prefix("Applied edit to ") {
                flush(&mut reply, &mut out);
                out.push(Item { at: None, entry: Entry::Call("edit".into(), f.trim().to_string()) });
            }
        } else if !reply.is_empty() || !line.trim().is_empty() {
            reply.push_str(line);
            reply.push('\n');
        }
    }
    flush(&mut reply, &mut out);
    out
}

/// `aider:<history file>#<block start>` stands in for a transcript path.
pub fn aider_path(file: &Path, started: i64) -> PathBuf {
    PathBuf::from(format!("aider:{}#{started}", file.display()))
}

pub fn aider_parts(path: &Path) -> Option<(PathBuf, i64)> {
    let s = path.to_str()?.strip_prefix("aider:")?;
    let (f, at) = s.rsplit_once('#')?;
    Some((PathBuf::from(f), at.parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    const GEMINI_JSONL: &str = r#"{"sessionId":"g-1","projectHash":"HASH","startTime":"2026-10-15T09:00:00.000Z","lastUpdated":"2026-10-15T09:00:10.000Z","kind":"main"}
{"id":"u1","timestamp":"2026-10-15T09:00:01.000Z","type":"user","content":[{"text":"why is the build slow?"}]}
{"id":"g1","timestamp":"2026-10-15T09:00:05.000Z","type":"gemini","content":[{"text":"Looking at the webpack config."}],"model":"gemini-3-pro","tokens":{"input":1000,"output":200,"cached":400,"thoughts":50,"tool":0,"total":1250},"toolCalls":[{"id":"c1","name":"read_file","args":{"path":"webpack.config.js"},"result":[{"text":"module.exports = {}"}],"status":"success","timestamp":"2026-10-15T09:00:04.000Z"}]}
{"id":"g2","timestamp":"2026-10-15T09:00:09.000Z","type":"gemini","content":[{"text":"Source maps are on in production."}],"model":"gemini-3-pro","tokens":{"input":1400,"output":100,"cached":1000,"total":1500}}
{"$set":{"summary":"Speed up the build"}}
"#;

    #[test]
    fn gemini_session() {
        let folders = HashMap::from([("HASH".to_string(), "/Users/you/code/web".to_string())]);
        let t = gemini_transcript(GEMINI_JSONL, &folders).unwrap();
        assert_eq!(t.id, "g-1");
        assert_eq!(t.name.as_deref(), Some("Speed up the build"));
        assert_eq!(t.cwd, "/Users/you/code/web");
        assert_eq!(t.model.as_deref(), Some("gemini-3-pro"));
        assert_eq!((t.turns, t.tool_calls, t.tokens), (2, 1, 2750));
        assert_eq!(t.first_user.as_deref(), Some("why is the build slow?"));
        assert_eq!(t.last_text.as_deref(), Some("Source maps are on in production."));
        assert_eq!(t.last_stop.as_deref(), Some("stop"));
        let convo: Vec<Entry> = gemini_conversation(GEMINI_JSONL).into_iter().map(|i| i.entry).collect();
        assert_eq!(convo[1], Entry::Reply("Looking at the webpack config.".into()));
        assert_eq!(convo[2], Entry::Call("read_file".into(), "webpack.config.js".into()));
        assert_eq!(convo[3], Entry::Result("module.exports = {}".into(), false));
    }

    #[test]
    fn gemini_waiting_for_approval_and_older_layout() {
        let pending = r#"{"sessionId":"g-2","projectHash":"x","startTime":"2026-10-15T09:00:00.000Z","lastUpdated":"2026-10-15T09:00:00.000Z","messages":[
          {"id":"u","timestamp":"2026-10-15T09:00:01.000Z","type":"user","content":"delete the build folder"},
          {"id":"g","timestamp":"2026-10-15T09:00:02.000Z","type":"gemini","content":"","toolCalls":[{"id":"c","name":"run_shell_command","args":{"command":"rm -rf build"},"status":"awaiting_approval","timestamp":"2026-10-15T09:00:02.000Z"}]}
        ]}"#;
        let t = gemini_transcript(pending, &HashMap::new()).unwrap();
        assert!(t.approval_requested);
        assert_eq!((t.last_stop.as_deref(), t.last_tool.as_deref()), (Some("toolUse"), Some("run_shell_command")));
        // sub-agent recordings aren't sessions of their own
        assert!(gemini_transcript(r#"{"sessionId":"s","kind":"subagent","messages":[]}"#, &HashMap::new()).is_none());
    }

    const AIDER: &str = "# aider chat started at 2026-10-14 10:00:00

> aider --model sonnet
> Main model: claude-sonnet-4-8 with diff edit format

#### add a health check endpoint

Here's the route.

> Applied edit to app/routes.py
> Tokens: 12k sent, 1.2k received. Cost: $0.05 message, $0.05 session.

# aider chat started at 2026-10-15 09:30:00

> Main model: claude-sonnet-4-8 with diff edit format

#### fix the failing test

The fixture was stale; updated it.

> Applied edit to tests/test_api.py
> Tokens: 8.5k sent, 900 received. Cost: $0.03 message, $0.03 session.

#### now run it again
";

    #[test]
    fn aider_history() {
        let blocks = aider_blocks(AIDER, 3600);
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[1].started, parse_ts("2026-10-15T08:30:00").unwrap(), "local time, BST");
        let first = aider_transcript(&blocks[0], "/r", 0, false);
        assert_eq!(first.first_user.as_deref(), Some("add a health check endpoint"));
        assert_eq!(first.model.as_deref(), Some("claude-sonnet-4-8"));
        assert_eq!((first.in_tokens, first.out_tokens, first.cost), (12_000, 1_200, 0.05));
        assert_eq!(first.files.iter().map(|(f, _)| f.as_str()).collect::<Vec<_>>(), vec!["/r/app/routes.py"]);
        assert_eq!(first.last_stop.as_deref(), Some("stop"));
        // the newest block ends with a prompt aider hasn't answered yet
        let last = aider_transcript(&blocks[1], "/r", 42, true);
        assert_eq!(last.last_role.as_deref(), Some("user"));
        assert!(last.is_busy());
        assert_eq!(last.last_text.as_deref(), Some("The fixture was stale; updated it."));
        let convo: Vec<Entry> = aider_conversation(&blocks[1]).into_iter().map(|i| i.entry).collect();
        assert_eq!(
            convo,
            vec![
                Entry::User("fix the failing test".into()),
                Entry::Reply("The fixture was stale; updated it.".into()),
                Entry::Call("edit".into(), "tests/test_api.py".into()),
                Entry::User("now run it again".into()),
            ]
        );
    }

    #[test]
    fn hashes_folders_like_gemini() {
        assert_eq!(&sha256_hex("/Users/you/code/api")[..16], "fa603e84a4b3c1aa");
    }
}
