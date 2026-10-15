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

// ── palette ───────────────────────────────────────────────────────────────────
// Two themes with the same roles. Dark leaves the background to the terminal; light paints its
// own paper background so it reads correctly in a dark terminal too.
pub struct Palette {
    pub fg: Color,
    pub soft: Color,
    pub muted: Color,
    pub faint: Color,
    pub border: Color,
    pub border_focus: Color,
    pub select: Color,
    pub blue: Color,
    pub cyan: Color,
    pub green: Color,
    pub amber: Color,
    pub red: Color,
    pub violet: Color,
    pub orange: Color,
    pub teal: Color,
    /// wordmark shadow
    pub shadow: Color,
    /// text on an accent-coloured badge
    pub on_accent: Color,
    /// painted background (None: the terminal's own)
    pub bg: Option<Color>,
}

impl Palette {
    /// The background to use where a cell is explicitly cleared.
    pub fn base(&self) -> Color {
        self.bg.unwrap_or(Color::Reset)
    }
}

const fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::Rgb(r, g, b)
}

pub const DARK: Palette = Palette {
    fg: rgb(192, 202, 245),
    soft: rgb(169, 177, 214),
    muted: rgb(108, 116, 158),
    faint: rgb(68, 75, 106),
    border: rgb(54, 60, 88),
    border_focus: rgb(92, 104, 150),
    select: rgb(36, 42, 66),
    blue: rgb(122, 162, 247),
    cyan: rgb(125, 207, 255),
    green: rgb(158, 206, 106),
    amber: rgb(224, 175, 104),
    red: rgb(247, 118, 142),
    violet: rgb(187, 154, 247),
    orange: rgb(255, 158, 100),
    teal: rgb(115, 218, 202),
    shadow: rgb(46, 52, 78),
    on_accent: rgb(26, 27, 38),
    bg: None,
};

/// Text colours hold at least 4.5:1 on the paper background, except `faint`, which is only
/// used for decoration (tree lines, dividers, shadows).
pub const LIGHT: Palette = Palette {
    fg: rgb(43, 48, 74),
    soft: rgb(66, 73, 104),
    muted: rgb(96, 104, 136),
    faint: rgb(138, 145, 174),
    border: rgb(208, 212, 226),
    border_focus: rgb(150, 158, 190),
    select: rgb(222, 228, 246),
    blue: rgb(40, 84, 196),
    cyan: rgb(0, 106, 150),
    green: rgb(46, 112, 34),
    amber: rgb(140, 88, 0),
    red: rgb(190, 34, 64),
    violet: rgb(112, 62, 196),
    orange: rgb(178, 82, 14),
    teal: rgb(14, 118, 108),
    shadow: rgb(214, 218, 232),
    on_accent: rgb(255, 255, 255),
    bg: Some(rgb(248, 249, 252)),
};

static LIGHT_MODE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn set_light(on: bool) {
    LIGHT_MODE.store(on, std::sync::atomic::Ordering::Relaxed);
}

pub fn is_light() -> bool {
    LIGHT_MODE.load(std::sync::atomic::Ordering::Relaxed)
}

/// The active palette.
pub fn pal() -> &'static Palette {
    if is_light() { &LIGHT } else { &DARK }
}

pub fn tool_color(t: Tool) -> Color {
    match t {
        Tool::Pi => pal().blue,
        Tool::Claude => pal().orange,
        Tool::Codex => pal().teal,
        Tool::Other(_) => pal().violet,
    }
}

/// Fixed-width coloured agent tag: `claude`, `pi    `, `codex `.
fn tool_tag(t: Tool, live: bool) -> Span<'static> {
    let c = if live { tool_color(t) } else { pal().faint };
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
        return Status { glyph: "○".into(), label: "closed".into(), color: pal().faint };
    }
    if s.t.is_busy() {
        let label = match (s.t.last_role.as_deref(), s.t.last_stop.as_deref()) {
            (Some("assistant"), Some("toolUse")) => s.t.last_tool.clone().unwrap_or_else(|| "tool".into()),
            _ => "thinking".into(),
        };
        return Status { glyph: spinner().into(), label, color: pal().amber };
    }
    match s.t.last_stop.as_deref() {
        Some("error") => Status { glyph: "●".into(), label: "error".into(), color: pal().red },
        Some("aborted") => Status { glyph: "●".into(), label: "interrupted".into(), color: pal().muted },
        _ if s.t.last_role.is_none() => Status { glyph: "●".into(), label: "new".into(), color: pal().cyan },
        _ => Status { glyph: "●".into(), label: "waiting for you".into(), color: pal().green },
    }
}

fn agent_status(a: &Agent) -> Status {
    if a.running {
        let label = a.activity.trim_start_matches("⚙ ").to_string();
        return Status { glyph: spinner().into(), label, color: pal().amber };
    }
    match a.status.as_str() {
        "done" | "complete" => Status { glyph: "✓".into(), label: "done".into(), color: pal().green },
        "error" | "failed" | "rejected" => Status { glyph: "✗".into(), label: a.status.clone(), color: pal().red },
        "stale" => Status { glyph: "◌".into(), label: "orphaned".into(), color: pal().muted },
        other => Status { glyph: "·".into(), label: other.into(), color: pal().muted },
    }
}

