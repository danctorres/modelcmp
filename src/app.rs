//! TUI state and key handling. No I/O here: side effects come back to the shell as `Effect`s,
//! so every key is unit-testable.

use crate::data::{self, Data, Model};
use crate::fit::{self, TASKS, Task};
use crate::store::Store;
use crate::view::{LEVELS, ctx, frontier, level_label, money, score, task_frontier, task_score, visible};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::widgets::TableState;
use std::cmp::{Ordering, Reverse};
use std::collections::BTreeMap;

/// A numeric column. The text columns (model name, developer) come first and are not listed here.
pub struct Col {
    pub name: &'static str,
    /// What the column means, for the top border and `?`.
    pub about: &'static str,
    /// The task whose benchmarks feed the column.
    pub task: Option<&'static str>,
    pub lower_better: bool,
    pub get: fn(&Model) -> Option<f64>,
    pub show: fn(f64) -> String,
}

impl Col {
    /// A benchmark column, the only kind a price frontier makes sense for.
    pub fn is_score(&self) -> bool {
        self.name == "ECI" || self.task.is_some()
    }
}

const fn col(name: &'static str, about: &'static str, get: fn(&Model) -> Option<f64>) -> Col {
    Col { name, about, task: None, lower_better: false, get, show: |v| score(Some(v)) }
}

fn positive(x: f64) -> Option<f64> {
    (x > 0.0).then_some(x)
}

/// Prices from the offer you'd pay, then the Epoch index, the task percentiles and the
/// Artificial Analysis indices.
pub const COLS: [Col; 10] = [
    Col {
        lower_better: true,
        show: money,
        ..col("Price", "USD per 1M tokens, blended 3:1 input:output", |m| m.cost())
    },
    Col {
        lower_better: true,
        show: money,
        ..col("$in", "USD per 1M input tokens, cheapest available provider", |m| Some(m.price()?.input))
    },
    Col {
        lower_better: true,
        show: money,
        ..col("$out", "USD per 1M output tokens, cheapest available provider", |m| Some(m.price()?.output))
    },
    Col {
        show: |v| ctx((v * 1000.0) as u64),
        ..col("Ctx", "context window, in tokens", |m| positive(m.context as f64 / 1000.0))
    },
    col("ECI", "Epoch Capabilities Index, overall capability", |m| m.eci),
    Col {
        task: Some("coding"),
        ..col("Coding", "mean percentile (0-100) on coding benchmarks", |m| task_score(m, "coding"))
    },
    Col {
        task: Some("agentic"),
        ..col("Agentic", "mean percentile (0-100) on agentic benchmarks", |m| task_score(m, "agentic"))
    },
    Col {
        task: Some("reasoning"),
        ..col("Reason", "mean percentile (0-100) on reasoning benchmarks", |m| task_score(m, "reasoning"))
    },
    Col { task: Some("math"), ..col("Math", "mean percentile (0-100) on math benchmarks", |m| task_score(m, "math")) },
    col("Code/$", "Coding divided by Price, as a percentile (0-100)", |m| m.fit.get("value").copied()),
];

/// Text columns before the numbers: 0 is the model name, 1 its developer. `VIA` follows them.
const TEXT: usize = 2;
/// Column index of the blended price, the default sort and the frontier's.
pub const PRICE: usize = TEXT;
/// Column index of ECI.
#[cfg(test)]
pub const ECI: usize = TEXT + 4;
/// Column index of where you have access.
pub const VIA: usize = TEXT + COLS.len();
/// Column index of the tasks a model is best for, and of your note, the last one.
pub const BEST: usize = VIA + 1;
pub const NOTES: usize = VIA + 2;
pub const NCOLS: usize = NOTES + 1;

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
        BEST => "Best for",
        NOTES => "Notes",
        _ => numeric(col).map_or("", |c| c.name),
    }
}

/// Whether the header of the column at cursor index `col` opens a dropdown with `d`.
pub fn has_menu(col: usize) -> bool {
    col == 1 || col == PRICE || col == VIA
}

/// What the column at cursor index `col` means.
pub fn col_about(col: usize) -> String {
    match col {
        0 => "model name; dimmed if no available provider".into(),
        1 => "company that trained the model".into(),
        VIA => "where you have access: the harnesses that list the model, env for a provider API key".into(),
        BEST => "the one or two tasks it ranks highest on, at the 60th percentile or above".into(),
        NOTES => "your note, n to edit".into(),
        _ => numeric(col).map_or("", |c| c.about).into(),
    }
}

