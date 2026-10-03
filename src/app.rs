//! TUI state and key handling. No I/O here: side effects come back to the shell as `Effect`s,
//! so every key is unit-testable.

use crate::data::{Data, Failure, Model, Source};
use crate::fit::{TASKS, Task};
use crate::store::Store;
use crate::view::{
    LEVELS, NO_ACCESS, NO_SELECTED, THEMES, by_value, ctx, custom_line, hits, in_reach, level_label, money, score,
    shown_via, task_line, task_score, truncate,
};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::widgets::TableState;
use std::cmp::Reverse;
use std::collections::BTreeMap;

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
    /// Only Artificial Analysis measures it: hidden with any other source (`hidden`).
    pub aa_only: bool,
    /// It hangs on the price, so a list price shows in it after a `~` (`on_price`).
    pub price: bool,
}

impl Col {
    /// Its name and meaning; the index column's are the benchmark source's, and so are the
    /// task columns' meanings, each a single benchmark with Artificial Analysis.
    fn text(&self) -> (&'static str, &'static str) {
        let about = match (self.id, crate::data::source()) {
            ("eci", s) => return s.index(),
            ("coding", Source::Aa) => "Artificial Analysis Coding Index",
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
        aa_only: false,
        price: false,
    }
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

/// The release, beside Dev, then prices from the offer you'd pay and context, the source's overall
/// index, the task scores and Code/$, then speed when Artificial Analysis measures it.
pub const COLS: [Col; 13] = [
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
        ..col("Ctx", "ctx", "context window, in tokens", |m| positive(m.context as f64 / 1000.0))
    },
    // Named by the source in use: `Col::text`.
    col("", "eci", "", |m| m.eci),
    col("Coding", "coding", "capability on coding benchmarks, ECI points", |m| task_score(m, "coding")),
    col("Agentic", "agentic", "capability on agentic benchmarks, ECI points", |m| task_score(m, "agentic")),
    col("Reason", "reasoning", "capability on reasoning benchmarks, ECI points", |m| task_score(m, "reasoning")),
    Col {
        price: true,
        ..col("Code/$", "value", "coding percentile ÷ Price, as a percentile, ~ on a list price", |m| {
            m.fit.get("value").copied()
        })
    },
    Col {
        aa_only: true,
        show: |v| format!("{v:.0}"),
        ..col("Tok/s", "tps", "output tokens per second, median across providers", |m| m.tps)
    },
    Col {
        aa_only: true,
        lower_better: true,
        show: |v| format!("{v:.1}s"),
        ..col("TTFT", "ttft", "seconds to the first token, median across providers", |m| m.ttft)
    },
];

/// Whether `COLS[i]` hangs on the price: Price, $in, $cache and $out, and Code/$, which divides by it.
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
const DEFAULT_SORT: (usize, bool) = (ECI, true);
/// Column index of ECI.
pub const ECI: usize = TEXT + 6;
/// Column index of Tok/s.
pub const SPEED: usize = TEXT + 11;
/// First column of each group: names and release, price and context, benchmarks, speed, your own.
pub const GROUPS: [usize; 5] = [0, PRICE, ECI, SPEED, VIA];
/// Column index of where you have access.
pub const VIA: usize = TEXT + COLS.len();
/// Column index of your note, the last one.
pub const NOTES: usize = VIA + 1;
pub const NCOLS: usize = NOTES + 1;

/// Whether the column at cursor index `col` is left out: one the source in use does not measure.
pub fn hidden(col: usize) -> bool {
    numeric(col).is_some_and(|c| c.aa_only && crate::data::source() != Source::Aa)
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
pub fn col_name(col: usize) -> &'static str {
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
    col == 1 || col == PRICE || col == VIA
}

/// What the column at cursor index `col` means.
pub fn col_about(col: usize) -> String {
    match col {
        0 => "model name (dimmed if Via is empty)".into(),
        1 => "company that trained the model".into(),
        VIA => "harnesses listing it, env if API key set".into(),
        NOTES => "your own note on the model".into(),
        PRICE => format!(
            "{}, {:.0}% of the input cached, ~ list price when yours has none",
            COLS[PRICE - TEXT].about,
            crate::data::cached() * 100.0
        ),
        _ => numeric(col).map_or("", Col::about).into(),
    }
}

pub const HELP: &[(&str, &[(&str, &str)])] = &[
    (
        "General",
        &[
            ("?", "this help; / keeps the lines that match"),
            ("esc", "back: overlay, highlight, filter, M, F, E, task"),
            ("q", "quit; asks first"),
            ("r", "refresh data now (auto at start after 24h)"),
            ("u", "upgrade modelcmp when a newer version is out; asks first"),
            ("B", "benchmarks from Epoch AI or Artificial Analysis"),
        ],
    ),
    (
        "Move",
        &[
            ("j k ↓ ↑", "move; a count repeats, as in 3j"),
            ("h l ← →", "pick a column; in compare a model, in recommend a model or the task"),
            ("0 _ $ w b", "first / last column; next / previous group"),
            ("gg G 3gg", "top / bottom / row 3"),
            ("( ) ^u ^d", "half a page up / down"),
            ("] [", "next / previous selected model"),
            ("} {", "next / previous available model"),
            ("v", "highlight a range; space e C act on all of it"),
        ],
    ),
    (
        "Filter and sort",
        &[
            ("s", "sort by the column; again reverses"),
            ("/", "filter models, compare rows, this help or a list"),
            ("> <", "minimum / maximum for the column, e.g. > 155 enter"),
            ("d", "dropdown on Dev, Price and Via (▾); space enter toggle"),
            ("a", "all models, including ones you have no access to"),
            ("%", "Price with none of the input cached, or back to --cache"),
            ("c", "clear filters, bounds, task, M, F and E; the selection stays"),
        ],
    ),
    (
        "Selected ✓, favorite ★, excluded ✗",
        &[
            ("✓", "selected: your shortlist for now; cleared when modelcmp closes"),
            ("★", "favorite: your pick for a task; always in its recommendation"),
            ("✗", "excluded: you have it but cannot use it; recommendations skip it"),
            ("space", "select the model; C compares the selected"),
            ("f", "favorite the model for a task, a tier of one, or a task you name"),
            ("r a", "in f's list: rename a task you named, write what it is about"),
            ("e", "exclude the model"),
            ("U", "deselect every model"),
            ("M F E", "selected / favorite / excluded only; again: every model"),
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
            ("R", "recommend: the best model per price for each task"),
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
    /// `u` asks before upgrading.
    Upgrade,
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
    /// `f`: the tasks a model is the favorite for; space or enter ticks one and the list stays open.
    Fav,
    /// `o`: the site to open the model on.
    Open,
    /// `x`: the harness to open on the model.
    Launch,
    /// `t`: the theme, previewed under the cursor.
    Theme,
    /// `B`: the benchmark source.
    Source,
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
    /// The entry under the cursor being written in place, in `f`'s list.
    pub edit: Option<Edit>,
}

/// An entry of `f`'s list written where it is, the list staying open: a task of your own being
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
        *self = List { sel: at, top: self.top, edit: Some(edit), ..Default::default() };
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

/// The last entry of `f`'s list: it asks the name of a task of your own.
const NEW_TASK: &str = "+ new task";

/// The part of a choice's label that `/` searches and marks. `f`'s tasks match by their name
/// alone, the slot after the box: a tick rewrites the rest of the label, and the entry would
/// leave the list from under the cursor.
pub fn searched((label, effect): &(String, Effect)) -> &str {
    match effect {
        Effect::Fav(_, slot) => slot,
        _ => label,
    }
}

/// Indices of the choice list entries whose searched part contains `query`, any case; empty
/// when none match, which the overlay says.
pub fn choice_rows(items: &[(String, Effect)], query: &str) -> Vec<usize> {
    let q = query.to_lowercase();
    (0..items.len()).filter(|&i| searched(&items[i]).to_lowercase().contains(&q)).collect()
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
    /// Favorite the model for the slot: a task, one tier of it, or a task of your own; the `f`
    /// chooser's items, applied by `App` itself.
    Fav(String, String),
    /// Ask the name of a new task of your own for the model; the last of the `f` chooser's
    /// items, applied by `App` itself.
    NewTask(String),
    /// A `view::THEMES` name; the `t` chooser's items, applied by `App` itself.
    Theme(&'static str),
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
    /// Click on the ✓ header: show marked models only, as `M` does.
    OnlyMarked,
    /// Click on the ★ header: show favorites only, as `F` does.
    OnlyFav,
    /// Click on the ✗ header: show excluded models only, as `E` does.
    OnlyExcluded,
    /// Click on a column header.
    Header(usize),
    /// Click on a header's ▾: open its dropdown.
    Menu(usize),
    /// Click on entry `n` of the open dropdown or choice list.
    Item(usize),
    /// Click outside the open dropdown or choice list: close it. In compare and recommend, a
    /// click on no model, which only clears the status as any click does.
    Outside,
    /// Click on a status bar hint: press its key.
    Key(KeyCode),
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
    /// Stop `i` of task `t` in recommend: 0 its name and the rest of its block, else its model `i - 1`.
    Recommend(usize, usize),
}

/// `provider/model` as opencode takes it, else pi, when either has the model (`launch_cmd`):
/// the offer you'd pay may be one only another harness reaches. With neither, that offer's.
pub fn model_id(m: &Model, listed: &BTreeMap<String, Vec<String>>) -> String {
    let harness = ["opencode", "pi"].iter().find_map(|h| launch_cmd(m, h, listed)?.pop());
    harness.unwrap_or_else(|| m.price().map_or_else(|| m.key.clone(), |o| format!("{}/{}", o.provider, o.id)))
}

/// The command that starts `harness` on `m`, if the harness has it: opencode and pi take
/// `provider/model` as they listed it (`listed`, pi's `openai-codex/...` for models.dev's `openai/...`),
/// the single-provider CLIs (claude, codex, gemini) the bare model id.
pub fn launch_cmd(m: &Model, harness: &str, listed: &BTreeMap<String, Vec<String>>) -> Option<Vec<String>> {
    let o = m.offer_via(harness).filter(|_| harness != "env")?;
    let id = if matches!(harness, "opencode" | "pi") {
        let id = format!("{}/{}", o.provider, o.id);
        let own = listed.get(harness).and_then(|ids| ids.iter().find(|i| crate::data::canonical(harness, i) == id));
        own.cloned().unwrap_or(id)
    } else {
        o.id.clone()
    };
    Some(vec![harness.into(), "--model".into(), id])
}

/// A task's line as (index into `Data::models`, score), and the models on it only for being
/// favorites (`view::task_line`).
type Front = (Vec<(usize, f64)>, Vec<usize>);

pub struct App {
    pub data: Data,
    /// `vals[i][c]` is `COLS[c]` of `data.models[i]`, computed once per data load: a price is a
    /// search through the offers, too slow to repeat for every model on every key.
    pub vals: Vec<[Option<f64>; COLS.len()]>,
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
    /// Developers picked from the Dev dropdown; empty is any.
    pub dev: Vec<String>,
    /// Harnesses (or "env") picked from the Via dropdown; empty is any.
    pub via: Vec<String>,
    /// Task whose price frontier the table shows, picked in the recommend overlay.
    pub task: Option<&'static Task>,
    /// Cursor in the recommend overlay: the task, and on it 0 for its name, else the model
    /// `task_sel - 1` on its line; past the end of an emptied line it is on the name too.
    pub task_cur: usize,
    pub task_sel: usize,
    /// The stop `task_to` wants when a shorter line gave less; none once the cursor is moved
    /// any other way, which then says where it is wanted.
    task_wanted: Option<usize>,
    pub query: String,
    /// The query matched nothing as typed, so it is matched allowing a typo per word.
    pub typos: bool,
    /// Indices into `data.models`, in display order.
    pub rows: Vec<usize>,
    /// Per task of `TASKS`. Set by `rebuild`, as each is a pass over every model and recommend
    /// draws them all.
    fronts: Vec<Front>,
    pub table: TableState,
    /// How many marks are of models the data has: the ones `M`, `C` and `U` act on, as a
    /// refresh or a source switch can drop a selected model while its mark stays. Set by
    /// `rebuild`, so a frame does not scan every model for it.
    pub marked_shown: usize,
    pub only_marked: bool,
    pub only_fav: bool,
    /// `E`: show excluded models only.
    pub only_excluded: bool,
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
    /// The terminal's background colour, when it told at start (`tui::terminal_bg`): with the
    /// terminal's own colours, what a marked row's faint fill is mixed from.
    pub term_bg: Option<u32>,
}

impl App {
    pub fn new(data: Data, store: Store) -> Self {
        let mut app = App {
            data: Data::default(),
            vals: vec![],
            listed: vec![],
            widths: [0; COLS.len()],
            ext: [None; COLS.len()],
            store,
            all: false,
            any_available: false,
            detail: String::new(),
            col: DEFAULT_SORT.0,
            sort_col: DEFAULT_SORT.0,
            descending: DEFAULT_SORT.1,
            bounds: vec![],
            dev: vec![],
            via: vec![],
            task: None,
            task_cur: 0,
            task_sel: 0,
            task_wanted: None,
            query: String::new(),
            typos: false,
            rows: vec![],
            fronts: vec![],
            table: TableState::default().with_selected(0),
            marked_shown: 0,
            only_marked: false,
            only_fav: false,
            only_excluded: false,
            visual: None,
            picked: vec![],
            view: View::Table,
            input: Input::None,
            scroll: 0,
            compare_sel: 0,
            compare_x: 0,
            spots: vec![],
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
            term_bg: None,
        };
        let start = crate::data::cached();
        app.cache_on = if start > 0.0 { start } else { crate::data::AGENT_CACHED };
        // ponytail: leaked once per App, which the TUI makes once; hints are &'static str.
        app.cache_hint = Box::leak(format!("% {:.0}% cached", app.cache_on * 100.0).into_boxed_str());
        app.set_data(data);
        app
    }

    /// Replace the data and recompute everything derived from it.
    pub fn set_data(&mut self, data: Data) {
        self.vals = data.models.iter().map(|m| COLS.each_ref().map(|c| (c.get)(m))).collect();
        self.listed = data.models.iter().map(Model::listed).collect();
        self.widths = std::array::from_fn(|c| {
            let shown = self.vals.iter().zip(&self.listed).filter_map(|(v, &l)| Some(shown(c, v[c]?, l)));
            shown.map(|s| s.chars().count()).max().unwrap_or(0)
        });
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
        // A refresh that could not ask for the newest release leaves nothing to upgrade to.
        if self.input == Input::Upgrade && self.data.update().is_none() {
            self.input = Input::None;
        }
        self.compare_sel = self.compare_sel.min(self.marked_models().len().saturating_sub(1));
        self.rebuild();
    }

    /// Recompute everything that depends on `data::cached()`: Code/$ and the column values.
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
        let (cur, len, count) = (self.selected(), self.rows.len(), n.unsigned_abs());
        let holds = |r: &usize| hit(self, &self.data.models[self.rows[*r]]);
        let to = if n > 0 {
            (cur + 1..len).filter(&holds).take(count).last().or_else(|| (0..len).find(&holds))
        } else {
            (0..cur).rev().filter(&holds).take(count).last().or_else(|| (0..len).rev().find(&holds))
        };
        match to {
            Some(r) if r != cur => self.select(r),
            _ => {
                let other = if self.current().is_some_and(|m| hit(self, m)) { "other " } else { "" };
                self.refuse(format!("no {other}{what} model is shown"));
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
    /// the one picked on the task's line in recommend, where a task's name has none.
    pub fn current(&self) -> Option<&Model> {
        if self.view == View::Compare {
            let marked = self.marked_models();
            return marked.get(self.compare_sel.min(marked.len().saturating_sub(1))).copied();
        }
        if self.view == View::Recommend {
            let i = self.task_sel.checked_sub(1)?;
            // A task of your own has no line but its model.
            let Some(t) = self.cur_task() else {
                let line = self.custom_line(self.custom_at()?);
                return line.get(i.min(line.len().saturating_sub(1))).copied();
            };
            let front = self.task_frontier(t);
            return front.get(i.min(front.len().saturating_sub(1))).map(|&(m, _)| m);
        }
        if matches!(self.view, View::Detail(_)) {
            return self.data.models.iter().find(|m| m.key == self.detail);
        }
        self.rows.get(self.selected()).map(|&i| &self.data.models[i])
    }

    /// How many stops the open overlay's sideways cursor moves over: compare's models, the
    /// task's name and its models in recommend.
    fn across_len(&self) -> usize {
        match self.view {
            View::Compare => self.marked_shown,
            _ => {
                1 + match (self.cur_task(), self.custom_at()) {
                    (Some(t), _) => self.task_frontier(t).len(),
                    (None, Some(t)) => self.custom_line(t).len(),
                    (None, None) => 0,
                }
            }
        }
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
        if self.view == View::Compare {
            return &mut self.compare_sel;
        }
        self.task_wanted = None;
        &mut self.task_sel
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

    /// Keys of the models a command acts on: in the table the selection, else every marked
    /// model when the current one is marked (as a file manager acts on the selection only from
    /// inside it, so stale marks never widen an action on an unmarked row), else the current one.
    fn targets(&self) -> Vec<String> {
        let cur = self.current().map(|m| m.key.clone());
        if self.view == View::Table {
            if self.selecting() {
                let key = |k: usize| self.data.models[self.rows[k]].key.clone();
                return (0..self.rows.len()).filter(|&k| self.is_selected(k)).map(key).collect();
            }
            if cur.as_ref().is_some_and(|k| self.store.marked.contains(k)) {
                return self.store.marked.clone();
            }
        }
        cur.map(|k| vec![k]).unwrap_or_default()
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

    /// Whether any model is marked, so `M` has something to show: a kept mark of a model the
    /// data no longer has is none.
    pub fn any_marked(&self) -> bool {
        self.marked_shown > 0
    }

    /// Whether the table row shows a ★, so `F` keeps it: `starred` without recommend's cursor,
    /// which the table ignores.
    fn is_fav(&self, key: &str) -> bool {
        self.store.is_favorite(self.task, key)
    }

    /// Whether any model shows a ★ in the table, so `F` has something to show.
    pub fn any_fav(&self) -> bool {
        self.data.models.iter().any(|m| self.is_fav(&m.key))
    }

    /// Whether the open choice list is `f`'s tasks, where space or enter ticks one and the list
    /// stays open.
    pub fn choosing_favs(&self) -> bool {
        matches!(self.input, Input::Choose { kind: Kind::Fav, .. })
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

    /// A slot's entry in `f`'s list for the model `key`, ticked where it is the favorite.
    fn fav_label(&self, key: &str, slot: &str) -> String {
        let name = |k: &str| self.data.models.iter().find(|m| m.key == k).map_or(k.to_string(), |m| m.name.clone());
        // What a task of your own is about, cut where a long one would stretch the list.
        let about = self.store.about(slot).map_or(String::new(), |a| format!("  {}", truncate(a, 48)));
        match self.store.favorite(slot) {
            Some(k) if k == key => format!("✓ {slot}{about}"),
            Some(k) => format!("☐ {slot}  (now {}){about}", name(k)),
            None => format!("☐ {slot}{about}"),
        }
    }

    /// `f`'s list for the model `key`: every task and each of its tiers, your own tasks, and
    /// the entry that names a new one. The key, not the current model, since a tick can move
    /// the rows under the list.
    fn fav_items(&self, key: &str) -> Vec<(String, Effect)> {
        self.store
            .all_slots()
            .map(|s| (self.fav_label(key, &s), Effect::Fav(key.to_string(), s)))
            .chain([(NEW_TASK.to_string(), Effect::NewTask(key.to_string()))])
            .collect()
    }

    /// Models passing every filter but the frontier, ignoring the ones on column `skip` except a
    /// minimum (the Price dropdown only replaces the maximum), so a dropdown can count what each
    /// of its entries would show.
    fn filtered(&self, skip: usize) -> impl Iterator<Item = (usize, &Model)> {
        // A selected model shows even out of reach, so it can be compared.
        self.data.models.iter().enumerate().filter(move |&(i, m)| {
            (self.in_reach(m) || self.store.is_marked(&m.key))
                && hits(
                    &self.query,
                    [&m.name, &m.developer, &m.via.join(", "), self.store.note(&m.key).unwrap_or("")],
                    self.typos,
                )
                .is_some()
                && (!self.only_marked || self.store.is_marked(&m.key))
                && (!self.only_fav || self.is_fav(&m.key))
                && (!self.only_excluded || self.store.is_excluded(&m.key))
                && (skip == 1 || self.dev.is_empty() || self.dev.contains(&m.developer))
                && (skip == VIA
                    || self.via.is_empty()
                    || self.via.iter().any(|h| self.shown_via(m).contains(&h.as_str())))
                && self
                    .bounds
                    .iter()
                    .filter(|b| b.0 != skip || b.1.is_finite())
                    .all(|&(c, lo, hi)| self.val(i, c).is_some_and(|v| v >= lo && v <= hi))
        })
    }

    /// Recompute the visible rows after any filter, sort or data change, keeping the selection.
    pub fn rebuild(&mut self) {
        self.marked_shown = self.data.models.iter().filter(|m| self.store.is_marked(&m.key)).count();
        // Unmarking the last marked model, or a refresh dropping it, leaves M (and unfavoriting
        // the last, F) for every model rather than an empty table.
        self.only_marked &= self.any_marked();
        self.only_fav &= self.any_fav();
        self.only_excluded &= !self.store.excluded.is_empty();
        // The rows move, so the selection follows its models by key and drops the ones filtered out.
        let key_of = |k: usize| self.rows.get(k).and_then(|&i| self.data.models.get(i)).map(|m| m.key.clone());
        let anchor = self.visual.and_then(key_of);
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
        // Before a task keeps only its line: the models every task's line is drawn from.
        self.fronts = TASKS.iter().map(|t| self.front(t, &rows)).collect();
        let ms = &self.data.models;
        if let Some(t) = self.task {
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
            // Blanks last either way, as with numbers.
            rows.sort_by_cached_key(|&i| {
                let t = text(&ms[i]);
                (t.is_empty() != self.descending, t, ms[i].name.to_lowercase())
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
        self.select(sel);
    }

    /// Rebuild after a selection or flag changes: a row that drops out, under `M` or for being
    /// out of reach, leaves the cursor where it was, as selecting never moves it.
    pub fn rebuild_in_place(&mut self) {
        let (at, row) = (self.selected(), self.rows.get(self.selected()).copied());
        self.rebuild();
        if self.rows.get(self.selected()).copied() != row {
            self.select(at);
        }
    }

    /// A background refresh finished.
    pub fn refreshed(&mut self, res: Result<Data, Failure>) {
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
        crate::data::set_source(src);
        self.store.source = src.id().to_string();
        self.report(Ok(format!("benchmarks from {} · B to change", src.label())));
        Some(Effect::Source(src))
    }

    /// After a switch of source: that source's cached data, or none. True when it must be
    /// downloaded, which replaces any refresh under way for the other source. Until then the
    /// table is empty rather than showing the other source's scores under this one's name.
    pub fn switched(&mut self, cached: Option<Data>) -> bool {
        // Off a column this source does not have: the cursor, the sort and any bound on it.
        if hidden(self.col) {
            self.col = DEFAULT_SORT.0;
        }
        if hidden(self.sort_col) {
            (self.sort_col, self.descending) = DEFAULT_SORT;
        }
        self.bounds.retain(|b| !hidden(b.0));
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

    /// Via as the table shows it: the harnesses, or `OUT_OF_REACH` for one you have no access to.
    fn shown_via<'a>(&self, m: &'a Model) -> Vec<&'a str> {
        shown_via(m, self.any_available)
    }

    /// Whether `m` is one to use: accessible, or any with `a` or none available. Only these are
    /// recommended: a selected model out of reach shows, but is never a pick.
    pub fn in_reach(&self, m: &Model) -> bool {
        in_reach(m, self.all, self.any_available)
    }

    /// Whether access to no model was found, so every model shows; not so before the data is in.
    pub fn no_access(&self) -> bool {
        !self.data.models.is_empty() && !self.any_available
    }

    /// Whether `m` is drawn muted: excluded, or one you have no access to.
    pub fn muted(&self, m: &Model) -> bool {
        self.store.is_excluded(&m.key) || !self.accessible(m)
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

    /// `f`'s list of tasks for the model `key`, starting on the task at hand, so f enter toggles it.
    fn ask_fav(&mut self, key: &str) {
        let items = self.fav_items(key);
        let own = if self.view == View::Recommend { self.custom_at() } else { None };
        let at = self.task_at_hand().map(|t| t.name).or(own);
        let sel = items.iter().position(|(_, e)| matches!(e, Effect::Fav(_, s) if Some(s.as_str()) == at));
        self.input = Input::choose("favorite for which tasks?", Kind::Fav, items, sel.unwrap_or(0));
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
        // With the list still open, its boxes follow. Each entry stays: a task of your own is
        // gone once unticked, and would leave the list from under the cursor.
        let mut input = std::mem::replace(&mut self.input, Input::None);
        if let Input::Choose { kind: Kind::Fav, items, .. } = &mut input {
            for (label, effect) in items {
                if let Effect::Fav(_, slot) = effect {
                    *label = self.fav_label(key, slot);
                }
            }
        }
        self.input = input;
        self.rebuild_in_place();
        Some(Effect::Save)
    }

    /// Recommend's cursor back on the task of your own it was `on`, after a change to them: they
    /// are in order of name, so one added, renamed or gone moves the others.
    fn back_on(&mut self, on: Option<String>) {
        if let Some(at) = on.and_then(|t| self.store.custom_tasks().iter().position(|x| *x == t)) {
            self.task_cur = TASKS.len() + at;
        }
    }

    /// `f`'s open list made again for the model `key`, with the cursor on the entry of `slot`,
    /// where `edit` goes on writing.
    fn relist(&mut self, key: &str, slot: &str, edit: Option<Edit>) {
        let fresh = self.fav_items(key);
        let at = fresh.iter().position(|(_, e)| matches!(e, Effect::Fav(_, s) if s == slot)).unwrap_or(0);
        if let Input::Choose { kind: Kind::Fav, items, list, .. } = &mut self.input {
            *items = fresh;
            *list = List { sel: at, top: list.top, edit, ..Default::default() };
        }
    }

    /// Whether an entry of `f`'s list is being written, when every key is its text's.
    fn editing(&self) -> bool {
        self.open_list().is_some_and(|l| l.edit.is_some())
    }

    /// Keys while an entry of `f`'s list is written in place.
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

    /// Enter on an entry written in `f`'s list for the model `key`: the task is named, renamed
    /// or described, and the list stays open on it.
    fn edited(&mut self, key: &str, e: Edit) -> Option<Effect> {
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
                self.relist(key, &name, None);
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
                self.relist(key, &task, None);
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
            let t = go(self.task_cur, self.task_count());
            self.task_to(t);
        } else {
            self.scroll = self.scroll.saturating_add_signed(n.clamp(i16::MIN as isize, i16::MAX as isize) as i16);
        }
    }

    /// Recommend's cursor to task `t`. On a model it stays on one: the stop it was last moved
    /// to sideways, or the line's last, so a shorter line on the way does not take it. Past the
    /// end of a line emptied it is on the name, and stays there.
    fn task_to(&mut self, t: usize) {
        let sel = self.task_sel.min(self.across_len() - 1);
        // Not one a line that lost models under the cursor no longer reaches.
        let wanted = self.task_wanted.filter(|_| sel == self.task_sel).unwrap_or(sel);
        self.task_cur = t;
        self.task_sel = wanted.min(self.across_len() - 1);
        self.task_wanted = Some(wanted);
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

    /// Open the dropdown of the column under the cursor, on the entry in effect.
    fn open_menu(&mut self) {
        let ms: Vec<(usize, &Model)> = self.filtered(self.col).collect();
        let (mut items, picked) = if self.col == 1 || self.col == VIA {
            let by_dev = self.col == 1;
            let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
            // Via as the table shows it, so "not available" can be picked too.
            let labels =
                ms.iter().flat_map(|&(_, m)| if by_dev { vec![m.developer.as_str()] } else { self.shown_via(m) });
            for l in labels.filter(|l| !l.is_empty()) {
                *counts.entry(l).or_default() += 1;
            }
            // Developers A-Z whatever their case, so xAI comes before Z.ai; harnesses with the
            // most models first, where the stable sort keeps ties A-Z.
            let mut names: Vec<(String, usize)> = counts.into_iter().map(|(d, n)| (d.to_string(), n)).collect();
            if by_dev {
                names.sort_by_key(|(d, _)| d.to_lowercase());
            } else {
                names.sort_by_key(|&(_, n)| Reverse(n));
            }
            let current = if self.col == 1 { &self.dev } else { &self.via };
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
        items.insert(0, ("any".into(), ms.len()));
        self.input = Input::Menu { col: self.col, items, list: List::at(picked.map_or(0, |i| i + 1)) };
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

    /// Text pasted in the terminal: typed into the search, note or bound being written, and
    /// nothing anywhere else, where its letters would run as keys.
    pub fn paste(&mut self, text: &str) {
        if matches!(self.input, Input::None | Input::Quit | Input::Upgrade) || self.open_list().is_some_and(List::idle)
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
            KeyCode::Home | KeyCode::Char('g') => self.go_to(0),
            KeyCode::End | KeyCode::Char('G') => self.go_to(usize::MAX),
            // ^e is not `e`: only the keys above take ctrl.
            KeyCode::Char(_) if ctrl => {}
            _ if list => return self.input_key(k.code, k.modifiers),
            _ => return self.table_key(k.code, n),
        }
        None
    }

    /// The wheel scrolls whatever `j k` move and sideways moves the column cursor; a click
    /// selects a row, and a double click opens what its cell shows: the details from the name or
    /// the developer, a harness in Via, as `x` does, and from a number the page it comes from;
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
        // An entry being written keeps the wheel still, and a click anywhere leaves it as esc does.
        if self.editing() {
            return match m {
                Mouse::Scroll(_) | Mouse::Cols(_) => None,
                _ => self.input_key(KeyCode::Esc, KeyModifiers::NONE),
            };
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
                Mouse::Item(n) => {
                    let (sel, len) = self.list()?;
                    if n >= len {
                        return None;
                    }
                    *sel = n;
                    self.input_key(KeyCode::Enter, KeyModifiers::NONE)
                }
                Mouse::Cols(_) => None,
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
                    (self.task_cur, self.task_sel, self.task_wanted) = (t, i, None);
                }
                _ => return None,
            }
            // Compare leaves out `space`: it would drop the model from the view.
            return match m {
                Mouse::Open(_) => self.on_key(KeyCode::Enter.into()),
                Mouse::MarkModel(_) if self.view == View::Recommend => self.on_key(KeyCode::Char(' ').into()),
                _ => None,
            };
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
                if !inside {
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
                // Only the clicked row, even on a selected one, where `f` refuses several.
                let key = self.current()?.key.clone();
                self.ask_fav(&key);
                return None;
            }
            // Only the clicked row, even on a mark, where `e` takes every mark.
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
                let cmd = launch_cmd(m, h, &self.data.harness);
                // `env` is an API key, not a harness.
                if cmd.is_none() {
                    self.refuse(format!("{h} cannot be opened on {}", m.name));
                }
                return cmd.map(Effect::Launch);
            }
            Mouse::Cell(n, col) if n < self.rows.len() => {
                self.deselect();
                self.select(n);
                if col < TEXT {
                    return self.table_key(KeyCode::Enter, 1);
                }
                // An empty cell, as Notes, has nothing to open.
                self.val(self.rows[n], col)?;
                let m = self.current()?;
                // The groups of `GROUPS` and where each comes from: the release, prices and context,
                // benchmarks, then speed, which only Artificial Analysis measures.
                let (site, page) = if col < ECI {
                    ("models.dev", m.price_page())
                } else {
                    let src = if col < SPEED { crate::data::source() } else { Source::Aa };
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
            Mouse::OnlyMarked => return self.table_key(KeyCode::Char('M'), 1),
            Mouse::OnlyFav => return self.table_key(KeyCode::Char('F'), 1),
            Mouse::OnlyExcluded => return self.table_key(KeyCode::Char('E'), 1),
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
        // Recommend has one on a model, not on a task's name nor past the end of a line emptied.
        let row = table
            || matches!(self.view, View::Detail(_))
            || (self.view == View::Recommend && self.current().is_some())
            || (self.view == View::Compare && self.marked_shown >= 2);
        // Compare and recommend move a model cursor sideways, wrapping, instead of the column.
        let across = matches!(self.view, View::Compare | View::Recommend);
        match code {
            KeyCode::Char('h') | KeyCode::Left if table => self.col = step_col(self.col, -n),
            KeyCode::Char('l') | KeyCode::Right if table => self.col = step_col(self.col, n),
            KeyCode::Char('h' | 'l') | KeyCode::Left | KeyCode::Right if across => {
                let n = if matches!(code, KeyCode::Char('h') | KeyCode::Left) { -n } else { n };
                let (len, sel) = (self.across_len(), self.across_sel());
                *sel = step((*sel).min(len.saturating_sub(1)), n, len);
            }
            KeyCode::Char('0' | '_') if table => self.col = 0,
            KeyCode::Char('$') if table => self.col = NCOLS - 1,
            KeyCode::Char('0' | '_') if across => *self.across_sel() = 0,
            KeyCode::Char('$') if across => *self.across_sel() = self.across_len().saturating_sub(1),
            KeyCode::Char('w') if table => {
                for _ in 0..n {
                    let end = if self.col == NCOLS - 1 { 0 } else { NCOLS - 1 };
                    self.col = GROUPS.into_iter().find(|&g| g > self.col && !hidden(g)).unwrap_or(end);
                }
            }
            KeyCode::Char('b') if table => {
                for _ in 0..n {
                    self.col = GROUPS.into_iter().rev().find(|&g| g < self.col && !hidden(g)).unwrap_or(VIA);
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
            KeyCode::Char('d') if table && has_menu(self.col) => self.open_menu(),
            KeyCode::Char('d') if table => self.refuse("d opens a dropdown on the Dev, Price and Via columns"),
            KeyCode::Char('M') if table && !self.only_marked && !self.any_marked() => self.refuse(NO_SELECTED),
            KeyCode::Char('M') if table => {
                self.only_marked = !self.only_marked;
                self.rebuild();
            }
            KeyCode::Char('F') if table && !self.only_fav && !self.any_fav() => {
                self.refuse("no favorites: f favorites the one under the cursor");
            }
            KeyCode::Char('F') if table => {
                self.only_fav = !self.only_fav;
                self.rebuild();
            }
            KeyCode::Char('E') if table && !self.only_excluded && self.store.excluded.is_empty() => {
                self.refuse("no excluded models: e excludes the one under the cursor");
            }
            KeyCode::Char('E') if table => {
                self.only_excluded = !self.only_excluded;
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
            KeyCode::Char('U') if table && !self.any_marked() => self.refuse(NO_SELECTED),
            // A mark of a model the data no longer has goes too, else nothing in the TUI clears it.
            KeyCode::Char('U') if table => {
                let n = self.marked_shown;
                let gone = std::mem::take(&mut self.store.marked).len() - n;
                self.rebuild_in_place();
                self.status = match gone {
                    0 => format!("deselected {n}"),
                    _ => format!("deselected {n}, and {gone} no longer listed"),
                };
                return Some(Effect::Save);
            }
            KeyCode::Char('c') if table => {
                self.query.clear();
                self.bounds.clear();
                self.dev.clear();
                self.via.clear();
                // The task set the sort; back to the default.
                if self.task.take().is_some() {
                    (self.sort_col, self.descending) = DEFAULT_SORT;
                }
                self.only_marked = false;
                self.only_fav = false;
                self.only_excluded = false;
                self.rebuild();
            }
            KeyCode::Char('q') => self.input = Input::Quit,
            KeyCode::Char('u') if self.data.update().is_some() => self.input = Input::Upgrade,
            KeyCode::Char('u') if self.data.latest.is_empty() => {
                self.refuse("the newest version is not known: r asks again");
            }
            KeyCode::Char('u') => {
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
                } else if self.only_marked {
                    // Back out of M to every model.
                    self.only_marked = false;
                    self.rebuild();
                } else if self.only_fav {
                    self.only_fav = false;
                    self.rebuild();
                } else if self.only_excluded {
                    self.only_excluded = false;
                    self.rebuild();
                } else if self.task.take().is_some() {
                    // Back to recommend, where enter picked the task.
                    (self.sort_col, self.descending) = DEFAULT_SORT;
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
                (self.task_sel, self.task_wanted) = (0, None);
            }
            KeyCode::Char('e' | 'f' | 'n' | 'o' | 'x' | 'y' | 'Y' | ' ') if self.view == View::Recommend && !row => {
                self.refuse("the cursor is on a task: l picks a model");
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
                    let what = if self.selecting() { "highlighted" } else { "selected" };
                    self.refuse(format!("a task has one favorite: f takes one model, {n} are {what}"));
                }
            }
            KeyCode::Char('v') if table => {
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
                let items: Vec<_> = m
                    .via
                    .iter()
                    .filter_map(|h| launch_cmd(m, h, &self.data.harness))
                    .map(|c| (c.join(" "), Effect::Launch(c)))
                    .collect();
                if items.is_empty() {
                    self.refuse(format!("no harness has {}; Via shows where you have access", m.name));
                } else {
                    self.input = Input::choose("open in which harness?", Kind::Launch, items, 0);
                }
            }
            KeyCode::Char('Y') if row => return Some(Effect::Copy(model_id(self.current()?, &self.data.harness))),
            KeyCode::Char('y') if row => return Some(Effect::Copy(self.current()?.name.clone())),
            KeyCode::Char(' ') if table && self.selecting() => {
                return self.flag(Store::is_marked, Store::toggle_marked, ["selected", "deselected"]);
            }
            KeyCode::Char(' ') if row && self.view != View::Compare => {
                self.toggle_mark();
                return Some(Effect::Save);
            }
            KeyCode::Char('C') if self.view == View::Compare => self.view = View::Table,
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
            KeyCode::Enter if self.view == View::Recommend && !row => {
                let Some(t) = self.cur_task() else {
                    // A task of your own has no line to rank: the table, on its cheapest model.
                    let line = self.custom_at().map_or(vec![], |t| self.custom_line(t));
                    let first = line.first().map(|m| (m.key.clone(), m.name.clone()));
                    let key = first.as_ref().map(|(k, _)| k.clone());
                    // A built-in task picked before would keep the table to its line, as esc undoes.
                    if self.task.take().is_some() {
                        (self.sort_col, self.descending) = DEFAULT_SORT;
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
                // On the frontier the priciest is the best: each row down is cheaper and scores lower.
                (self.sort_col, self.descending) = (PRICE, true);
                self.view = View::Table;
                self.rebuild();
                self.select(0);
                // An unscored favorite is on the line but neither best nor cheapest, and a
                // scored one may cost more than the best.
                let front: Vec<_> = self.task_frontier(t).into_iter().filter(|(_, s)| !s.is_nan()).collect();
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
            KeyCode::Char('/') if table || self.overlay_search() => {
                self.input = Input::Search { cur: 0, was: std::mem::take(self.search_target()) };
                if table {
                    self.rebuild();
                }
            }
            // With access to none every model shows already.
            KeyCode::Char('a') if table && self.no_access() => self.refuse(NO_ACCESS),
            KeyCode::Char('a') if table => {
                self.all = !self.all;
                self.rebuild();
            }
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
                        None => self.report(Err(format!("{text} is not a value for {}", col_name(col)))),
                    }
                    self.rebuild();
                }
                _ => {}
            },
            Input::Menu { col, items, list } => {
                let rows = menu_rows(items, &list.query);
                let at = rows.get(list.sel).copied();
                match code {
                    // Enter does what space does; while searching, space is typed. On Price it
                    // picks the level, or drops it when it is the picked one; on Dev and Via it
                    // adds or drops the entry, as space marks a model, and on "any" drops all.
                    // The dropdown stays open.
                    KeyCode::Enter | KeyCode::Char(' ') if code == KeyCode::Enter || !list.typing => {
                        let i = at?;
                        if *col == PRICE {
                            self.set_price_level(if self.price_level() == i.checked_sub(1) { 0 } else { i });
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
                    // Space ticks a task in f's list and keeps it open, as in the Dev and Via
                    // dropdowns, and so does enter. While searching, space is typed.
                    KeyCode::Char(' ') | KeyCode::Enter
                        if *kind == Kind::Fav && (code == KeyCode::Enter || !list.typing) =>
                    {
                        match items.get(at?) {
                            Some((_, Effect::Fav(key, slot))) => {
                                let (key, slot) = (key.clone(), slot.clone());
                                return self.fav(&key, &slot);
                            }
                            // The name of a new task is written right there, the list open.
                            Some((_, Effect::NewTask(_))) => list.write(at?, Edit::new(What::New, String::new())),
                            _ => {}
                        }
                    }
                    // `r` renames the task of your own under the cursor and `a` writes what it is
                    // about, both on its entry, not on a tier's; a built-in one keeps both.
                    KeyCode::Char(c @ ('r' | 'a')) if *kind == Kind::Fav && !list.typing => {
                        if let Some((_, Effect::Fav(_, slot))) = items.get(at?)
                            && self.store.custom_tasks().contains(&slot.as_str())
                        {
                            let task = slot.clone();
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
                            effect => return Some(effect),
                        }
                    }
                    _ if list.key(code, mods, at, |q| choice_rows(items, q).len(), false) => {}
                    // Closing the first start's choice picks the default.
                    KeyCode::Esc if self.first_start && *kind == Kind::Source => {
                        self.input = Input::None;
                        return self.switch(Source::default());
                    }
                    // The key that opens the theme list also closes it.
                    KeyCode::Char('t') if *kind == Kind::Theme => self.input = Input::None,
                    KeyCode::Esc => self.input = Input::None,
                    _ => {}
                }
            }
            Input::Quit => {
                if code == KeyCode::Char('q') {
                    return Some(Effect::Quit);
                }
                self.input = Input::None;
            }
            Input::Upgrade => {
                self.input = Input::None;
                if code == KeyCode::Char('u') {
                    return Some(Effect::Upgrade);
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
    use crate::data::Offer;
    use crate::fit;
    use crate::view::frontier;

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
        if let Some(c) = coding {
            m.fit.insert("coding".into(), c);
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
        assert!(col_about(PRICE).contains(" 0% of the input cached") && a.status.contains("one-off"));
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
        // f's tasks are searched by name, so a tick, which rewrites the label, keeps the entry.
        press(&mut a, "f/now");
        assert!(matches!(&a.input, Input::Choose { items, list, .. } if choice_rows(items, &list.query).is_empty()));
        code(&mut a, KeyCode::Esc);
        press(&mut a, "/coding:l");
        let ticked = |a: &App| match &a.input {
            Input::Choose { items, list, .. } => {
                let rows = choice_rows(items, &list.query);
                (rows.len(), items[rows[list.sel]].0.starts_with('✓'))
            }
            _ => (0, false),
        };
        assert_eq!(ticked(&a), (1, false));
        code(&mut a, KeyCode::Enter);
        assert_eq!(ticked(&a), (1, true), "ticked, and still under the cursor");
        code(&mut a, KeyCode::Enter);
        assert_eq!((ticked(&a), a.store.favorite("coding:low")), ((1, false), None), "enter again unticks it");
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
        a.refreshed(Err(Failure::BadKey));
        assert!(matches!(a.input, Input::Key { wrong: true, .. }));
        // A new key while the old one's refresh is under way starts another.
        crate::data::set_source(Source::Aa);
        a.refreshing = true;
        press(&mut a, "new");
        assert_eq!(code(&mut a, KeyCode::Enter), Some(Effect::Refresh));
        assert_eq!(crate::data::aa_key().as_deref(), Some("new"));
        crate::data::TEST_KEYS.with(|k| k.borrow_mut()[0] = Some("env".into()));
        let mut e = app();
        e.refreshed(Err(Failure::BadKey));
        assert!(e.input == Input::None && e.status.contains(crate::data::AA_KEY_ENV), "fixed where it is set");
        let mut b = app();
        b.refreshed(Err("offline".into()));
        assert_eq!(b.input, Input::None, "any other failure is only reported");
        let mut c = app();
        c.input = Input::Note { key: "gpt55".into(), text: "half".into(), cur: 4 };
        c.refreshed(Err(Failure::NoKey));
        assert!(matches!(c.input, Input::Note { .. }), "a note being typed is kept");
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
    fn dropdowns_pick_a_developer_and_a_price_level() {
        let mut a = app();
        a.col = ECI;
        press(&mut a, "d");
        assert_eq!(
            (&a.input, a.status.as_str()),
            (&Input::None, "d opens a dropdown on the Dev, Price and Via columns")
        );
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
        assert_eq!(col_name(a.col), "Via");
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
        let gpt = a.rows[0];
        a.data.models[gpt].via.push("env".into());
        assert_eq!(a.mouse(Mouse::Harness(0, 2)), None);
        assert_eq!(a.status, "env cannot be opened on gpt55", "env is no harness, and says so");
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
        assert!(col_about(0).contains("Via"));
        assert!(col_about(1).contains("trained"));
        assert!(COLS.iter().all(|c| !c.about().is_empty()));
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
        press(&mut a, &format!("{NCOLS}l"));
        assert_eq!(a.col, ECI, "Artificial Analysis shows them all");
    }

    #[test]
    fn column_jumps() {
        let mut a = app();
        a.col = 0;
        let mut cols = vec![];
        for _ in 0..4 {
            press(&mut a, "w");
            cols.push(a.col);
        }
        press(&mut a, "w");
        assert_eq!((cols, a.col), (vec![PRICE, ECI, VIA, NOTES], 0), "w wraps from the last column");
        press(&mut a, "b");
        assert_eq!(a.col, VIA, "b wraps from the first");
        a.col = ECI + 1;
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
        press(&mut a, "Rjj");
        assert_eq!(a.task_cur, 2);
        press(&mut a, "G");
        assert_eq!(a.task_cur, TASKS.len() - 1);
        press(&mut a, "gg");
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
        assert_eq!((a.sort_col, a.descending), DEFAULT_SORT, "c restores the default sort");
    }

    #[test]
    fn a_task_starts_at_the_low_tier() {
        let mut a = app();
        let mut data = std::mem::take(&mut a.data);
        data.models.push(model("weak", true, Some(30.0), 0.01));
        data.models.push(model("edge", true, Some(49.6), 0.05));
        a.set_data(data);
        let front: Vec<&str> =
            a.task_frontier(fit::task("coding").unwrap()).iter().map(|(m, _)| m.key.as_str()).collect();
        assert_eq!(front, ["edge", "mini", "gpt55"], "cheap alone is no recommendation; 49.6 shows as 50");
    }

    #[test]
    fn a_task_keeps_the_best_of_each_price_level() {
        let mut a = app();
        let mut data = std::mem::take(&mut a.data);
        data.models.push(model("flash", true, Some(70.0), 1.5));
        a.set_data(data);
        let front: Vec<&str> =
            a.task_frontier(fit::task("coding").unwrap()).iter().map(|(m, _)| m.key.as_str()).collect();
        assert_eq!(front, ["flash", "gpt55"], "flash beats mini at about the same price");
        press(&mut a, "R2gg");
        code(&mut a, KeyCode::Enter);
        assert_eq!(keys(&a), ["gpt55", "flash"], "enter shows the same line as the panel");
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
        press(&mut a, "Rj");
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
        llama.fit.insert("coding".into(), 55.0);
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
        a.refreshed(Ok(data));
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
        // cheaper and better than gptlite, yet gptlite stays.
        let mut data = std::mem::take(&mut a.data);
        let mut lite = model("gptlite", true, Some(55.0), 2.0);
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
        assert_eq!(crate::view::pick(&[(m, s)], "low").map(|e| e.0.key.as_str()), Some("opus5"), "the only entry");
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
        // The last entry of f's list takes the name of a new task right there, the list open.
        press(&mut a, "fG");
        assert_eq!(code(&mut a, KeyCode::Enter), None);
        let edit =
            |a: &App| a.open_list().and_then(|l| l.edit.as_ref().map(|e| (format!("{:?}", e.what), e.text.clone())));
        let under = |a: &App| match &a.input {
            Input::Choose { items, list, .. } => items[list.sel].0.clone(),
            _ => String::new(),
        };
        assert_eq!(edit(&a), Some(("New".into(), String::new())));
        a.paste("Tool Dispatch:");
        assert_eq!(code(&mut a, KeyCode::Enter), Some(Effect::Save));
        assert_eq!(a.store.favorite("tool-dispatch"), Some(on.as_str()), "no : in it");
        // The list is on the new task, where what it is about is written next; left empty,
        // nothing is saved.
        assert_eq!(
            (under(&a).as_str(), edit(&a)),
            ("✓ tool-dispatch", Some((r#"About("tool-dispatch")"#.into(), String::new())))
        );
        assert_eq!((code(&mut a, KeyCode::Enter), edit(&a), a.store.about("tool-dispatch")), (None, None, None));
        assert!(a.choosing_favs(), "the list is open still");
        // a on it writes it, starting from what it says, and its entry shows it.
        press(&mut a, "a");
        a.paste("routing tool calls");
        assert_eq!(code(&mut a, KeyCode::Enter), Some(Effect::Save));
        assert_eq!(
            (a.store.about("tool-dispatch"), under(&a).as_str()),
            (Some("routing tool calls"), "✓ tool-dispatch  routing tool calls")
        );
        // While writing, the keys that move are text, the wheel is still and esc leaves the entry alone.
        press(&mut a, "ajk");
        a.mouse(Mouse::Scroll(-3));
        assert_eq!(edit(&a), Some((r#"About("tool-dispatch")"#.into(), "routing tool callsjk".into())));
        code(&mut a, KeyCode::Esc);
        assert_eq!(
            (edit(&a), a.choosing_favs(), a.store.about("tool-dispatch")),
            (None, true, Some("routing tool calls"))
        );
        // Unticked it is gone, but keeps its entry while the list is open.
        press(&mut a, " ");
        assert_eq!((a.store.custom_tasks().len(), under(&a).as_str()), (0, "☐ tool-dispatch  routing tool calls"));
        press(&mut a, " ");
        assert_eq!(under(&a), "✓ tool-dispatch  routing tool calls");
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
        assert_eq!((a.store.custom_tasks(), under(&a).as_str()), (vec!["dispatch"], "✓ dispatch  routing tool calls"));
        assert_eq!(a.status, "tool-dispatch renamed to dispatch");
        press(&mut a, "ggra");
        assert_eq!(edit(&a), None, "overall is built in");
        code(&mut a, KeyCode::Esc);
        a.store.rename_task("dispatch", "tool-dispatch").unwrap();
        // Recommend lists it after the built-in tasks, the cursor on its name, its model after
        // it; enter on the name goes to the table, on that model.
        press(&mut a, "RG");
        assert_eq!((a.task_cur, a.custom_at(), a.task_at_hand().is_none()), (TASKS.len(), Some("tool-dispatch"), true));
        assert!(a.current().is_none(), "on the name");
        press(&mut a, "l");
        assert_eq!(a.current().map(|m| m.key.clone()), Some(on.clone()), "one model on its line");
        press(&mut a, "h");
        code(&mut a, KeyCode::Enter);
        assert_eq!((&a.view, a.current().unwrap().key.as_str()), (&View::Table, on.as_str()));
        assert!(a.status.ends_with("your model for tool-dispatch"), "{}", a.status);
        // f on its block starts on it; without its model the task is gone, and the cursor is on
        // the task before.
        press(&mut a, "RGf");
        assert!(a.failed && a.input == Input::None, "f on a task's name has no model to favorite");
        press(&mut a, "lf");
        assert_eq!(under(&a), "✓ tool-dispatch  routing tool calls");
        press(&mut a, " ");
        code(&mut a, KeyCode::Esc);
        assert_eq!((a.task_cur, a.store.favorite("tool-dispatch")), (TASKS.len() - 1, None));
        // An empty name names no task.
        press(&mut a, "2gglfG");
        code(&mut a, KeyCode::Enter);
        assert_eq!(edit(&a), Some(("New".into(), String::new())));
        assert_eq!((code(&mut a, KeyCode::Enter), edit(&a), a.store.custom_tasks().len()), (None, None, 0));
        code(&mut a, KeyCode::Esc);
        // A tier of it takes a model of its own, as a built-in task's: its entries follow the
        // task's in the list, and both models are on its line, cheapest first.
        a.store.toggle_favorite("tool-dispatch", &on);
        a.store.toggle_favorite("tool-dispatch:low", "mini");
        a.rebuild();
        press(&mut a, "G0l");
        assert_eq!((&a.view, a.current().map(|m| m.key.as_str())), (&View::Recommend, Some("mini")));
        press(&mut a, "l");
        assert_eq!(a.current().map(|m| m.key.clone()), Some(on.clone()), "h l move along its line");
        press(&mut a, "fj");
        assert_eq!(under(&a), "☐ tool-dispatch:low  (now mini)");
        press(&mut a, "ra");
        assert_eq!(edit(&a), None, "a tier has the task's name and about");
        press(&mut a, "3j");
        assert_eq!(under(&a), "+ new task", "after its three tiers");
        // A task named before it in the order leaves recommend's cursor on its own.
        code(&mut a, KeyCode::Enter);
        a.paste("api");
        code(&mut a, KeyCode::Enter);
        assert_eq!((a.store.custom_tasks(), a.custom_at()), (vec!["api", "tool-dispatch"], Some("tool-dispatch")));
    }

    #[test]
    fn f_favorites_a_model_for_the_task_at_hand() {
        let mut a = app();
        // No task in context: f asks which, listing every task, each followed by its tiers.
        assert_eq!(press(&mut a, "f"), None);
        assert!(matches!(&a.input, Input::Choose { items, .. } if items.len() == TASKS.len() * 4 + 1));
        press(&mut a, "4j");
        assert_eq!(code(&mut a, KeyCode::Enter), Some(Effect::Save));
        assert!(
            matches!(&a.input, Input::Choose { items, .. } if items[4].0 == "✓ coding"),
            "enter ticks as space does"
        );
        code(&mut a, KeyCode::Esc);
        assert_eq!(a.store.favorite("coding"), Some("gpt55"), "the second task is coding");
        assert!(a.starred("gpt55") && !a.starred("mini"), "★ with no task: favorite to any");
        // In recommend, f starts on the task under the cursor, so f enter toggles it.
        press(&mut a, "Rjl");
        assert_eq!(a.current().unwrap().key, "mini");
        assert_eq!(press(&mut a, "f"), None);
        assert!(
            matches!(&a.input, Input::Choose { list: List { sel: 4, .. }, items, .. } if items[4].0 == "☐ coding  (now gpt55)")
        );
        assert_eq!(code(&mut a, KeyCode::Enter), Some(Effect::Save));
        assert_eq!(a.store.favorite("coding"), Some("mini"));
        assert!(a.status.starts_with("★ mini"));
        code(&mut a, KeyCode::Esc);
        // Space ticks and keeps the list open, its boxes following.
        press(&mut a, "f");
        assert_eq!(press(&mut a, " "), Some(Effect::Save));
        assert_eq!(a.store.favorite("coding"), None, "again unfavorites");
        assert!(matches!(&a.input, Input::Choose { items, .. } if items[4].0 == "☐ coding"));
        // The entry below is coding's low tier: --tier low picks mini, the others the computed one.
        press(&mut a, "j ");
        assert_eq!((a.store.favorite("coding:low"), a.store.favorite("coding:mid")), (Some("mini"), None));
        assert!(a.status.ends_with("coding:low"));
        press(&mut a, " ");
        press(&mut a, "5k ");
        assert!(matches!(&a.input, Input::Choose { items, .. } if items[0].0 == "✓ overall"));
        press(&mut a, " ");
        code(&mut a, KeyCode::Esc);
        assert_eq!((&a.input, a.store.favorite_for("mini").len()), (&Input::None, 0));
        // A favorite model joins the task's line even off the frontier: opus5 has no coding score
        // in this fixture, so llama4 (40, below the floor) stands in once it is shown with a.
        press(&mut a, "R");
        press(&mut a, "a");
        a.store.toggle_favorite("coding", "llama4");
        a.rebuild();
        press(&mut a, "R");
        assert_eq!(a.task_cur, 1, "still on coding");
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
        assert!(a.current().is_none(), "the cursor starts on the first task's name");
        press(&mut a, "j");
        let front: Vec<String> =
            a.task_frontier(fit::task("coding").unwrap()).iter().map(|(m, _)| m.key.clone()).collect();
        assert_eq!(front, ["mini", "gpt55"], "cheapest first, best last");
        assert!(a.current().is_none(), "j k land on the name");
        assert!(press(&mut a, "o").is_none() && a.failed, "where the row keys have no model");
        press(&mut a, "l");
        assert_eq!(a.current().unwrap().key, "mini", "l steps onto the cheapest");
        press(&mut a, "l");
        assert_eq!(a.current().unwrap().key, "gpt55");
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
        // j k stay on a model: the same stop of the next line, or its last, which does not
        // become the stop wanted.
        a.store.toggle_favorite(TASKS[2].name, "mini");
        a.rebuild();
        press(&mut a, "j");
        assert_eq!((a.task_cur, a.current().unwrap().key.as_str()), (2, "mini"), "the last of a shorter line");
        press(&mut a, "k");
        assert_eq!((a.task_cur, a.current().unwrap().key.as_str()), (1, "gpt55"), "the stop it left going back");
        press(&mut a, "j0lk");
        assert_eq!((a.task_cur, a.current().unwrap().key.as_str()), (1, "mini"), "unless moved sideways since");
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
        assert_eq!(press(&mut a, "e"), Some(Effect::Save), "e excludes it, so it leaves the line");
        assert_eq!(a.current().unwrap().key, "mini", "the cursor lands on what is left");
        assert_eq!(press(&mut a, "e"), Some(Effect::Save));
        assert!(a.current().is_none(), "an emptied line leaves the cursor on the name");
        assert!(press(&mut a, "o").is_none() && a.failed, "where the row keys say so");
        press(&mut a, "j");
        assert_eq!((a.task_cur, a.task_sel), (2, 0), "from the name of a line emptied, j k land on the name");
        press(&mut a, "k$RR");
        assert_eq!(a.task_sel, 0, "so does reopening");
    }

    #[test]
    fn the_table_cursor_follows_the_model_picked_in_an_overlay() {
        let mut a = app();
        let row = |a: &App, key: &str| a.rows.iter().position(|&i| a.data.models[i].key == key).unwrap();
        press(&mut a, "Rjl");
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
    fn a_refresh_keeps_the_selection() {
        let mut a = app();
        press(&mut a, "vj");
        let same = Data { models: std::mem::take(&mut a.data.models), ..Data::default() };
        a.refreshed(Ok(same));
        assert_eq!((a.visual_range(), a.selected()), (Some(0..=1), 1), "the range follows its models");
        a.mouse(Mouse::Pick(2));
        let same = Data { models: std::mem::take(&mut a.data.models), ..Data::default() };
        a.refreshed(Ok(same));
        assert_eq!(a.picked, [0, 1, 2], "so do picked rows");
    }

    #[test]
    fn mark_only_marked_and_compare() {
        let mut a = app();
        assert_eq!(press(&mut a, "C"), None);
        assert_eq!(a.view, View::Compare, "opens to say models must be marked first");
        code(&mut a, KeyCode::Esc);
        press(&mut a, " G ");
        assert_eq!(a.store.marked, vec!["gpt55", "opus5"]);
        press(&mut a, "M");
        assert_eq!(keys(&a), ["gpt55", "opus5"]);
        press(&mut a, "M");
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
        press(&mut a, "Mc");
        assert_eq!((a.marked_models().len(), a.only_marked, a.rows.len()), (2, false, 3), "c keeps them and leaves M");
        press(&mut a, "ggj M");
        assert_eq!(keys(&a), ["gpt55", "mini", "opus5"]);
        press(&mut a, "ggvG ");
        assert_eq!(
            (a.store.marked.len(), a.only_marked, a.rows.len()),
            (0, false, 3),
            "unmarking every marked model leaves M for every model"
        );
        press(&mut a, "U");
        assert_eq!(a.status, NO_SELECTED);
        press(&mut a, "gg j M");
        assert_eq!(press(&mut a, "U"), Some(Effect::Save), "U saves");
        assert_eq!((a.store.marked.len(), a.only_marked, a.rows.len()), (0, false, 3), "U unmarks all and leaves M");
    }

    #[test]
    fn brackets_jump_between_selected_models() {
        let mut a = app();
        a.store.toggle_marked("gone");
        a.rebuild();
        for k in ["]", "M", "U"] {
            press(&mut a, k);
            assert_eq!(a.status, NO_SELECTED, "{k}: a model gone is none");
        }
        a.store.toggle_marked("opus5");
        a.rebuild();
        assert_eq!(press(&mut a, "U"), Some(Effect::Save), "U clears the gone model's mark too");
        assert_eq!((a.store.marked.len(), a.status.as_str()), (0, "deselected 1, and 1 no longer listed"));
        a.store.toggle_marked("opus5");
        a.store.toggle_marked("gpt55");
        a.rebuild();
        press(&mut a, "M");
        a.data.models.retain(|m| m.key != "gpt55");
        a.rebuild();
        assert!(a.only_marked, "a refresh that drops one selected model keeps M");
        press(&mut a, "gg ");
        assert!(!a.only_marked, "unmarking the last one shown leaves M though a gone one's mark stays");
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
    }

    #[test]
    fn esc_backs_out_of_views_and_toggles_close_what_they_open() {
        let mut a = app();
        press(&mut a, "M");
        assert_eq!((a.only_marked, a.status.as_str()), (false, NO_SELECTED));
        press(&mut a, " M/x");
        code(&mut a, KeyCode::Enter);
        code(&mut a, KeyCode::Esc);
        assert_eq!((a.query.as_str(), a.only_marked), ("", true), "esc clears the search first");
        code(&mut a, KeyCode::Esc);
        assert_eq!((a.only_marked, a.rows.len()), (false, 3), "then leaves M");
        assert_eq!(a.store.marked, ["gpt55"], "without touching the marks");
        press(&mut a, "F");
        assert_eq!((a.only_fav, a.status.as_str()), (false, "no favorites: f favorites the one under the cursor"));
        a.store.toggle_favorite("coding", "opus5");
        press(&mut a, "F");
        assert_eq!(keys(&a), ["opus5"], "F shows the favorites only");
        press(&mut a, "Mc");
        assert_eq!((a.only_marked, a.only_fav, a.rows.len()), (false, false, 3), "c leaves M and F");
        press(&mut a, "MF");
        assert!(a.rows.is_empty(), "M and F together: marked favorites");
        press(&mut a, "MF");
        a.mouse(Mouse::OnlyMarked);
        a.mouse(Mouse::OnlyFav);
        press(&mut a, "G");
        a.mouse(Mouse::Top);
        assert_eq!(a.selected(), 0, "the # header goes to the first row");
        assert!((a.only_marked, a.only_fav) == (true, true), "the ✓ and ★ headers do what M and F do");
        code(&mut a, KeyCode::Esc);
        assert_eq!((a.only_marked, a.only_fav), (false, true), "esc leaves M first");
        code(&mut a, KeyCode::Esc);
        assert_eq!((a.only_fav, a.rows.len()), (false, 3), "then F");
        a.store.toggle_favorite("coding", "opus5");
        press(&mut a, "Rj");
        code(&mut a, KeyCode::Enter);
        assert!(a.task.is_some());
        code(&mut a, KeyCode::Esc);
        assert!(a.task.is_none(), "esc drops the task");
        assert_eq!((&a.view, a.task_cur), (&View::Recommend, 1), "and goes back to recommend");
        assert_eq!((a.sort_col, a.descending), DEFAULT_SORT, "and the sort it started with");
        code(&mut a, KeyCode::Esc);
        press(&mut a, "CC");
        assert_eq!(a.view, View::Table, "C closes compare, as ? and R close theirs");
        press(&mut a, "c");
        a.mouse(Mouse::OnlyExcluded);
        assert_eq!(
            (a.only_excluded, a.status.as_str()),
            (false, "no excluded models: e excludes the one under the cursor")
        );
        a.store.toggle_excluded("mini");
        a.mouse(Mouse::OnlyExcluded);
        assert_eq!(keys(&a), ["mini"], "the ✗ header, as E, shows the excluded only");
        code(&mut a, KeyCode::Esc);
        assert_eq!((a.only_excluded, a.rows.len()), (false, 3), "esc leaves E");
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
        assert!(
            a.store.is_excluded("gpt55") && a.store.is_excluded("mini") && !a.store.is_excluded("opus5"),
            "e on a mark: all marks"
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
        assert_eq!(a.mouse(Mouse::Item(4)), Some(Effect::Save));
        assert!(a.choosing_favs(), "a click ticks a task and keeps the list open");
        assert_eq!(a.store.favorite("coding"), Some("opus5"));
        code(&mut a, KeyCode::Esc);
        // A task has one favorite: with several selected or highlighted, f says so and asks nothing.
        a.mouse(Mouse::Box(1));
        a.mouse(Mouse::Box(2));
        assert_eq!((press(&mut a, "f"), a.choosing_favs()), (None, false));
        assert_eq!(a.status, "a task has one favorite: f takes one model, 2 are selected");
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
        assert!(a.store.is_excluded(&k) && !a.store.is_excluded(&key(&a, 0)), "on a mark, not every mark as e does");
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
        a.refreshed(Ok(one));
        assert_eq!((a.compare_sel, a.rows.len(), a.selected()), (0, 1, 0), "clamped to the models left");
        assert_eq!(a.current().map(|m| m.key.as_str()), Some("gpt55"));
    }

    #[test]
    fn refresh_once_at_a_time() {
        let mut a = app();
        assert_eq!(press(&mut a, "r"), Some(Effect::Refresh));
        assert!(a.refreshing && a.status.is_empty(), "only the ⟳ indicator says so");
        assert_eq!(press(&mut a, "r"), None);
        a.refreshed(Err("offline".into()));
        assert!(a.failed && a.refresh_failed, "a failure is shown as one");
        assert_eq!(a.status, "refresh failed: offline");
        assert_eq!(press(&mut a, "r"), Some(Effect::Refresh));
        assert!(!a.failed && a.status.is_empty() && a.refresh_failed, "the frame keeps saying it failed");
        a.refreshed(Ok(Data::default()));
        assert!(!a.refresh_failed, "until one succeeds");
        assert_eq!(a.status, "data refreshed");
        // Nor after a refusal a key has since cleared.
        press(&mut a, "U");
        press(&mut a, "j");
        a.status = "benchmarks from Epoch AI · B to change".into();
        a.refreshed(Ok(Data::default()));
        assert!(a.status.ends_with("B to change"), "what a switch said outlasts its download");
        a.report(Err("user.json is not valid".into()));
        a.refreshed(Ok(Data::default()));
        assert_eq!(a.status, "user.json is not valid", "an error still standing is not replaced by good news");
        a.refreshed(Ok(Data { warning: Some("pi did not list its models".into()), ..Default::default() }));
        assert_eq!(a.status, "user.json is not valid; pi did not list its models", "nor by a warning");
        // Why a key did nothing is red too, and no error: the refresh's outcome replaces it.
        press(&mut a, "U");
        assert!(a.failed && a.status.starts_with("no selected models"));
        a.refreshed(Ok(Data::default()));
        assert_eq!((a.failed, a.status.as_str()), (false, "data refreshed"));
        press(&mut a, "U");
        a.refreshed(Ok(Data { warning: Some("pi did not list its models".into()), ..Default::default() }));
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
        a.mouse(Mouse::OnlyMarked);
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
        assert_eq!(a.mouse(Mouse::Cell(2, NOTES)), None, "nor do the notes");
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
        assert_eq!(ctrl(&mut a, 'c'), Some(Effect::Quit), "ctrl-c quits at once");
    }

    #[test]
    fn u_upgrades_only_to_a_newer_release() {
        let mut a = app();
        assert_eq!((press(&mut a, "u"), &a.input), (None, &Input::None), "nothing newer: nothing to ask");
        assert!(a.status.contains("not known"), "{}", a.status);
        a.data.latest = env!("CARGO_PKG_VERSION").into();
        press(&mut a, "u");
        assert!(a.status.contains("no newer version"), "{}", a.status);
        a.data.latest = "99.0.0".into();
        assert_eq!((press(&mut a, "u"), &a.input), (None, &Input::Upgrade), "u asks");
        assert_eq!((press(&mut a, "q"), &a.input), (None, &Input::None), "any other key cancels");
        assert_eq!(press(&mut a, "uu"), Some(Effect::Upgrade));
        assert_eq!(a.input, Input::None, "the TUI stays open when there is no command to run");
        press(&mut a, "u");
        a.set_data(Data::default());
        assert_eq!(a.input, Input::None, "a refresh that lost the release takes the question back");
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
        a.refreshed(Ok(Data { fetched: 0, models, ..Default::default() }));
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
