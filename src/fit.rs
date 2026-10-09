//! Task fit: which model is good for what.
//!
//! A task's score is one benchmark's, a widely run one of the source about the task, or a
//! mean of two of them (`AA_CODING`). It is ranked as a percentile (0..100) among the models
//! scored on it, and shown as that score, in percent.

use crate::data::Model;
use std::collections::{BTreeMap, HashMap};

/// Months over which the source's best score is taken to have risen steadily (`add_lag`).
const WINDOW: f64 = 12.0;

pub enum Need {
    None,
    Tools,
    Vision,
    /// No further behind your best on coding than a task's line allows (`view::FLOOR`): cheap alone is not
    /// enough. A cut of the task's line (`view::task_frontier`), not of the score: none makes
    /// the line while coding has no pace to tell by (`add_lag`).
    Coder,
}

pub struct Task {
    pub name: &'static str,
    pub about: &'static str,
    /// When a model high on this task is the right pick.
    pub when: &'static str,
    /// Epoch's benchmarks about the task, still run on new models: the first is the task's
    /// score, shown as is, and the column's dropdown lists the others.
    pub benches: &'static [&'static str],
    /// The same for Artificial Analysis: one of its API's `evaluations` fields, shown as is,
    /// or `AA_CODING`.
    pub aa: Option<&'static str>,
    /// Its other fields about the task, still run on new models: the column's dropdown lists
    /// them after `aa`.
    pub aa_more: &'static [&'static str],
    pub need: Need,
}

impl Task {
    /// With `src`: the benchmark the task's score is, and the others about the task.
    pub fn of(&self, src: crate::data::Source) -> (Option<&'static str>, &'static [&'static str]) {
        match (src, self.benches.split_first()) {
            (crate::data::Source::Aa, _) => (self.aa, self.aa_more),
            (_, Some((own, more))) => (Some(*own), more),
            (_, None) => (None, &[]),
        }
    }

    /// `of` the source in use.
    pub fn sourced(&self) -> (Option<&'static str>, &'static [&'static str]) {
        self.of(crate::data::source())
    }

    /// The other source's benchmarks about the task, with its data at hand too (`Data::borrow`):
    /// the column's dropdown lists them after `sourced`'s.
    pub fn lent(&self) -> Vec<&'static str> {
        match crate::data::source() {
            _ if !crate::data::lent() => vec![],
            crate::data::Source::Aa => self.benches.to_vec(),
            _ => self.aa.into_iter().chain(self.aa_more.iter().copied()).collect(),
        }
    }
}

/// The source that runs the benchmark `b` of a task.
pub fn bench_source(b: &str) -> crate::data::Source {
    let aa = TASKS.iter().any(|t| t.aa == Some(b) || t.aa_more.contains(&b));
    if aa { crate::data::Source::Aa } else { crate::data::Source::Epoch }
}

/// Artificial Analysis's overall index, its ECI.
pub const AA_INDEX: &str = "artificial_analysis_intelligence_index";
/// The mean of Terminal-Bench 4.0 and SciCode, the coding part of that index: no field of the
/// API, whose Coding Index is no longer given to new models (2026-09).
pub const AA_CODING: &str = "terminalbench_scicode_mean";

/// A task is a capability software engineering needs, judged by `when`; its benchmarks need
/// not be about code. Math and factual-recall benchmarks stay out, and those Epoch no longer
/// runs on new models: SWE-Bench verified, Terminal Bench, GSO-Bench, DeepResearch Bench (2026-10).
/// "overall" = Epoch Capabilities Index. "value" = coding per dollar (computed after prices are known).
/// Listed with the general pick first, then the ones a tier picks for, coding, agentic, reasoning and vision, and last
/// the cheaper pick, which has one model and not one per tier.
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
        aa: Some(AA_CODING),
        aa_more: &["terminalbench_v4_0", "scicode"],
        benches: &["WeirdML", "FrontierCode", "DeepSWE", "FrontierSWE", "MirrorCode"],
    },
    Task {
        name: "agentic",
        about: "multi-step tool use, long autonomous tasks",
        when: "unattended multi-step runs, migrations, fix-until-tests-pass loops",
        need: Need::Tools,
        aa: Some("terminalbench_v4_0"),
        aa_more: &[],
        benches: &["APEX-Agents", "Remote Labor Index", "OSWorld 2.0"],
    },
    Task {
        name: "reasoning",
        about: "hard science questions, puzzles, abstraction",
        when: "subtle bugs, algorithm and architecture design, contradictory specs, tricky invariants",
        need: Need::None,
        aa: Some("hle"),
        aa_more: &["lcr"],
        benches: &[
            "LMCA",
            "GPQA diamond",
            "HLE",
            "ARC-AGI-2",
            "ARC-AGI",
            "SimpleBench",
            "Mystery Game Puzzles",
            "Chess Puzzles",
            "DTBench",
        ],
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
    Task {
        name: "value",
        about: "coding per dollar, among the models close to your best on coding",
        when: "routine coding that needs no top reasoning",
        need: Need::Coder,
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
/// source's overall index or the task's benchmark score, 0..100; "value" alone stays a percentile.
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

/// What a source's models scored, each `(index, benchmark -> score)`: the index, and every
/// benchmark a task uses. One model's scores are ranked among them (`scored`).
#[derive(Default, Debug)]
pub struct Pools {
    index: Vec<f64>,
    benches: HashMap<&'static str, Vec<f64>>,
}

pub fn pools<'a>(groups: impl Iterator<Item = (Option<f64>, &'a BTreeMap<String, f64>)>) -> Pools {
    let groups: Vec<_> = groups.collect();
    Pools {
        index: groups.iter().filter_map(|g| g.0).collect(),
        benches: task_benches()
            .into_iter()
            .chain(aa_fields())
            .map(|b| (b, groups.iter().filter_map(|g| g.1.get(b).copied()).collect()))
            .collect(),
    }
}

