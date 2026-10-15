mod cost;
mod data;
mod models;
#[cfg(test)]
mod fixture_tests;
mod demo;
mod resume;
mod settings;
mod focus;
mod providers;
mod terms;
mod ui;

use data::{Scanner, Session, Snapshot};
use ratatui::{
    crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    widgets::TableState,
    DefaultTerminal,
};
use ui::{home_rel, one_line, short_model};
use std::{
    collections::HashSet,
    sync::mpsc,
    path::PathBuf,
    time::{Duration, Instant},
};

const HELP: &str = "whosup — live view of coding-agent sessions (pi, Claude Code, Codex, …), their sub-agents and open terminals

USAGE: whosup [--dir <pi agent dir>] [--recent <hours>] [--once [--all]] [--focus <tty>]
              [--resume <text>]

  --dir      pi agent dir (default: $PI_CODING_AGENT_DIR or ~/.pi/agent); Claude Code and Codex
             use $CLAUDE_CONFIG_DIR (~/.claude) and $CODEX_HOME (~/.codex)
  --recent   also list sessions touched in the last N hours (default 24; toggle with `a`)
  --once     print a plain-text snapshot and exit (--all: include closed recent sessions)
  --demo     show made-up sessions and terminals (for trying whosup out, or screenshots)

KEYS: / search · tab/1/2 sessions·terminals · ↑/↓ j/k move · ↵ go to the session's terminal tab
      ←/→ h/l fold · s (terminals) show its session · J/K scroll details
      a history: live only → recent → 7 days → all
      f hide finished agents · t light/dark theme · g/G top/bottom · r refresh · q quit";

#[derive(Clone)]
pub enum RowRef {
    Session(usize),
    Agent(usize, usize),
    Orphan(usize),
    Terminal(usize),
    Job(usize, usize),
    /// Non-selectable separator between live and recently closed sessions.
    Divider,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum View {
    Sessions,
    Terminals,
}

pub enum Target {
    Tty(String),
    App(String),
    /// a closed session: run its resume command
    Resume(resume::Plan),
}

/// What a live session was doing at the last refresh, for notifications.
#[derive(Clone, Copy, PartialEq)]
enum Phase {
    Busy,
    NeedsYou,
    Error,
    Approval,
    Stuck,
    Other,
}

/// A short message shown in place of the key hints.
pub struct Flash {
    pub text: String,
    pub error: bool,
    pub at: Instant,
}

pub struct App {
    view: View,
    pub flash: Option<Flash>,
    focus_tx: mpsc::Sender<Result<String, String>>,
    focus_rx: mpsc::Receiver<Result<String, String>>,
    /// selection remembered per view
    saved_sel: [Option<String>; 2],
    scanner: Scanner,
    snap: Snapshot,
    show_recent: bool,
    hide_finished: bool,
    /// live sessions the user folded
    collapsed: HashSet<String>,
    /// closed sessions the user unfolded (closed sessions start folded)
    expanded: HashSet<String>,
    rows: Vec<(String, RowRef)>,
    state: TableState,
    selected_key: Option<String>,
    detail_scroll: u16,
    pub settings: settings::Settings,
    pub hosts: settings::Hosts,
    /// each live session's phase at the last refresh (None until the first refresh)
    phases: Option<std::collections::HashMap<String, Phase>>,
    /// show made-up data (`--demo`) instead of scanning
    demo: bool,
    /// how far back closed sessions go (`a` cycles)
    pub history: History,
    /// the configured recent window, in ms (config `recent_hours` or `--recent`)
    base_window_ms: i64,
    /// refresh right after the next draw (so a slow load shows its message first)
    refresh_soon: bool,
    /// search text (`/`); every word must match
    pub query: String,
    /// typing into the search box
    pub searching: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum History {
    Live,
    Recent,
    Week,
    All,
}

impl History {
    fn next(self) -> Self {
        match self {
            History::Live => History::Recent,
            History::Recent => History::Week,
            History::Week => History::All,
            History::All => History::Live,
        }
    }

    pub fn label(self, base_ms: i64) -> String {
        match self {
            History::Live => "live only".into(),
            History::Recent => {
                let h = base_ms as f64 / 3_600_000.0;
                if h >= 48.0 && h % 24.0 == 0.0 { format!("{} days", h / 24.0) } else { format!("{h}h") }
            }
            History::Week => "7 days".into(),
            History::All => "all history".into(),
        }
    }
}

/// Does every word of the query appear in what we know about the session?
pub fn session_matches(s: &Session, words: &[String]) -> bool {
    if words.is_empty() {
        return true;
    }
    let mut hay = vec![
        s.title(),
        s.t.cwd.clone(),
        s.tool.label().to_string(),
        s.t.model.clone().unwrap_or_default(),
        s.t.first_user.clone().unwrap_or_default(),
        s.tty.clone().unwrap_or_default(),
        s.host.clone().unwrap_or_default(),
        // what the list shows (VS Code, s001)
        s.host.as_deref().map(ui::short_app).unwrap_or_default(),
        s.tty.as_deref().map(|t| t.trim_start_matches("tty").to_string()).unwrap_or_default(),
    ];
    for a in &s.agents {
        hay.push(format!("{} {} {}", a.id, a.agent, a.task));
    }
    let hay = hay.join("\n").to_lowercase();
    words.iter().all(|w| hay.contains(w.as_str()))
}

pub fn terminal_matches(t: &terms::Terminal, words: &[String]) -> bool {
    if words.is_empty() {
        return true;
    }
    let mut hay = vec![t.tty.clone(), t.short_tty(), t.app.clone(), ui::short_app(&t.app), t.cwd()];
    hay.extend(t.foreground.iter().map(|p| p.cmdline.clone()));
    hay.extend(t.jobs.iter().map(|p| p.cmdline.clone()));
    if let Some(l) = &t.session {
        hay.push(format!("{} {}", l.tool.label(), l.title));
    }
    let hay = hay.join("\n").to_lowercase();
    words.iter().all(|w| hay.contains(w.as_str()))
}

impl App {
    fn new(scanner: Scanner) -> Self {
        let mut a = Self::new_inner(scanner, false);
        a.refresh();
        a
    }

