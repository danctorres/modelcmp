//! Task fit: which model is good for what.
//!
//! Overall is Epoch's ECI as Epoch publishes it, and vision is ranked by it. Coding and agentic
//! are one measure's score each, the same for every model (`Task::bench`): FrontierCode's and
//! Arena's agent score. A model without that score has none for the task.

use crate::data::Model;

pub enum Need {
    None,
    Tools,
    Vision,
}

pub struct Task {
    pub name: &'static str,
    /// When a model high on this task is the right pick.
    pub when: &'static str,
    /// What ranks it when that is not a score of its own, said in recommend as `ranked by`.
    pub ranked: Option<&'static str>,
    /// The one measure that scores the task, for every model the same, as `Model::picked` has it.
    /// Without it the task is ranked by Epoch's ECI.
    pub bench: Option<&'static str>,
    /// Epoch's benchmarks about the task, as `benchmark_metadata.csv` names them, which the
    /// column's dropdown lists after the task's score.
    pub benches: &'static [&'static str],
    pub need: Need,
}

/// A task is a capability software engineering needs, judged by `when`. Listed with the general
/// pick first, then the ones a tier picks for, coding, agentic and vision.
pub const TASKS: &[Task] = &[
    Task {
        name: "overall",
        when: "a tiebreaker, or work that fits no other task",
        ranked: None,
        need: Need::None,
        bench: None,
        benches: &[],
    },
    Task {
        name: "coding",
        when: "a well-specified bug or feature that must come out mergeable, the result matters more than the run",
        ranked: None,
        need: Need::None,
        bench: Some("FrontierCode"),
        benches: &[
            "CursorBench",
            "DeepSWE",
            "FrontierSWE",
            "MirrorCode",
            "GSO-Bench",
            "WeirdML v3",
            "Surface Evolver Bench",
        ],
    },
    Task {
        name: "agentic",
        when: "long unattended runs, migrations, loops until tests pass, where not going astray matters more than the best fix",
        ranked: None,
        need: Need::Tools,
        bench: Some(crate::data::ARENA_AGENT),
        benches: &["APEX-Agents", "Remote Labor Index", "OSWorld 2.0", "EBR-bench", "SpireClimb"],
    },
    Task {
        name: "vision",
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

/// The benchmarks the ECI column's dropdown lists: the general ones that score the most new
/// models and still tell the best apart.
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

/// What ranks task `t`, as `--json`'s `basis` and `show` say it: "ECI", its benchmark's name or
/// "Arena".
pub fn basis(t: &Task) -> &'static str {
    match t.bench {
        None => "ECI",
        Some(crate::data::ARENA_AGENT) => "Arena",
        Some(b) => b,
    }
}

/// Whether the model has what the task needs, tool calling or vision.
pub fn qualifies(m: &Model, t: &Task) -> bool {
    match t.need {
        Need::Tools => m.tool_call,
        Need::Vision => m.vision,
        Need::None => true,
    }
}

/// The task's score, Epoch's ECI or its `Task::bench` as `Model::picked` has it, if the model
/// qualifies for the task.
pub fn fit(m: &Model, t: &Task) -> Option<f64> {
    t.bench.map_or(m.eci, |b| m.picked(b)).filter(|_| qualifies(m, t))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_task_is_scored_by_its_one_measure() {
        let scores = [("ProofBench", 0.74), ("FrontierCode", 0.413), ("APEX-Agents", 0.582)];
        let mut m = Model { scores: scores.map(|(b, s)| (b.to_string(), s)).into(), ..Default::default() };
        let score = |m: &Model, t| fit(m, task(t).unwrap());
        // No ECI is no overall, whatever benchmarks the model has.
        assert_eq!((score(&m, "overall"), score(&m, "coding")), (None, Some(41.3)), "FrontierCode's, in percent");
        (m.eci, m.vision) = (Some(160.0), true);
        assert_eq!((score(&m, "overall"), score(&m, "vision")), (Some(160.0), Some(160.0)), "vision by overall");
        assert_eq!(score(&m, "agentic"), None, "Arena's, not APEX-Agents'");
        m.arena = Some(1.5);
        assert_eq!(score(&m, "agentic"), None, "agentic needs tool calling");
        m.tool_call = true;
        assert_eq!(score(&m, "agentic"), Some(1.5));
    }

    #[test]
    fn a_basis_is_what_ranks_the_task() {
        let said = ["overall", "coding", "agentic", "vision"].map(|t| basis(task(t).unwrap()));
        assert_eq!(said, ["ECI", "FrontierCode", "Arena", "ECI"]);
    }
}
