//! Text shared by the CLI and the TUI: number formatting, the detail page, the comparison grid.

use crate::data::{Data, Model, Offer, norm};
use crate::fit::{self, TASKS};
use crate::store::Store;
use std::ops::Range;

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

/// A task's price frontier, cheapest first: each entry costs more and scores higher, the last
/// being the best model for the task. Each price level keeps only its best entry, since models
/// that close in price are not worth choosing between. Models without a price or a score are
/// left out, as are those under the `low` tier's floor: the frontier is a recommendation, and
/// cheap alone is not one.
pub fn task_frontier<'a>(models: impl Iterator<Item = &'a Model>, t: &fit::Task) -> Vec<(&'a Model, f64)> {
    let ranked: Vec<_> = fit::rank(models, t).into_iter().filter(|(_, s)| s.round() >= TIERS[0].1).collect();
    let mut v = frontier(&ranked, |(m, _)| m, |m| fit::fit(m, t));
    // Dearest first so dedup keeps the best of each level, then back to cheapest first.
    v.sort_by(|a, b| b.0.cost().partial_cmp(&a.0.cost()).unwrap_or(std::cmp::Ordering::Equal));
    v.dedup_by_key(|(m, _)| level(m.cost().unwrap_or(0.0)));
    v.reverse();
    v
}

/// `--tier` names and their score floors. A tier picks the cheapest frontier entry at or
/// above its floor, scores compared as shown like the frontier does, or the best entry when
/// none reaches it; `high` always picks the best.
// ponytail: fixed floors on a percentile, tune them if the picks look off.
pub const TIERS: [(&str, f64); 3] = [("low", 50.0), ("mid", 75.0), ("high", f64::INFINITY)];

/// The entry of a cheapest-first frontier that `tier` picks; `None` for an empty frontier.
pub fn pick<'a, T>(front: &'a [(T, f64)], tier: &str) -> Option<&'a (T, f64)> {
    let floor = TIERS.iter().find(|t| t.0 == tier).map_or(f64::INFINITY, |t| t.1);
    front.iter().find(|(_, s)| s.round() >= floor).or(front.last())
}

/// `$1.5`, or `free`.
pub fn usd(x: f64) -> String {
    if x == 0.0 { "free".into() } else { format!("${}", money(x)) }
}

/// What a frontier line shows, for the recommend panel and `modelcmp recommend`, which adds the key.
pub fn frontier_legend(keyed: bool) -> String {
    format!(
        "best per price: name{}, $ blended 3:1 in:out per 1M tokens, (task percentile: rank among Epoch's models, \
         not a quality gap), cheapest first and the best last; only models in the top half",
        if keyed { " [key]" } else { "" }
    )
}