    /// `a`: live → recent → 7 days → all history → live.
    pub fn cycle_history(&mut self) {
        self.history = self.history.next();
        self.show_recent = self.history != History::Live;
        self.scanner.recent_window_ms = match self.history {
            History::Live | History::Recent => self.base_window_ms,
            History::Week => self.base_window_ms.max(7 * 24 * 3_600_000),
            History::All => i64::MAX / 4,
        };
        if self.history == History::All {
            self.flash("Loading all history…", false);
        }
        self.refresh_soon = true;
        self.rebuild_rows();
    }

    fn new_inner(scanner: Scanner, demo: bool) -> Self {
        let base_window_ms = scanner.recent_window_ms;
        let (focus_tx, focus_rx) = mpsc::channel();
        let a = Self {
            view: View::Sessions,
            flash: None,
            focus_tx,
            focus_rx,
            saved_sel: [None, None],
            scanner,
            snap: Snapshot { sessions: vec![], orphan_procs: vec![], terminals: vec![] },
            show_recent: false,
            hide_finished: false,
            collapsed: HashSet::new(),
            expanded: HashSet::new(),
            rows: vec![],
            state: TableState::default(),
            selected_key: None,
            detail_scroll: 0,
            settings: settings::Settings::load(),
            hosts: settings::Hosts::load(),
            phases: None,
            demo,
            history: History::Live,
            base_window_ms,
            refresh_soon: false,
            query: String::new(),
            searching: false,
        };
        ui::set_light(a.settings.light);
        a
    }

    /// The window the configured "recent" covers, for labels.
    pub fn scanner_window(&self) -> i64 {
        self.base_window_ms
    }

    pub fn query_words(&self) -> Vec<String> {
        self.query.to_lowercase().split_whitespace().map(String::from).collect()
    }

    fn visible_sessions(&self) -> impl Iterator<Item = (usize, &Session)> {
        let words = self.query_words();
        // a search looks through closed sessions too
        let show_recent = self.show_recent || !words.is_empty();
        let settings = &self.settings;
        self.snap
            .sessions
            .iter()
            .enumerate()
            .filter(move |(_, s)| !settings.hides(s.tool.label()))
            .filter(move |(_, s)| show_recent || s.live() || s.running_agents() > 0)
            .filter(move |(_, s)| session_matches(s, &words))
    }

    fn rebuild_rows(&mut self) {
        let mut rows = Vec::new();
        if self.view == View::Terminals {
            let words = self.query_words();
            for (i, t) in self.snap.terminals.iter().enumerate() {
                if !terminal_matches(t, &words) {
                    continue;
                }
                rows.push((t.key.clone(), RowRef::Terminal(i)));
                for (j, job) in t.jobs.iter().enumerate() {
                    rows.push((format!("{}:job:{}", t.key, job.pid), RowRef::Job(i, j)));
                }
            }
            return self.finish_rows(rows);
        }
        let mut divided = false;
        let visible: Vec<usize> = self.visible_sessions().map(|(i, _)| i).collect();
        for i in visible {
            let s = &self.snap.sessions[i];
            if !divided && !Self::open_by_default(s) {
                if !rows.is_empty() {
                    rows.push(("div:recent".to_string(), RowRef::Divider));
                }
                divided = true;
            }
            rows.push((s.key.clone(), RowRef::Session(i)));
            if self.is_collapsed(s) {
                continue;
            }
            for (j, a) in s.agents.iter().enumerate() {
                if self.hide_finished && !a.running {
                    continue;
                }
                rows.push((a.key.clone(), RowRef::Agent(i, j)));
            }
        }
        for (k, p) in self.snap.orphan_procs.iter().enumerate() {
            if !self.settings.hides(p.tool.label()) {
                rows.push((format!("p:{}", p.pid), RowRef::Orphan(k)));
            }
        }
        self.finish_rows(rows);
    }

    fn open_by_default(s: &Session) -> bool {
        s.live() || s.running_agents() > 0
    }

    pub fn is_collapsed(&self, s: &Session) -> bool {
        if Self::open_by_default(s) { self.collapsed.contains(&s.key) } else { !self.expanded.contains(&s.key) }
    }

    fn finish_rows(&mut self, rows: Vec<(String, RowRef)>) {
        self.rows = rows;
        let mut idx = self
            .selected_key
            .as_ref()
            .and_then(|k| self.rows.iter().position(|(rk, _)| rk == k))
            .unwrap_or_else(|| self.state.selected().unwrap_or(0).min(self.rows.len().saturating_sub(1)));
        if matches!(self.rows.get(idx), Some((_, RowRef::Divider))) {
            idx += 1;
        }
        self.state.select(if self.rows.is_empty() { None } else { Some(idx) });
        self.selected_key = self.state.selected().map(|i| self.rows[i].0.clone());
    }

    fn new_demo(scanner: Scanner) -> Self {
        let mut a = Self::new_inner(scanner, true);
        a.refresh();
        a
    }

    fn refresh(&mut self) {
        if self.demo {
            self.snap = demo::snapshot();
            self.rebuild_rows();
            return;
        }
        self.snap = self.scanner.scan();
        self.remember_hosts();
        self.notify_changes();
        self.rebuild_rows();
    }

