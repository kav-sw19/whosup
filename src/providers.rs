//! Which coding agent a session or process belongs to, and transcript parsers
//! for the ones that keep a JSONL transcript on disk (pi, Claude Code, Codex).
//! Each parser maps its format onto the same `Transcript` summary:
//! `last_role` user/toolResult = the model is working, assistant + `toolUse` =
//! a tool is running, assistant + `stop` = waiting for you.

use crate::data::{parse_ts, Transcript};
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tool {
    Pi,
    Claude,
    Codex,
    Opencode,
    Gemini,
    Aider,
    /// A known agent CLI with no transcript support: shown from its process only.
    Other(&'static str),
}

impl Tool {
    pub fn label(&self) -> &'static str {
        match self {
            Tool::Pi => "pi",
            Tool::Claude => "claude",
            Tool::Codex => "codex",
            Tool::Opencode => "opencode",
            Tool::Gemini => "gemini",
            Tool::Aider => "aider",
            Tool::Other(n) => n,
        }
    }

    /// Recognise an agent CLI from its argv (`claude`, `node /…/codex`, …).
    pub fn from_argv(argv0: &str, argv1: Option<&str>, name: &str) -> Option<Tool> {
        // the desktop apps (Claude.app, Codex.app) are not terminal sessions
        if argv0.contains(".app/") {
            return None;
        }
        let base = |s: &str| s.rsplit('/').next().unwrap_or(s).to_string();
        let mut candidates = vec![base(argv0), name.to_string()];
        if matches!(base(argv0).as_str(), "node" | "bun" | "deno" | "python" | "python3") {
            if let Some(a) = argv1 {
                candidates.push(base(a).trim_end_matches(".js").trim_end_matches(".mjs").to_string());
            }
        }
        for c in candidates {
            let t = match c.as_str() {
                "pi" => Tool::Pi,
                "claude" => Tool::Claude,
                "codex" => Tool::Codex,
                "opencode" => Tool::Opencode,
                "gemini" => Tool::Gemini,
                "aider" => Tool::Aider,
                "amp" => Tool::Other("amp"),
                "goose" => Tool::Other("goose"),
                "crush" => Tool::Other("crush"),
                "qwen" => Tool::Other("qwen"),
                "cursor-agent" => Tool::Other("cursor"),
                "copilot" => Tool::Other("copilot"),
                "droid" => Tool::Other("droid"),
                _ => continue,
            };
            return Some(t);
        }
        None
    }
}

fn s<'a>(v: &'a Value, ptr: &str) -> Option<&'a str> {
    v.pointer(ptr).and_then(Value::as_str)
}

fn clip(t: &str) -> String {
    if t.chars().count() <= 4000 { t.to_string() } else { t.chars().take(4000).collect::<String>() + "…" }
}

/// Text a person typed, as opposed to harness-injected context (`<environment_context>`, …).
fn is_human_text(t: &str) -> bool {
    let t = t.trim_start();
    !t.is_empty() && !t.starts_with('<') && !t.starts_with("Caveat:")
}

/// Arguments may arrive as a JSON string.
fn args_obj(args: &Value) -> Value {
    match args {
        Value::String(x) => serde_json::from_str(x).unwrap_or(Value::Null),
        other => other.clone(),
    }
}

fn first_str(v: &Value, keys: &[&str]) -> String {
    keys.iter().find_map(|k| v.get(*k).and_then(Value::as_str)).unwrap_or("").to_string()
}

impl Transcript {
    /// Codex multi-agent tools: `spawn_agent` starts one; `wait_agent` / `interrupt_agent`
    /// (naming it, or its spawn call) end it.
    fn note_codex_collab(&mut self, name: &str, call_id: &str, args: &Value, ts: Option<i64>) {
        let a = args_obj(args);
        match name {
            "spawn_agent" => self.codex_spawns.push(crate::data::CodexSpawn {
                call_id: call_id.to_string(),
                name: first_str(&a, &["task_name", "name", "agent_name", "agent", "path", "id"]),
                task: first_str(&a, &["message", "task", "prompt", "instructions", "description"]),
                started: ts,
                ..Default::default()
            }),
            "wait_agent" | "interrupt_agent" => {
                let target = first_str(&a, &["task_name", "name", "agent_name", "agent", "path", "id", "target"]);
                // remember which agent this call is about, so its output can finish it
                self.pending_wait.push((call_id.to_string(), target));
            }
            _ => {}
        }
    }

