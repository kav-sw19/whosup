//! Parsing and discovery against small sample transcripts in each agent's real format
//! (tests/fixtures/). If an agent changes its format, these are the tests that should fail.

use crate::data::{Scanner, Snapshot, TranscriptCache};
use crate::providers::Tool;
use std::path::PathBuf;

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn one(rel: &str, tool: Tool) -> crate::data::Transcript {
    TranscriptCache::default().get(&fixtures().join(rel), tool, false).expect("fixture parses")
}

/// Discovery over the fixture folders only (no time window, so the dates never go stale).
pub fn scan_fixtures() -> Snapshot {
    let mut s = Scanner::new(fixtures().join("pi"), i64::MAX / 4);
    s.claude_dir = fixtures().join("claude");
    s.codex_dir = fixtures().join("codex");
    s.scan()
}

#[test]
fn pi_transcript() {
    let t = one(
        "pi/sessions/--Users-you-code-api--/2026-10-15T09-00-00-000Z_01a1aaaa-0000-7000-8000-000000000001.jsonl",
        Tool::Pi,
    );
    assert_eq!(t.name.as_deref(), Some("Add rate limiting"));
    assert_eq!(t.cwd, "/Users/you/code/api");
    assert_eq!(t.model.as_deref(), Some("claude-opus-5-5"));
    assert_eq!(t.thinking.as_deref(), Some("medium"));
    assert_eq!((t.turns, t.tool_calls, t.user_msgs), (3, 2, 1));
    assert!((t.cost - 0.08).abs() < 1e-9, "cost {}", t.cost);
    assert_eq!(t.activity(), "idle");
    assert!(!t.is_busy());
    assert_eq!(t.last_text.as_deref(), Some("Rate limiting is in place on /v1."));
    assert_eq!(t.first_user.as_deref(), Some("add rate limits to /v1"));
    assert_eq!(t.finished_agents.len(), 1);
    assert_eq!(t.finished_agents[0].status, "done");
}

#[test]
fn claude_transcript() {
    let t = one("claude/projects/-Users-you-code-web/2af7ac15-0000-4000-8000-000000000002.jsonl", Tool::Claude);
    assert_eq!(t.auto_title.as_deref(), Some("Fix login redirect"));
    assert_eq!(t.model.as_deref(), Some("claude-opus-4-8"));
    // two content-block lines of msg_1 count once
    assert_eq!(t.turns, 2);
    assert_eq!(t.tokens, (5000 + 2000 + 10000 + 300) + (10 + 17000 + 40));
    assert_eq!(t.tool_calls, 1);
    assert_eq!(t.last_stop.as_deref(), Some("stop"));
    // the sidechain prompt is not the session's
    assert_eq!(t.last_user.as_deref(), Some("fix the login redirect"));
    assert_eq!(t.entrypoint.as_deref(), Some("cli"));
    assert_eq!(t.last_text.as_deref(), Some("Fixed: the redirect now keeps the return path."));
}

#[test]
fn claude_interrupted_in_vscode() {
    let t = one("claude/projects/-Users-you-code-web/7de5d299-0000-4000-8000-000000000003.jsonl", Tool::Claude);
    assert_eq!(t.last_stop.as_deref(), Some("aborted"));
    assert!(!t.is_busy());
    assert_eq!(t.entrypoint.as_deref(), Some("claude-vscode"));
}

#[test]
fn codex_transcript() {
    let t = one("codex/sessions/2026/10/15/rollout-2026-10-15T12-00-00-01a1cccc-0000-7000-8000-000000000004.jsonl", Tool::Codex);
    assert_eq!(t.model.as_deref(), Some("gpt-6-sol"));
    assert_eq!(t.cwd, "/Users/you/code/shop");
    // harness-injected context is not the user's prompt
    assert_eq!(t.first_user.as_deref(), Some("why does checkout.spec.ts flake?"));
    assert_eq!(t.tokens, 30800);
    assert_eq!(t.tool_calls, 1);
    assert_eq!(t.last_stop.as_deref(), Some("stop"));
    assert_eq!(t.origin.as_deref(), Some("codex_cli_rs"));
    assert!(t.last_text.as_deref().unwrap_or("").contains("order.paid"));
}