    /// Remember the app each live session runs in, so it can be resumed there once closed.
    fn remember_hosts(&mut self) {
        for s in self.snap.sessions.iter().filter(|s| s.live() && !s.path.as_os_str().is_empty()) {
            let app = match &s.tty {
                Some(tty) => self.snap.terminals.iter().find(|t| &t.tty == tty).map(|t| t.app.clone()),
                None => s.host.clone(),
            };
            if let Some(app) = app.filter(|a| !matches!(a.as_str(), "" | "editor" | "terminal" | "ssh" | "screen" | "zellij")) {
                self.hosts.record(&s.key, &app);
            }
        }
        self.hosts.flush();
    }

    /// Notify (when enabled) as a live session finishes its turn or hits an error.
    fn notify_changes(&mut self) {
        let phase = |s: &Session| {
            if s.awaiting_approval {
                Phase::Approval
            } else if s.stuck().is_some() {
                Phase::Stuck
            } else if s.t.is_busy() {
                Phase::Busy
            } else if s.needs_you() && s.t.last_stop.as_deref() == Some("error") {
                Phase::Error
            } else if s.needs_you() {
                Phase::NeedsYou
            } else {
                Phase::Other
            }
        };
        let now: std::collections::HashMap<String, Phase> =
            self.snap.sessions.iter().filter(|s| s.live()).map(|s| (s.key.clone(), phase(s))).collect();
        if let (Some(before), true) = (&self.phases, self.settings.notify) {
            for s in self.snap.sessions.iter().filter(|s| s.live()) {
                let (was, is) = (before.get(&s.key).copied(), now[&s.key]);
                if was != Some(Phase::Busy) {
                    continue;
                }
                let place = s.tty.as_deref().map(|t| format!(" · {}", t.trim_start_matches("tty"))).unwrap_or_default();
                let subtitle = format!("{}{place}", s.tool.label());
                match is {
                    Phase::Approval => notify(
                        &format!("Approve {}?", s.t.last_tool.as_deref().unwrap_or("a tool call")),
                        &subtitle,
                        &s.title(),
                    ),
                    Phase::NeedsYou => notify("Waiting for you", &subtitle, &s.title()),
                    Phase::Stuck => notify(
                        &format!("Looks stuck: {}", s.stuck().unwrap_or_default().trim_start_matches(|c| c != ' ').trim()),
                        &subtitle,
                        &s.title(),
                    ),
                    Phase::Error => notify("Stopped with an error", &subtitle, &s.title()),
                    _ => {}
                }
            }
        }
        self.phases = Some(now);
    }

    fn move_sel(&mut self, d: isize) {
        if self.rows.is_empty() {
            return;
        }
        let cur = self.state.selected().unwrap_or(0) as isize;
        let last = self.rows.len() as isize - 1;
        let mut n = (cur + d).clamp(0, last);
        // never rest on the divider: keep going the same way, or back off at the ends
        if matches!(self.rows[n as usize].1, RowRef::Divider) {
            let step = if d >= 0 { 1 } else { -1 };
            n = if (0..=last).contains(&(n + step)) { n + step } else { n - step };
        }
        let n = n as usize;
        if self.state.selected() != Some(n) {
            self.detail_scroll = 0;
        }
        self.state.select(Some(n));
        self.selected_key = Some(self.rows[n].0.clone());
    }

    fn selected(&self) -> Option<&RowRef> {
        self.state.selected().and_then(|i| self.rows.get(i)).map(|(_, r)| r)
    }

    fn set_view(&mut self, v: View) {
        if v == self.view {
            return;
        }
        self.saved_sel[self.view as usize] = self.selected_key.take();
        self.view = v;
        self.selected_key = self.saved_sel[v as usize].clone();
        self.state.select(Some(0));
        self.detail_scroll = 0;
        self.rebuild_rows();
    }

    fn flash(&mut self, text: impl Into<String>, error: bool) {
        self.flash = Some(Flash { text: text.into(), error, at: Instant::now() });
    }

    /// Where Enter should take you: a terminal tab, or (no tty) the app the session runs in.
    fn selected_target(&self) -> Result<Target, String> {
        // a closed session (or a sub-agent of one): resume it
        if let Some(RowRef::Session(i)) | Some(RowRef::Agent(i, _)) = self.selected() {
            let s = &self.snap.sessions[*i];
            if !s.live() && !s.path.as_os_str().is_empty() {
                return resume::plan(s, self.hosts.get(&s.key).map(String::as_str)).map(Target::Resume);
            }
        }
        match self.selected_tty() {
            Ok(tty) => Ok(Target::Tty(tty)),
            Err(e) => {
                let host = match self.selected() {
                    Some(RowRef::Session(i)) | Some(RowRef::Agent(i, _)) => {
                        let s = &self.snap.sessions[*i];
                        s.live().then(|| s.host.clone()).flatten()
                    }
                    Some(RowRef::Orphan(k)) => self.snap.orphan_procs[*k].host.clone(),
                    _ => None,
                };
                match host.as_deref() {
                    Some("editor") | None => Err(e),
                    Some(app) => Ok(Target::App(app.to_string())),
                }
            }
        }
    }

    /// The tty of whatever is selected: a terminal, or the terminal a session runs in.
    fn selected_tty(&self) -> Result<String, String> {
        let term_of_pid = |pid: u32| {
            self.snap.terminals.iter().find(|t| t.foreground.iter().any(|p| p.pid == pid)).map(|t| t.tty.clone())
        };
        match self.selected() {
            Some(RowRef::Terminal(i)) | Some(RowRef::Job(i, _)) => Ok(self.snap.terminals[*i].tty.clone()),
            Some(RowRef::Session(i)) | Some(RowRef::Agent(i, _)) => {
                let s = &self.snap.sessions[*i];
                if let Some(tty) = &s.tty {
                    return Ok(tty.clone());
                }
                if s.app_live {
                    return Err(format!("This {} thread runs in an app, not a terminal", s.tool.label()));
                }
                if !s.live() {
                    return Err("This session is closed".into());
                }
                Err("This session isn't running in a terminal whosup can see".into())
            }
            Some(RowRef::Orphan(k)) => {
                term_of_pid(self.snap.orphan_procs[*k].pid).ok_or_else(|| "This process has no terminal".into())
            }
            _ => Err("Nothing selected".into()),
        }
    }

