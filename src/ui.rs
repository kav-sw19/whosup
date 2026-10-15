//! Rendering. One palette, one set of glyphs, used everywhere.

use crate::data::{now_ms, Agent, AgentSource, Session};
use crate::providers::Tool;
use crate::terms::Terminal;
use crate::{App, RowRef, View};
use ratatui::{
    layout::{Alignment, Constraint, Layout, Margin, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Cell, Padding, Paragraph, Row, Table, Wrap},
    Frame,
};

// ── palette (Tokyo Night-ish; background is left to the terminal) ─────────────
const FG: Color = Color::Rgb(192, 202, 245);
const SOFT: Color = Color::Rgb(169, 177, 214);
const MUTED: Color = Color::Rgb(108, 116, 158);
const FAINT: Color = Color::Rgb(68, 75, 106);
const BORDER: Color = Color::Rgb(54, 60, 88);
const BORDER_FOCUS: Color = Color::Rgb(92, 104, 150);
const SELECT: Color = Color::Rgb(36, 42, 66);
const BLUE: Color = Color::Rgb(122, 162, 247);
const CYAN: Color = Color::Rgb(125, 207, 255);
const GREEN: Color = Color::Rgb(158, 206, 106);
const AMBER: Color = Color::Rgb(224, 175, 104);
const RED: Color = Color::Rgb(247, 118, 142);
const VIOLET: Color = Color::Rgb(187, 154, 247);

const ORANGE: Color = Color::Rgb(255, 158, 100);
const TEAL: Color = Color::Rgb(115, 218, 202);

pub fn tool_color(t: Tool) -> Color {
    match t {
        Tool::Pi => BLUE,
        Tool::Claude => ORANGE,
        Tool::Codex => TEAL,
        Tool::Other(_) => VIOLET,
    }
}

/// Fixed-width coloured agent tag: `claude`, `pi    `, `codex `.
fn tool_tag(t: Tool, live: bool) -> Span<'static> {
    let c = if live { tool_color(t) } else { FAINT };
    Span::styled(format!("{:<7}", t.label()), Style::new().fg(c))
}

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

fn spinner() -> &'static str {
    SPINNER[((now_ms() / 80) as usize) % SPINNER.len()]
}

// ── formatting helpers ────────────────────────────────────────────────────────
pub fn ago(ms: Option<i64>) -> String {
    let Some(ms) = ms else { return "—".into() };
    let s = ((now_ms() - ms) / 1000).max(0);
    match s {
        0..=4 => "now".into(),
        5..=59 => format!("{s}s"),
        60..=3599 => format!("{}m", s / 60),
        3600..=86399 => format!("{}h {}m", s / 3600, (s % 3600) / 60),
        _ => format!("{}d", s / 86400),
    }
}

pub fn dur(a: Option<i64>, b: Option<i64>) -> String {
    match (a, b) {
        (Some(a), Some(b)) if b >= a => {
            let s = (b - a) / 1000;
            if s < 60 {
                format!("{s}s")
            } else if s < 3600 {
                format!("{}m {:02}s", s / 60, s % 60)
            } else {
                format!("{}h {:02}m", s / 3600, (s % 3600) / 60)
            }
        }
        _ => "—".into(),
    }
}

pub fn tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1e6)
    } else if n >= 1000 {
        format!("{:.1}k", n as f64 / 1e3)
    } else {
        n.to_string()
    }
}

pub fn money(c: f64) -> String {
    let c = c + 0.0; // normalise -0.0
    if c == 0.0 {
        "—".into()
    } else if c < 0.01 {
        "<$0.01".into()
    } else {
        format!("${c:.2}")
    }
}

pub fn one_line(s: &str, n: usize) -> String {
    let l = s.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
    if l.chars().count() > n {
        l.chars().take(n.saturating_sub(1)).collect::<String>() + "…"
    } else {
        l.to_string()
    }
}

pub fn short_model(m: &str) -> String {
    m.rsplit('/').next().unwrap_or(m).trim_start_matches("claude-").to_string()
}

/// `~/Documents/Dev/medallion` -> `…/Dev/medallion` when it doesn't fit.
fn short_path(p: &str, max: usize) -> String {
    let h = home_rel(p);
    if h.chars().count() <= max {
        return h;
    }
    let parts: Vec<&str> = h.split('/').collect();
    for k in (1..parts.len()).rev() {
        let tail = format!("…/{}", parts[parts.len() - k..].join("/"));
        if tail.chars().count() <= max {
            return tail;
        }
    }
    one_line(parts.last().unwrap_or(&""), max)
}

pub fn home_rel(p: &str) -> String {
    match std::env::var("HOME") {
        Ok(h) if !h.is_empty() && p.starts_with(&h) => format!("~{}", &p[h.len()..]),
        _ => p.to_string(),
    }
}

// ── semantic status ───────────────────────────────────────────────────────────
struct Status {
    glyph: String,
    label: String,
    color: Color,
}

fn session_status(s: &Session) -> Status {
    if !s.live() {
        return Status { glyph: "○".into(), label: "closed".into(), color: FAINT };
    }
    if s.t.is_busy() {
        let label = match (s.t.last_role.as_deref(), s.t.last_stop.as_deref()) {
            (Some("assistant"), Some("toolUse")) => s.t.last_tool.clone().unwrap_or_else(|| "tool".into()),
            _ => "thinking".into(),
        };
        return Status { glyph: spinner().into(), label, color: AMBER };
    }
    match s.t.last_stop.as_deref() {
        Some("error") => Status { glyph: "●".into(), label: "error".into(), color: RED },
        Some("aborted") => Status { glyph: "●".into(), label: "interrupted".into(), color: MUTED },
        _ if s.t.last_role.is_none() => Status { glyph: "●".into(), label: "new".into(), color: CYAN },
        _ => Status { glyph: "●".into(), label: "waiting for you".into(), color: GREEN },
    }
}

