//! Terminal shell: owns the terminal, the input loop and the refresh thread, and draws `App`.
//! Redraws only on input or when a background refresh lands.

//!
//! The table is drawn straight into the buffer: one pass over the visible rows, no widget
//! allocations per frame. Colours come from the terminal's 16-colour palette, so a themed
//! terminal themes modelcmp too, and nothing paints a background over a transparent one.

use crate::app::{
    App, COLS, ECI, Effect, GROUPS, HELP, Input, Mouse, NCOLS, NOTES, PRICE, VIA, View, choice_rows, col_about,
    col_name, has_menu, menu_rows,
};
use crate::data::{self, Data, Model};
use crate::fit::{self, TASKS};
use crate::store::Store;
use crate::view::{
    Palette, THEMES, age, compare_rows, detail_lines, frontier_legend, hits, level, money, priced, truncate, verdict,
};
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEvent,
    MouseEventKind,
};
use ratatui::crossterm::execute;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Widget};
use ratatui::{DefaultTerminal, Frame};
use std::ops::Range;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::Duration;

type Refresh = Receiver<Result<Data, String>>;

pub fn run(force: bool) -> Result<(), String> {
    let cached = if force { None } else { data::load_cache() };
    let data = match cached {
        Some(d) => d,
        None => {
            eprintln!("downloading model data (models.dev + Epoch AI)…");
            data::load(true)?.0
        }
    };
    let mut app = App::new(data, Store::load());
    let mut rx = None;
    if app.data.stale() && app.refresh().is_some() {
        rx = Some(spawn_refresh());
    }
    let mut terminal = ratatui::init();
    let _ = execute!(std::io::stdout(), EnableMouseCapture);
    let res = event_loop(&mut app, &mut terminal, rx);
    let _ = execute!(std::io::stdout(), DisableMouseCapture);
    ratatui::restore();
    res
}

fn spawn_refresh() -> Refresh {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(data::refresh());
    });
    rx
}

fn event_loop(app: &mut App, terminal: &mut DefaultTerminal, mut rx: Option<Refresh>) -> Result<(), String> {
    let mut dirty = true;
    loop {
        if dirty {
            terminal
                .draw(|f| {
                    draw(app, f);
                    let name = app.theme_preview().unwrap_or(&app.store.theme);
                    recolor(f.buffer_mut(), THEMES[crate::view::theme(name)].1.as_ref());
                })
                .map_err(|e| e.to_string())?;
        }
        // Block on input; wake every 200ms while a refresh is in flight, else once a minute to
        // repaint the data age in the frame.
        let timeout = if rx.is_some() { Duration::from_millis(200) } else { Duration::from_secs(60) };
        dirty = rx.is_none();
        // Handle every queued event before the next draw, so a held key never falls behind.
        let mut wait = timeout;
        while event::poll(wait).map_err(|e| e.to_string())? {
            wait = Duration::ZERO;
            // An agent may have marked or noted a model meanwhile: act on its file, not a stale copy.
            if app.store.reload_if_changed() {
                app.rebuild();
            }
            let effect = match event::read().map_err(|e| e.to_string())? {
                Event::Key(k) if k.kind == KeyEventKind::Press => app.key(k),
                Event::Mouse(m) => {
                    let size = terminal.size().map_err(|e| e.to_string())?;
                    hit(app, Rect::new(0, 0, size.width, size.height), m).and_then(|m| app.mouse(m))
                }
                _ => None,
            };
            {
                match effect {
                    Some(Effect::Quit) => return Ok(()),
                    Some(Effect::Save) => {
                        if let Err(e) = app.store.save() {
                            app.report(Err(format!("could not save: {e}")));
                        }
                    }
                    Some(Effect::Open(url)) => app.report(match open::that_detached(&url) {
                        Ok(()) => Ok(format!("opened {url}")),
                        Err(e) => Err(format!("could not open {url}: {e}")),
                    }),
                    Some(Effect::Copy(text)) => app.report(if copy(&text) {
                        Ok(format!("copied {text}"))
                    } else {
                        Err("no clipboard tool found".into())
                    }),
                    Some(Effect::Refresh) => rx = Some(spawn_refresh()),
                    Some(Effect::Launch(cmd)) => {
                        let line = cmd.join(" ");
                        app.report(match new_terminal(&cmd) {
                            Ok(()) => Ok(format!("opened {line} in a new terminal")),
                            Err(e) => Err(format!("could not open a terminal for {line}: {e}; set $TERMINAL")),
                        });
                    }
                    // The app applies its own chooser items before they get here.
                    Some(Effect::Fav(_) | Effect::Theme(_)) | None => {}
                }
            }
            dirty = true;
        }
        let done = rx.as_ref().and_then(|r| match r.try_recv() {
            Ok(res) => Some(res),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err("refresh thread died".into())),
        });
        if let Some(res) = done {
            rx = None;
            app.refreshed(res);
            dirty = true;
        }
    }
}

/// What a mouse event lands on, with the same geometry `draw` uses: the frame's inner area
/// holds the header and then the rows from `app.table.offset()`.
fn hit(app: &App, area: Rect, m: MouseEvent) -> Option<Mouse> {
    match m.kind {
        MouseEventKind::ScrollDown => return Some(Mouse::Scroll(3)),
        MouseEventKind::ScrollUp => return Some(Mouse::Scroll(-3)),
        MouseEventKind::ScrollRight => return Some(Mouse::Cols(1)),
        MouseEventKind::ScrollLeft => return Some(Mouse::Cols(-1)),
        MouseEventKind::Down(MouseButton::Left | MouseButton::Right) | MouseEventKind::Drag(MouseButton::Left) => {}
        _ => return None,
    }
    // Selection works as in a file manager: a click replaces, shift extends, ctrl adds one.
    let mark = m.kind == MouseEventKind::Down(MouseButton::Right);
    let pick = m.modifiers.contains(KeyModifiers::CONTROL);
    let extend = m.kind == MouseEventKind::Drag(MouseButton::Left) || m.modifiers.contains(KeyModifiers::SHIFT);
    if area.height < 4 || area.width < 4 {
        return None;
    }
    let pos = ratatui::layout::Position::new(m.column, m.row);
    let inner = Rect::new(1, 1, area.width - 2, area.height - 3);
    let l = layout(inner.width, app);
    // An open list first: a click on an entry acts on it, on its frame nothing, and any click
    // outside closes it.
    let list = match &app.input {
        Input::Menu { col, items, sel, query, .. } => {
            let rows = menu_rows(items, query);
            menu_box(inner, menu_x(inner, &l, *col), items, rows.len()).map(|(b, _)| (b, *sel, rows.len()))
        }
        Input::Choose { title, items, sel, query, .. } => {
            let rows = choice_rows(items, query).len();
            Some((
                overlay_rect(Rect { height: area.height - 1, ..area }, title, &choice_lines(items, *sel, query)),
                *sel,
                rows,
            ))
        }
        _ => None,
    };
    if let Some((rect, sel, len)) = list {
        if extend {
            return None;
        }
        if !rect.contains(pos) {
            return Some(Mouse::Outside);
        }
        let inner = rect.inner(ratatui::layout::Margin::new(1, 1));
        if !inner.contains(pos) {
            return None;
        }
        let top = sel.saturating_sub(inner.height as usize - 1);
        let k = top + (m.row - inner.y) as usize;
        return (k < len).then_some(Mouse::Item(k));
    }
    // A hint in the status bar presses its key.
    if m.row == area.bottom() - 1 {
        return (!mark && !extend).then(|| hint_at(app, area.width, m.column).map(Mouse::Key)).flatten();
    }
    // A drag past the table's edges still extends the range to its nearest row.
    if extend {
        if inner.height < 3 {
            return None;
        }
        let y = m.row.clamp(inner.y + 2, inner.bottom() - 1);
        return Some(Mouse::Extend(app.table.offset() + (y - inner.y - 2) as usize));
    }
    if !inner.contains(pos) {
        return None;
    }
    // The rule under the header.
    if m.row == inner.y + 1 {
        return None;
    }
    if m.row > inner.y {
        let n = app.table.offset() + (m.row - inner.y - 2) as usize;
        // The checkbox right of the row number toggles the mark, the ☆ after it picks the
        // tasks, and the ✗ box after that excludes the model.
        let (num_w, x) = (l.name_x - 7, m.column - inner.x);
        return Some(if mark {
            Mouse::Mark(n)
        } else if (num_w + 1..num_w + 3).contains(&x) {
            Mouse::Box(n)
        } else if (num_w + 3..num_w + 5).contains(&x) {
            Mouse::Star(n)
        } else if (num_w + 5..num_w + 7).contains(&x) {
            Mouse::Exclude(n)
        } else if pick {
            Mouse::Pick(n)
        } else {
            Mouse::Row(n)
        });
    }
    let x = m.column - inner.x;
    // The # header goes to the first row, as `gg` does; the ✓ header shows marked models only,
    // as `M` does, the ★ favorites only, as `F`, and the ✗ excluded only, as `E`.
    if x < l.name_x {
        let num_w = l.name_x - 7;
        return match x {
            _ if x <= num_w => Some(Mouse::Top),
            _ if (num_w + 1..num_w + 3).contains(&x) => Some(Mouse::OnlyMarked),
            _ if (num_w + 3..num_w + 5).contains(&x) => Some(Mouse::OnlyFav),
            _ if (num_w + 5..num_w + 7).contains(&x) => Some(Mouse::OnlyExcluded),
            _ => None,
        };
    }
    let dev_x = l.name_x + l.name_w + GAP;
    let (col, cx, w) = if x < dev_x {
        (0, l.name_x, l.name_w)
    } else if x < dev_x + l.dev_w {
        (1, dev_x, l.dev_w)
    } else if let Some(&(i, cx, w)) = l.cols.iter().find(|&&(_, cx, w)| (cx..cx + w).contains(&x)) {
        (i + 2, cx, w)
    } else if let Some((vx, w)) = l.via.filter(|&(vx, w)| (vx..vx + w).contains(&x)) {
        (VIA, vx, w)
    } else if let Some((nx, w)) = l.notes.filter(|&(nx, w)| (nx..nx + w).contains(&x)) {
        (NOTES, nx, w)
    } else {
        return None;
    };
    // The ▾ ends a left-aligned text header ("Dev▼ ▾") and is the last cell of a numeric one.
    let arrow = if col == 1 || col == VIA { cx + 4 + u16::from(app.sort_col == col) } else { cx + w - 1 };
    Some(if has_menu(col) && x >= arrow { Mouse::Menu(col) } else { Mouse::Header(col) })
}

/// Start `cmd` in a new terminal window here, without waiting: Windows Terminal under WSL,
/// else `$TERMINAL -e`, else x-terminal-emulator.
fn new_terminal(cmd: &[String]) -> std::io::Result<()> {
    let mut term = if let Ok(distro) = std::env::var("WSL_DISTRO_NAME") {
        // A new WSL session starts bare, so a login shell sets up PATH and keys as for a typed
        // command. Model ids hold no `;`, which Windows Terminal would split the command on.
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
        let line: Vec<String> = cmd.iter().map(|a| format!("'{}'", a.replace('\'', r"'\''"))).collect();
        let mut c = Command::new("wt.exe");
        c.args(["new-tab", "wsl.exe", "-d", &distro, "--cd"]).arg(std::env::current_dir()?);
        c.args(["-e", &shell, "-lic", &format!("exec {}", line.join(" "))]);
        c
    } else {
        let mut c = Command::new(std::env::var("TERMINAL").unwrap_or_else(|_| "x-terminal-emulator".into()));
        c.arg("-e").args(cmd);
        c
    };
    let mut child = term.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()?;
    // Reap it whenever it exits.
    std::thread::spawn(move || child.wait());
    Ok(())
}

/// Pipe `text` into the first clipboard tool that runs.
fn copy(text: &str) -> bool {
    let tools: &[&[&str]] = if cfg!(target_os = "macos") {
        &[&["pbcopy"]]
    } else {
        &[&["clip.exe"], &["wl-copy"], &["xclip", "-selection", "clipboard"], &["xsel", "--clipboard", "--input"]]
    };
    tools.iter().any(|argv| {
        let mut tool = Command::new(argv[0]);
        tool.args(&argv[1..]).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null());
        let Ok(mut child) = tool.spawn() else { return false };
        let written =
            child.stdin.take().is_some_and(|mut s| std::io::Write::write_all(&mut s, text.as_bytes()).is_ok());
        written && child.wait().is_ok_and(|s| s.success())
    })
}

// ---------- drawing ----------

/// Blank columns between two table columns.
const GAP: u16 = 2;
const NAME_MIN: u16 = 16;
/// Between two groups of hints in the status bar, drawn muted.
const SEP: &str = "│";
/// The actions on one model, in the one order every view shows them in.
const ACTIONS: [&str; 8] =
    ["enter details", "x launch", "o open", "y copy id", "m mark", "f fav", "e exclude", "n note"];

/// The last group of every overlay: `? help` is the last hint a narrow terminal drops.
const BACK: [&str; 3] = ["esc back", "q quit", "? help"];

/// The model actions a view offers, in `ACTIONS` order whatever order they are asked in.
fn actions(keys: &str) -> Vec<&'static str> {
    ACTIONS.into_iter().filter(|a| keys.split(' ').any(|k| a.split(' ').next() == Some(k))).collect()
}

