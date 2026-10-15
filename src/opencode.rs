//! opencode keeps its sessions in SQLite (`~/.local/share/opencode/opencode.db`, or
//! `$XDG_DATA_HOME/opencode/`). whosup reads it with the system `sqlite3` in read-only mode,
//! only for sessions updated inside the history window, and only when the database changed.
//!
//! A session is a `session` row (folder, title, agent, model, cost, tokens); its messages and
//! their parts (text, tool calls with a running/completed state, step finishes) say what it's
//! doing. Child sessions (`parent_id`) are opencode's sub-agents.

use crate::convo::{key_arg, Entry, Item};
use crate::data::{now_ms, Transcript};
use serde_json::Value;
use std::{path::PathBuf, process::Command};

pub fn db_path() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".local/share"))
        .join("opencode/opencode.db")
}

/// Sessions stand in for transcript files with an `opencode:<id>` path.
pub fn path_for(id: &str) -> PathBuf {
    PathBuf::from(format!("opencode:{id}"))
}

pub fn id_of(path: &std::path::Path) -> Option<String> {
    path.to_str()?.strip_prefix("opencode:").map(String::from)
}

fn query(db: &std::path::Path, sql: &str) -> Vec<Value> {
    let out = match Command::new("sqlite3").arg("-readonly").arg("-json").arg(db).arg(sql).output() {
        Ok(o) if o.status.success() => o,
        _ => return vec![],
    };
    serde_json::from_slice::<Vec<Value>>(&out.stdout).unwrap_or_default()
}

fn sql_list(ids: &[String]) -> String {
    ids.iter().map(|i| format!("'{}'", i.replace('\'', "''"))).collect::<Vec<_>>().join(",")
}

#[derive(Clone, Debug)]
pub struct OcSession {
    pub id: String,
    pub parent: Option<String>,
    pub agent: String,
    pub t: Transcript,
}

fn n(v: &Value, k: &str) -> u64 {
    v.get(k).and_then(Value::as_u64).unwrap_or(0)
}

