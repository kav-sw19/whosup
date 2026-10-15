//! The Usage tab's numbers: spend and tokens per day, and by agent, model and project, over the
//! last N days. A pure function of the sessions, so it's easy to test.

use crate::data::{day_of, AgentSource, Day, Session};
use std::collections::HashMap;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Group {
    pub label: String,
    pub cost: f64,
    pub tokens: u64,
    pub sessions: usize,
    pub estimated: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Report {
    pub range_days: i64,
    /// oldest first, one entry per day of the range (zeros included)
    pub days: Vec<(i64, Day)>,
    pub today: Day,
    pub week: Day,
    pub total: Day,
    pub sessions: usize,
    pub by_agent: Vec<Group>,
    pub by_model: Vec<Group>,
    pub by_project: Vec<Group>,
}

fn add(d: &mut Day, x: &Day) {
    d.cost += x.cost;
    d.tokens += x.tokens;
    d.estimated |= x.estimated;
    d.est_cost += x.est_cost;
}

pub fn build(sessions: &[Session], project_of: &dyn Fn(&str) -> String, today: i64, range_days: i64) -> Report {
    let first = today - range_days + 1;
    let mut per_day: HashMap<i64, Day> = HashMap::new();
    let mut groups: [HashMap<String, Group>; 3] = Default::default();
    let mut count = 0;
    for s in sessions {
        // this session's days in range, plus its sub-agents' spend on the day each started
        let mut days: Vec<(i64, Day)> = s.t.daily.range(first..=today).map(|(k, v)| (*k, *v)).collect();
        for a in &s.agents {
            if a.cost > 0.0 || a.tokens > 0 {
                let Some(at) = a.started.or(a.last_activity) else { continue };
                let day = day_of(at);
                if (first..=today).contains(&day) {
                    let est = matches!(a.source, AgentSource::ClaudeTask | AgentSource::CodexAgent);
                    let estimated = est && a.cost > 0.0;
                    days.push((day, Day { cost: a.cost, tokens: a.tokens, estimated, est_cost: if estimated { a.cost } else { 0.0 } }));
                }
            }
        }
        if days.is_empty() {
            continue;
        }
        count += 1;
        let mut sum = Day::default();
        for (k, v) in &days {
            add(per_day.entry(*k).or_default(), v);
            add(&mut sum, v);
        }
        let keys = [
            s.tool.label().to_string(),
            crate::ui::short_model(s.t.model.as_deref().unwrap_or("unknown")),
            crate::ui::home_rel(&project_of(&s.t.cwd)),
        ];
        for (g, key) in groups.iter_mut().zip(keys) {
            let e = g.entry(key.clone()).or_insert_with(|| Group { label: key, ..Group::default() });
            e.cost += sum.cost;
            e.tokens += sum.tokens;
            e.sessions += 1;
            e.estimated |= sum.estimated;
        }
    }
    let mut r = Report { range_days, sessions: count, ..Report::default() };
    for day in first..=today {
        let d = per_day.get(&day).copied().unwrap_or_default();
        add(&mut r.total, &d);
        if day > today - 7 {
            add(&mut r.week, &d);
        }
        if day == today {
            r.today = d;
        }
        r.days.push((day, d));
    }
    let sorted = |g: &HashMap<String, Group>| {
        let mut v: Vec<Group> = g.values().cloned().collect();
        v.sort_by(|a, b| b.cost.partial_cmp(&a.cost).unwrap_or(std::cmp::Ordering::Equal).then(b.tokens.cmp(&a.tokens)));
        v
    };
    r.by_agent = sorted(&groups[0]);
    r.by_model = sorted(&groups[1]);
    r.by_project = sorted(&groups[2]);
    r
}
