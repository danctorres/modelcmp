//! Task fit: which model is good for what.
//!
//! Epoch fits `score ≈ sigmoid(slope × (ECI − EDI))` per benchmark, on scores rescaled so that
//! guessing is 0. A model's capability on a task is the one that best explains its scores on the
//! task's benchmarks, starting from its ECI: a few scores move it a little, many scores that
//! agree move it more. It is ranked as a percentile (0..100) among every Epoch model, the same
//! models for every task, and shown in ECI points.
//! Models tested on different benchmarks stay comparable, and a 0 or 100% only says "at most"
//! or "at least".

use crate::data::Model;
use std::collections::{BTreeMap, HashMap};

/// Scores sit about 0.5 (logit) around Epoch's fit, so one score weighs as 1 / 0.5² / ¼ = 16
/// coin flips at 50%.
const WEIGHT: f64 = 16.0;
/// How far a task's capability strays from ECI: 1 to 2.5 points in Epoch's data (2026-09).
// ponytail: one spread for every task, fit it per task if one shows much more than the others.
const TASK_SD: f64 = 2.0;

/// Months over which the source's best score is taken to have risen steadily (`add_lag`).
const WINDOW: f64 = 12.0;

pub enum Need {
    None,
    Tools,
    Vision,
    /// No further behind the source's best on coding than the `low` tier allows: cheap alone
    /// is not enough. None is while coding has no pace to tell by (`add_lag`).
    Coder,
}

pub struct Task {
    pub name: &'static str,
    pub about: &'static str,
    /// When a model high on this task is the right pick.
    pub when: &'static str,
    pub benches: &'static [&'static str],
    /// The same for Artificial Analysis: one of its API's `evaluations` fields, shown as is.
    pub aa: Option<&'static str>,
    /// Its other fields about the task, still run on new models: the column's dropdown lists
    /// them after `aa`.
    pub aa_more: &'static [&'static str],
    pub need: Need,
}