/// Key reminders in the status bar, in groups every view keeps in the same order: moving,
/// the view's own keys, the model's actions, then the way back and help. Narrow terminals
/// drop them from the front, so the actions outlast the view's keys and `? help` goes last.
/// A toggle names what pressing it does; `M F E` show only once they would change
/// something, the rest of the keys are in `?`. `s` acts on the column picked with `h l`.
fn hints(app: &App) -> Vec<&'static str> {
    let groups: Vec<Vec<&'static str>> = match app.view {
        View::Table if app.selecting() => {
            vec![vec!["j k G extend"], vec!["C compare"], actions("m f e"), vec!["esc cancel", "q quit", "? help"]]
        }
        View::Table => {
            let mut view = vec!["/ filter", "s sort"];
            if has_menu(app.col) {
                view.push("d dropdown");
            }
            view.push("R recommend");
            if app.marked_models().len() >= 2 {
                view.push("C compare");
            }
            if app.only_marked {
                view.push("M every model");
            } else if app.any_marked() {
                view.push("M marked only");
            }
            if app.only_fav {
                view.push("F every model");
            } else if app.any_fav() {
                view.push("F favorites only");
            }
            if app.only_excluded {
                view.push("E every model");
            } else if !app.store.excluded.is_empty() {
                view.push("E excluded only");
            }
            view.push(if app.all { "a yours only" } else { "a all" });
            // On the price columns, or anywhere while it is off the default.
            let cached = data::cached() > 0.0;
            if (PRICE..ECI).contains(&app.col) || !cached {
                view.push(if cached { "% no cache" } else { app.cache_hint });
            }
            if !app.query.is_empty()
                || !app.bounds.is_empty()
                || !app.dev.is_empty()
                || !app.via.is_empty()
                || app.task.is_some()
                || app.only_marked
                || app.only_fav
                || app.only_excluded
            {
                view.push("c clear");
            }
            if stale(app) {
                view.push("r refresh");
            }
            let mut back = vec![];
            // Where esc goes back from, as in the overlays.
            if !app.query.is_empty() || app.only_marked || app.only_fav || app.only_excluded || app.task.is_some() {
                back.push("esc back");
            }
            back.extend(["q quit", "? help"]);
            vec![vec!["h l column"], view, actions("enter x o y m f e n"), back]
        }
        View::Detail => vec![vec!["j k scroll"], actions("x o y m f e n"), BACK.to_vec()],
        // `?` here closes help, which `esc back` already says.
        View::Help => vec![vec!["j k scroll"], vec!["/ search"], vec!["esc back", "q quit"]],
        View::Compare if app.marked_models().len() < 2 => vec![BACK.to_vec()],
        View::Compare => {
            vec![vec!["j k scroll", "h l 0 $ model"], vec!["/ rows"], actions("x o y f e n"), BACK.to_vec()]
        }
        View::Recommend => vec![
            vec!["j k task", "h l 0 $ model"],
            vec!["enter best models first"],
            actions("x o y m f e n"),
            BACK.to_vec(),
        ],
    };
    let groups: Vec<_> = groups.into_iter().filter(|g| !g.is_empty()).collect();
    groups.join(&SEP)
}

/// A hint's keys and what they do, which keeps its leading space: the keys are the leading
/// words of one character, or `enter` or `esc`.
fn split_hint(hint: &str) -> (&str, &str) {
    let mut end = 0;
    for word in hint.split(' ') {
        if word.chars().count() != 1 && word != "enter" && word != "esc" {
            break;
        }
        end += word.len() + 1;
    }
    hint.split_at(end.saturating_sub(1).min(hint.len()))
}

/// The key a click on a hint presses: only a hint with a single key has one.
fn hint_key(hint: &str) -> Option<KeyCode> {
    match split_hint(hint).0 {
        "enter" => Some(KeyCode::Enter),
        "esc" => Some(KeyCode::Esc),
        k => {
            let mut chars = k.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => Some(KeyCode::Char(c)),
                _ => None,
            }
        }
    }
}

/// The hints that fit a status bar `width` wide and the column the first starts at.
fn hint_layout(app: &App, width: u16) -> (Vec<&'static str>, u16) {
    // The pill, a space, then each part and its " · ".
    let left = mode(app).0.chars().count() + 3 + parts(app).iter().map(|p| p.width() + 3).sum::<usize>();
    let all = hints(app);
    let mut hints = &all[..];
    while hints.len() > 1 && (hints[0] == SEP || left + hints.join("  ").chars().count() + 1 > width as usize) {
        hints = &hints[1..];
    }
    (hints.to_vec(), width.saturating_sub(hints.join("  ").chars().count() as u16))
}

/// A click on the status bar at column `x` presses the key of the hint under it, as in tuiman.
fn hint_at(app: &App, width: u16, x: u16) -> Option<KeyCode> {
    if app.input != Input::None {
        return None;
    }
    let (hints, mut start) = hint_layout(app, width);
    for hint in hints {
        let w = hint.chars().count() as u16;
        if (start..start + w).contains(&x) {
            return (hint != SEP).then(|| hint_key(hint)).flatten();
        }
        start += w + 2;
    }
    None
}

/// Data past the cache's 24h with no refresh under way: the status bar says how old, and
/// hints `r`. Only the age counts: a stale format or benchmark list refreshes at start.
fn stale(app: &App) -> bool {
    !app.refreshing && app.data.age() > data::MAX_AGE
}

// The terminal's own palette, so the colours are whatever the rice set. Text stays the default foreground.
const ACCENT: Color = Color::Magenta;
const KEY: Color = Color::Cyan;
const MUTED: Color = Color::DarkGray;
const GOOD: Color = Color::Green;
const BAD: Color = Color::Red;
/// A marked row's ✓. Light blue, and the only thing drawn in it, so a palette's slot 12 is
/// free to be whatever parts from the muted ☐ (`the_mark_parts_from_an_empty_box`).
const MARK: Color = Color::LightBlue;
/// A favorite's ★ with no task at hand: gold, as stars are in mail clients and on GitHub.
const STAR: Color = Color::Yellow;
/// One colour per task in `TASKS` order: the ★ of its favorite and its name in recommend. Off the mark colour (✓ light blue), the key hints' cyan, the worst
/// value's red and yellow for a match; 16 colours leave no room to also skip the best's green.
const TASK: [Color; 7] = [
    Color::LightCyan,
    Color::Green,
    Color::LightGreen,
    Color::Blue,
    Color::LightMagenta,
    Color::LightYellow,
    Color::LightRed,
];
/// What a search matched, as the filter in the status bar.
const MATCH: Color = Color::Yellow;
/// Developers' and harnesses' colours in the terminal's own theme; the five harness names all differ.
const DEVS: [Color; 5] = [Color::Blue, Color::Yellow, Color::Cyan, Color::Magenta, Color::Green];
/// Price levels (`view::LEVELS`) from free to the most expensive.
const LEVEL: [Color; 6] = [Color::Green, Color::Green, Color::Cyan, Color::Yellow, Color::Red, Color::Magenta];
const BOLD: Modifier = Modifier::BOLD;

/// Swap the terminal colours for the theme's after drawing, so the drawing code keeps naming
/// terminal colours: the 16, default text, and a developer's placeholder (see `dev_color`).
/// Backgrounds with no colour get the theme's; with the terminal's own colours they stay transparent.
fn recolor(buf: &mut Buffer, palette: Option<&Palette>) {
    let bg = palette.map(|p| Color::from_u32(p.bg));
    for cell in &mut buf.content {
        // A pill is the only black text (see `pill`), and the dim half of a palette is too close
        // to its own background to read on: give it the theme's background as its text and the
        // half of its fill colour that stands furthest from it, which is dark in a light theme.
        if let (Color::Black, Some(p)) = (cell.fg, palette) {
            cell.fg = Color::from_u32(p.bg);
            cell.bg = contrasting(cell.bg, p);
            continue;
        }
        cell.fg = match (cell.fg, palette) {
            (Color::Reset, Some(p)) => Color::from_u32(p.text),
            (c, p) => resolve(c, p),
        };
        cell.bg = match (cell.bg, bg) {
            (Color::Reset, Some(b)) => b,
            (c, _) => resolve(c, palette),
        };
    }
}

/// The brighter or the dimmer half of a drawn colour, whichever is furthest from the theme's
/// background in perceived brightness, so a solid fill reads in a dark and a light theme alike.
fn contrasting(c: Color, p: &Palette) -> Color {
    let lum =
        |c: u32| 0.2126 * f64::from((c >> 16) & 255) + 0.7152 * f64::from((c >> 8) & 255) + 0.0722 * f64::from(c & 255);
    let Some(i) = ansi(c) else { return resolve(c, Some(p)) };
    let (dim, bright) = (p.ansi[i % 8], p.ansi[i % 8 + 8]);
    Color::from_u32(if (lum(dim) - lum(p.bg)).abs() > (lum(bright) - lum(p.bg)).abs() { dim } else { bright })
}

/// A drawn colour in the theme's palette, or the terminal's own.
fn resolve(c: Color, palette: Option<&Palette>) -> Color {
    match (c, palette) {
        (Color::Indexed(k), Some(p)) => Color::from_u32(p.accents[k as usize % p.accents.len()]),
        (Color::Indexed(k), None) => DEVS[k as usize % DEVS.len()],
        (c, Some(p)) => ansi(c).map_or(c, |i| Color::from_u32(p.ansi[i])),
        (c, None) => c,
    }
}

/// The ANSI slot a named terminal colour sits in, for a palette lookup.
fn ansi(c: Color) -> Option<usize> {
    Some(match c {
        Color::Black => 0,
        Color::Red => 1,
        Color::Green => 2,
        Color::Yellow => 3,
        Color::Blue => 4,
        Color::Magenta => 5,
        Color::Cyan => 6,
        Color::Gray => 7,
        Color::DarkGray => 8,
        Color::LightRed => 9,
        Color::LightGreen => 10,
        Color::LightYellow => 11,
        Color::LightBlue => 12,
        Color::LightMagenta => 13,
        Color::LightCyan => 14,
        Color::White => 15,
        _ => return None,
    })
}

const fn fg(c: Color) -> Style {
    Style::new().fg(c)
}

/// A developer's or harness's colour, as a placeholder `recolor` resolves to one of the theme's
/// accents: the name's byte sum mod 210, which keeps it mod 5, 10 and 14 (`DEVS`, `Palette::accents`).
fn dev_color(dev: &str) -> Color {
    Color::Indexed((dev.bytes().map(usize::from).sum::<usize>() % 210) as u8)
}

fn task_color(task: &str) -> Color {
    TASK[TASKS.iter().position(|t| t.name == task).unwrap_or(0)]
}

fn draw(app: &mut App, f: &mut Frame) {
    let area = f.area();
    if area.height < 4 || area.width < 4 {
        return;
    }
    let body = Rect { height: area.height - 1, ..area };
    let bar = Rect { y: area.bottom() - 1, height: 1, ..area };
    // The refresh state is always in view: under way, failed, or how old the data is.
    let age = format!("data {} old ", age(app.data.age()));
    let (state, color) = match (app.refreshing, app.refresh_failed, app.data.stale()) {
        (true, ..) => ("⟳ refreshing ".to_string(), Color::Yellow),
        (_, true, _) => (format!("refresh failed · {age}"), BAD),
        (_, _, true) => (age, BAD),
        _ => (age, MUTED),
    };
    let sort = format!(" {} by {} ", if app.descending { "▼" } else { "▲" }, col_name(app.sort_col));
    // What the column under the cursor means, centred and cut to clear the sort on either side.
    let side = sort.chars().count() + 2;
    let room = (body.width as usize).saturating_sub(2 * side).max(1);
    let about = truncate(&format!(" {}: {} ", col_name(app.col), col_about(app.col)), room);
    let frame = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(fg(MUTED))
        .title_top(Line::from(about).style(fg(MUTED)).centered())
        .title_top(Line::from(sort).style(fg(MUTED)).right_aligned())
        .title_bottom(
            Line::from(vec![Span::styled(" models.dev + Epoch AI · ", fg(MUTED)), Span::styled(state, fg(color))])
                .right_aligned(),
        );
    let inner = frame.inner(body);
    app.page = inner.height.saturating_sub(2);
    let buf = f.buffer_mut();
    frame.render(body, buf);
    let (right, above, below) = table(buf, inner, app);
    if right {
        // Columns cut off on the right: `l` scrolls to them.
        buf.set_stringn(body.right() - 1, inner.y, "›", 1, fg(ACCENT).add_modifier(BOLD));
    }
    if inner.height > 1 {
        // The rule under the header runs into the frame.
        buf.set_stringn(body.x, inner.y + 1, "├", 1, fg(MUTED));
        buf.set_stringn(body.right() - 1, inner.y + 1, "┤", 1, fg(MUTED));
    }
    if inner.height > 2 {
        vmarks(buf, body.x, inner.y + 2, inner.bottom() - 1, above, below);
    }
    let cursor = status(buf, bar, app);
    let lines = match app.view {
        View::Table => None,
        View::Help => Some(("keys".to_string(), help(&app.overlay_query))),
        View::Recommend => {
            Some(("recommend".to_string(), recommend(app, (area.width as usize).saturating_sub(4).min(130))))
        }
        View::Detail => app.current().map(|m| (detail_lines(m, &app.store).swap_remove(0), detail(m, &app.store))),
        View::Compare if app.marked_models().len() < 2 => {
            let key = |k: &'static str| Span::styled(k, fg(KEY).add_modifier(BOLD));
            let n = app.marked_models().len();
            let lines = vec![
                Line::from(format!("compare needs 2 or more marked models, {n} now")),
                Line::from(""),
                Line::from(vec![key("esc"), Span::raw(" back to the table, then")]),
                Line::from(vec![key("m"), Span::raw(" marks the model under the bar, or")]),
                Line::from(vec![key("V"), Span::raw(" / shift+click selects a range, and")]),
                Line::from(vec![key("C"), Span::raw(" compares them")]),
            ];
            Some(("compare".into(), lines))
        }
        View::Compare => {
            let (lines, first) = compare(
                &app.marked_models(),
                app.compare_sel,
                app.compare_x,
                area.width.saturating_sub(4) as usize,
                &app.overlay_query,
            );
            app.compare_x = first;
            Some(("compare".into(), lines))
        }
    };
    if let Some((title, lines)) = lines {
        if app.view == View::Recommend {
            // Keep the cursor's block in view: it runs from the highlighted name to the next blank line.
            let start =
                lines.iter().position(|l| l.spans.iter().any(|s| s.style.add_modifier.contains(Modifier::REVERSED)));
            if let Some(start) = start {
                let end = lines[start..].iter().position(|l| l.width() == 0).map_or(lines.len(), |n| start + n);
                let shown = body.height.saturating_sub(2) as usize;
                let lo = end.saturating_sub(shown).min(start);
                app.scroll = (app.scroll as usize).clamp(lo, start) as u16;
            }
        }
        overlay(buf, body, &title, lines, &mut app.scroll);
    }
    if app.input == Input::Quit {
        let keys = |k: &'static str| Span::styled(k, fg(KEY).add_modifier(BOLD));
        let lines = vec![Line::from(vec![keys("q"), Span::raw(" confirms · any other key cancels")])];
        overlay(buf, body, "quit?", lines, &mut 0);
    }
    if let Input::Choose { title, items, sel, query, .. } = &app.input {
        // Scrolled as the mouse maps it, so the bar stays in view when the list is taller than the screen.
        let lines = choice_lines(items, *sel, query);
        let shown = overlay_rect(body, title, &lines).height.saturating_sub(2).max(1);
        overlay(buf, body, title, lines, &mut (*sel as u16).saturating_sub(shown - 1));
    }
    if let Some(x) = cursor {
        f.set_cursor_position((x, bar.y));
    }
}

