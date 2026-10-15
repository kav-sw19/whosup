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