pub const HELP: &[(&str, &str)] = &[
    ("j k ↓ ↑", "move; a count repeats, as in 3j"),
    ("h l ← →", "pick a column; the top border says what it means"),
    ("s", "sort by the column; again reverses"),
    ("enter", "details: every benchmark, price per provider"),
    ("> <", "minimum / maximum for the column, e.g. > 70 enter on Coding"),
    ("d", "dropdown on the Dev, Price and Via headers (▾); / searches it, space toggles several"),
    ("( ) ^d ^u", "half a page up / down"),
    ("gg G 3gg", "top / bottom / row 3"),
    ("m", "mark the model"),
    ("M", "show marked models only; with F, marked and favorites"),
    ("C", "compare marked models: cheapest, best coder, most coding per $"),
    (
        "p",
        "best model per price level on a benchmark column: cheapest first, each row down costs more and scores higher",
    ),
    ("/", "filter by name; esc clears"),
    ("c", "clear everything: filters, bounds, frontier, task ranking, marks"),
    ("a", "all models, including ones you have no access to; again: yours only"),
    ("f F", "favorite / show favorites only"),
    ("n", "note for the model"),
    ("typing", "← → ^a ^e move, alt-b alt-f by word; ^w alt-d delete a word, ^u ^k to the start / end"),
    ("y Y", "copy the model id (provider/model) / the model name"),
    ("o", "open the model on openrouter.ai"),
    ("x", "launch a harness with the model: the Via filter's, else the first in Via; quit it to come back"),
    ("r", "refresh data now (auto every 24h)"),
    ("t", "tasks: what each one measures, when to use it, the best model per price; enter ranks the table by it"),
    ("?", "this help"),
    ("qq", "quit; the first q asks. esc closes an overlay or the filter"),
];

#[derive(PartialEq, Debug)]
pub enum View {
    Table,
    Detail,
    Compare,
    Help,
    Tasks,
}

#[derive(PartialEq, Debug)]
pub enum Input {
    None,
    /// Typing `App::query`; `cur` is the cursor's byte offset in it.
    Search {
        cur: usize,
    },
    Note {
        text: String,
        cur: usize,
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
}

/// Indices of the dropdown entries whose name contains `query`, any case; "any" always stays.
pub fn menu_rows(items: &[(String, usize)], query: &str) -> Vec<usize> {
    let q = query.to_lowercase();
    (0..items.len()).filter(|&i| i == 0 || items[i].0.to_lowercase().contains(&q)).collect()
}

/// Readline-style editing of a prompt with the cursor at byte `cur`: ← → ^b ^f move by a
/// character, alt-b alt-f by a word, home end ^a ^e to the ends; backspace and delete drop a
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
        text[..end].rfind(char::is_whitespace).map_or(0, |i| i + 1)
    };
    let word_end = {
        let rest = &text[*cur..];
        let start = rest.len() - rest.trim_start().len();
        *cur + start + rest[start..].find(char::is_whitespace).unwrap_or(rest.len() - start)
    };
    let to = match code {
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
    /// Run this command in the terminal, then come back.
    Launch(Vec<String>),
}

/// `provider/model` of the offer you'd pay, as harnesses name it.
pub fn model_id(m: &Model) -> String {
    m.price().map_or_else(|| m.key.clone(), |o| format!("{}/{}", o.provider, o.id))
}

