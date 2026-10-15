# pitop

A terminal UI with two tabs:

- **Sessions**: every coding-agent session — **pi**, **Claude Code** and **Codex** (read from their
  transcripts), plus any other agent CLI found running (opencode, gemini, aider, amp, goose, crush,
  qwen, cursor-agent, copilot, droid) — with the sub-agents running inside each.
- **Terminals**: every open terminal and what it is running, linked to the agent session inside it.

```
cargo install --path .
pitop              # live TUI (refreshes every second)
pitop --once       # plain-text snapshot of live sessions and terminals
pitop --once --all # include sessions closed within --recent hours (default 24)
```

Keys: `tab` (or `1`/`2`) switch Sessions / Terminals · `↵` on a terminal opens the agent session in it ·
`↑↓`/`jk` move · `←→`/`hl` collapse/expand a session · `a` show recent closed sessions ·
`f` hide finished agents · `J`/`K` scroll the details pane · `g`/`G` top/bottom · `r` refresh · `q` quit.

## Where the data comes from (read-only)

| What | Source |
|---|---|
| Live sessions | agent processes (sysinfo, recognised by argv), matched to transcripts of the same agent by cwd + start time |
| pi | `~/.pi/agent/sessions/<cwd>/<ts>_<id>.jsonl` (`$PI_CODING_AGENT_DIR`) |
| pi sub-agents | `<session>.subagents/*.jsonl` + `chalking-subagent` records; `$TMPDIR/pi-subagents-*/async-subagent-runs/*/status.json` |
| Claude Code | `~/.claude/projects/<cwd>/<uuid>.jsonl` (`$CLAUDE_CONFIG_DIR`); titles from `custom-title` / `ai-title` |
| Claude Code sub-agents | `<uuid>/subagents/agent-*.jsonl` + `.meta.json` (Task tool) |
| Codex | `~/.codex/sessions/Y/M/D/rollout-*.jsonl` (`$CODEX_HOME`); titles from `session_index.jsonl`. Desktop-app threads count as live while Codex.app runs and the thread was touched in the last 10–30 min |
| Other agents | process only (no transcript support yet) |
| Open terminals | every tty from `ps` (foreground group via `tpgid`, background jobs, owning app found by walking parents), cwd/cpu/memory from sysinfo |

Only pi records cost in its transcript, so cost is shown for pi sessions only; Claude Code and Codex
show token counts.
