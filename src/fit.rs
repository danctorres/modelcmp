//! Task fit: which model is good for what.
//!
//! A task's score is on Epoch's ECI scale (about 100 to 175): the capability that Epoch's own
//! model of benchmark scores fits to the task's benchmarks the model has (`estimate`). Overall
//! is Epoch's ECI when it has one, else that estimate over every benchmark. Coding and agentic
//! are one measure's score instead, the same for every model (`Task::bench`): FrontierCode's and
//! Arena's agent score. A model with none of a task's benchmarks has no score for it.

use crate::data::Model;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

pub enum Need {
    None,
    Tools,
    Vision,
}

pub struct Task {
    pub name: &'static str,
    pub about: &'static str,
    /// When a model high on this task is the right pick.
    pub when: &'static str,
    /// What ranks it when that is not a score of its own, said in recommend as `ranked by`.
    pub ranked: Option<&'static str>,
    /// The one measure that scores the task, for every model the same, as `Model::picked` has it,
    /// where an estimate over `benches` would mix the ones each model has.
    pub bench: Option<&'static str>,
    /// Epoch's benchmarks about the task, as `benchmark_metadata.csv` names them: without `bench`
    /// the score is the estimate over the ones the model has. The column's dropdown lists them
    /// after the task's score.
    pub benches: &'static [&'static str],
    pub need: Need,
}

impl Task {
    /// What it is about, and what ranks it where that is not a score of its own.
    pub fn about_ranked(&self) -> String {
        match self.ranked {
            Some(r) => format!("{}, ranked by {r}", self.about),
            None => self.about.to_string(),
        }
    }
}

/// A task is a capability software engineering needs, judged by `when`. Listed with the general
/// pick first, then the ones a tier picks for, coding, agentic and vision.
pub const TASKS: &[Task] = &[
    Task {
        name: "overall",
        about: "general capability",
        when: "a tiebreaker, or work that fits no other task",
        ranked: None,
        need: Need::None,
        bench: None,
        benches: &[],
    },
    Task {
        name: "coding",
        about: "writing and fixing code",
        when: "fixing a bug, adding a feature to an existing repo, refactors",
        ranked: None,
        need: Need::None,
        bench: Some("FrontierCode"),
        benches: &[
            "CursorBench",
            "DeepSWE",
            "FrontierSWE",
            "MirrorCode",
            "SWE-Bench verified",
            "Aider polyglot",
            "Terminal Bench",
            "GSO-Bench",
            "WeirdML v3",
            "WeirdML",
            "Surface Evolver Bench",
        ],
    },
    Task {
        name: "agentic",
        about: "multi-step tool use, long autonomous tasks",
        when: "unattended multi-step runs, migrations, fix-until-tests-pass loops",
        ranked: None,
        need: Need::Tools,
        bench: Some(crate::data::ARENA_AGENT),
        benches: &[
            "APEX-Agents",
            "Remote Labor Index",
            "OSWorld 2.0",
            "OSWorld",
            "EBR-bench",
            "SpireClimb",
            "METR Time Horizons",
            "The Agent Company",
            "GDPval",
            "Terminal Bench",
        ],
    },
    Task {
        name: "vision",
        about: "image input",
        when: "screenshots, UI mockups, diagrams as input",
        ranked: Some("overall"),
        need: Need::Vision,
        bench: None,
        benches: &[],
    },
];

pub fn task(name: &str) -> Option<&'static Task> {
    TASKS.iter().find(|t| t.name == name)
}

/// The benchmarks the ECI column's dropdown lists, as overall's score is from every benchmark the
/// model has: the general ones that score the most new models and still tell the best apart.
pub const OVERALL_BENCHES: &[&str] =
    &["FrontierMath T4", "SimpleQA Verified", "Chess Puzzles", "Mystery Game Puzzles", "LMCA", "Furniture Assembly"];