impl Task {
    /// With the source in use: the one benchmark the task's score is, when it is one (Artificial
    /// Analysis), and the others about the task; Epoch fits all of its into one score.
    pub fn sourced(&self) -> (Option<&'static str>, &'static [&'static str]) {
        match crate::data::source() {
            crate::data::Source::Aa => (self.aa, self.aa_more),
            _ => (None, self.benches),
        }
    }
}

/// Artificial Analysis's overall index, its ECI.
pub const AA_INDEX: &str = "artificial_analysis_intelligence_index";

/// A task is a capability software engineering needs, judged by `when`; its benchmarks need
/// not be about code. Math and factual-recall benchmarks stay out.
/// "overall" = Epoch Capabilities Index. "value" = coding per dollar (computed after prices are known).
/// Listed with the general pick first, then in the table's column order: coding, agentic, reasoning, the cheaper pick, vision.
pub const TASKS: &[Task] = &[
    Task {
        name: "overall",
        about: "overall index: ECI or AAII",
        when: "a tiebreaker, or work that fits no other task",
        need: Need::None,
        aa: None,
        aa_more: &[],
        benches: &[],
    },
    Task {
        name: "coding",
        about: "writing and fixing code",
        when: "fixing a bug, adding a feature to an existing repo, refactors",
        need: Need::None,
        aa: Some("artificial_analysis_coding_index"),
        aa_more: &["terminalbench_v4_0", "scicode"],
        benches: &[
            "DeepSWE",
            "FrontierCode",
            "FrontierSWE",
            "SWE-Bench verified",
            "Terminal Bench",
            "WeirdML",
            "MirrorCode",
            "GSO-Bench",
        ],
    },
    Task {
        name: "agentic",
        about: "multi-step tool use, long autonomous tasks",
        when: "unattended multi-step runs, migrations, fix-until-tests-pass loops",
        need: Need::Tools,
        aa: Some("terminalbench_v4_0"),
        aa_more: &["tau_banking"],
        benches: &["APEX-Agents", "Remote Labor Index", "OSWorld 2.0", "DeepResearch Bench", "Terminal Bench"],
    },
    Task {
        name: "reasoning",
        about: "hard science questions, puzzles, abstraction",
        when: "subtle bugs, algorithm and architecture design, contradictory specs, tricky invariants",
        need: Need::None,
        aa: Some("hle"),
        aa_more: &["gpqa", "lcr"],
        benches: &[
            "GPQA diamond",
            "HLE",
            "ARC-AGI-2",
            "ARC-AGI",
            "SimpleBench",
            "Mystery Game Puzzles",
            "Chess Puzzles",
            "LMCA",
            "DTBench",
        ],
    },
    Task {
        name: "value",
        about: "coding per dollar, among the models close to the best on coding",
        when: "routine coding that needs no top reasoning",
        need: Need::Coder,
        aa: None,
        aa_more: &[],
        benches: &[],
    },
    Task {
        name: "vision",
        about: "image input (ranked by overall capability)",
        when: "screenshots, UI mockups, diagrams as input",
        need: Need::Vision,
        aa: None,
        aa_more: &[],
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

/// Every Artificial Analysis field a task uses, sorted.
pub fn aa_fields() -> Vec<&'static str> {
    let mut v: Vec<&str> = TASKS.iter().flat_map(|t| t.aa.into_iter().chain(t.aa_more.iter().copied())).collect();
    v.sort();
    v.dedup();
    v
}

pub fn task_names() -> impl Iterator<Item = &'static str> {
    TASKS.iter().map(|t| t.name)
}

/// Tasks ranked by the ECI percentile; they show the raw ECI, as the table's ECI column does.
const ECI_TASKS: [&str; 2] = ["overall", "vision"];

/// The value a task shows for a model ranked `s`, in its table column and frontier: the
/// source's overall index, a task's capability in ECI points (Epoch) or its benchmark's score
/// (Artificial Analysis); "value" alone stays a percentile.
pub fn shown(m: &Model, t: &Task, s: f64) -> f64 {
    // A favorite the task cannot score (`view::task_frontier`) shows no score.
    if s.is_nan() {
        return s;
    }
    if ECI_TASKS.contains(&t.name) { m.eci.unwrap_or(s) } else { m.shown.get(t.name).copied().unwrap_or(s) }
}

/// Per task: key -> task -> percentile, and key -> task -> the value it shows (`shown`).
pub type Fit = (HashMap<String, BTreeMap<String, f64>>, HashMap<String, BTreeMap<String, f64>>);

/// Fraction of `all` below x (ties count half), as 0..100.
fn pct_rank(x: f64, all: &[f64]) -> f64 {
    let below = all.iter().filter(|v| **v < x).count() as f64;
    let equal = all.iter().filter(|v| **v == x).count() as f64;
    100.0 * (below + equal / 2.0) / all.len() as f64
}

/// Epoch's fit of one benchmark.
#[derive(Clone, Copy, Debug)]
pub struct Bench {
    /// Epoch Difficulty Index: the capability that scores 50%.
    pub edi: f64,
    /// How sharply the benchmark separates models: always > 0.
    pub slope: f64,
    /// Score from guessing alone.
    pub floor: f64,
    /// Best possible score: always > `floor`.
    pub ceiling: f64,
}

/// Most likely capability given `(bench, raw score)` pairs and a normal prior `(mean, sd)`.
fn capability(obs: &[(Bench, f64)], mean: f64, sd: f64) -> f64 {
    let prior = 1.0 / (sd * sd);
    let mut c = mean;
    // Newton's method: the log-likelihood is concave, so it converges in a few steps.
    for _ in 0..50 {
        let (mut grad, mut curv) = (prior * (mean - c), prior);
        for (b, s) in obs {
            let y = ((s - b.floor) / (b.ceiling - b.floor)).clamp(0.0, 1.0);
            let p = 1.0 / (1.0 + (-b.slope * (c - b.edi)).exp());
            grad += WEIGHT * b.slope * (y - p);
            curv += WEIGHT * b.slope * b.slope * p * (1.0 - p);
        }
        let step = (grad / curv).clamp(-10.0, 10.0);
        c += step;
        if step.abs() < 1e-6 {
            break;
        }
    }
    c
}

/// Per Epoch group `(key, eci, bench -> score)` and bench -> fit: each task's percentile and capability.
pub fn percentiles<'a>(
    groups: impl Iterator<Item = (&'a str, Option<f64>, &'a BTreeMap<String, f64>)>,
    benches: &HashMap<String, Bench>,
) -> Fit {
    let groups: Vec<_> = groups.collect();
    let ecis: Vec<f64> = groups.iter().filter_map(|g| g.1).collect();
    let n = ecis.len().max(1) as f64;
    let mean = ecis.iter().sum::<f64>() / n;
    let sd = (ecis.iter().map(|e| (e - mean).powi(2)).sum::<f64>() / n).sqrt().max(1.0);
    let obs = |names: &[&str], scores: &BTreeMap<String, f64>| -> Vec<(Bench, f64)> {
        names.iter().filter_map(|b| Some((*benches.get(*b)?, *scores.get(*b)?))).collect()
    };
    let all = task_benches();
    // (overall, task -> (capability, scored)). A task without scores leaves the capability at
    // the overall one, so every model sits in every task's pool and all columns rank the same models.
    let caps: Vec<(f64, Vec<(f64, bool)>)> = groups
        .iter()
        .map(|(_, eci, scores)| {
            // A model Epoch gave no ECI starts from one fit to all its scores.
            let center = eci.unwrap_or_else(|| capability(&obs(&all, scores), mean, sd));
            let tasks = TASKS
                .iter()
                .map(|t| {
                    let o = obs(t.benches, scores);
                    (capability(&o, center, TASK_SD), !o.is_empty())
                })
                .collect();
            (center, tasks)
        })
        .collect();
    let centers: Vec<f64> = caps.iter().map(|c| c.0).collect();
    let pools: Vec<Vec<f64>> = (0..TASKS.len()).map(|i| caps.iter().map(|c| c.1[i].0).collect()).collect();
    let (mut out, mut shown) = (HashMap::new(), HashMap::new());
    for ((key, eci, _), (center, tasks)) in groups.iter().zip(&caps) {
        let (mut fit, mut show) = (BTreeMap::new(), BTreeMap::new());
        if eci.is_some() {
            let p = pct_rank(*center, &centers);
            for t in ECI_TASKS {
                fit.insert(t.to_string(), p);
            }
        }
        for (i, t) in TASKS.iter().enumerate() {
            if let (c, true) = tasks[i] {
                fit.insert(t.name.to_string(), pct_rank(c, &pools[i]));
                show.insert(t.name.to_string(), c);
            }
        }
        out.insert(key.to_string(), fit);
        shown.insert(key.to_string(), show);
    }
    (out, shown)
}