/// Column layout relative to the table's left edge. A numeric column is as wide as its header
/// plus the sort arrow or its widest value, so neighbours always sit `GAP` apart whatever the filter.
struct Layout {
    /// Where Model starts: right of the row numbers, the checkbox, the ☆ and the ✗ box.
    name_x: u16,
    name_w: u16,
    dev_w: u16,
    /// The numeric columns that fit: (index into `COLS`, x, width).
    cols: Vec<(usize, u16, u16)>,
    /// Where the "Via" and "Notes" columns start and their widths, if there is room.
    via: Option<(u16, u16)>,
    notes: Option<(u16, u16)>,
    /// The first column right of Dev shown (0 is Price, the last Notes), for `App::hscroll`.
    first: usize,
    /// Columns cut off on the right; `‹` after Dev and `›` on the frame say where to scroll.
    more: bool,
    /// Where a `│` parts two groups of columns (`GROUPS`): after Dev, and before each group
    /// that starts right of it.
    seps: Vec<u16>,
}

fn layout(width: u16, app: &App) -> Layout {
    let ms = &app.data.models;
    // Headers need room for the sort arrow, and a dropdown's for its " ▾".
    let widths: [u16; COLS.len()] = std::array::from_fn(|i| {
        let c = &COLS[i];
        let head = c.name.chars().count() + 1 + if has_menu(i + 2) { 2 } else { 0 };
        head.max(app.widths[i]) as u16
    });
    let dev_w = ms.iter().map(|m| m.developer.chars().count()).max().unwrap_or(0).clamp(6, 12) as u16;
    let via_w =
        ms.iter().map(|m| m.via.iter().map(|v| v.len() + 1).sum::<usize>()).max().unwrap_or(0).clamp(6, 24) as u16;
    let notes_w = ms.iter().filter_map(|m| app.store.note(&m.key)).map(str::len).max().unwrap_or(0).clamp(6, 40) as u16;
    let longest = ms.iter().map(|m| m.name.chars().count()).max().unwrap_or(0) as u16;
    // Row numbers as wide as the last one, a space, then the checkbox, the ☆ and the ✗ box,
    // each with a spare cell: some terminals draw them two cells wide, and the
    // spare keeps that off the neighbour.
    let name_x = app.rows.len().max(1).to_string().len() as u16 + 7;
    // The columns right of Dev scroll sideways: only as far as it takes to show the selected
    // one, keeping the last position otherwise. Model and Dev stay put.
    let ws: Vec<u16> = widths.into_iter().chain([via_w, notes_w]).collect();
    // A column starting a group has a `│` in its gap, one cell wider; the first shown always
    // has one, parting it from Dev.
    let sep = |k: usize| u16::from(GROUPS.contains(&(k + 2)));
    let fixed: u16 = ws.iter().enumerate().map(|(k, w)| w + GAP + sep(k)).sum::<u16>() + dev_w + GAP;
    let name_w = width.saturating_sub(name_x + fixed).clamp(NAME_MIN, longest.max(NAME_MIN));
    let mut x = name_x + name_w + GAP + dev_w + GAP + 1;
    let room = width.saturating_sub(x) + GAP;
    let first = match app.col.checked_sub(2) {
        Some(s) => {
            let (mut lo, mut used) = (s, ws[s] + GAP);
            while lo > 0 && used + ws[lo - 1] + GAP + sep(lo) <= room {
                lo -= 1;
                used += ws[lo] + GAP + sep(lo + 1);
            }
            app.hscroll.clamp(lo, s)
        }
        None => 0,
    };
    let (mut cols, mut tail, mut more, mut seps) = (Vec::with_capacity(COLS.len()), [None; 2], false, vec![]);
    for (k, &w) in ws.iter().enumerate().skip(first) {
        let part = k == first || sep(k) == 1;
        if k > first {
            x += sep(k);
        }
        if x + w > width {
            more = true;
            break;
        }
        if part {
            seps.push(x - 2);
        }
        match k.checked_sub(COLS.len()) {
            Some(t) => tail[t] = Some((x, w)),
            None => cols.push((k, x, w)),
        }
        x += w + GAP;
    }
    Layout { name_x, name_w, dev_w, cols, via: tail[0], notes: tail[1], first, more, seps }
}

/// Header plus as many rows as fit in `area`, scrolled so the selection stays in view. Says
/// whether columns are cut off on the right and rows above and below, for the caller's border.
fn table(buf: &mut Buffer, area: Rect, app: &mut App) -> (bool, bool, bool) {
    if area.height == 0 {
        return (false, false, false);
    }
    let Layout { name_x, name_w, dev_w, cols, via, notes, first, more, seps } = layout(area.width, app);
    app.hscroll = first;
    let arrow = |i: usize| match i == app.sort_col {
        true if app.descending => "▼",
        true => "▲",
        false => "",
    };
    // The column under the cursor: its header reversed, its cells bold and a thick rule under it,
    // as VisiData shows its current column; the values keep their colours.
    let header = |i: usize| match i == app.col {
        true => fg(ACCENT).add_modifier(BOLD | Modifier::REVERSED),
        false => fg(ACCENT).add_modifier(BOLD),
    };
    // The marks: the checkbox, then the ☆, then the ✗ box.
    let num_w = (name_x - 7) as usize;
    let (box_x, star_x) = (area.x + num_w as u16 + 1, area.x + num_w as u16 + 3);
    let ex_x = area.x + num_w as u16 + 5;
    let (y, name_x, dev_x) = (area.y, area.x + name_x, area.x + name_x + name_w + GAP);
    let (nw, dw) = (name_w as usize, dev_w as usize);
    buf.set_stringn(area.x, y, format!("{:>num_w$}", "#"), num_w, fg(MUTED));
    // Each mark column labelled with its own glyph, as mail clients head a star column with a star.
    for (x, g) in [(box_x, "✓"), (star_x, "★"), (ex_x, "✗")] {
        buf.set_stringn(x, y, g, 1, fg(MUTED));
    }
    buf.set_stringn(name_x, y, format!("{:<nw$}", format!("Model{}", arrow(0))), nw, header(0));
    buf.set_stringn(dev_x, y, format!("{:<dw$}", format!("Dev{} ▾", arrow(1))), dw, header(1));
    if first > 0 {
        // Columns scrolled off to the left.
        buf.set_stringn(dev_x + dev_w, y, "‹", 1, fg(ACCENT).add_modifier(BOLD));
    }
    for &(i, x, w) in &cols {
        let text = format!("{}{}{}", arrow(i + 2), COLS[i].name, if has_menu(i + 2) { " ▾" } else { "" });
        buf.set_stringn(area.x + x, y, format!("{text:>w$}", w = w as usize), w as usize, header(i + 2));
    }
    if let Some((x, w)) = via {
        buf.set_stringn(area.x + x, y, format!("Via{} ▾", arrow(VIA)), w as usize, header(VIA));
    }
    if let Some((x, w)) = notes {
        buf.set_stringn(area.x + x, y, format!("{}{}", col_name(NOTES), arrow(NOTES)), w as usize, header(NOTES));
    }

    for &x in &seps {
        buf.set_stringn(area.x + x, y, "│", 1, fg(MUTED));
    }
    let cur = match app.col {
        0 => Some((name_x, name_w)),
        1 => Some((dev_x, dev_w)),
        VIA => via.map(|(x, w)| (area.x + x, w)),
        NOTES => notes.map(|(x, w)| (area.x + x, w)),
        c => cols.iter().find(|&&(i, ..)| i + 2 == c).map(|&(_, x, w)| (area.x + x, w)),
    };
    // A rule parts the header from the rows, crossing the group lines.
    if area.height > 1 {
        buf.set_stringn(area.x, y + 1, "─".repeat(area.width as usize), area.width as usize, fg(MUTED));
        for &x in &seps {
            buf.set_stringn(area.x + x, y + 1, "┼", 1, fg(MUTED));
        }
        if let Some((x, w)) = cur {
            buf.set_stringn(x, y + 1, "━".repeat(w as usize), w as usize, fg(ACCENT));
        }
    }

    let height = (area.height as usize).saturating_sub(2);
    let sel = app.table.selected().unwrap_or(0).min(app.rows.len().saturating_sub(1));
    // Keep the selection in view, and never leave rows blank below while some are hidden above.
    let top = app.table.offset().clamp(sel.saturating_sub(height.saturating_sub(1)), sel);
    let top = top.min(app.rows.len().saturating_sub(height));
    *app.table.offset_mut() = top;
    let ext = app.ext;
    let any = app.data.models.iter().any(|m| m.available);
    for (k, &r) in app.rows.iter().enumerate().skip(top).take(height) {
        let y = area.y + 2 + (k - top) as u16;
        let m = &app.data.models[r];
        // The selection, or the visual range, is a reverse-video bar; colours stay off it so it
        // reads as one.
        let on = k == sel || app.is_selected(k);
        let base = if on { Style::new().add_modifier(Modifier::REVERSED) } else { Style::new() };
        let tint = |c: Color| if on { base } else { fg(c) };
        buf.set_style(Rect { y, height: 1, ..area }, base);
        buf.set_stringn(area.x, y, format!("{:>num_w$}", k + 1), num_w, tint(MUTED));
        match app.store.is_marked(&m.key) {
            // Bold as well as blue: on the selection bar, which keeps colours off, that is
            // all the mark has left to show itself with.
            true => buf.set_stringn(box_x, y, "✓", 1, tint(MARK).add_modifier(BOLD)),
            false => buf.set_stringn(box_x, y, "☐", 1, tint(MUTED)),
        };
        if app.starred(&m.key) {
            // In the colour of the task at hand, as its name in recommend; gold with no task, as the ★
            // then stands for any of them. Bold so it stands out as much as the ✓.
            let star = tint(app.task_at_hand().map_or(STAR, |t| task_color(t.name)));
            buf.set_stringn(star_x, y, "★", 1, star.add_modifier(BOLD));
        } else {
            buf.set_stringn(star_x, y, "☆", 1, tint(MUTED));
        }
        let name = if m.available || !any { base } else { tint(MUTED) };
        buf.set_stringn(name_x, y, &m.name, nw, name);
        buf.set_stringn(dev_x, y, &m.developer, dw, tint(dev_color(&m.developer)));
        for &(i, x, w) in &cols {
            let Some(v) = app.vals[r][i] else {
                buf.set_stringn(area.x + x + w - 1, y, "-", 1, tint(MUTED));
                continue;
            };
            let style = match ext[i] {
                // The blended price is coloured by level, so its colour says the same thing on every screen.
                _ if i + 2 == PRICE => tint(LEVEL[level(v)]),
                Some((best, _)) if v == best => tint(GOOD).add_modifier(BOLD),
                Some((_, worst)) if v == worst => tint(BAD),
                _ => base,
            };
            let w = w as usize;
            buf.set_stringn(area.x + x, y, format!("{:>w$}", (COLS[i].show)(v)), w, style);
        }
        if let Some((x, w)) = via {
            let (mut x, end) = (area.x + x, area.x + x + w);
            for (j, h) in m.via.iter().enumerate() {
                if j > 0 {
                    x = buf.set_stringn(x, y, ", ", end.saturating_sub(x) as usize, base).0;
                }
                x = buf.set_stringn(x, y, h, end.saturating_sub(x) as usize, tint(dev_color(h))).0;
            }
        }
        let note = app.store.note(&m.key).unwrap_or("");
        if let Some((x, w)) = notes {
            buf.set_stringn(area.x + x, y, note, w as usize, base.add_modifier(Modifier::ITALIC));
        }
        if app.store.is_excluded(&m.key) {
            // The whole row is muted; search hits still show on top. The ✗ is drawn after it,
            // so it keeps its red.
            buf.set_style(Rect { y, height: 1, ..area }, tint(MUTED));
            buf.set_stringn(ex_x, y, "✗", 1, tint(BAD).add_modifier(BOLD));
        } else {
            buf.set_stringn(ex_x, y, "·", 1, tint(MUTED));
        }
        for &x in &seps {
            buf.set_stringn(area.x + x, y, "│", 1, tint(MUTED));
        }
        // What the search matched, underlined in bold; Notes may be scrolled off.
        // ponytail: a char is taken as one cell; wide chars would shift the underline.
        // Via is drawn as its harnesses joined by ", ", so the hits line up.
        if let Some(hits) = hits(&app.query, [&m.name, &m.developer, &m.via.join(", "), note], app.typos) {
            let style = tint(MATCH).add_modifier(BOLD | Modifier::UNDERLINED);
            let [via, note] = [via, notes].map(|c| c.map(|(x, w)| (area.x + x, w as usize)));
            for (field, ranges) in [Some((name_x, nw)), Some((dev_x, dw)), via, note].into_iter().zip(hits) {
                let Some((x, w)) = field else { continue };
                for r in ranges.into_iter().map(|r| r.start.min(w)..r.end.min(w)) {
                    buf.set_style(Rect::new(x + r.start as u16, y, r.len() as u16, 1), style);
                }
            }
        }
    }
    if let Some((x, w)) = cur {
        let drawn = app.rows.len().saturating_sub(top).min(height) as u16;
        buf.set_style(Rect::new(x, area.y + 2, w, drawn).intersection(area), Style::new().add_modifier(BOLD));
    }
    if let Input::Menu { col, items, sel, query, .. } = &app.input {
        let l = Layout { name_x: name_x - area.x, name_w, dev_w, cols, via, notes, first, more, seps };
        let picked = if *col == 1 { &app.dev } else { &app.via };
        dropdown(buf, area, menu_x(area, &l, *col), *col, items, query, *sel, picked);
    }
    (more, top > 0, top + height < app.rows.len())
}