/// Every benchmark of Epoch's a task or the ECI column's dropdown uses, sorted: not Arena's.
pub fn task_benches() -> Vec<&'static str> {
    let own = TASKS.iter().flat_map(|t| t.bench.iter().chain(t.benches).copied());
    let mut v: Vec<&str> = own.chain(OVERALL_BENCHES.iter().copied()).collect();
    v.retain(|b| *b != crate::data::ARENA_AGENT);
    v.sort();
    v.dedup();
    v
}

pub fn task_names() -> impl Iterator<Item = &'static str> {
    TASKS.iter().map(|t| t.name)
}

/// A benchmark in Epoch's model of scores (`edi_scores.csv`, `benchmark_metadata.csv`): its
/// difficulty on the ECI scale, its slope there, and the random baseline and the ceiling a score
/// is normalized between.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct Bench {
    pub edi: f64,
    pub slope: f64,
    pub baseline: f64,
    pub ceiling: f64,
}

impl Bench {
    /// A score, as Epoch scales it, as 0..1 between the random baseline and the ceiling.
    pub fn norm(&self, score: f64) -> f64 {
        ((score - self.baseline) / (self.ceiling - self.baseline)).clamp(0.0, 1.0)
    }
}

/// Where the estimate is searched: the ECI scale runs from about 55 to 185 today.
const RANGE: (f64, f64) = (60.0, 220.0);

/// The capability c that Epoch's model of scores, y ≈ sigmoid(slope × (c − edi)), fits to the
/// normalized scores `ys`: the root of Σ slope × (y − sigmoid(slope × (c − edi))), which falls
/// as c grows, by bisection on `RANGE`. Its bound when there is no root in it, every score at
/// the floor or the ceiling, so a score near 0 or 1 weighs little and never throws the estimate.
/// None with no score.
pub fn estimate(ys: &[(Bench, f64)]) -> Option<f64> {
    if ys.is_empty() {
        return None;
    }
    let sigmoid = |x: f64| 1.0 / (1.0 + (-x).exp());
    let f = |c: f64| ys.iter().map(|(b, y)| b.slope * (y - sigmoid(b.slope * (c - b.edi)))).sum::<f64>();
    let (mut lo, mut hi) = RANGE;
    if f(lo) <= 0.0 {
        return Some(lo);
    }
    if f(hi) >= 0.0 {
        return Some(hi);
    }
    for _ in 0..50 {
        let mid = (lo + hi) / 2.0;
        if f(mid) > 0.0 { lo = mid } else { hi = mid }
    }
    Some((lo + hi) / 2.0)
}

/// What a task's score is from: Epoch's ECI, the estimate over that many of the model's
/// benchmarks, or the task's one benchmark (`Task::bench`).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Basis {
    Eci,
    Benches(usize),
    Bench,
}

impl Basis {
    /// As `--json` and `show` say it for task `t`: "ECI", "3 benchmarks", its benchmark's name or
    /// "Arena".
    pub fn said(self, t: &Task) -> String {
        match (self, t.bench) {
            (Basis::Eci, _) => "ECI".into(),
            (Basis::Benches(1), _) => "1 benchmark".into(),
            (Basis::Benches(n), _) => format!("{n} benchmarks"),
            (Basis::Bench, Some(crate::data::ARENA_AGENT)) => "Arena".into(),
            (Basis::Bench, b) => b.unwrap_or_default().into(),
        }
    }

    /// Whether the score is `task`'s own: a measured ECI for overall and vision, else the task's
    /// benchmarks. An estimated overall is not, drawn dimmed and sorted after an own score it ties.
    pub fn own(self, task: &str) -> bool {
        match self {
            Basis::Eci | Basis::Bench => true,
            Basis::Benches(_) => !ECI_TASKS.contains(&task),
        }
    }
}

/// The tasks the overall score ranks.
const ECI_TASKS: [&str; 2] = ["overall", "vision"];

/// Task name -> its score, on the ECI scale or as `Task::bench` has it, and what it is from.
pub type Fit = BTreeMap<String, (f64, Basis)>;

