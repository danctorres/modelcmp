//! Task fit: which model is good for what.
//!
//! A task score is the mean percentile (0..100) of a model across the task's benchmarks,
//! ranked against every model Epoch has evaluated. Percentiles make benchmarks of different
//! difficulty comparable, and a model missing one benchmark is still scored by the others.

use crate::data::Model;
use std::collections::{BTreeMap, HashMap};

/// A benchmark scored by fewer models than this gives no percentile: the pool is too small.
pub const MIN_POOL: usize = 10;
/// Models below this overall percentile never count as "value": cheap alone is not enough.
pub const VALUE_FLOOR: f64 = 50.0;

pub enum Need {
    None,
    Tools,
    Vision,
    LongContext,
}

pub struct Task {
    pub name: &'static str,
    pub about: &'static str,
    pub benches: &'static [&'static str],
    pub need: Need,
}

/// "overall" = Epoch Capabilities Index. "value" = capability per dollar (computed after prices are known).
pub const TASKS: &[Task] = &[
    Task {
        name: "coding",
        about: "writing and fixing code",
        need: Need::None,
        benches: &[
            "DeepSWE",
            "FrontierCode",
            "SWE-Bench verified",
            "Terminal Bench",
            "WeirdML",
            "MirrorCode",
            "GSO-Bench",
            "Aider polyglot",
        ],
    },
    Task {
        name: "agentic",
        about: "multi-step tool use, long autonomous tasks",
        need: Need::Tools,
        benches: &[
            "APEX-Agents",
            "Remote Labor Index",
            "OSWorld 2.0",
            "OSWorld",
            "METR Time Horizons",
            "The Agent Company",
            "DeepResearch Bench",
            "Terminal Bench",
        ],
    },
    Task {
        name: "reasoning",
        about: "hard science questions, puzzles, abstraction",
        need: Need::None,
        benches: &[
            "GPQA diamond",
            "HLE",
            "ARC-AGI-2",
            "ARC-AGI",
            "SimpleBench",
            "Mystery Game Puzzles",
            "Chess Puzzles",
        ],
    },
    Task {
        name: "math",
        about: "competition and research math",
        need: Need::None,
        benches: &[
            "FrontierMath-Tiers-1-3-v2-Private",
            "FrontierMath-Tier-4-v2-Private",
            "OTIS Mock AIME 2024-2025",
            "ProofBench",
            "FrontierMath-2025-02-28-Private",
            "MATH level 5",
        ],
    },
    Task {
        name: "knowledge",
        about: "factual recall, low hallucination",
        need: Need::None,
        benches: &["SimpleQA Verified", "MMLU", "TriviaQA"],
    },
    Task { name: "vision", about: "image input (ranked by overall capability)", need: Need::Vision, benches: &[] },
    Task {
        name: "long-context",
        about: "≥200k context (ranked by overall capability)",
        need: Need::LongContext,
        benches: &[],
    },
    Task {
        name: "value",
        about: "capability per dollar, among models ≥50th percentile overall",
        need: Need::None,
        benches: &[],
    },
    Task { name: "overall", about: "Epoch Capabilities Index", need: Need::None, benches: &[] },
];

pub fn task(name: &str) -> Option<&'static Task> {
    TASKS.iter().find(|t| t.name == name)
}

pub fn task_names() -> impl Iterator<Item = &'static str> {
    TASKS.iter().map(|t| t.name)
}

/// Fraction of `all` below x (ties count half), as 0..100.
fn pct_rank(x: f64, all: &[f64]) -> f64 {
    let below = all.iter().filter(|v| **v < x).count() as f64;
    let equal = all.iter().filter(|v| **v == x).count() as f64;
    100.0 * (below + equal / 2.0) / all.len() as f64
}

/// Per Epoch group `(key, eci, bench -> score)`: key -> task -> percentile.
pub fn percentiles<'a>(
    groups: impl Iterator<Item = (&'a str, Option<f64>, &'a BTreeMap<String, f64>)> + Clone,
) -> HashMap<String, BTreeMap<String, f64>> {
    let mut per_bench: HashMap<&str, Vec<f64>> = HashMap::new();
    let mut ecis = Vec::new();
    for (_, eci, scores) in groups.clone() {
        ecis.extend(eci);
        for (b, s) in scores {
            per_bench.entry(b).or_default().push(*s);
        }
    }
    per_bench.retain(|_, pool| pool.len() >= MIN_POOL);
    let mut out = HashMap::new();
    for (key, eci, scores) in groups {
        let mut fit = BTreeMap::new();
        if let Some(e) = eci {
            let p = pct_rank(e, &ecis);
            for t in ["overall", "vision", "long-context"] {
                fit.insert(t.to_string(), p);
            }
        }
        for t in TASKS.iter().filter(|t| !t.benches.is_empty()) {
            let ps: Vec<f64> =
                t.benches.iter().filter_map(|b| Some(pct_rank(*scores.get(*b)?, per_bench.get(b)?))).collect();
            if !ps.is_empty() {
                fit.insert(t.name.to_string(), ps.iter().sum::<f64>() / ps.len() as f64);
            }
        }
        out.insert(key.to_string(), fit);
    }
    out
}

