//! TUI state and key handling. No I/O here: side effects come back to the shell as `Effect`s,
//! so every key is unit-testable.

use crate::data::{Data, Model, Source};
use crate::fit::{TASKS, Task};
use crate::store::{Store, slot, slots};
use crate::view::{
    LEVELS, THEMES, ctx, hits, in_reach, level_label, money, recommended, score, shown_via, task_frontier, task_score,
};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::widgets::TableState;
use std::cmp::{Ordering, Reverse};
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
    pub get: fn(&Model) -> Option<f64>,
    pub show: fn(f64) -> String,
    /// Only Artificial Analysis measures it: hidden with any other source (`hidden`).
    pub aa_only: bool,
}

impl Col {
    /// Its name and meaning; the index column's are the benchmark source's, and so are the
    /// task columns' meanings, each a single benchmark with Artificial Analysis.
    fn text(&self) -> (&'static str, &'static str) {
        let about = match (self.id, crate::data::source()) {
            ("eci", s) => return s.index(),
            ("coding", Source::Aa) => "Artificial Analysis Coding Index",
            ("agentic", Source::Aa) => "Terminal-Bench Hard score (0-100)",
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
    Col { name, id, about, lower_better: false, get, show: |v| score(Some(v)), aa_only: false }
}

fn positive(x: f64) -> Option<f64> {
    (x > 0.0).then_some(x)
}

/// Prices from the offer you'd pay, then the source's overall index, the task scores and
/// Code/$, then speed when Artificial Analysis measures it.
pub const COLS: [Col; 12] = [
    Col {
        lower_better: true,
        show: money,
        ..col("Price", "price", "USD per 1M tokens, 3:1 input:output", |m| m.cost())
    },
    Col {
        lower_better: true,
        show: money,
        ..col("$in", "in", "USD per 1M input tokens, cheapest available provider", |m| Some(m.priced_offer()?.input))
    },
    Col {
        lower_better: true,
        show: money,
        ..col("$cache", "cache", "USD per 1M cached input tokens ($in when the provider has no discount)", |m| {
            m.priced_offer().map(|o| o.cache_read.unwrap_or(o.input))
        })
    },
    Col {
        lower_better: true,
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
    col("Code/$", "value", "coding percentile ÷ Price, as a percentile", |m| m.fit.get("value").copied()),
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

/// Text columns before the numbers: 0 is the model name, 1 its developer. `VIA` follows them.
pub const TEXT: usize = 2;
/// Column index of the blended price, the frontier's sort.
pub const PRICE: usize = TEXT;
/// The sort the table starts with, and that `c` and leaving a task go back to: the source's
/// index (ECI or AAII), best first. The cursor starts on that column too.
const DEFAULT_SORT: (usize, bool) = (ECI, true);
/// Column index of ECI.
pub const ECI: usize = TEXT + 5;
/// Column index of Tok/s.
pub const SPEED: usize = TEXT + 10;
/// First column of each group: names, price and context, benchmarks, speed, your own.
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
        PRICE => format!("{}, {:.0}% of the input cached", COLS[PRICE - TEXT].about, crate::data::cached() * 100.0),
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
            ("B", "benchmarks from Epoch AI or Artificial Analysis"),
        ],
    ),
    (
        "Move",
        &[
            ("j k ↓ ↑", "move; a count repeats, as in 3j"),
            ("h l ← →", "pick a column; in compare and recommend, a model"),
            ("0 _ $ w b", "first / last column; next / previous group"),
            ("gg G 3gg", "top / bottom / row 3"),
            ("( ) ^d ^u", "half a page up / down"),
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
            ("f", "favorite the model for a task, or a tier of one"),
            ("e", "exclude the model"),
            ("U", "deselect every model"),
            ("M F E", "selected / favorite / excluded only; again: every model"),
        ],
    ),
    (
        "Model under the cursor",
        &[
            ("n", "note for the model, agents read it"),
            ("y Y", "copy the model id / name"),
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

#[derive(PartialEq, Debug)]
pub enum View {
    Table,
    Detail,
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
    Note {
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
    /// "any" first. `sel` indexes the entries matching `query`, see `menu_rows`.
    Menu {
        col: usize,
        items: Vec<(String, usize)>,
        sel: usize,
        query: String,
        cur: usize,
        /// Typing into `query`, after `/`.
        typing: bool,
    },
    /// `q` asks before quitting.
    Quit,
    /// A choice of what to do, `sel` under the cursor: `x` on a model several harnesses have
    /// launches one, `o` opens one of the model's pages. Each item is its label and effect.
    Choose {
        title: &'static str,
        items: Vec<(String, Effect)>,
        sel: usize,
        /// Filter on the entries, typed after `/` as in a dropdown; `sel` indexes what is left.
        query: String,
        cur: usize,
        typing: bool,
    },
}

impl Input {
    /// A choice list (`f`, `o`, `x`, `t`) with the cursor on `sel` and nothing searched yet.
    fn choose(title: &'static str, items: Vec<(String, Effect)>, sel: usize) -> Self {
        Self::Choose { title, items, sel, query: String::new(), cur: 0, typing: false }
    }
}

/// Indices of the dropdown entries whose name contains `query`, any case; "any" always stays.
pub fn menu_rows(items: &[(String, usize)], query: &str) -> Vec<usize> {
    let q = query.to_lowercase();
    (0..items.len()).filter(|&i| i == 0 || items[i].0.to_lowercase().contains(&q)).collect()
}

/// Indices of the choice list entries whose label contains `query`, any case; empty when none
/// match, which the overlay says.
pub fn choice_rows(items: &[(String, Effect)], query: &str) -> Vec<usize> {
    let q = query.to_lowercase();
    (0..items.len()).filter(|&i| items[i].0.to_lowercase().contains(&q)).collect()
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
    Save,
    Open(String),
    Copy(String),
    Refresh,
    /// Run this command in a new terminal window.
    Launch(Vec<String>),
    /// Favorite the current model for the task, or for one tier of it; the `f` chooser's items,
    /// applied by `App` itself.
    Fav(String, &'static str, Option<&'static str>),
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
    /// Click outside the open dropdown or choice list: close it.
    Outside,
    /// Click on a status bar hint: press its key.
    Key(KeyCode),
}

/// `provider/model` of the offer you'd pay, as harnesses name it.
pub fn model_id(m: &Model) -> String {
    m.price().map_or_else(|| m.key.clone(), |o| format!("{}/{}", o.provider, o.id))
}

/// The command that starts `harness` on `m`, if the harness has it: opencode and pi take
/// `provider/model` as they listed it (`listed`, pi's `openai-codex/...` for models.dev's `openai/...`),
/// the single-provider CLIs (claude, codex, gemini) the bare model id.
pub fn launch_cmd(m: &Model, harness: &str, listed: &BTreeMap<String, Vec<String>>) -> Option<Vec<String>> {
    let o = m.offers.iter().find(|o| o.via.iter().any(|v| v == harness) && harness != "env")?;
    let id = if matches!(harness, "opencode" | "pi") {
        let id = format!("{}/{}", o.provider, o.id);
        let own = listed.get(harness).and_then(|ids| ids.iter().find(|i| crate::data::canonical(harness, i) == id));
        own.cloned().unwrap_or(id)
    } else {
        o.id.clone()
    };
    Some(vec![harness.into(), "--model".into(), id])
}

pub struct App {
    pub data: Data,
    /// `vals[i][c]` is `COLS[c]` of `data.models[i]`, computed once per data load: a price is a
    /// search through the offers, too slow to repeat for every model on every key.
    pub vals: Vec<[Option<f64>; COLS.len()]>,
    /// Widest shown value of each column over every model, so the layout holds when filtering.
    pub widths: [usize; COLS.len()],
    /// Best and worst value of each column among `rows`; none when they all agree.
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
    /// Cursor in the recommend overlay: the task, and the model on its best-per-price line.
    pub task_cur: usize,
    pub task_sel: usize,
    pub query: String,
    /// The query matched nothing as typed, so it is matched allowing a typo per word.
    pub typos: bool,
    /// Indices into `data.models`, in display order.
    pub rows: Vec<usize>,
    pub table: TableState,
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
    pub refreshing: bool,
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
            query: String::new(),
            typos: false,
            rows: vec![],
            table: TableState::default().with_selected(0),
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
            overlay_query: String::new(),
            page: 20,
            hscroll: 0,
            count: 0,
            g_pending: None,
            status: String::new(),
            failed: false,
            refreshing: false,
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
        self.widths = std::array::from_fn(|c| {
            self.vals.iter().filter_map(|v| v[c]).map(|v| (COLS[c].show)(v).chars().count()).max().unwrap_or(0)
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

    /// What `/` edits: the compare and help overlays filter their own rows, the table its models.
    pub fn search_target(&mut self) -> &mut String {
        if self.overlay_search() { &mut self.overlay_query } else { &mut self.query }
    }

    /// Whether `/` filters the open overlay's rows instead of the table's models.
    pub fn overlay_search(&self) -> bool {
        matches!(self.view, View::Compare | View::Help)
    }

    /// The model under the cursor: the row in the table and details, the column in compare,
    /// the one picked on the task's line in recommend.
    pub fn current(&self) -> Option<&Model> {
        if self.view == View::Compare {
            return self.marked_models().get(self.compare_sel).copied();
        }
        if self.view == View::Recommend {
            let front = self.task_frontier(&TASKS[self.task_cur]);
            return front.get(self.task_sel.min(front.len().saturating_sub(1))).map(|&(m, _)| m);
        }
        if self.view == View::Detail {
            return self.data.models.iter().find(|m| m.key == self.detail);
        }
        self.rows.get(self.selected()).map(|&i| &self.data.models[i])
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

    /// Whether any model is marked, so `M` has something to show.
    pub fn any_marked(&self) -> bool {
        !self.store.marked.is_empty()
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
        matches!(&self.input, Input::Choose { items, .. } if matches!(items.first(), Some((_, Effect::Fav(..)))))
    }

    /// The theme under the cursor of the open `t` list, which the screen previews.
    pub fn theme_preview(&self) -> Option<&'static str> {
        match &self.input {
            Input::Choose { items, sel, query, .. } => match choice_rows(items, query).get(*sel).map(|&i| &items[i]) {
                Some((_, Effect::Theme(name))) => Some(name),
                _ => None,
            },
            _ => None,
        }
    }

    /// `f`'s list for the model `key`: every task and each of its tiers, ticked where it is the
    /// favorite. The key, not the current model, since a tick can move the rows under the list.
    fn fav_items(&self, key: &str) -> Vec<(String, Effect)> {
        let name = |k: &str| self.data.models.iter().find(|m| m.key == k).map_or(k.to_string(), |m| m.name.clone());
        let item = |s: String| match self.store.favorite(&s) {
            Some(k) if k == key => format!("✓ {s}"),
            Some(k) => format!("☐ {s}  (now {})", name(k)),
            None => format!("☐ {s}"),
        };
        slots().map(|(t, x)| (item(slot(t, x)), Effect::Fav(key.to_string(), t, x))).collect()
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
        // Unmarking the last marked model leaves M (and unfavoriting the last, F) for every model rather than an empty table.
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
        let ms = &self.data.models;
        if let Some(t) = self.task {
            // The same line the recommend panel and `list --task` show.
            let front: Vec<&str> = self.task_frontier(t).iter().map(|(m, _)| m.key.as_str()).collect();
            rows.retain(|&i| front.contains(&ms[i].key.as_str()));
        }
        if numeric(self.sort_col).is_some() {
            let desc = self.descending;
            // Blanks last either way, names breaking ties.
            rows.sort_by(|&a, &b| {
                let order = match (self.val(a, self.sort_col), self.val(b, self.sort_col)) {
                    (Some(x), Some(y)) if desc => y.total_cmp(&x),
                    (Some(x), Some(y)) => x.total_cmp(&y),
                    (Some(_), None) => Ordering::Less,
                    (None, Some(_)) => Ordering::Greater,
                    (None, None) => Ordering::Equal,
                };
                order.then_with(|| ms[a].name.cmp(&ms[b].name))
            });
        } else {
            // By name, or by developer or access with names breaking ties.
            let text = |m: &Model| match self.sort_col {
                1 => m.developer.to_lowercase(),
                VIA => self.shown_via(m).join(", "),
                NOTES => self.store.note(&m.key).unwrap_or("").to_lowercase(),
                _ => String::new(),
            };
            rows.sort_by_cached_key(|&i| (text(&ms[i]), ms[i].name.to_lowercase()));
            if self.descending {
                rows.reverse();
            }
        }
        // Among the models to use: a muted row is grey throughout, so it holds no extreme.
        self.ext = std::array::from_fn(|c| {
            let mut it = rows.iter().filter(|&&r| !self.muted(&ms[r])).filter_map(|&r| self.vals[r][c]);
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
    pub fn refreshed(&mut self, res: Result<Data, String>) {
        self.refreshing = false;
        self.refresh_failed = res.is_err();
        match res {
            Ok(d) => {
                self.status = "data refreshed".into();
                self.set_data(d);
            }
            // One from the environment is yours to change there, as a saved one would not replace it.
            Err(e) if e.contains(crate::data::AA_REJECTED) && crate::data::aa_key_env().is_some() => {
                self.report(Err(format!("{e} in {}", crate::data::AA_KEY_ENV)));
            }
            // No key, or the saved one was turned down: ask for one, unless you are typing or
            // choosing something else, which the prompt would throw away; `B` asks then.
            Err(e) if e.contains(crate::data::AA_REJECTED) || e.contains(crate::data::AA_MISSING) => {
                let wrong = e.contains(crate::data::AA_REJECTED);
                if self.input == Input::None {
                    self.report(Err(e));
                    self.input = Input::Key { text: String::new(), cur: 0, wrong };
                } else {
                    self.report(Err(format!("{e}; B then Artificial Analysis to enter one")));
                }
            }
            Err(e) => self.report(Err(format!("refresh failed: {e}"))),
        }
    }

    /// The outcome of an action in the status bar, an error in red.
    pub fn report(&mut self, res: Result<String, String>) {
        self.failed = res.is_err();
        self.status = res.unwrap_or_else(|e| e);
    }

    /// The `B` chooser, each source saying what it takes; the TUI opens with it until one is picked.
    pub fn ask_source(&mut self) {
        let items =
            Source::ALL.iter().map(|s| (format!("{:<20} {}", s.label(), s.about()), Effect::Source(*s))).collect();
        let sel = Source::ALL.iter().position(|s| *s == crate::data::source()).unwrap_or(0);
        let title = if self.first_start { "benchmarks? B changes it later" } else { "benchmarks?" };
        self.input = Input::choose(title, items, sel);
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
        self.status = format!("benchmarks from {} · B to change", src.label());
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
    pub fn task_frontier(&self, t: &Task) -> Vec<(&Model, f64)> {
        let usable = |m: &&Model| self.usable(m);
        // Ranked only when the filters show them, so a hidden favorite drops no shown model.
        let favs = self.store.task_favorites(t.name);
        let fav: Vec<&Model> =
            self.data.models.iter().filter(|m| usable(m) && favs.contains(&m.key.as_str())).collect();
        task_frontier(self.filtered(usize::MAX).map(|(_, m)| m).filter(usable), t, &fav)
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
        let usable = self.filtered(usize::MAX).map(|(_, m)| m).filter(|m| self.usable(m));
        self.store.is_favorite(Some(t), key) && !recommended(usable, t, key)
    }

    /// The task `f` and the ★ mark refer to: the one under the cursor in recommend, else the
    /// picked one.
    pub fn task_at_hand(&self) -> Option<&'static Task> {
        if self.view == View::Recommend { Some(&TASKS[self.task_cur]) } else { self.task }
    }

    /// Whether the model's row shows a ★: it is the favorite for the task at hand, or with no
    /// task for any. One ★ either way; the status bar and the detail name the tasks.
    pub fn starred(&self, key: &str) -> bool {
        self.store.is_favorite(self.task_at_hand(), key)
    }

    /// `f`: favorite the model `key` for the task or one tier of it, or unfavorite it when it
    /// already is.
    fn fav(&mut self, key: &str, task: &'static str, tier: Option<&'static str>) -> Option<Effect> {
        let name = self.data.models.iter().find(|m| m.key == key)?.name.clone();
        let task = &slot(task, tier);
        self.store.toggle_favorite(task, key);
        self.status = match self.store.favorite(task) == Some(key) {
            true => format!("★ {name} favorite for {task}"),
            false => format!("{name} no longer the favorite for {task}"),
        };
        // With the list still open (m), its boxes follow.
        if self.choosing_favs()
            && let (fresh, Input::Choose { items, .. }) = (self.fav_items(key), &mut self.input)
        {
            *items = fresh;
        }
        self.rebuild_in_place();
        Some(Effect::Save)
    }

    /// The cursor and length of the open dropdown or choice list (`o`, `x`), whose moves take the
    /// table's vim motions.
    fn list(&mut self) -> Option<(&mut usize, usize)> {
        match &mut self.input {
            Input::Menu { items, query, sel, .. } => Some((sel, menu_rows(items, query).len())),
            Input::Choose { items, sel, query, .. } => Some((sel, choice_rows(items, query).len())),
            _ => None,
        }
    }

    fn move_by(&mut self, n: isize) {
        if let Some((sel, len)) = self.list() {
            *sel = step(*sel, n, len);
        } else if self.view == View::Table {
            self.select(step(self.selected(), n, self.rows.len()));
        } else if self.view == View::Recommend {
            (self.task_cur, self.task_sel) = (step(self.task_cur, n, TASKS.len()), 0);
        } else {
            self.scroll = self.scroll.saturating_add_signed(n.clamp(i16::MIN as isize, i16::MAX as isize) as i16);
        }
    }

    fn go_to(&mut self, row: usize) {
        if let Some((sel, len)) = self.list() {
            *sel = row.min(len.saturating_sub(1));
        } else if self.view == View::Table {
            self.select(row);
        } else if self.view == View::Recommend {
            (self.task_cur, self.task_sel) = (row.min(TASKS.len() - 1), 0);
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
        self.input = Input::Menu {
            col: self.col,
            items,
            sel: picked.map_or(0, |i| i + 1),
            query: String::new(),
            cur: 0,
            typing: false,
        };
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
        let list = matches!(self.input, Input::Menu { typing: false, .. } | Input::Choose { typing: false, .. });
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
            KeyCode::Down | KeyCode::Char('j') => self.move_by(n),
            KeyCode::Up | KeyCode::Char('k') => self.move_by(-n),
            KeyCode::PageDown => self.move_by(n * half * 2),
            KeyCode::PageUp => self.move_by(-n * half * 2),
            KeyCode::Char(')') => self.move_by(n * half),
            KeyCode::Char('(') => self.move_by(-n * half),
            KeyCode::Char('d') if ctrl => self.move_by(n * half),
            KeyCode::Char('u') if ctrl => self.move_by(-n * half),
            KeyCode::Char('g') if count > 0 => self.go_to(count - 1),
            KeyCode::Home | KeyCode::Char('g') => self.go_to(0),
            KeyCode::End | KeyCode::Char('G') => self.go_to(usize::MAX),
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
    /// list stays open until a click outside; in the other choice lists it picks the entry.
    pub fn mouse(&mut self, m: Mouse) -> Option<Effect> {
        let effect = self.on_mouse(m);
        self.follow();
        effect
    }

    fn on_mouse(&mut self, m: Mouse) -> Option<Effect> {
        if let Mouse::Key(code) = m {
            return self.on_key(code.into());
        }
        let list = matches!(self.input, Input::Menu { typing: false, .. } | Input::Choose { typing: false, .. });
        if let Mouse::Scroll(n) = m {
            if list || self.input == Input::None {
                self.move_by(n);
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
                // Outside, or anything else that is not an entry: close it.
                _ => self.input_key(KeyCode::Esc, KeyModifiers::NONE),
            };
        }
        // The sideways wheel moves the model cursor wherever h and l do.
        if let (View::Compare | View::Recommend, Input::None, Mouse::Cols(n)) = (&self.view, &self.input, &m) {
            return self.table_key(KeyCode::Char(if *n < 0 { 'h' } else { 'l' }), n.abs());
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
                return self.table_key(KeyCode::Char('f'), 1);
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
                    self.status = format!("{h} cannot be opened on {}", m.name);
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
                // The groups of `GROUPS` and where each comes from: prices and context, benchmarks,
                // then speed, which only Artificial Analysis measures. With no page for the model,
                // the site itself.
                return Some(Effect::Open(if col < ECI {
                    m.price_page().unwrap_or_else(|| "https://models.dev".into())
                } else {
                    let src = if col < SPEED { crate::data::source() } else { Source::Aa };
                    m.page(src).unwrap_or_else(|| format!("https://{}", src.site()))
                }));
            }
            Mouse::Pick(n) if n < self.rows.len() => {
                // The range, if any, becomes picked rows, then the clicked row toggles.
                if let Some(r) = self.visual.take().map(|a| a.min(self.selected())..=a.max(self.selected())) {
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
        let row = table || matches!(self.view, View::Detail | View::Compare | View::Recommend);
        // Compare and recommend move a model cursor sideways, wrapping, instead of the column.
        let across = match self.view {
            View::Compare => Some(self.marked_models().len()),
            View::Recommend => Some(self.task_frontier(&TASKS[self.task_cur]).len()),
            _ => None,
        };
        match code {
            KeyCode::Char('h') | KeyCode::Left if table => self.col = step_col(self.col, -n),
            KeyCode::Char('l') | KeyCode::Right if table => self.col = step_col(self.col, n),
            KeyCode::Char('h') | KeyCode::Left if across.is_some() => {
                let (len, sel) = (across?, self.across_sel());
                *sel = step((*sel).min(len.saturating_sub(1)), -n, len);
            }
            KeyCode::Char('l') | KeyCode::Right if across.is_some() => {
                let (len, sel) = (across?, self.across_sel());
                *sel = step((*sel).min(len.saturating_sub(1)), n, len);
            }
            KeyCode::Char('0' | '_') if table => self.col = 0,
            KeyCode::Char('$') if table => self.col = NCOLS - 1,
            KeyCode::Char('0' | '_') if across.is_some() => *self.across_sel() = 0,
            KeyCode::Char('$') if across.is_some() => *self.across_sel() = across?.saturating_sub(1),
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
            KeyCode::Char('d') if table => self.status = "d opens a dropdown on the Dev, Price and Via columns".into(),
            KeyCode::Char('M') if table && !self.only_marked && !self.any_marked() => {
                self.status = "no selected models: space selects the one under the cursor".into();
            }
            KeyCode::Char('M') if table => {
                self.only_marked = !self.only_marked;
                self.rebuild();
            }
            KeyCode::Char('F') if table && !self.only_fav && !self.any_fav() => {
                self.status = "no favorites: f favorites the one under the cursor".into();
            }
            KeyCode::Char('F') if table => {
                self.only_fav = !self.only_fav;
                self.rebuild();
            }
            KeyCode::Char('E') if table && !self.only_excluded && self.store.excluded.is_empty() => {
                self.status = "no excluded models: e excludes the one under the cursor".into();
            }
            KeyCode::Char('E') if table => {
                self.only_excluded = !self.only_excluded;
                self.rebuild();
            }
            KeyCode::Char('U') if table && self.store.marked.is_empty() => {
                self.status = "no selected models: space selects the one under the cursor".into();
            }
            KeyCode::Char('U') if table => {
                let n = std::mem::take(&mut self.store.marked).len();
                self.rebuild_in_place();
                self.status = format!("deselected {n}");
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
            KeyCode::Esc => {
                if self.overlay_search() && !self.overlay_query.is_empty() {
                    self.overlay_query.clear();
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
                self.task_sel = 0;
            }
            KeyCode::Char('e') if row => {
                return self.flag(Store::is_excluded, Store::toggle_excluded, ["excluded", "unexcluded"]);
            }
            KeyCode::Char('f') if row => {
                // Starting on the task at hand, so f enter toggles it.
                let items = self.fav_items(&self.current()?.key);
                let sel = self.task_at_hand().and_then(|t| slots().position(|s| s == (t.name, None))).unwrap_or(0);
                self.input = Input::choose("favorite for which tasks?", items, sel);
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
                self.input = Input::Note { cur: text.len(), text };
            }
            KeyCode::Char('o') if row => {
                let mut items: Vec<_> =
                    self.current()?.links().into_iter().map(|(site, url)| (site.into(), Effect::Open(url))).collect();
                if items.len() == 1 {
                    return items.pop().map(|(_, e)| e);
                }
                self.input = Input::choose("open on which site?", items, 0);
            }
            KeyCode::Char('x') if row => {
                let m = self.current()?;
                let mut items: Vec<_> = m
                    .via
                    .iter()
                    .filter_map(|h| launch_cmd(m, h, &self.data.harness))
                    .map(|c| (c.join(" "), Effect::Launch(c)))
                    .collect();
                match items.len() {
                    0 => self.status = format!("no harness has {}; Via shows where you have access", m.name),
                    1 => return items.pop().map(|(_, e)| e),
                    _ => self.input = Input::choose("open in which harness?", items, 0),
                }
            }
            KeyCode::Char('y') if row => return Some(Effect::Copy(model_id(self.current()?))),
            KeyCode::Char('Y') if row => return Some(Effect::Copy(self.current()?.name.clone())),
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
                self.input = Input::choose("theme?", items, crate::view::theme(&self.store.theme));
            }
            KeyCode::Char('B') => self.ask_source(),
            KeyCode::Enter if self.view == View::Recommend => {
                let t = &TASKS[self.task_cur];
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
                self.status = match (best, front.first()) {
                    (Some(a), Some(b)) => format!("{} best, {} cheapest", a.0.name, b.0.name),
                    _ => format!("no model has data for {}", t.name),
                };
            }
            KeyCode::Enter if table && self.current().is_some() => {
                self.detail = self.current()?.key.clone();
                self.view = View::Detail;
                self.scroll = 0;
            }
            // A new search starts empty; esc brings the previous one back.
            KeyCode::Char('/') if table || self.overlay_search() => {
                self.input = Input::Search { cur: 0, was: std::mem::take(self.search_target()) };
                if table {
                    self.rebuild();
                }
            }
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
        match &mut self.input {
            Input::Search { cur, was } => {
                let overlay = matches!(self.view, View::Compare | View::Help);
                let query = if overlay { &mut self.overlay_query } else { &mut self.query };
                match code {
                    KeyCode::Enter => self.input = Input::None,
                    KeyCode::Esc => {
                        *query = std::mem::take(was);
                        self.input = Input::None;
                    }
                    KeyCode::Down | KeyCode::Up => {
                        self.move_by(if code == KeyCode::Down { 1 } else { -1 });
                        return None;
                    }
                    _ if edit(query, cur, code, mods, |_| true) => {}
                    _ => return None,
                }
                if !overlay {
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
            Input::Note { text, cur } => match code {
                KeyCode::Enter => {
                    let text = std::mem::take(text);
                    self.input = Input::None;
                    let key = self.current()?.key.clone();
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
                _ if edit(text, cur, code, mods, |c| matches!(c, '0'..='9' | '.')) => {}
                KeyCode::Enter => {
                    if let Ok(v) = text.parse::<f64>() {
                        let (col, lo, hi) =
                            (*col, if *min { v } else { f64::NEG_INFINITY }, if *min { f64::INFINITY } else { v });
                        // A new bound on a column replaces the old one of the same kind.
                        self.bounds.retain(|&(c, l, _)| c != col || l.is_finite() != lo.is_finite());
                        self.bounds.push((col, lo, hi));
                    }
                    self.input = Input::None;
                    self.rebuild();
                }
                _ => {}
            },
            Input::Menu { col, items, sel, query, cur, typing } => {
                let rows = menu_rows(items, query);
                let last = rows.len() - 1;
                // Enter does what space does; while searching, space is typed.
                let toggle = code == KeyCode::Enter || (code == KeyCode::Char(' ') && !*typing);
                match code {
                    KeyCode::Down => *sel = (*sel + 1).min(last),
                    KeyCode::Up => *sel = sel.saturating_sub(1),
                    // On Price it picks the level, or drops it when it is the picked one, and keeps the
                    // dropdown open.
                    _ if toggle && *col == PRICE => {
                        let i = rows[*sel];
                        self.set_price_level(if self.price_level() == i.checked_sub(1) { 0 } else { i });
                        self.rebuild();
                    }
                    // On Dev and Via it adds or drops the entry, as space marks a model, keeping the
                    // dropdown open; on "any" it drops all.
                    _ if toggle => {
                        let i = rows[*sel];
                        let list = if *col == 1 { &mut self.dev } else { &mut self.via };
                        match list.iter().position(|d| *d == items[i].0) {
                            _ if i == 0 => list.clear(),
                            Some(k) => drop(list.remove(k)),
                            None => list.push(items[i].0.clone()),
                        }
                        self.rebuild();
                    }
                    // Esc while searching drops the search but keeps the entry under the cursor.
                    KeyCode::Esc if *typing => {
                        (*sel, *typing, *cur) = (rows[*sel], false, 0);
                        query.clear();
                    }
                    _ if *typing && edit(query, cur, code, mods, |_| true) => {
                        // The first match, so that enter picks it.
                        *sel = menu_rows(items, query).len().min(2) - 1;
                    }
                    KeyCode::Char('/') => *typing = true,
                    KeyCode::Esc => self.input = Input::None,
                    _ => {}
                }
            }
            Input::Choose { items, sel, query, cur, typing, .. } => match code {
                // While searching, ↓ ↑ move the cursor, as in a dropdown.
                KeyCode::Down if *typing => *sel = (*sel + 1).min(choice_rows(items, query).len().saturating_sub(1)),
                KeyCode::Up if *typing => *sel = sel.saturating_sub(1),
                // Space ticks a task in f's list and keeps it open, as in the Dev and Via dropdowns,
                // and so does enter. While searching, space is typed.
                KeyCode::Char(' ') | KeyCode::Enter
                    if (code == KeyCode::Enter || !*typing) && matches!(items.first(), Some((_, Effect::Fav(..)))) =>
                {
                    let i = *choice_rows(items, query).get(*sel)?;
                    if let Some((_, Effect::Fav(key, task, tier))) = items.get(i) {
                        let (key, task, tier) = (key.clone(), *task, *tier);
                        return self.fav(&key, task, tier);
                    }
                }
                // Esc while searching drops the search but keeps the entry under the cursor.
                KeyCode::Esc if *typing => {
                    (*sel, *typing, *cur) = (*choice_rows(items, query).get(*sel).unwrap_or(&0), false, 0);
                    query.clear();
                }
                KeyCode::Enter => {
                    let i = *choice_rows(items, query).get(*sel)?;
                    let (_, effect) = items.swap_remove(i);
                    self.input = Input::None;
                    match effect {
                        Effect::Theme(name) => {
                            self.store.theme = if name == THEMES[0].0 { String::new() } else { name.to_string() };
                            self.status = format!("theme {name}");
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
                // The first match, so that enter picks it.
                _ if *typing && edit(query, cur, code, mods, |_| true) => *sel = 0,
                KeyCode::Char('/') => *typing = true,
                // Closing the first start's choice picks the default.
                KeyCode::Esc if self.first_start && matches!(items.first(), Some((_, Effect::Source(_)))) => {
                    self.input = Input::None;
                    return self.switch(Source::default());
                }
                // The key that opens the theme list also closes it.
                KeyCode::Char('t') if self.theme_preview().is_some() => self.input = Input::None,
                KeyCode::Esc => self.input = Input::None,
                _ => {}
            },
            Input::Quit => {
                if code == KeyCode::Char('q') {
                    return Some(Effect::Quit);
                }
                self.input = Input::None;
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
            url: format!("https://x/{key}"),
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
        assert_eq!(code(&mut a, KeyCode::Enter), None);
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
        let rejected = format!("could not download model data: {}", crate::data::AA_REJECTED);
        let mut a = app();
        a.refreshed(Err(rejected.clone()));
        assert!(matches!(a.input, Input::Key { wrong: true, .. }));
        // A new key while the old one's refresh is under way starts another.
        crate::data::set_source(Source::Aa);
        a.refreshing = true;
        press(&mut a, "new");
        assert_eq!(code(&mut a, KeyCode::Enter), Some(Effect::Refresh));
        assert_eq!(crate::data::aa_key().as_deref(), Some("new"));
        crate::data::TEST_KEYS.with(|k| k.borrow_mut()[0] = Some("env".into()));
        let mut e = app();
        e.refreshed(Err(rejected));
        assert!(e.input == Input::None && e.status.contains(crate::data::AA_KEY_ENV), "fixed where it is set");
        let mut b = app();
        b.refreshed(Err("offline".into()));
        assert_eq!(b.input, Input::None, "any other failure is only reported");
        let mut c = app();
        c.input = Input::Note { text: "half".into(), cur: 4 };
        c.refreshed(Err(crate::data::AA_MISSING.into()));
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
        assert!(matches!(a.input, Input::Menu { sel: 2, .. }), "G to the last entry");
        press(&mut a, "gg");
        assert!(matches!(a.input, Input::Menu { sel: 0, .. }), "gg to the first");
        press(&mut a, "3gg");
        assert!(matches!(a.input, Input::Menu { sel: 2, .. }), "a count picks the entry");
        code(&mut a, KeyCode::Enter);
        assert!(matches!(a.input, Input::Menu { sel: 2, .. }), "enter toggles as space does: the dropdown stays open");
        code(&mut a, KeyCode::Esc);
        assert_eq!((a.dev.as_slice(), keys(&a)), (&["openai".to_string()][..], vec!["gpt55", "mini"]));
        // Counts follow the developer picked; the price levels are maxima.
        press(&mut a, "ld");
        assert_eq!(menu(&a), [("any", 2), ("free", 0), ("≤$0.5", 0), ("≤$2", 1), ("≤$5", 1), ("≤$15", 2)]);
        press(&mut a, "jjj");
        code(&mut a, KeyCode::Enter);
        code(&mut a, KeyCode::Esc);
        assert_eq!(keys(&a), ["mini"]);
        // Reopening starts on the level in effect; esc leaves it alone, "any" drops it.
        press(&mut a, "d");
        assert!(matches!(a.input, Input::Menu { sel: 3, .. }));
        code(&mut a, KeyCode::Esc);
        assert_eq!(keys(&a), ["mini"]);
        press(&mut a, "dkkk");
        code(&mut a, KeyCode::Enter);
        code(&mut a, KeyCode::Esc);
        assert_eq!((a.bounds.len(), keys(&a).len()), (0, 2));
        // Space picks a level and keeps the dropdown open; again on it drops it.
        press(&mut a, "djjj ");
        assert_eq!((a.price_level(), keys(&a)), (Some(2), vec!["mini"]));
        assert!(matches!(a.input, Input::Menu { sel: 3, .. }));
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
    fn x_launches_the_only_harness_or_asks_which() {
        let mut a = app();
        for m in &mut a.data.models {
            let via = m.via.clone();
            m.offers[0].via = via;
        }
        let cmd = |h: &str, id: &str| Some(Effect::Launch(vec![h.into(), "--model".into(), id.into()]));
        assert_eq!(press(&mut a, "x"), None, "gpt55 has codex and opencode");
        assert!(matches!(&a.input, Input::Choose { items, sel: 0, .. } if items.len() == 2));
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
        assert_eq!(press(&mut a, "x"), cmd("claude", "opus5"), "one harness launches at once");
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
        assert!(matches!(&a.input, Input::Menu { query, sel: 1, typing: true, .. } if query == "OPEN"));
        code(&mut a, KeyCode::Enter);
        assert_eq!(a.dev, ["openai"], "enter toggles the first match");
        assert!(matches!(&a.input, Input::Menu { query, typing: true, .. } if query == "OPEN"), "and the search stays");
        code(&mut a, KeyCode::Esc);
        code(&mut a, KeyCode::Esc);
        press(&mut a, "d/anth");
        code(&mut a, KeyCode::Backspace);
        code(&mut a, KeyCode::Esc);
        assert!(
            matches!(&a.input, Input::Menu { query, sel: 1, typing: false, .. } if query.is_empty()),
            "esc drops the search, the cursor stays on anthropic"
        );
        press(&mut a, "/zzz");
        assert!(matches!(a.input, Input::Menu { sel: 0, .. }), "no match leaves only any");
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
        assert_eq!(a.input, Input::Note { text: "fast ".into(), cur: 5 }, "trailing spaces go with the word");
        ctrl(&mut a, 'w');
        ctrl(&mut a, 'w');
        assert_eq!(a.input, Input::Note { text: String::new(), cur: 0 });
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
        assert!(matches!(&a.input, Input::Menu { query, sel: 1, .. } if query.is_empty()));
    }

    #[test]
    fn prompts_edit_around_a_cursor() {
        let mut a = App::new(Data::default(), Store::default());
        let note = |a: &App| match &a.input {
            Input::Note { text, cur } => (text.clone(), *cur),
            _ => unreachable!(),
        };
        a.input = Input::Note { text: "über fast".into(), cur: 9 };
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
        press(&mut a, "ls");
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
        press(&mut a, "/opus");
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
        a.data.models.push(model("weak", true, Some(30.0), 0.01));
        a.data.models.push(model("edge", true, Some(49.6), 0.05));
        let front: Vec<&str> =
            a.task_frontier(fit::task("coding").unwrap()).iter().map(|(m, _)| m.key.as_str()).collect();
        assert_eq!(front, ["edge", "mini", "gpt55"], "cheap alone is no recommendation; 49.6 shows as 50");
    }

    #[test]
    fn a_task_keeps_the_best_of_each_price_level() {
        let mut a = app();
        a.data.models.push(model("flash", true, Some(70.0), 1.5));
        let front: Vec<&str> =
            a.task_frontier(fit::task("coding").unwrap()).iter().map(|(m, _)| m.key.as_str()).collect();
        assert_eq!(front, ["flash", "gpt55"], "flash beats mini at about the same price");
        let data = Data { models: std::mem::take(&mut a.data.models), ..Data::default() };
        a.set_data(data);
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
            assert_eq!((&a.view, a.current().unwrap().key.as_str()), (&View::Detail, "llama4"));
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
        assert_eq!(front(&a), ["mini", "opus5", "gpt55"]);
        assert!(!a.favorite_unrecommended(coding, "mini"), "on the frontier on its own");
        a.store.toggle_favorite("coding", "gpt55");
        assert!(!a.favorite_unrecommended(coding, "gpt55"), "the best model is recommended on its own");
    }

    #[test]
    fn f_favorites_a_model_for_the_task_at_hand() {
        let mut a = app();
        // No task in context: f asks which, listing every task, each followed by its tiers.
        assert_eq!(press(&mut a, "f"), None);
        assert!(matches!(&a.input, Input::Choose { items, .. } if items.len() == TASKS.len() * 4));
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
        press(&mut a, "Rj");
        assert_eq!(a.current().unwrap().key, "mini");
        assert_eq!(press(&mut a, "f"), None);
        assert!(matches!(&a.input, Input::Choose { sel: 4, items, .. } if items[4].0 == "☐ coding  (now gpt55)"));
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
        assert!(a.current().is_none(), "overall has no data in this fixture, so nothing is picked");
        press(&mut a, "j");
        let front: Vec<String> =
            a.task_frontier(fit::task("coding").unwrap()).iter().map(|(m, _)| m.key.clone()).collect();
        assert_eq!(front, ["mini", "gpt55"], "cheapest first, best last");
        assert_eq!(a.current().unwrap().key, "mini", "the cursor starts on the cheapest");
        press(&mut a, "l");
        assert_eq!(a.current().unwrap().key, "gpt55");
        press(&mut a, "l");
        assert_eq!(a.current().unwrap().key, "mini", "wraps");
        press(&mut a, "h");
        assert_eq!(a.current().unwrap().key, "gpt55", "and back");
        press(&mut a, "0");
        assert_eq!(a.current().unwrap().key, "mini");
        a.mouse(Mouse::Cols(1));
        assert_eq!(a.current().unwrap().key, "gpt55", "the sideways wheel moves the model cursor as in compare");
        press(&mut a, "0$");
        assert_eq!(a.current().unwrap().key, "gpt55");
        press(&mut a, "oG");
        assert_eq!(
            code(&mut a, KeyCode::Enter),
            Some(Effect::Open(a.current().unwrap().url.clone())),
            "o opens the picked model"
        );
        assert!(matches!(press(&mut a, "y"), Some(Effect::Copy(id)) if id.contains("gpt55")));
        assert_eq!(press(&mut a, "e"), Some(Effect::Save), "e excludes it, so it leaves the line");
        assert_eq!(a.current().unwrap().key, "mini", "the cursor lands on what is left");
        press(&mut a, "$j");
        assert_eq!((a.task_cur, a.task_sel), (2, 0), "j k move between tasks and start at the cheapest");
        press(&mut a, "k$RR");
        assert_eq!(a.task_sel, 0, "so does reopening");
    }

    #[test]
    fn the_table_cursor_follows_the_model_picked_in_an_overlay() {
        let mut a = app();
        let row = |a: &App, key: &str| a.rows.iter().position(|&i| a.data.models[i].key == key).unwrap();
        press(&mut a, "Rjl");
        assert_eq!(a.selected(), row(&a, "gpt55"), "recommend's pick");
        a.mouse(Mouse::Cols(1));
        assert_eq!(a.selected(), row(&a, "mini"), "the wheel too");
        code(&mut a, KeyCode::Esc);
        assert_eq!((&a.view, a.current().unwrap().key.as_str()), (&View::Table, "mini"), "esc lands on it");
        a.store.marked = vec!["gpt55".into(), "mini".into()];
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
        assert_eq!(
            press(&mut a, "o"),
            Some(Effect::Open("https://x/opus5".into())),
            "o opens the selected model, at once when only OpenRouter has it"
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
        assert_eq!(a.status, "no selected models: space selects the one under the cursor");
        press(&mut a, "gg j M");
        assert_eq!(press(&mut a, "U"), Some(Effect::Save), "U saves");
        assert_eq!((a.store.marked.len(), a.only_marked, a.rows.len()), (0, false, 3), "U unmarks all and leaves M");
    }

    #[test]
    fn esc_backs_out_of_views_and_toggles_close_what_they_open() {
        let mut a = app();
        press(&mut a, "M");
        assert_eq!(
            (a.only_marked, a.status.as_str()),
            (false, "no selected models: space selects the one under the cursor")
        );
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
            "epoch.ai, openrouter.ai: no models.dev page, as the developer does not offer it, nor an AA one"
        );
        assert_eq!(code(&mut a, KeyCode::Enter), Some(Effect::Open("https://epoch.ai/models/gpt55".into())));
        press(&mut a, "oG");
        assert_eq!(code(&mut a, KeyCode::Enter), Some(Effect::Open("https://x/gpt55".into())), "openrouter.ai last");
        assert_eq!(press(&mut a, "y"), Some(Effect::Copy("p/gpt55".into())));
        a.data.models[0].name = "GPT 5.5".into();
        assert_eq!(press(&mut a, "Y"), Some(Effect::Copy("GPT 5.5".into())));
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
        a.report(Ok("done".into()));
        assert!(!a.failed);
    }

    #[test]
    fn mouse_selects_sorts_and_scrolls() {
        let mut a = app();
        assert_eq!(a.mouse(Mouse::Row(2)), None);
        assert_eq!(a.selected(), 2);
        assert_eq!(a.mouse(Mouse::Row(2)), None);
        assert_eq!(a.view, View::Table, "a click only highlights, however often");
        a.mouse(Mouse::Row(1));
        assert_eq!(a.mouse(Mouse::Cell(2, 0)), None);
        assert_eq!((a.selected(), &a.view), (2, &View::Detail), "a double click on the name opens the details");
        assert_eq!(a.mouse(Mouse::Scroll(3)), None);
        assert_eq!(a.scroll, 3);
        assert_eq!(a.mouse(Mouse::Row(0)), None, "clicks do nothing behind an overlay");
        assert_eq!(a.view, View::Detail);
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
        assert_eq!(a.view, View::Detail);
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
    fn ctrl_w_takes_a_word_after_a_wide_space() {
        let mut a = app();
        a.input = Input::Note { text: "a\u{3000}b".into(), cur: 5 };
        ctrl(&mut a, 'w');
        assert_eq!(a.input, Input::Note { text: "a\u{3000}".into(), cur: 4 });
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