fn agent_status(a: &Agent) -> Status {
    if a.running {
        let label = a.activity.trim_start_matches("⚙ ").to_string();
        return Status { glyph: spinner().into(), label, color: AMBER };
    }
    match a.status.as_str() {
        "done" | "complete" => Status { glyph: "✓".into(), label: "done".into(), color: GREEN },
        "error" | "failed" | "rejected" => Status { glyph: "✗".into(), label: a.status.clone(), color: RED },
        "stale" => Status { glyph: "◌".into(), label: "orphaned".into(), color: MUTED },
        other => Status { glyph: "·".into(), label: other.into(), color: MUTED },
    }
}

fn panel<'a>(title: Line<'a>, focused: bool) -> Block<'a> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(if focused { BORDER_FOCUS } else { BORDER }))
        .title(title)
        .padding(Padding::horizontal(1))
}

fn title_line(text: impl Into<String>, extra: Option<Span<'static>>) -> Line<'static> {
    let mut spans = vec![Span::raw(" "), Span::styled(text.into(), Style::new().fg(FG).bold())];
    if let Some(e) = extra {
        spans.push(Span::raw(" "));
        spans.push(e);
    }
    spans.push(Span::raw(" "));
    Line::from(spans)
}

// ── frame ─────────────────────────────────────────────────────────────────────
pub fn draw(f: &mut Frame, app: &mut App) {
    let [header, _, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(5),
        Constraint::Length(1),
    ])
    .areas(f.area().inner(Margin { horizontal: 1, vertical: 0 }));

    draw_header(f, app, header);

    let wide = body.width >= 120;
    let [list_area, detail_area] = if wide {
        Layout::horizontal([Constraint::Percentage(60), Constraint::Percentage(40)]).spacing(1).areas(body)
    } else {
        Layout::vertical([Constraint::Percentage(55), Constraint::Percentage(45)]).areas(body)
    };
    if app.view == View::Terminals {
        draw_terminal_list(f, app, list_area);
    } else {
        draw_list(f, app, list_area);
    }
    draw_detail(f, app, detail_area);
    draw_footer(f, app, footer);
}

fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    let sessions = &app.snap.sessions;
    let live = sessions.iter().filter(|s| s.live()).count();
    let working = sessions.iter().filter(|s| s.live() && s.t.is_busy()).count();
    let agents: usize = sessions.iter().map(|s| s.running_agents()).sum();
    let cost = sessions
        .iter()
        .filter(|s| s.live())
        .map(|s| s.t.cost + s.agents.iter().map(|a| a.cost).sum::<f64>())
        .fold(0.0, |a, b| a + b);

    let sep = || Span::styled("   ", Style::new());
    let stat = |n: usize, label: &str, color: Color, glyph: &str| -> Vec<Span<'static>> {
        let c = if n == 0 { MUTED } else { color };
        vec![
            Span::styled(format!("{glyph} "), Style::new().fg(c)),
            Span::styled(n.to_string(), Style::new().fg(if n == 0 { MUTED } else { FG }).bold()),
            Span::styled(format!(" {label}"), Style::new().fg(MUTED)),
        ]
    };

    let terms = &app.snap.terminals;
    let tab = |label: &str, n: usize, key: &str, active: bool| -> Vec<Span<'static>> {
        let (fg, bg) = if active { (FG, SELECT) } else { (MUTED, Color::Reset) };
        vec![
            Span::styled(format!(" {key} "), Style::new().fg(if active { BLUE } else { FAINT }).bg(bg)),
            Span::styled(label.to_string(), Style::new().fg(fg).bg(bg).add_modifier(if active { Modifier::BOLD } else { Modifier::empty() })),
            Span::styled(format!(" {n} "), Style::new().fg(if active { SOFT } else { FAINT }).bg(bg)),
        ]
    };
    let mut core = vec![
        Span::styled(" pitop ", Style::new().fg(Color::Rgb(26, 27, 38)).bg(BLUE).bold()),
        Span::raw("  "),
    ];
    core.extend(tab("Sessions", live, "1", app.view == View::Sessions));
    core.push(Span::raw(" "));
    core.extend(tab("Terminals", terms.len(), "2", app.view == View::Terminals));

    let mut stats = vec![sep()];
    let right: Vec<Span> = if app.view == View::Sessions {
        stats.extend(stat(working, "working", AMBER, if working > 0 { spinner() } else { "◌" }));
        stats.push(sep());
        stats.extend(stat(agents, if agents == 1 { "agent" } else { "agents" }, VIOLET, "◆"));
        vec![
            Span::styled(money(cost), Style::new().fg(FG).bold()),
            Span::styled(" in live sessions ", Style::new().fg(MUTED)),
        ]
    } else {
        let running = terms.iter().filter(|t| !t.at_prompt()).count();
        let with_agent = terms.iter().filter(|t| t.session.is_some()).count();
        stats.extend(stat(running, "running", BLUE, "▶"));
        stats.push(sep());
        stats.extend(stat(with_agent, "with agents", VIOLET, "◆"));
        let cpu: f32 = terms.iter().map(|t| t.cpu).sum();
        let mem: u64 = terms.iter().map(|t| t.mem).sum();
        vec![
            Span::styled(format!("{cpu:.0}%"), Style::new().fg(FG).bold()),
            Span::styled(" cpu   ", Style::new().fg(MUTED)),
            Span::styled(mem_str(mem), Style::new().fg(FG).bold()),
            Span::styled(" memory ", Style::new().fg(MUTED)),
        ]
    };

    // Drop the least important parts first so nothing ever overlaps: right side, then stats.
    let width = |v: &[Span]| v.iter().map(|s| s.width()).sum::<usize>();
    let avail = area.width as usize;
    let (cw, sw, rw) = (width(&core), width(&stats), width(&right));
    let mut left = core;
    if cw + sw <= avail {
        left.extend(stats);
    }
    let lw = width(&left);
    f.render_widget(Line::from(left), area);
    if lw + rw + 2 <= avail {
        f.render_widget(Line::from(right).alignment(Alignment::Right), area);
    }
}

fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    // a recent action's result replaces the hints for a few seconds
    if let Some(fl) = app.flash.as_ref().filter(|fl| fl.at.elapsed().as_secs() < 4) {
        let (glyph, color) = if fl.error { ("✗", RED) } else if fl.text.ends_with('…') { (spinner(), CYAN) } else { ("✓", GREEN) };
        f.render_widget(
            Line::from(vec![
                Span::raw(" "),
                Span::styled(format!("{glyph} "), Style::new().fg(color)),
                Span::styled(fl.text.clone(), Style::new().fg(if fl.error { RED } else { SOFT })),
            ]),
            area,
        );
        return;
    }
    // (key, label, toggle state, priority: lower survives longer on narrow screens)
    let keys: Vec<(&str, &str, Option<bool>, u8)> = match app.view {
        View::Sessions => vec![
            ("tab", "terminals", None, 1),
            ("↑↓", "move", None, 2),
            ("↵", "go to tab", None, 2),
            ("←→", "fold", None, 5),
            ("J K", "scroll", None, 6),
            ("a", "recent", Some(app.show_recent), 3),
            ("f", "hide finished", Some(app.hide_finished), 4),
            ("q", "quit", None, 0),
        ],
        View::Terminals => vec![
            ("tab", "sessions", None, 1),
            ("↑↓", "move", None, 2),
            ("↵", "go to tab", None, 2),
            ("s", "show session", None, 3),
            ("J K", "scroll", None, 6),
            ("q", "quit", None, 0),
        ],
    };
    let render = |keys: &[(&str, &str, Option<bool>, u8)]| -> Vec<Span<'static>> {
        let mut spans = vec![Span::raw(" ")];
        for (i, (k, v, on, _)) in keys.iter().enumerate() {
            if i > 0 {
                spans.push(Span::styled("  ·  ", Style::new().fg(FAINT)));
            }
            spans.push(Span::styled(k.to_string(), Style::new().fg(SOFT).bold()));
            match on {
                Some(true) => {
                    spans.push(Span::styled(format!(" {v}"), Style::new().fg(VIOLET)));
                    spans.push(Span::styled(" ●", Style::new().fg(VIOLET)));
                }
                _ => spans.push(Span::styled(format!(" {v}"), Style::new().fg(MUTED))),
            }
        }
        spans
    };
    let mut keys = keys;
    let fits = |spans: &[Span]| spans.iter().map(|s| s.width()).sum::<usize>() <= area.width as usize;
    while !fits(&render(&keys)) && keys.len() > 1 {
        let worst = keys.iter().enumerate().max_by_key(|(_, k)| k.3).map(|(i, _)| i).unwrap();
        keys.remove(worst);
    }
    f.render_widget(Line::from(render(&keys)), area);
}

