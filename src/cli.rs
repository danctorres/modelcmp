//! Non-interactive commands. Text for humans, `--json` for agents.

use crate::app::{
    COLS, Col, TEXT, col_task, col_value, harness_cmd, harness_id, launch_cmd, meets, on_price, runs_on, shown,
    stored_bench,
};
use crate::data::{Data, Model, Offer, vias};
use crate::fit::{self, TASKS, Task};
use crate::store::{Store, slot, task_slots};
use crate::view::{
    CUSTOM_ABOUT, CUSTOM_WHEN, Picks, RECOMMEND_LEGEND, TIERS, by_value, can_fav, compare_rows, custom_front,
    custom_line, custom_when, detail_lines, excluded, fits, left_out, left_out_by, local_best, or_local, over_cap,
    price, priced, reach, shown_via, task_line, tier_pick, tier_picks, truncate, verdict, via, visible, with_local,
    within,
};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Write;

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
    /// Where you have access: the harnesses (opencode, claude, ...) listing it
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
    /// Site -> the model's page there, for each of models.dev, epoch.ai, artificialanalysis.ai,
    /// openrouter.ai and huggingface.co that has one, and arena.ai's agent leaderboard when it ranks the model
    #[serde(skip_serializing_if = "Option::is_none")]
    pages: Option<BTreeMap<&'static str, String>>,
    /// Epoch Capabilities Index, as Epoch publishes it
    eci: Option<f64>,
    /// Task -> its score on the ECI scale, as the table shows it, or a benchmark's 0..100 score
    /// when one is picked in the task's column
    tasks: BTreeMap<&'static str, f64>,
    /// Task -> what its score is from: "ECI", "3 benchmarks" (its own), "overall" (none of its
    /// own, so the overall score) or the benchmark picked in its column
    basis: BTreeMap<&'static str, String>,
    /// Net improvement in real agent sessions, in percent, from arena.ai's Agent leaderboard
    #[serde(skip_serializing_if = "Option::is_none")]
    arena_agent: Option<f64>,
    /// Rating from people's votes on chat, around 1500, from arena.ai's Text leaderboard
    #[serde(skip_serializing_if = "Option::is_none")]
    arena_chat: Option<f64>,
    /// Rating from people's votes on web development, from arena.ai's WebDev leaderboard
    #[serde(skip_serializing_if = "Option::is_none")]
    arena_webdev: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    benchmarks: Option<&'a BTreeMap<String, f64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    providers: Option<Vec<Price<'a>>>,
}

fn out<'a>(m: &'a Model, s: &'a Store, any: bool, full: bool) -> ModelOut<'a> {
    let links = m.links();
    ModelOut {
        key: &m.key,
        name: &m.name,
        developer: &m.developer,
        available: m.available,
        via: &m.via,
        selected: s.is_marked(&m.key),
        excluded: excluded(m, s, any),
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
        // As the table shows it, with the benchmark picked in the task's column.
        tasks: TASKS
            .iter()
            .filter_map(|t| {
                let score = crate::view::line_score(m, t, crate::app::task_bench(s, t))?;
                Some((t.name, (score * 10.0).round() / 10.0))
            })
            .collect(),
        basis: TASKS
            .iter()
            .filter_map(|t| match crate::app::task_bench(s, t) {
                Some(b) => m.picked(b).is_some().then(|| (t.name, b.to_string())),
                None => Some((t.name, fit::fit(m, t)?.1.said())),
            })
            .collect(),
        arena_agent: m.arena,
        arena_chat: m.arena_more.get(crate::data::ARENA_BOARDS[0].0).copied(),
        arena_webdev: m.arena_more.get(crate::data::ARENA_BOARDS[1].0).copied(),
        benchmarks: full.then_some(&m.scores),
        providers: full.then(|| m.offers.iter().map(Price::from).collect()),
    }
}

/// Indented for a person at a terminal, one line for an agent or a pipe: it reads the same
/// and costs a third less context.
fn print_json<T: Serialize>(v: &T) -> Result {
    let pretty = std::io::IsTerminal::is_terminal(&std::io::stdout());
    let s = if pretty { serde_json::to_string_pretty(v) } else { serde_json::to_string(v) };
    println!("{}", s.map_err(|e| e.to_string())?);
    Ok(())
}

/// The TUI's columns, so both show the same thing: Model, Dev, every numeric column, Via. A
/// column shows the benchmark picked in its dropdown, as the bounds go by it.
fn table(models: &[&Model], store: &Store, any: bool) {
    let cols: Vec<(usize, &Col)> = COLS.iter().enumerate().collect();
    let cell = |m: &Model, &(i, _): &(usize, &Col)| {
        let bench = stored_bench(store, i + TEXT);
        col_value(m, i, store).map_or("-".into(), |v| shown(i, v, on_price(i) && m.listed(), bench))
    };
    let cells: Vec<Vec<String>> = models.iter().map(|m| cols.iter().map(|c| cell(m, c)).collect()).collect();
    let cols: Vec<&Col> = cols.into_iter().map(|(_, c)| c).collect();
    let widths: Vec<usize> = (0..cols.len())
        .map(|i| cells.iter().map(|r| r[i].chars().count()).chain([cols[i].head().len()]).max().unwrap_or(0))
        .collect();
    let width = |f: fn(&Model) -> String, head: &str, max: usize| {
        models.iter().map(|m| f(m).chars().count()).chain([head.len()]).max().unwrap_or(0).min(max)
    };
    let (nw, dw) = (width(|m| m.name.clone(), "Model", 34), width(|m| m.developer.clone(), "Dev", 12));
    // Printed at once: stdout writes each line on its own.
    let mut out = String::new();
    let mut line = |mark: &str, name: &str, dev: &str, nums: &mut dyn Iterator<Item = &str>, via: &str| {
        let nums: String = nums.zip(&widths).map(|(v, &w)| format!(" {v:>w$}")).collect();
        let _ = writeln!(out, "{mark} {:<nw$} {:<dw$}{nums}  {via}", truncate(name, nw), truncate(dev, dw));
    };
    line("  ", "Model", "Dev", &mut cols.iter().map(|c| c.head()), "Via");
    for (m, row) in models.iter().zip(&cells) {
        let mark = if excluded(m, store, any) {
            "✗ "
        } else if store.is_marked(&m.key) {
            "✓ "
        } else {
            "  "
        };
        line(mark, &m.name, &m.developer, &mut row.iter().map(String::as_str), &via(m, any, None));
    }
    print!("{out}");
}

/// Every name in `asked` must be one of `known`, any case, or the filter would silently match nothing.
fn check<'a>(flag: &str, asked: &[String], known: impl Iterator<Item = &'a str>) -> Result {
    if asked.is_empty() {
        return Ok(());
    }
    // "not available" is no harness to ask for, as --all and its absence split by access.
    let known: BTreeSet<String> =
        known.filter(|k| !k.is_empty() && *k != crate::view::OUT_OF_REACH).map(str::to_lowercase).collect();
    match asked.iter().find(|a| !known.contains(&a.to_lowercase())) {
        Some(bad) => {
            let known: Vec<_> = known.into_iter().collect();
            Err(format!("{flag} '{bad}' matches no model, use one of: {}", known.join(", ")).into())
        }
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

#[derive(Clone, Default)]
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
    /// With `cmd`: the command that runs one prompt on the model and exits, as `pick` prints it.
    pub prompt: bool,
    /// With a tier: its pick on merit, your favorites aside.
    pub no_fav: bool,
    /// Keys of models to leave out, favorites too: `pick --not`.
    pub not: Vec<String>,
}

/// Whether `o` has a `--max price`, which replaces the Price level picked in the TUI.
fn max_price(o: &ListOpts) -> bool {
    o.bounds.iter().any(|b| b.0 == crate::app::PRICE - TEXT && b.2.is_finite())
}

/// The store with what `o`'s flags replace taken out: `--dev`, `--via` and `--max price` the
/// dropdowns' picks, and a `--min` or `--max` the bound of its kind on its column.
fn flagged(store: &Store, o: &ListOpts) -> Store {
    let mut s = store.clone();
    if !o.dev.is_empty() {
        s.dev_pick.clear();
    }
    if !o.via.is_empty() {
        s.via_pick.clear();
    }
    if max_price(o) {
        s.price_pick = None;
    }
    let flag = |b: &crate::store::Bound| {
        o.bounds.iter().any(|&(c, lo, _)| COLS[c].id == b.col && lo.is_finite() == b.min.is_some())
    };
    s.bounds.retain(|b| !flag(b));
    s
}

/// Says on stderr what of the TUI's filters hides models, as its status bar says it, so an
/// agent finding fewer models than it expects knows why, and the `flags` that replace them.
fn say_filters(store: &Store, flags: &str) {
    let on: Vec<String> = crate::view::picks_said(store).into_iter().chain(crate::view::bounds_said(store)).collect();
    if !on.is_empty() {
        let flags = if flags.is_empty() { String::new() } else { format!("{flags} replace the matching ones, and ") };
        eprintln!("note: filters set in the TUI hide some models: {} ({flags}c in the TUI clears them)", on.join(" "));
    }
    say_benches(store);
}

/// Says on stderr which tasks a benchmark picked in the TUI ranks.
fn say_benches(store: &Store) {
    let by: Vec<String> =
        TASKS.iter().filter_map(|t| Some(format!("{} by {}", t.name, crate::app::task_bench(store, t)?))).collect();
    if !by.is_empty() {
        eprintln!("note: benchmarks picked in the TUI rank some tasks: {} (c in the TUI resets them)", by.join(", "));
    }
}

/// The models `o` asks for, and how many before `--limit` cut them. `store` is `flagged` for it.
/// With a tier, whether the best model your machine runs took it (`local`), so its pick runs there.
/// `lines` keeps each task's line and best local model for the next tier `pick` asks.
fn matched<'a>(
    data: &'a Data,
    store: &'a Store,
    o: &ListOpts,
    lines: Option<&Lines<'a>>,
) -> Result<(Vec<&'a Model>, usize, bool)> {
    check("--dev", &o.dev, data.models.iter().flat_map(Model::devs))?;
    let any = data.any_available();
    check("--via", &o.via, data.models.iter().flat_map(|m| shown_via(m, any)))?;
    let has = |list: &[String], v: &str| list.iter().any(|x| x.eq_ignore_ascii_case(v));
    // A usage error, as when clap knew every task's name.
    if let Some(c) = o.custom.as_deref().filter(|c| !store.custom_tasks().contains(c)) {
        let tasks = [fit::task_names().collect(), store.custom_tasks()].concat().join(", ");
        let msg = format!(
            "no task '{c}': the tasks are {tasks}. To make it a task of your own, run modelcmp fav {c} <model> --tier <tier>"
        );
        return Err(Exit { code: 2, msg });
    }
    let task = o.task.is_some() || o.custom.is_some();
    // A task's frontier is a recommendation, so models you cannot use stay out of it.
    let keep = |m: &&Model| {
        (o.dev.is_empty() || m.devs().any(|d| has(&o.dev, d)))
                // On a task not through ollama or llama.cpp: its copy is no pick on merit (`Model::local`).
                && (o.via.is_empty()
                    || shown_via(m, any).iter().any(|v| has(&o.via, v) && !(task && crate::data::runs_here(v))))
                && o.bounds.iter().all(|&(c, lo, hi)| col_value(m, c, store).is_some_and(|v| meets(c, v, lo, hi)))
                && !(task && excluded(m, store, any))
                && !o.not.contains(&m.key)
    };
    let filtered = || visible(data, store, o.all, o.selected).map(|(_, m)| m).filter(keep);
    let mut ran = false;
    // Your favorites stay on a task's line whatever the filters, as in the recommend panel.
    let pool = || {
        let usable = |m: &&Model| !excluded(m, store, any) && !o.not.contains(&m.key);
        visible(data, store, o.all, false).map(|(_, m)| m).filter(usable)
    };
    let mut models: Vec<&Model> = if let Some(c) = &o.custom {
        // A task of your own has no ranking: the models you gave it, or with a tier that
        // tier's.
        let line = custom_line(pool(), store, c);
        match &o.tier {
            // It has no pick on merit: a built-in task that fits has.
            Some(_) if o.no_fav => vec![],
            Some(tier) => store
                .tier_favorite(c, tier)
                .and_then(|k| line.iter().find(|m| m.key == k && fits(store, tier, m)).copied())
                .into_iter()
                .collect(),
            None => line,
        }
    } else if let Some(t) = o.task {
        let line = || {
            let (front, off) = task_line(filtered(), pool(), store, t);
            // None with `--via`, whose harnesses an agent that cannot start another one names,
            // and where a copy on your machine is picked only as your favorite.
            // No download, as on the TUI's `mine`: a model not on your machine is not one you have.
            let local = o
                .tier
                .as_ref()
                .filter(|_| o.via.is_empty())
                .and_then(|_| local_best(filtered(), t, store, |_| None, None));
            (front, off, local)
        };
        let (front, off, local) = match lines {
            Some(l) => l.borrow_mut().entry((t.name, o.not.clone())).or_insert_with(line).clone(),
            None => line(),
        };
        match &o.tier {
            Some(tier) => {
                // Your caps still hold.
                let mut none = Store::default();
                none.tiers.clone_from(&store.tiers);
                let favs = if o.no_fav { &none } else { store };
                let pick = tier_pick(&front, &off, favs, t.name, tier);
                let won = or_local(pick, local, favs, t.name, tier).map(|e| e.0);
                ran = won.is_some_and(|m| pick.is_none_or(|p| p.0.key != m.key));
                won.into_iter().collect()
            }
            None => front.into_iter().map(|(m, _)| m).collect(),
        }
    } else {
        let mut models: Vec<&Model> = filtered().collect();
        if let Some(i) = o.sort {
            // Best first, names breaking ties, by what the column shows, as the bounds go.
            let (v, desc) = (|m: &Model| col_value(m, i, store), !COLS[i].lower_better);
            // An own score before an estimate it ties, as in the TUI.
            let own = |m: &Model| stored_bench(store, i + TEXT).is_some() || col_task(i).is_none_or(|t| fit::own(m, t));
            models.sort_by(|a, b| {
                by_value(v(a), v(b), desc).then_with(|| own(b).cmp(&own(a))).then_with(|| a.name.cmp(&b.name))
            });
        }
        models
    };
    let total = models.len();
    if o.limit > 0 {
        models.truncate(o.limit);
    }
    Ok((models, total, ran))
}

