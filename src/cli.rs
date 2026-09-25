//! Non-interactive commands. Text for humans, `--json` for agents.

use crate::app::COLS;
use crate::data::{Data, Model, Offer};
use crate::fit::{self, TASKS, Task};
use crate::store::Store;
use crate::view::{compare_rows, detail_lines, pick, priced, task_frontier, truncate, verdict, via, visible};
use serde::Serialize;
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

/// An error with the process exit code it deserves: 2 for an ambiguous model name, 1 otherwise.
pub struct Exit {
    pub code: i32,
    pub msg: String,
}

impl From<String> for Exit {
    fn from(msg: String) -> Self {
        Exit { code: 1, msg }
    }
}

impl From<std::io::Error> for Exit {
    fn from(e: std::io::Error) -> Self {
        Exit { code: 1, msg: e.to_string() }
    }
}

type Result<T = ()> = std::result::Result<T, Exit>;

#[derive(Serialize)]
struct Price<'a> {
    provider: &'a str,
    available: bool,
    /// Where you have access: harnesses (opencode, claude, ...) and "env" for an API key
    via: &'a [String],
    input_per_mtok: f64,
    output_per_mtok: f64,
}

impl<'a> From<&'a Offer> for Price<'a> {
    fn from(o: &'a Offer) -> Self {
        Price {
            provider: &o.provider,
            available: o.available,
            via: &o.via,
            input_per_mtok: o.input,
            output_per_mtok: o.output,
        }
    }
}

#[derive(Serialize)]
struct ModelOut<'a> {
    key: &'a str,
    name: &'a str,
    developer: &'a str,
    available: bool,
    via: &'a [String],
    favorite: bool,
    /// You have it but cannot use it; --task and tasks skip it
    excluded: bool,
    note: Option<&'a str>,
    price: Option<Price<'a>>,
    context: u64,
    max_output: u64,
    tool_call: bool,
    reasoning: bool,
    vision: bool,
    open_weights: bool,
    release: &'a str,
    knowledge: &'a str,
    url: &'a str,
    /// Epoch Capabilities Index
    eci: Option<f64>,
    /// Task -> 0..100 percentile among Epoch-evaluated models
    tasks: BTreeMap<&'static str, f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    benchmarks: Option<&'a BTreeMap<String, f64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    providers: Option<Vec<Price<'a>>>,
}

fn out<'a>(m: &'a Model, s: &'a Store, full: bool) -> ModelOut<'a> {
    ModelOut {
        key: &m.key,
        name: &m.name,
        developer: &m.developer,
        available: m.available,
        via: &m.via,
        favorite: s.is_fav(&m.key),
        excluded: s.is_excluded(&m.key),
        note: s.note(&m.key),
        price: m.price().map(Price::from),
        context: m.context,
        max_output: m.max_output,
        tool_call: m.tool_call,
        reasoning: m.reasoning,
        vision: m.vision,
        open_weights: m.open_weights,
        release: &m.release,
        knowledge: &m.knowledge,
        url: &m.url,
        eci: m.eci,
        tasks: TASKS.iter().filter_map(|t| Some((t.name, (fit::fit(m, t)? * 10.0).round() / 10.0))).collect(),
        benchmarks: full.then_some(&m.scores),
        providers: full.then(|| m.offers.iter().map(Price::from).collect()),
    }
}

fn print_json<T: Serialize>(v: &T) -> Result {
    println!("{}", serde_json::to_string_pretty(v).map_err(|e| e.to_string())?);
    Ok(())
}

