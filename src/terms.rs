//! Open terminals: every tty with live processes, what its shell is doing, and
//! which terminal app owns it.
//!
//! `ps` supplies what sysinfo can't on macOS (tty, process group, the tty's
//! foreground group `tpgid`); sysinfo supplies cwd, argv, start time, cpu and
//! memory for each pid.

use crate::providers::Tool;
use std::{collections::HashMap, ffi::OsString, path::Path, process::Command};
use sysinfo::{Pid, System};

const SHELLS: &[&str] = &["zsh", "bash", "fish", "sh", "dash", "ksh", "tcsh", "csh", "nu", "xonsh", "elvish", "pwsh"];

#[derive(Clone, Debug)]
pub struct Proc {
    pub pid: u32,
    pub name: String,
    pub cmdline: String,
    pub cwd: String,
    pub start_ms: i64,
    pub cpu: f32,
    pub mem: u64,
}

#[derive(Clone, Debug)]
pub struct Terminal {
    pub key: String,
    pub tty: String,
    pub app: String,
    pub shell: Option<Proc>,
    /// The tty's foreground process group, leader first. Empty when the shell is at its prompt.
    pub foreground: Vec<Proc>,
    /// Background jobs started from the shell (`cmd &`, suspended jobs), one per group.
    pub jobs: Vec<Proc>,
    pub procs: usize,
    pub started: Option<i64>,
    pub cpu: f32,
    pub mem: u64,
    /// The coding-agent session running in the foreground, if any.
    pub session: Option<SessionLink>,
}

#[derive(Clone, Debug)]
pub struct SessionLink {
    pub tool: Tool,
    /// row key in the Sessions tab (None: agent without transcript support)
    pub key: Option<String>,
    pub title: String,
}

impl Terminal {
    pub fn at_prompt(&self) -> bool {
        self.foreground.is_empty()
    }
    pub fn leader(&self) -> Option<&Proc> {
        self.foreground.first()
    }
    pub fn cwd(&self) -> String {
        self.leader()
            .map(|p| p.cwd.clone())
            .filter(|c| !c.is_empty())
            .or_else(|| self.shell.as_ref().map(|s| s.cwd.clone()))
            .unwrap_or_default()
    }
    /// `s001` from `ttys001`
    pub fn short_tty(&self) -> String {
        self.tty.trim_start_matches("tty").to_string()
    }
}

struct PsRow {
    pid: u32,
    ppid: u32,
    pgid: u32,
    tpgid: u32,
    tty: String,
    comm: String,
}