// ── list ──────────────────────────────────────────────────────────────────────
fn draw_list(f: &mut Frame, app: &mut App, area: Rect) {
    let right = |s: String, st: Style| Cell::from(Line::from(Span::styled(s, st)).alignment(Alignment::Right));
    let mut rows: Vec<Row> = Vec::new();
    let n_rows = app.rows.len();
    // Columns give way to the title as the list narrows; everything dropped here is still in
    // the details pane. Order: glyph, title, where, model, status, time, cost.
    let w = area.width;
    let model_w = app
        .rows
        .iter()
        .filter_map(|(_, r)| match r {
            RowRef::Session(i) => Some(short_model(app.snap.sessions[*i].t.model.as_deref().unwrap_or("")).chars().count()),
            RowRef::Agent(i, j) => Some(short_model(&app.snap.sessions[*i].agents[*j].model).chars().count()),
            _ => None,
        })
        .max()
        .unwrap_or(8)
        .clamp(5, 14) as u16;
    let keep = [true, true, w >= 130, w >= 76, true, true, w >= 90];
    let fit = move |v: Vec<Cell<'static>>| Row::new(v.into_iter().zip(keep).filter(|(_, k)| *k).map(|(c, _)| c));

    for (idx, (_, r)) in app.rows.iter().enumerate() {
        match r {
            RowRef::Session(i) => {
                let s = &app.snap.sessions[*i];
                let st = session_status(s);
                let live = s.live();
                let title_style = if live { Style::new().fg(FG).bold() } else { Style::new().fg(MUTED) };
                let fold = if s.agents.is_empty() {
                    " "
                } else if app.is_collapsed(s) {
                    "▸"
                } else {
                    "▾"
                };
                let mut title = vec![
                    Span::styled(format!("{fold} "), Style::new().fg(FAINT)),
                    tool_tag(s.tool, live),
                    Span::styled(one_line(&s.title(), 80), title_style),
                ];
                if !s.agents.is_empty() {
                    let running = s.running_agents();
                    let (txt, c) = if running > 0 {
                        (format!("  ◆ {running}/{}", s.agents.len()), VIOLET)
                    } else {
                        (format!("  ◇ {}", s.agents.len()), FAINT)
                    };
                    title.push(Span::styled(txt, Style::new().fg(c)));
                }
                let dim = |c: Color| if live { c } else { FAINT };
                rows.push(fit(vec![
                    Cell::from(Span::styled(st.glyph, Style::new().fg(st.color))),
                    Cell::from(Line::from(title)),
                    Cell::from(Span::styled(short_path(&s.t.cwd, 16), Style::new().fg(dim(MUTED)))),
                    Cell::from(Span::styled(short_model(s.t.model.as_deref().unwrap_or("")), Style::new().fg(dim(SOFT)))),
                    Cell::from(Span::styled(one_line(&st.label, 20), Style::new().fg(st.color))),
                    right(ago(s.t.last_ts.or(Some(s.mtime))), Style::new().fg(dim(MUTED))),
                    right(money(s.t.cost), Style::new().fg(dim(SOFT))),
                ]));
            }
            RowRef::Agent(i, j) => {
                let a = &app.snap.sessions[*i].agents[*j];
                let st = agent_status(a);
                // last child of its session in the visible list?
                let last = !matches!(app.rows.get(idx + 1), Some((_, RowRef::Agent(si, _))) if si == i);
                let branch = if last { "  └─ " } else { "  ├─ " };
                let id_style = if a.running { Style::new().fg(FG).bold() } else { Style::new().fg(SOFT) };
                rows.push(fit(vec![
                    Cell::from(""),
                    Cell::from(Line::from(vec![
                        Span::styled(branch, Style::new().fg(FAINT)),
                        Span::styled(format!("{} ", st.glyph), Style::new().fg(st.color)),
                        Span::styled(format!("{:<9}", a.id), id_style),
                        Span::styled(one_line(&a.task, 80), Style::new().fg(MUTED)),
                    ])),
                    Cell::from(Span::styled(a.agent.clone(), Style::new().fg(MUTED))),
                    Cell::from(Span::styled(short_model(&a.model), Style::new().fg(MUTED))),
                    Cell::from(Span::styled(one_line(&st.label, 20), Style::new().fg(st.color))),
                    right(
                        if a.running { ago(a.started) } else { dur(a.started, a.finished) },
                        Style::new().fg(MUTED),
                    ),
                    right(money(a.cost), Style::new().fg(MUTED)),
                ]));
            }
            RowRef::Divider => {
                rows.push(fit(vec![
                    Cell::from(""),
                    Cell::from(Span::styled("  recently closed", Style::new().fg(FAINT).add_modifier(Modifier::BOLD))),
                    Cell::from(""),
                    Cell::from(""),
                    Cell::from(""),
                    Cell::from(""),
                    Cell::from(""),
                ]));
            }
            RowRef::Terminal(_) | RowRef::Job(..) => {}
            RowRef::Orphan(k) => {
                let p = &app.snap.orphan_procs[*k];
                let supported = !matches!(p.tool, Tool::Other(_));
                let (glyph, label, color) = if supported {
                    (spinner(), "starting", CYAN)
                } else {
                    ("●", "running", tool_color(p.tool))
                };
                rows.push(fit(vec![
                    Cell::from(Span::styled(glyph, Style::new().fg(color))),
                    Cell::from(Line::from(vec![
                        Span::raw("  "),
                        tool_tag(p.tool, true),
                        Span::styled(format!("{} session", p.tool.label()), Style::new().fg(FG).bold()),
                        Span::styled(format!("  pid {}", p.pid), Style::new().fg(MUTED)),
                    ])),
                    Cell::from(Span::styled(short_path(&p.cwd, 16), Style::new().fg(MUTED))),
                    Cell::from(""),
                    Cell::from(Span::styled(label, Style::new().fg(color))),
                    right(ago(Some(p.start_ms)), Style::new().fg(MUTED)),
                    Cell::from(""),
                ]));
            }
        }
    }

    let head = |s: &'static str| Cell::from(Span::styled(s, Style::new().fg(FAINT).add_modifier(Modifier::BOLD)));
    let head_r = |s: &'static str| {
        Cell::from(Line::from(Span::styled(s, Style::new().fg(FAINT).add_modifier(Modifier::BOLD))).alignment(Alignment::Right))
    };
    let live = app.snap.sessions.iter().filter(|s| s.live() || s.running_agents() > 0).count();
    let recent = app.visible_sessions().count() - live.min(app.visible_sessions().count());
    let count_text = if app.show_recent { format!("{live} live · {recent} recent") } else { format!("{live} live") };
    let count = Span::styled(count_text, Style::new().fg(MUTED));

    let widths: Vec<Constraint> = [
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(16),
        Constraint::Length(model_w),
        Constraint::Length(15),
        Constraint::Length(7),
        Constraint::Length(7),
    ]
    .into_iter()
    .zip(keep)
    .filter(|(_, k)| *k)
    .map(|(c, _)| c)
    .collect();
    let header: Vec<Cell> = [
        head(""),
        head("  SESSION"),
        head("WHERE"),
        head("MODEL"),
        head("STATUS"),
        head_r("TIME"),
        head_r("COST"),
    ]
    .into_iter()
    .zip(keep)
    .filter(|(_, k)| *k)
    .map(|(c, _)| c)
    .collect();
    let table = Table::new(rows, widths)
    .column_spacing(2)
    .header(Row::new(header).bottom_margin(1))
    .row_highlight_style(Style::new().bg(SELECT))
    .highlight_symbol(Span::styled("▌", Style::new().fg(BLUE)))
    .highlight_spacing(ratatui::widgets::HighlightSpacing::Always)
    .block(panel(title_line("Sessions", Some(count)), true).padding(Padding::new(0, 1, 1, 0)));

    f.render_stateful_widget(table, area, &mut app.state);

    if n_rows == 0 {
        let inner = area.inner(Margin { horizontal: 2, vertical: 3 });
        let msg = vec![
            Line::from(Span::styled("No pi sessions running", Style::new().fg(SOFT).bold())),
            Line::default(),
            Line::from(vec![
                Span::styled("Press ", Style::new().fg(MUTED)),
                Span::styled("a", Style::new().fg(FG).bold()),
                Span::styled(" to show sessions from the last day", Style::new().fg(MUTED)),
            ]),
        ];
        f.render_widget(Paragraph::new(msg).alignment(Alignment::Center), inner);
    }
}