/// Screen column where the dropdown of `col` opens: under its header, or Dev's when scrolled off.
fn menu_x(area: Rect, l: &Layout, col: usize) -> u16 {
    match (col, l.via) {
        (VIA, Some((x, _))) => area.x + x,
        _ => l.cols.iter().find(|c| c.0 + 2 == col).map_or(area.x + l.name_x + l.name_w + GAP, |c| area.x + c.1),
    }
}

/// The dropdown's box and its entries' width, or none when fewer than one entry would fit.
fn menu_box(area: Rect, x: u16, items: &[(String, usize)], rows: usize) -> Option<(Rect, (usize, usize))> {
    let label_w = items.iter().map(|(s, _)| s.chars().count()).max().unwrap_or(0);
    let n_w = items.iter().map(|(_, n)| n.to_string().len()).max().unwrap_or(0);
    let w = ((label_w + n_w + 8) as u16).min(area.width);
    let h = (rows as u16 + 2).min(area.height.saturating_sub(1));
    if h < 3 {
        return None;
    }
    Some((Rect::new(x.saturating_sub(2).min(area.right() - w).max(area.x), area.y + 1, w, h), (label_w, n_w)))
}

/// The list under a header opened with `d`, each entry with how many models it would show,
/// scrolled so the selection stays in view. Only `rows`, the entries matching the search, are
/// listed; the width fits every entry so the box keeps still while typing. `picked` entries
/// show `✓`, the rest `☐`.
#[allow(clippy::too_many_arguments)]
fn dropdown(
    buf: &mut Buffer,
    area: Rect,
    x: u16,
    col: usize,
    items: &[(String, usize)],
    query: &str,
    sel: usize,
    picked: &[String],
) {
    let rows = &menu_rows(items, query);
    let Some((rect, (label_w, n_w))) = menu_box(area, x, items, rows.len()) else { return };
    let block = Block::bordered().border_type(BorderType::Rounded).border_style(fg(ACCENT));
    let inner = block.inner(rect);
    Clear.render(rect, buf);
    block.render(rect, buf);
    let shown = inner.height as usize;
    let top = sel.saturating_sub(shown - 1);
    for (k, &i) in rows.iter().enumerate().skip(top).take(shown) {
        let (label, n) = &items[i];
        let y = inner.y + (k - top) as u16;
        let base = if k == sel { Style::new().add_modifier(Modifier::REVERSED) } else { Style::new() };
        // Entries keep the colour they have in the table, off the selection bar like there.
        let color = match i {
            0 => Color::Reset,
            _ if col == PRICE => LEVEL[i - 1],
            _ => dev_color(label),
        };
        let tint = |c: Color| if k == sel { base } else { fg(c) };
        buf.set_style(Rect { y, height: 1, ..inner }, base);
        // Checkboxes as on the table's marks, in the mark's own colour there too, so a picked
        // entry does not read as an empty box in the entry's colour; "any" clears the picks,
        // so it has none.
        let (mark, style) = match i {
            0 => ("  ", tint(color)),
            _ if picked.contains(label) => ("✓ ", tint(MARK).add_modifier(BOLD)),
            _ => ("☐ ", tint(color)),
        };
        let x = buf.set_stringn(inner.x + 1, y, mark, 2, style).0;
        let at = x;
        let x = buf.set_stringn(x, y, format!("{label:<label_w$}  "), label_w + 2, tint(color)).0;
        for r in found(label, query).into_iter().map(|r| r.start.min(label_w)..r.end.min(label_w)) {
            buf.set_style(Rect::new(at + r.start as u16, y, r.len() as u16, 1), hit_style(tint(color)));
        }
        buf.set_stringn(x, y, format!("{n:>n_w$}"), n_w, tint(MUTED));
    }
    vmarks(buf, rect.x, inner.y, inner.bottom() - 1, top > 0, top + shown < rows.len());
}

/// `▲` and `▼` on the left border column `x`, at the first and last content row, for rows
/// scrolled off above or below. Every scrolling list uses these, as `‹` `›` mark columns.
fn vmarks(buf: &mut Buffer, x: u16, top: u16, bottom: u16, above: bool, below: bool) {
    let edge = fg(ACCENT).add_modifier(BOLD);
    if above {
        buf.set_stringn(x, top, "▲", 1, edge);
    }
    if below {
        buf.set_stringn(x, bottom, "▼", 1, edge);
    }
}

/// A solid label like a bar module; returns the column after it.
fn pill(buf: &mut Buffer, x: u16, y: u16, text: &str, color: Color, max: u16) -> u16 {
    buf.set_stringn(x, y, format!(" {text} "), max as usize, fg(Color::Black).bg(color).add_modifier(BOLD)).0
}

/// The status bar's left side, joined with " · ": the model count, then what filters it.
fn parts(app: &App) -> Vec<Line<'static>> {
    let any = app.data.models.iter().any(|m| m.available);
    let scope = match (app.all, any) {
        (false, true) => "available",
        (false, false) => "all (no access found)",
        (true, _) => "all",
    };
    let part = |s: String, c: Color| Line::styled(s, fg(c));
    let mut parts = vec![part(format!("{} {scope}", app.rows.len()), Color::Reset)];
    if stale(app) {
        parts.push(part(format!("data {} old", age(app.data.age())), BAD));
    }
    if app.any_marked() {
        let n = app.store.marked.len();
        parts.push(part(format!("{n} marked{}", if app.only_marked { " only" } else { "" }), MARK));
    }
    if app.only_fav {
        parts.push(part("★ favorites only".into(), STAR));
    }
    if app.only_excluded {
        parts.push(part("✗ excluded only".into(), BAD));
    }
    // The tasks the model under the cursor is the favorite for, each ★ in its task's colour:
    // the row's single ★ does not say which.
    if let Some(m) = app.current() {
        let tasks = app.store.favorite_for(&m.key);
        if !tasks.is_empty() {
            let spans = tasks.iter().enumerate().map(|(i, t)| {
                Span::styled(format!("{}★ {t}", if i > 0 { " " } else { "" }), fg(task_color(t)).add_modifier(BOLD))
            });
            parts.push(Line::from(spans.collect::<Vec<_>>()));
        }
    }
    if !app.query.is_empty() {
        parts.push(part(format!("/{}", app.query), Color::Yellow));
    }
    if let Some(t) = app.task {
        parts.push(part(format!("best {} per price", t.name), Color::Yellow));
    }
    if !app.dev.is_empty() {
        parts.push(part(format!("Dev={}", app.dev.join(",")), Color::Yellow));
    }
    if !app.via.is_empty() {
        parts.push(part(format!("Via={}", app.via.join(",")), Color::Yellow));
    }
    for &(col, lo, hi) in &app.bounds {
        let (sign, v) = if lo.is_finite() { ("≥", lo) } else { ("≤", hi) };
        // The level label would repeat what the number says.
        let shown = match crate::app::numeric(col) {
            _ if col == PRICE => money(v),
            Some(c) => (c.show)(v),
            None => v.to_string(),
        };
        parts.push(part(format!("{}{sign}{shown}", col_name(col)), Color::Yellow));
    }
    if !app.status.is_empty() {
        parts.push(part(app.status.clone(), if app.failed { BAD } else { GOOD }));
    }
    parts
}

/// The mode pill's label and colour.
fn mode(app: &App) -> (&'static str, Color) {
    match (&app.input, &app.view) {
        (Input::Search { .. }, _) => ("SEARCH", Color::Blue),
        (Input::Note { .. }, _) => ("NOTE", Color::Blue),
        (Input::Bound { .. }, _) => ("BOUND", Color::Yellow),
        (Input::Menu { .. }, _) => ("PICK", Color::Yellow),
        (Input::Quit, _) => ("QUIT", Color::Red),
        (Input::Choose { items, .. }, _) if matches!(items.first(), Some((_, Effect::Launch(_)))) => {
            ("LAUNCH", Color::Green)
        }
        (Input::Choose { .. }, _) if app.choosing_favs() => ("FAV", Color::Green),
        (Input::Choose { .. }, _) if app.theme_preview().is_some() => ("THEME", Color::Green),
        (Input::Choose { .. }, _) => ("OPEN", Color::Green),
        (Input::None, View::Table) if app.selecting() => ("VISUAL", Color::Yellow),
        (Input::None, View::Table) => ("NORMAL", Color::Magenta),
        (Input::None, View::Help) => ("HELP", Color::Cyan),
        (Input::None, View::Recommend) => ("RECOMMEND", Color::Cyan),
        (Input::None, View::Detail) => ("DETAIL", Color::Cyan),
        (Input::None, View::Compare) => ("COMPARE", Color::Cyan),
    }
}

/// Returns the cursor column while a search, note or bound is being typed.
fn status(buf: &mut Buffer, area: Rect, app: &App) -> Option<u16> {
    let width = area.width as usize;
    let (mode, color) = mode(app);
    let mut x = pill(buf, area.x, area.y, mode, color, area.width) + 1;
    if matches!(app.input, Input::Quit | Input::Choose { typing: false, .. }) {
        // The question is in a box in the middle of the screen.
        return None;
    }
    // The prompt's label, the text being typed and the cursor's byte offset in it.
    let prompt = match &app.input {
        Input::Search { cur, .. } => {
            let q = if app.overlay_search() { &app.overlay_query } else { &app.query };
            Some(("/".to_string(), q, *cur))
        }
        Input::Note { text, cur } => Some(("note: ".to_string(), text, *cur)),
        Input::Bound { col, min, text, cur } => {
            Some((format!("{} {} ", col_name(*col), if *min { "≥" } else { "≤" }), text, *cur))
        }
        Input::Menu { col, query, cur, typing, .. } => Some(match (*typing, query.is_empty()) {
            (false, true) => (format!("{} ▾", col_name(*col)), query, *cur),
            _ => (format!("{} ▾ /", col_name(*col)), query, *cur),
        }),
        Input::Choose { title, query, cur, .. } => Some((format!("{title} /"), query, *cur)),
        Input::Quit | Input::None => None,
    };
    if let Some((label, typed, cur)) = prompt {
        let text = format!("{label}{typed}");
        let (menu, typing) = match app.input {
            Input::Menu { typing, .. } => (true, typing),
            Input::Choose { .. } => (true, true),
            _ => (false, true),
        };
        let hint = match (menu, typing) {
            (true, false) => "j k move  / search  enter pick  m toggle  esc close",
            (true, true) => "↓ ↑ move  enter pick  esc clear",
            _ => "enter apply  esc cancel",
        };
        let hx = area.right().saturating_sub(hint.len() as u16 + 1);
        buf.set_stringn(x, area.y, &text, hx.saturating_sub(x) as usize, Style::new());
        buf.set_stringn(hx, area.y, hint, hint.len(), fg(MUTED));
        let cx = x + (label.chars().count() + typed[..cur].chars().count()) as u16;
        return typing.then_some(cx.min(area.right().saturating_sub(1)));
    }
    let parts = parts(app);
    // Key hints fill what the left side leaves free; whole hints drop from the front on
    // narrow terminals, and `? help` is the last to go.
    let (hints, start) = hint_layout(app, area.width);
    let limit = (area.x + start).saturating_sub(1);
    for (i, line) in parts.iter().enumerate() {
        if i > 0 {
            x = buf.set_stringn(x, area.y, " · ", limit.saturating_sub(x) as usize, fg(MUTED)).0;
        }
        for span in &line.spans {
            let style = line.style.patch(span.style);
            x = buf.set_stringn(x, area.y, &span.content, limit.saturating_sub(x) as usize, style).0;
        }
    }
    // Hints: the key in colour, what it does in plain text.
    let mut x = area.x + start;
    for (i, hint) in hints.iter().enumerate() {
        if *hint == SEP {
            x = buf.set_stringn(x, area.y, SEP, width, fg(MUTED)).0;
        } else {
            let (key, what) = split_hint(hint);
            x = buf.set_stringn(x, area.y, key, width, fg(KEY).add_modifier(BOLD)).0;
            x = buf.set_stringn(x, area.y, what, width, Style::new()).0;
        }
        if i + 1 < hints.len() {
            x = buf.set_stringn(x, area.y, "  ", width, Style::new()).0;
        }
    }
    None
}

/// The char ranges where `q` occurs in `s`, any case: what a substring search matched.
fn found(s: &str, q: &str) -> Vec<Range<usize>> {
    let low = |c: char| c.to_lowercase().next().unwrap_or(c);
    let (s, q): (Vec<char>, Vec<char>) = (s.chars().map(low).collect(), q.chars().map(low).collect());
    if q.is_empty() {
        return vec![];
    }
    (0..(s.len() + 1).saturating_sub(q.len())).filter(|&i| s[i..i + q.len()] == q[..]).map(|i| i..i + q.len()).collect()
}

/// How a search hit is drawn, as in the table: yellow, bold and underlined, but off the
/// reverse-video selection bar only bold and underlined, so the bar keeps no colour.
fn hit_style(style: Style) -> Style {
    let hit = style.add_modifier(BOLD | Modifier::UNDERLINED);
    if style.add_modifier.contains(Modifier::REVERSED) { hit } else { hit.fg(MATCH) }
}