    /// Enter: bring the selected session's (or terminal's) tab to the front.
    fn activate(&mut self) {
        let target = match self.selected_target() {
            Ok(t) => t,
            Err(e) => return self.flash(e, true),
        };
        let tx = self.focus_tx.clone();
        match target {
            Target::Tty(tty) => {
                let apps: focus::AppByTty = self.snap.terminals.iter().map(|t| (t.tty.clone(), t.app.clone())).collect();
                let app = apps.get(&tty).cloned().unwrap_or_default();
                self.flash(format!("Switching to {} in {app}…", tty.trim_start_matches("tty")), false);
                std::thread::spawn(move || {
                    let _ = tx.send(focus::focus(&tty, &app, &apps));
                });
            }
            Target::App(app) => {
                self.flash(format!("Bringing {app} to the front…"), false);
                std::thread::spawn(move || {
                    let _ = tx.send(focus::activate_app(&app));
                });
            }
            Target::Resume(plan) => {
                if let Some(RowRef::Session(i)) | Some(RowRef::Agent(i, _)) = self.selected() {
                    let s = &self.snap.sessions[*i];
                    if matches!(plan.place, resume::Place::Terminal(_)) {
                        data::expect_resume(&s.key, s.tool, &s.t.cwd);
                    }
                }
                self.flash(format!("Resuming in {}…", plan.place.describe()), false);
                std::thread::spawn(move || {
                    let _ = tx.send(resume::launch(&plan));
                });
            }
        }
    }

    /// From a terminal row, show the agent session running in it.
    fn show_session(&mut self) {
        let Some(RowRef::Terminal(i)) = self.selected() else { return };
        match self.snap.terminals[*i].session.as_ref().and_then(|l| l.key.clone()) {
            Some(key) => {
                self.set_view(View::Sessions);
                self.selected_key = Some(key);
                self.rebuild_rows();
            }
            None => self.flash("No agent session in this terminal", true),
        }
    }

    fn poll_focus(&mut self) {
        while let Ok(r) = self.focus_rx.try_recv() {
            match r {
                Ok(m) => self.flash(m, false),
                Err(e) => self.flash(e, true),
            }
        }
    }

    fn set_collapsed(&mut self, collapse: bool) {
        let si = match self.selected() {
            Some(RowRef::Session(i)) | Some(RowRef::Agent(i, _)) => *i,
            _ => return,
        };
        let s = &self.snap.sessions[si];
        let key = s.key.clone();
        let default_open = Self::open_by_default(s);
        match (collapse, default_open) {
            (true, true) => {
                self.collapsed.insert(key.clone());
            }
            (true, false) => {
                self.expanded.remove(&key);
            }
            (false, true) => {
                self.collapsed.remove(&key);
            }
            (false, false) => {
                self.expanded.insert(key.clone());
            }
        }
        if collapse {
            self.selected_key = Some(key);
        }
        self.rebuild_rows();
    }
}

fn print_once(snap: &Snapshot, all: bool) {
    for s in snap.sessions.iter().filter(|s| all || s.live() || s.running_agents() > 0) {
        let state = match (&s.proc, s.app_live) {
            (Some(p), _) => format!("pid {}", p.pid),
            (None, true) => "app".into(),
            (None, false) => "closed".into(),
        };
        println!(
            "{} {:<6} {}  [{}]  {}  {}  {}  {}",
            if s.live() { "●" } else { "○" },
            s.tool.label(),
            one_line(&s.title(), 50),
            state,
            home_rel(&s.t.cwd),
            short_model(s.t.model.as_deref().unwrap_or("")),
            if s.live() { s.t.activity() } else { "closed".into() },
            ui::cost_str(s.t.cost, s.t.cost_estimated)
        );
        for a in &s.agents {
            println!(
                "    {} {:<10} {:<8} {:<10} {:<14} {}",
                if a.running { "◆" } else { "◇" },
                a.id,
                a.agent,
                a.status,
                short_model(&a.model),
                one_line(&a.task, 60)
            );
        }
    }
    for p in &snap.orphan_procs {
        println!(
            "● {:<6} pid {} (no transcript) {}  in {}",
            p.tool.label(),
            p.pid,
            home_rel(&p.cwd),
            p.host.as_deref().unwrap_or("?")
        );
    }
    println!();
    for t in &snap.terminals {
        let what = match (&t.session, t.leader()) {
            (Some(l), _) => format!("{} · {}", l.tool.label(), one_line(&l.title, 50)),
            (None, Some(p)) => one_line(&p.cmdline, 60),
            (None, None) => format!("{} (prompt)", t.shell.as_ref().map(|s| s.name.as_str()).unwrap_or("shell")),
        };
        println!("{} {:<5} {:<9} {:<55} {}", if t.at_prompt() { "❯" } else { "▶" }, t.short_tty(), t.app, what, home_rel(&t.cwd()));
        for j in &t.jobs {
            println!("    & {} (pid {})", one_line(&j.cmdline, 60), j.pid);
        }
    }
}

/// A macOS notification (in the background, so the UI never waits on it).
fn notify(title: &str, subtitle: &str, body: &str) {
    let q = |s: &str| format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""));
    let script = format!(
        "display notification {} with title {} subtitle {} sound name \"Glass\"",
        q(body),
        q(&format!("whosup · {title}")),
        q(subtitle)
    );
    std::thread::spawn(move || {
        let _ = std::process::Command::new("osascript").arg("-e").arg(script).output();
    });
}

