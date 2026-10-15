//! Open ports: every TCP port a process of yours is listening on, which process holds it,
//! the terminal it runs in and the agent session that started it.
//!
//! `lsof` supplies the listening sockets (only your own processes unless run as root);
//! `ps` and sysinfo supply the rest, as for terminals.

use crate::terms::{Proc, SessionLink};
use std::{collections::BTreeMap, process::Command};
use sysinfo::System;

#[derive(Clone, Debug)]
pub struct Port {
    pub key: String,
    pub port: u16,
    /// the addresses it listens on: `*`, `127.0.0.1`, `[::1]`, …
    pub addrs: Vec<String>,
    pub proc: Proc,
    /// the terminal the process runs in (`ttys003`)
    pub tty: Option<String>,
    /// the agent session it was started from (the process or one of its parents is the agent)
    pub session: Option<SessionLink>,
    /// it runs in one of your folders (under your home, outside ~/Library)
    pub in_your_folder: bool,
}

impl Port {
    /// Reachable from other machines: it listens on every interface, or on a non-loopback one.
    pub fn exposed(&self) -> bool {
        self.addrs.iter().any(|a| !is_loopback(a))
    }
    pub fn url(&self) -> String {
        format!("http://localhost:{}", self.port)
    }
    /// Started from a terminal or by an agent, or running in one of your project folders (a dev
    /// server left behind after its terminal or agent closed), rather than kept open by an app.
    pub fn yours(&self) -> bool {
        self.tty.is_some() || self.session.is_some() || self.detached()
    }
    /// A server in one of your folders with no terminal or agent behind it any more.
    pub fn detached(&self) -> bool {
        if self.tty.is_some() || self.session.is_some() {
            return false;
        }
        self.in_your_folder
    }
}

fn in_your_folder(cwd: &str) -> bool {
    let home = std::env::var("HOME").unwrap_or_default();
    !home.is_empty() && cwd.starts_with(&format!("{home}/")) && !cwd.starts_with(&format!("{home}/Library"))
}

pub fn is_loopback(addr: &str) -> bool {
    let a = addr.trim_start_matches('[').trim_end_matches(']');
    a == "localhost" || a == "::1" || a.starts_with("127.")
}

/// One listening socket from `lsof -F pcn`.
#[derive(Debug, PartialEq)]
pub struct Listen {
    pub pid: u32,
    pub command: String,
    pub addr: String,
    pub port: u16,
}

/// Parse `lsof -nP -iTCP -sTCP:LISTEN -F pcn`: `p<pid>`, `c<command>`, then `f<fd>`/`n<addr:port>` per socket.
pub fn parse_lsof(text: &str) -> Vec<Listen> {
    let mut out = Vec::new();
    let (mut pid, mut command) = (0u32, String::new());
    for line in text.lines() {
        let (tag, rest) = line.split_at(line.len().min(1));
        match tag {
            "p" => pid = rest.parse().unwrap_or(0),
            "c" => command = rest.to_string(),
            "n" => {
                // `*:8080`, `127.0.0.1:7768`, `[::1]:42050`; the port follows the last colon
                let Some((addr, port)) = rest.rsplit_once(':') else { continue };
                let Ok(port) = port.parse::<u16>() else { continue };
                if pid > 0 {
                    out.push(Listen { pid, command: command.clone(), addr: addr.to_string(), port });
                }
            }
            _ => {}
        }
    }
    out
}

/// Listening ports, one per process and port (IPv4 and IPv6 sockets merged), lowest port first.
/// Agent sessions are linked afterwards by the scanner, which knows them.
pub fn scan(sys: &System) -> Vec<Port> {
    let Ok(out) = Command::new("lsof").args(["-nP", "-iTCP", "-sTCP:LISTEN", "-F", "pcn"]).output() else {
        return vec![];
    };
    let listens = parse_lsof(&String::from_utf8_lossy(&out.stdout));
    let rows = crate::terms::ps_rows();
    let mut merged: BTreeMap<(u16, u32), (Vec<String>, String)> = BTreeMap::new();
    for l in listens {
        let e = merged.entry((l.port, l.pid)).or_insert_with(|| (vec![], l.command.clone()));
        if !e.0.contains(&l.addr) {
            e.0.push(l.addr);
        }
    }
    merged
        .into_iter()
        .map(|((port, pid), (addrs, command))| {
            let row = rows.iter().find(|r| r.pid == pid);
            let proc = match row {
                Some(r) => {
                    let mut p = crate::terms::proc_info(sys, r);
                    p.cmdline = tidy_cmdline(&p.cmdline);
                    p
                }
                None => Proc { pid, name: command.clone(), cmdline: command, cwd: String::new(), start_ms: 0, cpu: 0.0, mem: 0 },
            };
            let tty = row.map(|r| r.tty.clone()).filter(|t| !t.is_empty() && t != "??" && t != "-");
            let in_your_folder = in_your_folder(&proc.cwd);
            Port { key: format!("port:{port}:{pid}"), port, addrs, proc, tty, session: None, in_your_folder }
        })
        .collect()
}