/// `line` with the chars each span's `ranges` cover drawn as search hits.
fn lit(mut line: Line<'static>, ranges: impl Fn(&str) -> Vec<Range<usize>>) -> Line<'static> {
    let outer = line.style;
    line.spans = std::mem::take(&mut line.spans)
        .into_iter()
        .flat_map(|span| {
            let r = ranges(&span.content);
            let hit = hit_style(outer.patch(span.style));
            let mut out: Vec<Span<'static>> = vec![];
            for (i, c) in span.content.chars().enumerate() {
                let style = if r.iter().any(|r| r.contains(&i)) { hit } else { span.style };
                match out.last_mut() {
                    Some(last) if last.style == style => last.content.to_mut().push(c),
                    _ => out.push(Span::styled(c.to_string(), style)),
                }
            }
            out
        })
        .collect();
    line
}

/// The entries of a choice list, each coloured by its first word: the harness or the site.
fn choice_lines(items: &[(String, Effect)], sel: usize, query: &str) -> Vec<Line<'static>> {
    let rows = choice_rows(items, query);
    let mut lines: Vec<Line> = rows
        .iter()
        .enumerate()
        .map(|(i, &k)| {
            let (label, effect) = &items[k];
            // f's tasks in their colours, harnesses and sites in theirs.
            let color = match effect {
                Effect::Fav(t) => task_color(t),
                _ => dev_color(label.split(' ').next().unwrap_or_default()),
            };
            let style = if i == sel { Style::new().add_modifier(Modifier::REVERSED) } else { fg(color) };
            // A ticked box in the mark's colour, as in the table and the dropdowns; the rest of
            // the label keeps the task's or the harness's own.
            match label.strip_prefix('✓') {
                Some(rest) if i != sel => Line::from(vec![
                    Span::styled(" ✓", fg(MARK).add_modifier(BOLD)),
                    Span::styled(format!("{rest} "), style),
                ]),
                _ => Line::from(format!(" {label} ")).style(style),
            }
        })
        .map(|l| lit(l, |s| found(s, query)))
        .collect();
    // What `/` left, said as the dropdowns say it.
    if rows.is_empty() {
        lines.push(Line::from(format!(" no entry matches {query} ")).style(fg(MUTED)));
    }
    let hint = match items.first() {
        Some((_, Effect::Fav(_))) => " j k move · / search · m toggle · enter toggle and close · esc close",
        Some((_, Effect::Theme(_))) => " j k preview · / search · enter saves · esc t close",
        _ => " j k move · / search · enter opens",
    };
    lines.push(Line::from(hint).style(fg(MUTED)));
    lines
}
/// Where an overlay with these lines sits: centred, as wide as its widest line or title.
fn overlay_rect(area: Rect, title: &str, lines: &[Line]) -> Rect {
    let widest = lines.iter().map(Line::width).max().unwrap_or(0) as u16;
    let w = (widest + 4).max(title.chars().count() as u16 + 6).min(area.width);
    let h = (lines.len() as u16 + 2).min(area.height);
    Rect::new(area.x + (area.width - w) / 2, area.y + (area.height - h) / 2, w, h)
}

/// A centred rounded box showing `lines` from `scroll` on, which is clamped to the content.
fn overlay(buf: &mut Buffer, area: Rect, title: &str, lines: Vec<Line<'static>>, scroll: &mut u16) {
    let rect = overlay_rect(area, title, &lines);
    let h = rect.height;
    let shown = h.saturating_sub(2) as usize;
    *scroll = (*scroll).min(lines.len().saturating_sub(shown) as u16);
    // Where you are; the keys are in the status bar.
    let footer = if lines.len() > shown {
        format!(" {}-{} of {} ", *scroll + 1, *scroll as usize + shown, lines.len())
    } else {
        String::new()
    };
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(fg(ACCENT))
        .title_top(Line::from(format!(" {title} ")).style(fg(ACCENT).add_modifier(BOLD)))
        .title_bottom(Line::from(footer).style(fg(MUTED)).right_aligned());
    let inner = block.inner(rect);
    Clear.render(rect, buf);
    block.render(rect, buf);
    let (above, below) = (*scroll > 0, *scroll as usize + shown < lines.len());
    for (line, y) in lines.into_iter().skip(*scroll as usize).zip(inner.y..inner.bottom()) {
        line.render(Rect { x: inner.x + 1, y, width: inner.width.saturating_sub(2), height: 1 }, buf);
    }
    vmarks(buf, rect.x, inner.y, inner.bottom() - 1, above, below);
}

fn heading(text: &str) -> Line<'static> {
    Line::from(text.to_string()).style(fg(ACCENT).add_modifier(BOLD))
}

fn help(query: &str) -> Vec<Line<'static>> {
    let key_w = HELP.iter().flat_map(|(_, keys)| keys.iter()).map(|(k, _)| k.chars().count()).max().unwrap_or(0);
    let mut v: Vec<Line> = Vec::new();
    for (name, keys) in HELP {
        v.push(heading(name));
        v.extend(keys.iter().map(|(k, what)| {
            Line::from(vec![Span::styled(format!("{k:<key_w$}  "), fg(KEY).add_modifier(BOLD)), Span::raw(*what)])
        }));
        v.push(Line::default());
    }
    v.push(heading("Columns: green the best shown, red the worst"));
    for c in 0..NCOLS {
        v.push(Line::from(vec![Span::styled(format!("{:<11}", col_name(c)), fg(KEY)), Span::raw(col_about(c))]));
    }
    v.push(Line::default());
    v.push(Line::from(format!("Saved in {}", crate::store::path().display())).style(fg(MUTED)));
    v.push(Line::from("More in the README · CLI: modelcmp --help").style(fg(MUTED)));
    // `/` keeps the lines that match, so a key or a column can be looked up in a long list.
    if !query.is_empty() {
        let q = query.to_lowercase();
        v.retain(|l| l.spans.iter().any(|s| s.content.to_lowercase().contains(&q)));
        v = v.into_iter().map(|l| lit(l, |s| found(s, query))).collect();
        if v.is_empty() {
            v.push(Line::from(format!("no help line matches {query}")).style(fg(MUTED)));
        }
    }
    v
}

/// One block per task: what it is, when to pick a model high on it and its best models per
/// price, wrapped to `width`. The cursor's block is highlighted; enter
/// ranks the table by it.
fn recommend(app: &App, width: usize) -> Vec<Line<'static>> {
    let cur = app.current().map(|m| m.key.clone());
    let name = |i: usize, s: String| {
        let style = fg(TASK[i]).add_modifier(BOLD);
        Span::styled(s, if i == app.task_cur { style.add_modifier(Modifier::REVERSED) } else { style })
    };
    let label = |s: &'static str| vec![Span::styled(s, fg(MUTED))];
    let words = |s: &str| s.split(' ').map(|w| Line::from(w.to_string())).collect();
    let space = Span::raw(" ");
    let mut v: Vec<Line> = wrapped(vec![], words(&frontier_legend(false)), &space, width)
        .into_iter()
        .map(|l| l.style(fg(MUTED)))
        .collect();
    for (i, t) in TASKS.iter().enumerate() {
        v.push(Line::default());
        v.extend(wrapped(vec![name(i, format!(" {} ", t.name)), space.clone()], words(t.about), &space, width));
        v.extend(wrapped(label("  use for:         "), words(t.when), &space, width));
        let picked = (i == app.task_cur).then_some(cur.as_deref()).flatten();
        v.extend(wrapped(
            label("  best per price:  "),
            frontier_spans(app, t, picked),
            &Span::styled(" · ", fg(MUTED)),
            width,
        ));
    }
    v.push(Line::default());
    let cli = words(
        "CLI: modelcmp recommend · modelcmp list --task <task> [--tier low|mid|high] · modelcmp fav <task> <model>",
    );
    v.extend(wrapped(vec![], cli, &space, width).into_iter().map(|l| l.style(fg(MUTED))));
    v
}

/// `label` then `items` joined by `glue`, broken between items at `width`, continuation
/// lines indented to the label. A break keeps the glue's punctuation ("," or " ·") at the end.
fn wrapped(
    label: Vec<Span<'static>>,
    items: Vec<Line<'static>>,
    glue: &Span<'static>,
    width: usize,
) -> Vec<Line<'static>> {
    let indent: usize = label.iter().map(Span::width).sum();
    let end = glue.content.trim_end();
    let end_w = Span::raw(end).width();
    let mut lines = Vec::new();
    let (mut cur, mut w) = (label, indent);
    for item in items {
        // Room is kept for the punctuation a later break would add.
        if w > indent && w + glue.width() + item.width() + end_w > width {
            if !end.is_empty() {
                cur.push(Span::styled(end.to_string(), glue.style));
            }
            lines.push(Line::from(std::mem::replace(&mut cur, vec![Span::raw(" ".repeat(indent))])));
            w = indent;
        } else if w > indent {
            w += glue.width();
            cur.push(glue.clone());
        }
        w += item.width();
        cur.extend(item.spans);
    }
    lines.push(Line::from(cur));
    lines
}

/// `name $price (score)` for each entry of the task's price frontier, cheapest first and the
/// best last, each in its price level's colour as in the Price column, the favorite's ★ in
/// the task's colour. The `picked` model is a reverse-video bar, colour kept off it as in the table.
fn frontier_spans(app: &App, t: &fit::Task, picked: Option<&str>) -> Vec<Line<'static>> {
    let front = app.task_frontier(t);
    if front.is_empty() {
        return vec![Line::from(Span::styled("no data", fg(MUTED)))];
    }
    front
        .iter()
        .map(|(m, s)| {
            let on = picked == Some(m.key.as_str());
            let tint = |c: Color| if on { Style::new().add_modifier(Modifier::REVERSED) } else { fg(c) };
            let mut spans = Vec::with_capacity(2);
            if app.store.favorite(t.name) == Some(m.key.as_str()) {
                spans.push(Span::styled("★ ", tint(task_color(t.name)).add_modifier(BOLD)));
            }
            spans.push(Span::styled(
                priced(m, fit::shown(m, t, *s), false, false),
                tint(m.cost().map_or(MUTED, |c| LEVEL[level(c)])),
            ));
            Line::from(spans)
        })
        .collect()
}

/// Every detail line, with `key:` labels and section headings coloured.
fn detail(m: &Model, store: &Store) -> Vec<Line<'static>> {
    let is_label = |k: &str| k.len() < 16 && k.trim().chars().all(|c| c.is_alphabetic() || c == ' ');
    detail_lines(m, store)
        .into_iter()
        .skip(1) // the name is the title
        .map(|s| match s.split_once(':') {
            Some((k, v)) if s.starts_with("  ") && is_label(k) => {
                Line::from(vec![Span::styled(format!("{k}:"), fg(KEY)), Span::raw(v.to_string())])
            }
            _ if s.ends_with(':') => heading(&s),
            _ => Line::from(s),
        })
        .collect()
}

/// The verdict, then the marked models side by side with the best value of each row in green
/// and the selected one as a reverse-video column. When they do not all fit in `avail` cells,
/// the view starts at model `first`, moved only as far as it takes to show the selection, and
/// the `first` in effect comes back for `App::compare_x`.
fn compare(models: &[&Model], sel: usize, first: usize, avail: usize, query: &str) -> (Vec<Line<'static>>, usize) {
    let mut rows = compare_rows(models);
    // The model row is the header; `query` filters the rest, forgiving a typo when nothing matches.
    let mut typos = false;
    if !query.trim().is_empty() {
        let keep = |typos: bool| {
            rows.iter().enumerate().filter(|(i, r)| *i == 0 || hits(query, [&r.label], typos).is_some()).count()
        };
        typos = keep(false) < 2;
        rows = rows
            .into_iter()
            .enumerate()
            .filter(|(i, r)| *i == 0 || hits(query, [&r.label], typos).is_some())
            .map(|(_, r)| r)
            .collect();
    }
    let n = models.len();
    let sel = sel.min(n.saturating_sub(1));
    let label_w = rows.iter().map(|r| r.label.chars().count()).max().unwrap_or(0);
    let widths: Vec<usize> =
        (0..n).map(|i| rows.iter().map(|r| r.cells[i].chars().count()).max().unwrap_or(0)).collect();
    // How many models from `f` on fit beside the labels; always at least one.
    let count = |f: usize, reserve: usize| {
        let mut room = avail.saturating_sub(label_w + reserve);
        let ok = |w: &&usize| {
            let fit = room >= **w + 2;
            room = room.saturating_sub(**w + 2);
            fit
        };
        widths[f..].iter().take_while(ok).count().max(1)
    };
    // Two cells go to the " ›" marking models cut off on the right, but only when there are any.
    let fits = |f: usize| {
        let all = count(f, 0);
        if f + all >= n { all } else { count(f, 2) }
    };
    let max_first = (0..n).find(|&f| f + fits(f) >= n).unwrap_or(0);
    let mut first = first.min(sel).min(max_first);
    while sel >= first + fits(first) {
        first += 1;
    }
    let shown = fits(first);
    let mut out = vec![heading("verdict:")];
    out.extend(verdict_lines(models));
    out.push(Line::default());
    if shown < n {
        out.push(Line::from(format!("models {}-{} of {n} · h l move", first + 1, first + shown)).style(fg(MUTED)));
    }
    // The model row carries `‹` and `›` for models scrolled off, as the table's header does.
    let edge = fg(ACCENT).add_modifier(BOLD);
    out.extend(rows.into_iter().enumerate().flat_map(|(k, r)| {
        let label = Line::from(Span::styled(format!("{:<label_w$}", r.label), fg(KEY)));
        let hit = hits(query, [&r.label], typos).map_or(vec![], |[r]| r);
        let mut spans = lit(label, |_| hit.clone()).spans;
        for (i, c) in r.cells.into_iter().enumerate().skip(first).take(shown) {
            let style = if i == sel {
                Style::new().add_modifier(Modifier::REVERSED)
            } else if r.best == Some(i) {
                fg(GOOD).add_modifier(BOLD)
            } else {
                Style::new()
            };
            if k == 0 && i == first && first > 0 {
                spans.push(Span::styled("‹ ", edge));
            } else {
                spans.push(Span::raw("  "));
            }
            spans.push(Span::styled(format!("{c:>w$}", w = widths[i]), style));
        }
        if k == 0 && first + shown < n {
            spans.push(Span::styled(" ›", edge));
        }
        let line = Line::from(spans);
        // A rule under the model row, as under the table's header.
        let rule = (k == 0).then(|| Line::styled("─".repeat(line.width()), fg(MUTED)));
        std::iter::once(line).chain(rule)
    }));
    (out, first)
}

