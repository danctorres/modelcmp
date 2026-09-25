//! Non-interactive commands. Text for humans, `--json` for agents.

use crate::data::{Data, Model, Offer};
use crate::fit::{self, TASKS, Task};
use crate::store::Store;
use crate::view::{
    compare_rows, ctx, detail_lines, frontier, money, priced, score, task_frontier, task_score, truncate, verdict, via,
    visible,
};
use serde::Serialize;
use std::collections::BTreeMap;

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
    best_for: Vec<&'static str>,
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
        best_for: fit::best_for(m),
        benchmarks: full.then_some(&m.scores),
        providers: full.then(|| m.offers.iter().map(Price::from).collect()),
    }
}

fn print_json<T: Serialize>(v: &T) -> Result {
    println!("{}", serde_json::to_string_pretty(v).map_err(|e| e.to_string())?);
    Ok(())
}

fn table(models: &[&Model], store: &Store, show_avail: bool) {
    println!(
        "{:<2}{:<34} {:<9} {:>7} {:>7} {:>7} {:>6} {:>5} {:>6} {:>6} {:>6} {:>5}  {:<22} BEST FOR",
        "", "MODEL", "DEV", "PRICE/M", "$IN/M", "$OUT/M", "CTX", "ECI", "CODE", "CODE/$", "REASON", "MATH", "VIA"
    );
    for m in models {
        let price = |f: fn(&Offer) -> f64| m.price().map_or("-".into(), |p| money(f(p)));
        let mark = match (store.is_fav(&m.key), show_avail && m.available) {
            (true, _) => "★",
            (_, true) => "●",
            _ => " ",
        };
        println!(
            "{mark} {:<34} {:<9} {:>7} {:>7} {:>7} {:>6} {:>5} {:>6} {:>6} {:>6} {:>5}  {:<22} {}",
            truncate(&m.name, 34),
            truncate(&m.developer, 9),
            m.cost().map_or("-".into(), money),
            price(|p| p.input),
            price(|p| p.output),
            ctx(m.context),
            score(m.eci),
            score(task_score(m, "coding")),
            score(m.fit.get("value").copied()),
            score(task_score(m, "reasoning")),
            score(task_score(m, "math")),
            truncate(&via(&m.via), 22),
            fit::best_for(m).join(", ")
        );
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
    pub all: bool,
    pub favorites: bool,
    pub dev: Vec<String>,
    pub via: Vec<String>,
    pub limit: usize,
    pub max_price: Option<f64>,
    pub frontier: bool,
    pub json: bool,
}

pub fn list(data: &Data, store: &Store, o: &ListOpts) -> Result {
    let mut models: Vec<&Model> = visible(data, store, o.all, o.favorites).map(|(_, m)| m).collect();
    if !o.dev.is_empty() {
        models.retain(|m| o.dev.iter().any(|d| m.developer.eq_ignore_ascii_case(d)));
    }
    if !o.via.is_empty() {
        models.retain(|m| m.via.iter().any(|v| o.via.iter().any(|h| v.eq_ignore_ascii_case(h))));
    }
    if let Some(p) = o.max_price {
        models.retain(|m| m.blended().is_some_and(|b| b <= p));
    }
    let mut scores = None;
    if let Some(t) = o.task {
        let ranked = fit::rank(models.into_iter(), t);
        models = ranked.iter().map(|(m, _)| *m).collect();
        scores = Some(ranked.into_iter().map(|(_, s)| s).collect::<Vec<f64>>());
    }
    // Cheapest first, like `p`: each row down costs more and scores higher. Price ties are
    // score ties too on a frontier, and the stable sort keeps them in rank order.
    if let (true, Some(t)) = (o.frontier, o.task) {
        models = frontier(&models, |m| m, |m| fit::fit(m, t));
        models.sort_by(|a, b| a.cost().partial_cmp(&b.cost()).unwrap_or(std::cmp::Ordering::Equal));
        scores = None;
    }
    // Over every ranked model, before the limit cuts the list.
    let front: Vec<String> = match (o.task, o.frontier) {
        (Some(t), false) => task_frontier(models.iter().copied(), t).iter().map(|(m, s)| priced(m, *s)).collect(),
        _ => vec![],
    };
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
    if let (Some(t), Some(s)) = (o.task, scores) {
        let top: Vec<String> = models.iter().zip(s).take(3).map(|(m, s)| format!("{} ({s:.0})", m.name)).collect();
        println!("\nbest for {}: {}", t.name, top.join(", "));
        println!("best per price: {}", front.join(", "));
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

pub fn fav(data: &Data, store: &mut Store, add: bool, q: &str) -> Result {
    let m = resolve(data, q)?;
    if add != store.is_fav(&m.key) {
        store.toggle_fav(&m.key);
    }
    store.save()?;
    println!("{} {}", if add { "★ added" } else { "removed" }, m.name);
    Ok(())
}

pub fn note(data: &Data, store: &mut Store, q: &str, text: Option<&str>) -> Result {
    let m = resolve(data, q)?;
    match text {
        None => println!("{}", store.note(&m.key).unwrap_or("")),
        Some(t) => {
            store.set_note(&m.key, t);
            store.save()?;
        }
    }
    Ok(())
}

/// What each task measures, when to pick a model high on it, and its benchmarks.
pub fn tasks() {
    for t in TASKS {
        println!("{}  {}  (modelcmp recommend {})", t.name, t.about, t.name);
        println!("  use for:     {}", t.when);
        if !t.benches.is_empty() {
            println!("  benchmarks:  {}", t.benches.join(", "));
        }
        println!();
    }
}