/// What Artificial Analysis's models scored, each `(index, field -> score)`: the index, and
/// every field a task uses. One model's scores are ranked among them (`aa_fit`).
#[derive(Default, Debug)]
pub struct AaPools {
    index: Vec<f64>,
    fields: HashMap<&'static str, Vec<f64>>,
}

pub fn aa_pools<'a>(groups: impl Iterator<Item = (Option<f64>, &'a BTreeMap<String, f64>)>) -> AaPools {
    let groups: Vec<_> = groups.collect();
    AaPools {
        index: groups.iter().filter_map(|g| g.0).collect(),
        fields: aa_fields()
            .into_iter()
            .map(|f| (f, groups.iter().filter_map(|g| g.1.get(f).copied()).collect()))
            .collect(),
    }
}

/// An Artificial Analysis model's fit, `index` and `field -> score`: each task's percentile among
/// the models scored on its field, and that score as 0..100; overall and vision the index's, as
/// with ECI. Epoch's fit needs Epoch's benchmark difficulties, so not here.
pub fn aa_fit(
    index: Option<f64>,
    scores: &BTreeMap<String, f64>,
    pools: &AaPools,
) -> (BTreeMap<String, f64>, BTreeMap<String, f64>) {
    let (mut fit, mut show) = (BTreeMap::new(), BTreeMap::new());
    if let Some(i) = index {
        for t in ECI_TASKS {
            fit.insert(t.to_string(), pct_rank(i, &pools.index));
        }
    }
    for t in TASKS {
        if let Some(f) = t.aa
            && let (Some(&x), Some(pool)) = (scores.get(f), pools.fields.get(f))
        {
            fit.insert(t.name.to_string(), pct_rank(x, pool));
            show.insert(t.name.to_string(), x * 100.0);
        }
    }
    (fit, show)
}

/// What a free model counts as costing in "value": below any price, so free models rank over
/// every priced one, and among themselves by coding.
const FREE: f64 = 1e-9;

/// A release date, "2026-09-18" or "2026-09", in months since 1970.
// ponytail: months of the mean length, so a day or two off; nothing here turns on a day.
fn months(date: &str) -> Option<f64> {
    let mut parts = date.split('-').map(|p| p.parse::<f64>().ok());
    let (year, month, day) = (parts.next()??, parts.next()??, parts.next().flatten().unwrap_or(15.0));
    Some((year - 1970.0) * 12.0 + (month - 1.0) + (day - 1.0) / 30.44)
}