fn ps_rows() -> Vec<PsRow> {
    let Ok(out) = Command::new("ps").args(["-ax", "-o", "pid=,ppid=,pgid=,tpgid=,tty=,comm="]).output() else {
        return vec![];
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let mut rows = Vec::new();
    for line in text.lines() {
        // five whitespace-separated fields, then the command (which may contain spaces)
        let mut rest = line.trim_start();
        let mut f = Vec::with_capacity(5);
        for _ in 0..5 {
            let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
            f.push(&rest[..end]);
            rest = rest[end..].trim_start();
        }
        let num = |s: &str| s.parse::<u32>().ok();
        if let (Some(pid), Some(ppid), Some(pgid), Some(tpgid)) = (num(f[0]), num(f[1]), num(f[2]), num(f[3])) {
            rows.push(PsRow { pid, ppid, pgid, tpgid, tty: f[4].to_string(), comm: rest.to_string() });
        }
    }
    rows
}

fn base(s: &str) -> String {
    Path::new(s.trim_start_matches('-')).file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_else(|| s.to_string())
}

fn pretty_cmd(cmd: &[OsString], fallback: &str) -> String {
    if cmd.is_empty() {
        return base(fallback);
    }
    let mut parts = vec![base(&cmd[0].to_string_lossy())];
    parts.extend(cmd[1..].iter().map(|a| a.to_string_lossy().to_string()));
    parts.join(" ")
}

/// Name the app that owns a tty by walking up from its session leader.
fn app_for(mut pid: u32, by_pid: &HashMap<u32, &PsRow>) -> String {
    for _ in 0..16 {
        let Some(r) = by_pid.get(&pid) else { break };
        if let Some(i) = r.comm.find(".app/") {
            return r.comm[..i].rsplit('/').next().unwrap_or("app").to_string();
        }
        match base(&r.comm).as_str() {
            n @ ("tmux" | "screen" | "zellij") => return n.to_string(),
            "sshd" | "sshd-session" => return "ssh".into(),
            _ => {}
        }
        if r.ppid <= 1 {
            break;
        }
        pid = r.ppid;
    }
    "terminal".into()
}

fn proc_info(sys: &System, r: &PsRow) -> Proc {
    match sys.process(Pid::from_u32(r.pid)) {
        Some(p) => Proc {
            pid: r.pid,
            name: p.cmd().first().map(|a| base(&a.to_string_lossy())).unwrap_or_else(|| base(&r.comm)),
            cmdline: pretty_cmd(p.cmd(), &r.comm),
            cwd: p.cwd().map(|c| c.to_string_lossy().to_string()).unwrap_or_default(),
            start_ms: p.start_time() as i64 * 1000,
            cpu: p.cpu_usage(),
            mem: p.memory(),
        },
        None => Proc { pid: r.pid, name: base(&r.comm), cmdline: base(&r.comm), cwd: String::new(), start_ms: 0, cpu: 0.0, mem: 0 },
    }
}

pub fn scan(sys: &System) -> Vec<Terminal> {
    let rows = ps_rows();
    let by_pid: HashMap<u32, &PsRow> = rows.iter().map(|r| (r.pid, r)).collect();
    let mut by_tty: HashMap<&str, Vec<&PsRow>> = HashMap::new();
    for r in &rows {
        if r.tty != "??" && r.tty != "-" && !r.tty.is_empty() {
            by_tty.entry(r.tty.as_str()).or_default().push(r);
        }
    }

    let mut out = Vec::new();
    for (tty, mut procs) in by_tty {
        procs.sort_by_key(|r| r.pid);
        let in_tty = |pid: u32| procs.iter().any(|r| r.pid == pid);
        // session leader: the process whose parent lives outside this tty (usually `login`)
        let Some(leader) = procs.iter().find(|r| !in_tty(r.ppid)).copied() else { continue };
        let is_shell = |r: &PsRow| SHELLS.contains(&base(&r.comm).as_str());
        // interactive shell: the leader itself or a shell spawned directly by it
        let shell = if is_shell(leader) {
            Some(leader)
        } else {
            procs.iter().find(|r| r.ppid == leader.pid && is_shell(r)).copied()
        };
        let tpgid = procs.iter().map(|r| r.tpgid).find(|&t| t != 0).unwrap_or(0);
        let shell_pgid = shell.map(|s| s.pgid);

        // at the prompt only when the shell itself owns the foreground
        let mut foreground: Vec<&PsRow> = if tpgid == 0 || Some(tpgid) == shell_pgid {
            vec![]
        } else {
            procs.iter().filter(|r| r.pgid == tpgid).copied().collect()
        };
        foreground.sort_by_key(|r| (r.pid != tpgid, r.pid));

        let jobs: Vec<&PsRow> = match shell {
            Some(s) => procs
                .iter()
                .filter(|r| r.ppid == s.pid && r.pgid != tpgid && r.pgid != s.pgid && r.pid == r.pgid)
                .copied()
                .collect(),
            None => vec![],
        };

        let infos: Vec<Proc> = procs.iter().map(|r| proc_info(sys, r)).collect();
        let find = |pid: u32| infos.iter().find(|p| p.pid == pid).cloned();
        let leader_info = find(leader.pid);
        out.push(Terminal {
            key: format!("t:{tty}"),
            tty: tty.to_string(),
            app: app_for(leader.ppid, &by_pid),
            shell: shell.and_then(|s| find(s.pid)),
            foreground: foreground.iter().filter_map(|r| find(r.pid)).collect(),
            jobs: jobs.iter().filter_map(|r| find(r.pid)).collect(),
            procs: procs.len(),
            // `login` runs as root, so its start time may be unreadable; the shell starts with it
            started: leader_info
                .map(|p| p.start_ms)
                .filter(|&s| s > 0)
                .or_else(|| shell.and_then(|s| find(s.pid)).map(|p| p.start_ms).filter(|&s| s > 0)),
            cpu: infos.iter().map(|p| p.cpu).sum(),
            mem: infos.iter().map(|p| p.mem).sum(),
            session: None,
        });
    }
    out.sort_by(|a, b| a.tty.cmp(&b.tty));
    out
}
