//! The whole conversation of one transcript, for the conversation view (`v`): your prompts,
//! replies, tool calls (with their key argument) and the first line of each result.

use crate::data::parse_ts;
use crate::providers::Tool;
use serde_json::Value;
use std::path::Path;

#[derive(Clone, Debug, PartialEq)]
pub enum Entry {
    User(String),
    Reply(String),
    /// tool name, its key argument
    Call(String, String),
    /// first lines of the output, error?
    Result(String, bool),
}

#[derive(Clone, Debug)]
pub struct Item {
    pub at: Option<i64>,
    pub entry: Entry,
}

fn s<'a>(v: &'a Value, ptr: &str) -> Option<&'a str> {
    v.pointer(ptr).and_then(Value::as_str)
}

fn ts(v: &Value) -> Option<i64> {
    match v.get("timestamp")? {
        Value::String(x) => parse_ts(x),
        Value::Number(n) => n.as_i64(),
        _ => None,
    }
}

fn clip(t: &str, lines: usize, chars: usize) -> String {
    let mut out: Vec<String> = t.lines().filter(|l| !l.trim().is_empty()).take(lines).map(|l| {
        if l.chars().count() > chars { l.chars().take(chars).collect::<String>() + "…" } else { l.to_string() }
    }).collect();
    if t.lines().filter(|l| !l.trim().is_empty()).count() > lines {
        out.push("…".into());
    }
    out.join("\n")
}

/// The argument that says what a tool call did: its command, file, pattern, query or URL.
pub fn key_arg(args: &Value) -> String {
    let args = match args {
        // some agents pass arguments as a JSON string
        Value::String(x) => serde_json::from_str::<Value>(x).unwrap_or(Value::String(x.clone())),
        other => other.clone(),
    };
    if let Value::String(x) = &args {
        return clip(x, 1, 100);
    }
    for k in ["command", "cmd", "file_path", "path", "filePath", "notebook_path", "pattern", "query", "url", "description", "prompt", "task"] {
        match args.get(k) {
            Some(Value::String(v)) => return clip(v, 1, 100),
            Some(Value::Array(a)) => {
                let joined: Vec<&str> = a.iter().filter_map(Value::as_str).collect();
                if !joined.is_empty() {
                    return clip(&joined.join(" "), 1, 100);
                }
            }
            _ => {}
        }
    }
    let compact = args.to_string();
    if compact == "null" || compact == "{}" { String::new() } else { clip(&compact, 1, 100) }
}