fn run(term: &mut DefaultTerminal, mut app: App) -> std::io::Result<()> {
    let data_tick = Duration::from_millis(app.settings.refresh_ms);
    let frame = Duration::from_millis(80); // spinner cadence
    let mut last = Instant::now();
    loop {
        app.poll_focus();
        term.draw(|f| ui::draw(f, &mut app))?;
        if app.refresh_soon {
            app.refresh_soon = false;
            app.refresh();
            if app.history == History::All {
                let n = app.snap.sessions.len();
                app.flash(format!("All history: {n} sessions"), false);
            }
            last = Instant::now();
            continue;
        }
        if event::poll(frame)? {
            if let Event::Key(k) = event::read()? {
                if k.kind != KeyEventKind::Press {
                    continue;
                }
                // typing into the search box
                if app.searching {
                    match k.code {
                        KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => return Ok(()),
                        KeyCode::Char(c) => app.query.push(c),
                        KeyCode::Backspace => {
                            app.query.pop();
                        }
                        KeyCode::Enter | KeyCode::Down | KeyCode::Up => app.searching = false,
                        KeyCode::Esc => {
                            app.searching = false;
                            app.query.clear();
                        }
                        _ => {}
                    }
                    app.rebuild_rows();
                    continue;
                }
                match k.code {
                    KeyCode::Char('/') => {
                        app.searching = true;
                    }
                    // Esc clears a search first, then quits
                    KeyCode::Esc if !app.query.is_empty() => {
                        app.query.clear();
                        app.rebuild_rows();
                    }
                    KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                    KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => return Ok(()),
                    KeyCode::Down | KeyCode::Char('j') => app.move_sel(1),
                    KeyCode::Up | KeyCode::Char('k') => app.move_sel(-1),
                    KeyCode::PageDown => app.move_sel(10),
                    KeyCode::PageUp => app.move_sel(-10),
                    KeyCode::Char('g') | KeyCode::Home => app.move_sel(-100_000),
                    KeyCode::Char('G') | KeyCode::End => app.move_sel(100_000),
                    KeyCode::Left | KeyCode::Char('h') => app.set_collapsed(true),
                    KeyCode::Right | KeyCode::Char('l') => app.set_collapsed(false),
                    KeyCode::Enter => app.activate(),
                    KeyCode::Char('s') if app.view == View::Terminals => app.show_session(),
                    KeyCode::Tab | KeyCode::BackTab => {
                        let v = if app.view == View::Sessions { View::Terminals } else { View::Sessions };
                        app.set_view(v)
                    }
                    KeyCode::Char('1') => app.set_view(View::Sessions),
                    KeyCode::Char('2') => app.set_view(View::Terminals),
                    KeyCode::Char('a') => app.cycle_history(),
                    KeyCode::Char('f') => {
                        app.hide_finished = !app.hide_finished;
                        app.rebuild_rows();
                    }
                    KeyCode::Char('r') => app.refresh(),
                    KeyCode::Char('t') => {
                        app.settings.light = !app.settings.light;
                        ui::set_light(app.settings.light);
                        let name = if app.settings.light { "Light theme" } else { "Dark theme" };
                        match app.settings.save() {
                            Ok(()) => app.flash(name, false),
                            Err(e) => app.flash(format!("{name} (couldn't save it: {e})"), true),
                        }
                    }
                    KeyCode::Char('n') => {
                        app.settings.notify = !app.settings.notify;
                        let msg = if app.settings.notify {
                            "Notifications on: you'll hear when a session finishes or errors"
                        } else {
                            "Notifications off"
                        };
                        if app.settings.notify {
                            notify("Notifications on", "whosup", "This is what they look like");
                        }
                        match app.settings.save() {
                            Ok(()) => app.flash(msg, false),
                            Err(e) => app.flash(format!("{msg} (couldn't save it: {e})"), true),
                        }
                    }
                    KeyCode::Char('J') => app.detail_scroll = app.detail_scroll.saturating_add(3),
                    KeyCode::Char('K') => app.detail_scroll = app.detail_scroll.saturating_sub(3),
                    _ => {}
                }
            }
        }
        if last.elapsed() >= data_tick {
            app.refresh();
            last = Instant::now();
        }
    }
}