/// `node …/node_modules/.bin/../vite/bin/vite.js --port 5199` → `node vite --port 5199`.
pub fn tidy_cmdline(cmd: &str) -> String {
    let mut parts: Vec<String> = cmd.split(' ').map(String::from).collect();
    for p in parts.iter_mut().skip(1) {
        if let Some(at) = p.rfind("node_modules/") {
            let pkg = p[at + "node_modules/".len()..]
                .split('/')
                .find(|c| !c.is_empty() && *c != ".bin" && *c != "..")
                .map(String::from);
            if let Some(pkg) = pkg {
                *p = pkg;
            }
            break;
        }
    }
    parts.join(" ")
}

/// The process and its parents, nearest first (at most 32), from sysinfo.
pub fn lineage(sys: &System, pid: u32) -> Vec<u32> {
    let mut out = vec![pid];
    let mut cur = sys.process(sysinfo::Pid::from_u32(pid)).and_then(|p| p.parent());
    while let Some(p) = cur {
        if out.len() >= 32 || p.as_u32() <= 1 {
            break;
        }
        out.push(p.as_u32());
        cur = sys.process(p).and_then(|x| x.parent());
    }
    out
}

/// Close a port by ending the process that holds it: SIGTERM and wait, or SIGKILL when forced.
pub fn close(pid: u32, port: u16, force: bool) -> Result<String, String> {
    if !force {
        return crate::focus::terminate(pid).map(|_| format!("Closed port {port} (ended pid {pid})"));
    }
    let ok = Command::new("kill").args(["-KILL", &pid.to_string()]).status().map_err(|e| e.to_string())?;
    if ok.success() {
        Ok(format!("Closed port {port} (killed pid {pid})"))
    } else {
        Err(format!("Couldn't kill pid {pid}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_lsof_fields() {
        let text = "p191\ncPython\nf4\nn*:8080\np952\ncSpotify\nf127\nn127.0.0.1:7768\nf148\nn*:57621\np1238\ncOneDrive\nf40\nn[::1]:42050\n";
        let l = parse_lsof(text);
        assert_eq!(l.len(), 4);
        assert_eq!(l[0], Listen { pid: 191, command: "Python".into(), addr: "*".into(), port: 8080 });
        assert_eq!(l[1].addr, "127.0.0.1");
        assert_eq!(l[2].port, 57621);
        assert_eq!(l[3], Listen { pid: 1238, command: "OneDrive".into(), addr: "[::1]".into(), port: 42050 });
    }

    #[test]
    fn tidies_node_paths() {
        assert_eq!(tidy_cmdline("node ./node_modules/.bin/../vite/bin/vite.js --port 5199"), "node vite --port 5199");
        assert_eq!(tidy_cmdline("node /x/node_modules/next/dist/bin/next dev"), "node next dev");
        assert_eq!(tidy_cmdline("python3 -m http.server"), "python3 -m http.server");
    }

    /// Opens a real port, finds it, closes it: `cargo test --release closes_a_real_port -- --ignored`.
    #[test]
    #[ignore]
    fn closes_a_real_port() {
        let mut child = Command::new("python3").args(["-m", "http.server", "18765", "--bind", "127.0.0.1"]).spawn().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1500));
        let sys = System::new_all();
        let p = scan(&sys).into_iter().find(|p| p.port == 18765).expect("port listed");
        assert_eq!(p.proc.pid, child.id());
        assert!(!p.exposed());
        let msg = close(p.proc.pid, p.port, false).unwrap();
        println!("{msg}");
        let _ = child.wait();
        assert!(scan(&System::new_all()).iter().all(|p| p.port != 18765));
    }

    #[test]
    fn loopback_only_is_not_exposed() {
        assert!(is_loopback("127.0.0.1") && is_loopback("[::1]") && is_loopback("localhost"));
        assert!(!is_loopback("*") && !is_loopback("0.0.0.0") && !is_loopback("192.168.1.4"));
    }
}
