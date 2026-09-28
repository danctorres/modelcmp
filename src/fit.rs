//! Task fit: which model is good for what.
//!
//! A task score is the mean percentile (0..100) of a model across the task's benchmarks,
//! ranked against every model Epoch has evaluated. Percentiles make benchmarks of different
//! difficulty comparable, and a model missing one benchmark is still scored by the others.

use crate::data::Model;
use std::collections::{BTreeMap, HashMap};

/// A benchmark scored by fewer models than this gives no percentile: the pool is too small.
pub const MIN_POOL: usize = 10;
/// Models below this coding percentile are never recommended for "value": cheap alone is not enough.
pub const VALUE_FLOOR: f64 = 50.0;

pub enum Need {
    None,
    Tools,
    Vision,
    LongContext,
    /// At or above `VALUE_FLOOR` on coding.
    Coder,
}

pub struct Task {
    pub name: &'static str,
    pub about: &'static str,
    /// When a model high on this task is the right pick.
    pub when: &'static str,
    pub benches: &'static [&'static str],
    pub need: Need,
}

/// A task is a capability software engineering needs, judged by `when`; its benchmarks need
/// not be about code. Math and factual-recall benchmarks stay out.
/// "overall" = Epoch Capabilities Index. "value" = coding per dollar (computed after prices are known).
/// Listed with the general pick first, then in decision order: coding, the cheaper pick, the rest.
pub const TASKS: &[Task] = &[
    Task {
        name: "overall",
        about: "Epoch Capabilities Index",
        when: "a tiebreaker, or work that fits no other task",
        need: Need::None,
        benches: &[],
    },
    Task {
        name: "coding",
        about: "writing and fixing code",
        when: "fixing a bug, adding a feature to an existing repo, refactors: code that must compile and pass tests",
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
        name: "value",
        about: "coding per dollar, among models ≥50th percentile on coding",
        when: "routine coding that needs no top reasoning: the good enough, cheaper pick",
        need: Need::Coder,
        benches: &[],
    },
    Task {
        name: "agentic",
        about: "multi-step tool use, long autonomous tasks",
        when: "unattended multi-step runs: migrate, run tests, fix what breaks; recovering from errors without drifting",
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
        when: "subtle bugs, algorithm and architecture design, contradictory specs, tricky invariants",
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
        name: "vision",
        about: "image input (ranked by overall capability)",
        when: "screenshots, UI mockups, diagrams as input",
        need: Need::Vision,
        benches: &[],
    },
    Task {
        name: "long-context",
        about: "≥200k context (ranked by overall capability)",
        when: "a whole repo, a long log or many files in one prompt; the window fits, not proof it is used well",
        need: Need::LongContext,
        benches: &[],
    },
];

pub fn task(name: &str) -> Option<&'static Task> {
    TASKS.iter().find(|t| t.name == name)
}

/// Every benchmark a task uses, sorted; the others are not even parsed.
pub fn task_benches() -> Vec<&'static str> {
    let mut v: Vec<&str> = TASKS.iter().flat_map(|t| t.benches.iter().copied()).collect();
    v.sort();
    v.dedup();
    v
}

pub fn task_names() -> impl Iterator<Item = &'static str> {
    TASKS.iter().map(|t| t.name)
}

/// Tasks ranked by the ECI percentile; they show the raw ECI, as the table's ECI column does.
const ECI_TASKS: [&str; 3] = ["overall", "vision", "long-context"];

/// The value a task's frontier shows for a model ranked `s`: its value in the task's table column.
pub fn shown(m: &Model, t: &Task, s: f64) -> f64 {
    if ECI_TASKS.contains(&t.name) { m.eci.unwrap_or(s) } else { s }
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
            for t in ECI_TASKS {
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

/// "value": coding percentile per blended dollar, itself ranked as a percentile, for every
/// model with both. The task only counts models at or above `VALUE_FLOOR` (see `Need::Coder`).
pub fn add_value(models: &mut [Model]) {
    let raw: Vec<Option<f64>> = models.iter().map(|m| Some(m.fit.get("coding")? / m.blended()?)).collect();
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
        Need::Coder => m.fit.get("coding").is_some_and(|&c| c >= VALUE_FLOOR),
    };
    if ok { m.fit.get(t.name).copied() } else { None }
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
        let a = scores(&[("METR Time Horizons", 0.9), ("DeepSWE", 0.2)]);
        let b = scores(&[("METR Time Horizons", 0.5), ("DeepSWE", 0.7)]);
        // Filler groups so both benchmarks reach MIN_POOL.
        let filler = scores(&[("METR Time Horizons", 0.0), ("DeepSWE", 1.0)]);
        let mut groups = vec![("a", Some(150.0), &a), ("b", Some(140.0), &b)];
        groups.extend((0..MIN_POOL).map(|_| ("f", None, &filler)));
        let p = percentiles(groups.iter().copied());
        assert!(p["a"]["agentic"] > p["b"]["agentic"]);
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
        // agentic needs tool calling; b lacks it
        assert!(fit(&models[1], task("agentic").unwrap()).is_none());
    }

    #[test]
    fn small_pools_give_no_percentile() {
        let s = scores(&[("DeepSWE", 0.9)]);
        let groups = [("a", None, &s), ("b", None, &s)];
        let p = percentiles(groups.iter().copied());
        assert!(!p["a"].contains_key("coding"));
    }

    #[test]
    fn value_needs_capability() {
        let mk = |coding: f64, price: f64| Model {
            fit: scores(&[("coding", coding)]),
            offers: vec![Offer { input: price, output: price, ..Default::default() }],
            ..Default::default()
        };
        let mut models = [mk(25.0, 0.01), mk(70.0, 1.0), mk(90.0, 10.0)];
        add_value(&mut models);
        assert!(models[0].fit.contains_key("value"), "the ratio is shown for every model");
        assert!(fit(&models[0], task("value").unwrap()).is_none(), "cheap but weak");
        assert!(models[1].fit["value"] > models[2].fit["value"]);
    }
}