#[test]
fn discovery_over_fixture_folders() {
    let snap = scan_fixtures();
    let titles: Vec<String> = snap.sessions.iter().map(|s| s.title()).collect();
    for want in ["Add rate limiting", "Fix login redirect", "refactor the cart", "Fix flaky checkout test"] {
        assert!(titles.iter().any(|t| t == want), "missing {want:?} in {titles:?}");
    }
    // none of these are running
    assert!(snap.sessions.iter().all(|s| !s.live()));
    let pi = snap.sessions.iter().find(|s| s.title() == "Add rate limiting").unwrap();
    assert_eq!(pi.agents.len(), 1);
    assert_eq!((pi.agents[0].id.as_str(), pi.agents[0].status.as_str()), ("scout-1", "done"));
    let claude = snap.sessions.iter().find(|s| s.title() == "Fix login redirect").unwrap();
    assert_eq!(claude.agents.len(), 1);
    assert_eq!((claude.agents[0].agent.as_str(), claude.agents[0].status.as_str()), ("Explore", "done"));
}

#[test]
fn resume_plans() {
    let snap = scan_fixtures();
    let get = |t: &str| snap.sessions.iter().find(|s| s.title() == t).unwrap();
    let pi = crate::resume::plan(get("Add rate limiting"), None).unwrap();
    assert!(pi.command.starts_with("pi --session '"), "{}", pi.command);
    assert_eq!(pi.cwd, "/Users/you/code/api");
    let claude = crate::resume::plan(get("Fix login redirect"), None).unwrap();
    assert_eq!(claude.command, "claude --resume '2af7ac15-0000-4000-8000-000000000002'");
    // started from the VS Code extension: reopen there
    let vscode = crate::resume::plan(get("refactor the cart"), None).unwrap();
    assert_eq!(vscode.place, crate::resume::Place::Editor("Visual Studio Code".into()));
    // where it last ran wins
    let ghostty = crate::resume::plan(get("refactor the cart"), Some("Ghostty")).unwrap();
    assert_eq!(ghostty.place, crate::resume::Place::Terminal("Ghostty".into()));
}

#[test]
fn cost_estimates() {
    let snap = scan_fixtures();
    let get = |t: &str| snap.sessions.iter().find(|s| s.title() == t).unwrap();
    // Claude, opus: 5010 in, 340 out, 27000 cache read, 2000 cache write
    let claude = get("Fix login redirect");
    assert!(claude.t.cost_estimated);
    let want = 5010.0 * 5.0 / 1e6 + 340.0 * 25.0 / 1e6 + 27000.0 * 0.5 / 1e6 + 2000.0 * 6.25 / 1e6;
    assert!((claude.t.cost - want).abs() < 1e-9, "{} vs {want}", claude.t.cost);
    // its sub-agent (haiku) is priced too
    assert!(claude.agents[0].cost > 0.0);
    // Codex: cached input counted at the cache rate
    let codex = get("Fix flaky checkout test");
    let want = 10000.0 * 1.25 / 1e6 + 20000.0 * 0.125 / 1e6 + 800.0 * 10.0 / 1e6;
    assert!((codex.t.cost - want).abs() < 1e-9, "{} vs {want}", codex.t.cost);
    // pi records its own cost: never estimated
    let pi = get("Add rate limiting");
    assert!(!pi.t.cost_estimated);
    assert!((pi.t.cost - 0.08).abs() < 1e-9);
}

#[test]
fn context_use() {
    let snap = scan_fixtures();
    let codex = snap.sessions.iter().find(|s| s.title() == "Fix flaky checkout test").unwrap();
    // Codex reports its own window: 30800 of 258400
    assert_eq!(codex.t.context_window, Some(258_400));
    assert!((codex.context_used().unwrap() - 30800.0 / 258400.0).abs() < 1e-9);
    // Claude Code: last message's input + cache + output, against the model's window
    let claude = snap.sessions.iter().find(|s| s.title() == "Fix login redirect").unwrap();
    assert_eq!(claude.t.context_tokens, 10 + 17000 + 40);
    assert!(claude.context_used().is_some());
}

