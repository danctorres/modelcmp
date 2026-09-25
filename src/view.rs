//! Text shared by the CLI and the TUI: number formatting, the detail page, the comparison grid.

use crate::data::{Data, Model, Offer};
use crate::fit::{self, TASKS};
use crate::store::Store;

pub fn money(x: f64) -> String {
    match x {
        0.0 => "free".into(),
        x if x < 1.0 => format!("{x:.2}"),
        x if x < 10.0 => format!("{x:.1}"),
        x => format!("{x:.0}"),
    }
}

/// Upper edges of the price levels in blended $/1M: free, then up to 0.5, 2, 5 and 15, then above.
/// They fall between model families: Flash-Lite, Flash and Haiku, Sonnet and Pro, Opus, Fable.
pub const LEVELS: [f64; 5] = [0.0, 0.5, 2.0, 5.0, 15.0];

/// Price level of a blended price, 0 (free) to 5 (above the last edge).
pub fn level(x: f64) -> usize {
    LEVELS.iter().take_while(|&&e| x > e).count()
}

/// Name of price level `l`: `free`, `≤$0.5` ... `≤$15`, `>$15`.
pub fn level_label(l: usize) -> String {
    match LEVELS.get(l) {
        Some(0.0) => "free".into(),
        Some(e) => format!("≤${e}"),
        None => format!(">${}", LEVELS[LEVELS.len() - 1]),
    }
}

pub fn ctx(n: u64) -> String {
    match n {
        0 => "-".into(),
        n if n >= 1_000_000 => format!("{}M", (n as f64 / 1e6 * 10.0).round() / 10.0),
        n => format!("{}k", n / 1000),
    }
}

pub fn score(x: Option<f64>) -> String {
    x.map_or("-".into(), |v| format!("{v:.0}"))
}

pub fn task_score(m: &Model, task: &str) -> Option<f64> {
    fit::fit(m, fit::task(task)?)
}

/// The items whose model no other beats on both `score` and blended price: the cheapest at
/// every quality level. Scores compare as shown, whole points, so a fraction of a point never
/// justifies a higher price. Free models count at $0; ties on both keep every tied item.
pub fn frontier<'m, T: Copy>(
    items: &[T],
    model: impl Fn(T) -> &'m Model,
    score: impl Fn(&Model) -> Option<f64>,
) -> Vec<T> {
    let pts: Vec<Option<(f64, f64)>> =
        items.iter().map(|&i| Some((score(model(i))?.round(), model(i).cost()?))).collect();
    let beats = |(s, c): (f64, f64), (t, d): (f64, f64)| s >= t && c <= d && (s > t || c < d);
    // ponytail: O(n²), fine for the few hundred rows the harnesses list.
    items
        .iter()
        .zip(&pts)
        .filter(|(_, a)| a.is_some_and(|a| !pts.iter().any(|b| b.is_some_and(|b| beats(b, a)))))
        .map(|(&i, _)| i)
        .collect()
}

pub fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.into() } else { s.chars().take(n - 1).chain(['…']).collect() }
}

/// Where a model or offer is available, "-" if nowhere: `claude, opencode, env`.
pub fn via(v: &[String]) -> String {
    if v.is_empty() { "-".into() } else { v.join(", ") }
}

fn or_dash(s: &str) -> &str {
    if s.is_empty() { "-" } else { s }
}

/// Models the user should see: the ones they have access to, unless `all` or none is available.
pub fn visible<'a>(
    data: &'a Data,
    store: &'a Store,
    all: bool,
    favs_only: bool,
) -> impl Iterator<Item = (usize, &'a Model)> {
    let any = data.models.iter().any(|m| m.available);
    data.models
        .iter()
        .enumerate()
        .filter(move |(_, m)| (all || !any || m.available) && (!favs_only || store.is_fav(&m.key)))
}