/// The TUI's columns, so both show the same thing: Model, Dev, every numeric column, Via.
fn table(models: &[&Model], store: &Store, show_avail: bool) {
    let cells: Vec<Vec<String>> =
        models.iter().map(|m| COLS.iter().map(|c| (c.get)(m).map_or("-".into(), c.show)).collect()).collect();
    let widths: Vec<usize> = (0..COLS.len())
        .map(|i| cells.iter().map(|r| r[i].chars().count()).chain([COLS[i].name.len()]).max().unwrap_or(0))
        .collect();
    let width = |f: fn(&Model) -> String, head: &str, max: usize| {
        models.iter().map(|m| f(m).chars().count()).chain([head.len()]).max().unwrap_or(0).min(max)
    };
    let (nw, dw) = (width(|m| m.name.clone(), "Model", 34), width(|m| m.developer.clone(), "Dev", 12));
    let line = |mark: &str, name: &str, dev: &str, nums: &mut dyn Iterator<Item = &str>, via: &str| {
        let nums: String = nums.zip(&widths).map(|(v, &w)| format!(" {v:>w$}")).collect();
        println!("{mark} {:<nw$} {:<dw$}{nums}  {via}", truncate(name, nw), truncate(dev, dw));
    };
    line(" ", "Model", "Dev", &mut COLS.iter().map(|c| c.name), "Via");
    for (m, row) in models.iter().zip(&cells) {
        let mark = match (store.is_fav(&m.key), show_avail && m.available) {
            _ if store.is_excluded(&m.key) => "✗",
            (true, _) => "★",
            (_, true) => "●",
            _ => " ",
        };
        line(mark, &m.name, &m.developer, &mut row.iter().map(String::as_str), &via(&m.via));
    }
}

/// Every name in `asked` must be one of `known`, any case, or the filter would silently match nothing.
fn check<'a>(flag: &str, asked: &[String], known: impl Iterator<Item = &'a str>) -> Result {
    let known: BTreeSet<String> = known.filter(|k| !k.is_empty()).map(str::to_lowercase).collect();
    match asked.iter().find(|a| !known.contains(&a.to_lowercase())) {
        Some(bad) => Err(format!("no model has {flag} '{bad}'; known: {}", Vec::from_iter(known).join(", ")).into()),
        None => Ok(()),
    }
}

fn resolve<'a>(data: &'a Data, q: &str) -> Result<&'a Model> {
    data.find(q).map_err(|c| match c.as_slice() {
        [] => Exit { code: 1, msg: format!("no model matches '{q}'") },
        c => {
            let names: Vec<String> = c.iter().take(15).map(|m| format!("  {} ({})", m.name, m.key)).collect();
            Exit { code: 2, msg: format!("'{q}' is ambiguous, candidates:\n{}", names.join("\n")) }
        }
    })
}

pub struct ListOpts {
    pub task: Option<&'static Task>,
    /// `low`, `mid` or `high`: one model from the task's frontier.
    pub tier: Option<String>,
    /// Index into `COLS`.
    pub sort: Option<usize>,
    /// (index into `COLS`, min, max)
    pub bounds: Vec<(usize, f64, f64)>,
    pub all: bool,
    pub favorites: bool,
    pub dev: Vec<String>,
    pub via: Vec<String>,
    pub limit: usize,
    pub json: bool,
}

pub fn list(data: &Data, store: &Store, o: &ListOpts) -> Result {
    check("--dev", &o.dev, data.models.iter().map(|m| m.developer.as_str()))?;
    check("--via", &o.via, data.models.iter().flat_map(|m| &m.via).map(String::as_str))?;
    let has = |list: &[String], v: &str| list.iter().any(|x| x.eq_ignore_ascii_case(v));
    // A task's frontier is a recommendation, so models you cannot use stay out of it.
    let mut models: Vec<&Model> = visible(data, store, o.all, o.favorites)
        .map(|(_, m)| m)
        .filter(|m| {
            (o.dev.is_empty() || has(&o.dev, &m.developer))
                && (o.via.is_empty() || m.via.iter().any(|v| has(&o.via, v)))
                && o.bounds.iter().all(|&(c, lo, hi)| (COLS[c].get)(m).is_some_and(|v| v >= lo && v <= hi))
                && !(o.task.is_some() && store.is_excluded(&m.key))
        })
        .collect();
    if let Some(t) = o.task {
        let front = task_frontier(models.into_iter(), t);
        models = match &o.tier {
            Some(tier) => pick(&front, tier).map(|e| e.0).into_iter().collect(),
            None => front.into_iter().map(|(m, _)| m).collect(),
        };
    } else if let Some(c) = o.sort.map(|c| &COLS[c]) {
        // Best first, blanks last, names breaking ties.
        models.sort_by(|a, b| {
            let order = match ((c.get)(a), (c.get)(b)) {
                (Some(x), Some(y)) if c.lower_better => x.total_cmp(&y),
                (Some(x), Some(y)) => y.total_cmp(&x),
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (None, None) => Ordering::Equal,
            };
            order.then_with(|| a.name.cmp(&b.name))
        });
    }
    let total = models.len();
    if o.limit > 0 {
        models.truncate(o.limit);
    }
    if o.json {
        return print_json(&models.iter().map(|m| out(m, store, false)).collect::<Vec<_>>());
    }
    if models.is_empty() {
        println!("no models match");
        return Ok(());
    }
    table(&models, store, o.all);
    if models.len() < total {
        println!("\n{} of {total} shown; -n 0 for all", models.len());
    }
    Ok(())
}

