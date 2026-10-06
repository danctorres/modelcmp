//! Terminal shell: owns the terminal, the input loop and the refresh thread, and draws `App`.
//! Redraws only on input or when a background refresh lands.

//!
//! The table is drawn straight into the buffer: one pass over the visible rows, no widget
//! allocations per frame. Colours come from the terminal's 16-colour palette, so a themed
//! terminal themes modelcmp too, and nothing paints a background over a transparent one but a
//! marked row's fill and the cursor's.

use crate::app::{
    App, BOXES, COLS, Download, ECI, EXCLUDED, Edit, Effect, FAV, GROUPS, HELP, HELP_TAB, Input, Kind, List, MARKED,
    Mouse, NOTES, PRICE, RECOMMEND, Stop, TABS, VIA, View, What, YOURS, box_slot, choice_rows, hidden, menu_rows,
    on_price, shown,
};
use crate::data::{self, Data, Model};
use crate::fit::{self, TASKS};
use crate::store::Store;
use crate::view::{
    CUSTOM_ABOUT, CUSTOM_WHEN, NO_ACCESS, OUT_OF_REACH, Palette, THEMES, TIERS, age, compare_rows, detail_rows, hits,
    level, level_label, money, priced, truncate, verdict,
};
use ratatui::buffer::Buffer;
use ratatui::crossterm::cursor::Show;
use ratatui::crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, Event, KeyCode,
    KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::crossterm::execute;
use ratatui::layout::{Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Widget};
use ratatui::{DefaultTerminal, Frame};
use std::borrow::Cow;
use std::io::IsTerminal;
use std::ops::Range;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

/// What a refresh sends: the data before its slow harnesses, when it has any to wait for, then
/// its outcome.
enum Refreshed {
    Early(Data),
    Done(Result<Data, data::Failure>, crate::app::Tools),
}

/// A refresh under way: where its result comes, and its steps for the frame's count.
type Refresh = (Receiver<Refreshed>, data::Steps);

/// `ask`: no source was ever picked, nor given with `--source`: open on the `B` chooser, with
/// no data until one is picked.
pub fn run(mut store: Store, force: bool, ask: bool) -> Result<(), String> {
    // Run by an agent or a script there is no terminal to draw on: say so before any download or
    // harness is started, not after.
    if !std::io::stdout().is_terminal() {
        return Err("the TUI needs a terminal; see modelcmp --help".into());
    }
    // The first start of a version, so the first ever or the first after an upgrade, plays the
    // intro. Noted now, while the store is fresh from the file.
    let new = store.seen != env!("CARGO_PKG_VERSION");
    if new {
        let _lock = crate::store::lock(&crate::store::path());
        store.reload_if_changed();
        store.seen = env!("CARGO_PKG_VERSION").into();
        // Unsaved, it plays again at the next start; the store's own warning says why.
        let _ = store.save();
    }
    // Asked first: reading the reply takes whatever else is queued with it, and here that is
    // nothing.
    let term_bg = cfg!(unix).then(terminal_bg).flatten();
    // Start from the cache however old, or with no cache from an empty table, and download behind
    // the intro and the table (`--refresh` too), so nothing waits for it and a failure shows in
    // the frame.
    let mut app = App::new((!ask).then(data::load_cache).flatten().unwrap_or_default(), store);
    app.term_bg = term_bg;
    let (mut rx, mut pre) = (None, None);
    if ask {
        app.first_start = true;
        app.ask_source();
        // No source is picked yet: the default's download starts under the intro, for the pick
        // to take up, unless the CLI has left a cache that will do.
        pre = (force || data::load_cache().is_none_or(|d| d.stale())).then(spawn_refresh);
    } else if (force || app.data.stale()) && app.refresh().is_some() {
        rx = Some(spawn_refresh());
    }
    // After the refresh starts, which clears the status: it is your marks and notes that are at
    // stake. The first start keeps it for the pick, as a key under its question clears the status.
    if !ask && let Some(w) = app.store.warning.take() {
        app.report(Err(w));
    }
    let mut terminal =
        ratatui::try_init().map_err(|e| format!("the TUI needs a terminal ({e}); see modelcmp --help"))?;
    // ratatui's panic hook restores the terminal but leaves mouse reporting on, and bracketed
    // paste, which keeps pasted text (ctrl+v in some terminals) from arriving as keys. And the
    // cursor hidden, which only its terminal's drop shows again: a release build aborts instead.
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(std::io::stdout(), DisableMouseCapture, DisableBracketedPaste, Show);
        hook(info);
    }));
    let _ = execute!(std::io::stdout(), EnableMouseCapture, EnableBracketedPaste);
    let res = if ask || new { intro(&app, &mut terminal).map_err(|e| e.to_string()) } else { Ok(()) }
        .and_then(|()| event_loop(&mut app, &mut terminal, rx, pre));
    let _ = execute!(std::io::stdout(), DisableMouseCapture, DisableBracketedPaste);
    ratatui::restore();
    let Some(cmds) = res? else { return Ok(()) };
    // On the terminal's own screen, where the upgrade's output shows. One that fails has still
    // closed the TUI.
    upgrade(&cmds).inspect_err(|_| {
        let _lock = crate::store::lock(&crate::store::path());
        let _ = close(&mut app.store);
    })
}

/// The selection is the shortlist of one session: closing the TUI clears it.
fn close(store: &mut Store) -> Result<(), String> {
    store.reload_if_changed();
    if store.marked.is_empty() {
        return Ok(());
    }
    store.marked.clear();
    store.save().map_err(|e| format!("could not save: {e}"))
}

const REPO: &str = "https://github.com/danctorres/modelcmp";

/// The commands that upgrade the binary at `exe` to release `new`, by what installed it: Homebrew
/// keeps it in its Cellar, cargo in its `bin`. None for one put there by hand.
fn upgrade_cmds(exe: &std::path::Path, new: &str) -> Option<Vec<Vec<String>>> {
    let exe = exe.to_string_lossy();
    let tag = format!("v{new}");
    let cmds: &[&[&str]] = if exe.contains("/Cellar/") {
        // `upgrade` alone goes by a list of formulae up to a day old.
        &[&["brew", "update"], &["brew", "upgrade", "danctorres/tap/modelcmp"]]
    } else if exe.contains("/.cargo/bin/") {
        &[&["cargo", "install", "--git", REPO, "--tag", &tag]]
    } else {
        return None;
    };
    Some(cmds.iter().map(|c| c.iter().map(|a| (*a).to_string()).collect()).collect())
}

/// Run the upgrade, then start the new modelcmp in this one's place: its intro shows the version.
fn upgrade(cmds: &[Vec<String>]) -> Result<(), String> {
    for cmd in cmds {
        let line = cmd.join(" ");
        eprintln!("$ {line}");
        match Command::new(&cmd[0]).args(&cmd[1..]).status() {
            Ok(s) if s.success() => {}
            Ok(s) => return Err(format!("{line}: {s}")),
            Err(e) => return Err(format!("{line}: {e}")),
        }
    }
    // By the name it was started with, not this binary's path: Homebrew's holds the version.
    let mut args = std::env::args_os();
    let name = args.next().unwrap_or_else(|| "modelcmp".into());
    // Homebrew's formula follows the release by some minutes: until then its upgrade does nothing
    // and says so here, where starting the same modelcmp again would wipe it off the screen.
    let version = Command::new(&name).arg("--version").output().map(|o| o.stdout).unwrap_or_default();
    if String::from_utf8_lossy(&version).trim() == concat!("modelcmp ", env!("CARGO_PKG_VERSION")) {
        return Err(concat!("still modelcmp v", env!("CARGO_PKG_VERSION"), ": try again in a while").into());
    }
    let mut new = Command::new(name);
    new.args(args);
    #[cfg(unix)]
    return Err(format!("upgraded, but could not start it: {}", std::os::unix::process::CommandExt::exec(&mut new)));
    #[cfg(not(unix))]
    new.status().map(|_| ()).map_err(|e| format!("upgraded, but could not start it: {e}"))
}

/// The terminal's background colour, for a marked row's faint fill with the terminal's own
/// colours. It is asked for with OSC 11, then for the cursor position, which every terminal
/// answers: replies come in the order asked, so once the position is in, the colour's either is
/// queued or is not coming, with no wait to guess at and no late reply read as keys. crossterm
/// knows no OSC reply, so it comes through as the keys that spell it. The fill is a 24-bit
/// colour, which the 16 never were, so a terminal that does not say it draws those is not asked:
/// one may answer here and still garble them. It says so in `COLORTERM`, or, as `ssh` and `sudo`
/// drop that, with a `TERM` only a terminal that draws them sets. Under WSL neither is set, and
/// every Windows console that runs it draws them.
/// ponytail: one that never answers the position costs crossterm's 2 s at each start, and a reply
/// later than that is read as keys; read the tty here with a wait of our own if one turns up.
fn terminal_bg() -> Option<u32> {
    use ratatui::crossterm::terminal::{disable_raw_mode, enable_raw_mode};
    use std::io::Write;
    let term = std::env::var("TERM").unwrap_or_default();
    if !matches!(std::env::var("COLORTERM").as_deref(), Ok("truecolor" | "24bit"))
        && !["kitty", "alacritty", "ghostty", "foot", "wezterm", "direct"].iter().any(|t| term.contains(t))
        && std::env::var_os("WSL_DISTRO_NAME").is_none()
    {
        return None;
    }
    // Raw, so the reply is not echoed.
    enable_raw_mode().ok()?;
    let mut out = std::io::stdout();
    let mut reply = String::new();
    if out.write_all(b"\x1b]11;?\x07").and_then(|()| out.flush()).is_ok() {
        // With no position in, what did come is still read, or it is left for the table as keys.
        let _ = ratatui::crossterm::cursor::position();
        while event::poll(Duration::ZERO).unwrap_or(false) {
            match event::read() {
                Ok(Event::Key(k)) => {
                    if let KeyCode::Char(c) = k.code {
                        reply.push(c);
                    }
                }
                Ok(_) => {}
                Err(_) => break,
            }
        }
    }
    let _ = disable_raw_mode();
    parse_bg(&reply)
}

/// The colour in an OSC 11 reply, `]11;rgb:1e1e/1e1e/2e2e`: the high byte of each channel.
fn parse_bg(reply: &str) -> Option<u32> {
    let mut rgb = reply.split_once("rgb:")?.1.split('/').map(|c| u32::from_str_radix(c.get(..2)?, 16).ok());
    Some(rgb.next()?? << 16 | rgb.next()?? << 8 | rgb.next()??)
}

/// Half blocks, two square pixels to a cell.
const LOGO: [&str; 8] = [
    "                          ▄▄            ███                                ",
    "                          ██             ██                                ",
    "███████▄  ▄█████▄    ▄██████   ▄█████▄   ██    ▄█████▄   ███████▄ ██▄████▄ ",
    "██ ██ ██ ██▀   ▀██  ██▀   ██  ██▀   ▀██  ██   ██▀   ▀██  ██ ██ ██ ██▀   ▀██",
    "██ ██ ██ ██     ██  ██    ██  █████████  ██   ██         ██ ██ ██ ██     ██",
    "██ ██ ██ ██▄   ▄██  ██▄   ██  ██▄   ▄▄▄  ██▄  ██▄   ▄██  ██ ██ ██ ███▄▄▄██▀",
    "██ ██ ██  ▀█████▀    ▀████▀██  ▀█████▀    ▀██  ▀█████▀   ██ ██ ██ ██ ▀▀▀▀  ",
    "                                                                  ▀▀       ",
];
/// Under the wordmark, after a blank row.
const TAGLINE: &str = "compare models, pick favorites, get recommendations";

/// Where the wordmark goes in `a`: its corner, and the rows under it that the tagline and the
/// version take, as far as there is room for them (0, 2 or 3). It is centred with `below` more
/// rows kept free under those. None in an area too small.
fn logo_at(a: Rect, below: u16) -> Option<(u16, u16, u16)> {
    let (w, rows) = (LOGO[0].chars().count() as u16, LOGO.len() as u16);
    if a.width < w || a.height < rows + below {
        return None;
    }
    // A blank row and the tagline, then the version.
    let under = match a.height - rows - below {
        0 | 1 => 0,
        2 => 2,
        _ => 3,
    };
    Some((a.x + (a.width - w) / 2, a.y + (a.height - rows - under - below) / 2, under))
}

/// The first start asks for the benchmarks under the wordmark, where the intro leaves it: where
/// the wordmark is (`logo_at`), and the rows under it that the question's box is centred in. None
/// once a source is picked, on a screen too small for both, which asks over the table, and with
/// neither the question nor its API key prompt open, as when the key could not be saved.
fn splash(app: &App, area: Rect) -> Option<((u16, u16, u16), Rect)> {
    if !app.first_start || !matches!(app.input, Input::Choose { kind: Kind::Source, .. } | Input::Key { .. }) {
        return None;
    }
    // Above the status bar, where a search of the list and an API key are typed.
    let body = Rect { height: area.height.checked_sub(1)?, ..area };
    // A blank row, then the box: its border, a line per source and the key hint.
    let ask = data::Source::ALL.len() as u16 + 4;
    let at = logo_at(body, ask)?;
    Some((at, Rect { y: at.1 + LOGO.len() as u16 + at.2 + 1, height: ask - 1, ..body }))
}

/// The wordmark with its corner at `(x, y)`, each cell in the style its column and row give, and
/// under it, where `under` leaves them room (`logo_at`), the first `typed` bytes of the tagline
/// and, once that is whole, the version. Rows past the end of `a` are left out.
fn wordmark(
    buf: &mut Buffer,
    a: Rect,
    (x, y, under): (u16, u16, u16),
    typed: usize,
    style: impl Fn(usize, usize) -> Style,
) {
    for (r, row) in LOGO.iter().enumerate() {
        let yr = y + r as u16;
        if yr >= a.bottom() {
            break;
        }
        for (c, ch) in row.chars().enumerate() {
            buf[(x + c as u16, yr)].set_char(ch).set_style(style(c, r));
        }
    }
    let (rows, tw) = (LOGO.len() as u16, TAGLINE.len());
    if under > 0 && typed > 0 {
        buf.set_string(a.x + (a.width - tw as u16) / 2, y + rows + 1, &TAGLINE[..typed], fg(Color::Reset));
        if under == 3 && typed == tw {
            let v = concat!("v", env!("CARGO_PKG_VERSION"));
            buf.set_string(a.x + (a.width - v.len() as u16) / 2, y + rows + 2, v, fg(MUTED));
        }
    }
}

/// The intro of the first launch, and of the first after an upgrade: the wordmark dim, gliding up
/// from below the screen and easing to a stop in the middle, then a rainbow rolling across it on a
/// diagonal, each cell running red to blue before it settles on the accent, the tagline typing in
/// under it as the rainbow passes and the version showing under that once it is whole. Any key
/// skips it, and is not passed on; a terminal too small for it skips it too. On the first start
/// it stops above the question that follows (`splash`), and stays.
fn intro(app: &App, terminal: &mut DefaultTerminal) -> std::io::Result<()> {
    // The hue wheel up to the accent, so the last step into magenta is a small one.
    const RAINBOW: [Color; 5] = [Color::Red, Color::Yellow, Color::Green, Color::Cyan, Color::Blue];
    /// Columns the rainbow crosses per frame.
    const SPEED: usize = 2;
    /// Frames each colour lasts in a cell, so the bands are `STEP * SPEED` columns wide.
    const STEP: usize = 3;
    /// Frames the glide takes at most: it ends as soon as the wordmark rounds into place.
    const SLIDE: usize = 20;
    let (w, rows, tw) = (LOGO[0].chars().count(), LOGO.len(), TAGLINE.len());
    let theme = palette(app);
    // The wave reaches a cell `SLIDE + (c + 2 * (rows - 1 - r)) / SPEED` frames in, the bottom row
    // first as cells are twice as tall as wide, and the last cell settles `RAINBOW.len() * STEP`
    // frames after that.
    let end = SLIDE + (w + 2 * (rows - 1)).div_ceil(SPEED) + RAINBOW.len() * STEP;
    // From 1, as frame 0 would put the wordmark just off the screen.
    let mut t = 1;
    while t <= end {
        let (mut fits, mut landed, mut asks) = (true, false, false);
        terminal.draw(|f| {
            let a = f.area();
            let at = splash(app, a).map(|s| s.0);
            asks = at.is_some();
            let Some((x, y, under)) = at.or_else(|| logo_at(a, 0)) else {
                fits = false;
                return;
            };
            // Ease out: from the bottom edge, fast at first and slowing into place.
            let left = 1.0 - (t.min(SLIDE) as f32 / SLIDE as f32);
            let off = (f32::from(a.bottom() - y) * left * left * left).round() as u16;
            landed = off == 0;
            // Each character as the wave's front passes over it: two rows under the bottom one,
            // so 4 columns ahead of the front there.
            let typed = if t > SLIDE { ((t - SLIDE) * SPEED + 4).saturating_sub((w - tw) / 2).min(tw) } else { 0 };
            let buf = f.buffer_mut();
            wordmark(buf, a, (x, y + off, under), typed, |c, r| {
                match t.checked_sub(SLIDE + (c + 2 * (rows - 1 - r)) / SPEED) {
                    None => fg(MUTED),
                    Some(k) => fg(RAINBOW.get(k / STEP).copied().unwrap_or(ACCENT)).add_modifier(BOLD),
                }
            });
            recolor(buf, theme, app.term_bg);
        })?;
        if !fits {
            return Ok(());
        }
        if landed {
            // Once it rounds into place, start the rainbow rather than hold it still.
            t = t.max(SLIDE);
        }
        // A key skips it; a pointer move or a resize does not, nor does it cut the frame short.
        // Past the deadline the wait is zero, so what is still queued is read and a key behind
        // a burst of pointer moves is not left for the table. The whole wordmark is held before
        // the table takes its place; the first start's question shows under it at once.
        let until = Instant::now() + Duration::from_millis(if t == end && !asks { 1100 } else { 25 });
        loop {
            if !event::poll(until.saturating_duration_since(Instant::now()))? {
                break;
            }
            if matches!(event::read()?, Event::Key(k) if k.kind == KeyEventKind::Press) {
                return Ok(());
            }
        }
        t += 1;
    }
    Ok(())
}

fn spawn_refresh() -> Refresh {
    let (tx, rx) = mpsc::channel();
    let steps = data::Steps::default();
    let counted = steps.clone();
    std::thread::spawn(move || {
        let early = tx.clone();
        let res = data::refresh(&counted, Some(move |d| drop(early.send(Refreshed::Early(d)))));
        // Its search of `PATH` here, not in the event loop.
        let _ = tx.send(Refreshed::Done(res, crate::app::tools()));
    });
    (rx, steps)
}

/// Returns at a quit, or with the commands of an upgrade to run once the screen is restored.
/// `pre`: the first start's download for the default source, held until a source is picked.
fn event_loop(
    app: &mut App,
    terminal: &mut DefaultTerminal,
    mut rx: Option<Refresh>,
    mut pre: Option<Refresh>,
) -> Result<Option<Vec<Vec<String>>>, String> {
    let mut dirty = true;
    // The sizes of downloads Hugging Face is asked for (`Effect::Size`), and how many are awaited.
    let (size_tx, size_rx) = mpsc::channel::<(String, Download)>();
    let mut sizing = 0usize;
    app.set_sizes(data::load_sizes());
    let mut gone = data::load_gone();
    app.set_gone(gone.keys().cloned());
    let ask_size = |key: String, base: String| {
        let tx = size_tx.clone();
        std::thread::spawn(move || {
            let size = data::gguf_repo(&base, &key).and_then(|r| r.map_or(Ok(None), |r| data::gguf_size(&r)));
            let answer = match size {
                Ok(Some(bytes)) => Download::Size(bytes),
                Ok(None) => Download::Gone,
                Err(_) => Download::NoAnswer,
            };
            let _ = tx.send((key, answer));
        });
    };
    // The last press on a cell, which a second one makes a double click (`double`), and the
    // screen row of the last press on a table row, which a drag extends from (`dragged`).
    let (mut click, mut press) = (None, None);
    loop {
        // Before the draw, so one that ended under the intro is in the first frame.
        let done = rx.as_ref().and_then(|r| match r.0.try_recv() {
            Ok(sent) => Some(sent),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                Some(Refreshed::Done(Err("refresh thread died".into()), crate::app::tools()))
            }
        });
        dirty |= done.is_some();
        match done {
            // The refresh goes on, and its news comes with the rest.
            Some(Refreshed::Early(d)) => app.set_data(d),
            Some(Refreshed::Done(res, tools)) => {
                rx = None;
                app.refreshed(res, tools);
            }
            None => {}
        }
        // One write for the answers that came together, not one each.
        let mut sized = false;
        while let Ok((key, answer)) = size_rx.try_recv() {
            sizing -= 1;
            app.sized(&key, answer);
            dirty |= answer != Download::NoAnswer;
            match answer {
                Download::Size(_) => sized = true,
                Download::Gone => data::save_gone(&mut gone, &key),
                _ => {}
            }
        }
        if sized {
            data::save_sizes(&app.sizes());
        }
        // The count of a refresh under way moves on its own, with no key pressed.
        let progress = rx.as_ref().map_or_else(String::new, |r| data::progress(&r.1));
        if progress != app.progress {
            app.progress = progress;
            dirty = true;
        }
        if dirty {
            paint(terminal, app)?;
        }
        // Block on input; wake every 200ms while a refresh is in flight, else once a minute to
        // repaint the data age in the frame.
        // A size awaited is looked for often, as `x`'s list is open for it, and the ones still to
        // ask for, of the local rows on screen, are asked once the cursor has rested, not for
        // each row it passes.
        let rest = app.size_wanted();
        let timeout = match (sizing > 0, rest, rx.is_some()) {
            (true, ..) => Duration::from_millis(30),
            (_, true, _) => Duration::from_millis(150),
            (.., true) => Duration::from_millis(200),
            _ => Duration::from_secs(60),
        };
        dirty = rx.is_none() && sizing == 0 && !rest;
        let fire = rest && sizing == 0;
        // Handle every queued event before the next draw, so a held key never falls behind.
        let mut wait = timeout;
        while event::poll(wait).map_err(|e| e.to_string())? {
            // Woken by input: draw only if some of it changes the screen.
            if wait > Duration::ZERO {
                dirty = false;
            }
            wait = Duration::ZERO;
            let size = terminal.size().map_err(|e| e.to_string())?;
            // Held until the input's change is saved, so an agent's write cannot land in between.
            let mut lock = None;
            let input = match event::read().map_err(|e| e.to_string())? {
                Event::Key(k) if k.kind == KeyEventKind::Press => {
                    click = None;
                    Some(Ok(k))
                }
                Event::Mouse(e) => {
                    // A click goes by the screen it lands on: keys before it in this batch and an
                    // agent's change of the marks are drawn first. A drag only extends the
                    // table's range, which goes by the rows, not the screen.
                    if matches!(e.kind, MouseEventKind::Down(_)) {
                        lock = Some(crate::store::lock(&crate::store::path()));
                        if app.store.reload_if_changed() {
                            app.rebuild_in_place();
                            dirty = true;
                        }
                        if dirty {
                            paint(terminal, app)?;
                            dirty = false;
                        }
                    }
                    let m = hit(app, Rect::new(0, 0, size.width, size.height), e);
                    match dragged(&mut press, e, m) {
                        Some(m) => Some(Err(double(&mut click, m, Instant::now()))),
                        None => continue,
                    }
                }
                Event::Paste(text) => {
                    app.paste(&text);
                    None
                }
                // A resize redraws; a key release, a pointer move and the like do nothing.
                Event::Resize(..) => None,
                _ => continue,
            };
            // An agent may have marked or noted a model meanwhile: act on its file, not a stale
            // copy. A click did so before finding what it is on.
            let _lock = lock.unwrap_or_else(|| {
                let held = crate::store::lock(&crate::store::path());
                if app.store.reload_if_changed() {
                    app.rebuild_in_place();
                }
                held
            });
            let effect = match input {
                Some(Ok(k)) => app.key(k),
                Some(Err(m)) => app.mouse(m),
                None => None,
            };
            {
                match effect {
                    Some(Effect::Quit) => return close(&mut app.store).map(|()| None),
                    // The selection stays: the new modelcmp starts where this one stops.
                    Some(Effect::Upgrade) => {
                        // Unresolved when it cannot be, as replaced under a running TUI it cannot:
                        // its path still says what installed it.
                        let exe = std::env::current_exe().map(|p| p.canonicalize().unwrap_or(p)).unwrap_or_default();
                        if let Some(cmds) = app.data.update().and_then(|new| upgrade_cmds(&exe, new)) {
                            return Ok(Some(cmds));
                        }
                        // Not Homebrew's nor cargo's to replace: the release's page has the binaries.
                        let url = format!("{REPO}/releases/latest");
                        app.report(match open::that_detached(&url) {
                            Ok(()) => Ok(format!("installed by hand: opening {url}")),
                            Err(e) => Err(format!("could not open {url}: {e}")),
                        });
                    }
                    Some(Effect::Save) => {
                        if let Err(e) = app.store.save() {
                            app.report(Err(format!("could not save: {e}")));
                        }
                    }
                    Some(Effect::Open(url)) => app.report(match open::that_detached(&url) {
                        // A browser is only asked to: what it does with the page is not known here.
                        Ok(()) => Ok(format!("opening {url}")),
                        Err(e) => Err(format!("could not open {url}: {e}")),
                    }),
                    Some(Effect::Copy(text)) => app.report(if copy(&text) {
                        Ok(format!("copied {text}"))
                    } else {
                        Err("no clipboard tool found".into())
                    }),
                    Some(Effect::Refresh) => {
                        data::clear_gone(&mut gone);
                        rx = Some(spawn_refresh());
                    }
                    Some(Effect::Launch(cmd)) => {
                        let line = cmd.join(" ");
                        app.report(match new_terminal(&cmd) {
                            Ok(()) => Ok(format!("opened {line} in a new terminal")),
                            Err(e) => {
                                let hint =
                                    if e.kind() == std::io::ErrorKind::NotFound { "; set $TERMINAL" } else { "" };
                                Err(format!("could not open a terminal for {line}: {e}{hint}"))
                            }
                        });
                    }
                    // Off the loop: the list is open and takes the size when it comes.
                    Some(Effect::Size(key, base)) => {
                        sizing += 1;
                        ask_size(key, base);
                    }
                    Some(Effect::Source(src)) => {
                        if let Err(e) = app.store.save() {
                            app.report(Err(format!("could not save: {e}")));
                        }
                        // A refresh under way is for the other source: drop it, or it lands here.
                        // The first start's is for the default: picked, it is the one to wait for.
                        // One that ended before the pick is taken up all the same, for its warning.
                        let pre = pre.take().filter(|_| src == data::Source::default());
                        let fetch = app.switched(data::load_cache());
                        rx = pre.or_else(|| fetch.then(spawn_refresh));
                        app.refreshing = rx.is_some();
                    }
                    // The app applies its own chooser items before they get here.
                    Some(
                        Effect::Fav(..) | Effect::Via(..) | Effect::NewTask(_) | Effect::Theme(_) | Effect::Harness(_),
                    )
                    | None => {}
                }
                if matches!(app.input, Input::None) {
                    // The first start's download is for its question: closed without a pick of
                    // the default, a later pick would put its old result over newer data.
                    pre = None;
                    // The start's warning, kept while the question was open, is said once it
                    // closes, however it does, after an error the closing gave.
                    if let Some(w) = app.store.warning.take() {
                        app.report(Err(if app.failed { format!("{}; {w}", app.status) } else { w }));
                    }
                }
            }
            dirty = true;
        }
        // No input in that time: the cursor rested.
        if fire && wait > Duration::ZERO {
            for (key, base) in app.size_ask() {
                sizing += 1;
                ask_size(key, base);
            }
        }
    }
}

