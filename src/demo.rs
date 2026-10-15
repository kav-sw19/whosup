//! Made-up sessions, sub-agents and terminals for `whosup --demo` and the README screenshots.
//! Times are relative to now, so the demo always looks current.

use crate::data::{now_ms, Agent, AgentSource, ProcInfo, Session, Snapshot, Transcript};
use crate::providers::Tool;
use crate::terms::{Proc, SessionLink, Terminal};
use std::path::PathBuf;

const HOME: &str = "/Users/you";
const MIN: i64 = 60_000;

fn ago(min: f64) -> i64 {
    now_ms() - (min * MIN as f64) as i64
}

struct S {
    tool: Tool,
    title: &'static str,
    cwd: &'static str,
    model: &'static str,
    thinking: &'static str,
    /// minutes since started / since last activity
    started: f64,
    last: f64,
    /// last_role, last_stop, last_tool
    state: (&'static str, Option<&'static str>, Option<&'static str>),
    turns: u64,
    tools: u64,
    tokens: u64,
    cost: f64,
    prompt: &'static str,
    reply: &'static str,
    /// tty number when live
    tty: Option<u32>,
    host: Option<&'static str>,
}

fn session(i: usize, d: S) -> Session {
    let key = format!("s:demo/{i}.jsonl");
    let t = Transcript {
        id: format!("01a14{i:03}-demo"),
        cwd: format!("{HOME}/{}", d.cwd),
        started: Some(ago(d.started)),
        name: Some(d.title.into()),
        model: Some(d.model.into()),
        thinking: Some(d.thinking.into()),
        messages: d.turns * 2,
        user_msgs: (d.turns / 6).max(1),
        turns: d.turns,
        tool_calls: d.tools,
        cost: d.cost,
        tokens: d.tokens,
        context_tokens: d.tokens / 40,
        // a realistic breakdown, so Claude Code and Codex get cost estimates like real sessions
        in_tokens: d.tokens / 50,
        cache_read_tokens: d.tokens * 9 / 10,
        out_tokens: d.tokens / 100,
        last_ts: Some(ago(d.last)),
        last_role: Some(d.state.0.into()),
        last_stop: d.state.1.map(Into::into),
        last_tool: d.state.2.map(Into::into),
        first_user: Some(d.prompt.into()),
        last_user: Some(d.prompt.into()),
        last_text: Some(d.reply.into()),
        ..Transcript::default()
    };
    let mut t = t;
    crate::cost::fill(&mut t);
    if t.daily.is_empty() {
        let (c, k, e) = (t.cost, t.tokens, t.cost_estimated);
        t.add_day(t.last_ts, c, k, e);
    }
    let live = d.tty.is_some() || d.host.is_some();
    Session {
        shared_files: vec![],
        awaiting_approval: false,
        tool: d.tool,
        host: d.host.map(Into::into).or(d.tty.map(|_| "Ghostty".into())),
        app_live: false,
        key: key.clone(),
        tty: d.tty.map(|n| format!("ttys{n:03}")),
        path: PathBuf::from(format!("{HOME}/.sessions/demo-{i}.jsonl")),
        mtime: ago(d.last),
        proc: live.then(|| ProcInfo {
            tool: d.tool,
            pid: 48_200 + i as u32 * 37,
            start_ms: ago(d.started),
            cwd: t.cwd.clone(),
            cpu: if d.state.0 == "assistant" && d.state.1 == Some("stop") { 0.3 } else { 6.5 },
            mem: (180 + i as u64 * 41) * 1_048_576,
            host: d.host.map(Into::into),
            server: false,
            resume_id: None,
        }),
        t,
        agents: vec![],
    }
}

#[allow(clippy::too_many_arguments)]
fn agent(id: &str, kind: &str, task: &str, model: &str, running: bool, activity: &str, started: f64, took: f64, cost: f64, out: &str) -> Agent {
    Agent {
        key: format!("demo:{id}"),
        id: id.into(),
        agent: kind.into(),
        source: AgentSource::ClaudeTask,
        status: if running { "running".into() } else { "done".into() },
        running,
        task: task.into(),
        model: model.into(),
        effort: "high".into(),
        activity: activity.into(),
        started: Some(ago(started)),
        finished: (!running).then(|| ago(started - took)),
        last_activity: Some(ago(if running { 0.1 } else { started - took })),
        turns: if running { 4 } else { 9 },
        tool_calls: if running { 6 } else { 14 },
        tokens: if running { 38_400 } else { 91_200 },
        cost,
        last_text: Some(out.into()),
        path: None,
    }
}

pub fn snapshot() -> Snapshot {
    let mut sessions = vec![
        session(0, S {
            tool: Tool::Claude,
            title: "Add rate limiting to the public API",
            cwd: "code/api",
            model: "claude-opus-4-8",
            thinking: "high",
            started: 34.0,
            last: 0.05,
            state: ("assistant", Some("toolUse"), Some("Edit")),
            turns: 41,
            tools: 57,
            tokens: 2_140_000,
            cost: 0.0,
            prompt: "Add per-key rate limits to /v1: 100 requests a minute by default, configurable per plan. Return 429 with Retry-After, and add tests.",
            reply: "The limiter is in place as middleware on the /v1 router, backed by the existing Redis client.\n\n**Done so far**\n- `RateLimiter` with a sliding window per API key\n- plan limits read from `plans.toml`\n- 429 responses with `Retry-After`\n\nNow adding tests for the burst and reset cases.",
            tty: Some(2),
            host: None,
        }),
        session(1, S {
            tool: Tool::Pi,
            title: "Migrate auth to OAuth 2.1",
            cwd: "code/web",
            model: "claude-opus-5-5",
            thinking: "medium",
            started: 52.0,
            last: 0.2,
            state: ("toolResult", None, None),
            turns: 63,
            tools: 71,
            tokens: 3_480_000,
            cost: 4.82,
            prompt: "Move login from the old session cookies to OAuth 2.1 with PKCE. Keep existing sessions working until they expire.",
            reply: "Both flows now run side by side: new logins use OAuth 2.1 with PKCE, and old cookies are still accepted until they expire.",
            tty: Some(1),
            host: None,
        }),
        session(2, S {
            tool: Tool::Codex,
            title: "Fix flaky checkout test",
            cwd: "code/shop",
            model: "gpt-6-sol",
            thinking: "medium",
            started: 21.0,
            last: 3.0,
            state: ("assistant", Some("stop"), None),
            turns: 18,
            tools: 22,
            tokens: 610_000,
            cost: 0.0,
            prompt: "checkout.spec.ts fails about one run in ten on CI. Find out why and fix it.",
            reply: "The test raced the payment webhook: it asserted the order status before the webhook handler finished.\n\nI made the test wait for the `order.paid` event instead of sleeping for 500 ms. 200 runs in a row passed locally.\n\nWant me to apply the same fix to `refund.spec.ts`, which has the same pattern?",
            tty: None,
            host: Some("Visual Studio Code"),
        }),
        session(3, S {
            tool: Tool::Claude,
            title: "Write the 2.4 release notes",
            cwd: "code/api",
            model: "claude-sonnet-4-8",
            thinking: "low",
            started: 140.0,
            last: 96.0,
            state: ("assistant", Some("stop"), None),
            turns: 9,
            tools: 6,
            tokens: 184_000,
            cost: 0.0,
            prompt: "Draft release notes for 2.4 from the merged PRs since 2.3.",
            reply: "Here's a draft grouped into new features, fixes and breaking changes.",
            tty: None,
            host: None,
        }),
        session(4, S {
            tool: Tool::Pi,
            title: "Profile the slow dashboard query",
            cwd: "code/web",
            model: "claude-opus-5-5",
            thinking: "high",
            started: 260.0,
            last: 191.0,
            state: ("assistant", Some("stop"), None),
            turns: 27,
            tools: 33,
            tokens: 1_020_000,
            cost: 2.14,
            prompt: "The dashboard takes 4 seconds to load. Find the slow query.",
            reply: "The `recent_orders` query did a full scan; an index on (account_id, created_at) brings it to 40 ms.",
            tty: None,
            host: None,
        }),
    ];
    sessions[0].agents = vec![
        agent("a3f91c0", "Explore", "Find every route under /v1 and how each one authenticates", "claude-opus-4-8", true, "Grep", 2.1, 0.0, 0.0, "Found 23 routes across 6 routers so far."),
        agent("b72d4e1", "Explore", "Check how the Redis client handles reconnects", "claude-haiku-4-5", false, "done", 9.0, 1.05, 0.0, "The client reconnects with backoff; commands during a reconnect fail fast."),
    ];
    sessions[1].agents = vec![
        Agent { source: AgentSource::Chalking, ..agent("scout-1", "scout", "Map where session cookies are read and written", "claude-haiku-5-5", true, "read", 0.6, 0.0, 0.02, "Cookies are read in 4 places: middleware, the API client, SSR and the logout route.") },
        Agent { source: AgentSource::Chalking, ..agent("scout-2", "scout", "List OAuth 2.1 providers we already have keys for", "claude-haiku-5-5", false, "done", 6.0, 0.7, 0.01, "Google and GitHub; Microsoft needs a new app registration.") },
    ];
    sessions[1].agents.iter_mut().for_each(|a| a.cost = if a.running { 0.02 } else { 0.01 });

    let term = |n: u32, app: &str, cwd: &str, fg: Option<(&str, f32, u64)>, started: f64, link: Option<(Tool, &str, &str)>| {
        let foreground = fg
            .map(|(cmd, cpu, mb)| {
                vec![Proc {
                    pid: 48_000 + n * 11,
                    name: cmd.split(' ').next().unwrap_or(cmd).into(),
                    cmdline: cmd.into(),
                    cwd: format!("{HOME}/{cwd}"),
                    start_ms: ago(started),
                    cpu,
                    mem: mb * 1_048_576,
                }]
            })
            .unwrap_or_default();
        Terminal {
            key: format!("t:ttys{n:03}"),
            tty: format!("ttys{n:03}"),
            app: app.into(),
            shell: Some(Proc {
                pid: 47_000 + n,
                name: "zsh".into(),
                cmdline: "-zsh".into(),
                cwd: format!("{HOME}/{cwd}"),
                start_ms: ago(started + 5.0),
                cpu: 0.0,
                mem: 4 * 1_048_576,
            }),
            cpu: foreground.iter().map(|p| p.cpu).sum(),
            mem: foreground.iter().map(|p| p.mem).sum::<u64>() + 6 * 1_048_576,
            foreground,
            jobs: vec![],
            procs: 6,
            started: Some(ago(started + 5.0)),
            session: link.map(|(tool, key, title)| SessionLink { tool, key: Some(key.into()), title: title.into() }),
        }
    };
    let terminals = vec![
        term(1, "Ghostty", "code/web", Some(("pi", 6.5, 412)), 52.0, Some((Tool::Pi, "s:demo/1.jsonl", "Migrate auth to OAuth 2.1"))),
        term(2, "Ghostty", "code/api", Some(("claude", 7.2, 388)), 34.0, Some((Tool::Claude, "s:demo/0.jsonl", "Add rate limiting to the public API"))),
        term(3, "Ghostty", "code/api", Some(("cargo watch -x test", 0.4, 96)), 75.0, None),
        term(4, "Ghostty", "code/web", None, 120.0, None),
        term(5, "Visual Studio Code", "code/shop", Some(("npm run dev", 1.1, 233)), 21.0, None),
    ];
    // the same order the scanner uses: live first (waiting-for-you first), newest first
    sessions.sort_by_key(|s| {
        let live = s.live();
        let key = if live { s.t.started.unwrap_or(0) } else { s.t.last_ts.unwrap_or(0) };
        (!live, !s.needs_you(), std::cmp::Reverse(key))
    });
    // a believable month of spend for the Usage tab: busier on weekdays, a little each day
    let today = crate::data::day_of(now_ms());
    for (i, s) in sessions.iter_mut().enumerate() {
        let est = s.t.cost_estimated || s.tool != Tool::Pi;
        for k in 1..30i64 {
            let weekday = (today - k + 4).rem_euclid(7) < 5; // 1970-01-01 was a Thursday
            let wave = ((k * 7 + i as i64 * 5) % 11) as f64 / 10.0;
            if (k + i as i64) % 4 == 0 && !weekday {
                continue;
            }
            let cost = (if weekday { 1.6 } else { 0.4 }) * (0.3 + wave) * (1.0 + i as f64 * 0.35);
            s.t.daily.insert(today - k, crate::data::Day { cost, tokens: (cost * 380_000.0) as u64, estimated: est, est_cost: if est { cost } else { 0.0 } });
        }
    }
    let port = |port: u16, addrs: &[&str], pid: u32, cmd: &str, cwd: &str, started: f64, mb: u64, tty: Option<u32>, link: Option<(Tool, &str, &str)>| crate::ports::Port {
        key: format!("port:{port}:{pid}"),
        port,
        addrs: addrs.iter().map(|a| a.to_string()).collect(),
        proc: Proc {
            pid,
            name: cmd.split(' ').next().unwrap_or(cmd).into(),
            cmdline: cmd.into(),
            cwd: if cwd.starts_with('/') { cwd.into() } else { format!("{HOME}/{cwd}") },
            start_ms: ago(started),
            cpu: 0.3,
            mem: mb * 1_048_576,
        },
        tty: tty.map(|n| format!("ttys{n:03}")),
        session: link.map(|(tool, key, title)| SessionLink { tool, key: Some(key.into()), title: title.into() }),
    };
    let ports = vec![
        port(3000, &["*"], 51_207, "node next dev", "code/shop", 20.0, 412, Some(5), None),
        port(5173, &["[::1]"], 51_840, "node vite --port 5173", "code/web", 38.0, 188, Some(1), Some((Tool::Pi, "s:demo/1.jsonl", "Migrate auth to OAuth 2.1"))),
        port(8080, &["127.0.0.1"], 52_311, "target/debug/api --port 8080", "code/api", 6.0, 64, Some(2), Some((Tool::Claude, "s:demo/0.jsonl", "Add rate limiting to the public API"))),
        port(9229, &["127.0.0.1"], 52_312, "node --inspect server.js", "code/api", 6.0, 92, Some(2), Some((Tool::Claude, "s:demo/0.jsonl", "Add rate limiting to the public API"))),
        port(4321, &["[::1]"], 50_977, "node astro dev", "code/docs", 190.0, 140, None, None),
        port(5432, &["127.0.0.1", "[::1]"], 812, "postgres -D /Users/demo/Library/Application Support/Postgres/var-16", "/", 2_900.0, 38, None, None),
        port(6379, &["127.0.0.1"], 815, "redis-server 127.0.0.1:6379", "/", 2_900.0, 12, None, None),
        port(57621, &["*"], 952, "Spotify", "/", 3_100.0, 310, None, None),
    ];
    Snapshot { sessions, orphan_procs: vec![], terminals, ports }
}