fn panel<'a>(title: Line<'a>, focused: bool) -> Block<'a> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(if focused { pal().border_focus } else { pal().border }))
        .title(title)
        .padding(Padding::horizontal(1))
}

fn title_line(text: impl Into<String>, extra: Option<Span<'static>>) -> Line<'static> {
    let mut spans = vec![Span::raw(" "), Span::styled(text.into(), Style::new().fg(pal().fg).bold())];
    if let Some(e) = extra {
        spans.push(Span::raw(" "));
        spans.push(e);
    }
    spans.push(Span::raw(" "));
    Line::from(spans)
}

// ── wordmark ──────────────────────────────────────────────────────────────────
// WHOSUP in CHALKING's style (chunky block letters with a drop shadow), smaller: 4 rows instead
// of 6. Letters are a 7-pixel-tall font with 2-pixel stems and 1-pixel bars, drawn with half
// blocks so each character cell holds two square pixels; the shadow sits one pixel down-right.
const GLYPHS: [(char, [&str; 7]); 6] = [
    ('W', ["##....##", "##....##", "##....##", "##.##.##", "##.##.##", "########", ".##..##."]),
    ('H', ["##..##", "##..##", "##..##", "######", "##..##", "##..##", "##..##"]),
    ('O', [".####.", "##..##", "##..##", "##..##", "##..##", "##..##", ".####."]),
    ('S', [".#####", "##....", "##....", ".####.", "....##", "....##", "#####."]),
    ('U', ["##..##", "##..##", "##..##", "##..##", "##..##", "##..##", ".####."]),
    ('P', ["#####.", "##..##", "##..##", "#####.", "##....", "##....", "##...."]),
];

/// Pixel grid for `word`: 0 empty, 1 letter, 2 shadow; 8 pixel rows (7 + shadow).
fn wordmark_pixels(word: &str) -> Vec<Vec<u8>> {
    let mut rows = vec![String::new(); 7];
    let letters: Vec<char> = word.chars().collect();
    for (i, ch) in letters.iter().enumerate() {
        let Some((_, g)) = GLYPHS.iter().find(|(c, _)| c == ch) else { continue };
        for y in 0..7 {
            rows[y].push_str(g[y]);
            if i + 1 < letters.len() {
                rows[y].push('.');
            }
        }
    }
    let w = rows[0].len() + 1;
    let mut px = vec![vec![0u8; w]; 8];
    for (y, r) in rows.iter().enumerate() {
        for (x, c) in r.chars().enumerate() {
            if c == '#' {
                px[y][x] = 1;
            }
        }
    }
    for y in 1..8 {
        for x in 1..w {
            if px[y][x] == 0 && px[y - 1][x - 1] == 1 {
                px[y][x] = 2;
            }
        }
    }
    px
}

pub const WORDMARK_WIDTH: u16 = 44;

fn mix(a: Color, b: Color, t: f32) -> Color {
    match (a, b) {
        (Color::Rgb(r1, g1, b1), Color::Rgb(r2, g2, b2)) => {
            let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
            Color::Rgb(l(r1, r2), l(g1, g2), l(b1, b2))
        }
        _ => a,
    }
}

/// Four lines of half-block art; letters run blue → violet with the shadow behind them.
fn wordmark() -> Vec<Line<'static>> {
    let px = wordmark_pixels("WHOSUP");
    let w = px[0].len();
    let p = pal();
    let color = |v: u8, x: usize| match v {
        1 => Some(mix(p.blue, p.violet, x as f32 / (w.max(2) - 1) as f32)),
        2 => Some(p.shadow),
        _ => None,
    };
    (0..4)
        .map(|row| {
            let spans: Vec<Span> = (0..w)
                .map(|x| {
                    let (top, bot) = (color(px[row * 2][x], x), color(px[row * 2 + 1][x], x));
                    match (top, bot) {
                        (None, None) => Span::raw(" "),
                        (Some(t), None) => Span::styled("▀", Style::new().fg(t).bg(p.base())),
                        (None, Some(b)) => Span::styled("▄", Style::new().fg(b).bg(p.base())),
                        (Some(t), Some(b)) if t == b => Span::styled("█", Style::new().fg(t)),
                        (Some(t), Some(b)) => Span::styled("▀", Style::new().fg(t).bg(b)),
                    }
                })
                .collect();
            Line::from(spans)
        })
        .collect()
}