/// The normalized scores that pin a capability down. Near a benchmark's floor or ceiling a point
/// moves it far, so an estimate from such scores alone can be off by tens of ECI points: one needs
/// a score in here, and then takes the others too, which Epoch's ECI is fit to as well.
const INFORMATIVE: std::ops::RangeInclusive<f64> = 0.1..=0.9;

/// Every task's score of a model, from its ECI and its `benchmark -> score` with the curves of
/// `benches`: overall Epoch's ECI, else the estimate over every benchmark it has; a task with
/// `Task::bench` that benchmark's score, in percent, when it is Epoch's; the others the estimate
/// over the task's benchmarks it has; vision the overall. No estimate without a score in
/// `INFORMATIVE`.
pub fn scored(eci: Option<f64>, scores: &BTreeMap<String, f64>, benches: &HashMap<String, Bench>) -> Fit {
    let over = |names: &[&str]| {
        let ys: Vec<(Bench, f64)> = scores
            .iter()
            .filter(|(b, _)| names.is_empty() || names.contains(&b.as_str()))
            .filter_map(|(b, s)| benches.get(b).map(|bench| (*bench, bench.norm(*s))))
            .collect();
        ys.iter().any(|(_, y)| INFORMATIVE.contains(y)).then_some(())?;
        Some((estimate(&ys)?, Basis::Benches(ys.len())))
    };
    let overall = eci.map(|e| (e, Basis::Eci)).or_else(|| over(&[]));
    let mut fit = Fit::new();
    for t in TASKS {
        let own = match t.bench {
            Some(b) => scores.get(b).map(|s| (s * 100.0, Basis::Bench)),
            None if t.benches.is_empty() => overall,
            None => over(t.benches),
        };
        if let Some(v) = own {
            fit.insert(t.name.to_string(), v);
        }
    }
    fit
}

/// Whether the model has what the task needs, tool calling or vision.
pub fn qualifies(m: &Model, t: &Task) -> bool {
    match t.need {
        Need::Tools => m.tool_call,
        Need::Vision => m.vision,
        Need::None => true,
    }
}

/// Task score, and what it is from, if the model qualifies for the task.
pub fn fit(m: &Model, t: &Task) -> Option<(f64, Basis)> {
    score(m, t.name).filter(|_| qualifies(m, t))
}

/// The score of task `name` and what it is from: `scored`'s, else its `Task::bench` as it is
/// now, as Arena's agent score comes after the refresh.
fn score(m: &Model, name: &str) -> Option<(f64, Basis)> {
    let now = || Some((m.picked(task(name)?.bench?)?, Basis::Bench));
    m.fit.get(name).copied().or_else(now)
}