/// End to end with a stand-in `claude` process (a sleeper built at /tmp/whosup-fake/bin/claude;
/// see the commit that added this). `cargo test --release live_approval -- --ignored`
#[test]
#[ignore]
fn live_approval() {
    use std::process::Command;
    let base = std::env::temp_dir().join(format!("whosup-approval-{}", std::process::id()));
    let project = base.join("project");
    let projects = base.join("claude/projects/p");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::create_dir_all(&projects).unwrap();
    let cwd = project.canonicalize().unwrap().display().to_string();
    let transcript = |at: &str| {
        format!(
            "{{\"type\":\"user\",\"message\":{{\"role\":\"user\",\"content\":\"run the tests\"}},\"timestamp\":\"{at}\",\"cwd\":\"{cwd}\",\"sessionId\":\"live-1\"}}\n\
             {{\"type\":\"assistant\",\"message\":{{\"model\":\"claude-opus-4-8\",\"id\":\"m1\",\"role\":\"assistant\",\"content\":[{{\"type\":\"tool_use\",\"id\":\"t1\",\"name\":\"Bash\",\"input\":{{\"command\":\"npm test\"}}}}],\"stop_reason\":\"tool_use\"}},\"timestamp\":\"{at}\",\"cwd\":\"{cwd}\",\"sessionId\":\"live-1\"}}\n"
        )
    };
    let iso = |ms: i64| {
        let secs = ms / 1000;
        let out = Command::new("date").args(["-u", "-r", &secs.to_string(), "+%Y-%m-%dT%H:%M:%S.000Z"]).output().unwrap();
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    };
    let scan = || {
        let mut s = Scanner::new(base.join("pi"), i64::MAX / 4);
        s.claude_dir = base.join("claude");
        s.codex_dir = base.join("codex");
        s.scan()
    };

    // 1. the agent asked to run Bash and nothing has happened since
    let mut agent = Command::new("/tmp/whosup-fake/bin/claude").arg("30").current_dir(&project).spawn().unwrap();
    std::fs::write(projects.join("live-1.jsonl"), transcript(&iso(crate::data::now_ms()))).unwrap();
    std::thread::sleep(std::time::Duration::from_secs(5));
    let snap = scan();
    let s = snap.sessions.iter().find(|s| s.t.id == "live-1").expect("found");
    assert!(s.live(), "matched to the running claude");
    assert!(s.awaiting_approval, "sitting at the approval prompt");
    assert!(s.needs_you());
    agent.kill().ok();

    // 2. same request, but the agent starts a command for it: running, not waiting
    let mut agent = Command::new("/tmp/whosup-fake/bin/claude").args(["30", "child"]).current_dir(&project).spawn().unwrap();
    std::fs::write(projects.join("live-1.jsonl"), transcript(&iso(crate::data::now_ms()))).unwrap();
    std::thread::sleep(std::time::Duration::from_secs(5));
    let snap = scan();
    let s = snap.sessions.iter().find(|s| s.t.id == "live-1").expect("found");
    assert!(s.live());
    assert!(!s.awaiting_approval, "its command is running");
    agent.kill().ok();
    Command::new("pkill").args(["-f", "/bin/sleep 20"]).status().ok();
    std::fs::remove_dir_all(&base).ok();
}

