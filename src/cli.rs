//! Non-interactive commands. Text for humans, `--json` for agents.

use crate::app::{COLS, Col, TEXT, hidden, launch_cmd, model_id, on_price, shown};
use crate::data::{Data, Model, Offer, vias};
use crate::fit::{self, TASKS, Task};
use crate::store::{Store, slot};
use crate::view::{
    CUSTOM_ABOUT, CUSTOM_WHEN, FRONTIER_LEGEND, TIERS, by_value, compare_rows, custom_line, custom_priced,
    detail_lines, priced, shown_via, task_line, tier_pick, truncate, used_benches, verdict, via, visible,
};
use serde::Serialize;
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
    /// True when this provider lists no price and the prices are the list ones of other providers
    #[serde(skip_serializing_if = "Option::is_none")]
    listed: Option<bool>,
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
            listed: None,
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
    /// The model's page, the one `open` opens: the first site of `pages` to have one, null when none has
    url: Option<String>,
    /// Site -> the model's page there, for each of models.dev, epoch.ai, artificialanalysis.ai
    /// and openrouter.ai that has one
    #[serde(skip_serializing_if = "Option::is_none")]
    pages: Option<BTreeMap<&'static str, String>>,
    /// The overall index of `source`: Epoch Capabilities Index, or Artificial Analysis Intelligence Index
    eci: Option<f64>,
    /// Where `eci`, `tasks` and `benchmarks` come from: "epoch" or "aa"
    source: &'static str,
    /// Task -> its score on the source's scale: ECI points (epoch) or the task's benchmark
    /// score, 0..100 (aa); overall and vision are `eci`, value a 0..100 percentile
    tasks: BTreeMap<&'static str, f64>,
    /// Output tokens per second, median; Artificial Analysis only
    #[serde(skip_serializing_if = "Option::is_none")]
    tokens_per_second: Option<f64>,
    /// Seconds to the first answer token, after any thinking, median; Artificial Analysis only
    #[serde(skip_serializing_if = "Option::is_none")]
    ttft_seconds: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    benchmarks: Option<&'a BTreeMap<String, f64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    providers: Option<Vec<Price<'a>>>,
}

fn out<'a>(m: &'a Model, s: &'a Store, full: bool) -> ModelOut<'a> {
    let links = m.links();
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
        price: m.price().map(|o| {
            // Yours lists no price: the id to call stays yours, the prices are the list ones.
            let p = Price::from(m.priced_offer().unwrap_or(o));
            Price {
                listed: Some(m.listed()),
                provider: &o.provider,
                id: &o.id,
                available: o.available,
                via: &o.via,
                ..p
            }
        }),
        context: m.context,
        max_output: m.max_output,
        tool_call: m.tool_call,
        reasoning: m.reasoning,
        vision: m.vision,
        open_weights: m.open_weights,
        release: &m.release,
        knowledge: &m.knowledge,
        url: links.first().map(|(_, url)| url.clone()),
        pages: full.then(|| links.into_iter().collect()),
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
    let cols: Vec<(usize, &Col)> = COLS.iter().enumerate().filter(|(i, _)| !hidden(i + TEXT)).collect();
    let cell =
        |m: &Model, &(i, c): &(usize, &Col)| (c.get)(m).map_or("-".into(), |v| shown(i, v, on_price(i) && m.listed()));
    let cells: Vec<Vec<String>> = models.iter().map(|m| cols.iter().map(|c| cell(m, c)).collect()).collect();
    let cols: Vec<&Col> = cols.into_iter().map(|(_, c)| c).collect();
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
        line(mark, &m.name, &m.developer, &mut row.iter().map(String::as_str), &via(m, any));
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
    /// A task of your own instead of a built-in one: its model.
    pub custom: Option<String>,
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
    /// Print only the command that starts a harness on the model per line, for a shell substitution.
    pub cmd: bool,
    /// Print only `provider/model` per line, for a shell substitution.
    pub id: bool,
}

