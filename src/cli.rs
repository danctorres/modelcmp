//! Non-interactive commands. Text for humans, `--json` for agents.

use crate::app::{COLS, Col, TEXT, hidden, model_id};
use crate::data::{Data, Model, Offer};
use crate::fit::{self, TASKS, Task};
use crate::store::{Store, slot};
use crate::view::{
    TIERS, compare_rows, detail_lines, frontier_legend, pick, priced, recommended, shown_via, task_frontier, truncate,
    verdict, visible,
};
use serde::Serialize;
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

/// An error with the process exit code it deserves: 3 for an ambiguous model name, 1 otherwise
/// (clap takes 2, for a usage error).
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
    /// The provider's model id: what a harness or API call takes
    id: &'a str,
    available: bool,
    /// Where you have access: harnesses (opencode, claude, ...) and "env" for an API key
    via: &'a [String],
    /// Null when the provider lists no price
    input_per_mtok: Option<f64>,
    /// Cached input; absent when the provider lists no discount, so input costs full price
    cache_read_per_mtok: Option<f64>,
    output_per_mtok: Option<f64>,
}

impl<'a> From<&'a Offer> for Price<'a> {
    fn from(o: &'a Offer) -> Self {
        Price {
            provider: &o.provider,
            id: &o.id,
            available: o.available,
            via: &o.via,
            input_per_mtok: (!o.unpriced).then_some(o.input),
            cache_read_per_mtok: o.cache_read,
            output_per_mtok: (!o.unpriced).then_some(o.output),
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
    selected: bool,
    /// You have it but cannot use it; --task and recommend skip it
    excluded: bool,
    note: Option<&'a str>,
    /// Tasks, or `task:tier`s, this model is your favorite for (`modelcmp fav`); `--tier` picks it for them
    favorite_for: Vec<String>,
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
    /// Site -> the model's page there: models.dev, epoch.ai, artificialanalysis.ai, openrouter.ai
    #[serde(skip_serializing_if = "Option::is_none")]
    pages: Option<BTreeMap<&'static str, String>>,
    /// The overall index of `source`: Epoch Capabilities Index, or Artificial Analysis Intelligence Index
    eci: Option<f64>,
    /// Where `eci`, `tasks` and `benchmarks` come from: "epoch" or "aa"
    source: &'static str,
    /// Task -> its score on the source's scale: ECI points (epoch) or the task's benchmark
    /// score, 0..100 (aa); overall and vision are `eci`, value a 0..100 percentile
    tasks: BTreeMap<&'static str, f64>,
    /// Output tokens per second, median across providers; Artificial Analysis only
    #[serde(skip_serializing_if = "Option::is_none")]
    tokens_per_second: Option<f64>,
    /// Seconds to the first token, median across providers; Artificial Analysis only
    #[serde(skip_serializing_if = "Option::is_none")]
    ttft_seconds: Option<f64>,
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
        selected: s.is_marked(&m.key),
        excluded: s.is_excluded(&m.key),
        note: s.note(&m.key),
        favorite_for: s.favorite_for(&m.key),
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
        pages: full.then(|| m.links().into_iter().collect()),
        eci: m.eci,
        source: crate::data::source().id(),
        tasks: TASKS
            .iter()
            .filter_map(|t| Some((t.name, (fit::shown(m, t, fit::fit(m, t)?) * 10.0).round() / 10.0)))
            .collect(),
        tokens_per_second: m.tps,
        ttft_seconds: m.ttft,
        benchmarks: full.then_some(&m.scores),
        providers: full.then(|| m.offers.iter().map(Price::from).collect()),
    }
}

fn print_json<T: Serialize>(v: &T) -> Result {
    println!("{}", serde_json::to_string_pretty(v).map_err(|e| e.to_string())?);
    Ok(())
}

