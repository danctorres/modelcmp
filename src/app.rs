//! TUI state and key handling. No I/O here: side effects come back to the shell as `Effect`s,
//! so every key is unit-testable.

use crate::data::{Data, Failure, Model, Source};
use crate::fit::{TASKS, Task};
use crate::store::Store;
use crate::view::{
    LEVELS, NO_ACCESS, NO_SELECTED, THEMES, TIERS, by_value, ctx, custom_line, hits, in_reach, level_label, money,
    score, shown_via, task_line, task_score, tier_pick,
};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::widgets::TableState;
use std::cmp::Reverse;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU32, Ordering::Relaxed};

/// A numeric column. The text columns (model name, developer) come first and are not listed here.
pub struct Col {
    /// Read through `head`, which names the index column by the source.
    name: &'static str,
    /// The column's name on the command line: `--sort`, `--min`, `--max`.
    pub id: &'static str,
    /// What the column means, for the top border and `?`; read through `about`.
    about: &'static str,
    pub lower_better: bool,
    /// Whether one end of its values is the better one: a release date has no best or worst.
    pub ranked: bool,
    pub get: fn(&Model) -> Option<f64>,
    pub show: fn(f64) -> String,
    /// A bound as typed, a maximum when the flag is set; none when it is not a value of the column.
    pub read: fn(&str, bool) -> Option<f64>,
    /// The one source that has it, when not both: hidden with the other (`hidden`).
    pub only: Option<Source>,
    /// It hangs on the price, so a list price shows in it after a `~` (`on_price`).
    pub price: bool,
}

impl Col {
    /// Its name and meaning; an index column's are its source's, and the task columns' meanings
    /// the benchmark source's, each a single benchmark with Artificial Analysis.
    fn text(&self) -> (&'static str, &'static str) {
        let about = match (self.id, crate::data::source()) {
            ("eci", _) => return Source::Epoch.index(),
            (OTHER, _) => return Source::Aa.index(),
            ("coding", Source::Aa) => "mean of Terminal-Bench 4.0 and SciCode (0-100)",
            ("agentic", Source::Aa) => "Terminal-Bench 4.0 score (0-100)",
            ("reasoning", Source::Aa) => "Humanity's Last Exam score (0-100)",
            _ => self.about,
        };
        (self.name, about)
    }

    pub fn head(&self) -> &'static str {
        self.text().0
    }

    pub fn about(&self) -> &'static str {
        self.text().1
    }

    /// The source its values are from: the one that alone has it, else the one in use.
    pub fn from(&self) -> Source {
        self.only.unwrap_or_else(crate::data::source)
    }
}

const fn col(name: &'static str, id: &'static str, about: &'static str, get: fn(&Model) -> Option<f64>) -> Col {
    Col {
        name,
        id,
        about,
        lower_better: false,
        ranked: true,
        get,
        show: |v| score(Some(v)),
        read: |s, _| s.parse().ok().filter(|v: &f64| !v.is_nan()),
        only: None,
        price: false,
    }
}

/// The benchmarks the dropdown of the task column `id` lists, after the entry for the task's
/// score: "all" with Epoch, which fits them into one, the task's own field with Artificial
/// Analysis, which may have no other about the task: the dropdown then names that one.
fn benched(id: &str) -> Option<(&'static str, &'static [&'static str])> {
    let (own, more) = crate::fit::task(id)?.sourced();
    (own.is_some() || !more.is_empty()).then_some((own.unwrap_or("all"), more))
}

fn positive(x: f64) -> Option<f64> {
    (x > 0.0).then_some(x)
}

/// "2026-09-18" as 20260918 and "2026-09" as 20260900, typed with `-` or `.` and with or without
/// leading zeros; none when it is not a date.
fn ymd(text: &str) -> Option<u32> {
    let mut parts = text.split(['-', '.']);
    let year = parts.next()?.parse::<u32>().ok().filter(|y| *y < 10000)?;
    let mut part = |max| parts.next().map_or(Some(0), |p| p.parse().ok().filter(|n| (1..=max).contains(n)));
    let (month, day) = (part(12)?, part(31)?);
    parts.next().is_none().then_some(year * 10000 + month * 100 + day)
}

/// "2026-09-18" as 2026.0918: sorted by the day, shown by the month (`month`).
fn released(m: &Model) -> Option<f64> {
    ymd(&m.release).map(|d| f64::from(d) / 1e4)
}

/// A bound on Released, typed as the column reads: a maximum runs to the end of the year or
/// the month it names, so `<` `2026-06` keeps June's models.
fn release_bound(text: &str, max: bool) -> Option<f64> {
    let d = ymd(text)?;
    let end = match (d / 100 % 100, d % 100) {
        _ if !max => 0,
        (0, _) => 1231,
        (_, 0) => 31,
        _ => 0,
    };
    Some(f64::from(d + end) / 1e4)
}

/// 2026.0918 as "2026-09", and a bound on the year alone, 2026, as "2026".
fn month(v: f64) -> String {
    let date = (v * 1e4).round() as u32;
    match date / 100 % 100 {
        0 => (date / 10000).to_string(),
        m => format!("{}-{m:02}", date / 10000),
    }
}

/// The id of the AAII column. On the command line it is the other source's index, as `eci` is
/// the one of the source in use, whichever columns those are (`by_role`).
const OTHER: &str = "other";

/// The release, beside Dev, then prices from the offer you'd pay and context, the source's overall
/// index, the task scores, Value and what a task cost and took when Epoch lists it, the other
/// source's index, then speed when Artificial Analysis measures it. What the source in use does
/// not measure is the other one's, when its data is there too (`absent`).
pub const COLS: [Col; 16] = [
    Col {
        ranked: false,
        show: month,
        read: release_bound,
        ..col("Released", "release", "release date, year and month", released)
    },
    Col {
        lower_better: true,
        price: true,
        show: money,
        ..col("Price", "price", "USD per 1M tokens, 3:1 input:output", |m| m.cost())
    },
    Col {
        lower_better: true,
        price: true,
        show: money,
        ..col("$in", "in", "USD per 1M input tokens, cheapest available provider", |m| Some(m.priced_offer()?.input))
    },
    Col {
        lower_better: true,
        price: true,
        show: money,
        ..col("$cache", "cache", "USD per 1M cached input tokens ($in when the provider has no discount)", |m| {
            m.priced_offer().map(|o| o.cache_read.unwrap_or(o.input))
        })
    },
    Col {
        lower_better: true,
        price: true,
        show: money,
        ..col("$out", "out", "USD per 1M output tokens, cheapest available provider", |m| {
            Some(m.priced_offer()?.output)
        })
    },
    Col {
        show: |v| ctx((v * 1000.0) as u64),
        // Typed in tokens, as an agent does: no window is 10M tokens wide.
        read: |s, _| s.parse().ok().filter(|v: &f64| !v.is_nan()).map(|v| if v >= 10_000.0 { v / 1000.0 } else { v }),
        ..col("Ctx", "ctx", "context window, in tokens", |m| positive(m.context as f64 / 1000.0))
    },
    // The two indexes, each in its place whichever source is in use, and named in `Col::text`.
    Col { only: Some(Source::Epoch), ..col("", "eci", "", |m| m.index(Source::Epoch)) },
    Col { only: Some(Source::Aa), ..col("", OTHER, "", |m| m.index(Source::Aa)) },
    col("Coding", "coding", "capability on coding benchmarks, ECI points", |m| task_score(m, "coding")),
    col("Agentic", "agentic", "capability on agentic benchmarks, ECI points", |m| task_score(m, "agentic")),
    col("Reason", "reasoning", "capability on reasoning benchmarks, ECI points", |m| task_score(m, "reasoning")),
    Col { price: true, ..col("Value", "value", "coding per dollar, ranked 0-100", |m| m.fit.get("value").copied()) },
    Col {
        only: Some(Source::Epoch),
        lower_better: true,
        show: money,
        ..col("$task", "cost", "USD per coding task on DeepSWE, measured by Epoch AI", |m| m.task_cost)
    },
    Col {
        only: Some(Source::Epoch),
        lower_better: true,
        // In tokens, not thousands as Ctx: a task may take fewer than Ctx's 10,000 that tell them apart.
        show: |v| ctx(v as u64),
        ..col("Tok/task", "tokens", "output tokens per coding task on DeepSWE, measured by Epoch AI", |m| m.task_tokens)
    },
    Col {
        only: Some(Source::Aa),
        show: |v| format!("{v:.0}"),
        ..col("Tok/s", "tps", "output tokens per second (median)", |m| m.tps)
    },
    Col {
        only: Some(Source::Aa),
        lower_better: true,
        show: |v| format!("{v:.1}s"),
        ..col("TTFT", "ttft", "seconds to the first answer token, after any thinking (median)", |m| m.ttft)
    },
];

/// Whether `COLS[i]` hangs on the price: Price, $in, $cache and $out, and Value, which divides by it.
pub fn on_price(i: usize) -> bool {
    COLS[i].price
}

/// A value of `COLS[i]` as its cell shows it: after a `~` when it comes from a list price (`Model::listed`).
pub fn shown(i: usize, v: f64, listed: bool) -> String {
    let s = (COLS[i].show)(v);
    if listed && on_price(i) { format!("~{s}") } else { s }
}

/// Text columns before the numbers: 0 is the model name, 1 its developer. `VIA` follows them.
pub const TEXT: usize = 2;
/// Column index of the blended price, the frontier's sort.
pub const PRICE: usize = TEXT + 1;
/// The sort the table starts with, and that `c` and leaving a task go back to: the source's
/// index (ECI or AAII), best first. The cursor starts on that column too.
fn default_sort() -> (usize, bool) {
    (index_col(), true)
}
/// Column index of ECI, and of AAII beside it.
pub const ECI: usize = TEXT + 6;
const AAII: usize = ECI + 1;

/// Column index of the index of the source in use.
pub fn index_col() -> usize {
    if crate::data::source() == Source::Aa { AAII } else { ECI }
}

/// `COLS[c]` as the command line names it: there `eci` is the index of the source in use and
/// `other` the other one's.
pub fn by_role(c: usize) -> usize {
    if crate::data::source() == Source::Aa && [ECI, AAII].contains(&(c + TEXT)) { ECI + AAII - 2 * TEXT - c } else { c }
}
/// Column index of $task, the first of what a run measured: a task's cost and tokens, then speed.
const MEASURED: usize = TEXT + 12;
/// First column of each group: names and release, price and context, scores, measured, your own.
pub const GROUPS: [usize; 5] = [0, PRICE, ECI, MEASURED, VIA];
/// Column index of where you have access.
pub const VIA: usize = TEXT + COLS.len();
/// Column index of your note, the last one.
pub const NOTES: usize = VIA + 1;
pub const NCOLS: usize = NOTES + 1;

/// The columns turned off in `|`, a bit per cursor index: one process-wide setting, like
/// `data::source`.
#[cfg(not(test))]
static OFF: AtomicU32 = AtomicU32::new(0);
#[cfg(test)]
thread_local!(static OFF: AtomicU32 = const { AtomicU32::new(0) });

fn off() -> u32 {
    #[cfg(test)]
    return OFF.with(|o| o.load(Relaxed));
    #[cfg(not(test))]
    OFF.load(Relaxed)
}

fn set_off(mask: u32) {
    #[cfg(test)]
    OFF.with(|o| o.store(mask, Relaxed));
    #[cfg(not(test))]
    OFF.store(mask, Relaxed);
}

/// Whether the column at cursor index `col` has nothing to show: one of the other source's,
/// with none of its data.
pub fn absent(col: usize) -> bool {
    numeric(col).is_some_and(|c| c.from() != crate::data::source() && !crate::data::lent())
}

/// The benchmark source the column at cursor index `col` has its values from, when it has them
/// from one: every column from the indexes on, the task scores and Value of the source in use.
pub fn col_source(col: usize) -> Option<Source> {
    numeric(col).filter(|_| col >= ECI).map(Col::from)
}

/// Whether the column at cursor index `col` is left out: `absent`, or turned off in `|`.
pub fn hidden(col: usize) -> bool {
    absent(col) || off() >> col & 1 == 1
}

/// The column at cursor index `col` as `Store::hide` names it. Model and Dev always show.
fn col_id(col: usize) -> &'static str {
    match col {
        VIA => "via",
        NOTES => "notes",
        _ => numeric(col).map_or("", |c| c.id),
    }
}

/// The first shown column of each of `GROUPS`.
pub fn group_starts() -> Vec<usize> {
    let end = |i: usize| GROUPS.get(i + 1).copied().unwrap_or(NCOLS);
    GROUPS.iter().enumerate().filter_map(|(i, &g)| (g..end(i)).find(|&c| !hidden(c))).collect()
}

/// Cursor index `col` moved `n` shown columns right, or left when negative, wrapping.
fn step_col(mut col: usize, n: isize) -> usize {
    for _ in 0..n.unsigned_abs() {
        loop {
            col = if n < 0 { (col + NCOLS - 1) % NCOLS } else { (col + 1) % NCOLS };
            if !hidden(col) {
                break;
            }
        }
    }
    col
}

/// The numeric column at cursor index `col`, if it is one.
pub fn numeric(col: usize) -> Option<&'static Col> {
    COLS.get(col.checked_sub(TEXT)?)
}

/// Header of the column at cursor index `col`.
pub fn base_col_name(col: usize) -> &'static str {
    match col {
        0 => "Model",
        1 => "Dev",
        VIA => "Via",
        NOTES => "Notes",
        _ => numeric(col).map_or("", Col::head),
    }
}

/// Whether the header of the column at cursor index `col` opens a dropdown with `d`.
pub fn has_menu(col: usize) -> bool {
    col == 1 || col == PRICE || col == VIA || TASK_COLS.contains(&col)
}

/// Coding, Agentic and Reason: the columns `col_benches` has benchmarks for, with either
/// source. Asked of every header on every frame, so not looked up.
const TASK_COLS: std::ops::RangeInclusive<usize> = AAII + 1..=AAII + 3;