pub fn list(data: &Data, store: &Store, o: &ListOpts) -> Result {
    check("--dev", &o.dev, data.models.iter().map(|m| m.developer.as_str()))?;
    // Via as the table shows it, so "not available" can be picked too.
    let any = data.any_available();
    check("--via", &o.via, data.models.iter().flat_map(|m| shown_via(m, any)))?;
    let has = |list: &[String], v: &str| list.iter().any(|x| x.eq_ignore_ascii_case(v));
    // A usage error, as when clap knew every task's name.
    if let Some(c) = o.custom.as_deref().filter(|c| !store.custom_tasks().contains(c)) {
        let tasks = [fit::task_names().collect(), store.custom_tasks()].concat().join(", ");
        let msg = format!("no task '{c}': there are {tasks}; modelcmp fav {c} <model> makes it one of your own");
        return Err(Exit { code: 2, msg });
    }
    let task = o.task.is_some() || o.custom.is_some();
    // A task's frontier is a recommendation, so models you cannot use stay out of it.
    let mut models: Vec<&Model> = visible(data, store, o.all, o.selected)
        .map(|(_, m)| m)
        .filter(|m| {
            (o.dev.is_empty() || has(&o.dev, &m.developer))
                && (o.via.is_empty() || shown_via(m, any).iter().any(|v| has(&o.via, v)))
                && o.bounds.iter().all(|&(c, lo, hi)| (COLS[c].get)(m).is_some_and(|v| v >= lo && v <= hi))
                && !(task && store.is_excluded(&m.key))
        })
        .collect();
    // Your favorites stay on a task's line whatever the filters, as in the recommend panel.
    let pool = || visible(data, store, o.all, false).map(|(_, m)| m).filter(|m| !store.is_excluded(&m.key));
    if let Some(c) = &o.custom {
        // A task of your own has no ranking: the models you gave it, or with a tier that
        // tier's, else the task's.
        let line = custom_line(pool(), store, c);
        models = match &o.tier {
            Some(tier) => store
                .tier_favorites(c, tier)
                .find_map(|k| line.iter().find(|m| m.key == k).copied())
                .into_iter()
                .collect(),
            None => line,
        };
    } else if let Some(t) = o.task {
        let (front, off) = task_line(models.iter().copied(), pool(), store, t);
        models = match &o.tier {
            Some(tier) => tier_pick(&front, &off, store, t.name, tier).map(|e| e.0).into_iter().collect(),
            None => front.into_iter().map(|(m, _)| m).collect(),
        };
    } else if let Some(c) = o.sort.map(|c| &COLS[c]) {
        // Best first, names breaking ties.
        models.sort_by(|a, b| by_value((c.get)(a), (c.get)(b), !c.lower_better).then_with(|| a.name.cmp(&b.name)));
    }
    let total = models.len();
    if o.limit > 0 {
        models.truncate(o.limit);
    }
    // `--tier` is the one model to use: none is a failure however it is printed, as a script
    // reading `[0]` of an empty list would go on with no model. So is an empty `--id` or
    // `--cmd`, whose substitution would start the harness on no model at all, or nothing.
    if models.is_empty() && (o.tier.is_some() || o.id || o.cmd) {
        return Err("no models match".to_string().into());
    }
    if o.json {
        return print_json(&models.iter().map(|m| out(m, store, false)).collect::<Vec<_>>());
    }
    if o.id || o.cmd {
        // The harness you run a favorite of the task on, while it has the model; not one
        // `--via` leaves out, which asks for another's.
        let name = o.task.map(|t| t.name).or(o.custom.as_deref());
        let via = |m: &Model| {
            let h = store.task_via(name?, o.tier.as_deref(), &m.key)?;
            Some(h).filter(|h| o.via.is_empty() || has(&o.via, h))
        };
        let line = |m: &&Model| match o.cmd {
            true => command(m, via(m), &o.via, &data.harness)
                .map(|c| c.join(" "))
                .ok_or_else(|| Exit::from(format!("no harness has {}: there is no command to start it", m.name))),
            false => Ok(via(m)
                .and_then(|h| launch_cmd(m, h, &data.harness)?.pop())
                .unwrap_or_else(|| model_id(m, &data.harness))),
        };
        // All or none: a script must not start on the first of two commands.
        for l in models.iter().map(line).collect::<Result<Vec<_>>>()? {
            println!("{l}");
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

/// The command that starts a harness on `m`: `first` when it has the model, else the first
/// that does, in Via's order and among `only` when it names any.
fn command(
    m: &Model,
    first: Option<&str>,
    only: &[String],
    listed: &BTreeMap<String, Vec<String>>,
) -> Option<Vec<String>> {
    let asked = |h: &&str| only.is_empty() || only.iter().any(|x| x.eq_ignore_ascii_case(h));
    let has = |h: &str| launch_cmd(m, h, listed);
    first.and_then(has).or_else(|| vias().filter(asked).find_map(has))
}

pub fn show(data: &Data, store: &Store, q: &str, json: bool) -> Result {
    let m = resolve(data, q)?;
    if json {
        return print_json(&out(m, store, true));
    }
    for line in detail_lines(m, store, data.any_available()) {
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
    let table = compare_rows(&models, data.any_available());
    // Artificial Analysis names its benchmarks at length.
    let lw = table.iter().map(|r| r.label.chars().count() + 1).max().unwrap_or(0).max(22);
    let mut section = "";
    for row in table {
        // A rule naming each topic above its first row, as in the TUI.
        if row.section != section {
            section = row.section;
            let name = format!("── {section} ");
            println!("{name}{}", "─".repeat((lw + 18 * models.len()).saturating_sub(name.chars().count())));
        }
        print!("{:<lw$}", row.label);
        for c in &row.cells {
            print!("{:>18}", truncate(c, 17));
        }
        println!();
    }
    Ok(())
}

/// The page `open --on` asks for: the first, as `o` then `enter` in the TUI, when it names no
/// site, else that of the site `on` starts the name of, in any case; "aa" is Artificial
/// Analysis, as in `--source`. Err says which sites have the model.
fn page(m: &Model, on: Option<&str>) -> std::result::Result<String, String> {
    let Some(on) = on else { return m.url() };
    let links = m.links();
    let site = Some(on.to_lowercase()).filter(|s| s != "aa").unwrap_or("artificialanalysis".into());
    let sites = links.iter().map(|(s, _)| *s).collect::<Vec<_>>().join(", ");
    let found = links.into_iter().find(|(s, _)| !site.is_empty() && s.starts_with(&site));
    let has = if sites.is_empty() { "no site has one".into() } else { format!("it has {sites}") };
    found.map(|(_, url)| url).ok_or(format!("{} has no page on {on}; {has}", m.name))
}

pub fn open(data: &Data, q: &str, on: Option<&str>) -> Result {
    let url = page(resolve(data, q)?, on)?;
    // Printed first, so that it is there to copy when nothing opens it.
    println!("{url}");
    open::that_detached(&url).map_err(|e| format!("could not open {url}: {e}"))?;
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

/// Give a task of your own another name.
pub fn rename(store: &mut Store, task: &str, new: &str) -> Result {
    store.rename_task(task, new)?;
    store.save()?;
    println!("{task} renamed to {new}");
    Ok(())
}

/// Show the favorite model of every task, of one, or set or clear one; with `about`, write what
/// a task of your own is about, alone or with its model. `q`: the model, and the harness to
/// run it on when one is given.
pub fn fav(
    data: &Data,
    store: &mut Store,
    task: Option<&str>,
    tier: Option<&str>,
    q: Option<(&str, Option<&str>)>,
    rm: bool,
    about: Option<&str>,
) -> Result {
    let model = |key: &str| data.models.iter().find(|m| m.key == key);
    // A favorite the task cannot score still makes its line, unranked.
    let unscored =
        |s: &str, m: &Model| fit::task(s.split(':').next().unwrap_or(s)).is_some_and(|t| fit::fit(m, t).is_none());
    if let (Some(t), Some(text)) = (task, about) {
        if fit::task(t).is_some() {
            return Err(format!("{t} is built in: --about is for a task of your own").into());
        }
        if q.is_none() && !store.custom_tasks().contains(&t) {
            return Err(format!("no task {t} of your own: modelcmp fav {t} <model> --about ... makes it").into());
        }
        store.set_about(t, text);
        // With a model, saved with it below.
        if q.is_none() {
            store.save()?;
            println!("{t}: {}", store.about(t).unwrap_or("no about"));
            return Ok(());
        }
    }
    let task = task.map(|t| slot(t, tier));
    let task = task.as_deref();
    let line = |store: &Store, t: &str, k: &str| {
        let m = model(k);
        let skipped = if m.is_some_and(|m| unscored(t, m)) { "  (no score)" } else { "" };
        let about = store.about(t).map_or(String::new(), |a| format!("  {a}"));
        let via = store.via(t).map_or(String::new(), |h| format!("  via {h}"));
        format!("★ {} [{k}]{via}{skipped}{about}", m.map_or(k, |m| m.name.as_str()))
    };
    match (task, q, rm) {
        (None, ..) => {
            if store.favorite.is_empty() {
                println!("no favorites; modelcmp fav <task> <model> sets one");
            }
            for (t, k) in &store.favorite {
                println!("{t:<18} {}", line(store, t, k));
            }
        }
        (Some(t), None, false) => match store.favorite(t) {
            Some(k) => println!("{}", line(store, t, k)),
            // A model's name where the task goes, or a typo, is no task: not a task with no favorite.
            None => {
                let name = t.split(':').next().unwrap_or(t);
                if fit::task(name).is_none() && !store.custom_tasks().contains(&name) {
                    let msg = format!("no task '{name}': modelcmp fav {name} <model> makes it one of your own");
                    return Err(Exit { code: 2, msg });
                }
                println!("no favorite for {t}");
            }
        },
        // Clearing what is clear already is no failure: a script's reset step runs twice.
        (Some(t), None, true) => {
            if !store.clear_favorite(t) {
                println!("no favorite for {t}");
                return Ok(());
            }
            store.save()?;
            println!("cleared {t}");
        }
        (Some(t), Some((q, via)), _) => {
            let m = resolve(data, q)?;
            // A harness that cannot run the model would leave `--id` with nothing to give.
            if let Some(h) = via.filter(|h| launch_cmd(m, h, &data.harness).is_none()) {
                let has: Vec<&str> = m.via.iter().map(String::as_str).filter(|v| *v != "env").collect();
                let has =
                    if has.is_empty() { "no harness has it".into() } else { format!("it is on {}", has.join(", ")) };
                return Err(format!("{h} does not have {}: {has}", m.name).into());
            }
            // Said, as a mistyped task makes one of your own too.
            let name = t.split(':').next().unwrap_or(t);
            let new = fit::task(name).is_none() && !store.custom_tasks().contains(&name);
            store.set_favorite(t, &m.key, via);
            store.save()?;
            let via = via.map_or(String::new(), |h| format!(" via {h}"));
            println!("★ {t}{}: {}{via}", if new { " (new task)" } else { "" }, m.name);
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

/// The frontier among the models you have and can use, as `list --task` gives it, and the
/// favorites on it only for being favorites.
fn front<'a>(data: &'a Data, store: &'a Store, t: &Task) -> (Vec<(&'a Model, f64)>, Vec<&'a str>) {
    task_line(usable(data, store), usable(data, store), store, t)
}

/// The models you have and can use: only they are recommended.
fn usable<'a>(data: &'a Data, store: &'a Store) -> impl Iterator<Item = &'a Model> {
    visible(data, store, false, false).map(|(_, m)| m).filter(|m| !store.is_excluded(&m.key))
}

/// What a task has and each of its tiers: a favorite, or the harness it runs on.
type Slots<'a> = (Option<&'a str>, BTreeMap<&'static str, &'a str>);

/// A task's favorite and its tiers', the ones `on` its line: one excluded or out of reach is
/// off it, so agents are not pointed at it either. Then the harnesses you run them on, while
/// they have the model, as `--id` and `--cmd` go by.
fn favorites<'a>(data: &Data, store: &'a Store, task: &str, on: impl Fn(&str) -> bool) -> (Slots<'a>, Slots<'a>) {
    let fav = |s: &str| store.favorite(s).filter(|k| on(k));
    let has = |k: &str, h: &str| data.models.iter().any(|m| m.key == k && launch_cmd(m, h, &data.harness).is_some());
    let via = |s: &str| fav(s).and_then(|k| store.via(s).filter(|h| has(k, h)));
    let slots = |of: &dyn Fn(&str) -> Option<&'a str>| {
        (of(task), TIERS.iter().filter_map(|x| Some((x.0, of(&slot(task, Some(x.0)))?))).collect())
    };
    (slots(&fav), slots(&via))
}

/// A model on a task's line in `recommend --json`; a `score` the task does not have is NaN, so null.
fn entry(m: &Model, store: &Store, score: f64, recommended: bool) -> serde_json::Value {
    let mut e = serde_json::json!({"key": m.key, "name": m.name, "context": m.context, "price": m.cost().map(|c| (c * 1000.0).round() / 1000.0), "score": (score * 10.0).round() / 10.0, "recommended": recommended});
    if let Some(n) = store.note(&m.key) {
        e["note"] = n.into();
    }
    // As `price.listed` of `list --json`: an estimate from other providers' list price.
    if m.listed() {
        e["listed"] = true.into();
    }
    e
}

/// `recommend --json`: what agents read to choose a task and its model. Your own tasks come
/// after the built-in ones, in their shape and marked `custom`.
fn recommend_json(data: &Data, store: &Store) -> Vec<serde_json::Value> {
    let custom = store.custom_tasks().into_iter().map(|t| {
        let line = custom_line(usable(data, store), store, t);
        let front: Vec<_> = line.iter().map(|m| entry(m, store, f64::NAN, false)).collect();
        let ((fav, tier_favs), (via, tier_via)) = favorites(data, store, t, |k| line.iter().any(|m| m.key == k));
        serde_json::json!({"name": t, "custom": true, "about": store.about(t).unwrap_or(CUSTOM_ABOUT), "when": CUSTOM_WHEN, "benchmarks": [], "favorite": fav, "tier_favorites": tier_favs, "via": via, "tier_via": tier_via, "frontier": front})
    });
    TASKS
        .iter()
        .map(|t| {
            let (line, off) = front(data, store, t);
            let front: Vec<_> = line
                .iter()
                .map(|&(m, s)| entry(m, store, fit::shown(m, t, s), !off.contains(&m.key.as_str())))
                .collect();
            let ((fav, tier_favs), (via, tier_via)) = favorites(data, store, t.name, |k| line.iter().any(|(m, _)| m.key == k));
            serde_json::json!({"name": t.name, "about": t.about, "when": t.when, "benchmarks": used_benches(t), "favorite": fav, "tier_favorites": tier_favs, "via": via, "tier_via": tier_via, "frontier": front})
        })
        .chain(custom)
        .collect()
}

/// The best model per price for each task, what it measures and when to use it.
pub fn recommend(data: &Data, store: &Store, json: bool) -> Result {
    if json {
        return print_json(&recommend_json(data, store));
    }
    println!("{FRONTIER_LEGEND}\n");
    for t in TASKS {
        println!("{}  {}  (modelcmp list --task {})", t.name, t.about, t.name);
        println!("  use for:         {}", t.when);
        let (line, off) = front(data, store, t);
        let front: Vec<String> = line
            .iter()
            .map(|(m, s)| {
                let p = priced(m, fit::shown(m, t, *s), true, store.is_favorite(Some(t), &m.key));
                if off.contains(&m.key.as_str()) { format!("{p} not recommended") } else { p }
            })
            .collect();
        println!("  best per price:  {}", if front.is_empty() { "no data".into() } else { front.join(" · ") });
        println!();
    }
    for t in store.custom_tasks() {
        println!("{t}  {}  (modelcmp list --task {t})", store.about(t).unwrap_or(CUSTOM_ABOUT));
        println!("  use for:         {CUSTOM_WHEN}");
        let line = custom_line(usable(data, store), store, t);
        let line: Vec<String> = line.iter().map(|m| custom_priced(m, store, t, true)).collect();
        println!("  your model:      {}", if line.is_empty() { "no data".into() } else { line.join(" · ") });
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
            custom: None,
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
            cmd: false,
        };
        assert!(list(&data, &store, &o).is_ok());
    }

    /// A favorite is the tier's model whatever the filters, as in the recommend panel.
    #[test]
    fn a_favorite_a_filter_hides_is_still_the_tier_s() {
        let mut data =
            Data { models: vec![model("gpt55", 90.0, 10.0), model("mini", 60.0, 1.0)], ..Default::default() };
        data.models[0].developer = "openai".into();
        data.models[0].fit.clear();
        let mut store = Store::default();
        let o = ListOpts {
            task: fit::task("coding"),
            custom: None,
            tier: Some("low".into()),
            sort: None,
            bounds: vec![],
            all: false,
            selected: false,
            dev: vec!["openai".into()],
            via: vec![],
            limit: 0,
            json: false,
            id: true,
            cmd: false,
        };
        assert!(list(&data, &store, &o).is_err(), "openai has no model for coding");
        store.set_favorite("coding", "mini", None);
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
            [
                "available",
                "cache_read_per_mtok",
                "id",
                "input_per_mtok",
                "listed",
                "output_per_mtok",
                "provider",
                "via"
            ]
        );
        let full = serde_json::to_value(out(&m, &store, true)).unwrap();
        let extra: Vec<_> = keys(&full).into_iter().filter(|k| !fields.contains(k)).collect();
        assert_eq!(extra, ["benchmarks", "pages", "providers"], "only show and compare add these");
        m.offers[0].unpriced = true;
        let unpriced = serde_json::to_value(out(&m, &store, false)).unwrap();
        assert_eq!(unpriced["price"]["input_per_mtok"], Value::Null, "an unknown price is not free");
        assert_eq!(unpriced["price"]["listed"], false, "nobody lists one");
        // Yours lists none but another provider does: the id is yours, the prices the list ones.
        m.offers[0].available = true;
        m.offers.push(Offer { provider: "q".into(), input: 2.0, output: 2.0, ..Default::default() });
        let price = &serde_json::to_value(out(&m, &store, true)).unwrap()["price"];
        assert_eq!(
            (&price["provider"], &price["input_per_mtok"], &price["listed"]),
            (&"p".into(), &2.0.into(), &true.into())
        );
    }

    #[test]
    fn the_command_starts_the_harness_asked_for() {
        let mut m = model("gpt55", 90.0, 10.0);
        let none = BTreeMap::new();
        assert_eq!(command(&m, None, &[], &none), None, "no harness has it");
        m.offers[0].via = vec!["codex".into(), "opencode".into(), "env".into()];
        let cmd = |first, only: &[&str]| {
            let only: Vec<String> = only.iter().map(|s| s.to_string()).collect();
            command(&m, first, &only, &none).map(|c| c.join(" "))
        };
        assert_eq!(cmd(None, &[]).as_deref(), Some("opencode --model p/gpt55"), "the first in Via's order");
        assert_eq!(cmd(Some("codex"), &[]).as_deref(), Some("codex --model gpt55"), "the favorite's");
        assert_eq!(cmd(Some("claude"), &[]).as_deref(), Some("opencode --model p/gpt55"), "one without it: the next");
        assert_eq!(cmd(None, &["Codex"]).as_deref(), Some("codex --model gpt55"), "--via's, in any case");
        assert_eq!(cmd(None, &["env"]), None, "an API key starts nothing");
    }

    #[test]
    fn open_picks_the_first_page_or_the_site_named() {
        let mut m = model("gpt55", 90.0, 10.0);
        assert_eq!(page(&m, None), Err("no site has a page for gpt55".into()));
        assert_eq!(page(&m, Some("epoch")), Err("gpt55 has no page on epoch; no site has one".into()));
        (m.epoch, m.aa, m.openrouter) = (Some("gpt-5-5".into()), Some("gpt-5-5".into()), Some("openai/gpt-5.5".into()));
        let (epoch, aa) = ("https://epoch.ai/models/gpt-5-5", "https://artificialanalysis.ai/models/gpt-5-5");
        assert_eq!(page(&m, None).as_deref(), Ok(epoch), "the first, as o then enter");
        assert_eq!(page(&m, Some("openrouter")).as_deref(), Ok("https://openrouter.ai/openai/gpt-5.5"));
        for site in ["aa", "AA", "Artificial", "artificialanalysis.ai"] {
            assert_eq!(page(&m, Some(site)).as_deref(), Ok(aa), "{site}");
        }
        let none = "gpt55 has no page on models; it has epoch.ai, artificialanalysis.ai, openrouter.ai";
        assert_eq!(page(&m, Some("models")), Err(none.into()));
        assert!(page(&m, Some("")).is_err(), "no site is not every site");
        let json = serde_json::to_value(out(&m, &Store::default(), true)).unwrap();
        assert_eq!((&json["url"], &json["pages"]["epoch.ai"]), (&Value::from(epoch), &Value::from(epoch)));
    }

    #[test]
    fn recommend_json_leaves_excluded_models_out() {
        let data = Data { models: vec![model("gpt55", 90.0, 10.0), model("mini", 60.0, 1.0)], ..Default::default() };
        let mut store = Store::default();
        let coding = |v: &[Value]| v.iter().find(|t| t["name"] == "coding").unwrap().clone();
        let names = |t: &Value| t["frontier"].as_array().unwrap().iter().map(|e| e["key"].clone()).collect::<Vec<_>>();
        let t = coding(&recommend_json(&data, &store));
        assert_eq!(
            keys(&t),
            ["about", "benchmarks", "favorite", "frontier", "name", "tier_favorites", "tier_via", "via", "when"]
        );
        assert_eq!(keys(&t["frontier"][0]), ["context", "key", "name", "price", "recommended", "score"]);
        assert_eq!(names(&t), ["mini", "gpt55"]);
        store.set_note("gpt55", "slow");
        assert_eq!(
            coding(&recommend_json(&data, &store))["frontier"][1]["note"],
            "slow",
            "a note only when there is one"
        );
        store.set_favorite("coding:low", "mini", Some("pi"));
        let t = coding(&recommend_json(&data, &store));
        assert_eq!(t["tier_favorites"], serde_json::json!({"low": "mini"}));
        assert_eq!(t["tier_via"], serde_json::json!({}), "a harness that lost the model is not one to start");
        let mut data = data;
        data.models[1].offers[0].via = vec!["pi".into()];
        let t = coding(&recommend_json(&data, &store));
        assert_eq!((&t["via"], &t["tier_via"]), (&Value::Null, &serde_json::json!({"low": "pi"})), "its harness");
        store.toggle_excluded("mini");
        let t = coding(&recommend_json(&data, &store));
        assert_eq!(names(&t), ["gpt55"]);
        assert_eq!(t["tier_favorites"], serde_json::json!({}), "nor as a favorite");
        store.toggle_favorite("coding", "gone");
        assert_eq!(coding(&recommend_json(&data, &store))["favorite"], Value::Null, "nor one you do not have");
        // A task of your own comes last, in a built-in one's shape, with the model you gave it.
        store.toggle_favorite("debugging", "gpt55");
        let all = recommend_json(&data, &store);
        let own = all.last().unwrap();
        let fields = [
            "about",
            "benchmarks",
            "custom",
            "favorite",
            "frontier",
            "name",
            "tier_favorites",
            "tier_via",
            "via",
            "when",
        ];
        assert_eq!((keys(own), all.len()), (fields.to_vec(), TASKS.len() + 1));
        assert_eq!(
            (&own["name"], &own["favorite"], names(own)),
            (&"debugging".into(), &"gpt55".into(), vec!["gpt55".into()])
        );
        assert_eq!(own["frontier"][0]["score"], Value::Null, "no benchmark scores it");
        // What it is about is yours to write; agents pick it over a built-in task that fits too.
        assert_eq!((&own["about"], &own["when"]), (&CUSTOM_ABOUT.into(), &CUSTOM_WHEN.into()));
        store.set_about("debugging", "finding and fixing a bug");
        assert_eq!(recommend_json(&data, &store).last().unwrap()["about"], "finding and fixing a bug");
        // A tier of it has a model of its own, as a built-in task's: both on its line, cheapest first.
        store.toggle_excluded("mini");
        store.toggle_favorite("debugging:low", "mini");
        let all = recommend_json(&data, &store);
        let own = all.last().unwrap();
        assert_eq!(
            (names(own), &own["tier_favorites"]),
            (vec!["mini".into(), "gpt55".into()], &serde_json::json!({"low": "mini"}))
        );
        let line = custom_line(data.models.iter(), &store, "debugging");
        let said: Vec<String> = line.iter().map(|m| custom_priced(m, &store, "debugging", false)).collect();
        assert_eq!(said, ["★ mini $1.0 (low)", "★ gpt55 $10"], "the tier it is for, unless the task's");
        store.toggle_excluded("gpt55");
        let all = recommend_json(&data, &store);
        let own = all.last().unwrap();
        assert_eq!((&own["favorite"], names(own)), (&Value::Null, vec!["mini".into()]), "excluded");
    }
}