/// Draws the app, in the theme's colours.
fn paint(terminal: &mut DefaultTerminal, app: &mut App) -> Result<(), String> {
    terminal
        .draw(|f| {
            draw(app, f);
            recolor(f.buffer_mut(), palette(app), app.term_bg);
        })
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// What a mouse event lands on, with the same geometry `draw` uses: the frame's inner area
/// holds the header and then the rows from `app.table.offset()`.
fn hit(app: &App, area: Rect, m: MouseEvent) -> Option<Mouse> {
    let shift = m.modifiers.contains(KeyModifiers::SHIFT);
    match m.kind {
        // Shift+wheel goes sideways, for terminals and mice that send no sideways wheel.
        MouseEventKind::ScrollDown if shift => return Some(Mouse::Cols(1)),
        MouseEventKind::ScrollUp if shift => return Some(Mouse::Cols(-1)),
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
    // Under the tabs' two lines and the frame's top border.
    let inner = Rect::new(1, 3, area.width - 2, area.height.saturating_sub(5));
    let l = layout(inner.width, app);
    let head = head(app);
    // A tab on the first two lines, or the edge left of it, over recommend too.
    let on_tabs = m.row <= area.y + 1 && !mark && !pick && !extend;
    let tab = tab_ends(app).position(|end| on_tabs && (1..end).contains(&usize::from(m.column))).map(Mouse::Tab);
    // An open list first: a click on an entry acts on it, on its frame nothing, and any click
    // outside closes it.
    let list = match &app.input {
        // Scrolled as last drawn.
        Input::Menu { col, items, list } => {
            let rows = menu_rows(items, &list.query);
            menu_box(inner, menu_x(inner, &l, *col), items, rows.len()).map(|(b, _)| (b, list.top, rows.len()))
        }
        Input::Choose { title, items, list, .. } => {
            let (within, lines) = chooser(app, area)?;
            Some((overlay_rect(within, title, &lines), list.top, choice_rows(items, &list.query).len()))
        }
        _ => None,
    };
    if let Some((rect, top, len)) = list {
        if extend {
            return None;
        }
        if !rect.contains(pos) {
            // A tab over the theme list, one itself, leaves it for that tab.
            let theme = matches!(app.input, Input::Choose { kind: Kind::Theme, .. });
            return Some(tab.filter(|_| theme).unwrap_or(Mouse::Outside));
        }
        let inner = rect.inner(ratatui::layout::Margin::new(1, 1));
        if !inner.contains(pos) {
            return None;
        }
        let k = top + (m.row - inner.y) as usize;
        // f's grid: a box is ticked, the name ticks the task's own, and the tiers' heading and
        // what follows the boxes are nothing to click.
        if let Input::Choose { kind: Kind::Fav, items, list, .. } = &app.input {
            let row = k.checked_sub(1).filter(|&r| r < len)?;
            if !matches!(items[choice_rows(items, &list.query)[row]].1, Effect::Fav(..)) {
                return Some(Mouse::Item(row));
            }
            let x = usize::from(m.column.saturating_sub(rect.x + 2));
            let col = x.saturating_sub(fav_name_w(items) + 2) / BOX_W;
            return (col < BOXES).then_some(Mouse::Tick(row, col));
        }
        if let Input::Choose { kind: Kind::Source, items, list, .. } = &app.input {
            let (label, effect) = &items[*choice_rows(items, &list.query).get(k)?];
            // Past the box's padding and the entry's own space, as `choice_lines` draws it.
            let x = usize::from(m.column - inner.x).checked_sub(2);
            if source_link(label, effect).zip(x).is_some_and(|(l, x)| l.contains(&x)) {
                return Some(Mouse::Link);
            }
        }
        return (k < len).then_some(Mouse::Item(k));
    }
    // A click outside the open panel closes it, as one outside a list does, unless it is on a
    // hint or a tab.
    let close = (!extend && app.panel.is_some_and(|r| !r.contains(pos))).then_some(Mouse::Close);
    // A hint in the status bar presses its key.
    if m.row == area.bottom() - 1 {
        if !mark && !extend && key_link(app).is_some_and(|l| l.contains(&m.column)) {
            return Some(Mouse::Link);
        }
        return (!mark && !extend).then(|| hint_at(app, area.width, m.column).map(Mouse::Key)).flatten().or(close);
    }
    if on_tabs {
        return tab.or(close);
    }
    if close.is_some() {
        return close;
    }
    // In compare and recommend a click is on what is drawn under it (`App::spots`): a plain one
    // opens it on a double click (`double`), a right one selects it, as in the table, and a ctrl
    // or shift one, with no highlight there to add to, moves to it. Elsewhere it still clears
    // the status, as a click does in the table.
    if matches!(app.view, View::Compare | View::Recommend) {
        let spot = app.spots.iter().find(|(r, _)| r.contains(pos)).map(|&(_, s)| s);
        return Some(spot.map_or(Mouse::Outside, |s| {
            if mark {
                Mouse::MarkModel(s)
            } else if extend || pick {
                Mouse::Model(s)
            } else {
                Mouse::Open(s)
            }
        }));
    }
    // A drag past the table's edges still extends the range to its nearest row; a shift click
    // there is on no row.
    if extend {
        let drag = m.kind == MouseEventKind::Drag(MouseButton::Left);
        if inner.height <= head || !(drag || (inner.y + head..inner.bottom()).contains(&m.row)) {
            return None;
        }
        let y = m.row.clamp(inner.y + head, inner.bottom() - 1);
        return Some(Mouse::Extend(app.table.offset() + (y - inner.y - head) as usize));
    }
    if !inner.contains(pos) {
        return None;
    }
    // The rule under the header, and the line `head` counts.
    if m.row > inner.y && m.row < inner.y + head {
        return None;
    }
    if m.row > inner.y {
        let n = app.table.offset() + (m.row - inner.y - head) as usize;
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
            // A cell opens what it shows, on a double click (`double`): Via the harness under
            // the pointer, and Notes the note to write.
            match col_at(&l, x) {
                Some((VIA, vx, _)) => harness_at(app, n, x - vx).map_or(Mouse::Row(n), |j| Mouse::Harness(n, j)),
                Some((col, ..)) => Mouse::Cell(n, col),
                _ => Mouse::Row(n),
            }
        });
    }
    let x = m.column - inner.x;
    // The # header goes to the first row, as `gg` does; the mark columns have no header.
    if x < l.name_x {
        return (x <= l.name_x - 7).then_some(Mouse::Top);
    }
    let (col, cx, w) = col_at(&l, x)?;
    // The ▾ ends a left-aligned text header ("Dev▼ ▾") and is the last cell of a numeric one.
    let arrow = if col == 1 || col == VIA { cx + 4 + u16::from(app.sort_col == col) } else { cx + w - 1 };
    Some(if app.menu(col) && x >= arrow { Mouse::Menu(col) } else { Mouse::Header(col) })
}

/// The column `x` is on, from the model name on: its cursor index, where it starts and its width.
fn col_at(l: &Layout, x: u16) -> Option<(usize, u16, u16)> {
    let dev_x = l.name_x + l.name_w + GAP;
    if x < l.name_x {
        None
    } else if x < dev_x {
        Some((0, l.name_x, l.name_w))
    } else if x < dev_x + l.dev_w {
        Some((1, dev_x, l.dev_w))
    } else if let Some(&(i, cx, w)) = l.cols.iter().find(|&&(_, cx, w)| (cx..cx + w).contains(&x)) {
        Some((i + 2, cx, w))
    } else if let Some((vx, w)) = l.via.filter(|&(vx, w)| (vx..vx + w).contains(&x)) {
        Some((VIA, vx, w))
    } else {
        l.notes.filter(|&(nx, w)| (nx..nx + w).contains(&x)).map(|(nx, w)| (NOTES, nx, w))
    }
}

/// Which of row `n`'s harnesses is `x` cells into its Via, as `draw` lays it out: the names
/// joined by ", ".
fn harness_at(app: &App, n: usize, x: u16) -> Option<usize> {
    let m = app.data.models.get(*app.rows.get(n)?).filter(|m| app.accessible(m))?;
    let mut end = 0;
    m.via.iter().position(|h| {
        let start = end;
        end += h.len() as u16 + 2;
        (start..end - 2).contains(&x)
    })
}

/// `m`, what the mouse event `e` landed on, unless it is a drag that selects nothing: one not
/// begun on a table row, as off a header, or still on the row it began on, where a click that
/// slips a cell sideways is a click. `press` is the screen row of the last press on a table row,
/// until the pointer leaves it; from then on the drag extends to any row, that one too.
fn dragged(press: &mut Option<u16>, e: MouseEvent, m: Option<Mouse>) -> Option<Mouse> {
    match e.kind {
        MouseEventKind::Down(_) => {
            let row = matches!(
                m,
                Some(Mouse::Row(_) | Mouse::Cell(..) | Mouse::Harness(..) | Mouse::Pick(_) | Mouse::Extend(_))
            );
            *press = row.then_some(e.row);
        }
        MouseEventKind::Drag(_) => {
            if press.is_none_or(|row| row == e.row) {
                return None;
            }
            *press = Some(u16::MAX);
        }
        _ => {}
    }
    m
}

/// How far apart the two presses of a double click may be.
const DOUBLE_CLICK: Duration = Duration::from_millis(400);

/// A cell opens on a double click, two presses on it within `DOUBLE_CLICK`: a single press,
/// the first of the two included, is a click on its row. `last` is the press to pair with.
fn double(last: &mut Option<(Instant, Mouse)>, m: Mouse, at: Instant) -> Mouse {
    let single = match m {
        Mouse::Cell(n, _) | Mouse::Harness(n, _) => Mouse::Row(n),
        Mouse::Open(s) => Mouse::Model(s),
        _ => {
            *last = None;
            return m;
        }
    };
    let again = last.is_some_and(|(t, was)| was == m && at.duration_since(t) < DOUBLE_CLICK);
    *last = (!again).then_some((at, m));
    if again { m } else { single }
}

/// Start `cmd` in a new terminal window here, without waiting: Windows Terminal under WSL,
/// else `$TERMINAL -e`, else x-terminal-emulator.
fn new_terminal(cmd: &[String]) -> std::io::Result<()> {
    // Windows Terminal splits its command line on `;` wherever it stands, and the ids come from
    // downloaded data: one holding a `;` would start a second command there. No real id has one.
    if cmd.iter().any(|a| a.contains(';')) {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "a model id holding ';' is refused"));
    }
    let mut term = if let Some(session) = data::wsl_session(cmd) {
        let mut c = Command::new("wt.exe");
        c.arg("new-tab").args(session);
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
    ["enter details", "x launch", "o open", "y copy name", "space select", "f fav", "e exclude", "n note"];

/// The last group of every overlay, the last hints a narrow terminal drops.
const BACK: [&str; 2] = ["esc back", "q quit"];

/// The model actions a view offers, in `ACTIONS` order whatever order they are asked in.
fn actions(keys: &str) -> Vec<&'static str> {
    ACTIONS.into_iter().filter(|a| keys.split(' ').any(|k| a.split(' ').next() == Some(k))).collect()
}

/// Key reminders in the status bar, in groups every view keeps in the same order: moving,
/// the view's own keys, the model's actions, then the way back. Narrow terminals
/// drop them from the front, so the actions outlast the view's keys and the way back goes last.
/// A toggle names what pressing it does; `A a S F E R C t ?` are on their tabs above the frame, and
/// one cut off a terminal `width` wide is here instead; the rest of the keys are in `?`. `s` acts
/// on the column picked with `h l`.
fn hints(app: &App, width: u16) -> Vec<&'static str> {
    let groups: Vec<Vec<&'static str>> = match app.view {
        View::Table if app.selecting() => {
            vec![vec!["j k G extend"], vec!["C compare"], actions("space f e"), vec!["esc cancel", "q quit"]]
        }
        View::Table => {
            let mut view = vec!["B benchmarks", "H harness", "/ filter", "s sort"];
            if app.menu(app.col) {
                view.push("d dropdown");
            }
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
                || app.only.is_some()
            {
                view.push("c clear");
            }
            if app.any_marked() {
                view.push("u deselect");
            }
            if app.only == Some(FAV) {
                view.push("D unfavorite all");
            }
            if app.only == Some(EXCLUDED) {
                view.push("X unexclude all");
            }
            if stale(app) {
                view.push("r refresh");
            }
            let mut back = vec![];
            // Where esc goes back from, as in the overlays.
            if !app.query.is_empty() || app.only.is_some() || app.task.is_some() {
                back.push("esc back");
            }
            back.push("q quit");
            vec![vec!["h l column"], view, actions("enter x o y space f e n"), back]
        }
        View::Detail(_) => vec![vec!["j k scroll"], actions("x o y space f e n"), BACK.to_vec()],
        // `?` here closes help, which `esc back` already says.
        View::Help => vec![vec!["j k scroll"], vec!["/ search"], vec!["esc back", "q quit"]],
        View::Compare if app.marked_shown < 2 => vec![BACK.to_vec()],
        View::Compare => {
            vec![vec!["j k scroll", "h l 0 $ model"], vec!["/ rows"], actions("enter x o y f e n"), BACK.to_vec()]
        }
        // On a task's name, enter ranks by it; on a tier's model, it and the others act on the model.
        // On the `among` line, the models it ranks.
        View::Recommend if app.among.is_some() => {
            vec![vec!["j k task", "h l 0 $ which models"], vec!["enter pick"], BACK.to_vec()]
        }
        View::Recommend if app.current().is_none() => {
            // A tier with no model has nothing for enter either.
            let enter = match (app.task_sel, app.custom_at()) {
                (1.., _) => vec![],
                (_, Some(_)) => vec!["enter your model"],
                _ => vec!["enter best models first"],
            };
            vec![vec!["j k task", "h l 0 $ tier"], enter, BACK.to_vec()]
        }
        View::Recommend => vec![vec!["j k task", "h l 0 $ tier"], actions("enter x o y space f e n"), BACK.to_vec()],
    };
    let mut groups: Vec<_> = groups.into_iter().filter(|g| !g.is_empty()).collect();
    // The tabs that do not fit above the frame, ahead of the way back, which a narrow terminal
    // drops last. Off the table only the panels', whose keys work there. `? help` goes after
    // the way back, so it is the last to go.
    let (mut cut, last) = (vec![], groups.len() - 1);
    for (i, (&(_, _, hint), end)) in TABS.iter().zip(tab_ends(app)).enumerate() {
        if end >= usize::from(width)
            && (app.view == View::Table || i >= RECOMMEND)
            && app.tab_has(i)
            && !app.tab_on(i)
            && !groups.iter().any(|g| g.contains(&hint))
        {
            if i == HELP_TAB { groups[last].push(hint) } else { cut.push(hint) }
        }
    }
    if !cut.is_empty() {
        groups.insert(last, cut);
    }
    groups.join(&SEP)
}