/// The command that starts `harness` on `m`, if the harness has it: opencode takes
/// `provider/model`, the single-provider CLIs (claude, codex, gemini) the bare model id.
pub fn launch_cmd(m: &Model, harness: &str) -> Option<Vec<String>> {
    let o = m.offers.iter().find(|o| o.via.iter().any(|v| v == harness) && harness != "env")?;
    let id = if harness == "opencode" { format!("{}/{}", o.provider, o.id) } else { o.id.clone() };
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
    pub favs: bool,
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
    /// Column whose price frontier is shown, set with `p`.
    pub frontier: Option<usize>,
    /// Task the table is ranked for, picked in the tasks overlay; replaces the column sort.
    pub task: Option<&'static Task>,
    /// Cursor in the tasks overlay.
    pub task_cur: usize,
    pub query: String,
    /// Indices into `data.models`, in display order.
    pub rows: Vec<usize>,
    pub table: TableState,
    pub marked: Vec<String>,
    pub only_marked: bool,
    pub view: View,
    pub input: Input,
    /// Scroll offset of the detail, compare and help views. The renderer clamps it.
    pub scroll: u16,
    /// Rows visible in the body, set by the renderer; drives page movement.
    pub page: u16,
    /// First of the columns right of Dev shown when they do not all fit; the renderer keeps
    /// the selected one in view.
    pub hscroll: usize,
    pub count: usize,
    /// The count before a first `g`, waiting for the second one of `gg`.
    g_pending: Option<usize>,
    pub status: String,
    pub refreshing: bool,
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
            favs: false,
            col: PRICE,
            sort_col: PRICE,
            descending: true,
            bounds: vec![],
            dev: vec![],
            via: vec![],
            frontier: None,
            task: None,
            task_cur: 0,
            query: String::new(),
            rows: vec![],
            table: TableState::default().with_selected(0),
            marked: vec![],
            only_marked: false,
            view: View::Table,
            input: Input::None,
            scroll: 0,
            page: 20,
            hscroll: 0,
            count: 0,
            g_pending: None,
            status: String::new(),
            refreshing: false,
        };
        app.set_data(data);
        app
    }

    /// Replace the data and recompute everything derived from it.
    pub fn set_data(&mut self, data: Data) {
        self.vals = data.models.iter().map(|m| COLS.each_ref().map(|c| (c.get)(m))).collect();
        self.widths = std::array::from_fn(|c| {
            self.vals.iter().filter_map(|v| v[c]).map(|v| (COLS[c].show)(v).chars().count()).max().unwrap_or(0)
        });
        self.data = data;
        self.rebuild();
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

    pub fn current(&self) -> Option<&Model> {
        self.rows.get(self.selected()).map(|&i| &self.data.models[i])
    }

    pub fn marked_models(&self) -> Vec<&Model> {
        self.marked.iter().filter_map(|k| self.data.models.iter().find(|m| m.key == *k)).collect()
    }

    /// Models passing every filter but the frontier, ignoring the ones on column `skip`, so a
    /// dropdown can count what each of its entries would show.
    fn filtered(&self, skip: usize) -> impl Iterator<Item = (usize, &Model)> {
        let q = data::norm(&self.query);
        // Marked only and favorites only together show both sets: the models the user picked.
        let (favs, marked) = (self.favs && !self.only_marked, self.only_marked);
        visible(&self.data, &self.store, self.all, favs).filter(move |&(i, m)| {
            (q.is_empty() || m.key.contains(&q) || data::norm(&m.developer).contains(&q))
                && (!marked || self.marked.contains(&m.key) || (self.favs && self.store.is_fav(&m.key)))
                && (skip == 1 || self.dev.is_empty() || self.dev.contains(&m.developer))
                && (skip == VIA || self.via.is_empty() || self.via.iter().any(|h| m.via.contains(h)))
                && self
                    .bounds
                    .iter()
                    .filter(|b| b.0 != skip)
                    .all(|&(c, lo, hi)| self.val(i, c).is_some_and(|v| v >= lo && v <= hi))
        })
    }

    /// Recompute the visible rows after any filter, sort or data change, keeping the selection.
    pub fn rebuild(&mut self) {
        let keep = self.current().map(|m| m.key.clone());
        let mut rows: Vec<usize> = self.filtered(usize::MAX).map(|(i, _)| i).collect();
        let ms = &self.data.models;
        if let Some(c) = self.frontier.and_then(numeric) {
            rows = frontier(&rows, |i| &ms[i], c.get);
        }
        if let Some(t) = self.task {
            // Best fit first; models without data for the task drop out.
            rows.retain(|&i| fit::fit(&ms[i], t).is_some());
            rows.sort_by(|&a, &b| {
                let (x, y) = (fit::fit(&ms[a], t).unwrap_or(0.0), fit::fit(&ms[b], t).unwrap_or(0.0));
                y.total_cmp(&x).then_with(|| ms[a].name.cmp(&ms[b].name))
            });
        } else if numeric(self.sort_col).is_some() {
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
                VIA => m.via.join(","),
                BEST => crate::fit::best_for(m).join(","),
                NOTES => self.store.note(&m.key).unwrap_or("").to_lowercase(),
                _ => String::new(),
            };
            rows.sort_by_cached_key(|&i| (text(&ms[i]), ms[i].name.to_lowercase()));
            if self.descending {
                rows.reverse();
            }
        }
        self.ext = std::array::from_fn(|c| {
            let mut it = rows.iter().filter_map(|&r| self.vals[r][c]);
            let first = it.next()?;
            let (lo, hi) = it.fold((first, first), |(lo, hi), v| (lo.min(v), hi.max(v)));
            (lo != hi).then_some(if COLS[c].lower_better { (lo, hi) } else { (hi, lo) })
        });
        self.rows = rows;
        let sel = keep.and_then(|k| self.rows.iter().position(|&i| self.data.models[i].key == k));
        self.select(sel.unwrap_or(0));
    }

    /// A background refresh finished.
    pub fn refreshed(&mut self, res: Result<Data, String>) {
        self.refreshing = false;
        match res {
            Ok(d) => {
                self.status = "data refreshed".into();
                self.set_data(d);
            }
            Err(e) => self.status = format!("refresh failed: {e}"),
        }
    }

    /// The status bar's `⟳ refreshing` says it is under way, so the message is cleared.
    pub fn refresh(&mut self) -> Option<Effect> {
        self.status.clear();
        if self.refreshing {
            return None;
        }
        self.refreshing = true;
        Some(Effect::Refresh)
    }

    /// The task's price frontier among the models the filters let through: cheapest first,
    /// the best model last.
    pub fn task_frontier(&self, t: &Task) -> Vec<(&Model, f64)> {
        task_frontier(self.filtered(usize::MAX).map(|(_, m)| m), t)
    }

    fn move_by(&mut self, n: isize) {
        if self.view == View::Table {
            let i = self.selected().saturating_add_signed(n);
            self.select(i);
        } else if self.view == View::Tasks {
            self.task_cur = self.task_cur.saturating_add_signed(n).min(TASKS.len() - 1);
        } else {
            self.scroll = self.scroll.saturating_add_signed(n.clamp(i16::MIN as isize, i16::MAX as isize) as i16);
        }
    }

    fn go_to(&mut self, row: usize) {
        if self.view == View::Table {
            self.select(row);
        } else if self.view == View::Tasks {
            self.task_cur = row.min(TASKS.len() - 1);
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
            for l in ms
                .iter()
                .flat_map(|(_, m)| if by_dev { std::slice::from_ref(&m.developer) } else { m.via.as_slice() })
                .filter(|l| !l.is_empty())
            {
                *counts.entry(l).or_default() += 1;
            }
            // Most models first; the stable sort keeps ties A-Z.
            let mut names: Vec<(String, usize)> = counts.into_iter().map(|(d, n)| (d.to_string(), n)).collect();
            names.sort_by_key(|&(_, n)| Reverse(n));
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
            let max = self.bounds.iter().find(|b| b.0 == PRICE && b.1.is_infinite()).map(|b| b.2);
            (items, max.and_then(|v| LEVELS.iter().position(|&e| e == v)))
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
        match self.marked.iter().position(|k| *k == key) {
            Some(i) => {
                self.marked.remove(i);
            }
            None => self.marked.push(key),
        }
        if self.view == View::Table {
            self.move_by(1);
        }
        if self.only_marked {
            self.rebuild();
        }
    }

    pub fn key(&mut self, k: KeyEvent) -> Option<Effect> {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && k.code == KeyCode::Char('c') {
            return Some(Effect::Quit);
        }
        if self.input != Input::None {
            return self.input_key(k.code, k.modifiers);
        }
        self.status.clear();
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
        let table = self.view == View::Table;
        // Keys that act on the current model, which help and compare hide.
        let row = table || self.view == View::Detail;
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
            KeyCode::Char('h') | KeyCode::Left if table => self.col = (self.col + NCOLS - n as usize % NCOLS) % NCOLS,
            KeyCode::Char('l') | KeyCode::Right if table => self.col = (self.col + n as usize) % NCOLS,
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
            // The frontier reads cheapest first: each row down costs more and scores higher.
            KeyCode::Char('p') if table && numeric(self.col).is_some_and(Col::is_score) => {
                self.frontier = (self.frontier != Some(self.col)).then_some(self.col);
                if self.frontier.is_some() {
                    (self.sort_col, self.descending) = (PRICE, false);
                }
                self.rebuild();
            }
            KeyCode::Char('p') if table => {
                let names: Vec<&str> = COLS.iter().filter(|c| c.is_score()).map(|c| c.name).collect();
                self.status = format!("p needs a benchmark column: {}", names.join(", "));
            }
            KeyCode::Char('M') if table => {
                self.only_marked = !self.only_marked && !self.marked.is_empty();
                self.rebuild();
            }
            KeyCode::Char('c') if table => {
                self.query.clear();
                self.bounds.clear();
                self.dev.clear();
                self.via.clear();
                self.frontier = None;
                self.task = None;
                self.marked.clear();
                self.only_marked = false;
                self.favs = false;
                self.rebuild();
            }
            KeyCode::Char('q') => self.input = Input::Quit,
            KeyCode::Esc => {
                if !table {
                    self.view = View::Table;
                } else if !self.query.is_empty() {
                    self.query.clear();
                    self.rebuild();
                }
            }
            KeyCode::Char('?') => self.view = if self.view == View::Help { View::Table } else { View::Help },
            KeyCode::Char('t') => self.view = if self.view == View::Tasks { View::Table } else { View::Tasks },
            KeyCode::Char('f') if row => {
                let key = self.current()?.key.clone();
                self.store.toggle_fav(&key);
                if self.favs {
                    self.rebuild();
                }
                return Some(Effect::Save);
            }
            KeyCode::Char('n') if row => {
                let m = self.current()?;
                let text = self.store.note(&m.key).unwrap_or("").to_string();
                self.input = Input::Note { cur: text.len(), text };
            }
            KeyCode::Char('o') if row => return Some(Effect::Open(self.current()?.url.clone())),
            KeyCode::Char('x') if row => {
                let m = self.current()?;
                let picked = self.via.iter().chain(&m.via).find_map(|h| launch_cmd(m, h));
                match picked {
                    Some(cmd) => return Some(Effect::Launch(cmd)),
                    None => self.status = format!("no harness has {}; Via shows where you have access", m.name),
                }
            }
            KeyCode::Char('y') if row => return Some(Effect::Copy(model_id(self.current()?))),
            KeyCode::Char('Y') if row => return Some(Effect::Copy(self.current()?.name.clone())),
            KeyCode::Char('m') if row => self.toggle_mark(),
            KeyCode::Char('C') => {
                if self.marked.len() >= 2 {
                    self.view = View::Compare;
                    self.scroll = 0;
                } else {
                    self.status = "mark 2+ models with m, then press C".into();
                }
            }
            KeyCode::Char('r') => return self.refresh(),
            KeyCode::Enter if self.view == View::Tasks => {
                let t = &TASKS[self.task_cur];
                self.task = Some(t);
                self.view = View::Table;
                self.rebuild();
                self.select(0);
                self.status = match self.current() {
                    Some(m) => format!("best for {}: {}; enter for details, c clears", t.name, m.name),
                    None => format!("no model has data for {}", t.name),
                };
            }
            KeyCode::Enter if table && self.current().is_some() => {
                self.view = View::Detail;
                self.scroll = 0;
            }
            KeyCode::Char('/') if table => self.input = Input::Search { cur: self.query.len() },
            KeyCode::Char('a') if table => {
                self.all = !self.all;
                self.rebuild();
            }
            KeyCode::Char('F') if table => {
                self.favs = !self.favs;
                self.rebuild();
            }
            _ => {}
        }
        None
    }

    /// Keys while typing a search, a note or a bound.
    fn input_key(&mut self, code: KeyCode, mods: KeyModifiers) -> Option<Effect> {
        match &mut self.input {
            Input::Search { cur } => {
                match code {
                    KeyCode::Enter => self.input = Input::None,
                    KeyCode::Esc => {
                        self.query.clear();
                        self.input = Input::None;
                    }
                    KeyCode::Down | KeyCode::Up => {
                        self.move_by(if code == KeyCode::Down { 1 } else { -1 });
                        return None;
                    }
                    _ if edit(&mut self.query, cur, code, mods, |_| true) => {}
                    _ => return None,
                }
                self.rebuild();
            }
            Input::Note { text, cur } => match code {
                KeyCode::Enter => {
                    let text = std::mem::take(text);
                    self.input = Input::None;
                    let key = self.current()?.key.clone();
                    self.store.set_note(&key, &text);
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
                match code {
                    KeyCode::Down => *sel = (*sel + 1).min(last),
                    KeyCode::Up => *sel = sel.saturating_sub(1),
                    KeyCode::Enter => {
                        let (col, i) = (*col, rows[*sel]);
                        let name = std::mem::take(&mut items[i].0);
                        self.input = Input::None;
                        if col == 1 || col == VIA {
                            let list = if col == 1 { &mut self.dev } else { &mut self.via };
                            list.clear();
                            if i > 0 {
                                list.push(name);
                            }
                        } else {
                            // A price level replaces the Price maximum; "any" drops it.
                            self.bounds.retain(|&(c, lo, _)| c != PRICE || lo.is_finite());
                            if i > 0 {
                                self.bounds.push((PRICE, f64::NEG_INFINITY, LEVELS[i - 1]));
                            }
                        }
                        self.rebuild();
                    }
                    // Space adds or drops the entry, keeping the dropdown open; on "any" it drops all.
                    KeyCode::Char(' ') if *col == 1 || *col == VIA => {
                        let i = rows[*sel];
                        let list = if *col == 1 { &mut self.dev } else { &mut self.via };
                        match list.iter().position(|d| *d == items[i].0) {
                            _ if i == 0 => list.clear(),
                            Some(k) => drop(list.remove(k)),
                            None => list.push(items[i].0.clone()),
                        }
                        self.rebuild();
                    }
                    // Esc while searching drops the search but keeps the entry under the bar.
                    KeyCode::Esc if *typing => {
                        (*sel, *typing, *cur) = (rows[*sel], false, 0);
                        query.clear();
                    }
                    _ if *typing && edit(query, cur, code, mods, |_| true) => {
                        // The first match, so that enter picks it.
                        *sel = menu_rows(items, query).len().min(2) - 1;
                    }
                    KeyCode::Char('/') => *typing = true,
                    KeyCode::Char('j') => *sel = (*sel + 1).min(last),
                    KeyCode::Char('k') => *sel = sel.saturating_sub(1),
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
            Input::None => {}
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::Offer;

    fn model(key: &str, available: bool, coding: Option<f64>, price: f64) -> Model {
        let mut m = Model {
            key: key.into(),
            name: key.into(),
            available,
            url: format!("https://x/{key}"),
            eci: coding.map(|c| c + 100.0),
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
        assert_eq!(menu(&a), [("any", 3), ("openai", 2), ("anthropic", 1)]);
        press(&mut a, "j");
        code(&mut a, KeyCode::Enter);
        assert_eq!((a.dev.as_slice(), keys(&a)), (&["openai".to_string()][..], vec!["gpt55", "mini"]));
        // Counts follow the developer picked; the price levels are maxima.
        press(&mut a, "ld");
        assert_eq!(menu(&a), [("any", 2), ("free", 0), ("≤$0.5", 0), ("≤$2", 1), ("≤$5", 1), ("≤$15", 2)]);
        press(&mut a, "jjj");
        code(&mut a, KeyCode::Enter);
        assert_eq!(keys(&a), ["mini"]);
        // Reopening starts on the level in effect; esc leaves it alone, "any" drops it.
        press(&mut a, "d");
        assert!(matches!(a.input, Input::Menu { sel: 3, .. }));
        code(&mut a, KeyCode::Esc);
        assert_eq!(keys(&a), ["mini"]);
        press(&mut a, "dkkk");
        code(&mut a, KeyCode::Enter);
        assert_eq!((a.bounds.len(), keys(&a).len()), (0, 2));
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
        assert_eq!((a.via.as_slice(), keys(&a)), (&["claude".to_string()][..], vec!["opus5"]));
        press(&mut a, "c");
        assert_eq!((a.via.len(), keys(&a).len()), (0, 3));
    }

    #[test]
    fn x_launches_the_via_filter_harness_else_the_first() {
        let mut a = app();
        for m in &mut a.data.models {
            let via = m.via.clone();
            m.offers[0].via = via;
        }
        let cmd = |h: &str, id: &str| Some(Effect::Launch(vec![h.into(), "--model".into(), id.into()]));
        assert_eq!(press(&mut a, "x"), cmd("codex", "gpt55"), "first row, gpt55, first in Via");
        a.via = vec!["opencode".into()];
        assert_eq!(press(&mut a, "x"), cmd("opencode", "p/gpt55"), "opencode takes provider/model");
        press(&mut a, "j");
        assert_eq!(press(&mut a, "x"), cmd("claude", "opus5"), "a filter the model lacks falls back");
        press(&mut a, "a");
        a.via.clear();
        a.rebuild();
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
        assert_eq!(a.dev, ["openai"], "enter picks the first match");
        press(&mut a, "d/anth");
        code(&mut a, KeyCode::Backspace);
        code(&mut a, KeyCode::Esc);
        assert!(
            matches!(&a.input, Input::Menu { query, sel: 2, typing: false, .. } if query.is_empty()),
            "esc drops the search, the bar stays on anthropic"
        );
        press(&mut a, "/zzz");
        assert!(matches!(a.input, Input::Menu { sel: 0, .. }), "no match leaves only any");
        code(&mut a, KeyCode::Enter);
        assert!(a.dev.is_empty());
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
    fn space_toggles_several_dropdown_entries() {
        let mut a = app();
        a.col = 1;
        press(&mut a, "dj ");
        assert!(matches!(a.input, Input::Menu { .. }), "the dropdown stays open");
        assert_eq!((a.dev.as_slice(), keys(&a)), (&["openai".to_string()][..], vec!["gpt55", "mini"]));
        press(&mut a, "j ");
        assert_eq!(keys(&a), ["gpt55", "opus5", "mini"], "both developers show");
        press(&mut a, "k ");
        assert_eq!((a.dev.as_slice(), keys(&a)), (&["anthropic".to_string()][..], vec!["opus5"]));
        press(&mut a, "/open ");
        assert_eq!(a.dev, ["anthropic", "openai"], "space toggles while searching too");
        code(&mut a, KeyCode::Esc);
        press(&mut a, "k ");
        assert!(a.dev.is_empty(), "space on any drops them all");
        press(&mut a, "jj");
        code(&mut a, KeyCode::Enter);
        assert_eq!((a.dev.as_slice(), &a.input), (&["anthropic".to_string()][..], &Input::None), "enter picks one");
    }

    #[test]
    fn row_keys_do_nothing_behind_help_and_compare() {
        let mut a = app();
        press(&mut a, "?");
        assert_eq!(press(&mut a, "y"), None);
        press(&mut a, "n f ");
        assert_eq!((&a.input, a.marked.len()), (&Input::None, 0));
        assert!(!a.store.is_fav(&a.current().unwrap().key.clone()));
    }

    #[test]
    fn shows_available_unless_all() {
        let mut a = app();
        assert_eq!(a.rows.len(), 3);
        press(&mut a, "a");
        assert_eq!(a.rows.len(), 4);
    }

    #[test]
    fn starts_by_price_and_blanks_sink_either_way() {
        let mut a = app();
        assert_eq!((a.sort_col, a.descending), (PRICE, true));
        assert_eq!(keys(&a), ["gpt55", "opus5", "mini"]);
        press(&mut a, "s");
        assert!(!a.descending, "the cursor starts on Price, so s reverses");
        assert_eq!(keys(&a), ["mini", "opus5", "gpt55"]);
        a.col = ECI;
        press(&mut a, "s");
        assert_eq!(keys(&a), ["gpt55", "mini", "opus5"]);
        press(&mut a, "s");
        assert_eq!(keys(&a), ["mini", "gpt55", "opus5"]);
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
        assert!(col_about(0).contains("no available provider"));
        assert!(col_about(1).contains("trained"));
        assert!(COLS.iter().all(|c| !c.about.is_empty()));
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
        assert_eq!(a.current().unwrap().key, "mini");
        press(&mut a, "3gg");
        assert_eq!(a.current().unwrap().key, "llama4");
        press(&mut a, "99j");
        assert_eq!(a.current().unwrap().key, "mini");
        ctrl(&mut a, 'u');
        assert_eq!(a.current().unwrap().key, "gpt55");
        press(&mut a, "15l");
        assert_eq!(a.col, PRICE, "counted column moves wrap around");
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
        let mut a = app();
        a.col = ECI;
        press(&mut a, "alp");
        assert_eq!(a.frontier, Some(a.col));
        assert_eq!((a.sort_col, a.descending), (PRICE, false), "cheapest first, each row down better");
        // mini (60 @ 1) beats llama4 (40 @ 1); gpt55 (80 @ 10) is the best, so it stays.
        assert_eq!(keys(&a), ["mini", "gpt55"]);
        press(&mut a, "p");
        assert_eq!(a.frontier, None);
        press(&mut a, "hhhhp");
        assert_eq!(a.frontier, None, "no frontier on a price column");
        assert!(a.status.starts_with("p needs a benchmark column: ECI, Coding"), "{}", a.status);
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
        assert_eq!(a.input, Input::Search { cur: 4 });
        assert_eq!(a.rows.len(), 1);
        code(&mut a, KeyCode::Enter);
        assert_eq!(a.input, Input::None);
        assert_eq!(a.query, "opus");
        assert_eq!(code(&mut a, KeyCode::Esc), None, "esc clears the filter first");
        assert_eq!(a.rows.len(), 3);
        assert_eq!(code(&mut a, KeyCode::Esc), None, "only q quits");
        assert_eq!(press(&mut a, "qq"), Some(Effect::Quit));
    }

    #[test]
    fn picking_a_task_ranks_the_table() {
        let mut a = app();
        press(&mut a, "tjj");
        assert_eq!(a.task_cur, 2);
        press(&mut a, "G");
        assert_eq!(a.task_cur, TASKS.len() - 1);
        press(&mut a, "gg");
        assert_eq!(a.task_cur, 0, "the cursor stays inside the list");
        let front: Vec<&str> = a.task_frontier(&TASKS[0]).iter().map(|(m, _)| m.key.as_str()).collect();
        assert_eq!(front, ["mini", "gpt55"], "cheapest first, the best last; llama4 has no access");
        code(&mut a, KeyCode::Enter);
        assert_eq!(a.view, View::Table);
        assert_eq!(a.task.map(|t| t.name), Some("coding"));
        assert_eq!(keys(&a), ["gpt55", "mini"], "best coder first, models without the score dropped");
        assert_eq!(a.selected(), 0);
        assert!(a.status.starts_with("best for coding: gpt55"), "{}", a.status);
        press(&mut a, "c");
        assert!(a.task.is_none());
        assert_eq!(a.rows.len(), 3, "c restores the column sort");
    }

    #[test]
    fn t_toggles_the_tasks_overlay() {
        let mut a = app();
        press(&mut a, "t");
        assert_eq!(a.view, View::Tasks);
        press(&mut a, "t");
        assert_eq!(a.view, View::Table, "t again closes it");
        press(&mut a, "t");
        code(&mut a, KeyCode::Esc);
        assert_eq!(a.view, View::Table, "esc closes it too");
    }

    #[test]
    fn mark_only_marked_and_compare() {
        let mut a = app();
        assert_eq!(press(&mut a, "C"), None);
        assert_eq!(a.view, View::Table);
        press(&mut a, "mm");
        assert_eq!(a.marked, vec!["gpt55", "opus5"]);
        press(&mut a, "M");
        assert_eq!(keys(&a), ["gpt55", "opus5"]);
        press(&mut a, "M");
        assert_eq!(a.rows.len(), 3);
        press(&mut a, "C");
        assert_eq!(a.view, View::Compare);
        assert_eq!(a.input, Input::None);
        press(&mut a, "q");
        assert_eq!(a.input, Input::Quit, "q asks to quit from any view");
        code(&mut a, KeyCode::Esc);
        assert_eq!(a.view, View::Compare, "any other key only cancels the question");
        code(&mut a, KeyCode::Esc);
        assert_eq!(a.view, View::Table, "esc closes the overlay");
        press(&mut a, "Mc");
        assert!(a.marked.is_empty() && !a.only_marked, "c clears the marks too");
        assert_eq!(a.rows.len(), 3);
        press(&mut a, "ggmmGf");
        press(&mut a, "MF");
        assert_eq!(keys(&a), ["gpt55", "opus5", "mini"], "m and F together: marked or favorite");
        press(&mut a, "F");
        assert_eq!(keys(&a), ["gpt55", "opus5"], "F off again: marked only");
    }

    #[test]
    fn favorites_notes_copy_open() {
        let mut a = app();
        assert_eq!(press(&mut a, "f"), Some(Effect::Save));
        assert!(a.store.is_fav("gpt55"));
        assert_eq!(press(&mut a, "nfast"), None);
        assert_eq!(code(&mut a, KeyCode::Enter), Some(Effect::Save));
        assert_eq!(a.store.note("gpt55"), Some("fast"));
        assert_eq!(press(&mut a, "o"), Some(Effect::Open("https://x/gpt55".into())));
        assert_eq!(press(&mut a, "y"), Some(Effect::Copy("p/gpt55".into())));
        a.data.models[0].name = "GPT 5.5".into();
        assert_eq!(press(&mut a, "Y"), Some(Effect::Copy("GPT 5.5".into())));
        press(&mut a, "F");
        assert_eq!(keys(&a), ["gpt55"], "F shows favorites only");
        press(&mut a, "c");
        assert_eq!(a.rows.len(), 3, "c clears it");
    }

    #[test]
    fn refresh_once_at_a_time() {
        let mut a = app();
        assert_eq!(press(&mut a, "r"), Some(Effect::Refresh));
        assert!(a.refreshing && a.status.is_empty(), "only the ⟳ indicator says so");
        assert_eq!(press(&mut a, "r"), None);
        a.refreshed(Err("offline".into()));
        assert_eq!(a.status, "refresh failed: offline");
        assert_eq!(press(&mut a, "r"), Some(Effect::Refresh));
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
}