// ── detail ────────────────────────────────────────────────────────────────────
fn section(name: &str) -> Line<'static> {
    Line::from(vec![Span::styled(name.to_uppercase(), Style::new().fg(FAINT).bold())])
}

/// A row of label/value pairs: `TURNS 9   TOOLS 9   TOKENS 498k`.
fn stats(pairs: &[(&str, String)]) -> Line<'static> {
    let mut spans = Vec::new();
    for (i, (k, v)) in pairs.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw("    "));
        }
        spans.push(Span::styled(v.clone(), Style::new().fg(FG).bold()));
        spans.push(Span::styled(format!(" {k}"), Style::new().fg(MUTED)));
    }
    Line::from(spans)
}

fn meta(parts: Vec<String>) -> Line<'static> {
    let mut spans = Vec::new();
    for (i, p) in parts.into_iter().filter(|p| !p.is_empty()).enumerate() {
        if i > 0 {
            spans.push(Span::styled("  ·  ", Style::new().fg(FAINT)));
        }
        spans.push(Span::styled(p, Style::new().fg(MUTED)));
    }
    Line::from(spans)
}

fn status_line(st: &Status, extra: Vec<String>) -> Line<'static> {
    let mut spans = vec![
        Span::styled(format!("{} ", st.glyph), Style::new().fg(st.color)),
        Span::styled(st.label.clone(), Style::new().fg(st.color).bold()),
    ];
    for e in extra.into_iter().filter(|e| !e.is_empty()) {
        spans.push(Span::styled("  ·  ", Style::new().fg(FAINT)));
        spans.push(Span::styled(e, Style::new().fg(MUTED)));
    }
    Line::from(spans)
}

/// Inline markdown: **bold** and `code`.
fn inline_md(text: &str, base: Style) -> Vec<Span<'static>> {
    let mut out = Vec::new();
    let (mut bold, mut code) = (false, false);
    let mut buf = String::new();
    let mut chars = text.chars().peekable();
    let flush = |buf: &mut String, out: &mut Vec<Span<'static>>, bold: bool, code: bool| {
        if !buf.is_empty() {
            let mut st = base;
            if code {
                st = st.fg(CYAN);
            }
            if bold {
                st = st.fg(FG).add_modifier(Modifier::BOLD);
            }
            out.push(Span::styled(std::mem::take(buf), st));
        }
    };
    while let Some(c) = chars.next() {
        if c == '`' {
            flush(&mut buf, &mut out, bold, code);
            code = !code;
        } else if c == '*' && chars.peek() == Some(&'*') && !code {
            chars.next();
            flush(&mut buf, &mut out, bold, code);
            bold = !bold;
        } else {
            buf.push(c);
        }
    }
    flush(&mut buf, &mut out, bold, code);
    out
}

/// Light markdown for transcript text: headings, bullets, fences, tables, inline marks.
fn markdown(text: &str, max_lines: usize) -> Vec<Line<'static>> {
    let base = Style::new().fg(SOFT);
    let mut out = Vec::new();
    let mut in_code = false;
    for raw in text.lines().take(max_lines) {
        let t = raw.trim_start();
        if t.starts_with("```") {
            in_code = !in_code;
            continue;
        }
        if in_code {
            out.push(Line::from(vec![
                Span::styled("│ ", Style::new().fg(FAINT)),
                Span::styled(raw.to_string(), Style::new().fg(CYAN)),
            ]));
        } else if let Some(h) = t.strip_prefix('#') {
            let h = h.trim_start_matches('#').trim();
            out.push(Line::from(Span::styled(h.to_string(), Style::new().fg(BLUE).bold())));
        } else if t.starts_with("- ") || t.starts_with("* ") {
            let indent = raw.len() - t.len();
            let mut spans = vec![Span::styled(format!("{}• ", " ".repeat(indent)), Style::new().fg(MUTED))];
            spans.extend(inline_md(&t[2..], base));
            out.push(Line::from(spans));
        } else if t.starts_with('|') {
            if t.chars().all(|c| matches!(c, '|' | '-' | ':' | ' ')) {
                continue; // table rule row
            }
            let cells: Vec<&str> = t.trim_matches('|').split('|').map(str::trim).collect();
            let mut spans = Vec::new();
            for (i, c) in cells.iter().enumerate() {
                if i > 0 {
                    spans.push(Span::styled("  │  ", Style::new().fg(FAINT)));
                }
                spans.extend(inline_md(c, base));
            }
            out.push(Line::from(spans));
        } else {
            out.push(Line::from(inline_md(raw, base)));
        }
    }
    out
}