// ── frame ─────────────────────────────────────────────────────────────────────
pub fn draw(f: &mut Frame, app: &mut App) {
    let p = pal();
    if let Some(bg) = p.bg {
        f.render_widget(Block::new().style(Style::new().bg(bg).fg(p.fg)), f.area());
    }
    let area = f.area().inner(Margin { horizontal: 1, vertical: 0 });
    // the wordmark header needs room; short or narrow windows get the one-line header
    let tall = area.height >= 26 && area.width >= WORDMARK_WIDTH + 40;
    let head_h = if tall { 4 } else { 1 };
    let [header, _, body, footer] = Layout::vertical([
        Constraint::Length(head_h),
        Constraint::Length(1),
        Constraint::Min(5),
        Constraint::Length(1),
    ])
    .areas(area);

    if tall {
        draw_tall_header(f, app, header);
    } else {
        draw_header(f, app, header);
    }

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

struct HeaderParts {
    tabs: Vec<Span<'static>>,
    stats: Vec<Span<'static>>,
    right: Vec<Span<'static>>,
}

fn header_parts(app: &App) -> HeaderParts {
    let p = pal();
    let sessions = &app.snap.sessions;
    let live = sessions.iter().filter(|s| s.live()).count();
    let working = sessions.iter().filter(|s| s.live() && s.t.is_busy()).count();
    let agents: usize = sessions.iter().map(|s| s.running_agents()).sum();
    let cost = sessions
        .iter()
        .filter(|s| s.live())
        .map(|s| s.t.cost + s.agents.iter().map(|a| a.cost).sum::<f64>())
        .fold(0.0, |a, b| a + b);
    let terms = &app.snap.terminals;

    let sep = || Span::raw("   ");
    let stat = |n: usize, label: &str, color: Color, glyph: &str| -> Vec<Span<'static>> {
        let c = if n == 0 { p.muted } else { color };
        vec![
            Span::styled(format!("{glyph} "), Style::new().fg(c)),
            Span::styled(n.to_string(), Style::new().fg(if n == 0 { p.muted } else { p.fg }).bold()),
            Span::styled(format!(" {label}"), Style::new().fg(p.muted)),
        ]
    };
    let tab = |label: &str, n: usize, key: &str, active: bool| -> Vec<Span<'static>> {
        let (fg, bg) = if active { (p.fg, p.select) } else { (p.muted, p.base()) };
        vec![
            Span::styled(format!(" {key} "), Style::new().fg(if active { p.blue } else { p.faint }).bg(bg)),
            Span::styled(
                label.to_string(),
                Style::new().fg(fg).bg(bg).add_modifier(if active { Modifier::BOLD } else { Modifier::empty() }),
            ),
            Span::styled(format!(" {n} "), Style::new().fg(if active { p.soft } else { p.faint }).bg(bg)),
        ]
    };
    let mut tabs = tab("Sessions", live, "1", app.view == View::Sessions);
    tabs.push(Span::raw(" "));
    tabs.extend(tab("Terminals", terms.len(), "2", app.view == View::Terminals));

    let mut stats = Vec::new();
    let right: Vec<Span> = if app.view == View::Sessions {
        let needs = sessions.iter().filter(|s| s.needs_you()).count();
        stats.extend(stat(needs, if needs == 1 { "needs you" } else { "need you" }, p.green, "●"));
        stats.push(sep());
        stats.extend(stat(working, "working", p.amber, if working > 0 { spinner() } else { "◌" }));
        stats.push(sep());
        stats.extend(stat(agents, if agents == 1 { "agent running" } else { "agents running" }, p.violet, "◆"));
        vec![
            Span::styled(money(cost), Style::new().fg(p.fg).bold()),
            Span::styled(" spent in live sessions", Style::new().fg(p.muted)),
        ]
    } else {
        let running = terms.iter().filter(|t| !t.at_prompt()).count();
        let with_agent = terms.iter().filter(|t| t.session.is_some()).count();
        stats.extend(stat(running, "running", p.blue, "▶"));
        stats.push(sep());
        stats.extend(stat(with_agent, "with agents", p.violet, "◆"));
        let cpu: f32 = terms.iter().map(|t| t.cpu).sum();
        let mem: u64 = terms.iter().map(|t| t.mem).sum();
        vec![
            Span::styled(format!("{cpu:.0}%"), Style::new().fg(p.fg).bold()),
            Span::styled(" cpu   ", Style::new().fg(p.muted)),
            Span::styled(mem_str(mem), Style::new().fg(p.fg).bold()),
            Span::styled(" memory", Style::new().fg(p.muted)),
        ]
    };
    HeaderParts { tabs, stats, right }
}

fn spans_width(v: &[Span]) -> usize {
    v.iter().map(|s| s.width()).sum()
}

