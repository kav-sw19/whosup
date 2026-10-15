mod data;
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
    path::PathBuf,
    time::{Duration, Instant},
};

const HELP: &str = "pitop — live view of coding-agent sessions (pi, Claude Code, Codex, …), their sub-agents and open terminals

USAGE: pitop [--dir <pi agent dir>] [--recent <hours>] [--once [--all]]

  --dir      pi agent dir (default: $PI_CODING_AGENT_DIR or ~/.pi/agent); Claude Code and Codex
             use $CLAUDE_CONFIG_DIR (~/.claude) and $CODEX_HOME (~/.codex)
  --recent   also list sessions touched in the last N hours (default 24; toggle with `a`)
  --once     print a plain-text snapshot and exit (--all: include closed recent sessions)

KEYS: tab/1/2 sessions·terminals · ↑/↓ j/k move · ←/→ h/l fold · ↵ open · J/K scroll details
      a all-recent/live-only
      f hide finished agents · g/G top/bottom · r refresh · q quit";

#[derive(Clone)]
pub enum RowRef {
    Session(usize),
    Agent(usize, usize),
    Orphan(usize),
    Terminal(usize),
    Job(usize, usize),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum View {
    Sessions,
    Terminals,
}

pub struct App {
    view: View,
    /// selection remembered per view
    saved_sel: [Option<String>; 2],
    scanner: Scanner,
    snap: Snapshot,
    show_recent: bool,
    hide_finished: bool,
    collapsed: HashSet<String>,
    rows: Vec<(String, RowRef)>,
    state: TableState,
    selected_key: Option<String>,
    detail_scroll: u16,
}

impl App {
    fn new(scanner: Scanner) -> Self {
        let mut a = Self {
            view: View::Sessions,
            saved_sel: [None, None],
            scanner,
            snap: Snapshot { sessions: vec![], orphan_procs: vec![], terminals: vec![] },
            show_recent: false,
            hide_finished: false,
            collapsed: HashSet::new(),
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
        for (i, s) in self.visible_sessions() {
            rows.push((s.key.clone(), RowRef::Session(i)));
            if self.collapsed.contains(&s.key) {
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

    fn finish_rows(&mut self, rows: Vec<(String, RowRef)>) {
        self.rows = rows;
        let idx = self
            .selected_key
            .as_ref()
            .and_then(|k| self.rows.iter().position(|(rk, _)| rk == k))
            .unwrap_or_else(|| self.state.selected().unwrap_or(0).min(self.rows.len().saturating_sub(1)));
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
        let n = (cur + d).clamp(0, self.rows.len() as isize - 1) as usize;
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

    /// Enter: expand a session, or jump from a terminal to the pi session inside it.
    fn activate(&mut self) {
        if let Some(RowRef::Terminal(i)) = self.selected() {
            if let Some(key) = self.snap.terminals[*i].session.as_ref().and_then(|l| l.key.clone()) {
                self.set_view(View::Sessions);
                self.selected_key = Some(key);
                self.rebuild_rows();
            }
            return;
        }
        self.set_collapsed(false);
    }

    fn set_collapsed(&mut self, collapse: bool) {
        let si = match self.selected() {
            Some(RowRef::Session(i)) | Some(RowRef::Agent(i, _)) => *i,
            _ => return,
        };
        let key = self.snap.sessions[si].key.clone();
        if collapse {
            self.collapsed.insert(key.clone());
            self.selected_key = Some(key);
        } else {
            self.collapsed.remove(&key);
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
        println!("● {:<6} pid {} (no transcript) {}", p.tool.label(), p.pid, home_rel(&p.cwd));
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

fn run(term: &mut DefaultTerminal, mut app: App) -> std::io::Result<()> {
    let data_tick = Duration::from_millis(1000);
    let frame = Duration::from_millis(80); // spinner cadence
    let mut last = Instant::now();
    loop {
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

    /// `cargo test render -- --ignored --nocapture` renders the real data to stdout.
    #[test]
    #[ignore]
    fn render() {
        let dir = PathBuf::from(std::env::var("HOME").unwrap()).join(".pi/agent");
        let mut app = App::new(Scanner::new(dir, 30 * 3_600_000));
        app.show_recent = true;
        app.rebuild_rows();
        for w in [160u16, 100] {
            let mut t = Terminal::new(TestBackend::new(w, 40)).unwrap();
            t.draw(|f| ui::draw(f, &mut app)).unwrap();
            let buf = t.backend().buffer().clone();
            let mut html = String::from("<html><head><meta charset='utf-8'></head><body style='margin:0;background:#1a1b26;width:max-content'><pre style='margin:0;padding:12px;font:13px/1.25 Menlo,monospace;color:#c0caf5'>");
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
            std::fs::write(format!("/tmp/pitop-{w}.html"), html).unwrap();
            app.move_sel(7);
        }
        app.set_view(View::Terminals);
        let mut t = Terminal::new(TestBackend::new(150, 22)).unwrap();
        t.draw(|f| ui::draw(f, &mut app)).unwrap();
        let buf = t.backend().buffer().clone();
        for y in 0..buf.area.height {
            let line: String = (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect();
            println!("{line}");
        }
    }
}