/// Sessions updated since `since_ms`, each summarised as a Transcript.
pub fn load(db: &std::path::Path, since_ms: i64) -> Vec<OcSession> {
    let rows = query(
        db,
        &format!(
            "select id, parent_id, directory, title, cost, tokens_input, tokens_output, tokens_reasoning, \
             tokens_cache_read, tokens_cache_write, agent, model, time_created, time_updated \
             from session where time_archived is null and time_updated >= {since_ms} order by time_updated desc"
        ),
    );
    if rows.is_empty() {
        return vec![];
    }
    let ids: Vec<String> = rows.iter().filter_map(|r| r.get("id").and_then(Value::as_str).map(String::from)).collect();
    let list = sql_list(&ids);
    // the newest part and message of each session say what it's doing now
    let last_part = query(
        db,
        &format!(
            "select session_id, data from (select session_id, data, row_number() over \
             (partition by session_id order by time_created desc, id desc) rn from part where session_id in ({list})) where rn = 1"
        ),
    );
    let last_msg = query(
        db,
        &format!(
            "select session_id, data from (select session_id, data, row_number() over \
             (partition by session_id order by time_created desc, id desc) rn from message where session_id in ({list})) where rn = 1"
        ),
    );
    // first and last text of each role, and how many assistant messages
    let texts = query(
        db,
        &format!(
            "select session_id, role, text, first from (select p.session_id, json_extract(m.data, '$.role') role, \
             json_extract(p.data, '$.text') text, \
             row_number() over (partition by p.session_id, json_extract(m.data, '$.role') order by p.time_created desc) rn_last, \
             row_number() over (partition by p.session_id, json_extract(m.data, '$.role') order by p.time_created asc) rn_first, \
             case when row_number() over (partition by p.session_id, json_extract(m.data, '$.role') order by p.time_created asc) = 1 then 1 else 0 end first \
             from part p join message m on m.id = p.message_id \
             where p.session_id in ({list}) and json_extract(p.data, '$.type') = 'text' and coalesce(json_extract(p.data, '$.synthetic'), 0) = 0) \
             where rn_last = 1 or rn_first = 1"
        ),
    );
    let counts = query(
        db,
        &format!(
            "select session_id, sum(json_extract(data, '$.role') = 'assistant') turns, sum(json_extract(data, '$.role') = 'user') users \
             from message where session_id in ({list}) group by session_id"
        ),
    );
    let tool_counts = query(
        db,
        &format!("select session_id, count(*) calls from part where session_id in ({list}) and json_extract(data, '$.type') = 'tool' group by session_id"),
    );
    let get = |v: &[Value], id: &str| v.iter().filter(|r| r.get("session_id").and_then(Value::as_str) == Some(id)).cloned().collect::<Vec<_>>();

    let mut out = Vec::new();
    for r in &rows {
        let id = r.get("id").and_then(Value::as_str).unwrap_or("").to_string();
        let model: Value = r.get("model").and_then(Value::as_str).and_then(|m| serde_json::from_str(m).ok()).unwrap_or(Value::Null);
        let (i, o, re, cr, cw) = (n(r, "tokens_input"), n(r, "tokens_output"), n(r, "tokens_reasoning"), n(r, "tokens_cache_read"), n(r, "tokens_cache_write"));
        let mut t = Transcript {
            id: id.clone(),
            cwd: r.get("directory").and_then(Value::as_str).unwrap_or("").to_string(),
            started: r.get("time_created").and_then(Value::as_i64),
            name: r.get("title").and_then(Value::as_str).map(String::from),
            model: model.get("id").and_then(Value::as_str).map(String::from),
            thinking: model.get("variant").and_then(Value::as_str).map(String::from),
            cost: r.get("cost").and_then(Value::as_f64).unwrap_or(0.0),
            in_tokens: i,
            out_tokens: o + re,
            cache_read_tokens: cr,
            cache_write_tokens: cw,
            tokens: i + o + re + cr + cw,
            last_ts: r.get("time_updated").and_then(Value::as_i64),
            ..Transcript::default()
        };
        if let Some(c) = get(&counts, &id).first() {
            t.turns = n(c, "turns");
            t.user_msgs = n(c, "users");
            t.messages = t.turns + t.user_msgs;
        }
        t.tool_calls = get(&tool_counts, &id).first().map(|c| n(c, "calls")).unwrap_or(0);
        for row in get(&texts, &id) {
            let text = row.get("text").and_then(Value::as_str).unwrap_or("").to_string();
            match (row.get("role").and_then(Value::as_str), row.get("first").and_then(Value::as_i64) == Some(1)) {
                (Some("user"), true) if t.first_user.is_none() => t.first_user = Some(text.clone()),
                _ => {}
            }
            match row.get("role").and_then(Value::as_str) {
                Some("user") => t.last_user = Some(text),
                Some("assistant") => t.last_text = Some(text),
                _ => {}
            }
        }
        // what it's doing: the newest message's role, and its newest part
        let msg: Value = get(&last_msg, &id).first().and_then(|m| m.get("data")?.as_str().and_then(|d| serde_json::from_str(d).ok())).unwrap_or(Value::Null);
        let part: Value = get(&last_part, &id).first().and_then(|m| m.get("data")?.as_str().and_then(|d| serde_json::from_str(d).ok())).unwrap_or(Value::Null);
        set_state(&mut t, &msg, &part);
        if let Some(w) = msg.pointer("/tokens/total").and_then(Value::as_u64) {
            t.context_tokens = w;
        }
        out.push(OcSession {
            id,
            parent: r.get("parent_id").and_then(Value::as_str).map(String::from),
            agent: r.get("agent").and_then(Value::as_str).unwrap_or("").to_string(),
            t,
        });
    }
    out
}

/// Map opencode's newest message and part onto whosup's states.
pub fn set_state(t: &mut Transcript, msg: &Value, part: &Value) {
    let role = msg.get("role").and_then(Value::as_str);
    let completed = msg.pointer("/time/completed").is_some();
    if role == Some("assistant") && msg.get("error").is_some_and(|e| !e.is_null()) {
        t.last_role = Some("assistant".into());
        t.last_stop = Some(if msg.pointer("/error/name").and_then(Value::as_str) == Some("MessageAbortedError") { "aborted" } else { "error" }.into());
        return;
    }
    match (role, part.get("type").and_then(Value::as_str)) {
        (Some("user"), _) => {
            t.last_role = Some("user".into());
            t.last_stop = None;
        }
        (Some("assistant"), Some("tool")) if matches!(part.pointer("/state/status").and_then(Value::as_str), Some("running") | Some("pending")) => {
            t.last_role = Some("assistant".into());
            t.last_stop = Some("toolUse".into());
            t.last_tool = part.get("tool").and_then(Value::as_str).map(String::from);
        }
        (Some("assistant"), Some("step-finish")) => match part.get("reason").and_then(Value::as_str) {
            Some("tool-calls") => t.last_role = Some("toolResult".into()),
            Some(r) => {
                t.last_role = Some("assistant".into());
                t.last_stop = Some(if r == "stop" { "stop".into() } else { r.to_string() });
            }
            None => {
                t.last_role = Some("assistant".into());
                t.last_stop = Some("stop".into());
            }
        },
        (Some("assistant"), _) if completed => {
            t.last_role = Some("assistant".into());
            t.last_stop = Some("stop".into());
        }
        // mid-reply: streaming text or reasoning, or a tool just finished
        (Some("assistant"), _) => {
            t.last_role = Some("toolResult".into());
        }
        _ => {}
    }
}