/// A task's line and the best model your machine runs for it, by task and the models left out:
/// the same for every tier, which `pick` asks one by one.
type Lines<'a> = std::cell::RefCell<HashMap<(&'static str, Vec<String>), Line<'a>>>;
type Line<'a> = (Vec<(&'a Model, f64)>, Vec<&'a str>, Option<(&'a Model, f64)>);

/// `launch`, with a model your machine runs that took the tier (`or_local`, `ran`) started on
/// its runner and not on a harness that runs it in full, at a price: the one you run it on (`H`)
/// when it is a runner, else the first, but not one left out in Via, as `local_best` takes it.
fn launch_local(data: &Data, store: &Store, o: &ListOpts, m: &Model, ran: bool) -> Result<String> {
    if !ran {
        return launch(data, store, o, m);
    }
    let runs = |h: &str| crate::data::runs_here(h) && !left_out(store, h);
    let runner = runs_on(store, m).filter(|h| runs(h)).or_else(|| m.via.iter().map(String::as_str).find(|h| runs(h)));
    // Free on your machine, so no tier's price (`within`) holds it back.
    launch(data, store, &ListOpts { via: runner.into_iter().map(String::from).collect(), tier: None, ..o.clone() }, m)
}

const NO_HARNESS: &str = "no harness lists any models";

pub fn list(data: &Data, store: &Store, o: &ListOpts) -> Result {
    let store = &flagged(store, o);
    let (models, total, ran) = matched(data, store, o, None)?;
    say_filters(store, "--dev, --via, --min and --max");
    let any = data.any_available();
    // `--tier` is the one model to use: none is a failure however it is printed, as a script
    // reading `[0]` of an empty list would go on with no model. So is an empty `--id` or
    // `--cmd`, whose substitution would start the harness on no model at all, or nothing.
    if models.is_empty() && (o.tier.is_some() || o.id || o.cmd) {
        return Err("no models match".to_string().into());
    }
    // Nor one out of every model there is, shown for want of yours: an id no harness here takes.
    if !any && !o.all && (o.tier.is_some() || o.id || o.cmd) {
        return Err(format!("{NO_HARNESS}, so there is no model to start (--all lists every model)").into());
    }
    if o.json {
        return print_json(&models.iter().map(|m| out(m, store, any, false)).collect::<Vec<_>>());
    }
    if o.id || o.cmd {
        // All or none: a script must not start on the first of two commands.
        for l in models.iter().map(|m| launch_local(data, store, o, m, ran)).collect::<Result<Vec<_>>>()? {
            println!("{l}");
        }
        // What an agent checks before it uses a tier's pick, on stderr so a `$(...)` reads
        // the id alone.
        for m in models.iter().filter(|_| o.tier.is_some()) {
            let note = store.note(&m.key).map(|n| format!(", note: {n}")).unwrap_or_default();
            eprintln!("{}: context {}{note}", m.name, m.context);
        }
        return Ok(());
    }
    if models.is_empty() {
        println!("no models match");
        return Ok(());
    }
    table(&models, store, any);
    if models.len() < total {
        println!("\n{} of {total} shown, -n 0 shows all", models.len());
    }
    Ok(())
}

/// For an agent, in one call: every task with what it is for, yours with a model last, and under it the
/// model of each tier as `--tier` picks it, with its context and your note. With `via` the id
/// that harness takes, else the command that starts the harness you run the model on. Tiers
/// with the same model share a line, and tasks with the same lines share them (`grouped`).
/// The models `not` names are left out, as ones a note rules out.
pub fn pick(data: &Data, store: &Store, o: &ListOpts, not: &[String]) -> Result {
    println!("{}", picked(data, store, o, not)?.join("\n"));
    Ok(())
}

/// The lines `pick` prints.
fn picked(data: &Data, store: &Store, o: &ListOpts, not: &[String]) -> Result<Vec<String>> {
    let not = not.iter().map(|q| Ok(resolve(data, q)?.key.clone())).collect::<Result<Vec<_>>>()?;
    let store = &flagged(store, o);
    say_filters(store, "--via and --min");
    // An agent told only what is known goes on to guess a model for its harness.
    check("--via", &o.via, data.models.iter().flat_map(|m| shown_via(m, data.any_available())))
        .map_err(|e| Exit { msg: format!("{}. Do not choose a model, tell the user", e.msg), ..e })?;
    // Every line would be a model with no way to run it.
    if !data.any_available() {
        return Err(format!("{NO_HARNESS}. Do not choose a model, tell the user").into());
    }
    let own = store.tasks_with_models();
    let own = own.iter().map(|&c| (c, None, format!("the user's own task, {}", store.about(c).unwrap_or(CUSTOM_WHEN))));
    let tasks: Vec<_> = TASKS.iter().map(|t| (t.name, Some(t), t.when.to_string())).chain(own).collect();
    // One task, or one tier, for an agent that chose it already.
    if let Some(c) = o.custom.as_deref().filter(|c| !tasks.iter().any(|t| t.0 == *c)) {
        let names = tasks.iter().map(|t| t.0).collect::<Vec<_>>().join(", ");
        return Err(Exit { code: 2, msg: format!("no task '{c}': the tasks are {names}") });
    }
    let (mut said, mut found) = (Vec::new(), false);
    let known = Lines::default();
    for (name, task, when) in tasks.into_iter().filter(|t| o.custom.as_deref().is_none_or(|c| c == t.0)) {
        let mut lines: Vec<(String, String)> = Vec::new();
        let asked = o.tier.is_some();
        for (tier, _) in TIERS.iter().filter(|x| o.tier.as_deref().is_none_or(|t| t == x.0)) {
            let o = ListOpts {
                task,
                custom: task.is_none().then(|| name.to_string()),
                tier: Some((*tier).into()),
                not: not.clone(),
                cmd: o.via.is_empty(),
                prompt: true,
                ..o.clone()
            };
            let line = tier_line(data, store, &o, &known)?;
            // Most tasks have no free model: no line for it, unless it is the tier asked for or
            // has your favorite on another harness.
            if *tier == TIERS[0].0 && !asked && !has_model(&line) {
                continue;
            }
            match lines.last_mut() {
                Some((tiers, l)) if *l == line => *tiers = format!("{tiers}/{tier}"),
                _ => lines.push(((*tier).into(), line)),
            }
        }
        found |= lines.iter().any(|(_, l)| has_model(l));
        said.push((format!("{name}: {when}"), lines.iter().map(|(tiers, l)| format!("  {tiers}  {l}")).collect()));
    }
    // `no model` on every line would send an agent from task to task for nothing.
    if !found {
        // One task with none is no reason to stop: another task may have one.
        if let Some(c) = o.custom.as_deref() {
            return Err(format!("{c} has no model for this request, run pick without --task for another task").into());
        }
        // A copy your machine runs is no pick on merit (`Model::local`).
        let local = !o.via.is_empty() && o.via.iter().all(|v| crate::data::runs_here(&v.to_lowercase()));
        let why = if local { ". A model your machine runs is only picked as a favorite (modelcmp fav)" } else { "" };
        return Err(format!("no task has a model for this request{why}. Do not choose a model, tell the user").into());
    }
    Ok(grouped(said))
}

/// Whether a line of `pick` has a model to run: a favorite on another harness is a model too,
/// and its line the one to relay.
fn has_model(line: &str) -> bool {
    !line.starts_with(NO_MODEL) || line.contains("★ favorite")
}

/// The lines of `pick`: each task's head, then its tiers' lines. Tasks with the same lines are
/// said together, heads first, at the first one's place: with few models most tasks pick the
/// same ones, and an agent reads them once.
fn grouped(tasks: Vec<(String, Vec<String>)>) -> Vec<String> {
    let mut groups: Vec<(Vec<String>, Vec<String>)> = Vec::new();
    for (head, lines) in tasks {
        match groups.iter_mut().find(|g| g.1 == lines) {
            Some(g) => g.0.push(head),
            None => groups.push((vec![head], lines)),
        }
    }
    groups.into_iter().flat_map(|(heads, lines)| heads.into_iter().chain(lines)).collect()
}

const NO_MODEL: &str = "no model";