/// `name $price (score)` for a frontier entry, `name [key] $price (score)` with `keyed`.
pub fn priced(m: &Model, s: f64, keyed: bool) -> String {
    let key = if keyed { format!(" [{}]", m.key) } else { String::new() };
    format!("{}{key} {} ({s:.0})", m.name, usd(m.cost().unwrap_or(0.0)))
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

/// Where every word of the search `q` starts a word of one of `fields`, any case, punctuation
/// ignored: char ranges in each field, for highlighting. "opus 4.5", "anthropic opus" and
/// "gpt5" match a name and developer; "mini" misses Gemini. None when a word misses them all.
/// With `typos`, a word of 3+ letters that starts no word may be one slip off one ("opsu").
pub fn hits<const N: usize>(q: &str, fields: [&str; N], typos: bool) -> Option<[Vec<Range<usize>>; N]> {
    let mut out = [(); N].map(|_| vec![]);
    for t in q.split_whitespace().map(norm).filter(|t| !t.is_empty()) {
        let find = |typo| fields.iter().enumerate().find_map(|(i, s)| Some((i, word_hit(s, &t, typo)?)));
        let fuzzy = typos && t.len() >= 3 && t.bytes().all(|b| b.is_ascii_alphabetic());
        let (i, r) = find(false).or_else(|| fuzzy.then(|| find(true)).flatten())?;
        out[i].push(r);
    }
    Some(out)
}

/// The chars of `s` that normalized `t` covers where it starts a word of `s`: at the beginning,
/// after a space or punctuation, or where letters turn to digits or back ("Qwen3", "4o").
/// With `typo`, what it covers is instead one slip away from `t`, see `one_typo`.
fn word_hit(s: &str, t: &str, typo: bool) -> Option<Range<usize>> {
    // `s` normalized, the char index in `s` of each of its bytes, and where its words start.
    let (mut n, mut pos, mut starts, mut prev) = (String::new(), vec![], vec![], ' ');
    for (i, c) in s.chars().enumerate() {
        if c.is_ascii_alphanumeric() {
            let c = c.to_ascii_lowercase();
            if !prev.is_ascii_alphanumeric() || prev.is_ascii_digit() != c.is_ascii_digit() {
                starts.push(n.len());
            }
            n.push(c);
            pos.push(i);
        }
        prev = c;
    }
    let (at, len) = if typo {
        let (n, t) = (n.as_bytes(), t.as_bytes());
        starts.into_iter().find_map(|k| {
            [t.len(), t.len() + 1, t.len() - 1]
                .into_iter()
                .find(|&l| k + l <= n.len() && one_typo(&n[k..k + l], t))
                .map(|l| (k, l))
        })?
    } else {
        (starts.into_iter().find(|&k| n[k..].starts_with(t))?, t.len())
    };
    Some(pos[at]..pos[at + len - 1] + 1)
}

/// `a` is `b` with one letter wrong, missing, extra, or swapped with the next.
fn one_typo(a: &[u8], b: &[u8]) -> bool {
    let p = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    let (a, b) = (&a[p..], &b[p..]);
    match (a.len(), b.len()) {
        (0, 0) => false,
        (x, y) if x == y => a[1..] == b[1..] || (x >= 2 && a[..2] == [b[1], b[0]] && a[2..] == b[2..]),
        (x, y) if x == y + 1 => a[1..] == *b,
        (x, y) if x + 1 == y => *a == b[1..],
        _ => false,
    }
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
        format!(
            "{}{}{}",
            if store.is_fav(&m.key) { "★ " } else { "" },
            m.name,
            if store.is_excluded(&m.key) { " (excluded)" } else { "" }
        ),
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
        format!("  pages:      {}", m.links().into_iter().map(|(_, u)| u).collect::<Vec<_>>().join("  ")),
        format!("  note:       {}", store.note(&m.key).unwrap_or("-")),
        String::new(),
        format!("  ECI {}", score(m.eci)),
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
    v.push("  providers (model id, $ per 1M tokens in / out):".into());
    let mut offers: Vec<&Offer> = m.offers.iter().collect();
    // Available first, then cheapest; unknown price ("-") last.
    let cost = |o: &Offer| Some(o.input + o.output).filter(|c| *c > 0.0).unwrap_or(f64::MAX);
    offers.sort_by(|a, b| b.available.cmp(&a.available).then(cost(a).total_cmp(&cost(b))));
    for o in offers {
        v.push(format!(
            "    {} {:<20}{:<34}{:>8} {:>8}  {}",
            if o.available { "●" } else { " " },
            truncate(&o.provider_name, 19),
            truncate(&o.id, 33),
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
#[allow(clippy::single_range_in_vec_init)]
mod tests {
    use super::*;

    #[test]
    fn tiers_pick_the_cheapest_good_enough() {
        let front = [("free", 30.0), ("mini", 60.0), ("sonnet", 74.8), ("opus", 90.0)];
        let key = |t| pick(&front, t).map(|e| e.0);
        assert_eq!(key("low"), Some("mini"));
        assert_eq!(key("mid"), Some("sonnet"), "74.8 shows as 75");
        assert_eq!(key("high"), Some("opus"));
        assert_eq!(pick(&front[..2], "mid").map(|e| e.0), Some("mini"), "none reaches it: the best");
        assert_eq!(pick::<&str>(&[], "low"), None);
    }

    #[test]
    fn search_words_start_words_of_the_name_or_developer() {
        let matches = |q, f| hits(q, f, false).is_some();
        let (opus, pro, mini, qwen) = (
            ["Claude Opus 4.5", "anthropic"],
            ["Gemini 3.1 Pro", "google"],
            ["GPT-5 mini", "openai"],
            ["Qwen3-235B-A22B", "alibaba"],
        );
        for q in ["opus", "anthropic opus", "opus claude", "OPUS 4.5", "claudeopus45", ""] {
            assert!(matches(q, opus), "{q}");
        }
        assert!(matches("gemini pro", pro) && matches("gpt5", mini) && matches("qwen 3", qwen));
        assert!(matches("235b", qwen) && matches("mini", mini));
        assert!(!matches("mini", pro), "mini is not a word of Gemini");
        assert!(!matches("laude", opus) && !matches("opus sonnet", opus));
        assert_eq!(hits("4.5 anthropic", opus, false), Some([vec![12..15], vec![0..9]]), "the dot sits inside the hit");
        assert_eq!(hits("a22b", qwen, false), Some([vec![11..15], vec![]]));
    }

    #[test]
    fn typos_are_one_slip_in_a_word_of_letters() {
        let (opus, son) = (["Claude Opus 4.5", "anthropic"], ["Claude Sonnet 4.5", "anthropic"]);
        let typo = |q, f| hits(q, f, true);
        for q in ["opsu", "anthorpic opus", "claud sonet", "sonnnet", "snonet", "sonbet", "sonen"] {
            assert!(typo(q, son).is_some() || typo(q, opus).is_some(), "{q}");
        }
        assert_eq!(typo("sonet", son), Some([vec![7..13], vec![]]), "the whole word lights up");
        assert_eq!(typo("opsu", opus), Some([vec![7..11], vec![]]));
        assert!(hits("opsu", opus, false).is_none(), "only when asked");
        assert!(typo("4.6", opus).is_none() && typo("op", son).is_none(), "digits and short words stay exact");
        assert!(typo("sonnet", opus).is_none() && typo("osup", opus).is_none(), "one slip, not two");
    }

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