/// Each task's `Model::lag`: how far the model is behind the source's best on it, in months of
/// progress, a month being a twelfth of what the best score rose over the last `WINDOW`
/// months. `now` is in seconds since 1970. One scale for every source and task, which a share
/// of the best score is not (ECI points have no zero) and a gap in points is not either (each
/// benchmark moves at its own pace); a percentile among every model ever scored counts a
/// model at half the best score as near the top. A task scored for less than `WINDOW` months,
/// or whose best has not risen in them, has no lag, and each of its tiers picks the best;
/// "value" has none, being a rank.
// ponytail: a straight line over `WINDOW`; a benchmark whose scores rose in a few of those
// months counts its models as closer than they are. The best score at each date if it bites.
pub fn add_lag(models: &mut [Model], now: f64) {
    let now = now / 86400.0 / 30.436_875;
    for t in TASKS.iter().filter(|t| t.name != "value") {
        let score = |m: &Model| if ECI_TASKS.contains(&t.name) { m.eci } else { m.shown.get(t.name).copied() };
        let best = |old: bool| {
            let scores = models.iter().filter(|m| !old || months(&m.release).is_some_and(|r| r <= now - WINDOW));
            scores.filter_map(score).fold(f64::NEG_INFINITY, f64::max)
        };
        let (best, rate) = (best(false), (best(false) - best(true)) / WINDOW);
        for m in models.iter_mut() {
            match score(m).filter(|_| rate > 0.0 && rate.is_finite()) {
                Some(s) => m.lag.insert(t.name.to_string(), (best - s) / rate),
                None => m.lag.remove(t.name),
            };
        }
    }
}