/// A tier's line in `pick`: your favorite when `o`'s harness has it and its bounds allow it,
/// else the tier's pick on merit, followed by the favorite's command on the harness that has it.
fn tier_line<'a>(data: &'a Data, store: &'a Store, o: &ListOpts, lines: &Lines<'a>) -> Result<String> {
    let allowed =
        |m: &&Model| o.bounds.iter().all(|&(c, lo, hi)| col_value(m, c, store).is_some_and(|v| meets(c, v, lo, hi)));
    let said = |m: &Model, id: String| {
        let ctx = COLS.iter().find(|c| c.id == "ctx").and_then(|c| Some((c.show)((c.get)(m)?)));
        let note = store.note(&m.key).map(|n| format!("  note: {n}")).unwrap_or_default();
        format!("{id}  {}{note}", ctx.unwrap_or("-".into()))
    };
    // One the bounds allow, with what calls it: an id alone is no command to run.
    let line = |o: &ListOpts| {
        let (models, _, ran) = matched(data, store, o, Some(lines))?;
        let m = models.first().copied();
        let call = |m: &Model| launch_local(data, store, o, m, ran).ok();
        Ok::<_, Exit>((m, m.filter(allowed).and_then(|m| Some(said(m, call(m)?)))))
    };
    let (first, said_first) = line(o)?;
    if let Some(l) = said_first {
        return Ok(l);
    }
    // The tier's model ruled out: the next it picks, else its pick without your favorites.
    let rest = ListOpts { not: o.not.iter().cloned().chain(first.map(|m| m.key.clone())).collect(), ..o.clone() };
    let merit = match line(&rest)?.1 {
        Some(l) => l,
        None => line(&ListOpts { no_fav: true, ..rest })?.1.unwrap_or(NO_MODEL.into()),
    };
    let fav = first.filter(allowed);
    let anywhere = ListOpts { via: vec![], cmd: true, ..o.clone() };
    let elsewhere = fav.and_then(|m| {
        Some(format!(
            "  (★ favorite {} is not on {}, run it with: {})",
            m.name,
            o.via.join(", "),
            launch(data, store, &anywhere, m).ok()?
        ))
    });
    Ok(merit + &elsewhere.unwrap_or_default())
}

/// What `--id` or `--cmd` prints for `m`: the id a harness takes, or the command that starts it.
fn launch(data: &Data, store: &Store, o: &ListOpts, m: &Model) -> Result<String> {
    // A tier's pick starts on a harness at the tier's price, among `--via`'s too: `on_via` priced
    // it at the cheapest of them, which need not be the one `harness_cmd` starts.
    let capped = o.tier.as_deref().map(|t| within(store, t, m));
    let m = capped.as_ref().unwrap_or(m);
    // `--via` naming a harness asks for the id that one takes, so one without the model is
    // an error for `--id` as for `--cmd`, not another harness's id.
    let named = o.via.iter().any(|v| vias().any(|h| h.eq_ignore_ascii_case(v)));
    if !o.cmd && !named {
        return Ok(harness_id(store, m, &o.via, &data.harness));
    }
    harness_cmd(store, m, &o.via, &data.harness)
        .and_then(|mut c| match (o.cmd, o.prompt) {
            (true, true) => one_shot(c),
            (true, false) => Some(c.into_iter().map(quote).collect::<Vec<_>>().join(" ")),
            _ => c.pop(),
        })
        .ok_or_else(|| {
            // A favorite stays the pick though `--via` asks for a harness without it.
            let on =
                if o.via.is_empty() { "no harness has".into() } else { format!("{} does not have", o.via.join(", ")) };
            Exit::from(format!("{on} {}, so there is no command to start it", m.name))
        })
}

/// Prices each model a harness of `via` has at its offers there (`reach`), so it is ranked,
/// bounded and given a tier at the price paid on them: one opencode has free costs what pi
/// charges with `--via pi`. A model none of them has keeps its price, as a favorite run
/// elsewhere. `list` and `pick` take `data` so.
pub fn on_via(data: &mut Data, via: &[String]) {
    if via.is_empty() {
        return;
    }
    for m in &mut data.models {
        reach(m, via);
    }
}

/// What a command of `pick` starts with: `quiet` runs the rest.
const QUIET: &str = "modelcmp quiet";

/// `cmd` run as `pick` prints it: the steps a harness prints on stderr (opencode's every
/// command with its output) stay out of the agent's context, and a run that fails says their
/// last lines, where the error is, and fails with its code. With no stdin, which a harness
/// given a prompt would wait on.
pub fn quiet(cmd: &[String]) -> Result {
    use std::process::{Command, Stdio};
    let run = Command::new(&cmd[0]).args(&cmd[1..]).stdin(Stdio::null()).stdout(Stdio::inherit()).output();
    let out = run.map_err(|e| format!("{}: {e}", cmd[0]))?;
    if out.status.success() {
        return Ok(());
    }
    let log = String::from_utf8_lossy(&out.stderr);
    let lines: Vec<&str> = log.lines().collect();
    // A harness that fails saying nothing still leaves the agent something to read.
    let msg = if lines.is_empty() {
        format!("{}: {}", cmd[0], out.status)
    } else {
        lines[lines.len().saturating_sub(20)..].join("\n")
    };
    Err(Exit { code: out.status.code().unwrap_or(1), msg })
}

/// `cmd`, which opens a harness on a model, as the command that runs one prompt there and exits:
/// what an agent runs, where the other waits on a terminal. With the flags that let the harness
/// edit files and run commands (tests, builds) where it would ask, as opencode, pi and omp do
/// unasked: with nobody to ask, it would refuse them.
/// None for llama.cpp's server, which takes no prompt.
/// Behind `QUIET`.
fn one_shot(mut cmd: Vec<String>) -> Option<String> {
    let (sub, flags): (&[&str], &[&str]) = match cmd[0].as_str() {
        "llama-server" => return None,
        "opencode" => (&["run"], &[]),
        "codex" => (&["exec", "--sandbox", "workspace-write"], &[]),
        "claude" => (&[], &["--permission-mode", "acceptEdits", "--allowedTools", "Bash", "-p"]),
        "gemini" => (&[], &["--approval-mode", "yolo", "-p"]),
        "copilot" => (&[], &["--allow-all-tools", "-p"]),
        // Its answer alone: no thinking, and no line wrapping's terminal codes.
        crate::data::OLLAMA => (&[], &["--nowordwrap", "--hidethinking"]),
        // By name, as `launch_cmd` put it there: no search of `PATH` for each line of `pick`.
        crate::data::LLAMA | "llama" => (&[], &["-st", "-p"]),
        // pi, omp
        _ => (&[], &["-p"]),
    };
    cmd.splice(1..1, sub.iter().map(|s| s.to_string()));
    cmd.extend(flags.iter().map(|s| s.to_string()));
    let mut words: Vec<String> = std::iter::once(QUIET.into()).chain(cmd.into_iter().map(quote)).collect();
    words.push("\"<prompt>\"".into());
    Some(words.join(" "))
}

/// A file's path with a space, a quote or a `;` in it is one word to the shell.
fn quote(w: String) -> String {
    // `~` too: it opens no word of an id ("openrouter/~anthropic/..."), where alone the shell expands it.
    let plain = |c: char| c.is_ascii_alphanumeric() || "_@%+=:,./-~".contains(c);
    if w.chars().all(plain) && !w.starts_with('~') { w } else { format!("'{}'", w.replace('\'', "'\\''")) }
}

pub fn show(data: &Data, store: &Store, q: &str, json: bool, all: bool) -> Result {
    let m = resolve(data, q)?;
    if json {
        return print_json(&out(m, store, data.any_available(), true));
    }
    let mut lines = detail_lines(m, store, data.any_available());
    // The providers end the lines, yours first: the others are prices you do not pay.
    let others = m.offers.iter().filter(|o| !o.available).count();
    let cut = !all && others > 0 && others < m.offers.len();
    if cut {
        lines.truncate(lines.len() - others);
        lines.push(format!("    {others} more providers that none of your harnesses use, --all lists them"));
    }
    for line in lines {
        println!("{line}");
    }
    Ok(())
}