pub fn show(data: &Data, store: &Store, q: &str, json: bool) -> Result {
    let m = resolve(data, q)?;
    if json {
        return print_json(&out(m, store, true));
    }
    for line in detail_lines(m, store) {
        println!("{line}");
    }
    Ok(())
}

pub fn compare(data: &Data, store: &Store, qs: &[String], json: bool) -> Result {
    let models = qs.iter().map(|q| resolve(data, q)).collect::<Result<Vec<_>>>()?;
    if json {
        return print_json(&models.iter().map(|m| out(m, store, true)).collect::<Vec<_>>());
    }
    let rows = verdict(&models);
    let w = |i: usize| rows.iter().map(|r| r[i].chars().count()).max().unwrap_or(0);
    let (w0, w1) = (w(0), w(1));
    println!("verdict:");
    for [q, win, margin] in rows {
        println!("  {q:<w0$}  {win:<w1$}  {margin}");
    }
    println!();
    for row in compare_rows(&models) {
        print!("{:<22}", row.label);
        for c in &row.cells {
            print!("{:>18}", truncate(c, 17));
        }
        println!();
    }
    Ok(())
}

pub fn open(data: &Data, q: &str) -> Result {
    let m = resolve(data, q)?;
    open::that_detached(&m.url).map_err(|e| format!("could not open {}: {e}", m.url))?;
    println!("{}", m.url);
    Ok(())
}

pub fn fav(data: &Data, store: &mut Store, q: &str, rm: bool) -> Result {
    let m = resolve(data, q)?;
    if rm == store.is_fav(&m.key) {
        store.toggle_fav(&m.key);
    }
    store.save()?;
    println!("{} {}", if rm { "removed" } else { "★ added" }, m.name);
    Ok(())
}

pub fn exclude(data: &Data, store: &mut Store, q: &str, rm: bool) -> Result {
    let m = resolve(data, q)?;
    if rm == store.is_excluded(&m.key) {
        store.toggle_excluded(&m.key);
    }
    store.save()?;
    println!("{} {}", if rm { "included" } else { "✗ excluded" }, m.name);
    Ok(())
}

pub fn note(data: &Data, store: &mut Store, q: &str, text: Option<&str>, rm: bool) -> Result {
    let m = resolve(data, q)?;
    match (text, rm) {
        (None, false) => println!("{}", store.note(&m.key).unwrap_or("")),
        (t, _) => {
            store.set_note(&m.key, t.unwrap_or(""));
            store.save()?;
        }
    }
    Ok(())
}

/// What each task measures, when to pick a model high on it, and its benchmarks.
pub fn tasks(data: &Data, store: &Store, json: bool) -> Result {
    // The frontier among the models you have and can use, as `list --task` gives it.
    let front =
        |t| task_frontier(visible(data, store, false, false).map(|(_, m)| m).filter(|m| !store.is_excluded(&m.key)), t);
    if json {
        let v: Vec<_> = TASKS
            .iter()
            .map(|t| {
                let front: Vec<_> = front(t)
                    .into_iter()
                    .map(|(m, s)| serde_json::json!({"key": m.key, "name": m.name, "price": m.cost().map(|c| (c * 1000.0).round() / 1000.0), "score": (s * 10.0).round() / 10.0}))
                    .collect();
                serde_json::json!({"name": t.name, "about": t.about, "when": t.when, "benchmarks": t.benches, "frontier": front})
            })
            .collect();
        return print_json(&v);
    }
    for t in TASKS {
        println!("{}  {}  (modelcmp list --task {})", t.name, t.about, t.name);
        println!("  use for:     {}", t.when);
        let front: Vec<String> = front(t).iter().map(|(m, s)| priced(m, *s)).collect();
        println!("  best/price:  {}", if front.is_empty() { "no data".into() } else { front.join(" · ") });
        if !t.benches.is_empty() {
            println!("  benchmarks:  {}", t.benches.join(", "));
        }
        println!();
    }
    Ok(())
}