/// A hint's keys and what they do, which keeps its leading space: the keys are the leading
/// words of one character, or `enter`, `esc` or `space`.
fn split_hint(hint: &str) -> (&str, &str) {
    let mut end = 0;
    for word in hint.split(' ') {
        if word.chars().count() != 1 && !["enter", "esc", "space"].contains(&word) {
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
        "space" => Some(KeyCode::Char(' ')),
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
fn hint_layout(app: &App, parts: &[Line], width: u16) -> (Vec<&'static str>, u16) {
    // The pill, a space, then each part and its " · ".
    let left = mode(app).0.chars().count() + 3 + parts.iter().map(|p| p.width() + 3).sum::<usize>();
    let all = hints(app, width);
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
    let (hints, mut start) = hint_layout(app, &parts(app), width);
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

/// The theme in effect: the one under the cursor in the theme panel, else the saved one.
fn palette(app: &App) -> Option<&'static Palette> {
    THEMES[crate::view::theme(app.theme_preview().unwrap_or(&app.store.theme))].1.as_ref()
}

// The terminal's own palette, so the colours are whatever the rice set. Text stays the default foreground.
const ACCENT: Color = Color::Magenta;
const KEY: Color = Color::Cyan;
const MUTED: Color = Color::DarkGray;
const GOOD: Color = Color::Green;
const BAD: Color = Color::Red;
/// A marked row's fill and its ✓, the ✓ of a ticked entry in a list and the count in the
/// status bar. Light blue, and nothing else is drawn in it, so a palette's slot 12 is free to
/// be whatever parts from the muted ☐ (`the_mark_parts_from_an_empty_box`) and parts its fill
/// from the cursor's (`roles_are_told_apart`).
const MARK: Color = Color::LightBlue;
/// A favorite's ★ with no task at hand: gold, as stars are in mail clients and on GitHub.
const STAR: Color = Color::Yellow;
/// One colour per task in `TASKS` order: the ★ of its favorite and its name in recommend. Off the mark colour (light blue), the key hints' cyan, the worst
/// value's red and yellow for a match; 16 colours leave no room to also skip the best's green, but no two are a pair.
const TASK: [Color; 6] =
    [Color::LightCyan, Color::Green, Color::Blue, Color::LightMagenta, Color::LightRed, Color::LightYellow];
/// The colours of the tasks of your own: none is a built-in task's, nor the gold of a ★ with no
/// task at hand, nor the red of the ✗ and the worst value, so its ★ and name say which.
const OWN: [Color; 3] = [Color::Magenta, Color::LightGreen, Color::Cyan];
/// What a search matched, as the filter in the status bar.
const MATCH: Color = Color::Yellow;
/// Developers' and harnesses' colours in the terminal's own theme: one per Via colour (`dev_color`).
const DEVS: [Color; 7] =
    [Color::Blue, Color::Yellow, Color::Cyan, Color::Magenta, Color::Green, Color::LightMagenta, Color::LightGreen];
/// Price levels (`view::LEVELS`) from free to the most expensive.
const LEVEL: [Color; 6] = [Color::Green, Color::Green, Color::Cyan, Color::Yellow, Color::Red, Color::Magenta];
const BOLD: Modifier = Modifier::BOLD;
/// The colour tab `i` of `TABS` is on in.
fn tab_color(i: usize) -> Color {
    match i {
        MARKED => MARK,
        FAV => STAR,
        EXCLUDED => BAD,
        _ if i < RECOMMEND => ACCENT,
        _ => Color::Cyan,
    }
}

/// What follows the name of tab `i`: on yours `+N`, the selected models out of reach it shows too.
fn tab_plus(app: &App, i: usize) -> String {
    if i == YOURS && app.marked_out > 0 { format!(" +{}", app.marked_out) } else { String::new() }
}

/// The cells tab `i` takes: its left edge, then its name and its key with a space around each.
fn tab_width(app: &App, i: usize) -> usize {
    TABS[i].0.chars().count() + tab_plus(app, i).len() + 5
}

/// Where each tab ends, in cells from the screen's left edge, the first starting one cell in:
/// for the clicks and for the hints of the tabs cut off, which `tabs` draws `tab_width` apart.
fn tab_ends(app: &App) -> impl Iterator<Item = usize> {
    (0..TABS.len()).scan(1, |end, i| {
        *end += tab_width(app, i);
        Some(*end)
    })
}

/// The tabs on the two lines above `frame`, as a browser's: one that is on is inside a frame of
/// its own, whose sides run down into `frame`, its top border open under it; a thin line parts
/// two that are off. A panel's, the panel a box of
/// its own over `frame`, stands on the border, closed. One with nothing to show is muted. The
/// sort, `sort_w` cells at the border's right end, stays. Returns the cells of the frame's top
/// border under the tab that is on, cut to the frame.
fn tabs(buf: &mut Buffer, frame: Rect, sort_w: u16, app: &App) -> (u16, u16) {
    // Not over the frame's right corner, nor past it; nor, on its border, over the sort.
    let put = |buf: &mut Buffer, x: u16, y: u16, text: &str, style: Style| {
        let end = frame.right().saturating_sub(1 + if y == frame.y { sort_w } else { 0 });
        buf.set_stringn(x, y, text, usize::from(end.saturating_sub(x)), style);
    };
    let panel = (RECOMMEND..TABS.len()).any(|i| app.tab_on(i));
    let (y, mut x, mut before, mut span) = (frame.y - 1, frame.x + 1, false, None::<(u16, u16)>);
    for i in 0..=TABS.len() {
        let on = i < TABS.len() && app.tab_on(i);
        let (top, side, down) = match (before, on) {
            // Between two that are off, the thin line a browser has there.
            (false, false) if i > 0 && i < TABS.len() => (" ", "│", "─"),
            (false, false) => (" ", " ", "─"),
            (false | true, _) if panel => (if on { "╭" } else { "╮" }, "│", "┴"),
            (false, true) => ("╭", "│", "╯"),
            (true, _) => ("╮", "│", "╰"),
        };
        put(buf, x, y - 1, top, fg(MUTED));
        put(buf, x, y, side, fg(MUTED));
        // Away from a tab that is on the frame's border is left as it is, the sort on it too.
        if before || on {
            put(buf, x, y + 1, down, fg(MUTED));
        }
        let Some(&(name, key, _)) = TABS.get(i) else { break };
        let (style, key_style) = match (on, app.tab_has(i)) {
            (true, _) => (fg(tab_color(i)).add_modifier(BOLD), fg(KEY).add_modifier(BOLD)),
            (_, true) => (Style::new(), fg(KEY).add_modifier(BOLD)),
            _ => (fg(MUTED), fg(MUTED)),
        };
        // The `+N` in the colour of the ✓ its models have.
        let (plus, len) = (tab_plus(app, i), name.chars().count() as u16);
        put(buf, x + 1, y, &format!(" {name} "), style);
        put(buf, x + 2 + len, y, &format!("{plus} "), fg(MARK).add_modifier(BOLD));
        put(buf, x + 3 + len + plus.len() as u16, y, &format!("{key} "), key_style);
        let w = tab_width(app, i) as u16;
        if on {
            put(buf, x + 1, y - 1, &"─".repeat(usize::from(w - 1)), fg(MUTED));
            if !panel {
                put(buf, x + 1, y + 1, &" ".repeat(usize::from(w - 1)), Style::new());
            }
            span = Some((x - frame.x, x + w + 1 - frame.x));
        }
        (x, before) = (x + w, on);
    }
    span.map_or((0, 0), |(lo, hi)| (lo.min(frame.width), hi.min(frame.width)))
}

/// The cursor's fill, on a row, a column of compare or a name in recommend, between two bars of
/// the accent that say where it is (`cursor_ends`, `cursor`). Drawn as the accent, which tells it
/// from a marked row's, and painted as a faint grey (`fill`): a hue on a row says something of
/// it, as a marked row's blue does, and the accent's is a developer's and a price level's too.
const CURSOR: Color = ACCENT;
const FILL: Style = Style::new().bg(CURSOR);
/// The bars at the cursor's two ends.
const EDGE: Style = Style::new().fg(ACCENT).bg(CURSOR).add_modifier(BOLD);
/// The share of the mark colour in a marked row's fill with the terminal's own colours, in
/// percent, the rest being its background: faint, as the row's colours are read on it. A theme
/// has its own (`Palette::wash`).
/// ponytail: text on it reads at 2.2:1 or better, not the 3:1 it has on the background; past
/// that the palettes need retuning (`every_theme_reads_on_its_own_background`).
const WASH: u32 = 12;
/// And of the text colour in the cursor's: the most that every theme reads on and tells from a
/// marked row's fill (`roles_are_told_apart`), about what an editor's cursor line has.
const CURSOR_WASH: u32 = 8;
/// The mark colour in that fill with the terminal's own colours, which do not tell their slot 12.
/// ponytail: one blue for a dark and a light terminal; ask for slot 12 (OSC 4) to follow the rice.
const TERM_MARK: u32 = 0x5c9cff;

/// Swap the terminal colours for the theme's after drawing, so the drawing code keeps naming
/// terminal colours: the 16, default text, and a developer's placeholder (see `dev_color`).
/// Backgrounds with no colour get the theme's; with the terminal's own colours they stay transparent.
/// `term_bg` is the terminal's background, if it told (`terminal_bg`).
fn recolor(buf: &mut Buffer, palette: Option<&Palette>, term_bg: Option<u32>) {
    let bg = palette.map(|p| Color::from_u32(p.bg));
    for cell in &mut buf.content {
        // The terminal's own colours with its background unknown have no faint fill: the cursor
        // is a reverse-video bar there, its two ends part of it and colours off it so it reads
        // as one. A pill in the accent keeps its fill.
        if palette.is_none() && term_bg.is_none() && cell.bg == CURSOR && cell.fg != Color::Black {
            if matches!(cell.symbol(), "▌" | "▐") {
                cell.set_symbol(" ");
            }
            (cell.fg, cell.bg) = (Color::Reset, Color::Reset);
            cell.modifier.insert(Modifier::REVERSED);
            continue;
        }
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
        // Any other text on a colour is a marked row's or the cursor's: the fill is faint, so the
        // text reads on it. The terminal's own colours have a faint one only when its background
        // is known.
        cell.bg = match (cell.bg, palette, term_bg) {
            (Color::Reset, ..) => bg.unwrap_or(Color::Reset),
            (c, Some(p), _) => fill(c, p),
            // The terminal does not tell its text colour: white on a dark background, else black.
            (CURSOR, None, Some(b)) if cell.fg != Color::Black => {
                wash(Color::from_u32(if lum(b) < 128.0 { 0xffffff } else { 0 }), b, CURSOR_WASH)
            }
            (_, None, Some(b)) if cell.fg != Color::Black => wash(Color::from_u32(TERM_MARK), b, WASH),
            (c, None, _) => c,
        };
    }
}

/// The brighter or the dimmer half of a drawn colour, whichever is furthest from the theme's
/// background in perceived brightness, so a solid fill reads in a dark and a light theme alike.
fn contrasting(c: Color, p: &Palette) -> Color {
    let Some(i) = ansi(c) else { return resolve(c, Some(p)) };
    let (dim, bright) = (p.ansi[i % 8], p.ansi[i % 8 + 8]);
    Color::from_u32(if (lum(dim) - lum(p.bg)).abs() > (lum(bright) - lum(p.bg)).abs() { dim } else { bright })
}

/// Perceived brightness, 0 to 255.
fn lum(c: u32) -> f64 {
    0.2126 * f64::from((c >> 16) & 255) + 0.7152 * f64::from((c >> 8) & 255) + 0.0722 * f64::from(c & 255)
}

/// A fill in a theme: a marked row's is the mark colour, the cursor's the text colour, each
/// faint on the background.
fn fill(c: Color, p: &Palette) -> Color {
    match c {
        CURSOR => wash(Color::from_u32(p.text), p.bg, CURSOR_WASH),
        c => wash(resolve(c, Some(p)), p.bg, p.wash),
    }
}

/// The background `bg` with `share` percent of `c` in it.
fn wash(c: Color, bg: u32, share: u32) -> Color {
    let Color::Rgb(r, g, b) = c else { return c };
    let [_, x, y, z] = bg.to_be_bytes();
    let mix = |c: u8, bg: u8| ((u32::from(c) * share + u32::from(bg) * (100 - share)) / 100) as u8;
    Color::Rgb(mix(r, x), mix(g, y), mix(b, z))
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
/// accents: a Via name's place in `data::vias`, so no two share one, else the name's byte sum mod
/// 210, which keeps it mod 7, 10 and 14 (`DEVS`, `Palette::accents`). omp, a fork of pi, has
/// pi's, and llama-cli ollama's, your machine's: an eighth colour is more than the terminal's own
/// seven or gameboy's.
fn dev_color(dev: &str) -> Color {
    let dev = match dev {
        "omp" => "pi",
        crate::data::LLAMA => crate::data::OLLAMA,
        d => d,
    };
    let k = crate::data::vias().filter(|v| !matches!(*v, "omp" | crate::data::LLAMA)).position(|v| v == dev);
    Color::Indexed(k.unwrap_or_else(|| dev.bytes().map(usize::from).sum::<usize>() % 210) as u8)
}

/// A task's colour, or its tier's (`coding:low`), which is the task's; a task of your own has
/// one of `OWN`, picked by its name.
// ponytail: by name, so two of your own tasks can land on one colour; colour by place in
// `Store::custom_tasks` if that shows.
fn task_color(task: &str) -> Color {
    let task = task.split(':').next().unwrap_or(task);
    let own = || OWN[task.bytes().map(usize::from).sum::<usize>() % OWN.len()];
    TASKS.iter().position(|t| t.name == task).map_or_else(own, |i| TASK[i])
}

fn draw(app: &mut App, f: &mut Frame) {
    let area = f.area();
    if area.height < 4 || area.width < 4 {
        return;
    }
    // The tabs have the first two lines, the frame the rest down to the status bar.
    let body = Rect { y: area.y + 2, height: area.height - 3, ..area };
    let bar = Rect { y: area.bottom() - 1, height: 1, ..area };
    // The first start asks its question under the wordmark, settled as the intro leaves it.
    let splash = splash(app, area);
    if let Some((at, _)) = splash {
        wordmark(f.buffer_mut(), area, at, TAGLINE.len(), |_, _| fg(ACCENT).add_modifier(BOLD));
    } else {
        // The refresh state is always in view: under way, failed, or how old the data is.
        let age = format!("{} ", data_age(&app.data));
        let version = match app.data.update() {
            Some(new) => Line::from(format!(" modelcmp v{new} available: U upgrades ")).style(fg(Color::Yellow)),
            None => Line::from(concat!(" modelcmp v", env!("CARGO_PKG_VERSION"), " ")).style(fg(MUTED)),
        };
        let source = format!(" models.dev + {} · ", data::source().label());
        // Where the count and what it waits for would run over the version, the count goes alone.
        let room = usize::from(body.width).saturating_sub(2 + version.width() + source.chars().count());
        let refreshing = |p: &str| format!("⟳ refreshing {p}{}", if p.is_empty() { "" } else { " " });
        let (state, color) = match (app.refreshing, app.refresh_failed, app.data.stale()) {
            (true, ..) if refreshing(&app.progress).chars().count() > room => {
                (refreshing(app.progress.split(',').next().unwrap_or_default()), Color::Yellow)
            }
            (true, ..) => (refreshing(&app.progress), Color::Yellow),
            (_, true, _) => (format!("refresh failed · {age}"), BAD),
            (_, _, true) => (age, BAD),
            _ => (age, MUTED),
        };
        // A state still too long would run over the version: the source goes, which `B` shows too.
        let source = if state.chars().count() > room { String::new() } else { source };
        let sort = format!(" {} by {} ", if app.descending { "▼" } else { "▲" }, app.col_name(app.sort_col));
        let sort_w = sort.chars().count();
        let frame = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(fg(MUTED))
            .title_top(Line::from(sort).style(fg(MUTED)).right_aligned())
            .title_bottom(version)
            .title_bottom(
                Line::from(vec![Span::styled(source, fg(MUTED)), Span::styled(state, fg(color))]).right_aligned(),
            );
        let inner = frame.inner(body);
        let head = head(app);
        app.page = inner.height.saturating_sub(head);
        let buf = f.buffer_mut();
        frame.render(body, buf);
        // What the column under the cursor means, centred and cut to clear the sort on either
        // side; where that would run under the tabs that are on, centred on the wider side of them.
        let (lo, hi) = tabs(buf, body, sort_w as u16, app);
        let (lo, hi) = (usize::from(lo), usize::from(hi));
        let (text, width) = (format!(" {}: {} ", app.col_name(app.col), app.col_about(app.col)), body.width as usize);
        let mut end = width.saturating_sub(sort_w + 1);
        let mut about = truncate(&text, width.saturating_sub(2 * (sort_w + 2)).max(1));
        let mut x = width.saturating_sub(about.chars().count()) / 2;
        if x < hi && x + about.chars().count() > lo {
            let left = lo.min(end);
            let start;
            (start, end) = if left.saturating_sub(1) > end.saturating_sub(hi) { (1, left) } else { (hi, end) };
            let room = end.saturating_sub(start);
            about = truncate(&text, room.max(1));
            x = start + room.saturating_sub(about.chars().count()) / 2;
        }
        buf.set_stringn(body.x + x as u16, body.y, about, end.saturating_sub(x), fg(MUTED));
        let (right, above, below) = table(buf, inner, app);
        if right && inner.height > 0 {
            // Columns cut off on the right: `l` scrolls to them.
            buf.set_stringn(body.right() - 1, inner.y, "›", 1, fg(ACCENT).add_modifier(BOLD));
        }
        if inner.height > 1 {
            // The rule under the header runs into the frame.
            buf.set_stringn(body.x, inner.y + 1, "├", 1, fg(MUTED));
            buf.set_stringn(body.right() - 1, inner.y + 1, "┤", 1, fg(MUTED));
        }
        if inner.height > head {
            vmarks(buf, body.x, inner.y + head, inner.bottom() - 1, above, below);
        }
    }
    let buf = f.buffer_mut();
    // Where the text cursor goes: in the status bar's prompt, or on an entry written in a list.
    let mut cursor = status(buf, bar, app).map(|x| (x, bar.y));
    // Where compare and recommend put each model, in their lines, and recommend the cursor's task.
    let (mut spots, mut block, mut pin) = (vec![], None, 0..0);
    let lines = match app.view {
        View::Table => None,
        View::Help => Some(("keys".to_string(), help(&app.overlay_query))),
        View::Recommend => {
            let lines;
            (lines, block, pin) = recommend(app, (area.width as usize).saturating_sub(4).min(130), &mut spots);
            Some(("recommend".to_string(), lines))
        }
        View::Detail(_) => {
            let get = |m| (app.download(m), app.size(m));
            app.current().map(|m| detail(m, &app.store, app.any_available(), get(m)))
        }
        View::Compare if app.marked_shown < 2 => {
            let key = |k: &'static str| Span::styled(k, fg(KEY).add_modifier(BOLD));
            let n = app.marked_shown;
            let lines = vec![
                Line::from(format!("compare needs 2 or more selected models, {n} now")),
                Line::from(""),
                Line::from(vec![key("esc"), Span::raw(" back to the table, then")]),
                Line::from(vec![key("space"), Span::raw(" selects the model under the cursor, or")]),
                Line::from(vec![key("v"), Span::raw(" / shift+click highlights a range, and")]),
                Line::from(vec![key("C"), Span::raw(" compares them")]),
            ];
            Some(("compare".into(), lines))
        }
        View::Compare => {
            let (lines, first) = compare(
                &app.marked_models(),
                (app.any_available(), &|m| app.download(m)),
                (app.compare_sel, app.compare_x),
                area.width.saturating_sub(4) as usize,
                &app.overlay_query,
                |m| app.muted(m),
                &mut spots,
            );
            app.compare_x = first;
            Some(("compare".into(), lines))
        }
    };
    app.spots.clear();
    app.panel = lines.as_ref().map(|(title, lines)| overlay_rect(body, title, lines));
    if let Some((title, lines)) = lines {
        if app.view == View::Recommend {
            // Keep the cursor's task row in view, over the lines pinned under the grid.
            if let Some(std::ops::Range { start, end }) = block {
                let rows = body.height.saturating_sub(2) as usize;
                let under = pinned(lines.len(), rows, &pin);
                let (shown, len) = (rows - under, lines.len() - under);
                // No cursor goes to the lines above the first row and under the last: they
                // show with it, where they fit.
                let (first, last) = (app.task_cur == 0, app.task_cur + 1 >= app.task_count());
                let (start, end) = (
                    if first && end <= shown { 0 } else { start },
                    if last && len - start <= shown { len } else { end },
                );
                let lo = end.saturating_sub(shown).min(start);
                app.scroll = (app.scroll as usize).clamp(lo, start) as u16;
            }
        }
        let under = pinned(lines.len(), body.height.saturating_sub(2) as usize, &pin);
        let (text, ..) = overlay(buf, body, &title, lines, &mut app.scroll, Color::Reset, (0, 0, pin));
        // Where each spot landed on screen, scrolled and cut to the box, for a click to find it.
        let top = app.scroll as usize;
        app.spots = spots
            .into_iter()
            .filter_map(|p| {
                let ys = p.lines.start.max(top)..p.lines.end.min(top + text.height as usize - under);
                let xs = p.x.start..p.x.end.min(text.width as usize);
                let rect = Rect::new(
                    text.x + xs.start as u16,
                    text.y + (ys.start - top) as u16,
                    xs.len() as u16,
                    ys.len() as u16,
                );
                (!ys.is_empty() && !xs.is_empty()).then_some((rect, p.at))
            })
            .collect();
    }
    // `q`, `U`, `D` and `X` ask first: the same box, confirmed by the same key again.
    let ask = match app.input {
        Input::Quit => Some(("q", "quit?".to_string())),
        Input::Upgrade => app.data.update().map(|new| ("U", format!("upgrade to v{new}?"))),
        Input::Unfavorite => Some(("D", "unfavorite every model?".to_string())),
        Input::Unexclude => Some(("X", "unexclude every model?".to_string())),
        _ => None,
    };
    if let Some((key, title)) = ask {
        let key = Span::styled(key, fg(KEY).add_modifier(BOLD));
        let lines = vec![Line::from(vec![key, Span::raw(" confirms · any other key cancels")])];
        overlay(buf, body, &title, lines, &mut 0, Color::Reset, (0, 0, 0..0));
    }
    let chooser = chooser(app, area);
    if let (Some((within, lines)), Input::Choose { title, kind, items, list }) = (chooser, &mut app.input) {
        let rect = overlay_rect(within, title, &lines);
        let rows = choice_rows(items, &list.query).len();
        // f's grid has its tiers' heading over the rows, and its cursor on a box, drawn with it;
        // a row being written has no box, so the cursor is the row's, as in every list.
        // ponytail: the heading scrolls off with the rows in a box too short for them all; pin
        // it if a dozen tasks of your own on a short terminal turn out to be a thing.
        let grid = *kind == Kind::Fav;
        let head = usize::from(grid);
        let (sel, top) = (&list.sel, &mut list.top);
        let shown = usize::from(rect.height.saturating_sub(2));
        // At the last entry every line to the end, so the key hint below them shows too, unless
        // that would scroll the cursor's line off. And at an entry being written, whose keys or
        // error that line has.
        // ponytail: an entry more than a box's height from the end keeps it out of view; the
        // error on the line under the entry if it bites.
        let from = if *sel + 1 >= rows || list.edit.is_some() { lines.len().saturating_sub(shown) } else { *top };
        let mut scroll = list_top(from, *sel + head, shown) as u16;
        // Under the wordmark the box is muted, as the table's frame; over the table it has
        // the text's colour, as every box there, which parts it from that frame.
        let border = if splash.is_some() { MUTED } else { Color::Reset };
        let ends = (head, lines.len() - rows - head, 0..0);
        let (_, above, below) = overlay(buf, within, title, lines, &mut scroll, border, ends);
        *top = usize::from(scroll);
        if rows > 0 {
            // The cursor runs through the box's border, as in the table, and the marks go over
            // its bar, as there.
            let y = (rect.y + 1 + (*sel + head) as u16 - scroll).min(rect.bottom() - 1);
            if !grid || list.edit.is_some() {
                cursor_ends(buf, rect.x, rect.right() - 1, y);
            }
            // An entry being written has the text cursor, after what `edit_line` draws before it.
            if let Some(e) = &list.edit {
                let prefix = Span::raw(edit_prefix(e)).width();
                let start = scrolled(prefix, &e.text, e.cur, edit_room(within));
                let before = prefix + Span::raw(&e.text[start..e.cur]).width();
                cursor = Some(((rect.x + 2 + before as u16).min(rect.right().saturating_sub(2)), y));
            }
            let inner = rect.inner(Margin::new(1, 1));
            vmarks(buf, rect.x, inner.y, inner.bottom() - 1, above, below);
        }
    }
    if let Some(at) = cursor {
        f.set_cursor_position(at);
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
    /// The first column right of Dev shown (0 is Released, the last Notes), for `App::hscroll`.
    first: usize,
    /// Columns cut off on the right; `‹` after Dev and `›` on the frame say where to scroll.
    more: bool,
    /// Where a `│` parts two groups of columns (`GROUPS`): after Dev when scrolled, and before
    /// each group that starts right of it.
    seps: Vec<u16>,
}

/// Header of the column at cursor index `col`: a picked benchmark's name cut short, so a long
/// one does not push the columns after it off the screen; the top border has it whole.
fn col_head(app: &App, col: usize) -> Cow<'static, str> {
    match app.col_bench(col) {
        Some(b) if b.chars().count() > 10 => truncate(b, 10).into(),
        _ => app.col_name(col).into(),
    }
}

fn layout(width: u16, app: &App) -> Layout {
    let ms = &app.data.models;
    // Headers need room for the sort arrow, and a dropdown's for its " ▾".
    let widths: [u16; COLS.len()] = std::array::from_fn(|i| {
        let head = col_head(app, i + 2).chars().count() + 1 + if app.menu(i + 2) { 2 } else { 0 };
        head.max(app.widths[i]) as u16
    });
    let dev_w = ms.iter().map(|m| m.developer.chars().count()).max().unwrap_or(0).clamp(6, 12) as u16;
    // A shown model you have no access to says so in Via.
    let out = app.rows.iter().any(|&r| !app.accessible(&ms[r]));
    // As drawn: joined by ", ".
    let listed = |m: &Model| m.via.iter().map(|v| v.len() + 2).sum::<usize>().saturating_sub(2);
    // A shown one `x` can download says that after them, or alone when out of reach.
    let got = app.rows.iter().filter_map(|&r| {
        let before = Some(listed(&ms[r])).filter(|&w| w > 0 && app.accessible(&ms[r])).map_or(0, |w| w + 1);
        Some(before + app.download(&ms[r])?.chars().count())
    });
    let via_w =
        ms.iter().map(listed).chain(got).chain(out.then_some(OUT_OF_REACH.len())).max().unwrap_or(0).clamp(6, 24)
            as u16;
    // As wide as drawn: a CJK character or an emoji takes two cells.
    let notes_w = ms.iter().filter_map(|m| app.store.note(&m.key)).map(|s| Span::raw(s).width()).max().unwrap_or(0);
    let notes_w = notes_w.clamp(6, 40) as u16;
    let longest = ms.iter().map(|m| m.name.chars().count()).max().unwrap_or(0) as u16;
    // Row numbers as wide as the last one, a space, then the checkbox, the ☆ and the ✗ box,
    // each with a spare cell: some terminals draw them two cells wide, and the
    // spare keeps that off the neighbour.
    let name_x = app.rows.len().max(1).to_string().len() as u16 + 7;
    // The columns right of Dev scroll sideways: only as far as it takes to show the selected
    // one, keeping the last position otherwise. Model and Dev stay put.
    let ws: Vec<u16> = widths.into_iter().chain([via_w, notes_w]).collect();
    // A column starting a group has a `│` in its gap, one cell wider; so does the first shown
    // when scrolled, parting it from Dev. Released, in Dev's group, has neither.
    let sep = |k: usize| u16::from(GROUPS.contains(&(k + 2)));
    // A column the source does not measure takes no room at all.
    let ws: Vec<u16> = ws.into_iter().enumerate().map(|(k, w)| if hidden(k + 2) { 0 } else { w }).collect();
    let span = |k: usize| if ws[k] == 0 { 0 } else { ws[k] + GAP + sep(k) };
    let fixed: u16 = (0..ws.len()).map(span).sum::<u16>() + dev_w + GAP;
    let name_w = width.saturating_sub(name_x + fixed).clamp(NAME_MIN, longest.max(NAME_MIN));
    let mut x = name_x + name_w + GAP + dev_w + GAP;
    let room = width.saturating_sub(x) + GAP;
    let first = match app.col.checked_sub(2) {
        Some(s) => {
            // The leftmost first column that still shows column `s`. Each column brought in on
            // the left costs its width and gap, and the one it displaces as first its `│`;
            // hidden ones cost nothing and are never first.
            let reach = |s: usize| {
                let (mut lo, mut used) = (s, ws[s] + GAP);
                for k in (0..s).rev().filter(|&k| ws[k] > 0) {
                    let cost = ws[k] + GAP + sep(lo);
                    // Plus the first's own `│`, unless it is Released.
                    if used + cost + u16::from(k > 0) > room {
                        break;
                    }
                    (lo, used) = (k, used + cost);
                }
                lo
            };
            // Never scrolled further than it takes to show the last column, so a wider
            // window brings back the columns on the left.
            let last = ws.iter().rposition(|&w| w > 0).unwrap_or(s);
            app.hscroll.clamp(reach(s), s).min(reach(last))
        }
        None => 0,
    };
    let (mut cols, mut tail, mut more, mut seps) = (Vec::with_capacity(COLS.len()), [None; 2], false, vec![]);
    let mut started = false;
    for (k, &w) in ws.iter().enumerate().skip(first).filter(|(_, w)| **w > 0) {
        let part = if started { sep(k) == 1 } else { k > 0 };
        x += u16::from(part);
        // The first shown column is cut to the room left rather than dropped: it may be the
        // one under the cursor, wider than all the room right of Dev. Via and Notes only: a
        // number cut short reads as another number.
        let w = if started || k < COLS.len() { w } else { w.min(width.saturating_sub(x)) };
        started = true;
        if w == 0 || x + w > width {
            more = true;
            break;
        }
        more = w < ws[k];
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

/// Lines of the table above its first row: the header, the rule and, with access to no model,
/// `NO_ACCESS`.
fn head(app: &App) -> u16 {
    2 + u16::from(app.no_access())
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
    // In a terminal too narrow for both they are cut at the frame, not drawn over its border.
    let room = |x: u16, w: u16| usize::from(w.min(area.right().saturating_sub(x)));
    let (nw, dw) = (room(name_x, name_w), room(dev_x, dev_w));
    buf.set_stringn(area.x, y, format!("{:>num_w$}", "#"), num_w, fg(MUTED));
    buf.set_stringn(name_x, y, format!("{:<nw$}", format!("Model{}", arrow(0))), nw, header(0));
    buf.set_stringn(dev_x, y, format!("{:<dw$}", format!("Dev{} ▾", arrow(1))), dw, header(1));
    if first > 0 && dev_x + dev_w < area.right() {
        // Columns scrolled off to the left.
        buf.set_stringn(dev_x + dev_w, y, "‹", 1, fg(ACCENT).add_modifier(BOLD));
    }
    for &(i, x, w) in &cols {
        let text = format!("{}{}{}", arrow(i + 2), col_head(app, i + 2), if app.menu(i + 2) { " ▾" } else { "" });
        buf.set_stringn(area.x + x, y, format!("{text:>w$}", w = w as usize), w as usize, header(i + 2));
    }
    if let Some((x, w)) = via {
        buf.set_stringn(area.x + x, y, format!("Via{} ▾", arrow(VIA)), w as usize, header(VIA));
    }
    if let Some((x, w)) = notes {
        buf.set_stringn(area.x + x, y, format!("{}{}", app.col_name(NOTES), arrow(NOTES)), w as usize, header(NOTES));
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

    let head = head(app);
    if head > 2 && area.height > 2 {
        buf.set_stringn(name_x, y + 2, NO_ACCESS, room(name_x, u16::MAX), fg(Color::Yellow));
    }
    let height = usize::from(area.height.saturating_sub(head));
    let sel = app.table.selected().unwrap_or(0).min(app.rows.len().saturating_sub(1));
    // Keep the selection in view, and never leave rows blank below while some are hidden above.
    let top = app.table.offset().clamp(sel.saturating_sub(height.saturating_sub(1)), sel);
    let top = top.min(app.rows.len().saturating_sub(height));
    *app.table.offset_mut() = top;
    let ext = app.ext;
    // A glyph with the gap after it, short of the frame's border on a table too narrow for both.
    let gap = |x: u16| usize::from(area.right().saturating_sub(x)).min(2);
    let faint = palette(app).is_some() || app.term_bg.is_some();
    for (k, &r) in app.rows.iter().enumerate().skip(top).take(height) {
        let y = area.y + head + (k - top) as u16;
        let m = &app.data.models[r];
        // The cursor, or the visual range, is a faint fill through the frame's border, which
        // becomes its two bars; the row keeps its colours on it.
        let on = k == sel || app.is_selected(k);
        // A model excluded or out of reach has its row muted: the text and the developer, harness,
        // price level, best and worst colours go grey, while the ✓, ★ and ✗ keep theirs. The
        // extremes are those of the other rows (`App::ext`), so a column does not lose them.
        let (excluded, reach) = (app.store.is_excluded(&m.key), app.accessible(m));
        let dim = excluded || !reach;
        // A marked row off the cursor is filled through the border too, in the mark's colour.
        // The fill is faint (`WASH`), so the row keeps its colours on it. The terminal's own 16
        // have no faint one, so with them it takes a terminal that told its background; else
        // the fill is solid like a pill, with colours off it, a muted row's grey included, and
        // the filled ★ and the ✗ say favorite and excluded by their shape.
        let marked = app.store.is_marked(&m.key);
        let fill = !on && marked;
        let solid = fill && !faint;
        let base = match (on, fill) {
            (true, _) => FILL,
            (_, true) if solid => fg(Color::Black).bg(MARK),
            (_, true) => Style::new().bg(MARK),
            _ => Style::new(),
        };
        let tint = |c: Color| if solid { base } else { fg(c) };
        let text = if dim { tint(MUTED) } else { base };
        let soft = |c: Color| if dim { text } else { tint(c) };
        buf.set_style(Rect { y, height: 1, ..area }.outer(Margin::new(1, 0)).intersection(buf.area), base);
        if on && area.x > 0 {
            cursor_ends(buf, area.x - 1, area.right(), y);
        }
        buf.set_stringn(area.x, y, format!("{:>num_w$}", k + 1), num_w, tint(MUTED));
        // Off, a mark is its own glyph in grey, as the ☆ is the ★'s, so it says what a click on
        // it does. Where colours are off (the reverse-video cursor, a solid fill) a grey one
        // would read as on, so there off is a shape of its own.
        let (unmarked, included) = if faint { ("✓", "✗") } else { ("☐", "·") };
        match marked {
            // Bold as well as blue: on a solid fill, which keeps colours off, that is all the
            // mark has left to show itself with.
            true => buf.set_stringn(box_x, y, "✓", 1, tint(MARK).add_modifier(BOLD)),
            false => buf.set_stringn(box_x, y, unmarked, 1, tint(MUTED)),
        };
        if app.starred(&m.key) {
            // In the colour of the task at hand, as its name in recommend; gold with no task, as the ★
            // then stands for any of them. Bold so it stands out as much as the ✓.
            let star = tint(app.task_at_hand().map_or(STAR, |t| task_color(t.name)));
            // The gap after it takes the colour too: a font may draw the ★ wider than its cell,
            // and a terminal paints what hangs over in the next cell's colour.
            buf.set_stringn(star_x, y, "★ ", gap(star_x), star.add_modifier(BOLD));
        } else {
            buf.set_stringn(star_x, y, "☆", 1, tint(MUTED));
        }
        // A marked model's name is bold, as the ✓, and takes no colour: the fill says the row is
        // marked, and under the cursor the ✓ and the bold do. Out of reach, its Via says so.
        let name = if marked { text.add_modifier(BOLD) } else { text };
        buf.set_stringn(name_x, y, &m.name, nw, name);
        buf.set_stringn(dev_x, y, &m.developer, dw, soft(dev_color(&m.developer)));
        for &(i, x, w) in &cols {
            let Some(v) = app.vals[r][i] else {
                buf.set_stringn(area.x + x + w - 1, y, "-", 1, tint(MUTED));
                continue;
            };
            let listed = app.listed[r];
            let style = match ext[i] {
                // A list price is not one you'd pay: no level, no best or worst, nor for Value over it.
                _ if listed && on_price(i) => tint(MUTED).add_modifier(Modifier::ITALIC),
                // The blended price is coloured by level, so its colour says the same thing on every screen.
                _ if i + 2 == PRICE => soft(LEVEL[level(v)]),
                // Free is green and no more, in $in, $cache and $out as in Price: every free model
                // ties for the best, and a column of them in bold says nothing of any one.
                _ if COLS[i].price && COLS[i].lower_better && v == 0.0 => soft(GOOD),
                // Nor is the largest context bold: it is a size many models share, as free is.
                Some((best, _)) if !dim && v == best && COLS[i].id == "ctx" => tint(GOOD),
                Some((best, _)) if !dim && v == best => tint(GOOD).add_modifier(BOLD),
                Some((_, worst)) if !dim && v == worst => tint(BAD),
                _ => text,
            };
            let w = w as usize;
            buf.set_stringn(area.x + x, y, format!("{:>w$}", shown(i, v, listed)), w, style);
        }
        if let Some((x, w)) = via {
            let (start, end) = (area.x + x, area.x + x + w);
            let (mut x, get) = (start, app.download(m));
            if reach {
                for (j, h) in m.via.iter().enumerate() {
                    if j > 0 {
                        x = buf.set_stringn(x, y, ", ", end.saturating_sub(x) as usize, text).0;
                    }
                    x = buf.set_stringn(x, y, h, end.saturating_sub(x) as usize, soft(dev_color(h))).0;
                }
            } else if get.is_none() {
                buf.set_stringn(x, y, OUT_OF_REACH, w as usize, tint(MUTED).add_modifier(Modifier::ITALIC));
            }
            // The download `x` offers, after the harnesses or alone.
            if let Some(get) = get {
                x += u16::from(x > start);
                buf.set_stringn(x, y, get, end.saturating_sub(x) as usize, text);
            }
        }
        let note = app.store.note(&m.key).unwrap_or("");
        if let Some((x, w)) = notes {
            buf.set_stringn(area.x + x, y, note, w as usize, text.add_modifier(Modifier::ITALIC));
        }
        if excluded {
            buf.set_stringn(ex_x, y, "✗ ", gap(ex_x), tint(BAD).add_modifier(BOLD));
        } else {
            buf.set_stringn(ex_x, y, included, 1, tint(MUTED));
        }
        for &x in &seps {
            buf.set_stringn(area.x + x, y, "│", 1, tint(MUTED));
        }
        // What the search matched, underlined in bold; Notes may be scrolled off. On a solid
        // fill the yellow is behind the hit, as yellow text would not read there.
        // ponytail: a char is taken as one cell; wide chars would shift the underline.
        // Via is drawn as its harnesses joined by ", ", so the hits line up; out of reach, it is not.
        if !app.query.is_empty()
            && let Some(hits) = hits(&app.query, [&m.name, &m.developer, &m.via.join(", "), note], app.typos)
        {
            let style = if solid { base.bg(MATCH) } else { tint(MATCH) }.add_modifier(BOLD | Modifier::UNDERLINED);
            let [via, note] = [via.filter(|_| reach), notes].map(|c| c.map(|(x, w)| (area.x + x, w as usize)));
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
        buf.set_style(Rect::new(x, area.y + head, w, drawn).intersection(area), Style::new().add_modifier(BOLD));
    }
    let level = app.price_level().map(|l| vec![level_label(l)]).unwrap_or_default();
    let bench = if let Input::Menu { col, .. } = &app.input { app.col_bench(*col) } else { None };
    let bench: Vec<String> = bench.into_iter().map(String::from).collect();
    if let Input::Menu { col, items, list } = &mut app.input {
        let l = Layout { name_x: name_x - area.x, name_w, dev_w, cols, via, notes, first, more, seps };
        let picked = match *col {
            1 => &app.dev,
            VIA => &app.via,
            PRICE => &level,
            _ => &bench,
        };
        dropdown(buf, area, menu_x(area, &l, *col), *col, items, list, picked);
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
    let h = (rows as u16 + 2).min(area.height.saturating_sub(1));
    // Room for where you are, when the entries may not all fit: by all of them, so the box
    // keeps still while typing.
    let n = items.len();
    let foot = if n + 2 > area.height.saturating_sub(1) as usize { position(n, 0, n).len() + 2 } else { 0 };
    let w = ((label_w + n_w + 8).max(foot) as u16).min(area.width);
    if h < 3 {
        return None;
    }
    Some((Rect::new(x.saturating_sub(2).min(area.right() - w).max(area.x), area.y + 1, w, h), (label_w, n_w)))
}

/// The list under a header opened with `d`, each entry with how many models it would show,
/// scrolled so the selection stays in view. Only `rows`, the entries matching the search, are
/// listed; the width fits every entry so the box keeps still while typing. `picked` entries
/// show `✓`, the rest `☐`.
fn dropdown(
    buf: &mut Buffer,
    area: Rect,
    x: u16,
    col: usize,
    items: &[(String, usize)],
    list: &mut List,
    picked: &[String],
) {
    let (query, sel, top) = (list.query.as_str(), list.sel, &mut list.top);
    let rows = &menu_rows(items, query);
    let Some((rect, (label_w, n_w))) = menu_box(area, x, items, rows.len()) else { return };
    let shown = rect.height.saturating_sub(2) as usize;
    // No blank lines under the last entry when a search shortens the list.
    *top = list_top(*top, sel, shown).min(rows.len().saturating_sub(shown));
    let top = *top;
    // Where you are, as every box says it.
    let footer = if rows.len() > shown { position(top, shown, rows.len()) } else { String::new() };
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(fg(Color::Reset))
        .title_bottom(Line::from(footer).style(fg(MUTED)).right_aligned());
    let inner = block.inner(rect);
    Clear.render(rect, buf);
    block.render(rect, buf);
    for (k, &i) in rows.iter().enumerate().skip(top).take(shown) {
        let (label, n) = &items[i];
        let y = inner.y + (k - top) as u16;
        // Entries keep the colour they have in the table, under the cursor like there.
        let color = match i {
            0 => Color::Reset,
            _ if col == PRICE => LEVEL[i - 1],
            _ if col == 1 || col == VIA => dev_color(label),
            _ => Color::Reset,
        };
        // The cursor runs through the box's border, as in the table.
        if k == sel {
            cursor_ends(buf, rect.x, rect.right() - 1, y);
        }
        // Checkboxes as on the table's marks, in the mark's own colour there too, so a picked
        // entry does not read as an empty box in the entry's colour; "any" is ticked while
        // nothing is picked, as it is then what applies.
        let on = if i == 0 { picked.is_empty() } else { picked.contains(label) };
        let (mark, style) = match i {
            _ if on => ("✓ ", fg(MARK).add_modifier(BOLD)),
            _ => ("☐ ", fg(color)),
        };
        let x = buf.set_stringn(inner.x + 1, y, mark, 2, style).0;
        let at = x;
        let x = buf.set_stringn(x, y, format!("{label:<label_w$}  "), label_w + 2, fg(color)).0;
        for r in found(label, query).into_iter().map(|r| r.start.min(label_w)..r.end.min(label_w)) {
            buf.set_style(Rect::new(at + r.start as u16, y, r.len() as u16, 1), hit_style(fg(color)));
        }
        buf.set_stringn(x, y, format!("{n:>n_w$}"), n_w, fg(MUTED));
    }
    vmarks(buf, rect.x, inner.y, inner.bottom() - 1, top > 0, top + shown < rows.len());
}

/// The cursor on row `y`: its fill from the column `left` to `right`, and its two bars at those,
/// the borders of the frame or the box, which the bars take the place of.
fn cursor_ends(buf: &mut Buffer, left: u16, right: u16, y: u16) {
    buf.set_style(Rect::new(left, y, right - left + 1, 1), FILL);
    for (x, bar) in [(left, "▌"), (right, "▐")] {
        if let Some(cell) = buf.cell_mut((x, y)) {
            cell.set_symbol(bar).set_style(EDGE);
        }
    }
}

/// `spans` with a cell at each end: under the cursor those are its bars and the spans are on its
/// fill, keeping their colours; else they are blank, so nothing moves when the cursor does.
fn cursor(on: bool, spans: Vec<Span<'static>>) -> Vec<Span<'static>> {
    if !on {
        return [Span::raw(" ")].into_iter().chain(spans).chain([Span::raw(" ")]).collect();
    }
    let fill = spans.into_iter().map(|s| s.patch_style(FILL));
    [Span::styled("▌", EDGE)].into_iter().chain(fill).chain([Span::styled("▐", EDGE)]).collect()
}

/// `▲` and `▼` on the left border column `x`, at the first and last content row, for rows
/// scrolled off above or below. Every scrolling list uses these, as `‹` `›` mark columns. On a
/// marked row's solid fill, which runs through the border, the accent would not read, so the
/// mark is black as the rest of that row.
fn vmarks(buf: &mut Buffer, x: u16, top: u16, bottom: u16, above: bool, below: bool) {
    let mut put = |y: u16, glyph: &str| {
        let solid = buf.cell((x, y)).is_some_and(|c| c.fg == Color::Black);
        buf.set_stringn(x, y, glyph, 1, fg(if solid { Color::Black } else { ACCENT }).add_modifier(BOLD));
    };
    if above {
        put(top, "▲");
    }
    if below {
        put(bottom, "▼");
    }
}

/// The API key prompt's label: it names the site a key comes from, which is its link.
fn key_ask(wrong: bool) -> String {
    let (ask, site) = (if wrong { "key rejected, another" } else { "API key" }, data::Source::Aa.site());
    format!("{ask} from {site} (or set {}): ", data::AA_KEY_ENV)
}

/// The status bar columns of the link in the API key prompt, after the pill and a space.
fn key_link(app: &App) -> Option<std::ops::Range<u16>> {
    let Input::Key { wrong, .. } = app.input else { return None };
    let site = data::Source::Aa.site();
    let start = (mode(app).0.len() + 3 + key_ask(wrong).find(site)?) as u16;
    Some(start..start + site.len() as u16)
}

/// A solid label like a bar module; returns the column after it.
fn pill(buf: &mut Buffer, x: u16, y: u16, text: &str, color: Color, max: u16) -> u16 {
    buf.set_stringn(x, y, format!(" {text} "), max as usize, fg(Color::Black).bg(color).add_modifier(BOLD)).0
}

/// The status bar's left side, joined with " · ": the model count, then what filters it.
fn parts(app: &App) -> Vec<Line<'static>> {
    let scope = match (app.all, app.data.any_available()) {
        (false, true) => "available",
        (false, false) => "all (no access found)",
        (true, _) => "all",
    };
    let part = |s: String, c: Color| Line::styled(s, fg(c));
    // Selected models show out of reach too, as `F` and `E` show theirs, so they are counted apart, as their Via says.
    let out = app.rows.iter().filter(|&&r| !app.in_reach(&app.data.models[r])).count();
    let count = match out {
        0 => format!("{} {scope}", app.rows.len()),
        _ => format!("{} {scope} + {out} {OUT_OF_REACH}", app.rows.len() - out),
    };
    let mut parts = vec![part(count, Color::Reset)];
    if stale(app) {
        parts.push(part(data_age(&app.data), BAD));
    }
    // The marks of models the data has, as S, C and u count them.
    let selected = format!("{} selected", app.marked_shown);
    if app.any_marked() && app.only != Some(MARKED) {
        parts.push(part(selected.clone(), MARK));
    }
    // The one of `S` `F` `E` that is on.
    let only = [(selected.as_str(), MARK), ("★ favorites", STAR), ("✗ excluded", BAD)];
    if let Some(i) = app.only {
        let (name, color) = only[i - MARKED];
        parts.push(part(format!("{name} only"), color));
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
        parts.push(part(format!("{}{sign}{shown}", app.col_name(col)), Color::Yellow));
    }
    if !app.status.is_empty() {
        parts.push(part(app.status.clone(), if app.failed { BAD } else { GOOD }));
    }
    parts
}

/// "data 3h old", or "no data" before a new source's first download.
fn data_age(d: &Data) -> String {
    if d.fetched == 0 { "no data".into() } else { format!("data {} old", age(d.age())) }
}

/// The mode pill's label and colour.
fn mode(app: &App) -> (&'static str, Color) {
    match (&app.input, &app.view) {
        (Input::Search { .. }, _) => ("SEARCH", Color::Blue),
        (Input::Note { .. }, _) => ("NOTE", Color::Blue),
        (Input::Key { .. }, _) => ("KEY", Color::Blue),
        (Input::Bound { .. }, _) => ("BOUND", Color::Yellow),
        (Input::Menu { .. }, _) => ("PICK", Color::Yellow),
        (Input::Quit, _) => ("QUIT", Color::Red),
        (Input::Upgrade, _) => ("UPGRADE", Color::Yellow),
        (Input::Unfavorite, _) => ("UNFAVORITE", Color::Red),
        (Input::Unexclude, _) => ("UNEXCLUDE", Color::Red),
        (Input::Choose { kind, .. }, _) => {
            let name = match kind {
                Kind::Launch => "LAUNCH",
                Kind::Via => "VIA",
                Kind::Fav => "FAV",
                Kind::Theme => "THEME",
                Kind::Source => "SOURCE",
                Kind::Harness => "HARNESS",
                Kind::Open => "OPEN",
            };
            (name, Color::Green)
        }
        (Input::None, View::Table) if app.selecting() => ("HIGHLIGHT", Color::Yellow),
        (Input::None, View::Table) => ("NORMAL", Color::Magenta),
        (Input::None, View::Help) => ("HELP", Color::Cyan),
        (Input::None, View::Recommend) => ("RECOMMEND", Color::Cyan),
        (Input::None, View::Detail(_)) => ("DETAIL", Color::Cyan),
        (Input::None, View::Compare) => ("COMPARE", Color::Cyan),
    }
}

/// Returns the cursor column while a search, note or bound is being typed.
fn status(buf: &mut Buffer, area: Rect, app: &App) -> Option<u16> {
    let width = area.width as usize;
    let (mode, color) = mode(app);
    let end = pill(buf, area.x, area.y, mode, color, area.width);
    let mut x = end + 1;
    if matches!(
        &app.input,
        Input::Quit
            | Input::Upgrade
            | Input::Unfavorite
            | Input::Unexclude
            | Input::Choose { list: List { typing: false, .. }, .. }
    ) {
        // The question is in a box in the middle of the screen. Under the first start's, the
        // start's warning shows until the pick reports it.
        // And what a pick could not save, which the next key clears.
        if let Some(w) = app.store.warning.as_deref().or(app.failed.then_some(app.status.as_str())) {
            buf.set_stringn(x, area.y, w, usize::from(area.right().saturating_sub(x)), fg(BAD));
        }
        return None;
    }
    // The prompt's label, the text being typed and the cursor's byte offset in it.
    let masked;
    let prompt = match &app.input {
        Input::Search { cur, .. } => {
            let q = if app.overlay_search() { &app.overlay_query } else { &app.query };
            Some(("/".to_string(), q, *cur))
        }
        Input::Note { text, cur, .. } => Some(("note: ".to_string(), text, *cur)),
        // Hidden from anyone looking at the screen; one `*` per byte keeps `cur` in place.
        Input::Key { text, cur, wrong } => {
            masked = "*".repeat(text.len());
            Some((key_ask(*wrong), &masked, *cur))
        }
        Input::Bound { col, min, text, cur } => {
            Some((format!("{} {} ", app.col_name(*col), if *min { "≥" } else { "≤" }), text, *cur))
        }
        Input::Menu { col, list, .. } => Some(match (list.typing, list.query.is_empty()) {
            (false, true) => (format!("{} ▾", app.col_name(*col)), &list.query, list.cur),
            _ => (format!("{} ▾ /", app.col_name(*col)), &list.query, list.cur),
        }),
        Input::Choose { title, list, .. } => Some((format!("{title} /"), &list.query, list.cur)),
        Input::Quit | Input::Upgrade | Input::Unfavorite | Input::Unexclude | Input::None => None,
    };
    if let Some((label, typed, cur)) = prompt {
        // Ctx is bounded in thousands of tokens, as its cells read: `Ctx ≥ 200k`.
        let unit = match app.input {
            Input::Bound { col, .. } if crate::app::numeric(col).is_some_and(|c| c.id == "ctx") => "k",
            _ => "",
        };
        let (menu, typing) = match &app.input {
            Input::Menu { list, .. } => (true, list.typing),
            Input::Choose { .. } => (true, true),
            _ => (false, true),
        };
        let toggles = matches!(app.input, Input::Menu { .. }) || app.choosing_favs();
        let hint = match (menu, typing) {
            (true, false) => "j k move  / search  space enter toggle  esc close",
            (true, true) if toggles => "↓ ↑ move  enter toggle  esc clear",
            (true, true) => "↓ ↑ move  enter pick  esc clear",
            _ => "enter apply  esc cancel",
        };
        let width = |s: &str| Span::raw(s).width();
        // The hint shows only where it leaves the label and a few typed cells their room.
        let fits = usize::from(x) + width(&label) + 8 + width(hint) < usize::from(area.right());
        let hx = if fits { area.right() - (width(hint) as u16 + 1) } else { area.right() };
        // Text too long for the room scrolls sideways, so the cursor stays in view.
        let room = usize::from(hx.saturating_sub(x));
        let start = scrolled(width(&label), typed, cur, room);
        let text = format!("{label}{}{unit}", &typed[start..]);
        buf.set_stringn(x, area.y, &text, room, Style::new());
        // Underlined as a link: a click on it opens the page (`hit`).
        if let Some(link) = key_link(app) {
            let w = (link.end.min(hx)).saturating_sub(link.start);
            buf.set_style(Rect::new(link.start, area.y, w, 1), Style::new().add_modifier(Modifier::UNDERLINED));
        }
        if fits {
            buf.set_stringn(hx, area.y, hint, width(hint), fg(MUTED));
        }
        let cx = x + (width(&label) + width(&typed[start..cur])) as u16;
        return typing.then_some(cx.min(hx.saturating_sub(1)));
    }
    let parts = parts(app);
    // Key hints fill what the left side leaves free; whole hints drop from the front on
    // narrow terminals, and the way back is the last to go.
    let (hints, start) = hint_layout(app, &parts, area.width);
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
    // The last hint stays however narrow the bar: where it ran over the pill, the pill wins.
    if area.x + start < end {
        pill(buf, area.x, area.y, mode, color, area.width);
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

/// How a search hit is drawn, as in the table: yellow, bold and underlined.
fn hit_style(style: Style) -> Style {
    style.fg(MATCH).add_modifier(BOLD | Modifier::UNDERLINED)
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

/// What stays of a row of `f`'s grid before the text written on it: the `+` of the entry that
/// names a task, and for what a task is about its name.
fn edit_prefix(e: &Edit) -> String {
    match &e.what {
        What::New => " + ".into(),
        What::Rename(_) => " ".into(),
        What::About(task) => format!(" {task}  "),
    }
}

/// A row of `f`'s grid while it is written in place: `edit_prefix`, then the text, or in grey
/// what to write while there is none. Text too long for `room` cells scrolls sideways.
fn edit_line(e: &Edit, color: Color, room: usize) -> Line<'static> {
    let prefix = edit_prefix(e);
    let empty = match e.what {
        What::About(_) => "what it is about",
        What::New | What::Rename(_) => "its name",
    };
    let text = match e.text.is_empty() {
        true => Span::styled(format!("{empty} "), fg(MUTED)),
        false => Span::raw(format!("{} ", &e.text[scrolled(Span::raw(&prefix).width(), &e.text, e.cur, room)..])),
    };
    Line::from(vec![Span::styled(prefix, fg(color)), text])
}

/// What `/` left of a list when nothing matches, said as the dropdowns say it.
fn no_match(query: &str) -> Line<'static> {
    Line::from(format!(" no entry matches {query} ")).style(fg(MUTED))
}

/// The cells a box of `f`'s grid takes along its row: the cursor's bars around the mark, and
/// the gap to the next.
const BOX_W: usize = 6;

/// How wide the names of `f`'s grid are: its boxes start two cells after.
fn fav_name_w(items: &[(String, Effect)]) -> usize {
    items.iter().map(|(label, _)| Span::raw(label.as_str()).width()).max().unwrap_or(0)
}

/// `f`'s grid: the tiers' heading, a row per task with a box for the task and for each tier,
/// `✓` where the model is the favorite, `●` where another is and `☐` where none, the entry that
/// names a new task, what the box under the cursor holds, and the keys. Read from the store at
/// each draw, so the boxes follow a change made anywhere.
fn fav_lines(app: &App, items: &[(String, Effect)], list: &List, room: usize) -> Vec<Line<'static>> {
    let query = list.query.as_str();
    let rows = choice_rows(items, query);
    let w = fav_name_w(items);
    let tiers = std::iter::once("any").chain(crate::view::TIERS.iter().map(|t| t.0));
    let head: String = tiers.map(|t| format!(" {t:<0$}", BOX_W - 1)).collect();
    let mut lines = vec![Line::from(format!(" {:w$} {head}", "")).style(fg(MUTED))];
    let mut under = String::new();
    for (i, &k) in rows.iter().enumerate() {
        let (label, effect) = &items[k];
        let on = i == list.sel;
        let Effect::Fav(key, task) = effect else {
            // The entry that names a new task.
            lines.push(match (&list.edit, on) {
                (Some(e), true) => edit_line(e, Color::Reset, room),
                _ => Line::from(cursor(on, vec![Span::raw(label.clone())])),
            });
            continue;
        };
        let color = task_color(task);
        if let (Some(e), true) = (&list.edit, on) {
            lines.push(edit_line(e, color, room));
            continue;
        }
        let name = lit(Line::from(Span::styled(label.clone(), fg(color))), |s| found(s, query));
        // Padded by the cells the name takes, so a wide one keeps its boxes under the heading.
        let pad = w - Span::raw(label.as_str()).width();
        let mut spans = vec![Span::raw(" ")];
        spans.extend(name.spans);
        spans.push(Span::raw(" ".repeat(pad + 1)));
        for col in 0..BOXES {
            let slot = box_slot(task, col);
            let mark = match app.store.favorite(&slot) {
                Some(k) if k == key => Span::styled("✓", fg(MARK).add_modifier(BOLD)),
                Some(_) => Span::styled("●", fg(MUTED)),
                None => Span::styled("☐", fg(color)),
            };
            let at = on && col == list.col;
            spans.extend(cursor(at, vec![mark]));
            spans.push(Span::raw(" ".repeat(BOX_W - 3)));
            if at {
                let name =
                    |k: &str| app.data.models.iter().find(|m| m.key == k).map_or(k.to_string(), |m| m.name.clone());
                under = match (app.store.favorite(&slot), app.store.via(&slot)) {
                    (Some(k), Some(h)) if k == key => format!("{slot}: {}, via {h}", name(k)),
                    (Some(k), _) if k == key => format!("{slot}: {}", name(k)),
                    (Some(k), _) => format!("{slot}: now {}", name(k)),
                    (None, _) => format!("{slot}: no favorite"),
                };
            }
        }
        // What the task is about, yours in your words, cut to what the row has left of `room`.
        let left = room.saturating_sub(w + 2 + BOXES * BOX_W).min(48);
        if let Some(about) = app.store.about(task).or(fit::task(task).map(|t| t.about)).filter(|_| left > 1) {
            spans.push(Span::styled(truncate(about, left), fg(MUTED)));
        }
        lines.push(Line::from(spans));
    }
    if rows.is_empty() {
        lines.push(no_match(query));
    }
    let own = items.iter().any(|(_, e)| matches!(e, Effect::Fav(_, t) if fit::task(t).is_none()));
    let hint = match (list.typing, own) {
        // While searching the letters are typed, as the status bar says.
        (true, _) => " ↓ ↑ move · enter toggle · esc clear",
        // `r` and `a` are said once there is a task of your own. `j k` and enter work too, left
        // unsaid: with them the line is wider than 80 columns hold.
        (_, true) => " h l tier · / search · space toggle · v via · r rename · a about · esc close",
        _ => " j k h l move · / search · space enter toggle · v via · esc close",
    };
    // As wide as the hint at most, so the box keeps its width as the cursor moves.
    let width = hint.chars().count();
    lines.push(Line::default());
    lines.push(Line::from(format!(" {}", truncate(&under, width - 1))).style(fg(MUTED)));
    // A row being written takes every key; a name enter did not take says why. As wide as the
    // hint it stands for.
    lines.push(match &list.edit {
        Some(Edit { err: Some(err), .. }) => Line::from(format!("{:<width$}", format!(" {err}"))).style(fg(BAD)),
        Some(_) => Line::from(format!("{:<width$}", " enter apply · esc cancel")).style(fg(MUTED)),
        None => Line::from(hint).style(fg(MUTED)),
    });
    lines
}

/// The link in a source's entry of the "benchmarks?" list, as bytes of its ASCII label: the site
/// its API key is had at, which its description ends with.
fn source_link(label: &str, effect: &Effect) -> Option<Range<usize>> {
    let site = data::Source::Aa.site();
    (*effect == Effect::Source(data::Source::Aa) && label.ends_with(site))
        .then(|| label.len() - site.len()..label.len())
}

/// The entries of a choice list, each coloured by its first word: the harness or the site.
/// `first`: the first start's question, where esc picks the default and `B` asks again later.
/// `f`'s grid has lines of its own, `fav_lines`.
fn choice_lines(kind: Kind, items: &[(String, Effect)], list: &List, first: bool) -> Vec<Line<'static>> {
    let (query, typing) = (list.query.as_str(), list.typing);
    let rows = choice_rows(items, query);
    let mut lines: Vec<Line> = rows
        .iter()
        .map(|&k| {
            let (label, effect) = &items[k];
            // What has a colour in the table keeps it here: a harness. A site, a theme and a
            // source have none, so they take the text's and not one their name gives.
            let color = match effect {
                Effect::Launch(_) | Effect::Via(_, _, Some(_)) | Effect::Harness(Some(_)) => {
                    dev_color(label.split(' ').next().unwrap_or_default())
                }
                _ => Color::Reset,
            };
            // A name, then what follows it muted: a source and what it takes, a harness and
            // the command that opens it.
            let name = match effect {
                Effect::Source(src) => Some((label.len() - src.about().len(), Style::new().add_modifier(BOLD))),
                Effect::Launch(_) | Effect::Via(..) | Effect::Harness(_) => {
                    Some((label.find(' ').unwrap_or(label.len()), fg(color)))
                }
                _ => None,
            };
            if let Some((end, style)) = name {
                let (name, rest) = label.split_at(end);
                // Where a key is had is a link: underlined, and a click on it opens the page (`hit`).
                let line = Line::from(match source_link(label, effect) {
                    Some(l) => vec![
                        Span::styled(format!(" {name}"), style),
                        Span::styled(label[end..l.start].to_string(), fg(MUTED)),
                        Span::styled(label[l].to_string(), fg(MUTED).add_modifier(Modifier::UNDERLINED)),
                        Span::styled(" ", fg(MUTED)),
                    ],
                    None => vec![Span::styled(format!(" {name}"), style), Span::styled(format!("{rest} "), fg(MUTED))],
                });
                return lit(line, |s| found(s, query));
            }
            lit(Line::from(format!(" {label} ")).style(fg(color)), |s| found(s, query))
        })
        .collect();
    if rows.is_empty() {
        lines.push(no_match(query));
    }
    let hint = match kind {
        _ if typing => " ↓ ↑ move · enter pick · esc clear",
        Kind::Fav => unreachable!("f's grid has lines of its own, fav_lines"),
        Kind::Theme => " j k preview · / search · enter saves · esc t close",
        Kind::Source if first => " j k move · / search · enter picks · esc default · B changes it later",
        Kind::Source | Kind::Harness => " j k move · / search · enter picks · esc close",
        Kind::Via => " j k move · / search · enter picks · esc back",
        Kind::Open | Kind::Launch => " j k move · / search · enter opens · esc close",
    };
    lines.push(Line::from(hint).style(fg(MUTED)));
    lines
}
/// The first line in view of a list `shown` lines tall with the cursor on `sel`: where it was,
/// `top`, moved only as far as it takes to show the cursor, as the table scrolls.
fn list_top(top: usize, sel: usize, shown: usize) -> usize {
    top.clamp(sel.saturating_sub(shown.max(1) - 1), sel)
}

/// An open chooser's lines and the area its box is centred in, for `draw` and `hit` alike: the
/// frame between the tabs and the status bar, or on the first start the rows under the wordmark
/// (`splash`). A terminal too short for a box there has it over the tabs.
fn chooser(app: &App, area: Rect) -> Option<(Rect, Vec<Line<'static>>)> {
    let Input::Choose { kind, items, list, .. } = &app.input else { return None };
    let tabs = if area.height < 6 { 0 } else { 2 };
    let body = Rect { y: area.y + tabs, height: area.height - 1 - tabs, ..area };
    let within = splash(app, area).map_or(body, |s| s.1);
    let lines = match kind {
        Kind::Fav => fav_lines(app, items, list, edit_room(within)),
        _ => choice_lines(*kind, items, list, app.first_start),
    };
    Some((within, lines))
}

/// The cells a line has in a box as wide as `within`, between its borders and their margins.
fn edit_room(within: Rect) -> usize {
    usize::from(within.width).saturating_sub(4)
}

/// Where text too long for `room` cells starts, `before` cells in, so that the cursor at byte
/// `cur` stays in view: the byte offset of the first character shown.
fn scrolled(before: usize, text: &str, cur: usize, room: usize) -> usize {
    let mut start = 0;
    while start < cur && before + Span::raw(&text[start..cur]).width() >= room {
        start += text[start..].chars().next().map_or(1, char::len_utf8);
    }
    start
}

/// Where an overlay with these lines sits: centred, as wide as its widest line or title.
fn overlay_rect(area: Rect, title: &str, lines: &[Line]) -> Rect {
    let widest = lines.iter().map(Line::width).max().unwrap_or(0) as u16;
    let w = (widest + 4).max(title.chars().count() as u16 + 6).min(area.width);
    let h = (lines.len() as u16 + 2).min(area.height);
    Rect::new(area.x + (area.width - w) / 2, area.y + (area.height - h) / 2, w, h)
}

type Lines = std::ops::Range<usize>;

/// How many of an overlay's `pin` lines stay put under the others, which scroll: all of them in
/// a box too short for every line, unless they would leave the others no row.
fn pinned(lines: usize, shown: usize, pin: &Lines) -> usize {
    if lines > shown && pin.len() < shown { pin.len() } else { 0 }
}

/// Where overlay lines show something to click: cells `x` of lines `lines`, and what is there.
struct Spot {
    lines: std::ops::Range<usize>,
    x: std::ops::Range<usize>,
    at: Stop,
}

/// A centred rounded box in `border`, its title bold in the text's colour, showing `lines` from
/// `scroll` on, which is clamped to the content: the accent is left to the table's headers and
/// the cursor. The lines `pin`, which say what the cursor is on, stay at the bottom of a box
/// too short for every line (`pinned`). Returns where the lines went, and whether some are
/// scrolled off above and below.
fn overlay(
    buf: &mut Buffer,
    area: Rect,
    title: &str,
    mut lines: Vec<Line<'static>>,
    scroll: &mut u16,
    border: Color,
    (head, tail, pin): (usize, usize, Lines),
) -> (Rect, bool, bool) {
    let rect = overlay_rect(area, title, &lines);
    let h = rect.height;
    let rows = h.saturating_sub(2) as usize;
    let pin: Vec<_> = if pinned(lines.len(), rows, &pin) > 0 { lines.drain(pin).collect() } else { vec![] };
    let shown = rows - pin.len();
    *scroll = (*scroll).min(lines.len().saturating_sub(shown) as u16);
    // Where you are; the keys are in the status bar. The first `head` lines, a grid's heading,
    // and the last `tail`, a list's hint, are none of what is counted.
    let (top, n) = (*scroll as usize, lines.len() - head - tail);
    let footer = if n + head > shown {
        position(top.saturating_sub(head), shown - head.saturating_sub(top).min(shown), n)
    } else {
        String::new()
    };
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(fg(border))
        .title_top(Line::from(format!(" {title} ")).style(Style::new().add_modifier(BOLD)))
        .title_bottom(Line::from(footer).style(fg(MUTED)).right_aligned());
    // Inside the border, a cell of padding either side.
    let text = rect.inner(ratatui::layout::Margin::new(2, 1));
    Clear.render(rect, buf);
    block.render(rect, buf);
    let (above, below) = (*scroll > 0, *scroll as usize + shown < lines.len());
    for (line, y) in lines.into_iter().skip(top).take(shown).chain(pin).zip(text.y..text.bottom()) {
        line.render(Rect { y, height: 1, ..text }, buf);
    }
    // A box of one line, under the tabs on a screen of four, has no text to mark.
    if text.height > 0 {
        vmarks(buf, rect.x, text.y, text.y + shown as u16 - 1, above, below);
    }
    (text, above, below)
}

/// Where a box scrolled to `top` is among its `n` lines or entries, for its bottom border.
fn position(top: usize, shown: usize, n: usize) -> String {
    format!(" {}-{} of {n} ", (top + 1).min(n), (top + shown).min(n))
}

fn heading(text: &str) -> Line<'static> {
    Line::from(text.to_string()).style(Style::new().add_modifier(BOLD))
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
    v.push(Line::from("Columns: described in the top border; green the best shown, red the worst").style(fg(MUTED)));
    v.push(Line::from(format!("Saved in {}", crate::store::path().display())).style(fg(MUTED)));
    v.push(Line::from("Every key: github.com/danctorres/modelcmp/blob/main/KEYS.md").style(fg(MUTED)));
    v.push(Line::from("CLI: modelcmp --help").style(fg(MUTED)));
    // `/` keeps the lines that match, so a key can be looked up in a long list.
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

/// What a box of recommend's grid shows.
const TIER_LEGEND: &str = "each tier's model: ★ your favorite, else the cheapest that scores enough (high: the best), \
                           as name $/1M tokens (score on the task)";

/// A row per task, the built-in ones and then your own, with a box per tier for the model it
/// picks, as `--tier` does, wrapped to `width`; under the grid, what the cursor's task is, when
/// to use it and its box's model in full. Where each name and box is goes to `spots`, and the
/// lines of the cursor's row come back too, with the ones under the grid, for `overlay` to pin.
fn recommend(app: &App, width: usize, spots: &mut Vec<Spot>) -> (Vec<Line<'static>>, Option<Lines>, Lines) {
    let mut block = None;
    let label = |s: &'static str| vec![Span::styled(s, fg(MUTED))];
    let words = |s: &str| s.split(' ').map(|w| Line::from(w.to_string())).collect();
    let space = Span::raw(" ");
    // The models it ranks, a tab of the table's each: the one on has the filled dot, bold in the
    // colour its tab is on in, and one with nothing to show is grey.
    let tabs = (0..EXCLUDED).map(|i| {
        let on = i == app.among_on();
        let style = match on {
            true => fg(tab_color(i)).add_modifier(BOLD),
            _ if app.tab_has(i) => Style::new(),
            _ => fg(MUTED),
        };
        let text = format!("{} {}", if on { "●" } else { "○" }, TABS[i].0);
        Line::from(cursor(app.among == Some(i), vec![Span::styled(text, style)]))
    });
    let (mut v, at) = wrapped_at(label("among:"), tabs.collect(), &Span::styled("·", fg(MUTED)), width);
    spots.extend(at.into_iter().enumerate().map(|(i, (l, x))| Spot { lines: l..l + 1, x, at: Stop::Among(i) }));
    if app.filtered_too() {
        let note = Span::styled(" (filtered)", fg(MUTED));
        // On a line of its own under the tabs when the last has no room for it.
        if v.last().unwrap().width() + note.width() > width {
            v.push(Line::from(" ".repeat("among:".len())));
        }
        v.last_mut().unwrap().push_span(note);
    }
    let among = v.len();
    v.extend(wrapped(vec![], words(TIER_LEGEND), &space, width).into_iter().map(|l| l.style(fg(MUTED))));
    v.push(Line::default());
    let own = app.store.custom_tasks();
    let about = |t| app.store.about(t).unwrap_or(CUSTOM_ABOUT);
    let tasks: Vec<(&str, &str, &str)> = TASKS
        .iter()
        .map(|t| (t.name, t.about, t.when))
        .chain(own.iter().map(|&t| (t, about(t), CUSTOM_WHEN)))
        .collect();
    let picks: Vec<_> = (0..tasks.len()).map(|i| app.tier_picks(i)).collect();
    let (tasks, picks) = (&tasks, &picks);
    let cells = |i: usize, room| (0..TIERS.len()).map(move |c| entry(app, tasks[i].0, c, picks[i][c], room));
    // The names' column, then a box per tier: as wide as the widest entry, or as `width` leaves
    // each, where a name is cut to fit.
    let w = tasks.iter().map(|t| Span::raw(t.0).width()).max().unwrap_or(0).min(20);
    let mut grid: Vec<Vec<_>> = (0..tasks.len()).map(|i| cells(i, Some(usize::MAX)).collect()).collect();
    let widest = grid.iter().flatten().map(|e| e.iter().map(Span::width).sum::<usize>()).max().unwrap_or(0);
    let room = (width.saturating_sub(w + 2) / TIERS.len()).saturating_sub(2).clamp(3, widest.max(4));
    // Made again, cut, only where the width leaves a box less than its entry takes.
    if widest > room {
        grid = (0..tasks.len()).map(|i| cells(i, Some(room)).collect()).collect();
    }
    let head: String = TIERS.iter().map(|t| format!(" {:<room$} ", t.0)).collect();
    v.push(Line::from(format!(" {:w$} {head}", "")).style(fg(MUTED)));
    let cur = app.among.is_none().then_some((app.task_cur, app.task_sel));
    for (i, (t, row)) in tasks.iter().zip(grid).enumerate() {
        let y = v.len();
        let name = truncate(t.0, w);
        let pad = w.saturating_sub(Span::raw(name.as_str()).width());
        let mut spans = cursor(cur == Some((i, 0)), vec![Span::styled(name, fg(task_color(t.0)).add_modifier(BOLD))]);
        spans.push(Span::raw(" ".repeat(pad)));
        // One pick takes the row: a box as wide as the tiers', which every stop past the name is on.
        let one = app.one_pick(i);
        let (boxes, room) = if one { (1, TIERS.len() * (room + 2) - 2) } else { (TIERS.len(), room) };
        for (c, mut cell) in row.into_iter().take(boxes).enumerate() {
            // Padded to the box, so the cursor's fill is as wide on every one.
            cell.push(Span::raw(" ".repeat(room.saturating_sub(cell.iter().map(Span::width).sum()))));
            let x = w + 2 + c * (room + 2);
            spots.push(Spot { lines: y..y + 1, x: x..x + room + 2, at: Stop::Recommend(i, c + 1) });
            spans.extend(cursor(cur.is_some_and(|(row, sel)| row == i && (sel == c + 1 || one && sel > 0)), cell));
        }
        spots.push(Spot { lines: y..y + 1, x: 0..width, at: Stop::Recommend(i, 0) });
        if cur.is_some_and(|c| c.0 == i) {
            block = Some(y..y + 1);
        }
        v.push(Line::from(spans));
    }
    let pin = v.len();
    v.push(Line::default());
    let mut under = Vec::new();
    if let Some((i, sel)) = cur.filter(|c| c.0 < tasks.len()) {
        let t = tasks[i];
        let name = vec![Span::styled(t.0.to_string(), fg(task_color(t.0)).add_modifier(BOLD)), space.clone()];
        under.extend(wrapped(name, words(t.1), &space, width));
        under.extend(wrapped(label("  use for: "), words(t.2), &space, width));
        if let Some(c) = sel.checked_sub(1).filter(|&c| c < TIERS.len()) {
            let tier = if app.one_pick(i) { "pick" } else { TIERS[c].0 };
            let mut line = vec![Span::styled(format!("  {:<9}", format!("{tier}:")), fg(MUTED))];
            line.extend(entry(app, t.0, c, picks[i][c], None));
            under.push(Line::from(line));
        }
    }
    // Three lines whatever the cursor is on, so the box keeps its height as it moves.
    under.resize(under.len().max(3), Line::default());
    v.extend(under);
    let pin = pin..v.len();
    v.push(Line::default());
    let cli = words(
        "CLI: modelcmp recommend · modelcmp list --task <task> [--tier low|mid|high] · modelcmp fav <task> <model> [--tier low|mid|high]",
    );
    v.extend(wrapped(vec![], cli, &space, width).into_iter().map(|l| l.style(fg(MUTED))));
    (v, block.or(Some(0..among)), pin)
}

/// `label` then `items` joined by `glue`, broken between items at `width`, continuation
/// lines indented to the label. A break keeps the glue's punctuation ("," or " ·") at the end.
fn wrapped(
    label: Vec<Span<'static>>,
    items: Vec<Line<'static>>,
    glue: &Span<'static>,
    width: usize,
) -> Vec<Line<'static>> {
    wrapped_at(label, items, glue, width).0
}

/// `wrapped`, with the line and cells each item landed on.
fn wrapped_at(
    label: Vec<Span<'static>>,
    items: Vec<Line<'static>>,
    glue: &Span<'static>,
    width: usize,
) -> (Vec<Line<'static>>, Vec<(usize, std::ops::Range<usize>)>) {
    let indent: usize = label.iter().map(Span::width).sum();
    let end = glue.content.trim_end();
    let end_w = Span::raw(end).width();
    let (mut lines, mut at) = (Vec::new(), Vec::new());
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
        at.push((lines.len(), w..w + item.width()));
        w += item.width();
        cur.extend(item.spans);
    }
    lines.push(Line::from(cur));
    (lines, at)
}

/// What tier `col` of `task` picks, for its box of recommend's grid: `name $price (score)` in
/// its price level's colour as in the Price column, with no score on a task of your own, after
/// a ★ in the task's colour when it is your favorite for the tier; grey when that favorite is
/// not one recommended, or is out of reach as its row in the table, and `-` with no model. With
/// `room` the name is cut from its start to fit it; without, the entry is whole and says why it
/// is grey.
fn entry(app: &App, task: &str, col: usize, pick: Option<(&Model, f64)>, room: Option<usize>) -> Vec<Span<'static>> {
    let built_in = fit::task(task);
    let Some((m, s)) = pick else {
        let none = if room.is_some() {
            "-"
        } else if built_in.is_some() {
            "no data"
        } else {
            "no favorite"
        };
        return vec![Span::styled(none, fg(MUTED))];
    };
    let fav = app.store.tier_favorites(task, TIERS[col].0).any(|k| k == m.key);
    let off = fav && built_in.is_some_and(|t| app.favorite_unrecommended(t, &m.key));
    let out = out_of_reach(app, m);
    let grey = off || out.is_some();
    let price = m.quoted().filter(|q| !(grey || q.1)).map_or(MUTED, |q| LEVEL[level(q.0.blended())]);
    let rest = match built_in {
        Some(t) => priced(m, fit::shown(m, t, s), false, false)[m.name.len()..].to_string(),
        None => format!(" {}", crate::view::price(m)),
    };
    let n = m.name.chars().count();
    let text = match room.map(|r| r.saturating_sub(if fav { 2 } else { 0 })) {
        Some(r) if n + rest.chars().count() > r => match r.saturating_sub(rest.chars().count()) {
            // No room for a name: what fits of the whole, so the box keeps its width.
            0 | 1 => truncate(&format!("{}{rest}", m.name), r.max(1)),
            // The end of a name tells it from the others, as in `Claude Opus 5.5`, so the cut keeps that.
            k => format!("…{}{rest}", m.name.chars().skip(n + 1 - k).collect::<String>().trim_start()),
        },
        _ => format!("{}{rest}", m.name),
    };
    let mut spans = Vec::with_capacity(4);
    if fav {
        spans.push(Span::styled("★ ", fg(task_color(task)).add_modifier(BOLD)));
    }
    spans.push(Span::styled(text, fg(price)));
    if room.is_none() {
        if off {
            // Both read as a list: "not recommended, not available".
            let said = if out.is_some() { " not recommended," } else { " not recommended" };
            spans.push(Span::styled(said, fg(MUTED).add_modifier(Modifier::ITALIC)));
        }
        spans.extend(out);
    }
    spans
}

/// What follows a recommend entry out of reach: "not available", as its Via in the table.
fn out_of_reach(app: &App, m: &Model) -> Option<Span<'static>> {
    (!app.accessible(m)).then(|| Span::styled(format!(" {OUT_OF_REACH}"), fg(MUTED).add_modifier(Modifier::ITALIC)))
}

/// The model's name, the title, then every detail line, with `key:` labels and section headings
/// coloured.
fn detail(
    m: &Model,
    store: &Store,
    any: bool,
    (get, size): (Option<String>, Option<u64>),
) -> (String, Vec<Line<'static>>) {
    let is_label = |k: &str| k.len() < 16 && k.trim().chars().all(|c| c.is_alphabetic() || c == ' ');
    let mut lines = detail_rows(m, store, any, get.as_deref(), size).into_iter();
    let title = lines.next().map(|r| r.1).unwrap_or_default();
    // A task's name in its colour, as its ★ in the table: on its fit line and after `favorite:`.
    let named = |t: &str| Span::styled(t.to_string(), fg(task_color(t)).add_modifier(BOLD));
    let lines = lines
        .map(|(task, s)| match s.split_once(':') {
            Some((k, v)) if k.trim() == "favorite" && v.trim() != "-" => {
                let pad = v.len() - v.trim_start().len();
                let mut spans = vec![Span::styled(format!("{k}:"), fg(KEY)), Span::raw(v[..pad].to_string())];
                for (i, t) in v.trim().split(", ").enumerate() {
                    spans.extend((i > 0).then(|| Span::raw(", ")));
                    spans.push(named(t));
                }
                Line::from(spans)
            }
            _ if let Some((t, (pre, post))) = task.and_then(|t| Some((t, s.split_once(t)?))) => {
                Line::from(vec![Span::raw(pre.to_string()), named(t), Span::raw(post.to_string())])
            }
            Some((k, v)) if s.starts_with("  ") && is_label(k) => {
                Line::from(vec![Span::styled(format!("{k}:"), fg(KEY)), Span::raw(v.to_string())])
            }
            _ if s.ends_with(':') => heading(&s),
            _ => Line::from(s),
        })
        .collect();
    (title, lines)
}

/// The verdict, then the marked models side by side with the best value of each row in green,
/// the worst in red, and the one under the cursor filled between its two bars; a `muted` model's
/// column is grey, its bests and worsts too, as its row in the table. When they do not all fit in
/// `avail` cells, the view starts at model `first`, moved only as far as it takes to show the
/// selection, and the `first` in effect comes back for `App::compare_x`. Each shown model's
/// column, between its two bars and from the model row down, goes to `spots`. `any` is whether
/// you have access to a model, and `get` the mark of a model's download, for Via to read as in
/// the table.
fn compare(
    models: &[&Model],
    (any, get): (bool, crate::view::Get),
    (sel, first): (usize, usize),
    avail: usize,
    query: &str,
    muted: impl Fn(&Model) -> bool,
    spots: &mut Vec<Spot>,
) -> (Vec<Line<'static>>, usize) {
    let mut rows = compare_rows(models, any, get);
    let muted: Vec<bool> = models.iter().map(|m| muted(m)).collect();
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
    // In cells as the terminal draws them, so a wide character keeps the columns in line.
    let widths: Vec<usize> =
        (0..n).map(|i| rows.iter().map(|r| Span::raw(r.cells[i].as_str()).width()).max().unwrap_or(0)).collect();
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
    // Two cells go to the " ›" marking models cut off on the right, but only when there are any;
    // else one, to the right bar of the cursor on the last model.
    let fits = |f: usize| {
        let all = count(f, 1);
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
    let top = out.len();
    // The model row carries `‹` and `›` for models scrolled off, as the table's header does.
    let edge = fg(ACCENT).add_modifier(BOLD);
    let (mut width, mut section) = (0usize, "");
    for (k, r) in rows.into_iter().enumerate() {
        // A rule naming each topic above its first shown row, as wide as the model row.
        if r.section != section {
            section = r.section;
            let name = format!("── {section} ");
            let rest = width.saturating_sub(name.chars().count());
            out.push(Line::styled(name + &"─".repeat(rest), fg(MUTED)));
        }
        let label = Line::from(Span::styled(format!("{:<label_w$}", r.label), fg(KEY)));
        let hit = hits(query, [&r.label], typos).map_or(vec![], |[r]| r);
        let mut spans = lit(label, |_| hit.clone()).spans;
        // The two cells between models are the right bar of the cursor on the one and its left
        // bar on the next, blank off it, so a model's cells stay where they are.
        let bar = |on: bool, bar: &'static str| if on { Span::styled(bar, EDGE) } else { Span::raw(" ") };
        for (i, c) in r.cells.into_iter().enumerate().skip(first).take(shown) {
            let style = match r.ext.and_then(|e| Some((e, r.vals[i]?))) {
                _ if muted[i] => fg(MUTED),
                // A list price, as in the table: not one you'd pay.
                _ if r.listed.get(i) == Some(&true) => fg(MUTED).add_modifier(Modifier::ITALIC),
                Some(((best, _), v)) if v == best => fg(GOOD).add_modifier(BOLD),
                Some(((_, worst), v)) if v == worst => fg(BAD),
                _ => Style::new(),
            };
            if k == 0 && i == first && first > 0 {
                spans.push(Span::styled("‹", edge));
            } else {
                spans.push(bar(i > first && i - 1 == sel, "▐"));
            }
            spans.push(bar(i == sel, "▌"));
            let style = if i == sel { style.bg(CURSOR) } else { style };
            let pad = " ".repeat(widths[i] - Span::raw(c.as_str()).width());
            spans.push(Span::styled(pad + &c, style));
        }
        spans.push(bar(first + shown == sel + 1, "▐"));
        if k == 0 && first + shown < n {
            spans.push(Span::styled("›", edge));
        }
        let line = Line::from(spans);
        if k == 0 {
            // A rule under the model row, as under the table's header.
            width = line.width();
            out.extend([line, Line::styled("─".repeat(width), fg(MUTED))]);
        } else {
            out.push(line);
        }
    }
    // Each model's cells follow the cell before it, `‹` or the bar of the one before; the `‹`
    // and `›` are no model's, as in the table's header.
    let mut x = label_w + 1;
    for (i, w) in widths.iter().enumerate().skip(first).take(shown) {
        spots.push(Spot { lines: top..out.len(), x: x..x + w + 2, at: Stop::Compare(i) });
        x += w + 2;
    }
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
    use crate::app::{Back, ECI, NCOLS};
    use crate::data::Offer;
    use ratatui::crossterm::event::KeyCode;

    #[test]
    fn the_upgrade_is_the_installer_s() {
        let cmd = |exe: &str| upgrade_cmds(std::path::Path::new(exe), "0.2.0").map(|c| c.last().unwrap().join(" "));
        let brew = Some("brew upgrade danctorres/tap/modelcmp".to_string());
        assert_eq!(cmd("/opt/homebrew/Cellar/modelcmp/0.1.0/bin/modelcmp"), brew);
        assert_eq!(cmd("/home/linuxbrew/.linuxbrew/Cellar/modelcmp/0.1.0/bin/modelcmp"), brew);
        let cargo = format!("cargo install --git {REPO} --tag v0.2.0");
        assert_eq!(cmd("/home/you/.cargo/bin/modelcmp"), Some(cargo));
        assert_eq!(cmd("/usr/local/bin/modelcmp"), None, "put there by hand: nothing to run");
        assert_eq!(cmd(""), None, "the path is not known");
    }

    #[test]
    fn the_logo_rows_line_up() {
        let w = LOGO[0].chars().count();
        assert!(LOGO.iter().all(|r| r.chars().count() == w), "centring and the band assume one width");
        assert!(TAGLINE.is_ascii() && TAGLINE.len() <= w, "the intro slices it by byte and centres it in that width");
    }

    #[test]
    fn the_first_start_asks_under_the_wordmark() {
        let screen = |a: &mut App, w: u16, h: u16| {
            let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h)).unwrap();
            term.draw(|f| draw(a, f)).unwrap();
            let buf = term.backend().buffer();
            (0..h).map(|y| (0..w).map(|x| buf[(x, y)].symbol()).collect::<String>()).collect::<Vec<_>>()
        };
        let row = |lines: &[String], pat: &str| lines.iter().position(|l| l.contains(pat)).map(|y| y as u16);
        let mut a = app();
        a.first_start = true;
        a.ask_source();
        a.store.warning = Some("user.json is not valid".into());
        let lines = screen(&mut a, 120, 30);
        assert!(lines[29].contains("user.json is not valid"), "the start's warning shows under it");
        assert_eq!(row(&lines, "Model"), None, "no table behind the question");
        let (tagline, ask) = (row(&lines, TAGLINE).unwrap(), row(&lines, "benchmarks?").unwrap());
        assert!(tagline < ask, "the question is under the wordmark");
        // A click lands on the entry where it is drawn.
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 60,
            row: row(&lines, "Artificial Analysis").unwrap(),
            modifiers: KeyModifiers::NONE,
        };
        assert_eq!(hit(&a, Rect::new(0, 0, 120, 30), click), Some(Mouse::Item(1)));
        // And on its link opens where a key is made, the question staying.
        let site = lines[usize::from(click.row)].find("artificialanalysis.ai").unwrap();
        let column = lines[usize::from(click.row)][..site].chars().count() as u16;
        assert_eq!(hit(&a, Rect::new(0, 0, 120, 30), MouseEvent { column, ..click }), Some(Mouse::Link));
        assert_eq!(hit(&a, Rect::new(0, 0, 120, 30), MouseEvent { column: column - 1, ..click }), Some(Mouse::Item(1)));
        assert_eq!(a.mouse(Mouse::Link), Some(Effect::Open(data::AA_KEY_URL.into())));
        assert!(matches!(a.input, Input::Choose { kind: Kind::Source, .. }));
        // Too small for both: over the table, as `B` asks later.
        let lines = screen(&mut a, 120, 12);
        assert!(row(&lines, "Model").is_some() && row(&lines, TAGLINE).is_none());
        // The key could not be saved: the table, where the status bar says so.
        a.input = Input::None;
        assert_eq!(row(&screen(&mut a, 120, 30), TAGLINE), None);
    }

    fn model(name: &str, dev: &str, eci: Option<f64>, price: f64) -> Model {
        Model {
            key: name.into(),
            name: name.into(),
            developer: dev.into(),
            available: true,
            via: vec!["opencode".into()],
            eci,
            epoch: eci.map(|_| name.into()),
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
        a.col = PRICE;
        let (_, lines) = render(&mut a, 200, 4);
        assert!(!lines[3].contains(" old") && !lines[3].contains("r refresh"), "fresh: {}", lines[3]);
        a.data.fetched -= data::MAX_AGE.as_secs() + 3600;
        let (buf, lines) = render(&mut a, 200, 4);
        assert!(lines[3].starts_with(" NORMAL  2 available · data 25h old"), "{}", lines[3]);
        assert_eq!(buf[(cell(&lines[3], "data"), 3)].fg, BAD);
        assert!(lines[3].contains("s sort  d dropdown  % no cache  r refresh  │  enter details"), "{}", lines[3]);
        a.refreshing = true;
        data::set_cached(0.0);
        let (_, lines) = render(&mut a, 200, 4);
        assert!(
            lines[3].contains("s sort  d dropdown  % 90% cached  │"),
            "off the default, the way back: {}",
            lines[3]
        );
        assert!(!lines[3].contains(" old") && !lines[3].contains("r refresh"), "refreshing: {}", lines[3]);
    }

    #[test]
    fn via_fits_every_harness_of_a_model() {
        let mut a = app();
        let mut data = std::mem::take(&mut a.data);
        data.models[0].via = vec!["claude".into(), "opencode".into(), "pi".into()];
        a.set_data(data);
        let (_, lines) = render(&mut a, 200, 5);
        assert!(lines.iter().any(|l| l.contains("claude, opencode, pi")), "{lines:?}");
    }

    fn render(app: &mut App, width: u16, height: u16) -> (Buffer, Vec<String>) {
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        table(&mut buf, Rect { height: height - 1, ..area }, app);
        status(&mut buf, Rect { y: height - 1, height: 1, ..area }, app);
        let lines = text(&buf);
        (buf, lines)
    }

    /// Each line of `buf` as it reads.
    fn text(buf: &Buffer) -> Vec<String> {
        let a = buf.area;
        (a.y..a.bottom())
            .map(|y| (a.x..a.right()).map(|x| buf[(x, y)].symbol()).collect::<String>().trim_end().to_owned())
            .collect()
    }

    #[test]
    fn theme_swaps_colours_and_paints_its_background() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 3, 1));
        buf[(0, 0)].set_fg(Color::Black).set_bg(Color::LightBlue);
        buf[(1, 0)].set_fg(Color::Reset);
        buf[(2, 0)].set_fg(BAD).set_bg(Color::LightBlue);
        recolor(&mut buf, THEMES[crate::view::theme("nord")].1.as_ref(), None);
        // A marked row's ✗: nord's red, on its background with a little of the light blue in it.
        assert_eq!((buf[(2, 0)].fg, buf[(2, 0)].bg), (Color::from_u32(0xbf616a), Color::from_u32(0x3c4654)));
        // A pill: nord's background as the text, and the light blue kept, being the half of blue
        // that stands furthest from it.
        assert_eq!((buf[(0, 0)].fg, buf[(0, 0)].bg), (Color::from_u32(0x2e3440), Color::from_u32(0xa3d0e8)));
        assert_eq!((buf[(1, 0)].fg, buf[(1, 0)].bg), (Color::from_u32(0xd8dee9), Color::from_u32(0x2e3440)));
        let mut buf = Buffer::empty(Rect::new(0, 0, 1, 1));
        recolor(&mut buf, None, None);
        assert_eq!(buf[(0, 0)].bg, Color::Reset, "the terminal's own colours keep its background");
        // A marked row with them: a solid fill, or a faint one mixed from the terminal's
        // background once it told it, which leaves a pill as it is.
        let mut buf = Buffer::empty(Rect::new(0, 0, 2, 1));
        buf[(0, 0)].set_fg(BAD).set_bg(MARK);
        buf[(1, 0)].set_fg(Color::Black).set_bg(MARK);
        recolor(&mut buf, None, None);
        assert_eq!(buf[(0, 0)].bg, MARK);
        recolor(&mut buf, None, parse_bg("]11;rgb:1e1e/1e1e/2e2eg"));
        assert_eq!((buf[(0, 0)].fg, buf[(0, 0)].bg), (BAD, Color::from_u32(0x252d47)));
        assert_eq!(buf[(1, 0)].bg, MARK);
        // The cursor's is a grey: white in a dark background, black in a light one, or the
        // theme's text colour in its background.
        for (theme, bg, fill) in [(None, 0x1e1e2e, 0x30303e), (None, 0xffffff, 0xeaeaea), (Some("nord"), 0, 0x3b414d)] {
            let mut buf = Buffer::empty(Rect::new(0, 0, 1, 1));
            buf[(0, 0)].set_bg(CURSOR);
            recolor(&mut buf, theme.and_then(|t| THEMES[crate::view::theme(t)].1.as_ref()), Some(bg));
            assert_eq!(buf[(0, 0)].bg, Color::from_u32(fill), "{theme:?} on {bg:06x}");
        }
        assert_eq!([parse_bg(""), parse_bg("rgb:ff/00")], [None, None], "no reply, or half of one");
    }

    /// Text a theme paints must be legible on what is behind it: the WCAG ratio for bold text,
    /// 3:1, for every colour a row or a pill can take, and 2.2:1 on a marked row's faint fill.
    /// Dark text on a light background is thinner to the eye than light on dark, so a light
    /// theme's colours need 5:1, or they are thin next to its plain text; gameboy's four shades
    /// of one green have no room for it.
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
        for (name, p) in THEMES.iter().filter_map(|(n, p)| p.as_ref().map(|p| (n, p))) {
            // A marked row's fill, or the cursor's when that leaves a colour harder to read.
            let fills = [MARK, CURSOR].map(|c| hex(fill(c, p)));
            let on_fill = |c: u32| fills.iter().map(|f| ratio(c, *f)).fold(f64::MAX, f64::min);
            let min = if lum(p.bg) > 0.5 && *name != "gameboy" { 5.0 } else { 3.0 };
            for c in p.accents.iter().chain([&p.text]) {
                assert!(ratio(*c, p.bg) >= min, "{name}: {c:06x} on the background, {:.1}:1", ratio(*c, p.bg));
                assert!(on_fill(*c) >= 2.2, "{name}: {c:06x} on a fill, {:.1}:1", on_fill(*c));
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
                assert!(ratio(text, p.bg) >= min, "{name}: {c:?} as text, {:.1}:1", ratio(text, p.bg));
                assert!(ratio(fill, p.bg) >= 3.0, "{name}: {c:?} as a pill, {:.1}:1", ratio(fill, p.bg));
                assert!(on_fill(text) >= 2.2, "{name}: {c:?} on a fill, {:.1}:1", on_fill(text));
            }
            assert!(ratio(p.ansi[8], p.bg) >= 2.5, "{name}: muted text, {:.1}:1", ratio(p.ansi[8], p.bg));
            assert!(on_fill(p.ansi[8]) >= 2.2, "{name}: muted on a fill, {:.1}:1", on_fill(p.ansi[8]));
        }
    }

    /// Two developers must not get colours that look the same: the accents stay apart by the
    /// weighted RGB distance (a redmean approximation), and their count divides `dev_color`'s 210
    /// so a developer keeps its colour from theme to theme where the counts share a factor.
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

    /// A ticked entry in a list shows it by the colour of its ✓, so that colour cannot look like
    /// a muted ☐. Slot 12 is the mark's and slot 8 the muted one's.
    #[test]
    fn the_mark_parts_from_an_empty_box() {
        for (name, p) in THEMES.iter().filter_map(|(n, p)| p.as_ref().map(|p| (n, p))) {
            let d = apart(p.ansi[12], p.ansi[8]);
            assert!(d >= 130.0, "{name}: ✓ {:06x} and ☐ {:06x} look alike, {d:.0}", p.ansi[12], p.ansi[8]);
        }
    }

    fn hex(c: Color) -> u32 {
        match c {
            Color::Rgb(r, g, b) => u32::from_be_bytes([0, r, g, b]),
            c => panic!("not a theme colour: {c:?}"),
        }
    }

    /// What a row says by colour has to hold in every theme: a best, a worst, a plain and a
    /// muted value are four colours, a task's ★ is its own and not the muted ☆'s, no developer
    /// is in the grey of a model out of reach, and the cursor's fill is not a marked row's.
    #[test]
    fn roles_are_told_apart() {
        for (name, p) in THEMES.iter().filter_map(|(n, p)| p.as_ref().map(|p| (n, p))) {
            let of = |c: Color| hex(resolve(c, Some(p)));
            let far = |what: &str, x: u32, y: u32, min: f64| {
                assert!(apart(x, y) >= min, "{name}: {what} {x:06x} and {y:06x} look alike, {:.0}", apart(x, y));
            };
            let values = [of(GOOD), of(BAD), p.text, of(MUTED)];
            for (i, &x) in values.iter().enumerate() {
                values[i + 1..].iter().for_each(|&y| far("values", x, y, 100.0));
            }
            let tasks = TASK.map(of);
            for (i, &x) in tasks.iter().enumerate() {
                tasks[i + 1..].iter().for_each(|&y| far("tasks", x, y, 60.0));
                far("a task and muted", x, of(MUTED), 60.0);
            }
            p.accents.iter().for_each(|&x| far("a developer and muted", x, of(MUTED), 60.0));
            let [mark, cursor] = [MARK, CURSOR].map(|c| hex(fill(c, p)));
            far("the fills", mark, cursor, 14.0);
            // A match is told from the name around it by its yellow, as a gold ★ and a mid price
            // are from plain text. Amber is one hue, where the underline and the bold say it.
            if *name != "amber" {
                far("yellow and plain text", of(MATCH), p.text, 100.0);
            }
        }
    }

    /// `dev_color` gives each Via name its own place, so every theme needs a colour for each.
    #[test]
    fn every_theme_keeps_the_harnesses_apart() {
        let n = crate::data::vias().map(dev_color).collect::<std::collections::HashSet<_>>().len();
        assert_eq!(n, crate::data::vias().count() - 2, "only omp shares one, pi's, and llama-cli ollama's");
        assert!(DEVS.len() >= n, "the terminal's own: {} for {n}", DEVS.len());
        for (name, p) in THEMES.iter().filter_map(|(n, p)| p.as_ref().map(|p| (n, p))) {
            assert!(p.accents.len() >= n, "{name}: {} accents for {n}", p.accents.len());
        }
    }

    #[test]
    fn f_and_e_hint_the_key_that_empties_them() {
        let mut a = app();
        a.store.toggle_favorite("coding", "flash");
        a.store.toggle_excluded("flash");
        a.rebuild();
        for (tab, hint) in [('F', "D unfavorite all"), ('E', "X unexclude all")] {
            assert!(!hints(&a, 200).contains(&hint), "{hint}: not outside {tab}");
            a.key(KeyCode::Char(tab).into());
            assert!(hints(&a, 200).contains(&hint), "{hint}: a hint in {tab}");
            a.key(KeyCode::Char(tab).into());
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
        a.rebuild();
        let deselect = |a: &App| hints(a, 200).contains(&"u deselect");
        assert!(deselect(&a) && !deselect(&app()), "u is a hint while a model is selected");
        let filled = |y: u16| (0..120).all(|x| buf[(x, y)].bg == MARK && buf[(x, y)].fg == Color::Black);
        assert!(filled(flash), "the row is filled through the border, black on the mark colour");
        assert!((0..120).all(|x| buf[(x, opus)].bg != MARK), "and only that row");
        assert!(buf[(cell(&lines[flash as usize], "flash"), flash)].modifier.contains(BOLD));
        // Out of reach and not marked: the row is muted, but for what says the same on every row.
        let mut data = std::mem::take(&mut a.data);
        data.models[0].available = false;
        a.set_data(data);
        a.key(KeyCode::Char('a').into());
        a.key(KeyCode::Char('G').into()); // off the cursor, which has a fill of its own
        let (buf, lines) = render(&mut a, 160, 5);
        let opus = lines.iter().position(|l| l.contains("opus")).unwrap() as u16;
        let at = |pat: &str| buf[(cell(&lines[opus as usize], pat), opus)].fg;
        assert_eq!(at("opus"), MUTED, "{lines:?}");
        assert_eq!(at("anthropic"), MUTED, "the developer's colour gives way");
        assert_eq!(at("5.0"), MUTED, "and so does the price level's");
        assert_eq!(at("150"), MUTED, "and the best's: the extremes are of the models to use");
        // Marked as well, a favorite and excluded: filled all the same. With the terminal's own
        // colours the fill is solid, so the ★ and the ✗ are black too.
        a.store.marked.push("opus".into());
        a.store.toggle_favorite("coding", "opus");
        a.store.toggle_excluded("opus");
        let (buf, lines) = render(&mut a, 160, 5);
        assert!(lines[opus as usize].contains('★') && lines[opus as usize].contains('✗'), "{lines:?}");
        assert!((0..160).all(|x| buf[(x, opus)].bg == MARK && buf[(x, opus)].fg == Color::Black), "{lines:?}");
        assert!(buf[(cell(&lines[opus as usize], "opus"), opus)].modifier.contains(BOLD));
        assert!(lines[opus as usize].contains("not available"), "{lines:?}");
        // A search hit on the fill has the yellow behind it, still black.
        a.query = "opus".into();
        let (buf, lines) = render(&mut a, 160, 5);
        let hit = &buf[(cell(&lines[opus as usize], "opus"), opus)];
        assert_eq!((hit.fg, hit.bg), (Color::Black, MATCH), "{lines:?}");
        // A theme's fill is faint (`recolor`), so there the row keeps its colours on it.
        a.store.theme = "nord".into();
        let (buf, lines) = render(&mut a, 160, 5);
        let at = |pat: &str| buf[(cell(&lines[opus as usize], pat), opus)].fg;
        assert!((0..160).all(|x| buf[(x, opus)].bg == MARK), "{lines:?}");
        assert_eq!([at("✓"), at("★"), at("✗"), at("opus")], [MARK, STAR, BAD, MATCH], "{lines:?}");
        a.query.clear();
        let (buf, lines) = render(&mut a, 160, 5);
        let at = |pat: &str| buf[(cell(&lines[opus as usize], pat), opus)].fg;
        assert_eq!([at("opus"), at("anthropic")], [MUTED, MUTED], "muted as off the fill: {lines:?}");
        a.store.theme.clear();
        // And so with the terminal's own colours, once it told its background.
        a.term_bg = Some(0);
        let (buf, lines) = render(&mut a, 160, 5);
        assert_eq!(buf[(cell(&lines[opus as usize], "★"), opus)].fg, STAR, "{lines:?}");
        a.term_bg = None;
        // In compare its column is muted.
        let rows =
            compare(&a.marked_models(), (a.any_available(), &|_| None), (0, 0), 200, "", |m| a.muted(m), &mut vec![]).0;
        let names = rows.iter().find(|l| l.to_string().starts_with("model ")).unwrap();
        let opus = names.spans.iter().find(|s| s.content.contains("opus")).unwrap();
        assert_eq!(opus.style.fg, Some(MUTED), "{names:?}");
        // Back in the available view it shows only for being marked, and says so.
        a.key(KeyCode::Char('A').into());
        let (_, lines) = render(&mut a, 160, 5);
        let opus = lines.iter().position(|l| l.contains("opus")).unwrap();
        assert!(lines[opus].contains("not available"), "{lines:?}");
        assert!(lines[4].starts_with(" NORMAL  1 available + 1 not available"), "{}", lines[4]);
        // Its tab counts it, and a click past the count is still on the next tab.
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(160, 20)).unwrap();
        term.draw(|f| draw(&mut a, f)).unwrap();
        let tabs = &text(term.backend().buffer())[1];
        assert!(tabs.contains(" yours +1 A "), "{tabs}");
        assert_eq!(tab_ends(&a).next(), Some(14));
    }

    #[test]
    fn access_to_no_model_says_so_above_the_first_row() {
        let mut a = app();
        assert!(!render(&mut a, 120, 6).1.concat().contains(NO_ACCESS));
        let mut data = std::mem::take(&mut a.data);
        data.models.iter_mut().for_each(|m| m.available = false);
        a.set_data(data);
        let (buf, lines) = render(&mut a, 120, 6);
        assert!(lines[2].contains(NO_ACCESS) && buf[(cell(&lines[2], "no harness"), 2)].fg == Color::Yellow);
        assert!(lines[3].contains("opus") && lines[5].starts_with(" NORMAL  2 all (no access found)"), "{lines:?}");
        a.key(KeyCode::Char('a').into());
        assert!(!a.all && a.status == NO_ACCESS, "and the key says so: {}", a.status);
        // The clicks on the rows start under it too; the tabs and the frame put them three lines further down.
        let click = |row| MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 12,
            row,
            modifiers: KeyModifiers::NONE,
        };
        let on = |row| hit(&a, Rect::new(0, 0, 120, 9), click(row));
        assert_eq!((on(5), on(6)), (None, Some(Mouse::Cell(0, 0))));
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
        let (buf, lines) = render(&mut a, 206, 6);
        let header = "# Model Dev ▾ Released │ Price ▾ $in $cache $out Ctx │ ▼ECI Coding ▾ Agentic ▾ \
                      Reason ▾ Value │ Via ▾ Notes";
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
        assert_eq!(&words(&lines[2])[..11], ["1", "☐", "☆", "·", "opus", "anthropic", "-", "│", "5.0", "5.0", "5.0"]);
        assert_eq!(&words(&lines[3])[..11], ["2", "☐", "☆", "·", "flash", "google", "-", "│", "0.10", "0.10", "0.10"]);
        assert!(lines[5].starts_with(" NORMAL  2 available"), "{}", lines[5]);
        assert!(
            lines[5].ends_with(
                "h l column  │  B benchmarks  H harness  / filter  s sort  │  enter details  x launch  o open  y copy name  space select  f fav  e exclude  n note  │  q quit"
            ),
            "{}",
            lines[5]
        );
        assert_eq!(buf[(cell(&lines[5], "│"), 5)].fg, MUTED, "groups are split by a muted rule");
        assert!((0..200).all(|x| buf[(x, 2)].bg == CURSOR), "row 0 is under the cursor");
        assert_eq!(buf[(cell(&lines[2], "anthropic"), 2)].fg, dev_color("anthropic"), "and keeps its colours");
        assert_eq!(buf[(0, 3)].fg, MUTED, "row numbers are muted");
        assert_eq!(buf[(cell(&lines[3], "flash"), 3)].fg, Color::Reset, "names are plain text");
        assert_eq!(buf[(cell(&lines[3], "☆"), 3)].fg, MUTED, "the empty ☆ is muted");
        // Where colours show, an off mark is its own glyph in grey, as the ☆.
        a.term_bg = Some(0);
        let (buf, lines) = render(&mut a, 200, 6);
        assert_eq!(&words(&lines[3])[..4], ["2", "✓", "☆", "✗"]);
        assert_eq!([buf[(cell(&lines[3], "✓"), 3)].fg, buf[(cell(&lines[3], "✗"), 3)].fg], [MUTED, MUTED]);
        a.term_bg = None;
        let (buf, lines) = render(&mut a, 200, 6);
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
        // A task shows its own scores, so its columns offer no dropdown while one is picked.
        a.col = ECI + 1;
        let (_, lines) = render(&mut a, 206, 6);
        assert!(lines[0].matches('▾').count() == 6 && lines[5].contains("d dropdown"), "{lines:?}");
        a.task = fit::task("agentic");
        let (_, lines) = render(&mut a, 206, 6);
        assert!(lines[0].matches('▾').count() == 3 && !lines[5].contains("d dropdown"), "{lines:?}");
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
        assert_eq!(&words(&lines[2])[5..9], ["▌", "✓", "any", "2"], "any is ticked while nothing is picked");
        assert_eq!(&words(&lines[3])[5..9], ["│", "☐", "anthropic", "1"]);
        assert_eq!(buf[(dev as u16 + 2, 2)].bg, CURSOR, "any is under the cursor");
        assert_eq!((buf[(dev as u16 + 2, 2)].fg, buf[(dev as u16 + 2, 3)].fg), (MARK, dev_color("anthropic")));
        assert_eq!(buf[(dev as u16, 2)].bg, CURSOR, "the cursor runs through the border");
        assert_ne!(buf[(dev as u16, 3)].bg, CURSOR);
        assert!(lines[7].starts_with(" PICK  Dev ▾"), "{}", lines[7]);
        let (_, short) = render(&mut a, 170, 6);
        assert!(short.iter().any(|l| l.contains(" 1-2 of 3 ")), "a list scrolled says where it is: {short:?}");
        for c in "/anth".chars() {
            a.key(KeyCode::Char(c).into());
        }
        let (_, lines) = render(&mut a, 170, 8);
        assert_eq!(&words(&lines[3])[5..9], ["▌", "☐", "anthropic", "1"], "only matches are listed");
        assert!(from(&lines[4]).starts_with("╰"), "{}", lines[4]);
        assert!(lines[7].starts_with(" PICK  Dev ▾ /anth"), "{}", lines[7]);
    }

    #[test]
    fn a_click_on_a_hint_presses_its_key() {
        use ratatui::crossterm::event::KeyModifiers;
        assert_eq!(split_hint("h l 0 $ model"), ("h l 0 $", " model"));
        assert_eq!(split_hint("enter best models first"), ("enter", " best models first"));
        assert_eq!(
            (hint_key("s sort"), hint_key("esc back"), hint_key("space select"), hint_key("j k scroll")),
            (Some(KeyCode::Char('s')), Some(KeyCode::Esc), Some(KeyCode::Char(' ')), None)
        );
        let mut a = app();
        // Too narrow for the help tab, so its hint is in the bar.
        let (w, h) = (100, 6);
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
    fn a_click_in_compare_and_recommend_is_on_the_model_under_it() {
        let mut a = app();
        let mut data = std::mem::take(&mut a.data);
        for (m, pct) in data.models.iter_mut().zip([90.0, 60.0]) {
            m.fit.insert("overall".into(), pct);
            // flash is 6 months behind opus: `low`, and no further.
            m.lag.insert("overall".into(), (100.0 - pct) / 5.0);
        }
        a.set_data(data);
        let (w, h) = (120, 32);
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h)).unwrap();
        // Where `pat` last shows on screen, as `draw` puts it.
        let mut at = |a: &mut App, pat: &str, mods: KeyModifiers| {
            term.draw(|f| draw(a, f)).unwrap();
            let lines = text(term.backend().buffer());
            let y = lines.iter().rposition(|l| l.contains(pat)).unwrap();
            let x = lines[y][..lines[y].find(pat).unwrap()].chars().count() as u16;
            let e =
                MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: x, row: y as u16, modifiers: mods };
            hit(a, Rect::new(0, 0, w, h), e)
        };
        "  j C".chars().skip(1).for_each(|c| _ = a.key(KeyCode::Char(c).into()));
        let none = KeyModifiers::NONE;
        // The model row, under the verdict, which names them too.
        assert_eq!(
            (at(&mut a, "opus", none), at(&mut a, "flash", none)),
            (Some(Mouse::Open(Stop::Compare(0))), Some(Mouse::Open(Stop::Compare(1))))
        );
        assert_eq!(at(&mut a, "verdict:", none), Some(Mouse::Outside), "above the model row no model");
        let click =
            |column, row| MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column, row, modifiers: none };
        let on = |x, y| hit(&a, Rect::new(0, 0, w, h), click(x, y));
        assert_eq!(on(0, 5), Some(Mouse::Close), "beside the panel: closes it");
        assert_eq!(on(w - 1, 0), Some(Mouse::Close), "past the tabs too");
        assert_eq!(on(w - 1, h - 1), Some(Mouse::Key(KeyCode::Char('q'))), "a hint is still its key");
        assert_eq!(on(10, 0), Some(Mouse::Tab(0)), "a tab is still that tab");
        assert_eq!(
            at(&mut a, "flash", KeyModifiers::CONTROL),
            Some(Mouse::Model(Stop::Compare(1))),
            "a ctrl click moves"
        );
        a.key(KeyCode::Esc.into());
        a.key(KeyCode::Char('R').into());
        // Overall's row: flash for low, then opus for mid and high.
        assert_eq!(
            (at(&mut a, "opus $", none), at(&mut a, "flash $", none)),
            (Some(Mouse::Open(Stop::Recommend(0, 2))), Some(Mouse::Open(Stop::Recommend(0, 1))))
        );
        assert_eq!(at(&mut a, "✓ selected", none), Some(Mouse::Open(Stop::Among(MARKED))), "a tab of the among line");
        let name = Some(Mouse::Open(Stop::Recommend(1, 0)));
        assert_eq!(at(&mut a, TASKS[1].name, none), name, "a task's name is its task");
        let empty = Some(Mouse::Open(Stop::Recommend(TASKS.len() - 1, 1)));
        assert_eq!(at(&mut a, " - ", none), empty, "and a box with no model is still its tier");
        a.mouse(Mouse::Model(Stop::Recommend(1, 0)));
        assert_eq!((a.task_cur, a.task_sel, a.current().is_none()), (1, 0, true), "a click moves to the task");
        a.mouse(Mouse::Model(Stop::Compare(2)));
        assert_eq!((a.task_cur, a.task_sel), (1, 0), "compare's stop is none of recommend's");
        a.mouse(Mouse::Model(Stop::Recommend(0, 2)));
        assert_eq!((a.task_cur, a.current().unwrap().key.as_str()), (0, "opus"), "a click moves to the model");
        let was = a.store.is_marked("flash");
        assert_eq!(a.mouse(Mouse::MarkModel(Stop::Recommend(0, 1))), Some(Effect::Save));
        assert_ne!(a.store.is_marked("flash"), was, "a right click toggles its selection, as space does");
        a.mouse(Mouse::Open(Stop::Recommend(0, 1)));
        assert!(
            matches!(a.view, View::Detail(Back::Recommend(_))) && a.current().unwrap().key == "flash",
            "a double click on a model opens its details"
        );
        a.key(KeyCode::Esc.into());
        assert_eq!((&a.view, a.task_sel), (&View::Recommend, 1), "esc goes back to it");
        a.mouse(Mouse::Open(Stop::Recommend(0, 0)));
        assert_eq!(a.view, View::Table, "a double click on a task ranks by it");
        a.store.toggle_favorite("debugging", "flash");
        "RG".chars().for_each(|c| _ = a.key(KeyCode::Char(c).into()));
        assert!(hints(&a, 200).contains(&"enter your model"), "on a task of your own, enter goes to its model");
    }

    #[test]
    fn a_cell_takes_a_double_click() {
        let (t, ms) = (Instant::now(), Duration::from_millis);
        let (cell, row) = (Mouse::Cell(3, PRICE), Mouse::Row(3));
        let mut last = None;
        assert_eq!(double(&mut last, cell, t), row, "one press highlights the row");
        assert_eq!(double(&mut last, cell, t + ms(200)), cell, "a second on the cell opens it");
        assert_eq!(double(&mut last, cell, t + ms(300)), row, "a third starts over");
        assert_eq!(double(&mut last, cell, t + ms(800)), row, "too slow: two clicks");
        assert_eq!(double(&mut last, Mouse::Harness(3, 0), t + ms(900)), row, "another cell: a click");
        assert_eq!(double(&mut last, Mouse::Extend(4), t + ms(950)), Mouse::Extend(4), "a drag is itself");
        let (open, model) = (Mouse::Open(Stop::Compare(1)), Mouse::Model(Stop::Compare(1)));
        assert_eq!(double(&mut last, open, t + ms(1000)), model, "a model: a click moves");
        assert_eq!(double(&mut last, open, t + ms(1100)), open, "and a double click opens");
        assert_eq!(double(&mut last, Mouse::Harness(3, 0), t + ms(999)), row, "and breaks the pair");
    }

    #[test]
    fn a_drag_selects_once_it_leaves_the_row_it_began_on() {
        let at = |kind, row| MouseEvent { kind, column: 9, row, modifiers: KeyModifiers::NONE };
        let (down, drag) = (MouseEventKind::Down(MouseButton::Left), MouseEventKind::Drag(MouseButton::Left));
        let (cell, to) = (Some(Mouse::Cell(1, PRICE)), |n| Some(Mouse::Extend(n)));
        let mut press = None;
        assert_eq!(dragged(&mut press, at(down, 4), cell), cell, "a press is itself");
        assert_eq!(dragged(&mut press, at(drag, 4), to(1)), None, "a slip along the row is no drag");
        assert_eq!(dragged(&mut press, at(drag, 5), to(2)), to(2), "off the row it extends");
        assert_eq!(dragged(&mut press, at(drag, 4), to(1)), to(1), "and back onto it too");
        assert_eq!(dragged(&mut press, at(down, 4), cell), cell);
        assert_eq!(
            dragged(&mut press, at(MouseEventKind::ScrollDown, 4), Some(Mouse::Scroll(3))),
            Some(Mouse::Scroll(3))
        );
        assert_eq!(dragged(&mut press, at(drag, 4), to(1)), None, "a new press starts over, whatever the wheel did");
        assert_eq!(dragged(&mut press, at(down, 1), Some(Mouse::Header(2))), Some(Mouse::Header(2)));
        assert_eq!(dragged(&mut press, at(drag, 5), to(2)), None, "a drag off a header selects nothing");
        assert_eq!(dragged(&mut press, at(down, 0), None), None);
        assert_eq!(dragged(&mut press, at(drag, 5), to(2)), None, "nor one off the frame");
    }

    #[test]
    fn a_click_on_the_key_prompts_link_opens_where_a_key_is_made() {
        let mut a = app();
        a.input = Input::Key { text: String::new(), cur: 0, wrong: false };
        let area = Rect::new(0, 0, 120, 12);
        let click = |x| MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: x,
            row: 11,
            modifiers: ratatui::crossterm::event::KeyModifiers::NONE,
        };
        // " KEY " and a space, then "API key from artificialanalysis.ai".
        assert_eq!(hit(&a, area, click(19)), Some(Mouse::Link));
        assert_ne!(hit(&a, area, click(18)), Some(Mouse::Link), "only on the link");
        assert_eq!(a.mouse(Mouse::Link), Some(Effect::Open(data::AA_KEY_URL.into())));
        assert!(matches!(a.input, Input::Key { .. }), "the prompt stays for the key");
    }

    #[test]
    fn clicks_land_on_rows_headers_and_dropdown_entries() {
        use ratatui::crossterm::event::KeyModifiers;
        let mut a = app();
        let (w, h) = (170, 12);
        let area = Rect::new(0, 0, w, h);
        // `draw` puts the tabs on rows 0 and 1 and under them the frame, which `click` counts
        // from: the header on its row 1, its rule on row 2 and the first model on row 3, one cell in.
        let (_, lines) = render(&mut a, w - 2, h - 5);
        // Screen column of a header, one cell in: `▾` takes several bytes.
        let col = |s: &str| lines[0][..lines[0].find(s).unwrap()].chars().count() as u16 + 1;
        let click = |x, y| MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: x,
            row: y + 2,
            modifiers: KeyModifiers::NONE,
        };
        // " │ yours A │ all a │ ✓ selected S │ ★ favorites F │ ✗ excluded E │ recommend R │ compare C │ theme t │ help ?  ": a tab, the edge left
        // of it too, and nothing right of the last one.
        assert_eq!(
            [10, 11, 19, 65, 79, 91, 101, 110].map(|x| hit(&a, area, MouseEvent { row: x % 2, ..click(x, 0) })),
            [Some(0), Some(1), Some(2), Some(5), Some(6), Some(7), Some(8), None].map(|i| i.map(Mouse::Tab))
        );
        // Cut off a narrow terminal, a tab is a hint in the status bar instead, help's the last.
        let cut = |w| hints(&a, w).into_iter().filter(|h| TABS.iter().any(|t| t.2 == *h)).collect::<Vec<_>>();
        assert_eq!((cut(111), cut(110), cut(102)), (vec![], vec!["? help"], vec!["? help"]));
        assert_eq!((cut(101), cut(79)), (vec!["t theme", "? help"], vec!["R recommend", "t theme", "? help"]));
        assert_eq!(hints(&a, 79).last(), Some(&"? help"));
        // Over the open theme list a click on a tab is that tab, and elsewhere outside the list.
        a.key(KeyCode::Char('t').into());
        assert_eq!(
            [10, 150].map(|x| hit(&a, area, MouseEvent { row: 1, ..click(x, 0) })),
            [Some(Mouse::Tab(0)), Some(Mouse::Outside)]
        );
        a.key(KeyCode::Esc.into());
        assert_eq!(hit(&a, area, click(12, 4)), Some(Mouse::Cell(1, 0)));
        assert_eq!(hit(&a, area, click(col("Dev"), 4)), Some(Mouse::Cell(1, 1)));
        assert_eq!(hit(&a, area, click(1, 4)), Some(Mouse::Row(1)), "the row number is no cell");
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
        assert_eq!(hit(&a, area, shift(3, 0)), None, "a shift click on the tabs is on no row");
        assert_eq!(hit(&a, area, drag(3, h)), Some(Mouse::Extend(h as usize - 8)), "below: the last row");
        assert_eq!(hit(&a, Rect::new(0, 0, w, 7), drag(3, 2)), None, "no rows to extend over");
        assert_eq!(hit(&a, area, click(1, 1)), Some(Mouse::Top), "the # header: the first row");
        assert_eq!([3, 5, 7].map(|x| hit(&a, area, click(x, 1))), [None; 3], "no header over the marks");
        assert_eq!(hit(&a, area, click(9, 1)), Some(Mouse::Header(0)));
        assert_eq!(hit(&a, area, click(3, 2)), None, "the rule under the header");
        assert_eq!(hit(&a, area, click(col("Dev"), 1)), Some(Mouse::Header(1)));
        assert_eq!(hit(&a, area, click(col("Dev") + 4, 1)), Some(Mouse::Menu(1)), "the ▾ after Dev");
        assert_eq!(hit(&a, area, click(col("Price"), 1)), Some(Mouse::Header(PRICE)));
        assert_eq!(hit(&a, area, click(col("Price") + 6, 1)), Some(Mouse::Menu(PRICE)), "the ▾ after Price");
        assert_eq!(hit(&a, area, click(col("Coding"), 1)), Some(Mouse::Header(9)));
        assert_eq!(hit(&a, area, click(0, 0)), None, "the frame");
        assert_eq!(hit(&a, area, click(3, h - 2)), None, "the status bar");
        let (k, via) = lines.iter().enumerate().find_map(|(k, l)| Some((k, l.find("opencode")?))).unwrap();
        let (x, y) = (lines[k][..via].chars().count() as u16 + 1, k as u16 + 1);
        assert_eq!(hit(&a, area, click(x + 7, y)), Some(Mouse::Harness(k - 2, 0)), "a harness in Via");
        assert_eq!(hit(&a, area, click(x + 8, y)), Some(Mouse::Row(k - 2)), "past its name: the row");
        assert_eq!(hit(&a, area, ctrl(x, y)), Some(Mouse::Pick(k - 2)), "ctrl click still picks");
        assert_eq!(hit(&a, area, click(col("Coding"), 4)), Some(Mouse::Cell(1, 9)), "a benchmark score");
        assert_eq!(hit(&a, area, click(col("Price"), 4)), Some(Mouse::Cell(1, PRICE)), "a price");
        assert_eq!(hit(&a, area, click(col("Notes"), 4)), Some(Mouse::Cell(1, NOTES)), "the note to write");
        let wheel = |kind| MouseEvent { kind, column: 0, row: 0, modifiers: KeyModifiers::NONE };
        assert_eq!(hit(&a, area, wheel(MouseEventKind::ScrollLeft)), Some(Mouse::Cols(-1)));
        let shifted = MouseEvent { modifiers: KeyModifiers::SHIFT, ..wheel(MouseEventKind::ScrollDown) };
        assert_eq!(hit(&a, area, shifted), Some(Mouse::Cols(1)), "shift+wheel goes sideways");
        a.mouse(Mouse::Menu(1));
        let (_, lines) = render(&mut a, w - 2, h - 4);
        let any = lines[2][..lines[2].find("any").unwrap()].chars().count() as u16 + 1;
        assert_eq!(hit(&a, area, click(any, 3)), Some(Mouse::Item(0)));
        assert_eq!(hit(&a, area, click(any, 4)), Some(Mouse::Item(1)));
        assert_eq!(hit(&a, area, click(any, 5)), Some(Mouse::Item(2)));
        assert_eq!(hit(&a, area, click(any, 6)), None, "the box's bottom border");
        assert_eq!(hit(&a, area, right(any, 4)), Some(Mouse::Item(1)), "either button");
        assert_eq!(hit(&a, area, drag(any, 4)), None, "a drag over a list does nothing");
        assert_eq!(hit(&a, area, right(w - 2, 3)), Some(Mouse::Outside), "any click outside closes it");
        assert_eq!(hit(&a, area, click(w - 2, 3)), Some(Mouse::Outside), "outside: closes it");
        // f's grid: a click on a box ticks it, on the name the task's own, on the heading nothing.
        a.mouse(Mouse::Outside);
        a.mouse(Mouse::Star(0));
        let (within, lines) = chooser(&a, area).unwrap();
        // `click` is two rows down, under the tabs.
        let rect = overlay_rect(within, "favorite for which tasks?", &lines);
        let rect = Rect { y: rect.y - 2, ..rect };
        let Input::Choose { items, .. } = &a.input else { panic!("f's grid is open") };
        let boxes = rect.x + 2 + fav_name_w(items) as u16 + 2;
        assert_eq!(hit(&a, area, click(boxes + 2 * BOX_W as u16 + 1, rect.y + 3)), Some(Mouse::Tick(1, 2)));
        assert_eq!(hit(&a, area, click(rect.x + 3, rect.y + 3)), Some(Mouse::Tick(1, 0)), "the name: the task's");
        assert_eq!(hit(&a, area, click(boxes + BOXES as u16 * BOX_W as u16, rect.y + 3)), None, "past the boxes");
        assert_eq!(hit(&a, area, click(boxes, rect.y + 1)), None, "the tiers' heading");
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
        a.key(KeyCode::Char('j').into()); // the cursor is on flash, off opus
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
            // Off flash, as the other checks are off the cursor.
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
        assert_eq!(buf[(x + 1, flash as u16)].fg, task_color("agentic"), "and the gap, for a ★ wider than its cell");
        // The recommend panel: the task name and the favorite's ★ in the task's colour, the model in its price level's.
        let mut data = std::mem::take(&mut a.data);
        for (m, pct) in data.models.iter_mut().zip([90.0, 60.0]) {
            m.fit.insert("coding".into(), pct);
            // flash is 6 months behind opus: `low`, and no further.
            m.lag.insert("coding".into(), (100.0 - pct) / 5.0);
        }
        a.set_data(data);
        let lines = recommend(&a, 200, &mut vec![]).0;
        let spans: Vec<&Span> = lines.iter().flat_map(|l| l.spans.iter()).collect();
        let name = spans.iter().find(|s| s.content == "coding").unwrap();
        assert_eq!(name.style.fg, Some(task_color("coding")));
        let star = spans.iter().position(|s| s.content == "★ " && s.style.fg == Some(task_color("coding"))).unwrap();
        assert!(spans[star + 1].content.starts_with("opus "), "★ then the name: {:?}", spans[star + 1]);
        assert_eq!(spans[star + 1].style.fg, Some(LEVEL[level(5.0)]), "the name keeps its price level");
        // A favorite off the frontier, flash under the low tier's floor, is grey and says so.
        let mut data = std::mem::take(&mut a.data);
        data.models.iter_mut().find(|m| m.key == "flash").unwrap().lag.insert("coding".into(), 12.0);
        a.store.toggle_favorite("coding", "flash");
        a.set_data(data);
        // Under the grid, the box under the cursor says why.
        (a.view, a.among, a.task_cur, a.task_sel) = (View::Recommend, None, 1, 1);
        let lines = recommend(&a, 200, &mut vec![]).0;
        let spans: Vec<&Span> = lines.iter().flat_map(|l| l.spans.iter()).collect();
        let star = spans.iter().rposition(|s| s.content == "★ " && s.style.fg == Some(task_color("coding"))).unwrap();
        assert!(spans[star + 1].content.starts_with("flash "));
        assert_eq!(spans[star + 1].style.fg, Some(MUTED));
        assert_eq!(spans[star + 2].content, " not recommended");
    }

    #[test]
    fn narrow_table_drops_columns_and_hints_without_panicking() {
        let mut a = app();
        let (_, lines) = render(&mut a, 46, 4);
        assert_eq!(words(&lines[0]), ["#", "Model", "Dev", "▾", "‹│", "▼ECI"], "the cursor starts on the index");
        assert!(layout(46, &a).more, "columns cut off on the right");
        assert!(!layout(400, &a).more, "all columns fit");
        assert_eq!(layout(400, &a).first, 0, "a window made wide again shows the columns scrolled off on the left");
        for w in 46..400 {
            let l = layout(w, &a);
            assert!(l.name_w == NAME_MIN || !l.more, "names are cut only so every column fits: {w}");
        }
        let l = layout(400, &a);
        assert_eq!(l.cols[0].1, l.name_x + l.name_w + GAP + l.dev_w + GAP, "Released a gap after Dev, as in a group");
        // Rows above or below the window are reported for the frame's ▲ ▼.
        for w in 46..400 {
            let l = layout(w, &a);
            assert!(l.name_w == NAME_MIN || !l.more, "names are cut only so every column fits: {w}");
        }
        let l = layout(400, &a);
        assert_eq!(l.cols[0].1, l.name_x + l.name_w + GAP + l.dev_w + GAP, "Released a gap after Dev, as in a group");
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
        assert!(!lines[3].contains("space select"), "hints that do not fit are dropped whole");
        // Moving past the right edge scrolls the columns right of Dev; Model and Dev stay.
        a.col = NCOLS - 1;
        let (_, lines) = render(&mut a, 46, 4);
        assert_eq!(words(&lines[0]), ["#", "Model", "Dev", "▾", "‹│", "Notes"]);
        // A column wider than the room right of Dev is cut, not dropped.
        a.store.set_note("opus", &"x".repeat(40));
        for w in 44..80 {
            let l = layout(w, &a);
            assert!(l.notes.is_some_and(|(x, nw)| nw > 0 && x + nw <= w), "Notes under the cursor shows: {w}");
        }
        a.store.set_note("opus", "");
        a.col = VIA;
        let (_, lines) = render(&mut a, 47, 4);
        assert_eq!(words(&lines[0]), ["#", "Model", "Dev", "▾", "‹│", "Via", "▾"]);
        for _ in 0..2 {
            a.key(KeyCode::Char('h').into());
        }
        let (_, lines) = render(&mut a, 56, 4);
        assert_eq!(words(&lines[0])[4..], ["‹│", "Reason", "▾", "Value"], "scrolls back only as far as needed");
        a.key(KeyCode::Char('l').into());
        let (_, lines) = render(&mut a, 56, 4);
        assert_eq!(words(&lines[0])[4..], ["‹│", "Reason", "▾", "Value"], "{}", lines[0]);
        a.col = 0;
        let (_, lines) = render(&mut a, 48, 4);
        assert_eq!(words(&lines[0]), ["#", "Model", "Dev", "▾", "Released"], "no │ in Dev's group");
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
    fn a_list_price_is_marked_and_muted() {
        let mut a = app();
        let mut data = std::mem::take(&mut a.data);
        // Yours lists no price; another provider's does.
        let yours = Offer { provider: "g".into(), unpriced: true, available: true, ..Default::default() };
        data.models[1].offers.insert(0, yours);
        data.models[1].fit.insert("value".into(), 50.0);
        a.set_data(data);
        let (buf, lines) = render(&mut a, 200, 6);
        let y = lines.iter().position(|l| l.contains("flash")).unwrap();
        assert_eq!(lines[y].matches("~0.10").count(), 4, "Price, $in, $cache and $out: {}", lines[y]);
        assert_eq!(lines[y].matches('~').count(), 5, "and Value, which divides by it: {}", lines[y]);
        let x = lines[y].chars().position(|c| c == '~').unwrap() as u16;
        let cell = &buf[(x, y as u16)];
        assert!(cell.modifier.contains(Modifier::ITALIC) && cell.fg == MUTED, "as not available");
        assert!(!lines.iter().any(|l| l.contains("opus") && l.contains('~')), "a price of yours is not");
    }

    #[test]
    fn scrolls_to_keep_the_selection_visible() {
        let mut a = app();
        let mut data = std::mem::take(&mut a.data);
        data.models.extend((0..20).map(|i| model(&format!("m{i}"), "x", Some(i as f64), 1.0)));
        a.set_data(data);
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
        a.input = Input::Bound { col: PRICE + 1, min: true, text: "4".into(), cur: 1 };
        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 1));
        status(&mut buf, Rect::new(0, 0, 40, 1), &a);
        assert_eq!((0..15).map(|x| buf[(x, 0)].symbol()).collect::<String>(), " BOUND  $in ≥ 4");
        a.input = Input::Bound { col: ECI - 1, min: true, text: "200".into(), cur: 3 };
        let mut buf = Buffer::empty(Rect::new(0, 0, 60, 1));
        assert_eq!(status(&mut buf, Rect::new(0, 0, 60, 1), &a), Some(8 + 6 + 3), "the k is after the cursor");
        assert_eq!((0..18).map(|x| buf[(x, 0)].symbol()).collect::<String>(), " BOUND  Ctx ≥ 200k", "in thousands");
    }

    #[test]
    fn a_launch_refuses_an_id_windows_terminal_would_split() {
        let cmd = ["opencode", "--model", "p/x;new-tab"].map(String::from);
        let e = new_terminal(&cmd).unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::InvalidInput, "refused before anything is spawned: {e}");
    }

    #[test]
    fn compare_colours_every_tied_cell() {
        let mk = |n: &str, context| Model { name: n.into(), context, ..Default::default() };
        let (a, b, c) = (mk("a", 1_000_000), mk("b", 200_000), mk("c", 200_000));
        let lines = compare(&[&a, &b, &c], (false, &|_| None), (0, 0), 200, "", |_| false, &mut vec![]).0;
        let ctx = lines.iter().find(|l| l.to_string().starts_with("context")).unwrap();
        let fgs: Vec<_> =
            ctx.spans.iter().filter(|s| s.content.trim().ends_with(['M', 'k'])).map(|s| s.style.fg).collect();
        assert_eq!(fgs, [Some(GOOD), Some(BAD), Some(BAD)], "{ctx:?}");
    }

    #[test]
    fn compare_scrolls_models_sideways() {
        let a = app();
        let ms: Vec<&Model> =
            ["opus", "flash"].iter().map(|k| a.data.models.iter().find(|m| m.key == *k).unwrap()).collect();
        let row = |v: &Vec<Line>| v.iter().find(|l| l.to_string().starts_with("model ")).unwrap().to_string();
        let (full, first) = compare(&ms, (false, &|_| None), (1, 1), 200, "", |_| false, &mut vec![]);
        assert!(row(&full).contains("opus") && row(&full).contains("flash"));
        assert_eq!(first, 0, "everything fits, so nothing scrolls off");
        assert!(!row(&full).contains('‹') && !row(&full).contains('›'), "no scroll marks when all fit");
        let mut spots = vec![];
        let (cut, first) = compare(&ms, (false, &|_| None), (1, 0), 20, "", |_| false, &mut spots);
        let edge = row(&cut).chars().position(|c| c == '‹').unwrap();
        assert_eq!(spots[0].x.start, edge + 1, "a click on ‹ is on no model");
        assert!(!row(&cut).contains("opus") && row(&cut).contains("flash"), "scrolls to show the selection");
        assert_eq!(first, 1);
        assert!(cut.iter().any(|l| l.to_string().starts_with("models 2-2 of 2")));
        assert!(row(&cut).contains('‹') && !row(&cut).contains('›'), "‹ marks models off to the left");
        let (past, first) = compare(&ms, (false, &|_| None), (7, 0), 20, "", |_| false, &mut vec![]);
        assert!(row(&past).contains("flash") && first == 1, "a cursor past the models lands on the last");
        let mut spots = vec![];
        let (back, first) = compare(&ms, (false, &|_| None), (0, 1), 20, "", |_| false, &mut spots);
        assert_eq!(spots[0].x.end, row(&back).chars().count() - 1, "nor on ›");
        assert!(row(&back).contains("opus") && !row(&back).contains("flash"));
        assert_eq!(first, 0);
        assert!(row(&back).ends_with("opus▐›") && !row(&back).contains('‹'), "› marks models off to the right");
        assert!(row(&back).contains('▌'), "the cursor's left bar, before the model's column");
        // The model under the cursor keeps its colours on the fill, between the two bars.
        let eci = back.iter().find(|l| l.to_string().starts_with("ECI")).unwrap();
        let cur = eci.spans.iter().find(|s| s.style.bg == Some(CURSOR) && s.content.contains("150")).unwrap();
        assert_eq!(cur.style.fg, Some(GOOD), "{eci:?}");
        let labels = |v: &Vec<Line>| {
            v.iter()
                .map(Line::to_string)
                .filter(|l| l.contains("  ") || l.contains('▌'))
                .map(|l| l.split("  ").next().unwrap().split('▌').next().unwrap().trim().to_string())
                .collect::<Vec<_>>()
        };
        let rule = full.iter().position(|l| l.to_string().starts_with("model ")).unwrap() + 1;
        assert!(full[rule].to_string().chars().all(|c| c == '─'), "a rule under the model row, as in the table");
        let topics: Vec<String> = full.iter().map(Line::to_string).filter(|l| l.starts_with("── ")).collect();
        assert!(topics[0].starts_with("── scores ─"), "{topics:?}");
        assert!(topics.iter().all(|t| t.chars().count() == full[rule].width()), "topic rules span the model row");
        let (some, _) = compare(&ms, (false, &|_| None), (0, 0), 200, "eci", |_| false, &mut vec![]);
        assert_eq!(
            labels(&some).iter().filter(|l| !l.is_empty()).collect::<Vec<_>>(),
            ["model", "ECI"],
            "the model row stays as the header"
        );
        let names = |v: &Vec<Line>| v.iter().filter(|l| l.to_string().starts_with("── ")).count();
        assert_eq!(names(&some), 1, "only the topics with a shown row keep their rule");
        let (typo, _) = compare(&ms, (false, &|_| None), (0, 0), 200, "contxt", |_| false, &mut vec![]);
        assert!(labels(&typo).contains(&"context".to_string()), "a typo is forgiven when nothing matches");
    }

    /// One check per defect a review found in the drawing.
    #[test]
    fn review_fixes_hold() {
        assert_eq!(list_top(0, 5, 3), 3, "going down, the cursor is on the last line shown");
        assert_eq!(list_top(3, 4, 3), 3, "back up inside the window, the list keeps still, as the table does");
        assert_eq!(list_top(3, 2, 3), 2, "and past it, the cursor is on the first line");
        let mut a = app();
        // The table in the left 30 columns of a wider buffer, as inside its frame.
        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 6));
        table(&mut buf, Rect::new(0, 0, 30, 6), &mut a);
        let past: String = (0..6).flat_map(|y| (30..40).map(move |x| (x, y))).map(|at| buf[at].symbol()).collect();
        assert_eq!(past.trim(), "", "Model and Dev stop at the frame");
        a.col = 1;
        a.key(KeyCode::Char('d').into());
        let (_, lines) = render(&mut a, 50, 7);
        let bar = lines.last().unwrap();
        assert!(bar.contains("PICK") && bar.contains("Dev ▾"), "the prompt wins over its key hint: {bar}");
        assert_eq!(scrolled(4, "abcdefgh", 8, 8), 5, "text past its room starts where the cursor still shows");
        assert_eq!(scrolled(4, "abc", 3, 8), 0, "and text that fits, at its start");
        // Recommend's legend and its last lines have no cursor: they show with the first and
        // the last task.
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 16)).unwrap();
        let mut a = app();
        let mut screen = |a: &mut App, keys: &str| {
            keys.chars().for_each(|c| drop(a.key(KeyCode::Char(c).into())));
            term.draw(|f| draw(a, f)).unwrap();
            let buf = term.backend().buffer();
            (0..16).map(|y| (0..80).map(|x| buf[(x, y)].symbol()).collect::<String>()).collect::<Vec<_>>().join("\n")
        };
        let last = screen(&mut a, "RG");
        assert!(last.contains("CLI: modelcmp recommend"), "{last}");
        let first = screen(&mut a, "gg");
        assert!(first.contains("each tier's model:"), "{first}");
        // The lines under the grid, which say what the cursor is on, stay under the rows that scroll.
        let top = screen(&mut a, "jl");
        assert!(top.contains("overall") && top.contains("  low:     no data"), "{top}");
        let last = screen(&mut a, "G");
        assert!(
            last.contains("vision") && last.contains("  use for: screenshots") && !last.contains("among:"),
            "{last}"
        );
        // f's hints fit a terminal 80 columns wide, with a task of your own too.
        let items = vec![("debugging".to_string(), Effect::Fav("k".into(), "debugging".into()))];
        let hint = fav_lines(&a, &items, &List::default(), 80).pop().unwrap();
        assert!(hint.width() <= 76 && hint.to_string().contains("r rename"), "{hint}");
    }

    #[test]
    fn frame_counts_the_steps_of_a_refresh() {
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 8)).unwrap();
        let mut a = app();
        a.refreshing = true;
        let mut border = |a: &mut App| {
            term.draw(|f| draw(a, f)).unwrap();
            let buf = term.backend().buffer();
            (0..120).map(|x| buf[(x, 6)].symbol()).collect::<String>()
        };
        assert!(border(&mut a).contains("· ⟳ refreshing ╯"), "before the first count");
        a.progress = "7/9, waiting for opencode".into();
        assert!(border(&mut a).contains("· ⟳ refreshing 7/9, waiting for opencode ╯"));
        // Too long for the border with the version in it, the count goes alone.
        a.progress = format!("7/9, waiting for {}", "artificialanalysis.ai, epoch.ai, and a very long name of a site");
        let line = border(&mut a);
        assert!(line.contains(concat!("╰ modelcmp v", env!("CARGO_PKG_VERSION"), " ─")), "{line}");
        assert!(line.contains("· ⟳ refreshing 7/9 ╯"), "{line}");
    }

    #[test]
    fn tabs_leave_the_sort_and_the_column_s_description_in_view() {
        let mut a = app();
        let key = a.data.models[0].key.clone();
        a.store.toggle_excluded(&key);
        a.store.toggle_marked(&a.data.models[1].key.clone());
        a.rebuild();
        "SE".chars().for_each(|c| _ = a.key(KeyCode::Char(c).into()));
        let rows = |a: &mut App, w: u16| {
            let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, 20)).unwrap();
            term.draw(|f| draw(a, f)).unwrap();
            let buf = term.backend().buffer();
            (0..20).map(|y| (0..w).map(|x| buf[(x, y)].symbol()).collect::<String>()).collect::<Vec<_>>()
        };
        // ✗'s tab, on, opens the border left of the sort at 70 columns and is over the sort at 60.
        for (w, left) in [
            (60, "╭─────────── ECI: Epoch AI's overall capability…─"),
            (70, "╭─── ECI: Epoch AI's overall capability index ────╯"),
        ] {
            let lines = rows(&mut a, w);
            assert!(lines[2].starts_with(left) && lines[2].ends_with(" ▼ by ECI ╮"), "{}", lines[2]);
            assert!(lines[19].contains("✗ excluded only"), "E left S: {}", lines[19]);
        }
        // ✓'s, further left, leaves the wider side on its right.
        a.key(KeyCode::Char('S').into());
        let top = &rows(&mut a, 100)[2];
        assert!(
            top.contains("╯              ╰────── ECI: Epoch AI's overall capability index ──────"),
            "right of it: {top}"
        );
        // A panel's tab stands on the frame, closed, and a cut one is a hint there too.
        a.key(KeyCode::Char('C').into());
        let top = &rows(&mut a, 120)[2];
        assert!(top.contains("┴───────────┴") && !top.contains('╯'), "{top}");
        let cut = hints(&a, 20);
        assert!(cut.contains(&"t theme") && !cut.contains(&"a all"), "a's key does nothing in a panel: {cut:?}");
    }

    #[test]
    fn theme_list_scrolls_to_the_cursor() {
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(60, 9)).unwrap();
        let mut a = app();
        a.key(KeyCode::Char('t').into());
        // Mid-list the cursor is on the last row shown, where the ▼ is: it shows in place of the
        // left bar, on the cursor's fill, as in the table.
        for _ in 0..8 {
            a.key(KeyCode::Char('j').into());
        }
        term.draw(|f| draw(&mut a, f)).unwrap();
        let buf = term.backend().buffer();
        let text: String = (0..9).flat_map(|y| (0..60).map(move |x| buf[(x, y)].symbol())).collect();
        assert!(text.contains(&format!(" of {} ", THEMES.len())), "the hint is not one of the themes counted: {text}");
        let below = buf.content.iter().find(|c| c.symbol() == "▼" && c.bg == CURSOR);
        assert_eq!(below.map(|c| c.fg), Some(ACCENT), "themes below the cursor");
        let tabs: String = (0..2).flat_map(|y| (0..60).map(move |x| buf[(x, y)].symbol())).collect();
        assert!(tabs.contains("yours") && !tabs.contains('╮'), "a list too tall leaves the tabs in view: {tabs}");
        for _ in 9..THEMES.len() {
            a.key(KeyCode::Char('j').into());
        }
        term.draw(|f| draw(&mut a, f)).unwrap();
        let buf = term.backend().buffer();
        let text: String = (0..9).flat_map(|y| (0..60).map(move |x| buf[(x, y)].symbol())).collect();
        assert!(text.contains(THEMES[THEMES.len() - 1].0) && text.contains('▲'), "{text}");
        assert!(text.contains("enter saves") && !text.contains('▼'), "at the last theme, the hint below it: {text}");
        // With one row to show, it is the cursor's and not the hint, so the cursor is off the border.
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(60, 4)).unwrap();
        term.draw(|f| draw(&mut a, f)).unwrap();
        let buf = term.backend().buffer();
        let row = |y: u16| (0..60).map(|x| buf[(x, y)].symbol()).collect::<String>();
        assert!(row(0).contains('╭') && row(0).contains('╮'), "{}", row(0));
        assert!(row(1).contains(THEMES[THEMES.len() - 1].0) && row(1).contains('▐'), "{}", row(1));
    }

    #[test]
    fn a_long_prompt_scrolls_and_keeps_its_cursor_off_the_hint() {
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(60, 8)).unwrap();
        let mut a = app();
        a.key(KeyCode::Char('n').into());
        for c in "🔥 a note far longer than the room it has".chars() {
            a.key(KeyCode::Char(c).into());
        }
        term.draw(|f| draw(&mut a, f)).unwrap();
        let hint = 60 - "enter apply  esc cancel".len() as u16 - 1;
        let pos = term.get_cursor_position().unwrap();
        let buf = term.backend().buffer();
        let line: String = (0..60).map(|x| buf[(x, pos.y)].symbol()).collect();
        assert!(pos.x < hint && line.contains("room it has"), "cursor at {} on {line:?}", pos.x);
    }

    #[test]
    fn cursor_runs_through_the_frame() {
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 10)).unwrap();
        let mut a = app();
        let on = |term: &ratatui::Terminal<ratatui::backend::TestBackend>, x: u16, y: u16| {
            term.backend().buffer()[(x, y)].bg == CURSOR
        };
        term.draw(|f| draw(&mut a, f)).unwrap();
        assert_eq!([on(&term, 0, 5), on(&term, 50, 5), on(&term, 99, 5), on(&term, 0, 6)], [true, true, true, false]);
        let buf = term.backend().buffer();
        assert_eq!((buf[(0, 5)].symbol(), buf[(99, 5)].symbol()), ("▌", "▐"), "the border is its two bars");
        assert_eq!((buf[(0, 5)].fg, buf[(0, 6)].symbol()), (ACCENT, "│"));
        a.key(KeyCode::Char('v').into());
        a.key(KeyCode::Char('j').into());
        term.draw(|f| draw(&mut a, f)).unwrap();
        assert_eq!([on(&term, 0, 5), on(&term, 99, 6)], [true, true], "the visual range too");
        a.key(KeyCode::Esc.into());
        a.key(KeyCode::Char('t').into());
        term.draw(|f| draw(&mut a, f)).unwrap();
        let buf = term.backend().buffer();
        // The overlay's top-left corner, under the tabs and right of the frame's; the cursor is
        // on the row under it, the first entry.
        let (x, y) = (2..10).flat_map(|y| (2..100).map(move |x| (x, y))).find(|&p| buf[p].symbol() == "╭").unwrap();
        assert!(on(&term, x, y + 1) && buf[(x, y + 1)].symbol() == "▌", "and the choice list's");
        // With the terminal's own colours and its background unknown, a reverse-video bar with
        // colours off it; a pill in the accent stays one.
        let mut buf = buf.clone();
        buf[(x + 1, y)].set_fg(Color::Black).set_bg(ACCENT);
        recolor(&mut buf, None, None);
        let (end, text) = (&buf[(x, y + 1)], &buf[(x + 2, y + 1)]);
        assert_eq!((end.symbol(), end.fg, end.bg), (" ", Color::Reset, Color::Reset));
        assert!(end.modifier.contains(Modifier::REVERSED) && text.modifier.contains(Modifier::REVERSED));
        assert_eq!((text.fg, text.bg, buf[(x + 1, y)].bg), (Color::Reset, Color::Reset, ACCENT));
    }

    #[test]
    fn every_view_draws_at_any_size() {
        // Each with the solid fill and the faint one.
        for (w, h, term_bg) in [(120, 30), (60, 10), (20, 5), (4, 4), (3, 3)]
            .into_iter()
            .flat_map(|(w, h)| [(w, h, None), (w, h, Some(0))])
        {
            let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h)).unwrap();
            for view in [View::Table, View::Help, View::Detail(Back::Table), View::Compare, View::Recommend] {
                let mut a = app();
                a.store.marked = vec!["opus".into(), "flash".into()];
                (a.view, a.term_bg) = (view, term_bg);
                a.task_cur = 3;
                term.draw(|f| draw(&mut a, f)).unwrap();
                // What `recolor` and `vmarks` tell a cell by: a colour behind text that is not
                // black is the cursor's fill or a selected row's, and nothing else.
                let odd = term
                    .backend()
                    .buffer()
                    .content
                    .iter()
                    .find(|c| c.fg != Color::Black && ![Color::Reset, CURSOR, MARK].contains(&c.bg));
                assert_eq!(odd, None, "{:?} at {w}x{h}", a.view);
            }
        }
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(101, 10)).unwrap();
        let mut a = app();
        a.col = ECI;
        term.draw(|f| draw(&mut a, f)).unwrap();
        let row = |term: &ratatui::Terminal<ratatui::backend::TestBackend>, y: u16| {
            (0..101).map(|x| term.backend().buffer()[(x, y)].symbol()).collect::<String>()
        };
        // The tab that is on has a frame of its own, which runs down into the table's, open under it.
        assert_eq!(row(&term, 0).trim_end(), " ╭─────────╮");
        let tabs = row(&term, 1);
        assert!(
            tabs.starts_with(
                " │ yours A │ all a │ ✓ selected S │ ★ favorites F │ ✗ excluded E │ recommend R │ compare C │ theme t"
            ),
            "{tabs}"
        );
        let top = row(&term, 2);
        assert!(top.ends_with("─ ▼ by ECI ╮"), "{top}");
        assert!(top.starts_with("╭╯         ╰──────────"), "{top}");
        let (l, r) = top.split_once(" ECI: Epoch AI's overall capability index ").unwrap();
        assert!(l.chars().count().abs_diff(r.chars().count()) <= 1, "centred: {top}");
        // Recommend's tab is the one on over its panel.
        a.view = View::Recommend;
        term.draw(|f| draw(&mut a, f)).unwrap();
        let on = (0..101).filter(|&x| {
            term.backend().buffer()[(x, 1)].modifier.contains(BOLD)
                && row(&term, 1).chars().nth(x as usize) == Some('r')
        });
        assert_eq!(on.count(), 1, "{}", row(&term, 1));
        a.view = View::Table;
        term.draw(|f| draw(&mut a, f)).unwrap();
        let bottom: String = (0..101).map(|x| term.backend().buffer()[(x, 8)].symbol()).collect();
        assert!(bottom.contains("models.dev + Epoch AI"), "{bottom}");
        assert!(bottom.starts_with(concat!("╰ modelcmp v", env!("CARGO_PKG_VERSION"), " ─")), "{bottom}");
        assert_eq!(a.page, 3, "10 lines minus the tabs, status bar, two borders, the header and its rule");
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
        let marks = [edge(&term, 100, 3), edge(&term, 0, 4), edge(&term, 100, 4), edge(&term, 0, 5), edge(&term, 0, 7)];
        assert_eq!(marks, ["›", "├", "┤", "▌", "▼"], "the rule joins the frame, the cursor's bar is on it");
        a.key(KeyCode::Char('G').into());
        term.draw(|f| draw(&mut a, f)).unwrap();
        assert_eq!([edge(&term, 0, 5), edge(&term, 0, 7)], ["▲", "▌"]);
        assert_eq!(term.backend().buffer()[(0, 5)].fg, ACCENT);
        // On a marked row's fill the ▲ is black as the row, where the accent would not read.
        a.store.marked = a.data.models.iter().map(|m| m.key.clone()).collect();
        term.draw(|f| draw(&mut a, f)).unwrap();
        let top = &term.backend().buffer()[(0, 5)];
        assert_eq!((top.symbol(), top.fg, top.bg), ("▲", Color::Black, MARK));
        a.store.marked.clear();
        // A tall overlay stops above the status bar, which shows its keys.
        a.key(KeyCode::Char('?').into());
        term.draw(|f| draw(&mut a, f)).unwrap();
        let bar: String = (0..100).map(|x| edge(&term, x, 9)).collect();
        assert!(bar.starts_with(" HELP "), "{bar}");
    }

    #[test]
    fn overlays_fit_and_scroll() {
        let area = Rect::new(0, 0, 30, 6);
        let mut buf = Buffer::empty(area);
        let mut scroll = 99;
        overlay(&mut buf, area, "keys", help(""), &mut scroll, Color::Reset, (0, 0, 0..0));
        assert_eq!(buf[(0, 0)].symbol(), "╭");
        assert_eq!(buf[(0, 0)].fg, Color::Reset, "a box over the table has the text's colour");
        assert_eq!(scroll as usize, help("").len() - 4, "scroll is clamped to the content");
        assert_eq!((buf[(0, 1)].symbol(), buf[(0, 4)].symbol()), ("▲", "│"), "at the end: lines above only");
        scroll = 0;
        overlay(&mut buf, area, "keys", help(""), &mut scroll, Color::Reset, (0, 0, 0..0));
        assert_eq!((buf[(0, 1)].symbol(), buf[(0, 4)].symbol()), ("│", "▼"), "at the top: lines below only");
        // A grid's heading is no row: the border counts the rows under it, 20 here in a box of 4 lines.
        let grid = |scroll: &mut u16| {
            let mut buf = Buffer::empty(area);
            let lines = (0..22).map(|i| Line::from(format!("{i:<20}"))).collect();
            overlay(&mut buf, area, "f", lines, scroll, Color::Reset, (1, 1, 0..0));
            (0..30).map(|x| buf[(x, 5)].symbol()).collect::<String>()
        };
        assert!(grid(&mut 0).contains(" 1-3 of 20 "), "{}", grid(&mut 0));
        assert!(grid(&mut 5).contains(" 5-8 of 20 "), "{}", grid(&mut 5));
        let a = app();
        let text: Vec<String> = detail(&a.data.models[0], &a.store, a.any_available(), (None, None))
            .1
            .iter()
            .map(ToString::to_string)
            .collect();
        assert!(text.iter().any(|l| l.starts_with("  developer:  anthropic")), "{text:?}");
        let mut m = a.data.models[0].clone();
        m.fit.insert("coding".into(), 50.0);
        let lines = detail(&m, &a.store, a.any_available(), (None, None)).1;
        let coding = lines.iter().flat_map(|l| &l.spans).find(|s| s.content == "coding").expect("a coding fit line");
        assert_eq!(coding.style.fg, Some(task_color("coding")), "a task's name is in its colour");
        let rows =
            compare(&a.marked_models(), (a.any_available(), &|_| None), (0, 0), 200, "", |_| false, &mut vec![]).0;
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

    /// An entry of a list has a colour where the table has one for it: a harness, and not a
    /// site, a theme or a source, whose names would give one that says nothing.
    #[test]
    fn a_list_colours_what_the_table_does() {
        let fgs = |kind, label: &str, effect| {
            let lines = choice_lines(kind, &[(label.to_string(), effect)], &List::default(), false);
            lines[0].spans.iter().map(|s| lines[0].style.patch(s.style).fg).collect::<Vec<_>>()
        };
        assert_eq!(fgs(Kind::Theme, "nord", Effect::Theme("nord")), [Some(Color::Reset)]);
        assert_eq!(fgs(Kind::Open, "epoch.ai", Effect::Open(String::new())), [Some(Color::Reset)]);
        let pi = fgs(Kind::Launch, "pi --model x", Effect::Launch(vec![]));
        assert_eq!(pi, [Some(dev_color("pi")), Some(MUTED)], "the harness as in Via, its command muted");
        let src = data::Source::Epoch;
        let label = format!("{:<20} {}", src.label(), src.about());
        assert_eq!(fgs(Kind::Source, &label, Effect::Source(src)), [None, Some(MUTED)], "what it takes muted");
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
        let search = |q: &str| List { query: q.into(), typing: true, ..Default::default() };
        let lines = choice_lines(Kind::Theme, &items, &search("uv"), false);
        assert_eq!(lit_text(&lines), ["uv"]);
        // Under the cursor too a hit is yellow, as the cursor keeps colours.
        let lines = choice_lines(Kind::Theme, &items, &search("gr"), false);
        let hit = lines[0].spans.iter().find(|s| s.content == "gr").unwrap();
        assert_eq!(hit.style.fg, Some(MATCH));
        // f's grid marks what it searches, the task's name.
        let a = app();
        let items = vec![("coding".to_string(), Effect::Fav("k".into(), "coding".into()))];
        assert_eq!(lit_text(&fav_lines(&a, &items, &search("od"), 80)), ["od"]);
        // A wide name takes the cells it shows in, so its boxes stay under the heading.
        let wide = vec![
            ("調整".to_string(), Effect::Fav("k".into(), "調整".into())),
            ("debugging".to_string(), Effect::Fav("k".into(), "debugging".into())),
        ];
        let rows = fav_lines(&a, &wide, &List::default(), 80);
        assert_eq!(rows[1].width(), rows[2].width());
        // A row being written shows the text and how to leave; a name not taken, why.
        let text = |what: What, text: &str, err: Option<&str>| {
            let edit = Some(Edit { what, text: text.into(), cur: 0, err: err.map(String::from) });
            let lines = fav_lines(&a, &items, &List { edit, ..Default::default() }, 80);
            [lines[1].to_string(), lines.last().unwrap().to_string()]
        };
        let wide = " j k h l move · / search · space enter toggle · v via · esc close".chars().count();
        assert_eq!(
            text(What::Rename("x".into()), "y", None),
            [" y ".to_string(), format!("{:<wide$}", " enter apply · esc cancel")],
            "as wide as the hint it stands for"
        );
        assert_eq!(
            text(What::About("x".into()), "", Some("no")),
            [" x  what it is about ".to_string(), format!("{:<wide$}", " no")]
        );
        let new = Edit { what: What::New, text: String::new(), cur: 0, err: None };
        assert_eq!(edit_prefix(&new), " + ");
    }

    #[test]
    fn recommend_panel_is_a_grid_of_tasks_and_tiers() {
        let mut a = app();
        (a.view, a.task_cur) = (View::Recommend, TASKS.iter().position(|t| t.name == "vision").unwrap());
        let lines = recommend(&a, 60, &mut vec![]).0;
        let text: Vec<String> = lines.iter().map(ToString::to_string).collect();
        assert_eq!(
            text[0], "among: ● yours · ○ all · ○ ✓ selected · ○ ★ favorites ",
            "the models it ranks come first, the dot on the ones in use"
        );
        let gap = text.iter().position(String::is_empty).unwrap();
        assert!(gap > 2 && text[1..gap].join(" ") == TIER_LEGEND, "the legend wraps: {:?}", &text[..gap]);
        let head = gap + 1;
        assert_eq!(text[head].split_whitespace().collect::<Vec<_>>(), ["low", "mid", "high"], "a box per tier");
        // A row per task, the cursor on the cursor task's name alone.
        let names: Vec<&str> = text[head + 1..].iter().map_while(|l| l.split_whitespace().next()).collect();
        let bare: Vec<&str> = names.iter().map(|n| n.trim_matches(['▌', '▐'])).collect();
        assert_eq!(bare, TASKS.iter().map(|t| t.name).collect::<Vec<_>>());
        assert_eq!(names.iter().filter(|n| n.starts_with('▌')).collect::<Vec<_>>(), [&"▌vision▐"]);
        let cursor = &lines[head + 1 + a.task_cur].spans[1];
        assert_eq!(
            (cursor.content.as_ref(), cursor.style.fg, cursor.style.bg),
            ("vision", Some(TASK[a.task_cur]), Some(CURSOR))
        );
        // Under the grid, what that task is and when to use it.
        let under = head + 1 + TASKS.len() + 1;
        assert!(text[under].starts_with("vision image input"), "{}", text[under]);
        assert!(text[under + 1].starts_with("  use for: screenshots"), "{}", text[under + 1]);
        let long: Vec<&String> = text.iter().filter(|l| l.chars().count() > 60).collect();
        assert!(long.is_empty(), "wrapped to the width: {long:?}");
        let mut b = app();
        let mut data = std::mem::take(&mut b.data);
        for (m, pct) in data.models.iter_mut().zip([90.0, 60.0]) {
            m.fit.insert("overall".into(), pct);
            // flash is 6 months behind opus: `low`, and no further.
            m.lag.insert("overall".into(), (100.0 - pct) / 5.0);
        }
        b.set_data(data);
        (b.view, b.task_sel) = (View::Recommend, 2);
        // On the `among` line it is on a tab alone, the one in use bold, and "(filtered)" says a search narrows them.
        (b.among, b.query) = (Some(1), "o".into());
        let (top, block, _) = recommend(&b, 200, &mut vec![]);
        let on: Vec<&Span> = top.iter().flat_map(|l| l.spans.iter()).filter(|s| s.style.bg == Some(CURSOR)).collect();
        assert!(on.len() == 3 && on[1].content == "○ all" && block == Some(0..1), "{on:?} {block:?}");
        let yours = top[0].spans.iter().find(|s| s.content == "● yours").unwrap();
        assert!(
            yours.style == fg(ACCENT).add_modifier(BOLD) && top[0].to_string().ends_with(" (filtered)"),
            "{}",
            top[0]
        );
        let narrow = recommend(&b, 60, &mut vec![]).0;
        assert_eq!(narrow[1].to_string(), "       (filtered)", "it wraps where the tabs fill the line");
        // On a tier, the cursor is on its box, as wide as every other, and not on the name.
        (b.among, b.query) = (None, String::new());
        let all = recommend(&b, 200, &mut vec![]).0;
        let bars: Vec<&str> = all
            .iter()
            .flat_map(|l| l.spans.iter())
            .filter(|s| s.style.bg == Some(CURSOR))
            .map(|s| &*s.content)
            .collect();
        assert_eq!(bars, ["▌", "opus $5.0 (150)", "  ", "▐"], "mid of overall, padded to the widest entry");
        let row = all.iter().map(ToString::to_string).find(|l| l.starts_with(" overall")).unwrap();
        let picks: Vec<&str> = row.split_whitespace().filter(|w| w.ends_with(['s', 'h'])).collect();
        assert_eq!(picks, ["flash", "▌opus", "opus"], "what each tier picks: low, mid and high");
        let value = all.iter().map(ToString::to_string).find(|l| l.starts_with(" value")).unwrap();
        assert_eq!(value.split_whitespace().collect::<Vec<_>>(), ["value", "-"], "a rank has one pick, not a tier's");
        assert!(all.iter().any(|l| l.to_string() == "  mid:     opus $5.0 (150)"), "under the grid, the box's model");
        // A name is cut from its start to the room its box has, the price and score kept.
        let cut: String = entry(&b, "overall", 1, b.tier_picks(0)[1], Some(14)).iter().map(|s| &*s.content).collect();
        assert_eq!(cut, "…us $5.0 (150)");
        // Among all, a model out of reach is grey, and says so under the grid; one of yours keeps its price's colour.
        let mut data = std::mem::take(&mut b.data);
        data.models[0].available = false;
        b.set_data(data);
        b.all = true;
        b.rebuild();
        let all = recommend(&b, 200, &mut vec![]).0;
        let of = |n: &str| all.iter().flat_map(|l| l.spans.iter()).find(|s| s.content.starts_with(n)).unwrap().style;
        assert!(
            of("opus ").fg == Some(MUTED) && of("flash ").fg != Some(MUTED),
            "{:?} {:?}",
            of("opus "),
            of("flash ")
        );
        let said = |n: &str| all.iter().any(|l| l.to_string().contains(&format!("{n} not available")));
        assert!(said("(150)") && !said("(120)"), "opus says so, flash does not");
        // A task of your own is the last row, in a colour of its own, every tier with the model you gave it; under
        // the grid, what you wrote it is about and that it is picked over a built-in task.
        a.store.toggle_favorite("debugging", "opus");
        a.store.set_about("debugging", "finding and fixing a bug");
        (a.view, a.task_cur) = (View::Recommend, TASKS.len());
        let lines = recommend(&a, 80, &mut vec![]).0;
        let text: Vec<String> = lines.iter().map(ToString::to_string).collect();
        let own = text.iter().position(|l| l.starts_with("▌debugging▐")).unwrap();
        assert_eq!((lines[own].spans[1].style.fg, text[own + 1].as_str()), (Some(task_color("debugging")), ""));
        assert_ne!(task_color("debugging"), task_color("docs"), "tasks of your own can differ in colour");
        assert!(
            OWN.iter().all(|c| !TASK.contains(c) && ![STAR, BAD].contains(c)),
            "and none has a built-in task's, nor the gold ★'s, nor the red ✗'s"
        );
        let models = |row: &str| row.split("★ ").skip(1).map(|s| s.trim().to_string()).collect::<Vec<_>>();
        assert_eq!(models(&text[own]), ["opus $5.0", "opus $5.0", "opus $5.0"]);
        assert_eq!(text[own + 2], "debugging finding and fixing a bug");
        assert_eq!(text[own + 3], format!("  use for: {CUSTOM_WHEN}"));
        // A tier's model is in that tier's box, the task's in the others.
        a.store.toggle_favorite("debugging:low", "flash");
        let text: Vec<String> = recommend(&a, 80, &mut vec![]).0.iter().map(ToString::to_string).collect();
        assert_eq!(models(&text[own]), ["flash $0.10", "opus $5.0", "opus $5.0"]);
    }
}