/// A model's fit with `src`, from its `index` and `benchmark -> score`: each task's percentile
/// among the models scored on its benchmark, and that score as 0..100; overall and vision the
/// index's.
pub fn scored(
    src: crate::data::Source,
    index: Option<f64>,
    scores: &BTreeMap<String, f64>,
    pools: &Pools,
) -> (BTreeMap<String, f64>, BTreeMap<String, f64>) {
    let (mut fit, mut show) = (BTreeMap::new(), BTreeMap::new());
    if let Some(i) = index {
        for t in ECI_TASKS {
            fit.insert(t.to_string(), pct_rank(i, &pools.index));
        }
    }
    for t in TASKS {
        if let Some(b) = t.of(src).0
            && let (Some(&x), Some(pool)) = (scores.get(b), pools.benches.get(b))
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
pub fn months(date: &str) -> Option<f64> {
    let mut parts = date.split('-').map(|p| p.parse::<f64>().ok());
    let (year, month, day) = (parts.next()??, parts.next()??, parts.next().flatten().unwrap_or(15.0));
    Some((year - 1970.0) * 12.0 + (month - 1.0) + (day - 1.0) / 30.44)
}

/// Each task's `Model::lag`: how far the model is behind the source's best on it, in months of
/// progress, a month being a twelfth of what the best score rose over the last `WINDOW`
/// months. `now` is in seconds since 1970. One scale for every source and task, which a share
/// of the best score is not (an index has no zero) and a gap in points is not either (each
/// benchmark moves at its own pace); a percentile among every model ever scored counts a
/// model at half the best score as near the top. A task scored for less than `WINDOW` months,
/// or whose best has not risen in them, has no lag, and its line leaves none out for it. A
/// best of then that scored nothing still gives a pace: the floor hides part of the rise, so
/// the lags come out too long, the strict side. "value" has none, being a rank.
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
        let (best, old) = (best(false), best(true));
        let rate = (best - old) / WINDOW;
        for m in models.iter_mut() {
            match score(m).filter(|_| rate > 0.0 && rate.is_finite()) {
                Some(s) => m.lag.insert(t.name.to_string(), (best - s) / rate),
                None => m.lag.remove(t.name),
            };
        }
    }
}

/// "value": coding percentile per blended dollar, itself ranked as a percentile, for every
/// model with both. The task's line only takes the models close to your best on coding (see `Need::Coder`).
pub fn add_value(models: &mut [Model]) {
    // Not one only your machine runs: the score is not that copy's, and free it would top the rank.
    let value = |m: &Model| Some(m.fit.get("coding").filter(|_| !m.local())? / m.cost()?.max(FREE));
    let raw: Vec<Option<f64>> = models.iter().map(value).collect();
    let all: Vec<f64> = raw.iter().flatten().copied().collect();
    for (m, v) in models.iter_mut().zip(raw) {
        match v {
            Some(v) => m.fit.insert("value".into(), pct_rank(v, &all)),
            None => m.fit.remove("value"),
        };
    }
}