/// Everything about one model, one line per entry.
pub fn detail_lines(m: &Model, store: &Store) -> Vec<String> {
    let yes = |b: bool| if b { "yes" } else { "no" };
    let mut v = vec![
        format!("{}{}", if store.is_fav(&m.key) { "★ " } else { "" }, m.name),
        format!("  developer:  {}", or_dash(&m.developer)),
        format!("  via:        {}", via(&m.via)),
        format!("  context:    {} (max output {})", ctx(m.context), ctx(m.max_output)),
        format!(
            "  features:   tools {} · reasoning {} · vision {} · open weights {}",
            yes(m.tool_call),
            yes(m.reasoning),
            yes(m.vision),
            yes(m.open_weights)
        ),
        format!("  released:   {}   knowledge: {}", or_dash(&m.release), or_dash(&m.knowledge)),
        format!("  url:        {}", m.url),
        format!("  note:       {}", store.note(&m.key).unwrap_or("-")),
        String::new(),
        format!("  ECI {}   best for: {}", score(m.eci), or_dash(&fit::best_for(m).join(", "))),
        "  task fit (percentile vs all evaluated models):".into(),
    ];
    for t in TASKS {
        if let Some(s) = fit::fit(m, t) {
            v.push(format!("    {:<13}{:>4.0}  {}", t.name, s, t.about));
        }
    }
    if !m.scores.is_empty() {
        v.push(String::new());
        v.push("  benchmarks (Epoch AI, best effort setting):".into());
        for (b, s) in &m.scores {
            v.push(format!("    {:<36}{:>5.1}%", b, s * 100.0));
        }
    }
    v.push(String::new());
    v.push("  providers ($ per 1M tokens in / out):".into());
    let mut offers: Vec<&Offer> = m.offers.iter().collect();
    // Available first, then cheapest; unknown price ("-") last.
    let cost = |o: &Offer| Some(o.input + o.output).filter(|c| *c > 0.0).unwrap_or(f64::MAX);
    offers.sort_by(|a, b| b.available.cmp(&a.available).then(cost(a).total_cmp(&cost(b))));
    for o in offers {
        v.push(format!(
            "    {} {:<28}{:>8} {:>8}  {}",
            if o.available { "●" } else { " " },
            truncate(&o.provider_name, 28),
            money(o.input),
            money(o.output),
            o.via.join(", ")
        ));
    }
    v
}

pub struct Row {
    pub label: String,
    pub cells: Vec<String>,
    /// Index of the best cell, if the row is comparable.
    pub best: Option<usize>,
}

/// Rows for side-by-side comparison.
pub fn compare_rows(models: &[&Model]) -> Vec<Row> {
    fn row(label: &str, vals: Vec<Option<f64>>, fmt: impl Fn(f64) -> String, higher: bool) -> Row {
        let best = vals
            .iter()
            .enumerate()
            .filter_map(|(i, v)| Some((i, (*v)?)))
            .max_by(|a, b| if higher { a.1.total_cmp(&b.1) } else { b.1.total_cmp(&a.1) })
            .map(|(i, _)| i)
            .filter(|_| vals.iter().flatten().count() > 1);
        Row { label: label.into(), cells: vals.into_iter().map(|v| v.map_or("-".into(), &fmt)).collect(), best }
    }
    let price = |f: fn(&Offer) -> f64| -> Vec<Option<f64>> {
        models.iter().map(|m| m.price().map(f).filter(|p| *p > 0.0)).collect()
    };
    let mut rows = vec![
        Row { label: "model".into(), cells: models.iter().map(|m| m.name.clone()).collect(), best: None },
        Row { label: "via".into(), cells: models.iter().map(|m| via(&m.via)).collect(), best: None },
        row("$ in / 1M", price(|o| o.input), money, false),
        row("$ out / 1M", price(|o| o.output), money, false),
        row("context", models.iter().map(|m| Some(m.context as f64)).collect(), |c| ctx(c as u64), true),
        row("ECI", models.iter().map(|m| m.eci).collect(), |v| format!("{v:.1}"), true),
    ];
    for t in TASKS.iter().filter(|t| t.name != "overall") {
        let vals: Vec<Option<f64>> = models.iter().map(|m| fit::fit(m, t)).collect();
        if vals.iter().any(Option::is_some) {
            rows.push(row(t.name, vals, |v| format!("{v:.0}"), true));
        }
    }
    let mut benches: Vec<&String> = models.iter().flat_map(|m| m.scores.keys()).collect();
    benches.sort();
    benches.dedup();
    for b in benches {
        let vals = models.iter().map(|m| m.scores.get(b).copied()).collect();
        rows.push(row(b, vals, |v| format!("{:.1}%", v * 100.0), true));
    }
    rows
}

