//! Cost estimates for agents that record tokens but not cost (Claude Code, Codex), priced per
//! million tokens. Prices come from, in order: `price.<model>=…` lines in the config (matched
//! against a fragment of the model name), pi's model list (models-store.json), then a built-in
//! table by model family.

use crate::data::Transcript;
use crate::settings::Price;
use std::sync::OnceLock;

static USER_PRICES: OnceLock<Vec<(String, Price)>> = OnceLock::new();

pub fn set_prices(p: Vec<(String, Price)>) {
    let _ = USER_PRICES.set(p);
}

const fn p(input: f64, output: f64, cache_read: f64, cache_write: f64) -> Price {
    Price { input, output, cache_read, cache_write }
}

/// List prices, USD per million tokens. Only an estimate: check your plan's actual rates.
const DEFAULTS: &[(&str, Price)] = &[
    ("opus", p(5.0, 25.0, 0.5, 6.25)),
    ("sonnet", p(3.0, 15.0, 0.3, 3.75)),
    ("haiku", p(1.0, 5.0, 0.1, 1.25)),
    ("gpt", p(1.25, 10.0, 0.125, 0.0)),
    ("codex", p(1.25, 10.0, 0.125, 0.0)),
];

pub fn price_for(model: &str) -> Option<Price> {
    let m = model.to_lowercase();
    USER_PRICES
        .get()
        .into_iter()
        .flatten()
        .find(|(k, _)| m.contains(k.as_str()))
        .map(|(_, p)| p.clone())
        .or_else(|| crate::models::lookup(model).and_then(|i| i.price))
        .or_else(|| DEFAULTS.iter().find(|(k, _)| m.contains(k)).map(|(_, p)| p.clone()))
}

/// Estimated USD for a transcript's token counts, if its model has a price.
pub fn estimate(t: &Transcript) -> Option<f64> {
    let price = price_for(t.model.as_deref()?)?;
    let m = 1_000_000.0;
    let c = t.in_tokens as f64 * price.input / m
        + t.out_tokens as f64 * price.output / m
        + t.cache_read_tokens as f64 * price.cache_read / m
        + t.cache_write_tokens as f64 * price.cache_write / m;
    (c > 0.0).then_some(c)
}

/// Fill in `cost` from tokens when the agent didn't record one.
pub fn fill(t: &mut Transcript) {
    if t.cost == 0.0 {
        if let Some(c) = estimate(t) {
            t.cost = c;
            t.cost_estimated = true;
        }
    }
}