/// The TUI's columns, so both show the same thing: Model, Dev, every numeric column the
/// source measures, Via.
fn table(models: &[&Model], store: &Store, any: bool) {
    let cols: Vec<&Col> = COLS.iter().enumerate().filter(|(i, _)| !hidden(i + TEXT)).map(|(_, c)| c).collect();
    let cells: Vec<Vec<String>> =
        models.iter().map(|m| cols.iter().map(|c| (c.get)(m).map_or("-".into(), c.show)).collect()).collect();
    let widths: Vec<usize> = (0..cols.len())
        .map(|i| cells.iter().map(|r| r[i].chars().count()).chain([cols[i].head().len()]).max().unwrap_or(0))
        .collect();
    let width = |f: fn(&Model) -> String, head: &str, max: usize| {
        models.iter().map(|m| f(m).chars().count()).chain([head.len()]).max().unwrap_or(0).min(max)
    };
    let (nw, dw) = (width(|m| m.name.clone(), "Model", 34), width(|m| m.developer.clone(), "Dev", 12));
    let line = |mark: &str, name: &str, dev: &str, nums: &mut dyn Iterator<Item = &str>, via: &str| {
        let nums: String = nums.zip(&widths).map(|(v, &w)| format!(" {v:>w$}")).collect();
        println!("{mark} {:<nw$} {:<dw$}{nums}  {via}", truncate(name, nw), truncate(dev, dw));
    };
    line("  ", "Model", "Dev", &mut cols.iter().map(|c| c.head()), "Via");
    for (m, row) in models.iter().zip(&cells) {
        let mark = if store.is_excluded(&m.key) {
            "✗ "
        } else if store.is_marked(&m.key) {
            "✓ "
        } else {
            "  "
        };
        let v = shown_via(m, any);
        let via = if v.is_empty() { "-".into() } else { v.join(", ") };
        line(mark, &m.name, &m.developer, &mut row.iter().map(String::as_str), &via);
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
            Exit { code: 3, msg: format!("'{q}' is ambiguous, candidates:\n{}", names.join("\n")) }
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
    pub selected: bool,
    pub dev: Vec<String>,
    pub via: Vec<String>,
    pub limit: usize,
    pub json: bool,
    /// Print only `provider/model` per line, for a shell substitution.
    pub id: bool,
}

pub fn list(data: &Data, store: &Store, o: &ListOpts) -> Result {
    check("--dev", &o.dev, data.models.iter().map(|m| m.developer.as_str()))?;
    // Via as the table shows it, so "not available" can be picked too.
    let any = data.any_available();
    check("--via", &o.via, data.models.iter().flat_map(|m| shown_via(m, any)))?;
    let has = |list: &[String], v: &str| list.iter().any(|x| x.eq_ignore_ascii_case(v));
    // A task's frontier is a recommendation, so models you cannot use stay out of it.
    let mut models: Vec<&Model> = visible(data, store, o.all, o.selected)
        .map(|(_, m)| m)
        .filter(|m| {
            (o.dev.is_empty() || has(&o.dev, &m.developer))
                && (o.via.is_empty() || shown_via(m, any).iter().any(|v| has(&o.via, v)))
                && o.bounds.iter().all(|&(c, lo, hi)| (COLS[c].get)(m).is_some_and(|v| v >= lo && v <= hi))
                && !(o.task.is_some() && store.is_excluded(&m.key))
        })
        .collect();
    if let Some(t) = o.task {
        let favs = store.task_favorites(t.name);
        let fav: Vec<&Model> = models.iter().copied().filter(|m| favs.contains(&m.key.as_str())).collect();
        // The tier picks on merit: a favorite appended to the line is neither best nor good enough.
        let merit = task_frontier(models.iter().copied(), t, &[]);
        let front = task_frontier(models.into_iter(), t, &fav);
        models = match &o.tier {
            // Your favorite for the tier, else for the task, beats the tier's pick; one the
            // filters hide gives way to the next.
            Some(tier) => store
                .tier_favorites(t.name, tier)
                .find_map(|k| front.iter().find(|(m, _)| m.key == k))
                .or_else(|| pick(&merit, tier))
                .map(|e| e.0)
                .into_iter()
                .collect(),
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
    if o.id {
        for m in &models {
            println!("{}", model_id(m));
        }
        return Ok(());
    }
    if models.is_empty() {
        println!("no models match");
        return Ok(());
    }
    table(&models, store, any);
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
    let mut section = "";
    for row in compare_rows(&models) {
        // A rule naming each topic above its first row, as in the TUI.
        if row.section != section {
            section = row.section;
            let name = format!("── {section} ");
            println!("{name}{}", "─".repeat((22 + 18 * models.len()).saturating_sub(name.chars().count())));
        }
        print!("{:<22}", row.label);
        for c in &row.cells {
            print!("{:>18}", truncate(c, 17));
        }
        println!();
    }
    Ok(())
}

pub fn open(data: &Data, q: &str, on: &str) -> Result {
    let m = resolve(data, q)?;
    let links = m.links();
    let Some((_, url)) = links.iter().find(|(site, _)| site.starts_with(on)) else {
        let sites: Vec<_> = links.iter().map(|(s, _)| *s).collect();
        return Err(Exit { code: 1, msg: format!("{} has no page on {on}; it has {}", m.name, sites.join(", ")) });
    };
    open::that_detached(url).map_err(|e| format!("could not open {url}: {e}"))?;
    println!("{url}");
    Ok(())
}

pub fn select(data: &Data, store: &mut Store, q: &str, rm: bool) -> Result {
    let m = resolve(data, q)?;
    if rm == store.is_marked(&m.key) {
        store.toggle_marked(&m.key);
    }
    store.save()?;
    println!("{} {}", if rm { "deselected" } else { "✓ selected" }, m.name);
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

/// Show the favorite model of every task, of one, or set or clear one.
pub fn fav(
    data: &Data,
    store: &mut Store,
    task: Option<&str>,
    tier: Option<&str>,
    q: Option<&str>,
    rm: bool,
) -> Result {
    let model = |key: &str| data.models.iter().find(|m| m.key == key);
    // A favorite the task cannot score still makes its line, unranked.
    let unscored =
        |s: &str, m: &Model| fit::task(s.split(':').next().unwrap_or(s)).is_some_and(|t| fit::fit(m, t).is_none());
    let task = task.map(|t| slot(t, tier));
    let task = task.as_deref();
    let line = |t: &str, k: &str| {
        let m = model(k);
        let skipped = if m.is_some_and(|m| unscored(t, m)) { "  (no score)" } else { "" };
        format!("★ {} [{k}]{skipped}", m.map_or(k, |m| m.name.as_str()))
    };
    match (task, q, rm) {
        (None, ..) => {
            if store.favorite.is_empty() {
                println!("no favorites; modelcmp fav <task> <model> sets one");
            }
            for (t, k) in &store.favorite {
                println!("{t:<18}{}", line(t, k));
            }
        }
        (Some(t), None, false) => match store.favorite(t) {
            Some(k) => println!("{}", line(t, k)),
            None => println!("no favorite for {t}"),
        },
        (Some(t), None, true) => {
            store.favorite.remove(t);
            store.save()?;
            println!("cleared {t}");
        }
        (Some(t), Some(q), _) => {
            let m = resolve(data, q)?;
            store.favorite.insert(t.to_string(), m.key.clone());
            store.save()?;
            println!("★ {t}: {}", m.name);
        }
    }
    Ok(())
}

pub fn note(data: &Data, store: &mut Store, q: &str, text: Option<&str>, rm: bool) -> Result {
    let m = resolve(data, q)?;
    match (text, rm) {
        (None, false) => match store.note(&m.key) {
            Some(n) => println!("{n}"),
            None => println!("no note for {}", m.name),
        },
        (t, _) => {
            store.set_note(&m.key, t.unwrap_or(""));
            store.save()?;
        }
    }
    Ok(())
}

/// The frontier among the models you have and can use, as `list --task` gives it.
fn front<'a>(data: &'a Data, store: &'a Store, t: &Task) -> Vec<(&'a Model, f64)> {
    let models: Vec<&Model> =
        visible(data, store, false, false).map(|(_, m)| m).filter(|m| !store.is_excluded(&m.key)).collect();
    let favs = store.task_favorites(t.name);
    let fav: Vec<&Model> = models.iter().copied().filter(|m| favs.contains(&m.key.as_str())).collect();
    task_frontier(models.into_iter(), t, &fav)
}

/// The task's favorites that are on the line only for being favorites.
fn unrecommended<'a>(data: &Data, store: &'a Store, t: &Task) -> Vec<&'a str> {
    let models: Vec<&Model> =
        visible(data, store, false, false).map(|(_, m)| m).filter(|m| !store.is_excluded(&m.key)).collect();
    store.task_favorites(t.name).into_iter().filter(|k| !recommended(models.iter().copied(), t, k)).collect()
}

/// `recommend --json`: what agents read to choose a task and its model.
fn recommend_json(data: &Data, store: &Store) -> Vec<serde_json::Value> {
    TASKS
        .iter()
        .map(|t| {
            let off = unrecommended(data, store, t);
            let front: Vec<_> = front(data, store, t)
                .into_iter()
                .map(|(m, s)| {
                    let mut e = serde_json::json!({"key": m.key, "name": m.name, "context": m.context, "price": m.cost().map(|c| (c * 1000.0).round() / 1000.0), "score": (fit::shown(m, t, s) * 10.0).round() / 10.0, "recommended": !off.contains(&m.key.as_str())});
                    if let Some(n) = store.note(&m.key) {
                        e["note"] = n.into();
                    }
                    e
                })
                .collect();
            // An excluded favorite is off the line, so agents are not pointed at it either.
            let fav = |s: &str| store.favorite(s).filter(|k| !store.is_excluded(k));
            let tier_favs: BTreeMap<&str, &str> =
                TIERS.iter().filter_map(|x| Some((x.0, fav(&slot(t.name, Some(x.0)))?))).collect();
            serde_json::json!({"name": t.name, "about": t.about, "when": t.when, "benchmarks": t.benches, "favorite": fav(t.name), "tier_favorites": tier_favs, "frontier": front})
        })
        .collect()
}

/// The best model per price for each task, what it measures and when to use it.
pub fn recommend(data: &Data, store: &Store, json: bool) -> Result {
    if json {
        return print_json(&recommend_json(data, store));
    }
    println!("{}\n", frontier_legend(true));
    for t in TASKS {
        println!("{}  {}  (modelcmp list --task {})", t.name, t.about, t.name);
        println!("  use for:         {}", t.when);
        let off = unrecommended(data, store, t);
        let front: Vec<String> = front(data, store, t)
            .iter()
            .map(|(m, s)| {
                let p = priced(m, fit::shown(m, t, *s), true, store.is_favorite(Some(t), &m.key));
                if off.contains(&m.key.as_str()) { format!("{p} not recommended") } else { p }
            })
            .collect();
        println!("  best per price:  {}", if front.is_empty() { "no data".into() } else { front.join(" · ") });
        println!();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn model(key: &str, coding: f64, price: f64) -> Model {
        let mut m = Model {
            key: key.into(),
            name: key.into(),
            offers: vec![Offer {
                provider: "p".into(),
                id: key.into(),
                input: price,
                output: price,
                ..Default::default()
            }],
            ..Default::default()
        };
        m.fit.insert("coding".into(), coding);
        m
    }

    /// `list --selected` leaves out a selected model out of reach, as an agent cannot call it.
    #[test]
    fn the_selected_list_leaves_out_models_out_of_reach() {
        let mut data =
            Data { models: vec![model("gpt55", 90.0, 10.0), model("llama4", 60.0, 1.0)], ..Default::default() };
        data.models[0].available = true;
        let mut store = Store::default();
        store.toggle_marked("gpt55");
        store.toggle_marked("llama4");
        let keys = |marked| visible(&data, &store, false, marked).map(|(_, m)| m.key.as_str()).collect::<Vec<_>>();
        assert_eq!(keys(true), ["gpt55"]);
        // With --all, Via reads "not available" for it, and filters by that too.
        let o = ListOpts {
            task: None,
            tier: None,
            sort: None,
            bounds: vec![],
            all: true,
            selected: false,
            dev: vec![],
            via: vec!["Not Available".into()],
            limit: 0,
            json: false,
            id: true,
        };
        assert!(list(&data, &store, &o).is_ok());
    }

    fn keys(v: &Value) -> Vec<&str> {
        v.as_object().unwrap().keys().map(String::as_str).collect()
    }

    /// The JSON is the agents' API: a renamed or dropped field breaks their scripts.
    #[test]
    fn model_json_keeps_its_fields() {
        let store = Store::default();
        let mut m = model("gpt55", 80.0, 2.0);
        let short = serde_json::to_value(out(&m, &store, false)).unwrap();
        let fields = [
            "available",
            "context",
            "developer",
            "eci",
            "excluded",
            "favorite_for",
            "key",
            "knowledge",
            "max_output",
            "name",
            "note",
            "open_weights",
            "price",
            "reasoning",
            "release",
            "selected",
            "source",
            "tasks",
            "tool_call",
            "url",
            "via",
            "vision",
        ];
        assert_eq!(keys(&short), fields);
        assert_eq!(
            keys(&short["price"]),
            ["available", "cache_read_per_mtok", "id", "input_per_mtok", "output_per_mtok", "provider", "via"]
        );
        let full = serde_json::to_value(out(&m, &store, true)).unwrap();
        let extra: Vec<_> = keys(&full).into_iter().filter(|k| !fields.contains(k)).collect();
        assert_eq!(extra, ["benchmarks", "pages", "providers"], "only show and compare add these");
        m.offers[0].unpriced = true;
        let unpriced = serde_json::to_value(out(&m, &store, false)).unwrap();
        assert_eq!(unpriced["price"]["input_per_mtok"], Value::Null, "an unknown price is not free");
    }

    #[test]
    fn recommend_json_leaves_excluded_models_out() {
        let data = Data { models: vec![model("gpt55", 90.0, 10.0), model("mini", 60.0, 1.0)], ..Default::default() };
        let mut store = Store::default();
        let coding = |v: &[Value]| v.iter().find(|t| t["name"] == "coding").unwrap().clone();
        let names = |t: &Value| t["frontier"].as_array().unwrap().iter().map(|e| e["key"].clone()).collect::<Vec<_>>();
        let t = coding(&recommend_json(&data, &store));
        assert_eq!(keys(&t), ["about", "benchmarks", "favorite", "frontier", "name", "tier_favorites", "when"]);
        assert_eq!(keys(&t["frontier"][0]), ["context", "key", "name", "price", "recommended", "score"]);
        assert_eq!(names(&t), ["mini", "gpt55"]);
        store.set_note("gpt55", "slow");
        assert_eq!(
            coding(&recommend_json(&data, &store))["frontier"][1]["note"],
            "slow",
            "a note only when there is one"
        );
        store.toggle_favorite("coding:low", "mini");
        assert_eq!(coding(&recommend_json(&data, &store))["tier_favorites"], serde_json::json!({"low": "mini"}));
        store.toggle_excluded("mini");
        let t = coding(&recommend_json(&data, &store));
        assert_eq!(names(&t), ["gpt55"]);
        assert_eq!(t["tier_favorites"], serde_json::json!({}), "nor as a favorite");
    }
}