fn draw_detail(f: &mut Frame, app: &mut App, area: Rect) {
    // paths are shortened to one line (inner width minus borders and padding)
    let path_w = area.width.saturating_sub(6).max(10) as usize;
    let mut lines: Vec<Line> = Vec::new();
    let title: Line;
    match app.selected().cloned() {
        Some(RowRef::Session(i)) => {
            let s = &app.snap.sessions[i];
            let st = session_status(s);
            title = title_line(
                "Session",
                Some(Span::styled(s.tool.label(), Style::new().fg(tool_color(s.tool)).bold())),
            );
            lines.push(Line::from(Span::styled(s.title(), Style::new().fg(FG).bold())));
            let proc_info = match &s.proc {
                Some(p) => vec![
                    format!("pid {}", p.pid),
                    format!("up {}", ago(Some(p.start_ms))),
                    format!("{:.0}% cpu", p.cpu),
                    format!("{} MB", p.mem / 1_048_576),
                ],
                None if s.app_live => vec![
                    format!("{} app", s.t.origin.as_deref().unwrap_or("desktop")),
                    format!("active {} ago", ago(s.t.last_ts.or(Some(s.mtime)))),
                ],
                None => vec![format!("last active {} ago", ago(s.t.last_ts.or(Some(s.mtime))))],
            };
            let mut proc_info = proc_info;
            if let Some(tty) = &s.tty {
                proc_info.insert(0, format!("in {}", tty.trim_start_matches("tty")));
            }
            lines.push(status_line(&st, proc_info));
            lines.push(meta(vec![
                home_rel(&s.t.cwd),
                s.t.model.clone().unwrap_or_default(),
                s.t.thinking.clone().map(|t| format!("{t} thinking")).unwrap_or_default(),
            ]));
            lines.push(Line::default());
            let agent_cost = s.agents.iter().map(|a| a.cost).fold(0.0, |a, b| a + b);
            lines.push(stats(&[
                ("turns", s.t.turns.to_string()),
                ("tools", s.t.tool_calls.to_string()),
                ("tokens", tokens(s.t.tokens)),
                ("context", tokens(s.t.context_tokens)),
            ]));
            if s.tool != Tool::Pi && s.agents.is_empty() {
                // only pi records cost in its transcript
            } else if s.agents.is_empty() {
                lines.push(stats(&[("spent", money(s.t.cost))]));
            } else {
                lines.push(stats(&[
                    ("session", money(s.t.cost)),
                    ("agents", money(agent_cost)),
                    ("running", format!("{}/{}", s.running_agents(), s.agents.len())),
                ]));
            }
            if let Some(u) = &s.t.last_user {
                lines.push(Line::default());
                lines.push(section("Last prompt"));
                lines.extend(markdown(u, 10));
            }
            if let Some(t) = &s.t.last_text {
                lines.push(Line::default());
                lines.push(section("Last reply"));
                lines.extend(markdown(t, 200));
            }
            lines.push(Line::default());
            lines.push(Line::from(Span::styled(short_path(&s.path.display().to_string(), path_w), Style::new().fg(FAINT))));
        }
        Some(RowRef::Agent(i, j)) => {
            let s = &app.snap.sessions[i];
            let a = &s.agents[j];
            let st = agent_status(a);
            title = title_line(format!("Agent {}", a.id), None);
            lines.push(Line::from(vec![
                Span::styled(a.id.clone(), Style::new().fg(FG).bold()),
                Span::styled(format!("  {}", a.agent), Style::new().fg(VIOLET)),
                Span::styled(format!("  in {}", one_line(&s.title(), 40)), Style::new().fg(MUTED)),
            ]));
            let timing = if a.running {
                vec![format!("started {} ago", ago(a.started)), format!("active {} ago", ago(a.last_activity))]
            } else {
                vec![format!("took {}", dur(a.started, a.finished))]
            };
            lines.push(status_line(&st, timing));
            lines.push(meta(vec![
                a.model.clone(),
                if a.effort.is_empty() { String::new() } else { format!("{} effort", a.effort) },
                match a.source {
                    AgentSource::Chalking => "agent_run".into(),
                    AgentSource::PiSubagents => "pi-subagents".into(),
                    AgentSource::ClaudeTask => "Claude Code Task tool".into(),
                },
            ]));
            lines.push(Line::default());
            lines.push(stats(&[
                ("turns", a.turns.to_string()),
                ("tools", a.tool_calls.to_string()),
                ("tokens", tokens(a.tokens)),
                ("cost", money(a.cost)),
            ]));
            lines.push(Line::default());
            lines.push(section("Task"));
            lines.extend(markdown(&a.task, 14));
            if let Some(t) = &a.last_text {
                lines.push(Line::default());
                lines.push(section(if a.running { "Latest output" } else { "Result" }));
                lines.extend(markdown(t, 200));
            }
            if let Some(p) = &a.path {
                lines.push(Line::default());
                lines.push(Line::from(Span::styled(short_path(&p.display().to_string(), path_w), Style::new().fg(FAINT))));
            }
        }
        Some(RowRef::Orphan(k)) => {
            let p = &app.snap.orphan_procs[k];
            title = title_line("Process", None);
            lines.push(Line::from(vec![
                Span::styled(format!("{} ", p.tool.label()), Style::new().fg(tool_color(p.tool)).bold()),
                Span::styled("session", Style::new().fg(FG).bold()),
            ]));
            lines.push(meta(vec![format!("pid {}", p.pid), home_rel(&p.cwd), format!("up {}", ago(Some(p.start_ms)))]));
            lines.push(Line::default());
            lines.push(stats(&[("cpu", format!("{:.0}%", p.cpu)), ("memory", mem_str(p.mem))]));
            lines.push(Line::default());
            let note = if matches!(p.tool, Tool::Other(_)) {
                format!("pitop can't read {}'s transcripts yet, so only the process is shown.", p.tool.label())
            } else {
                format!("{} hasn't written a transcript yet; it will appear here after the first message.", p.tool.label())
            };
            lines.push(Line::from(Span::styled(note, Style::new().fg(MUTED))));
        }
        Some(RowRef::Terminal(i)) => {
            title = title_line("Terminal", None);
            terminal_detail(&app.snap.terminals[i], &mut lines);
        }
        Some(RowRef::Job(i, j)) => {
            let t = &app.snap.terminals[i];
            let p = &t.jobs[j];
            title = title_line("Background job", None);
            lines.push(Line::from(Span::styled(p.cmdline.clone(), Style::new().fg(FG).bold())));
            lines.push(meta(vec![
                format!("pid {}", p.pid),
                format!("in {}", t.short_tty()),
                format!("running {}", ago(Some(p.start_ms))),
            ]));
            lines.push(Line::default());
            lines.push(stats(&[("cpu", format!("{:.0}%", p.cpu)), ("memory", mem_str(p.mem))]));
            lines.push(Line::default());
            lines.push(Line::from(Span::styled(home_rel(&p.cwd), Style::new().fg(FAINT))));
        }
        Some(RowRef::Divider) | None => {
            title = title_line("Details", None);
        }
    }

    // clamp scroll to the content (approximate: wrapped lines are counted after rendering width)
    let inner_w = area.width.saturating_sub(4).max(1) as usize;
    let total: usize = lines.iter().map(|l| l.width().max(1).div_ceil(inner_w)).sum();
    let visible = area.height.saturating_sub(3) as usize;
    let max_scroll = total.saturating_sub(visible) as u16;
    app.detail_scroll = app.detail_scroll.min(max_scroll);
    let mut block = panel(title, false).padding(Padding::new(1, 1, 1, 0));
    if max_scroll > 0 {
        let pct = (app.detail_scroll as usize * 100) / max_scroll as usize;
        block = block.title_bottom(
            Line::from(Span::styled(format!(" {pct}% "), Style::new().fg(MUTED))).alignment(Alignment::Right),
        );
    }
    f.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }).scroll((app.detail_scroll, 0)).block(block),
        area,
    );
}