fn main() -> std::io::Result<()> {
    let mut args = std::env::args().skip(1);
    let mut dir: Option<PathBuf> = std::env::var_os("PI_CODING_AGENT_DIR").map(PathBuf::from);
    let config = settings::Settings::load();
    if let Some(t) = &config.terminal {
        resume::set_default_terminal(t);
    }
    cost::set_prices(config.prices.clone());
    data::set_stuck_after_minutes(config.stuck_minutes);
    let mut hours: f64 = config.recent_hours;
    let mut once = false;
    let mut all = false;
    let mut demo = false;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--dir" => dir = args.next().map(PathBuf::from),
            "--recent" => hours = args.next().and_then(|h| h.parse().ok()).unwrap_or(hours),
            "--once" => once = true,
            "--all" => all = true,
            "--demo" => demo = true,
            // resume the first closed session whose title or transcript path contains the text
            "--resume" => {
                let query = args.next().unwrap_or_default().to_lowercase();
                let dir = dir.clone().unwrap_or_else(|| PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".pi/agent"));
                let mut scanner = Scanner::new(dir, (hours * 3_600_000.0) as i64);
                let snap = scanner.scan();
                let hosts = settings::Hosts::load();
                let Some(s) = snap.sessions.iter().find(|s| {
                    !s.live() && (s.title().to_lowercase().contains(&query) || s.key.to_lowercase().contains(&query))
                }) else {
                    eprintln!("no closed session matches {query:?} (closed in the last {hours} hours)");
                    std::process::exit(1);
                };
                let result = resume::plan(s, hosts.get(&s.key).map(String::as_str)).and_then(|p| {
                    println!("{} → {} ({}): {}", s.title(), p.place.describe(), p.because, p.command);
                    if matches!(p.place, resume::Place::Terminal(_)) {
                        data::expect_resume(&s.key, s.tool, &s.t.cwd);
                    }
                    resume::launch(&p)
                });
                match result {
                    Ok(m) => println!("{m}"),
                    Err(e) => {
                        eprintln!("{e}");
                        std::process::exit(1);
                    }
                }
                return Ok(());
            }
            // bring a tty's tab to the front, e.g. `whosup --focus ttys003`
            "--focus" => {
                let tty = args.next().unwrap_or_default();
                let terms = crate::terms::scan(&sysinfo::System::new_all());
                let apps: focus::AppByTty = terms.iter().map(|t| (t.tty.clone(), t.app.clone())).collect();
                let app = apps.get(&tty).cloned().unwrap_or_default();
                match focus::focus(&tty, &app, &apps) {
                    Ok(m) => println!("{m}"),
                    Err(e) => {
                        eprintln!("{e}");
                        std::process::exit(1);
                    }
                }
                return Ok(());
            }
            "-h" | "--help" => {
                println!("{HELP}");
                return Ok(());
            }
            other => {
                eprintln!("unknown argument: {other}\n\n{HELP}");
                std::process::exit(2);
            }
        }
    }
    let dir = dir.unwrap_or_else(|| PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".pi/agent"));
    models::load(&dir);
    let mut scanner = Scanner::new(dir, (hours * 3_600_000.0) as i64);

    if once {
        print_once(&if demo { demo::snapshot() } else { scanner.scan() }, all);
        return Ok(());
    }
    let app = if demo { App::new_demo(scanner) } else { App::new(scanner) };
    let mut term = ratatui::init();
    let res = run(&mut term, app);
    ratatui::restore();
    res
}

#[cfg(test)]
mod render_test {
    use super::*;
    use ratatui::{backend::TestBackend, Terminal};

    /// Render real data: `WHOSUP_SIZE=120x30 WHOSUP_VIEW=terminals WHOSUP_SEL=3 WHOSUP_RECENT=1
    /// cargo test --release render -- --ignored --nocapture`. Also writes /tmp/whosup-<size>.html.
    #[test]
    #[ignore]
    fn render() {
        let env = |k: &str| std::env::var(k).ok();
        let (w, h) = env("WHOSUP_SIZE")
            .and_then(|s| s.split_once('x').map(|(a, b)| (a.parse().unwrap(), b.parse().unwrap())))
            .unwrap_or((160u16, 40u16));
        let dir = PathBuf::from(std::env::var("HOME").unwrap()).join(".pi/agent");
        ui::set_light(env("WHOSUP_LIGHT").is_some());
        models::load(&dir);
        let mut app = if env("WHOSUP_DEMO").is_some() {
            App::new_demo(Scanner::new(dir.clone(), 0))
        } else {
            App::new(Scanner::new(dir, 30 * 3_600_000))
        };
        if env("WHOSUP_RECENT").is_some() {
            app.cycle_history();
            app.refresh();
        }
        app.hide_finished = env("WHOSUP_HIDE").is_some();
        app.rebuild_rows();
        if env("WHOSUP_VIEW").as_deref() == Some("terminals") {
            app.set_view(View::Terminals);
        }
        if let Some(q) = env("WHOSUP_QUERY") {
            app.query = q;
            app.searching = true;
            app.rebuild_rows();
        }
        if let Some(n) = env("WHOSUP_SEL").and_then(|n| n.parse::<isize>().ok()) {
            app.move_sel(n);
        }
        let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
        t.draw(|f| ui::draw(f, &mut app)).unwrap();
        let buf = t.backend().buffer().clone();
        let mut html = String::from("<html><body style='margin:0;background:#1a1b26;width:max-content'><pre style='margin:0;padding:12px;font:13px/1.25 Menlo,monospace;color:#c0caf5'>");
        for y in 0..buf.area.height {
            let line: String = (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect();
            println!("{line}");
            for x in 0..buf.area.width {
                let c = &buf[(x, y)];
                let col = |c: ratatui::style::Color| match c {
                    ratatui::style::Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
                    _ => "inherit".into(),
                };
                let bg = match c.bg { ratatui::style::Color::Reset => "transparent".into(), b => col(b) };
                let bold = if c.modifier.contains(ratatui::style::Modifier::BOLD) { "font-weight:bold;" } else { "" };
                let sym: String = c.symbol().chars().map(|ch| match ch {
                    '&' => "&amp;".to_string(),
                    '<' => "&lt;".to_string(),
                    '>' => "&gt;".to_string(),
                    c if c.is_ascii() => c.to_string(),
                    c => format!("&#{};", c as u32),
                }).collect();
                html.push_str(&format!("<span style='color:{};background:{};{bold}'>{sym}</span>", col(c.fg), bg));
            }
            html.push('\n');
        }
        html.push_str("</pre></body></html>");
        std::fs::write(format!("/tmp/whosup-{w}x{h}.html"), html).unwrap();
    }
}

/// README screenshots: the real UI on `--demo` data, drawn to SVG cell by cell.
/// `cargo test --release screenshots -- --ignored` writes docs/*.svg.
#[cfg(test)]
mod screenshots {
    use super::*;
    use ratatui::{backend::TestBackend, buffer::Buffer, style::Color, Terminal};

    const CW: f32 = 8.8; // cell width
    const CH: f32 = 19.0; // cell height
    const PAD: f32 = 18.0;
    const BAR: f32 = 34.0; // window title bar