fn text_parts(content: &Value) -> String {
    match content {
        Value::String(x) => x.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter(|p| matches!(s(p, "/type"), Some("text") | Some("input_text") | Some("output_text")))
            .filter_map(|p| s(p, "/text"))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn human(t: &str) -> bool {
    let t = t.trim_start();
    !t.is_empty() && !t.starts_with('<') && !t.starts_with("Caveat:")
}

fn push(out: &mut Vec<Item>, at: Option<i64>, entry: Entry) {
    out.push(Item { at, entry });
}

fn pi_line(v: &Value, out: &mut Vec<Item>) {
    if s(v, "/type") != Some("message") {
        return;
    }
    let at = ts(v);
    let Some(m) = v.get("message") else { return };
    match s(m, "/role") {
        Some("user") => {
            let t = text_parts(m.get("content").unwrap_or(&Value::Null));
            if !t.trim().is_empty() {
                push(out, at, Entry::User(t));
            }
        }
        Some("assistant") => {
            for p in m.get("content").and_then(Value::as_array).into_iter().flatten() {
                match s(p, "/type") {
                    Some("text") => {
                        if let Some(t) = s(p, "/text").filter(|t| !t.trim().is_empty()) {
                            push(out, at, Entry::Reply(t.to_string()));
                        }
                    }
                    Some("toolCall") => push(
                        out,
                        at,
                        Entry::Call(s(p, "/name").unwrap_or("tool").into(), key_arg(p.get("arguments").unwrap_or(&Value::Null))),
                    ),
                    _ => {}
                }
            }
        }
        Some("toolResult") => {
            let t = text_parts(m.get("content").unwrap_or(&Value::Null));
            let err = m.get("isError").and_then(Value::as_bool).unwrap_or(false);
            push(out, at, Entry::Result(clip(&t, 3, 160), err));
        }
        _ => {}
    }
}

fn claude_line(v: &Value, out: &mut Vec<Item>, sidechain_file: bool) {
    if v.get("isSidechain").and_then(Value::as_bool) == Some(true) && !sidechain_file {
        return;
    }
    let at = ts(v);
    let Some(m) = v.get("message") else { return };
    match s(v, "/type") {
        Some("user") => {
            if v.get("isMeta").and_then(Value::as_bool) == Some(true) {
                return;
            }
            match m.get("content") {
                Some(Value::String(t)) if human(t) => push(out, at, Entry::User(t.clone())),
                Some(Value::Array(parts)) => {
                    for p in parts {
                        match s(p, "/type") {
                            Some("tool_result") => {
                                let t = match p.get("content") {
                                    Some(c @ Value::Array(_)) => text_parts(c),
                                    Some(Value::String(x)) => x.clone(),
                                    _ => String::new(),
                                };
                                let err = p.get("is_error").and_then(Value::as_bool).unwrap_or(false);
                                push(out, at, Entry::Result(clip(&t, 3, 160), err));
                            }
                            Some("text") => {
                                if let Some(t) = s(p, "/text").filter(|t| human(t)) {
                                    push(out, at, Entry::User(t.to_string()));
                                }
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
        Some("assistant") => {
            for p in m.get("content").and_then(Value::as_array).into_iter().flatten() {
                match s(p, "/type") {
                    Some("text") => {
                        if let Some(t) = s(p, "/text").filter(|t| !t.trim().is_empty()) {
                            push(out, at, Entry::Reply(t.to_string()));
                        }
                    }
                    Some("tool_use") => push(
                        out,
                        at,
                        Entry::Call(s(p, "/name").unwrap_or("tool").into(), key_arg(p.get("input").unwrap_or(&Value::Null))),
                    ),
                    _ => {}
                }
            }
        }
        _ => {}
    }
}

fn codex_line(v: &Value, out: &mut Vec<Item>) {
    if s(v, "/type") != Some("response_item") {
        return;
    }
    let at = ts(v);
    let Some(p) = v.get("payload") else { return };
    match s(p, "/type") {
        Some("message") => {
            let t = text_parts(p.get("content").unwrap_or(&Value::Null));
            match s(p, "/role") {
                Some("user") if human(&t) => push(out, at, Entry::User(t)),
                Some("assistant") if !t.trim().is_empty() => push(out, at, Entry::Reply(t)),
                _ => {}
            }
        }
        Some("function_call") | Some("custom_tool_call") | Some("local_shell_call") => {
            let args = p.get("arguments").or(p.get("input")).or(p.get("action")).unwrap_or(&Value::Null);
            push(out, at, Entry::Call(s(p, "/name").unwrap_or("tool").into(), key_arg(args)));
        }
        Some("function_call_output") | Some("custom_tool_call_output") | Some("local_shell_call_output") => {
            let t = match p.get("output") {
                Some(Value::String(x)) => x.clone(),
                Some(o @ Value::Array(_)) => text_parts(o),
                Some(o) => o.to_string(),
                None => String::new(),
            };
            push(out, at, Entry::Result(clip(&t, 3, 160), false));
        }
        _ => {}
    }
}

/// Read a whole transcript into its conversation.
pub fn load(path: &Path, tool: Tool, sidechain_file: bool) -> Vec<Item> {
    if tool == Tool::Opencode {
        return crate::opencode::id_of(path).map(|id| crate::opencode::conversation(&crate::opencode::db_path(), &id)).unwrap_or_default();
    }
    let Ok(text) = std::fs::read_to_string(path) else { return vec![] };
    let mut out = Vec::new();
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        match tool {
            Tool::Pi => pi_line(&v, &mut out),
            Tool::Claude => claude_line(&v, &mut out, sidechain_file),
            Tool::Codex => codex_line(&v, &mut out),
            Tool::Opencode | Tool::Other(_) => {}
        }
    }
    out
}