/// The whole conversation of one session, for `v` and `e`.
pub fn conversation(db: &std::path::Path, id: &str) -> Vec<Item> {
    let rows = query(
        db,
        &format!(
            "select json_extract(m.data, '$.role') role, p.data data, p.time_created at from part p join message m on m.id = p.message_id \
             where p.session_id = '{}' order by p.time_created, p.id",
            id.replace('\'', "''")
        ),
    );
    let mut out = Vec::new();
    for r in rows {
        let at = r.get("at").and_then(Value::as_i64);
        let role = r.get("role").and_then(Value::as_str).unwrap_or("");
        let Ok(p) = serde_json::from_str::<Value>(r.get("data").and_then(Value::as_str).unwrap_or("")) else { continue };
        let entry = match (role, p.get("type").and_then(Value::as_str)) {
            ("user", Some("text")) if p.get("synthetic").and_then(Value::as_bool) != Some(true) => {
                Entry::User(p.get("text").and_then(Value::as_str).unwrap_or("").to_string())
            }
            ("assistant", Some("text")) => Entry::Reply(p.get("text").and_then(Value::as_str).unwrap_or("").to_string()),
            (_, Some("tool")) => {
                let name = p.get("tool").and_then(Value::as_str).unwrap_or("tool").to_string();
                out.push(Item { at, entry: Entry::Call(name, key_arg(p.pointer("/state/input").unwrap_or(&Value::Null))) });
                let status = p.pointer("/state/status").and_then(Value::as_str);
                let text = p.pointer("/state/output").or(p.pointer("/state/error")).and_then(Value::as_str).unwrap_or("");
                if status == Some("completed") || status == Some("error") {
                    let first: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).take(3).collect();
                    Entry::Result(first.join("\n"), status == Some("error"))
                } else {
                    continue;
                }
            }
            _ => continue,
        };
        if matches!(&entry, Entry::User(t) | Entry::Reply(t) if t.trim().is_empty()) {
            continue;
        }
        out.push(Item { at, entry });
    }
    out
}

/// A cheap change marker: the database's and its write-ahead log's modification times.
pub fn stamp(db: &std::path::Path) -> (i64, i64) {
    let m = |p: PathBuf| {
        std::fs::metadata(p)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    };
    (m(db.to_path_buf()), m(PathBuf::from(format!("{}-wal", db.display()))))
}

/// opencode reports times in ms; anything this old isn't loaded.
pub fn since(window_ms: i64) -> i64 {
    now_ms().saturating_sub(window_ms)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn state(msg: Value, part: Value) -> (Option<String>, Option<String>, Option<String>) {
        let mut t = Transcript::default();
        set_state(&mut t, &msg, &part);
        (t.last_role, t.last_stop, t.last_tool)
    }

    #[test]
    fn states() {
        let s = |x: &str| Some(x.to_string());
        assert_eq!(state(json!({"role": "user"}), json!({"type": "text"})), (s("user"), None, None));
        assert_eq!(
            state(json!({"role": "assistant"}), json!({"type": "tool", "tool": "bash", "state": {"status": "running"}})),
            (s("assistant"), s("toolUse"), s("bash"))
        );
        assert_eq!(state(json!({"role": "assistant"}), json!({"type": "step-finish", "reason": "tool-calls"})).0, s("toolResult"));
        assert_eq!(state(json!({"role": "assistant", "time": {"completed": 1}}), json!({"type": "step-finish", "reason": "stop"})), (s("assistant"), s("stop"), None));
        assert_eq!(state(json!({"role": "assistant"}), json!({"type": "reasoning"})).0, s("toolResult"), "mid-reply is busy");
        assert_eq!(state(json!({"role": "assistant", "error": {"name": "MessageAbortedError"}}), json!({})).1, s("aborted"));
        assert_eq!(state(json!({"role": "assistant", "error": {"name": "APIError"}}), json!({})).1, s("error"));
    }
}

#[cfg(test)]
mod db_tests {
    use super::*;

