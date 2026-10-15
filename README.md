<div align="center">

# pitop

**`top` for your coding agents.**

See every pi, Claude Code and Codex session you have running, what each one is doing right now,
the sub-agents working inside it, and every open terminal, in one live view.

</div>

```
 pitop   1 Sessions 3   2 Terminals 5    ⠹ 2 working   ◆ 3 agents                          $4.18 in live sessions
╭ Sessions 3 live · 9 rows ──────────────────────────────────────╮ ╭ Session claude ──────────────────────────────╮
│                                                                │ │                                              │
│      SESSION                         MODEL      STATUS    TIME │ │ Add rate limiting to the public API          │
│                                                                │ │ ⠹ Edit  ·  in s002  ·  pid 48211  ·  up 12m  │
│▌⠹  ▾ claude Add rate limiting to th  opus-4-8   Edit       now │ │ ~/code/api  ·  claude-opus-4-8               │
│      ├─ ⠹ a3f91c0  Find every route  opus-4-8   Grep        8s │ │                                              │
│      └─ ✓ b72d4e1  Check Redis clie  haiku-4-5  done    1m 02s │ │ 23 turns    31 tools    1.2M tokens          │
│ ⠹  ▾ pi     Migrate auth to OAuth 2  opus-5-5   thinking   12s │ │ 2/3 running                                  │
│      ├─ ⠹ scout-1  Map the session   haiku-5-5  read       31s │ │                                              │
│      └─ ✓ scout-2  List OAuth provi  haiku-5-5  done       44s │ │ LAST PROMPT                                  │
│ ●    codex  Fix flaky checkout  gpt-6-sol  waiting for you  4m │ │ add per-key rate limits to /v1, 429 with     │
│ ○    pi     Write release notes      opus-5-5   closed  1h 20m │ │ Retry-After, and tests                       │
╰────────────────────────────────────────────────────────────────╯ ╰──────────────────────────────────────────────╯
  tab terminals  ·  ↑↓ move  ·  ←→ fold  ·  J K scroll  ·  a recent  ·  f finished  ·  q quit
```

<sub>Illustrative preview. The real thing is in colour: each agent has its own tag colour, and spinners show what's working.</sub>

## Why

Coding agents run for minutes at a time, start sub-agents of their own, and end up spread across
terminal tabs. pitop answers the questions you'd otherwise answer by clicking through every tab:

- **Which sessions are working, and which are waiting for me?**
- **What is each one doing right now?** Thinking, or running `bash`, `Edit`, `Grep`, …
- **Which sub-agents are running inside it**, and what did the finished ones report?
- **What is every open terminal running**, and which one holds that session?

It only reads the files the agents already write and the system's process list. It never talks to
an agent, sends anything over the network, or changes a file.

## Features

- **Every agent in one list.** pi, Claude Code and Codex sessions are read from their transcripts.
  Other agent CLIs (opencode, gemini, aider, amp, goose, crush, qwen, cursor-agent, copilot, droid)
  are found by their process.
- **Live status.** Each session shows *thinking*, the tool it's running, *waiting for you*,
  *interrupted* or *error*, updated every second.
- **Sub-agents nested under their session:** Claude Code Task-tool agents, pi `agent_run` agents
  and pi-subagents background runs, each with its task, model, status, duration and result.
- **A Terminals tab** listing every open terminal: its tty, the app that owns it (Ghostty, iTerm2,
  Terminal, tmux, ssh, …), the foreground command, background jobs, folder, CPU and memory.
  `↵` on a terminal jumps to the agent session inside it.
- **A details pane** with the last prompt and reply (lightly formatted Markdown), turns, tool calls,
  tokens, context size and, for pi, cost.
- **Fast on large histories.** Transcripts are read incrementally, so multi-megabyte sessions cost
  almost nothing after the first read.
- **Scriptable.** `pitop --once` prints a plain-text snapshot instead of opening the UI.

## Install

Requires Rust 1.85 or newer.

```sh
cargo install --git https://github.com/kav-sw19/pitop
```

Or from a clone:

```sh
git clone https://github.com/kav-sw19/pitop && cd pitop
cargo install --path .
```

## Usage

```sh
pitop                      # live view, refreshes every second
pitop --once               # print live sessions and open terminals, then exit
pitop --once --all         # also include sessions closed in the last 24 hours
pitop --recent 72          # with `a`, show sessions closed in the last 72 hours
pitop --dir ~/my-pi-dir    # use a different pi data folder
```