// ── terminals ─────────────────────────────────────────────────────────────────
pub fn mem_str(b: u64) -> String {
    let mb = b as f64 / 1_048_576.0;
    if mb >= 1024.0 {
        format!("{:.1} GB", mb / 1024.0)
    } else {
        format!("{mb:.0} MB")
    }
}

struct TermState {
    glyph: &'static str,
    color: Color,
}

fn term_state(t: &Terminal) -> TermState {
    if let Some(l) = &t.session {
        TermState { glyph: "◆", color: tool_color(l.tool) }
    } else if t.at_prompt() {
        TermState { glyph: "❯", color: FAINT }
    } else if t.foreground.iter().map(|p| p.cpu).sum::<f32>() >= 5.0 {
        TermState { glyph: "▶", color: AMBER } // busy
    } else {
        TermState { glyph: "▶", color: BLUE } // running, mostly idle (editor, pager, server)
    }
}

fn shell_name(t: &Terminal) -> String {
    t.shell.as_ref().map(|s| s.name.clone()).unwrap_or_else(|| "shell".into())
}

fn draw_terminal_list(f: &mut Frame, app: &mut App, area: Rect) {
    // as wide as the longest folder shown, within limits, so RUNNING gets the rest
    let where_w = app
        .snap
        .terminals
        .iter()
        .flat_map(|t| std::iter::once(t.cwd()).chain(t.jobs.iter().map(|j| j.cwd.clone())))
        .map(|c| short_path(&c, 24).chars().count())
        .max()
        .unwrap_or(5)
        .clamp(5, 24);
    let right = |s: String, st: Style| Cell::from(Line::from(Span::styled(s, st)).alignment(Alignment::Right));
    let show_where = area.width >= 110;
    let fit = |mut v: Vec<Cell<'static>>| {
        if !show_where {
            v.remove(3);
        }
        Row::new(v)
    };
    let mut rows = Vec::new();
    for (idx, (_, r)) in app.rows.iter().enumerate() {
        match r {
            RowRef::Terminal(i) => {
                let t = &app.snap.terminals[*i];
                let st = term_state(t);
                let what: Line = match (&t.session, t.leader()) {
                    (Some(l), _) => Line::from(vec![
                        tool_tag(l.tool, true),
                        Span::styled(one_line(&l.title, 60), Style::new().fg(FG)),
                    ]),
                    (None, Some(p)) => Line::from(Span::styled(one_line(&p.cmdline, 80), Style::new().fg(FG).bold())),
                    (None, None) => Line::from(vec![
                        Span::styled(shell_name(t), Style::new().fg(MUTED)),
                        Span::styled("  at prompt", Style::new().fg(FAINT)),
                    ]),
                };
                let (time, cpu) = match t.leader() {
                    Some(p) => (ago(Some(p.start_ms)), t.foreground.iter().map(|p| p.cpu).sum::<f32>()),
                    None => (ago(t.started), t.cpu),
                };
                let jobs = if t.jobs.is_empty() { String::new() } else { format!("  +{}", t.jobs.len()) };
                rows.push(fit(vec![
                    Cell::from(Span::styled(st.glyph, Style::new().fg(st.color).bold())),
                    Cell::from(Line::from(vec![
                        Span::styled(t.short_tty(), Style::new().fg(SOFT).bold()),
                        Span::styled(format!(" {}", t.app), Style::new().fg(MUTED)),
                    ])),
                    Cell::from(Line::from({
                        let mut v = what.spans;
                        v.push(Span::styled(jobs, Style::new().fg(BLUE)));
                        v
                    })),
                    Cell::from(Span::styled(short_path(&t.cwd(), where_w), Style::new().fg(MUTED))),
                    right(time, Style::new().fg(MUTED)),
                    right(format!("{cpu:.0}%"), Style::new().fg(if cpu >= 5.0 { AMBER } else { MUTED })),
                    right(mem_str(t.mem), Style::new().fg(MUTED)),
                ]));
            }
            RowRef::Job(i, j) => {
                let p = &app.snap.terminals[*i].jobs[*j];
                let last = !matches!(app.rows.get(idx + 1), Some((_, RowRef::Job(ti, _))) if ti == i);
                rows.push(fit(vec![
                    Cell::from(""),
                    Cell::from(Span::styled(if last { "  └─ &" } else { "  ├─ &" }, Style::new().fg(FAINT))),
                    Cell::from(Span::styled(one_line(&p.cmdline, 80), Style::new().fg(SOFT))),
                    Cell::from(Span::styled(short_path(&p.cwd, where_w), Style::new().fg(FAINT))),
                    right(ago(Some(p.start_ms)), Style::new().fg(MUTED)),
                    right(format!("{:.0}%", p.cpu), Style::new().fg(MUTED)),
                    right(mem_str(p.mem), Style::new().fg(MUTED)),
                ]));
            }
            _ => {}
        }
    }

    let head = |s: &'static str| Cell::from(Span::styled(s, Style::new().fg(FAINT).add_modifier(Modifier::BOLD)));
    let head_r = |s: &'static str| {
        Cell::from(Line::from(Span::styled(s, Style::new().fg(FAINT).add_modifier(Modifier::BOLD))).alignment(Alignment::Right))
    };
    let mut widths = vec![
        Constraint::Length(1),
        Constraint::Length(14),
        Constraint::Fill(1),
        Constraint::Length(where_w as u16),
        Constraint::Length(7),
        Constraint::Length(5),
        Constraint::Length(8),
    ];
    let mut header = vec![head(""), head("TTY"), head("RUNNING"), head("WHERE"), head_r("TIME"), head_r("CPU"), head_r("MEM")];
    if !show_where {
        widths.remove(3);
        header.remove(3);
    }
    let n = app.snap.terminals.len();
    let busy = app.snap.terminals.iter().filter(|t| !t.at_prompt()).count();
    let count = Span::styled(format!("{n} open · {busy} running"), Style::new().fg(MUTED));
    let table = Table::new(rows, widths)
        .column_spacing(2)
        .header(Row::new(header).bottom_margin(1))
        .row_highlight_style(Style::new().bg(SELECT))
        .highlight_symbol(Span::styled("▌", Style::new().fg(BLUE)))
        .highlight_spacing(ratatui::widgets::HighlightSpacing::Always)
        .block(panel(title_line("Terminals", Some(count)), true).padding(Padding::new(0, 1, 1, 0)));
    f.render_stateful_widget(table, area, &mut app.state);

    if n == 0 {
        let inner = area.inner(Margin { horizontal: 2, vertical: 3 });
        f.render_widget(
            Paragraph::new(Line::from(Span::styled("No open terminals found", Style::new().fg(SOFT).bold())))
                .alignment(Alignment::Center),
            inner,
        );
    }
}

