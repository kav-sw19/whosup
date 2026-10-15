//! Model facts from pi's model list (`<pi agent dir>/models-store.json`): each model's context
//! window and price per million tokens, as its provider publishes them. Native providers
//! (anthropic, openai-codex) win over resellers when both list a model.

use crate::settings::Price;
use serde_json::Value;
use std::{collections::HashMap, path::Path, sync::OnceLock};

#[derive(Clone, Debug, Default)]
pub struct ModelInfo {
    pub window: Option<u64>,
    pub price: Option<Price>,
}

static STORE: OnceLock<HashMap<String, ModelInfo>> = OnceLock::new();

pub fn parse(json: &str) -> HashMap<String, ModelInfo> {
    let mut out: HashMap<String, ModelInfo> = HashMap::new();
    let Ok(Value::Object(providers)) = serde_json::from_str::<Value>(json) else { return out };
    let mut names: Vec<&String> = providers.keys().collect();
    names.sort_by_key(|n| match n.as_str() {
        "anthropic" | "openai-codex" | "openai" => 0,
        _ => 1,
    });
    for name in names {
        for m in providers[name].get("models").and_then(Value::as_array).into_iter().flatten() {
            let Some(id) = m.get("id").and_then(Value::as_str) else { continue };
            let c = |k: &str| m.pointer(&format!("/cost/{k}")).and_then(Value::as_f64);
            let info = ModelInfo {
                window: m.get("contextWindow").and_then(Value::as_u64),
                price: match (c("input"), c("output")) {
                    (Some(i), Some(o)) if i > 0.0 || o > 0.0 => Some(Price {
                        input: i,
                        output: o,
                        cache_read: c("cacheRead").unwrap_or(i * 0.1),
                        cache_write: c("cacheWrite").unwrap_or(i * 1.25),
                    }),
                    _ => None,
                },
            };
            // `anthropic/claude-opus-4.8` is also known as `claude-opus-4.8`
            let short = id.rsplit('/').next().unwrap_or(id).trim_start_matches('~').to_string();
            for key in [id.to_string(), short] {
                out.entry(key).or_insert_with(|| info.clone());
            }
        }
    }
    out
}

pub fn load(agent_dir: &Path) {
    let map = std::fs::read_to_string(agent_dir.join("models-store.json")).map(|t| parse(&t)).unwrap_or_default();
    let _ = STORE.set(map);
}

/// Model names differ in punctuation between agents (`claude-opus-4-8` vs `claude-opus-4.8`).
fn variants(model: &str) -> Vec<String> {
    let m = model.rsplit('/').next().unwrap_or(model).to_lowercase();
    let m = m.split('[').next().unwrap_or(&m).to_string(); // `claude-opus-4-8[1m]`
    vec![m.clone(), m.replace('.', "-"), dotted(&m)]
}

/// `claude-opus-4-8` -> `claude-opus-4.8` (dot between the last two numeric parts)
fn dotted(m: &str) -> String {
    let parts: Vec<&str> = m.split('-').collect();
    let n = parts.len();
    if n >= 2 && parts[n - 1].chars().all(|c| c.is_ascii_digit()) && parts[n - 2].chars().all(|c| c.is_ascii_digit()) {
        format!("{}.{}", parts[..n - 1].join("-"), parts[n - 1])
    } else {
        m.to_string()
    }
}

pub fn lookup_in<'a>(map: &'a HashMap<String, ModelInfo>, model: &str) -> Option<&'a ModelInfo> {
    variants(model).iter().find_map(|v| map.get(v))
}

pub fn lookup(model: &str) -> Option<ModelInfo> {
    lookup_in(STORE.get()?, model).cloned()
}

/// Context window for a model: pi's model list, else a default for the family.
pub fn window(model: &str) -> Option<u64> {
    lookup(model).and_then(|i| i.window).or_else(|| {
        let m = model.to_lowercase();
        if m.contains("claude") || m.contains("opus") || m.contains("sonnet") || m.contains("haiku") {
            Some(200_000)
        } else if m.contains("gpt") || m.contains("codex") {
            Some(272_000)
        } else {
            None
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
      "openrouter": {"models": [
        {"id": "anthropic/claude-opus-4.8", "contextWindow": 1000000, "cost": {"input": 99, "output": 99}}
      ]},
      "anthropic": {"models": [
        {"id": "claude-opus-4-8", "contextWindow": 1000000, "cost": {"input": 5, "output": 25, "cacheRead": 0.5, "cacheWrite": 6.25}},
        {"id": "claude-haiku-4-5", "contextWindow": 200000, "cost": {"input": 1, "output": 5}}
      ]},
      "openai-codex": {"models": [{"id": "gpt-6-sol", "contextWindow": 272000, "cost": {"input": 0, "output": 0}}]}
    }"#;

    #[test]
    fn reads_windows_and_prices() {
        let m = parse(SAMPLE);
        let opus = lookup_in(&m, "claude-opus-4-8").unwrap();
        assert_eq!(opus.window, Some(1_000_000));
        // the native provider's price, not the reseller's
        assert_eq!(opus.price.as_ref().unwrap().input, 5.0);
        // other spellings of the same model
        assert!(lookup_in(&m, "anthropic/claude-opus-4.8").is_some());
        assert!(lookup_in(&m, "claude-opus-4-8[1m]").is_some());
        let haiku = lookup_in(&m, "claude-haiku-4-5").unwrap();
        assert_eq!(haiku.price.as_ref().unwrap().cache_read, 0.1);
        // a zero price (subscription models) is no price
        assert!(lookup_in(&m, "gpt-6-sol").unwrap().price.is_none());
    }
}
