mod data;
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

  --dir      pi agent dir (default: $PI_CODING_AGENT_DIR or ~/.pi/agent); Claude Code and Codex
             use $CLAUDE_CONFIG_DIR (~/.claude) and $CODEX_HOME (~/.codex)
  --recent   also list sessions touched in the last N hours (default 24; toggle with `a`)
  --once     print a plain-text snapshot and exit (--all: include closed recent sessions)

KEYS: tab/1/2 sessions·terminals · ↑/↓ j/k move · ↵ go to the session's terminal tab
      ←/→ h/l fold · s (terminals) show its session · J/K scroll details · a all-recent/live-only
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
}

impl App {
    fn new(scanner: Scanner) -> Self {
        let (focus_tx, focus_rx) = mpsc::channel();
        let mut a = Self {
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
        };
        a.refresh();
        a
    }

    fn visible_sessions(&self) -> impl Iterator<Item = (usize, &Session)> {
        let show_recent = self.show_recent;
        self.snap
            .sessions
            .iter()
            .enumerate()
            .filter(move |(_, s)| show_recent || s.live() || s.running_agents() > 0)
    }

    fn rebuild_rows(&mut self) {
        let mut rows = Vec::new();
        if self.view == View::Terminals {
            for (i, t) in self.snap.terminals.iter().enumerate() {
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
            rows.push((format!("p:{}", p.pid), RowRef::Orphan(k)));
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

    fn refresh(&mut self) {
        self.snap = self.scanner.scan();
        self.rebuild_rows();
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
            "{} {:<6} {}  [{}]  {}  {}  {}  ${:.2}",
            if s.live() { "●" } else { "○" },
            s.tool.label(),
            one_line(&s.title(), 50),
            state,
            home_rel(&s.t.cwd),
            short_model(s.t.model.as_deref().unwrap_or("")),
            if s.live() { s.t.activity() } else { "closed".into() },
            s.t.cost
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

/// `~/.config/whosup/theme` (or `$XDG_CONFIG_HOME/whosup/theme`): "light" or "dark".
fn theme_file() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".config"));
    base.join("whosup").join("theme")
}

fn load_theme() {
    let light = std::fs::read_to_string(theme_file()).map(|s| s.trim() == "light").unwrap_or(false);
    ui::set_light(light);
}

fn save_theme() -> std::io::Result<()> {
    let f = theme_file();
    if let Some(dir) = f.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(f, if ui::is_light() { "light\n" } else { "dark\n" })
}

fn run(term: &mut DefaultTerminal, mut app: App) -> std::io::Result<()> {
    let data_tick = Duration::from_millis(1000);
    let frame = Duration::from_millis(80); // spinner cadence
    let mut last = Instant::now();
    loop {
        app.poll_focus();
        term.draw(|f| ui::draw(f, &mut app))?;
        if event::poll(frame)? {
            if let Event::Key(k) = event::read()? {
                if k.kind != KeyEventKind::Press {
                    continue;
                }
                match k.code {
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
                    KeyCode::Char('a') => {
                        app.show_recent = !app.show_recent;
                        app.rebuild_rows();
                    }
                    KeyCode::Char('f') => {
                        app.hide_finished = !app.hide_finished;
                        app.rebuild_rows();
                    }
                    KeyCode::Char('r') => app.refresh(),
                    KeyCode::Char('t') => {
                        ui::set_light(!ui::is_light());
                        let name = if ui::is_light() { "Light" } else { "Dark" };
                        match save_theme() {
                            Ok(()) => app.flash(format!("{name} theme"), false),
                            Err(e) => app.flash(format!("{name} theme (couldn't save it: {e})"), true),
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
    let mut hours: f64 = 24.0;
    let mut once = false;
    let mut all = false;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--dir" => dir = args.next().map(PathBuf::from),
            "--recent" => hours = args.next().and_then(|h| h.parse().ok()).unwrap_or(hours),
            "--once" => once = true,
            "--all" => all = true,
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
    let mut scanner = Scanner::new(dir, (hours * 3_600_000.0) as i64);

    if once {
        print_once(&scanner.scan(), all);
        return Ok(());
    }
    load_theme();
    let app = App::new(scanner);
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
        let mut app = App::new(Scanner::new(dir, 30 * 3_600_000));
        app.show_recent = env("WHOSUP_RECENT").is_some();
        app.hide_finished = env("WHOSUP_HIDE").is_some();
        app.rebuild_rows();
        if env("WHOSUP_VIEW").as_deref() == Some("terminals") {
            app.set_view(View::Terminals);
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