    fn hex(c: Color, fallback: &str) -> String {
        match c {
            Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
            _ => fallback.to_string(),
        }
    }

    fn esc(s: &str) -> String {
        s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
    }

    fn svg(buf: &Buffer, bg: &str, fg: &str, title: &str, chrome: &str) -> String {
        let (w, h) = (buf.area.width as f32 * CW, buf.area.height as f32 * CH);
        let (tw, th) = (w + PAD * 2.0, h + PAD * 2.0 + BAR);
        let mut o = String::new();
        o += &format!(
            "<svg xmlns='http://www.w3.org/2000/svg' width='{tw}' height='{th}' viewBox='0 0 {tw} {th}'>\n\
             <rect width='{tw}' height='{th}' rx='12' fill='{bg}'/>\n\
             <rect width='{tw}' height='{BAR}' rx='12' fill='{chrome}'/><rect y='{y}' width='{tw}' height='12' fill='{chrome}'/>\n\
             <circle cx='22' cy='17' r='6' fill='#ff5f57'/><circle cx='42' cy='17' r='6' fill='#febc2e'/><circle cx='62' cy='17' r='6' fill='#28c840'/>\n\
             <text x='{cx}' y='22' text-anchor='middle' font-family='-apple-system,Segoe UI,sans-serif' font-size='12.5' fill='{fg}' opacity='0.6'>{title}</text>\n\
             <g transform='translate({PAD},{oy})' font-family='Menlo,SF Mono,Monaco,Consolas,monospace' font-size='14.2'>\n",
            y = BAR - 12.0,
            cx = tw / 2.0,
            oy = BAR + PAD
        );
        for y in 0..buf.area.height {
            let top = y as f32 * CH;
            // backgrounds, merged into runs
            let mut x = 0;
            while x < buf.area.width {
                let c = &buf[(x, y)];
                if let Color::Rgb(..) = c.bg {
                    let start = x;
                    while x < buf.area.width && buf[(x, y)].bg == c.bg {
                        x += 1;
                    }
                    o += &format!(
                        "<rect x='{:.1}' y='{:.1}' width='{:.1}' height='{CH}' fill='{}'/>\n",
                        start as f32 * CW,
                        top,
                        (x - start) as f32 * CW + 0.4,
                        hex(c.bg, bg)
                    );
                } else {
                    x += 1;
                }
            }
            // glyphs: blocks and box lines as shapes, everything else as positioned text
            let mut run: Option<(String, bool, Vec<f32>, String)> = None; // fg, bold, xs, text
            // one <text> per character: every renderer honours a single x, not all honour x lists
            let flush = |run: &mut Option<(String, bool, Vec<f32>, String)>, o: &mut String| {
                if let Some((f, b, xs, t)) = run.take() {
                    *o += &format!("<g fill='{f}'{}>", if b { " font-weight='bold'" } else { "" });
                    for (x, ch) in xs.iter().zip(t.chars()) {
                        *o += &format!("<text x='{x:.1}' y='{:.1}'>{}</text>", top + CH * 0.72, esc(&ch.to_string()));
                    }
                    *o += "</g>\n";
                }
            };
            for x in 0..buf.area.width {
                let c = &buf[(x, y)];
                let sym = c.symbol();
                let f = hex(c.fg, fg);
                let (l, m, r) = (x as f32 * CW, x as f32 * CW + CW / 2.0, (x + 1) as f32 * CW);
                let (t, mid, b) = (top, top + CH / 2.0, top + CH);
                let line = |d: String| format!("<path d='{d}' stroke='{f}' stroke-width='1.2' fill='none'/>\n");
                let shape = match sym {
                    " " | "" => Some(String::new()),
                    "█" => Some(format!("<rect x='{l:.1}' y='{t:.1}' width='{:.1}' height='{:.1}' fill='{f}'/>\n", CW + 0.4, CH + 0.4)),
                    "▀" => Some(format!("<rect x='{l:.1}' y='{t:.1}' width='{:.1}' height='{:.1}' fill='{f}'/>\n", CW + 0.4, CH / 2.0 + 0.3)),
                    "▄" => Some(format!("<rect x='{l:.1}' y='{mid:.1}' width='{:.1}' height='{:.1}' fill='{f}'/>\n", CW + 0.4, CH / 2.0 + 0.4)),
                    "▌" => Some(format!("<rect x='{l:.1}' y='{t:.1}' width='{:.1}' height='{CH}' fill='{f}'/>\n", CW * 0.4)),
                    "─" => Some(line(format!("M{l:.1} {mid:.1}H{r:.1}"))),
                    "│" => Some(line(format!("M{m:.1} {t:.1}V{b:.1}"))),
                    "╭" => Some(line(format!("M{r:.1} {mid:.1}Q{m:.1} {mid:.1} {m:.1} {b:.1}"))),
                    "╮" => Some(line(format!("M{l:.1} {mid:.1}Q{m:.1} {mid:.1} {m:.1} {b:.1}"))),
                    "╰" => Some(line(format!("M{m:.1} {t:.1}Q{m:.1} {mid:.1} {r:.1} {mid:.1}"))),
                    "╯" => Some(line(format!("M{m:.1} {t:.1}Q{m:.1} {mid:.1} {l:.1} {mid:.1}"))),
                    "├" => Some(line(format!("M{m:.1} {t:.1}V{b:.1}M{m:.1} {mid:.1}H{r:.1}"))),
                    "└" => Some(line(format!("M{m:.1} {t:.1}V{mid:.1}H{r:.1}"))),
                    _ => None,
                };
                let bold = c.modifier.contains(ratatui::style::Modifier::BOLD);
                match shape {
                    Some(s) => {
                        flush(&mut run, &mut o);
                        o += &s;
                    }
                    None => {
                        let same = matches!(&run, Some((rf, rb, _, _)) if *rf == f && *rb == bold);
                        if !same {
                            flush(&mut run, &mut o);
                            run = Some((f.clone(), bold, vec![], String::new()));
                        }
                        let rr = run.as_mut().unwrap();
                        for ch in sym.chars() {
                            rr.2.push(l);
                            rr.3.push(ch);
                        }
                    }
                }
            }
            flush(&mut run, &mut o);
        }
        o += "</g>\n</svg>\n";
        o
    }