### Keys

| Key | Action |
| --- | --- |
| `tab` · `1` · `2` | Switch between Sessions and Terminals |
| `↑` `↓` · `j` `k` | Move the selection (`PgUp` / `PgDn`, `g` / `G` jump) |
| `←` `→` · `h` `l` | Collapse or expand a session's sub-agents |
| `↵` | On a terminal: open the agent session running in it |
| `J` `K` | Scroll the details pane |
| `a` | Show recently closed sessions as well as live ones |
| `f` | Hide finished sub-agents |
| `r` | Refresh now |
| `q` · `Esc` | Quit |

### Reading the status column

| Status | Meaning |
| --- | --- |
| spinner + **thinking** | Your message or a tool result was sent; the model hasn't replied yet |
| spinner + **bash**, **Edit**, … | The model is waiting for that tool to finish |
| **waiting for you** | The agent finished its reply and is at the prompt |
| **interrupted** | The last reply was cut off (for example, you pressed Esc) |
| **error** | The last reply ended in an error |
| **closed** | The session's process has exited (shown with `a`) |

Sub-agents show ✓ **done**, ✗ **error**, or ◌ **orphaned** when the session that started them is gone.

## Supported agents

| Agent | Detected by | What you see | Data read |
| --- | --- | --- | --- |
| **pi** | process + transcript | Everything, including cost and sub-agents | `~/.pi/agent/sessions/` (`$PI_CODING_AGENT_DIR`) |
| **Claude Code** | process + transcript | Title, model, status, tokens, prompt and reply, Task-tool sub-agents | `~/.claude/projects/` (`$CLAUDE_CONFIG_DIR`) |
| **Codex** | process or desktop app + transcript | Thread name, model, status, tokens, prompt and reply | `~/.codex/sessions/` (`$CODEX_HOME`) |
| opencode, gemini, aider, amp, goose, crush, qwen, cursor-agent, copilot, droid | process only | Folder, uptime, CPU, memory, terminal | — |

Only pi records cost in its transcript; Claude Code and Codex sessions show token counts instead.

## How it works

1. **Find agent processes.** pitop scans the process list and recognises agent CLIs by their command
   name, including ones launched through `node` or `bun`.
2. **Read transcripts.** It reads the JSONL transcripts each agent writes, and each format is mapped
   onto one common summary: title, model, turns, tool calls, tokens and the last thing that happened.
   Files are read incrementally from where the last read stopped.
3. **Match processes to sessions.** A running process is matched to a transcript from the same agent
   in the same folder, first by start time, then by the most recently written file. This also covers
   resumed sessions.
4. **Find terminals.** `ps` gives every tty and its foreground process group; walking each tty's
   parent processes finds the app that owns it.

Codex desktop-app threads have no process of their own, so they count as live while Codex.app is
running and the thread was written to in the last 10 minutes (if working) or 30 minutes (if idle).

## Platform support

Built and tested on **macOS**. The process and terminal scanning uses `ps` and
[`sysinfo`](https://crates.io/crates/sysinfo), so Linux should mostly work, but it hasn't been
tested there. Windows is not supported.

## Limitations

- CPU is measured between refreshes, so it reads 0% in `--once` and for the live view's first second.
- When several sessions of the same agent are resumed in the same folder, the process-to-session
  match can pick the wrong one.
- opencode keeps its history in SQLite, so it (like the other process-only agents) shows no prompt,
  reply or status yet.
- tmux panes show `tmux` as their app rather than the terminal window they're displayed in.
- Text you've typed into an agent but not sent isn't visible, so that session still shows
  *waiting for you*.

## Development

```sh
cargo build --release
cargo test

# render both tabs from your real data to stdout (and colour previews to /tmp/pitop-*.html)
cargo test --release render -- --ignored --nocapture
```

| File | What it does |
| --- | --- |
| `src/main.rs` | Arguments, app state, keys and the refresh loop |
| `src/data.rs` | Session discovery, incremental transcript cache, process matching, sub-agents |
| `src/providers.rs` | Agent detection, plus the Claude Code and Codex transcript parsers |
| `src/terms.rs` | Terminal discovery: ttys, foreground commands, jobs, owning app |
| `src/ui.rs` | Rendering: palette, tabs, lists, details pane |

Adding an agent with a JSONL transcript means adding it to `Tool`, writing an `ingest_*` parser in
`providers.rs` and listing its transcript folder in `Scanner::transcript_files`.