    /// A tiny database with opencode's tables: a session waiting for you with one sub-agent,
    /// and a busy one running a tool.
    fn sample_db() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("whosup-oc-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("opencode.db");
        let _ = std::fs::remove_file(&db);
        let now = now_ms();
        let sql = format!(r#"
create table session (id text primary key, project_id text, parent_id text, directory text, title text, cost real default 0,
  tokens_input integer default 0, tokens_output integer default 0, tokens_reasoning integer default 0,
  tokens_cache_read integer default 0, tokens_cache_write integer default 0, agent text, model text,
  time_created integer, time_updated integer, time_archived integer);
create table message (id text primary key, session_id text, time_created integer, time_updated integer, data text);
create table part (id text primary key, message_id text, session_id text, time_created integer, time_updated integer, data text);
insert into session values ('ses_a', 'p', null, '/Users/you/code/shop', 'Fix the cart total', 0.42, 1000, 200, 0, 5000, 0, 'build', '{{"id":"gpt-6-sol","providerID":"openai","variant":"high"}}', {t0}, {t1}, null);
insert into session values ('ses_b', 'p', 'ses_a', '/Users/you/code/shop', 'Find the cart code (@explore subagent)', 0, 10, 5, 0, 0, 0, 'explore', '{{"id":"gpt-6-sol"}}', {t0}, {t1}, null);
insert into session values ('ses_c', 'p', null, '/Users/you/code/api', 'Run the migrations', 0, 10, 5, 0, 0, 0, 'build', '{{"id":"gpt-6-sol"}}', {t0}, {t1}, null);
insert into message values ('m1', 'ses_a', {t0}, {t0}, '{{"role":"user"}}');
insert into part values ('p1', 'm1', 'ses_a', {t0}, {t0}, '{{"type":"text","text":"the cart total is wrong"}}');
insert into message values ('m2', 'ses_a', {t0p}, {t1}, '{{"role":"assistant","time":{{"completed":{t1}}},"tokens":{{"total":6200}}}}');
insert into part values ('p2', 'm2', 'ses_a', {t0p}, {t0p}, '{{"type":"tool","tool":"read","state":{{"status":"completed","input":{{"filePath":"src/cart.ts"}},"output":"export function total()"}}}}');
insert into part values ('p3', 'm2', 'ses_a', {t1m}, {t1m}, '{{"type":"text","text":"Fixed: discounts were applied twice."}}');
insert into part values ('p4', 'm2', 'ses_a', {t1}, {t1}, '{{"type":"step-finish","reason":"stop"}}');
insert into message values ('m3', 'ses_c', {t0}, {t1}, '{{"role":"assistant"}}');
insert into part values ('p5', 'm3', 'ses_c', {t1}, {t1}, '{{"type":"tool","tool":"bash","state":{{"status":"running","input":{{"command":"npm run migrate"}}}}}}');
"#, t0 = now - 60_000, t0p = now - 50_000, t1m = now - 2_000, t1 = now - 1_000);
        let ok = Command::new("sqlite3").arg(&db).arg(sql).status().unwrap();
        assert!(ok.success());
        db
    }

    #[test]
    fn reads_sessions_status_and_conversation() {
        let db = sample_db();
        let all = load(&db, 0);
        assert_eq!(all.len(), 3);
        let a = all.iter().find(|s| s.id == "ses_a").unwrap();
        assert_eq!(a.t.name.as_deref(), Some("Fix the cart total"));
        assert_eq!(a.t.cwd, "/Users/you/code/shop");
        assert_eq!(a.t.model.as_deref(), Some("gpt-6-sol"));
        assert_eq!((a.t.cost, a.t.tokens, a.t.tool_calls), (0.42, 6200, 1));
        assert_eq!((a.t.last_role.as_deref(), a.t.last_stop.as_deref()), (Some("assistant"), Some("stop")));
        assert_eq!(a.t.first_user.as_deref(), Some("the cart total is wrong"));
        assert_eq!(a.t.last_text.as_deref(), Some("Fixed: discounts were applied twice."));
        let b = all.iter().find(|s| s.id == "ses_b").unwrap();
        assert_eq!(b.parent.as_deref(), Some("ses_a"));
        let c = all.iter().find(|s| s.id == "ses_c").unwrap();
        assert_eq!((c.t.last_stop.as_deref(), c.t.last_tool.as_deref()), (Some("toolUse"), Some("bash")));
        assert!(c.t.is_busy());

        let convo: Vec<Entry> = conversation(&db, "ses_a").into_iter().map(|i| i.entry).collect();
        assert_eq!(
            convo,
            vec![
                Entry::User("the cart total is wrong".into()),
                Entry::Call("read".into(), "src/cart.ts".into()),
                Entry::Result("export function total()".into(), false),
                Entry::Reply("Fixed: discounts were applied twice.".into()),
            ]
        );
        // only sessions updated in the window
        assert!(load(&db, now_ms() + 60_000).is_empty());
        std::fs::remove_dir_all(db.parent().unwrap()).ok();
    }
}