/// Drop whole stats from the end (they're separated by 3-space spans) until the rest fits,
/// so the first one, "needs you", is the last to go.
fn fit_stats(mut stats: Vec<Span<'static>>, avail: usize) -> Vec<Span<'static>> {
    while spans_width(&stats) > avail {
        match stats.iter().rposition(|s| s.content == "   ") {
            Some(i) if i > 0 => stats.truncate(i),
            _ => return vec![],
        }
    }
    stats
}

/// Wordmark on the left; tabs, live stats and totals stacked beside it.
fn draw_tall_header(f: &mut Frame, app: &App, area: Rect) {
    let art = wordmark();
    let art_w = art[0].width() as u16;
    let [art_area, _, info] =
        Layout::horizontal([Constraint::Length(art_w), Constraint::Length(4), Constraint::Fill(1)]).areas(area);
    f.render_widget(Paragraph::new(art), art_area);
    let h = header_parts(app);
    // centred against the 4-row wordmark: tabs, stats, totals on rows 1–3
    let rows = Layout::vertical([Constraint::Length(1); 4]).split(info);
    let rows = [rows[0], rows[1], rows[2]];
    f.render_widget(Line::from(h.tabs), rows[0]);
    let mut stats = fit_stats(h.stats, rows[1].width.saturating_sub(1) as usize);
    stats.insert(0, Span::raw(" "));
    f.render_widget(Line::from(stats), rows[1]);
    let mut right = h.right;
    right.insert(0, Span::raw(" "));
    if spans_width(&right) <= rows[2].width as usize {
        f.render_widget(Line::from(right), rows[2]);
    }
}

/// One line: name badge, tabs, then whatever else fits.
fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    let p = pal();
    let h = header_parts(app);
    let mut core = vec![
        Span::styled(" whosup ", Style::new().fg(p.on_accent).bg(p.blue).bold()),
        Span::raw("  "),
    ];
    core.extend(h.tabs);
    let mut stats = vec![Span::raw("   ")];
    stats.extend(h.stats);
    let mut right = h.right;
    right.push(Span::raw(" "));
    // Drop the least important parts first so nothing ever overlaps: totals, then stats.
    let avail = area.width as usize;
    let (cw, sw, rw) = (spans_width(&core), spans_width(&stats), spans_width(&right));
    let mut left = core;
    if cw + sw <= avail {
        left.extend(stats);
    }
    let lw = spans_width(&left);
    f.render_widget(Line::from(left), area);
    if lw + rw + 2 <= avail {
        f.render_widget(Line::from(right).alignment(Alignment::Right), area);
    }
}

fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    // a recent action's result replaces the hints for a few seconds
    if let Some(fl) = app.flash.as_ref().filter(|fl| fl.at.elapsed().as_secs() < 4) {
        let (glyph, color) = if fl.error { ("✗", pal().red) } else if fl.text.ends_with('…') { (spinner(), pal().cyan) } else { ("✓", pal().green) };
        f.render_widget(
            Line::from(vec![
                Span::raw(" "),
                Span::styled(format!("{glyph} "), Style::new().fg(color)),
                Span::styled(fl.text.clone(), Style::new().fg(if fl.error { pal().red } else { pal().soft })),
            ]),
            area,
        );
        return;
    }
    // Enter resumes a closed session and jumps to a live one
    let enter_label = match app.selected_target() {
        Ok(crate::Target::Resume(_)) => "resume",
        _ => "go to its tab",
    };
    // (key, label, toggle state, priority: lower survives longer on narrow screens)
    let keys: Vec<(&str, &str, Option<bool>, u8)> = match app.view {
        View::Sessions => vec![
            ("↵", enter_label, None, 0),
            ("tab", "terminals", None, 1),
            ("↑↓", "move", None, 2),
            ("←→", "fold", None, 6),
            ("J K", "scroll", None, 7),
            ("a", "recent", Some(app.show_recent), 3),
            ("f", "hide finished", Some(app.hide_finished), 4),
            ("n", "notify", Some(app.settings.notify), 4),
            ("t", "theme", None, 5),
            ("q", "quit", None, 0),
        ],
        View::Terminals => vec![
            ("↵", "go to tab", None, 0),
            ("tab", "sessions", None, 1),
            ("↑↓", "move", None, 2),
            ("s", "show session", None, 3),
            ("J K", "scroll", None, 7),
            ("n", "notify", Some(app.settings.notify), 4),
            ("t", "theme", None, 5),
            ("q", "quit", None, 0),
        ],
    };
    let render = |keys: &[(&str, &str, Option<bool>, u8)]| -> Vec<Span<'static>> {
        let mut spans = vec![Span::raw(" ")];
        for (i, (k, v, on, _)) in keys.iter().enumerate() {
            if i > 0 {
                spans.push(Span::styled("  ·  ", Style::new().fg(pal().faint)));
            }
            // Enter is the main action: its key is drawn in the accent colour
            let key_color = if *k == "↵" { pal().blue } else { pal().soft };
            spans.push(Span::styled(k.to_string(), Style::new().fg(key_color).bold()));
            match on {
                Some(true) => {
                    spans.push(Span::styled(format!(" {v}"), Style::new().fg(pal().violet)));
                    spans.push(Span::styled(" ●", Style::new().fg(pal().violet)));
                }
                _ => spans.push(Span::styled(format!(" {v}"), Style::new().fg(pal().muted))),
            }
        }
        spans
    };
    let mut keys = keys;
    let fits = |spans: &[Span]| spans.iter().map(|s| s.width()).sum::<usize>() <= area.width as usize;
    while !fits(&render(&keys)) && keys.iter().any(|k| k.3 > 0) {
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
    // glyph, title, where, in, model, status, time, cost
    let keep = [true, true, w >= 140, w >= 70, w >= 100, true, true, w >= 90];
    let fit = move |v: Vec<Cell<'static>>| Row::new(v.into_iter().zip(keep).filter(|(_, k)| *k).map(|(c, _)| c));

    for (idx, (_, r)) in app.rows.iter().enumerate() {
        match r {
            RowRef::Session(i) => {
                let s = &app.snap.sessions[*i];
                let st = session_status(s);
                let live = s.live();
                let title_style = if live { Style::new().fg(pal().fg).bold() } else { Style::new().fg(pal().muted) };
                let fold = if s.agents.is_empty() {
                    " "
                } else if app.is_collapsed(s) {
                    "▸"
                } else {
                    "▾"
                };
                let mut title = vec![
                    Span::styled(format!("{fold} "), Style::new().fg(pal().faint)),
                    tool_tag(s.tool, live),
                    Span::styled(one_line(&s.title(), 80), title_style),
                ];
                if !s.agents.is_empty() {
                    let running = s.running_agents();
                    let (txt, c) = if running > 0 {
                        (format!("  ◆ {running}/{}", s.agents.len()), pal().violet)
                    } else {
                        (format!("  ◇ {}", s.agents.len()), pal().faint)
                    };
                    title.push(Span::styled(txt, Style::new().fg(c)));
                }
                let dim = |c: Color| if live { c } else { pal().faint };
                rows.push(fit(vec![
                    Cell::from(Span::styled(st.glyph, Style::new().fg(st.color))),
                    Cell::from(Line::from(title)),
                    Cell::from(Span::styled(short_path(&s.t.cwd, 16), Style::new().fg(dim(pal().muted)))),
                    Cell::from(Span::styled(runs_in(s, app.hosts.get(&s.key)), Style::new().fg(dim(pal().soft)))),
                    Cell::from(Span::styled(short_model(s.t.model.as_deref().unwrap_or("")), Style::new().fg(dim(pal().soft)))),
                    // waiting for you / error: bold, so they stand out from the rest
                    Cell::from(Span::styled(
                        one_line(&st.label, 20),
                        if s.needs_you() { Style::new().fg(st.color).bold() } else { Style::new().fg(st.color) },
                    )),
                    right(ago(s.t.last_ts.or(Some(s.mtime))), Style::new().fg(dim(pal().muted))),
                    right(money(s.t.cost), Style::new().fg(dim(pal().soft))),
                ]));
            }
            RowRef::Agent(i, j) => {
                let a = &app.snap.sessions[*i].agents[*j];
                let st = agent_status(a);
                // last child of its session in the visible list?
                let last = !matches!(app.rows.get(idx + 1), Some((_, RowRef::Agent(si, _))) if si == i);
                let branch = if last { "  └─ " } else { "  ├─ " };
                let id_style = if a.running { Style::new().fg(pal().fg).bold() } else { Style::new().fg(pal().soft) };
                rows.push(fit(vec![
                    Cell::from(""),
                    Cell::from(Line::from(vec![
                        Span::styled(branch, Style::new().fg(pal().faint)),
                        Span::styled(format!("{} ", st.glyph), Style::new().fg(st.color)),
                        Span::styled(format!("{:<9}", a.id), id_style),
                        Span::styled(one_line(&a.task, 80), Style::new().fg(pal().muted)),
                    ])),
                    Cell::from(Span::styled(a.agent.clone(), Style::new().fg(pal().muted))),
                    Cell::from(""),
                    Cell::from(Span::styled(short_model(&a.model), Style::new().fg(pal().muted))),
                    Cell::from(Span::styled(one_line(&st.label, 20), Style::new().fg(st.color))),
                    right(
                        if a.running { ago(a.started) } else { dur(a.started, a.finished) },
                        Style::new().fg(pal().muted),
                    ),
                    right(money(a.cost), Style::new().fg(pal().muted)),
                ]));
            }
            RowRef::Divider => {
                rows.push(fit(vec![
                    Cell::from(""),
                    Cell::from(Span::styled("  recently closed", Style::new().fg(pal().faint).add_modifier(Modifier::BOLD))),
                    Cell::from(""),
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
                    (spinner(), "starting", pal().cyan)
                } else {
                    ("●", "running", tool_color(p.tool))
                };
                rows.push(fit(vec![
                    Cell::from(Span::styled(glyph, Style::new().fg(color))),
                    Cell::from(Line::from(vec![
                        Span::raw("  "),
                        tool_tag(p.tool, true),
                        Span::styled(format!("{} session", p.tool.label()), Style::new().fg(pal().fg).bold()),
                        Span::styled(format!("  pid {}", p.pid), Style::new().fg(pal().muted)),
                    ])),
                    Cell::from(Span::styled(short_path(&p.cwd, 16), Style::new().fg(pal().muted))),
                    Cell::from(Span::styled(p.host.as_deref().map(short_app).unwrap_or_default(), Style::new().fg(pal().soft))),
                    Cell::from(""),
                    Cell::from(Span::styled(label, Style::new().fg(color))),
                    right(ago(Some(p.start_ms)), Style::new().fg(pal().muted)),
                    Cell::from(""),
                ]));
            }
        }
    }

    let head = |s: &'static str| Cell::from(Span::styled(s, Style::new().fg(pal().faint).add_modifier(Modifier::BOLD)));
    let head_r = |s: &'static str| {
        Cell::from(Line::from(Span::styled(s, Style::new().fg(pal().faint).add_modifier(Modifier::BOLD))).alignment(Alignment::Right))
    };
    let live = app.snap.sessions.iter().filter(|s| s.live() || s.running_agents() > 0).count();
    let recent = app.visible_sessions().count() - live.min(app.visible_sessions().count());
    let count_text = if app.show_recent { format!("{live} live · {recent} recent") } else { format!("{live} live") };
    let count = Span::styled(count_text, Style::new().fg(pal().muted));

    let widths: Vec<Constraint> = [
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(16),
        Constraint::Length(9),
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
        head("IN"),
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
    .row_highlight_style(Style::new().bg(pal().select))
    .highlight_symbol(Span::styled("▌", Style::new().fg(pal().blue)))
    .highlight_spacing(ratatui::widgets::HighlightSpacing::Always)
    .block(panel(title_line("Sessions", Some(count)), true).padding(Padding::new(0, 1, 1, 0)));

    f.render_stateful_widget(table, area, &mut app.state);

    if n_rows == 0 {
        let inner = area.inner(Margin { horizontal: 2, vertical: 3 });
        let msg = vec![
            Line::from(Span::styled("No pi sessions running", Style::new().fg(pal().soft).bold())),
            Line::default(),
            Line::from(vec![
                Span::styled("Press ", Style::new().fg(pal().muted)),
                Span::styled("a", Style::new().fg(pal().fg).bold()),
                Span::styled(" to show sessions from the last day", Style::new().fg(pal().muted)),
            ]),
        ];
        f.render_widget(Paragraph::new(msg).alignment(Alignment::Center), inner);
    }
}

// ── detail ────────────────────────────────────────────────────────────────────
fn section(name: &str) -> Line<'static> {
    Line::from(vec![Span::styled(name.to_uppercase(), Style::new().fg(pal().faint).bold())])
}

/// A row of label/value pairs: `TURNS 9   TOOLS 9   TOKENS 498k`.
fn stats(pairs: &[(&str, String)]) -> Line<'static> {
    let mut spans = Vec::new();
    for (i, (k, v)) in pairs.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw("    "));
        }
        spans.push(Span::styled(v.clone(), Style::new().fg(pal().fg).bold()));
        spans.push(Span::styled(format!(" {k}"), Style::new().fg(pal().muted)));
    }
    Line::from(spans)
}