/// The conclusion above a comparison: for each question, which model wins and by how much
/// against the runner-up, as `[question, winner, margin]`. Values compare as shown, so two
/// that print the same tie, and the winner cell names every tied model. With fewer than two
/// models to compare, the winner is `-`.
pub fn verdict(models: &[&Model]) -> Vec<[String; 3]> {
    let usd = |x: f64| if x == 0.0 { "free".into() } else { format!("${}", money(x)) };
    let coding = |m: &Model| task_score(m, "coding");
    vec![
        best(models, "cheaper", Model::cost, false, usd),
        best(models, "better at coding", coding, true, |v| format!("{v:.0}")),
        best(models, "coding per $", |m| Some(coding(m)? / m.blended()?), true, |v| format!("{v:.1}")),
    ]
}

fn best(
    models: &[&Model],
    question: &str,
    val: impl Fn(&Model) -> Option<f64>,
    higher: bool,
    fmt: impl Fn(f64) -> String,
) -> [String; 3] {
    let mut vals: Vec<(&str, f64)> = models.iter().filter_map(|m| Some((m.name.as_str(), val(m)?))).collect();
    vals.sort_by(|a, b| if higher { b.1.total_cmp(&a.1) } else { a.1.total_cmp(&b.1) });
    if vals.len() < 2 {
        return [question.into(), "-".into(), "not enough data".into()];
    }
    let top = fmt(vals[0].1);
    let tied: Vec<&str> = vals.iter().take_while(|v| fmt(v.1) == top).map(|v| v.0).collect();
    let next = vals.get(tied.len()).map(|v| fmt(v.1));
    let (winner, margin) = match (tied.len(), next) {
        (1, Some(n)) => (tied[0].into(), format!("{top} vs {n}")),
        (_, Some(n)) => (format!("tie: {}", tied.join(", ")), format!("{top} each, next {n}")),
        (_, None) => (format!("tie: {}", tied.join(", ")), format!("{top} each")),
    };
    [question.into(), winner, margin]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {
        assert_eq!(money(0.0), "free");
        assert_eq!(
            [0.0, 0.3, 0.5, 2.0, 10.0, 20.0].map(level),
            [0, 1, 1, 2, 4, 5],
            "an edge belongs to the level below"
        );
        assert_eq!(money(0.153), "0.15");
        assert_eq!(money(2.5), "2.5");
        assert_eq!(money(15.0), "15");
        assert_eq!(ctx(0), "-");
        assert_eq!(ctx(128_000), "128k");
        assert_eq!(ctx(1_048_576), "1M");
        assert_eq!(truncate("abcdef", 4), "abc…");
        assert_eq!(truncate("abc", 4), "abc");
    }

    #[test]
    fn compare_marks_best() {
        let mk = |n: &str, c: u64, eci: Option<f64>| Model { name: n.into(), context: c, eci, ..Default::default() };
        let (a, b) = (mk("a", 100, Some(150.0)), mk("b", 200, None));
        let rows = compare_rows(&[&a, &b]);
        let find = |l: &str| rows.iter().find(|r| r.label == l).unwrap();
        assert_eq!(find("context").best, Some(1));
        assert_eq!(find("ECI").best, None, "a single value is not a comparison");
        assert_eq!(find("ECI").cells, vec!["150.0", "-"]);
    }

    #[test]
    fn verdict_picks_a_winner_per_question() {
        let offer = |p: f64| Offer { input: p, output: p, ..Default::default() };
        let mk = |n: &str, coding: Option<f64>, p: f64| Model {
            name: n.into(),
            fit: coding.map(|c| ("coding".to_string(), c)).into_iter().collect(),
            offers: vec![offer(p)],
            ..Default::default()
        };
        let (big, small, new) = (mk("big", Some(90.0), 10.0), mk("small", Some(60.0), 2.0), mk("new", None, 1.0));
        let v = verdict(&[&big, &small, &new]);
        let s = |r: [&str; 3]| r.map(String::from);
        assert_eq!(v[0], s(["cheaper", "new", "$1.0 vs $2.0"]));
        assert_eq!(v[1], s(["better at coding", "big", "90 vs 60"]), "a model without the score is skipped");
        assert_eq!(v[2], s(["coding per $", "small", "30.0 vs 9.0"]));
        assert_eq!(verdict(&[&big, &new])[1], s(["better at coding", "-", "not enough data"]));
        assert_eq!(verdict(&[&small, &small])[0], s(["cheaper", "tie: small, small", "$2.0 each"]));
        let twin = mk("twin", Some(60.0), 2.0);
        assert_eq!(verdict(&[&big, &small, &twin])[0], s(["cheaper", "tie: small, twin", "$2.0 each, next $10"]));
    }
}