#[test]
fn codex_approval_events() {
    let dir = std::env::temp_dir().join(format!("whosup-codex-approval-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("rollout.jsonl");
    let head = r#"{"timestamp":"2026-10-15T11:00:00.000Z","type":"session_meta","payload":{"id":"c1","cwd":"/x"}}
{"timestamp":"2026-10-15T11:00:01.000Z","type":"response_item","payload":{"type":"function_call","name":"exec_command","call_id":"c1"}}
{"timestamp":"2026-10-15T11:00:01.100Z","type":"event_msg","payload":{"type":"exec_approval_request","call_id":"c1"}}
"#;
    std::fs::write(&f, head).unwrap();
    let mut cache = TranscriptCache::default();
    assert!(cache.get(&f, Tool::Codex, false).unwrap().approval_requested);
    std::fs::write(&f, format!("{head}{}\n", r#"{"timestamp":"2026-10-15T11:00:05.000Z","type":"event_msg","payload":{"type":"exec_command_begin","call_id":"c1"}}"#)).unwrap();
    assert!(!cache.get(&f, Tool::Codex, false).unwrap().approval_requested, "answered");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn stuck_and_looping() {
    use crate::data::{stuck_reason, Transcript};
    let now = 10_000_000;
    let busy = |stop: Option<&str>, quiet_ms: i64| Transcript {
        last_role: Some(if stop.is_some() { "assistant" } else { "toolResult" }.into()),
        last_stop: stop.map(Into::into),
        last_tool: Some("bash".into()),
        last_ts: Some(now - quiet_ms),
        ..Transcript::default()
    };
    // a tool running for 14 minutes
    assert_eq!(stuck_reason(&busy(Some("toolUse"), 14 * 60_000), true, now, 10 * 60_000).as_deref(), Some("stuck? bash 14m"));
    // thinking for 11 minutes
    assert_eq!(stuck_reason(&busy(None, 11 * 60_000), true, now, 10 * 60_000).as_deref(), Some("stuck? thinking 11m"));
    // not yet, not when idle, not when closed
    assert!(stuck_reason(&busy(Some("toolUse"), 60_000), true, now, 10 * 60_000).is_none());
    assert!(stuck_reason(&busy(Some("toolUse"), 60 * 60_000), false, now, 10 * 60_000).is_none());
    let mut idle = busy(Some("stop"), 60 * 60_000);
    idle.last_role = Some("assistant".into());
    assert!(stuck_reason(&idle, true, now, 10 * 60_000).is_none());

    // the same call three times in a row, from a real transcript format
    let dir = std::env::temp_dir().join(format!("whosup-loop-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("loop.jsonl");
    let call = |i: u32| format!(
        "{{\"type\":\"assistant\",\"message\":{{\"model\":\"claude-opus-4-8\",\"id\":\"m{i}\",\"role\":\"assistant\",\"content\":[{{\"type\":\"tool_use\",\"id\":\"t{i}\",\"name\":\"Read\",\"input\":{{\"file_path\":\"/x/a.rs\"}}}}],\"stop_reason\":\"tool_use\"}},\"timestamp\":\"2026-10-15T10:00:0{i}.000Z\",\"cwd\":\"/x\",\"sessionId\":\"s\"}}\n\
         {{\"type\":\"user\",\"message\":{{\"role\":\"user\",\"content\":[{{\"type\":\"tool_result\",\"tool_use_id\":\"t{i}\",\"content\":\"x\"}}]}},\"timestamp\":\"2026-10-15T10:00:0{i}.500Z\",\"cwd\":\"/x\",\"sessionId\":\"s\"}}\n"
    );
    std::fs::write(&f, (1..=4).map(call).collect::<String>()).unwrap();
    let t = TranscriptCache::default().get(&f, Tool::Claude, false).unwrap();
    assert_eq!(t.call_repeats, 4);
    assert_eq!(stuck_reason(&t, true, crate::data::now_ms(), 10 * 60_000).as_deref(), Some("looping? Read ×4"));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn files_touched() {
    let snap = scan_fixtures();
    let get = |t: &str| snap.sessions.iter().find(|s| s.title() == t).unwrap();
    // pi `edit` with a relative path, resolved against the session's folder (`read` doesn't count)
    let pi: Vec<&str> = get("Add rate limiting").t.files.iter().map(|(f, _)| f.as_str()).collect();
    assert_eq!(pi, vec!["/Users/you/code/api/src/router.rs"]);
    // Claude Code `Edit`
    let claude: Vec<&str> = get("Fix login redirect").t.files.iter().map(|(f, _)| f.as_str()).collect();
    assert_eq!(claude, vec!["/Users/you/code/web/src/login.ts"]);
    // a patch merely mentioned inside some other tool call is not an edit
    let mut t = crate::data::Transcript { cwd: "/x".into(), ..Default::default() };
    t.note_call("bash", &serde_json::json!({"command": "cat > t.rs <<EOF\n*** Begin Patch\n*** Update File: nope.rs\nEOF"}));
    assert!(t.files.is_empty());
    t.note_call("exec_command", &serde_json::json!({"cmd": "apply_patch <<'EOF'\n*** Begin Patch\n*** Update File: yes.rs\n*** End Patch\nEOF"}));
    assert_eq!(t.files.iter().map(|(f, _)| f.as_str()).collect::<Vec<_>>(), vec!["/x/yes.rs"]);

    // Codex apply_patch, as a custom tool call with the patch as its input
    let dir = std::env::temp_dir().join(format!("whosup-patch-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("rollout.jsonl");
    std::fs::write(&f, r#"{"timestamp":"2026-10-15T11:00:00.000Z","type":"session_meta","payload":{"id":"c1","cwd":"/Users/you/code/shop"}}
{"timestamp":"2026-10-15T11:00:01.000Z","type":"response_item","payload":{"type":"custom_tool_call","name":"apply_patch","call_id":"p1","input":"*** Begin Patch\n*** Update File: tests/checkout.spec.ts\n@@\n-a\n+b\n*** Add File: tests/helpers/wait.ts\n+export {}\n*** End Patch"}}
"#).unwrap();
    let t = TranscriptCache::default().get(&f, Tool::Codex, false).unwrap();
    let files: Vec<&str> = t.files.iter().map(|(f, _)| f.as_str()).collect();
    assert_eq!(files, vec!["/Users/you/code/shop/tests/checkout.spec.ts", "/Users/you/code/shop/tests/helpers/wait.ts"]);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn two_live_sessions_editing_one_file() {
    let mut snap = crate::demo::snapshot();
    let now = crate::data::now_ms();
    let live: Vec<usize> = snap.sessions.iter().enumerate().filter(|(_, s)| s.live()).map(|(i, _)| i).take(2).collect();
    snap.sessions[live[0]].t.files = vec![("/Users/you/code/api/src/limits.rs".into(), now - 60_000)];
    snap.sessions[live[1]].t.files = vec![
        ("/Users/you/code/api/src/limits.rs".into(), now - 5_000),
        ("/Users/you/code/api/README.md".into(), now - 5_000),
    ];
    // an edit from two hours ago doesn't count
    let closed = snap.sessions.iter().position(|s| !s.live()).unwrap();
    snap.sessions[closed].t.files = vec![("/Users/you/code/api/src/limits.rs".into(), now)];
    crate::data::mark_shared_files(&mut snap.sessions, now);
    assert_eq!(snap.sessions[live[0]].shared_files.len(), 1);
    assert_eq!(snap.sessions[live[0]].shared_files[0].1, snap.sessions[live[1]].title());
    assert_eq!(snap.sessions[live[1]].shared_files.len(), 1);
    assert!(snap.sessions[closed].shared_files.is_empty(), "closed sessions don't count");
}

#[test]
fn conversations() {
    use crate::convo::{load, Entry};
    let entries = |rel: &str, tool: Tool, side: bool| -> Vec<Entry> {
        load(&fixtures().join(rel), tool, side).into_iter().map(|i| i.entry).collect()
    };
    let pi = entries(
        "pi/sessions/--Users-you-code-api--/2026-10-15T09-00-00-000Z_01a1aaaa-0000-7000-8000-000000000001.jsonl",
        Tool::Pi,
        false,
    );
    assert_eq!(
        pi,
        vec![
            Entry::User("add rate limits to /v1".into()),
            Entry::Call("read".into(), "src/router.rs".into()),
            Entry::Result("fn router() {}".into(), false),
            Entry::Call("edit".into(), "src/router.rs".into()),
            Entry::Result("ok".into(), false),
            Entry::Reply("Rate limiting is in place on /v1.".into()),
        ]
    );
    let claude = entries("claude/projects/-Users-you-code-web/2af7ac15-0000-4000-8000-000000000002.jsonl", Tool::Claude, false);
    assert_eq!(claude.first(), Some(&Entry::User("fix the login redirect".into())));
    assert!(claude.contains(&Entry::Call("Edit".into(), "/Users/you/code/web/src/login.ts".into())));
    assert!(!claude.iter().any(|e| matches!(e, Entry::User(t) if t.contains("sidechain"))), "sub-agent lines stay out");
    assert_eq!(claude.last(), Some(&Entry::Reply("Fixed: the redirect now keeps the return path.".into())));
    let codex = entries(
        "codex/sessions/2026/10/15/rollout-2026-10-15T12-00-00-01a1cccc-0000-7000-8000-000000000004.jsonl",
        Tool::Codex,
        false,
    );
    assert_eq!(
        codex,
        vec![
            Entry::User("why does checkout.spec.ts flake?".into()),
            Entry::Call("exec_command".into(), "npm test".into()),
            Entry::Result("1 failed".into(), false),
            Entry::Reply("It races the payment webhook.".into()),
        ]
    );
}