    fn shot(light: bool, view: View, sel: isize, w: u16, h: u16, file: &str) {
        // the demo's made-up home, so paths show as ~/code/…
        unsafe { std::env::set_var("HOME", "/Users/you") };
        ui::set_light(light);
        let dir = PathBuf::from("/nonexistent");
        let mut app = App::new_demo(Scanner::new(dir, 0));
        // creating the app applies the saved theme; the screenshot's theme wins
        app.settings.light = light;
        ui::set_light(light);
        app.settings.notify = true;
        app.cycle_history();
        app.rebuild_rows();
        if view == View::Terminals {
            app.set_view(View::Terminals);
        }
        app.move_sel(sel);
        let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
        t.draw(|f| ui::draw(f, &mut app)).unwrap();
        let p = ui::pal();
        let (bg, fg) = (hex(p.bg.unwrap_or(Color::Rgb(26, 27, 38)), "#1a1b26"), hex(p.fg, "#c0caf5"));
        let chrome = if light { "#e9ebf2" } else { "#15161f" };
        std::fs::create_dir_all("docs").unwrap();
        std::fs::write(format!("docs/{file}"), svg(t.backend().buffer(), &bg, &fg, "whosup", chrome)).unwrap();
    }

    #[test]
    #[ignore]
    fn screenshots() {
        shot(false, View::Sessions, 1, 150, 27, "whosup-dark.svg");
        shot(true, View::Sessions, 7, 150, 27, "whosup-light.svg");
        shot(false, View::Terminals, 1, 150, 22, "whosup-terminals.svg");
    }
}

#[cfg(test)]
mod search_tests {
    use super::*;

    fn demo_app() -> App {
        App::new_demo(Scanner::new(PathBuf::from("/nonexistent"), 0))
    }

    fn titles(app: &App) -> Vec<String> {
        app.rows
            .iter()
            .filter_map(|(_, r)| match r {
                RowRef::Session(i) => Some(app.snap.sessions[*i].title()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn filters_by_any_field() {
        let mut app = demo_app();
        // a sub-agent's task
        app.query = "redis".into();
        app.rebuild_rows();
        assert_eq!(titles(&app), vec!["Add rate limiting to the public API"]);
        // agent name
        app.query = "codex".into();
        app.rebuild_rows();
        assert_eq!(titles(&app), vec!["Fix flaky checkout test"]);
        // every word must match; closed sessions are searched too
        app.query = "pi dashboard".into();
        app.rebuild_rows();
        assert_eq!(titles(&app), vec!["Profile the slow dashboard query"]);
        app.query = "nothing like this".into();
        app.rebuild_rows();
        assert!(titles(&app).is_empty());
        app.query.clear();
        app.rebuild_rows();
        assert_eq!(titles(&app).len(), 3, "live ones only when not searching");
    }

    #[test]
    fn filters_terminals() {
        let mut app = demo_app();
        app.set_view(View::Terminals);
        app.query = "cargo".into();
        app.rebuild_rows();
        assert_eq!(app.rows.len(), 1);
        app.query = "vs code".into();
        app.rebuild_rows();
        assert_eq!(app.rows.len(), 1, "the name the list shows");
        app.query = "visual".into();
        app.rebuild_rows();
        assert_eq!(app.rows.len(), 1);
    }
}

#[cfg(test)]
mod history_tests {
    use super::*;

    #[test]
    fn a_cycles_through_history() {
        let mut app = App::new_demo(Scanner::new(PathBuf::from("/nonexistent"), 24 * 3_600_000));
        assert_eq!(app.history, History::Live);
        app.cycle_history();
        assert_eq!((app.history, app.scanner.recent_window_ms), (History::Recent, 24 * 3_600_000));
        assert_eq!(app.history.label(app.scanner_window()), "24h");
        app.cycle_history();
        assert_eq!((app.history, app.scanner.recent_window_ms), (History::Week, 7 * 24 * 3_600_000));
        app.cycle_history();
        assert_eq!(app.history, History::All);
        assert!(app.scanner.recent_window_ms > 100 * 365 * 24 * 3_600_000);
        app.cycle_history();
        assert_eq!(app.history, History::Live);
        assert!(!app.show_recent);
        assert_eq!(History::Recent.label(72 * 3_600_000), "3 days");
    }

    #[test]
    fn the_window_decides_which_closed_sessions_load() {
        // a copy dated 2020, so no running agent counts as having touched it
        let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        let fixtures = std::env::temp_dir().join(format!("whosup-history-{}", std::process::id()));
        std::process::Command::new("cp").args(["-R"]).arg(&src).arg(&fixtures).status().unwrap();
        std::process::Command::new("sh")
            .arg("-c")
            .arg(format!("find {} -type f -exec touch -t 202001010000 {{}} +", fixtures.display()))
            .status()
            .unwrap();
        let scan = |window: i64| {
            let mut s = Scanner::new(fixtures.join("pi"), window);
            s.claude_dir = fixtures.join("claude");
            s.codex_dir = fixtures.join("codex");
            s.scan().sessions.len()
        };
        // the fixture files are older than a second, so a 1 ms window loads none of them
        assert_eq!(scan(1), 0);
        assert_eq!(scan(i64::MAX / 4), 4);
        std::fs::remove_dir_all(&fixtures).ok();
    }
}