fn terminal_detail(t: &Terminal, lines: &mut Vec<Line<'static>>) {
    let st = term_state(t);
    let headline = match (&t.session, t.leader()) {
        (Some(l), _) => format!("{} · {}", l.tool.label(), l.title),
        (None, Some(p)) => p.cmdline.clone(),
        (None, None) => format!("{} at prompt", shell_name(t)),
    };
    lines.push(Line::from(Span::styled(headline, Style::new().fg(FG).bold())));
    let state = Status {
        glyph: st.glyph.into(),
        label: if t.at_prompt() { "idle".into() } else { "running".into() },
        color: st.color,
    };
    let mut extra = vec![t.tty.clone(), t.app.clone()];
    if let Some(p) = t.leader() {
        extra.push(format!("for {}", ago(Some(p.start_ms))));
    }
    lines.push(status_line(&state, extra));
    lines.push(meta(vec![
        home_rel(&t.cwd()),
        t.shell.as_ref().map(|s| format!("{} pid {}", s.name, s.pid)).unwrap_or_default(),
        format!("open {}", ago(t.started)),
    ]));
    lines.push(Line::default());
    lines.push(stats(&[
        ("cpu", format!("{:.0}%", t.cpu)),
        ("memory", mem_str(t.mem)),
        (if t.procs == 1 { "process" } else { "processes" }, t.procs.to_string()),
        (if t.jobs.len() == 1 { "job" } else { "jobs" }, t.jobs.len().to_string()),
    ]));
    if let Some(l) = &t.session {
        lines.push(Line::default());
        lines.push(section(&format!("{} session", l.tool.label())));
        lines.push(Line::from(Span::styled(l.title.clone(), Style::new().fg(SOFT))));
        lines.push(Line::from(vec![
            Span::styled("s", Style::new().fg(FG).bold()),
            Span::styled(" show it in Sessions   ", Style::new().fg(MUTED)),
            Span::styled("↵", Style::new().fg(FG).bold()),
            Span::styled(" go to this tab", Style::new().fg(MUTED)),
        ]));
    }
    if !t.foreground.is_empty() {
        lines.push(Line::default());
        lines.push(section("Foreground"));
        for p in &t.foreground {
            lines.push(Line::from(vec![
                Span::styled(format!("{:>6}  ", p.pid), Style::new().fg(FAINT)),
                Span::styled(p.cmdline.clone(), Style::new().fg(SOFT)),
            ]));
        }
    }
    if !t.jobs.is_empty() {
        lines.push(Line::default());
        lines.push(section("Background jobs"));
        for p in &t.jobs {
            lines.push(Line::from(vec![
                Span::styled(format!("{:>6}  ", p.pid), Style::new().fg(FAINT)),
                Span::styled(p.cmdline.clone(), Style::new().fg(SOFT)),
            ]));
        }
    }
}