fn meta(parts: Vec<String>) -> Line<'static> {
    let mut spans = Vec::new();
    for (i, p) in parts.into_iter().filter(|p| !p.is_empty()).enumerate() {
        if i > 0 {
            spans.push(Span::styled("  ·  ", Style::new().fg(pal().faint)));
        }
        spans.push(Span::styled(p, Style::new().fg(pal().muted)));
    }
    Line::from(spans)
}

fn status_line(st: &Status, extra: Vec<String>) -> Line<'static> {
    let mut spans = vec![
        Span::styled(format!("{} ", st.glyph), Style::new().fg(st.color)),
        Span::styled(st.label.clone(), Style::new().fg(st.color).bold()),
    ];
    for e in extra.into_iter().filter(|e| !e.is_empty()) {
        spans.push(Span::styled("  ·  ", Style::new().fg(pal().faint)));
        spans.push(Span::styled(e, Style::new().fg(pal().muted)));
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
                st = st.fg(pal().cyan);
            }
            if bold {
                st = st.fg(pal().fg).add_modifier(Modifier::BOLD);
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
    let base = Style::new().fg(pal().soft);
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
                Span::styled("│ ", Style::new().fg(pal().faint)),
                Span::styled(raw.to_string(), Style::new().fg(pal().cyan)),
            ]));
        } else if let Some(h) = t.strip_prefix('#') {
            let h = h.trim_start_matches('#').trim();
            out.push(Line::from(Span::styled(h.to_string(), Style::new().fg(pal().blue).bold())));
        } else if t.starts_with("- ") || t.starts_with("* ") {
            let indent = raw.len() - t.len();
            let mut spans = vec![Span::styled(format!("{}• ", " ".repeat(indent)), Style::new().fg(pal().muted))];
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
                    spans.push(Span::styled("  │  ", Style::new().fg(pal().faint)));
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

/// `s001`, or the app a session runs in when it has no terminal (`VS Code`, `Codex`).
fn runs_in(s: &Session, last_host: Option<&String>) -> String {
    if !s.live() {
        // closed: where it last ran, which is where ↵ resumes it
        return last_host.map(|h| short_app(h)).unwrap_or_default();
    }
    match (&s.tty, &s.host) {
        (Some(t), _) => t.trim_start_matches("tty").to_string(),
        (None, Some(h)) if h != "editor" => short_app(h),
        (None, Some(_)) => "editor".into(),
        _ => String::new(),
    }
}

fn short_app(name: &str) -> String {
    match name {
        "Visual Studio Code" => "VS Code".into(),
        "Visual Studio Code - Insiders" => "VS Code β".into(),
        n => one_line(n, 9),
    }
}

/// The details pane's call to action: what Enter will do for the selected row.
fn goto_line(app: &App) -> Line<'static> {
    let p = pal();
    let key = Span::styled(" ↵ ", Style::new().fg(p.on_accent).bg(p.blue).bold());
    match app.selected_target() {
        Ok(crate::Target::Tty(tty)) => {
            let appname = app.snap.terminals.iter().find(|t| t.tty == tty).map(|t| t.app.clone()).unwrap_or_default();
            Line::from(vec![
                key,
                Span::styled(format!(" go to {}", tty.trim_start_matches("tty")), Style::new().fg(p.fg).bold()),
                Span::styled(format!(" in {appname}"), Style::new().fg(p.muted)),
            ])
        }
        Ok(crate::Target::App(a)) => Line::from(vec![
            key,
            Span::styled(format!(" bring {a} to the front"), Style::new().fg(p.fg).bold()),
        ]),
        Ok(crate::Target::Resume(plan)) => Line::from(vec![
            key,
            Span::styled(format!(" resume in {}", plan.place.describe()), Style::new().fg(p.fg).bold()),
            Span::styled(format!("  ({})", plan.because), Style::new().fg(p.muted)),
        ]),

        Err(e) => Line::from(vec![
            Span::styled(" ↵ ", Style::new().fg(p.muted).bg(p.select)),
            Span::styled(format!(" {e}"), Style::new().fg(p.muted)),
        ]),
    }
}