/// The verdict as a small table: question, winner in green, margin dimmed.
fn verdict_lines(models: &[&Model]) -> Vec<Line<'static>> {
    let rows = verdict(models);
    let w = |i: usize| rows.iter().map(|r| r[i].chars().count()).max().unwrap_or(0);
    let (w0, w1) = (w(0), w(1));
    rows.into_iter()
        .map(|[q, win, margin]| {
            Line::from(vec![
                Span::styled(format!("  {q:<w0$}  "), fg(KEY)),
                Span::styled(format!("{win:<w1$}  "), fg(GOOD).add_modifier(BOLD)),
                Span::styled(margin, fg(MUTED)),
            ])
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::ECI;
    use crate::data::Offer;
    use ratatui::crossterm::event::KeyCode;

    fn model(name: &str, dev: &str, eci: Option<f64>, price: f64) -> Model {
        Model {
            key: name.into(),
            name: name.into(),
            developer: dev.into(),
            available: true,
            via: vec!["opencode".into()],
            eci,
            context: 200_000,
            offers: vec![Offer {
                provider: "p".into(),
                id: name.into(),
                input: price,
                output: price,
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    fn app() -> App {
        let models = vec![model("opus", "anthropic", Some(150.0), 5.0), model("flash", "google", Some(120.0), 0.1)];
        let fetched = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        App::new(Data { models, fetched, ..Default::default() }, Store::default())
    }

    #[test]
    fn old_data_says_so_in_the_status_bar() {
        let mut a = app();
        let (_, lines) = render(&mut a, 200, 4);
        assert!(!lines[3].contains(" old") && !lines[3].contains("r refresh"), "fresh: {}", lines[3]);
        a.data.fetched -= data::MAX_AGE.as_secs() + 3600;
        let (buf, lines) = render(&mut a, 200, 4);
        assert!(lines[3].starts_with(" NORMAL  2 available · data 25h old"), "{}", lines[3]);
        assert_eq!(buf[(cell(&lines[3], "data"), 3)].fg, BAD);
        assert!(lines[3].contains("a all  % no cache  r refresh  │  enter details"), "{}", lines[3]);
        a.refreshing = true;
        data::set_cached(0.0);
        let (_, lines) = render(&mut a, 200, 4);
        assert!(lines[3].contains("a all  % 90% cached  │"), "off the default, the way back: {}", lines[3]);
        assert!(!lines[3].contains(" old") && !lines[3].contains("r refresh"), "refreshing: {}", lines[3]);
    }

    fn render(app: &mut App, width: u16, height: u16) -> (Buffer, Vec<String>) {
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        table(&mut buf, Rect { height: height - 1, ..area }, app);
        status(&mut buf, Rect { y: height - 1, height: 1, ..area }, app);
        let lines = (0..height)
            .map(|y| (0..width).map(|x| buf[(x, y)].symbol()).collect::<String>().trim_end().to_owned())
            .collect();
        (buf, lines)
    }

    #[test]
    fn theme_swaps_colours_and_paints_its_background() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 3, 1));
        buf[(0, 0)].set_fg(Color::Black).set_bg(Color::LightBlue);
        buf[(1, 0)].set_fg(Color::Reset);
        recolor(&mut buf, THEMES[crate::view::theme("nord")].1.as_ref());
        // A pill: nord's background as the text, and the light blue kept, being the half of blue
        // that stands furthest from it.
        assert_eq!((buf[(0, 0)].fg, buf[(0, 0)].bg), (Color::from_u32(0x2e3440), Color::from_u32(0xa3d0e8)));
        assert_eq!((buf[(1, 0)].fg, buf[(1, 0)].bg), (Color::from_u32(0xd8dee9), Color::from_u32(0x2e3440)));
        let mut buf = Buffer::empty(Rect::new(0, 0, 1, 1));
        recolor(&mut buf, None);
        assert_eq!(buf[(0, 0)].bg, Color::Reset, "the terminal's own colours keep its background");
    }

    /// Text a theme paints must be legible on what is behind it: the WCAG ratio for bold text,
    /// 3:1, for every colour a row or a pill can take.
    #[test]
    fn every_theme_reads_on_its_own_background() {
        let lum = |c: u32| {
            let f = |s: u32| {
                let v = f64::from((c >> s) & 255) / 255.0;
                if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
            };
            0.2126 * f(16) + 0.7152 * f(8) + 0.0722 * f(0)
        };
        let ratio = |a: u32, b: u32| (lum(a).max(lum(b)) + 0.05) / (lum(a).min(lum(b)) + 0.05);
        let hex = |c: Color| match c {
            Color::Rgb(r, g, b) => u32::from_be_bytes([0, r, g, b]),
            c => panic!("not a theme colour: {c:?}"),
        };
        for (name, p) in THEMES.iter().filter_map(|(n, p)| p.as_ref().map(|p| (n, p))) {
            for c in p.accents.iter().chain([&p.text]) {
                assert!(ratio(*c, p.bg) >= 3.0, "{name}: {c:06x} on the background, {:.1}:1", ratio(*c, p.bg));
            }
            // Every colour the drawing code names, as text on the background and as a pill's fill.
            for c in [
                Color::Red,
                Color::Green,
                Color::Yellow,
                Color::Blue,
                Color::Magenta,
                Color::Cyan,
                Color::LightRed,
                Color::LightGreen,
                Color::LightYellow,
                Color::LightBlue,
                Color::LightMagenta,
                Color::LightCyan,
            ] {
                let (text, fill) = (hex(resolve(c, Some(p))), hex(contrasting(c, p)));
                assert!(ratio(text, p.bg) >= 3.0, "{name}: {c:?} as text, {:.1}:1", ratio(text, p.bg));
                assert!(ratio(fill, p.bg) >= 3.0, "{name}: {c:?} as a pill, {:.1}:1", ratio(fill, p.bg));
            }
            assert!(ratio(p.ansi[8], p.bg) >= 2.5, "{name}: muted text, {:.1}:1", ratio(p.ansi[8], p.bg));
        }
    }

    /// Two developers must not get colours that look the same: the accents stay apart by the
    /// weighted RGB distance (a redmean approximation), and their count divides `dev_color`'s 210
    /// so the five harness names never collide.
    #[test]
    fn accents_are_told_apart() {
        for (name, p) in THEMES.iter().filter_map(|(n, p)| p.as_ref().map(|p| (n, p))) {
            assert_eq!(210 % p.accents.len(), 0, "{name}: {} accents do not divide 210", p.accents.len());
            for (i, &x) in p.accents.iter().enumerate() {
                for &y in &p.accents[i + 1..] {
                    assert!(apart(x, y) >= 60.0, "{name}: {x:06x} and {y:06x} look alike, {:.0}", apart(x, y));
                }
            }
        }
    }

    fn chan(c: u32, shift: u32) -> f64 {
        f64::from((c >> shift) & 255)
    }

    fn apart(x: u32, y: u32) -> f64 {
        let (dr, dg, db) = (chan(x, 16) - chan(y, 16), chan(x, 8) - chan(y, 8), chan(x, 0) - chan(y, 0));
        let rm = (chan(x, 16) + chan(y, 16)) / 2.0;
        ((2.0 + rm / 256.0) * dr * dr + 4.0 * dg * dg + (3.0 - rm / 256.0) * db * db).sqrt()
    }

    /// A marked row shows it by the colour of its ✓, so that colour cannot look like the muted
    /// ☐ beside it. Slot 12 is the mark's and slot 8 the muted one's.
    #[test]
    fn the_mark_parts_from_an_empty_box() {
        for (name, p) in THEMES.iter().filter_map(|(n, p)| p.as_ref().map(|p| (n, p))) {
            let d = apart(p.ansi[12], p.ansi[8]);
            assert!(d >= 130.0, "{name}: ✓ {:06x} and ☐ {:06x} look alike, {d:.0}", p.ansi[12], p.ansi[8]);
        }
    }

    #[test]
    fn every_theme_keeps_the_harnesses_apart() {
        for (name, p) in &THEMES {
            let colors: Vec<_> =
                ["opencode", "claude", "codex", "gemini", "env"].map(|h| resolve(dev_color(h), p.as_ref())).into();
            assert!((1..5).all(|i| !colors[..i].contains(&colors[i])), "{name}: {colors:?}");
        }
    }

    #[test]
    fn marked_row_is_checked_and_coloured() {
        let mut a = app();
        a.store.marked = vec!["flash".into()];
        let (buf, lines) = render(&mut a, 120, 5);
        let row = |m: &str| lines.iter().position(|l| l.contains(m)).unwrap() as u16;
        let (opus, flash) = (row("opus"), row("flash"));
        assert!(lines[flash as usize].contains('✓') && lines[opus as usize].contains('☐'), "checkboxes: {lines:?}");
        let box_x = cell(&lines[flash as usize], "✓");
        assert_eq!(buf[(box_x, flash)].fg, MARK, "the ✓ is in the mark colour");
        assert!((0..120).all(|x| buf[(x, flash)].bg != MARK), "the row itself is not highlighted");
    }

    /// The cell where `pat` starts on `line`, which may hold multi-byte glyphs before it.
    fn cell(line: &str, pat: &str) -> u16 {
        line[..line.find(pat).unwrap()].chars().count() as u16
    }

    fn words(line: &str) -> Vec<&str> {
        line.split_whitespace().collect()
    }

    #[test]
    fn search_hits_are_underlined() {
        let mut a = app();
        a.query = "goo fla".into();
        a.rebuild();
        let (buf, lines) = render(&mut a, 120, 4);
        let row = &lines[2];
        let (name, dev) = (cell(row, "flash"), cell(row, "google"));
        let under = |x: u16| buf[(x, 2)].modifier.contains(Modifier::UNDERLINED);
        assert!((name..name + 3).all(under) && !under(name + 3), "fla of flash");
        assert!((dev..dev + 3).all(under) && !under(dev + 3), "goo of google");
        a.store.set_note("opus", "good at refactors");
        a.query = "refac".into();
        a.rebuild();
        let (buf, lines) = render(&mut a, 200, 4);
        assert_eq!((a.rows.len(), words(&lines[2])[4]), (1, "opus"), "notes are searched");
        let x = cell(&lines[2], "refactors");
        let under = |x: u16| buf[(x, 2)].modifier.contains(Modifier::UNDERLINED);
        assert!((x..x + 5).all(under) && !under(x + 5), "refac of the note");
        a.query = "openc".into();
        a.rebuild();
        let (buf, lines) = render(&mut a, 200, 4);
        assert_eq!(a.rows.len(), 2, "Via is searched");
        let x = cell(&lines[2], "opencode");
        assert!(buf[(x, 2)].modifier.contains(Modifier::UNDERLINED), "openc of the Via column");
    }

    #[test]
    fn wide_table_shows_every_column_and_extremes() {
        let mut a = app();
        let (buf, lines) = render(&mut a, 200, 6);
        let header = "# ✓ ★ ✗ Model Dev ▾ │ ▼Price ▾ $in $cache $out Ctx │ ECI Coding Agentic Reason \
                      Code/$ │ Via ▾ Notes";
        assert_eq!(words(&lines[0]), words(header));
        // A rule under the header, crossing the lines between the groups of columns.
        assert!(lines[1].starts_with('─') && lines[1].matches('┼').count() == 3, "{}", lines[1]);
        let sep = cell(&lines[0], "│");
        assert_eq!((cell(&lines[1], "┼"), cell(&lines[3], "│")), (sep, sep), "the group lines run straight down");
        assert_eq!(buf[(sep, 3)].fg, MUTED, "and are muted");
        let via = lines[3].find("opencode").expect(&lines[3]);
        assert_eq!(
            buf[(lines[3][..via].chars().count() as u16, 3)].fg,
            dev_color("opencode"),
            "harnesses are coloured"
        );
        assert_eq!(&words(&lines[2])[..11], ["1", "☐", "☆", "·", "opus", "anthropic", "│", "5.0", "5.0", "5.0", "5.0"]);
        assert_eq!(
            &words(&lines[3])[..11],
            ["2", "☐", "☆", "·", "flash", "google", "│", "0.10", "0.10", "0.10", "0.10"]
        );
        assert!(lines[5].starts_with(" NORMAL  2 available"), "{}", lines[5]);
        assert!(
            lines[5].ends_with(
                "h l column  │  / filter  s sort  d dropdown  R recommend  a all  % no cache  │  enter details  x launch  o open  y copy id  m mark  f fav  e exclude  n note  │  q quit  ? help"
            ),
            "{}",
            lines[5]
        );
        assert_eq!(buf[(cell(&lines[5], "│"), 5)].fg, MUTED, "groups are split by a muted rule");
        assert!(buf[(0, 2)].modifier.contains(Modifier::REVERSED), "row 0 is selected");
        assert!((0..200).all(|x| buf[(x, 2)].fg == Color::Reset), "no colour breaks the selection bar");
        assert_eq!(buf[(0, 3)].fg, MUTED, "row numbers are muted");
        assert_eq!(buf[(cell(&lines[3], "flash"), 3)].fg, Color::Reset, "names are plain text");
        assert_eq!(buf[(cell(&lines[3], "☆"), 3)].fg, MUTED, "the empty ☆ is muted");
        assert_eq!(buf[(lines[3].find("google").unwrap() as u16, 3)].fg, dev_color("google"));
        // Screen column of a byte offset: `▾` takes several bytes.
        let x = |i: usize| lines[3][..i].chars().count() as u16;
        assert_eq!(buf[(x(lines[3].find("120").unwrap()), 3)].fg, BAD, "the lowest ECI shown is red");
        assert_eq!(buf[(x(lines[3].rfind("0.10").unwrap()), 3)].fg, GOOD, "the cheapest $out is green");
        assert_eq!(
            buf[(x(lines[3].find("0.10").unwrap()), 3)].fg,
            LEVEL[1],
            "the blended price takes its level's colour"
        );
    }

    #[test]
    fn dropdown_opens_under_its_header() {
        let mut a = app();
        a.col = 1;
        let (_, lines) = render(&mut a, 170, 8);
        assert!(lines[7].contains("  d dropdown  "), "{}", lines[7]);
        a.key(KeyCode::Char('d').into());
        let (buf, lines) = render(&mut a, 170, 8);
        let dev = cell(&lines[0], "Dev") as usize - 2;
        let from = |l: &str| l.chars().skip(dev).collect::<String>();
        assert!(from(&lines[1]).starts_with("╭────────────────╮"), "{}", lines[1]);
        assert_eq!(&words(&lines[2])[5..8], ["│", "any", "2"]);
        assert_eq!(&words(&lines[3])[5..9], ["│", "☐", "anthropic", "1"]);
        assert!(buf[(dev as u16 + 2, 2)].modifier.contains(Modifier::REVERSED), "any is selected");
        assert!(lines[7].starts_with(" PICK  Dev ▾"), "{}", lines[7]);
        for c in "/anth".chars() {
            a.key(KeyCode::Char(c).into());
        }
        let (_, lines) = render(&mut a, 170, 8);
        assert_eq!(&words(&lines[3])[5..9], ["│", "☐", "anthropic", "1"], "only matches are listed");
        assert!(from(&lines[4]).starts_with("╰"), "{}", lines[4]);
        assert!(lines[7].starts_with(" PICK  Dev ▾ /anth"), "{}", lines[7]);
    }

    #[test]
    fn a_click_on_a_hint_presses_its_key() {
        use ratatui::crossterm::event::KeyModifiers;
        assert_eq!(split_hint("h l 0 $ model"), ("h l 0 $", " model"));
        assert_eq!(split_hint("enter best models first"), ("enter", " best models first"));
        assert_eq!(
            (hint_key("a all"), hint_key("esc back"), hint_key("j k scroll")),
            (Some(KeyCode::Char('a')), Some(KeyCode::Esc), None)
        );
        let mut a = app();
        let (w, h) = (200, 6);
        let (_, lines) = render(&mut a, w, h);
        let bar = &lines[h as usize - 1];
        let x = bar[..bar.find("? help").unwrap()].chars().count() as u16;
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: x,
            row: h - 1,
            modifiers: KeyModifiers::NONE,
        };
        let hit = hit(&a, Rect::new(0, 0, w, h), click);
        assert_eq!(hit, Some(Mouse::Key(KeyCode::Char('?'))));
        a.mouse(hit.unwrap());
        assert_eq!(a.view, View::Help);
    }

    #[test]
    fn clicks_land_on_rows_headers_and_dropdown_entries() {
        use ratatui::crossterm::event::KeyModifiers;
        let mut a = app();
        let (w, h) = (170, 10);
        let area = Rect::new(0, 0, w, h);
        // `draw` puts the header on row 1, its rule on row 2 and the first model on row 3, one cell in.
        let (_, lines) = render(&mut a, w - 2, h - 3);
        // Screen column of a header, one cell in: `▾` takes several bytes.
        let col = |s: &str| lines[0][..lines[0].find(s).unwrap()].chars().count() as u16 + 1;
        let click = |x, y| MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: x,
            row: y,
            modifiers: KeyModifiers::NONE,
        };
        assert_eq!(hit(&a, area, click(12, 4)), Some(Mouse::Row(1)));
        assert_eq!(hit(&a, area, click(3, 4)), Some(Mouse::Box(1)), "a click on the checkbox cycles it");
        assert_eq!(hit(&a, area, click(5, 4)), Some(Mouse::Star(1)), "and on the ☆ picks tasks");
        assert_eq!(hit(&a, area, click(7, 4)), Some(Mouse::Exclude(1)), "and on the ✗ box excludes the model");
        let right = |x, y| MouseEvent { kind: MouseEventKind::Down(MouseButton::Right), ..click(x, y) };
        assert_eq!(hit(&a, area, right(12, 4)), Some(Mouse::Mark(1)), "right click marks");
        let ctrl = |x, y| MouseEvent { modifiers: KeyModifiers::CONTROL, ..click(x, y) };
        assert_eq!(hit(&a, area, ctrl(12, 4)), Some(Mouse::Pick(1)), "ctrl click picks");
        let drag = |x, y| MouseEvent { kind: MouseEventKind::Drag(MouseButton::Left), ..click(x, y) };
        assert_eq!(hit(&a, area, drag(3, 5)), Some(Mouse::Extend(2)));
        let shift = |x, y| MouseEvent { modifiers: KeyModifiers::SHIFT, ..click(x, y) };
        assert_eq!(hit(&a, area, shift(3, 5)), Some(Mouse::Extend(2)), "shift click extends like a drag");
        assert_eq!(hit(&a, area, drag(3, 0)), Some(Mouse::Extend(0)), "a drag above the table: the first row");
        assert_eq!(hit(&a, area, drag(3, h)), Some(Mouse::Extend(h as usize - 6)), "below: the last row");
        assert_eq!(hit(&a, Rect::new(0, 0, w, 5), drag(3, 2)), None, "no rows to extend over");
        assert_eq!(hit(&a, area, click(1, 1)), Some(Mouse::Top), "the # header: the first row");
        assert_eq!(hit(&a, area, click(3, 1)), Some(Mouse::OnlyMarked), "the ✓ header: marked only");
        assert_eq!(hit(&a, area, click(5, 1)), Some(Mouse::OnlyFav), "the ★ header: favorites only");
        assert_eq!(hit(&a, area, click(7, 1)), Some(Mouse::OnlyExcluded), "the ✗ header: excluded only");
        assert_eq!(hit(&a, area, click(9, 1)), Some(Mouse::Header(0)));
        assert_eq!(hit(&a, area, click(3, 2)), None, "the rule under the header");
        assert_eq!(hit(&a, area, click(col("Dev"), 1)), Some(Mouse::Header(1)));
        assert_eq!(hit(&a, area, click(col("Dev") + 4, 1)), Some(Mouse::Menu(1)), "the ▾ after Dev");
        assert_eq!(hit(&a, area, click(col("▼Price"), 1)), Some(Mouse::Header(2)));
        assert_eq!(hit(&a, area, click(col("▼Price") + 7, 1)), Some(Mouse::Menu(2)), "the ▾ after Price");
        assert_eq!(hit(&a, area, click(col("Coding"), 1)), Some(Mouse::Header(8)));
        assert_eq!(hit(&a, area, click(0, 0)), None, "the frame");
        assert_eq!(hit(&a, area, click(3, h - 1)), None, "the status bar");
        let wheel = |kind| MouseEvent { kind, column: 0, row: 0, modifiers: KeyModifiers::NONE };
        assert_eq!(hit(&a, area, wheel(MouseEventKind::ScrollLeft)), Some(Mouse::Cols(-1)));
        a.mouse(Mouse::Menu(1));
        let (_, lines) = render(&mut a, w - 2, h - 3);
        let any = lines[2][..lines[2].find("any").unwrap()].chars().count() as u16 + 1;
        assert_eq!(hit(&a, area, click(any, 3)), Some(Mouse::Item(0)));
        assert_eq!(hit(&a, area, click(any, 4)), Some(Mouse::Item(1)));
        assert_eq!(hit(&a, area, click(any, 5)), Some(Mouse::Item(2)));
        assert_eq!(hit(&a, area, click(any, 6)), None, "the box's bottom border");
        assert_eq!(hit(&a, area, right(any, 4)), Some(Mouse::Item(1)), "either button");
        assert_eq!(hit(&a, area, drag(any, 4)), None, "a drag over a list does nothing");
        assert_eq!(hit(&a, area, right(w - 2, 3)), Some(Mouse::Outside), "any click outside closes it");
        assert_eq!(hit(&a, area, click(w - 2, 3)), Some(Mouse::Outside), "outside: closes it");
    }

    #[test]
    fn favorite_models_get_a_star() {
        let mut a = app();
        a.store.toggle_favorite("coding", "opus");
        a.store.toggle_favorite("agentic", "opus");
        let (_, lines) = render(&mut a, 120, 5);
        let row = |m: &str| lines.iter().find(|l| l.contains(m)).unwrap().clone();
        assert!(row("opus").contains("★") && !row("flash").contains("★"), "★ before the favorite model's name");
        // The status bar names the tasks for the model under the cursor.
        for _ in 0..2 {
            let (_, lines) = render(&mut a, 120, 5);
            match a.current().unwrap().key.as_str() {
                "opus" => assert!(lines[4].contains("★ coding ★ agentic"), "status names the tasks: {}", lines[4]),
                _ => assert!(!lines[4].contains('★'), "flash is the favorite for nothing: {}", lines[4]),
            }
            a.key(KeyCode::Char('j').into());
        }
    }

    #[test]
    fn star_takes_its_tasks_colour() {
        assert_eq!(TASK.len(), TASKS.len(), "a colour per task");
        let mut a = app();
        a.store.toggle_favorite("coding", "opus");
        a.store.toggle_favorite("agentic", "flash");
        a.key(KeyCode::Char('j').into()); // the bar is on flash, so opus keeps its colours
        let (buf, lines) = render(&mut a, 120, 5);
        // The cell where `pat` starts on line `y`; the lines hold wide glyphs before it.
        let at = |y: usize, pat: &str| (lines[y][..lines[y].find(pat).unwrap()].chars().count() as u16, y as u16);
        let star = |m: &str| buf[at(lines.iter().position(|l| l.contains(m)).unwrap(), "★")].fg;
        assert_eq!(star("opus"), STAR, "no task picked: a gold ★ that says favorite for some task");
        assert_eq!(buf[at(0, "Coding")].fg, ACCENT, "headers are one colour, a task's column too");
        assert_eq!(buf[at(4, "★")].fg, task_color("agentic"), "and in the status bar");
        let opus = lines.iter().position(|l| l.contains("opus")).unwrap();
        let name_x = at(opus, "opus").0;
        a.store.toggle_favorite("reasoning", "opus");
        let (_, lines) = render(&mut a, 120, 5);
        assert_eq!(lines[opus].matches('★').count(), 1, "one ★ however many tasks: {}", lines[opus]);
        assert_eq!(at(opus, "opus").0, name_x, "so the names never shift");
        a.key(KeyCode::Char('k').into()); // the bar on opus: the status bar names its tasks
        let (buf, lines) = render(&mut a, 120, 5);
        assert!(lines[4].contains("★ coding ★ reasoning"), "each in the status bar too: {}", lines[4]);
        let sx = lines[4].find("★ reasoning").map(|i| lines[4][..i].chars().count() as u16).unwrap();
        assert_eq!(buf[(sx, 4)].fg, task_color("reasoning"));
        // With a task picked, the ★ is that task's favorite in that task's colour.
        a.task = fit::task("agentic");
        for k in ['k', 'j'] {
            // Off flash: on the selection bar the ★ would be plain like everything else.
            if a.current().unwrap().key == "flash" {
                a.key(KeyCode::Char(k).into());
            }
        }
        let (buf, lines) = render(&mut a, 120, 5);
        let opus = lines.iter().position(|l| l.contains("opus")).unwrap();
        let flash = 5 - opus;
        assert!(!lines[opus].contains('★') && lines[flash].contains('★'), "only agentic's favorite: {lines:?}");
        let x = lines[flash][..lines[flash].find('★').unwrap()].chars().count() as u16;
        assert_eq!(buf[(x, flash as u16)].fg, task_color("agentic"));
        // The recommend panel: the task name and the favorite's ★ in the task's colour, the model in its price level's.
        let mut data = std::mem::take(&mut a.data);
        for (m, pct) in data.models.iter_mut().zip([90.0, 60.0]) {
            m.fit.insert("coding".into(), pct);
        }
        a.set_data(data);
        let lines = recommend(&a, 200);
        let spans: Vec<&Span> = lines.iter().flat_map(|l| l.spans.iter()).collect();
        let pill = spans.iter().find(|s| s.content == " coding ").unwrap();
        assert_eq!(pill.style.fg, Some(task_color("coding")));
        let star = spans.iter().position(|s| s.content == "★ " && s.style.fg == Some(task_color("coding"))).unwrap();
        assert!(spans[star + 1].content.starts_with("opus "), "★ then the name: {:?}", spans[star + 1]);
        assert_eq!(spans[star + 1].style.fg, Some(LEVEL[level(5.0)]), "the name keeps its price level");
    }

    #[test]
    fn narrow_table_drops_columns_and_hints_without_panicking() {
        let mut a = app();
        let (_, lines) = render(&mut a, 46, 4);
        assert_eq!(
            words(&lines[0]),
            ["#", "✓", "★", "✗", "Model", "Dev", "▾", "│", "▼Price", "▾"],
            "the cursor starts on Price"
        );
        assert!(layout(46, &a).more, "columns cut off on the right");
        assert!(!layout(400, &a).more, "all columns fit");
        // Rows above or below the window are reported for the frame's ▲ ▼.
        let mut data = std::mem::take(&mut a.data);
        data.models.push(model("mini", "openai", Some(100.0), 0.5));
        a.set_data(data);
        let mut buf = Buffer::empty(Rect::new(0, 0, 46, 3));
        assert_eq!(table(&mut buf, Rect::new(0, 0, 46, 3), &mut a), (true, false, true), "one row hidden below");
        a.key(KeyCode::Char('G').into());
        assert_eq!(table(&mut buf, Rect::new(0, 0, 46, 3), &mut a), (true, true, false), "then above");
        let mut buf = Buffer::empty(Rect::new(0, 0, 46, 9));
        assert_eq!(table(&mut buf, Rect::new(0, 0, 46, 9), &mut a), (true, false, false), "all rows fit");
        assert!(lines[3].ends_with("? help"), "{}", lines[3]);
        assert!(!lines[3].contains("m mark"), "hints that do not fit are dropped whole");
        // Moving past the right edge scrolls the columns right of Dev; Model and Dev stay.
        a.col = NCOLS - 1;
        let (_, lines) = render(&mut a, 46, 4);
        assert_eq!(words(&lines[0]), ["#", "✓", "★", "✗", "Model", "Dev", "▾", "‹│", "Notes"]);
        a.col = VIA;
        let (_, lines) = render(&mut a, 47, 4);
        assert_eq!(words(&lines[0]), ["#", "✓", "★", "✗", "Model", "Dev", "▾", "‹│", "Via", "▾"]);
        for _ in 0..2 {
            a.key(KeyCode::Char('h').into());
        }
        let (_, lines) = render(&mut a, 54, 4);
        assert_eq!(words(&lines[0])[7..], ["‹│", "Reason", "Code/$"], "scrolls back only as far as needed");
        a.key(KeyCode::Char('l').into());
        let (_, lines) = render(&mut a, 54, 4);
        assert_eq!(words(&lines[0])[7..], ["‹│", "Reason", "Code/$"], "{}", lines[0]);
        a.col = 0;
        let (_, lines) = render(&mut a, 46, 4);
        assert_eq!(words(&lines[0]), ["#", "✓", "★", "✗", "Model", "Dev", "▾", "│", "▼Price", "▾"]);
        for (w, h) in [(1, 1), (3, 2), (0, 0), (30, 3), (12, 1)] {
            let area = Rect::new(0, 0, w, h);
            let mut buf = Buffer::empty(area);
            table(&mut buf, area, &mut a);
            status(&mut buf, area, &a);
        }
    }

    #[test]
    fn selected_column_is_bold_with_a_thick_rule() {
        let mut a = app();
        a.col = ECI;
        let area = Rect::new(0, 0, 200, 4);
        let mut buf = Buffer::empty(area);
        table(&mut buf, area, &mut a);
        let l = layout(200, &a);
        let &(_, x, w) = l.cols.iter().find(|&&(i, ..)| i + 2 == ECI).unwrap();
        let &(_, px, _) = l.cols.iter().find(|&&(i, ..)| i + 2 == PRICE).unwrap();
        assert_eq!(buf[(x, 1)].symbol(), "━", "a thick rule under the cursor's column");
        assert_eq!(buf[(x + w - 1, 1)].symbol(), "━");
        assert_eq!(buf[(px, 1)].symbol(), "─", "a thin one elsewhere");
        assert!(buf[(x + w - 1, 3)].modifier.contains(BOLD), "its cells are bold");
        assert!(!buf[(px + 1, 3)].modifier.contains(BOLD), "other columns are not");
    }

    #[test]
    fn scrolls_to_keep_the_selection_visible() {
        let mut a = app();
        let mut data = std::mem::take(&mut a.data);
        data.models.extend((0..20).map(|i| model(&format!("m{i}"), "x", Some(i as f64), 1.0)));
        a.set_data(data);
        a.col = ECI;
        a.key(KeyCode::Char('s').into());
        a.table.select(Some(15));
        let (_, lines) = render(&mut a, 60, 6);
        assert_eq!(words(&lines[4])[..5], ["16", "☐", "☆", "·", "m6"], "row 15 (m6) is the last of the 3 visible rows");
        a.table.select(Some(0));
        let (_, lines) = render(&mut a, 60, 6);
        assert_eq!(words(&lines[2])[..5], ["1", "☐", "☆", "·", "opus"], "scrolls back up");
    }

    #[test]
    fn prompts_show_the_text_and_the_cursor() {
        let mut a = app();
        a.input = Input::Search { cur: 3, was: String::new() };
        a.query = "gem".into();
        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 1));
        assert_eq!(status(&mut buf, Rect::new(0, 0, 40, 1), &a), Some(8 + 1 + 4));
        let line: String = (0..14).map(|x| buf[(x, 0)].symbol()).collect();
        assert_eq!(line, " SEARCH  /gem ");
        a.input = Input::Search { cur: 1, was: String::new() };
        assert_eq!(status(&mut buf, Rect::new(0, 0, 40, 1), &a), Some(8 + 1 + 2), "the cursor sits inside the text");
        a.input = Input::Bound { col: 3, min: true, text: "4".into(), cur: 1 };
        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 1));
        status(&mut buf, Rect::new(0, 0, 40, 1), &a);
        assert_eq!((0..15).map(|x| buf[(x, 0)].symbol()).collect::<String>(), " BOUND  $in ≥ 4");
    }

    #[test]
    fn compare_scrolls_models_sideways() {
        let a = app();
        let ms: Vec<&Model> =
            ["opus", "flash"].iter().map(|k| a.data.models.iter().find(|m| m.key == *k).unwrap()).collect();
        let row = |v: &Vec<Line>| v.iter().find(|l| l.to_string().starts_with("model ")).unwrap().to_string();
        let (full, first) = compare(&ms, 1, 1, 200, "");
        assert!(row(&full).contains("opus") && row(&full).contains("flash"));
        assert_eq!(first, 0, "everything fits, so nothing scrolls off");
        assert!(!row(&full).contains('‹') && !row(&full).contains('›'), "no scroll marks when all fit");
        let (cut, first) = compare(&ms, 1, 0, 20, "");
        assert!(!row(&cut).contains("opus") && row(&cut).contains("flash"), "scrolls to show the selection");
        assert_eq!(first, 1);
        assert!(cut.iter().any(|l| l.to_string().starts_with("models 2-2 of 2")));
        assert!(row(&cut).contains('‹') && !row(&cut).contains('›'), "‹ marks models off to the left");
        let (past, first) = compare(&ms, 7, 0, 20, "");
        assert!(row(&past).contains("flash") && first == 1, "a cursor past the models lands on the last");
        let (back, first) = compare(&ms, 0, 1, 20, "");
        assert!(row(&back).contains("opus") && !row(&back).contains("flash"));
        assert_eq!(first, 0);
        assert!(row(&back).ends_with("opus ›") && !row(&back).contains('‹'), "› marks models off to the right");
        let labels = |v: &Vec<Line>| {
            v.iter()
                .map(Line::to_string)
                .filter(|l| l.contains("  "))
                .map(|l| l.split("  ").next().unwrap().trim().to_string())
                .collect::<Vec<_>>()
        };
        let rule = full.iter().position(|l| l.to_string().starts_with("model ")).unwrap() + 1;
        assert!(full[rule].to_string().chars().all(|c| c == '─'), "a rule under the model row, as in the table");
        let (some, _) = compare(&ms, 0, 0, 200, "eci");
        assert_eq!(
            labels(&some).iter().filter(|l| !l.is_empty()).collect::<Vec<_>>(),
            ["model", "ECI"],
            "the model row stays as the header"
        );
        let (typo, _) = compare(&ms, 0, 0, 200, "contxt");
        assert!(labels(&typo).contains(&"context".to_string()), "a typo is forgiven when nothing matches");
    }

    #[test]
    fn theme_list_scrolls_to_the_bar() {
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(60, 8)).unwrap();
        let mut a = app();
        a.key(KeyCode::Char('t').into());
        for _ in 1..THEMES.len() {
            a.key(KeyCode::Char('j').into());
        }
        term.draw(|f| draw(&mut a, f)).unwrap();
        let buf = term.backend().buffer();
        let text: String = (0..8).flat_map(|y| (0..60).map(move |x| buf[(x, y)].symbol())).collect();
        assert!(text.contains(THEMES[THEMES.len() - 1].0) && text.contains('▲'), "{text}");
    }

    #[test]
    fn every_view_draws_at_any_size() {
        for (w, h) in [(120, 30), (60, 10), (20, 5), (4, 4), (3, 3)] {
            let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h)).unwrap();
            for view in [View::Table, View::Help, View::Detail, View::Compare, View::Recommend] {
                let mut a = app();
                a.store.marked = vec!["opus".into(), "flash".into()];
                a.view = view;
                a.task_cur = 3;
                term.draw(|f| draw(&mut a, f)).unwrap();
            }
        }
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 8)).unwrap();
        let mut a = app();
        a.col = ECI;
        term.draw(|f| draw(&mut a, f)).unwrap();
        let top: String = (0..100).map(|x| term.backend().buffer()[(x, 0)].symbol()).collect();
        assert!(top.starts_with("╭──"), "{top}");
        assert!(top.ends_with("─ ▼ by Price ╮"), "{top}");
        let (l, r) = top.split_once(" ECI: Epoch AI's overall capability index ").unwrap();
        assert!(l.chars().count().abs_diff(r.chars().count()) <= 1, "centred: {top}");
        let bottom: String = (0..100).map(|x| term.backend().buffer()[(x, 6)].symbol()).collect();
        assert!(bottom.contains("models.dev + Epoch AI"), "{bottom}");
        assert_eq!(a.page, 3, "8 lines minus status bar, two borders, the header and its rule");
        // The right border marks columns off to the right, the left one rows below, then above.
        let mut data = std::mem::take(&mut a.data);
        for i in 0..6 {
            data.models.push(model(&format!("m{i}"), "openai", Some(100.0), 0.5));
        }
        a.set_data(data);
        term.draw(|f| draw(&mut a, f)).unwrap();
        let edge = |term: &ratatui::Terminal<ratatui::backend::TestBackend>, x: u16, y: u16| {
            term.backend().buffer()[(x, y)].symbol().to_string()
        };
        let marks = [edge(&term, 99, 1), edge(&term, 0, 2), edge(&term, 99, 2), edge(&term, 0, 3), edge(&term, 0, 5)];
        assert_eq!(marks, ["›", "├", "┤", "│", "▼"], "the rule joins the frame");
        a.key(KeyCode::Char('G').into());
        term.draw(|f| draw(&mut a, f)).unwrap();
        assert_eq!([edge(&term, 0, 3), edge(&term, 0, 5)], ["▲", "│"]);
        // A tall overlay stops above the status bar, which shows its keys.
        a.key(KeyCode::Char('?').into());
        term.draw(|f| draw(&mut a, f)).unwrap();
        let bar: String = (0..100).map(|x| edge(&term, x, 7)).collect();
        assert!(bar.starts_with(" HELP "), "{bar}");
    }

    #[test]
    fn overlays_fit_and_scroll() {
        let area = Rect::new(0, 0, 30, 6);
        let mut buf = Buffer::empty(area);
        let mut scroll = 99;
        overlay(&mut buf, area, "keys", help(""), &mut scroll);
        assert_eq!(buf[(0, 0)].symbol(), "╭");
        assert_eq!(buf[(0, 0)].fg, ACCENT);
        assert_eq!(scroll as usize, help("").len() - 4, "scroll is clamped to the content");
        assert_eq!((buf[(0, 1)].symbol(), buf[(0, 4)].symbol()), ("▲", "│"), "at the end: lines above only");
        scroll = 0;
        overlay(&mut buf, area, "keys", help(""), &mut scroll);
        assert_eq!((buf[(0, 1)].symbol(), buf[(0, 4)].symbol()), ("│", "▼"), "at the top: lines below only");
        let a = app();
        let text: Vec<String> = detail(&a.data.models[0], &a.store).iter().map(ToString::to_string).collect();
        assert!(text.iter().any(|l| l.starts_with("  developer:  anthropic")), "{text:?}");
        let rows = compare(&a.marked_models(), 0, 0, 200, "").0;
        assert!(rows[0].to_string().starts_with("verdict"), "the verdict comes first");
        assert!(rows.iter().any(|l| l.to_string().starts_with("model")));
    }

    #[test]
    fn wrapped_breaks_between_items_and_keeps_punctuation() {
        let label = vec![Span::raw("  b: ")];
        let items = ["aa", "bb", "cc", "dd"].map(Line::from).to_vec();
        let text: Vec<String> = wrapped(label, items, &Span::raw(", "), 13).iter().map(ToString::to_string).collect();
        assert_eq!(text, ["  b: aa, bb,", "     cc, dd"], "the kept comma stays inside the width");
        let text: Vec<String> = wrapped(vec![], vec![Line::from("a"), Line::from("b")], &Span::raw(" "), 2)
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(text, ["a", "b"], "a space at a break is dropped");
    }

    #[test]
    fn searches_show_what_matched() {
        let lit_text = |lines: &[Line]| -> Vec<String> {
            let hit = |s: &&Span| s.style.add_modifier.contains(Modifier::UNDERLINED);
            lines.iter().flat_map(|l| l.spans.iter().filter(hit).map(|s| s.content.to_string())).collect()
        };
        assert_eq!(found("Gruvbox gRUV", "ruv"), [1..4, 9..12]);
        let hits = lit_text(&help("THEME"));
        assert!(!hits.is_empty() && hits.iter().all(|h| h.eq_ignore_ascii_case("theme")), "{hits:?}");
        let items = vec![("nord".to_string(), Effect::Theme("nord")), ("gruvbox".into(), Effect::Theme("gruvbox"))];
        let lines = choice_lines(&items, 0, "uv");
        assert_eq!(lit_text(&lines), ["uv"]);
        // On the selection bar a hit is bold and underlined but gets no colour.
        let lines = choice_lines(&items, 0, "gr");
        let hit = lines[0].spans.iter().find(|s| s.content == "gr").unwrap();
        assert_eq!(hit.style.fg, None);
    }

    #[test]
    fn recommend_panel_wraps_and_highlights_the_cursor() {
        let mut a = app();
        a.task_cur = TASKS.iter().position(|t| t.name == "vision").unwrap();
        let lines = recommend(&a, 60);
        let text: Vec<String> = lines.iter().map(ToString::to_string).collect();
        assert!(text[0].starts_with("best per price: the top model"), "{}", text[0]);
        let gap = text.iter().position(String::is_empty).unwrap();
        assert!(gap > 1 && text[..gap].join(" ") == frontier_legend(false), "the legend wraps: {:?}", &text[..gap]);
        assert_eq!(
            text.iter().filter(|l| l.is_empty()).count(),
            TASKS.len() + 1,
            "a block per task, then the CLI line"
        );
        let names: Vec<&str> = text.iter().filter_map(|l| l.strip_prefix(' ')?.split_whitespace().next()).collect();
        assert_eq!(names[..2], ["overall", "use"], "overall comes first");
        // The picked model on the cursor's task is a reverse-video bar; other tasks have none.
        let mut b = app();
        let mut data = std::mem::take(&mut b.data);
        for (m, pct) in data.models.iter_mut().zip([90.0, 60.0]) {
            m.fit.insert("overall".into(), pct);
        }
        b.set_data(data);
        (b.view, b.task_sel) = (View::Recommend, 1);
        let bars: Vec<String> = recommend(&b, 200)
            .iter()
            .flat_map(|l| l.spans.iter())
            .filter(|s| s.style.add_modifier.contains(Modifier::REVERSED))
            .map(|s| s.content.to_string())
            .collect();
        assert_eq!(bars.len(), 2, "the task name and one model: {bars:?}");
        assert!(bars[1].starts_with("opus "), "the best of overall: {bars:?}");
        let vision = text.iter().position(|l| l.starts_with(" vision ")).unwrap();
        let models = text[vision..].iter().position(|l| l.starts_with("  best per price:  "));
        assert!(models.is_some_and(|n| n <= 3), "every task lists its models: {:?}", &text[vision..vision + 4]);
        let long: Vec<&String> = text.iter().filter(|l| l.chars().count() > 60).collect();
        assert!(long.is_empty(), "wrapped to the width: {long:?}");
        let cursor = lines[vision].spans.iter().find(|s| s.style.add_modifier.contains(Modifier::REVERSED)).unwrap();
        assert_eq!(cursor.content, " vision ");
    }
}