/// Task score if the model qualifies for the task.
pub fn fit(m: &Model, t: &Task) -> Option<f64> {
    let ok = match t.need {
        Need::Tools => m.tool_call,
        Need::Vision => m.vision,
        Need::None | Need::Coder => true,
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
        use crate::data::Source::{Aa, Epoch};
        let a = scores(&[("APEX-Agents", 0.9), ("WeirdML", 0.2), ("DeepSWE", 0.9)]);
        let b = scores(&[("APEX-Agents", 0.5), ("WeirdML", 0.7)]);
        let p = pools([(Some(145.0), &a), (None, &b)].into_iter());
        let ((pa, shown_a), (pb, _)) = (scored(Epoch, Some(145.0), &a, &p), scored(Epoch, None, &b, &p));
        assert!(pa["agentic"] > pb["agentic"]);
        assert!(pb["coding"] > pa["coding"], "DeepSWE is not the task's score");
        assert_eq!(shown_a["coding"], 20.0, "shown as the score, 0..100");
        assert!(!pa.contains_key("reasoning"), "no LMCA, no reasoning score");

        let mk = |k: &str, fit: &BTreeMap<String, f64>, tools: bool| Model {
            key: k.into(),
            fit: fit.clone(),
            tool_call: tools,
            ..Default::default()
        };
        let models = [mk("a", &pa, true), mk("b", &pb, false)];
        let r = rank(models.iter(), task("coding").unwrap());
        assert_eq!(r[0].0.key, "b");
        // agentic needs tool calling; b lacks it
        assert!(fit(&models[1], task("agentic").unwrap()).is_none());

        let a = scores(&[(AA_CODING, 0.6), ("hle", 0.4)]);
        let b = scores(&[(AA_CODING, 0.4), ("hle", 0.3)]);
        let p = pools([(Some(60.0), &a), (None, &b)].into_iter());
        let ((pa, shown_a), (pb, shown_b)) = (scored(Aa, Some(60.0), &a, &p), scored(Aa, None, &b, &p));
        assert!(pa["coding"] > pb["coding"]);
        assert!(pa["reasoning"] > pb["reasoning"]);
        assert_eq!((shown_a["coding"], shown_b["reasoning"]), (60.0, 30.0));
        assert!(pa.contains_key("overall") && !pb.contains_key("overall"), "no index, no overall");
        assert!(!pa.contains_key("agentic"), "no agentic field, no agentic score");
        // A setting of a model is ranked among the models, though it is none of them.
        let low = scored(Aa, Some(10.0), &scores(&[("hle", 0.1)]), &p).0;
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
        ms[1].shown.insert("coding".into(), 48.0);
        add_lag(&mut ms, now);
        let lag = |ms: &[Model], task: &str| ms.iter().map(|m| m.lag.get(task).copied()).collect::<Vec<_>>();
        assert_eq!(lag(&ms, "overall"), [Some(12.0), Some(3.0), Some(0.0), None]);
        assert_eq!(lag(&ms, "vision"), lag(&ms, "overall"), "ranked by the same index");
        assert_eq!(lag(&ms, "coding"), [None; 4], "one score, a year ago none: no pace to go by");
        assert_eq!(lag(&ms, "value"), [None; 4], "a rank, not a score");
        // A year ago the best scored nothing: a pace still, of 4 points a month.
        ms[0].shown.insert("coding".into(), 0.0);
        add_lag(&mut ms, now);
        assert_eq!(lag(&ms, "coding"), [Some(12.0), Some(0.0), None, None]);
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
        assert!(models[1].fit["value"] > models[2].fit["value"]);

        let mut models = [mk(70.0, 0.0), mk(60.0, 0.0), mk(90.0, 0.01)];
        add_value(&mut models);
        let v = |i: usize| models[i].fit["value"];
        assert!(v(0) > v(1) && v(1) > v(2), "free beats any price, the better coder first");
    }
}