/// The benchmarks the dropdown of the task column at cursor index `col` lists.
fn col_benches(col: usize) -> Option<(&'static str, &'static [&'static str])> {
    benched(numeric(col)?.id)
}

/// What the column at cursor index `col` means.
pub fn base_col_about(col: usize) -> String {
    match col {
        0 => "model name (dimmed if Via is empty)".into(),
        1 => "company that trained the model".into(),
        VIA => "harnesses listing it (↓ can be downloaded)".into(),
        NOTES => "your own note on the model".into(),
        PRICE => format!(
            "{}, {:.0}% of the input cached, ~ list price when yours has none",
            COLS[PRICE - TEXT].about,
            crate::data::cached() * 100.0
        ),
        _ => {
            let c = numeric(col);
            // Epoch fits a task's benchmarks into one score, so the column names them.
            let about = match c.and_then(|c| crate::fit::task(c.id)).map(Task::sourced) {
                Some((None, b)) if !b.is_empty() => format!("fitted from {}, ECI points", named(b)),
                _ => c.map_or("", Col::about).into(),
            };
            // A task's column is of the source in use, beside both sources' indexes.
            if TASK_COLS.contains(&col) { of_source(&about) } else { about }
        }
    }
}

/// `about` after the source in use, whose scores it tells of.
fn of_source(about: &str) -> String {
    format!("{}, {about}", crate::data::source().label())
}

/// "A, B and C", or the first two and how many more.
fn named(b: &[&str]) -> String {
    match b {
        [one] => one.to_string(),
        [most @ .., last] if b.len() <= 3 => format!("{} and {last}", most.join(", ")),
        _ => format!("{}, {} and {} more", b[0], b[1], b.len() - 2),
    }
}

pub const HELP: &[(&str, &[(&str, &str)])] = &[
    (
        "General",
        &[
            ("?", "this help, / keeps the lines that match"),
            ("esc", "back: overlay, highlight, filter, S, F, E, task"),
            ("q", "quit, asks first"),
            ("r", "refresh data now (auto at start after 24h)"),
            ("U", "upgrade modelcmp when a newer version is out, asks first"),
            ("B", "benchmarks from Epoch AI or Artificial Analysis"),
            ("H", "default harness, for x, Y, --cmd and --id"),
        ],
    ),
    (
        "Move",
        &[
            ("j k ↓ ↑", "move, and a count repeats, as in 3j"),
            ("h l ← →", "pick a column. In compare a model, in recommend a tier or the task"),
            ("0 _ $ w b", "first / last column, next / previous group"),
            ("gg G 3gg", "top / bottom / row 3"),
            ("( ) ^u ^d", "half a page up / down"),
            ("] [", "next / previous selected model, or ticked entry"),
            ("} {", "next / previous available model"),
            ("v", "highlight a range, space e C act on all of it"),
            ("V", "highlight the selected models, Ve excludes them all"),
        ],
    ),
    (
        "Filter and sort",
        &[
            ("s", "sort by the column, again reverses"),
            ("/", "filter models, compare rows, this help or a list"),
            ("> <", "minimum / maximum for the column, e.g. > 155 enter"),
            ("d", "dropdown on a header with ▾, space enter toggle"),
            ("|", "columns to show, space enter toggle"),
            ("a A", "all models, including ones you have no access to / yours only"),
            ("tab", "next tab, shift+tab back"),
            ("%", "Price with none of the input cached, or back to --cache"),
            ("c", "clear filters, bounds, task, S, F and E, and the selection stays"),
        ],
    ),
    (
        "Selected ✓, favorite ★, excluded ✗",
        &[
            ("✓", "selected: your shortlist, kept until you deselect it"),
            ("★", "favorite: your pick for a task, always in its recommendation"),
            ("✗", "excluded: you have it but cannot use it, and recommendations skip it"),
            ("space", "select the model, C compares the selected"),
            ("f", "favorite the model for a task, a tier of one, or a task you name"),
            ("f: r a", "rename a task you named, write what it is about"),
            ("e", "exclude the model"),
            ("u", "deselect every model"),
            ("D", "unfavorite every model, asks first"),
            ("X", "unexclude every model, asks first"),
            ("S F E", "selected / favorite / excluded only"),
        ],
    ),
    (
        "Model under the cursor",
        &[
            ("n", "note for the model, agents read it"),
            ("y Y", "copy the model name / id"),
            ("o", "open the model's page on a site"),
            ("x", "open a harness on the model in a new terminal"),
        ],
    ),
    (
        "Panels",
        &[
            ("enter", "details: every benchmark, price per provider"),
            ("C", "compare the selected models"),
            ("R", "recommend: the model each tier picks for each task"),
            ("t", "theme"),
        ],
    ),
    (
        "Input and mouse",
        &[
            ("typing", "^a ^e ^← ^→ move, ^w ^u ^k delete"),
            ("mouse", "click highlights, double click opens; right click selects"),
        ],
    ),
];

/// The Via dropdown's entries that are no harness, after the harnesses: the models your machine
/// runs through ollama or llama.cpp, the others, and the ones `x` can download.
const LOCAL: &str = "local";
const NOT_LOCAL: &str = "not local";
const DOWNLOAD: &str = "↓ download";
const VIA_KINDS: [&str; 3] = [LOCAL, NOT_LOCAL, DOWNLOAD];

/// The tabs above the table, each with its name, its key and its hint in the status bar while
/// it is cut off: yours, all, the selected, favorite and excluded only, then the panels,
/// recommend and compare, the theme list and the help.
pub const TABS: [(&str, char, &str); 9] = [
    ("yours", 'A', "A yours"),
    ("all", 'a', "a all"),
    ("✓ selected", 'S', "S selected only"),
    ("★ favorites", 'F', "F favorites only"),
    ("✗ excluded", 'E', "E excluded only"),
    ("recommend", 'R', "R recommend"),
    ("compare", 'C', "C compare"),
    ("theme", 't', "t theme"),
    ("help", '?', "? help"),
];
/// Where each tab is in `TABS`; the panels' are from recommend's on.
pub const YOURS: usize = 0;
const ALL: usize = 1;
pub const MARKED: usize = 2;
pub const FAV: usize = 3;
pub const EXCLUDED: usize = 4;
pub const RECOMMEND: usize = 5;
const COMPARE: usize = 6;
const THEME: usize = 7;
pub const HELP_TAB: usize = 8;

/// Where esc leaves details for: the table, or the overlay they were opened from at its scroll.
#[derive(PartialEq, Debug, Clone, Copy)]
pub enum Back {
    Table,
    Compare(u16),
    Recommend(u16),
}

#[derive(PartialEq, Debug)]
pub enum View {
    Table,
    /// Details, and where esc goes back to.
    Detail(Back),
    Compare,
    Help,
    Recommend,
}

#[derive(PartialEq, Debug)]
pub enum Input {
    None,
    /// Typing `App::query`; `cur` is the cursor's byte offset in it, `was` the query esc
    /// goes back to.
    Search {
        cur: usize,
        was: String,
    },
    /// `key`: the model the note is for, as the rows can move under the prompt.
    Note {
        key: String,
        text: String,
        cur: usize,
    },
    /// Typing Artificial Analysis's API key: after picking it with `B` with none saved, or when
    /// it turned the saved one down (`wrong`).
    Key {
        text: String,
        cur: usize,
        wrong: bool,
    },
    /// Typing a minimum (`>`) or maximum (`<`) for column `col`.
    Bound {
        col: usize,
        min: bool,
        text: String,
        cur: usize,
    },
    /// The dropdown under the Dev or $ header: each entry and how many models it would show,
    /// "any" first. `list.sel` indexes the entries matching its query, see `menu_rows`.
    Menu {
        col: usize,
        items: Vec<(String, usize)>,
        list: List,
    },
    /// `q` asks before quitting.
    Quit,
    /// `U` asks before upgrading.
    Upgrade,
    /// `D` asks before unfavoriting every model.
    Unfavorite,
    /// `X` asks before unexcluding every model.
    Unexclude,
    /// A choice of what to do, `list.sel` under the cursor: `x` on a model several harnesses
    /// have launches one, `o` opens one of the model's pages. Each item is its label and effect.
    Choose {
        title: &'static str,
        kind: Kind,
        items: Vec<(String, Effect)>,
        list: List,
    },
}

/// What a choice list chooses: the key that opened it.
#[derive(PartialEq, Debug, Clone, Copy)]
pub enum Kind {
    /// `f`: the tasks a model is the favorite for, a row each with a box per tier; space or
    /// enter ticks the box under the cursor and the grid stays open.
    Fav,
    /// `o`: the site to open the model on.
    Open,
    /// `x`: the harness to open on the model.
    Launch,
    /// `v` in `f`'s grid: the harness a task's favorite runs on; picking one goes back to that list.
    Via,
    /// `t`: the theme, previewed under the cursor.
    Theme,
    /// `B`: the benchmark source.
    Source,
    /// `H`: your default harness.
    Harness,
    /// `|`: the columns the table shows; space or enter ticks one and the list stays open.
    Cols,
}

/// The cursor and the search of an open list, a dropdown or a choice list.
#[derive(PartialEq, Debug, Default)]
pub struct List {
    /// The cursor, among the entries matching `query`.
    pub sel: usize,
    /// The first entry in view, kept by the draw (`tui::list_top`) as the table keeps its own.
    pub top: usize,
    /// Filter on the entries, typed after `/`.
    pub query: String,
    /// The cursor's byte offset in `query`.
    pub cur: usize,
    /// Typing into `query`, after `/`.
    pub typing: bool,
    /// The entry under the cursor being written in place, in `f`'s grid.
    pub edit: Option<Edit>,
    /// The box under the cursor along a row of `f`'s grid: 0 the task's own, then its tiers'.
    pub col: usize,
}

/// An entry of `f`'s grid written where it is, the list staying open: a task of your own being
/// named, renamed or described.
#[derive(PartialEq, Debug)]
pub struct Edit {
    pub what: What,
    pub text: String,
    /// The cursor's byte offset in `text`.
    pub cur: usize,
    /// Why enter did not take the text, said under the list.
    pub err: Option<String>,
}

/// What an `Edit` writes.
#[derive(PartialEq, Debug)]
pub enum What {
    /// The name of a new task, on `+ new task`.
    New,
    /// Another name for this task.
    Rename(String),
    /// What this task is about.
    About(String),
}

impl Edit {
    /// Starting from `text`, the cursor at its end.
    fn new(what: What, text: String) -> Self {
        Edit { what, cur: text.len(), text, err: None }
    }
}

impl List {
    fn at(sel: usize) -> Self {
        List { sel, ..Default::default() }
    }

    /// Whether its keys move and pick: nothing is being typed, a search or an entry.
    fn idle(&self) -> bool {
        !self.typing && self.edit.is_none()
    }

    /// Write `edit` on entry `at`, among them all: a search is dropped, as it could hide it.
    fn write(&mut self, at: usize, edit: Edit) {
        *self = List { sel: at, top: self.top, col: self.col, edit: Some(edit), ..Default::default() };
    }

    /// The keys a list takes whatever it lists: `/` starts a search, and while searching ↓ ↑
    /// move, esc drops the search and the rest is typed. `at` is the entry under the cursor,
    /// among them all, and `len` counts the entries a query leaves; with `any` the first always
    /// stays, as a dropdown's "any". False when the key is none of these.
    fn key(
        &mut self,
        code: KeyCode,
        mods: KeyModifiers,
        at: Option<usize>,
        len: impl Fn(&str) -> usize,
        any: bool,
    ) -> bool {
        match code {
            KeyCode::Down if self.typing => self.sel = step(self.sel, 1, len(&self.query)),
            KeyCode::Up if self.typing => self.sel = step(self.sel, -1, len(&self.query)),
            // Esc while searching drops the search but keeps the entry under the cursor.
            KeyCode::Esc if self.typing => {
                (self.sel, self.typing, self.cur) = (at.unwrap_or(0), false, 0);
                self.query.clear();
            }
            _ if self.typing => {
                let was = self.query.clone();
                // The first match, so that enter picks it; a move within the text keeps the cursor.
                if edit(&mut self.query, &mut self.cur, code, mods, |_| true) && self.query != was {
                    self.sel = if any { len(&self.query).min(2).saturating_sub(1) } else { 0 };
                }
            }
            KeyCode::Char('/') => self.typing = true,
            _ => return false,
        }
        true
    }
}

impl Input {
    /// A choice list with the cursor on `sel` and nothing searched yet.
    fn choose(title: &'static str, kind: Kind, items: Vec<(String, Effect)>, sel: usize) -> Self {
        Self::Choose { title, kind, items, list: List::at(sel) }
    }
}

/// Indices of the dropdown entries whose name contains `query`, any case; "any" always stays.
pub fn menu_rows(items: &[(String, usize)], query: &str) -> Vec<usize> {
    let q = query.to_lowercase();
    (0..items.len()).filter(|&i| i == 0 || items[i].0.to_lowercase().contains(&q)).collect()
}

/// The last entry of `f`'s grid: it asks the name of a task of your own.
const NEW_TASK: &str = "+ new task";

/// The boxes along a row of `f`'s grid: the task's own, then one per tier.
pub const BOXES: usize = 1 + crate::view::TIERS.len();

/// The favorite slot of box `col` on the row of `task`.
pub fn box_slot(task: &str, col: usize) -> String {
    crate::store::slot(task, col.checked_sub(1).and_then(|i| crate::view::TIERS.get(i)).map(|x| x.0))
}

/// A slot's task and the box it has on that task's row.
fn slot_box(slot: &str) -> (&str, usize) {
    let (task, tier) = slot.split_once(':').map_or((slot, None), |(t, x)| (t, Some(x)));
    (task, tier.and_then(|x| crate::view::TIERS.iter().position(|t| t.0 == x)).map_or(0, |i| i + 1))
}

/// Indices of the choice list entries whose label contains `query`, any case; empty when none
/// match, which the overlay says.
pub fn choice_rows(items: &[(String, Effect)], query: &str) -> Vec<usize> {
    let q = query.to_lowercase();
    (0..items.len()).filter(|&i| items[i].0.to_lowercase().contains(&q)).collect()
}

/// The `n`th index after `cur` that `hit` holds for among `len`, before it for a negative `n`:
/// as `step` does, a move stops at the last such one, and one that starts there wraps around to
/// the first. None when that leaves the cursor where it is.
fn nth_hit(cur: usize, len: usize, n: isize, hit: impl Fn(&usize) -> bool) -> Option<usize> {
    let count = n.unsigned_abs();
    let to = if n > 0 {
        (cur + 1..len).filter(&hit).take(count).last().or_else(|| (0..len).find(&hit))
    } else {
        (0..cur.min(len)).rev().filter(&hit).take(count).last().or_else(|| (0..len).rev().find(&hit))
    };
    to.filter(|&r| r != cur)
}

/// Index `i` moved by `n` in a list of `len`: a move stops at an end, and one that starts
/// there wraps around to the other end.
fn step(i: usize, n: isize, len: usize) -> usize {
    let last = len.saturating_sub(1);
    match i.checked_add_signed(n) {
        _ if n > 0 && i >= last => 0,
        _ if n < 0 && i == 0 => last,
        Some(j) => j.min(last),
        None => 0,
    }
}

/// Readline-style editing of a prompt with the cursor at byte `cur`: ← → ^b ^f move by a
/// character, alt-b alt-f ^← ^→ by a word, home end ^a ^e to the ends; backspace and delete drop a
/// character, ^w and alt-d a word, ^u and ^k the line before / after the cursor. `accept` says
/// which characters are typed. True when the key was one of these.
pub fn edit(
    text: &mut String,
    cur: &mut usize,
    code: KeyCode,
    mods: KeyModifiers,
    accept: impl Fn(char) -> bool,
) -> bool {
    let (ctrl, alt) = (mods.contains(KeyModifiers::CONTROL), mods.contains(KeyModifiers::ALT));
    let prev = text[..*cur].chars().next_back().map_or(0, |c| *cur - c.len_utf8());
    let next = text[*cur..].chars().next().map_or(*cur, |c| *cur + c.len_utf8());
    // Start of the word before the cursor, and end of the word after it.
    let word_start = {
        let end = text[..*cur].trim_end().len();
        text[..end].char_indices().rfind(|(_, c)| c.is_whitespace()).map_or(0, |(i, c)| i + c.len_utf8())
    };
    let word_end = {
        let rest = &text[*cur..];
        let start = rest.len() - rest.trim_start().len();
        *cur + start + rest[start..].find(char::is_whitespace).unwrap_or(rest.len() - start)
    };
    let to = match code {
        KeyCode::Left if ctrl || alt => word_start,
        KeyCode::Right if ctrl || alt => word_end,
        KeyCode::Left => prev,
        KeyCode::Right => next,
        KeyCode::Home => 0,
        KeyCode::End => text.len(),
        KeyCode::Char(c) if ctrl || alt => {
            // Where the cursor goes, and what is cut.
            let (to, cut) = match (c, ctrl) {
                ('b', true) => (prev, None),
                ('f', true) => (next, None),
                ('a', true) => (0, None),
                ('e', true) => (text.len(), None),
                ('b', false) => (word_start, None),
                ('f', false) => (word_end, None),
                ('u', true) => (0, Some(0..*cur)),
                ('k', true) => (*cur, Some(*cur..text.len())),
                ('w', true) => (word_start, Some(word_start..*cur)),
                ('d', false) => (*cur, Some(*cur..word_end)),
                _ => return false,
            };
            if let Some(range) = cut {
                text.replace_range(range, "");
            }
            to
        }
        KeyCode::Backspace => {
            text.replace_range(prev..*cur, "");
            prev
        }
        KeyCode::Delete => {
            text.replace_range(*cur..next, "");
            *cur
        }
        KeyCode::Char(c) if accept(c) => {
            text.insert(*cur, c);
            *cur + c.len_utf8()
        }
        _ => return false,
    };
    *cur = to;
    true
}

/// What the shell must do after a key.
#[derive(PartialEq, Debug)]
pub enum Effect {
    Quit,
    /// Leave the TUI, upgrade modelcmp to the newer release and start that one.
    Upgrade,
    Save,
    Open(String),
    Copy(String),
    Refresh,
    /// Run this command in a new terminal window.
    Launch(Vec<String>),
    /// Ask Hugging Face the size of the download of the model `key`, whose repo there is the
    /// second; `App::sized` takes the answer.
    Size(String, String),
    /// A row of `f`'s grid: the model and a task, built in or your own, whose boxes favorite the
    /// model for the task or one tier of it; applied by `App` itself.
    Fav(String, String),
    /// Run the model's favorite for the slot on this harness, or on none in particular; the
    /// items of `v`'s list in the `f` chooser, applied by `App` itself.
    Via(String, String, Option<String>),
    /// Ask the name of a new task of your own for the model; the last of the `f` chooser's
    /// items, applied by `App` itself.
    NewTask(String),
    /// A `view::THEMES` name; the `t` chooser's items, applied by `App` itself.
    Theme(&'static str),
    /// Your default harness, or none; the `H` chooser's items, applied by `App` itself.
    Harness(Option<&'static str>),
    /// The column at this cursor index; the `|` chooser's items, applied by `App` itself.
    Col(usize),
    /// The `B` chooser's items; out of it, the source was switched and its data must be loaded.
    Source(crate::data::Source),
}

/// A mouse action, already mapped to the table by the shell.
#[derive(PartialEq, Debug, Clone, Copy)]
pub enum Mouse {
    /// Wheel: rows to move, negative is up.
    Scroll(isize),
    /// Click on row `n` of the table (an index into `rows`): highlight it.
    Row(usize),
    /// Click on box `col` of row `n` of `f`'s grid: tick it, as space there does.
    Tick(usize, usize),
    /// Double click on the cell of row `n` in the column at cursor index `col`: the name or the
    /// developer opens the details, a number its page in the browser.
    Cell(usize, usize),
    /// Right click on row `n`: mark it, as space does, without moving; a selection becomes marks.
    Mark(usize),
    /// A click on row `n`'s checkbox: toggle its mark, as space does.
    Box(usize),
    /// A click on row `n`'s ☆: pick the tasks it is the favorite for, as `f` does.
    Star(usize),
    /// A click on row `n`'s ✗ box: exclude it or take the exclusion off, that row alone.
    Exclude(usize),
    /// Double click on harness `j` of row `n`'s Via: open it on the model, as `x` and picking it does.
    Harness(usize, usize),
    /// Ctrl click on row `n`: toggle it in the selection on its own, keeping the rest.
    Pick(usize),
    /// Shift click or left drag to row `n`: extend the selection to it as a visual range.
    Extend(usize),
    /// Horizontal wheel: columns to move the cursor, negative is left.
    Cols(isize),
    /// Click on the # header: the first row, as `gg` goes.
    Top,
    /// Click on tab `i`: yours or all alone, and ✓ ★ ✗ on or off, as `S` `F` `E`.
    Tab(usize),
    /// Click on a column header.
    Header(usize),
    /// Click on a header's ▾: open its dropdown.
    Menu(usize),
    /// Click on entry `n` of the open dropdown or choice list.
    Item(usize),
    /// Click outside the open dropdown or choice list: close it. In compare and recommend, a
    /// click on no model, which only clears the status as any click does.
    Outside,
    /// Click outside the open panel (keys, details, compare, recommend): close it, as `esc` does.
    Close,
    /// Click on a status bar hint: press its key.
    Key(KeyCode),
    /// Click on the link in the API key prompt: open where a key is made, the prompt staying.
    Link,
    /// Click on a stop in compare or recommend: move the cursor to it.
    Model(Stop),
    /// Double click on it: do what `enter` does, open the model's details or rank the table by
    /// the task.
    Open(Stop),
    /// Right click on it: move the cursor to it and do what `space` does where it does anything.
    MarkModel(Stop),
}

/// Where the sideways cursor of compare or recommend can be.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Stop {
    /// Model `i` in compare.
    Compare(usize),
    /// Stop `i` of task `t` in recommend: 0 its name and the rest of its row, else its tier `i - 1`.
    Recommend(usize, usize),
    /// Tab `i` on recommend's `among` line.
    Among(usize),
}

/// `provider/model` as opencode takes it, else pi, else omp, when one has the model (`launch_cmd`):
/// the offer you'd pay may be one only another harness reaches. With neither, that offer's, or
/// the bare tag `ollama run` takes, or the repo or file `llama-cli` does.
pub fn model_id(m: &Model, listed: &BTreeMap<String, Vec<String>>) -> String {
    let harness = ["opencode", "pi", "omp"].iter().find_map(|h| launch_cmd(m, h, listed)?.pop());
    let own = |o: &crate::data::Offer| if o.local { o.id.clone() } else { format!("{}/{}", o.provider, o.id) };
    harness.unwrap_or_else(|| m.price().map_or_else(|| m.key.clone(), own))
}

/// The command that starts `harness` on `m`, if the harness has it: opencode, pi and omp take
/// `provider/model` as they listed it (`listed`, pi's `openai-codex/...` for models.dev's `openai/...`),
/// the others (claude, codex, gemini, copilot) the bare model id. ollama runs its tag, llama-cli the
/// repo it downloaded or the file you did.
pub fn launch_cmd(m: &Model, harness: &str, listed: &BTreeMap<String, Vec<String>>) -> Option<Vec<String>> {
    let o = m.offer_via(harness)?;
    if o.local && harness == o.provider {
        let file = o.id.ends_with(".gguf");
        let how = if harness == crate::data::OLLAMA {
            "run"
        } else if file {
            "-m"
        } else {
            "-hf"
        };
        // Asked only for llama.cpp: it is a search of `PATH` (`tools`).
        let bin = (harness == crate::data::LLAMA).then(crate::data::llama_cmd).flatten();
        let bin = bin.map_or_else(|| vec![harness.into()], |c| c.iter().map(|s| s.to_string()).collect());
        return Some([bin, vec![how.into(), o.id.clone()]].concat());
    }
    let id = if matches!(harness, "opencode" | "pi" | "omp") {
        let id = format!("{}/{}", o.provider, o.id);
        let own = listed.get(harness).and_then(|ids| ids.iter().find(|i| crate::data::canonical(harness, i) == id));
        own.cloned().unwrap_or(id)
    } else {
        o.id.clone()
    };
    Some(vec![harness.into(), "--model".into(), id])
}

/// The ways to get a model no runner on this machine has yet, each as its label and the
/// `modelcmp get` that does it in a new terminal: one per runner installed here that lacks the
/// model, then one per runner that can be installed, when no runner here has the model. None for a model with no repo on Hugging Face.
/// `size` is the download's, ` (4.7 GB)`, once known.
fn get_items(m: &Model, size: &str, tools: &Tools) -> Vec<(String, Effect)> {
    // Linux says `… (deleted)` of a binary replaced under a running TUI: the new one is at the path.
    let exe = std::env::current_exe().map_or_else(|_| "modelcmp".into(), |p| p.to_string_lossy().into_owned());
    let exe = exe.trim_end_matches(" (deleted)").to_string();
    let item = |(h, how): (&str, &str)| {
        let cmd = [&exe, "get", &m.key, "--via", h, "--pause"].map(String::from).to_vec();
        (format!("{h} {how}{size}"), Effect::Launch(cmd))
    };
    getters(m, tools).map(item).collect()
}

/// The runners `get_items` gets the model by, each with how it is said.
fn getters<'a>(m: &'a Model, (have, install): &'a Tools) -> impl Iterator<Item = (&'static str, &'static str)> + 'a {
    let lacking = have.iter().filter(|h| !m.via.iter().any(|v| v == *h));
    let got = lacking.map(|h| (*h, "download and run"));
    // Not for one a runner here has: nothing is installed for a model that already runs.
    let new = install.iter().filter(|_| !m.here()).map(|h| (*h, "install, download and run"));
    got.chain(new).filter(|_| m.hf_repo().is_some())
}

/// The runners installed here, ollama then llama.cpp, and the ones that are not and can be
/// installed.
pub type Tools = (Vec<&'static str>, Vec<&'static str>);

/// `Tools` as this machine has them now: a search of `PATH`, slow where it holds Windows's
/// folders under WSL, so made at start and by a refresh's thread, not for each model.
pub fn tools() -> Tools {
    use crate::data::{LLAMA, OLLAMA, has, install_cmd};
    let have: Vec<_> = [OLLAMA, LLAMA].into_iter().filter(|h| has(h)).collect();
    let install = [LLAMA, OLLAMA].into_iter().filter(|h| !have.contains(h) && install_cmd(h).is_some()).collect();
    (have, install)
}

/// What Hugging Face said of a model's download, which `x` offers.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Download {
    /// Asked, and no answer yet.
    Awaited,
    /// No answer came: `x` asks again.
    NoAnswer,
    /// There is no GGUF copy to download: `x` offers none. Kept from one run to the next, for a
    /// week (`data::load_gone`) or until a refresh you ask for.
    Gone,
    /// Its size in bytes, which `x` says: kept from one run to the next.
    Size(u64),
}

/// Said of a model with no GGUF copy on Hugging Face.
fn no_copy(name: &str) -> String {
    format!("Hugging Face has no GGUF copy of {name} to download")
}

/// The key of the model a `modelcmp get` of `get_items` downloads.
fn got(e: &Effect) -> Option<&str> {
    match e {
        Effect::Launch(c) if c.len() > 2 && c[1] == "get" => Some(&c[2]),
        _ => None,
    }
}

/// A task's line as (index into `Data::models`, score), and the models on it only for being
/// favorites (`view::task_line`).
type Front = (Vec<(usize, f64)>, Vec<usize>);

pub struct App {
    pub data: Data,
    /// `vals[i][c]` is `COLS[c]` of `data.models[i]`, computed once per data load: a price is a
    /// search through the offers, too slow to repeat for every model on every key.
    pub vals: Vec<[Option<f64>; COLS.len()]>,
    /// The benchmark each task's column shows, picked in its dropdown; none for the task's
    /// score. `fill_col` puts the benchmark's scores in `vals`.
    bench: [Option<&'static str>; COLS.len()],
    /// `Model::listed` of each model, cached as `vals`.
    pub listed: Vec<bool>,
    /// Widest shown value of each column over every model, so the layout holds when filtering.
    pub widths: [usize; COLS.len()],
    /// Best and worst value of each column among `rows`; none when they all agree, or it is not ranked.
    pub ext: [Option<(f64, f64)>; COLS.len()],
    pub store: Store,
    pub all: bool,
    /// Whether the user has access to any model, set with the data: with none, every model is in reach.
    any_available: bool,
    /// Key of the model details show, held apart from the rows, which may drop it meanwhile.
    detail: String,
    /// Column under the cursor: 0 is the name, 1 the developer, then `COLS`.
    pub col: usize,
    pub sort_col: usize,
    pub descending: bool,
    /// (column, min, max) filters set with `>` and `<`.
    pub bounds: Vec<(usize, f64, f64)>,
    /// Developers and countries picked from the Dev dropdown; empty is any.
    pub dev: Vec<String>,
    /// Harnesses picked from the Via dropdown, and of `VIA_KINDS`; empty is any.
    pub via: Vec<String>,
    /// What is known of each model's download, by key (`Download`).
    downloads: std::collections::HashMap<String, Download>,
    tools: Tools,
    /// Task whose price frontier the table shows, picked in the recommend overlay.
    pub task: Option<&'static Task>,
    /// Cursor in the recommend overlay: the task, and on its row 0 for its name, else the tier
    /// `task_sel - 1` of `TIERS`.
    pub task_cur: usize,
    pub task_sel: usize,
    /// The cursor on recommend's `among` line instead, the models it ranks: on the tab `among`,
    /// one of yours, all, selected and favorites.
    pub among: Option<usize>,
    pub query: String,
    /// The query matched nothing as typed, so it is matched allowing a typo per word.
    pub typos: bool,
    /// Indices into `data.models`, in display order.
    pub rows: Vec<usize>,
    /// Per task of `TASKS`. Set by `rebuild`, as each is a pass over every model and recommend
    /// draws them all.
    fronts: Vec<Front>,
    pub table: TableState,
    /// How many marks are of models the data has: the ones `S`, `C` and `u` act on, as a
    /// refresh or a source switch can drop a selected model while its mark stays. Set by
    /// `rebuild`, so a frame does not scan every model for it.
    pub marked_shown: usize,
    /// How many of them are out of reach whatever `a`, which yours shows too: its tab's `+N`.
    pub marked_out: usize,
    /// Whether `F` and `E` have a model to show, set by `rebuild` as the marks' count is: one
    /// the data no longer has is none, one out of reach still is.
    fav_shown: bool,
    excluded_shown: bool,
    /// The one of `S` `F` `E` that is on, as its tab in `TABS`: its models only.
    pub only: Option<usize>,
    /// Row where `v` started a visual range; the range runs to the cursor.
    pub visual: Option<usize>,
    /// Rows picked one by one with ctrl click; selected along with the visual range.
    pub picked: Vec<usize>,
    pub view: View,
    pub input: Input,
    /// Scroll offset of the detail, compare and help views. The renderer clamps it.
    pub scroll: u16,
    /// The model under the cursor in the compare view, moved with `h l`.
    pub compare_sel: usize,
    /// First model the compare view shows, set by the renderer so the cursor stays in view.
    pub compare_x: usize,
    /// Where the renderer drew each model of the compare or recommend overlay, and the click on it.
    pub spots: Vec<(ratatui::layout::Rect, Stop)>,
    /// The box of the open panel as last drawn, for a click outside it to close it.
    pub panel: Option<ratatui::layout::Rect>,
    /// Filter on the rows of the compare or help overlay, typed with `/` in either.
    pub overlay_query: String,
    /// Rows visible in the body, set by the renderer; drives page movement.
    pub page: u16,
    /// First of the columns right of Dev shown when they do not all fit; the renderer keeps
    /// the selected one in view.
    pub hscroll: usize,
    pub count: usize,
    /// The count before a first `g`, waiting for the second one of `gg`.
    g_pending: Option<usize>,
    pub status: String,
    /// The status is an error, shown in red.
    pub failed: bool,
    /// The status says why a key or a click did nothing (`refuse`): red, but no error to keep.
    refused: bool,
    pub refreshing: bool,
    /// How far the refresh under way is (`data::progress`), for the frame.
    pub progress: String,
    /// The last refresh failed; stays in the frame until one succeeds.
    pub refresh_failed: bool,
    /// The share `%` turns back on: `--cache`, or an agent's when that was 0.
    pub cache_on: f64,
    /// The `%` hint while no input is cached, naming `cache_on`.
    pub cache_hint: &'static str,
    /// The TUI opened on the `B` chooser, with no data, as no source was ever picked: closing it
    /// picks the default. Not so with `--source`, which picked one for the run.
    pub first_start: bool,
    /// What `q` asked to quit from, which any key but a second `q` goes back to: an open list.
    asked_from: Input,
    /// The terminal's background colour, when it told at start (`tui::terminal_bg`): with the
    /// terminal's own colours, what a marked row's faint fill is mixed from.
    pub term_bg: Option<u32>,
}

impl App {
    pub fn new(data: Data, store: Store) -> Self {
        let mut app = App {
            data: Data::default(),
            vals: vec![],
            bench: [None; COLS.len()],
            listed: vec![],
            widths: [0; COLS.len()],
            ext: [None; COLS.len()],
            store,
            all: false,
            any_available: false,
            detail: String::new(),
            col: default_sort().0,
            sort_col: default_sort().0,
            descending: default_sort().1,
            bounds: vec![],
            dev: vec![],
            via: vec![],
            downloads: Default::default(),
            tools: tools(),
            task: None,
            task_cur: 0,
            task_sel: 0,
            among: None,
            query: String::new(),
            typos: false,
            rows: vec![],
            fronts: vec![],
            table: TableState::default().with_selected(0),
            marked_shown: 0,
            marked_out: 0,
            fav_shown: false,
            excluded_shown: false,
            only: None,
            visual: None,
            picked: vec![],
            view: View::Table,
            input: Input::None,
            scroll: 0,
            compare_sel: 0,
            compare_x: 0,
            spots: vec![],
            panel: None,
            overlay_query: String::new(),
            page: 20,
            hscroll: 0,
            count: 0,
            g_pending: None,
            status: String::new(),
            failed: false,
            refused: false,
            refreshing: false,
            progress: String::new(),
            refresh_failed: false,
            cache_on: 0.0,
            cache_hint: "",
            first_start: false,
            asked_from: Input::None,
            term_bg: None,
        };
        let start = crate::data::cached();
        app.cache_on = if start > 0.0 { start } else { crate::data::AGENT_CACHED };
        // ponytail: leaked once per App, which the TUI makes once; hints are &'static str.
        app.cache_hint = Box::leak(format!("% {:.0}% cached", app.cache_on * 100.0).into_boxed_str());
        app.set_cols();
        app.set_data(data);
        app
    }

    /// Replace the data and recompute everything derived from it.
    pub fn set_data(&mut self, data: Data) {
        self.listed = data.models.iter().map(Model::listed).collect();
        self.any_available = data.any_available();
        // The rows index the old data: point them at the same models in the new, so `rebuild`
        // keeps the cursor and the highlight on them; a model gone points nowhere. Data taken out
        // to change in place (`reprice`) comes back in its order.
        if !self.data.models.is_empty() {
            let at: std::collections::HashMap<&str, usize> =
                data.models.iter().enumerate().map(|(i, m)| (m.key.as_str(), i)).collect();
            let old = &self.data.models;
            for r in &mut self.rows {
                *r = old.get(*r).and_then(|m| at.get(m.key.as_str())).copied().unwrap_or(usize::MAX);
            }
        }
        self.data = data;
        // The other source's columns come and go with its data.
        crate::data::set_lent(self.data.lent);
        self.off_hidden();
        self.fill();
        // A refresh that could not ask for the newest release leaves nothing to upgrade to.
        if self.input == Input::Upgrade && self.data.update().is_none() {
            self.input = Input::None;
        }
        self.compare_sel = self.compare_sel.min(self.marked_models().len().saturating_sub(1));
        self.rebuild();
    }

    /// The column values and their widths, from the data and the benchmarks picked.
    fn fill(&mut self) {
        self.vals = vec![[None; COLS.len()]; self.data.models.len()];
        (0..COLS.len()).for_each(|c| self.fill_col(c));
    }

    /// `fill` for `COLS[c]` alone: a pick changes one column, and the prices are slow to search.
    fn fill_col(&mut self, c: usize) {
        let pick = self.bench[c];
        for (v, m) in self.vals.iter_mut().zip(&self.data.models) {
            v[c] = match pick {
                Some(b) => m.scores.get(b).map(|s| s * 100.0),
                None => (COLS[c].get)(m),
            };
        }
        let shown = self.vals.iter().zip(&self.listed).filter_map(|(v, &l)| Some(shown(c, v[c]?, l)));
        self.widths[c] = shown.map(|s| s.chars().count()).max().unwrap_or(0);
    }

    /// Entry `i` of a task column's dropdown is what the column shows, the first or the picked
    /// one again the task's score. Bounds on it go, typed for what it showed; not when the
    /// entry is what it shows already.
    fn pick_bench(&mut self, col: usize, i: usize) {
        let Some((c, (_, benches))) = col.checked_sub(TEXT).zip(col_benches(col)) else { return };
        let pick = i.checked_sub(1).and_then(|i| benches.get(i).copied()).filter(|b| self.bench[c] != Some(b));
        if pick == self.bench[c] {
            return;
        }
        self.bench[c] = pick;
        self.bounds.retain(|b| b.0 != col);
        self.fill_col(c);
    }

    /// Every task column back to its task's score, and the bounds typed for a picked benchmark
    /// gone. Without `refill` the values wait for the `set_data` that follows.
    fn drop_benches(&mut self, refill: bool) {
        for c in 0..COLS.len() {
            if self.bench[c].take().is_some() {
                self.bounds.retain(|b| b.0 != c + TEXT);
                if refill {
                    self.fill_col(c);
                }
            }
        }
    }

    /// The benchmark picked for the task column at cursor index `col`, shown in place of its score.
    pub fn col_bench(&self, col: usize) -> Option<&'static str> {
        *self.bench.get(col.checked_sub(TEXT)?)?
    }

    /// Header of the column at cursor index `col`: a task column's picked benchmark, else `col_name`.
    pub fn col_name(&self, col: usize) -> &'static str {
        self.col_bench(col).unwrap_or_else(|| base_col_name(col))
    }

    /// What the column at cursor index `col` means.
    pub fn col_about(&self, col: usize) -> String {
        match self.col_bench(col) {
            Some(_) => of_source("score on this benchmark alone, 0-100"),
            None => base_col_about(col),
        }
    }

    /// Recompute everything that depends on `data::cached()`: Value and the column values.
    pub fn reprice(&mut self) {
        let mut data = std::mem::take(&mut self.data);
        crate::fit::add_value(&mut data.models);
        self.set_data(data);
    }

    /// Value of the column at cursor index `col` for `data.models[i]`.
    fn val(&self, i: usize, col: usize) -> Option<f64> {
        *self.vals[i].get(col.checked_sub(TEXT)?)?
    }

    fn selected(&self) -> usize {
        self.table.selected().unwrap_or(0).min(self.rows.len().saturating_sub(1))
    }

    fn select(&mut self, i: usize) {
        self.table.select(Some(i.min(self.rows.len().saturating_sub(1))));
    }

    /// Moves the cursor to the `n`th row below it whose model `hit` holds for, up for a negative
    /// `n`: as `step` does, a move stops at the last such row, and one that starts there wraps
    /// around to the first. When the cursor stays, says no other `what` model is shown.
    fn jump(&mut self, n: isize, what: &str, hit: impl Fn(&Self, &Model) -> bool) {
        let cur = self.selected();
        match nth_hit(cur, self.rows.len(), n, |r| hit(self, &self.data.models[self.rows[*r]])) {
            Some(r) => self.select(r),
            None => {
                let other = if self.current().is_some_and(|m| hit(self, m)) { "other " } else { "" };
                self.refuse(format!("no {other}{what} model is shown"));
            }
        }
    }

    /// `]` `[` in `f`'s grid and the dropdowns: the cursor to the `n`th ticked entry
    /// below it, up for a negative `n`, as `jump` moves it in the table.
    fn jump_ticked(&mut self, n: isize) {
        let (ticked, what): (Vec<bool>, _) = match &self.input {
            Input::Choose { items, list, .. } => {
                // A row with a box ticked.
                let on = |i: usize| match &items[i].1 {
                    Effect::Fav(k, t) => (0..BOXES).any(|c| self.store.favorite(&box_slot(t, c)) == Some(k)),
                    Effect::Col(c) => !hidden(*c),
                    _ => false,
                };
                let what = if self.choosing_favs() { "task" } else { "column" };
                (choice_rows(items, &list.query).into_iter().map(on).collect(), what)
            }
            Input::Menu { col, items, list } => {
                // The ticks the dropdown draws: "any" has one while nothing is picked.
                let bench: Vec<String> = self.col_bench(*col).into_iter().map(String::from).collect();
                let picked = match *col {
                    1 => &self.dev,
                    VIA => &self.via,
                    _ => &bench,
                };
                let on = |i: usize| match i {
                    _ if *col == PRICE => self.price_level() == i.checked_sub(1),
                    0 => picked.is_empty(),
                    _ => picked.contains(&items[i].0),
                };
                (menu_rows(items, &list.query).into_iter().map(on).collect(), "entry")
            }
            _ => return,
        };
        let Some((sel, _)) = self.list() else { return };
        let cur = *sel;
        match nth_hit(cur, ticked.len(), n, |r| ticked[*r]) {
            Some(r) => *sel = r,
            None => {
                let other = if ticked.get(cur) == Some(&true) { "other " } else { "" };
                self.refuse(format!("no {other}ticked {what} is shown"));
            }
        }
    }

    /// What `/` edits: the compare and help overlays filter their own rows, the table its models.
    pub fn search_target(&mut self) -> &mut String {
        if self.overlay_search() { &mut self.overlay_query } else { &mut self.query }
    }

    /// Whether `/` filters the open overlay's rows instead of the table's models.
    pub fn overlay_search(&self) -> bool {
        matches!(self.view, View::Compare | View::Help)
    }

    /// The model under the cursor: the row in the table and details, the column in compare,
    /// the one a tier of the task picks in recommend, where a task's name has none.
    pub fn current(&self) -> Option<&Model> {
        if self.view == View::Compare {
            let marked = self.marked_models();
            return marked.get(self.compare_sel.min(marked.len().saturating_sub(1))).copied();
        }
        if self.view == View::Recommend {
            if self.among.is_some() {
                return None;
            }
            return (*self.tier_picks(self.task_cur).get(self.task_sel.checked_sub(1)?)?).map(|e| e.0);
        }
        if matches!(self.view, View::Detail(_)) {
            return self.data.models.iter().find(|m| m.key == self.detail);
        }
        self.rows.get(self.selected()).map(|&i| &self.data.models[i])
    }

    /// How many stops the open overlay's sideways cursor moves over: compare's models, the
    /// task's name and its tiers in recommend.
    fn across_len(&self) -> usize {
        match self.view {
            View::Compare => self.marked_shown,
            _ if self.one_pick(self.task_cur) => 2,
            _ => 1 + TIERS.len(),
        }
    }

    /// Whether recommend's task `i` has one pick and not one per tier: "value", a rank with no
    /// score for a tier to be near the best on (`fit::add_lag`), unless your favorites give its
    /// tiers models of their own.
    pub fn one_pick(&self, i: usize) -> bool {
        if TASKS.get(i).is_none_or(|t| t.name != "value") {
            return false;
        }
        let picks = self.tier_picks(i).map(|p| p.map(|e| &e.0.key));
        picks.iter().all(|p| *p == picks[0])
    }

    /// What each tier of recommend's task `i` picks, as `--tier` does, with its score: your
    /// favorite for the tier, else for the task, else on a built-in task the tier's pick of its
    /// frontier. None where the task has no model for the tier.
    pub fn tier_picks(&self, i: usize) -> [Option<(&Model, f64)>; 3] {
        let (name, front, off) = match TASKS.get(i) {
            Some(t) => {
                let off =
                    self.cached(t).map_or(vec![], |f| f.1.iter().map(|&i| self.data.models[i].key.as_str()).collect());
                (t.name, self.task_frontier(t), off)
            }
            // A task of your own has no ranking: its models are on its line only as favorites.
            None => {
                let Some(&name) = self.store.custom_tasks().get(i - TASKS.len()) else { return [None; 3] };
                let line = self.custom_line(name);
                (name, line.iter().map(|&m| (m, f64::NAN)).collect(), line.iter().map(|m| m.key.as_str()).collect())
            }
        };
        TIERS.map(|x| tier_pick(&front, &off, &self.store, name, x.0))
    }

    /// The built-in task under recommend's cursor, which runs over `TASKS` and then your own.
    fn cur_task(&self) -> Option<&'static Task> {
        TASKS.get(self.task_cur)
    }

    /// The task of your own under recommend's cursor.
    pub fn custom_at(&self) -> Option<&str> {
        self.store.custom_tasks().get(self.task_cur.checked_sub(TASKS.len())?).copied()
    }

    /// How many tasks recommend lists: the built-in ones and your own.
    pub fn task_count(&self) -> usize {
        TASKS.len() + self.store.custom_tasks().len()
    }

    /// The line of a task of your own in recommend: the models you gave it and its tiers that
    /// are ones to use, cheapest first.
    pub fn custom_line(&self, task: &str) -> Vec<&Model> {
        custom_line(self.data.models.iter().filter(|m| self.usable(m)), &self.store, task)
    }

    /// The sideways model cursor of the open overlay: compare's, else recommend's.
    fn across_sel(&mut self) -> &mut usize {
        if self.view == View::Compare { &mut self.compare_sel } else { &mut self.task_sel }
    }

    /// Rows of the visual range, in order.
    pub fn visual_range(&self) -> Option<std::ops::RangeInclusive<usize>> {
        let (a, b) = (self.visual?, self.selected());
        Some(a.min(b)..=a.max(b))
    }

    /// Whether rows are selected: a visual range, picked rows, or both.
    pub fn selecting(&self) -> bool {
        self.visual.is_some() || !self.picked.is_empty()
    }

    /// Whether row `k` is in the selection.
    pub fn is_selected(&self, k: usize) -> bool {
        self.visual_range().is_some_and(|r| r.contains(&k)) || self.picked.contains(&k)
    }

    fn deselect(&mut self) {
        self.visual = None;
        self.picked.clear();
    }

    /// Keys of the models a command acts on: in the table the highlighted rows, else the current
    /// one alone, marked or not (`V` highlights the marked ones, to act on them all).
    fn targets(&self) -> Vec<String> {
        if self.view == View::Table && self.selecting() {
            let key = |k: usize| self.data.models[self.rows[k]].key.clone();
            return (0..self.rows.len()).filter(|&k| self.is_selected(k)).map(key).collect();
        }
        self.current().map(|m| vec![m.key.clone()]).unwrap_or_default()
    }

    /// Sets a flag on every target, or clears it when all of them have it. Ends a visual range
    /// and says what happened when it was more than one model.
    fn flag(
        &mut self,
        has: fn(&Store, &str) -> bool,
        toggle: fn(&mut Store, &str),
        [done, undone]: [&str; 2],
    ) -> Option<Effect> {
        let keys = self.targets();
        let on = !keys.iter().all(|k| has(&self.store, k));
        for k in &keys {
            if has(&self.store, k) != on {
                toggle(&mut self.store, k);
            }
        }
        if keys.len() > 1 {
            self.status = format!("{} {} models", if on { done } else { undone }, keys.len());
        }
        self.deselect();
        self.rebuild_in_place();
        (!keys.is_empty()).then_some(Effect::Save)
    }

    pub fn marked_models(&self) -> Vec<&Model> {
        self.store.marked.iter().filter_map(|k| self.data.models.iter().find(|m| m.key == *k)).collect()
    }

    /// Whether any model is marked, so `S` has something to show: a kept mark of a model the
    /// data no longer has is none.
    pub fn any_marked(&self) -> bool {
        self.marked_shown > 0
    }

    /// Whether the table row shows a ★, so `F` keeps it: `starred` without recommend's cursor,
    /// which the table ignores.
    fn is_fav(&self, key: &str) -> bool {
        self.store.is_favorite(self.task, key)
    }

    /// Whether tab `i` has anything to show.
    pub fn tab_has(&self, i: usize) -> bool {
        match i {
            YOURS => !self.no_access(),
            MARKED => self.any_marked(),
            FAV => self.fav_shown,
            EXCLUDED => self.excluded_shown,
            // It takes two models.
            COMPARE => self.marked_shown >= 2,
            _ => true,
        }
    }

    /// Whether tab `i` is on: a panel's alone while it is open, else yours or all while none of
    /// `S` `F` `E` is, and the one of those that is.
    pub fn tab_on(&self, i: usize) -> bool {
        let panel = match (&self.input, &self.view) {
            (Input::Choose { kind: Kind::Theme, .. }, _) => Some(THEME),
            (_, View::Recommend) => Some(RECOMMEND),
            (_, View::Compare) => Some(COMPARE),
            (_, View::Help) => Some(HELP_TAB),
            _ => None,
        };
        match i {
            _ if panel.is_some() || i >= RECOMMEND => panel == Some(i),
            YOURS | ALL => self.only.is_none() && (i == ALL) == (self.all || self.no_access()),
            _ => self.only == Some(i),
        }
    }

    /// The next tab with something to show, past the last back to the first; `back` the one
    /// before. It leaves the theme list, a tab as the panels are.
    fn step_tab(&mut self, back: bool) -> Option<Effect> {
        let len = TABS.len();
        let cur = (0..len).rev().find(|&i| self.tab_on(i)).unwrap_or(0);
        let step = if back { len - 1 } else { 1 };
        let i = (1..len).map(|k| (cur + k * step) % len).find(|&i| self.tab_has(i))?;
        self.input = Input::None;
        self.set_tab(i)
    }

    /// Show tab `i` alone, leaving a panel or the details for the table; one with nothing to
    /// show says so, as its key does.
    fn set_tab(&mut self, i: usize) -> Option<Effect> {
        if self.tab_on(i) && i >= RECOMMEND {
            return None;
        }
        let view = std::mem::replace(&mut self.view, View::Table);
        let key = KeyCode::Char(TABS[i].1);
        // A panel's key opens it from the table; compare with fewer than 2 selected says how to
        // select them, as `C` does.
        if i >= RECOMMEND {
            // From a panel the highlight is set aside: it is not what gets compared there, as
            // `C` from the panel leaves the selected models alone.
            let held = (view != View::Table).then(|| (self.visual.take(), std::mem::take(&mut self.picked)));
            let effect = self.table_key(key, 1);
            if let Some(held) = held {
                (self.visual, self.picked) = held;
            }
            return effect;
        }
        // An empty one's key says why as it does in the table, and the panel stays open.
        if !self.tab_has(i) {
            let effect = self.table_key(key, 1);
            self.view = view;
            return effect;
        }
        // With access to none `a` stays off, as its key leaves it: all is every model already.
        if i < MARKED {
            self.all = i == ALL && !self.no_access();
        }
        self.only = (i >= MARKED).then_some(i);
        self.rebuild();
        None
    }

    /// Whether the open choice list is `f`'s tasks, where space or enter ticks one and the list
    /// stays open.
    pub fn choosing_favs(&self) -> bool {
        matches!(self.input, Input::Choose { kind: Kind::Fav, .. })
    }

    /// Whether the open choice list is `|`'s columns, ticked as `f`'s tasks are.
    pub fn choosing_cols(&self) -> bool {
        matches!(self.input, Input::Choose { kind: Kind::Cols, .. })
    }

    /// The theme under the cursor of the open `t` list, which the screen previews.
    pub fn theme_preview(&self) -> Option<&'static str> {
        match &self.input {
            Input::Choose { items, list, .. } => {
                match choice_rows(items, &list.query).get(list.sel).map(|&i| &items[i]) {
                    Some((_, Effect::Theme(name))) => Some(name),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// `f`'s grid for the model `key`: a row per task, built in then your own, and the entry
    /// that names a new one. The key, not the current model, since a tick can move the rows
    /// under the grid. The boxes are drawn from the store, so they follow any change to it.
    fn fav_items(&self, key: &str) -> Vec<(String, Effect)> {
        let tasks = TASKS.iter().map(|t| t.name).chain(self.store.custom_tasks());
        tasks
            .map(|t| (t.to_string(), Effect::Fav(key.to_string(), t.to_string())))
            .chain([(NEW_TASK.to_string(), Effect::NewTask(key.to_string()))])
            .collect()
    }

    /// Models passing every filter but the frontier, ignoring the ones on column `skip` except
    /// Price's minimum (its dropdown only replaces the maximum; a task's drops both bounds), so
    /// a dropdown can count what each of its entries would show.
    fn filtered(&self, skip: usize) -> impl Iterator<Item = (usize, &Model)> {
        // A selected model shows even out of reach, so it can be compared, and `S` `F` `E` show
        // theirs whatever `a`: the same models from yours as from all.
        self.data.models.iter().enumerate().filter(move |&(i, m)| {
            (self.only.is_some() || self.in_reach(m) || self.store.is_marked(&m.key))
                && hits(
                    &self.query,
                    [&m.name, &m.developer, &m.via.join(", "), self.store.note(&m.key).unwrap_or("")],
                    self.typos,
                )
                .is_some()
                && match self.only {
                    Some(MARKED) => self.store.is_marked(&m.key),
                    Some(FAV) => self.is_fav(&m.key),
                    Some(EXCLUDED) => self.store.is_excluded(&m.key),
                    _ => true,
                }
                && (skip == 1 || self.dev.is_empty() || m.devs().any(|d| self.dev.iter().any(|x| x == d)))
                && (skip == VIA
                    || self.via.is_empty()
                    || self.via.iter().any(|h| self.via_labels(m).contains(&h.as_str())))
                && self
                    .bounds
                    .iter()
                    .filter(|b| b.0 != skip || skip == PRICE && b.1.is_finite())
                    .all(|&(c, lo, hi)| self.val(i, c).is_some_and(|v| v >= lo && v <= hi))
        })
    }

    /// Recompute the visible rows after any filter, sort or data change, keeping the selection.
    pub fn rebuild(&mut self) {
        let marked = || self.data.models.iter().filter(|m| self.store.is_marked(&m.key));
        (self.marked_shown, self.marked_out) = (marked().count(), marked().filter(|m| !self.accessible(m)).count());
        // Unmarking the last marked model, or a refresh dropping it, leaves S (and unfavoriting
        // the last, F) for every model rather than an empty table.
        // One out of reach shows in them too, as `filtered` keeps it.
        let fav = self.data.models.iter().any(|m| self.is_fav(&m.key));
        let excluded = self.data.models.iter().any(|m| self.store.is_excluded(&m.key));
        (self.fav_shown, self.excluded_shown) = (fav, excluded);
        self.only = self.only.filter(|&i| [self.any_marked(), fav, excluded][i - MARKED]);
        // The rows move, so the selection follows its models by key and drops the ones filtered out.
        let key_of = |k: usize| self.rows.get(k).and_then(|&i| self.data.models.get(i)).map(|m| m.key.clone());
        let anchor = self.visual.and_then(key_of);
        let range: Vec<String> = self.visual_range().into_iter().flatten().filter_map(key_of).collect();
        let picked: Vec<String> = self.picked.iter().filter_map(|&k| key_of(k)).collect();
        let keep = key_of(self.selected());
        let matching = |app: &Self| -> Vec<usize> { app.filtered(usize::MAX).map(|(i, _)| i).collect() };
        self.typos = false;
        let mut rows = matching(self);
        // Typos only when nothing matches exactly, so "codex" does not also show Codestral.
        if rows.is_empty() && !self.query.trim().is_empty() {
            self.typos = true;
            rows = matching(self);
        }
        // A task of your own is gone with its model, from under recommend's cursor too.
        self.task_cur = self.task_cur.min(self.task_count() - 1);
        // Before a task keeps only its line: the models every task's line is drawn from. `E`
        // narrows none, as an excluded model is never recommended, and its exact matches come
        // before the typos the table's rows took.
        let mut unnarrowed;
        let pool = if self.only == Some(EXCLUDED) {
            let typos = std::mem::take(&mut self.typos);
            self.only = None;
            unnarrowed = matching(self);
            if unnarrowed.is_empty() && typos {
                self.typos = true;
                unnarrowed = matching(self);
            }
            (self.only, self.typos) = (Some(EXCLUDED), typos);
            &unnarrowed
        } else {
            &rows
        };
        self.fronts = TASKS.iter().map(|t| self.front(t, pool)).collect();
        let ms = &self.data.models;
        // `F` and `E` keep theirs off the line too: one out of reach or excluded is on none.
        if let Some(t) = self.task.filter(|_| !matches!(self.only, Some(FAV | EXCLUDED))) {
            // The same line the recommend panel and `list --task` show.
            let front = &self.fronts[TASKS.iter().position(|x| x.name == t.name).unwrap_or(0)].0;
            rows.retain(|i| front.iter().any(|(k, _)| k == i));
        }
        if numeric(self.sort_col).is_some() {
            // Names breaking ties.
            rows.sort_by(|&a, &b| {
                by_value(self.val(a, self.sort_col), self.val(b, self.sort_col), self.descending)
                    .then_with(|| ms[a].name.cmp(&ms[b].name))
            });
        } else {
            // By name, or by developer or access with names breaking ties.
            let text = |m: &Model| match self.sort_col {
                1 => m.developer.to_lowercase(),
                VIA => self.shown_via(m).join(", "),
                NOTES => self.store.note(&m.key).unwrap_or("").to_lowercase(),
                _ => String::new(),
            };
            // Under Via, the ones that read alike go by the size their `↓` says, as all do with
            // `not available` picked.
            let marked = |m: &Model| self.sort_col == VIA && getters(m, &self.tools).next().is_some();
            let size = |m: &Model| self.size(m).filter(|_| marked(m));
            // Blanks last either way, as with numbers.
            rows.sort_by_cached_key(|&i| {
                let (t, size) = (text(&ms[i]), size(&ms[i]));
                (t.is_empty() != self.descending, t, size.is_none() != self.descending, size, ms[i].name.to_lowercase())
            });
            if self.descending {
                rows.reverse();
            }
        }
        // Among the models to use: a muted row is grey throughout, so it holds no extreme, nor does a list price.
        self.ext = std::array::from_fn(|c| {
            if !COLS[c].ranked {
                return None;
            }
            let own = |r: usize| !(self.listed[r] && on_price(c));
            let mut it = rows.iter().filter(|&&r| !self.muted(&ms[r]) && own(r)).filter_map(|&r| self.vals[r][c]);
            let first = it.next()?;
            let (lo, hi) = it.fold((first, first), |(lo, hi), v| (lo.min(v), hi.max(v)));
            (lo != hi).then_some(if COLS[c].lower_better { (lo, hi) } else { (hi, lo) })
        });
        self.rows = rows;
        let pos = |k: &str| self.rows.iter().position(|&i| self.data.models[i].key == k);
        let sel = keep.and_then(|k| pos(&k)).unwrap_or(0);
        self.visual = anchor.and_then(|k| pos(&k));
        self.picked = picked.iter().filter_map(|k| pos(k)).collect();
        // A sort or a refresh can part the range's models: then they are picked one by one, as a
        // range between its two ends would take in models that were never highlighted.
        let at: Vec<usize> = range.iter().filter_map(|k| pos(k)).collect();
        let run = self.visual.is_some_and(|a| {
            let span = a.min(sel)..=a.max(sel);
            at.len() == span.clone().count() && at.iter().all(|p| span.contains(p))
        });
        if !run {
            self.visual = None;
            for k in at {
                if !self.picked.contains(&k) {
                    self.picked.push(k);
                }
            }
        }
        self.select(sel);
    }

    /// Rebuild after a selection or flag changes: a row that drops out, under `S` or for being
    /// out of reach, leaves the cursor where it was, as selecting never moves it.
    pub fn rebuild_in_place(&mut self) {
        let (at, row) = (self.selected(), self.rows.get(self.selected()).copied());
        self.rebuild();
        if self.rows.get(self.selected()).copied() != row {
            self.select(at);
        }
    }

    /// A background refresh finished.
    pub fn refreshed(&mut self, res: Result<Data, Failure>, tools: Tools) {
        // A runner may be installed since, and the sizes that did not come are asked for again.
        self.tools = tools;
        self.downloads.retain(|_, d| *d != Download::NoAnswer);
        self.refreshing = false;
        self.refresh_failed = res.is_err();
        match res {
            Ok(mut d) => {
                // An error still standing, as the start's of an unreadable user.json, is not
                // replaced by the news that the refresh went well, nor by its warning. Why a
                // key did nothing is. The news replaces no other message either: what a switch
                // of source said, `B to change`, stays when its download ends.
                let standing = self.failed && !self.refused;
                match d.warning.take() {
                    Some(w) if standing => self.status = format!("{}; {w}", self.status),
                    Some(w) => self.report(Err(w)),
                    None if self.status.is_empty() || (self.failed && self.refused) => {
                        self.report(Ok("data refreshed".into()));
                    }
                    None => {}
                }
                self.set_data(d);
            }
            // One from the environment is yours to change there, as a saved one would not replace it.
            Err(e @ Failure::BadKey) if crate::data::aa_key_env().is_some() => {
                self.report(Err(format!("{e} in {}", crate::data::AA_KEY_ENV)));
            }
            // No key, or the saved one was turned down: ask for one, unless you are typing or
            // choosing something else, which the prompt would throw away; `B` asks then.
            Err(e @ (Failure::BadKey | Failure::NoKey)) => {
                if self.input == Input::None {
                    self.report(Err(e.to_string()));
                    self.input = Input::Key { text: String::new(), cur: 0, wrong: e == Failure::BadKey };
                } else {
                    self.report(Err(format!("{e}; B then Artificial Analysis to enter one")));
                }
            }
            Err(e) => self.report(Err(format!("refresh failed: {e}"))),
        }
    }

    /// The outcome of an action in the status bar, an error in red.
    pub fn report(&mut self, res: Result<String, String>) {
        (self.failed, self.refused) = (res.is_err(), false);
        self.status = res.unwrap_or_else(|e| e);
    }

    /// Why a key or a click did nothing, in red as an error is. Unlike one, the outcome of a
    /// refresh replaces it.
    fn refuse(&mut self, why: impl Into<String>) {
        self.report(Err(why.into()));
        self.refused = true;
    }

    /// The `|` chooser: every column right of Dev that has something to show, ticked while shown,
    /// on the one under the cursor.
    fn ask_cols(&mut self) {
        let cols = (TEXT..NCOLS).filter(|&c| !absent(c));
        let items: Vec<_> =
            cols.map(|c| (format!("{:<8} {}", base_col_name(c), base_col_about(c)), Effect::Col(c))).collect();
        let sel = items.iter().position(|(_, e)| *e == Effect::Col(self.col)).unwrap_or(0);
        self.input = Input::choose("columns?", Kind::Cols, items, sel);
    }

    /// Show the column at cursor index `col`, or leave it out, from now on.
    fn toggle_col(&mut self, col: usize) -> Option<Effect> {
        let id = col_id(col);
        if !self.store.hide.remove(id) {
            self.store.hide.insert(id.to_string());
        }
        self.set_cols();
        self.rebuild();
        Some(Effect::Save)
    }

    /// Leave out the columns `Store::hide` names, as saved or as another modelcmp changed them.
    pub fn set_cols(&mut self) {
        set_off((TEXT..NCOLS).filter(|&c| self.store.hide.contains(col_id(c))).fold(0, |m, c| m | 1 << c));
        self.off_hidden();
    }

    /// Off a column that is left out: the cursor, the sort and any bound on it.
    fn off_hidden(&mut self) {
        if hidden(self.col) {
            self.col = Some(default_sort().0).filter(|&c| !hidden(c)).unwrap_or(0);
        }
        if hidden(self.sort_col) {
            (self.sort_col, self.descending) = default_sort();
        }
        self.bounds.retain(|b| !hidden(b.0));
    }

    /// The `B` chooser, each source saying what it takes; the TUI opens with it until one is picked.
    pub fn ask_source(&mut self) {
        let items =
            Source::ALL.iter().map(|s| (format!("{:<20} {}", s.label(), s.about()), Effect::Source(*s))).collect();
        let sel = Source::ALL.iter().position(|s| *s == crate::data::source()).unwrap_or(0);
        self.input = Input::choose("benchmarks?", Kind::Source, items, sel);
    }

    /// Use benchmarks from `src` from now on; the shell loads its data (`switched`). The first
    /// pick loads it even when it is the default, as the TUI opened with none.
    fn switch(&mut self, src: Source) -> Option<Effect> {
        if src == crate::data::source() && !self.first_start {
            return None;
        }
        self.first_start = false;
        // A pick is one of the other source's benchmarks.
        self.drop_benches(false);
        // And a bound on a task is on the other source's scale: 155 ECI points are no score.
        self.bounds.retain(|b| !TASK_COLS.contains(&b.0));
        let was = index_col();
        crate::data::set_source(src);
        // The cursor and the sort follow the index to this source's column.
        for c in [&mut self.col, &mut self.sort_col] {
            if *c == was {
                *c = index_col();
            }
        }
        self.store.source = src.id().to_string();
        self.report(Ok(format!("benchmarks from {} · B to change", src.label())));
        Some(Effect::Source(src))
    }

    /// After a switch of source: that source's cached data, or none. True when it must be
    /// downloaded, which replaces any refresh under way for the other source. Until then the
    /// table is empty rather than showing the other source's scores under this one's name.
    pub fn switched(&mut self, cached: Option<Data>) -> bool {
        let fetch = cached.as_ref().is_none_or(Data::stale);
        self.set_data(cached.unwrap_or_default());
        (self.refreshing, self.refresh_failed) = (fetch, false);
        fetch
    }

    /// The frame's `⟳ refreshing` says it is under way, so the message is cleared.
    pub fn refresh(&mut self) -> Option<Effect> {
        self.status.clear();
        self.failed = false;
        if self.refreshing {
            return None;
        }
        self.refreshing = true;
        // One may be uploaded since, or found by a newer modelcmp: the shell forgets them too.
        self.downloads.retain(|_, d| *d != Download::Gone);
        Some(Effect::Refresh)
    }

    /// The task's price frontier among the models the filters let through, plus the task's
    /// favorite whatever the filters, excluded ones left out: cheapest first, the best model last.
    /// As of the last `rebuild`.
    pub fn task_frontier(&self, t: &Task) -> Vec<(&Model, f64)> {
        self.cached(t).map(|f| f.0.iter().map(|&(i, s)| (&self.data.models[i], s)).collect()).unwrap_or_default()
    }

    fn cached(&self, t: &Task) -> Option<&Front> {
        self.fronts.get(TASKS.iter().position(|x| x.name == t.name)?)
    }

    /// `task_frontier` and the favorites on it only for being favorites, computed from `shown`,
    /// the models the filters let through.
    fn front(&self, t: &Task, shown: &[usize]) -> Front {
        let usable = |m: &&Model| self.usable(m);
        // Ranked only when the filters show them, so a hidden favorite drops no shown model.
        let shown = shown.iter().map(|&i| &self.data.models[i]).filter(usable);
        let (line, off) = task_line(shown, self.data.models.iter().filter(usable), &self.store, t);
        let at = |m: &Model| self.data.models.iter().position(|x| std::ptr::eq(x, m)).unwrap_or(0);
        let off = line.iter().filter(|(m, _)| off.contains(&m.key.as_str())).map(|&(m, _)| at(m)).collect();
        (line.into_iter().map(|(m, s)| (at(m), s)).collect(), off)
    }

    /// Whether the user has access to `m` whatever `a`, as its Via shows.
    pub fn accessible(&self, m: &Model) -> bool {
        in_reach(m, false, self.any_available)
    }

    /// Whether `m` is local: one your machine runs, and with `a` one it can, as ollama or
    /// llama.cpp can download it from its Hugging Face repo (`x`, `modelcmp get`).
    fn local(&self, m: &Model) -> bool {
        m.here() || self.all && m.hf_repo().is_some()
    }

    /// The models whose download's size is still to ask for: the one under the cursor, and the
    /// local ones among the table's rows on screen, so `x` has it by the time it is pressed.
    fn unsized_models(&self) -> impl Iterator<Item = &Model> {
        let (top, page) = (self.table.offset(), self.page as usize);
        let seen = self.rows.iter().skip(top).take(page).map(|&i| &self.data.models[i]).filter(|m| self.local(m));
        let new = |m: &&Model| m.hf_repo().is_some() && !self.downloads.contains_key(&m.key);
        self.current().into_iter().chain(seen).filter(new)
    }

    /// Whether a size is still to ask for. Asked ahead of `x`, once the cursor rests, and once
    /// until a refresh.
    pub fn size_wanted(&self) -> bool {
        self.unsized_models().next().is_some()
    }

    /// The questions of those sizes, each a model's key and repo: of one a runner here has
    /// too, as its details say the size.
    // ponytail: a screenful asked at once, two requests each, to queue if Hugging Face
    // starts refusing (429).
    pub fn size_ask(&mut self) -> Vec<(String, String)> {
        let models: Vec<_> =
            self.unsized_models().map(|m| (m.key.clone(), m.hf_repo().unwrap_or("").to_string())).collect();
        // Once each: the model under the cursor is a row on screen too.
        let new = |(key, _): &(String, String)| self.downloads.insert(key.clone(), Download::Awaited).is_none();
        models.into_iter().filter(new).collect()
    }

    /// Hugging Face's answer on the download of the model `key`, which `x` goes by from now
    /// on. Its open list says it too, on the entry, which stays where it is under the cursor.
    pub fn sized(&mut self, key: &str, answer: Download) {
        self.downloads.insert(key.into(), answer);
        // Via sorts by it, and a model with no copy leaves the ones picked by `DOWNLOAD`.
        let picked = answer == Download::Gone && self.via.iter().any(|v| v == DOWNLOAD);
        if picked || self.sort_col == VIA && matches!(answer, Download::Size(_)) {
            self.rebuild();
        }
        let said = match answer {
            Download::Size(b) => format!(" ({})", crate::data::gb(b)),
            Download::Gone => " (no GGUF copy)".into(),
            _ => return,
        };
        if let Input::Choose { kind: Kind::Launch, items, .. } = &mut self.input {
            for (label, _) in items.iter_mut().filter(|i| got(&i.1) == Some(key)) {
                label.push_str(&said);
            }
        }
    }

    /// The sizes known, bytes by model key, for the shell to keep (`data::save_sizes`).
    pub fn sizes(&self) -> std::collections::HashMap<String, u64> {
        let size = |(k, d): (&String, &Download)| match d {
            Download::Size(b) => Some((k.clone(), *b)),
            _ => None,
        };
        self.downloads.iter().filter_map(size).collect()
    }

    /// The sizes kept by a run before.
    pub fn set_sizes(&mut self, sizes: std::collections::HashMap<String, u64>) {
        self.downloads.extend(sizes.into_iter().map(|(k, b)| (k, Download::Size(b))));
    }

    /// The size of the model's GGUF copy on Hugging Face, once known, which its details say.
    pub fn size(&self, m: &Model) -> Option<u64> {
        match self.downloads.get(&m.key) {
            Some(Download::Size(b)) => Some(*b),
            _ => None,
        }
    }

    /// What Via says of a model `x` can download: `↓`, and the download's size once known. None
    /// for one it offers no download of, as Hugging Face has no GGUF copy of it or a runner
    /// here has it already.
    pub fn download(&self, m: &Model) -> Option<String> {
        getters(m, &self.tools).next()?;
        match self.downloads.get(&m.key) {
            Some(Download::Gone) => None,
            _ => Some(self.size(m).map_or("↓".into(), |b| format!("↓ {}", crate::data::gb(b)))),
        }
    }

    /// The models a run before found no GGUF copy of.
    pub fn set_gone(&mut self, keys: impl Iterator<Item = String>) {
        self.downloads.extend(keys.map(|k| (k, Download::Gone)));
    }

    /// Whether you have access to a model, as the data last set had it.
    pub fn any_available(&self) -> bool {
        self.any_available
    }

    /// What the Via dropdown picks `m` by: `shown_via`, then whether your machine runs it
    /// through ollama or llama.cpp, and `DOWNLOAD` for one `x` can download (`download`).
    fn via_labels<'a>(&self, m: &'a Model) -> Vec<&'a str> {
        let mut v = self.shown_via(m);
        v.push(if m.here() { LOCAL } else { NOT_LOCAL });
        v.extend(self.download(m).map(|_| DOWNLOAD));
        v
    }

    /// Via as the table shows it: the harnesses, or `OUT_OF_REACH` for one you have no access to.
    fn shown_via<'a>(&self, m: &'a Model) -> Vec<&'a str> {
        shown_via(m, self.any_available)
    }

    /// Whether `m` is one to use: accessible, or any with `a` or none available. Only these are
    /// recommended: a selected model out of reach shows, as `F` and `E` show theirs, but is never a pick.
    pub fn in_reach(&self, m: &Model) -> bool {
        in_reach(m, self.all, self.any_available)
    }

    /// Whether access to no model was found, so every model shows; not so before the data is
    /// in, nor while a harness with no listing yet is still asked for its models.
    pub fn no_access(&self) -> bool {
        !self.data.models.is_empty() && !self.any_available && !self.data.listing
    }

    /// Whether `m` is drawn muted: excluded, or one you have no access to.
    pub fn muted(&self, m: &Model) -> bool {
        self.store.is_excluded(&m.key) || !self.accessible(m)
    }

    /// The tab of the models recommend ranks, the one on in its `among` line: `E` aside, as it
    /// narrows none.
    pub fn among_on(&self) -> usize {
        match self.only {
            Some(i) if i != EXCLUDED => i,
            _ if self.all || self.no_access() => ALL,
            _ => YOURS,
        }
    }

    /// Whether a search, a dropdown or a bound narrows the models too.
    pub fn filtered_too(&self) -> bool {
        !(self.query.trim().is_empty() && self.bounds.is_empty() && self.dev.is_empty() && self.via.is_empty())
    }

    /// Recommend ranks the models of tab `i`, picked on its `among` line: the table's tab too,
    /// and an empty one says why, as its key does there, the cursor staying off it as `h` `l` do.
    fn pick_among(&mut self, i: usize) -> Option<Effect> {
        if self.tab_has(i) {
            self.among = Some(i);
        }
        let effect = self.set_tab(i);
        self.view = View::Recommend;
        effect
    }

    /// The `among` line's cursor `n` tabs along, round its ends, past the ones with nothing to show.
    fn among_by(&mut self, mut i: usize, n: isize) {
        for _ in 0..n.unsigned_abs() {
            loop {
                i = step(i, n.signum(), EXCLUDED);
                if self.tab_has(i) {
                    break;
                }
            }
        }
        self.among = Some(i);
    }

    /// Whether `m` can be recommended: in reach and not excluded.
    fn usable(&self, m: &Model) -> bool {
        self.in_reach(m) && !self.store.is_excluded(&m.key)
    }

    /// Whether `key`, a favorite of the task, is on its line only for being a favorite.
    pub fn favorite_unrecommended(&self, t: &Task, key: &str) -> bool {
        self.cached(t).is_some_and(|f| f.1.iter().any(|&i| self.data.models[i].key == key))
    }

    /// The task `f` and the ★ mark refer to: the one under the cursor in recommend, else the
    /// picked one. None on a task of your own, whose ★ is that of any task.
    pub fn task_at_hand(&self) -> Option<&'static Task> {
        if self.view == View::Recommend { self.cur_task() } else { self.task }
    }

    /// Whether the model's row shows a ★: it is the favorite for the task at hand, or with no
    /// task for any. One ★ either way; the status bar and the detail name the tasks.
    pub fn starred(&self, key: &str) -> bool {
        self.store.is_favorite(self.task_at_hand(), key)
    }

    /// `f`'s grid of tasks for the model `key`, starting on the task at hand, so f enter toggles
    /// it, on the box the model has there, else that of the cursor's tier in recommend; with no
    /// task at hand, on the first box it has.
    fn ask_fav(&mut self, key: &str) {
        let own = if self.view == View::Recommend { self.custom_at() } else { None };
        let at = self.task_at_hand().map(|t| t.name).or(own);
        let favs = self.store.favorite_for(key);
        let fav = favs.iter().find(|s| at.is_none_or(|t| slot_box(s).0 == t));
        // On a tier in recommend, that tier's box, so f enter is for it alone, unless the model
        // is there as the favorite of another box of the task.
        let on_tier = self.view == View::Recommend && self.task_sel > 0 && !self.one_pick(self.task_cur);
        let tier = at.filter(|_| on_tier).map(|t| box_slot(t, self.task_sel));
        let slot = match (tier, fav) {
            (Some(t), Some(f)) if !favs.contains(&t) => f.clone(),
            (Some(t), _) => t,
            (None, f) => f.map(String::as_str).or(at).unwrap_or(TASKS[0].name).to_string(),
        };
        self.fav_at(key, &slot);
    }

    /// `f`: favorite the model `key` for the slot, a task, one tier of it or a task of your own,
    /// or unfavorite it when it already is.
    fn fav(&mut self, key: &str, task: &str) -> Option<Effect> {
        let name = self.data.models.iter().find(|m| m.key == key)?.name.clone();
        let on = self.custom_at().map(String::from);
        self.store.toggle_favorite(task, key);
        self.back_on(on);
        self.report(Ok(match self.store.favorite(task) == Some(key) {
            true => format!("★ {name} favorite for {task}"),
            false => format!("{name} no longer the favorite for {task}"),
        }));
        self.rebuild_in_place();
        Some(Effect::Save)
    }

    /// The `H` chooser: the harnesses you have a model on, after "any", as `v` lists them, with the cursor on
    /// yours. It is the one `x`, `Y`, `--cmd` and `--id` go by for a model it has, after a
    /// favorite's own.
    fn ask_harness(&mut self) {
        let mine = |h: &&str| *h == self.store.harness || self.data.models.iter().any(|m| m.via.iter().any(|v| v == h));
        let items: Vec<_> = std::iter::once(("any harness".to_string(), Effect::Harness(None)))
            .chain(crate::data::vias().filter(mine).map(|h| (h.to_string(), Effect::Harness(Some(h)))))
            .collect();
        let has = |e: &Effect| matches!(e, Effect::Harness(Some(h)) if *h == self.store.harness);
        let sel = items.iter().position(|(_, e)| has(e)).unwrap_or(0);
        self.input = Input::choose("default harness?", Kind::Harness, items, sel);
    }

    /// `v` in `f`'s grid: the harnesses that have the slot's favorite `key`, to run it on one,
    /// as `x` lists them, after "any", with the cursor on the one it has. Only a ticked entry
    /// has a model to run.
    fn ask_via(&mut self, key: &str, slot: &str) {
        if self.store.favorite(slot) != Some(key) {
            return self.refuse(format!("{slot} is not ticked: a harness is for its favorite"));
        }
        let Some(m) = self.data.models.iter().find(|m| m.key == key) else { return };
        // With its harness: llama.cpp's command starts with the binary this machine has, which is not its name.
        let cmds: Vec<_> =
            crate::data::vias().filter_map(|h| Some((h, launch_cmd(m, h, &self.data.harness)?))).collect();
        if cmds.is_empty() {
            return self.refuse(format!("no harness has {}; Via shows where you have access", m.name));
        }
        let via = |h: Option<String>| Effect::Via(key.to_string(), slot.to_string(), h);
        let items: Vec<_> = std::iter::once(("any harness".to_string(), via(None)))
            .chain(cmds.into_iter().map(|(h, c)| (c.join(" "), via(Some(h.to_string())))))
            .collect();
        let has = |e: &Effect| matches!(e, Effect::Via(_, _, h) if h.as_deref() == self.store.via(slot));
        let sel = items.iter().position(|(_, e)| has(e)).unwrap_or(0);
        self.input = Input::choose("run on which harness?", Kind::Via, items, sel);
    }

    /// `f`'s grid for the model `key`, opened on the box of `slot`: where `f` starts, and where
    /// `v`'s list came from.
    fn fav_at(&mut self, key: &str, slot: &str) {
        self.input = Input::choose("favorite for which tasks?", Kind::Fav, vec![], 0);
        self.relist(key, slot, None);
    }

    /// Recommend's cursor back on the task of your own it was `on`, after a change to them: they
    /// are in order of name, so one added, renamed or gone moves the others.
    fn back_on(&mut self, on: Option<String>) {
        if let Some(at) = on.and_then(|t| self.store.custom_tasks().iter().position(|x| *x == t)) {
            self.task_cur = TASKS.len() + at;
        }
    }

    /// `f`'s open grid made again for the model `key`, with the cursor on the box of `slot`,
    /// where `edit` goes on writing.
    fn relist(&mut self, key: &str, slot: &str, edit: Option<Edit>) {
        let fresh = self.fav_items(key);
        let (task, col) = slot_box(slot);
        let at = fresh.iter().position(|(_, e)| matches!(e, Effect::Fav(_, t) if t == task)).unwrap_or(0);
        if let Input::Choose { kind: Kind::Fav, items, list, .. } = &mut self.input {
            *items = fresh;
            *list = List { sel: at, top: list.top, col, edit, ..Default::default() };
        }
    }

    /// Whether an entry of `f`'s grid is being written, when every key is its text's.
    fn editing(&self) -> bool {
        self.open_list().is_some_and(|l| l.edit.is_some())
    }

    /// Keys while an entry of `f`'s grid is written in place.
    fn edit_key(&mut self, code: KeyCode, mods: KeyModifiers) -> Option<Effect> {
        let Input::Choose { items, list, .. } = &mut self.input else { return None };
        match code {
            KeyCode::Esc => list.edit = None,
            KeyCode::Enter => {
                // The model the list is for, which its last entry holds.
                let Some((_, Effect::NewTask(key))) = items.last() else { return None };
                let (key, e) = (key.clone(), list.edit.take()?);
                return self.edited(&key, e);
            }
            _ => {
                let e = list.edit.as_mut()?;
                e.err = None;
                // `:` sets a tier apart, so no name has one.
                let about = matches!(e.what, What::About(_));
                edit(&mut e.text, &mut e.cur, code, mods, |c| about || c != ':');
            }
        }
        None
    }

    /// Enter on an entry written in `f`'s grid for the model `key`: the task is named, renamed
    /// or described, and the list stays open on it.
    fn edited(&mut self, key: &str, e: Edit) -> Option<Effect> {
        // The box the cursor was on, which a name or an about leaves where it is.
        let col = self.open_list().map_or(0, |l| l.col);
        let name = crate::store::task_name(&e.text);
        match e.what {
            What::New => {
                // No name names no task.
                let name = name.ok()?;
                // A name the model already has would be toggled off.
                let saved = if self.store.favorite(&name) == Some(key) { None } else { self.fav(key, &name) };
                // What it is about is asked next, on its entry: agents pick the task by it.
                let about = self.store.about(&name).unwrap_or("").to_string();
                let next = crate::fit::task(&name).is_none().then(|| Edit::new(What::About(name.clone()), about));
                self.relist(key, &name, next);
                saved
            }
            What::Rename(was) => {
                let name = name.ok().filter(|n| *n != was)?;
                let on = self.custom_at().map(|t| if t == was { name.clone() } else { t.to_string() });
                if let Err(err) = self.store.rename_task(&was, &name) {
                    // Said under the list, the name still there to change.
                    if let Input::Choose { list, .. } = &mut self.input {
                        list.edit = Some(Edit { what: What::Rename(was), err: Some(err), ..e });
                    }
                    return None;
                }
                self.back_on(on);
                self.report(Ok(format!("{was} renamed to {name}")));
                self.relist(key, &box_slot(&name, col), None);
                Some(Effect::Save)
            }
            What::About(task) => {
                let was = self.store.about(&task).unwrap_or("").to_string();
                self.store.set_about(&task, &e.text);
                let now = self.store.about(&task).unwrap_or("").to_string();
                if now == was {
                    return None;
                }
                self.report(Ok(if now.is_empty() {
                    format!("{task} has no about now")
                } else {
                    format!("{task}: {now}")
                }));
                self.relist(key, &box_slot(&task, col), None);
                Some(Effect::Save)
            }
        }
    }

    /// The open dropdown's or choice list's cursor and search.
    pub fn open_list(&self) -> Option<&List> {
        match &self.input {
            Input::Menu { list, .. } | Input::Choose { list, .. } => Some(list),
            _ => None,
        }
    }

    /// The cursor and length of the open dropdown or choice list (`o`, `x`), whose moves take the
    /// table's vim motions.
    fn list(&mut self) -> Option<(&mut usize, usize)> {
        match &mut self.input {
            Input::Menu { items, list, .. } => Some((&mut list.sel, menu_rows(items, &list.query).len())),
            Input::Choose { items, list, .. } => Some((&mut list.sel, choice_rows(items, &list.query).len())),
            _ => None,
        }
    }

    /// `wrap`: a step from an end goes round to the other, as `j k` do; the wheel and a page
    /// stop there.
    fn move_by(&mut self, n: isize, wrap: bool) {
        let go = |i: usize, len: usize| {
            if wrap { step(i, n, len) } else { i.saturating_add_signed(n).min(len.saturating_sub(1)) }
        };
        if let Some((sel, len)) = self.list() {
            *sel = go(*sel, len);
        } else if self.view == View::Table {
            self.select(go(self.selected(), self.rows.len()));
        } else if self.view == View::Recommend {
            // The `among` line comes before the first task.
            match go(self.among.map_or(self.task_cur + 1, |_| 0), self.task_count() + 1).checked_sub(1) {
                Some(t) => self.task_to(t),
                None => self.among = Some(self.among_on()),
            }
        } else {
            self.scroll = self.scroll.saturating_add_signed(n.clamp(i16::MIN as isize, i16::MAX as isize) as i16);
        }
    }

    /// Recommend's cursor to task `t`, on the name or the tier it was on.
    fn task_to(&mut self, t: usize) {
        (self.among, self.task_cur) = (None, t);
    }

    fn go_to(&mut self, row: usize) {
        if let Some((sel, len)) = self.list() {
            *sel = row.min(len.saturating_sub(1));
        } else if self.view == View::Table {
            self.select(row);
        } else if self.view == View::Recommend {
            self.task_to(row.min(self.task_count() - 1));
        } else {
            self.scroll = row.min(u16::MAX as usize) as u16;
        }
    }

    /// Whether `col`'s header offers its dropdown now: a task's columns do not while a task is
    /// picked, when `open_menu` refuses them.
    pub fn menu(&self, col: usize) -> bool {
        has_menu(col) && !(self.task.is_some() && TASK_COLS.contains(&col))
    }

    /// Open the dropdown of the column under the cursor, on the entry in effect.
    fn open_menu(&mut self) {
        // A task's line is drawn from its score, so its columns show that.
        if self.task.is_some() && col_benches(self.col).is_some() {
            return self.refuse("a task shows its own scores: c leaves the task");
        }
        let (items, picked) = self.menu_items(self.col);
        self.input = Input::Menu { col: self.col, items, list: List::at(picked.map_or(0, |i| i + 1)) };
    }

    /// The entries of `col`'s dropdown, each with how many models it would show, and the one
    /// in effect, counted from the first entry after "any".
    fn menu_items(&self, col: usize) -> (Vec<(String, usize)>, Option<usize>) {
        let ms: Vec<(usize, &Model)> = self.filtered(col).collect();
        let benches = col_benches(col);
        // The first entry of a task's dropdown counts the models with the task's score.
        let scored = numeric(col).filter(|_| benches.is_some());
        let first = scored.map_or(ms.len(), |c| ms.iter().filter(|(_, m)| (c.get)(m).is_some()).count());
        let (mut items, picked) = if let Some((_, benches)) = benches {
            // How many models each benchmark scored.
            let count = |b: &str| ms.iter().filter(|(_, m)| m.scores.contains_key(b)).count();
            let picked = self.col_bench(col).and_then(|b| benches.iter().position(|x| *x == b));
            (benches.iter().map(|&b| (b.to_string(), count(b))).collect(), picked)
        } else if col == 1 || col == VIA {
            let by_dev = col == 1;
            let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
            // Via as the table shows it, so "not available" can be picked too, then `LOCAL` and the like.
            let labels = ms.iter().flat_map(|&(_, m)| if by_dev { m.devs().collect() } else { self.via_labels(m) });
            for l in labels.filter(|l| !l.is_empty()) {
                *counts.entry(l).or_default() += 1;
            }
            // With no local model the rest are every model, as "any" is.
            if !counts.contains_key(LOCAL) {
                counts.remove(NOT_LOCAL);
            }
            // Countries, then developers, each A-Z whatever their case, so xAI comes before Z.ai;
            // harnesses with the most models first, where the stable sort keeps ties A-Z.
            let mut names: Vec<(String, usize)> = counts.into_iter().map(|(d, n)| (d.to_string(), n)).collect();
            if by_dev {
                let countries: std::collections::HashSet<&str> = ms.iter().map(|(_, m)| m.country.as_str()).collect();
                names.sort_by_key(|(d, _)| (!countries.contains(d.as_str()), d.to_lowercase()));
            } else {
                names.sort_by_key(|(h, n)| (VIA_KINDS.iter().position(|k| k == h), Reverse(*n)));
            }
            let current = if col == 1 { &self.dev } else { &self.via };
            let picked = current.first().and_then(|d| names.iter().position(|(x, _)| x == d));
            (names, picked)
        } else {
            let items = LEVELS
                .iter()
                .enumerate()
                .map(|(l, &e)| {
                    (level_label(l), ms.iter().filter(|&&(i, _)| self.val(i, PRICE).is_some_and(|c| c <= e)).count())
                })
                .collect();
            (items, self.price_level())
        };
        items.insert(0, (benches.map_or("any", |b| b.0).into(), first));
        // The entry a task column shows already keeps the bounds on it: it counts what the table shows.
        if benches.is_some() {
            items[picked.map_or(0, |i| i + 1)].1 =
                self.filtered(usize::MAX).filter(|&(i, _)| self.val(i, col).is_some()).count();
        }
        (items, picked)
    }

    fn toggle_mark(&mut self) {
        let Some(key) = self.current().map(|m| m.key.clone()) else { return };
        self.store.toggle_marked(&key);
        self.rebuild_in_place();
    }

    pub fn key(&mut self, k: KeyEvent) -> Option<Effect> {
        let effect = self.on_key(k);
        self.follow();
        effect
    }

    /// Ask whether to quit, keeping what was open to go back to.
    fn ask_quit(&mut self) {
        self.asked_from = std::mem::replace(&mut self.input, Input::Quit);
    }

    /// Text pasted in the terminal: typed into the search, note or bound being written, and
    /// nothing anywhere else, where its letters would run as keys.
    pub fn paste(&mut self, text: &str) {
        if matches!(self.input, Input::None | Input::Quit | Input::Upgrade | Input::Unfavorite | Input::Unexclude)
            || self.open_list().is_some_and(List::idle)
        {
            self.refuse("nothing to paste into: / searches, n writes a note");
            return;
        }
        // No line breaks, which would be enter.
        let text: String = text.chars().filter(|c| !c.is_control()).collect();
        // A search takes it in one go, so the table is filtered once and not at every letter.
        if let Input::Search { cur, .. } = &mut self.input {
            let at = std::mem::replace(cur, *cur + text.len());
            self.search_target().insert_str(at, &text);
            if !self.overlay_search() {
                self.rebuild();
            }
            return;
        }
        for c in text.chars() {
            self.input_key(KeyCode::Char(c), KeyModifiers::NONE);
        }
    }

    /// The table's cursor follows the model picked in compare and recommend, so after esc you are
    /// on it. A selection in progress keeps the cursor, which is its end.
    fn follow(&mut self) {
        if !matches!(self.view, View::Compare | View::Recommend) || self.selecting() {
            return;
        }
        let Some(key) = self.current().map(|m| m.key.clone()) else { return };
        if let Some(n) = self.rows.iter().position(|&i| self.data.models[i].key == key) {
            self.select(n);
        }
    }

    fn on_key(&mut self, k: KeyEvent) -> Option<Effect> {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && k.code == KeyCode::Char('c') {
            return Some(Effect::Quit);
        }
        // A dropdown not being searched and a choice list take counts and motions too.
        let list = self.open_list().is_some_and(List::idle);
        if self.input != Input::None && !list {
            return self.input_key(k.code, k.modifiers);
        }
        self.status.clear();
        self.failed = false;
        let mut count = std::mem::take(&mut self.count);
        let g_pending = self.g_pending.take();
        // Digits are a count, except a leading 0, as in vim.
        if let KeyCode::Char(c) = k.code
            && !ctrl
            && let Some(d) = c.to_digit(10).filter(|&d| d > 0 || count > 0)
        {
            self.count = (count * 10 + d as usize).min(99_999);
            return None;
        }
        // `g` waits for a second one: `gg` goes to the top, `3gg` to row 3.
        if k.code == KeyCode::Char('g') && !ctrl {
            match g_pending {
                Some(pending) => count = pending,
                None => {
                    self.g_pending = Some(count);
                    return None;
                }
            }
        }
        let n = count.max(1) as isize;
        let half = (self.page / 2).max(1) as isize;
        match k.code {
            KeyCode::Down | KeyCode::Char('j') => self.move_by(n, true),
            KeyCode::Up | KeyCode::Char('k') => self.move_by(-n, true),
            KeyCode::PageDown => self.move_by(n * half * 2, false),
            KeyCode::PageUp => self.move_by(-n * half * 2, false),
            KeyCode::Char(')') => self.move_by(n * half, false),
            KeyCode::Char('(') => self.move_by(-n * half, false),
            KeyCode::Char('d') if ctrl => self.move_by(n * half, false),
            KeyCode::Char('u') if ctrl => self.move_by(-n * half, false),
            KeyCode::Char('g') if count > 0 => self.go_to(count - 1),
            // Recommend's top is its `among` line, as `k` from the first task; `1gg` is that task.
            KeyCode::Home | KeyCode::Char('g') if self.view == View::Recommend && !list => {
                self.among = Some(self.among_on());
            }
            KeyCode::Home | KeyCode::Char('g') => self.go_to(0),
            KeyCode::End | KeyCode::Char('G') => self.go_to(usize::MAX),
            // ^e is not `e`: only the keys above take ctrl.
            KeyCode::Char(_) if ctrl => {}
            // `h l` move along the boxes of a row of f's grid, stopping at its ends.
            KeyCode::Char('h' | 'l') | KeyCode::Left | KeyCode::Right if self.choosing_favs() => {
                let back = matches!(k.code, KeyCode::Char('h') | KeyCode::Left);
                if let Input::Choose { list, .. } = &mut self.input {
                    list.col = list.col.saturating_add_signed(if back { -n } else { n }).min(BOXES - 1);
                }
            }
            KeyCode::Char(c @ (']' | '['))
                if self.choosing_favs() || self.choosing_cols() || matches!(self.input, Input::Menu { .. }) =>
            {
                self.jump_ticked(if c == ']' { n } else { -n })
            }
            _ if list => return self.input_key(k.code, k.modifiers),
            _ => return self.table_key(k.code, n),
        }
        None
    }

    /// The wheel scrolls whatever `j k` move and sideways moves the column cursor; a click
    /// selects a row, and a double click opens what its cell shows: the details from the name or
    /// the developer, a harness in Via, as `x` does, from a number the page it comes from, and
    /// the note to write, as `n` does;
    /// a click on a header sorts by it, as `s` does, and on its ▾ opens the dropdown. A click on
    /// an entry does what enter does: in a dropdown and `f`'s tasks it toggles the entry and the
    /// list stays open until a click outside; in the other choice lists it picks the entry. In
    /// compare and recommend a click moves the cursor to the model or task, and a double click
    /// does what enter does.
    pub fn mouse(&mut self, m: Mouse) -> Option<Effect> {
        let effect = self.on_mouse(m);
        self.follow();
        effect
    }

    fn on_mouse(&mut self, m: Mouse) -> Option<Effect> {
        if let Mouse::Key(code) = m {
            return self.on_key(code.into());
        }
        // A prompt over the panel and the panel's filter take an esc of their own first. The
        // details have no filter: the one of the compare they were opened from stays, as esc
        // leaves it. An entry or a note being written is only left, its panel staying open.
        let writing = self.editing() || matches!(self.input, Input::Note { .. });
        if m == Mouse::Close && !writing {
            if self.input != Input::None {
                self.on_key(KeyCode::Esc.into());
            }
            if self.overlay_search() {
                self.overlay_query.clear();
            }
            return self.on_key(KeyCode::Esc.into());
        }
        if m == Mouse::Link {
            return Some(Effect::Open(crate::data::AA_KEY_URL.into()));
        }
        // A prompt keeps the wheel still, and a click anywhere else leaves it as esc does: an
        // entry or a note being written, a bound, the key, and what `q`, `U`, `D` and `X` ask. A search
        // is kept as enter keeps it, and the click is then on what it found.
        if self.editing() || (self.input != Input::None && self.open_list().is_none()) {
            let search = matches!(self.input, Input::Search { .. });
            match m {
                Mouse::Scroll(_) | Mouse::Cols(_) => return None,
                _ if search => drop(self.input_key(KeyCode::Enter, KeyModifiers::NONE)),
                _ => return self.input_key(KeyCode::Esc, KeyModifiers::NONE),
            }
        }
        let typing = self.open_list().is_some_and(|l| l.typing);
        let list = self.open_list().is_some();
        // As a key does, and not under a prompt either.
        if self.input == Input::None || (list && !typing) {
            self.status.clear();
            self.failed = false;
            (self.count, self.g_pending) = (0, None);
        }
        if let Mouse::Scroll(n) = m {
            if list || self.input == Input::None {
                self.move_by(n, false);
            }
            return None;
        }
        if list {
            return match m {
                Mouse::Item(n) | Mouse::Tick(n, _) => {
                    let (sel, len) = self.list()?;
                    if n >= len {
                        return None;
                    }
                    *sel = n;
                    if let (Mouse::Tick(_, col), Input::Choose { list, .. }) = (m, &mut self.input) {
                        list.col = col.min(BOXES - 1);
                    }
                    self.input_key(KeyCode::Enter, KeyModifiers::NONE)
                }
                Mouse::Cols(_) => None,
                // A click on another tab leaves the theme list for it, as `tab` does.
                Mouse::Tab(i) if self.tab_on(i) => None,
                Mouse::Tab(_) => {
                    self.input = Input::None;
                    self.on_mouse(m)
                }
                // The first start's question stays open: closing it would pick the default.
                _ if self.first_start && matches!(self.input, Input::Choose { kind: Kind::Source, .. }) => None,
                // Outside, or anything else that is not an entry: close it. A search takes an
                // esc of its own first.
                _ => {
                    if typing {
                        self.input_key(KeyCode::Esc, KeyModifiers::NONE);
                    }
                    self.input_key(KeyCode::Esc, KeyModifiers::NONE)
                }
            };
        }
        // The sideways wheel moves the model cursor wherever h and l do.
        if let (View::Compare | View::Recommend, Input::None, Mouse::Cols(n)) = (&self.view, &self.input, &m) {
            return self.table_key(KeyCode::Char(if *n < 0 { 'h' } else { 'l' }), n.abs());
        }
        if let (Input::None, Mouse::Model(s) | Mouse::Open(s) | Mouse::MarkModel(s)) = (&self.input, m) {
            match (s, &self.view) {
                (Stop::Compare(i), View::Compare) => self.compare_sel = i,
                (Stop::Recommend(t, i), View::Recommend) => {
                    (self.task_cur, self.task_sel, self.among) = (t, i, None);
                }
                (Stop::Among(i), View::Recommend) => return self.pick_among(i),
                _ => return None,
            }
            // Compare leaves out `space`: it would drop the model from the view.
            return match m {
                Mouse::Open(_) => self.on_key(KeyCode::Enter.into()),
                Mouse::MarkModel(_) if self.view == View::Recommend => self.on_key(KeyCode::Char(' ').into()),
                _ => None,
            };
        }
        // A tab shows alone from a panel or the details too; in the table ✓ ★ ✗ go on or off, as
        // `S` `F` `E`.
        if let (Input::None, Mouse::Tab(i)) = (&self.input, m)
            && (self.view != View::Table || !(MARKED..RECOMMEND).contains(&i))
            && i < TABS.len()
        {
            // With access to none a click on yours or all in recommend says so and stays, as `a`.
            if self.view == View::Recommend && i < MARKED && self.no_access() {
                self.refuse(NO_ACCESS);
                return None;
            }
            return self.set_tab(i);
        }
        if self.view != View::Table || self.input != Input::None {
            return None;
        }
        match m {
            Mouse::Cols(n) => return self.table_key(KeyCode::Char(if n < 0 { 'h' } else { 'l' }), n.abs()),
            // A plain click replaces the selection, as in a file manager.
            Mouse::Row(n) if n < self.rows.len() => {
                self.deselect();
                self.select(n);
            }
            Mouse::Mark(n) if n < self.rows.len() => {
                let inside = self.is_selected(n);
                if self.selecting() {
                    for k in self.targets() {
                        if !self.store.is_marked(&k) {
                            self.store.marked.push(k);
                        }
                    }
                    self.deselect();
                }
                self.select(n);
                // Marking the range only adds marks, so no row drops out.
                if inside {
                    self.rebuild_in_place();
                } else {
                    self.toggle_mark();
                }
                return Some(Effect::Save);
            }
            Mouse::Box(n) if n < self.rows.len() => {
                self.deselect();
                self.select(n);
                self.toggle_mark();
                return Some(Effect::Save);
            }
            Mouse::Star(n) if n < self.rows.len() => {
                self.deselect();
                self.select(n);
                // Only the clicked row, even in a highlight, where `f` refuses several.
                let key = self.current()?.key.clone();
                self.ask_fav(&key);
                return None;
            }
            // Only the clicked row, even in a highlight, where `e` takes all of it.
            Mouse::Exclude(n) if n < self.rows.len() => {
                self.deselect();
                self.select(n);
                let key = self.current()?.key.clone();
                self.store.toggle_excluded(&key);
                self.rebuild_in_place();
                return Some(Effect::Save);
            }
            Mouse::Harness(n, j) if n < self.rows.len() => {
                self.deselect();
                self.select(n);
                let m = self.current()?;
                let h = m.via.get(j)?;
                return launch_cmd(m, h, &self.data.harness).map(Effect::Launch);
            }
            Mouse::Cell(n, col) if n < self.rows.len() => {
                self.deselect();
                self.select(n);
                match col {
                    ..TEXT => return self.table_key(KeyCode::Enter, 1),
                    NOTES => return self.table_key(KeyCode::Char('n'), 1),
                    _ => {}
                }
                // An empty cell has nothing to open.
                self.val(self.rows[n], col)?;
                let m = self.current()?;
                // Where each comes from: the release, prices and context from models.dev, the
                // rest from the source that measures it.
                let (site, page) = if col < ECI {
                    ("models.dev", m.price_page())
                } else {
                    let src = numeric(col).map_or_else(crate::data::source, Col::from);
                    (src.site(), m.page(src))
                };
                if page.is_none() {
                    self.refuse(format!("{} has no page on {site}", m.name));
                }
                return page.map(Effect::Open);
            }
            Mouse::Pick(n) if n < self.rows.len() => {
                // The range, if any, becomes picked rows, then the clicked row toggles.
                let range = self.visual_range();
                self.visual = None;
                if let Some(r) = range {
                    for k in r {
                        if !self.picked.contains(&k) {
                            self.picked.push(k);
                        }
                    }
                }
                match self.picked.iter().position(|&k| k == n) {
                    Some(i) => {
                        self.picked.remove(i);
                    }
                    None => self.picked.push(n),
                }
                self.select(n);
            }
            Mouse::Extend(n) if !self.rows.is_empty() => {
                if self.visual.is_none() {
                    self.visual = Some(self.selected());
                }
                self.select(n);
            }
            Mouse::Top => self.go_to(0),
            Mouse::Tab(i @ MARKED..RECOMMEND) => return self.table_key(KeyCode::Char(TABS[i].1), 1),
            Mouse::Header(c) if c < NCOLS => {
                self.col = c;
                return self.table_key(KeyCode::Char('s'), 1);
            }
            Mouse::Menu(c) if has_menu(c) => {
                self.col = c;
                return self.table_key(KeyCode::Char('d'), 1);
            }
            _ => {}
        }
        None
    }

    /// Index in `LEVELS` of the Price maximum picked in its dropdown, if any.
    pub fn price_level(&self) -> Option<usize> {
        let max = self.bounds.iter().find(|b| b.0 == PRICE && b.1.is_infinite())?.2;
        LEVELS.iter().position(|&e| e == max)
    }

    /// A price level, entry `i` of the Price dropdown, replaces the Price maximum; "any" drops it.
    fn set_price_level(&mut self, i: usize) {
        self.bounds.retain(|&(c, lo, _)| c != PRICE || lo.is_finite());
        if i > 0 {
            self.bounds.push((PRICE, f64::NEG_INFINITY, LEVELS[i - 1]));
        }
    }

    /// Keys other than motions when no prompt or list is open.
    fn table_key(&mut self, code: KeyCode, n: isize) -> Option<Effect> {
        let table = self.view == View::Table;
        // Keys that act on the current model, which only help hides.
        // Compare shows none with fewer than 2 selected, and then has no current model to act on.
        // Recommend has one on a tier with a model, not on a task's name.
        let row = table
            || matches!(self.view, View::Detail(_))
            || (self.view == View::Recommend && self.current().is_some())
            || (self.view == View::Compare && self.marked_shown >= 2);
        // Compare and recommend move a model cursor sideways, wrapping, instead of the column.
        let across = matches!(self.view, View::Compare | View::Recommend);
        // On recommend's `among` line the cursor runs over the tabs, and enter or space picks one.
        let among = self.among.filter(|_| self.view == View::Recommend);
        match (code, among) {
            // With access to none `a` and `A` say so and stay, as in the table.
            (KeyCode::Char('a' | 'A'), _) if self.view == View::Recommend && self.no_access() => {
                self.refuse(NO_ACCESS);
                return None;
            }
            // A tab's key leaves recommend for it, as a click on it does.
            (KeyCode::Char(c), _) if self.view == View::Recommend && TABS[..RECOMMEND].iter().any(|t| t.1 == c) => {
                return self.set_tab(TABS.iter().position(|t| t.1 == c).unwrap_or(YOURS));
            }
            (KeyCode::Enter | KeyCode::Char(' '), Some(i)) => return self.pick_among(i),
            (KeyCode::Char('h') | KeyCode::Left, Some(i)) => {
                self.among_by(i, -n);
                return None;
            }
            (KeyCode::Char('l') | KeyCode::Right, Some(i)) => {
                self.among_by(i, n);
                return None;
            }
            (KeyCode::Char('0' | '_'), Some(_)) => {
                self.among_by(EXCLUDED - 1, 1);
                return None;
            }
            (KeyCode::Char('$'), Some(_)) => {
                self.among_by(0, -1);
                return None;
            }
            (KeyCode::Char('e' | 'f' | 'n' | 'o' | 'x' | 'y' | 'Y'), Some(_)) => {
                self.refuse("the cursor is on the models to rank: j goes to a task");
                return None;
            }
            _ => {}
        }
        match code {
            KeyCode::Char('h') | KeyCode::Left if table => self.col = step_col(self.col, -n),
            KeyCode::Char('l') | KeyCode::Right if table => self.col = step_col(self.col, n),
            KeyCode::Char('h' | 'l') | KeyCode::Left | KeyCode::Right if across => {
                let n = if matches!(code, KeyCode::Char('h') | KeyCode::Left) { -n } else { n };
                let (len, sel) = (self.across_len(), self.across_sel());
                *sel = step((*sel).min(len.saturating_sub(1)), n, len);
            }
            KeyCode::Char('0' | '_') if table => self.col = 0,
            KeyCode::Char('$') if table => self.col = step_col(0, -1),
            KeyCode::Char('0' | '_') if across => *self.across_sel() = 0,
            KeyCode::Char('$') if across => *self.across_sel() = self.across_len().saturating_sub(1),
            KeyCode::Char('w') if table => {
                let (starts, last) = (group_starts(), step_col(0, -1));
                for _ in 0..n {
                    let end = if self.col == last { 0 } else { last };
                    self.col = starts.iter().copied().find(|&g| g > self.col).unwrap_or(end);
                }
            }
            KeyCode::Char('b') if table => {
                let starts = group_starts();
                for _ in 0..n {
                    self.col = starts.iter().rev().copied().find(|&g| g < self.col).unwrap_or(starts[starts.len() - 1]);
                }
            }
            KeyCode::Char('s') if table => {
                if self.sort_col == self.col {
                    self.descending = !self.descending;
                } else {
                    // Best first: text A-Z, prices cheapest first, other numbers highest first.
                    self.sort_col = self.col;
                    self.descending = numeric(self.col).is_some_and(|c| !c.lower_better);
                }
                self.rebuild();
            }
            KeyCode::Char(c @ ('>' | '<')) if table && numeric(self.col).is_some() => {
                self.input = Input::Bound { col: self.col, min: c == '>', text: String::new(), cur: 0 };
            }
            KeyCode::Char('>' | '<') if table => self.refuse("> and < bound a column of numbers"),
            KeyCode::Char('d') if table && has_menu(self.col) => self.open_menu(),
            KeyCode::Char('d') if table => self.refuse("d opens a dropdown on the columns marked ▾"),
            // The next tab with something to show, past the last back to the first.
            KeyCode::Tab | KeyCode::BackTab => return self.step_tab(code == KeyCode::BackTab),
            KeyCode::Char('S') if table && self.only != Some(MARKED) && !self.any_marked() => self.refuse(NO_SELECTED),
            KeyCode::Char('S') if table => {
                self.only = (self.only != Some(MARKED)).then_some(MARKED);
                self.rebuild();
            }
            KeyCode::Char('F') if table && self.only != Some(FAV) && !self.fav_shown => {
                self.refuse("no favorites: f favorites the one under the cursor");
            }
            KeyCode::Char('F') if table => {
                self.only = (self.only != Some(FAV)).then_some(FAV);
                self.rebuild();
            }
            KeyCode::Char('E') if table && self.only != Some(EXCLUDED) && !self.excluded_shown => {
                self.refuse("no excluded models: e excludes the one under the cursor");
            }
            KeyCode::Char('E') if table => {
                self.only = (self.only != Some(EXCLUDED)).then_some(EXCLUDED);
                self.rebuild();
            }
            KeyCode::Char(']' | '[') if table && !self.any_marked() => self.refuse(NO_SELECTED),
            KeyCode::Char(c @ (']' | '[')) if table => {
                self.jump(if c == ']' { n } else { -n }, "selected", |a, m| a.store.is_marked(&m.key));
            }
            KeyCode::Char('}' | '{') if table && self.no_access() => self.refuse(NO_ACCESS),
            KeyCode::Char(c @ ('}' | '{')) if table => {
                self.jump(if c == '}' { n } else { -n }, "available", |a, m| a.accessible(m));
            }
            KeyCode::Char('u') if table && !self.any_marked() => self.refuse(NO_SELECTED),
            // A mark of a model the data no longer has goes too, else nothing in the TUI clears it.
            KeyCode::Char('u') if table => {
                let n = self.marked_shown;
                let gone = std::mem::take(&mut self.store.marked).len() - n;
                self.rebuild_in_place();
                self.status = match gone {
                    0 => format!("deselected {n}"),
                    _ => format!("deselected {n}, and {gone} no longer listed"),
                };
                return Some(Effect::Save);
            }
            KeyCode::Char('D') if table && self.store.favorite.is_empty() => {
                self.refuse("no favorites: f favorites the one under the cursor");
            }
            KeyCode::Char('D') if table => self.input = Input::Unfavorite,
            KeyCode::Char('X') if table && self.store.excluded.is_empty() => {
                self.refuse("no excluded models: e excludes the one under the cursor");
            }
            KeyCode::Char('X') if table => self.input = Input::Unexclude,
            KeyCode::Char('c') if table => {
                self.query.clear();
                self.bounds.clear();
                self.dev.clear();
                self.via.clear();
                self.drop_benches(true);
                // The task set the sort; back to the default.
                if self.task.take().is_some() {
                    (self.sort_col, self.descending) = default_sort();
                }
                self.only = None;
                self.rebuild();
            }
            KeyCode::Char('q') => self.ask_quit(),
            KeyCode::Char('U') if self.data.update().is_some() => self.input = Input::Upgrade,
            KeyCode::Char('U') if self.data.latest.is_empty() => {
                self.refuse("the newest version is not known: r asks again");
            }
            KeyCode::Char('U') => {
                self.status = concat!("no newer version: this is modelcmp v", env!("CARGO_PKG_VERSION")).into();
            }
            KeyCode::Esc => {
                if self.overlay_search() && !self.overlay_query.is_empty() {
                    self.overlay_query.clear();
                } else if let View::Detail(Back::Compare(scroll)) = self.view {
                    (self.view, self.scroll) = (View::Compare, scroll);
                } else if let View::Detail(Back::Recommend(scroll)) = self.view {
                    (self.view, self.scroll) = (View::Recommend, scroll);
                } else if !table {
                    self.view = View::Table;
                } else if self.selecting() {
                    self.deselect();
                } else if !self.query.is_empty() {
                    self.query.clear();
                    self.rebuild();
                } else if self.only.take().is_some() {
                    // Back out of S, F or E to every model.
                    self.rebuild();
                } else if self.task.take().is_some() {
                    // Back to recommend, where enter picked the task.
                    (self.sort_col, self.descending) = default_sort();
                    self.rebuild();
                    self.view = View::Recommend;
                }
            }
            KeyCode::Char('?') => {
                self.view = if self.view == View::Help { View::Table } else { View::Help };
                self.overlay_query.clear();
                self.scroll = 0;
            }
            KeyCode::Char('R') => {
                self.view = if self.view == View::Recommend { View::Table } else { View::Recommend };
                // It opens on its `among` line, the models it ranks.
                (self.task_sel, self.among) = (0, Some(self.among_on()));
                self.scroll = 0;
            }
            // Enter on a task's name is below; on a tier with no model it has none to show either.
            KeyCode::Char('e' | 'f' | 'n' | 'o' | 'x' | 'y' | 'Y' | ' ') | KeyCode::Enter
                if self.view == View::Recommend && !row && (code != KeyCode::Enter || self.task_sel > 0) =>
            {
                self.refuse(match (self.among, self.task_sel) {
                    (Some(_), _) => "no model under the cursor: j picks a task",
                    (_, 0) => "no model under the cursor: l picks a tier",
                    _ => "this tier has no model",
                });
            }
            KeyCode::Char('e') if row => {
                return self.flag(Store::is_excluded, Store::toggle_excluded, ["excluded", "unexcluded"]);
            }
            KeyCode::Char('f') if row => {
                // A task has one favorite, so f says so rather than quietly take the cursor's model.
                let keys = self.targets();
                if let [key] = &keys[..] {
                    self.ask_fav(key);
                } else if let n @ 2.. = keys.len() {
                    self.refuse(format!("a task has one favorite: f takes one model, {n} are highlighted"));
                }
            }
            KeyCode::Char('V') if table && !self.any_marked() => self.refuse(NO_SELECTED),
            // The marked rows shown, for `e` or `space` to act on them all.
            KeyCode::Char('V') if table => {
                self.visual = None;
                self.picked = (0..self.rows.len())
                    .filter(|&k| self.store.is_marked(&self.data.models[self.rows[k]].key))
                    .collect();
                // A search or a dropdown hides them all.
                if self.picked.is_empty() {
                    self.refuse("no selected model is shown");
                }
            }
            // Not on an empty table: a range of no rows would have `C` keep none of the selected.
            KeyCode::Char('v') if table && (self.selecting() || !self.rows.is_empty()) => {
                if self.selecting() {
                    self.deselect();
                } else {
                    self.visual = Some(self.selected());
                }
            }
            KeyCode::Char('n') if row => {
                let m = self.current()?;
                let text = self.store.note(&m.key).unwrap_or("").to_string();
                self.input = Input::Note { key: m.key.clone(), cur: text.len(), text };
            }
            KeyCode::Char('o') if row => {
                let m = self.current()?;
                let items: Vec<_> = m.links().into_iter().map(|(site, url)| (site.into(), Effect::Open(url))).collect();
                if items.is_empty() {
                    // Worded by `url`, as for the CLI.
                    let none = m.url().err().unwrap_or_default();
                    self.refuse(none);
                } else {
                    self.input = Input::choose("open on which site?", Kind::Open, items, 0);
                }
            }
            // A list even of one, as `o`'s.
            KeyCode::Char('x') if row => {
                let m = self.current()?;
                let known = self.downloads.get(&m.key).copied();
                let size = match known {
                    Some(Download::Size(b)) => format!(" ({})", crate::data::gb(b)),
                    _ => String::new(),
                };
                let gone = known == Some(Download::Gone);
                let items: Vec<_> = m
                    .via
                    .iter()
                    .filter_map(|h| launch_cmd(m, h, &self.data.harness))
                    .map(|c| (c.join(" "), Effect::Launch(c)))
                    .chain(get_items(m, &size, &self.tools).into_iter().filter(|_| !gone))
                    .collect();
                // Asked already where the cursor rested on the model, and again here when no
                // answer came.
                let ask = (matches!(known, None | Some(Download::NoAnswer))
                    && items.iter().any(|i| got(&i.1).is_some()))
                .then(|| (m.key.clone(), m.hf_repo().unwrap_or("").to_string()));
                if items.is_empty() {
                    let none = format!("no harness has {}; Via shows where you have access", m.name);
                    // A download was all there was to offer.
                    let why = if gone && !get_items(m, "", &self.tools).is_empty() { no_copy(&m.name) } else { none };
                    self.refuse(why);
                } else {
                    // On your default harness when it has the model.
                    let on = |e: &Effect| matches!(e, Effect::Launch(c) if c[0] == self.store.harness);
                    let sel = items.iter().position(|(_, e)| on(e)).unwrap_or(0);
                    self.input = Input::choose("open in which harness?", Kind::Launch, items, sel);
                    let (key, repo) = ask?;
                    self.downloads.insert(key.clone(), Download::Awaited);
                    return Some(Effect::Size(key, repo));
                }
            }
            // The id your default harness takes when it has the model, as `--id`.
            KeyCode::Char('Y') if row => {
                let m = self.current()?;
                let own = launch_cmd(m, &self.store.harness, &self.data.harness).and_then(|mut c| c.pop());
                return Some(Effect::Copy(own.unwrap_or_else(|| model_id(m, &self.data.harness))));
            }
            KeyCode::Char('y') if row => return Some(Effect::Copy(self.current()?.name.clone())),
            KeyCode::Char(' ') if table && self.selecting() => {
                return self.flag(Store::is_marked, Store::toggle_marked, ["selected", "deselected"]);
            }
            KeyCode::Char(' ') if row && self.view != View::Compare => {
                self.toggle_mark();
                return Some(Effect::Save);
            }
            KeyCode::Char('C') if self.view == View::Compare => self.view = View::Table,
            // One row is nothing to compare, and would replace the selection, which is saved.
            KeyCode::Char('C') if table && self.selecting() && self.targets().len() < 2 => {
                self.refuse("compare takes 2 models, 1 is highlighted")
            }
            KeyCode::Char('C') => {
                // A selection is what gets compared.
                let save = table && self.selecting();
                if save {
                    self.store.marked = self.targets();
                    self.deselect();
                    self.rebuild_in_place();
                }
                // With fewer than 2 marked, the overlay says how to mark them.
                self.view = View::Compare;
                self.overlay_query.clear();
                self.scroll = 0;
                self.compare_sel = 0;
                self.compare_x = 0;
                return save.then_some(Effect::Save);
            }
            KeyCode::Char('r') => return self.refresh(),
            KeyCode::Char('t') => {
                let items = THEMES.iter().map(|t| (t.0.to_string(), Effect::Theme(t.0))).collect();
                self.input = Input::choose("theme?", Kind::Theme, items, crate::view::theme(&self.store.theme));
            }
            KeyCode::Char('B') => self.ask_source(),
            KeyCode::Char('|') => self.ask_cols(),
            KeyCode::Char('H') => self.ask_harness(),
            KeyCode::Enter if self.view == View::Recommend && !row => {
                let Some(t) = self.cur_task() else {
                    // A task of your own has no line to rank: the table, on its cheapest model.
                    let line = self.custom_at().map_or(vec![], |t| self.custom_line(t));
                    let first = line.first().map(|m| (m.key.clone(), m.name.clone()));
                    let key = first.as_ref().map(|(k, _)| k.clone());
                    // A built-in task picked before would keep the table to its line, as esc undoes.
                    if self.task.take().is_some() {
                        (self.sort_col, self.descending) = default_sort();
                        self.rebuild();
                    }
                    let row = key.and_then(|k| self.rows.iter().position(|&i| self.data.models[i].key == k));
                    let said = self.custom_at().map(|task| match (&first, row) {
                        (Some((_, m)), Some(_)) => Ok(format!("{m}, your model for {task}")),
                        (Some((_, m)), None) => Err(format!("{m}, your model for {task}, is filtered out: c clears")),
                        (None, _) => Err(format!("the model of {task} is not one you can use")),
                    });
                    self.view = View::Table;
                    if let Some(n) = row {
                        self.deselect();
                        self.select(n);
                    }
                    if let Some(said) = said {
                        self.report(said);
                    }
                    return None;
                };
                self.task = Some(t);
                // The line is drawn from the task's score, so its column shows that.
                self.drop_benches(true);
                // On the frontier the priciest is the best: each row down is cheaper and scores lower.
                (self.sort_col, self.descending) = (PRICE, true);
                self.view = View::Table;
                self.rebuild();
                self.select(0);
                // A favorite on the line only for being one is neither best nor cheapest,
                // though "value" may score highest a model cut on coding, and one on the
                // frontier may cost more than the best.
                let front = self.task_frontier(t).into_iter().filter(|(m, _)| !self.favorite_unrecommended(t, &m.key));
                let front: Vec<_> = front.collect();
                let best = front.iter().max_by(|a, b| a.1.total_cmp(&b.1));
                match (best, front.first()) {
                    (Some(a), Some(b)) => self.status = format!("{} best, {} cheapest", a.0.name, b.0.name),
                    _ => self.refuse(format!("no model has data for {}", t.name)),
                }
            }
            // Recommend's enter on a task's name is above.
            KeyCode::Enter if row && !matches!(self.view, View::Detail(_)) => {
                self.detail = self.current()?.key.clone();
                self.view = View::Detail(match self.view {
                    View::Compare => Back::Compare(self.scroll),
                    View::Recommend => Back::Recommend(self.scroll),
                    _ => Back::Table,
                });
                self.scroll = 0;
            }
            // A new search starts empty; esc brings the previous one back.
            // Said, as every key that has nothing to act on here says why.
            KeyCode::Char('/') if !(table || self.overlay_search()) => {
                self.refuse("/ filters the table, compare and the lists")
            }
            KeyCode::Char('/') if table || self.overlay_search() => {
                self.input = Input::Search { cur: 0, was: std::mem::take(self.search_target()) };
                if table {
                    self.rebuild();
                }
            }
            // With access to none every model shows already.
            KeyCode::Char('a' | 'A') if table && self.no_access() => self.refuse(NO_ACCESS),
            // To the all tab, and `A` to yours, out of `S` `F` `E` too.
            KeyCode::Char('a') if table => return self.set_tab(ALL),
            KeyCode::Char('A') if table => return self.set_tab(YOURS),
            KeyCode::Char('%') if table => {
                let off = crate::data::cached() > 0.0;
                crate::data::set_cached(if off { 0.0 } else { self.cache_on });
                self.reprice();
                self.status = if off {
                    "no input cached, as a one-off prompt".into()
                } else {
                    format!("{:.0}% of the input cached", self.cache_on * 100.0)
                };
            }
            _ => {}
        }
        None
    }

    /// Keys while typing a search, a note or a bound.
    fn input_key(&mut self, code: KeyCode, mods: KeyModifiers) -> Option<Effect> {
        if self.editing() {
            return self.edit_key(code, mods);
        }
        match &mut self.input {
            Input::Search { cur, was } => {
                let overlay = matches!(self.view, View::Compare | View::Help);
                let query = if overlay { &mut self.overlay_query } else { &mut self.query };
                let before = query.clone();
                match code {
                    KeyCode::Enter => self.input = Input::None,
                    KeyCode::Esc => {
                        *query = std::mem::take(was);
                        self.input = Input::None;
                    }
                    KeyCode::Down | KeyCode::Up => {
                        self.move_by(if code == KeyCode::Down { 1 } else { -1 }, true);
                        return None;
                    }
                    _ if edit(query, cur, code, mods, |_| true) => {}
                    _ => return None,
                }
                // Not on a key that only moves in the text.
                if !overlay && self.query != before {
                    self.rebuild();
                }
            }
            Input::Key { text, cur, .. } => match code {
                KeyCode::Enter if !text.trim().is_empty() => {
                    let res = crate::data::save_aa_key(text);
                    self.input = Input::None;
                    return match res {
                        // A refresh under way has the old key: start over, the shell drops it.
                        Ok(()) if crate::data::source() == Source::Aa => {
                            self.refreshing = false;
                            self.refresh()
                        }
                        Ok(()) => self.switch(Source::Aa),
                        Err(e) => {
                            self.report(Err(format!("could not save the key: {e}")));
                            None
                        }
                    };
                }
                // On the first start, back to the choice: there is no data to go back to.
                KeyCode::Esc if self.first_start => self.ask_source(),
                KeyCode::Esc => self.input = Input::None,
                _ => drop(edit(text, cur, code, mods, |_| true)),
            },
            Input::Note { key, text, cur } => match code {
                KeyCode::Enter => {
                    let (key, text) = (std::mem::take(key), std::mem::take(text));
                    self.input = Input::None;
                    self.store.set_note(&key, &text);
                    // Search and the Notes sort read notes.
                    self.rebuild_in_place();
                    return Some(Effect::Save);
                }
                KeyCode::Esc => self.input = Input::None,
                _ => drop(edit(text, cur, code, mods, |_| true)),
            },
            Input::Bound { col, min, text, cur } => match code {
                KeyCode::Esc => self.input = Input::None,
                // `-` for a date typed as Released shows it.
                _ if edit(text, cur, code, mods, |c| matches!(c, '0'..='9' | '.' | '-')) => {}
                KeyCode::Enter => {
                    let (col, min, text) = (*col, *min, std::mem::take(text));
                    self.input = Input::None;
                    match numeric(col).and_then(|c| (c.read)(&text, !min)) {
                        Some(v) => {
                            let (lo, hi) = if min { (v, f64::INFINITY) } else { (f64::NEG_INFINITY, v) };
                            // A new bound on a column replaces the old one of the same kind.
                            self.bounds.retain(|&(c, l, _)| c != col || l.is_finite() != lo.is_finite());
                            self.bounds.push((col, lo, hi));
                        }
                        None if text.is_empty() => {}
                        None => self.report(Err(format!("{text} is not a value for {}", self.col_name(col)))),
                    }
                    self.rebuild();
                }
                _ => {}
            },
            Input::Menu { col, items, list } => {
                let rows = menu_rows(items, &list.query);
                let at = rows.get(list.sel).copied();
                match code {
                    // `q` asks to quit from an open list as from the table; while typing it is typed.
                    KeyCode::Char('q') if list.idle() => self.ask_quit(),
                    // Enter does what space does; while searching, space is typed. On Price it
                    // picks the level, or drops it when it is the picked one; on Dev and Via it
                    // adds or drops the entry, as space marks a model, and on "any" drops all.
                    // The dropdown stays open.
                    KeyCode::Enter | KeyCode::Char(' ') if code == KeyCode::Enter || !list.typing => {
                        let i = at?;
                        if *col == PRICE {
                            self.set_price_level(if self.price_level() == i.checked_sub(1) { 0 } else { i });
                        } else if col_benches(*col).is_some() {
                            let col = *col;
                            self.pick_bench(col, i);
                            // The pick drops the column's bounds, which the counts were under.
                            let fresh = self.menu_items(col).0;
                            if let Input::Menu { items, .. } = &mut self.input {
                                *items = fresh;
                            }
                        } else {
                            let picked = if *col == 1 { &mut self.dev } else { &mut self.via };
                            match picked.iter().position(|d| *d == items[i].0) {
                                _ if i == 0 => picked.clear(),
                                Some(k) => drop(picked.remove(k)),
                                None => picked.push(items[i].0.clone()),
                            }
                        }
                        self.rebuild();
                    }
                    _ if list.key(code, mods, at, |q| menu_rows(items, q).len(), true) => {}
                    KeyCode::Esc => self.input = Input::None,
                    _ => {}
                }
            }
            Input::Choose { kind, items, list, .. } => {
                let at = choice_rows(items, &list.query).get(list.sel).copied();
                match code {
                    // `q` asks to quit from an open list as from the table; while typing it is typed.
                    // Not on the first start's choice, which has no table to go back to.
                    KeyCode::Char('q') if list.idle() && !self.first_start => self.ask_quit(),
                    // Space ticks a column and keeps the list open, as in f's grid, and so does enter.
                    KeyCode::Char(' ') | KeyCode::Enter
                        if *kind == Kind::Cols && (code == KeyCode::Enter || !list.typing) =>
                    {
                        let Some((_, Effect::Col(c))) = items.get(at?) else { return None };
                        let c = *c;
                        return self.toggle_col(c);
                    }
                    // Space ticks a task in f's grid and keeps it open, as in the Dev and Via
                    // dropdowns, and so does enter. While searching, space is typed.
                    KeyCode::Char(' ') | KeyCode::Enter
                        if *kind == Kind::Fav && (code == KeyCode::Enter || !list.typing) =>
                    {
                        match items.get(at?) {
                            Some((_, Effect::Fav(key, task))) => {
                                let (key, slot) = (key.clone(), box_slot(task, list.col));
                                return self.fav(&key, &slot);
                            }
                            // The name of a new task is written right there, the list open.
                            Some((_, Effect::NewTask(_))) => list.write(at?, Edit::new(What::New, String::new())),
                            _ => {}
                        }
                    }
                    // `v` lists the harnesses for the ticked box under the cursor.
                    KeyCode::Char('v') if *kind == Kind::Fav && !list.typing => {
                        if let Some((_, Effect::Fav(key, task))) = items.get(at?) {
                            let (key, slot) = (key.clone(), box_slot(task, list.col));
                            self.ask_via(&key, &slot);
                        }
                    }
                    // `r` renames the task of your own under the cursor and `a` writes what it is
                    // about, both on its row; a built-in one keeps both, and one with nothing
                    // ticked is no task yet.
                    KeyCode::Char(c @ ('r' | 'a')) if *kind == Kind::Fav && !list.typing => {
                        let Some((_, Effect::Fav(_, task))) = items.get(at?) else { return None };
                        let task = task.clone();
                        if crate::fit::task(&task).is_some() {
                            self.refuse(format!("{task} is built in: r and a are for a task of your own"));
                        } else if !self.store.custom_tasks().contains(&task.as_str()) {
                            self.refuse(format!("{task} has no model: tick a box of it first"));
                        } else {
                            let edit = if c == 'r' {
                                Edit::new(What::Rename(task.clone()), task)
                            } else {
                                let about = self.store.about(&task).unwrap_or("").to_string();
                                Edit::new(What::About(task), about)
                            };
                            list.write(at?, edit);
                        }
                    }
                    KeyCode::Enter => {
                        let (_, effect) = items.swap_remove(at?);
                        self.input = Input::None;
                        match effect {
                            Effect::Theme(name) => {
                                self.store.theme = if name == THEMES[0].0 { String::new() } else { name.to_string() };
                                self.report(Ok(format!("theme {name}")));
                                return Some(Effect::Save);
                            }
                            // Asked once: a key it turns down is asked for again (`refreshed`), and
                            // picking it again when it is the source changes a saved key.
                            Effect::Source(Source::Aa)
                                if crate::data::aa_key().is_none()
                                    || (crate::data::source() == Source::Aa
                                        && !self.first_start
                                        && crate::data::aa_key_env().is_none()) =>
                            {
                                self.input = Input::Key { text: String::new(), cur: 0, wrong: false };
                            }
                            Effect::Source(src) => return self.switch(src),
                            Effect::Harness(h) => {
                                self.store.harness = h.unwrap_or("").to_string();
                                self.report(Ok(format!("default harness: {}", h.unwrap_or("any harness"))));
                                return Some(Effect::Save);
                            }
                            Effect::Via(key, slot, h) => {
                                self.store.set_favorite(&slot, &key, h.as_deref());
                                let on = h.as_deref().unwrap_or("any harness");
                                self.report(Ok(format!("★ the favorite for {slot} runs on {on}")));
                                self.fav_at(&key, &slot);
                                return Some(Effect::Save);
                            }
                            effect => return Some(effect),
                        }
                    }
                    _ if list.key(code, mods, at, |q| choice_rows(items, q).len(), false) => {}
                    // Closing the first start's choice picks the default.
                    KeyCode::Esc if self.first_start && *kind == Kind::Source => {
                        self.input = Input::None;
                        return self.switch(Source::preferred());
                    }
                    // Esc leaves the harnesses for f's grid they were opened from, on the same task.
                    KeyCode::Esc if *kind == Kind::Via => {
                        if let Some((_, Effect::Via(key, slot, _))) = items.first() {
                            let (key, slot) = (key.clone(), slot.clone());
                            self.fav_at(&key, &slot);
                        }
                    }
                    // The key that opens the theme list also closes it, and so the columns'.
                    KeyCode::Char('t') if *kind == Kind::Theme => self.input = Input::None,
                    KeyCode::Char('|') if *kind == Kind::Cols => self.input = Input::None,
                    // And it is a tab, which tab leaves for the next.
                    KeyCode::Tab | KeyCode::BackTab if *kind == Kind::Theme => {
                        return self.step_tab(code == KeyCode::BackTab);
                    }
                    KeyCode::Esc => self.input = Input::None,
                    _ => {}
                }
            }
            Input::Quit => {
                if code == KeyCode::Char('q') {
                    return Some(Effect::Quit);
                }
                self.input = std::mem::replace(&mut self.asked_from, Input::None);
            }
            Input::Upgrade => {
                self.input = Input::None;
                if code == KeyCode::Char('U') {
                    return Some(Effect::Upgrade);
                }
            }
            Input::Unfavorite => {
                self.input = Input::None;
                if code == KeyCode::Char('D') {
                    // A harness is its favorite's, and goes with it.
                    self.store.favorite.clear();
                    self.store.via.clear();
                    self.rebuild_in_place();
                    self.status = "unfavorited every model".into();
                    return Some(Effect::Save);
                }
            }
            Input::Unexclude => {
                self.input = Input::None;
                if code == KeyCode::Char('X') {
                    self.store.excluded.clear();
                    self.rebuild_in_place();
                    self.status = "unexcluded every model".into();
                    return Some(Effect::Save);
                }
            }
            Input::None => {}
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `--min ctx=600` and `--min ctx=600000` are the same 600,000 tokens.
    #[test]
    fn a_context_bound_reads_tokens_or_thousands() {
        let read = COLS.iter().find(|c| c.id == "ctx").unwrap().read;
        assert_eq!((read("600", false), read("600000", false)), (Some(600.0), Some(600.0)));
    }
    use crate::data::Offer;
    use crate::fit;
    use crate::view::frontier;

    /// ECI and AAII keep their columns whichever source is in use: the cursor and the sort move.
    #[test]
    fn the_indexes_keep_their_columns() {
        let mut a = app();
        assert_eq!((a.col, a.sort_col, by_role(ECI - TEXT) + TEXT), (ECI, ECI, ECI));
        a.switch(Source::Aa);
        assert_eq!([ECI, AAII].map(base_col_name), ["ECI", "AAII"]);
        assert_eq!((a.col, a.sort_col), (AAII, AAII), "they follow the index of the source");
        assert!(hidden(ECI) && !hidden(AAII), "Epoch's index shows with its data alone");
        assert_eq!([ECI, AAII].map(|c| by_role(c - TEXT) + TEXT), [AAII, ECI], "`eci` on the command line is AAII");
    }

    /// `|` ticks the columns the table shows, saved, and the other source's show with its data.
    #[test]
    fn columns_are_picked_and_the_other_sources_show_with_its_data() {
        let col = |id: &str| TEXT + COLS.iter().position(|c| c.id == id).unwrap();
        let (tps, other, price) = (col("tps"), col(OTHER), col("price"));
        let mut a = app();
        assert!(hidden(tps) && hidden(other), "Epoch alone has no speed, nor another index");
        // With Artificial Analysis's data too, its columns show, the index under its name.
        let mut data = std::mem::take(&mut a.data);
        data.lent = true;
        a.set_data(data);
        assert!(!hidden(tps) && !hidden(other));
        assert_eq!(base_col_name(other), "AAII");
        // The list opens on the cursor's column, and a tick leaves it out, the cursor off it.
        a.col = price;
        a.bounds = vec![(price, 0.0, 1.0)];
        press(&mut a, "|");
        assert_eq!(code(&mut a, KeyCode::Enter), Some(Effect::Save));
        assert!(hidden(price) && a.store.hide.contains("price") && a.choosing_cols(), "and the list stays open");
        assert_eq!((a.col, a.bounds.len()), (ECI, 0), "nor a bound on it");
        press(&mut a, "]");
        assert!(matches!(&a.input, Input::Choose { list, .. } if list.sel == 2), "] to the next ticked one");
        code(&mut a, KeyCode::Esc);
        a.col = PRICE - 1;
        press(&mut a, "l");
        assert_eq!(a.col, PRICE + 1, "l steps over it");
        a.col = 0;
        press(&mut a, "w");
        assert_eq!(a.col, PRICE + 1, "and its group starts at the next one");
        // Via and Notes too: `$` is the last column shown.
        a.store.hide.extend(["via".to_string(), "notes".to_string()]);
        a.set_cols();
        press(&mut a, "$");
        assert_eq!(a.col, col("ttft"));
        // Ticked again, it is back.
        a.col = PRICE + 1;
        press(&mut a, "|k");
        code(&mut a, KeyCode::Enter);
        assert!(!hidden(price) && !a.store.hide.contains("price"));
    }

    fn model(key: &str, available: bool, coding: Option<f64>, price: f64) -> Model {
        let mut m = Model {
            key: key.into(),
            name: key.into(),
            available,
            openrouter: Some(format!("x/{key}")),
            eci: coding.map(|c| c + 100.0),
            epoch: coding.map(|_| key.into()),
            offers: vec![Offer {
                provider: "p".into(),
                id: key.into(),
                input: price,
                output: price,
                ..Default::default()
            }],
            ..Default::default()
        };
        // A point of the score is 0.4 months: `low` reaches 20 points under the best, `mid` 7.5.
        if let Some(c) = coding {
            m.fit.insert("coding".into(), c);
            m.lag.insert("coding".into(), (100.0 - c) / 2.5);
        }
        m
    }

    fn app() -> App {
        let models = vec![
            model("gpt55", true, Some(80.0), 10.0),
            model("opus5", true, None, 5.0),
            model("llama4", false, Some(40.0), 1.0),
            model("mini", true, Some(60.0), 1.0),
        ];
        let mut models = models;
        for (m, dev) in models.iter_mut().zip(["openai", "anthropic", "meta", "openai"]) {
            m.developer = dev.into();
            m.via = match dev {
                "openai" => vec!["codex".into(), "opencode".into()],
                "anthropic" => vec!["claude".into()],
                _ => vec![],
            };
        }
        App::new(Data { fetched: 0, models, ..Default::default() }, Store::default())
    }

    #[test]
    fn tabs_show_one_set_of_models() {
        let mut a = app();
        let on = |a: &App| (0..TABS.len()).filter(|&i| a.tab_on(i)).collect::<Vec<_>>();
        assert_eq!((on(&a), a.rows.len()), (vec![0], 3), "yours at start");
        code(&mut a, KeyCode::Tab);
        assert_eq!((on(&a), a.rows.len()), (vec![1], 4), "tab: all");
        code(&mut a, KeyCode::Tab);
        assert_eq!((on(&a), &a.view), (vec![5], &View::Recommend), "the tabs with nothing to show are skipped");
        code(&mut a, KeyCode::Tab);
        assert!(matches!(a.input, Input::Choose { kind: Kind::Theme, .. }) && on(&a) == [7], "then the theme list");
        code(&mut a, KeyCode::Tab);
        assert_eq!((on(&a), &a.view, &a.input), (vec![8], &View::Help, &Input::None), "which tab leaves for help");
        a.mouse(Mouse::Tab(5));
        assert_eq!((on(&a), &a.view), (vec![5], &View::Recommend), "a click on a tab leaves help for it");
        a.mouse(Mouse::Tab(8));
        code(&mut a, KeyCode::Tab);
        assert_eq!((on(&a), &a.view), (vec![0], &View::Table), "past help, the last, back to yours");
        a.mouse(Mouse::Tab(5));
        a.mouse(Mouse::Tab(5));
        assert_eq!(a.view, View::Recommend, "a click on recommend opens it, and again leaves it open");
        a.mouse(Mouse::Tab(1));
        assert_eq!((on(&a), &a.view), (vec![1], &View::Table), "a click on a tab leaves recommend for it");
        press(&mut a, "A ");
        a.mouse(Mouse::Tab(6));
        assert_eq!((on(&a), &a.view), (vec![6], &View::Compare), "compare's tab opens it, as C does");
        a.mouse(Mouse::Tab(0));
        code(&mut a, KeyCode::BackTab);
        code(&mut a, KeyCode::BackTab);
        assert_eq!(on(&a), [7], "shift+tab goes back from yours to help, then the theme list");
        code(&mut a, KeyCode::BackTab);
        assert_eq!(on(&a), [5], "with one model selected, shift+tab goes past compare");
        code(&mut a, KeyCode::BackTab);
        assert_eq!(
            (on(&a), a.rows.len()),
            (vec![2], 1),
            "shift+tab goes back, past the empty ones to the selected only"
        );
        a.mouse(Mouse::Tab(3));
        assert_eq!((on(&a), a.status.contains("no favorites")), (vec![2], true), "an empty tab says why");
        press(&mut a, "R");
        a.status.clear();
        a.mouse(Mouse::Tab(3));
        assert_eq!((&a.view, a.status.contains("no favorites")), (&View::Recommend, true), "and leaves a panel open");
        press(&mut a, "Rt");
        a.mouse(Mouse::Tab(7));
        assert!(matches!(a.input, Input::Choose { kind: Kind::Theme, .. }), "a click on theme's tab keeps its list");
        a.mouse(Mouse::Tab(5));
        assert_eq!((&a.input, &a.view), (&Input::None, &View::Recommend), "one on another tab leaves it for that tab");
        press(&mut a, "R");
        a.store.toggle_favorite("coding", "opus5");
        a.rebuild();
        a.mouse(Mouse::Tab(3));
        assert_eq!((on(&a), a.rows.len()), (vec![3], 1), "a click on ★ leaves ✓ for the favorites, as F does");
        a.mouse(Mouse::Tab(3));
        assert_eq!(on(&a), [0], "and one on a tab that is on takes it off");
        press(&mut a, "Fa");
        assert_eq!((on(&a), a.rows.len()), (vec![1], 4), "a goes to all, out of ★");
        press(&mut a, "a");
        assert_eq!(on(&a), [1], "a again stays on all");
        press(&mut a, "FA");
        assert_eq!((on(&a), a.rows.len()), (vec![0], 3), "A goes to yours");
        a.mouse(Mouse::Tab(1));
        assert_eq!((on(&a), a.all, a.rows.len()), (vec![1], true, 4), "a click on all shows it alone");
        for m in &mut a.data.models {
            m.available = false;
        }
        let mut a = App::new(std::mem::take(&mut a.data), Store::default());
        a.mouse(Mouse::Tab(0));
        assert_eq!((on(&a), a.status.as_str()), (vec![1], NO_ACCESS), "with access to none, all is the tab on");
        a.mouse(Mouse::Tab(1));
        assert!(!a.all, "and a stays off");
    }

    #[test]
    fn tabs_work_from_the_details_and_skip_what_would_show_nothing() {
        let mut a = app();
        code(&mut a, KeyCode::Enter);
        code(&mut a, KeyCode::Tab);
        assert_eq!((&a.view, a.all), (&View::Table, true), "tab leaves the details for the next tab");
        code(&mut a, KeyCode::Enter);
        a.mouse(Mouse::Tab(RECOMMEND));
        assert_eq!(a.view, View::Recommend, "and so does a click on one");
        press(&mut a, "RA");
        let out = a.data.models.iter().find(|m| !m.available).unwrap().key.clone();
        a.store.toggle_excluded(&out);
        a.rebuild();
        press(&mut a, "E");
        assert_eq!(
            (a.only == Some(EXCLUDED), a.rows.len()),
            (true, 1),
            "E shows an excluded model out of reach from yours"
        );
        press(&mut a, "aE");
        assert_eq!((a.only == Some(EXCLUDED), a.rows.len()), (true, 1), "and the same from all");
        a.store.toggle_excluded(&out);
        a.store.toggle_favorite("coding", &out);
        a.task = TASKS.iter().find(|t| t.name == "coding");
        press(&mut a, "AF");
        assert_eq!(keys(&a), [out.as_str()], "F shows a picked task's favorite out of reach, off its line");
        a.task = None;
        a.data.models.retain(|m| m.key != out);
        a.rebuild();
        assert!(!a.tab_has(EXCLUDED) && a.only != Some(EXCLUDED), "and gone from the data it is none again");
    }

    #[test]
    fn percent_toggles_the_cached_input_in_price() {
        let mut a = app();
        let gpt = a.data.models.iter().position(|m| m.key == "gpt55").unwrap();
        a.data.models[gpt].offers[0].cache_read = Some(0.5);
        a.reprice();
        let sel = |a: &App| a.current().unwrap().key.clone();
        press(&mut a, "gg");
        let was = sel(&a);
        // 3 × (0.9 × 0.5 + 0.1 × 10) + 10, over 4.
        assert_eq!(a.val(gpt, PRICE), Some(3.5875));
        press(&mut a, "%");
        assert_eq!((a.val(gpt, PRICE), sel(&a)), (Some(10.0), was), "full input price, the cursor stays");
        assert!(base_col_about(PRICE).contains(" 0% of the input cached") && a.status.contains("one-off"));
        press(&mut a, "%");
        assert_eq!(a.val(gpt, PRICE), Some(3.5875), "again: back to an agent's 90%");
        // Started with --cache 50, % goes back to 50, not 90.
        crate::data::set_cached(0.5);
        let mut a = app();
        press(&mut a, "%%");
        assert_eq!((crate::data::cached(), a.status.as_str()), (0.5, "50% of the input cached"));
        press(&mut a, "%");
        assert_eq!(a.cache_hint, "% 50% cached", "the hint names it");
    }

    /// One check per defect a review found, each of which did the wrong thing before.
    #[test]
    fn review_fixes_hold() {
        let mut a = app();
        press(&mut a, "G");
        let last = a.selected();
        a.mouse(Mouse::Scroll(1));
        assert_eq!(a.selected(), last, "the wheel stops at the last row");
        press(&mut a, "j");
        assert_eq!(a.selected(), 0, "j goes round");
        assert_eq!(a.key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL)), None);
        assert!(a.store.excluded.is_empty(), "^e is not e");
        let key = a.current().unwrap().key.clone();
        press(&mut a, "nfast");
        a.select(1);
        assert_eq!(code(&mut a, KeyCode::Enter), Some(Effect::Save));
        assert_eq!(a.store.note(&key), Some("fast"), "a note stays with the model it was opened on");
        a.col = NOTES;
        for order in ["A-Z", "Z-A"] {
            press(&mut a, "s");
            assert_eq!(a.data.models[a.rows[0]].key, key, "models without a note sort last, {order}");
        }
        press(&mut a, "t/nor");
        assert_eq!(a.mouse(Mouse::Item(0)), Some(Effect::Save), "a click picks in a list being searched");
        assert_eq!(a.store.theme, "nord");
        press(&mut a, "t/nor");
        a.mouse(Mouse::Outside);
        assert_eq!(a.input, Input::None, "and a click outside closes it");
        a.store.toggle_marked("gpt55");
        a.store.toggle_marked("mini");
        press(&mut a, "Cl");
        a.store.toggle_marked("mini");
        assert_eq!(a.current().map(|m| m.key.as_str()), Some("gpt55"), "compare's cursor stays on a model");
    }

    #[test]
    fn t_picks_a_theme_from_a_panel() {
        let mut a = app();
        assert_eq!(press(&mut a, "tj"), None);
        assert_eq!((a.theme_preview(), a.store.theme.as_str()), (Some("gruvbox"), ""), "previewed, not saved");
        press(&mut a, "t");
        assert_eq!((a.theme_preview(), &a.input), (None, &Input::None), "t closes it unchanged");
        press(&mut a, "tjj");
        assert_eq!(a.key(KeyEvent::from(KeyCode::Enter)), Some(Effect::Save));
        assert_eq!((a.store.theme.as_str(), a.status.as_str()), ("nord", "theme nord"));
        press(&mut a, "t");
        assert_eq!(a.theme_preview(), Some("nord"), "opens on the saved theme");
        press(&mut a, "gg");
        a.key(KeyEvent::from(KeyCode::Enter));
        assert_eq!(a.store.theme, "", "the terminal's colours are saved as nothing");
    }

    #[test]
    fn slash_searches_a_choice_list_and_the_help() {
        let mut a = app();
        press(&mut a, "t/nor");
        assert_eq!(a.theme_preview(), Some("nord"), "/ keeps the matching themes, the cursor on the first");
        code(&mut a, KeyCode::Esc);
        assert_eq!(a.theme_preview(), Some("nord"), "esc drops the search, keeping the theme under the cursor");
        assert_eq!(code(&mut a, KeyCode::Enter), Some(Effect::Save));
        assert_eq!(a.store.theme, "nord");
        press(&mut a, "t/zzz");
        assert_eq!(a.theme_preview(), None, "nothing matches, so enter picks nothing");
        assert!(matches!(a.input, Input::Choose { kind: Kind::Theme, .. }), "and it is the theme list still");
        assert_eq!(code(&mut a, KeyCode::Enter), None);
        code(&mut a, KeyCode::Esc);
        code(&mut a, KeyCode::Esc);
        // f's tasks are searched by name, and a tick keeps the row under the cursor.
        press(&mut a, "f/now");
        assert!(matches!(&a.input, Input::Choose { items, list, .. } if choice_rows(items, &list.query).is_empty()));
        code(&mut a, KeyCode::Esc);
        press(&mut a, "/codi");
        let ticked = |a: &App| match &a.input {
            Input::Choose { items, list, .. } => {
                (choice_rows(items, &list.query).len(), a.store.favorite("coding").is_some())
            }
            _ => (0, false),
        };
        assert_eq!(ticked(&a), (1, false));
        code(&mut a, KeyCode::Enter);
        assert_eq!(ticked(&a), (1, true), "ticked, and still under the cursor");
        code(&mut a, KeyCode::Enter);
        assert_eq!(ticked(&a), (1, false), "enter again unticks it");
        code(&mut a, KeyCode::Esc);
        code(&mut a, KeyCode::Esc);
        press(&mut a, "?/sort");
        assert_eq!((&a.view, a.overlay_query.as_str(), a.query.as_str()), (&View::Help, "sort", ""));
        code(&mut a, KeyCode::Enter);
        code(&mut a, KeyCode::Esc);
        assert_eq!((&a.view, a.overlay_query.as_str()), (&View::Help, ""), "esc clears the filter first");
        code(&mut a, KeyCode::Esc);
        assert_eq!(a.view, View::Table);
    }

    #[test]
    fn h_picks_the_default_harness() {
        let mut a = app();
        for m in &mut a.data.models {
            m.via = vec!["opencode".into(), "codex".into()];
            m.offers[0].via = m.via.clone();
        }
        let at = |a: &App| match &a.input {
            Input::Choose { kind, items, list, .. } => Some((*kind, items.len(), list.sel)),
            _ => None,
        };
        press(&mut a, "H");
        assert_eq!(at(&a), Some((Kind::Harness, 3, 0)), "any, opencode and codex: on any, it has none yet");
        press(&mut a, "jj");
        assert_eq!(code(&mut a, KeyCode::Enter), Some(Effect::Save));
        assert_eq!((a.store.harness.as_str(), a.status.as_str()), ("codex", "default harness: codex"));
        let key = a.current().map(|m| m.key.clone()).unwrap_or_default();
        assert_eq!(press(&mut a, "Y"), Some(Effect::Copy(key)), "the id codex takes, not opencode's p/id");
        press(&mut a, "x");
        assert_eq!(at(&a), Some((Kind::Launch, 2, 1)), "x starts on it");
        code(&mut a, KeyCode::Esc);
        press(&mut a, "H");
        assert_eq!(at(&a), Some((Kind::Harness, 3, 2)), "on yours");
        press(&mut a, "gg");
        assert_eq!(code(&mut a, KeyCode::Enter), Some(Effect::Save));
        assert_eq!(a.store.harness, "", "any harness: none");
    }

    #[test]
    fn b_picks_the_benchmark_source() {
        let mut a = app();
        // `--source aa` with none ever picked: closing the chooser keeps it.
        crate::data::set_source(Source::Aa);
        press(&mut a, "B");
        assert!(code(&mut a, KeyCode::Esc).is_none() && a.input == Input::None);
        assert_eq!((a.store.source.as_str(), crate::data::source()), ("", Source::Aa));
        crate::data::set_source(Source::Epoch);
        a.first_start = true;
        a.ask_source();
        assert!(a.mouse(Mouse::Outside).is_none() && matches!(a.input, Input::Choose { .. }), "a click beside it");
        let label = |a: &App, i: usize| match &a.input {
            Input::Choose { items, .. } => items[i].0.clone(),
            _ => String::new(),
        };
        assert!(label(&a, 0).contains("no API key") && label(&a, 1).contains("needs an API key"));
        press(&mut a, "j");
        code(&mut a, KeyCode::Enter);
        assert!(matches!(a.input, Input::Key { .. }), "Artificial Analysis asks for its key");
        code(&mut a, KeyCode::Esc);
        assert!(matches!(a.input, Input::Choose { .. }), "on the first start, esc goes back to the choice");
        // Closing the first start's choice picks the default, and loads it.
        assert!(matches!(code(&mut a, KeyCode::Esc), Some(Effect::Source(Source::Epoch))));
        assert_eq!((a.store.source.as_str(), crate::data::source()), ("epoch", Source::Epoch));
        press(&mut a, "B");
        assert!(code(&mut a, KeyCode::Enter).is_none(), "picked before: nothing to switch");
        // A source never downloaded shows nothing until it is, not the other one's scores.
        assert!(a.switched(None) && a.data.models.is_empty() && a.refreshing);
    }

    #[test]
    fn a_rejected_key_is_asked_for_again() {
        let mut a = app();
        a.refreshed(Err(Failure::BadKey), tools());
        assert!(matches!(a.input, Input::Key { wrong: true, .. }));
        // A new key while the old one's refresh is under way starts another.
        crate::data::set_source(Source::Aa);
        a.refreshing = true;
        press(&mut a, "new");
        assert_eq!(code(&mut a, KeyCode::Enter), Some(Effect::Refresh));
        assert_eq!(crate::data::aa_key().as_deref(), Some("new"));
        crate::data::TEST_KEYS.with(|k| k.borrow_mut()[0] = Some("env".into()));
        let mut e = app();
        e.refreshed(Err(Failure::BadKey), tools());
        assert!(e.input == Input::None && e.status.contains(crate::data::AA_KEY_ENV), "fixed where it is set");
        let mut b = app();
        b.refreshed(Err("offline".into()), tools());
        assert_eq!(b.input, Input::None, "any other failure is only reported");
        let mut c = app();
        c.input = Input::Note { key: "gpt55".into(), text: "half".into(), cur: 4 };
        c.refreshed(Err(Failure::NoKey), tools());
        assert!(matches!(c.input, Input::Note { .. }), "a note being typed is kept");
    }

    #[test]
    fn the_selected_outlive_an_empty_table_and_a_model_gone() {
        let mut a = app();
        a.store.marked = vec!["gpt55".into(), "mini".into(), "gone".into()];
        a.rebuild();
        press(&mut a, "V");
        assert_eq!(a.targets(), ["gpt55", "mini"], "a kept mark of a model the data lost is no target");
        press(&mut a, "V");
        a.rows.clear();
        press(&mut a, "V");
        assert!(a.failed && !a.selecting() && a.status == "no selected model is shown");
        press(&mut a, "vC");
        assert_eq!(a.store.marked.len(), 3, "a range of no rows does not replace the selected");
    }

    fn press(app: &mut App, keys: &str) -> Option<Effect> {
        keys.chars().map(|c| app.key(KeyEvent::from(KeyCode::Char(c)))).last().flatten()
    }

    fn code(app: &mut App, code: KeyCode) -> Option<Effect> {
        app.key(KeyEvent::from(code))
    }

    fn ctrl(app: &mut App, c: char) -> Option<Effect> {
        app.key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL))
    }

    fn keys(app: &App) -> Vec<&str> {
        app.rows.iter().map(|&i| app.data.models[i].key.as_str()).collect()
    }

    fn menu(app: &App) -> Vec<(&str, usize)> {
        match &app.input {
            Input::Menu { items, .. } => items.iter().map(|(s, n)| (s.as_str(), *n)).collect(),
            _ => vec![],
        }
    }

    #[test]
    fn a_task_column_shows_the_benchmark_picked_in_its_dropdown() {
        let mut a = app();
        for (key, s) in [("gpt55", 0.4), ("mini", 0.7)] {
            let m = a.data.models.iter_mut().find(|m| m.key == key).unwrap();
            m.scores.insert("DeepSWE".into(), s);
        }
        a.col = ECI + 2;
        press(&mut a, "d");
        assert_eq!(menu(&a)[..2], [("all", 2), ("DeepSWE", 2)], "each with the models it scored");
        // The entry already shown changes nothing: a bound typed for the column stays.
        a.bounds.push((a.col, 1.0, f64::MAX));
        press(&mut a, " ");
        assert_eq!(a.bounds.len(), 1);
        a.bounds.clear();
        press(&mut a, "j ");
        code(&mut a, KeyCode::Esc);
        press(&mut a, "s");
        assert_eq!((a.col_name(a.col), keys(&a)), ("DeepSWE", vec!["mini", "gpt55", "opus5"]), "unscored last");
        assert_eq!(a.val(a.rows[0], a.col), Some(70.0));
        press(&mut a, "c");
        assert_eq!((a.col_name(a.col), keys(&a)[0]), ("Coding", "gpt55"), "c is back to the task's score");
        // A bound on the column goes with a pick, so the counts leave it out; the entry shown
        // already keeps it, and counts what the table shows.
        a.bounds.push((a.col, 1e9, f64::MAX));
        press(&mut a, "d");
        assert_eq!(menu(&a)[..2], [("all", 0), ("DeepSWE", 2)]);
        // A task's line is drawn from its score: choosing one shows it again, and no other is picked.
        press(&mut a, "j ");
        assert_eq!(menu(&a)[..2], [("all", 2), ("DeepSWE", 2)], "the open list counts again, the bound gone");
        code(&mut a, KeyCode::Esc);
        press(&mut a, "R2gg");
        code(&mut a, KeyCode::Enter);
        assert_eq!((a.task.is_some(), a.col_name(ECI + 2)), (true, "Coding"));
        a.col = ECI + 2;
        press(&mut a, "d");
        assert_eq!((&a.input, a.col_name(a.col)), (&Input::None, "Coding"));
        assert!((0..NCOLS).all(|c| TASK_COLS.contains(&c) == col_benches(c).is_some()));
        // Leaving the task took its sort along.
        press(&mut a, "cs");
        // Another source has other benchmarks: the pick goes, and a bound typed for it.
        press(&mut a, "dj ");
        code(&mut a, KeyCode::Esc);
        a.bounds.push((a.col, 60.0, f64::MAX));
        // One on an index stays, as ECI keeps its column and its scale. A price stays a price.
        a.bounds.extend([(ECI, 155.0, f64::MAX), (PRICE, 0.0, f64::MAX)]);
        a.switch(Source::Aa);
        assert_eq!((a.col_name(a.col), a.bounds.iter().map(|b| b.0).collect::<Vec<_>>()), ("Coding", vec![ECI, PRICE]));
        a.bounds.clear();
        assert!((0..NCOLS).all(|c| TASK_COLS.contains(&c) == col_benches(c).is_some()), "with either source");
        // Artificial Analysis lists the task's own benchmark, then its others about the task.
        let m = a.data.models.iter_mut().find(|m| m.key == "mini").unwrap();
        m.scores.insert("scicode".into(), 0.3);
        press(&mut a, "d");
        assert_eq!(menu(&a), [(crate::fit::AA_CODING, 2), ("terminalbench_v4_0", 0), ("scicode", 1)]);
        press(&mut a, "G ");
        assert_eq!((a.col_name(a.col), a.val(a.rows[0], a.col)), ("scicode", Some(30.0)));
        // A task with no other benchmark still has its dropdown, which names the one.
        code(&mut a, KeyCode::Esc);
        a.col = ECI + 3;
        press(&mut a, "d");
        assert_eq!(menu(&a).iter().map(|e| e.0).collect::<Vec<_>>(), ["terminalbench_v4_0"]);
    }

    #[test]
    fn the_dev_dropdown_lists_countries_before_developers() {
        let mut a = app();
        for m in a.data.models.iter_mut().filter(|m| m.developer != "meta") {
            m.country = "USA".into();
        }
        a.rebuild();
        a.col = 0;
        press(&mut a, "ld");
        assert_eq!(menu(&a), [("any", 3), ("USA", 3), ("anthropic", 1), ("openai", 2)]);
        press(&mut a, "j ");
        assert_eq!(keys(&a).len(), 3, "every model of a developer from there");
        // With a developer picked too, either one shows a model.
        press(&mut a, "j ");
        assert_eq!((a.dev.len(), keys(&a).len()), (2, 3));
    }

    #[test]
    fn dropdowns_pick_a_developer_and_a_price_level() {
        let mut a = app();
        a.col = ECI;
        press(&mut a, "d");
        assert_eq!((&a.input, a.status.as_str()), (&Input::None, "d opens a dropdown on the columns marked ▾"));
        a.col = 0;
        press(&mut a, "ld");
        assert_eq!(menu(&a), [("any", 3), ("anthropic", 1), ("openai", 2)], "developers A-Z, not by count");
        press(&mut a, "G");
        assert!(matches!(a.input, Input::Menu { list: List { sel: 2, .. }, .. }), "G to the last entry");
        press(&mut a, "gg");
        assert!(matches!(a.input, Input::Menu { list: List { sel: 0, .. }, .. }), "gg to the first");
        press(&mut a, "3gg");
        assert!(matches!(a.input, Input::Menu { list: List { sel: 2, .. }, .. }), "a count picks the entry");
        code(&mut a, KeyCode::Enter);
        assert!(
            matches!(a.input, Input::Menu { list: List { sel: 2, .. }, .. }),
            "enter toggles as space does: the dropdown stays open"
        );
        code(&mut a, KeyCode::Esc);
        assert_eq!((a.dev.as_slice(), keys(&a)), (&["openai".to_string()][..], vec!["gpt55", "mini"]));
        // Counts follow the developer picked; the price levels are maxima.
        press(&mut a, "lld");
        assert_eq!(menu(&a), [("any", 2), ("free", 0), ("≤$0.5", 0), ("≤$2", 1), ("≤$5", 1), ("≤$15", 2)]);
        press(&mut a, "jjj");
        code(&mut a, KeyCode::Enter);
        code(&mut a, KeyCode::Esc);
        assert_eq!(keys(&a), ["mini"]);
        // Reopening starts on the level in effect; esc leaves it alone, "any" drops it.
        press(&mut a, "d");
        assert!(matches!(a.input, Input::Menu { list: List { sel: 3, .. }, .. }));
        code(&mut a, KeyCode::Esc);
        assert_eq!(keys(&a), ["mini"]);
        press(&mut a, "dkkk");
        code(&mut a, KeyCode::Enter);
        code(&mut a, KeyCode::Esc);
        assert_eq!((a.bounds.len(), keys(&a).len()), (0, 2));
        // Space picks a level and keeps the dropdown open; again on it drops it.
        press(&mut a, "djjj ");
        assert_eq!((a.price_level(), keys(&a)), (Some(2), vec!["mini"]));
        assert!(matches!(a.input, Input::Menu { list: List { sel: 3, .. }, .. }));
        press(&mut a, "j ");
        assert_eq!((a.price_level(), keys(&a).len()), (Some(3), 1), "another level replaces it");
        code(&mut a, KeyCode::Enter);
        assert_eq!((a.price_level(), keys(&a).len()), (None, 2), "enter on the picked level drops it");
        code(&mut a, KeyCode::Esc);
        press(&mut a, "c");
        assert_eq!((a.dev.len(), keys(&a).len()), (0, 3));
    }

    #[test]
    fn via_dropdown_picks_a_harness() {
        let mut a = app();
        a.col = VIA;
        assert_eq!(base_col_name(a.col), "Via");
        press(&mut a, "d");
        assert_eq!(menu(&a), [("any", 3), ("codex", 2), ("opencode", 2), ("claude", 1)]);
        press(&mut a, "/cla");
        code(&mut a, KeyCode::Enter);
        code(&mut a, KeyCode::Esc);
        code(&mut a, KeyCode::Esc);
        assert_eq!((a.via.as_slice(), keys(&a)), (&["claude".to_string()][..], vec!["opus5"]));
        press(&mut a, "c");
        assert_eq!((a.via.len(), keys(&a).len()), (0, 3));
        // A selected model you have no access to shows, and is picked, as its Via reads.
        a.store.toggle_marked("llama4");
        a.rebuild();
        press(&mut a, "d");
        assert!(menu(&a).contains(&("not available", 1)), "{:?}", menu(&a));
        press(&mut a, "/not");
        code(&mut a, KeyCode::Enter);
        code(&mut a, KeyCode::Esc);
        code(&mut a, KeyCode::Esc);
        assert_eq!(keys(&a), ["llama4"]);
    }

    #[test]
    fn via_dropdown_picks_the_local_models_or_the_rest() {
        let mut a = app();
        a.col = VIA;
        a.data.models[3].via.push(crate::data::OLLAMA.into());
        a.data.models[2].hf = Some("meta/llama4".into());
        a.tools = (vec![crate::data::OLLAMA], vec![]);
        let pick = |a: &mut App, entry: &str| {
            press(a, "d");
            press(a, &format!("/{entry}"));
            code(a, KeyCode::Enter);
            code(a, KeyCode::Esc);
            code(a, KeyCode::Esc);
        };
        press(&mut a, "d");
        assert_eq!(menu(&a)[menu(&a).len() - 2..], [("local", 1), ("not local", 2)], "after the harnesses");
        code(&mut a, KeyCode::Esc);
        pick(&mut a, "local");
        assert_eq!((a.via.as_slice(), keys(&a)), (&["local".to_string()][..], vec!["mini"]), "whoever else has it");
        assert!(a.filtered_too(), "recommend says so");
        press(&mut a, "c");
        pick(&mut a, "not local");
        assert_eq!((keys(&a).len(), a.via.len()), (2, 1), "the others");
        press(&mut a, "c");
        assert_eq!((keys(&a).len(), a.via.len()), (3, 0), "c clears it");
        // With all, the ones ollama or llama.cpp can download have an entry of their own.
        press(&mut a, "a");
        pick(&mut a, "download");
        assert_eq!(keys(&a), ["llama4"], "it has a repo to download");
        a.sized("llama4", Download::Gone);
        assert!(keys(&a).is_empty(), "Hugging Face has no GGUF copy of it after all");
    }

    #[test]
    fn x_says_the_size_of_a_download_once_known() {
        let mut a = app();
        let get = |k: &str| Effect::Launch(["modelcmp", "get", k, "--via", "ollama"].map(String::from).to_vec());
        let run = Effect::Launch(vec!["codex".into(), "--model".into(), "mini".into()]);
        let items = vec![("codex".to_string(), run), ("get".into(), get("mini")), ("other".into(), get("llama4"))];
        a.input = Input::choose("open in which harness?", Kind::Launch, items, 0);
        a.sized("mini", Download::Size(2_500_000_000));
        a.sized("llama4", Download::Gone);
        let Input::Choose { items, .. } = &a.input else { panic!("the list stays open") };
        let labels: Vec<&str> = items.iter().map(|i| i.0.as_str()).collect();
        assert_eq!(labels, ["codex", "get (2.5 GB)", "other (no GGUF copy)"], "each on its model's download");
        assert_eq!(a.sizes(), [("mini".to_string(), 2_500_000_000)].into(), "the size is kept, not the lack of one");
        // Hugging Face is asked once for the model under the cursor, and again after a refresh
        // when it did not answer.
        a.input = Input::None;
        let key = a.current().unwrap().key.clone();
        assert!(key != "mini" && key != "llama4", "{key}");
        a.data.models.iter_mut().for_each(|m| m.hf = Some(format!("o/{}", m.key)));
        assert!(a.size_wanted());
        a.sized(&key, Download::NoAnswer);
        assert!(!a.size_wanted(), "not while the cursor stays");
        a.refreshed(Err("offline".to_string().into()), tools());
        assert!(a.size_wanted() && a.sizes().len() == 1, "a refresh asks again, the sizes staying");
        assert_eq!(a.downloads["llama4"], Download::Gone, "and the lack of a copy");
        // The local rows on screen are asked with it, each once.
        let other = a.rows.iter().map(|&i| a.data.models[i].key.clone()).find(|k| *k != key && k != "mini").unwrap();
        a.data.models.iter_mut().for_each(|m| m.via.push(crate::data::OLLAMA.into()));
        // Of one ollama has too, with no download to offer: its details say the size.
        a.tools = (vec![crate::data::OLLAMA], vec![]);
        let asked: Vec<String> = a.size_ask().into_iter().map(|q| q.0).collect();
        a.tools = (vec![crate::data::LLAMA], vec![]);
        assert!(asked.contains(&key) && asked.contains(&other), "{asked:?}");
        assert_eq!(asked.iter().filter(|k| **k == key).count(), 1, "the cursor's row is asked once");
        assert!(!asked.contains(&"mini".to_string()) && !a.size_wanted(), "not the ones known");
        // Via marks the ones `x` can download, with the size once known.
        let marks = ["mini", "llama4", &key].map(|k| a.download(a.data.models.iter().find(|m| m.key == k).unwrap()));
        assert_eq!(marks, [Some("↓ 2.5 GB".into()), None, Some("↓".into())], "none where there is no copy");
        a.tools = (vec![crate::data::OLLAMA], vec![]);
        assert_eq!(a.download(a.current().unwrap()), None, "nor where ollama has it and is all there is");
        a.tools.1 = vec![crate::data::LLAMA];
        assert_eq!(a.download(a.current().unwrap()), None, "nor to install another runner for it");
        let mut none = a.current().unwrap().clone();
        none.via.clear();
        let how: Vec<_> = getters(&none, &a.tools).collect();
        assert_eq!(how, [("ollama", "download and run"), ("llama-cli", "install, download and run")]);
        assert_eq!(press(&mut a, "r"), Some(Effect::Refresh));
        assert!(!a.downloads.contains_key("llama4") && a.sizes().len() == 1, "r asks again of a lack of a copy");
    }

    #[test]
    fn pi_launches_with_the_provider_it_listed() {
        let o = crate::data::Offer {
            provider: "openai".into(),
            id: "gpt55".into(),
            via: vec!["pi".into()],
            ..Default::default()
        };
        let m = Model { offers: vec![o], ..Default::default() };
        let listed = BTreeMap::from([("pi".to_string(), vec!["openai-codex/gpt55".to_string()])]);
        assert_eq!(launch_cmd(&m, "pi", &listed).unwrap(), ["pi", "--model", "openai-codex/gpt55"]);
        assert_eq!(launch_cmd(&m, "pi", &BTreeMap::new()).unwrap()[2], "openai/gpt55", "unlisted: models.dev's own");
    }

    #[test]
    fn the_id_is_the_one_opencode_takes() {
        let offer = |provider: &str, id: &str, via: &str| crate::data::Offer {
            provider: provider.into(),
            id: id.into(),
            via: vec![via.into()],
            available: true,
            input: 3.0,
            output: 15.0,
            ..Default::default()
        };
        let offers = vec![
            offer("anthropic", "claude-sonnet-5-5", "claude"),
            offer("openrouter", "anthropic/claude-sonnet-5.5", "opencode"),
        ];
        let mut m = Model { key: "sonnet".into(), offers, ..Default::default() };
        let listed = BTreeMap::new();
        assert_eq!(m.price().unwrap().provider, "anthropic", "at the same price, the first is the one you'd pay");
        assert_eq!(model_id(&m, &listed), "openrouter/anthropic/claude-sonnet-5.5", "opencode has OpenRouter's");
        // Of two that opencode reaches, the one you'd pay, not the first.
        let mut bedrock = offer("amazon-bedrock", "anthropic.claude-sonnet-5-5", "opencode");
        bedrock.input = 6.0;
        m.offers.insert(0, bedrock);
        assert_eq!(model_id(&m, &listed), "openrouter/anthropic/claude-sonnet-5.5");
        assert_eq!(launch_cmd(&m, "opencode", &listed).unwrap()[2], "openrouter/anthropic/claude-sonnet-5.5");
        m.offers.truncate(2);
        m.offers.remove(0);
        assert_eq!(model_id(&m, &listed), "anthropic/claude-sonnet-5-5", "with neither, the one you'd pay");
    }

    #[test]
    fn x_lists_the_harnesses_even_one() {
        let mut a = app();
        for m in &mut a.data.models {
            let via = m.via.clone();
            m.offers[0].via = via;
        }
        let cmd = |h: &str, id: &str| Some(Effect::Launch(vec![h.into(), "--model".into(), id.into()]));
        assert_eq!(press(&mut a, "x"), None, "gpt55 has codex and opencode");
        assert!(matches!(&a.input, Input::Choose { items, list: List { sel: 0, .. }, .. } if items.len() == 2));
        assert_eq!(press(&mut a, "jj"), None, "j stops at the last");
        code(&mut a, KeyCode::Esc);
        assert_eq!(a.input, Input::None, "esc cancels");
        press(&mut a, "j");
        assert_eq!(a.mouse(Mouse::Harness(0, 1)), cmd("opencode", "p/gpt55"), "a double click on it in Via opens it");
        assert_eq!((a.selected(), &a.input), (0, &Input::None), "its row highlighted");
        press(&mut a, "xj");
        assert_eq!(code(&mut a, KeyCode::Enter), cmd("opencode", "p/gpt55"), "opencode takes provider/model");
        assert_eq!(a.input, Input::None);
        press(&mut a, "G");
        assert_eq!(press(&mut a, "x"), None, "one harness: still listed, as o's one site");
        assert_eq!(code(&mut a, KeyCode::Enter), cmd("claude", "opus5"));
        press(&mut a, "a");
        let llama = a.rows.iter().position(|&r| a.data.models[r].key == "llama4").unwrap();
        a.select(llama);
        assert_eq!(press(&mut a, "x"), None);
        assert!(a.status.starts_with("no harness has"), "{}", a.status);
    }

    #[test]
    fn dropdown_search() {
        let mut a = app();
        a.col = 0;
        press(&mut a, "ld/OPEN");
        assert!(
            matches!(&a.input, Input::Menu { list: List { query, sel: 1, typing: true, .. }, .. } if query == "OPEN")
        );
        code(&mut a, KeyCode::Enter);
        assert_eq!(a.dev, ["openai"], "enter toggles the first match");
        assert!(
            matches!(&a.input, Input::Menu { list: List { query, typing: true, .. }, .. } if query == "OPEN"),
            "and the search stays"
        );
        code(&mut a, KeyCode::Esc);
        code(&mut a, KeyCode::Esc);
        press(&mut a, "d/anth");
        code(&mut a, KeyCode::Backspace);
        code(&mut a, KeyCode::Esc);
        assert!(
            matches!(&a.input, Input::Menu { list: List { query, sel: 1, typing: false, .. }, .. } if query.is_empty()),
            "esc drops the search, the cursor stays on anthropic"
        );
        press(&mut a, "/zzz");
        assert!(matches!(a.input, Input::Menu { list: List { sel: 0, .. }, .. }), "no match leaves only any");
        code(&mut a, KeyCode::Enter);
        assert!(a.dev.is_empty(), "enter on any drops them all");
    }

    #[test]
    fn search_forgives_a_typo_only_when_nothing_matches() {
        let mut a = app();
        press(&mut a, "/opsu");
        assert_eq!((keys(&a), a.typos), (vec!["opus5"], true));
        ctrl(&mut a, 'u');
        press(&mut a, "mini");
        assert_eq!((keys(&a), a.typos), (vec!["mini"], false));
    }

    #[test]
    fn ctrl_w_deletes_a_word_in_every_prompt() {
        let mut a = app();
        press(&mut a, "nfast enough  ");
        ctrl(&mut a, 'w');
        assert_eq!(
            a.input,
            Input::Note { key: "gpt55".into(), text: "fast ".into(), cur: 5 },
            "trailing spaces go with the word"
        );
        ctrl(&mut a, 'w');
        ctrl(&mut a, 'w');
        assert_eq!(a.input, Input::Note { key: "gpt55".into(), text: String::new(), cur: 0 });
        code(&mut a, KeyCode::Esc);
        press(&mut a, "/gpt 5");
        ctrl(&mut a, 'w');
        assert_eq!((a.query.as_str(), keys(&a).len()), ("gpt ", 1));
        ctrl(&mut a, 'u');
        assert_eq!((a.query.as_str(), keys(&a).len()), ("", 3));
        code(&mut a, KeyCode::Esc);
        a.col = 1;
        press(&mut a, "d/anth");
        ctrl(&mut a, 'w');
        assert!(matches!(&a.input, Input::Menu { list: List { query, sel: 1, .. }, .. } if query.is_empty()));
    }

    #[test]
    fn prompts_edit_around_a_cursor() {
        let mut a = App::new(Data::default(), Store::default());
        let note = |a: &App| match &a.input {
            Input::Note { text, cur, .. } => (text.clone(), *cur),
            _ => unreachable!(),
        };
        a.input = Input::Note { key: "gpt55".into(), text: "über fast".into(), cur: 9 };
        let alt = |a: &mut App, c: char| a.key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::ALT));
        alt(&mut a, 'b');
        assert_eq!(note(&a), ("über fast".into(), 6), "alt-b to the word start");
        code(&mut a, KeyCode::Left);
        code(&mut a, KeyCode::Backspace);
        assert_eq!(note(&a), ("übe fast".into(), 4), "backspace drops the multibyte char before");
        code(&mut a, KeyCode::Delete);
        press(&mut a, "r");
        assert_eq!(note(&a), ("überfast".into(), 5));
        ctrl(&mut a, 'a');
        alt(&mut a, 'd');
        assert_eq!(note(&a), (String::new(), 0), "alt-d drops the word after");
        press(&mut a, "one two");
        let ctrl_arrow = |a: &mut App, c| a.key(KeyEvent::new(c, KeyModifiers::CONTROL));
        ctrl_arrow(&mut a, KeyCode::Left);
        assert_eq!(note(&a), ("one two".into(), 4), "ctrl-left to the word start");
        ctrl_arrow(&mut a, KeyCode::Right);
        assert_eq!(note(&a), ("one two".into(), 7), "ctrl-right to the word end");
        ctrl(&mut a, 'b');
        ctrl(&mut a, 'b');
        ctrl(&mut a, 'u');
        assert_eq!(note(&a), ("wo".into(), 0), "ctrl-u drops what is before the cursor");
        ctrl(&mut a, 'f');
        ctrl(&mut a, 'k');
        code(&mut a, KeyCode::Home);
        code(&mut a, KeyCode::End);
        code(&mut a, KeyCode::Right);
        assert_eq!(note(&a), ("w".into(), 1), "ctrl-k drops the rest; end stays put");
        alt(&mut a, 'f');
        assert_eq!(note(&a), ("w".into(), 1));
        // A bound only takes digits, wherever the cursor is.
        a.input = Input::Bound { col: PRICE, min: true, text: "15".into(), cur: 1 };
        press(&mut a, "x.");
        assert!(matches!(&a.input, Input::Bound { text, cur: 2, .. } if text == "1.5"));
    }

    #[test]
    fn m_toggles_several_dropdown_entries() {
        let mut a = app();
        a.col = 1;
        press(&mut a, "djjm");
        assert!(a.dev.is_empty() && matches!(a.input, Input::Menu { .. }), "m does nothing");
        press(&mut a, " ");
        assert!(matches!(a.input, Input::Menu { .. }), "the dropdown stays open");
        assert_eq!((a.dev.as_slice(), keys(&a)), (&["openai".to_string()][..], vec!["gpt55", "mini"]));
        press(&mut a, "k ");
        assert_eq!(keys(&a), ["gpt55", "mini", "opus5"], "both developers show");
        // ] [ go round the ticked entries, as in f's grid: anthropic at 1, openai at 2.
        for sel in [2, 1] {
            press(&mut a, "]");
            assert!(matches!(a.input, Input::Menu { list: List { sel: s, .. }, .. } if s == sel), "] to {sel}");
        }
        press(&mut a, "j ");
        assert_eq!((a.dev.as_slice(), keys(&a)), (&["anthropic".to_string()][..], vec!["opus5"]));
        press(&mut a, "/open ");
        assert_eq!(a.dev, ["anthropic"], "while searching, space is typed");
        code(&mut a, KeyCode::Backspace);
        code(&mut a, KeyCode::Esc);
        press(&mut a, " ");
        assert_eq!(a.dev, ["anthropic", "openai"], "esc ends the search on the match, space toggles it");
        press(&mut a, "kk ");
        assert!(a.dev.is_empty(), "space on any drops them all");
        press(&mut a, "]");
        assert_eq!(a.status, "no other ticked entry is shown", "any is the ticked one with nothing picked");
        press(&mut a, "j");
        code(&mut a, KeyCode::Enter);
        assert_eq!(a.dev, ["anthropic"], "enter toggles one");
        code(&mut a, KeyCode::Enter);
        assert!(a.dev.is_empty() && matches!(a.input, Input::Menu { .. }), "again drops it, the dropdown open");
    }

    #[test]
    fn row_keys_do_nothing_behind_help_and_compare() {
        let mut a = app();
        press(&mut a, "?");
        assert_eq!(press(&mut a, "y"), None);
        press(&mut a, "n f ");
        assert_eq!((&a.input, a.store.marked.len()), (&Input::None, 0));
    }

    #[test]
    fn shows_available_unless_all() {
        let mut a = app();
        assert_eq!(a.rows.len(), 3);
        press(&mut a, "a");
        assert_eq!(a.rows.len(), 4);
    }

    #[test]
    fn starts_by_the_index_best_first_and_blanks_sink_either_way() {
        let mut a = app();
        assert_eq!((a.col, a.sort_col, a.descending), (ECI, ECI, true));
        assert_eq!(keys(&a), ["gpt55", "mini", "opus5"]);
        press(&mut a, "s");
        assert!(!a.descending, "the cursor starts on the index, so s reverses");
        assert_eq!(keys(&a), ["mini", "gpt55", "opus5"]);
        a.col = PRICE;
        press(&mut a, "s");
        assert_eq!(keys(&a), ["mini", "opus5", "gpt55"]);
        press(&mut a, "s");
        assert_eq!(keys(&a), ["gpt55", "opus5", "mini"]);
    }

    #[test]
    fn column_cursor_and_sort_by_price() {
        let mut a = app();
        a.col = 0;
        press(&mut a, "s");
        assert!(!a.descending, "names read A-Z");
        assert_eq!(keys(&a), ["gpt55", "mini", "opus5"]);
        press(&mut a, "ls");
        assert_eq!(a.col, 1);
        assert_eq!(keys(&a), ["opus5", "gpt55", "mini"], "by developer, names breaking ties");
        press(&mut a, "s");
        assert_eq!(keys(&a), ["mini", "gpt55", "opus5"]);
        press(&mut a, "lls");
        assert_eq!(numeric(a.col).unwrap().name, "Price");
        assert!(!a.descending, "prices cheapest first");
        assert_eq!(keys(&a), ["mini", "opus5", "gpt55"]);
        a.col = NCOLS - 1;
        press(&mut a, "l");
        assert_eq!(a.col, 0, "wraps");
        press(&mut a, "h");
        assert_eq!(a.col, NCOLS - 1);
    }

    #[test]
    fn every_column_says_what_it_means() {
        assert!(base_col_about(0).contains("Via"));
        assert!(base_col_about(1).contains("trained"));
        assert!(COLS.iter().all(|c| !c.about().is_empty()));
        let of = |id| col_source(TEXT + COLS.iter().position(|c| c.id == id).unwrap());
        assert_eq!(
            (of("price"), of("ctx"), of("eci"), of("value"), of("cost")),
            (None, None, Some(Source::Epoch), Some(Source::Epoch), Some(Source::Epoch))
        );
        assert!(COLS[ECI - TEXT..].iter().all(|c| c.only.is_some() || c.price || crate::fit::task(c.id).is_some()));
        let about = |id| base_col_about(TEXT + COLS.iter().position(|c| c.id == id).unwrap());
        assert_eq!(about("coding"), "Epoch AI, fitted from DeepSWE, FrontierCode and 3 more, ECI points");
        assert_eq!(
            about("agentic"),
            "Epoch AI, fitted from APEX-Agents, Remote Labor Index and OSWorld 2.0, ECI points"
        );
        assert_eq!(about("value"), "coding per dollar, ranked 0-100", "no benchmarks of its own");
        crate::data::set_source(Source::Aa);
        assert_eq!(
            about("agentic"),
            "Artificial Analysis, Terminal-Bench 4.0 score (0-100)",
            "the one benchmark it is"
        );
    }

    #[test]
    fn a_release_date_sorts_and_bounds_as_it_reads() {
        let on = |d: &str| released(&Model { release: d.into(), ..Default::default() });
        assert_eq!(on("").or(on("soon")), None, "no date, no value");
        assert_eq!(on("2026-06"), Some(2026.06), "a bound typed 2026.06 keeps June's models");
        assert!(on("2026-10-02") > on("2026-09-18") && on("2026-09-18") > on("2026-09"));
        assert_eq!(on("2026-09-18").map(month).as_deref(), Some("2026-09"));
        assert_eq!((month(2026.06), month(2026.0)), ("2026-06".to_string(), "2026".to_string()));
        let mut a = app();
        let mut data = std::mem::take(&mut a.data);
        (data.models[0].release, data.models[1].release) = ("2026-09".into(), "2025-01".into());
        a.set_data(data);
        let col = COLS.iter().position(|c| c.id == "release").unwrap();
        assert!(a.vals[0][col] > a.vals[1][col] && a.ext[col].is_none(), "dated, but no best or worst to colour");
        // A maximum keeps the month or the year it names; a date is typed as the column shows it.
        let read = COLS[col].read;
        assert!(on("2026-06-15") <= read("2026-06", true) && on("2026-07-01") > read("2026.06", true));
        assert!(on("2025-12-31") <= read("2025", true) && on("2026-01") > read("2025", true));
        assert_eq!((read("2026-6", false), read("2026.06.15", true)), (on("2026-06"), on("2026-06-15")));
        assert_eq!(read("2026-60", false).or(read("202606", false)).or(read("2026-06-15-1", false)), None);
        (a.col, a.sort_col) = (col + TEXT, 0);
        press(&mut a, "<2026-09");
        code(&mut a, KeyCode::Enter);
        assert_eq!((a.rows.len(), month(a.bounds[0].2)), (2, "2026-09".to_string()), "September's stays");
        press(&mut a, ">2026.60");
        code(&mut a, KeyCode::Enter);
        assert!(a.failed && a.bounds.len() == 1, "{}", a.status);
        assert_eq!(a.status, "2026.60 is not a value for Released");
    }

    #[test]
    fn counts_and_gg() {
        let mut a = app();
        press(&mut a, "a");
        press(&mut a, "2j");
        assert_eq!(a.current().unwrap().key, "llama4");
        press(&mut a, "gg");
        assert_eq!(a.current().unwrap().key, "gpt55");
        press(&mut a, "G");
        assert_eq!(a.current().unwrap().key, "opus5");
        press(&mut a, "3gg");
        assert_eq!(a.current().unwrap().key, "llama4");
        press(&mut a, "99j");
        assert_eq!(a.current().unwrap().key, "opus5", "a move stops at the end");
        press(&mut a, "j");
        assert_eq!(a.current().unwrap().key, "gpt55", "and wraps from there");
        press(&mut a, "k");
        assert_eq!(a.current().unwrap().key, "opus5");
        ctrl(&mut a, 'u');
        assert_eq!(a.current().unwrap().key, "gpt55");
        let shown = (0..NCOLS).filter(|&c| !hidden(c)).count();
        assert!(shown < NCOLS, "Epoch has no speed columns");
        press(&mut a, &format!("{shown}l"));
        assert_eq!(a.col, ECI, "counted column moves wrap around, over the shown ones");
        crate::data::set_source(Source::Aa);
        let shown = (0..NCOLS).filter(|&c| !hidden(c)).count();
        assert!(shown < NCOLS, "Artificial Analysis has no cost of a task");
        a.off_hidden();
        press(&mut a, &format!("{shown}l"));
        assert_eq!(a.col, AAII, "with its speed columns in their place, from its own index");
    }

    #[test]
    fn column_jumps() {
        let mut a = app();
        a.col = 0;
        let mut cols = vec![];
        for _ in 0..5 {
            press(&mut a, "w");
            cols.push(a.col);
        }
        press(&mut a, "w");
        assert_eq!((cols, a.col), (vec![PRICE, ECI, MEASURED, VIA, NOTES], 0), "w wraps from the last column");
        press(&mut a, "b");
        assert_eq!(a.col, VIA, "b wraps from the first");
        a.col = ECI + 2;
        press(&mut a, "b");
        assert_eq!(a.col, ECI, "b to the start of the group first");
        press(&mut a, "b");
        assert_eq!(a.col, PRICE);
        press(&mut a, "0");
        assert_eq!(a.col, 0);
        press(&mut a, "2w");
        assert_eq!(a.col, ECI);
        press(&mut a, "$");
        assert_eq!(a.col, NOTES);
        press(&mut a, "_");
        assert_eq!(a.col, 0);
        press(&mut a, "$");
        assert_eq!(a.col, NOTES);
        press(&mut a, "a10j");
        assert_eq!((a.col, a.current().unwrap().key.as_str()), (NOTES, "opus5"), "0 inside a count");
    }

    #[test]
    fn bounds_filter_and_replace() {
        let mut a = app();
        a.col = ECI;
        press(&mut a, "al");
        assert_eq!(numeric(a.col).unwrap().name, "Coding");
        press(&mut a, ">50");
        assert!(matches!(a.input, Input::Bound { min: true, .. }));
        code(&mut a, KeyCode::Enter);
        assert_eq!(keys(&a), ["gpt55", "mini"]);
        press(&mut a, ">70");
        code(&mut a, KeyCode::Enter);
        assert_eq!(a.bounds.len(), 1, "a new minimum replaces the old one");
        assert_eq!(keys(&a), ["gpt55"]);
        press(&mut a, "c");
        assert_eq!(a.rows.len(), 4);
    }

    #[test]
    fn frontier_keeps_cheapest_per_quality() {
        let models = [model("fable", true, Some(95.4), 20.0), model("opus", true, Some(95.2), 10.0)];
        let coding = |m: &Model| task_score(m, "coding");
        let front = |ms: &[Model]| frontier(&(0..ms.len()).collect::<Vec<_>>(), |i| &ms[i], coding);
        assert_eq!(front(&models), [1], "95.4 shows as 95: not worth twice the price");
        let models = [model("opus", true, Some(95.0), 10.0), model("free", true, Some(60.0), 0.0)];
        assert_eq!(front(&models), [0, 1], "a free model is the cheapest there is");
    }

    #[test]
    fn search_filters_and_esc_clears() {
        let mut a = app();
        // A paste is text for what is being typed; in the table its letters are not keys.
        a.paste("f\nq");
        assert_eq!((&a.input, a.status.as_str()), (&Input::None, "nothing to paste into: / searches, n writes a note"));
        // Nor at the quit prompt, where the first letter would cancel and the rest run.
        press(&mut a, "q");
        a.paste("xe");
        assert_eq!((&a.input, a.store.excluded.len()), (&Input::Quit, 0));
        press(&mut a, "x");
        press(&mut a, "/");
        a.paste("op\nus");
        assert_eq!(a.input, Input::Search { cur: 4, was: String::new() });
        assert_eq!(a.rows.len(), 1);
        code(&mut a, KeyCode::Enter);
        assert_eq!(a.input, Input::None);
        assert_eq!(a.query, "opus");
        press(&mut a, "/");
        assert_eq!((a.query.as_str(), a.rows.len()), ("", 3), "/ again starts a new search");
        press(&mut a, "gpt");
        code(&mut a, KeyCode::Esc);
        assert_eq!((a.query.as_str(), a.rows.len()), ("opus", 1), "esc drops the edit, not the filter");
        assert_eq!(code(&mut a, KeyCode::Esc), None, "esc clears the filter first");
        assert_eq!(a.rows.len(), 3);
        assert_eq!(code(&mut a, KeyCode::Esc), None, "only q quits");
        assert_eq!(press(&mut a, "qq"), Some(Effect::Quit));
    }

    #[test]
    fn picking_a_task_shows_its_frontier() {
        let mut a = app();
        press(&mut a, "Rjjj");
        assert_eq!(a.task_cur, 2);
        press(&mut a, "G");
        assert_eq!(a.task_cur, TASKS.len() - 1);
        press(&mut a, "1gg");
        assert_eq!(a.task_cur, 0, "the cursor stays inside the list");
        assert_eq!(TASKS[0].name, "overall", "the general pick comes first");
        press(&mut a, "2gg");
        let front: Vec<&str> =
            a.task_frontier(fit::task("coding").unwrap()).iter().map(|(m, _)| m.key.as_str()).collect();
        assert_eq!(front, ["mini", "gpt55"], "cheapest first, the best last; llama4 has no access");
        code(&mut a, KeyCode::Enter);
        assert_eq!(a.view, View::Table);
        assert_eq!(a.task.map(|t| t.name), Some("coding"));
        assert_eq!(keys(&a), ["gpt55", "mini"], "best first, each row down cheaper");
        assert_eq!((a.sort_col, a.descending), (PRICE, true));
        assert_eq!(a.selected(), 0);
        assert!(a.status.starts_with("gpt55 best, mini cheapest"), "{}", a.status);
        press(&mut a, "c");
        assert!(a.task.is_none());
        assert_eq!(a.rows.len(), 3);
        assert_eq!((a.sort_col, a.descending), default_sort(), "c restores the default sort");
    }

    #[test]
    fn a_task_starts_at_the_low_tier() {
        let mut a = app();
        let mut data = std::mem::take(&mut a.data);
        data.models.push(model("weak", true, Some(59.0), 0.01));
        data.models.push(model("edge", true, Some(60.0), 0.05));
        a.set_data(data);
        let front: Vec<&str> =
            a.task_frontier(fit::task("coding").unwrap()).iter().map(|(m, _)| m.key.as_str()).collect();
        assert_eq!(front, ["edge", "gpt55"], "cheap alone is no recommendation; 8 months behind the best is one");
    }

    #[test]
    fn a_task_keeps_the_best_of_each_price_level() {
        let mut a = app();
        let mut data = std::mem::take(&mut a.data);
        data.models.push(model("flash", true, Some(70.0), 1.5));
        a.set_data(data);
        let front: Vec<&str> =
            a.task_frontier(fit::task("coding").unwrap()).iter().map(|(m, _)| m.key.as_str()).collect();
        assert_eq!(front, ["mini", "flash", "gpt55"], "flash is its level's best, mini what low picks");
        press(&mut a, "R2gg");
        code(&mut a, KeyCode::Enter);
        assert_eq!(keys(&a), ["gpt55", "flash", "mini"], "enter shows the same line as the panel");
    }

    #[test]
    fn a_search_on_excluded_ranks_its_exact_matches_alone() {
        let mut a = app();
        let mut data = std::mem::take(&mut a.data);
        data.models.push(model("mint", true, Some(99.0), 0.01));
        a.set_data(data);
        press(&mut a, "eE/mini");
        code(&mut a, KeyCode::Enter);
        let front: Vec<&str> =
            a.task_frontier(fit::task("coding").unwrap()).iter().map(|(m, _)| m.key.as_str()).collect();
        assert_eq!((a.only, a.typos), (Some(EXCLUDED), true), "no excluded model is mini, so the table takes typos");
        assert_eq!(front, ["mini"], "recommend has mini itself, so not mint, a typo away");
    }

    #[test]
    fn excluded_models_leave_the_recommendations() {
        let mut a = app();
        let front = |a: &App| {
            a.task_frontier(fit::task("coding").unwrap()).iter().map(|(m, _)| m.key.clone()).collect::<Vec<_>>()
        };
        assert_eq!(press(&mut a, "e"), Some(Effect::Save));
        assert!(a.store.is_excluded("gpt55"));
        assert_eq!(a.rows.len(), 3, "the table still shows it");
        assert_eq!(front(&a), ["mini"]);
        press(&mut a, "E");
        assert_eq!((keys(&a), front(&a)), (vec!["gpt55"], vec!["mini".to_string()]), "E narrows no line");
        assert_eq!((a.among_on(), a.filtered_too()), (YOURS, false), "nor does recommend say so");
        press(&mut a, "E/mini");
        code(&mut a, KeyCode::Enter);
        assert!(a.filtered_too());
        // Recommend opens on its `among` line, before the first task: h l run over the tabs with
        // something to show, and enter picks the models it ranks, the panel staying open.
        press(&mut a, "cR");
        assert_eq!((a.among, a.current().is_none()), (Some(YOURS), true), "on the one in use");
        press(&mut a, "l");
        assert_eq!((a.among, a.all), (Some(ALL), false), "moving picks none");
        press(&mut a, "l");
        assert_eq!(a.among, Some(YOURS), "nothing selected, no favorite: past them, round the end");
        press(&mut a, "$");
        code(&mut a, KeyCode::Enter);
        assert_eq!((&a.view, a.among_on(), a.all), (&View::Recommend, ALL, true));
        assert_eq!(a.mouse(Mouse::Model(Stop::Among(MARKED))), None);
        assert_eq!(
            (&a.view, a.among_on(), a.status.as_str()),
            (&View::Recommend, ALL, NO_SELECTED),
            "an empty one says why"
        );
        assert_eq!(a.among, Some(ALL), "and the cursor stays off it");
        press(&mut a, "f");
        assert!(a.status.starts_with("the cursor is on the models to rank"), "{}", a.status);
        a.mouse(Mouse::Model(Stop::Among(YOURS)));
        assert_eq!((&a.view, a.among_on()), (&View::Recommend, YOURS), "a click picks too");
        press(&mut a, "j");
        assert_eq!((a.among, a.task_cur), (None, 0), "j is back on the first task");
        press(&mut a, "Ggg");
        assert_eq!(a.among, Some(YOURS), "gg is the top, the among line");
        press(&mut a, "1gg");
        assert_eq!((a.among, a.task_cur), (None, 0), "1gg is the first task");
        // A tab's key leaves recommend for it, as a click on the tab does.
        press(&mut a, "a");
        assert_eq!((&a.view, a.all), (&View::Table, true));
        press(&mut a, "RS");
        assert_eq!((&a.view, a.status.as_str()), (&View::Recommend, NO_SELECTED), "an empty one keeps the panel");
        code(&mut a, KeyCode::Esc);
        press(&mut a, "A");
        press(&mut a, "R2gg");
        code(&mut a, KeyCode::Enter);
        assert_eq!(keys(&a), ["mini"], "nor does the table");
        press(&mut a, "c");
        a.store.toggle_excluded("gpt55");
        a.rebuild();
        assert_eq!(front(&a), ["mini", "gpt55"], "e again brings it back");
    }

    #[test]
    fn a_selected_model_shows_out_of_reach_but_is_never_a_pick() {
        let mut a = app();
        let shown = |a: &App| a.rows.iter().map(|&i| a.data.models[i].key.clone()).collect::<Vec<_>>();
        let front = |a: &App| {
            a.task_frontier(fit::task("coding").unwrap()).iter().map(|(m, _)| m.key.clone()).collect::<Vec<_>>()
        };
        let mut data = std::mem::take(&mut a.data);
        let llama = &mut data.models[2];
        (llama.offers[0].input, llama.offers[0].output) = (0.1, 0.1);
        llama.fit.insert("coding".into(), 60.0);
        llama.lag.insert("coding".into(), 16.0);
        a.store.toggle_marked("llama4");
        a.set_data(data);
        assert!(shown(&a).contains(&"llama4".into()), "{:?}", shown(&a));
        assert!(!front(&a).contains(&"llama4".into()), "{:?}", front(&a));
        // Deselecting drops its row at once, and the cursor stays where it was.
        let at = shown(&a).iter().position(|k| k == "llama4").unwrap();
        a.select(at);
        a.key(KeyCode::Char(' ').into());
        assert!(!shown(&a).contains(&"llama4".into()), "{:?}", shown(&a));
        assert_eq!(a.selected(), at.min(a.rows.len() - 1));
        // In details the model stays on screen until you are back on the table, however.
        for leave in [&[KeyCode::Esc][..], &[KeyCode::Char('?'), KeyCode::Char('?')]] {
            a.store.toggle_marked("llama4");
            a.rebuild();
            a.select(at);
            a.key(KeyCode::Enter.into());
            a.key(KeyCode::Char(' ').into());
            assert_eq!((&a.view, a.current().unwrap().key.as_str()), (&View::Detail(Back::Table), "llama4"));
            leave.iter().for_each(|&k| _ = a.key(k.into()));
            assert_eq!(a.view, View::Table);
            assert!(!shown(&a).contains(&"llama4".into()), "{leave:?}: {:?}", shown(&a));
        }
        // Details keep their model through a refresh that drops its row.
        a.store.toggle_marked("llama4");
        a.rebuild();
        a.select(at);
        a.key(KeyCode::Enter.into());
        a.store.toggle_marked("llama4");
        let data = std::mem::take(&mut a.data);
        a.refreshed(Ok(data), tools());
        assert!(!shown(&a).contains(&"llama4".into()), "{:?}", shown(&a));
        assert_eq!(a.current().unwrap().key, "llama4");
        a.key(KeyCode::Esc.into());
        // Comparing a selection replaces the marks, so one out of reach drops out.
        a.store.toggle_marked("llama4");
        a.rebuild();
        a.select(0);
        press(&mut a, "vjC");
        assert!(!shown(&a).contains(&"llama4".into()), "{:?}", shown(&a));
        a.key(KeyCode::Esc.into());
        a.store.toggle_marked("llama4");
        a.all = true;
        a.rebuild();
        assert!(front(&a).contains(&"llama4".into()), "with `a` it is ranked as any: {:?}", front(&a));
    }

    #[test]
    fn a_favorite_stays_in_recommend_whatever_the_filters() {
        let mut a = app();
        let front = |a: &App| {
            a.task_frontier(fit::task("coding").unwrap()).iter().map(|(m, _)| m.key.clone()).collect::<Vec<_>>()
        };
        a.store.toggle_favorite("coding", "mini");
        a.query = "gpt".into();
        a.rebuild();
        assert_eq!(front(&a), ["mini", "gpt55"], "the filter hides mini from the table, not from recommend");
        let coding = fit::task("coding").unwrap();
        assert!(a.favorite_unrecommended(coding, "mini"), "hidden, so not recommended");
        // A hidden favorite is not ranked, so it drops no model the filter shows: mini is
        // cheaper and as good as gptlite, yet gptlite stays.
        let mut data = std::mem::take(&mut a.data);
        let mut lite = model("gptlite", true, Some(60.0), 2.0);
        (lite.developer, lite.via) = ("openai".into(), vec!["codex".into()]);
        data.models.push(lite);
        a.set_data(data);
        assert_eq!(front(&a), ["mini", "gptlite", "gpt55"]);
        let mut data = std::mem::take(&mut a.data);
        data.models.pop();
        a.set_data(data);
        a.store.toggle_excluded("mini");
        a.rebuild();
        assert_eq!(front(&a), ["gpt55"], "an excluded favorite still leaves");
        // opus5 has no coding score: it joins the line all the same, its score shown as `-`.
        a.store.toggle_favorite("coding", "opus5");
        a.query.clear();
        a.rebuild();
        assert_eq!(front(&a), ["opus5", "gpt55"]);
        let (m, s) = a.task_frontier(fit::task("coding").unwrap())[0];
        assert!(crate::view::priced(m, s, false, true).ends_with("(-)"));
        assert_eq!(
            crate::view::pick([(m, s)].iter(), "coding", "low").map(|e| e.0.key.as_str()),
            Some("opus5"),
            "the only entry"
        );
        assert!(a.favorite_unrecommended(coding, "opus5"), "no score, so not recommended");
        // A tier's favorite joins the line too, beside the task's.
        a.store.toggle_favorite("coding:low", "mini");
        a.store.toggle_excluded("mini");
        a.rebuild();
        assert_eq!(front(&a), ["mini", "opus5", "gpt55"]);
        assert!(!a.favorite_unrecommended(coding, "mini"), "on the frontier on its own");
        a.store.toggle_favorite("coding", "gpt55");
        a.rebuild();
        assert!(!a.favorite_unrecommended(coding, "gpt55"), "the best model is recommended on its own");
    }

    #[test]
    fn enter_on_a_task_of_your_own_lands_on_its_model_or_says_why() {
        let mut a = app();
        a.store.toggle_favorite("debugging", "opus5");
        a.rebuild();
        // After a built-in task's line, which its model is not on.
        press(&mut a, "R2gg");
        code(&mut a, KeyCode::Enter);
        press(&mut a, "RG");
        code(&mut a, KeyCode::Enter);
        assert_eq!((a.task.is_none(), a.current().map(|m| m.key.as_str())), (true, Some("opus5")));
        // Filtered out, it says so rather than name the row the cursor is on.
        press(&mut a, "/gpt");
        code(&mut a, KeyCode::Enter);
        press(&mut a, "RG");
        code(&mut a, KeyCode::Enter);
        assert!(a.failed && a.status.contains("filtered out"), "{}", a.status);
        // Compare with one selected shows no model, so a model's keys act on none.
        let mut a = app();
        press(&mut a, "j ggC");
        assert_eq!((press(&mut a, "e"), a.store.excluded.is_empty()), (None, true));
    }

    #[test]
    fn a_task_of_your_own_has_the_model_you_give_it() {
        let mut a = app();
        let on = a.current().unwrap().key.clone();
        // The last entry of f's grid takes the name of a new task right there, the list open.
        press(&mut a, "fG");
        assert_eq!(code(&mut a, KeyCode::Enter), None);
        let edit =
            |a: &App| a.open_list().and_then(|l| l.edit.as_ref().map(|e| (format!("{:?}", e.what), e.text.clone())));
        let under = |a: &App| match &a.input {
            Input::Choose { items, list, .. } => items[list.sel].0.clone(),
            _ => String::new(),
        };
        let ticked = |a: &App| a.store.favorite("tool-dispatch").is_some();
        assert_eq!(edit(&a), Some(("New".into(), String::new())));
        a.paste("Tool Dispatch:");
        assert_eq!(code(&mut a, KeyCode::Enter), Some(Effect::Save));
        assert_eq!(a.store.favorite("tool-dispatch"), Some(on.as_str()), "no : in it");
        // The list is on the new task, where what it is about is written next; left empty,
        // nothing is saved.
        assert_eq!(
            (under(&a).as_str(), edit(&a)),
            ("tool-dispatch", Some((r#"About("tool-dispatch")"#.into(), String::new())))
        );
        assert_eq!((code(&mut a, KeyCode::Enter), edit(&a), a.store.about("tool-dispatch")), (None, None, None));
        assert!(a.choosing_favs(), "the list is open still");
        // a on it writes it, starting from what it says, and its entry shows it.
        press(&mut a, "a");
        a.paste("routing tool calls");
        assert_eq!(code(&mut a, KeyCode::Enter), Some(Effect::Save));
        assert_eq!((a.store.about("tool-dispatch"), under(&a).as_str()), (Some("routing tool calls"), "tool-dispatch"));
        // While writing, the keys that move are text, the wheel is still and esc leaves the entry alone.
        press(&mut a, "ajk");
        a.mouse(Mouse::Scroll(-3));
        assert_eq!(edit(&a), Some((r#"About("tool-dispatch")"#.into(), "routing tool callsjk".into())));
        code(&mut a, KeyCode::Esc);
        assert_eq!(
            (edit(&a), a.choosing_favs(), a.store.about("tool-dispatch")),
            (None, true, Some("routing tool calls"))
        );
        // Unticked it is gone, but keeps its row while the grid is open; r and a say what it lacks.
        press(&mut a, " ");
        assert_eq!((a.store.custom_tasks().len(), under(&a).as_str()), (0, "tool-dispatch"));
        press(&mut a, "r");
        assert_eq!((edit(&a), a.status.as_str()), (None, "tool-dispatch has no model: tick a box of it first"));
        press(&mut a, " ");
        assert!(ticked(&a) && under(&a) == "tool-dispatch");
        // r writes another name on it, starting from its own; one a task has is said under the
        // list, the name still there to change.
        press(&mut a, "r");
        assert_eq!(edit(&a), Some((r#"Rename("tool-dispatch")"#.into(), "tool-dispatch".into())));
        ctrl(&mut a, 'u');
        a.paste("coding");
        assert_eq!(code(&mut a, KeyCode::Enter), None);
        let err = |a: &App| a.open_list().and_then(|l| l.edit.as_ref()?.err.clone());
        assert_eq!(
            (err(&a).as_deref(), edit(&a).unwrap().1.as_str()),
            (Some("there is a task coding already"), "coding")
        );
        ctrl(&mut a, 'u');
        a.paste("Dispatch");
        assert_eq!((err(&a), code(&mut a, KeyCode::Enter)), (None, Some(Effect::Save)));
        assert_eq!((a.store.custom_tasks(), under(&a).as_str()), (vec!["dispatch"], "dispatch"));
        assert_eq!(a.status, "tool-dispatch renamed to dispatch");
        press(&mut a, "ggr");
        assert_eq!((edit(&a), a.failed), (None, true), "overall is built in, and r says so");
        assert!(a.status.starts_with("overall is built in"), "{}", a.status);
        code(&mut a, KeyCode::Esc);
        a.store.rename_task("dispatch", "tool-dispatch").unwrap();
        // Recommend lists it after the built-in tasks, the cursor on its name, its model after
        // it; enter on the name goes to the table, on that model.
        press(&mut a, "RG");
        assert_eq!((a.task_cur, a.custom_at(), a.task_at_hand().is_none()), (TASKS.len(), Some("tool-dispatch"), true));
        assert!(a.current().is_none(), "on the name");
        press(&mut a, "l");
        assert_eq!(a.current().map(|m| m.key.clone()), Some(on.clone()), "its model is every tier's");
        press(&mut a, "h");
        code(&mut a, KeyCode::Enter);
        assert_eq!((&a.view, a.current().unwrap().key.as_str()), (&View::Table, on.as_str()));
        assert!(a.status.ends_with("your model for tool-dispatch"), "{}", a.status);
        // f on its row starts on it; without its model the task is gone, and the cursor is on
        // the task before.
        press(&mut a, "RGf");
        assert!(a.failed && a.input == Input::None, "f on a task's name has no model to favorite");
        press(&mut a, "lf");
        assert!(ticked(&a) && under(&a) == "tool-dispatch");
        press(&mut a, " ");
        code(&mut a, KeyCode::Esc);
        assert_eq!((a.task_cur, a.store.favorite("tool-dispatch")), (TASKS.len() - 1, None));
        // An empty name names no task.
        press(&mut a, "2gglfG");
        code(&mut a, KeyCode::Enter);
        assert_eq!(edit(&a), Some(("New".into(), String::new())));
        assert_eq!((code(&mut a, KeyCode::Enter), edit(&a), a.store.custom_tasks().len()), (None, None, 0));
        code(&mut a, KeyCode::Esc);
        // A tier of it takes a model of its own, as a built-in task's: its boxes follow the
        // task's on its row, and in recommend its low tier has that model, the others the task's.
        a.store.toggle_favorite("tool-dispatch", &on);
        a.store.toggle_favorite("tool-dispatch:low", "mini");
        a.rebuild();
        press(&mut a, "G0l");
        assert_eq!((&a.view, a.current().map(|m| m.key.as_str())), (&View::Recommend, Some("mini")));
        press(&mut a, "l");
        assert_eq!(a.current().map(|m| m.key.clone()), Some(on.clone()), "h l move along its tiers");
        press(&mut a, "fl");
        assert_eq!((under(&a).as_str(), a.open_list().map(|l| l.col)), ("tool-dispatch", Some(1)), "l: its low tier");
        press(&mut a, "a");
        a.paste(" fast");
        assert_eq!(code(&mut a, KeyCode::Enter), Some(Effect::Save));
        assert_eq!(a.open_list().map(|l| l.col), Some(1), "what it is about leaves the cursor on its box");
        press(&mut a, "4l5h");
        assert_eq!(a.open_list().map(|l| l.col), Some(0), "h l stop at the ends of the row");
        press(&mut a, "j");
        assert_eq!(under(&a), "+ new task", "after the last task");
        // A task named before it in the order leaves recommend's cursor on its own.
        code(&mut a, KeyCode::Enter);
        a.paste("api");
        code(&mut a, KeyCode::Enter);
        assert_eq!((a.store.custom_tasks(), a.custom_at()), (vec!["api", "tool-dispatch"], Some("tool-dispatch")));
    }

    #[test]
    fn v_puts_a_favorite_on_a_harness() {
        let mut a = app();
        for m in &mut a.data.models {
            let via = m.via.clone();
            m.offers[0].via = via;
        }
        let kind = |a: &App| match &a.input {
            Input::Choose { kind, list, .. } => Some((*kind, list.sel)),
            _ => None,
        };
        press(&mut a, "fjv");
        assert_eq!(kind(&a), Some((Kind::Fav, 1)), "not ticked: no model to run, f's grid stays");
        assert!(a.status.starts_with("coding is not ticked"), "{}", a.status);
        press(&mut a, " v");
        assert!(matches!(&a.input, Input::Choose { items, .. } if items.len() == 3), "any, opencode and codex");
        assert_eq!(kind(&a), Some((Kind::Via, 0)), "on any: it has none yet");
        press(&mut a, "j");
        assert_eq!(code(&mut a, KeyCode::Enter), Some(Effect::Save));
        assert_eq!(a.store.via("coding"), Some("opencode"));
        assert_eq!(kind(&a), Some((Kind::Fav, 1)), "back on the task in f's grid");
        press(&mut a, "v");
        assert_eq!(kind(&a), Some((Kind::Via, 1)), "on the one it has");
        code(&mut a, KeyCode::Esc);
        assert_eq!((kind(&a), a.store.via("coding")), (Some((Kind::Fav, 1)), Some("opencode")), "esc: back, as it was");
        press(&mut a, "vk");
        code(&mut a, KeyCode::Enter);
        assert_eq!(a.store.via("coding"), None, "any: none in particular");
        press(&mut a, "vj");
        code(&mut a, KeyCode::Enter);
        press(&mut a, " ");
        assert_eq!((a.store.favorite("coding"), a.store.via("coding")), (None, None), "gone with the favorite");
        // A tier's box has a harness of its own, and v comes back to that box.
        press(&mut a, "2l vj");
        code(&mut a, KeyCode::Enter);
        assert_eq!((a.store.via("coding:mid"), a.open_list().map(|l| l.col)), (Some("opencode"), Some(2)));
    }

    #[test]
    fn f_favorites_a_model_for_the_task_at_hand() {
        let mut a = app();
        // No task in context: f asks which, a row per task with a box for it and for each tier.
        assert_eq!(press(&mut a, "f"), None);
        assert!(matches!(&a.input, Input::Choose { items, .. } if items.len() == TASKS.len() + 1));
        press(&mut a, "j");
        assert_eq!(code(&mut a, KeyCode::Enter), Some(Effect::Save));
        assert!(a.choosing_favs() && a.store.favorite("coding").is_some(), "enter ticks as space does");
        code(&mut a, KeyCode::Esc);
        assert_eq!(a.store.favorite("coding"), Some("gpt55"), "the second task is coding");
        assert!(a.starred("gpt55") && !a.starred("mini"), "★ with no task: favorite to any");
        press(&mut a, "f");
        assert!(
            matches!(&a.input, Input::Choose { list: List { sel: 1, col: 0, .. }, .. }),
            "f starts on the task it is the favorite for"
        );
        code(&mut a, KeyCode::Esc);
        // In recommend, f starts on the task under the cursor, on the box its favorite has there.
        press(&mut a, "Rjjl");
        assert_eq!(a.current().unwrap().key, "gpt55", "the task's favorite is every tier's");
        press(&mut a, "f");
        assert!(matches!(&a.input, Input::Choose { list: List { sel: 1, col: 0, .. }, .. }));
        code(&mut a, KeyCode::Esc);
        a.store.toggle_favorite("coding", "gpt55");
        a.rebuild();
        // On a tier's own pick, f starts on that tier's box, so f enter is for the tier alone.
        assert_eq!(a.current().unwrap().key, "mini");
        assert_eq!(press(&mut a, "f"), None);
        assert!(matches!(&a.input, Input::Choose { list: List { sel: 1, col: 1, .. }, .. }));
        assert_eq!(code(&mut a, KeyCode::Enter), Some(Effect::Save));
        assert_eq!((a.store.favorite("coding:low"), a.store.favorite("coding")), (Some("mini"), None));
        assert!(a.status.starts_with("★ mini"));
        code(&mut a, KeyCode::Esc);
        // Space ticks and keeps the list open, its boxes following.
        press(&mut a, "f");
        assert_eq!(press(&mut a, " "), Some(Effect::Save));
        assert_eq!(a.store.favorite("coding:low"), None, "again unfavorites");
        // And again: --tier low picks mini, the others the computed one.
        press(&mut a, " ");
        assert_eq!((a.store.favorite("coding:low"), a.store.favorite("coding:mid")), (Some("mini"), None));
        assert!(a.status.ends_with("coding:low"));
        // ] [ go round the rows with a tick: coding at 1 and, once ticked, overall at 0. The
        // box under the cursor stays the tier's from row to row, until h.
        press(&mut a, "]");
        assert!(a.failed && a.status == "no other ticked task is shown", "{}", a.status);
        press(&mut a, "kh ");
        assert_eq!(a.store.favorite("overall"), Some("mini"));
        for (key, sel) in [("]", 1), ("]", 0), ("[", 1), ("2[", 0), ("2]", 1)] {
            press(&mut a, key);
            assert!(matches!(&a.input, Input::Choose { list: List { sel: s, .. }, .. } if *s == sel), "{key} to {sel}");
        }
        press(&mut a, "l kh");
        assert_eq!((a.store.favorite("coding:low"), a.store.favorite("overall")), (None, Some("mini")));
        press(&mut a, " ");
        code(&mut a, KeyCode::Esc);
        assert_eq!((&a.input, a.store.favorite_for("mini").len()), (&Input::None, 0));
        // A favorite model joins the task's line even off the frontier: opus5 has no coding score
        // in this fixture, so llama4 (40, below the floor) stands in once it is shown with a.
        press(&mut a, "R");
        press(&mut a, "a");
        a.store.toggle_favorite("coding", "llama4");
        a.rebuild();
        press(&mut a, "R2gg");
        assert_eq!(a.task_cur, 1, "on coding");
        let front: Vec<_> = a.task_frontier(fit::task("coding").unwrap()).iter().map(|(m, _)| m.key.clone()).collect();
        assert_eq!(front, ["llama4", "mini", "gpt55"], "cheapest first, the favorite one among them");
        // enter shows the task in the table, where f acts on the picked task and ★ marks its model.
        code(&mut a, KeyCode::Enter);
        assert_eq!(keys(&a), ["gpt55", "llama4", "mini"], "priciest first, then names: llama4 and mini both cost 1");
        assert!(a.starred("llama4") && !a.starred("gpt55"));
        press(&mut a, "ggf");
        assert_eq!(code(&mut a, KeyCode::Enter), Some(Effect::Save), "f starts on the picked task");
        code(&mut a, KeyCode::Esc);
        assert_eq!(a.store.favorite("coding"), Some("gpt55"));
        assert_eq!(keys(&a), ["gpt55", "mini"], "llama4 leaves the line with the fav");
    }

    #[test]
    fn shift_r_toggles_the_recommend_overlay() {
        let mut a = app();
        press(&mut a, "R");
        assert_eq!(a.view, View::Recommend);
        press(&mut a, "R");
        assert_eq!(a.view, View::Table, "R again closes it");
        press(&mut a, "R");
        code(&mut a, KeyCode::Esc);
        assert_eq!(a.view, View::Table, "esc closes it too");
    }

    #[test]
    fn recommend_moves_a_model_cursor_that_the_row_keys_act_on() {
        let mut a = app();
        press(&mut a, "R");
        assert_eq!((a.among, a.current().is_none()), (Some(YOURS), true), "the cursor starts on the among line");
        press(&mut a, "jj");
        let front: Vec<String> =
            a.task_frontier(fit::task("coding").unwrap()).iter().map(|(m, _)| m.key.clone()).collect();
        assert_eq!(front, ["mini", "gpt55"], "cheapest first, best last");
        assert!(a.current().is_none(), "j k land on the name");
        assert!(press(&mut a, "o").is_none() && a.failed, "where the row keys have no model");
        assert_eq!(a.status, "no model under the cursor: l picks a tier");
        press(&mut a, "l");
        assert_eq!(a.current().unwrap().key, "mini", "l steps onto the cheapest");
        press(&mut a, "l");
        assert_eq!(a.current().unwrap().key, "gpt55");
        press(&mut a, "l");
        assert_eq!(a.current().unwrap().key, "gpt55", "high is the best, mid's too here");
        press(&mut a, "l");
        assert!(a.current().is_none(), "wraps to the name");
        press(&mut a, "h");
        assert_eq!(a.current().unwrap().key, "gpt55", "and back");
        press(&mut a, "0");
        assert!(a.current().is_none(), "0 goes to the name");
        a.mouse(Mouse::Cols(1));
        assert_eq!(a.current().unwrap().key, "mini", "the sideways wheel moves the cursor as in compare");
        code(&mut a, KeyCode::Enter);
        assert!(matches!(a.view, View::Detail(Back::Recommend(_))), "enter on a model opens its details");
        code(&mut a, KeyCode::Esc);
        assert_eq!((&a.view, a.current().unwrap().key.as_str()), (&View::Recommend, "mini"), "esc goes back");
        press(&mut a, "0$");
        assert_eq!(a.current().unwrap().key, "gpt55");
        // j k stay on the tier: the favorite of a task with no ranking is every tier's.
        a.store.toggle_favorite(TASKS[2].name, "mini");
        a.rebuild();
        press(&mut a, "j");
        assert_eq!((a.task_cur, a.task_sel, a.current().unwrap().key.as_str()), (2, 3, "mini"));
        press(&mut a, "k");
        assert_eq!((a.task_cur, a.current().unwrap().key.as_str()), (1, "gpt55"), "high again");
        press(&mut a, "j0lk");
        assert_eq!((a.task_cur, a.current().unwrap().key.as_str()), (1, "mini"), "low of both");
        a.store.toggle_favorite(TASKS[2].name, "mini");
        a.rebuild();
        press(&mut a, "$");
        press(&mut a, "oG");
        assert_eq!(
            code(&mut a, KeyCode::Enter),
            Some(Effect::Open("https://openrouter.ai/x/gpt55".into())),
            "o opens the picked model"
        );
        assert!(matches!(press(&mut a, "Y"), Some(Effect::Copy(id)) if id.contains("gpt55")));
        assert_eq!(press(&mut a, "e"), Some(Effect::Save), "e excludes it, so the tier picks another");
        assert_eq!(a.current().unwrap().key, "mini", "the best of what is left");
        assert_eq!(press(&mut a, "e"), Some(Effect::Save));
        assert!(a.current().is_none(), "a tier with no model left has none");
        assert!(press(&mut a, "o").is_none() && a.failed, "where the row keys say so");
        code(&mut a, KeyCode::Enter);
        assert_eq!((&a.view, a.status.as_str()), (&View::Recommend, "this tier has no model"), "and enter too");
        press(&mut a, "j");
        assert_eq!((a.task_cur, a.task_sel, a.current().is_none()), (2, 3, true), "j k stay on the tier");
        press(&mut a, "RR");
        assert_eq!((a.among, a.task_sel), (Some(YOURS), 0), "reopening starts on the among line");
    }

    #[test]
    fn the_table_cursor_follows_the_model_picked_in_an_overlay() {
        let mut a = app();
        let row = |a: &App, key: &str| a.rows.iter().position(|&i| a.data.models[i].key == key).unwrap();
        press(&mut a, "Rjjl");
        assert_eq!(a.selected(), row(&a, "mini"), "recommend's pick");
        a.mouse(Mouse::Cols(1));
        assert_eq!(a.selected(), row(&a, "gpt55"), "the wheel too");
        a.mouse(Mouse::Cols(1));
        assert_eq!(a.selected(), row(&a, "gpt55"), "a task's name leaves it");
        code(&mut a, KeyCode::Esc);
        assert_eq!((&a.view, a.current().unwrap().key.as_str()), (&View::Table, "gpt55"), "esc lands on it");
        a.store.marked = vec!["gpt55".into(), "mini".into()];
        a.rebuild();
        press(&mut a, "Cl");
        assert_eq!(a.selected(), row(&a, "mini"), "compare's too");
        press(&mut a, "h");
        assert_eq!(a.selected(), row(&a, "gpt55"));
    }

    #[test]
    fn a_sort_keeps_the_range_on_its_models() {
        let mut a = app();
        press(&mut a, "vj");
        let mut before = a.targets();
        before.sort();
        // Every column both ways: whichever parts the two, they stay the two.
        for col in TEXT..NCOLS {
            a.col = col;
            for _ in 0..2 {
                press(&mut a, "s");
                let mut now = a.targets();
                now.sort();
                assert_eq!(now, before, "sorted by {}", a.col_name(col));
            }
        }
    }

    #[test]
    fn a_refresh_keeps_the_selection() {
        let mut a = app();
        press(&mut a, "vj");
        let same = Data { models: std::mem::take(&mut a.data.models), ..Data::default() };
        a.refreshed(Ok(same), tools());
        assert_eq!((a.visual_range(), a.selected()), (Some(0..=1), 1), "the range follows its models");
        a.mouse(Mouse::Pick(2));
        let same = Data { models: std::mem::take(&mut a.data.models), ..Data::default() };
        a.refreshed(Ok(same), tools());
        assert_eq!(a.picked, [0, 1, 2], "so do picked rows");
    }

    #[test]
    fn compare_on_one_highlighted_row_keeps_the_selection() {
        let mut a = app();
        press(&mut a, " G ");
        assert_eq!(press(&mut a, "vC"), None);
        assert_eq!((&a.view, a.store.marked.len(), a.refused), (&View::Table, 2, true));
    }

    #[test]
    fn mark_only_marked_and_compare() {
        let mut a = app();
        assert_eq!(press(&mut a, "C"), None);
        assert_eq!(a.view, View::Compare, "opens to say models must be marked first");
        code(&mut a, KeyCode::Esc);
        press(&mut a, " G ");
        assert_eq!(a.store.marked, vec!["gpt55", "opus5"]);
        press(&mut a, "S");
        assert_eq!(keys(&a), ["gpt55", "opus5"]);
        press(&mut a, "S");
        assert_eq!(a.rows.len(), 3);
        press(&mut a, "C");
        assert_eq!(a.view, View::Compare);
        press(&mut a, "ll");
        assert_eq!(a.compare_sel, 0, "l moves over the models and wraps");
        press(&mut a, "h");
        assert_eq!(a.compare_sel, 1, "h wraps the other way");
        press(&mut a, "0");
        assert_eq!(a.compare_sel, 0, "0 picks the first model");
        press(&mut a, "$");
        assert_eq!(a.compare_sel, 1, "$ picks the last model");
        press(&mut a, "2");
        a.mouse(Mouse::Model(Stop::Compare(0)));
        assert_eq!((&a.view, a.compare_sel), (&View::Compare, 0), "a click moves to the model");
        press(&mut a, "l");
        assert_eq!(a.compare_sel, 1, "and drops a count typed before it");
        press(&mut a, "h");
        a.mouse(Mouse::Open(Stop::Compare(1)));
        assert_eq!(
            (&a.view, a.current().unwrap().key.as_str()),
            (&View::Detail(Back::Compare(0)), "opus5"),
            "a double click opens it"
        );
        code(&mut a, KeyCode::Esc);
        a.scroll = 3;
        code(&mut a, KeyCode::Enter);
        assert_eq!(
            (&a.view, a.current().unwrap().key.as_str(), a.scroll),
            (&View::Detail(Back::Compare(3)), "opus5", 0),
            "enter shows its details"
        );
        code(&mut a, KeyCode::Esc);
        assert_eq!(
            (&a.view, a.compare_sel, a.scroll),
            (&View::Compare, 1, 3),
            "esc goes back to compare, where it was"
        );
        assert_eq!(press(&mut a, "o"), None, "o lists the sites even when only OpenRouter has it");
        assert_eq!(
            code(&mut a, KeyCode::Enter),
            Some(Effect::Open("https://openrouter.ai/x/opus5".into())),
            "o opens the selected model"
        );
        press(&mut a, "/eci");
        assert_eq!((a.overlay_query.as_str(), a.query.as_str()), ("eci", ""), "/ in compare filters its rows");
        code(&mut a, KeyCode::Enter);
        assert_eq!(a.input, Input::None);
        code(&mut a, KeyCode::Esc);
        assert_eq!((&a.view, a.overlay_query.as_str()), (&View::Compare, ""), "esc clears the filter first");
        assert_eq!(a.input, Input::None);
        press(&mut a, "$e");
        assert!(
            a.store.is_excluded("opus5") && a.selected() == 2,
            "e on the compared model leaves the table row alone"
        );
        press(&mut a, "e");
        press(&mut a, "q");
        assert_eq!(a.input, Input::Quit, "q asks to quit from any view");
        code(&mut a, KeyCode::Esc);
        assert_eq!(a.view, View::Compare, "any other key only cancels the question");
        code(&mut a, KeyCode::Esc);
        assert_eq!(a.view, View::Table, "esc closes the overlay");
        press(&mut a, "Sc");
        assert_eq!(
            (a.marked_models().len(), a.only == Some(MARKED), a.rows.len()),
            (2, false, 3),
            "c keeps them and leaves S"
        );
        press(&mut a, "ggj S");
        assert_eq!(keys(&a), ["gpt55", "mini", "opus5"]);
        press(&mut a, "ggvG ");
        assert_eq!(
            (a.store.marked.len(), a.only == Some(MARKED), a.rows.len()),
            (0, false, 3),
            "unmarking every marked model leaves S for every model"
        );
        press(&mut a, "u");
        assert_eq!(a.status, NO_SELECTED);
        press(&mut a, "gg j S");
        assert_eq!(press(&mut a, "u"), Some(Effect::Save), "u saves");
        assert_eq!(
            (a.store.marked.len(), a.only == Some(MARKED), a.rows.len()),
            (0, false, 3),
            "u unmarks all and leaves S"
        );
    }

    #[test]
    fn brackets_jump_between_selected_models() {
        let mut a = app();
        a.store.toggle_marked("gone");
        a.rebuild();
        for k in ["]", "S", "u"] {
            press(&mut a, k);
            assert_eq!(a.status, NO_SELECTED, "{k}: a model gone is none");
        }
        a.store.toggle_marked("opus5");
        a.rebuild();
        assert_eq!(press(&mut a, "u"), Some(Effect::Save), "u clears the gone model's mark too");
        assert_eq!((a.store.marked.len(), a.status.as_str()), (0, "deselected 1, and 1 no longer listed"));
        a.store.toggle_marked("opus5");
        a.store.toggle_marked("gpt55");
        a.rebuild();
        press(&mut a, "S");
        a.data.models.retain(|m| m.key != "gpt55");
        a.rebuild();
        assert!(a.only == Some(MARKED), "a refresh that drops one selected model keeps S");
        press(&mut a, "gg ");
        assert!(a.only != Some(MARKED), "unmarking the last one shown leaves S though a gone one's mark stays");
        a = app();
        press(&mut a, "a");
        assert_eq!(keys(&a), ["gpt55", "mini", "llama4", "opus5"]);
        for k in ["gpt55", "llama4", "opus5"] {
            a.store.toggle_marked(k);
        }
        a.rebuild();
        press(&mut a, "2gg]");
        assert_eq!(a.selected(), 2, "from mini, not selected, to the next selected below");
        press(&mut a, "2gg[");
        assert_eq!(a.selected(), 0, "and the previous above");
        press(&mut a, "]");
        assert_eq!(a.selected(), 2);
        press(&mut a, "2]");
        assert_eq!(a.selected(), 3, "a count stops at the last, as 5j does");
        press(&mut a, "]");
        assert_eq!(a.selected(), 0, "past the last back to the first");
        press(&mut a, "[");
        assert_eq!(a.selected(), 3, "before the first back to the last");
        a.query = "gpt55".into();
        a.rebuild();
        press(&mut a, "]");
        assert_eq!(a.status, "no other selected model is shown");
        a.query = "mini".into();
        a.rebuild();
        press(&mut a, "]");
        assert_eq!(a.status, "no selected model is shown");
    }

    #[test]
    fn braces_jump_between_available_models() {
        let mut a = app();
        press(&mut a, "a");
        press(&mut a, "2gg}");
        assert_eq!(a.selected(), 3, "past llama4, which you have no access to");
        press(&mut a, "{");
        assert_eq!(a.selected(), 1);
        press(&mut a, "}}");
        assert_eq!(a.selected(), 0, "past the last back to the first");
        a.query = "opus".into();
        a.rebuild();
        press(&mut a, "}");
        assert_eq!(a.status, "no other available model is shown");
        a.query = "llama".into();
        a.rebuild();
        press(&mut a, "}");
        assert_eq!(a.status, "no available model is shown");
        a.any_available = false;
        press(&mut a, "}");
        assert_eq!(a.status, NO_ACCESS, "as a says with access to no model");
        press(&mut a, "Ra");
        assert_eq!((&a.view, a.status.as_str()), (&View::Recommend, NO_ACCESS), "and in recommend, which stays open");
        a.status.clear();
        a.mouse(Mouse::Tab(ALL));
        assert_eq!((&a.view, a.status.as_str()), (&View::Recommend, NO_ACCESS), "a click on the tab too");
        press(&mut a, "R");
        // A refresh's early data, a harness yet to list its models: too soon to say so.
        a.data.listing = true;
        assert!(!a.no_access());
    }

    #[test]
    fn esc_backs_out_of_views_and_toggles_close_what_they_open() {
        let mut a = app();
        press(&mut a, "S");
        assert_eq!((a.only == Some(MARKED), a.status.as_str()), (false, NO_SELECTED));
        press(&mut a, " S/x");
        code(&mut a, KeyCode::Enter);
        code(&mut a, KeyCode::Esc);
        assert_eq!((a.query.as_str(), a.only == Some(MARKED)), ("", true), "esc clears the search first");
        code(&mut a, KeyCode::Esc);
        assert_eq!((a.only == Some(MARKED), a.rows.len()), (false, 3), "then leaves S");
        assert_eq!(a.store.marked, ["gpt55"], "without touching the marks");
        press(&mut a, "F");
        assert_eq!(
            (a.only == Some(FAV), a.status.as_str()),
            (false, "no favorites: f favorites the one under the cursor")
        );
        a.store.toggle_favorite("coding", "opus5");
        a.rebuild();
        press(&mut a, "F");
        assert_eq!(keys(&a), ["opus5"], "F shows the favorites only");
        press(&mut a, "Sc");
        assert_eq!((a.only == Some(MARKED), a.only == Some(FAV), a.rows.len()), (false, false, 3), "c leaves S and F");
        press(&mut a, "SF");
        assert_eq!((a.only == Some(MARKED), keys(&a)), (false, vec!["opus5"]), "F leaves S for the favorites alone");
        press(&mut a, "F");
        a.mouse(Mouse::Tab(FAV));
        a.mouse(Mouse::Tab(MARKED));
        press(&mut a, "G");
        a.mouse(Mouse::Top);
        assert_eq!(a.selected(), 0, "the # header goes to the first row");
        assert!((a.only == Some(MARKED), a.only == Some(FAV)) == (true, false), "the ✓ and ★ tabs do what S and F do");
        code(&mut a, KeyCode::Esc);
        assert_eq!((a.only == Some(MARKED), a.rows.len()), (false, 3), "esc leaves S");
        press(&mut a, "F");
        code(&mut a, KeyCode::Esc);
        assert_eq!((a.only == Some(FAV), a.rows.len()), (false, 3), "and F");
        a.store.toggle_favorite("coding", "opus5");
        press(&mut a, "Rjj");
        code(&mut a, KeyCode::Enter);
        assert!(a.task.is_some());
        code(&mut a, KeyCode::Esc);
        assert!(a.task.is_none(), "esc drops the task");
        assert_eq!((&a.view, a.task_cur), (&View::Recommend, 1), "and goes back to recommend");
        assert_eq!((a.sort_col, a.descending), default_sort(), "and the sort it started with");
        code(&mut a, KeyCode::Esc);
        press(&mut a, "CC");
        assert_eq!(a.view, View::Table, "C closes compare, as ? and R close theirs");
        press(&mut a, "c");
        a.mouse(Mouse::Tab(EXCLUDED));
        assert_eq!(
            (a.only == Some(EXCLUDED), a.status.as_str()),
            (false, "no excluded models: e excludes the one under the cursor")
        );
        a.store.toggle_excluded("mini");
        a.rebuild();
        a.mouse(Mouse::Tab(EXCLUDED));
        assert_eq!(keys(&a), ["mini"], "the ✗ tab, as E, shows the excluded only");
        code(&mut a, KeyCode::Esc);
        assert_eq!((a.only == Some(EXCLUDED), a.rows.len()), (false, 3), "esc leaves E");
    }

    #[test]
    fn visual_ranges_and_marks_act_on_groups() {
        let mut a = app();
        press(&mut a, "vj");
        assert_eq!(a.visual_range(), Some(0..=1));
        assert_eq!(press(&mut a, "e"), Some(Effect::Save));
        assert!(a.store.is_excluded("gpt55") && a.store.is_excluded("mini") && !a.store.is_excluded("opus5"));
        assert_eq!((a.visual, a.status.as_str()), (None, "excluded 2 models"), "an action ends the range");
        press(&mut a, "ggvje");
        assert!(!a.store.is_excluded("gpt55") && !a.store.is_excluded("mini"), "all had it: cleared");
        press(&mut a, "Gvk");
        code(&mut a, KeyCode::Esc);
        assert_eq!((a.visual, a.rows.len()), (None, 3), "esc cancels the range only");
        press(&mut a, "ggvj ");
        assert_eq!(a.store.marked, ["gpt55", "mini"]);
        press(&mut a, "Ge");
        assert!(a.store.is_excluded("opus5") && !a.store.is_excluded("gpt55"), "e on an unmarked row acts on it alone");
        press(&mut a, "Gegge");
        assert!(a.store.is_excluded("gpt55") && !a.store.is_excluded("mini"), "e on a mark acts on it alone too");
        press(&mut a, "Ve");
        assert!(
            a.store.is_excluded("gpt55") && a.store.is_excluded("mini") && !a.store.is_excluded("opus5"),
            "Ve: all marks"
        );
        press(&mut a, "ggvjj ");
        assert_eq!(a.store.marked, ["gpt55", "mini", "opus5"], "space on a partly marked range marks the rest");
        press(&mut a, "ggvjj ");
        assert_eq!(
            (a.store.marked.len(), a.status.as_str()),
            (0, "deselected 3 models"),
            "space on an all-marked range unmarks it"
        );
        press(&mut a, " GvkC");
        assert_eq!(
            (&a.view, a.marked_models().len()),
            (&View::Compare, 2),
            "C compares the range alone, the earlier mark dropped"
        );
    }

    #[test]
    fn box_click_toggles_the_mark_and_star_click_picks_tasks() {
        let mut a = app();
        assert_eq!(a.mouse(Mouse::Box(1)), Some(Effect::Save));
        assert_eq!((a.selected(), a.store.is_marked("mini")), (1, true), "☐ → ✓, and the cursor goes there");
        a.mouse(Mouse::Box(1));
        assert!(!a.store.is_marked("mini"), "✓ → ☐");
        assert_eq!(a.mouse(Mouse::Star(2)), None);
        assert!(a.choosing_favs() && a.selected() == 2, "the ☆ lists the tasks for its row");
        assert_eq!(a.mouse(Mouse::Tick(1, 0)), Some(Effect::Save));
        assert!(a.choosing_favs(), "a click ticks a box and keeps the grid open");
        assert_eq!(a.mouse(Mouse::Tick(1, 2)), Some(Effect::Save));
        assert_eq!((a.store.favorite("coding"), a.store.favorite("coding:mid")), (Some("opus5"), Some("opus5")));
        a.mouse(Mouse::Tick(1, 2));
        code(&mut a, KeyCode::Esc);
        // On a selected row f is for that row alone, the others selected or not.
        a.mouse(Mouse::Box(1));
        a.mouse(Mouse::Box(2));
        press(&mut a, "f");
        assert!(
            matches!(&a.input, Input::Choose { items, .. } if matches!(&items[0].1, Effect::Fav(k, ..) if k == "opus5")),
            "{:?}",
            a.input
        );
        code(&mut a, KeyCode::Esc);
        // A task has one favorite: with several highlighted, f says so and asks nothing.
        assert_eq!((press(&mut a, "Vf"), a.choosing_favs()), (None, false));
        assert_eq!(a.status, "a task has one favorite: f takes one model, 2 are highlighted");
        // e on a selected row excludes that row alone, and Ve every selected one.
        code(&mut a, KeyCode::Esc);
        press(&mut a, "e");
        assert!(a.store.excluded.iter().eq(["opus5"]));
        press(&mut a, "eVe");
        assert_eq!((a.store.excluded.len(), a.status.as_str(), a.selecting()), (2, "excluded 2 models", false));
        press(&mut a, "Ve");
        assert!(a.store.excluded.is_empty());
        press(&mut a, "ggvjf");
        assert!(!a.choosing_favs() && a.status.ends_with("2 are highlighted"), "{}", a.status);
        a.mouse(Mouse::Star(2));
        assert!(a.choosing_favs(), "a click on the ☆ is for its row alone");
        code(&mut a, KeyCode::Esc);
        // One highlighted row is the one f takes, as e does, wherever the cursor went.
        a.mouse(Mouse::Pick(1));
        press(&mut a, "jf");
        assert!(
            matches!(&a.input, Input::Choose { items, .. } if matches!(&items[0].1, Effect::Fav(k, ..) if k == "mini")),
            "{:?}",
            a.input
        );
    }

    #[test]
    fn a_click_on_the_box_excludes_the_model_and_takes_it_back() {
        let mut a = app();
        let key = |a: &App, n: usize| a.data.models[a.rows[n]].key.clone();
        let (k, other) = (key(&a, 1), key(&a, 2));
        a.mouse(Mouse::Row(2));
        a.mouse(Mouse::Extend(0));
        assert_eq!(a.mouse(Mouse::Exclude(1)), Some(Effect::Save));
        assert!(a.store.is_excluded(&k) && !a.store.is_excluded(&other), "the clicked row alone, not the range");
        assert_eq!((a.selected(), a.selecting()), (1, false));
        assert_eq!(a.mouse(Mouse::Exclude(1)), Some(Effect::Save));
        assert!(!a.store.is_excluded(&k), "a second click takes the exclusion off");
        press(&mut a, "gg ");
        a.mouse(Mouse::Box(1));
        a.mouse(Mouse::Exclude(1));
        assert!(a.store.is_excluded(&k) && !a.store.is_excluded(&key(&a, 0)), "on a mark, that row alone");
        // Under E a click that drops the row leaves the cursor where it was, as e does.
        for n in 0..3 {
            if !a.store.is_excluded(&key(&a, n)) {
                a.mouse(Mouse::Exclude(n));
            }
        }
        press(&mut a, "E");
        a.select(1);
        a.mouse(Mouse::Exclude(1));
        assert_eq!((a.rows.len(), a.selected()), (2, 1));
    }

    #[test]
    fn via_sorts_as_it_reads() {
        let mut a = app();
        a.store.toggle_marked("llama4");
        (a.sort_col, a.descending) = (VIA, false);
        a.rebuild();
        assert_eq!(keys(&a), ["opus5", "gpt55", "mini", "llama4"], "not available after codex");
        // The ones that read alike by the size of their download, one with none last either way.
        a.data.models.iter_mut().for_each(|m| m.hf = Some(format!("o/{}", m.key)));
        a.tools = (vec![crate::data::OLLAMA], vec![]);
        a.sized("mini", Download::Size(1));
        assert_eq!(keys(&a), ["opus5", "mini", "gpt55", "llama4"], "a size coming in sorts again");
        a.sized("gpt55", Download::Size(2));
        assert_eq!(keys(&a), ["opus5", "mini", "gpt55", "llama4"], "the smaller first");
        a.descending = true;
        a.rebuild();
        assert_eq!(keys(&a), ["llama4", "gpt55", "mini", "opus5"], "and the larger");
    }

    #[test]
    fn notes_copy_open() {
        let mut a = app();
        assert_eq!(press(&mut a, "nfast"), None);
        assert_eq!(code(&mut a, KeyCode::Enter), Some(Effect::Save));
        assert_eq!(a.store.note("gpt55"), Some("fast"));
        assert_eq!(press(&mut a, "o"), None, "o asks which site");
        assert!(
            matches!(&a.input, Input::Choose { items, .. } if items.len() == 2),
            "epoch.ai, openrouter.ai: neither models.dev nor Artificial Analysis has a page for it"
        );
        assert_eq!(code(&mut a, KeyCode::Enter), Some(Effect::Open("https://epoch.ai/models/gpt55".into())));
        press(&mut a, "oG");
        let last = Some(Effect::Open("https://openrouter.ai/x/gpt55".into()));
        assert_eq!(code(&mut a, KeyCode::Enter), last, "openrouter.ai last");
        a.data.models[0].openrouter = None;
        assert_eq!(press(&mut a, "o"), None, "one page: still listed");
        assert_eq!(code(&mut a, KeyCode::Enter), Some(Effect::Open("https://epoch.ai/models/gpt55".into())));
        a.data.models[0].epoch = None;
        assert_eq!((press(&mut a, "o"), a.status.as_str()), (None, "no site has a page for gpt55"), "none: it says so");
        assert!(a.failed, "in red, as nothing was opened");
        a.data.models[0].epoch = Some("gpt55".into());
        assert_eq!(press(&mut a, "Y"), Some(Effect::Copy("p/gpt55".into())));
        a.data.models[0].name = "GPT 5.5".into();
        assert_eq!(press(&mut a, "y"), Some(Effect::Copy("GPT 5.5".into())));
    }

    #[test]
    fn a_refresh_that_drops_models_keeps_the_cursors_valid() {
        let mut a = app();
        press(&mut a, " j C$");
        assert_eq!((&a.view, a.compare_sel), (&View::Compare, 1));
        let one = Data { models: a.data.models.drain(..1).collect(), ..Data::default() };
        a.refreshed(Ok(one), tools());
        assert_eq!((a.compare_sel, a.rows.len(), a.selected()), (0, 1, 0), "clamped to the models left");
        assert_eq!(a.current().map(|m| m.key.as_str()), Some("gpt55"));
    }

    #[test]
    fn refresh_once_at_a_time() {
        let mut a = app();
        assert_eq!(press(&mut a, "r"), Some(Effect::Refresh));
        assert!(a.refreshing && a.status.is_empty(), "only the ⟳ indicator says so");
        assert_eq!(press(&mut a, "r"), None);
        a.refreshed(Err("offline".into()), tools());
        assert!(a.failed && a.refresh_failed, "a failure is shown as one");
        assert_eq!(a.status, "refresh failed: offline");
        assert_eq!(press(&mut a, "r"), Some(Effect::Refresh));
        assert!(!a.failed && a.status.is_empty() && a.refresh_failed, "the frame keeps saying it failed");
        a.refreshed(Ok(Data::default()), tools());
        assert!(!a.refresh_failed, "until one succeeds");
        assert_eq!(a.status, "data refreshed");
        // Nor after a refusal a key has since cleared.
        press(&mut a, "u");
        press(&mut a, "j");
        a.status = "benchmarks from Epoch AI · B to change".into();
        a.refreshed(Ok(Data::default()), tools());
        assert!(a.status.ends_with("B to change"), "what a switch said outlasts its download");
        a.report(Err("user.json is not valid".into()));
        a.refreshed(Ok(Data::default()), tools());
        assert_eq!(a.status, "user.json is not valid", "an error still standing is not replaced by good news");
        a.refreshed(Ok(Data { warning: Some("pi did not list its models".into()), ..Default::default() }), tools());
        assert_eq!(a.status, "user.json is not valid; pi did not list its models", "nor by a warning");
        // Why a key did nothing is red too, and no error: the refresh's outcome replaces it.
        press(&mut a, "u");
        assert!(a.failed && a.status.starts_with("no selected models"));
        a.refreshed(Ok(Data::default()), tools());
        assert_eq!((a.failed, a.status.as_str()), (false, "data refreshed"));
        press(&mut a, "u");
        a.refreshed(Ok(Data { warning: Some("pi did not list its models".into()), ..Default::default() }), tools());
        assert_eq!(a.status, "pi did not list its models", "as its warning does");
        // A message set under an open list is no error, whatever stood before it.
        a.first_start = true;
        a.switch(crate::data::source());
        assert!(!a.failed && a.status.starts_with("benchmarks from"));
        a.report(Ok("done".into()));
        assert!(!a.failed);
    }

    #[test]
    fn mouse_selects_sorts_and_scrolls() {
        let mut a = app();
        a.mouse(Mouse::Tab(MARKED));
        assert!(a.failed && a.status.starts_with("no selected models"), "{}", a.status);
        assert_eq!(a.mouse(Mouse::Row(2)), None);
        assert!(!a.failed && a.status.is_empty(), "the next click clears it, as a key does");
        assert_eq!(a.selected(), 2);
        assert_eq!(a.mouse(Mouse::Row(2)), None);
        assert_eq!(a.view, View::Table, "a click only highlights, however often");
        a.mouse(Mouse::Row(1));
        assert_eq!(a.mouse(Mouse::Cell(2, 0)), None);
        assert_eq!(
            (a.selected(), &a.view),
            (2, &View::Detail(Back::Table)),
            "a double click on the name opens the details"
        );
        assert_eq!(a.mouse(Mouse::Scroll(3)), None);
        assert_eq!(a.scroll, 3);
        assert_eq!(a.mouse(Mouse::Row(0)), None, "clicks do nothing behind an overlay");
        assert_eq!(a.view, View::Detail(Back::Table));
        press(&mut a, "n");
        a.mouse(Mouse::Close);
        assert_eq!((&a.view, &a.input), (&View::Detail(Back::Table), &Input::None), "a note being written: only it");
        a.mouse(Mouse::Close);
        assert_eq!(a.view, View::Table, "a click outside the panel closes it");
        press(&mut a, "?/sort");
        a.mouse(Mouse::Close);
        assert_eq!((&a.view, &a.input, a.overlay_query.as_str()), (&View::Table, &Input::None, ""), "filtered too");
        // Every prompt is left by a click, as esc leaves it, and the wheel keeps it.
        for keys in ["q", ">", "/"] {
            press(&mut a, keys);
            a.mouse(Mouse::Scroll(1));
            assert_ne!(a.input, Input::None, "{keys}: the wheel keeps it open");
            a.mouse(Mouse::Row(0));
            assert_eq!(a.input, Input::None, "{keys}: a click leaves it");
        }
        // A search is kept, as enter keeps it, and the click is on what it found.
        let name = a.data.models[a.rows[1]].name.clone();
        press(&mut a, "/");
        press(&mut a, &name);
        a.mouse(Mouse::Row(0));
        let found = a.current().map(|m| m.name.clone());
        assert_eq!((&a.input, a.query.as_str(), found), (&Input::None, name.as_str(), Some(name.clone())));
        a.query.clear();
        a.rebuild();
        a.mouse(Mouse::Row(2));
        // Back in the details, for what follows.
        code(&mut a, KeyCode::Enter);
        code(&mut a, KeyCode::Esc);
        a.mouse(Mouse::Scroll(-1));
        assert_eq!(a.selected(), 1);
        assert_eq!(a.mouse(Mouse::Row(9)), None, "past the end is ignored");
        assert_eq!(a.selected(), 1);
        let open = |url: &str| Some(Effect::Open(url.into()));
        assert_eq!(a.mouse(Mouse::Cell(0, ECI)), open("https://epoch.ai/models/gpt55"), "a score: its source's page");
        assert_eq!((a.selected(), &a.view), (0, &View::Table), "its row highlighted");
        let provider = open("https://models.dev/providers/p/");
        assert_eq!(a.mouse(Mouse::Cell(0, PRICE)), provider, "a price: the page of the provider you'd pay");
        a.data.models[0].epoch = None;
        let said = (a.mouse(Mouse::Cell(0, ECI)), a.status.as_str());
        assert_eq!(said, (None, "gpt55 has no page on epoch.ai"), "a score Epoch has no page for says so");
        assert_eq!(a.mouse(Mouse::Cell(2, ECI)), None, "an empty cell opens nothing");
        let key = a.data.models[a.rows[2]].key.clone();
        a.store.set_note(&key, "slow");
        a.mouse(Mouse::Row(0));
        assert_eq!(a.mouse(Mouse::Cell(2, NOTES)), None);
        assert_eq!(a.input, Input::Note { key: key.clone(), text: "slow".into(), cur: 4 }, "a note: written, as n");
        assert_eq!(a.mouse(Mouse::Scroll(1)), None);
        assert!(matches!(a.input, Input::Note { .. }), "the wheel keeps it open");
        a.mouse(Mouse::Row(0));
        assert_eq!((&a.input, a.selected()), (&Input::None, 2), "a click leaves it, as esc does");
        a.store.set_note(&key, "");
        a.mouse(Mouse::Row(1));
        a.mouse(Mouse::Mark(2));
        assert_eq!((a.selected(), a.store.marked.len()), (2, 1), "right click marks and stays");
        a.mouse(Mouse::Mark(2));
        assert!(a.store.marked.is_empty(), "again unmarks");
        a.mouse(Mouse::Row(0));
        a.mouse(Mouse::Extend(1));
        a.mouse(Mouse::Extend(9));
        assert_eq!(a.visual_range(), Some(0..=2), "a drag selects from the click, clamped to the end");
        a.mouse(Mouse::Row(2));
        assert_eq!((a.visual, &a.view, a.selected()), (None, &View::Table, 2), "a plain click drops the range");
        a.mouse(Mouse::Extend(0));
        a.mouse(Mouse::Pick(1));
        assert_eq!(
            (a.visual, &a.picked, a.selected()),
            (None, &vec![0, 2], 1),
            "ctrl click: the range becomes picks, the row toggles off"
        );
        a.mouse(Mouse::Pick(1));
        assert!(a.is_selected(1) && !a.is_selected(9));
        assert_eq!(press(&mut a, "C"), Some(Effect::Save));
        assert_eq!((&a.view, a.store.marked.len(), a.selecting()), (&View::Compare, 3, false), "C compares the picks");
        code(&mut a, KeyCode::Esc);
        assert_eq!(a.selected(), 0, "the cursor stays on the model compare had picked");
        a.store.marked.clear();
        press(&mut a, "j");
        a.mouse(Mouse::Row(0));
        a.mouse(Mouse::Extend(1));
        a.mouse(Mouse::Mark(2));
        assert_eq!((a.selecting(), a.store.marked.len()), (false, 3), "right click: the range becomes marks too");
        a.store.marked.clear();
        a.mouse(Mouse::Row(0));
        a.mouse(Mouse::Extend(1));
        a.mouse(Mouse::Mark(0));
        assert_eq!((a.selecting(), a.store.marked.len(), a.selected()), (false, 2, 0), "inside the range: no unmark");
        assert_eq!(a.marked_shown, 2, "and the count of selected models follows");
        // From a panel, tab to compare keeps the selected models, as `C` there does.
        a.mouse(Mouse::Row(0));
        press(&mut a, "v");
        press(&mut a, "R");
        a.set_tab(COMPARE);
        assert_eq!((&a.view, a.store.marked.len()), (&View::Compare, 2), "the highlight is not what gets compared");
        // The details opened from a filtered compare go back to it filtered, by esc or a click.
        press(&mut a, "/pri");
        code(&mut a, KeyCode::Enter);
        code(&mut a, KeyCode::Enter);
        assert!(matches!(a.view, View::Detail(_)));
        a.mouse(Mouse::Close);
        assert_eq!((&a.view, a.overlay_query.as_str()), (&View::Compare, "pri"), "a click outside keeps the filter");
        a.overlay_query.clear();
        code(&mut a, KeyCode::Esc);
        code(&mut a, KeyCode::Esc);
        a.store.marked.clear();
        a.mouse(Mouse::Row(1));
        a.mouse(Mouse::Header(1));
        assert_eq!((a.col, a.sort_col, a.descending), (1, 1, false));
        a.mouse(Mouse::Header(1));
        assert!(a.descending);
        let sel = a.selected();
        press(&mut a, "/");
        a.mouse(Mouse::Scroll(1));
        assert_eq!(a.selected(), sel, "the wheel does nothing while typing");
        code(&mut a, KeyCode::Esc);
        a.mouse(Mouse::Cols(2));
        assert_eq!(a.col, 3);
        a.mouse(Mouse::Cols(-3));
        assert_eq!(a.col, 0);
        a.mouse(Mouse::Menu(0));
        assert_eq!(a.input, Input::None, "Model has no dropdown");
        a.mouse(Mouse::Menu(1));
        assert!(matches!(a.input, Input::Menu { col: 1, .. }));
        a.mouse(Mouse::Scroll(1));
        a.mouse(Mouse::Outside);
        assert_eq!((a.input == Input::None, a.col), (true, 1), "a click outside closes the dropdown");
        a.mouse(Mouse::Menu(1));
        assert_eq!(a.mouse(Mouse::Item(9)), None);
        assert!(matches!(a.input, Input::Menu { .. }), "past the end is ignored");
        a.mouse(Mouse::Item(1));
        assert!(matches!(a.input, Input::Menu { .. }), "a click keeps a multi-choice dropdown open");
        assert_eq!(a.dev, ["anthropic"]);
        a.mouse(Mouse::Item(2));
        assert_eq!(a.dev, ["anthropic", "openai"]);
        a.mouse(Mouse::Item(1));
        assert_eq!(a.dev, ["openai"]);
        a.mouse(Mouse::Row(0));
        assert_eq!((a.input == Input::None, a.dev.as_slice()), (true, &["openai".to_string()][..]));
        a.mouse(Mouse::Menu(PRICE));
        a.mouse(Mouse::Item(2));
        assert!(matches!(a.input, Input::Menu { .. }), "a click picks a Price level and keeps the dropdown open");
        assert_eq!(a.bounds, [(PRICE, f64::NEG_INFINITY, LEVELS[1])]);
    }

    #[test]
    fn detail_and_quit() {
        let mut a = app();
        code(&mut a, KeyCode::Enter);
        assert_eq!(a.view, View::Detail(Back::Table));
        press(&mut a, "jjj");
        assert_eq!(a.scroll, 3);
        press(&mut a, "gg");
        assert_eq!(a.scroll, 0);
        assert_eq!((press(&mut a, "q"), &a.input), (None, &Input::Quit), "q asks even in the detail");
        press(&mut a, "n");
        assert_eq!(code(&mut a, KeyCode::Esc), None, "esc closes the detail");
        assert_eq!(a.view, View::Table);
        assert_eq!((press(&mut a, "q"), &a.input), (None, &Input::Quit), "then asks");
        assert_eq!(press(&mut a, "n"), None);
        assert_eq!(a.input, Input::None, "any other key cancels");
        press(&mut a, "q");
        assert_eq!((press(&mut a, "y"), &a.input), (None, &Input::None), "only q confirms");
        assert_eq!(press(&mut a, "qq"), Some(Effect::Quit));
        for open in ["t", "f"] {
            let mut a = app();
            press(&mut a, open);
            assert!(matches!(a.input, Input::Choose { .. }), "{open} opens a list");
            assert_eq!(press(&mut a, "qq"), Some(Effect::Quit), "qq quits from {open}'s list");
            let mut a = app();
            press(&mut a, open);
            assert_eq!((press(&mut a, "jq"), &a.input), (None, &Input::Quit));
            press(&mut a, "n");
            let back = matches!(a.input, Input::Choose { list: List { sel: 1, .. }, .. });
            assert!(back, "any other key is back on {open}'s list, where it was");
            let mut a = app();
            press(&mut a, open);
            assert_eq!((press(&mut a, "/qq"), a.input == Input::Quit), (None, false), "a search types it");
        }
        let mut a = app();
        a.col = 0;
        press(&mut a, "ld");
        assert!(matches!(a.input, Input::Menu { .. }), "d opens a dropdown");
        assert_eq!((press(&mut a, "q"), &a.input), (None, &Input::Quit), "a dropdown asks too");
        press(&mut a, "n");
        assert!(matches!(a.input, Input::Menu { .. }), "and is back");
        assert_eq!(press(&mut a, "qq"), Some(Effect::Quit));
        let mut a = app();
        a.first_start = true;
        a.ask_source();
        assert!(press(&mut a, "q").is_none() && matches!(a.input, Input::Choose { .. }), "not on the first start");
        assert_eq!(ctrl(&mut a, 'c'), Some(Effect::Quit), "ctrl-c quits at once");
    }

    #[test]
    fn upgrade_only_to_a_newer_release() {
        let mut a = app();
        assert_eq!((press(&mut a, "U"), &a.input), (None, &Input::None), "nothing newer: nothing to ask");
        assert!(a.status.contains("not known"), "{}", a.status);
        a.data.latest = env!("CARGO_PKG_VERSION").into();
        press(&mut a, "U");
        assert!(a.status.contains("no newer version"), "{}", a.status);
        a.data.latest = "99.0.0".into();
        assert_eq!((press(&mut a, "U"), &a.input), (None, &Input::Upgrade), "U asks");
        assert_eq!((press(&mut a, "q"), &a.input), (None, &Input::None), "any other key cancels");
        assert_eq!(press(&mut a, "UU"), Some(Effect::Upgrade));
        assert_eq!(a.input, Input::None, "the TUI stays open when there is no command to run");
        press(&mut a, "U");
        a.set_data(Data::default());
        assert_eq!(a.input, Input::None, "a refresh that lost the release takes the question back");
    }

    #[test]
    fn value_is_one_pick_in_recommend() {
        let mut a = app();
        press(&mut a, "R");
        let value = TASKS.iter().position(|t| t.name == "value").unwrap();
        (a.among, a.task_cur, a.task_sel) = (None, value, 0);
        assert!(a.one_pick(value) && !a.one_pick(0), "a rank has no tiers to tell apart");
        assert_eq!((press(&mut a, "l"), a.task_sel), (None, 1));
        assert_eq!((press(&mut a, "l"), a.task_sel), (None, 0), "one box past the name");
        assert_eq!((press(&mut a, "$"), a.task_sel), (None, 1));
        a.store.toggle_favorite("value:low", "mini");
        a.rebuild();
        assert!(!a.one_pick(value), "a tier's favorite gives the tiers models of their own");
        assert_eq!((press(&mut a, "$"), a.task_sel), (None, 3));
    }

    #[test]
    fn dd_unfavorites_every_model() {
        let mut a = app();
        assert_eq!((press(&mut a, "D"), &a.input), (None, &Input::None), "no favorites: nothing to ask");
        assert!(a.status.contains("no favorites"), "{}", a.status);
        a.store.set_favorite("coding", "gpt55", Some("opencode"));
        a.store.set_favorite("coding:low", "flash", None);
        a.rebuild();
        press(&mut a, "F");
        assert_eq!((press(&mut a, "D"), &a.input), (None, &Input::Unfavorite), "D asks");
        assert_eq!((press(&mut a, "q"), a.store.favorite.len()), (None, 2), "any other key cancels");
        assert_eq!(press(&mut a, "DD"), Some(Effect::Save));
        assert!(a.store.favorite.is_empty() && a.store.via.is_empty() && a.only.is_none(), "and F is left");
    }

    #[test]
    fn xx_unexcludes_every_model() {
        let mut a = app();
        assert_eq!((press(&mut a, "X"), &a.input), (None, &Input::None), "none excluded: nothing to ask");
        assert!(a.status.contains("no excluded"), "{}", a.status);
        a.store.toggle_excluded("gpt55");
        a.store.toggle_excluded("flash");
        a.rebuild();
        press(&mut a, "E");
        assert_eq!((press(&mut a, "X"), &a.input), (None, &Input::Unexclude), "X asks");
        assert_eq!((press(&mut a, "q"), a.store.excluded.len()), (None, 2), "any other key cancels");
        assert_eq!(press(&mut a, "XX"), Some(Effect::Save));
        assert!(a.store.excluded.is_empty() && a.only.is_none(), "and E is left");
    }

    #[test]
    fn ctrl_w_takes_a_word_after_a_wide_space() {
        let mut a = app();
        a.input = Input::Note { key: "gpt55".into(), text: "a\u{3000}b".into(), cur: 5 };
        ctrl(&mut a, 'w');
        assert_eq!(a.input, Input::Note { key: "gpt55".into(), text: "a\u{3000}".into(), cur: 4 });
    }

    #[test]
    fn a_refresh_keeps_the_cursor_and_highlight_on_their_models() {
        let mut a = app();
        press(&mut a, "Gv");
        let key = a.current().unwrap().key.clone();
        let mut models = a.data.models.clone();
        models.reverse();
        a.refreshed(Ok(Data { fetched: 0, models, ..Default::default() }), tools());
        assert_eq!(a.current().unwrap().key, key);
        assert_eq!(a.targets(), [key.as_str()]);
    }

    #[test]
    fn a_new_note_refilters_the_table() {
        let mut a = app();
        a.store.set_note("mini", "speedy");
        press(&mut a, "/speedy");
        code(&mut a, KeyCode::Enter);
        assert_eq!(keys(&a), ["mini"]);
        press(&mut a, "n");
        ctrl(&mut a, 'u');
        press(&mut a, "slow");
        code(&mut a, KeyCode::Enter);
        assert!(keys(&a).is_empty());
    }

    #[test]
    fn the_fav_list_keeps_its_model_when_the_rows_move() {
        let mut a = app();
        a.store.toggle_favorite("coding", "opus5");
        a.task = Some(&TASKS[1]);
        a.rebuild();
        let i = a.rows.iter().position(|&r| a.data.models[r].key == "opus5").unwrap();
        a.select(i);
        // Unfavorited, opus5 leaves the task's line; the second tick is still for it.
        press(&mut a, "f  ");
        assert_eq!(a.store.favorite("coding"), Some("opus5"));
    }
}