pub fn compare(data: &Data, store: &Store, qs: &[String], json: bool) -> Result {
    let models = qs.iter().map(|q| resolve(data, q)).collect::<Result<Vec<_>>>()?;
    if json {
        return print_json(&models.iter().map(|m| out(m, store, data.any_available(), true)).collect::<Vec<_>>());
    }
    say_benches(store);
    let rows = verdict(&models, store);
    let w = |i: usize| rows.iter().map(|r| r[i].chars().count()).max().unwrap_or(0);
    let (w0, w1) = (w(0), w(1));
    println!("verdict:");
    for [q, win, margin] in rows {
        println!("  {q:<w0$}  {win:<w1$}  {margin}");
    }
    println!();
    let table = compare_rows(&models, data.any_available(), &|_| None, store);
    let lw = table.iter().map(|r| r.label.chars().count() + 1).max().unwrap_or(0).max(22);
    let mut section = "";
    for row in table {
        // A rule above each group of rows, as the TUI splits its columns.
        if row.section != section {
            section = row.section;
            println!("{}", "─".repeat(lw + 18 * models.len()));
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
/// Analysis and "hf" Hugging Face. Err says which sites have the model.
fn page(m: &Model, on: Option<&str>) -> std::result::Result<String, String> {
    let Some(on) = on else { return m.url() };
    let links = m.links();
    let site = match on.to_lowercase().as_str() {
        "aa" => "artificialanalysis".into(),
        "hf" => "huggingface".into(),
        site => site.to_string(),
    };
    let sites = links.iter().map(|(s, _)| *s).collect::<Vec<_>>().join(", ");
    let found = links.into_iter().find(|(s, _)| !site.is_empty() && s.starts_with(&site));
    let has = if sites.is_empty() { "or on any other site".into() } else { format!("only on {sites}") };
    found.map(|(_, url)| url).ok_or(format!("{} has no page on {on}, {has}", m.name))
}

pub fn open(data: &Data, q: &str, on: Option<&str>) -> Result {
    let url = page(resolve(data, q)?, on)?;
    // Printed first, so that it is there to copy when nothing opens it.
    println!("{url}");
    open::that_detached(&url).map_err(|e| format!("could not open {url}: {e}"))?;
    Ok(())
}

/// Download a copy of the model to this machine and run it there, on `via` or the runner
/// installed here, installing one first, once you agree, where there is none: the copy it
/// already has, else the GGUF one Hugging Face has of the model's repo (`data::gguf_repo`).
pub fn get(data: &Data, q: &str, via: Option<&str>, wait: bool) -> Result {
    use crate::data::{LLAMA, OLLAMA, gb, get_cmd, gguf_repo, gguf_size, has, install_cmd, keep_repo};
    let m = resolve(data, q)?;
    let local = [LLAMA, OLLAMA];
    let runner = match via {
        Some(v) => *local
            .iter()
            .find(|h| h.eq_ignore_ascii_case(v))
            .ok_or_else(|| format!("{v} does not run models on this machine, --via takes {OLLAMA} or {LLAMA}"))?,
        // The one that has the model, else one installed, else one that can be.
        None => {
            let can = |f: &dyn Fn(&str) -> bool| local.into_iter().find(|h| f(h));
            can(&|h| launch_cmd(m, h, &data.harness).is_some())
                .or_else(|| can(&has))
                .or_else(|| can(&|h| install_cmd(h).is_some()))
                .unwrap_or(LLAMA)
        }
    };
    let here = launch_cmd(m, runner, &data.harness);
    let cmd = match &here {
        Some(cmd) => cmd.clone(),
        None => {
            let none = || format!("{} has no repo on Hugging Face to download", m.name);
            let base = m.hf_repo().ok_or_else(none)?;
            let repo = gguf_repo(base, &m.key)?.ok_or_else(|| format!("Hugging Face has no GGUF copy of {base}"))?;
            // Said before anything is installed or downloaded, and the download goes on without it.
            if let Ok(Some(bytes)) = gguf_size(&repo) {
                eprintln!("{repo} is a download of about {}", gb(bytes));
            }
            get_cmd(runner, &repo)
        }
    };
    if !has(runner) {
        let install = install_cmd(runner).ok_or_else(|| {
            let page =
                if runner == OLLAMA { "https://ollama.com/download" } else { "https://github.com/ggml-org/llama.cpp" };
            format!("{runner} is not installed, and modelcmp does not know how to install it here: see {page}")
        })?;
        let line = install.strip_prefix(&["sh", "-c"]).unwrap_or(install).join(" ");
        eprint!("{runner} is not installed. Run `{line}`? [y/N] ");
        let mut answer = String::new();
        let _ = std::io::stdin().read_line(&mut answer);
        if !matches!(answer.trim(), "y" | "Y" | "yes") {
            return Err(format!("{runner} is not installed").into());
        }
        let done = std::process::Command::new(install[0]).args(&install[1..]).status();
        if !done.is_ok_and(|s| s.success()) {
            return Err(format!("{line} failed").into());
        }
    }
    eprintln!("$ {}", cmd.join(" "));
    if let (None, Some(repo)) = (&here, cmd.last()) {
        // The copy is that model here from now on, whatever its repo is called.
        keep_repo(&m.key, repo.trim_start_matches("hf.co/"));
        eprintln!("Once it is downloaded, `modelcmp --refresh` (or `r` in the TUI) shows it as on your machine.");
    }
    let mut run = std::process::Command::new(&cmd[0]);
    run.args(&cmd[1..]);
    // In a terminal opened for it, an error of the runner's own is waited on too.
    #[cfg(unix)]
    if !wait {
        let e = std::os::unix::process::CommandExt::exec(&mut run);
        return Err(format!("could not start {}: {e}", cmd[0]).into());
    }
    match run.status() {
        Ok(s) if s.success() => Ok(()),
        Ok(s) => Err(format!("{} ended with {s}", cmd[0]).into()),
        Err(e) => Err(format!("could not start {}: {e}", cmd[0]).into()),
    }
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
    match left_out_by(m, store, data.any_available(), false) {
        // Taken back in the TUI alone: the dropdown's entry, which excludes more than this model.
        Some(by) if rm => println!("included {}, but the TUI's filters still hide it: {by}", m.name),
        _ => println!("{} {}", if rm { "included" } else { "✗ excluded" }, m.name),
    }
    Ok(())
}

/// Show your default harness, or set or clear it: the one `--cmd` and `--id` go by when none is
/// asked for and it has the model. With models, the harness each runs on, ahead of the default.
pub fn harness(data: &Data, store: &mut Store, name: Option<&str>, models: &[String], rm: bool) -> Result {
    let known = vias;
    let unknown = |h: &str| {
        let msg = format!("no harness '{h}': the harnesses are {}", known().collect::<Vec<_>>().join(", "));
        Exit { code: 2, msg }
    };
    // `--rm` takes every name as a model's, all found before any is cleared, so a typo leaves
    // each as it was. A key `harness` lists goes too, for a model the data no longer has.
    if rm && name.is_some() {
        let names: Vec<&str> = name.into_iter().chain(models.iter().map(String::as_str)).collect();
        if let Some(h) = names.iter().find(|q| known().any(|v| v.eq_ignore_ascii_case(q))) {
            let msg = format!("--rm takes the models alone, not {h}: modelcmp harness --rm <model>");
            return Err(Exit { code: 2, msg });
        }
        let found = names
            .iter()
            .map(|&q| match store.runs_on.contains_key(q) && !data.models.iter().any(|m| m.key == q) {
                true => Ok((q.to_string(), q.to_string())),
                false => resolve(data, q).map(|m| (m.key.clone(), m.name.clone())),
            })
            .collect::<Result<Vec<_>>>()?;
        for (k, name) in found {
            store.set_runs_on(&k, None);
            println!("{name}: any harness");
        }
        return Ok(store.save()?);
    }
    match name.map(str::to_lowercase) {
        Some(h) if !known().any(|v| v == h) => return Err(unknown(&h)),
        Some(h) if !models.is_empty() => {
            let models = models.iter().map(|q| resolve(data, q)).collect::<Result<Vec<_>>>()?;
            // A harness that cannot run the model would leave `--cmd` with nothing to start.
            if let Some(m) = models.iter().find(|m| launch_cmd(m, &h, &data.harness).is_none()) {
                let has: Vec<&str> = m.via.iter().map(String::as_str).collect();
                let has =
                    if has.is_empty() { "no harness has it".into() } else { format!("it is on {}", has.join(", ")) };
                return Err(format!("{h} does not have {}: {has}", m.name).into());
            }
            for m in models {
                store.set_runs_on(&m.key, Some(&h));
                println!("{}: {h}", m.name);
            }
            store.save()?;
        }
        Some(h) => {
            store.harness = h;
            store.save()?;
            println!("harness: {}", store.harness);
        }
        None if rm => {
            store.harness.clear();
            store.save()?;
            println!("no default harness");
        }
        None => {
            match store.harness.as_str() {
                "" => println!("no default harness, modelcmp harness <name> sets one"),
                h => println!("{h}"),
            }
            for (k, h) in &store.runs_on {
                println!("{k}: {h}");
            }
        }
    }
    Ok(())
}

/// Show the tiers' price caps, or set or reset `low`'s or `mid`'s.
pub fn tier(store: &mut Store, name: Option<&str>, usd: Option<f64>, rm: bool) -> Result {
    let said = |s: &Store, t: &str| match s.tier_cap(t) {
        0.0 => "only free models".to_string(),
        c if c.is_infinite() => "any price".into(),
        c => format!("up to ${c} per 1M tokens"),
    };
    let Some(name) = name else {
        for (t, _) in TIERS {
            println!("{t:<5} {}", said(store, t));
        }
        return Ok(());
    };
    if usd.is_some() || rm {
        store.set_tier_cap(name, usd).map_err(|msg| Exit { code: 2, msg })?;
        store.save()?;
    }
    println!("{name}: {}", said(store, name));
    Ok(())
}

/// Show the memory local models run in, or set it or go back to the one found
/// (`data::memory`): on the TUI's `all` and `✓ selected` tabs a tier picks a model whose download fits in it
/// (`view::local_best`), and a size that does not is grey (`App::fits_here`).
pub fn memory(store: &mut Store, gb: Option<f64>, rm: bool) -> Result {
    match gb {
        Some(g) if !(g.is_finite() && g > 0.0) => {
            return Err(Exit { code: 2, msg: format!("{g} GB is not a valid memory size") });
        }
        Some(g) => store.memory = Some(g),
        None if rm => store.memory = None,
        None => {}
    }
    if gb.is_some() || rm {
        store.save()?;
    }
    let Some(bytes) = crate::view::memory(store) else {
        println!("could not find this machine's memory, modelcmp memory <GB> sets it");
        return Ok(());
    };
    // In GB of 2^30 bytes, as a GPU's is said: 24 GB, not 25.8.
    let gb = bytes as f64 / (1u64 << 30) as f64;
    let memory = if (gb - gb.round()).abs() < 0.05 { format!("{gb:.0} GB") } else { format!("{gb:.1} GB") };
    let whose = if store.memory.is_some() { "set by you" } else { "found on this machine" };
    let room = crate::view::room(store).map_or("-".into(), crate::data::gb);
    println!("{memory} ({whose}): fits local models with a download of up to {room}");
    Ok(())
}

/// Show the folder of the `.gguf` files you downloaded, or set or clear it: llama.cpp runs the
/// models in it, and a refresh lists them.
pub fn models_dir(store: &mut Store, dir: Option<&str>, rm: bool) -> Result {
    let set =
        std::env::var_os("LLAMA_ARG_MODELS_DIR").filter(|d| !d.is_empty()).map(|d| d.to_string_lossy().into_owned());
    match dir {
        Some(d) => {
            let path = std::fs::canonicalize(d).ok().filter(|p| p.is_dir());
            let path = path.ok_or_else(|| Exit { code: 2, msg: format!("{d} is not a folder") })?;
            store.models_dir = path.to_string_lossy().into_owned();
            store.save()?;
            println!("models folder: {}, modelcmp --refresh lists its models", store.models_dir);
            if let Some(env) = set {
                println!("LLAMA_ARG_MODELS_DIR is set, so {env} is read instead");
            }
        }
        None if rm => {
            store.models_dir.clear();
            store.save()?;
            println!("no models folder");
        }
        None => match set.as_deref().or(Some(store.models_dir.as_str())).filter(|d| !d.is_empty()) {
            Some(d) => println!("{d}"),
            None => println!("no models folder, modelcmp models-dir <folder> sets one"),
        },
    }
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
    q: Option<&str>,
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
        // A tier is for a model alone, not for what the task is about.
        if let (None, Some(x)) = (q, tier) {
            let msg = format!("--tier needs a model: modelcmp fav {t} <model> --tier {x}");
            return Err(Exit { code: 2, msg });
        }
        let new = !store.custom_tasks().contains(&t);
        store.set_about(t, text);
        // With a model, saved with it below.
        if q.is_none() {
            store.save()?;
            println!("{t}: {}", store.about(t).unwrap_or("no --about text"));
            // A typo of a task's name makes one too, so it is said.
            if new && store.about(t).is_some() {
                println!("new task {t}: agents see it once it has a model (modelcmp fav {t} <model> --tier <tier>)");
            }
            return Ok(());
        }
    }
    // The task's tiers, or with --tier the one.
    let slots: Vec<String> = match (task, tier) {
        (Some(t), Some(x)) => vec![slot(t, x)],
        (Some(t), None) => task_slots(t).collect(),
        (None, _) => vec![],
    };
    // What is shown or cleared: the task, or with --tier the one.
    let what = task.map(|t| tier.map_or(t.to_string(), |x| slot(t, x))).unwrap_or_default();
    let line = |store: &Store, t: &str, k: &str| {
        let m = model(k);
        let skipped = if m.is_some_and(|m| unscored(t, m)) { "  (no score)" } else { "" };
        let about = store.about(t.split(':').next().unwrap_or(t)).map_or(String::new(), |a| format!("  {a}"));
        let via = m.and_then(|m| crate::app::runs_on(store, m)).map_or(String::new(), |h| format!("  via {h}"));
        // One a later cap left dearer than its tier: kept for when the cap is raised.
        let over = m.and_then(|m| over_cap(store, t, m)).map_or(String::new(), |w| format!("  (unused: {w})"));
        format!("★ {} [{k}]{via}{skipped}{over}{about}", m.map_or(k, |m| m.name.as_str()))
    };
    match (task, q, rm) {
        (None, ..) => {
            if store.favorite.is_empty() {
                println!("no favorites, modelcmp fav <task> <model> --tier <tier> sets one");
            }
            for (t, k) in &store.favorite {
                println!("{t:<18} {}", line(store, t, k));
            }
        }
        (Some(name), None, false) => {
            let favs: Vec<(&String, &str)> = slots.iter().filter_map(|s| Some((s, store.favorite(s)?))).collect();
            // A model's name where the task goes, or a typo, is no task: not a task with no favorite.
            if favs.is_empty() && fit::task(name).is_none() && !store.custom_tasks().contains(&name) {
                let msg = format!(
                    "no task '{name}'. To make it a task of your own, run modelcmp fav {name} <model> --tier <tier>"
                );
                return Err(Exit { code: 2, msg });
            }
            match (favs.as_slice(), tier) {
                ([], _) => println!("no favorite for {what}"),
                ([(s, k)], Some(_)) => println!("{}", line(store, s, k)),
                _ => favs.iter().for_each(|(s, k)| println!("{s:<18} {}", line(store, s, k))),
            }
        }
        // Clearing what is clear already is no failure: a script's reset step runs twice.
        (Some(_), None, true) => {
            let cleared: Vec<&String> = slots.iter().filter(|s| store.clear_favorite(s)).collect();
            if cleared.is_empty() {
                println!("no favorite for {what}");
                return Ok(());
            }
            store.save()?;
            for s in cleared {
                println!("cleared {s}");
            }
        }
        (Some(name), Some(q), _) => {
            // Clap asks for --tier with a model, and a slot of no tier would be gone on the next load.
            let Some(x) = tier else {
                let msg = format!("a model needs --tier: modelcmp fav {name} <model> --tier <tier>");
                return Err(Exit { code: 2, msg });
            };
            let t = &slot(name, x);
            let m = resolve(data, q)?;
            // A tier is a budget: its favorite fits it.
            if let Err(why) = can_fav(store, t, m) {
                return Err(format!("{why}, so make it the favorite of a higher tier").into());
            }
            // Said, as a mistyped task makes one of your own too.
            let name = t.split(':').next().unwrap_or(t);
            let new = fit::task(name).is_none() && !store.custom_tasks().contains(&name);
            store.set_favorite(t, &m.key);
            store.save()?;
            println!("★ {t}{}: {}", if new { " (new task)" } else { "" }, m.name);
            // Kept for when a harness has it, and said: `pick` and `recommend` pass it over.
            if !crate::view::in_reach(m, false, data.any_available()) {
                eprintln!("note: no harness here has {}, so no pick uses it until one does", m.name);
            }
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

/// The frontier among the models you have and can use, the ones the TUI's filters exclude left
/// out, as its recommend ranks them, and the favorites on it only for being favorites.
fn front<'a>(data: &'a Data, store: &'a Store, t: &Task) -> (Vec<(&'a Model, f64)>, Vec<&'a str>) {
    task_line(usable(data, store), usable(data, store), store, t)
}

/// Each tier's pick of task `t`, whose line is `front` (`tier_picks`), with the best model your
/// machine runs in a tier whose pick it beats, the tiers it took (`with_local`)
/// and that model with its score.
fn local_picks<'a>(
    data: &'a Data,
    store: &Store,
    t: &Task,
    front: &[(&'a Model, f64)],
    off: &[&str],
) -> (Picks<'a>, [bool; TIERS.len()], Option<(&'a Model, f64)>) {
    let any = data.any_available();
    // No download, as on the TUI's `mine`: a model not on your machine is not one you have.
    let local = local_best(data.models.iter().filter(|m| !excluded(m, store, any)), t, store, |_| None, None);
    let (picks, took) = with_local(tier_picks(front, off, store, t.name), local, store, t.name);
    (picks, took, local)
}

/// The models you have and can use: only they are recommended.
fn usable<'a>(data: &'a Data, store: &'a Store) -> impl Iterator<Item = &'a Model> {
    let any = data.any_available();
    visible(data, store, false, false).map(|(_, m)| m).filter(move |m| !excluded(m, store, any))
}

/// A task's tiers' favorites, the ones `on` its line: one excluded or out of reach is off it,
/// so agents are not pointed at it either.
fn favorites<'a>(
    data: &Data,
    store: &'a Store,
    task: &str,
    on: impl Fn(&str) -> bool,
) -> BTreeMap<&'static str, &'a str> {
    // One dearer than the tier is no pick of it either.
    let in_budget = |tier: &str, k: &str| data.models.iter().find(|m| m.key == k).is_none_or(|m| fits(store, tier, m));
    let fav = |x: &str| store.tier_favorite(task, x).filter(|k| on(k) && in_budget(x, k));
    TIERS.iter().filter_map(|x| Some((x.0, fav(x.0)?))).collect()
}

/// A model on a task's line in `recommend --json`, with the harnesses that have it (`via`) and
/// the one you run it on (`harness`) while it has it; a `score` the task does not have is NaN, so null.
fn entry(m: &Model, store: &Store, score: f64, recommended: bool) -> serde_json::Value {
    let mut e = serde_json::json!({"key": m.key, "name": m.name, "context": m.context, "via": m.via, "price": m.cost().map(|c| (c * 1000.0).round() / 1000.0), "score": (score * 10.0).round() / 10.0, "recommended": recommended});
    if let Some(n) = store.note(&m.key) {
        e["note"] = n.into();
    }
    if let Some(h) = crate::app::runs_on(store, m) {
        e["harness"] = h.into();
    }
    // As `price.listed` of `list --json`: an estimate from other providers' list price.
    if m.listed() {
        e["listed"] = true.into();
    }
    e
}

/// `recommend --json`: what agents read to choose a task and its model. Your own tasks with a
/// model come after the built-in ones, in their shape and marked `custom`.
fn recommend_json(data: &Data, store: &Store) -> Vec<serde_json::Value> {
    let custom = store.tasks_with_models().into_iter().map(|t| {
        let line = custom_line(usable(data, store), store, t);
        let front: Vec<_> = line.iter().map(|m| entry(m, store, f64::NAN, false)).collect();
        let tier_favs = favorites(data, store, t, |k| line.iter().any(|m| m.key == k));
        let (own, off) = custom_front(&line);
        let tiers = tiers(tier_picks(&own, &off, store, t));
        serde_json::json!({"name": t, "custom": true, "about": CUSTOM_ABOUT, "when": custom_when(store, t), "benchmarks": [], "tier_favorites": tier_favs, "tiers": tiers, "frontier": front})
    });
    TASKS
        .iter()
        .map(|t| {
            let (line, off) = front(data, store, t);
            let front: Vec<_> = line
                .iter()
                .map(|&(m, s)| entry(m, store, s, !off.contains(&m.key.as_str())))
                .collect();
            let tier_favs = favorites(data, store, t.name, |k| line.iter().any(|(m, _)| m.key == k));
            let (picks, _, local) = local_picks(data, store, t, &line, &off);
            // The key of the best model your machine runs.
            let local = local.map(|(m, _)| &m.key);
            let tiers = tiers(picks);
            serde_json::json!({"name": t.name, "about": t.about_ranked(), "when": t.when, "benchmarks": crate::app::task_bench(store, t).map_or_else(|| t.benches.to_vec(), |b| vec![b]), "tier_favorites": tier_favs, "tiers": tiers, "local": local, "frontier": front})
        })
        .chain(custom)
        .collect()
}

/// `recommend --json`'s `tiers`: the key each tier picks, as recommend's grid in the TUI shows.
fn tiers(picks: Picks) -> serde_json::Value {
    let key = |p: Option<(&Model, f64)>| p.map(|e| e.0.key.clone());
    TIERS.iter().zip(picks).map(|(t, p)| (t.0.to_string(), key(p).into())).collect::<serde_json::Map<_, _>>().into()
}

/// The best model at each price for each task, as recommend's grid in the TUI shows it, what the
/// task measures and when to use it.
pub fn recommend(data: &Data, store: &Store, json: bool) -> Result {
    say_filters(store, "");
    if json {
        return print_json(&recommend_json(data, store));
    }
    println!("{}\n", RECOMMEND_LEGEND.replace("{tiers}", &crate::view::tiers_said(store)));
    // Each tier's pick.
    let show = |picks: [Option<(&Model, f64)>; TIERS.len()], text: &dyn Fn(&Model, f64, &str) -> String| {
        for (t, p) in TIERS.iter().zip(picks) {
            let label = format!("{}:", t.0);
            println!("  {label:<17}{}", p.map_or("-".into(), |(m, s)| text(m, s, t.0)));
        }
    };
    for t in TASKS {
        // What ranks it where that is not its own score, as in the TUI: a benchmark picked in its
        // column, or what a task with none goes by.
        let by = crate::app::task_bench(store, t).or(t.ranked).map(|b| format!(", ranked by {b}")).unwrap_or_default();
        println!("{}  {}{by}  (modelcmp list --task {})", t.name, t.about, t.name);
        println!("  use for:         {}", t.when);
        let (line, off) = front(data, store, t);
        let (picks, took, local) = local_picks(data, store, t, &line, &off);
        // `local_took`: the best model your machine runs took the tier.
        let said = |m: &Model, s: f64, fav: bool, local_took: bool| {
            let p = priced(m, s, true, fav);
            match local_took {
                // Free on your machine, which its price would not say.
                true => format!("{} [{}] · free on your machine · {}", m.name, m.key, crate::view::scored(s)),
                false if off.contains(&m.key.as_str()) => format!("{p} not recommended"),
                false => p,
            }
        };
        show(picks, &|m, s, tier| {
            let fav = crate::view::fav_pick(store, t.name, tier, m);
            let mut text = said(m, s, fav, took[TIERS.iter().position(|x| x.0 == tier).unwrap_or(0)]);
            // A favorite in place of the tier's own pick, with what it would recommend, as the TUI.
            if fav {
                let (rec, rec_local) = crate::view::recommended(&line, &off, store.tier_cap(tier), local);
                if let Some((r, rs)) = rec.filter(|(r, _)| r.key != m.key) {
                    text += &format!("\n  {:<17}{}", "recommended:", said(r, rs, false, rec_local));
                }
            }
            text
        });
        println!();
    }
    for t in store.tasks_with_models() {
        // What you wrote about it is when to use it.
        println!("{t}  {CUSTOM_ABOUT}  (modelcmp list --task {t})");
        println!("  use for:         {}", custom_when(store, t));
        let line = custom_line(usable(data, store), store, t);
        let (own, off) = custom_front(&line);
        show(tier_picks(&own, &off, store, t), &|m, _, _| format!("★ {} [{}] {}", m.name, m.key, price(m)));
        println!();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::command;
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
        m.fit.insert("coding".into(), (coding, crate::fit::Basis::Benches(1)));
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
        // With --all, Via reads "not available" for it, but that is no harness to filter by.
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
            prompt: false,
            cmd: false,
            no_fav: false,
            not: vec![],
        };
        assert!(list(&data, &store, &o).is_err());
    }

    /// An entry left out of the TUI's Dev or Price dropdown leaves its models out of `recommend`.
    #[test]
    fn recommend_skips_what_the_tui_dropdowns_leave_out() {
        let mut data =
            Data { models: vec![model("gpt55", 90.0, 10.0), model("mini", 60.0, 1.0)], ..Default::default() };
        data.models[0].developer = "openai".into();
        let t = TASKS.iter().find(|t| t.name == "coding").unwrap();
        let line = |s: &Store| front(&data, s, t).0.iter().map(|(m, _)| m.key.clone()).collect::<Vec<_>>();
        let mut store = Store::default();
        assert_eq!(line(&store), ["mini", "gpt55"]);
        store.dev_not = vec!["openai".into()];
        assert_eq!(line(&store), ["mini"]);
        (store.dev_not, store.price_not) = (vec![], Some(2));
        assert_eq!(line(&store), ["gpt55"], "mini costs no more than $2");
    }

    /// The entries picked in the TUI's dropdowns and its bounds exclude the other models, from
    /// `recommend` and a task's line, a flag replacing its own.
    #[test]
    fn the_tui_s_picks_and_bounds_exclude_models() {
        let mut data =
            Data { models: vec![model("gpt55", 90.0, 10.0), model("mini", 60.0, 1.0)], ..Default::default() };
        (data.models[0].developer, data.models[1].developer) = ("openai".into(), "mistral".into());
        let t = TASKS.iter().find(|t| t.name == "coding").unwrap();
        let line = |s: &Store| front(&data, s, t).0.iter().map(|(m, _)| m.key.clone()).collect::<Vec<_>>();
        let listed = |s: &Store, o: &ListOpts| {
            matched(&data, &flagged(s, o), o, None).ok().unwrap().0.iter().map(|m| m.key.clone()).collect::<Vec<_>>()
        };
        let on_task = |o: ListOpts| ListOpts { task: Some(t), ..o };
        let bound = |col: &str, lo: f64, hi: f64| {
            let c = COLS.iter().position(|c| c.id == col).unwrap();
            on_task(ListOpts { bounds: vec![(c, lo, hi)], ..Default::default() })
        };
        let why = |s: &Store| left_out_by(&data.models[1], s, false, false);
        let mut store = Store::default();
        store.dev_pick = vec!["openai".into()];
        assert_eq!((line(&store), why(&store)), (vec!["gpt55".to_string()], Some("openai picked in Dev".into())));
        assert_eq!(listed(&store, &on_task(ListOpts::default())), ["gpt55"]);
        assert_eq!(listed(&store, &ListOpts::default()).len(), 2, "`list` shows it, marked excluded");
        let dev = on_task(ListOpts { dev: vec!["mistral".into()], ..Default::default() });
        assert_eq!(listed(&store, &dev), ["mini"], "--dev replaces the pick");
        (store.dev_pick, store.price_pick) = (vec![], Some(2));
        assert_eq!(line(&store), ["mini"], "at most $2");
        assert_eq!(listed(&store, &bound("price", f64::NEG_INFINITY, 20.0)).len(), 2, "--max price replaces it");
        assert_eq!(listed(&store, &bound("cache", f64::NEG_INFINITY, 20.0)), ["mini"], "--max cache does not");
        // A bound typed with `>` in the TUI.
        store.price_pick = None;
        store.bounds = vec![crate::store::Bound { col: "coding".into(), min: Some(80.0), max: None }];
        assert_eq!((line(&store), why(&store)), (vec!["gpt55".to_string()], Some("Coding≥80".into())));
        assert_eq!(listed(&store, &bound("coding", 50.0, f64::INFINITY)).len(), 2, "--min coding replaces it");
        assert_eq!(listed(&store, &bound("coding", f64::NEG_INFINITY, 100.0)), ["gpt55"], "--max coding does not");
        (store.bounds, store.price_pick) = (vec![], Some(0));
        assert_eq!(crate::view::picks_said(&store), ["Price=free"]);
    }

    /// A bound takes a score as the table shows it, in whole points: a model shown at 64 meets
    /// `--min coding=64`, and one shown at 63 `--max coding=63`.
    #[test]
    fn a_bound_takes_a_score_in_whole_points() {
        let data = Data { models: vec![model("kimi", 63.986, 2.0), model("opus", 63.3, 5.0)], ..Default::default() };
        let c = COLS.iter().position(|c| c.id == "coding").unwrap();
        let listed = |lo, hi| {
            let o = ListOpts { bounds: vec![(c, lo, hi)], ..Default::default() };
            matched(&data, &Store::default(), &o, None)
                .ok()
                .unwrap()
                .0
                .iter()
                .map(|m| m.key.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(listed(64.0, f64::INFINITY), ["kimi"]);
        assert_eq!(listed(f64::NEG_INFINITY, 63.0), ["opus"]);
    }

    /// A model a dropdown entry leaves out is excluded in `list --json` too, and says which entry.
    #[test]
    fn a_model_a_dropdown_leaves_out_shows_as_excluded() {
        let mut m = model("gpt55", 90.0, 10.0);
        m.developer = "OpenAI".into();
        let mut store = Store::default();
        let shown = |s: &Store| serde_json::to_value(out(&m, s, false, false)).unwrap()["excluded"].clone();
        assert_eq!(shown(&store), false);
        store.dev_not = vec!["OpenAI".into()];
        assert_eq!(
            (shown(&store), left_out_by(&m, &store, false, false)),
            (true.into(), Some("OpenAI left out in Dev".into()))
        );
        (store.dev_not, store.price_not) = (vec![], Some(4));
        assert_eq!(left_out_by(&m, &store, false, false), Some("≤$15 left out in Price".into()));
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
            all: true,
            selected: false,
            dev: vec!["openai".into()],
            via: vec![],
            limit: 0,
            json: false,
            id: true,
            prompt: false,
            cmd: false,
            no_fav: false,
            not: vec![],
        };
        assert!(list(&data, &store, &o).is_err(), "openai has no model for coding");
        store.set_favorite("coding:low", "mini");
        assert!(list(&data, &store, &o).is_ok());
    }

    /// With no harness listing a model every model shows, and a tier's pick out of them is an
    /// id nothing here takes: `opencode -m $(...)` must fail, as `pick` does.
    #[test]
    fn no_harness_is_no_pick() {
        let data = Data { models: vec![model("mini", 60.0, 1.0)], ..Default::default() };
        let o = ListOpts { task: fit::task("coding"), tier: Some("low".into()), id: true, ..Default::default() };
        assert!(list(&data, &Store::default(), &o).is_err());
        assert!(list(&data, &Store::default(), &ListOpts { all: true, ..o }).is_ok());
    }

    /// A favorite is a tier's: one of no tier would be gone on the next load, so it is refused.
    #[test]
    fn a_favorite_needs_a_tier() {
        let data = Data { models: vec![model("mini", 60.0, 1.0)], ..Default::default() };
        let mut store = Store::default();
        let e = fav(&data, &mut store, Some("coding"), None, Some("mini"), false, None).unwrap_err();
        assert_eq!((e.code, store.favorite.is_empty()), (2, true), "{}", e.msg);
    }

    /// `--id --via pi` is the id pi takes: a favorite pi lacks is an error, not opencode's id.
    #[test]
    fn the_id_is_the_one_the_harness_asked_for_takes() {
        let mut data = Data { models: vec![model("mini", 60.0, 1.0), model("gpt55", 90.0, 2.0)], ..Default::default() };
        for (m, h) in data.models.iter_mut().zip(["opencode", "pi"]) {
            (m.available, m.via, m.offers[0].via) = (true, vec![h.into()], vec![h.into()]);
        }
        let mut store = Store::default();
        store.set_favorite("coding:low", "mini");
        let mut o = ListOpts {
            task: fit::task("coding"),
            custom: None,
            tier: Some("low".into()),
            sort: None,
            bounds: vec![],
            all: false,
            selected: false,
            dev: vec![],
            via: vec!["opencode".into()],
            limit: 0,
            json: false,
            id: true,
            prompt: false,
            cmd: false,
            no_fav: false,
            not: vec![],
        };
        assert!(list(&data, &store, &o).is_ok());
        o.via = vec!["pi".into()];
        assert!(list(&data, &store, &o).is_err(), "pi lacks it");
        o.no_fav = true;
        assert!(list(&data, &store, &o).is_ok(), "the tier's pick among pi's");
        // `pick` gives both: pi's own, then the favorite where it runs.
        (o.no_fav, o.prompt) = (false, true);
        let line = |data: &Data, store: &Store, o: &ListOpts| tier_line(data, store, o, &Default::default()).ok();
        let elsewhere = format!(
            "p/gpt55  -  (★ favorite mini is not on pi, run it with: {QUIET} opencode run --model p/mini \"<prompt>\")"
        );
        assert_eq!(line(&data, &store, &o), Some(elsewhere));
        store.set_note("mini", "slow");
        o.via = vec!["opencode".into()];
        assert_eq!(line(&data, &store, &o).as_deref(), Some("p/mini  -  note: slow"));
        o.not = vec!["mini".into()];
        assert_eq!(line(&data, &store, &o).as_deref(), Some("no model"), "ruled out, and opencode has no other");
        (o.not, o.via, o.cmd) = (vec![], vec![], true);
        assert_eq!(
            line(&data, &store, &o),
            Some(format!("{QUIET} opencode run --model p/mini \"<prompt>\"  -  note: slow")),
            "any harness: the command that runs a prompt there"
        );
        // A bound is for the favorite too: a prompt it cannot hold rules it out.
        data.models[1].context = 1_000_000;
        o.bounds = vec![(COLS.iter().position(|c| c.id == "ctx").unwrap(), 600.0, f64::INFINITY)];
        assert_eq!(line(&data, &store, &o), Some(format!("{QUIET} pi --model p/gpt55 -p \"<prompt>\"  1M")));
        let run = |cmd: &[&str]| one_shot(cmd.iter().map(|s| s.to_string()).collect());
        let runs = [
            (&["codex", "--model", "gpt55"][..], "codex exec --sandbox workspace-write --model gpt55"),
            (
                &["claude", "--model", "opus"],
                "claude --model opus --permission-mode acceptEdits --allowedTools Bash -p",
            ),
            (&["gemini", "--model", "flash"], "gemini --model flash --approval-mode yolo -p"),
            (&["copilot", "--model", "gpt55"], "copilot --model gpt55 --allow-all-tools -p"),
            (&["ollama", "run", "qwen"], "ollama run qwen --nowordwrap --hidethinking"),
            (&["llama-cli", "-hf", "a/b"], "llama-cli -hf a/b -st -p"),
            (&["llama-cli", "-m", "/my models/a.gguf"], "llama-cli -m '/my models/a.gguf' -st -p"),
            (&["llama-cli", "-m", "/it's/a;b.gguf"], "llama-cli -m '/it'\\''s/a;b.gguf' -st -p"),
        ];
        for (cmd, said) in runs {
            assert_eq!(run(cmd), Some(format!("{QUIET} {said} \"<prompt>\"")));
        }
        assert_eq!(run(&["llama-server", "-hf", "a/b"]), None, "a server runs no prompt");
    }

    /// `pick --via pi` keeps the free line of a favorite only another harness has, though pi has
    /// no free model: the line says where the favorite runs, as on the other tiers.
    #[test]
    fn pick_keeps_the_free_line_of_a_favorite_on_another_harness() {
        let mut data = Data { models: vec![model("mini", 60.0, 0.0), model("gpt55", 90.0, 2.0)], ..Default::default() };
        for (m, h) in data.models.iter_mut().zip(["opencode", "pi"]) {
            (m.available, m.via, m.offers[0].via) = (true, vec![h.into()], vec![h.into()]);
        }
        let mut store = Store::default();
        store.set_favorite("coding:free", "mini");
        let o = ListOpts { custom: Some("coding".into()), via: vec!["pi".into()], ..Default::default() };
        on_via(&mut data, &o.via);
        let lines = picked(&data, &store, &o, &[]).ok().unwrap();
        let free = format!(
            "  free  no model  (★ favorite mini is not on pi, run it with: {QUIET} opencode run --model p/mini \"<prompt>\")"
        );
        assert!(lines.contains(&free), "{lines:?}");
    }

    /// A model on your machine that scores higher than the free tier's pick takes it, run on
    /// its runner though a harness offers it at a price, not one left out in Via and the one
    /// you run it on first, and not with `--via`.
    #[test]
    fn a_tier_runs_your_machine_s_copy_there() {
        use crate::data::{LLAMA, OLLAMA};
        let mut hosted = model("hosted", 50.0, 0.0);
        (hosted.available, hosted.via, hosted.offers[0].via, hosted.offers[0].available) =
            (true, vec!["opencode".into()], vec!["opencode".into()], true);
        let mut pulled = model("pulled", 80.0, 1.0);
        (pulled.available, pulled.offers[0].via, pulled.offers[0].available) = (true, vec!["opencode".into()], true);
        let copy = Offer { provider: OLLAMA.into(), id: "pulled:4b".into(), local: true, ..pulled.offers[0].clone() };
        let llama =
            Offer { provider: LLAMA.into(), id: "org/pulled-GGUF".into(), via: vec![LLAMA.into()], ..copy.clone() };
        pulled.offers.extend([Offer { via: vec![OLLAMA.into()], ..copy }, llama]);
        pulled.via = vec!["opencode".into(), OLLAMA.into(), LLAMA.into()];
        let data = Data { models: vec![hosted, pulled], ..Default::default() };
        let t = TASKS.iter().find(|t| t.name == "coding").unwrap();
        let o = ListOpts { task: Some(t), tier: Some("free".into()), cmd: true, prompt: true, ..Default::default() };
        let line = |s: &Store, o: &ListOpts| tier_line(&data, s, o, &Default::default()).ok();
        let store = Store::default();
        assert_eq!(
            line(&store, &o),
            Some(format!("{QUIET} ollama run pulled:4b --nowordwrap --hidethinking \"<prompt>\"  -")),
            "not opencode's, at a price"
        );
        let llama = Some(format!("{QUIET} llama-cli -hf org/pulled-GGUF -st -p \"<prompt>\"  -"));
        let mut out = Store::default();
        out.via_not = vec![OLLAMA.into()];
        assert_eq!(line(&out, &o), llama, "not ollama, left out in Via");
        let mut h = Store::default();
        h.set_runs_on("pulled", Some(LLAMA));
        assert_eq!(line(&h, &o), llama, "the runner you run it on");
        let pi = ListOpts { via: vec!["opencode".into()], cmd: false, ..o };
        assert_eq!(line(&store, &pi).as_deref(), Some("p/hosted  -"), "a harness asked for: its own free pick");
    }

    /// A tier judges a model at the price paid where it runs: one opencode has free and pi at a
    /// price is the free tier's on opencode, your default harness pi aside, and none of pi's.
    #[test]
    fn a_tier_s_pick_runs_where_its_price_fits() {
        let mut flash = model("flash", 60.0, 0.0);
        flash.offers[0].provider = "opencode".into();
        let paid = Offer { provider: "openrouter".into(), input: 0.2, output: 0.5, ..flash.offers[0].clone() };
        flash.offers.push(paid);
        for (o, h) in flash.offers.iter_mut().zip(["opencode", "pi"]) {
            (o.available, o.via) = (true, vec![h.into()]);
        }
        (flash.available, flash.via) = (true, vec!["opencode".into(), "pi".into()]);
        let mut data = Data { models: vec![flash], ..Default::default() };
        let mut store = Store::default();
        store.harness = "pi".into();
        let o = ListOpts {
            task: fit::task("coding"),
            tier: Some("free".into()),
            cmd: true,
            prompt: true,
            ..Default::default()
        };
        let line = |data: &Data, o: &ListOpts| tier_line(data, &store, o, &Default::default()).ok();
        let free = format!("{QUIET} opencode run --model opencode/flash \"<prompt>\"  -");
        assert_eq!(line(&data, &o), Some(free), "not pi's, at a price");
        let pi = ListOpts { via: vec!["pi".into()], cmd: false, ..o };
        on_via(&mut data, &pi.via);
        assert_eq!(line(&data, &pi).as_deref(), Some(NO_MODEL), "pi has it at a price");
    }

    /// With `--via` naming both, the free tier's pick, free on one of them, starts on that one:
    /// on opencode, which has it free, not on pi, your default, which charges for it.
    #[test]
    fn a_tier_s_pick_on_two_harnesses_runs_where_its_price_fits() {
        let mut flash = model("flash", 60.0, 0.0);
        flash.offers[0].provider = "opencode".into();
        let paid = Offer { provider: "openrouter".into(), input: 0.2, output: 0.5, ..flash.offers[0].clone() };
        flash.offers.push(paid);
        for (o, h) in flash.offers.iter_mut().zip(["opencode", "pi"]) {
            (o.available, o.via) = (true, vec![h.into()]);
        }
        (flash.available, flash.via) = (true, vec!["opencode".into(), "pi".into()]);
        let mut data = Data { models: vec![flash], ..Default::default() };
        let mut store = Store::default();
        store.harness = "pi".into();
        let o = ListOpts {
            task: fit::task("coding"),
            tier: Some("free".into()),
            via: vec!["opencode".into(), "pi".into()],
            ..Default::default()
        };
        on_via(&mut data, &o.via);
        let line = tier_line(&data, &store, &o, &Default::default()).ok();
        assert_eq!(line.as_deref(), Some("opencode/flash  -"), "pick: not pi's id, at a price");
        let cmd = ListOpts { cmd: true, ..o };
        let m = matched(&data, &store, &cmd, None).ok().unwrap().0[0];
        assert_eq!(launch(&data, &store, &cmd, m).ok().as_deref(), Some("opencode --model opencode/flash"), "--cmd");
    }

    /// `--sort` goes by the benchmark picked in a column, as the table, `--min` and `--max` do.
    #[test]
    fn a_column_sorts_by_the_benchmark_picked_in_it() {
        let mut data =
            Data { models: vec![model("gpt55", 90.0, 10.0), model("mini", 60.0, 1.0)], ..Default::default() };
        for (m, s) in data.models.iter_mut().zip([0.5, 0.7]) {
            m.scores.insert("FrontierSWE".into(), s);
        }
        let mut store = Store::default();
        store.bench.insert("coding".into(), "FrontierSWE".into());
        let o = ListOpts { sort: COLS.iter().position(|c| c.id == "coding"), all: true, ..Default::default() };
        let keys = matched(&data, &store, &o, None).ok().unwrap().0.iter().map(|m| m.key.clone()).collect::<Vec<_>>();
        assert_eq!(keys, ["mini", "gpt55"], "mini scores 70 on FrontierSWE, gpt55 50");
    }

    #[test]
    fn pick_says_the_lines_tasks_share_once() {
        let task = |name: &str, lines: &[&str]| (name.to_string(), lines.iter().map(|l| l.to_string()).collect());
        let said =
            grouped(vec![task("a", &["low x", "high y"]), task("b", &["low z"]), task("c", &["low x", "high y"])]);
        assert_eq!(said, ["a", "c", "low x", "high y", "b", "low z"]);
    }

    #[test]
    fn quiet_says_the_end_of_a_failed_run_alone() {
        let sh = |script: &str| quiet(&["sh".into(), "-c".into(), script.into()]);
        assert!(sh("echo steps >&2").is_ok());
        let log = (1..=30).map(|n| format!("echo {n} >&2")).collect::<Vec<_>>().join("; ");
        let e = sh(&format!("{log}; exit 3")).unwrap_err();
        assert_eq!((e.code, e.msg.lines().count(), e.msg.lines().last()), (3, 20, Some("30")));
        assert!(quiet(&["no-such-harness".into()]).is_err());
    }

    fn keys(v: &Value) -> Vec<&str> {
        v.as_object().unwrap().keys().map(String::as_str).collect()
    }

    /// The JSON is the agents' API: a renamed or dropped field breaks their scripts.
    #[test]
    fn model_json_keeps_its_fields() {
        let store = Store::default();
        let mut m = model("gpt55", 80.0, 2.0);
        let short = serde_json::to_value(out(&m, &store, false, false)).unwrap();
        let fields = [
            "available",
            "basis",
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
        let full = serde_json::to_value(out(&m, &store, false, true)).unwrap();
        let extra: Vec<_> = keys(&full).into_iter().filter(|k| !fields.contains(k)).collect();
        assert_eq!(extra, ["benchmarks", "pages", "providers"], "only show and compare add these");
        m.offers[0].unpriced = true;
        let unpriced = serde_json::to_value(out(&m, &store, false, false)).unwrap();
        assert_eq!(unpriced["price"]["input_per_mtok"], Value::Null, "an unknown price is not free");
        assert_eq!(unpriced["price"]["listed"], false, "nobody lists one");
        // Yours lists none but another provider does: the id is yours, the prices the list ones.
        m.offers[0].available = true;
        m.offers.push(Offer { provider: "q".into(), input: 2.0, output: 2.0, ..Default::default() });
        let price = &serde_json::to_value(out(&m, &store, false, true)).unwrap()["price"];
        assert_eq!(
            (&price["provider"], &price["input_per_mtok"], &price["listed"]),
            (&"p".into(), &2.0.into(), &true.into())
        );
    }

    #[test]
    fn harness_rm_clears_the_models_or_none() {
        let data = Data { models: vec![model("mini", 60.0, 1.0)], ..Default::default() };
        let dir = std::env::temp_dir().join(format!("modelcmp-harness-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut store = Store::load_from(dir.join("user.json"));
        store.set_runs_on("mini", Some("codex"));
        store.set_runs_on("gone", Some("pi"));
        let rm = |store: &mut Store, names: &[&str]| {
            let models: Vec<String> = names[1..].iter().map(|s| s.to_string()).collect();
            harness(&data, store, Some(names[0]), &models, true).map_err(|e| e.code)
        };
        assert_eq!(rm(&mut store, &["codex", "mini"]), Err(2), "a harness is no model to clear");
        assert_eq!(rm(&mut store, &["mini", "typo"]), Err(1), "a typo");
        assert_eq!(store.runs_on("mini"), Some("codex"), "clears none before every one is found");
        assert_eq!(rm(&mut store, &["mini", "gone"]), Ok(()), "a key the data no longer has");
        assert!(store.runs_on.is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_command_starts_the_harness_asked_for() {
        let mut m = model("gpt55", 90.0, 10.0);
        let none = BTreeMap::new();
        assert_eq!(command(&m, &[], &[], &[], None, &none), None, "no harness has it");
        m.offers[0].via = vec!["codex".into(), "opencode".into()];
        let cmd = |first: &[Option<&str>], only: &[&str]| {
            let only: Vec<String> = only.iter().map(|s| s.to_string()).collect();
            command(&m, first, &only, &[], None, &none).map(|c| c.join(" "))
        };
        assert_eq!(cmd(&[], &[]).as_deref(), Some("codex --model gpt55"), "its developer's own, before Via's order");
        assert_eq!(cmd(&[], &["opencode"]).as_deref(), Some("opencode --model p/gpt55"), "--via's");
        assert_eq!(cmd(&[Some("opencode")], &[]).as_deref(), Some("opencode --model p/gpt55"), "the model's");
        assert_eq!(cmd(&[Some("claude")], &[]).as_deref(), Some("codex --model gpt55"), "one without it: the next");
        assert_eq!(
            cmd(&[None, Some("opencode")], &[]).as_deref(),
            Some("opencode --model p/gpt55"),
            "your default one"
        );
        assert_eq!(
            cmd(&[Some("codex"), Some("opencode")], &[]).as_deref(),
            Some("codex --model gpt55"),
            "after the model's"
        );
        assert_eq!(cmd(&[], &["Codex"]).as_deref(), Some("codex --model gpt55"), "--via's, in any case");
        let mut store = Store::default();
        assert_eq!(harness_id(&store, &m, &[], &none), "gpt55", "--id: the id of the one --cmd starts");
        // A harness left out in Via is skipped while another has the model, and --via still asks for it.
        store.via_not = vec!["codex".into()];
        assert_eq!(harness_id(&store, &m, &[], &none), "p/gpt55", "codex left out: opencode's");
        assert_eq!(harness_id(&store, &m, &["codex".into()], &none), "gpt55", "--via codex");
        store.harness = "codex".into();
        assert_eq!(harness_id(&store, &m, &[], &none), "gpt55", "your default, left out or not");
        store.via_not = vec!["codex".into(), "opencode".into()];
        store.harness.clear();
        assert_eq!(harness_id(&store, &m, &[], &none), "gpt55", "every one left out: as if none were");
        store.via_not.clear();
        m.offers[0].via.push(crate::data::OLLAMA.into());
        store.harness = crate::data::OLLAMA.into();
        let start = harness_cmd(&store, &m, &[], &none).map(|c| c.join(" "));
        assert_eq!(start.as_deref(), Some("codex --model gpt55"), "no ollama for one another harness runs in full");
        m.offers[0].via = vec!["pi".into(), "opencode".into()];
        let cmd = command(&m, &[], &[], &[], None, &none).map(|c| c.join(" "));
        assert_eq!(cmd.as_deref(), Some("opencode --model p/gpt55"), "no developer's own: the first in Via's order");
        let cmd = command(&m, &[], &[], &[], Some("pi"), &none).map(|c| c.join(" "));
        assert_eq!(cmd.as_deref(), Some("pi --model p/gpt55"), "the harness running modelcmp, before Via's order");
        m.offers[0].via.push("codex".into());
        let cmd = command(&m, &[], &[], &[], Some("pi"), &none).map(|c| c.join(" "));
        assert_eq!(cmd.as_deref(), Some("codex --model gpt55"), "after its developer's own");
    }

    #[test]
    fn open_picks_the_first_page_or_the_site_named() {
        let mut m = model("gpt55", 90.0, 10.0);
        assert_eq!(page(&m, None), Err("no site has a page for gpt55".into()));
        assert_eq!(page(&m, Some("epoch")), Err("gpt55 has no page on epoch, or on any other site".into()));
        (m.epoch, m.aa, m.openrouter) = (Some("gpt-5-5".into()), Some("gpt-5-5".into()), Some("openai/gpt-5.5".into()));
        let (epoch, aa) = ("https://epoch.ai/models/gpt-5-5", "https://artificialanalysis.ai/models/gpt-5-5");
        assert_eq!(page(&m, None).as_deref(), Ok(epoch), "the first, as o then enter");
        assert_eq!(page(&m, Some("openrouter")).as_deref(), Ok("https://openrouter.ai/openai/gpt-5.5"));
        for site in ["aa", "AA", "Artificial", "artificialanalysis.ai"] {
            assert_eq!(page(&m, Some(site)).as_deref(), Ok(aa), "{site}");
        }
        let none = "gpt55 has no page on models, only on epoch.ai, artificialanalysis.ai, openrouter.ai";
        assert_eq!(page(&m, Some("models")), Err(none.into()));
        assert!(page(&m, Some("")).is_err(), "no site is not every site");
        m.offers.push(Offer { provider: "huggingface".into(), id: "openai/gpt-5.5".into(), ..Default::default() });
        for site in ["hf", "HF", "hugging"] {
            assert_eq!(page(&m, Some(site)).as_deref(), Ok("https://huggingface.co/openai/gpt-5.5"), "{site}");
        }
        let json = serde_json::to_value(out(&m, &Store::default(), false, true)).unwrap();
        assert_eq!((&json["url"], &json["pages"]["epoch.ai"]), (&Value::from(epoch), &Value::from(epoch)));
    }

    /// A benchmark picked in a task's column in the TUI ranks the task here too, as there.
    #[test]
    fn recommend_follows_a_benchmark_picked_in_the_tui() {
        let mut data =
            Data { models: vec![model("gpt55", 90.0, 10.0), model("mini", 60.0, 1.0)], ..Default::default() };
        for (m, s) in data.models.iter_mut().zip([0.5, 0.7]) {
            m.scores.insert("FrontierSWE".into(), s);
        }
        let mut store = Store::default();
        let coding = |s: &Store| recommend_json(&data, s).into_iter().find(|t| t["name"] == "coding").unwrap();
        store.bench.insert("coding".into(), "FrontierSWE".into());
        let t = coding(&store);
        assert_eq!(t["benchmarks"], serde_json::json!(["FrontierSWE"]));
        let tiers = serde_json::json!({"free": null, "low": "mini", "mid": "mini", "high": "mini"});
        assert_eq!(t["tiers"], tiers, "mini beats gpt55 on FrontierSWE for less");
        assert_eq!(t["frontier"][0]["score"], 70.0, "scored on FrontierSWE");
        // A bound typed on the column is on the benchmark too.
        store.bounds = vec![crate::store::Bound { col: "coding".into(), min: Some(65.0), max: None }];
        store.bench.insert("coding".into(), "FrontierSWE".into());
        assert_eq!(crate::view::bounds_said(&store).collect::<Vec<_>>(), ["FrontierSWE≥65"]);
        let out = data.models.iter().map(|m| excluded(m, &store, false)).collect::<Vec<_>>();
        assert_eq!(out, [true, false], "gpt55 scores 50 on FrontierSWE, 90 on coding");
        // One no dropdown lists with the source in use ranks nothing, and its bound bounds nothing:
        // not the task's score, which it was not typed for.
        store.bench.insert("coding".into(), "nope".into());
        assert_eq!(coding(&store)["tiers"]["high"], "gpt55");
        let out = data.models.iter().map(|m| excluded(m, &store, false)).collect::<Vec<_>>();
        assert_eq!(
            (out, crate::view::bounds_said(&store).count()),
            (vec![false, false], 0),
            "mini scores 60 on coding"
        );
    }

    /// A model's `tasks` is the task's table column, so it shows the benchmark picked there.
    #[test]
    fn model_json_tasks_follow_a_benchmark_picked_in_the_tui() {
        let mut m = model("mini", 60.0, 1.0);
        m.scores.insert("FrontierSWE".into(), 0.7);
        let mut store = Store::default();
        store.bench.insert("coding".into(), "FrontierSWE".into());
        let json = serde_json::to_value(out(&m, &store, false, false)).unwrap();
        assert_eq!(json["tasks"]["coding"], 70.0, "FrontierSWE, not the coding score of 60");
    }

    #[test]
    fn recommend_json_leaves_excluded_models_out() {
        let data = Data { models: vec![model("gpt55", 90.0, 10.0), model("mini", 60.0, 1.0)], ..Default::default() };
        let mut store = Store::default();
        let coding = |v: &[Value]| v.iter().find(|t| t["name"] == "coding").unwrap().clone();
        let names = |t: &Value| t["frontier"].as_array().unwrap().iter().map(|e| e["key"].clone()).collect::<Vec<_>>();
        let t = coding(&recommend_json(&data, &store));
        assert_eq!(keys(&t), ["about", "benchmarks", "frontier", "local", "name", "tier_favorites", "tiers", "when"]);
        let tiers = serde_json::json!({"free": null, "low": "mini", "mid": "mini", "high": "gpt55"});
        assert_eq!(t["tiers"], tiers, "as recommend's grid in the TUI");
        assert_eq!(keys(&t["frontier"][0]), ["context", "key", "name", "price", "recommended", "score", "via"]);
        assert_eq!(names(&t), ["mini", "gpt55"]);
        store.set_note("gpt55", "slow");
        assert_eq!(
            coding(&recommend_json(&data, &store))["frontier"][1]["note"],
            "slow",
            "a note only when there is one"
        );
        store.set_favorite("coding:low", "mini");
        store.set_runs_on("mini", Some("pi"));
        let t = coding(&recommend_json(&data, &store));
        assert_eq!(t["tier_favorites"], serde_json::json!({"low": "mini"}));
        let harness = |t: &serde_json::Value| {
            t["frontier"].as_array().unwrap().iter().find(|e| e["key"] == "mini").unwrap()["harness"].clone()
        };
        assert_eq!(harness(&t), serde_json::Value::Null, "a harness that lost the model is not one to start");
        let mut data = data;
        let mini = data.models.iter().position(|m| m.key == "mini").unwrap();
        data.models[mini].via.push("pi".into());
        data.models[mini].offers[0].via.push("pi".into());
        let t = coding(&recommend_json(&data, &store));
        assert_eq!(harness(&t), "pi", "its harness");
        store.toggle_excluded("mini");
        let t = coding(&recommend_json(&data, &store));
        assert_eq!(names(&t), ["gpt55"]);
        assert_eq!(t["tier_favorites"], serde_json::json!({}), "nor as a favorite");
        store.toggle_favorite("coding:high", "gone");
        let t = coding(&recommend_json(&data, &store));
        assert_eq!(t["tier_favorites"], serde_json::json!({}), "nor one you do not have");
        // One with no model yet is not shown: agents would have nothing to run.
        store.set_about("bug-hunt", "finding bugs");
        assert_eq!(recommend_json(&data, &store).len(), TASKS.len());
        // A task of your own comes last, in a built-in one's shape, with the model you gave it.
        store.toggle_favorite("debugging:high", "gpt55");
        let all = recommend_json(&data, &store);
        let own = all.last().unwrap();
        let fields = ["about", "benchmarks", "custom", "frontier", "name", "tier_favorites", "tiers", "when"];
        assert_eq!((keys(own), all.len()), (fields.to_vec(), TASKS.len() + 1));
        assert_eq!(
            (&own["name"], &own["tier_favorites"], names(own)),
            (&"debugging".into(), &serde_json::json!({"high": "gpt55"}), vec!["gpt55".into()])
        );
        assert_eq!(own["frontier"][0]["score"], Value::Null, "no benchmark scores it");
        // When to use it is yours to write; agents pick it over a built-in task that fits too.
        assert_eq!((&own["about"], &own["when"]), (&CUSTOM_ABOUT.into(), &CUSTOM_WHEN.into()));
        store.set_about("debugging", "finding and fixing a bug");
        assert_eq!(
            recommend_json(&data, &store).last().unwrap()["when"],
            "finding and fixing a bug, even when a built-in task fits too"
        );
        // A tier of it has a model of its own, as a built-in task's: both on its line, cheapest first.
        store.toggle_excluded("mini");
        store.toggle_favorite("debugging:low", "mini");
        let all = recommend_json(&data, &store);
        let own = all.last().unwrap();
        assert_eq!(
            (names(own), &own["tier_favorites"]),
            (vec!["mini".into(), "gpt55".into()], &serde_json::json!({"low": "mini", "high": "gpt55"}))
        );
        let tiers = serde_json::json!({"free": null, "low": "mini", "mid": null, "high": "gpt55"});
        assert_eq!(own["tiers"], tiers, "each tier the model it is given, none for the rest");
        // One dearer than its tier, as a cap lowered after it leaves it, points no agent at it.
        store.set_favorite("debugging:mid", "gpt55");
        let all = recommend_json(&data, &store);
        let own = all.last().unwrap();
        let favs = serde_json::json!({"low": "mini", "high": "gpt55"});
        assert_eq!((&own["tier_favorites"], &own["tiers"]), (&favs, &tiers));
        store.clear_favorite("debugging:mid");
        store.toggle_excluded("gpt55");
        let all = recommend_json(&data, &store);
        let own = all.last().unwrap();
        let favs = serde_json::json!({"low": "mini"});
        assert_eq!((&own["tier_favorites"], names(own)), (&favs, vec!["mini".into()]), "excluded");
    }
}