fn draw_detail(f: &mut Frame, app: &mut App, area: Rect) {
    // paths are shortened to one line (inner width minus borders and padding)
    let path_w = area.width.saturating_sub(6).max(10) as usize;
    let goto = match app.selected() {
        Some(RowRef::Divider) | None => None,
        _ => Some(goto_line(app)),
    };
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
            lines.push(Line::from(Span::styled(s.title(), Style::new().fg(pal().fg).bold())));
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
            lines.push(Line::from(Span::styled(short_path(&s.path.display().to_string(), path_w), Style::new().fg(pal().faint))));
        }
        Some(RowRef::Agent(i, j)) => {
            let s = &app.snap.sessions[i];
            let a = &s.agents[j];
            let st = agent_status(a);
            title = title_line(format!("Agent {}", a.id), None);
            lines.push(Line::from(vec![
                Span::styled(a.id.clone(), Style::new().fg(pal().fg).bold()),
                Span::styled(format!("  {}", a.agent), Style::new().fg(pal().violet)),
                Span::styled(format!("  in {}", one_line(&s.title(), 40)), Style::new().fg(pal().muted)),
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
                lines.push(Line::from(Span::styled(short_path(&p.display().to_string(), path_w), Style::new().fg(pal().faint))));
            }
        }
        Some(RowRef::Orphan(k)) => {
            let p = &app.snap.orphan_procs[k];
            title = title_line("Process", None);
            lines.push(Line::from(vec![
                Span::styled(format!("{} ", p.tool.label()), Style::new().fg(tool_color(p.tool)).bold()),
                Span::styled("session", Style::new().fg(pal().fg).bold()),
            ]));
            lines.push(meta(vec![format!("pid {}", p.pid), home_rel(&p.cwd), format!("up {}", ago(Some(p.start_ms)))]));
            lines.push(Line::default());
            lines.push(stats(&[("cpu", format!("{:.0}%", p.cpu)), ("memory", mem_str(p.mem))]));
            lines.push(Line::default());
            let note = if matches!(p.tool, Tool::Other(_)) {
                format!("whosup can't read {}'s transcripts yet, so only the process is shown.", p.tool.label())
            } else {
                format!("{} hasn't written a transcript yet; it will appear here after the first message.", p.tool.label())
            };
            lines.push(Line::from(Span::styled(note, Style::new().fg(pal().muted))));
        }
        Some(RowRef::Terminal(i)) => {
            title = title_line("Terminal", None);
            terminal_detail(&app.snap.terminals[i], &mut lines);
        }
        Some(RowRef::Job(i, j)) => {
            let t = &app.snap.terminals[i];
            let p = &t.jobs[j];
            title = title_line("Background job", None);
            lines.push(Line::from(Span::styled(p.cmdline.clone(), Style::new().fg(pal().fg).bold())));
            lines.push(meta(vec![
                format!("pid {}", p.pid),
                format!("in {}", t.short_tty()),
                format!("running {}", ago(Some(p.start_ms))),
            ]));
            lines.push(Line::default());
            lines.push(stats(&[("cpu", format!("{:.0}%", p.cpu)), ("memory", mem_str(p.mem))]));
            lines.push(Line::default());
            lines.push(Line::from(Span::styled(home_rel(&p.cwd), Style::new().fg(pal().faint))));
        }
        Some(RowRef::Divider) | None => {
            title = title_line("Details", None);
        }
    }

    // the go-to action sits right under the headline
    if let Some(g) = goto {
        let at = lines.len().min(1);
        lines.insert(at, g);
        let mut next = at + 1;
        // for a resume, show exactly what will run
        if let Ok(crate::Target::Resume(plan)) = app.selected_target() {
            lines.insert(
                next,
                Line::from(vec![
                    Span::styled("    $ ", Style::new().fg(pal().faint)),
                    Span::styled(
                        match std::env::var("HOME") {
                            Ok(h) if !h.is_empty() => plan.command.replace(&h, "~"),
                            _ => plan.command.clone(),
                        },
                        Style::new().fg(pal().cyan),
                    ),
                ]),
            );
            next += 1;
        }
        lines.insert(next, Line::default());
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
            Line::from(Span::styled(format!(" {pct}% "), Style::new().fg(pal().muted))).alignment(Alignment::Right),
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
        TermState { glyph: "❯", color: pal().faint }
    } else if t.foreground.iter().map(|p| p.cpu).sum::<f32>() >= 5.0 {
        TermState { glyph: "▶", color: pal().amber } // busy
    } else {
        TermState { glyph: "▶", color: pal().blue } // running, mostly idle (editor, pager, server)
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
                        Span::styled(one_line(&l.title, 60), Style::new().fg(pal().fg)),
                    ]),
                    (None, Some(p)) => Line::from(Span::styled(one_line(&p.cmdline, 80), Style::new().fg(pal().fg).bold())),
                    (None, None) => Line::from(vec![
                        Span::styled(shell_name(t), Style::new().fg(pal().muted)),
                        Span::styled("  at prompt", Style::new().fg(pal().faint)),
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
                        Span::styled(t.short_tty(), Style::new().fg(pal().soft).bold()),
                        Span::styled(format!(" {}", t.app), Style::new().fg(pal().muted)),
                    ])),
                    Cell::from(Line::from({
                        let mut v = what.spans;
                        v.push(Span::styled(jobs, Style::new().fg(pal().blue)));
                        v
                    })),
                    Cell::from(Span::styled(short_path(&t.cwd(), where_w), Style::new().fg(pal().muted))),
                    right(time, Style::new().fg(pal().muted)),
                    right(format!("{cpu:.0}%"), Style::new().fg(if cpu >= 5.0 { pal().amber } else { pal().muted })),
                    right(mem_str(t.mem), Style::new().fg(pal().muted)),
                ]));
            }
            RowRef::Job(i, j) => {
                let p = &app.snap.terminals[*i].jobs[*j];
                let last = !matches!(app.rows.get(idx + 1), Some((_, RowRef::Job(ti, _))) if ti == i);
                rows.push(fit(vec![
                    Cell::from(""),
                    Cell::from(Span::styled(if last { "  └─ &" } else { "  ├─ &" }, Style::new().fg(pal().faint))),
                    Cell::from(Span::styled(one_line(&p.cmdline, 80), Style::new().fg(pal().soft))),
                    Cell::from(Span::styled(short_path(&p.cwd, where_w), Style::new().fg(pal().faint))),
                    right(ago(Some(p.start_ms)), Style::new().fg(pal().muted)),
                    right(format!("{:.0}%", p.cpu), Style::new().fg(pal().muted)),
                    right(mem_str(p.mem), Style::new().fg(pal().muted)),
                ]));
            }
            _ => {}
        }
    }

    let head = |s: &'static str| Cell::from(Span::styled(s, Style::new().fg(pal().faint).add_modifier(Modifier::BOLD)));
    let head_r = |s: &'static str| {
        Cell::from(Line::from(Span::styled(s, Style::new().fg(pal().faint).add_modifier(Modifier::BOLD))).alignment(Alignment::Right))
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
    let count = Span::styled(format!("{n} open · {busy} running"), Style::new().fg(pal().muted));
    let table = Table::new(rows, widths)
        .column_spacing(2)
        .header(Row::new(header).bottom_margin(1))
        .row_highlight_style(Style::new().bg(pal().select))
        .highlight_symbol(Span::styled("▌", Style::new().fg(pal().blue)))
        .highlight_spacing(ratatui::widgets::HighlightSpacing::Always)
        .block(panel(title_line("Terminals", Some(count)), true).padding(Padding::new(0, 1, 1, 0)));
    f.render_stateful_widget(table, area, &mut app.state);

    if n == 0 {
        let inner = area.inner(Margin { horizontal: 2, vertical: 3 });
        f.render_widget(
            Paragraph::new(Line::from(Span::styled("No open terminals found", Style::new().fg(pal().soft).bold())))
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
    lines.push(Line::from(Span::styled(headline, Style::new().fg(pal().fg).bold())));
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
        lines.push(Line::from(Span::styled(l.title.clone(), Style::new().fg(pal().soft))));
        lines.push(Line::from(vec![
            Span::styled("s", Style::new().fg(pal().fg).bold()),
            Span::styled(" show it in Sessions   ", Style::new().fg(pal().muted)),
            Span::styled("↵", Style::new().fg(pal().fg).bold()),
            Span::styled(" go to this tab", Style::new().fg(pal().muted)),
        ]));
    }
    if !t.foreground.is_empty() {
        lines.push(Line::default());
        lines.push(section("Foreground"));
        for p in &t.foreground {
            lines.push(Line::from(vec![
                Span::styled(format!("{:>6}  ", p.pid), Style::new().fg(pal().faint)),
                Span::styled(p.cmdline.clone(), Style::new().fg(pal().soft)),
            ]));
        }
    }
    if !t.jobs.is_empty() {
        lines.push(Line::default());
        lines.push(section("Background jobs"));
        for p in &t.jobs {
            lines.push(Line::from(vec![
                Span::styled(format!("{:>6}  ", p.pid), Style::new().fg(pal().faint)),
                Span::styled(p.cmdline.clone(), Style::new().fg(pal().soft)),
            ]));
        }
    }
}