    fn note_codex_collab_output(&mut self, call_id: &str, out: &str, ts: Option<i64>) {
        if let Some(i) = self.pending_wait.iter().position(|(c, _)| c == call_id) {
            let (_, target) = self.pending_wait.remove(i);
            for sp in self.codex_spawns.iter_mut().filter(|sp| sp.finished.is_none()) {
                if target.is_empty() || sp.name == target || target.ends_with(&format!("/{}", sp.name)) || sp.call_id == target {
                    sp.finished = ts;
                    sp.result = Some(clip(out));
                }
            }
        }
    }

    fn note_user_text(&mut self, text: &str) {
        self.user_msgs += 1;
        if is_human_text(text) {
            if self.first_user.is_none() {
                self.first_user = Some(clip(text));
            }
            self.last_user = Some(clip(text));
        }
    }

    // ── Claude Code ───────────────────────────────────────────────────────────
    // One line per content block; blocks of one API message share `message.id`.
    pub(crate) fn ingest_claude(&mut self, v: &Value) {
        let ts = v.get("timestamp").and_then(Value::as_str).and_then(parse_ts);
        if self.started.is_none() {
            self.started = ts;
        }
        if self.id.is_empty() {
            if let Some(id) = s(v, "/sessionId") {
                self.id = id.into();
            }
        }
        if let Some(c) = s(v, "/cwd") {
            if self.cwd.is_empty() {
                self.cwd = c.into();
            }
        }
        if let Some(e) = s(v, "/entrypoint") {
            self.entrypoint = Some(e.into());
        }
        if v.get("isSidechain").and_then(Value::as_bool) == Some(true) && !self.sidechain_file {
            return; // older Claude Code wrote sub-agent turns into the parent file
        }
        match s(v, "/type").unwrap_or("") {
            "custom-title" => self.name = s(v, "/customTitle").map(Into::into),
            "ai-title" => self.auto_title = s(v, "/aiTitle").map(Into::into),
            "summary" if self.auto_title.is_none() => self.auto_title = s(v, "/summary").map(Into::into),
            "user" => {
                let Some(m) = v.get("message") else { return };
                if ts.is_some() {
                    self.last_ts = ts;
                }
                self.messages += 1;
                match m.get("content") {
                    Some(Value::String(t)) => {
                        if t.starts_with("[Request interrupted") {
                            self.last_role = Some("assistant".into());
                            self.last_stop = Some("aborted".into());
                            return;
                        }
                        if v.get("isMeta").and_then(Value::as_bool) == Some(true) {
                            return;
                        }
                        self.note_user_text(t);
                        self.last_role = Some("user".into());
                        self.last_stop = None;
                    }
                    Some(Value::Array(parts)) => {
                        let is_result = parts.iter().any(|p| s(p, "/type") == Some("tool_result"));
                        if is_result {
                            self.last_role = Some("toolResult".into());
                        } else {
                            let text: Vec<&str> = parts.iter().filter_map(|p| s(p, "/text")).collect();
                            let text = text.join("\n");
                            if text.starts_with("[Request interrupted") {
                                self.last_role = Some("assistant".into());
                                self.last_stop = Some("aborted".into());
                                return;
                            }
                            self.note_user_text(&text);
                            self.last_role = Some("user".into());
                            self.last_stop = None;
                        }
                    }
                    _ => {}
                }
            }
            "assistant" => {
                let Some(m) = v.get("message") else { return };
                if ts.is_some() {
                    self.last_ts = ts;
                }
                let mid = s(m, "/id").unwrap_or("").to_string();
                let new_message = mid.is_empty() || self.last_msg_id.as_deref() != Some(mid.as_str());
                if new_message {
                    self.messages += 1;
                    self.turns += 1;
                    self.last_msg_id = Some(mid);
                    self.last_tool = None;
                    if let Some(u) = m.get("usage") {
                        let g = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
                        self.in_tokens += g("input_tokens");
                        self.out_tokens += g("output_tokens");
                        self.cache_read_tokens += g("cache_read_input_tokens");
                        self.cache_write_tokens += g("cache_creation_input_tokens");
                        let ctx = g("input_tokens") + g("cache_creation_input_tokens") + g("cache_read_input_tokens");
                        self.tokens += ctx + g("output_tokens");
                        self.context_tokens = ctx + g("output_tokens");
                    }
                }
                if let Some(model) = s(m, "/model") {
                    if !model.starts_with('<') {
                        self.model = Some(model.into());
                    }
                }
                self.last_role = Some("assistant".into());
                self.last_stop = match s(m, "/stop_reason") {
                    Some("tool_use") => Some("toolUse".into()),
                    Some("end_turn") | Some("stop_sequence") => Some("stop".into()),
                    Some(other) => Some(other.into()),
                    None => self.last_stop.take(),
                };
                for p in m.get("content").and_then(Value::as_array).into_iter().flatten() {
                    match s(p, "/type") {
                        Some("tool_use") => {
                            self.tool_calls += 1;
                            let name = s(p, "/name").unwrap_or("tool").to_string();
                            self.note_call(&name, p.get("input").unwrap_or(&Value::Null));
                            self.last_tool = Some(match self.last_tool.take() {
                                Some(prev) if !prev.split(", ").any(|x| x == name) => format!("{prev}, {name}"),
                                Some(prev) => prev,
                                None => name,
                            });
                            self.last_stop = Some("toolUse".into());
                        }
                        Some("text") => {
                            if let Some(t) = s(p, "/text") {
                                if !t.trim().is_empty() {
                                    self.last_text = Some(clip(t));
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    // ── Codex ─────────────────────────────────────────────────────────────────
    pub(crate) fn ingest_codex(&mut self, v: &Value) {
        let ts = v.get("timestamp").and_then(Value::as_str).and_then(parse_ts);
        let Some(p) = v.get("payload") else { return };
        match s(v, "/type").unwrap_or("") {
            "session_meta" => {
                self.id = s(p, "/id").unwrap_or("").into();
                self.cwd = s(p, "/cwd").unwrap_or("").into();
                self.started = s(p, "/timestamp").and_then(parse_ts).or(ts);
                self.origin = s(p, "/originator").map(Into::into);
                self.entrypoint = s(p, "/source").map(Into::into);
                // a sub-agent thread: not started by you, and naming its parent
                let by_user = matches!(s(p, "/thread_source"), None | Some("user"));
                if !by_user {
                    if let Some(obj) = p.as_object() {
                        self.parent_thread = obj
                            .iter()
                            .filter(|(k, _)| k.contains("parent") || k.contains("spawned_by") || k.contains("forked_from"))
                            .find_map(|(_, v)| v.as_str().map(String::from));
                    }
                }
            }
            "turn_context" => {
                if let Some(m) = s(p, "/model") {
                    self.model = Some(m.into());
                }
                if let Some(e) = s(p, "/effort").or(s(p, "/reasoning_effort")) {
                    self.thinking = Some(e.into());
                }
            }
            "world_state" => {
                if let Some(m) = s(p, "/state/collaboration_mode/model") {
                    self.model.get_or_insert_with(|| m.into());
                }
            }
            "event_msg" => {
                if ts.is_some() {
                    self.last_ts = ts;
                }
                match s(p, "/type").unwrap_or("") {
                    "task_started" => {
                        if let Some(w) = p.get("model_context_window").and_then(Value::as_u64) {
                            self.context_window = Some(w);
                        }
                        self.last_role = Some("user".into());
                        self.last_stop = None;
                    }
                    "task_complete" => {
                        self.approval_requested = false;
                        self.last_role = Some("assistant".into());
                        self.last_stop = Some("stop".into());
                        if let Some(t) = s(p, "/last_agent_message") {
                            self.last_text = Some(clip(t));
                        }
                    }
                    "turn_aborted" => {
                        self.last_role = Some("assistant".into());
                        self.last_stop = Some("aborted".into());
                        self.approval_requested = false;
                    }
                    // exec_approval_request, apply_patch_approval_request, …
                    t if t.ends_with("approval_request") => self.approval_requested = true,
                    t if t.ends_with("_begin") || t.ends_with("_end") => {
                        self.approval_requested = false;
                    }
                    "token_count" => {
                        if let Some(t) = p.pointer("/info/total_token_usage/total_tokens").and_then(Value::as_u64) {
                            self.tokens = t;
                        }
                        if let Some(u) = p.pointer("/info/total_token_usage") {
                            let g = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
                            // Codex counts cached input inside input_tokens
                            self.cache_read_tokens = g("cached_input_tokens");
                            self.in_tokens = g("input_tokens").saturating_sub(self.cache_read_tokens);
                            self.out_tokens = g("output_tokens");
                        }
                        if let Some(t) = p.pointer("/info/last_token_usage/total_tokens").and_then(Value::as_u64) {
                            self.context_tokens = t;
                        }
                        if let Some(w) = p.pointer("/info/model_context_window").and_then(Value::as_u64) {
                            self.context_window = Some(w);
                        }
                    }
                    "thread_name_updated" | "thread_renamed" => {
                        if let Some(n) = s(p, "/thread_name").or(s(p, "/name")) {
                            self.name = Some(n.into());
                        }
                    }
                    _ => {}
                }
            }
            "response_item" => {
                if ts.is_some() {
                    self.last_ts = ts;
                }
                match s(p, "/type").unwrap_or("") {
                    "message" => {
                        let text: Vec<&str> = p
                            .get("content")
                            .and_then(Value::as_array)
                            .into_iter()
                            .flatten()
                            .filter_map(|c| s(c, "/text"))
                            .collect();
                        let text = text.join("\n");
                        match s(p, "/role") {
                            Some("user") => {
                                self.messages += 1;
                                if is_human_text(&text) {
                                    self.note_user_text(&text);
                                }
                            }
                            Some("assistant") => {
                                self.messages += 1;
                                self.turns += 1;
                                if !text.trim().is_empty() {
                                    self.last_text = Some(clip(&text));
                                }
                            }
                            _ => {}
                        }
                    }
                    "function_call" | "custom_tool_call" | "local_shell_call" | "web_search_call" => {
                        self.tool_calls += 1;
                        let name = s(p, "/name").unwrap_or_else(|| s(p, "/type").unwrap_or("tool"));
                        let args = p.get("arguments").or(p.get("input")).or(p.get("action")).cloned().unwrap_or(Value::Null);
                        self.note_call(name, &args);
                        self.note_codex_collab(name, s(p, "/call_id").unwrap_or(""), &args, ts);
                        self.last_tool = Some(name.trim_end_matches("_call").into());
                        self.last_role = Some("assistant".into());
                        self.last_stop = Some("toolUse".into());
                    }
                    "function_call_output" | "custom_tool_call_output" | "local_shell_call_output" => {
                        self.last_role = Some("toolResult".into());
                        self.approval_requested = false;
                        let out = match p.get("output") {
                            Some(Value::String(x)) => x.clone(),
                            Some(o) => o.to_string(),
                            None => String::new(),
                        };
                        self.note_codex_collab_output(s(p, "/call_id").unwrap_or(""), &out, ts);
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Tool;

    #[test]
    fn detects_agents_wherever_they_run() {
        // plain CLI, in any terminal (Ghostty, VS Code, Cursor, Zed, JetBrains, Atom)
        assert_eq!(Tool::from_argv("claude", None, "claude"), Some(Tool::Claude));
        assert_eq!(Tool::from_argv("/opt/homebrew/bin/codex", None, "codex"), Some(Tool::Codex));
        assert_eq!(Tool::from_argv("pi", None, "node"), Some(Tool::Pi));
        // launched through a runtime
        assert_eq!(Tool::from_argv("/usr/local/bin/node", Some("/usr/local/lib/node_modules/@x/claude.js"), "node"), Some(Tool::Claude));
        // editor extensions: Claude Code for VS Code ships its own binary; Codex runs `codex app-server`
        assert_eq!(
            Tool::from_argv("/Users/u/.vscode/extensions/anthropic.claude-code-2.1.0-darwin-arm64/resources/native-binary/claude", Some("--output-format"), "claude"),
            Some(Tool::Claude)
        );
        assert_eq!(
            Tool::from_argv("/Users/u/.cursor/extensions/openai.chatgpt-0.4.0-darwin-arm64/bin/macos-aarch64/codex", Some("app-server"), "codex"),
            Some(Tool::Codex)
        );
        // desktop apps are not terminal sessions
        assert_eq!(Tool::from_argv("/Applications/Claude.app/Contents/MacOS/Claude", None, "Claude"), None);
        assert_eq!(Tool::from_argv("/Applications/Codex.app/Contents/MacOS/codex", None, "codex"), None);
        assert_eq!(Tool::from_argv("zsh", None, "zsh"), None);
    }
}