/// "value": coding percentile per blended dollar, itself ranked as a percentile, for every
/// model with both. The task only counts the models close to the best on coding (see `Need::Coder`).
pub fn add_value(models: &mut [Model]) {
    let raw: Vec<Option<f64>> = models.iter().map(|m| Some(m.fit.get("coding")? / m.cost()?.max(FREE))).collect();
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
        Need::Coder => m.lag.get("coding").is_some_and(|&l| l <= crate::view::TIERS[0].1),
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

    fn bench(edi: f64, slope: f64, floor: f64) -> Bench {
        Bench { edi, slope, floor, ceiling: 1.0 }
    }

    fn benches(pairs: &[(&str, Bench)]) -> HashMap<String, Bench> {
        pairs.iter().map(|(b, f)| (b.to_string(), *f)).collect()
    }

    #[test]
    fn ranks_by_task() {
        let a = scores(&[("APEX-Agents", 0.9), ("DeepSWE", 0.2)]);
        let b = scores(&[("APEX-Agents", 0.5), ("DeepSWE", 0.7)]);
        let e = benches(&[("APEX-Agents", bench(140.0, 0.1, 0.0)), ("DeepSWE", bench(140.0, 0.1, 0.0))]);
        let groups = [("a", Some(145.0), &a), ("b", Some(145.0), &b)];
        let (p, _) = percentiles(groups.iter().copied(), &e);
        assert!(p["a"]["agentic"] > p["b"]["agentic"]);
        assert!(p["b"]["coding"] > p["a"]["coding"]);

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
    fn one_score_moves_little() {
        let hard = bench(150.0, 0.2, 0.0);
        // 99% far above its ECI converts to 173 on its own; with the ECI it stays well below.
        let one = capability(&[(hard, 0.99)], 130.0, TASK_SD);
        assert!(one > 130.0 && one < 150.0, "{one}");
        // Many agreeing scores move it further than one.
        assert!(capability(&[(hard, 0.99); 8], 130.0, TASK_SD) > one + 5.0);
        // A 0 where 0 is expected says nothing new, instead of pinning the model at a clamp.
        let zero = capability(&[(hard, 0.0)], 130.0, TASK_SD);
        assert!((zero - 130.0).abs() < 1.0, "{zero}");
    }

    #[test]
    fn guessing_counts_as_zero() {
        let mc = capability(&[(bench(140.0, 0.1, 0.25), 0.25)], 140.0, TASK_SD);
        let open = capability(&[(bench(140.0, 0.1, 0.0), 0.0)], 140.0, TASK_SD);
        assert!((mc - open).abs() < 1e-9);
        // A benchmark with no fit is skipped.
        let s = scores(&[("DeepSWE", 0.5)]);
        let (p, _) = percentiles([("a", None, &s)].into_iter(), &HashMap::new());
        assert!(!p["a"].contains_key("coding"));
    }

    #[test]
    fn tasks_rank_the_same_models() {
        // a scores just as its ECI predicts; b and c have no coding scores but still count.
        let a = scores(&[("DeepSWE", 0.5)]);
        let none = scores(&[]);
        let e = benches(&[("DeepSWE", bench(170.0, 0.1, 0.0))]);
        let groups = [("a", Some(170.0), &a), ("b", Some(140.0), &none), ("c", Some(130.0), &none)];
        let (p, shown) = percentiles(groups.iter().copied(), &e);
        assert_eq!(p["a"]["coding"], p["a"]["overall"]);
        assert!(!p["b"].contains_key("coding"));
        assert!((shown["a"]["coding"] - 170.0).abs() < 0.01, "shown in ECI points: {}", shown["a"]["coding"]);
        assert!(!shown["b"].contains_key("coding"));
    }

    #[test]
    fn aa_ranks_by_task() {
        let a = scores(&[("artificial_analysis_coding_index", 0.6), ("hle", 0.4)]);
        let b = scores(&[("artificial_analysis_coding_index", 0.4), ("hle", 0.3)]);
        let pools = aa_pools([(Some(60.0), &a), (None, &b)].into_iter());
        let ((pa, shown_a), (pb, shown_b)) = (aa_fit(Some(60.0), &a, &pools), aa_fit(None, &b, &pools));
        assert!(pa["coding"] > pb["coding"]);
        assert!(pa["reasoning"] > pb["reasoning"]);
        assert_eq!((shown_a["coding"], shown_b["reasoning"]), (60.0, 30.0), "shown as the score, 0..100");
        assert!(pa.contains_key("overall") && !pb.contains_key("overall"), "no index, no overall");
        assert!(!pa.contains_key("agentic"), "no agentic field, no agentic score");
        // A setting of a model is ranked among the models, though it is none of them.
        let low = aa_fit(Some(10.0), &scores(&[("hle", 0.1)]), &pools).0;
        assert_eq!((low["overall"], low["reasoning"]), (0.0, 0.0), "below every model");
    }

    #[test]
    fn lag_is_in_months_of_the_best_score_s_progress() {
        let model = |eci: Option<f64>, release: &str| Model { eci, release: release.into(), ..Default::default() };
        let now = months("2026-10-01").unwrap() * 30.436_875 * 86400.0;
        // The best was 100 a year ago and is 124: 2 points a month.
        let mut ms = [
            model(Some(100.0), "2025-06-01"),
            model(Some(118.0), "2026-03"),
            model(Some(124.0), "2026-09-20"),
            model(None, "2026-09-20"),
        ];
        ms[1].shown.insert("coding".into(), 50.0);
        add_lag(&mut ms, now);
        let lag = |ms: &[Model], task: &str| ms.iter().map(|m| m.lag.get(task).copied()).collect::<Vec<_>>();
        assert_eq!(lag(&ms, "overall"), [Some(12.0), Some(3.0), Some(0.0), None]);
        assert_eq!(lag(&ms, "vision"), lag(&ms, "overall"), "ranked by the same index");
        assert_eq!(lag(&ms, "coding"), [None; 4], "one score, a year ago none: no pace to go by");
        assert_eq!(lag(&ms, "value"), [None; 4], "a rank, not a score");
        // Scored for under a year: no pace either, and the lags of the last data go.
        ms[0].release = "2026-01-01".into();
        add_lag(&mut ms, now);
        assert_eq!(lag(&ms, "overall"), [None; 4]);
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
        // Weak is too far behind the best on coding, and as far as `low` allows is not.
        models[0].lag.insert("coding".into(), 8.1);
        models[1].lag.insert("coding".into(), 8.0);
        let value = |m: &Model| fit(m, task("value").unwrap()).is_some();
        assert!(!value(&models[0]) && value(&models[1]));

        let mut models = [mk(70.0, 0.0), mk(60.0, 0.0), mk(90.0, 0.01)];
        add_value(&mut models);
        let v = |i: usize| models[i].fit["value"];
        assert!(v(0) > v(1) && v(1) > v(2), "free beats any price, the better coder first");
    }
}