/// Whether the model's score on task `name` is the task's own (`Basis::own`): a score to sort before
/// an estimate it ties.
pub fn own(m: &Model, name: &str) -> bool {
    score(m, name).is_some_and(|f| f.1.own(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scores(pairs: &[(&str, f64)]) -> BTreeMap<String, f64> {
        pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    /// Epoch's curves of 2026-10-30 for the benchmarks the models below were run on.
    fn benches() -> HashMap<String, Bench> {
        let b = |edi, slope, baseline, ceiling| Bench { edi, slope, baseline, ceiling };
        [
            ("GPQA diamond", b(134.72, 0.1321, 0.25, 0.97)),
            ("FrontierMath T1-3", b(150.19, 0.1676, 0.0, 0.95)),
            ("FrontierMath T4", b(158.34, 0.2483, 0.0, 1.0)),
            ("OTIS Mock AIME 2024-2025", b(137.18, 0.2237, 0.001, 1.0)),
            ("SimpleQA Verified", b(156.65, 0.0710, 0.0, 1.0)),
            ("EBR-bench", b(163.95, 0.1788, 0.0, 1.0)),
            ("Mystery Game Puzzles", b(163.06, 0.1663, 0.0922, 1.0)),
            ("APEX-Agents", b(156.28, 0.0821, 0.0, 1.0)),
            ("Furniture Assembly", b(165.03, 0.2843, 0.3, 1.0)),
            ("WeirdML v3", b(167.72, 0.1540, 0.0, 0.9)),
            ("Chess Puzzles", b(161.09, 0.1068, 0.0496, 1.0)),
            ("ARC-AGI", b(145.08, 0.2256, 0.0, 1.0)),
            ("Balrog", b(154.35, 0.0493, 0.0, 1.0)),
            ("SimpleBench", b(151.65, 0.0915, 0.1667, 1.0)),
            ("WeirdML", b(145.65, 0.0807, 0.0, 1.0)),
            ("ARC-AGI-2", b(153.65, 0.2696, 0.0, 1.0)),
            ("Surface Evolver Bench", b(150.24, 0.1320, 0.0, 1.0)),
            ("ProofBench", b(156.12, 0.2942, 0.0, 1.0)),
            ("FrontierCode", b(163.78, 0.0754, 0.0, 1.0)),
            ("LMCA", b(151.58, 0.0758, 0.0, 0.85)),
            ("DTBench", b(136.70, 0.0997, 0.4, 1.0)),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect()
    }

    /// Epoch's own ECI, from the scores it computed it from, within a point.
    #[test]
    fn the_estimate_recovers_epochs_eci() {
        let terra = scores(&[
            ("GPQA diamond", 0.943),
            ("FrontierMath T1-3", 0.909),
            ("FrontierMath T4", 0.927),
            ("OTIS Mock AIME 2024-2025", 1.0),
            ("SimpleQA Verified", 0.573),
            ("EBR-bench", 0.781),
            ("Mystery Game Puzzles", 0.74),
            ("APEX-Agents", 0.683),
            ("Furniture Assembly", 0.792),
            ("WeirdML v3", 0.404),
        ]);
        let fit = scored(None, &terra, &benches());
        let (overall, basis) = fit["overall"];
        assert!((overall - 168.05).abs() < 1.0, "GPT-6.1 Terra: {overall}");
        assert_eq!(basis, Basis::Benches(10));
        assert!(!fit.contains_key("coding"), "coding is FrontierCode's, not WeirdML v3's");
        assert!(!fit.contains_key("agentic"), "agentic is Arena's, not APEX-Agents' and EBR-bench's");
        assert_eq!(fit["vision"], fit["overall"], "ranked by overall");
        let terra56 = scores(&[
            ("GPQA diamond", 0.933),
            ("FrontierMath T1-3", 0.86),
            ("FrontierMath T4", 0.707),
            ("OTIS Mock AIME 2024-2025", 0.997),
            ("SimpleQA Verified", 0.432),
            ("Chess Puzzles", 0.47),
            ("Mystery Game Puzzles", 0.35),
            ("ARC-AGI", 0.965),
            ("Balrog", 0.532),
            ("SimpleBench", 0.489),
            ("WeirdML", 0.783),
            ("APEX-Agents", 0.582),
            ("ARC-AGI-2", 0.839),
            ("Surface Evolver Bench", 0.838),
            ("ProofBench", 0.74),
            ("FrontierCode", 0.413),
            ("LMCA", 0.549),
            ("DTBench", 0.933),
            ("Furniture Assembly", 0.542),
        ]);
        let overall = scored(None, &terra56, &benches())["overall"].0;
        assert!((overall - 160.25).abs() < 1.0, "GPT-5.6 Terra: {overall}");
        // With an ECI, that is the overall, and the tasks go on from their own benchmarks.
        let fit = scored(Some(160.0), &terra56, &benches());
        assert_eq!(fit["overall"], (160.0, Basis::Eci));
        assert_eq!(fit["coding"], (41.3, Basis::Bench), "FrontierCode's, in percent");
    }

    #[test]
    fn a_task_with_none_of_its_benchmarks_has_no_score() {
        let fit = scored(None, &scores(&[("ProofBench", 0.85)]), &benches());
        let (overall, basis) = fit["overall"];
        assert!((overall - 162.0).abs() < 1.0, "from ProofBench alone: {overall}");
        assert_eq!(basis, Basis::Benches(1));
        assert!(!fit.contains_key("coding") && !fit.contains_key("agentic"), "not the overall's");
        assert!(scored(None, &scores(&[]), &benches()).is_empty(), "nothing to go by");
        let eci = scored(Some(150.0), &scores(&[]), &benches());
        assert_eq!(eci.keys().collect::<Vec<_>>(), ["overall", "vision"]);
        // A benchmark Epoch's model has no curve for counts for nothing.
        assert!(scored(None, &scores(&[("SuperGLUE", 0.9)]), &benches()).is_empty());
        // Coding is FrontierCode's score alone, in percent. CursorBench, with no curve, is in no estimate.
        assert!(scored(None, &scores(&[("CursorBench", 0.5)]), &benches()).is_empty());
        let fit = scored(None, &scores(&[("FrontierCode", 0.5)]), &benches());
        assert_eq!(fit["coding"], (50.0, Basis::Bench));
    }

    /// A score near a benchmark's ceiling or floor says little: alone it is no estimate, beside
    /// one that says more it counts.
    #[test]
    fn an_estimate_needs_a_score_away_from_floor_and_ceiling() {
        for alone in [0.95, 0.05] {
            assert!(scored(None, &scores(&[("ProofBench", alone)]), &benches()).is_empty(), "{alone}");
        }
        let two = scored(None, &scores(&[("ProofBench", 0.95), ("FrontierCode", 0.5)]), &benches());
        assert_eq!(two["overall"].1, Basis::Benches(2));
    }

    #[test]
    fn scores_at_the_floor_or_the_ceiling_stay_in_range() {
        let b = Bench { edi: 150.0, slope: 0.2, baseline: 0.25, ceiling: 1.0 };
        assert_eq!(estimate(&[(b, 0.0)]), Some(RANGE.0));
        assert_eq!(estimate(&[(b, 1.0)]), Some(RANGE.1));
        assert_eq!(estimate(&[]), None);
        assert!((estimate(&[(b, 0.5)]).unwrap() - 150.0).abs() < 1e-6, "half way is the difficulty");
        assert_eq!((b.norm(0.25), b.norm(1.0), b.norm(0.1), b.norm(0.625)), (0.0, 1.0, 0.0, 0.5));
    }

    #[test]
    fn a_basis_is_own_by_task() {
        assert!(Basis::Eci.own("overall") && Basis::Eci.own("vision") && Basis::Bench.own("coding"));
        assert!(Basis::Benches(2).own("agentic") && !Basis::Benches(9).own("overall"), "an estimated overall");
        let coding = task("coding").unwrap();
        assert_eq!(Basis::Benches(1).said(coding), "1 benchmark");
        assert_eq!(Basis::Benches(3).said(coding), "3 benchmarks");
        assert_eq!((Basis::Eci.said(coding), Basis::Bench.said(coding)), ("ECI".into(), "FrontierCode".into()));
        let mk = |fit: &[(&str, (f64, Basis))], tools: bool| Model {
            fit: fit.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
            tool_call: tools,
            ..Default::default()
        };
        let a = mk(&[("agentic", (150.0, Basis::Benches(1)))], true);
        let b = mk(&[("agentic", (150.0, Basis::Benches(1))), ("overall", (150.0, Basis::Benches(2)))], false);
        assert_eq!(fit(&a, task("agentic").unwrap()), Some((150.0, Basis::Benches(1))));
        assert!(fit(&b, task("agentic").unwrap()).is_none(), "agentic needs tool calling");
        assert!(own(&a, "agentic") && !own(&b, "overall") && !own(&a, "coding"));
    }
}