/// "value": overall percentile per blended dollar, itself ranked as a percentile,
/// only for models at or above `VALUE_FLOOR` so that cheap weak models do not win.
pub fn add_value(models: &mut [Model]) {
    let raw: Vec<Option<f64>> = models
        .iter()
        .map(|m| {
            let overall = *m.fit.get("overall")?;
            (overall >= VALUE_FLOOR).then_some(overall / m.blended()?)
        })
        .collect();
    let all: Vec<f64> = raw.iter().flatten().copied().collect();
    for (m, v) in models.iter_mut().zip(raw) {
        if let Some(v) = v {
            m.fit.insert("value".into(), pct_rank(v, &all));
        }
    }
}

/// Task score if the model qualifies for the task.
pub fn fit(m: &Model, t: &Task) -> Option<f64> {
    let ok = match t.need {
        Need::None => true,
        Need::Tools => m.tool_call,
        Need::Vision => m.vision,
        Need::LongContext => m.context >= 200_000,
    };
    if ok { m.fit.get(t.name).copied() } else { None }
}

/// Top two benchmark-backed tasks the model is strong at (≥60th percentile).
pub fn best_for(m: &Model) -> Vec<&'static str> {
    let mut v: Vec<(&str, f64)> = TASKS
        .iter()
        .filter(|t| !t.benches.is_empty() || t.name == "value")
        .filter_map(|t| Some((t.name, fit(m, t)?)))
        .filter(|(_, s)| *s >= 60.0)
        .collect();
    v.sort_by(|a, b| b.1.total_cmp(&a.1));
    v.into_iter().take(2).map(|(n, _)| n).collect()
}

/// Models ranked for a task, best first; models without data for the task are dropped.
pub fn rank<'a>(models: impl Iterator<Item = &'a Model>, t: &Task) -> Vec<(&'a Model, f64)> {
    let mut v: Vec<_> = models.filter_map(|m| Some((m, fit(m, t)?))).collect();
    v.sort_by(|a, b| b.1.total_cmp(&a.1));
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::Offer;

    fn scores(pairs: &[(&str, f64)]) -> BTreeMap<String, f64> {
        pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    #[test]
    fn ranks_by_task() {
        let a = scores(&[("GPQA diamond", 0.9), ("DeepSWE", 0.2)]);
        let b = scores(&[("GPQA diamond", 0.5), ("DeepSWE", 0.7)]);
        // Filler groups so both benchmarks reach MIN_POOL.
        let filler = scores(&[("GPQA diamond", 0.0), ("DeepSWE", 1.0)]);
        let mut groups = vec![("a", Some(150.0), &a), ("b", Some(140.0), &b)];
        groups.extend((0..MIN_POOL).map(|_| ("f", None, &filler)));
        let p = percentiles(groups.iter().copied());
        assert!(p["a"]["reasoning"] > p["b"]["reasoning"]);
        assert!(p["b"]["coding"] > p["a"]["coding"]);
        assert!(p["a"]["overall"] > p["b"]["overall"]);

        let mk = |k: &str, fit: &BTreeMap<String, f64>, tools: bool| Model {
            key: k.into(),
            fit: fit.clone(),
            tool_call: tools,
            ..Default::default()
        };
        let models = [mk("a", &p["a"], true), mk("b", &p["b"], false)];
        let r = rank(models.iter(), task("coding").unwrap());
        assert_eq!(r[0].0.key, "b");
        assert_eq!(best_for(&models[0]), vec!["reasoning"]);
        // agentic needs tool calling; b lacks it
        assert!(fit(&models[1], task("agentic").unwrap()).is_none());
    }

    #[test]
    fn small_pools_give_no_percentile() {
        let s = scores(&[("TriviaQA", 0.9)]);
        let groups = [("a", None, &s), ("b", None, &s)];
        let p = percentiles(groups.iter().copied());
        assert!(!p["a"].contains_key("knowledge"));
    }

    #[test]
    fn value_needs_capability() {
        let mk = |overall: f64, price: f64| Model {
            fit: scores(&[("overall", overall)]),
            offers: vec![Offer { input: price, output: price, ..Default::default() }],
            ..Default::default()
        };
        let mut models = [mk(25.0, 0.01), mk(70.0, 1.0), mk(90.0, 10.0)];
        add_value(&mut models);
        assert!(!models[0].fit.contains_key("value"), "cheap but weak");
        assert!(models[1].fit["value"] > models[2].fit["value"]);
    }
}
