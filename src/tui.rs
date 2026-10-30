//! Terminal shell: owns the terminal, the input loop and the refresh thread, and draws `App`.
//! Redraws only on input or when a background refresh lands.

//!
//! The table is drawn straight into the buffer: one pass over the visible rows, no widget
//! allocations per frame. Colours come from the terminal's 16-colour palette, so a themed
//! terminal themes modelcmp too, and nothing paints a background over a transparent one but a
//! marked row's fill and the cursor's.

use crate::app::{
    App, BOXES, COLS, Download, ECI, EXCLUDED, Edit, Effect, FAV, HELP, HELP_TAB, Input, Kind, List, MARKED, MINE,
    Mouse, NCOLS, NEW_TASK, NOTES, PRICE, RECOMMEND, Stop, TABS, VIA, View, What, base_col_name, bindable, bound_keys,
    box_slot, found, got, group_starts, help_rows, hidden, on_price, runs_on, shown, slot_box,
};
use crate::data::{self, Data, Model};
use crate::fit::{self, TASKS};
use crate::store::Store;
use crate::view::{
    CUSTOM_WHEN, NO_ACCESS, OUT_OF_REACH, Palette, THEMES, TIERS, age, detail_rows, hits, level, level_label, short,
    truncate, verdict, via_names, via_text,
};
use ratatui::backend::CrosstermBackend;
use ratatui::buffer::{Buffer, Cell, CellWidth};
use ratatui::crossterm::cursor::Show;
use ratatui::crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, Event, KeyCode,
    KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::crossterm::execute;
use ratatui::layout::{Alignment, Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Widget};
use ratatui::{Frame, Terminal};
use std::collections::{BTreeMap, HashMap};
use std::io::{BufWriter, IsTerminal, Stdout};
use std::ops::Range;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

/// What a refresh sends: the data before its slow harnesses, when it has any to wait for, then
/// its outcome.
enum Refreshed {
    Early(Data),
    Done(Result<Data, String>, crate::app::Tools),
}

/// A refresh under way: where its result comes, and its steps for the frame's count.
type Refresh = (Receiver<Refreshed>, data::Steps);

pub fn run(mut store: Store, force: bool) -> Result<(), String> {
    // Run by an agent or a script there is no terminal to draw on: say so before any download or
    // harness is started, not after.
    if !std::io::stdout().is_terminal() {
        return Err("the TUI needs a terminal, see modelcmp --help".into());
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
    let (term_bg, term_ansi) = if cfg!(unix) { terminal_colors() } else { (None, None) };
    // Start from the cache however old, or with no cache from an empty table, and download behind
    // the intro and the table (`--refresh` too), so nothing waits for it and a failure shows in
    // the frame.
    let mut app = App::new(data::load_cache().unwrap_or_default(), store);
    app.dir = std::env::current_dir().map(|d| d.to_string_lossy().into_owned()).unwrap_or_default();
    (app.term_bg, app.term_ansi) = (term_bg, term_ansi);
    let mut rx = None;
    if (force || app.data.stale() || app.data.of_another_day()) && app.refresh().is_some() {
        rx = Some(spawn_refresh());
    }
    // After the refresh starts, which clears the status: it is your marks and notes that are at
    // stake. What the last refresh said is the frame's `⚠` and `!`'s.
    if let Some(w) = app.store.warning.take() {
        app.report(Err(w));
    }
    // ratatui's setup (raw mode, the alternate screen, its panic hook), and a terminal of our own
    // that writes each frame in one go.
    let fail = |e| format!("the TUI needs a terminal ({e}), see modelcmp --help");
    ratatui::try_init().map_err(fail)?;
    TAKEN.store(true, Ordering::SeqCst);
    #[cfg(unix)]
    restore_on_signals();
    let mut terminal = Terminal::new(buffered(std::io::stdout())).map_err(fail)?;
    // ratatui's panic hook restores the terminal but leaves mouse reporting on, and bracketed
    // paste, which keeps pasted text (ctrl+v in some terminals) from arriving as keys. And the
    // cursor hidden, which only its terminal's drop shows again: a release build aborts instead.
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(std::io::stdout(), DisableMouseCapture, DisableBracketedPaste, Show);
        hook(info);
    }));
    let _ = execute!(std::io::stdout(), EnableMouseCapture, EnableBracketedPaste);
    let res = if new { intro(&app, &mut terminal).map_err(|e| e.to_string()) } else { Ok(()) }
        .and_then(|()| event_loop(&mut app, &mut terminal, rx));
    restore();
    let Some((cmds, unsaved)) = res? else { return Ok(()) };
    // On the terminal's own screen, where the upgrade's output shows.
    if let Some(e) = unsaved {
        eprintln!("modelcmp: {e}");
    }
    upgrade(&cmds)
}

/// Whether the TUI has the terminal: raw, on its own screen, with mouse reporting on.
static TAKEN: AtomicBool = AtomicBool::new(false);

/// Give the terminal back as the TUI found it. The cursor too: the terminal shows it only when
/// dropped, after an upgrade a ^C may end. Only once, as leaving a screen it is no longer on
/// moves the cursor back to where the TUI started, over what an upgrade wrote since. Quietly, as
/// after a hang-up there is no terminal, and saying so on it panics, which aborts a release build.
fn restore() {
    if TAKEN.swap(false, Ordering::SeqCst) {
        let _ = execute!(std::io::stdout(), DisableMouseCapture, DisableBracketedPaste, Show);
        let _ = ratatui::try_restore();
    }
}

/// Killed from outside, by `kill`, `timeout` or a hang-up, the TUI gives the terminal back as a
/// quit does, or the shell's prompt comes back on its screen with no cursor and the pointer's
/// moves typed into it. Then it exits as the signal would, 128 and its number. A ^C in the TUI is
/// a key, so only a SIGINT sent by another process comes here.
#[cfg(unix)]
fn restore_on_signals() {
    use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};
    let Ok(mut signals) = signal_hook::iterator::Signals::new([SIGTERM, SIGINT, SIGHUP]) else { return };
    std::thread::spawn(move || {
        let Some(sig) = signals.forever().next() else { return };
        // Held to the exit, so a frame being drawn cannot land on the shell's screen after it.
        let _out = std::io::stdout().lock();
        restore();
        std::process::exit(128 + sig);
    });
}

/// The terminal drawn on.
type Term = Terminal<CrosstermBackend<BufWriter<Stdout>>>;

/// `out` behind a buffer that holds a whole frame, which ratatui flushes once it is drawn.
/// `Stdout` alone passes a frame on a kilobyte at a time, a write(2) each.
fn buffered<W: std::io::Write>(out: W) -> CrosstermBackend<BufWriter<W>> {
    CrosstermBackend::new(BufWriter::with_capacity(1 << 17, out))
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
        &[&["cargo", "install", "--locked", "--git", REPO, "--tag", &tag]]
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
        return Err(concat!(
            "still modelcmp v",
            env!("CARGO_PKG_VERSION"),
            " after the upgrade, try again in a few minutes"
        )
        .into());
    }
    let mut new = Command::new(name);
    new.args(args);
    #[cfg(unix)]
    return Err(format!(
        "upgraded, but could not start the new version: {}",
        std::os::unix::process::CommandExt::exec(&mut new)
    ));
    #[cfg(not(unix))]
    new.status().map(|_| ()).map_err(|e| format!("upgraded, but could not start the new version: {e}"))
}

/// The terminal's background colour, for a marked row's faint fill with the terminal's own
/// colours, and its 16 (OSC 4), for a pill's faded one behind a box. It is asked for with OSC 11, then for the cursor position, which every terminal
/// answers: replies come in the order asked, so once the position is in, the colour's either is
/// queued or is not coming, with no late reply read as keys. Read from the tty here, not through
/// crossterm, which waits 2 s for a position that does not come and knows no OSC reply. The fill
/// is a 24-bit colour, which the 16 never were, so a terminal that does not say it draws those is
/// not asked: one may answer here and still garble them. It says so in `COLORTERM`, or, as `ssh`
/// and `sudo` drop that, with a `TERM` only a terminal that draws them sets. Under WSL neither is
/// set, and every Windows console that runs it draws them.
/// ponytail: a reply later than the wait, a second here and two over ssh, is read as keys by the
/// table, where its `e` excludes a model. Ask the terminal ahead of the first frame if one is slower.
fn terminal_colors() -> (Option<u32>, Option<[u32; 16]>) {
    use ratatui::crossterm::terminal::{disable_raw_mode, enable_raw_mode};
    use std::io::{ErrorKind, Read, Write};
    let term = std::env::var("TERM").unwrap_or_default();
    if !matches!(std::env::var("COLORTERM").as_deref(), Ok("truecolor" | "24bit"))
        && !["kitty", "alacritty", "ghostty", "foot", "wezterm", "direct"].iter().any(|t| term.contains(t))
        && std::env::var_os("WSL_DISTRO_NAME").is_none()
    {
        return (None, None);
    }
    // Not blocking, so the wait is ours. Raw, so the reply is not echoed.
    #[cfg(unix)]
    let tty = std::os::unix::fs::OpenOptionsExt::custom_flags(std::fs::OpenOptions::new().read(true), libc::O_NONBLOCK)
        .open("/dev/tty");
    #[cfg(not(unix))]
    let tty = std::fs::File::open("/dev/tty");
    let Ok(mut tty) = tty else { return (None, None) };
    if enable_raw_mode().is_err() {
        return (None, None);
    }
    let mut out = std::io::stdout();
    let mut reply = Vec::new();
    let ask: String = (0..16).map(|i| format!("\x1b]4;{i};?\x07")).collect();
    if out.write_all(format!("\x1b]11;?\x07{ask}\x1b[6n").as_bytes()).and_then(|()| out.flush()).is_ok() {
        // The most the first frame waits for them, which a terminal that answers ends at once: one
        // here in a few milliseconds, more under a loaded WSL, one over ssh in a round trip. A
        // reply later than that is read as keys, so the wait is long.
        let wait = Duration::from_secs(if std::env::var_os("SSH_CONNECTION").is_some() { 2 } else { 1 });
        let (end, mut buf) = (Instant::now() + wait, [0; 1024]);
        // ponytail: looked at every millisecond rather than waited on, as poll(2) is unsafe code.
        while !positioned(&String::from_utf8_lossy(&reply)) && Instant::now() < end {
            match tty.read(&mut buf) {
                Ok(n) if n > 0 => reply.extend_from_slice(&buf[..n]),
                Err(e) if e.kind() == ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(1)),
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                _ => break,
            }
        }
    }
    let _ = disable_raw_mode();
    let reply = String::from_utf8_lossy(&reply);
    (parse_bg(&reply), parse_ansi(&reply))
}

/// Whether the cursor position's reply, `ESC [ row ; col R`, is in: the last of those asked for.
fn positioned(reply: &str) -> bool {
    let number = |n: &str| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit());
    reply
        .split("\x1b[")
        .skip(1)
        .any(|s| s.split_once('R').and_then(|(at, _)| at.split_once(';')).is_some_and(|(r, c)| number(r) && number(c)))
}

/// The colour in an OSC 11 reply, `]11;rgb:1e1e/1e1e/2e2e`: the high byte of each channel. Not
/// an OSC 4 one's, `]4;11;rgb:...` included, from a terminal that answers those alone.
fn parse_bg(reply: &str) -> Option<u32> {
    let (i, _) = reply
        .match_indices("rgb:")
        .find(|&(i, _)| reply[..i].strip_suffix("11;").is_some_and(|h| !h.ends_with(';')))?;
    rgb(&reply[i + 4..])
}

/// The colour at the start of `s`, `1e1e/1e1e/2e2e`: the high byte of each channel.
fn rgb(s: &str) -> Option<u32> {
    let mut rgb = s.split('/').map(|c| u32::from_str_radix(c.get(..2)?, 16).ok());
    Some(rgb.next()?? << 16 | rgb.next()?? << 8 | rgb.next()??)
}

/// The terminal's 16 colours in the OSC 4 replies after the OSC 11 one, each `4;<slot>;rgb:...`
/// after its ESC `]` (`terminal_colors`), when it told all of them.
fn parse_ansi(reply: &str) -> Option<[u32; 16]> {
    let mut slots = [None; 16];
    let parts: Vec<&str> = reply.split("rgb:").collect();
    for w in parts.windows(2) {
        // What precedes `rgb:` ends in `4;<slot>;`, after the previous colour's digits.
        let Some((head, slot)) = w[0].strip_suffix(';').and_then(|h| h.rsplit_once(';')) else { continue };
        if let (true, Ok(i @ 0..16)) = (head.ends_with('4'), slot.parse::<usize>()) {
            slots[i] = rgb(w[1]);
        }
    }
    slots.iter().all(Option::is_some).then(|| slots.map(Option::unwrap_or_default))
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
/// The wordmark where the big one has no room, a tiled or split window: the same letters, smaller.
const SMALL: [&str; 4] = [
    "                    ██        ██                        ",
    "██▀██▀█▄ ▄█▀▀█▄ ▄█▀▀██ ▄████▄ ██  ▄█▀▀▀▀ ██▀██▀█▄ ██▀▀█▄",
    "██ ██ ██ ▀█▄▄█▀ ▀█▄▄██ ▀█▄▄▄▄ ▀█▄ ▀█▄▄▄▄ ██ ██ ██ ██▄▄█▀",
    "                                                  ██    ",
];
/// The wordmarks, the first that fits is shown (`logo_at`).
const LOGOS: [&[&str]; 2] = [&LOGO, &SMALL];
/// A wordmark's corner, the rows under it that the tagline and the version take, and its rows.
type At = (u16, u16, u16, &'static [&'static str]);
/// Under the wordmark, after a blank row.
const TAGLINE: &str = "the right AI model for each agent, automatically";

/// Where the wordmark `logo` goes in `a`: its corner, and the rows under it that the tagline and
/// the version take, as far as there is room for them (0, 2 or 3). It is centred with `below` more
/// rows kept free under those. None in an area too small.
fn logo_at(logo: &'static [&'static str], a: Rect, below: u16) -> Option<At> {
    let (w, rows) = (logo[0].chars().count() as u16, logo.len() as u16);
    if a.width < w || a.height < rows + below {
        return None;
    }
    // A blank row and the tagline, then the version.
    let under = match a.height - rows - below {
        0 | 1 => 0,
        2 => 2,
        _ => 3,
    };
    Some((a.x + (a.width - w) / 2, a.y + (a.height - rows - under - below) / 2, under, logo))
}

/// The wordmark with its corner at `(x, y)`, each cell in the style its column and row give, and
/// under it, where `under` leaves them room (`logo_at`), the first `typed` bytes of the tagline
/// and, once that is whole, the version. Rows past the end of `a` are left out.
fn wordmark(buf: &mut Buffer, a: Rect, (x, y, under, logo): At, typed: usize, style: impl Fn(usize, usize) -> Style) {
    for (r, row) in logo.iter().enumerate() {
        let yr = y + r as u16;
        if yr >= a.bottom() {
            break;
        }
        for (c, ch) in row.chars().enumerate() {
            buf[(x + c as u16, yr)].set_char(ch).set_style(style(c, r));
        }
    }
    let (rows, tw) = (logo.len() as u16, TAGLINE.len());
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
/// skips it, and is not passed on. A window too small for the wordmark has the small one, and one
/// too small for that skips it too.
fn intro(app: &App, terminal: &mut Term) -> std::io::Result<()> {
    // The hue wheel up to the accent, so the last step into magenta is a small one.
    const RAINBOW: [Color; 5] = [Color::Red, Color::Yellow, Color::Green, Color::Cyan, Color::Blue];
    /// Columns the rainbow crosses per frame.
    const SPEED: usize = 2;
    /// Frames each colour lasts in a cell, so the bands are `STEP * SPEED` columns wide.
    const STEP: usize = 3;
    /// Frames the glide takes at most: it ends as soon as the wordmark rounds into place.
    const SLIDE: usize = 20;
    let tw = TAGLINE.len();
    let theme = palette(app);
    // From 1, as frame 0 would put the wordmark just off the screen.
    let mut t = 1;
    loop {
        // `end`: the last frame, 0 with no room for a wordmark.
        let (mut end, mut landed) = (0, false);
        terminal.draw(|f| {
            let a = f.area();
            let Some((x, y, under, logo)) = LOGOS.into_iter().find_map(|l| logo_at(l, a, 0)) else {
                return;
            };
            let (w, rows) = (logo[0].chars().count(), logo.len());
            // The wave reaches a cell `SLIDE + (c + 2 * (rows - 1 - r)) / SPEED` frames in, the
            // bottom row first as cells are twice as tall as wide, and the last cell settles
            // `RAINBOW.len() * STEP` frames after that.
            end = SLIDE + (w + 2 * (rows - 1)).div_ceil(SPEED) + RAINBOW.len() * STEP;
            // Ease out: from the bottom edge, fast at first and slowing into place.
            let left = 1.0 - (t.min(SLIDE) as f32 / SLIDE as f32);
            let off = (f32::from(a.bottom() - y) * left * left * left).round() as u16;
            landed = off == 0;
            // Each character as the wave's front passes over it: two rows under the bottom one,
            // so 4 columns ahead of the front there.
            let typed = if t > SLIDE { ((t - SLIDE) * SPEED + 4).saturating_sub((w - tw) / 2).min(tw) } else { 0 };
            let buf = f.buffer_mut();
            wordmark(buf, a, (x, y + off, under, logo), typed, |c, r| {
                match t.checked_sub(SLIDE + (c + 2 * (rows - 1 - r)) / SPEED) {
                    None => fg(MUTED),
                    Some(k) => fg(RAINBOW.get(k / STEP).copied().unwrap_or(ACCENT)).add_modifier(BOLD),
                }
            });
            recolor(buf, theme, app.term_bg, app.term_ansi.as_ref());
        })?;
        if end == 0 {
            return Ok(());
        }
        if landed {
            // Once it rounds into place, start the rainbow rather than hold it still.
            t = t.max(SLIDE);
        }
        // A key skips it; a pointer move or a resize does not, nor does it cut the frame short.
        // Past the deadline the wait is zero, so what is still queued is read and a key behind
        // a burst of pointer moves is not left for the table. The whole wordmark is held before
        // the table takes its place.
        let until = Instant::now() + Duration::from_millis(if t >= end { 1100 } else { 25 });
        loop {
            if !event::poll(until.saturating_duration_since(Instant::now()))? {
                break;
            }
            if matches!(event::read()?, Event::Key(k) if k.kind == KeyEventKind::Press) {
                return Ok(());
            }
        }
        if t >= end {
            return Ok(());
        }
        t += 1;
    }
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
/// What a quit for an upgrade leaves to do: its commands, and a save that failed, said first.
type Upgrading = (Vec<Vec<String>>, Option<String>);

fn event_loop(app: &mut App, terminal: &mut Term, mut rx: Option<Refresh>) -> Result<Option<Upgrading>, String> {
    let mut dirty = true;
    // The sizes of downloads Hugging Face is asked for (`Effect::Size`), and how many are awaited.
    let (size_tx, size_rx) = mpsc::channel::<(String, Download)>();
    let mut sizing = 0usize;
    // What a copy, a new terminal or a page opened said, each done off the loop (`off`), and how
    // many are awaited.
    let (said_tx, said_rx) = mpsc::channel();
    let mut saying = 0usize;
    // The store's lock, from the first input of a batch to the save after its frame, so an agent's
    // write cannot land in between (`relock`). What that save is to write: the store, and the sizes
    // and the downloads with no copy that Hugging Face said meanwhile.
    let (mut held, mut unsaved, mut sized, mut gone_now) = (None, false, false, vec![]);
    app.set_sizes(data::load_sizes());
    let mut gone = data::load_gone();
    app.set_gone(gone.keys().cloned());
    let ask_size = |key: String, base: String| {
        let tx = size_tx.clone();
        std::thread::spawn(move || {
            let size = data::download_size(&base, &key);
            let answer = match size {
                Ok(Some(bytes)) => Download::Size(bytes),
                Ok(None) => Download::Gone,
                Err(_) => Download::NoAnswer,
            };
            let _ = tx.send((key, answer));
        });
    };
    // The newest release, asked for by a start that refreshes nothing: a refresh asks for it too,
    // but only once a day.
    let mut newest = rx.is_none().then(|| {
        let (tx, newest) = mpsc::channel();
        std::thread::spawn(move || data::newest_release().map(|v| tx.send(v)));
        newest
    });
    // The last press on a cell, which a second one makes a double click (`double`), and the
    // screen row of the last press on a table row, which a drag extends from (`dragged`).
    let (mut click, mut press) = (None, None);
    let mut ask_harness = true;
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
        match newest.as_ref().map(Receiver::try_recv) {
            Some(Ok(v)) => {
                (app.data.latest, newest, dirty) = (v, None, true);
            }
            Some(Err(TryRecvError::Disconnected)) => newest = None,
            _ => {}
        }
        // One write and one rebuild for the answers that came together, not one each.
        let answers: Vec<(String, Download)> = size_rx.try_iter().collect();
        sizing -= answers.len();
        for (key, answer) in &answers {
            dirty |= *answer != Download::NoAnswer;
            match answer {
                Download::Size(_) => sized = true,
                Download::Gone => gone_now.push(key.clone()),
                _ => {}
            }
        }
        app.sized(answers.iter().map(|(k, a)| (k.as_str(), *a)));
        for said in said_rx.try_iter() {
            (saying, dirty) = (saying - 1, true);
            app.report(said);
        }
        // The count of a refresh under way moves on its own, with no key pressed.
        let progress = rx.as_ref().map_or_else(String::new, |r| data::progress(&r.1));
        if progress != app.progress {
            app.progress = progress;
            dirty = true;
        }
        // A start with two harnesses asks for your default, on its first frame with nothing open
        // (after the key prompt), and saves that it asked at once: later it would take keys meant
        // for the table, and a quit would ask again.
        if ask_harness && app.input == Input::None {
            ask_harness = false;
            relock(&mut held, app);
            unsaved |= app.ask_default();
        }
        let mut painted = if dirty { paint(terminal, app) } else { Ok(()) };
        // Only now, after the frame: what the input changed shows at once, not after its write.
        // Saved too when the frame could not be drawn, the terminal gone.
        if std::mem::take(&mut unsaved)
            && let Err(e) = app.store.save()
        {
            app.report(Err(format!("could not save: {e}")));
            painted = painted.and_then(|()| paint(terminal, app));
        }
        held = None;
        if std::mem::take(&mut sized) {
            data::save_sizes(&app.sizes());
        }
        if !gone_now.is_empty() {
            data::save_gone(&mut gone, gone_now.drain(..));
        }
        painted?;
        // Block on input; wake every 200ms while a refresh or the newest release is awaited, else
        // once a minute to repaint the data age in the frame.
        // A size awaited is looked for often, as `x`'s list is open for it, and so is what a copy,
        // a new terminal or a page opened says. The sizes still to ask for, of the local rows on
        // screen and the ones Via marks `↓`, are asked once the cursor has rested, not for each
        // row it passes.
        let rest = app.size_wanted();
        let timeout = match (sizing + saying > 0, rest, rx.is_some() || newest.is_some()) {
            (true, ..) => Duration::from_millis(30),
            (_, true, _) => Duration::from_millis(150),
            (.., true) => Duration::from_millis(200),
            _ => Duration::from_secs(60),
        };
        dirty = rx.is_none() && newest.is_none() && sizing + saying == 0 && !rest;
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
                        dirty |= relock(&mut held, app);
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
            relock(&mut held, app);
            let effect = match input {
                Some(Ok(k)) => app.key(k),
                Some(Err(m)) => app.mouse(m),
                None => None,
            };
            // The bounds, wherever they changed, are kept for the CLI as the dropdowns' picks are.
            unsaved |= app.take_bounds_changed() || effect == Some(Effect::Save);
            {
                match effect {
                    // No frame follows to save after.
                    Some(Effect::Quit) => {
                        save_now(app, unsaved)?;
                        return Ok(None);
                    }
                    Some(Effect::Upgrade) => {
                        // Unresolved when it cannot be, as replaced under a running TUI it cannot:
                        // its path still says what installed it.
                        let exe = std::env::current_exe().map(|p| p.canonicalize().unwrap_or(p)).unwrap_or_default();
                        if let Some(cmds) = app.data.update().and_then(|new| upgrade_cmds(&exe, new)) {
                            // A save that fails keeps no upgrade from running: it is said before.
                            return Ok(Some((cmds, save_now(app, unsaved).err())));
                        }
                        // Not Homebrew's nor cargo's to replace: the release's page has the binaries.
                        let url = format!("{REPO}/releases/latest");
                        off(&said_tx, &mut saying, move || match open::that_detached(&url) {
                            Ok(()) => Ok(format!("installed by hand, opening {url} for the new version")),
                            Err(e) => Err(format!("could not open {url}: {e}")),
                        });
                    }
                    Some(Effect::Open(url)) => off(&said_tx, &mut saying, move || match open::that_detached(&url) {
                        // A browser is only asked to: what it does with the page is not known here.
                        Ok(()) => Ok(format!("opening {url}")),
                        Err(e) => Err(format!("could not open {url}: {e}")),
                    }),
                    Some(Effect::Copy(text)) => off(&said_tx, &mut saying, move || {
                        if copy(&text) { Ok(format!("copied {text}")) } else { Err("no clipboard tool found".into()) }
                    }),
                    Some(Effect::Refresh) => {
                        data::clear_gone(&mut gone);
                        rx = Some(spawn_refresh());
                    }
                    Some(Effect::Launch(cmd)) => off(&said_tx, &mut saying, move || launch(&cmd, None)),
                    Some(Effect::LaunchIn(dir, cmd)) => off(&said_tx, &mut saying, move || launch(&cmd, Some(&dir))),
                    // Off the loop: the list is open and takes the size when it comes.
                    Some(Effect::Size(key, base)) => {
                        sizing += 1;
                        ask_size(key, base);
                    }
                    // The app applies its own chooser items before they get here.
                    Some(
                        Effect::Save
                        | Effect::Fav(..)
                        | Effect::Via(..)
                        | Effect::NewTask(_)
                        | Effect::Theme(_)
                        | Effect::Harness(_)
                        | Effect::Col(_)
                        | Effect::Bench(..)
                        | Effect::Pick(..),
                    )
                    | None => {}
                }
                // The start's warning, kept while a question was open, is said once it closes,
                // however it does, after an error the closing gave.
                if matches!(app.input, Input::None)
                    && let Some(w) = app.store.warning.take()
                {
                    app.report(Err(if app.failed { format!("{} · {w}", app.status) } else { w }));
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

/// The store's lock for a batch of input, taken at its first (`held`): what an agent saved before
/// then is read in, and from then to the save after the frame nothing lands in between. True when
/// the store was read again.
fn relock(held: &mut Option<std::io::Result<std::fs::File>>, app: &mut App) -> bool {
    if held.is_some() {
        return false;
    }
    *held = Some(crate::store::lock(&crate::store::path()));
    let again = app.store.reload_if_changed();
    if again {
        app.load_bounds();
        app.set_cols();
        app.rebuild_in_place();
    }
    again
}

/// What the batch changed, saved before a quit or an upgrade leaves no frame to save after.
fn save_now(app: &mut App, unsaved: bool) -> Result<(), String> {
    if unsaved {
        app.store.save().map_err(|e| format!("could not save: {e}"))?;
    }
    Ok(())
}

/// Runs `f` off the loop, as the program it starts takes its time: clip.exe a tenth of a second,
/// and under WSL a search of `PATH` through Windows's folders for wt.exe as long. What it says
/// comes back on `tx`, `awaited` counting it until then.
fn off(
    tx: &mpsc::Sender<Result<String, String>>,
    awaited: &mut usize,
    f: impl FnOnce() -> Result<String, String> + Send + 'static,
) {
    *awaited += 1;
    let tx = tx.clone();
    std::thread::spawn(move || drop(tx.send(f())));
}

/// Draws the app, in the theme's colours.
fn paint(terminal: &mut Term, app: &mut App) -> Result<(), String> {
    terminal
        .draw(|f| {
            draw(app, f);
            app.more = more_at(f.buffer_mut());
            recolor(f.buffer_mut(), palette(app), app.term_bg, app.term_ansi.as_ref());
        })
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Every `‹` `›` `▲` `▼` drawn in `buf` as `MORE`, which nothing else is, and the way it
/// points, which a click on it moves a screen (`hit`), for a phone or a mouse with no sideways
/// wheel.
/// Found on the screen, as compare's are in its lines' spans and land where those do.
fn more_at(buf: &Buffer) -> Vec<(u16, u16, Mouse)> {
    (buf.content.iter().enumerate())
        .filter_map(|(i, cell)| {
            if (Some(cell.fg), Some(cell.bg)) != (MORE.fg, MORE.bg) || !cell.modifier.contains(BOLD) {
                return None;
            }
            let (x, y) = buf.pos_of(i);
            let m = match cell.symbol() {
                "▲" => Mouse::Scroll(-1),
                "▼" => Mouse::Scroll(1),
                "‹" => Mouse::Cols(-1),
                "›" => Mouse::Cols(1),
                _ => return None,
            };
            Some((x, y, m))
        })
        .collect()
}

/// How many columns compare shows: what a click on its `‹` `›` moves, so the view moves by a
/// screen. One model in recommend.
fn across(app: &App) -> isize {
    if app.view == View::Compare { app.compare_shown.max(1) as isize } else { 1 }
}

/// What a mouse event lands on, with the same geometry `draw` uses: the frame's inner area
/// holds the header and then the rows from `app.table.offset()`.
fn hit(app: &App, area: Rect, m: MouseEvent) -> Option<Mouse> {
    let held = !m.modifiers.is_empty();
    match m.kind {
        // Shift, alt or ctrl+wheel goes sideways, for terminals and mice that send no sideways
        // wheel. Any of the three, as most terminals keep one for themselves (often shift).
        MouseEventKind::ScrollDown if held => return Some(Mouse::Cols(1)),
        MouseEventKind::ScrollUp if held => return Some(Mouse::Cols(-1)),
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
    let plain = m.kind == MouseEventKind::Down(MouseButton::Left) && !pick && !extend;
    // Under the tabs' two lines and the frame's top border.
    let inner = Rect::new(1, 3, area.width - 2, area.height.saturating_sub(5));
    let l = layout(inner.width, app);
    let more = app.more.iter().find(|&&(x, y, _)| plain && (x, y) == (m.column, m.row)).map(|&(.., a)| match a {
        Mouse::Scroll(d) => Mouse::Page(d * app.page.max(1) as isize),
        // The table pages: `›` to the first column cut off, which then starts the view, `‹` to the
        // one before the view, which then ends it, `hscroll` 0 letting `layout` bring in all it can.
        Mouse::Cols(d) if app.view == View::Table => {
            let to = if d > 0 {
                l.next.map(|c| (c - 1, c))
            } else {
                (1..=l.first).rev().find(|&c| !hidden(c)).map(|c| (0, c))
            };
            to.map_or(Mouse::Cols(d), |(h, c)| Mouse::ToCol(h, c))
        }
        Mouse::Cols(d) => Mouse::Across(d * across(app)),
        a => a,
    });
    let head = head(app);
    // A tab on the first two lines, or the edge left of it, over recommend too.
    let on_tabs = m.row <= area.y + 1 && !mark && !pick && !extend;
    let tab = tab_ends(app).position(|end| on_tabs && (1..end).contains(&usize::from(m.column))).map(Mouse::Tab);
    // An open list first: a click on an entry acts on it, on its frame nothing, and any click
    // outside closes it.
    let list = match &app.input {
        // Scrolled as last drawn.
        Input::Menu { col, items, list } => {
            menu_box(inner, menu_x(inner, &l, *col), *col, items).map(|(b, _)| (b, list.top, items.len()))
        }
        Input::Choose { title, items, list, .. } => {
            let (within, lines) = chooser(app, area)?;
            Some((overlay_rect(within, title, &lines), list.top, items.len()))
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
            return more;
        }
        let k = top + (m.row - inner.y) as usize;
        // f's grid: a box is ticked, the name moves the cursor to its row, and the tiers' heading
        // and what follows the boxes are nothing to click.
        if let Input::Choose { kind: Kind::Fav, items, .. } = &app.input {
            let row = k.checked_sub(1).filter(|&r| r < len)?;
            if !matches!(items[row].1, Effect::Fav(..)) {
                return Some(Mouse::Item(row));
            }
            let x = usize::from(m.column.saturating_sub(rect.x + 2));
            let Some(col) = x.checked_sub(fav_name_w(items) + 2).map(|x| x / BOX_W) else {
                return Some(Mouse::Row(row));
            };
            return (col < BOXES).then_some(Mouse::Tick(row, col));
        }
        // The ✗ after the ✓, on an entry of Dev, Price or Via that has one, leaves it out.
        let ex = (inner.x + 3..inner.x + 5).contains(&m.column)
            && matches!(&app.input, Input::Menu { col: 1 | PRICE | VIA, items, .. } if k != 0 && k < items.len());
        return (k < len).then_some(if ex { Mouse::Exclude(k) } else { Mouse::Item(k) });
    }
    // The frame's ⚠ opens the issues, as `!` does, over a panel too.
    if plain && m.row == area.bottom() - 2 && app.issues_at.as_ref().is_some_and(|r| r.contains(&m.column)) {
        return Some(Mouse::Key(KeyCode::Char('!')));
    }
    // A click outside the open panel closes it, as one outside a list does, unless it is on a
    // hint or a tab.
    let close = (!extend && app.panel.is_some_and(|r| !r.contains(pos))).then_some(Mouse::Close);
    // A hint in the status bar presses its key.
    if m.row == area.bottom() - 1 {
        return (!mark && !extend).then(|| hint_at(app, area.width, m.column).map(Mouse::Key)).flatten().or(close);
    }
    if on_tabs {
        return tab.or(close);
    }
    if close.is_some() || more.is_some() {
        return close.or(more);
    }
    // In compare, recommend and the keys a click is on what is drawn under it (`App::spots`): a
    // plain one opens it on a double click (`double`), a right one selects it, as in the table,
    // and a ctrl or shift one, with no highlight there to add to, moves to it. Elsewhere it still
    // clears the status, as a click does in the table.
    if matches!(app.view, View::Compare | View::Recommend | View::Help) {
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

/// Which of row `n`'s harnesses is `x` cells into its Via, as `draw` lays it out: the short
/// names, a space between.
fn harness_at(app: &App, n: usize, x: u16) -> Option<usize> {
    let m = app.data.models.get(*app.rows.get(n)?).filter(|m| app.accessible(m))?;
    let mut end = 0;
    m.via.iter().position(|h| {
        let start = end;
        end += short(h).len() as u16 + 1;
        (start..end - 1).contains(&x)
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

/// Open `cmd` in a new terminal, in `dir` when one was written, and say how it went.
fn launch(cmd: &[String], dir: Option<&str>) -> Result<String, String> {
    let line = cmd.join(" ");
    let at = dir.map(|d| format!(", in {d}")).unwrap_or_default();
    match new_terminal(cmd, dir) {
        Ok(()) => Ok(format!("opened {line} in a new terminal{at}")),
        Err(e) => {
            let hint = if e.kind() == std::io::ErrorKind::NotFound { ", set $TERMINAL" } else { "" };
            Err(format!("could not open a terminal for {line}: {e}{hint}"))
        }
    }
}

/// Start `cmd` in a new terminal window in `dir`, else here, without waiting: Windows Terminal
/// under WSL, else `$TERMINAL -e`, else x-terminal-emulator.
fn new_terminal(cmd: &[String], dir: Option<&str>) -> std::io::Result<()> {
    // Windows Terminal splits its command line on `;` wherever it stands, and the ids come from
    // downloaded data: one holding a `;` would start a second command there. No real id has one.
    // A folder is refused where it is written (`App::edit_key`), and here once more under WSL.
    if cmd.iter().any(|a| a.contains(';')) || dir.is_some_and(data::splits) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "a ';' in the command or folder is not allowed",
        ));
    }
    let mut term = if let Some(session) = data::wsl_session(cmd, dir) {
        let mut c = Command::new("wt.exe");
        c.arg("new-tab").args(session);
        c
    } else {
        let mut c = Command::new(std::env::var("TERMINAL").unwrap_or_else(|_| "x-terminal-emulator".into()));
        c.arg("-e").args(cmd).current_dir(dir.unwrap_or("."));
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
/// Every box in recommend has it, two cells wide, muted but on the cursor's: `d` opens its dropdown.
const MENU: &str = " ▾";
/// The actions on one model, in the one order every view shows them in.
const ACTIONS: [&str; 9] =
    ["enter details", "x launch", "H harness", "o open", "y copy name", "space select", "f fav", "e exclude", "i note"];

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
/// on the column picked with `← →`.
fn hints(app: &App, width: u16) -> Vec<&'static str> {
    // A panel's search, and once it has found something the keys to go from match to match.
    let search = if app.overlay_query.is_empty() { vec!["/ search"] } else { vec!["/ search", "n N match"] };
    // On a box of `low` or `mid` in recommend, `<` writes its cap.
    let tier = app.task_sel.checked_sub(1).filter(|_| app.among.is_none() && !app.caps && !app.on_new_task());
    let cap = if tier.is_some_and(|c| crate::view::capped(TIERS[c].0)) { vec!["< price cap"] } else { vec![] };
    let groups: Vec<Vec<&'static str>> = match app.view {
        View::Table if app.selecting() => {
            vec![vec!["↓ ↑ G extend"], vec!["C compare"], actions("space f e"), vec!["esc cancel", "q quit"]]
        }
        View::Table => {
            let mut view = vec!["| columns", "/ filter", "s sort"];
            if app.menu(app.col) {
                view.push("d dropdown");
            }
            // On the price columns, or anywhere while it is off the default.
            let cached = data::cached() > 0.0;
            if (PRICE..ECI).contains(&app.col) || !cached {
                view.push(if cached { "% no cache" } else { app.cache_hint });
            }
            if !app.query.is_empty() || app.filtered_too() || app.task.is_some() || app.only.is_some() {
                view.push("c clear");
            }
            if app.any_marked() {
                view.push("u deselect");
                view.push("V highlight");
            }
            if app.only == Some(FAV) {
                view.push("D unfavorite all");
            }
            // Out of `E` too while the filters exclude models, as `X` takes those back.
            if app.only == Some(EXCLUDED) || app.filters_exclude() && app.tab_has(EXCLUDED) {
                view.push("X unexclude all");
            }
            if stale(app) {
                view.push("r refresh");
            }
            if app.data.warning.is_some() {
                view.push("! issues");
            }
            let mut back = vec![];
            // Where esc goes back from, as in the overlays.
            if !app.query.is_empty() || app.only.is_some() || app.task.is_some() {
                back.push("esc back");
            }
            back.push("q quit");
            vec![vec!["← → column"], view, actions("enter x H o y space f e i"), back]
        }
        View::Detail(_) => vec![vec!["↓ ↑ scroll"], search, actions("x H o y space f e i"), BACK.to_vec()],
        View::Issues => vec![vec!["↓ ↑ scroll"], [&search[..], &["r refresh"]].concat(), BACK.to_vec()],
        // `?` here closes help, which `esc back` already says.
        View::Help if matches!(app.input, Input::Bind { .. }) => vec![vec!["esc cancel"]],
        View::Help => {
            // Offered on a row whose key changes, and on `↺`, which puts every key back.
            let row = help_rows().nth(app.help_sel).map_or("", |r| r.0);
            let enter: &[&str] = match row {
                "↺" => &["enter reset keys"],
                _ if bindable(row).is_some() => &["enter change key"],
                _ => &[],
            };
            vec![vec!["↓ ↑ move"], [enter, &search[..]].concat(), vec!["esc back", "q quit"]]
        }
        View::Compare if app.marked_shown < 2 => vec![BACK.to_vec()],
        View::Compare => {
            vec![vec!["↓ ↑ model", "← → 0 $ column", "s sort"], search, actions("enter x H o y f e i"), BACK.to_vec()]
        }
        // On a task's name, enter ranks by it; on a tier's model, it and the others act on the model.
        // On the `from` line, the models it ranks.
        View::Recommend if matches!(app.input, Input::Task(_)) => vec![field_hints(&app.input).to_vec()],
        View::Recommend if matches!(app.input, Input::Cap { .. }) => vec![vec!["enter apply", "esc cancel"]],
        View::Recommend if app.among.is_none() && app.caps => {
            let enter = if crate::view::capped(TIERS[app.cap_col()].0) { vec!["enter price cap"] } else { vec![] };
            vec![vec!["↓ ↑ task", "← → 0 $ tier"], [enter, search].concat(), BACK.to_vec()]
        }
        View::Recommend if app.among.is_some() => {
            vec![vec!["↓ ↑ task", "← → 0 $ which models"], [&["enter pick"], &search[..]].concat(), BACK.to_vec()]
        }
        View::Recommend if app.on_new_task() => {
            vec![vec!["↓ ↑ task"], [&["enter new task"], &search[..]].concat(), BACK.to_vec()]
        }
        View::Recommend if app.current().is_none() => {
            // A tier with no model has nothing for enter either.
            let enter = match (app.task_sel, app.custom_at()) {
                (1.., _) => vec!["d dropdown"],
                (_, Some(_)) => vec!["enter your model", "T edit", "D delete"],
                _ => vec!["enter best per price"],
            };
            vec![vec!["↓ ↑ task", "← → 0 $ tier"], [enter, search].concat(), cap, BACK.to_vec()]
        }
        View::Recommend => {
            let view = [&["d dropdown"], &search[..]].concat();
            vec![vec!["↓ ↑ task", "← → 0 $ tier"], view, actions("enter x H o y space f e i"), cap, BACK.to_vec()]
        }
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
        if word.chars().count() != 1 && !["enter", "esc", "space", "tab", "ctrl+c"].contains(&word) {
            break;
        }
        end += word.len() + 1;
    }
    hint.split_at(end.saturating_sub(1).min(hint.len()))
}

/// The line of keys under a list or in a box, `hint`'s parts between ` · `: muted, with each
/// part's keys in the keys' colour, as the status bar has them.
fn key_line(hint: &str, bound: &BTreeMap<char, char>) -> Line<'static> {
    let mut spans = Vec::new();
    for (i, part) in hint.split(" · ").enumerate() {
        let text = part.trim_start();
        let (key, what) = split_hint(text);
        spans.push(Span::raw(format!("{}{}", if i == 0 { "" } else { " · " }, &part[..part.len() - text.len()])));
        spans.push(Span::styled(bound_keys(key, bound), fg(KEY).add_modifier(BOLD)));
        spans.push(Span::raw(what.to_string()));
    }
    Line::from(spans).style(fg(MUTED))
}

/// The key a click on a hint presses: only a hint with a single key has one.
fn hint_key(hint: &str) -> Option<KeyCode> {
    match split_hint(hint).0 {
        "enter" => Some(KeyCode::Enter),
        "esc" => Some(KeyCode::Esc),
        "space" => Some(KeyCode::Char(' ')),
        "tab" => Some(KeyCode::Tab),
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
const TASK: [Color; 4] = [Color::LightCyan, Color::Green, Color::Blue, Color::LightMagenta];
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
/// The name of a tab that is on: bold in the text colour, its frame telling it from the others.
const TAB_ON: Style = Style::new().add_modifier(BOLD);
/// What follows the name of tab `i`: on mine `+N`, the selected models out of reach it shows too.
fn tab_plus(app: &App, i: usize) -> String {
    if i == MINE && app.marked_out > 0 { format!(" +{}", app.marked_out) } else { String::new() }
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
        put(buf, x, y, text, usize::from(end.saturating_sub(x)), style);
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
            (true, _) => (TAB_ON, fg(KEY).add_modifier(BOLD)),
            (_, true) => (Style::new(), fg(KEY).add_modifier(BOLD)),
            _ => (fg(MUTED), fg(MUTED)),
        };
        // The `+N` in the colour of the ✓ its models have.
        let (plus, len) = (tab_plus(app, i), name.chars().count() as u16);
        put(buf, x + 1, y, &format!(" {name} "), style);
        put(buf, x + 2 + len, y, &format!("{plus} "), fg(MARK).add_modifier(BOLD));
        put(buf, x + 3 + len + plus.len() as u16, y, &format!("{} ", app.key_of(key)), key_style);
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
/// The cursor's column in the table and compare, a fainter grey than its row (`COLUMN_WASH`),
/// and bold where there is no faint fill to have.
const COLUMN: Color = Color::LightMagenta;
/// The bars at the cursor's two ends.
const EDGE: Style = Style::new().fg(ACCENT).bg(CURSOR).add_modifier(BOLD);
/// The `‹` `›` `▲` `▼` for what is scrolled off: a solid cell, as a pill, found at a glance. Yellow, as the
/// notices in the status bar: the accent is the headers' and the cursor's, and is lost among them.
const MORE: Style = Style::new().fg(Color::Black).bg(Color::Yellow).add_modifier(BOLD);
/// The share of the mark colour in a marked row's fill with the terminal's own colours, in
/// percent, the rest being its background: faint, as the row's colours are read on it. A theme
/// has its own (`Palette::wash`).
/// ponytail: text on it reads at 2.2:1 or better, not the 3:1 it has on the background; past
/// that the palettes need retuning (`every_theme_reads_on_its_own_background`).
const WASH: u32 = 12;
/// And of the text colour in the cursor's: the most that every theme reads on and tells from a
/// marked row's fill (`roles_are_told_apart`), about what an editor's cursor line has.
const CURSOR_WASH: u32 = 8;
/// And in the cursor's column: less than the row's, so the row stands out where they cross,
/// and as much as tells it from the background in every theme (`the_column_reads`).
const COLUMN_WASH: u32 = 6;
/// And of a pill's colour in its fill behind a box, faded as the text around it.
const PILL_FADE: u32 = 50;
/// The mark colour in that fill with the terminal's own colours, which do not tell their slot 12.
/// ponytail: one blue for a dark and a light terminal; ask for slot 12 (OSC 4) to follow the rice.
const TERM_MARK: u32 = 0x5c9cff;

/// Swap the terminal colours for the theme's after drawing, so the drawing code keeps naming
/// terminal colours: the 16, default text, and a developer's placeholder (see `dev_color`).
/// Backgrounds with no colour get the theme's; with the terminal's own colours they stay transparent.
/// `term_bg` is the terminal's background and `slots` its 16, if it told (`terminal_colors`).
fn recolor(buf: &mut Buffer, palette: Option<&Palette>, term_bg: Option<u32>, slots: Option<&[u32; 16]>) {
    let bg = palette.map(|p| Color::from_u32(p.bg));
    for cell in &mut buf.content {
        // A pill behind a box (`Modifier::DIM`) fades as the rest does, which faint does to no
        // background: its fill half mixed into the background, else, with no colours to mix,
        // its colour as text.
        if cell.fg == Color::Black && cell.bg != Color::Reset && cell.modifier.contains(Modifier::DIM) {
            let fill = match (palette, term_bg, slots) {
                (Some(p), ..) => Some((contrasting(cell.bg, p), p.bg, Color::from_u32(p.bg))),
                (None, Some(b), Some(s)) => {
                    ansi(resolve(cell.bg, None)).map(|i| (Color::from_u32(s[i]), b, Color::Black))
                }
                _ => None,
            };
            if let Some((c, bg, text)) = fill {
                (cell.fg, cell.bg) = (text, wash(c, bg, PILL_FADE));
                continue;
            }
            // A selected row's solid fill (`solid`) is black on the mark too, and stays.
            if term_bg.is_some() || cell.bg != MARK {
                (cell.fg, cell.bg) = (cell.bg, Color::Reset);
            }
        }
        // Nor a faint column: its cells are bold there.
        if palette.is_none() && term_bg.is_none() && cell.bg == COLUMN {
            cell.bg = Color::Reset;
            cell.modifier.insert(BOLD);
        }
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
            (c, None, Some(b)) if cell.fg != Color::Black => match grey(c) {
                Some(share) => wash(Color::from_u32(if lum(b) < 128.0 { 0xffffff } else { 0 }), b, share),
                None => wash(Color::from_u32(TERM_MARK), b, WASH),
            },
            // A pill in a harness's colour (`chip`) is in the one its name has off it.
            (c, None, _) => resolve(c, None),
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
    match grey(c) {
        Some(share) => wash(Color::from_u32(p.text), p.bg, share),
        None => wash(resolve(c, Some(p)), p.bg, p.wash),
    }
}

/// The share of the text colour in the cursor's fills, which are grey: its row's and its column's.
fn grey(c: Color) -> Option<u32> {
    match c {
        CURSOR => Some(CURSOR_WASH),
        COLUMN => Some(COLUMN_WASH),
        _ => None,
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

/// `Buffer::set_stringn`, a few times quicker for printable ASCII and a glyph of one cell, most of
/// what is drawn: each char is one cell, with no graphemes to find.
fn put(buf: &mut Buffer, x: u16, y: u16, s: impl AsRef<str>, max: usize, style: impl Into<Style>) -> (u16, u16) {
    let (s, style) = (s.as_ref(), style.into());
    let room = usize::from(buf.area.right().saturating_sub(x)).min(max);
    if !plain(s) {
        // A glyph of one cell, as a mark or a `│` is.
        let mut chars = s.chars();
        if let (Some(c), None) = (chars.next(), chars.next())
            && !c.is_control()
            && s.cell_width() == 1
        {
            if room == 0 {
                return (x, y);
            }
            buf[(x, y)].set_symbol(s).set_style(style);
            return (x + 1, y);
        }
        return buf.set_stringn(x, y, s, max, style);
    }
    let n = room.min(s.len());
    if n > 0 {
        let i = buf.index_of(x, y);
        for (c, b) in buf.content[i..i + n].iter_mut().zip(s.bytes()) {
            c.set_char(char::from(b)).set_style(style);
        }
    }
    (x + n as u16, y)
}

/// Each cell of `r` in `buf`, a row at a time: several times quicker than cell by cell, as
/// `Buffer::set_style` and `Clear` go.
fn cells(buf: &mut Buffer, r: Rect, mut f: impl FnMut(&mut Cell)) {
    let r = r.intersection(buf.area);
    if r.is_empty() {
        return;
    }
    for y in r.top()..r.bottom() {
        let i = buf.index_of(r.x, y);
        buf.content[i..i + usize::from(r.width)].iter_mut().for_each(&mut f);
    }
}

/// `r` emptied for a box drawn over it, and a glyph of two cells its left edge cuts blanked: a
/// frame never sends the cell after such a glyph, so the terminal would show the glyph's right
/// half where the box's left border is.
fn clear(buf: &mut Buffer, r: Rect) {
    cells(buf, r, Cell::reset);
    let r = r.intersection(buf.area);
    if r.x > buf.area.x {
        for y in r.top()..r.bottom() {
            let c = &mut buf[(r.x - 1, y)];
            if c.cell_width() > 1 {
                c.set_symbol(" ");
            }
        }
    }
}

/// `Buffer::set_style`, quicker (`cells`).
fn restyle(buf: &mut Buffer, r: Rect, style: impl Into<Style>) {
    let style = style.into();
    cells(buf, r, |c| {
        c.set_style(style);
    });
}

/// `s` right-aligned in `w` cells from `x`, as `put` draws `format!("{s:>w$}")`, with no string
/// made for it.
fn put_right(buf: &mut Buffer, x: u16, y: u16, s: &str, w: u16, style: Style) {
    let pad = w.saturating_sub(s.chars().count() as u16);
    cells(buf, Rect::new(x, y, pad, 1), |c| {
        c.set_char(' ').set_style(style);
    });
    put(buf, x + pad, y, s, usize::from(w - pad), style);
}

/// Printable ASCII, which `put` and `put_line` draw a char to a cell.
fn plain(s: &str) -> bool {
    s.bytes().all(|b| (b' '..=b'~').contains(&b))
}

/// `line` in the row `area` as `Line::render` draws it, its plain spans the quick way (`put`).
fn put_line(buf: &mut Buffer, line: &Line, area: Rect) {
    let at = area.intersection(buf.area);
    let width = line.width();
    // Cut short, it is cut as ratatui cuts it.
    if at.is_empty() || width == 0 || width > usize::from(at.width) {
        return line.render(area, buf);
    }
    let at = Rect { height: 1, ..at };
    restyle(buf, at, line.style);
    let mut x = at.x
        + match line.alignment {
            Some(Alignment::Center) => (at.width - width as u16) / 2,
            Some(Alignment::Right) => at.width - width as u16,
            _ => 0,
        };
    for span in &line.spans {
        let w = span.width() as u16;
        if plain(&span.content) {
            put(buf, x, at.y, &span.content, w.into(), span.style);
        } else {
            span.render(Rect { x, width: at.right() - x, ..at }, buf);
        }
        x += w;
    }
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
    {
        // The refresh state is always in view: under way, failed, or how old the data is.
        let age = format!("{} ", data_age(&app.data));
        let version = match app.data.update() {
            Some(new) => Line::from(format!(" modelcmp v{new} available: {} upgrades ", app.key_of('U')))
                .style(fg(Color::Yellow)),
            None => Line::from(concat!(" modelcmp v", env!("CARGO_PKG_VERSION"), " ")).style(fg(MUTED)),
        };
        let source = " models.dev + Epoch AI · ";
        // Where the count and what it waits for would run over the version, the count goes alone.
        let room = usize::from(body.width).saturating_sub(2 + version.width() + source.chars().count());
        let refreshing = |p: &str| format!("⟳ refreshing {p}{}", if p.is_empty() { "" } else { " " });
        let (state, color) = match (app.refreshing, app.refresh_failed, stale(app)) {
            (true, ..) if refreshing(&app.progress).chars().count() > room => {
                (refreshing(app.progress.split(',').next().unwrap_or_default()), Color::Yellow)
            }
            (true, ..) => (refreshing(&app.progress), Color::Yellow),
            (_, true, _) => (format!("refresh failed · {age}"), BAD),
            // Until a refresh without any: `!` lists them.
            (_, _, old) if app.data.warning.is_some() => {
                (format!("⚠ refresh had issues · {age}"), if old { BAD } else { Color::Yellow })
            }
            (_, _, true) => (age, BAD),
            _ => (age, MUTED),
        };
        // A state still too long would run over the version: the source goes.
        let named = state.chars().count() <= room;
        // Last on the bottom border, before its corner.
        let end = body.right().saturating_sub(1);
        app.issues_at =
            state.starts_with('⚠').then(|| end.saturating_sub(Line::from(state.as_str()).width() as u16)..end);
        // Via by the download's size alone says so, as its order is not the column's text.
        let size = if app.by_size() { " size" } else { "" };
        let sort = format!(" {} by {}{size} ", if app.descending { "▼" } else { "▲" }, app.col_name(app.sort_col));
        let sort_w = sort.chars().count();
        let frame = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(fg(MUTED))
            .title_top(Line::from(sort).style(fg(MUTED)).right_aligned())
            .title_bottom(version)
            .title_bottom(
                Line::from_iter(
                    named.then(|| Span::styled(source, fg(MUTED))).into_iter().chain([Span::styled(state, fg(color))]),
                )
                .right_aligned(),
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
        put(buf, body.x + x as u16, body.y, about, end.saturating_sub(x), fg(MUTED));
        let (right, above, below) = table(buf, inner, app);
        if right && inner.height > 0 {
            // Columns cut off on the right: `l` scrolls to them.
            put(buf, body.right() - 1, inner.y, "›", 1, MORE);
        }
        if inner.height > 1 {
            // The rule under the header runs into the frame.
            put(buf, body.x, inner.y + 1, "├", 1, fg(MUTED));
            put(buf, body.right() - 1, inner.y + 1, "┤", 1, fg(MUTED));
        }
        if inner.height > head {
            vmarks(buf, body.x, inner.y + head, inner.bottom() - 1, above, below);
        }
    }
    let buf = f.buffer_mut();
    // Help and the issues show over the panel they were opened from, as every box over what is
    // behind it.
    if matches!(app.view, View::Help | View::Issues) {
        app.under_help(|app| {
            panel(app, buf, area, body);
        });
    }
    let text = panel(app, buf, area, body);
    // Where the text cursor goes: in the status bar's prompt, or on an entry written in a list.
    // After the panel's lines, as finding its search's match says so there.
    let mut cursor = status(buf, bar, app).map(|x| (x, bar.y));
    if let Some(at) = text {
        cursor = at;
    }
    // `q`, `U`, `D`, `X` and `↺` ask first: the same box, confirmed by the same key again, each
    // as it is bound now.
    let key = |k: char| app.key_of(k).to_string();
    let ask = match app.input {
        Input::Quit => Some((key('q'), "quit?".to_string())),
        Input::Upgrade => app.data.update().map(|new| (key('U'), format!("upgrade to v{new}?"))),
        Input::Unfavorite => Some((key('D'), "unfavorite every model?".to_string())),
        Input::DeleteTask(ref t) => Some((key('D'), format!("delete {t}?"))),
        Input::Reset => Some(("enter".to_string(), "put every key back to its default?".to_string())),
        Input::Unexclude => Some((
            key('X'),
            if app.filters_exclude() {
                "unexclude every model, hidden ones included?"
            } else {
                "unexclude every model?"
            }
            .to_string(),
        )),
        _ => None,
    };
    if let Some((key, title)) = ask {
        let key = Span::styled(key, fg(KEY).add_modifier(BOLD));
        let lines = vec![Line::from(vec![key, Span::raw(" confirms · any other key cancels")])];
        overlay(buf, body, &title, lines, &mut 0, (0, 0, 0..0));
    }
    let chooser = chooser(app, area);
    if let (Some((within, lines)), Input::Choose { title, kind, items, list }) = (chooser, &mut app.input) {
        let rect = overlay_rect(within, title, &lines);
        let rows = items.len();
        // f's grid has its tiers' heading over the rows, and its cursor on a box, drawn with it;
        // a row being written has no box, so the cursor is the row's, as in every list.
        // ponytail: the heading scrolls off with the rows in a box too short for them all; pin
        // it if a dozen tasks of your own on a short terminal turn out to be a thing.
        let grid = *kind == Kind::Fav;
        let head = usize::from(grid);
        let none = list.no_match(items.iter().map(|e| e.0.as_str()), false);
        let (sel, top) = (&list.sel, &mut list.top);
        let shown = usize::from(rect.height.saturating_sub(2));
        // At the last entry every line to the end, so the key hint below them shows too, unless
        // that would scroll the cursor's line off. And at an entry being written, whose keys or
        // error that line has.
        // ponytail: an entry more than a box's height from the end keeps it out of view; the
        // error on the line under the entry if it bites.
        let from = if *sel + 1 >= rows || list.edit.is_some() { lines.len().saturating_sub(shown) } else { *top };
        let mut scroll = list_top(from, *sel + head, shown) as u16;
        let ends = (head, lines.len() - rows - head, 0..0);
        // The cells the entry being written was drawn in: a task in f's grid the box's, a folder
        // what the screen has.
        let room = if grid { fav_room(&lines, edit_room(within)) } else { edit_room(within) };
        let (_, above, below) = overlay(buf, within, title, lines, &mut scroll, ends);
        *top = usize::from(scroll);
        // On the bottom border, as a dropdown says it, so it shows however the list scrolls.
        if none {
            put(buf, rect.x + 2, rect.bottom() - 1, " no match ", usize::from(rect.width / 2), fg(MUTED));
        }
        if rows > 0 {
            // The cursor runs through the box's border, as in the table, and the marks go over
            // its bar, as there.
            let y = (rect.y + 1 + (*sel + head) as u16 - scroll).min(rect.bottom() - 1);
            if !grid || list.edit.is_some() {
                cursor_ends(buf, rect.x, rect.right() - 1, y);
            }
            // An entry being written has the text cursor where `edit_line` puts it.
            if let Some(e) = &list.edit {
                let (_, before) = edit_line(e, Span::raw(edit_prefix(e)), Style::new(), room);
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

/// The open panel's box over the table in `body`, and the text cursor recommend has on a cap
/// being written: `None` leaves the status bar's cursor, `Some(None)` hides it, the cap's line
/// being scrolled off.
fn panel(app: &mut App, buf: &mut Buffer, area: Rect, body: Rect) -> Option<Option<(u16, u16)>> {
    let mut cursor = None;
    // Where compare and recommend put each model, in their lines, and recommend the cursor's task.
    let (mut spots, mut block, mut pin) = (vec![], None, 0..0);
    // The line of a panel's search `n` `N` are on, which has the cursor.
    let mut hit_line = None;
    // Recommend's text cursor, on the header of a tier whose cap is written.
    let mut text_at = None;
    let lines = match app.view {
        View::Table => None,
        View::Help => {
            let (lines, rows) = help(app);
            // The cursor's row in view, and at the first and the last row the lines above and under it.
            let at = rows[app.help_sel.min(rows.len() - 1)];
            let shown = body.height.saturating_sub(2) as usize;
            app.scroll = match app.help_sel {
                0 => 0,
                s if s + 1 == rows.len() => lines.len().saturating_sub(shown),
                _ => list_top(app.scroll as usize, at, shown),
            } as u16;
            hit_line = Some(at);
            spots.extend(rows.iter().enumerate().map(|(i, &l)| Spot {
                lines: l..l + 1,
                x: 0..usize::MAX,
                at: Stop::Help(i),
            }));
            Some(("keys".to_string(), lines))
        }
        View::Recommend => {
            let lines;
            (lines, block, pin, text_at) = recommend(app, (area.width as usize).saturating_sub(4), &mut spots);
            // Its cursor goes to a match, the task or model `n` `N` are on, and those alone are underlined.
            Some(("recommend".to_string(), lines))
        }
        View::Issues => {
            let issues = app.issues();
            let said = issues.as_deref().unwrap_or("none");
            let words = |w: &str| w.split(' ').map(|s| Line::from(s.to_string())).collect();
            let width = (area.width as usize).saturating_sub(4);
            let lines =
                said.split(data::SEP).flat_map(|w| wrapped(vec![Span::raw("• ")], words(w), &Span::raw(" "), width));
            let (lines, hits) = lit_lines(lines.collect(), &app.overlay_query);
            hit_line = to_hit(app, &hits);
            Some(("issues".into(), lines))
        }
        View::Detail(_) => {
            let get = |m| (app.download(m), app.size(m));
            let lines = app.current().map(|m| detail(m, &app.store, app.any_available(), get(m)));
            lines.map(|(title, lines)| {
                let (lines, hits) = lit_lines(lines, &app.overlay_query);
                hit_line = to_hit(app, &hits);
                (title, lines)
            })
        }
        View::Compare if app.marked_shown < 2 => {
            // Nothing to find, which a search is told.
            hit_line = to_hit(app, &[]);
            let key = |k: String| Span::styled(k, fg(KEY).add_modifier(BOLD));
            let n = app.marked_shown;
            let lines = vec![
                Line::from(format!("compare needs 2 or more selected models, you have {n}")),
                Line::from(""),
                Line::from(vec![key("esc".into()), Span::raw(" back to the table, then")]),
                Line::from(vec![key("space".into()), Span::raw(" selects the model under the cursor, or")]),
                Line::from(vec![
                    key(app.key_of('v').to_string()),
                    Span::raw(" or "),
                    key("shift+click".into()),
                    Span::raw(" highlights a range, and"),
                ]),
                Line::from(vec![key(app.key_of('C').to_string()), Span::raw(" compares them")]),
            ];
            Some(("compare".into(), lines))
        }
        View::Compare => {
            // `n` `N` start the view at the column whose label they found.
            let (n, jump) = app.hit;
            let searched = !app.overlay_query.is_empty();
            let sort = app.compare_sort.as_ref().map(|(l, d)| (l.as_str(), *d));
            let c = compare(
                &app.marked_models(),
                (app.any_available(), &|m| app.download(m), &app.store),
                (app.compare_sel, (app.compare_col, app.compare_x), (jump && searched).then_some(n)),
                (area.width.saturating_sub(4) as usize, sort),
                &app.overlay_query,
                |m| app.muted(m),
                &mut spots,
            );
            (app.compare_col, app.compare_x, app.compare_shown) = (c.col, c.first, c.shown);
            if searched {
                app.hit = (n.rem_euclid(c.hits.max(1) as isize), false);
                if jump {
                    app.status = match c.hits {
                        0 => format!("no column matches {}", app.overlay_query),
                        k => format!("column {} of {k}", app.hit.0 + 1),
                    };
                }
            }
            // The cursor's model in view, and at the first and the last the lines above and under it.
            let shown = body.height.saturating_sub(2) as usize;
            app.scroll = match app.compare_sel {
                0 => list_top(0, c.at, shown),
                s if s + 1 >= app.marked_shown => c.lines.len().saturating_sub(shown),
                _ => list_top(app.scroll as usize, c.at, shown),
            } as u16;
            hit_line = Some(c.at);
            Some(("compare".into(), c.lines))
        }
    };
    app.spots.clear();
    app.panel = lines.as_ref().map(|(title, lines)| overlay_rect(body, title, lines));
    if let Some((title, lines)) = lines {
        if app.view == View::Recommend {
            // Keep the cursor's task row in view, over the lines pinned under the grid. A box too
            // short to pin them scrolls them with the rows, and then keeps the line being
            // written in view.
            let rows = body.height.saturating_sub(2) as usize;
            let block = match text_at {
                Some((l, _)) if pinned(lines.len(), rows, &pin) == 0 && pin.contains(&l) => Some(l..l + 1),
                _ => block,
            };
            if let Some(std::ops::Range { start, end }) = block {
                let under = pinned(lines.len(), rows, &pin);
                let (shown, len) = (rows - under, lines.len() - under);
                // No cursor goes to the lines above the first row and under the last: they
                // show with it, where they fit.
                let (first, last) = (app.task_cur == 0, app.on_new_task());
                let (start, end) = (
                    if first && end <= shown { 0 } else { start },
                    if last && len - start <= shown { len } else { end },
                );
                let lo = end.saturating_sub(shown).min(start);
                app.scroll = (app.scroll as usize).clamp(lo, start) as u16;
            }
        }
        let under = pinned(lines.len(), body.height.saturating_sub(2) as usize, &pin);
        let (text, above, below) = overlay(buf, body, &title, lines, &mut app.scroll, (0, 0, pin.clone()));
        if let Some((l, x)) = text_at {
            // A line pinned under the grid is drawn after the rows shown.
            let y = match l.checked_sub(pin.start) {
                Some(k) if under > 0 && k < under => Some(text.height as usize - under + k),
                _ => l.checked_sub(app.scroll as usize).filter(|&y| y < text.height as usize),
            };
            cursor = Some(y.map(|y| ((text.x + x as u16).min(text.right().saturating_sub(1)), text.y + y as u16)));
        }
        // The cursor runs through the box's border, as in a list, and the marks go over its bar.
        if let Some(y) = hit_line.and_then(|l| l.checked_sub(app.scroll as usize)).filter(|&y| y < text.height as usize)
        {
            cursor_ends(buf, text.x - 2, text.right() + 1, text.y + y as u16);
            vmarks(buf, text.x - 2, text.y, text.bottom() - 1, above, below);
        }
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
    cursor
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
    /// The first column right of Model shown (0 is Dev, the last Notes), for `App::hscroll`.
    first: usize,
    /// Columns cut off on the right; `‹` after Model and `›` on the frame say where to scroll.
    more: bool,
    /// The cursor index a click on `›` pages to: the first column right of the view or cut short,
    /// or the one after it when it starts the view.
    next: Option<usize>,
    /// Where a `│` parts two groups of columns (`GROUPS`): after Model when scrolled, and before
    /// each group that starts right of it.
    seps: Vec<u16>,
}

/// Header of the column at cursor index `col`, its own name whatever its dropdown picked, and
/// the `*` after it while a benchmark picked there is shown in its place. Apart, as it is asked
/// of every header on every frame.
fn col_head(app: &App, col: usize) -> (&'static str, &'static str) {
    (base_col_name(col), if app.col_bench(col).is_some() { "*" } else { "" })
}

fn layout(width: u16, app: &App) -> Layout {
    let ms = &app.data.models;
    // Headers need room for the sort arrow, and a dropdown's for its " ▾".
    let widths: [u16; COLS.len()] = std::array::from_fn(|i| {
        let (name, mark) = col_head(app, i + 2);
        let head = name.chars().count() + mark.len() + 1 + if app.menu(i + 2) { 2 } else { 0 };
        head.max(app.widths[i]) as u16
    });
    let dev_w = ms.iter().map(|m| m.developer.chars().count()).max().unwrap_or(0).clamp(6, 12) as u16;
    // A shown model you have no access to says so in Via.
    let out = app.rows.iter().any(|&r| !app.accessible(&ms[r]));
    // As drawn: the short names, a space between.
    let listed = |m: &Model| m.via.iter().map(|v| short(v).len() + 1).sum::<usize>().saturating_sub(1);
    // A shown one `x` can download says that after them, or alone when out of reach.
    let got = app.rows.iter().filter_map(|&r| {
        let before = Some(listed(&ms[r])).filter(|&w| w > 0 && app.accessible(&ms[r])).map_or(0, |w| w + 1);
        Some(before + app.download(&ms[r])?.chars().count())
    });
    let via_w =
        ms.iter().map(listed).chain(got).chain(out.then_some(OUT_OF_REACH.len())).max().unwrap_or(0).max(6) as u16;
    // As wide as drawn: a CJK character or an emoji takes two cells. Of the models the data has,
    // found from the few notes rather than from every model.
    let noted = app.store.notes.iter().filter(|(k, _)| ms.iter().any(|m| m.key == **k));
    let notes_w = noted.map(|(_, s)| Span::raw(s).width()).max().unwrap_or(0);
    let notes_w = notes_w.clamp(6, 40) as u16;
    // Of the models listed, not of all: one long name filtered out leaves no gap before Dev.
    let longest = app.rows.iter().map(|&r| ms[r].name.chars().count()).max().unwrap_or(0) as u16;
    // Row numbers as wide as the last one, a space, then the checkbox, the ☆ and the ✗ box,
    // each with a spare cell: some terminals draw them two cells wide, and the
    // spare keeps that off the neighbour.
    let name_x = app.rows.len().max(1).to_string().len() as u16 + 7;
    // The columns right of Model scroll sideways, Dev the first of them: only as far as it takes
    // to show the selected one, keeping the last position otherwise. Model stays put.
    let ws: Vec<u16> = widths.into_iter().chain([via_w, notes_w]).collect();
    // A column left out takes no room at all.
    let ws: Vec<u16> = ws.into_iter().enumerate().map(|(k, w)| if hidden(k + 2) { 0 } else { w }).collect();
    // By cursor index less one: Dev, the numbers, Via and Notes.
    let ws: Vec<u16> = std::iter::once(if hidden(1) { 0 } else { dev_w }).chain(ws).collect();
    // The leftmost shown: Dev, or the next one when it is left out.
    let lead = ws.iter().position(|&w| w > 0).unwrap_or(0);
    // A column starting a group has a `│` in its gap, one cell wider; so does the first shown
    // when scrolled, parting it from Model. Dev and Released, in Model's group, have neither.
    let starts = group_starts();
    let sep = |k: usize| u16::from(starts.contains(&(k + 1)));
    // Model is not cut to make room for every column: those right of it scroll, and `›` says
    // so. Only for one, the widest of Dev and the numbers, so a long name never leaves the
    // table with none.
    let keep = ws[..=COLS.len()].iter().max().map_or(0, |w| w + GAP + 1);
    let name_w = longest.min(width.saturating_sub(name_x + GAP + keep)).max(NAME_MIN);
    let mut x = name_x + name_w + GAP;
    let room = width.saturating_sub(x) + GAP;
    let first = match app.col.checked_sub(1) {
        Some(s) => {
            // The leftmost first column that still shows column `s`. Each column brought in on
            // the left costs its width and gap, and the one it displaces as first its `│`;
            // hidden ones cost nothing and are never first.
            let reach = |s: usize| {
                let (mut lo, mut used) = (s, ws[s] + GAP);
                for k in (0..s).rev().filter(|&k| ws[k] > 0) {
                    let cost = ws[k] + GAP + sep(lo);
                    // Plus the first's own `│`, unless it is the leftmost.
                    if used + cost + u16::from(k > lead) > room {
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
    // The cut column, or the one after it when it already starts the view, as a cursor index.
    let page = |k: usize| if k > first { Some(k + 1) } else { (k + 1..ws.len()).find(|&j| ws[j] > 0).map(|j| j + 1) };
    let mut next = None;
    let (mut started, mut dev_w) = (false, 0);
    for (k, &w) in ws.iter().enumerate().skip(first).filter(|(_, w)| **w > 0) {
        let part = if started { sep(k) == 1 } else { k > lead };
        x += u16::from(part);
        // Dev, Via and Notes are cut to the room left rather than dropped, so none of it is
        // blank. Not a number: cut short it reads as another number.
        let w = if (1..=COLS.len()).contains(&k) { w } else { w.min(width.saturating_sub(x)) };
        started = true;
        if w == 0 || x + w > width {
            more = true;
            next = next.or_else(|| page(k));
            break;
        }
        more = w < ws[k];
        if more {
            next = next.or_else(|| page(k));
        }
        if part {
            seps.push(x - 2);
        }
        match k.checked_sub(1).map(|n| (n, n.checked_sub(COLS.len()))) {
            None => dev_w = w,
            Some((_, Some(t))) => tail[t] = Some((x, w)),
            Some((n, None)) => cols.push((n, x, w)),
        }
        x += w + GAP;
    }
    Layout { name_x, name_w, dev_w, cols, via: tail[0], notes: tail[1], first, more, next, seps }
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
    let Layout { name_x, name_w, dev_w, cols, via, notes, first, more, next, seps } = layout(area.width, app);
    app.hscroll = first;
    let arrow = |i: usize| match i == app.sort_col {
        true if app.descending => "▼",
        true => "▲",
        false => "",
    };
    // The column under the cursor: its header reversed, its cells on a faint fill (`COLUMN`)
    // and a thick rule under it, as VisiData shows its current column; the values keep their
    // colours.
    let header = |i: usize| {
        let style = fg(ACCENT).add_modifier(BOLD);
        if i == app.col { style.add_modifier(Modifier::REVERSED) } else { style }
    };
    // The marks: the checkbox, then the ☆, then the ✗ box.
    let num_w = (name_x - 7) as usize;
    let (box_x, star_x) = (area.x + num_w as u16 + 1, area.x + num_w as u16 + 3);
    let ex_x = area.x + num_w as u16 + 5;
    let (y, name_x, dev_x) = (area.y, area.x + name_x, area.x + name_x + name_w + GAP);
    // In a terminal too narrow for both they are cut at the frame, not drawn over its border.
    let room = |x: u16, w: u16| usize::from(w.min(area.right().saturating_sub(x)));
    let (nw, dw) = (room(name_x, name_w), room(dev_x, dev_w));
    put(buf, area.x, y, format!("{:>num_w$}", "#"), num_w, fg(MUTED));
    put(buf, name_x, y, format!("{:<nw$}", format!("Model{}", arrow(0))), nw, header(0));
    put(buf, dev_x, y, format!("{:<dw$}", format!("Dev{} ▾", arrow(1))), dw, header(1));
    if (0..first).any(|k| !hidden(k + 1)) && name_x + name_w < area.right() {
        // Columns scrolled off to the left, Dev the first.
        put(buf, name_x + name_w, y, "‹", 1, MORE);
    }
    for &(i, x, w) in &cols {
        let (name, mark) = col_head(app, i + 2);
        let text = format!("{}{name}{mark}{}", arrow(i + 2), if app.menu(i + 2) { " ▾" } else { "" });
        put(buf, area.x + x, y, format!("{text:>w$}", w = w as usize), w as usize, header(i + 2));
    }
    if let Some((x, w)) = via {
        put(buf, area.x + x, y, format!("Via{} ▾", arrow(VIA)), w as usize, header(VIA));
    }
    if let Some((x, w)) = notes {
        put(buf, area.x + x, y, format!("{}{}", app.col_name(NOTES), arrow(NOTES)), w as usize, header(NOTES));
    }

    for &x in &seps {
        put(buf, area.x + x, y, "│", 1, fg(MUTED));
    }
    let span = |c: usize| match c {
        0 => Some((name_x, name_w)),
        1 => Some((dev_x, dev_w)),
        VIA => via.map(|(x, w)| (area.x + x, w)),
        NOTES => notes.map(|(x, w)| (area.x + x, w)),
        c => cols.iter().find(|&&(i, ..)| i + 2 == c).map(|&(_, x, w)| (area.x + x, w)),
    };
    // A header whose dropdown narrows the table, by an entry picked or left out, has its ▾ in
    // the yellow the status bar says the filter in. One showing a benchmark picked in it has it
    // yellow too, though no model is hidden and the status bar does not say it.
    let picked = |c: usize| match c {
        1 => !app.store.dev_pick.is_empty(),
        VIA => !app.store.via_pick.is_empty(),
        PRICE => app.price_level().is_some(),
        _ => app.col_bench(c).is_some(),
    };
    for (x, w) in (1..NCOLS).filter(|&c| picked(c) || !app.left_out(c).is_empty()).filter_map(span) {
        for x in x..(x + w).min(area.right()) {
            if buf[(x, y)].symbol() == "▾" {
                buf[(x, y)].set_fg(Color::Yellow);
            }
        }
    }
    let cur = span(app.col);
    // A rule parts the header from the rows, crossing the group lines.
    if area.height > 1 {
        put(buf, area.x, y + 1, "─".repeat(area.width as usize), area.width as usize, fg(MUTED));
        for &x in &seps {
            put(buf, area.x + x, y + 1, "┼", 1, fg(MUTED));
        }
        if let Some((x, w)) = cur {
            put(buf, x, y + 1, "━".repeat(w as usize), w as usize, fg(ACCENT));
        }
    }

    let head = head(app);
    if head > 2 && area.height > 2 {
        put(buf, name_x, y + 2, NO_ACCESS, room(name_x, u16::MAX), fg(Color::Yellow));
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
        restyle(buf, Rect { y, height: 1, ..area }.outer(Margin::new(1, 0)).intersection(buf.area), base);
        if on && area.x > 0 {
            cursor_ends(buf, area.x - 1, area.right(), y);
        }
        put(buf, area.x, y, format!("{:>num_w$}", k + 1), num_w, tint(MUTED));
        // Off, a mark is its own glyph in grey, as the ☆ is the ★'s, so it says what a click on
        // it does. Where colours are off (the reverse-video cursor, a solid fill) a grey one
        // would read as on, so there off is a shape of its own.
        let (unmarked, included) = if faint { ("✓", "✗") } else { ("☐", "·") };
        match marked {
            // Bold as well as blue: on a solid fill, which keeps colours off, that is all the
            // mark has left to show itself with.
            true => put(buf, box_x, y, "✓", 1, tint(MARK).add_modifier(BOLD)),
            false => put(buf, box_x, y, unmarked, 1, tint(MUTED)),
        };
        if app.starred(&m.key) {
            // In the colour of the task at hand, as its name in recommend; gold with no task, as the ★
            // then stands for any of them. Bold so it stands out as much as the ✓.
            let star = tint(app.task_at_hand().map_or(STAR, |t| task_color(t.name)));
            // The gap after it takes the colour too: a font may draw the ★ wider than its cell,
            // and a terminal paints what hangs over in the next cell's colour.
            put(buf, star_x, y, "★ ", gap(star_x), star.add_modifier(BOLD));
        } else {
            put(buf, star_x, y, "☆", 1, tint(MUTED));
        }
        // A marked model's name is bold, as the ✓, and takes no colour: the fill says the row is
        // marked, and under the cursor the ✓ and the bold do. Out of reach, its Via says so.
        let name = if marked { text.add_modifier(BOLD) } else { text };
        // A cut name says so: "Claude Sonnet 5…" is not "Claude Sonnet 5.5" misread as "5.".
        put(buf, name_x, y, truncate(&m.name, nw.max(1)), nw, name);
        put(buf, dev_x, y, &m.developer, dw, soft(dev_color(&m.developer)));
        for &(i, x, w) in &cols {
            let Some(v) = app.vals[r][i] else {
                put(buf, area.x + x + w - 1, y, "-", 1, tint(MUTED));
                continue;
            };
            let listed = app.listed[r];
            let style = match ext[i] {
                // A list price is not one you'd pay: no level, no best or worst.
                _ if listed && on_price(i) => tint(MUTED).add_modifier(Modifier::ITALIC),
                // A score that is not the task's own, an estimate or the overall in its place: grey.
                _ if app.estimated(r, i) => tint(MUTED),
                // The blended price is coloured by level, so its colour says the same thing on every screen.
                _ if i + 2 == PRICE => soft(LEVEL[level(v)]),
                // Free is green and no more, in $in, $cache and $out as in Price: every free
                // model ties for the best, and a column of them in bold says nothing of any one.
                _ if COLS[i].lower_better && v == 0.0 => soft(GOOD),
                // Nor is the largest context bold: it is a size many models share, as free is.
                Some((best, _)) if !dim && v == best && COLS[i].id == "ctx" => tint(GOOD),
                Some((best, _)) if !dim && v == best => tint(GOOD).add_modifier(BOLD),
                Some((_, worst)) if !dim && v == worst => tint(BAD),
                _ => text,
            };
            put_right(buf, area.x + x, y, &shown(i, v, listed, app.col_bench(i + crate::app::TEXT)), w, style);
        }
        if let Some((x, w)) = via {
            let (start, end) = (area.x + x, area.x + x + w);
            let (mut x, get) = (start, app.download(m));
            if reach {
                // The harness it runs on (`H`) a chip in its colour, so a model with one shows it.
                let runs = runs_on(&app.store, m);
                for (j, h) in m.via.iter().enumerate() {
                    x += u16::from(j > 0);
                    let c = via_color(&app.store.via_not, h);
                    let style = if runs == Some(h.as_str()) { chip(if dim { MUTED } else { c }) } else { soft(c) };
                    x = put(buf, x, y, short(h), end.saturating_sub(x) as usize, style).0;
                }
            } else if get.is_none() {
                put(buf, x, y, OUT_OF_REACH, w as usize, tint(MUTED).add_modifier(Modifier::ITALIC));
            }
            // The download `x` offers, after the harnesses or alone: grey when it is too big for
            // your machine's memory, and in the text's colour when it fits, on a muted row too.
            if let Some(get) = get {
                x += u16::from(x > start);
                let style = match app.fits_here(m) {
                    Some(false) => tint(MUTED),
                    Some(true) if !excluded => base,
                    _ => text,
                };
                put(buf, x, y, get, end.saturating_sub(x) as usize, style);
            }
        }
        let note = app.store.note(&m.key).unwrap_or("");
        if let Some((x, w)) = notes {
            put(buf, area.x + x, y, note, w as usize, text.add_modifier(Modifier::ITALIC));
        }
        if excluded {
            put(buf, ex_x, y, "✗ ", gap(ex_x), tint(BAD).add_modifier(BOLD));
        } else {
            put(buf, ex_x, y, included, 1, tint(MUTED));
        }
        for &x in &seps {
            put(buf, area.x + x, y, "│", 1, tint(MUTED));
        }
        // What the search matched, underlined in bold; Notes may be scrolled off. On a solid
        // fill the yellow is behind the hit, as yellow text would not read there.
        // ponytail: a char is taken as one cell; wide chars would shift the underline.
        // Via is drawn as its harnesses' short names, so one a hit touches, in either of its
        // names (`via_text`), is lit whole; out of reach, it is not.
        if !app.query.is_empty()
            && let Some(mut hits) = hits(&app.query, [&m.name, &m.developer, &via_text(m), note], app.typos)
        {
            let (mut from, mut at) = (0, 0);
            hits[2] = (m.via.iter())
                .filter_map(|h| {
                    let (both, cell) = (from..from + via_names(h).len(), at..at + short(h).len());
                    (from, at) = (both.end + 2, cell.end + 1);
                    hits[2].iter().any(|r| r.start < both.end && r.end > both.start).then_some(cell)
                })
                .collect();
            let style = if solid { base.bg(MATCH) } else { tint(MATCH) }.add_modifier(BOLD | Modifier::UNDERLINED);
            let [via, note] = [via.filter(|_| reach), notes].map(|c| c.map(|(x, w)| (area.x + x, w as usize)));
            for (field, ranges) in [Some((name_x, nw)), Some((dev_x, dw)), via, note].into_iter().zip(hits) {
                let Some((x, w)) = field else { continue };
                for r in ranges.into_iter().map(|r| r.start.min(w)..r.end.min(w)) {
                    restyle(buf, Rect::new(x + r.start as u16, y, r.len() as u16, 1), style);
                }
            }
        }
    }
    if let Some((x, w)) = cur {
        let drawn = app.rows.len().saturating_sub(top).min(height) as u16;
        // The cursor's column is a fainter fill than its row (`COLUMN`), on the rows with none of
        // their own, so the row stays the one read and a marked row its blue.
        cells(buf, Rect::new(x, area.y + head, w, drawn).intersection(area), |c| {
            if c.bg == Color::Reset {
                c.bg = COLUMN;
            }
        });
    }
    let level = app.price_level().map(|l| vec![level_label(l)]).unwrap_or_default();
    let bench = if let Input::Menu { col, .. } = &app.input { app.col_bench(*col) } else { None };
    let bench: Vec<String> = bench.into_iter().map(crate::app::bench_label).collect();
    let not = if let Input::Menu { col, .. } = &app.input { app.left_out(*col) } else { vec![] };
    if let Input::Menu { col, items, list } = &mut app.input {
        let l = Layout { name_x: name_x - area.x, name_w, dev_w, cols, via, notes, first, more, next, seps };
        let picked = match *col {
            1 => &app.store.dev_pick,
            VIA => &app.store.via_pick,
            PRICE => &level,
            _ => &bench,
        };
        dropdown(buf, area, menu_x(area, &l, *col), *col, items, list, picked, &not, faint);
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

/// What Via calls the harness `label` of its dropdown, when not by its name: said after it there.
fn tag(col: usize, label: &str) -> Option<&str> {
    Some(short(label)).filter(|&t| col == VIA && t != label)
}

/// The dropdown's box and its entries' width, or none when fewer than one entry would fit.
fn menu_box(area: Rect, x: u16, col: usize, items: &[(String, usize)]) -> Option<(Rect, (usize, usize))> {
    let tag = |s: &str| tag(col, s).map_or(0, |t| t.len() + 1);
    let label_w = items.iter().map(|(s, _)| s.chars().count() + tag(s)).max().unwrap_or(0);
    let n_w = items.iter().map(|(_, n)| n.to_string().len()).max().unwrap_or(0);
    let n = items.len();
    let h = (n as u16 + 2).min(area.height.saturating_sub(1));
    // Room for where you are, when the entries may not all fit.
    let foot = if n + 2 > area.height.saturating_sub(1) as usize { position(n, 0, n).len() + 2 } else { 0 };
    // The ✗ box after the ✓ one, where an entry can be left out.
    let ex = if matches!(col, 1 | PRICE | VIA) { 2 } else { 0 };
    let w = ((label_w + n_w + 8 + ex).max(foot) as u16).min(area.width);
    if h < 3 {
        return None;
    }
    Some((Rect::new(x.saturating_sub(2).min(area.right() - w).max(area.x), area.y + 1, w, h), (label_w, n_w)))
}

/// The list under a header opened with `d`, each entry with how many models it would show,
/// scrolled so the selection stays in view. Every entry is listed, what the search found
/// underlined. `picked` entries
/// show `✓`, the rest the mark a table row has when off, grey (`faint`) or a shape of its own.
/// Where one can be left out its `✗` follows, as on a table row: red for the ones left out (`not`).
#[allow(clippy::too_many_arguments)]
fn dropdown(
    buf: &mut Buffer,
    area: Rect,
    x: u16,
    col: usize,
    items: &[(String, usize)],
    list: &mut List,
    picked: &[String],
    not: &[String],
    faint: bool,
) {
    let range = list.range();
    // Said in the footer, short as the box keeps its width while typing.
    let none = list.no_match(items.iter().map(|e| e.0.as_str()), true);
    let (query, sel, top) = (list.query.as_str(), list.sel, &mut list.top);
    let len = items.len();
    let Some((rect, (label_w, n_w))) = menu_box(area, x, col, items) else { return };
    let shown = rect.height.saturating_sub(2) as usize;
    *top = list_top(*top, sel, shown).min(len.saturating_sub(shown));
    let top = *top;
    // Where you are, as every box says it.
    let footer = if len > shown { position(top, shown, len) } else { String::new() };
    let footer = if none { " no match ".to_string() } else { footer };
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .title_bottom(Line::from(footer).style(fg(MUTED)).right_aligned());
    let inner = block.inner(rect);
    // What is behind the box fades, as behind every box.
    restyle(buf, area, Style::new().add_modifier(Modifier::DIM));
    clear(buf, rect);
    block.render(rect, buf);
    for (i, (label, n)) in items.iter().enumerate().skip(top).take(shown) {
        let y = inner.y + (i - top) as u16;
        // Entries keep the colour they have in the table, under the cursor like there.
        let color = match i {
            0 => Color::Reset,
            _ if col == PRICE => LEVEL[i - 1],
            _ if col == 1 || col == VIA => dev_color(label),
            _ => Color::Reset,
        };
        // The cursor runs through the box's border, as in the table.
        if i == sel || range.as_ref().is_some_and(|r| r.contains(&i)) {
            cursor_ends(buf, rect.x, rect.right() - 1, y);
        }
        // Checkboxes as on the table's marks, in the mark's own colour there too, so a picked
        // entry does not read as an empty box in the entry's colour; "all" is ticked while
        // nothing is picked, as it is then what applies.
        let on = if i == 0 { picked.is_empty() && not.is_empty() } else { picked.contains(label) };
        let (unmarked, included) = if faint { ("✓ ", "✗") } else { ("☐ ", "·") };
        let (mark, style) = if on { ("✓ ", fg(MARK).add_modifier(BOLD)) } else { (unmarked, fg(MUTED)) };
        let mut x = put(buf, inner.x + 1, y, mark, 2, style).0;
        if matches!(col, 1 | PRICE | VIA) {
            let (s, style) = match i {
                _ if not.contains(label) => ("✗", fg(BAD).add_modifier(BOLD)),
                0 => (" ", fg(MUTED)),
                _ => (included, fg(MUTED)),
            };
            put(buf, x, y, s, 1, style);
            x += 2;
        }
        let at = x;
        let x = put(buf, x, y, format!("{label:<label_w$}  "), label_w + 2, fg(color)).0;
        if let Some(t) = tag(col, label) {
            put(buf, at + label.len() as u16 + 1, y, t, t.len(), fg(MUTED));
        }
        // "all" is never a match.
        let hits = if i == 0 { vec![] } else { found(label, query) };
        for r in hits.into_iter().map(|r| r.start.min(label_w)..r.end.min(label_w)) {
            restyle(buf, Rect::new(at + r.start as u16, y, r.len() as u16, 1), hit_style(fg(color)));
        }
        put(buf, x, y, format!("{n:>n_w$}"), n_w, fg(MUTED));
    }
    vmarks(buf, rect.x, inner.y, inner.bottom() - 1, top > 0, top + shown < len);
}

/// The cursor on row `y`: its fill from the column `left` to `right`, and its two bars at those,
/// the borders of the frame or the box, which the bars take the place of.
fn cursor_ends(buf: &mut Buffer, left: u16, right: u16, y: u16) {
    restyle(buf, Rect::new(left, y, right - left + 1, 1), FILL);
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
/// scrolled off above or below. Every scrolling list uses these, as `‹` `›` mark columns.
fn vmarks(buf: &mut Buffer, x: u16, top: u16, bottom: u16, above: bool, below: bool) {
    let mut put = |y: u16, glyph: &str| {
        put(buf, x, y, glyph, 1, MORE);
    };
    if above {
        put(top, "▲");
    }
    if below {
        put(bottom, "▼");
    }
}

/// A solid label like a bar module; returns the column after it.
fn pill(buf: &mut Buffer, x: u16, y: u16, text: &str, color: Color, max: u16) -> u16 {
    put(buf, x, y, format!(" {text} "), max as usize, fg(Color::Black).bg(color).add_modifier(BOLD)).0
}

/// The status bar's left side, joined with " · ": the model count, then what filters it.
fn parts(app: &App) -> Vec<Line<'static>> {
    let scope = match (app.all, app.any_available()) {
        (false, true) => "available",
        (false, false) => "all (none available)",
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
        // Once per task however many of its tiers it is for, so a few tasks still leave room.
        let mut tasks: Vec<String> =
            app.store.favorite_for(&m.key).iter().map(|s| s.split(':').next().unwrap_or(s).to_string()).collect();
        tasks.dedup();
        if !tasks.is_empty() {
            let spans = tasks.iter().enumerate().map(|(i, t)| {
                Span::styled(format!("{}★ {t}", if i > 0 { " " } else { "" }), fg(task_color(t)).add_modifier(BOLD))
            });
            parts.push(Line::from(spans.collect::<Vec<_>>()));
        }
    }
    // Why `E` shows a model with a grey ✗: the filter that hides it, which the row does not say.
    if let Some(by) = app.hidden_by() {
        parts.push(part(format!("hidden: {by}"), MUTED));
    }
    if !app.query.is_empty() {
        parts.push(part(format!("/{}", app.query), Color::Yellow));
    }
    if let Some(t) = app.task {
        parts.push(part(format!("best for {} per price", t.name), Color::Yellow));
    }
    // What the filters exclude, as the CLI's note says it: the bounds the TUI's own, as one on a
    // benchmark picked is.
    let hide = &app.store.hide;
    let bounds = app.bounds.iter().filter(|b| crate::app::bound_applies(b.0, hide));
    let bounds =
        bounds.map(|&(col, lo, hi)| crate::view::bound_said(app.bound_name(col), col, lo, hi, app.col_bench(col)));
    for said in crate::view::picks_said(&app.store).into_iter().chain(bounds) {
        parts.push(part(said, Color::Yellow));
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
        (Input::Bound { .. }, _) => ("BOUND", Color::Yellow),
        (Input::Cap { .. }, _) => ("TIER", Color::Yellow),
        (Input::Task(_), _) => ("TASK", Color::Yellow),
        (Input::Menu { list, .. }, _) if list.visual.is_some() => ("HIGHLIGHT", Color::Yellow),
        (Input::Menu { .. }, _) => ("PICK", Color::Yellow),
        (Input::Quit, _) => ("QUIT", Color::Red),
        (Input::Upgrade, _) => ("UPGRADE", Color::Yellow),
        (Input::Unfavorite, _) => ("UNFAVORITE", Color::Red),
        (Input::Unexclude, _) => ("UNEXCLUDE", Color::Red),
        (Input::DeleteTask(_), _) => ("DELETE", Color::Red),
        (Input::Reset, _) => ("RESET", Color::Red),
        (Input::Bind { .. }, _) => ("BIND", Color::Blue),
        (Input::Choose { kind, .. }, _) => {
            let name = match kind {
                Kind::Launch => "LAUNCH",
                Kind::Via | Kind::Harness | Kind::FirstHarness => "HARNESS",
                Kind::Fav => "FAV",
                Kind::Theme => "THEME",
                Kind::Cols => "COLUMNS",
                Kind::Open => "OPEN",
                Kind::Pick => "PICK",
                Kind::Bench => "BENCH",
            };
            (name, Color::Green)
        }
        (Input::None, View::Table) if app.selecting() => ("HIGHLIGHT", Color::Yellow),
        (Input::None, View::Table) => ("NORMAL", Color::Magenta),
        (Input::None, View::Help) => ("HELP", Color::Cyan),
        (Input::None, View::Recommend) => ("RECOMMEND", Color::Cyan),
        (Input::None, View::Detail(_)) => ("DETAIL", Color::Cyan),
        (Input::None, View::Issues) => ("ISSUES", Color::Cyan),
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
            | Input::Reset
            | Input::DeleteTask(_)
            | Input::Choose { list: List { typing: false, .. }, .. }
    ) && app.open_list().is_none_or(|l| l.query.is_empty())
    {
        // The question is in a box in the middle of the screen. Under it, what a pick or a key
        // could not save, which the next key clears, else the start's warning, kept while the
        // first start's question is open.
        if let Some(w) = app.failed.then_some(app.status.as_str()).or(app.store.warning.as_deref()) {
            put(buf, x, area.y, w, usize::from(area.right().saturating_sub(x)), fg(BAD));
        }
        return None;
    }
    // The prompt's label, the text being typed and the cursor's byte offset in it.
    let prompt = match &app.input {
        Input::Search { cur, .. } => {
            let q = if app.overlay_search() { &app.overlay_query } else { &app.query };
            Some(("/".to_string(), q, *cur))
        }
        Input::Note { text, cur, .. } => Some(("note: ".to_string(), text, *cur)),
        Input::Bound { col, min, text, cur } => {
            Some((format!("{} {} ", app.bound_name(*col), if *min { "≥" } else { "≤" }), text, *cur))
        }
        Input::Menu { col, list, .. } => Some(match (list.typing, list.query.is_empty()) {
            (false, true) => (format!("{} ▾", base_col_name(*col)), &list.query, list.cur),
            _ => (format!("{} ▾ /", base_col_name(*col)), &list.query, list.cur),
        }),
        Input::Choose { title, list, .. } => Some((format!("{title} /"), &list.query, list.cur)),
        Input::Quit
        | Input::Upgrade
        | Input::Unfavorite
        | Input::Unexclude
        | Input::Reset
        | Input::DeleteTask(_)
        | Input::Bind { .. }
        // Written on its header or its row (`recommend`).
        | Input::Cap { .. }
        | Input::Task(_)
        | Input::None => None,
    };
    if let Some((label, typed, cur)) = prompt {
        // Ctx is bounded in thousands of tokens, as its cells read: `Ctx ≥ 200k`, and Via by the
        // download's size in GB, `size ≤ 8 GB`. The number stays as typed until enter.
        let unit = match app.input {
            Input::Bound { col, .. } if crate::app::numeric(col).is_some_and(|c| c.id == "ctx") => "k",
            Input::Bound { col: VIA, .. } => " GB",
            _ => "",
        };
        // A choice list says its keys in its box, so the bar does not say them again.
        let (choose, menu, typing) = match &app.input {
            Input::Menu { list, .. } => (false, true, list.typing),
            Input::Choose { list, .. } => (true, false, list.typing),
            _ => (false, false, true),
        };
        // Not under a range, which esc cancels first and whose hint has no search.
        let kept = app.open_list().is_some_and(|l| !l.query.is_empty() && l.visual.is_none());
        let hint = match (menu, typing) {
            _ if choose => "",
            (true, false) if matches!(&app.input, Input::Menu { list, .. } if list.visual.is_some()) => {
                "↓ ↑ extend  space enter toggle  e leave out  esc cancel"
            }
            (true, false) if matches!(app.input, Input::Menu { col: 1 | VIA, .. }) => {
                "↓ ↑ move  / search  v range  space enter toggle  e leave out  esc close"
            }
            (true, false) if matches!(app.input, Input::Menu { col: PRICE, .. }) => {
                "↓ ↑ move  / search  space enter toggle  e leave out  esc close"
            }
            (true, false) => "↓ ↑ move  / search  space enter toggle  esc close",
            (true, true) => "↓ ↑ match  enter apply  esc cancel",
            _ => "enter apply  esc cancel",
        };
        let hint = if menu && kept && !typing { kept_hint(hint, "  ") } else { hint.to_string() };
        let hint = hint.as_str();
        let width = |s: &str| Span::raw(s).width();
        // The hint shows only where it leaves the label and a few typed cells their room.
        let fits = !hint.is_empty() && usize::from(x) + width(&label) + 8 + width(hint) < usize::from(area.right());
        let hx = if fits { area.right() - (width(hint) as u16 + 1) } else { area.right() };
        // Text too long for the room scrolls sideways, so the cursor stays in view.
        let room = usize::from(hx.saturating_sub(x));
        let start = scrolled(width(&label) + width(unit), typed, cur, room);
        let text = format!("{label}{}{unit}", &typed[start..]);
        put(buf, x, area.y, &text, room, Style::new());
        if fits {
            // Its keys in their colour, as in the hints of the table's bar.
            let mut hx = hx;
            for part in hint.split("  ") {
                let (key, what) = split_hint(part);
                let key = bound_keys(key, &app.store.keys);
                hx = put(buf, hx, area.y, &key, width(&key), fg(KEY).add_modifier(BOLD)).0;
                hx = put(buf, hx, area.y, what, width(what), fg(MUTED)).0 + 2;
            }
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
            x = put(buf, x, area.y, " · ", limit.saturating_sub(x) as usize, fg(MUTED)).0;
        }
        for span in &line.spans {
            let style = line.style.patch(span.style);
            x = put(buf, x, area.y, &span.content, limit.saturating_sub(x) as usize, style).0;
        }
    }
    // Hints: the key in colour, what it does in plain text.
    let mut x = area.x + start;
    for (i, hint) in hints.iter().enumerate() {
        if *hint == SEP {
            x = put(buf, x, area.y, SEP, width, fg(MUTED)).0;
        } else {
            let (key, what) = split_hint(hint);
            x = put(buf, x, area.y, bound_keys(key, &app.store.keys), width, fg(KEY).add_modifier(BOLD)).0;
            x = put(buf, x, area.y, what, width, Style::new()).0;
        }
        if i + 1 < hints.len() {
            x = put(buf, x, area.y, "  ", width, Style::new()).0;
        }
    }
    // The last hint stays however narrow the bar: where it ran over the pill, the pill wins.
    if area.x + start < end {
        pill(buf, area.x, area.y, mode, color, area.width);
    }
    None
}

/// How a search hit is drawn, as in the table: yellow, bold and underlined.
fn hit_style(style: Style) -> Style {
    style.fg(MATCH).add_modifier(BOLD | Modifier::UNDERLINED)
}

/// `line` with what the search `query` found in it drawn as hits, across its spans as it reads.
fn lit(line: Line<'static>, query: &str) -> Line<'static> {
    let r = found(&line.to_string(), query);
    lit_at(line, &r)
}

/// `line` with the chars `r` covers, counted from its start, drawn as search hits.
fn lit_at(mut line: Line<'static>, r: &[Range<usize>]) -> Line<'static> {
    if r.is_empty() {
        return line;
    }
    let outer = line.style;
    // Each span's chars, counted from the line's start.
    let mut at = 0;
    line.spans = std::mem::take(&mut line.spans)
        .into_iter()
        .flat_map(|span| {
            let hit = hit_style(outer.patch(span.style));
            let mut out: Vec<Span<'static>> = vec![];
            for c in span.content.chars() {
                let style = if r.iter().any(|r| r.contains(&at)) { hit } else { span.style };
                at += 1;
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

/// `span` with what the search `query` found in it drawn as hits.
fn lit_span(span: Span<'static>, query: &str) -> Vec<Span<'static>> {
    lit(Line::from(span), query).spans
}

/// The hints while a task is written, in its name and in what it is about: enter goes from the
/// name to what it is about, and there saves both.
const TASK_HINTS: [[&str; 3]; 2] =
    [["enter description", "tab other field", "esc cancel"], ["enter apply", "tab other field", "esc cancel"]];

/// `TASK_HINTS` for the field the cursor is in, in recommend.
fn field_hints(input: &Input) -> [&'static str; 3] {
    let about = matches!(input, Input::Task(Edit { what: What::Task { about: true, .. }, .. }));
    TASK_HINTS[usize::from(about)]
}

/// What stays of an entry of a list before the text written on it: the `+` of the one that
/// names a new task, and the harness before its folder.
fn edit_prefix(e: &Edit) -> String {
    match &e.what {
        What::Task { was: None, .. } => " + ".into(),
        What::Task { .. } => " ".into(),
        What::Dir(cmd, ..) => format!(" {} in ", cmd[0]),
    }
}

/// A harness's colour, grey when it is left out in the Via dropdown (`out`): the model shows for
/// its others. The table, `x`, the details and compare all say it so.
fn via_color(out: &[String], h: &str) -> Color {
    if out.iter().any(|x| x == h) { MUTED } else { dev_color(h) }
}

/// The harness a model runs on (`H`) in Via: dark on `c`, its colour, so it stands out of the list.
fn chip(c: Color) -> Style {
    fg(Color::Black).bg(c).add_modifier(BOLD)
}

/// `said`, `view::via`'s text for `m`, in `style`, a harness left out in the Via dropdown grey
/// and the one it runs on (`H`) a `chip`, as in the table.
fn via_spans(m: &Model, any: bool, get: bool, said: &str, store: &Store, style: Style) -> Vec<Span<'static>> {
    let harnesses = crate::view::via_harnesses(m, any, get);
    let Some(rest) = said.strip_prefix(&*harnesses.join(", ")) else {
        return vec![Span::styled(said.to_string(), style)];
    };
    let runs = runs_on(store, m);
    let mut spans = vec![];
    for (i, h) in harnesses.into_iter().enumerate() {
        spans.extend((i > 0).then(|| Span::styled(", ", style)));
        let c = via_color(&store.via_not, h);
        let style = match (runs == Some(h), c == MUTED) {
            (true, _) => style.patch(chip(c)),
            (_, true) => style.fg(MUTED),
            _ => style,
        };
        spans.push(Span::styled(h.to_string(), style));
    }
    spans.push(Span::styled(rest.to_string(), style));
    spans
}

/// An entry written in place, `prefix` before it, in `room` cells, and the column of the text
/// cursor in it. A folder is one field. A task is two, its name in the style `name` and then what
/// it is about. The field the cursor is in scrolls sideways to keep it in view, what is past
/// `room` is cut, so the entry keeps its box's width, and an empty field says in grey what to
/// write there.
fn edit_line(e: &Edit, prefix: Span<'static>, name: Style, room: usize) -> (Line<'static>, usize) {
    // `text` from byte `start`, in `cells` cells.
    let field = |text: &str, start: usize, cells: usize, empty: &str, style: Style| {
        let (text, style) = if text.is_empty() { (empty, fg(MUTED)) } else { (&text[start..], style) };
        let mut w = 0;
        let fits = |c: &char| {
            w += Span::raw(c.to_string()).width();
            w <= cells
        };
        Span::styled(text.chars().take_while(fits).collect::<String>(), style)
    };
    let at = |before: usize, start: usize| before + Span::raw(&e.text[start..e.cur]).width();
    // A cell is kept at the end for the text cursor.
    let (lead, room) = (prefix.width(), room.saturating_sub(1));
    let Some((n, a)) = e.fields() else {
        let start = scrolled(lead, &e.text, e.cur, room);
        let text = field(&e.text, start, room.saturating_sub(lead), "a folder", Style::new());
        return (Line::from(vec![prefix, text]), at(lead, start));
    };
    let on_about = matches!(e.what, What::Task { about: true, .. });
    let n_start = if on_about { 0 } else { scrolled(lead, n, e.cur, room) };
    // While what it is about is written, the name takes half the room at most, so that has some.
    let n_room = room.saturating_sub(lead) / if on_about { 2 } else { 1 };
    let n = field(n, n_start, n_room, "a name", name);
    // One space between them, as recommend's line under the grid has.
    let before = lead + n.width() + 1;
    let a_start = if on_about { scrolled(before, a, e.cur, room) } else { 0 };
    let a = field(a, a_start, room.saturating_sub(before), "a description", Style::new());
    let x = if on_about { at(before, a_start) } else { at(lead, n_start) };
    (Line::from(vec![prefix, n, Span::raw(" "), a]), x)
}

/// The cells a box of `f`'s grid takes along its row: the cursor's bars around the mark, and
/// the gap to the next.
const BOX_W: usize = 6;

/// How wide the names of `f`'s grid are: its boxes start two cells after.
fn fav_name_w(items: &[(String, Effect)]) -> usize {
    items.iter().map(|(label, _)| Span::raw(label.as_str()).width()).max().unwrap_or(0)
}

/// A list's hint, its keys apart by `sep`, while its search is kept, as enter keeps it: `n` `N`
/// go to the matches and esc clears it.
fn kept_hint(hint: &str, sep: &str) -> String {
    let hint = hint.replace("/ search", "n N match");
    match hint.rsplit_once(&format!("{sep}esc")) {
        Some((keys, _)) => format!("{keys}{sep}esc clear"),
        None => hint,
    }
}

/// `f`'s grid: the tiers' heading, a row per task with a box for each tier,
/// `✓` where the model is the favorite, `●` where another is, `☐` where none and `×` on a tier
/// it is dearer than, which cannot take it (`view::over_cap`), bold where it is the favorite
/// all the same, a cap lowered after it, the entry that
/// names a new task, what the box under the cursor holds, and the keys. Read from the store at
/// each draw, so the boxes follow a change made anywhere.
fn fav_lines(app: &App, items: &[(String, Effect)], list: &List, room: usize) -> Vec<Line<'static>> {
    let query = list.query.as_str();
    let w = fav_name_w(items);
    let head: String = crate::view::TIERS.iter().map(|t| format!(" {:<1$}", t.0, BOX_W - 1)).collect();
    let mut lines = vec![Line::from(format!(" {:w$} {head}", "")).style(fg(MUTED))];
    // And the harness the favorite runs on, a `chip` after it as in Via.
    let (mut under, mut runs_chip) = (String::new(), None);
    // The grid is for one model: looked up once, not per box.
    let key = items.iter().find_map(|(_, e)| if let Effect::Fav(k, _) = e { Some(k) } else { None });
    let model = key.and_then(|k| app.data.models.iter().find(|m| m.key == *k));
    let runs = model.and_then(|m| runs_on(&app.store, m));
    // The row being written and its name's style, drawn once the others are, in their width.
    let mut edit_at = None;
    for (i, (label, effect)) in items.iter().enumerate() {
        let on = i == list.sel;
        let Effect::Fav(key, task) = effect else {
            // The entry that names a new task.
            if list.edit.is_some() && on {
                edit_at = Some((lines.len(), Style::new()));
            }
            lines.push(Line::from(cursor(on, vec![Span::raw(label.clone())])));
            continue;
        };
        let color = task_color(task);
        if list.edit.is_some() && on {
            edit_at = Some((lines.len(), fg(color)));
            lines.push(Line::default());
            continue;
        }
        let name = lit(Line::from(Span::styled(label.clone(), fg(color))), query);
        // Padded by the cells the name takes, so a wide one keeps its boxes under the heading.
        let pad = w - Span::raw(label.as_str()).width();
        let mut spans = vec![Span::raw(" ")];
        spans.extend(name.spans);
        spans.push(Span::raw(" ".repeat(pad + 1)));
        for col in 0..BOXES {
            let slot = box_slot(task, col);
            let over = model.and_then(|m| crate::view::over_cap(&app.store, &slot, m));
            let mark = match (app.store.favorite(&slot), &over) {
                // Its own glyph, not a grey ✓, so it reads without colours too. A favorite a
                // later cap left there is bold, as a ✓ is, the cursor stopping on it so space
                // clears it. Over the cap, another model's favorite there is a `×` too: h l skip it.
                (Some(k), Some(_)) if k == key => Span::styled("×", fg(MARK).add_modifier(BOLD)),
                (Some(k), _) if k == key => Span::styled("✓", fg(MARK).add_modifier(BOLD)),
                (_, Some(_)) => Span::styled("×", fg(MUTED)),
                (Some(_), None) => Span::styled("●", fg(MUTED)),
                (None, None) => Span::styled("☐", fg(color)),
            };
            let at = on && col == list.col;
            spans.extend(cursor(at, vec![mark]));
            spans.push(Span::raw(" ".repeat(BOX_W - 3)));
            if at {
                let name =
                    |k: &str| app.data.models.iter().find(|m| m.key == k).map_or(k.to_string(), |m| m.name.clone());
                runs_chip = runs.filter(|_| app.store.favorite(&slot) == Some(key.as_str()) && over.is_none());
                under = match (app.store.favorite(&slot), runs, &over) {
                    (Some(k), _, Some(why)) if k == key => format!("{slot}: unused, {why}, space clears it"),
                    (Some(k), Some(_), _) if k == key => format!("{slot}: {}", name(k)),
                    (Some(k), ..) if k == key => format!("{slot}: {}", name(k)),
                    (Some(k), ..) => format!("{slot}: the favorite is {}", name(k)),
                    (None, _, Some(why)) => format!("{slot}: {why}"),
                    (None, _, None) => format!("{slot}: no favorite"),
                };
            }
        }
        // What the task is about, yours in your words, cut to what the row has left of `room`.
        let left = room.saturating_sub(w + 2 + BOXES * BOX_W).min(48);
        let about = app.store.about(task).map(String::from).or_else(|| fit::task(task).map(fit::Task::about_ranked));
        if let Some(about) = about.filter(|_| left > 1) {
            spans.push(Span::styled(truncate(&about, left), fg(MUTED)));
        }
        lines.push(Line::from(spans));
    }
    let own = items.iter().any(|(_, e)| matches!(e, Effect::Fav(_, t) if fit::task(t).is_none()));
    let hint = match (list.typing, own) {
        // While searching the letters are typed, as the status bar says.
        (true, _) => " ↓ ↑ match · enter apply · esc cancel",
        // `T` is said once there is a task of your own. `↓ ↑`, enter and esc work too, left
        // unsaid: with them the line is wider than 80 columns hold.
        (_, true) => " ← → tier · / search · space toggle · H harness · T edit",
        _ => " ↓ ↑ ← → move · / search · space enter toggle · H harness · esc close",
    };
    let hint = if list.typing || list.query.is_empty() { hint.to_string() } else { kept_hint(hint, " · ") };
    // As wide as the hint at most, so the box keeps its width as the cursor moves.
    let width = hint.chars().count();
    lines.push(Line::default());
    // The name is cut, not the `via` that leads to the chip.
    let (via, chip_w) = runs_chip.map_or(("", 0), |h| (", via ", 6 + h.chars().count()));
    let said = format!(" {}{via}", truncate(&under, (width - 1).saturating_sub(chip_w)));
    let mut said = vec![Span::styled(said, fg(MUTED))];
    said.extend(runs_chip.map(|h| Span::styled(h.to_string(), chip(via_color(&app.store.via_not, h)))));
    lines.push(Line::from(said));
    // A row being written takes every key; a name enter did not take says why, on as many lines
    // as that needs. As wide as the hint it stands for.
    let pad = |l: Line| Line::from(format!("{:<width$}", l.to_string()));
    match &list.edit {
        Some(Edit { err: Some(err), .. }) => {
            let words = err.split(' ').map(|w| Line::from(w.to_string())).collect();
            let said = wrapped(vec![Span::raw(" ")], words, &Span::raw(" "), width);
            lines.extend(said.into_iter().map(|l| pad(l).style(fg(BAD))));
        }
        Some(Edit { what: What::Task { about, .. }, .. }) => {
            let hints = TASK_HINTS[usize::from(*about)].join(" · ");
            lines.push(key_line(&format!("{:<width$}", format!(" {hints}")), &app.store.keys));
        }
        Some(_) => lines.push(key_line(&format!("{:<width$}", " enter apply · esc cancel"), &app.store.keys)),
        None => lines.push(key_line(&hint, &app.store.keys)),
    }
    // The row being written, as wide as the others at most, so the box keeps its width.
    if let (Some((at, style)), Some(e)) = (edit_at, &list.edit) {
        lines[at] = edit_line(e, Span::raw(edit_prefix(e)), style, fav_room(&lines, room)).0;
    }
    lines
}

/// The cells a task written in `f`'s grid has: the widest of its other `lines`, so the box keeps
/// its width, `room` at most.
fn fav_room(lines: &[Line], room: usize) -> usize {
    lines.iter().map(Line::width).max().unwrap_or(0).min(room)
}

/// The entries of a choice list, each coloured by its first word: the harness or the site.
/// `out` are the harnesses left out in the Via dropdown, grey as in the table, and `colors` a
/// tier's models' colours (`pick_colors`), an item each, and `ticks` which entries have their box
/// ticked (`App::ticked`). `f`'s grid has lines of its own, `fav_lines`.
#[allow(clippy::too_many_arguments)]
fn choice_lines(
    kind: Kind,
    items: &[(String, Effect)],
    list: &List,
    room: usize,
    bound: &BTreeMap<char, char>,
    fits: Option<bool>,
    out: &[String],
    colors: &[Color],
    ticks: &[bool],
) -> Vec<Line<'static>> {
    let (query, typing) = (list.query.as_str(), list.typing);
    let mut lines: Vec<Line> = items
        .iter()
        .enumerate()
        .map(|(n, (label, effect))| {
            let harness = crate::data::harness_of(label.split(' ').next().unwrap_or_default());
            // A harness whose folder is being written, in the harness's colour.
            if let Some(e) = list.edit.as_ref().filter(|_| n == list.sel) {
                return edit_line(e, Span::styled(edit_prefix(e), fg(via_color(out, harness))), Style::new(), room).0;
            }
            // What has a colour in the table keeps it here: a harness. A site, a theme and a
            // source have none, so they take the text's and not one their name gives.
            let color = match effect {
                Effect::Launch(_) | Effect::Via(_, Some(_), _) | Effect::Harness(Some(_)) => via_color(out, harness),
                _ => Color::Reset,
            };
            // What the search found in the label, as the cursor goes by it, `at` chars into the line.
            let label_hits =
                |at: usize| found(label, query).into_iter().map(|r| r.start + at..r.end + at).collect::<Vec<_>>();
            // A name, then what follows it muted: a harness and the command that opens it.
            let name = match effect {
                Effect::Launch(_) | Effect::Via(_, Some(_), _) | Effect::Harness(Some(_)) | Effect::Col(_) => {
                    Some((label.find(' ').unwrap_or(label.len()), fg(color)))
                }
                _ => None,
            };
            if let Some((end, style)) = name {
                let (name, rest) = label.split_at(end);
                let mut spans = vec![Span::styled(format!(" {name}"), style)];
                // A download's size in the text's colour when it fits in your machine's memory, as in Via.
                match rest.rfind(" (").filter(|_| fits == Some(true) && got(effect).is_some()) {
                    Some(at) => spans.extend([
                        Span::styled(rest[..at].to_string(), fg(MUTED)),
                        Span::raw(format!("{} ", &rest[at..])),
                    ]),
                    None => spans.push(Span::styled(format!("{rest} "), fg(MUTED))),
                }
                // A column has its checkbox, as an entry of a dropdown: ticked while it shows.
                if let Effect::Col(_) = effect {
                    spans.insert(0, tick(ticks.get(n) == Some(&true)));
                }
                let at = 1 + if matches!(effect, Effect::Col(_)) { 2 } else { 0 };
                return lit_at(Line::from(spans), &label_hits(at));
            }
            // A tier's model in its colour with its dots grey, as in recommend's boxes, its ★ in
            // the task's colour and "recommended:" in the text's.
            if let Effect::Pick(slot, _) = effect {
                let mut spans = vec![Span::raw(" ")];
                let mut parts = label.split(" · ");
                let first = parts.next().unwrap_or_default();
                if let Some(name) = first.strip_prefix("★ ") {
                    spans.extend([
                        Span::styled("★ ", fg(task_color(slot)).add_modifier(BOLD)),
                        Span::raw(name.to_string()),
                    ]);
                } else if let Some(name) = first.strip_prefix("recommended: ") {
                    spans.extend([Span::styled("recommended: ", fg(Color::Reset)), Span::raw(name.to_string())]);
                } else {
                    spans.push(Span::raw(first.to_string()));
                }
                for part in parts {
                    spans.extend([Span::styled(" · ", fg(MUTED)), Span::raw(part.to_string())]);
                }
                spans.push(Span::raw(" "));
                let color = colors.get(n).copied().unwrap_or(Color::Reset);
                return lit_at(Line::from(spans).style(fg(color)), &label_hits(1));
            }
            // A benchmark has its box too, ticked while its column shows it, as in the dropdown.
            if let Effect::Bench(..) = effect {
                let spans = vec![tick(ticks.get(n) == Some(&true)), Span::raw(format!(" {label} "))];
                return lit_at(Line::from(spans), &label_hits(3));
            }
            lit_at(Line::from(format!(" {label} ")).style(fg(color)), &label_hits(1))
        })
        .collect();
    let hint = match kind {
        _ if typing => " ↓ ↑ match · enter apply · esc cancel",
        Kind::Cols => " ↓ ↑ move · / search · space enter toggle · esc | close",
        Kind::Fav => unreachable!("f's grid has lines of its own, fav_lines"),
        Kind::Theme => " ↓ ↑ preview · / search · space enter saves · esc t close",
        Kind::Pick => " ↓ ↑ move · / search · space enter picks · esc close",
        Kind::Harness => " ↓ ↑ move · / search · space enter picks · esc P close",
        Kind::FirstHarness => " ↓ ↑ move · / search · space enter picks · esc skips",
        Kind::Bench => " ↓ ↑ move · / search · space enter picks · esc B close",
        Kind::Via => " ↓ ↑ move · / search · space enter picks · esc back",
        Kind::Open | Kind::Launch => " ↓ ↑ move · / search · space enter opens · esc close",
    };
    let hint = if typing || query.is_empty() { hint.to_string() } else { kept_hint(hint, " · ") };
    // A folder being written takes every key: what enter did not take says why, and tab what
    // the folder could be. As wide as the hint it stands for.
    let width = hint.chars().count();
    // The folders tab found, as a shell lists them: on as many lines as they take, a few at
    // most, the ones around the folder tab is on, which stands out.
    if let Some(Edit { what: What::Dir(_, more, on), err: None, .. }) = &list.edit
        && !more.is_empty()
    {
        const MOST: usize = 6;
        // A name in the cells a line has, by what it takes on screen: a wide one takes two a letter.
        let fit = |name: &str| {
            let mut name = truncate(name, width - 1);
            while Span::raw(&name).width() >= width {
                name = truncate(&name, name.chars().count() - 1);
            }
            name
        };
        // Each line's folders, and the cells they take.
        let mut rows: Vec<(Vec<usize>, usize)> = vec![];
        for (i, name) in more.iter().enumerate() {
            let w = Span::raw(fit(name)).width();
            match rows.last_mut() {
                Some((row, used)) if *used + 2 + w < width => {
                    row.push(i);
                    *used += 2 + w;
                }
                _ => rows.push((vec![i], 1 + w)),
            }
        }
        let at = on.and_then(|i| rows.iter().position(|(row, _)| row.contains(&i))).unwrap_or(0);
        let cut = rows.len() > MOST;
        let shown = if cut { MOST - 1 } else { rows.len() };
        let from = at.saturating_sub(shown - 1);
        let rest = more.len() - rows[from..from + shown].iter().map(|(row, _)| row.len()).sum::<usize>();
        for (row, used) in &rows[from..from + shown] {
            let mut spans = vec![Span::raw(" ")];
            for (k, &i) in row.iter().enumerate() {
                let style = if *on == Some(i) { Style::new().add_modifier(BOLD) } else { fg(MUTED) };
                spans.push(Span::raw(if k > 0 { "  " } else { "" }));
                spans.push(Span::styled(fit(&more[i]), style));
            }
            spans.push(Span::raw(" ".repeat(width.saturating_sub(*used))));
            lines.push(Line::from(spans));
        }
        if cut {
            lines.push(Line::from(format!("{:<width$}", format!(" … {rest} more"))).style(fg(MUTED)));
        }
        return lines;
    }
    // P is no key of this box: said in a sentence of its own, not with the hints under it.
    if kind == Kind::FirstHarness {
        let p = Span::styled(bound_keys("P", bound), fg(KEY).add_modifier(BOLD));
        lines.extend([
            Line::default(),
            Line::from(vec![Span::raw(" you can change it later with "), p, Span::raw(" ")]).style(fg(MUTED)),
        ]);
    }
    lines.push(match &list.edit {
        Some(Edit { err: Some(err), .. }) => Line::from(format!("{:<width$}", format!(" {err}"))).style(fg(BAD)),
        Some(_) => key_line(&format!("{:<width$}", " tab complete · enter opens · esc back"), bound),
        None => key_line(&hint, bound),
    });
    lines
}

/// The first line in view of a list `shown` lines tall with the cursor on `sel`: where it was,
/// `top`, moved only as far as it takes to show the cursor, as the table scrolls.
fn list_top(top: usize, sel: usize, shown: usize) -> usize {
    top.clamp(sel.saturating_sub(shown.max(1) - 1), sel)
}

/// A list entry's checkbox, `✓` while `on`.
fn tick(on: bool) -> Span<'static> {
    if on { Span::styled(" ✓", fg(MARK).add_modifier(BOLD)) } else { Span::raw(" ☐") }
}

/// An open chooser's lines and the area its box is centred in, for `draw` and `hit` alike: the
/// frame between the tabs and the status bar. A terminal too short for a box there has it over
/// the tabs.
fn chooser(app: &App, area: Rect) -> Option<(Rect, Vec<Line<'static>>)> {
    let Input::Choose { kind, items, list, .. } = &app.input else { return None };
    let tabs = if area.height < 6 { 0 } else { 2 };
    let within = Rect { y: area.y + tabs, height: area.height - 1 - tabs, ..area };
    let lines = match kind {
        Kind::Fav => fav_lines(app, items, list, edit_room(within)),
        _ => {
            let fits = app.current().and_then(|m| app.fits_here(m)).filter(|_| *kind == Kind::Launch);
            let colors = if *kind == Kind::Pick { pick_colors(app, items) } else { vec![] };
            let (keys, out) = (&app.store.keys, &app.store.via_not);
            let ticks: Vec<bool> = items.iter().map(|(_, e)| app.ticked(e)).collect();
            choice_lines(*kind, items, list, edit_room(within), keys, fits, out, &colors, &ticks)
        }
    };
    Some((within, lines))
}

/// The colours of the models `d` lists for a tier, an item each: their price level's, the first
/// line's the model the tier picks with no favorite. By the price the line shows, so the best
/// model your machine runs is not green there as in its box, and a favorite not recommended is
/// not grey, everything listed being one to pick.
fn pick_colors(app: &App, items: &[(String, Effect)]) -> Vec<Color> {
    let by_key: HashMap<&str, &Model> = app.data.models.iter().map(|m| (m.key.as_str(), m)).collect();
    let picked = |slot: &str| {
        let (task, c) = slot_box(slot);
        fit::task(task).and_then(|t| app.recommended(t, TIERS[c].0).0).map(|p| p.0)
    };
    (items.iter())
        .map(|(_, e)| {
            let m = match e {
                Effect::Pick(_, Some(k)) => by_key.get(k.as_str()).copied(),
                Effect::Pick(slot, None) => picked(slot),
                _ => None,
            };
            m.map_or(MUTED, |m| pick_color(app, m, false, false))
        })
        .collect()
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

/// Where an overlay with these lines sits: centred, as wide as its widest line or title, or as
/// `area` where that would leave one cell on a side, the table's border showing there by its own.
fn overlay_rect(area: Rect, title: &str, lines: &[Line]) -> Rect {
    let widest = lines.iter().map(Line::width).max().unwrap_or(0) as u16;
    let w = (widest + 4).max(title.chars().count() as u16 + 6);
    let w = if w + 4 > area.width { area.width } else { w };
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

/// A centred rounded box, its border in the text's colour so it stands out from the grey table
/// frame and what is dimmed behind it, showing `lines` from `scroll` on, which is clamped to the
/// content: the accent is left to the table's headers and the cursor. The lines `pin`, which say
/// what the cursor is on, stay at the bottom of a box too short for every line (`pinned`). Returns
/// where the lines went, and whether some are scrolled off above and below. The rest of `area` is
/// dimmed.
fn overlay(
    buf: &mut Buffer,
    area: Rect,
    title: &str,
    mut lines: Vec<Line<'static>>,
    scroll: &mut u16,
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
        .title_top(Line::from(format!(" {title} ")).style(Style::new().add_modifier(BOLD)))
        .title_bottom(Line::from(footer).style(fg(MUTED)).right_aligned());
    // Inside the border, a cell of padding either side.
    let text = rect.inner(ratatui::layout::Margin::new(2, 1));
    // What is behind the box fades, so the box is what shows.
    restyle(buf, area, Style::new().add_modifier(Modifier::DIM));
    clear(buf, rect);
    block.render(rect, buf);
    let (above, below) = (*scroll > 0, *scroll as usize + shown < lines.len());
    for (line, y) in lines.into_iter().skip(top).take(shown).chain(pin).zip(text.y..text.bottom()) {
        put_line(buf, &line, Rect { y, height: 1, ..text });
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

/// `?`'s lines, and the line of each row of `HELP`, the cursor's stops, which alone have what
/// `/` found underlined, as `n` finds it (`App::next_hit`). A key you can move is in the keys'
/// colour, as you bound it, with its default after what it does once it is another, and the one
/// waiting for its new key says so. The rest are grey. The keys a row names are drawn as bound.
pub fn help(app: &App) -> (Vec<Line<'static>>, Vec<usize>) {
    let bound = &app.store.keys;
    let shown = |k: &str| bindable(k).map_or_else(|| bound_keys(k, bound), |d| app.key_of(d).to_string());
    let key_w = help_rows().map(|r| shown(r.0).chars().count()).max().unwrap_or(0);
    let (mut v, mut rows): (Vec<Line>, _) = (Vec::new(), vec![]);
    for (name, keys) in HELP {
        v.push(heading(name));
        for &(k, what) in keys.iter() {
            rows.push(v.len());
            let d = bindable(k);
            let waiting = match app.input {
                Input::Bind { key, wrong } if d == Some(key) => Some(wrong),
                _ => None,
            };
            let key = if waiting.is_some() { "…".to_string() } else { shown(k) };
            let style = if d.is_some() || k == "↺" { fg(KEY).add_modifier(BOLD) } else { fg(MUTED) };
            let mut spans = vec![Span::styled(format!("{key:<key_w$}  "), style)];
            // Between backticks the keys it names, in the keys' colour.
            for (i, part) in what.split('`').enumerate() {
                spans.push(match i % 2 {
                    0 => Span::raw(part.to_string()),
                    _ => Span::styled(bound_keys(part, bound), fg(KEY)),
                });
            }
            spans.push(match (waiting, d) {
                (Some(true), _) => Span::styled("  press a letter or a symbol, esc cancels", fg(BAD)),
                (Some(false), _) => Span::styled("  press its new key", fg(MUTED)),
                (None, Some(d)) if app.key_of(d) != d => Span::styled(format!(" (default {d})"), fg(MUTED)),
                _ => Span::raw(""),
            });
            let line = Line::from(spans);
            let hits = found(&line.to_string(), &app.overlay_query);
            v.push(lit_at(line, &hits));
        }
        v.push(Line::default());
    }
    v.push(Line::from("Columns: see the top border. Green is the best shown, red the worst").style(fg(MUTED)));
    v.push(Line::from(format!("Saved in {}", crate::store::path().display())).style(fg(MUTED)));
    v.push(Line::from("Every key: github.com/danctorres/modelcmp/blob/main/KEYS.md").style(fg(MUTED)));
    v.push(Line::from("CLI: modelcmp --help").style(fg(MUTED)));
    (v, rows)
}

/// `lines` with what `/` found underlined, every line kept, and the ones it is on.
fn lit_lines(lines: Vec<Line<'static>>, query: &str) -> (Vec<Line<'static>>, Vec<usize>) {
    // Redrawn on every frame: with nothing searched, nothing to underline.
    if query.is_empty() {
        return (lines, vec![]);
    }
    let mut hits = vec![];
    let lines = (lines.into_iter().enumerate())
        .map(|(i, l)| {
            let r = found(&l.to_string(), query);
            hits.extend((!r.is_empty()).then_some(i));
            lit_at(l, &r)
        })
        .collect();
    (lines, hits)
}

/// The line of `hits` that `n` `N` are on, the panel scrolled to it when they or the search
/// just moved, which the status bar says.
fn to_hit(app: &mut App, hits: &[usize]) -> Option<usize> {
    if app.overlay_query.is_empty() {
        return None;
    }
    // From the first, round again past either end.
    let (n, jump) = app.hit;
    app.hit = (n.rem_euclid(hits.len().max(1) as isize), false);
    let at = hits.get(app.hit.0 as usize).copied();
    if jump {
        app.status = match at {
            Some(at) => {
                app.scroll = at as u16;
                // Lines, as `n` goes from line to line, not from match to match on one.
                format!("line {} of {}", app.hit.0 + 1, hits.len())
            }
            None => format!("no line matches {}", app.overlay_query),
        };
    }
    at
}

/// What a box of recommend's grid shows.
const TIER_LEGEND: &str = "the best model for each task at each price, or ★ your favorite, shown as name · $ per 1M tokens · score. \
                           On all, ↓ marks a model that x downloads first.";

/// What recommend leaves out, and how to pick it anyway.
const UNRATED: &str =
    "models without a benchmark score aren't recommended, but you can still pick one with d on a tier.";

/// A row per task, the built-in ones and then your own, with a box per tier for the model it
/// picks, as `--tier` does, wrapped to `width`; under the grid, what the cursor's task is, when
/// to use it and its box's model in full. Where each name and box is goes to `spots`, and the
/// lines of the cursor's row come back too, with the ones under the grid, for `overlay` to pin,
/// and the line and cell of the text cursor while a tier's cap is written on its header.
#[allow(clippy::type_complexity)]
fn recommend(
    app: &App,
    width: usize,
    spots: &mut Vec<Spot>,
) -> (Vec<Line<'static>>, Option<Lines>, Lines, Option<(usize, usize)>) {
    let mut block = None;
    let label = |s: &'static str| vec![Span::styled(s, fg(MUTED))];
    let words = |s: &str| s.split(' ').map(|w| Line::from(w.to_string())).collect();
    // The grid takes the whole width, so no name is cut that has room, the text a readable one.
    let text = width.min(130);
    let space = Span::raw(" ");
    // The models it ranks, a tab of the table's each: the one on has the filled dot, bold
    // as a tab that is on, and one with nothing to show is grey.
    let tabs = (0..FAV).map(|i| {
        let on = i == app.among_on();
        let style = match on {
            true => TAB_ON,
            _ if app.tab_has(i) => Style::new(),
            _ => fg(MUTED),
        };
        let text = format!("{} {}", if on { "●" } else { "○" }, TABS[i].0);
        Line::from(cursor(app.among == Some(i), vec![Span::styled(text, style)]))
    });
    let (mut v, at) = wrapped_at(label("from:"), tabs.collect(), &Span::styled("·", fg(MUTED)), text);
    spots.extend(at.into_iter().enumerate().map(|(i, (l, x))| Spot { lines: l..l + 1, x, at: Stop::Among(i) }));
    if app.filtered_too() {
        let note = Span::styled(" (filtered)", fg(MUTED));
        // On a line of its own under the tabs when the last has no room for it.
        if v.last().unwrap().width() + note.width() > text {
            v.push(Line::from(" ".repeat("from:".len())));
        }
        v.last_mut().unwrap().push_span(note);
    }
    let among = v.len();
    v.extend(
        wrapped(vec![], words(&TIER_LEGEND.replace(" x ", &format!(" {} ", app.key_of('x')))), &space, text)
            .into_iter()
            .map(|l| l.style(fg(MUTED))),
    );
    // The keys they name as they are bound now.
    let warn: Vec<Line> = words(&UNRATED.replace(" d ", &format!(" {} ", app.key_of('d'))));
    let warning = vec![Span::styled("warning: ", fg(Color::Yellow))];
    v.extend(wrapped(warning, warn, &space, text).into_iter().map(|l| l.style(fg(MUTED))));
    v.push(Line::default());
    let own = app.store.custom_tasks();
    // What you wrote about a task of your own is when to use it.
    let when = |t| app.store.about(t).unwrap_or(CUSTOM_WHEN);
    let tasks: Vec<(&str, &str)> =
        TASKS.iter().map(|t| (t.name, t.when)).chain(own.iter().map(|&t| (t, when(t)))).collect();
    // With the tiers the best model your machine runs took, once a frame.
    let (picks, took): (Vec<_>, Vec<_>) = (0..tasks.len()).map(|i| app.tier_picks_took(i)).unzip();
    let (tasks, picks, took) = (&tasks, &picks, &took);
    let cells = |i: usize| (0..TIERS.len()).map(move |c| entry(app, tasks[i].0, c, picks[i][c], took[i][c], true));
    let w = tasks.iter().map(|t| Span::raw(t.0).width()).max().unwrap_or(0).min(20);
    let mut grid: Vec<Vec<_>> = (0..tasks.len()).map(|i| cells(i).collect()).collect();
    for c in 0..TIERS.len() {
        align(grid.iter_mut().map(|r| &mut r[c]));
    }
    // The names' column, then a box per tier as wide as its widest entry, the widest boxes
    // narrowed alike to what `width` leaves, where an entry wraps to more lines, so no name is cut.
    // Each tier's name with its cap, `low ≤$2`, or the one being typed (`<`).
    let editing = match &app.input {
        Input::Cap { tier, text, cur } => Some((*tier, text.as_str(), *cur)),
        _ => None,
    };
    let heads: Vec<String> = TIERS
        .iter()
        .enumerate()
        .map(|(c, t)| match editing {
            Some((e, text, _)) if e == c => format!("{} ≤ ${text} ", t.0),
            _ if crate::view::capped(t.0) => format!("{} ≤${}", t.0, app.store.tier_cap(t.0)),
            _ => t.0.to_string(),
        })
        .collect();
    // Room for the ▾ every box has.
    let need: Vec<usize> = (0..TIERS.len())
        .map(|c| {
            grid.iter()
                .map(|r| r[c].iter().map(Span::width).sum::<usize>() + 2)
                .fold(Span::raw(heads[c].as_str()).width(), usize::max)
        })
        .collect();
    let left = width.saturating_sub(w + 2 + 2 * TIERS.len());
    // A box no narrower than its header where that fits, so the header shows the cap, else
    // narrower, the header the tier's name alone.
    let heads_w: Vec<usize> = heads.iter().map(|h| h.chars().count()).collect();
    let sized = |cap: usize, head: bool| -> Vec<usize> {
        need.iter().zip(&heads_w).map(|(&n, &h)| n.min(if head { cap.max(h) } else { cap })).collect()
    };
    let caps = (4..=need.iter().copied().max().unwrap_or(4)).rev();
    let fits = |r: &Vec<usize>| r.iter().sum::<usize>() <= left;
    let room = (caps.clone().map(|c| sized(c, true)).find(fits))
        .or_else(|| caps.map(|c| sized(c, false)).find(fits))
        .unwrap_or_else(|| sized(4, false));
    let head_y = v.len();
    let on_caps = app.among.is_none() && app.caps;
    let mut head = vec![Span::styled(format!(" {:w$} ", ""), fg(MUTED))];
    let mut text_at = None;
    for (c, (h, &r)) in heads.iter().zip(&room).enumerate() {
        // Too narrow for the cap, the tier's name alone, so the header stays over its column.
        let h = match editing {
            Some((e, ..)) if e == c => truncate(h, r),
            _ if h.chars().count() > r => TIERS[c].0.to_string(),
            _ => h.clone(),
        };
        let piece = Span::raw(format!("{h:<r$}"));
        match editing {
            // The cursor's bars around it, the text cursor after what is typed up to `cur`.
            Some((e, text, cur)) if e == c => {
                let x = head.iter().map(Span::width).sum::<usize>() + 1;
                let before = Span::raw(format!("{} ≤ ${}", TIERS[c].0, &text[..cur])).width();
                text_at = Some((head_y, x + before.min(r.saturating_sub(1))));
                head.extend(cursor(true, vec![piece]));
            }
            _ => {
                // A click on a cap writes it, as `<` on a box of its tier does.
                let x = head.iter().map(Span::width).sum::<usize>();
                if crate::view::capped(TIERS[c].0) {
                    spots.push(Spot { lines: head_y..head_y + 1, x: x..x + r + 2, at: Stop::Cap(c) });
                }
                let on = on_caps && app.cap_col() == c;
                head.extend(cursor(on, vec![piece]));
            }
        }
    }
    v.push(Line::from(head));
    let cur = (app.among.is_none() && !app.caps).then_some((app.task_cur, app.task_sel));
    // The task being written under the grid, and its row.
    let edit = match &app.input {
        Input::Task(e) => app.edit_row().map(|at| (at, e)),
        _ => None,
    };
    for (i, (t, row)) in tasks.iter().zip(grid).enumerate() {
        let y = v.len();
        // The name being written under the grid shows here too, as it is typed. Cut to the cells
        // the column has, as it is sized: a wide character takes two.
        let shown = edit.filter(|e| e.0 == i).and_then(|e| e.1.fields()).map_or(t.0, |f| f.0);
        let mut name = truncate(shown, w);
        while Span::raw(name.as_str()).width() > w {
            name = truncate(&name, name.chars().count() - 1);
        }
        let pad = w.saturating_sub(Span::raw(name.as_str()).width());
        let name = Span::styled(name, fg(task_color(t.0)).add_modifier(BOLD));
        let mut first = cursor(cur == Some((i, 0)), lit_span(name, &app.overlay_query));
        first.push(Span::raw(" ".repeat(pad)));
        // The model in each box, where the search found its name, after a ★.
        let hits = |c: usize, cell: &[Span]| {
            let star = if cell.first().is_some_and(|s| s.content == "★ ") { 2 } else { 0 };
            let r = picks[i][c].map_or(vec![], |(m, _)| found(&m.name, &app.overlay_query));
            r.into_iter().map(|r| r.start + star..r.end + star).collect::<Vec<_>>()
        };
        // Each box has a ▾, as a header with a dropdown does: `d` lists its models. The
        // cursor's is bright, the others muted.
        let menu = cur.filter(|&(t, sel)| t == i && sel > 0).map(|(_, sel)| sel - 1);
        let row: Vec<_> = (row.into_iter().zip(&room).enumerate())
            .map(|(c, (cell, &r))| {
                let hits = hits(c, &cell);
                let mut lines = wrap_spans(cell.clone(), r, &hits);
                // On the last line with room for it, so the box takes no more lines, else the
                // box wrapped 2 narrower, unless that takes more lines than the ▾ alone on one.
                let mark = Span::styled(MENU, if menu == Some(c) { Style::new() } else { fg(MUTED) });
                let room = |l: &Vec<Span>| l.iter().map(Span::width).sum::<usize>() + 2 <= r;
                if !lines.iter().any(room) {
                    let narrow = wrap_spans(cell, r.saturating_sub(2), &hits);
                    if narrow.len() <= lines.len() + 1 {
                        lines = narrow;
                    } else {
                        lines.push(vec![]);
                    }
                }
                if let Some(line) = lines.iter_mut().rev().find(|l| room(l)) {
                    line.push(mark);
                }
                lines
            })
            .collect();
        // As many lines as its longest box takes, the name on the first.
        let h = row.iter().map(Vec::len).max().unwrap_or(0).max(1);
        let mut lines: Vec<_> = (0..h).map(|_| vec![Span::raw(" ".repeat(w + 2))]).collect();
        lines[0] = first;
        for (c, cell) in row.into_iter().enumerate() {
            let (x, room) = (w + 2 + room[..c].iter().map(|r| r + 2).sum::<usize>(), room[c]);
            // A click on the ▾ opens its dropdown, ahead of the box's own spot.
            let mark = cell.iter().enumerate().find_map(|(k, line)| {
                let at = line.iter().position(|s| s.content.contains('▾'))?;
                let before: usize = line[..at].iter().map(Span::width).sum();
                Some((k, x + 1 + before + Span::raw(line[at].content.trim_end_matches('▾')).width()))
            });
            if let Some((k, at)) = mark {
                spots.push(Spot { lines: y + k..y + k + 1, x: at..at + 1, at: Stop::Menu(i, c + 1) });
            }
            spots.push(Spot { lines: y..y + h, x: x..x + room + 2, at: Stop::Recommend(i, c + 1) });
            let on = |(row, sel): (usize, usize)| row == i && sel == c + 1;
            let mut cell = cell.into_iter();
            for line in &mut lines {
                let mut piece = cell.next().unwrap_or_default();
                // Padded to the box, so the cursor's fill is as wide on every one.
                piece.push(Span::raw(" ".repeat(room.saturating_sub(piece.iter().map(Span::width).sum()))));
                line.extend(cursor(cur.is_some_and(on), piece));
            }
        }
        spots.push(Spot { lines: y..y + h, x: 0..width, at: Stop::Recommend(i, 0) });
        if cur.is_some_and(|c| c.0 == i) {
            block = Some(y..y + h);
        }
        v.extend(lines.into_iter().map(Line::from));
    }
    // `+ new task` after the last, where enter or space writes one of your own under the grid.
    let y = v.len();
    let on = cur.is_some_and(|c| c.0 == tasks.len());
    spots.push(Spot { lines: y..y + 1, x: 0..width, at: Stop::Recommend(tasks.len(), 0) });
    if on {
        block = Some(y..y + 1);
    }
    // A new task's name shows on it too, as it is typed, in as many cells.
    let label = match edit.filter(|e| e.0 == tasks.len()).and_then(|e| e.1.fields()) {
        Some((n, _)) if !n.is_empty() => truncate(&format!("+ {n}"), w.max(NEW_TASK.len())),
        _ => NEW_TASK.to_string(),
    };
    v.push(Line::from(cursor(on, vec![Span::raw(format!("{label:w$}"))])));
    let pin = v.len();
    v.push(Line::default());
    // A task's line under the grid: its name and when to use it, and what ranks it where that is
    // not its own score, a benchmark picked in its column or what a task with none goes by.
    let task_line = |i: usize| {
        let t = tasks[i];
        let name = vec![Span::styled(t.0.to_string(), fg(task_color(t.0)).add_modifier(BOLD)), space.clone()];
        let mut when: Vec<Line> = words(t.1);
        if let Some(r) = TASKS.get(i).and_then(|t| app.task_bench(t).or(t.ranked)) {
            // `ranked by` and what stay together on a line.
            when.extend([Line::from("·"), Line::from(format!("ranked by {r}"))]);
        }
        wrapped(name, when, &space, text)
    };
    // A task being written there instead, its name in the style `name` and what it is about on
    // one line, and under it why enter did not take them, or else the `hint`.
    let mut write = |under: &mut Vec<Line<'static>>, e: &Edit, name: Style| {
        let (line, x) = edit_line(e, Span::raw(""), name, text);
        text_at = Some((pin + 1 + under.len(), x));
        under.push(line);
        let hint = match e.what {
            What::Task { about: false, .. } => "enter goes on to what it is about, which agents choose it by",
            What::Task { was: None, .. } => "enter adds the task, tab goes back to its name",
            _ => "enter saves both, tab goes back to its name",
        };
        let (said, style) = match &e.err {
            Some(err) => (err.as_str(), fg(BAD)),
            None => (hint, fg(MUTED)),
        };
        // Keys in the keys' colour, as in the hints.
        let key = |w: Line<'static>| match w.to_string().as_str() {
            "enter" | "tab" => Line::from(Span::styled(w.to_string(), fg(KEY).add_modifier(BOLD))),
            _ => w,
        };
        let said = words(said).into_iter().map(key).collect();
        under.extend(wrapped(vec![], said, &space, text).into_iter().map(|l| l.style(style)));
    };
    // Each task's, once a frame: the longest sizes the lines kept under the grid.
    let task_lines: Vec<_> = (0..tasks.len()).map(task_line).collect();
    let mut under = Vec::new();
    if let Some((i, sel)) = cur.filter(|c| c.0 < tasks.len()) {
        let t = tasks[i];
        // While it is written, the lines under the grid are about that alone, so they keep their
        // height as enter takes it or says why not.
        if let Some((_, e)) = edit.filter(|e| e.0 == i) {
            write(&mut under, e, fg(task_color(t.0)).add_modifier(BOLD));
        } else {
            under.extend(task_lines[i].clone());
        }
        if let Some(c) = sel.checked_sub(1).filter(|&c| c < TIERS.len() && edit.is_none()) {
            let tier = TIERS[c].0;
            // Wrapped to the panel as the boxes are, so the name and what it says are whole.
            let mut put = |label: &str, spans| {
                let head = Span::styled(format!("  {:<w$}", format!("{label}:"), w = "recommended: ".len()), fg(MUTED));
                let indent = head.width();
                for (k, piece) in wrap_spans(spans, text.saturating_sub(indent), &[]).into_iter().enumerate() {
                    let lead = if k == 0 { head.clone() } else { Span::raw(" ".repeat(indent)) };
                    under.push(Line::from([vec![lead], piece].concat()));
                }
            };
            put(tier, entry(app, t.0, c, picks[i][c], took[i][c], false));
            // A favorite in place of the tier's own pick, with what it would recommend.
            let fav = picks[i][c].filter(|(m, _)| crate::view::fav_pick(&app.store, t.0, TIERS[c].0, m));
            if let (Some((f, _)), Some(task)) = (fav, TASKS.get(i)) {
                let (rec, local) = app.recommended(task, TIERS[c].0);
                if let Some((m, s)) = rec.filter(|(m, _)| m.key != f.key) {
                    put("recommended", entry(app, t.0, c, Some((m, s)), local, false));
                }
            }
        }
    }
    if on {
        match edit.filter(|e| e.0 == tasks.len()) {
            Some((_, e)) => write(&mut under, e, Style::new().add_modifier(BOLD)),
            None => under.extend(wrapped(
                vec![Span::styled(format!("{NEW_TASK}: "), fg(MUTED))],
                words("enter adds a task of your own"),
                &space,
                text,
            )),
        }
    }
    // On the tiers' header, or while a cap is written, what the tier picks and how to change it.
    if on_caps || editing.is_some() {
        let c = editing.map_or(app.cap_col(), |e| e.0);
        let what = match TIERS[c].0 {
            t if crate::view::capped(t) => {
                let def = Store::default().tier_cap(t);
                let how = match editing {
                    Some(_) => format!("clear it for the default ${def}"),
                    None => format!("enter changes it (default ${def})"),
                };
                format!("the best model up to ${} per 1M tokens, {how}", app.store.tier_cap(t))
            }
            "free" => "the best free model".into(),
            _ => "the best model at any price".into(),
        };
        under.push(Line::from(vec![Span::styled(format!("{}: ", TIERS[c].0), fg(MUTED)), Span::raw(what)]));
    }
    // As many lines whatever the cursor is on, so the box keeps its height as it moves: the
    // longest task's line, a tier and its `recommended:`.
    let most = task_lines.iter().map(Vec::len).max().unwrap_or(1) + 2;
    under.resize(under.len().max(most), Line::default());
    v.extend(under);
    let pin = pin..v.len();
    // The header being written stays in view with the cursor's row.
    let block = if on_caps { Some(head_y..head_y + 1) } else { block };
    let block = block.map(|b| if editing.is_some() { head_y.min(b.start)..b.end } else { b });
    (v, block.or(Some(0..among)), pin, text_at)
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

/// What tier `col` of `task` picks, for its box of recommend's grid: `name · $price · score` in
/// its price level's colour as in the Price column, with no score on a task of your own, after
/// a ★ in the task's colour when it is your favorite for the tier; grey when that favorite is
/// not one recommended, or is out of reach as its row in the table, and `-` with no model. Out of
/// its `boxed` grid the entry says why it is grey.
/// `local`: the pick is the best model your machine runs, in place of the tier's own
/// (`view::with_local`).
fn entry(
    app: &App,
    task: &str,
    col: usize,
    pick: Option<(&Model, f64)>,
    local: bool,
    boxed: bool,
) -> Vec<Span<'static>> {
    let built_in = fit::task(task);
    let Some((m, s)) = pick else {
        let none = if boxed {
            "-"
        } else if built_in.is_some() {
            "no model"
        } else {
            "no favorite"
        };
        return vec![Span::styled(none, fg(MUTED))];
    };
    let fav = crate::view::fav_pick(&app.store, task, TIERS[col].0, m);
    let off = fav && built_in.is_some_and(|t| app.favorite_unrecommended(t, &m.key));
    let out = out_of_reach(app, m);
    let price = pick_color(app, m, local, off);
    let score = crate::view::scored(s);
    // A score that is not the task's own, an estimate or the overall in its place: grey, as in the table.
    let own = built_in.is_none_or(|t| crate::view::own_score(m, t, app.task_bench(t)));
    let scored = if own { fg(price) } else { fg(MUTED) };
    let (cost, score) = match built_in {
        // On `all`, a download that fits is free here too, once `x` fetches it.
        Some(_) if local => {
            let at = app.size(m).filter(|_| !m.here()).map_or("here".into(), |b| format!("↓ {}", crate::data::gb(b)));
            (format!(" free {at} "), score)
        }
        Some(_) => (format!(" {} ", crate::view::price(m)), score),
        None => (format!(" {}", crate::view::price(m)), String::new()),
    };
    let mut spans = Vec::with_capacity(6);
    if fav {
        spans.push(Span::styled("★ ", fg(task_color(task)).add_modifier(BOLD)));
    }
    // The name, its dot, the price, then the score with its dot, apart for a box too narrow for
    // them all to wrap between the name and the price, and so `align` lines them up in a tier's
    // boxes. The dots are grey, only marking them off, the score's after it a no-break space, so
    // the dot stays with its number.
    let dot = if score.is_empty() { "" } else { "·\u{a0}" };
    spans.extend([
        Span::styled(m.name.clone(), fg(price)),
        Span::styled(" ·", fg(MUTED)),
        Span::styled(cost, fg(price)),
        Span::styled(dot, fg(MUTED)),
        Span::styled(score, scored),
    ]);
    if !boxed {
        // The harness the model runs on (`H`), as `f`'s grid says it, before what else the line
        // says of it, as a list, the harness a `chip` as in Via.
        if let Some(h) = runs_on(&app.store, m) {
            let and = if off || out.is_some() { "," } else { "" };
            spans.push(Span::styled(" via ", fg(MUTED)));
            spans.push(Span::styled(h.to_string(), chip(via_color(&app.store.via_not, h))));
            spans.push(Span::styled(and, fg(MUTED)));
        }
        if off {
            // Both read as a list: "not recommended, not available".
            let said = if out.is_some() { " not recommended," } else { " not recommended" };
            spans.push(Span::styled(said, fg(MUTED).add_modifier(Modifier::ITALIC)));
        }
        spans.extend(out);
    }
    spans
}

/// A tier's model's colour in recommend, its boxes and its dropdown: its price level's as in the
/// Price column, the best your machine runs free's, and grey when `off`, out of reach or with only
/// list prices.
fn pick_color(app: &App, m: &Model, local: bool, off: bool) -> Color {
    match m.quoted() {
        _ if local => LEVEL[0],
        q => q.filter(|q| !(off || q.1 || !app.accessible(m))).map_or(MUTED, |q| LEVEL[level(q.0.blended())]),
    }
}

/// The entries of a tier's boxes (`entry`) lined up: the names padded alike, the prices and the
/// scores after them right-aligned, as numbers are in the table. A box with no model is left as it is.
fn align<'a>(cells: impl Iterator<Item = &'a mut Vec<Span<'static>>>) {
    // The name with its ★, then its dot, the price and the score with its dot: the last four spans.
    let parts = |cell: &[Span]| {
        let n = cell.len().checked_sub(4).filter(|&n| n > 0)?;
        let w = |s: &[Span]| s.iter().map(Span::width).sum::<usize>();
        Some((n, w(&cell[..n]), Span::raw(cell[n + 1].content.trim()).width(), w(&cell[n + 2..])))
    };
    let mut cells: Vec<_> = cells.filter_map(|c| Some((parts(c)?, c))).collect();
    let most = |f: fn(&(usize, usize, usize, usize)) -> usize| cells.iter().map(|c| f(&c.0)).max().unwrap_or(0);
    let (name, price, score) = (most(|p| p.1), most(|p| p.2), most(|p| p.3));
    for ((n, nw, pw, sw), cell) in &mut cells {
        let (pad, ppad) = (" ".repeat(name - *nw), " ".repeat(price - *pw));
        let cost = cell[*n + 1].content.trim().to_string();
        // The dots lined up and the prices and scores after them right-aligned. The padding is
        // where a wrap drops it: before the price, and the score's in no-break spaces after its
        // dot, as `entry` keeps them. A box with no score is padded after its price instead.
        cell[*n].content = format!(" {pad}·").into();
        if cell[*n + 2].content.is_empty() {
            cell[*n + 1].content = format!(" {ppad}{cost} {}", " ".repeat(score - *sw)).into();
        } else {
            cell[*n + 1].content = format!(" {ppad}{cost} ").into();
            cell[*n + 2].content = format!("·{}", "\u{a0}".repeat(1 + score - *sw)).into();
        }
    }
}

/// `spans` in lines of at most `width` cells: broken before a span that starts with a space, as
/// a price after its name, then between words, and a word longer than a line where it reaches the
/// end, so nothing is cut. A part that is a lone dot marks off the part after it: it goes on
/// that part's line, and is dropped where that part starts one. Each piece keeps its span's style. The chars `hits` covers in the
/// first part, where a search found the model name of a box, are drawn as hits, which leaves
/// where it wraps as it was.
fn wrap_spans(spans: Vec<Span<'static>>, width: usize, hits: &[Range<usize>]) -> Vec<Vec<Span<'static>>> {
    type Cells = Vec<(char, Style)>;
    // Back to spans, a run of one style each.
    let spans_of = |line: Cells| {
        let mut v: Vec<Span<'static>> = vec![];
        for (c, style) in line {
            match v.last_mut() {
                Some(s) if s.style == style => s.content.to_mut().push(c),
                _ => v.push(Span::styled(c.to_string(), style)),
            }
        }
        v
    };
    let fits = spans.iter().map(Span::width).sum::<usize>() <= width;
    if fits && hits.is_empty() {
        return vec![spans];
    }
    let mut parts: Vec<Cells> = vec![];
    for s in &spans {
        if parts.is_empty() || s.content.starts_with(' ') {
            parts.push(vec![]);
        }
        parts.last_mut().unwrap().extend(s.content.chars().map(|c| (c, s.style)));
    }
    if let Some(name) = parts.first_mut() {
        for r in hits {
            (name.iter_mut().take(r.end).skip(r.start)).for_each(|c| c.1 = hit_style(c.1));
        }
    }
    if fits {
        return vec![spans_of(parts.concat())];
    }
    // In cells, as the boxes are measured: a wide character takes two.
    let cell = |c: char| Span::raw(&*c.encode_utf8(&mut [0; 4])).width();
    let cells = |s: &[(char, Style)]| s.iter().map(|&(c, _)| cell(c)).sum::<usize>();
    let mut lines: Vec<Cells> = vec![vec![]];
    // On the line after a space, else on a new line, else not at all.
    let put = |lines: &mut Vec<Cells>, piece: &[(char, Style)]| {
        let w = cells(lines.last().unwrap());
        if w + usize::from(w > 0) + cells(piece) <= width {
            let line = lines.last_mut().unwrap();
            line.extend((w > 0).then_some((' ', Style::new())));
            line.extend_from_slice(piece);
        } else if cells(piece) <= width {
            lines.push(piece.to_vec());
        } else {
            return false;
        }
        true
    };
    let mut dot = None;
    for part in &parts {
        let start = part.iter().position(|c| c.0 != ' ').unwrap_or(part.len());
        let end = part.iter().rposition(|c| c.0 != ' ').map_or(start, |e| e + 1);
        let piece = &part[start..end];
        if let [('·', _)] = piece {
            dot = Some(piece);
            continue;
        }
        if let Some(d) = dot.take() {
            let w = cells(lines.last().unwrap());
            if w > 0 && w + 1 + cells(d) + 1 + cells(piece) <= width {
                put(&mut lines, d);
            } else if w > 0 {
                lines.push(vec![]);
            }
        }
        if put(&mut lines, piece) {
            continue;
        }
        for word in piece.split(|c| c.0 == ' ').filter(|w| !w.is_empty()) {
            if put(&mut lines, word) {
                continue;
            }
            if !lines.last().unwrap().is_empty() {
                lines.push(vec![]);
            }
            for &c in word {
                let line = lines.last().unwrap();
                if !line.is_empty() && cells(line) + cell(c.0) > width {
                    lines.push(vec![]);
                }
                lines.last_mut().unwrap().push(c);
            }
        }
    }
    lines.into_iter().map(spans_of).collect()
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
    let big = size.zip(crate::view::room(store)).is_some_and(|(b, r)| b > r);
    let mut lines = detail_rows(m, store, any, get.as_deref(), size).into_iter();
    let title = lines.next().map(|r| r.1).unwrap_or_default();
    // A task's name in its colour, as its ★ in the table: on its fit line and after `favorite:`.
    let named = |t: &str| Span::styled(t.to_string(), fg(task_color(t)).add_modifier(BOLD));
    let lines = lines
        .map(|(task, s)| {
            // Outside the match: an `if let` guard needs a newer Rust than Cargo.toml asks for.
            let fit = task.and_then(|t| Some((t, s.split_once(t)?)));
            match s.split_once(':') {
                Some((k, v)) if k.trim() == "favorite" && v.trim() != "-" => {
                    let pad = v.len() - v.trim_start().len();
                    let mut spans = vec![Span::styled(format!("{k}:"), fg(KEY)), Span::raw(v[..pad].to_string())];
                    for (i, t) in v.trim().split(", ").enumerate() {
                        spans.extend((i > 0).then(|| Span::raw(", ")));
                        spans.push(named(t));
                    }
                    Line::from(spans)
                }
                _ if fit.is_some() => {
                    let (t, (pre, post)) = fit.unwrap_or_default();
                    Line::from(vec![Span::raw(pre.to_string()), named(t), Span::raw(post.to_string())])
                }
                Some((k, v)) if k.trim() == "via" && s.starts_with("  ") => {
                    let (pad, said) = v.split_at(v.len() - v.trim_start().len());
                    let mut spans = vec![Span::styled(format!("{k}:"), fg(KEY)), Span::raw(pad.to_string())];
                    spans.extend(via_spans(m, any, get.is_some(), said, store, Style::new()));
                    Line::from(spans)
                }
                // A download's size grey when it is too big for your machine's memory, as in Via.
                Some((k, v)) if k.trim() == "size" && big => {
                    Line::from(vec![Span::styled(format!("{k}:"), fg(KEY)), Span::styled(v.to_string(), fg(MUTED))])
                }
                Some((k, v)) if s.starts_with("  ") && is_label(k) => {
                    Line::from(vec![Span::styled(format!("{k}:"), fg(KEY)), Span::raw(v.to_string())])
                }
                _ if s.ends_with(':') => heading(&s),
                _ => Line::from(s),
            }
        })
        .collect();
    (title, lines)
}

/// What `compare` drew, for `draw` to scroll it and `App` to move over it.
struct Compared {
    lines: Vec<Line<'static>>,
    /// The column under the cursor, for `App::compare_col`.
    col: usize,
    /// The first column shown, for `App::compare_x`, and how many show, a screen for `‹` `›`.
    first: usize,
    shown: usize,
    /// How many labels the search finds.
    hits: usize,
    /// The line of the model under the cursor.
    at: usize,
}

/// A label wraps no narrower than this, or than itself, so a column of short values still has
/// a label that reads in a line or two.
const LABEL_MIN: usize = 10;

/// `label` in lines of at most `w` chars, broken after a space, `_` or `-`, a word longer than
/// `w` on a line of its own, each with the char it starts at, for the search to underline.
fn wrap_label(label: &str, w: usize) -> Vec<(usize, String)> {
    let mut lines: Vec<(usize, String)> = vec![];
    let mut at = 0;
    for p in label.split_inclusive([' ', '_', '-']) {
        match lines.last_mut() {
            Some((_, l)) if format!("{l}{p}").trim_end().chars().count() <= w => l.push_str(p),
            _ => lines.push((at, p.to_string())),
        }
        at += p.chars().count();
    }
    lines.into_iter().map(|(a, l)| (a, l.trim_end().to_string())).collect()
}

/// The verdict, then a row per marked model and a column per value, as in the table: the best
/// of each column in green and the worst in red, of the models not `muted`, whose rows are grey,
/// and Via last, the widest. A label wraps over the header's lines, so a column is as wide as
/// its values. The cursor's column, `col` or the `jump`th label `query` finds, is marked under
/// its label and filled as in the table, and the one `sort` names, by its label and whether
/// highest first, has `▼` or `▲`, the `marked` models sorted by it as `App::compare_models`
/// sorts them. When the columns do not all fit in `avail` cells, the view starts at column
/// `first`, moved only as far as it takes to show the cursor's. Each model's row and each
/// column's label goes to `spots`. `any` is whether you have access to a model, and `get` the
/// mark of a model's download, for Via to read as in the table.
fn compare(
    marked: &[&Model],
    (any, get, store): (bool, crate::view::Get, &Store),
    (sel, (col, first), jump): (usize, (usize, usize), Option<isize>),
    (avail, sort): (usize, Option<(&str, bool)>),
    query: &str,
    muted: impl Fn(&Model) -> bool,
    spots: &mut Vec<Spot>,
) -> Compared {
    let (mut names, mut cols) = crate::view::compare_cols(marked, any, get, store);
    // Sorted here, so the rows are made once, as they are slow to make: their cells move with
    // their models.
    let mut models = marked.to_vec();
    if let Some(sort) = sort {
        let order = crate::view::compare_order(models.len(), &cols, sort);
        std::iter::once(&mut names).chain(&mut cols).for_each(|r| r.reorder(&order));
        models = order.into_iter().map(|i| marked[i]).collect();
    }
    let models = models.as_slice();
    // The sort's arrow before its label, as on the table's header.
    let arrow = |label: &str| match sort {
        Some((l, desc)) if l == label => {
            if desc {
                "▼"
            } else {
                "▲"
            }
        }
        _ => "",
    };
    let searched: Vec<String> = cols.iter().map(|c| c.label.clone()).collect();
    for c in &mut cols {
        c.label = format!("{}{}", arrow(&c.label), c.label);
    }
    let muted: Vec<bool> = models.iter().map(|m| muted(m)).collect();
    let (n, m) = (models.len(), cols.len());
    // The cells that read as the best and the worst of each column, of the models not muted: a
    // muted one holds neither, as in the table (`App::ext`), so a column does not lose them.
    let ext: Vec<Option<(&str, &str)>> = (cols.iter())
        .map(|r| {
            // None where every cell reads alike, the muted ones' too.
            r.ext?;
            let shown = (0..n).filter(|&i| !muted[i]);
            let mut it = shown.filter_map(|i| Some((r.vals.get(i).copied()??, r.cells[i].as_str())));
            let first = it.next()?;
            let (lo, hi) = it
                .fold((first, first), |(lo, hi), e| (if e.0 < lo.0 { e } else { lo }, if e.0 > hi.0 { e } else { hi }));
            let (best, worst) = if r.higher { (hi.1, lo.1) } else { (lo.1, hi.1) };
            (best != worst).then_some((best, worst))
        })
        .collect();
    let sel = sel.min(n.saturating_sub(1));
    // In cells as the terminal draws them, so a wide character keeps the columns in line.
    let cell_w = |s: &str| Span::raw(s).width();
    let name_w = names.cells.iter().map(|c| cell_w(c)).chain([names.label.chars().count()]).max().unwrap_or(0);
    let widths: Vec<usize> = cols
        .iter()
        .map(|c| {
            let vals = c.cells.iter().map(|s| cell_w(s)).max().unwrap_or(0);
            let words = wrap_label(&c.label, 0).iter().map(|(_, l)| l.chars().count()).max().unwrap_or(0);
            vals.max(words).max(LABEL_MIN.min(c.label.chars().count()))
        })
        .collect();
    let heads: Vec<_> = cols.iter().zip(&widths).map(|(c, &w)| wrap_label(&c.label, w)).collect();
    // A group of columns starts after the names and where the section changes, split from the
    // one before by a `│` in its gap, a cell wider, as in the table.
    let starts = |c: usize, f: usize| c == f || cols[c].section != cols[c - 1].section;
    let gap = |c: usize, f: usize| 2 + usize::from(starts(c, f));
    // How many columns from `f` on fit beside the names, always at least one. The `‹` goes in the
    // gap before the first, the `›` after the last takes a cell, but only when there are more.
    let count = |f: usize, reserve: usize| {
        let mut room = avail.saturating_sub(name_w + reserve);
        let ok = |&(c, w): &(usize, &usize)| {
            let fit = room >= w + gap(c, f);
            room = room.saturating_sub(w + gap(c, f));
            fit
        };
        widths.iter().enumerate().skip(f.min(m)).take_while(ok).count().max(1)
    };
    let fits = |f: usize| {
        let all = count(f, 0);
        if f + all >= m { all } else { count(f, 1) }
    };
    // Found in the label as it reads, after the arrow.
    let found_in = |c: usize| {
        let shift = cols[c].label.chars().count() - searched[c].chars().count();
        found(&searched[c], query).into_iter().map(|r| r.start + shift..r.end + shift).collect::<Vec<_>>()
    };
    let hits: Vec<usize> = (0..m).filter(|&c| !found_in(c).is_empty()).collect();
    let col = jump
        .and_then(|j| hits.get(j.rem_euclid(hits.len().max(1) as isize) as usize))
        .copied()
        .unwrap_or(col)
        .min(m.saturating_sub(1));
    let max_first = (0..m).find(|&f| f + fits(f) >= m).unwrap_or(0);
    let mut first = first.min(col).min(max_first);
    while col >= first + fits(first) {
        first += 1;
    }
    let shown = fits(first).min(m - first.min(m));
    let vis = first..first + shown;
    let mut out = vec![heading("verdict:")];
    out.extend(verdict_lines(models, store, avail));
    out.push(Line::default());
    if shown < m {
        // The arrows keep their keys.
        out.push(key_line(&format!("columns {}-{} of {m} · ← → move", first + 1, first + shown), &BTreeMap::new()));
    }
    // The labels, their last lines on the header's last, as the values under them: right
    // aligned, Via's left.
    let edge = MORE;
    let left = |c: usize| searched[c] == "Via";
    let h = heads.iter().map(Vec::len).max().unwrap_or(1);
    let head_top = out.len();
    for j in 0..h {
        let last = j + 1 == h;
        let name = if last { names.label.as_str() } else { "" };
        let mut spans = vec![Span::styled(format!("{name:<name_w$}"), fg(KEY))];
        for c in vis.clone() {
            if last && c == first && first > 0 {
                spans.extend([Span::styled("‹", edge), Span::styled(SEP, fg(MUTED)), Span::raw(" ")]);
            } else if starts(c, first) {
                spans.extend([Span::raw(" "), Span::styled(SEP, fg(MUTED)), Span::raw(" ")]);
            } else {
                spans.push(Span::raw("  "));
            }
            let Some((at, piece)) = (j + heads[c].len()).checked_sub(h).map(|k| &heads[c][k]) else {
                spans.push(Span::raw(" ".repeat(widths[c])));
                continue;
            };
            let (len, w) = (piece.chars().count(), widths[c]);
            let pad = w - len;
            let (before, after) = if left(c) { (0, pad) } else { (pad, 0) };
            // The search's matches in the label, on the piece of it this line holds.
            let lit: Vec<_> = found_in(c)
                .into_iter()
                .filter_map(|r| {
                    let (s, e) = (r.start.max(*at), r.end.min(at + len));
                    (s < e).then(|| s - at + before..e - at + before)
                })
                .collect();
            let text = format!("{}{piece}{}", " ".repeat(before), " ".repeat(after));
            spans.extend(lit_at(Line::styled(text, fg(KEY)), &lit).spans);
        }
        if last && first + shown < m {
            spans.push(Span::styled("›", edge));
        }
        out.push(Line::from(spans));
    }
    // Each label, from the gap before it, sorts by its column on a click.
    let mut x = name_w;
    for c in vis.clone() {
        let g = gap(c, first);
        spots.push(Spot { lines: head_top..out.len(), x: x + g - 1..x + g + widths[c], at: Stop::CompareCol(c) });
        x += widths[c] + g;
    }
    // The rule under the header, heavy under the cursor's column and crossing the group lines,
    // as in the table. With columns off screen the panel takes the whole width, so it keeps its
    // size as it scrolls.
    let width = if shown < m { avail } else { x };
    let mut rule = vec![Span::styled("─".repeat(name_w), fg(MUTED))];
    for c in vis.clone() {
        rule.push(Span::styled(if starts(c, first) { "─┼─" } else { "──" }, fg(MUTED)));
        let (line, style) = if c == col { ("━", fg(ACCENT)) } else { ("─", fg(MUTED)) };
        rule.push(Span::styled(line.repeat(widths[c]), style));
    }
    rule.push(Span::styled("─".repeat(width.saturating_sub(x)), fg(MUTED)));
    out.push(Line::from(rule));
    let top = out.len();
    for (i, model) in models.iter().enumerate() {
        let plain = if muted[i] { fg(MUTED) } else { Style::new() };
        let name = &names.cells[i];
        let mut spans = vec![Span::styled(format!("{name}{}", " ".repeat(name_w - cell_w(name))), plain)];
        for c in vis.clone() {
            let r = &cols[c];
            let cell = r.cells[i].as_str();
            let style = match ext[c] {
                _ if muted[i] => fg(MUTED),
                // A list price, as in the table: not one you'd pay.
                _ if r.listed.get(i) == Some(&true) => fg(MUTED).add_modifier(Modifier::ITALIC),
                // A score that is not the task's own, as in the table.
                _ if r.estimated.get(i) == Some(&true) => fg(MUTED),
                Some((best, _)) if cell == best => fg(GOOD).add_modifier(BOLD),
                Some((_, worst)) if cell == worst => fg(BAD),
                _ => Style::new(),
            };
            let pad = " ".repeat(widths[c] - cell_w(cell));
            if starts(c, first) {
                spans.extend([Span::raw(" "), Span::styled(SEP, fg(MUTED)), Span::raw(" ")]);
            } else {
                spans.push(Span::raw("  "));
            }
            let from = spans.len();
            if left(c) {
                if muted[i] {
                    spans.push(Span::styled(cell.to_string(), style));
                } else {
                    spans.extend(via_spans(model, any, get(model).is_some(), cell, store, style));
                }
                spans.push(Span::raw(pad));
            } else {
                spans.push(Span::styled(pad + cell, style));
            }
            // The cursor's column on a faint fill, as in the table, a chip keeping its own.
            if c == col {
                spans[from..].iter_mut().filter(|s| s.style.bg.is_none()).for_each(|s| s.style.bg = Some(COLUMN));
            }
        }
        spots.push(Spot { lines: out.len()..out.len() + 1, x: 0..usize::MAX, at: Stop::Compare(i) });
        out.push(Line::from(spans));
    }
    Compared { lines: out, col, first, shown, hits: hits.len(), at: top + sel }
}

/// The verdict as a small table: question, winner in green, margin dimmed.
/// A task's models that do not fit in `width` go on under the first, indented to it.
fn verdict_lines(models: &[&Model], store: &Store, width: usize) -> Vec<Line<'static>> {
    let rows = verdict(models, store);
    let w = |i: usize| rows.iter().map(|r| r[i].chars().count()).max().unwrap_or(0);
    let (w0, w1) = (w(0), w(1));
    let glue = Span::styled(" · ", fg(MUTED));
    rows.into_iter()
        .flat_map(|[q, win, margin]| {
            let label = vec![
                Span::styled(format!("  {q:<w0$}  "), fg(KEY)),
                Span::styled(format!("{win:<w1$}  "), fg(GOOD).add_modifier(BOLD)),
            ];
            let items = margin.split(" · ").map(|m| Line::styled(m.to_string(), fg(MUTED))).collect();
            wrapped(label, items, &glue, width)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{Back, ECI, NCOLS};
    use crate::data::Offer;
    use ratatui::crossterm::event::KeyCode;

    /// Killed from outside, the TUI gives the terminal back and exits as the signal would. The
    /// signal ends the process, so the test runs itself in a child to be killed.
    #[cfg(unix)]
    #[test]
    fn a_kill_gives_the_terminal_back() {
        if std::env::var_os("MODELCMP_KILLED").is_some() {
            TAKEN.store(true, Ordering::SeqCst);
            restore_on_signals();
            let _ = signal_hook::low_level::raise(signal_hook::consts::SIGTERM);
            std::thread::sleep(Duration::from_secs(5));
            return;
        }
        let out = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "tui::tests::a_kill_gives_the_terminal_back", "--nocapture"])
            .env("MODELCMP_KILLED", "1")
            .output()
            .unwrap();
        let screen = String::from_utf8_lossy(&out.stdout);
        assert_eq!(out.status.code(), Some(128 + 15), "the status a shell gives a SIGTERM");
        assert!(screen.contains("\x1b[?1000l") && screen.contains("\x1b[?2004l"), "mouse reporting and paste off");
        assert!(screen.contains("\x1b[?25h") && screen.contains("\x1b[?1049l"), "the cursor shown, the shell's screen");
    }

    #[test]
    fn the_upgrade_is_the_installer_s() {
        let cmd = |exe: &str| upgrade_cmds(std::path::Path::new(exe), "0.2.0").map(|c| c.last().unwrap().join(" "));
        let brew = Some("brew upgrade danctorres/tap/modelcmp".to_string());
        assert_eq!(cmd("/opt/homebrew/Cellar/modelcmp/0.1.0/bin/modelcmp"), brew);
        assert_eq!(cmd("/home/linuxbrew/.linuxbrew/Cellar/modelcmp/0.1.0/bin/modelcmp"), brew);
        let cargo = format!("cargo install --locked --git {REPO} --tag v0.2.0");
        assert_eq!(cmd("/home/you/.cargo/bin/modelcmp"), Some(cargo));
        assert_eq!(cmd("/usr/local/bin/modelcmp"), None, "put there by hand: nothing to run");
        assert_eq!(cmd(""), None, "the path is not known");
    }

    #[test]
    fn the_logo_rows_line_up() {
        for logo in LOGOS {
            let w = logo[0].chars().count();
            assert!(logo.iter().all(|r| r.chars().count() == w), "centring and the band assume one width");
            assert!(
                TAGLINE.is_ascii() && TAGLINE.len() <= w,
                "the intro slices it by byte and centres it in that width"
            );
        }
    }

    fn model(name: &str, dev: &str, eci: Option<f64>, price: f64) -> Model {
        Model {
            key: name.into(),
            name: name.into(),
            developer: dev.into(),
            available: true,
            via: vec!["opencode".into()],
            eci,
            fit: eci.map(|e| ("overall".to_string(), (e, crate::fit::Basis::Eci))).into_iter().collect(),
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
        assert!(
            lines[3].contains("d dropdown  % no cache  r refresh  │  enter details  x launch  H harness"),
            "{}",
            lines[3]
        );
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
        data.models[0].via = crate::data::vias().map(String::from).collect();
        a.set_data(data);
        let (_, lines) = render(&mut a, 222, 5);
        let via = lines[0].find("Via").unwrap();
        assert!(lines[2].trim_end().ends_with("│ oc pi omp cc cx gem cp oll llm"), "one line, short: {lines:?}");
        // A click finds the harness under it, as `draw` puts the frame around the table.
        let (area, x) = (Rect::new(0, 0, 224, 10), lines[0][..via].chars().count() as u16 + 1);
        let click = |column| MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column,
            row: 5,
            modifiers: ratatui::crossterm::event::KeyModifiers::NONE,
        };
        assert_eq!(hit(&a, area, click(x + 13)), Some(Mouse::Harness(0, 4)), "cx is codex");
        assert_eq!(hit(&a, area, click(x + 12)), Some(Mouse::Row(0)), "the gap before it is none");
        // A search by the harness's name underlines its short one, whole.
        a.query = "codex".into();
        let (buf, _) = render(&mut a, 215, 5);
        let on = |x: u16| buf[(x, 2)].modifier.contains(Modifier::UNDERLINED);
        let x = x - 1 + 13;
        assert!(on(x) && on(x + 1) && !on(x - 1) && !on(x + 2), "cx alone");
        // And so does one by the short name itself.
        a.query = "cx".into();
        a.rebuild();
        let (buf, _) = render(&mut a, 215, 5);
        let on = |x: u16| buf[(x, 2)].modifier.contains(Modifier::UNDERLINED);
        assert!(a.rows.len() == 1 && on(x) && on(x + 1) && !on(x + 3), "cx finds codex");
        a.query.clear();
        a.rebuild();
        // A harness left out in the dropdown is grey, the model still showing for its others.
        a.store.via_not = vec!["codex".into()];
        let (buf, lines) = render(&mut a, 215, 5);
        let at = |s: &str| lines[2][..lines[2].find(s).unwrap()].chars().count() as u16;
        let (cx, oc) = (at("cx gem"), at("oc pi"));
        assert_eq!((buf[(cx, 2)].fg, buf[(oc, 2)].fg), (MUTED, dev_color("opencode")), "cx grey, oc not");
        a.store.via_not.clear();
        // The dropdown says which harness a short name is.
        a.col = VIA;
        a.key(KeyCode::Char('d').into());
        let (_, lines) = render(&mut a, 200, 14);
        assert!(lines.iter().any(|l| l.contains("☐ · opencode oc ")), "{lines:#?}");
        assert!(lines.iter().any(|l| l.contains("☐ · pi  ")), "pi is pi: {lines:#?}");
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
        recolor(&mut buf, THEMES[crate::view::theme("nord")].1.as_ref(), None, None);
        // A marked row's ✗: nord's red, on its background with a little of the light blue in it.
        assert_eq!((buf[(2, 0)].fg, buf[(2, 0)].bg), (Color::from_u32(0xbf616a), Color::from_u32(0x3c4654)));
        // A pill: nord's background as the text, and the light blue kept, being the half of blue
        // that stands furthest from it.
        assert_eq!((buf[(0, 0)].fg, buf[(0, 0)].bg), (Color::from_u32(0x2e3440), Color::from_u32(0xa3d0e8)));
        assert_eq!((buf[(1, 0)].fg, buf[(1, 0)].bg), (Color::from_u32(0xd8dee9), Color::from_u32(0x2e3440)));
        let mut buf = Buffer::empty(Rect::new(0, 0, 1, 1));
        recolor(&mut buf, None, None, None);
        assert_eq!(buf[(0, 0)].bg, Color::Reset, "the terminal's own colours keep its background");
        // A harness's chip is filled with the colour its name has when it is not the chosen one.
        for theme in [None, THEMES[crate::view::theme("nord")].1.as_ref()] {
            for term_bg in [None, Some(0x1e1e2e)] {
                let mut buf = Buffer::empty(Rect::new(0, 0, 2, 1));
                buf[(0, 0)].set_style(fg(dev_color("claude")));
                buf[(1, 0)].set_style(chip(dev_color("claude")));
                recolor(&mut buf, theme, term_bg, None);
                assert_eq!(buf[(1, 0)].bg, buf[(0, 0)].fg, "{term_bg:?}");
                let full = buf[(1, 0)].bg;
                // Behind a box, its fill half mixed into the background, so it fades as the text
                // around it, once there are colours to mix: else its colour as text.
                let slots: [u32; 16] = std::array::from_fn(|i| 0x0f0f0f * i as u32);
                for s in [None, Some(&slots)] {
                    let mut buf = Buffer::empty(Rect::new(0, 0, 2, 1));
                    buf[(0, 0)].set_style(fg(dev_color("claude")).add_modifier(Modifier::DIM));
                    buf[(1, 0)].set_style(chip(dev_color("claude")).add_modifier(Modifier::DIM));
                    recolor(&mut buf, theme, term_bg, s);
                    let (name, pill) = (&buf[(0, 0)], &buf[(1, 0)]);
                    let mixed = match (theme, term_bg, s) {
                        (Some(p), ..) => Some(wash(full, p.bg, PILL_FADE)),
                        (None, Some(b), Some(s)) => Some(wash(Color::from_u32(s[ansi(full).unwrap()]), b, PILL_FADE)),
                        _ => None,
                    };
                    match mixed {
                        Some(c) => assert_eq!(pill.bg, c, "{term_bg:?} {s:?}"),
                        None => assert_eq!((pill.fg, pill.bg), (name.fg, name.bg), "{term_bg:?}"),
                    }
                }
            }
        }
        let reply = "]11;rgb:1e1e/1e1e/2e2e".to_string()
            + &(0..16).map(|i| format!("]4;{i};rgb:{i:02x}{i:02x}/0000/ffff\\")).collect::<String>();
        let slots = parse_ansi(&reply).unwrap();
        assert_eq!((slots[0], slots[11], slots[15]), (0x0000ff, 0x0b00ff, 0x0f00ff));
        assert_eq!(parse_bg(&reply), Some(0x1e1e2e), "the background's reply comes first");
        assert_eq!(parse_bg(&reply[22..]), None, "a slot's colour is not the background");
        assert_eq!(parse_ansi("]11;rgb:1e1e/1e1e/2e2e]4;0;rgb:0000/0000/0000"), None, "all 16 or none");
        // As the tty sends them, each ended by BEL or ST, and the cursor's position last.
        let raw = "\x1b]11;rgb:1e1e/1e1e/2e2e\x07".to_string()
            + &(0..16).map(|i| format!("\x1b]4;{i};rgb:{i:02x}{i:02x}/0000/ffff\x1b\\")).collect::<String>();
        assert_eq!((parse_bg(&raw), parse_ansi(&raw)), (Some(0x1e1e2e), Some(slots)));
        assert!(!positioned(&raw) && !positioned(&format!("{raw}\x1b[24;1")), "not until its R is in");
        assert!(positioned(&format!("{raw}\x1b[24;1R")), "then nothing more is waited for");
        assert!(!positioned("\x1b[A\x1b[1;5A"), "an arrow key is no position");
        // A marked row with them: a solid fill, or a faint one mixed from the terminal's
        // background once it told it, which leaves a pill as it is.
        let mut buf = Buffer::empty(Rect::new(0, 0, 2, 1));
        buf[(0, 0)].set_fg(BAD).set_bg(MARK);
        buf[(1, 0)].set_fg(Color::Black).set_bg(MARK);
        recolor(&mut buf, None, None, None);
        assert_eq!(buf[(0, 0)].bg, MARK);
        let mut behind = Buffer::empty(Rect::new(0, 0, 1, 1));
        behind[(0, 0)].set_style(fg(Color::Black).bg(MARK).add_modifier(Modifier::DIM));
        recolor(&mut behind, None, None, None);
        assert_eq!(behind[(0, 0)].bg, MARK, "a selected row behind a box keeps its fill");
        recolor(&mut buf, None, parse_bg("]11;rgb:1e1e/1e1e/2e2eg"), None);
        assert_eq!((buf[(0, 0)].fg, buf[(0, 0)].bg), (BAD, Color::from_u32(0x252d47)));
        assert_eq!(buf[(1, 0)].bg, MARK);
        // The cursor's is a grey: white in a dark background, black in a light one, or the
        // theme's text colour in its background.
        for (theme, bg, fill) in [(None, 0x1e1e2e, 0x30303e), (None, 0xffffff, 0xeaeaea), (Some("nord"), 0, 0x3b414d)] {
            let mut buf = Buffer::empty(Rect::new(0, 0, 1, 1));
            buf[(0, 0)].set_bg(CURSOR);
            recolor(&mut buf, theme.and_then(|t| THEMES[crate::view::theme(t)].1.as_ref()), Some(bg), None);
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

    /// The cursor's column is told from the background in every theme, its row more so.
    #[test]
    fn the_column_reads() {
        for (name, p) in THEMES.iter().filter_map(|(n, p)| p.as_ref().map(|p| (n, p))) {
            let [column, cursor] = [COLUMN, CURSOR].map(|c| hex(fill(c, p)));
            let (col, row) = (apart(column, p.bg), apart(cursor, p.bg));
            assert!(col >= 20.0, "{name}: the column's fill {column:06x} looks like the background, {col:.0}");
            assert!(row > col, "{name}: the row's fill is no stronger than the column's");
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
        a.store.toggle_favorite("coding:high", "flash");
        a.store.toggle_excluded("flash");
        a.rebuild();
        for (tab, hint) in [('F', "D unfavorite all"), ('E', "X unexclude all")] {
            assert!(!hints(&a, 200).contains(&hint), "{hint}: not outside {tab}");
            a.key(KeyCode::Char(tab).into());
            assert!(hints(&a, 200).contains(&hint), "{hint}: a hint in {tab}");
            a.key(KeyCode::Char(tab).into());
        }
        a.store.excluded.clear();
        a.store.dev_not = vec!["anthropic".into()];
        a.rebuild();
        assert!(hints(&a, 200).contains(&"X unexclude all"), "out of E too while a dropdown hides models");
        a.key(KeyCode::Char('E').into());
        assert!(hints(&a, 200).contains(&"X unexclude all"), "for the ones a dropdown leaves out too");
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
        let deselect = |a: &App| ["u deselect", "V highlight"].iter().all(|h| hints(a, 200).contains(h));
        assert!(deselect(&a) && !deselect(&app()), "u and V are hints while a model is selected");
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
        let (buf, lines) = render(&mut a, 203, 5);
        let opus = lines.iter().position(|l| l.contains("opus")).unwrap() as u16;
        let at = |pat: &str| buf[(cell(&lines[opus as usize], pat), opus)].fg;
        assert_eq!(at("opus"), MUTED, "{lines:?}");
        assert_eq!(at("anthropic"), MUTED, "the developer's colour gives way");
        assert_eq!(at("5.0"), MUTED, "and so does the price level's");
        assert_eq!(at("150"), MUTED, "and the best's: the extremes are of the models to use");
        // Marked as well, a favorite and excluded: filled all the same. With the terminal's own
        // colours the fill is solid, so the ★ and the ✗ are black too.
        a.store.marked.push("opus".into());
        a.store.toggle_favorite("coding:high", "opus");
        a.store.toggle_excluded("opus");
        let (buf, lines) = render(&mut a, 203, 5);
        assert!(lines[opus as usize].contains('★') && lines[opus as usize].contains('✗'), "{lines:?}");
        assert!((0..203).all(|x| buf[(x, opus)].bg == MARK && buf[(x, opus)].fg == Color::Black), "{lines:?}");
        assert!(buf[(cell(&lines[opus as usize], "opus"), opus)].modifier.contains(BOLD));
        assert!(lines[opus as usize].contains("not available"), "{lines:?}");
        // A search hit on the fill has the yellow behind it, still black.
        a.query = "opus".into();
        let (buf, lines) = render(&mut a, 203, 5);
        let hit = &buf[(cell(&lines[opus as usize], "opus"), opus)];
        assert_eq!((hit.fg, hit.bg), (Color::Black, MATCH), "{lines:?}");
        // A theme's fill is faint (`recolor`), so there the row keeps its colours on it.
        a.store.theme = "nord".into();
        let (buf, lines) = render(&mut a, 203, 5);
        let at = |pat: &str| buf[(cell(&lines[opus as usize], pat), opus)].fg;
        assert!((0..203).all(|x| buf[(x, opus)].bg == MARK), "{lines:?}");
        assert_eq!([at("✓"), at("★"), at("✗"), at("opus")], [MARK, STAR, BAD, MATCH], "{lines:?}");
        a.query.clear();
        let (buf, lines) = render(&mut a, 203, 5);
        let at = |pat: &str| buf[(cell(&lines[opus as usize], pat), opus)].fg;
        assert_eq!([at("opus"), at("anthropic")], [MUTED, MUTED], "muted as off the fill: {lines:?}");
        a.store.theme.clear();
        // And so with the terminal's own colours, once it told its background.
        a.term_bg = Some(0);
        let (buf, lines) = render(&mut a, 203, 5);
        assert_eq!(buf[(cell(&lines[opus as usize], "★"), opus)].fg, STAR, "{lines:?}");
        a.term_bg = None;
        // In compare its row is muted.
        let rows = compare(
            &a.marked_models(),
            (a.any_available(), &|_| None, &a.store),
            (0, (0, 0), None),
            (200, None),
            "",
            |m| a.muted(m),
            &mut vec![],
        )
        .lines;
        let row = rows.iter().rfind(|l| l.to_string().contains("opus")).unwrap();
        assert!(row.spans.iter().all(|s| s.content.trim().is_empty() || s.style.fg == Some(MUTED)), "{row:?}");
        // Back in the available view it shows only for being marked, and says so.
        a.key(KeyCode::Char('m').into());
        let (_, lines) = render(&mut a, 203, 5);
        let opus = lines.iter().position(|l| l.contains("opus")).unwrap();
        assert!(lines[opus].contains("not available"), "{lines:?}");
        assert!(lines[4].starts_with(" NORMAL  1 available + 1 not available"), "{}", lines[4]);
        // Its tab counts it, and a click past the count is still on the next tab.
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(181, 20)).unwrap();
        term.draw(|f| draw(&mut a, f)).unwrap();
        let tabs = &text(term.backend().buffer())[1];
        assert!(tabs.contains(" mine +1 m "), "{tabs}");
        assert_eq!(tab_ends(&a).next(), Some(13));
    }

    #[test]
    fn the_frame_says_a_sort_by_size() {
        let mut a = app();
        let top = |a: &mut App| {
            let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 12)).unwrap();
            term.draw(|f| draw(a, f)).unwrap();
            text(term.backend().buffer()).join("\n")
        };
        (a.sort_col, a.descending) = (VIA, false);
        a.rebuild();
        assert!(top(&mut a).contains(" ▲ by Via "), "{}", top(&mut a));
        a.store.via_pick = vec![crate::app::DOWNLOAD.into()];
        a.rebuild();
        assert!(top(&mut a).contains(" ▲ by Via size "), "{}", top(&mut a));
    }

    #[test]
    fn a_click_on_the_more_marker_pages_past_a_column_cut_short_at_the_start() {
        // A narrow screen and a wide Via: Via starts the view cut short, and `›` still goes on to
        // Notes rather than to Via again.
        let mut a = app();
        a.data.models[0].via = crate::data::vias().map(String::from).collect();
        a.col = VIA;
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(44, 12)).unwrap();
        term.draw(|f| draw(&mut a, f)).unwrap();
        a.more = more_at(term.backend().buffer());
        let l = layout(42, &a);
        assert_eq!((l.first, l.next), (VIA - 1, Some(NOTES)), "Via starts the view, cut short");
        let &(column, row, _) = a.more.iter().find(|(.., m)| *m == Mouse::Cols(1)).expect("a ›");
        let e =
            MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column, row, modifiers: KeyModifiers::NONE };
        a.mouse(hit(&a, Rect::new(0, 0, 44, 12), e).unwrap());
        term.draw(|f| draw(&mut a, f)).unwrap();
        assert_eq!((a.col, a.hscroll), (NOTES, NOTES - 1));
    }

    #[test]
    fn a_click_on_a_more_marker_scrolls_that_way() {
        // A phone's terminal sends taps and no sideways wheel: the `›` pages the columns, the
        // first one cut off starting the view, and `‹` back, the one before it ending the view.
        let mut a = app();
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(60, 12)).unwrap();
        let area = Rect::new(0, 0, 60, 12);
        let mut draw_at = |a: &mut App| {
            term.draw(|f| draw(a, f)).unwrap();
            a.more = more_at(term.backend().buffer());
        };
        let click = |a: &App, dir: isize| {
            let &(column, row, _) = a.more.iter().find(|(.., m)| *m == Mouse::Cols(dir)).expect("a marker");
            let e = MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column,
                row,
                modifiers: KeyModifiers::NONE,
            };
            hit(a, area, e)
        };
        draw_at(&mut a);
        let cut = layout(58, &a).next.expect("columns cut off");
        assert_eq!(click(&a, 1), Some(Mouse::ToCol(cut - 1, cut)));
        a.mouse(Mouse::ToCol(cut - 1, cut));
        draw_at(&mut a);
        assert_eq!((a.col, layout(58, &a).first), (cut, cut - 1), "the first column cut off starts the view");
        let Some(Mouse::ToCol(0, back)) = click(&a, -1) else { panic!("{:?}", click(&a, -1)) };
        assert!(back < cut);
        a.mouse(Mouse::ToCol(0, back));
        draw_at(&mut a);
        assert_eq!((a.col, layout(58, &a).next), (back, Some(cut)), "the one before the view ends it");
        // Under a prompt it is a click, which leaves it, where the wheel is still.
        a.key(KeyCode::Char('/').into());
        a.mouse(Mouse::Cols(1));
        assert!(matches!(a.input, Input::Search { .. }));
        a.mouse(click(&a, -1).unwrap());
        assert_eq!(a.input, Input::None);
    }

    #[test]
    fn a_click_on_the_down_marker_moves_one_page() {
        // As `PgDn` does: a page, not the wheel's three of them.
        let mut a = app();
        let mut data = std::mem::take(&mut a.data);
        data.models.extend((0..40).map(|i| model(&format!("m{i}"), "x", Some(i as f64), 1.0)));
        a.set_data(data);
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 16)).unwrap();
        term.draw(|f| draw(&mut a, f)).unwrap();
        a.more = more_at(term.backend().buffer());
        let &(column, row, _) = a.more.iter().find(|&&(x, ..)| x == 0).expect("a ▼ on the left border");
        let e =
            MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column, row, modifiers: KeyModifiers::NONE };
        let before = a.table.selected().unwrap_or(0);
        a.mouse(hit(&a, Rect::new(0, 0, 80, 16), e).unwrap());
        assert_eq!(a.table.selected().unwrap_or(0) - before, a.page as usize);
    }

    #[test]
    fn a_click_on_a_key_in_help_moves_to_it() {
        let mut a = app();
        a.key(KeyCode::Char('?').into());
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 30)).unwrap();
        term.draw(|f| draw(&mut a, f)).unwrap();
        let (rect, _) = *a.spots.iter().find(|(_, s)| *s == Stop::Help(2)).expect("a spot per row");
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: rect.x,
            row: rect.y,
            modifiers: KeyModifiers::NONE,
        };
        let hit = hit(&a, Rect::new(0, 0, 120, 30), click);
        assert_eq!(hit, Some(Mouse::Open(Stop::Help(2))));
        a.mouse(double(&mut None, hit.unwrap(), Instant::now()));
        assert_eq!(a.help_sel, 2);
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
        assert!(lines[3].contains("opus") && lines[5].starts_with(" NORMAL  2 all (none available)"), "{lines:?}");
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
        let (buf, lines) = render(&mut a, 215, 4);
        assert_eq!((a.rows.len(), words(&lines[2])[4]), (1, "opus"), "notes are searched");
        let x = cell(&lines[2], "refactors");
        let under = |x: u16| buf[(x, 2)].modifier.contains(Modifier::UNDERLINED);
        assert!((x..x + 5).all(under) && !under(x + 5), "refac of the note");
        a.query = "openc".into();
        a.rebuild();
        let (buf, lines) = render(&mut a, 215, 4);
        assert_eq!(a.rows.len(), 2, "Via is searched");
        let x = cell(&lines[2], "│ oc") + 2;
        assert!(buf[(x, 2)].modifier.contains(Modifier::UNDERLINED), "openc is the Via column's oc");
    }

    #[test]
    fn wide_table_shows_every_column_and_extremes() {
        let mut a = app();
        let (buf, lines) = render(&mut a, 229, 6);
        let header = "# Model Dev ▾ Released │ Price ▾ $in $cache $out Ctx │ ▼ECI Arena ▾ Coding ▾ Agentic ▾ │ Tok/s │ Via ▾ Notes";
        assert_eq!(words(&lines[0]), words(header));
        // A rule under the header, crossing the lines between the groups of columns.
        assert!(lines[1].starts_with('─') && lines[1].matches('┼').count() == 4, "{}", lines[1]);
        let sep = cell(&lines[0], "│");
        assert_eq!((cell(&lines[1], "┼"), cell(&lines[3], "│")), (sep, sep), "the group lines run straight down");
        assert_eq!(buf[(sep, 3)].fg, MUTED, "and are muted");
        let via = lines[3].rfind("oc").expect(&lines[3]);
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
                "← → column  │  | columns  / filter  s sort  │  enter details  x launch  H harness  o open  y copy name  space select  f fav  e exclude  i note  │  q quit"
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
        // A task shows its own scores, so its columns offer no dropdown while one is picked. Arena's does.
        a.col = ECI + 2;
        let (_, lines) = render(&mut a, 206, 6);
        assert!(lines[0].matches('▾').count() == 6 && lines[5].contains("d dropdown"), "{lines:?}");
        assert!(a.col_about(a.col).ends_with("blank = not tested, d for other benchmarks"), "what its dropdown is for");
        a.task = fit::task("agentic");
        let (_, lines) = render(&mut a, 206, 6);
        assert!(lines[0].matches('▾').count() == 4 && !lines[5].contains("d dropdown"), "{lines:?}");
        assert!(a.col_about(a.col).ends_with("blank = not tested"), "no dropdown, no hint");
        // An entry left out or picked turns its header's ▾ yellow, and that one alone.
        let yellow = |a: &mut App| {
            let (buf, lines) = render(a, 206, 6);
            let on = |x: &u16| buf[(*x, 0)].symbol() == "▾" && buf[(*x, 0)].fg == Color::Yellow;
            let at =
                |h: &str| lines[0][..lines[0].find(h).unwrap()].chars().count() as u16 + h.chars().count() as u16 - 1;
            ((0..206).filter(on).collect::<Vec<_>>(), at("Dev ▾"), at("Price ▾"), at("Via ▾"))
        };
        a.store.price_not = Some(0);
        let (on, dev, price, via) = yellow(&mut a);
        assert_eq!(on, [price], "left out");
        (a.store.price_not, a.store.dev_pick, a.store.via_not) = (None, vec!["OpenAI".into()], vec!["claude".into()]);
        assert_eq!(yellow(&mut a).0, [dev, via], "picked on Dev, left out on Via");
        // A benchmark picked keeps the header's name, marked `*`, its ▾ yellow.
        (a.task, a.store.dev_pick, a.store.via_not) = (None, vec![], vec![]);
        for k in [KeyCode::Char('d'), KeyCode::Char('G'), KeyCode::Char(' '), KeyCode::Esc] {
            a.key(k.into());
        }
        let bench = a.col_bench(a.col).expect("a benchmark picked");
        let (on, lines) = (yellow(&mut a).0, render(&mut a, 206, 6).1);
        let coding = lines[0][..lines[0].find("Coding* ▾").unwrap()].chars().count() as u16 + 8;
        assert!(on == [coding] && !lines[0].contains(bench), "{on:?} {lines:?}");
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
        assert!(from(&lines[1]).starts_with("╭──────────────────╮"), "{}", lines[1]);
        assert_eq!(buf[(dev as u16, 1)].fg, Color::Reset, "its border in the text's colour, as every box's");
        assert_eq!(&words(&lines[2])[5..10], ["▌", "✓", "all", "2", "▐"], "all is ticked, and has no ✗ box");
        assert_eq!(cell(&lines[2], "all"), cell(&lines[3], "anthropic"), "the names line up all the same");
        assert_eq!(&words(&lines[3])[5..10], ["│", "☐", "·", "anthropic", "1"]);
        let ex = dev as u16 + 4;
        assert_eq!((buf[(ex, 3)].symbol(), buf[(ex, 3)].fg), ("·", MUTED), "grey until left out");
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: ex + 1,
            row: 6,
            modifiers: KeyModifiers::NONE,
        };
        // On the screen the table is inside the frame, under the tabs.
        let area = Rect::new(0, 0, 172, 12);
        assert_eq!(hit(&a, area, click), Some(Mouse::Exclude(1)), "a click on the ✗ box leaves the entry out");
        assert_eq!(hit(&a, area, MouseEvent { column: ex - 1, ..click }), Some(Mouse::Item(1)), "elsewhere picks it");
        assert_eq!(hit(&a, area, MouseEvent { row: 5, ..click }), Some(Mouse::Item(0)), "all has no ✗: picked too");
        a.mouse(Mouse::Exclude(1));
        let (out, _) = render(&mut a, 170, 8);
        assert_eq!((out[(ex, 3)].symbol(), out[(ex, 3)].fg), ("✗", BAD), "red when left out");
        a.mouse(Mouse::Exclude(1));
        assert_eq!(buf[(dev as u16 + 2, 2)].bg, CURSOR, "all is under the cursor");
        assert_eq!((buf[(dev as u16 + 2, 2)].fg, buf[(dev as u16 + 2, 3)].fg), (MARK, MUTED), "off, the mark is grey");
        assert_eq!(buf[(dev as u16 + 6, 3)].fg, dev_color("anthropic"), "the name keeps its colour");
        // Where grey shows, off is the mark's own glyph, as on a row of the table.
        a.term_bg = Some(0);
        let (grey, _) = render(&mut a, 170, 8);
        assert_eq!([2, 4].map(|x| grey[(dev as u16 + x, 3)].symbol()), ["✓", "✗"]);
        assert_eq!([2, 4].map(|x| grey[(dev as u16 + x, 3)].fg), [MUTED, MUTED]);
        a.term_bg = None;
        assert_eq!(buf[(dev as u16, 2)].bg, CURSOR, "the cursor runs through the border");
        assert_ne!(buf[(dev as u16, 3)].bg, CURSOR);
        assert!(lines[7].starts_with(" PICK  Dev ▾"), "{}", lines[7]);
        let (_, short) = render(&mut a, 170, 6);
        assert!(short.iter().any(|l| l.contains(" 1-2 of 3 ")), "a list scrolled says where it is: {short:?}");
        for c in "/anth".chars() {
            a.key(KeyCode::Char(c).into());
        }
        let (_, lines) = render(&mut a, 170, 8);
        let anth = lines.iter().find(|l| l.contains("anthropic")).unwrap();
        assert_eq!(&words(anth)[5..10], ["▌", "☐", "·", "anthropic", "1"], "the cursor on the match");
        assert!(lines.iter().any(|l| l.contains("google")), "every entry stays");
        assert!(lines[7].starts_with(" PICK  Dev ▾ /anth"), "{}", lines[7]);
        a.key(KeyCode::Char('z').into());
        let (_, lines) = render(&mut a, 170, 8);
        assert!(lines.iter().any(|l| l.contains(" no match ")), "{lines:?}");
    }

    #[test]
    fn recommend_underlines_what_n_stops_on() {
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(200, 40)).unwrap();
        let mut a = app();
        a.view = View::Recommend;
        let mut underlined = |a: &mut App| {
            term.draw(|f| draw(a, f)).unwrap();
            term.backend().buffer().content().iter().filter(|c| c.modifier.contains(Modifier::UNDERLINED)).count()
        };
        a.overlay_query = "price".into();
        assert_eq!(underlined(&mut a), 0, "the legend says price, but n stops on no legend");
        a.overlay_query = "-".into();
        assert_eq!(underlined(&mut a), 0, "nor on an empty box's -");
        a.overlay_query = "coding".into();
        assert!(underlined(&mut a) > 0, "a task's name is underlined");
    }

    #[test]
    fn help_underlines_what_n_stops_on_and_shows_the_keys_you_set() {
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(200, 120)).unwrap();
        let mut a = app();
        a.key(KeyCode::Char('?').into());
        let mut screen = |a: &mut App| {
            term.draw(|f| draw(a, f)).unwrap();
            let buf = term.backend().buffer();
            let lit = buf.content().iter().filter(|c| c.modifier.contains(Modifier::UNDERLINED)).count();
            (lit, text(buf).join("\n"))
        };
        a.overlay_query = "see the top".into();
        assert_eq!(screen(&mut a).0, 0, "the footer says see the top, but n stops on no footer");
        a.overlay_query = "column".into();
        assert!(screen(&mut a).0 > 0, "a key's row is underlined");
        // A key named in what a row does, and the box `q` opens, are drawn as bound.
        a.store.keys = BTreeMap::from([('C', 'z'), ('z', 'C'), ('q', 'Q')]);
        a.store.keys.remove(&'z');
        let help = screen(&mut a).1;
        assert!(help.contains("select the model, z compares the selected"), "{help}");
        a.input = Input::Quit;
        assert!(screen(&mut a).1.contains("Q confirms"), "the key that confirms");
    }

    #[test]
    fn a_click_on_a_hint_presses_its_key() {
        use ratatui::crossterm::event::KeyModifiers;
        assert_eq!(split_hint("← → 0 $ model"), ("← → 0 $", " model"));
        assert_eq!(split_hint("enter best per price"), ("enter", " best per price"));
        // Under a list, each part's keys in their colour and the rest muted.
        let line = key_line(" ↓ ↑ move · esc t close · ctrl+c quits", &BTreeMap::new());
        assert_eq!(line.to_string(), " ↓ ↑ move · esc t close · ctrl+c quits");
        let keys = line.spans.iter().filter(|s| s.style.fg == Some(KEY)).map(|s| &*s.content);
        assert_eq!(keys.collect::<Vec<_>>(), ["↓ ↑", "esc t", "ctrl+c"]);
        assert_eq!(
            (hint_key("s sort"), hint_key("esc back"), hint_key("space select"), hint_key("↓ ↑ scroll")),
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
        let data = std::mem::take(&mut a.data);
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
        assert_eq!(on(9, 0), Some(Mouse::Tab(0)), "a tab is still that tab");
        assert_eq!(
            at(&mut a, "flash", KeyModifiers::CONTROL),
            Some(Mouse::Model(Stop::Compare(1))),
            "a ctrl click moves"
        );
        a.key(KeyCode::Esc.into());
        a.key(KeyCode::Char('R').into());
        // Overall's row: none for free, flash for low, then opus for mid and high.
        assert_eq!(
            (at(&mut a, "opus · $", none), at(&mut a, "flash · $", none)),
            (Some(Mouse::Open(Stop::Recommend(0, 3))), Some(Mouse::Open(Stop::Recommend(0, 2))))
        );
        assert_eq!(at(&mut a, "✓ selected", none), Some(Mouse::Open(Stop::Among(MARKED))), "a tab of the from line");
        let name = Some(Mouse::Open(Stop::Recommend(1, 0)));
        assert_eq!(at(&mut a, TASKS[1].name, none), name, "a task's name is its task");
        let empty = Some(Mouse::Open(Stop::Recommend(TASKS.len() - 1, 1)));
        assert_eq!(at(&mut a, " - ", none), empty, "and a box with no model is still its tier");
        a.mouse(Mouse::Model(Stop::Recommend(1, 0)));
        assert_eq!((a.task_cur, a.task_sel, a.current().is_none()), (1, 0, true), "a click moves to the task");
        a.mouse(Mouse::Model(Stop::Compare(2)));
        assert_eq!((a.task_cur, a.task_sel), (1, 0), "compare's stop is none of recommend's");
        a.mouse(Mouse::Model(Stop::Recommend(0, 3)));
        assert_eq!((a.task_cur, a.current().unwrap().key.as_str()), (0, "opus"), "a click moves to the model");
        let was = a.store.is_marked("flash");
        assert_eq!(a.mouse(Mouse::MarkModel(Stop::Recommend(0, 2))), Some(Effect::Save));
        assert_ne!(a.store.is_marked("flash"), was, "a right click toggles its selection, as space does");
        a.mouse(Mouse::Open(Stop::Recommend(0, 2)));
        assert!(
            matches!(a.view, View::Detail(Back::Recommend(_))) && a.current().unwrap().key == "flash",
            "a double click on a model opens its details"
        );
        a.key(KeyCode::Esc.into());
        assert_eq!((&a.view, a.task_sel), (&View::Recommend, 2), "esc goes back to it");
        a.mouse(Mouse::Open(Stop::Recommend(0, 0)));
        assert_eq!(a.view, View::Table, "a double click on a task ranks by it");
        a.store.toggle_favorite("debugging:high", "flash");
        "RGk".chars().for_each(|c| _ = a.key(KeyCode::Char(c).into()));
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
    fn clicks_land_on_rows_headers_and_dropdown_entries() {
        use ratatui::crossterm::event::KeyModifiers;
        let mut a = app();
        let (w, h) = (204, 12);
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
        // " │ mine m │ all a │ ✓ selected S │ ★ favorites F │ ✗ excluded E │ recommend R │ compare C │ theme t │ help ?  ": a tab, the edge left
        // of it too, and nothing right of the last one.
        assert_eq!(
            [9, 10, 18, 64, 78, 90, 100, 109].map(|x| hit(&a, area, MouseEvent { row: x % 2, ..click(x, 0) })),
            [Some(0), Some(1), Some(2), Some(5), Some(6), Some(7), Some(8), None].map(|i| i.map(Mouse::Tab))
        );
        // Cut off a narrow terminal, a tab is a hint in the status bar instead, help's the last.
        let cut = |w| hints(&a, w).into_iter().filter(|h| TABS.iter().any(|t| t.2 == *h)).collect::<Vec<_>>();
        assert_eq!((cut(110), cut(109), cut(101)), (vec![], vec!["? help"], vec!["? help"]));
        assert_eq!((cut(100), cut(78)), (vec!["t theme", "? help"], vec!["R recommend", "t theme", "? help"]));
        assert_eq!(hints(&a, 79).last(), Some(&"? help"));
        // Over the open theme list a click on a tab is that tab, and elsewhere outside the list.
        a.key(KeyCode::Char('t').into());
        assert_eq!(
            [9, 150].map(|x| hit(&a, area, MouseEvent { row: 1, ..click(x, 0) })),
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
        assert_eq!(hit(&a, area, click(col("Coding"), 1)), Some(Mouse::Header(10)));
        assert_eq!(hit(&a, area, click(0, 0)), None, "the frame");
        assert_eq!(hit(&a, area, click(3, h - 2)), None, "the status bar");
        let (k, via) = lines.iter().enumerate().find_map(|(k, l)| Some((k, l.rfind("│ oc")? + "│ ".len()))).unwrap();
        let (x, y) = (lines[k][..via].chars().count() as u16 + 1, k as u16 + 1);
        assert_eq!(hit(&a, area, click(x + 1, y)), Some(Mouse::Harness(k - 2, 0)), "a harness in Via");
        assert_eq!(hit(&a, area, click(x + 2, y)), Some(Mouse::Row(k - 2)), "past its name: the row");
        assert_eq!(hit(&a, area, ctrl(x, y)), Some(Mouse::Pick(k - 2)), "ctrl click still picks");
        assert_eq!(hit(&a, area, click(col("Coding"), 4)), Some(Mouse::Cell(1, 10)), "a benchmark score");
        assert_eq!(hit(&a, area, click(col("Price"), 4)), Some(Mouse::Cell(1, PRICE)), "a price");
        assert_eq!(hit(&a, area, click(col("Notes"), 4)), Some(Mouse::Cell(1, NOTES)), "the note to write");
        let wheel = |kind| MouseEvent { kind, column: 0, row: 0, modifiers: KeyModifiers::NONE };
        assert_eq!(hit(&a, area, wheel(MouseEventKind::ScrollLeft)), Some(Mouse::Cols(-1)));
        let shifted = MouseEvent { modifiers: KeyModifiers::SHIFT, ..wheel(MouseEventKind::ScrollDown) };
        assert_eq!(hit(&a, area, shifted), Some(Mouse::Cols(1)), "shift+wheel goes sideways");
        let alt = MouseEvent { modifiers: KeyModifiers::ALT, ..wheel(MouseEventKind::ScrollUp) };
        assert_eq!(hit(&a, area, alt), Some(Mouse::Cols(-1)), "as alt+wheel does, where shift is the terminal's");
        a.mouse(Mouse::Menu(1));
        let (_, lines) = render(&mut a, w - 2, h - 4);
        let any = lines[2][..lines[2].find("all").unwrap()].chars().count() as u16 + 1;
        assert_eq!(hit(&a, area, click(any, 3)), Some(Mouse::Item(0)));
        assert_eq!(hit(&a, area, click(any, 4)), Some(Mouse::Item(1)));
        assert_eq!(hit(&a, area, click(any, 5)), Some(Mouse::Item(2)));
        assert_eq!(hit(&a, area, click(any, 6)), None, "the box's bottom border");
        assert_eq!(hit(&a, area, right(any, 4)), Some(Mouse::Item(1)), "either button");
        assert_eq!(hit(&a, area, drag(any, 4)), None, "a drag over a list does nothing");
        assert_eq!(hit(&a, area, right(w - 2, 3)), Some(Mouse::Outside), "any click outside closes it");
        assert_eq!(hit(&a, area, click(w - 2, 3)), Some(Mouse::Outside), "outside: closes it");
        // f's grid: a click on a box ticks it, on the name goes to its row, on the heading nothing.
        a.mouse(Mouse::Outside);
        a.mouse(Mouse::Star(0));
        let (within, lines) = chooser(&a, area).unwrap();
        // `click` is two rows down, under the tabs.
        let rect = overlay_rect(within, "favorite for which tasks?", &lines);
        let rect = Rect { y: rect.y - 2, ..rect };
        let Input::Choose { items, .. } = &a.input else { panic!("f's grid is open") };
        let boxes = rect.x + 2 + fav_name_w(items) as u16 + 2;
        assert_eq!(hit(&a, area, click(boxes + 2 * BOX_W as u16 + 1, rect.y + 3)), Some(Mouse::Tick(1, 2)));
        assert_eq!(hit(&a, area, click(rect.x + 3, rect.y + 3)), Some(Mouse::Row(1)), "the name");
        let favs = a.store.favorite.clone();
        a.mouse(Mouse::Row(2));
        assert!(matches!(&a.input, Input::Choose { list, .. } if list.sel == 2), "the cursor to its row");
        assert_eq!(a.store.favorite, favs, "ticking nothing");
        assert_eq!(hit(&a, area, click(boxes + BOXES as u16 * BOX_W as u16, rect.y + 3)), None, "past the boxes");
        assert_eq!(hit(&a, area, click(boxes, rect.y + 1)), None, "the tiers' heading");
    }

    #[test]
    fn favorite_models_get_a_star() {
        let mut a = app();
        a.store.toggle_favorite("coding:mid", "opus");
        a.store.toggle_favorite("coding:high", "opus");
        a.store.toggle_favorite("agentic:high", "opus");
        let (_, lines) = render(&mut a, 120, 5);
        let row = |m: &str| lines.iter().find(|l| l.contains(m)).unwrap().clone();
        assert!(row("opus").contains("★") && !row("flash").contains("★"), "★ before the favorite model's name");
        // The status bar names the tasks for the model under the cursor.
        for _ in 0..2 {
            let (_, lines) = render(&mut a, 120, 5);
            match a.current().unwrap().key.as_str() {
                // Once each, for however many tiers.
                "opus" => assert!(lines[4].contains("★ coding ★ agentic"), "status names the tasks: {}", lines[4]),
                _ => assert!(!lines[4].contains('★'), "flash is the favorite for nothing: {}", lines[4]),
            }
            a.key(KeyCode::Char('j').into());
        }
    }

    #[test]
    fn a_tiers_prices_and_scores_line_up() {
        let cell = |t: &[&str]| t.iter().map(|&t| Span::raw(t.to_string())).collect::<Vec<_>>();
        let mut cells = [
            cell(&["GLM-5.3", " ·", " $0.41 ", "·\u{a0}", "50"]),
            cell(&["★ ", "Muse Spark 1.3", " ·", " $1.3 ", "·\u{a0}", "100"]),
            cell(&["-"]),
            cell(&["mine", " ·", " $2.0", "", ""]),
        ];
        align(cells.iter_mut());
        let text: Vec<String> = cells.iter().map(|c| c.iter().map(|s| &*s.content).collect()).collect();
        assert_eq!(
            text,
            [
                "GLM-5.3          · $0.41 ·\u{a0}\u{a0}50",
                "★ Muse Spark 1.3 ·  $1.3 ·\u{a0}100",
                "-",
                "mine             ·  $2.0      "
            ]
        );
        // Wrapped, a box drops the padding with the dot before its price.
        let lines: Vec<String> =
            wrap_spans(cells[1].clone(), 12, &[]).iter().map(|l| Line::from(l.clone()).to_string()).collect();
        assert_eq!(lines, ["★ Muse Spark", "1.3", "$1.3 ·\u{a0}100"]);
    }

    #[test]
    fn star_takes_its_tasks_colour() {
        assert_eq!(TASK.len(), TASKS.len(), "a colour per task");
        let mut a = app();
        a.store.toggle_favorite("coding:high", "opus");
        a.store.toggle_favorite("agentic:high", "flash");
        a.key(KeyCode::Char('j').into()); // the cursor is on flash, off opus
        let (buf, lines) = render(&mut a, 120, 5);
        // The cell where `pat` starts on line `y`; the lines hold wide glyphs before it.
        let at = |y: usize, pat: &str| (lines[y][..lines[y].find(pat).unwrap()].chars().count() as u16, y as u16);
        let star = |m: &str| buf[at(lines.iter().position(|l| l.contains(m)).unwrap(), "★")].fg;
        assert_eq!(star("opus"), STAR, "no task picked: a gold ★ that says favorite for some task");
        // Every header is one colour, as the source's name on the bottom border.
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(160, 10)).unwrap();
        term.draw(|f| draw(&mut a, f)).unwrap();
        let (wide, rows) = (term.backend().buffer(), text(term.backend().buffer()));
        // The first cell of `pat`: a header's, or on the bottom border, where models.dev is.
        let cell = |pat: &str| {
            let on = if pat == "Epoch AI" { "models.dev" } else { "Released" };
            let y = rows.iter().position(|l| l.contains(on)).unwrap();
            wide[(rows[y][..rows[y].find(pat).unwrap()].chars().count() as u16, y as u16)].fg
        };
        for pat in ["Price", "ECI", "Coding", "Agentic"] {
            assert_eq!(cell(pat), ACCENT, "{pat}");
        }
        assert_eq!(cell("Epoch AI"), MUTED, "the source is the border's grey, as models.dev beside it");
        assert_eq!(buf[at(4, "★")].fg, task_color("agentic"), "and in the status bar");
        let opus = lines.iter().position(|l| l.contains("opus")).unwrap();
        let name_x = at(opus, "opus").0;
        a.store.toggle_favorite("vision:high", "opus");
        let (_, lines) = render(&mut a, 120, 5);
        assert_eq!(lines[opus].matches('★').count(), 1, "one ★ however many tasks: {}", lines[opus]);
        assert_eq!(at(opus, "opus").0, name_x, "so the names never shift");
        a.key(KeyCode::Char('k').into()); // the bar on opus: the status bar names its tasks
        let (buf, lines) = render(&mut a, 120, 5);
        assert!(lines[4].contains("★ coding ★ vision"), "each in the status bar too: {}", lines[4]);
        let sx = lines[4].find("★ vision").map(|i| lines[4][..i].chars().count() as u16).unwrap();
        assert_eq!(buf[(sx, 4)].fg, task_color("vision"));
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
            m.fit.insert("coding".into(), (pct, crate::fit::Basis::Benches(1)));
        }
        a.set_data(data);
        let lines = recommend(&a, 200, &mut vec![]).0;
        let spans: Vec<&Span> = lines.iter().flat_map(|l| l.spans.iter()).collect();
        let name = spans.iter().find(|s| s.content == "coding").unwrap();
        assert_eq!(name.style.fg, Some(task_color("coding")));
        let star = spans.iter().position(|s| s.content == "★ " && s.style.fg == Some(task_color("coding"))).unwrap();
        assert!(spans[star + 1].content == "opus", "★ then the name: {:?}", spans[star + 1]);
        assert_eq!(spans[star + 1].style.fg, Some(LEVEL[level(5.0)]), "the name keeps its price level");
        // A favorite off the frontier, flash dearer than opus and scoring less, is grey and says so.
        let mut data = std::mem::take(&mut a.data);
        for o in &mut data.models.iter_mut().find(|m| m.key == "flash").unwrap().offers {
            (o.input, o.output) = (6.0, 6.0);
        }
        a.store.toggle_favorite("coding:high", "flash");
        a.set_data(data);
        // Under the grid, the box under the cursor says why.
        // High's: at $6 it fits no other tier.
        (a.view, a.among, a.task_cur, a.task_sel) = (View::Recommend, None, 1, 4);
        let lines = recommend(&a, 200, &mut vec![]).0;
        let spans: Vec<&Span> = lines.iter().flat_map(|l| l.spans.iter()).collect();
        let star = spans.iter().rposition(|s| s.content == "★ " && s.style.fg == Some(task_color("coding"))).unwrap();
        assert_eq!(spans[star + 1].content, "flash");
        assert_eq!(spans[star + 1].style.fg, Some(MUTED));
        assert_eq!(spans[star + 6].content, " not recommended");
        // And the line after it names the model the tier would recommend.
        let text: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
        let at = text.iter().position(|l| l.starts_with("  high:") && l.contains("flash")).unwrap();
        assert!(text[at + 1].starts_with("  recommended: opus"), "{text:?}");
        // `d`'s dropdown has each model in its price level's colour, its ★ in the task's.
        a.key(KeyCode::Char('d').into());
        let lines = chooser(&a, Rect::new(0, 0, 120, 30)).unwrap().1;
        let line = |m: &str| lines.iter().find(|l| l.to_string().contains(m)).unwrap();
        assert_eq!(line("recommended: opus").style.fg, Some(LEVEL[level(5.0)]));
        assert_eq!(
            line("recommended: opus").spans[1].style.fg,
            Some(Color::Reset),
            "recommended: in the text's colour"
        );
        let flash = line("★ flash");
        assert_eq!(flash.style.fg, Some(LEVEL[level(6.0)]), "colour by price, the list being what the tier takes");
        assert_eq!(flash.spans[1].style.fg, Some(task_color("coding")));
        a.key(KeyCode::Esc.into());
        // With the recommended one the favorite, no such line.
        a.store.toggle_favorite("coding:high", "opus");
        a.rebuild();
        let text: Vec<String> = recommend(&a, 200, &mut vec![]).0.iter().map(|l| l.to_string()).collect();
        assert!(!text.iter().any(|l| l.contains("recommended:")), "{text:?}");
        // A model's harness follows it under the grid, not in its box, while it has the model.
        a.store.set_runs_on("opus", Some("claude"));
        let text: Vec<String> = recommend(&a, 200, &mut vec![]).0.iter().map(|l| l.to_string()).collect();
        assert!(!text.iter().any(|l| l.contains("via claude")), "claude does not have it: {text:?}");
        let opus = a.data.models.iter_mut().find(|m| m.key == "opus").unwrap();
        opus.via.push("claude".into());
        opus.offers[0].via.push("claude".into());
        let text: Vec<String> = recommend(&a, 200, &mut vec![]).0.iter().map(|l| l.to_string()).collect();
        assert!(text.iter().any(|l| l.starts_with("  high:") && l.ends_with("via claude")), "{text:?}");
        assert_eq!(text.iter().filter(|l| l.contains("via claude")).count(), 1, "{text:?}");
        let lines = recommend(&a, 200, &mut vec![]).0;
        let claude = lines.iter().flat_map(|l| &l.spans).find(|s| s.content == "claude").unwrap();
        assert_eq!(claude.style.bg, Some(dev_color("claude")), "a chip, as in Via");
        // And under `f`'s grid, on the tier it is the favorite of.
        let items = vec![("coding".to_string(), Effect::Fav("opus".into(), "coding".into()))];
        let lines = fav_lines(&a, &items, &List { col: 3, ..Default::default() }, 200);
        let line = lines.iter().find(|l| l.to_string().contains("coding:high")).unwrap();
        assert!(line.to_string().ends_with("via claude"), "{line}");
        assert_eq!(line.spans.last().unwrap().style.bg, Some(dev_color("claude")), "a chip, as in Via");
    }

    #[test]
    fn via_chips_the_harness_a_model_runs_on() {
        let mut a = app();
        let m = &mut a.data.models[0];
        m.via = vec!["opencode".into(), "claude".into()];
        m.offers[0].via = m.via.clone();
        let (m, bold) = (m.clone(), |s: &Span| s.style.add_modifier.contains(BOLD));
        let said = crate::view::via(&m, true, None);
        assert!(!via_spans(&m, true, false, &said, &a.store, Style::new()).iter().any(bold), "none chosen");
        a.store.set_runs_on(&m.key, Some("claude"));
        let spans = via_spans(&m, true, false, &said, &a.store, Style::new());
        let bolded: Vec<&str> = spans.iter().filter(|s| bold(s)).map(|s| &*s.content).collect();
        assert_eq!(bolded, ["claude"]);
        let claude = spans.iter().find(|s| s.content == "claude").unwrap();
        assert_eq!((claude.style.fg, claude.style.bg), (Some(Color::Black), Some(dev_color("claude"))), "a chip");
    }

    #[test]
    fn a_long_name_leaves_room_for_a_column() {
        let mut a = app();
        let mut data = std::mem::take(&mut a.data);
        data.models.push(model(&"n".repeat(47), "openai", Some(100.0), 0.5));
        a.set_data(data);
        assert_eq!(layout(200, &a).name_w, 47, "whole where there is room");
        for col in 2..NCOLS {
            a.col = col;
            for w in 60..200 {
                let l = layout(w, &a);
                assert!(l.cols.len() + usize::from(l.via.is_some()) + usize::from(l.notes.is_some()) > 0, "{col} {w}");
            }
        }
    }

    #[test]
    fn narrow_table_drops_columns_and_hints_without_panicking() {
        let mut a = app();
        let (_, lines) = render(&mut a, 46, 4);
        assert_eq!(
            words(&lines[0]),
            ["#", "Model", "‹│", "$out", "Ctx", "│", "▼ECI"],
            "the cursor starts on the index, and Dev scrolls off"
        );
        assert!(layout(46, &a).more, "columns cut off on the right");
        assert!(!layout(400, &a).more, "all columns fit");
        assert_eq!(layout(400, &a).first, 0, "a window made wide again shows the columns scrolled off on the left");
        for w in 46..400 {
            let l = layout(w, &a);
            assert_eq!(l.name_w, layout(400, &a).name_w, "names are whole at any width: {w}");
        }
        let l = layout(400, &a);
        assert_eq!(l.cols[0].1, l.name_x + l.name_w + GAP + l.dev_w + GAP, "Released a gap after Dev, as in a group");
        // Dev left out in `|`: Released takes its place, with no `│` and nothing scrolled off.
        let seps = l.seps.len();
        a.store.hide.insert("dev".into());
        a.set_cols();
        let l = layout(400, &a);
        assert_eq!((l.dev_w, l.cols[0].1, l.seps.len()), (0, l.name_x + l.name_w + GAP, seps));
        let scrolled = a.hscroll;
        let (_, wide) = render(&mut a, 400, 4);
        assert_eq!(words(&wide[0])[..3], ["#", "Model", "Released"]);
        a.store.hide.clear();
        a.set_cols();
        a.hscroll = scrolled;
        // Rows above or below the window are reported for the frame's ▲ ▼.
        for w in 46..400 {
            let l = layout(w, &a);
            assert_eq!(l.name_w, layout(400, &a).name_w, "names are whole at any width: {w}");
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
        // Moving past the right edge scrolls the columns right of Model, Dev too; Model stays.
        a.col = NCOLS - 1;
        let (_, lines) = render(&mut a, 46, 4);
        assert_eq!(words(&lines[0]), ["#", "Model", "‹│", "Via", "▾", "Notes"]);
        // A column wider than the room right of Model is cut, not dropped.
        a.store.set_note("opus", &"x".repeat(40));
        for w in 44..80 {
            let l = layout(w, &a);
            assert!(l.notes.is_some_and(|(x, nw)| nw > 0 && x + nw <= w), "Notes under the cursor shows: {w}");
        }
        a.store.set_note("opus", "");
        a.col = VIA;
        let (_, lines) = render(&mut a, 47, 4);
        assert_eq!(words(&lines[0]), ["#", "Model", "‹│", "Via", "▾", "Notes"]);
        for _ in 0..2 {
            a.key(KeyCode::Char('h').into());
        }
        let (_, lines) = render(&mut a, 51, 4);
        let shown = ["‹│", "Agentic", "▾", "│", "Tok/s", "│", "Vi"];
        assert_eq!(words(&lines[0])[2..], shown, "scrolls back only as far as needed");
        a.key(KeyCode::Char('l').into());
        let (_, lines) = render(&mut a, 51, 4);
        assert_eq!(words(&lines[0])[2..], shown, "{}", lines[0]);
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
    fn selected_column_is_faint_with_a_thick_rule() {
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
        assert_eq!(buf[(x + w - 1, 3)].bg, COLUMN, "its cells are filled");
        assert_eq!(buf[(px + 1, 3)].bg, Color::Reset, "other columns are not");
        let drawn = buf.clone();
        recolor(&mut buf, None, Some(0x1e1e1e), None);
        let cell = &buf[(x + w - 1, 3)];
        assert!(cell.bg != Color::from_u32(0x1e1e1e) && !cell.modifier.contains(BOLD), "a faint fill, not bold");
        let mut buf = drawn;
        recolor(&mut buf, None, None, None);
        let cell = &buf[(x + w - 1, 3)];
        assert!(cell.bg == Color::Reset && cell.modifier.contains(BOLD), "bold with no faint fill to have");
        assert!(!buf[(px + 1, 3)].modifier.contains(BOLD), "other columns are not");
    }

    #[test]
    fn a_list_price_is_marked_and_muted() {
        let mut a = app();
        let mut data = std::mem::take(&mut a.data);
        // Yours lists no price; another provider's does.
        let yours = Offer { provider: "g".into(), unpriced: true, available: true, ..Default::default() };
        data.models[1].offers.insert(0, yours);
        a.set_data(data);
        let (buf, lines) = render(&mut a, 200, 6);
        let y = lines.iter().position(|l| l.contains("flash")).unwrap();
        assert_eq!(lines[y].matches("~0.10").count(), 4, "Price, $in, $cache and $out: {}", lines[y]);
        assert_eq!(lines[y].matches('~').count(), 4, "{}", lines[y]);
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
        a.input = Input::Bound { col: VIA, min: false, text: "8".into(), cur: 1 };
        let mut buf = Buffer::empty(Rect::new(0, 0, 60, 1));
        assert_eq!(status(&mut buf, Rect::new(0, 0, 60, 1), &a), Some(8 + 7 + 1), "the GB is after the cursor");
        assert_eq!(
            (0..20).map(|x| buf[(x, 0)].symbol()).collect::<String>(),
            " BOUND  size ≤ 8 GB ",
            "the download's size"
        );
    }

    #[test]
    fn a_launch_refuses_an_id_windows_terminal_would_split() {
        let cmd = ["opencode", "--model", "p/x;new-tab"].map(String::from);
        let e = new_terminal(&cmd, None).unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::InvalidInput, "refused before anything is spawned: {e}");
    }

    #[test]
    fn compare_colours_every_tied_cell() {
        let mk = |n: &str, context| Model { name: n.into(), context, ..Default::default() };
        let (a, b, c) = (mk("a", 1_000_000), mk("b", 200_000), mk("c", 200_000));
        let lines = compare(
            &[&a, &b, &c],
            (false, &|_| None, &Store::default()),
            (0, (0, 0), None),
            (200, None),
            "",
            |_| false,
            &mut vec![],
        )
        .lines;
        let fgs: Vec<_> = lines[lines.len() - 3..]
            .iter()
            .map(|l| l.spans.iter().find(|s| s.content.trim().ends_with(['M', 'k'])).unwrap().style.fg)
            .collect();
        assert_eq!(fgs, [Some(GOOD), Some(BAD), Some(BAD)], "a column per value, a row per model");
        // The cursor's column is filled in each model's row, as in the table, and not over it.
        let filled = |l: &Line| l.spans.iter().any(|s| s.style.bg == Some(COLUMN) && !s.content.trim().is_empty());
        assert!(lines[lines.len() - 3..].iter().all(filled), "{lines:#?}");
        assert!(!lines[..lines.len() - 3].iter().any(filled), "not the header");
        // A harness left out in Via is grey, as in the table.
        let m = Model { name: "m".into(), via: vec!["codex".into(), "pi".into()], ..Default::default() };
        let mut store = Store::default();
        store.via_not = vec!["codex".into()];
        let lines =
            compare(&[&m], (false, &|_| None, &store), (0, (0, 0), None), (200, None), "", |_| false, &mut vec![])
                .lines;
        let row = lines.last().unwrap();
        let fg = |h: &str| row.spans.iter().find(|s| s.content == h).map(|s| s.style.fg);
        assert_eq!((fg("codex"), fg("pi")), (Some(Some(MUTED)), Some(None)), "{row:?}");
    }

    #[test]
    fn compare_takes_no_best_or_worst_from_a_muted_model() {
        let offer = |p: f64| Offer { input: p, output: p, ..Default::default() };
        let mk =
            |n: &str, p: f64| Model { key: n.into(), name: n.into(), offers: vec![offer(p)], ..Default::default() };
        let ms = [mk("cheapest", 0.1), mk("cheap", 1.0), mk("dear", 5.0), mk("dearest", 9.0)];
        let ms: Vec<&Model> = ms.iter().collect();
        let muted = |m: &Model| ["cheapest", "dearest"].contains(&m.key.as_str());
        let at = (0, (0, 0), None);
        let c = compare(&ms, (false, &|_| None, &Store::default()), at, (200, None), "", muted, &mut vec![]);
        // Price, the first column: its cell after the name and the `│` before it.
        let fgs: Vec<_> = c.lines[c.at..].iter().map(|l| l.spans[4].style.fg).collect();
        assert_eq!(
            fgs,
            [Some(MUTED), Some(GOOD), Some(BAD), Some(MUTED)],
            "the others' best and worst: {:#?}",
            c.lines
        );
    }

    #[test]
    fn a_verdict_too_wide_goes_on_under_itself() {
        let offer = |p: f64| Offer { input: p, output: p, ..Default::default() };
        let mk = |n: &str, coding: f64, p: f64| Model {
            name: n.into(),
            fit: [("coding".to_string(), (coding, crate::fit::Basis::Benches(1)))].into(),
            offers: vec![offer(p)],
            ..Default::default()
        };
        let ms = [mk("big", 90.0, 10.0), mk("mid", 70.0, 5.0), mk("small", 50.0, 1.0)];
        let ms: Vec<&Model> = ms.iter().collect();
        let lines: Vec<String> = verdict_lines(&ms, &Store::default(), 40).iter().map(Line::to_string).collect();
        let at = lines.iter().position(|l| l.trim_start().starts_with("coding")).unwrap();
        assert!(lines[at].ends_with(" ·") && lines[at + 1].trim_start().starts_with("mid"), "{lines:#?}");
        assert!(lines.iter().all(|l| l.chars().count() <= 40), "{lines:#?}");
    }

    #[test]
    fn compare_wraps_labels_over_the_header() {
        assert_eq!(wrap_label("$ cached in / 1M", 8), [(0, "$ cached".into()), (9, "in / 1M".into())]);
        assert_eq!(wrap_label("terminalbench_scicode_mean", 14)[0].1, "terminalbench_", "a word breaks after _");
        assert_eq!(wrap_label("context", 3), [(0, "context".into())], "a word longer than the width keeps its line");
    }

    #[test]
    fn compare_sorts_by_a_column() {
        let mut a = app();
        a.store.marked = vec!["opus".into(), "flash".into()];
        a.rebuild();
        let keys = |a: &App| a.compare_models().iter().map(|m| m.key.clone()).collect::<Vec<_>>();
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(160, 40)).unwrap();
        a.key(KeyCode::Char('C').into());
        term.draw(|f| draw(&mut a, f)).unwrap();
        assert_eq!(keys(&a), ["opus", "flash"], "in the order they were selected");
        // The first column is the price: cheapest first, the cursor staying on its model.
        a.key(KeyCode::Char('s').into());
        assert_eq!((keys(&a), a.current().unwrap().key.as_str()), (vec!["flash".into(), "opus".into()], "opus"));
        term.draw(|f| draw(&mut a, f)).unwrap();
        let lines = text(term.backend().buffer());
        let screen = lines.join("\n");
        assert!(screen.contains("▲Price"), "{screen}");
        // Drawn in that order, as clicks find them.
        let row = |i| a.spots.iter().find(|s| s.1 == Stop::Compare(i)).map(|s| &lines[usize::from(s.0.y)]);
        assert!(row(0).is_some_and(|l| l.contains("flash")) && row(1).is_some_and(|l| l.contains("opus")), "{screen}");
        a.key(KeyCode::Char('s').into());
        assert_eq!(keys(&a), ["opus", "flash"], "again reverses");
        a.key(KeyCode::Char('$').into());
        assert_eq!(a.compare_col, a.compare_cols() - 1, "$ to the last column");
        a.mouse(Mouse::Open(Stop::CompareCol(4)));
        assert_eq!(a.compare_col, 4, "a click on a label sorts by it");
        assert!(a.compare_sort.as_ref().is_some_and(|s| s.0 == "Ctx"), "the fifth column, after the four prices");
    }

    #[test]
    fn compare_scrolls_columns_sideways() {
        let a = app();
        let ms: Vec<&Model> =
            ["opus", "flash"].iter().map(|k| a.data.models.iter().find(|m| m.key == *k).unwrap()).collect();
        let draw = |col, avail, query, jump| {
            let mut spots = vec![];
            let at = (1, (col, 0), jump);
            let c =
                compare(&ms, (false, &|_| None, &Store::default()), at, (avail, None), query, |_| false, &mut spots);
            (c, spots)
        };
        let head = |c: &Compared| c.lines[c.at - 3].to_string();
        let (full, spots) = draw(0, 200, "", None);
        assert_eq!((full.first, full.lines.len()), (0, full.at + 1), "everything fits, flash's row last");
        assert!(full.lines[full.at].to_string().starts_with("flash"), "the cursor's row");
        assert!(!full.lines.iter().any(|l| l.to_string().contains(['‹', '›'])), "no scroll marks when all fit");
        assert_eq!(
            spots
                .iter()
                .filter(|s| matches!(s.at, Stop::Compare(_)))
                .map(|s| (s.lines.start, s.at))
                .collect::<Vec<_>>(),
            [(full.at - 1, Stop::Compare(0)), (full.at, Stop::Compare(1))]
        );
        let label = spots.iter().find(|s| s.at == Stop::CompareCol(0)).unwrap();
        assert_eq!(label.lines.end, full.at - 2, "a column's label, over the rule, is its spot");
        let rule = &full.lines[full.at - 2];
        assert!(
            rule.to_string().starts_with("──────┼─━━━"),
            "a rule under the header, heavy under the cursor's column: {rule}"
        );
        let crossed = |l: &Line| l.to_string().chars().filter(|&c| c == '│' || c == '┼').count();
        assert!(full.lines[full.at - 2..].iter().all(|l| crossed(l) > 1), "group lines run down the panel");
        let (cut, _) = draw(0, 40, "", None);
        assert!(cut.shown < full.shown && head(&cut).ends_with('›') && !head(&cut).contains('‹'), "{}", head(&cut));
        assert!(cut.lines.iter().any(|l| l.to_string().starts_with(&format!("columns 1-{} of", cut.shown))));
        let (moved, _) = draw(cut.shown, 40, "", None);
        assert!(moved.first > 0 && head(&moved).contains('‹'), "the view follows the cursor, ‹ marks columns off");
        assert_eq!(moved.col, cut.shown);
        let size = |c: &Compared| (c.lines.len(), c.lines.iter().map(Line::width).max());
        assert_eq!(size(&moved), size(&cut), "the panel keeps its size as it scrolls");
        let (end, _) = draw(usize::MAX, 40, "", None);
        assert!(!head(&end).ends_with('›') && end.first + end.shown == full.shown, "no further than the last");
        let (found, _) = draw(0, 40, "eci", Some(0));
        assert_eq!(found.hits, 1);
        assert!(found.first > 0 && found.col > 0, "a match moves the cursor to its column");
        let lit = found.lines.iter().flat_map(|l| &l.spans).any(|s| s.content == "ECI" && s.style != fg(KEY));
        assert!(lit, "and its label is drawn as found");
        assert_eq!((draw(0, 200, "rice", None).0.hits, draw(0, 200, "prce", None).0.hits), (1, 0), "no typo");
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
        // Recommend's legend has no cursor: it shows with the first task, and the lines under
        // the grid with the last.
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 16)).unwrap();
        let mut a = app();
        let mut screen = |a: &mut App, keys: &str| {
            keys.chars().for_each(|c| drop(a.key(KeyCode::Char(c).into())));
            term.draw(|f| draw(a, f)).unwrap();
            let buf = term.backend().buffer();
            (0..16).map(|y| (0..80).map(|x| buf[(x, y)].symbol()).collect::<String>()).collect::<Vec<_>>().join("\n")
        };
        let last = screen(&mut a, "RG");
        assert!(last.contains("+ new task: "), "{last}");
        let first = screen(&mut a, "gg");
        assert!(first.contains("the best model for each task"), "{first}");
        // The lines under the grid, which say what the cursor is on, stay under the rows that scroll.
        let top = screen(&mut a, "jj2l");
        assert!(top.contains("overall") && top.contains("  low:         flash"), "{top}");
        let last = screen(&mut a, "Gk");
        assert!(last.contains("vision screenshots") && !last.contains("from:"), "{last}");
        // f's hints fit a terminal 80 columns wide, with a task of your own too.
        let items = vec![("debugging".to_string(), Effect::Fav("k".into(), "debugging".into()))];
        let hint = fav_lines(&a, &items, &List::default(), 80).pop().unwrap();
        assert!(hint.width() <= 76 && hint.to_string().contains("T edit"), "{hint}");
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
        // Done with a warning, the frame keeps saying so, as the next start does from the cache.
        a.refreshed(
            Ok(Data { warning: Some("copilot did not list its models".into()), ..Data::default() }),
            (vec![], vec![]),
        );
        assert!(border(&mut a).contains("⚠ refresh had issues ·"), "after the status is cleared too");
        a.refreshed(Ok(Data::default()), (vec![], vec![]));
        assert!(!border(&mut a).contains("⚠ refresh"), "until a refresh without any");
    }

    #[test]
    fn issues_open_on_a_key_and_on_a_click_of_the_frames_mark() {
        let (w, h) = (120, 12);
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h)).unwrap();
        let mut a = app();
        let mut screen = |a: &mut App| {
            term.draw(|f| draw(a, f)).unwrap();
            let buf = term.backend().buffer();
            (0..h).map(|y| (0..w).map(|x| buf[(x, y)].symbol()).collect::<String>()).collect::<Vec<_>>()
        };
        let bang = |a: &mut App| a.key(ratatui::crossterm::event::KeyEvent::from(KeyCode::Char('!')));
        bang(&mut a);
        assert!(a.view == View::Table && a.status == "no issues to list");
        a.data.warning = Some("copilot did not list its models · openrouter.ai did not list its models".into());
        let lines = screen(&mut a);
        let x = lines[h as usize - 2][..lines[h as usize - 2].find("⚠").unwrap()].chars().count() as u16;
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: x + 3,
            row: h - 2,
            modifiers: KeyModifiers::NONE,
        };
        let got = hit(&a, Rect::new(0, 0, w, h), click);
        assert_eq!(got, Some(Mouse::Key(KeyCode::Char('!'))));
        a.mouse(got.unwrap());
        assert_eq!(a.view, View::Issues);
        let lines = screen(&mut a);
        assert!(lines.iter().any(|l| l.contains("• copilot did not list its models")), "one a line");
        assert!(lines.iter().any(|l| l.contains("• openrouter.ai did not list its models")));
        bang(&mut a);
        assert_eq!(a.view, View::Table, "`!` closes it too");
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
            (60, "╭─────────── ECI: Epoch AI's overall index, gre…─"),
            (70, "╭ ECI: Epoch AI's overall index, grey = estimate ╯"),
        ] {
            let lines = rows(&mut a, w);
            assert!(lines[2].starts_with(left) && lines[2].ends_with(" ▼ by ECI ╮"), "{}", lines[2]);
            assert!(lines[19].contains("✗ excluded only"), "E left S: {}", lines[19]);
        }
        // ✓'s, further left, leaves the wider side on its right.
        a.key(KeyCode::Char('S').into());
        let top = &rows(&mut a, 100)[2];
        assert!(
            top.contains("╯              ╰─── ECI: Epoch AI's overall index, grey = estimate ────"),
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
        let below = buf.content.iter().find(|c| c.symbol() == "▼" && c.bg == Color::Yellow);
        assert_eq!(below.map(|c| c.fg), Some(Color::Black), "themes below the cursor");
        let tabs: String = (0..2).flat_map(|y| (0..60).map(move |x| buf[(x, y)].symbol())).collect();
        assert!(tabs.contains("mine") && !tabs.contains('╮'), "a list too tall leaves the tabs in view: {tabs}");
        for _ in 9..THEMES.len() {
            a.key(KeyCode::Char('j').into());
        }
        term.draw(|f| draw(&mut a, f)).unwrap();
        let buf = term.backend().buffer();
        let text: String = (0..9).flat_map(|y| (0..60).map(move |x| buf[(x, y)].symbol())).collect();
        assert!(text.contains(THEMES[THEMES.len() - 1].0) && text.contains('▲'), "{text}");
        assert!(
            text.contains("space enter saves") && !text.contains('▼'),
            "at the last theme, the hint below it: {text}"
        );
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
        a.key(KeyCode::Char('i').into());
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
        recolor(&mut buf, None, None, None);
        let (end, text) = (&buf[(x, y + 1)], &buf[(x + 2, y + 1)]);
        assert_eq!((end.symbol(), end.fg, end.bg), (" ", Color::Reset, Color::Reset));
        assert!(end.modifier.contains(Modifier::REVERSED) && text.modifier.contains(Modifier::REVERSED));
        assert_eq!((text.fg, text.bg, buf[(x + 1, y)].bg), (Color::Reset, Color::Reset, ACCENT));
    }

    #[test]
    fn details_search_underlines_and_keeps_every_line() {
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 60)).unwrap();
        let mut a = app();
        a.key(KeyCode::Enter.into());
        assert_eq!(a.view, View::Detail(Back::Table));
        term.draw(|f| draw(&mut a, f)).unwrap();
        let all = text(term.backend().buffer());
        a.overlay_query = "price".into();
        term.draw(|f| draw(&mut a, f)).unwrap();
        let buf = term.backend().buffer();
        // But for the status bar, whose hints add `n N match`.
        // And for the cursor's bars, on the line `n` is on.
        let plain = |l: &[String]| l.iter().map(|l| l.replace(['▌', '▐'], "│")).collect::<Vec<_>>();
        assert_eq!(plain(&text(buf)[..59]), plain(&all[..59]), "every line still shows");
        assert!(buf.content.iter().any(|c| c.fg == MATCH && c.modifier.contains(Modifier::UNDERLINED)), "{all:#?}");

        // Too short for every line: a search scrolls to its first match and `n` to the next.
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 12)).unwrap();
        a.overlay_query.clear();
        let type_in = |a: &mut App, keys: &str| keys.chars().for_each(|c| _ = a.key(KeyCode::Char(c).into()));
        type_in(&mut a, "/5.0");
        a.key(KeyCode::Enter.into());
        // The scroll, the status and the line with the box's cursor bar, the right one as `▲`
        // goes over the left, the table's being on the frame's edge.
        let mut at = |a: &mut App| {
            term.draw(|f| draw(a, f)).unwrap();
            let buf = term.backend().buffer();
            let y = (0..12).find(|&y| (1..119).any(|x| buf[(x, y)].symbol() == "▐"));
            (a.scroll, a.status.clone(), y.map(|y| text(buf)[y as usize].clone()))
        };
        let (scroll, status, on) = at(&mut a);
        assert_eq!((scroll, status.as_str()), (3, "line 1 of 2"), "the price line, the title not counted");
        assert!(on.unwrap().contains("price:"), "the cursor on it");
        a.key(KeyCode::Char('n').into());
        let (scroll, status, on) = at(&mut a);
        assert_eq!((scroll, status.as_str()), (12, "line 2 of 2"), "the providers' row, as far down as it scrolls");
        let on = on.unwrap();
        assert!(on.contains("5.0  opus"), "the cursor on it, not at the top: {on}");
        a.key(KeyCode::Char('n').into());
        assert_eq!(at(&mut a).0, 3, "round again after the last");
        a.key(KeyCode::Char('N').into());
        assert_eq!(at(&mut a).0, 12, "N the previous, round to the last");
        a.key(KeyCode::Char('N').into());
        assert_eq!(at(&mut a).1, "line 1 of 2");
        type_in(&mut a, "/zzz");
        let (_, status, on) = at(&mut a);
        assert_eq!((status.as_str(), on), ("no line matches zzz", None), "no cursor on no match");
        a.key(KeyCode::Esc.into());
        a.key(KeyCode::Esc.into());
        a.key(KeyCode::Char('n').into());
        assert!(a.status.contains("/ searches first") && a.input == Input::None, "{}", a.status);
        a.key(KeyCode::Char('i').into());
        assert!(matches!(a.input, Input::Note { .. }), "i writes a note");
    }

    #[test]
    fn help_shows_over_the_panel_it_was_opened_from() {
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(160, 30)).unwrap();
        let mut frame = |a: &mut App| {
            term.draw(|f| draw(a, f)).unwrap();
            (term.backend().buffer().clone(), a.panel.expect("a box"))
        };
        let mut a = app();
        a.key(KeyCode::Char('R').into());
        let (before, rec) = frame(&mut a);
        a.key(KeyCode::Char('?').into());
        let (after, help) = frame(&mut a);
        let behind: Vec<_> = rec.positions().filter(|p| !help.contains(*p)).collect();
        assert!(!behind.is_empty(), "{rec:?} is under {help:?}");
        assert!(behind.iter().all(|&p| after[p].symbol() == before[p].symbol()), "recommend is behind help");
        assert!(after[behind[0]].modifier.contains(Modifier::DIM), "and dimmed");
        a.key(KeyCode::Char('?').into());
        assert_eq!(frame(&mut a).0, before, "closing help leaves recommend as it was");
    }

    #[test]
    fn help_and_compare_search_keep_every_line() {
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 20)).unwrap();
        let mut a = app();
        let type_in = |a: &mut App, keys: &str| keys.chars().for_each(|c| _ = a.key(KeyCode::Char(c).into()));
        // As the status bar shows it, drawn in the frame that found the match.
        let mut status = |a: &mut App| {
            term.draw(|f| draw(a, f)).unwrap();
            let buf = term.backend().buffer();
            let bar: String = (0..buf.area.width).map(|x| buf[(x, buf.area.height - 1)].symbol()).collect();
            // While typing, the bar is the prompt's.
            if a.input == Input::None {
                assert!(bar.contains(&a.status), "{bar:?} says {:?}", a.status);
            }
            a.status.clone()
        };
        type_in(&mut a, "?/column");
        a.key(KeyCode::Enter.into());
        let first = status(&mut a);
        let n: usize = first.rsplit(' ').next().unwrap().parse().unwrap();
        assert!(first.starts_with("match 1 of ") && n > 2, "{first}");
        let at = a.help_sel;
        assert!(at > 0, "the cursor goes to the first key with column");
        a.key(KeyCode::Char('n').into());
        assert_eq!(status(&mut a), format!("match 2 of {n}"));
        a.key(KeyCode::Char('N').into());
        assert_eq!((status(&mut a), a.help_sel), (format!("match 1 of {n}"), at));
        type_in(&mut a, "2n");
        assert_eq!(status(&mut a), format!("match 3 of {n}"), "a count goes that many matches on");
        a.key(KeyCode::Char('N').into());
        a.key(KeyCode::Char('N').into());
        status(&mut a);
        let line = Line::from(vec![Span::raw("price: "), Span::raw("$5")]);
        assert_eq!(lit_lines(vec![line], "price: $").1, [0], "a match across spans, as the line reads");
        assert_eq!(lit_lines(help(&a).0, "sort").0.len(), help(&a).0.len(), "every line stays");
        a.key(KeyCode::Char('n').into());
        status(&mut a);
        let on = a.help_sel;
        type_in(&mut a, "/zzzz");
        status(&mut a);
        a.key(KeyCode::Esc.into());
        status(&mut a);
        assert_eq!((a.overlay_query.as_str(), a.help_sel), ("column", on), "esc goes back to where it was");
        type_in(&mut a, "/");
        a.key(KeyCode::Down.into());
        assert!(!a.status.contains("searches first"), "nothing typed yet, ↓ moves: {}", a.status);
        a.key(KeyCode::Esc.into());
        a.key(KeyCode::Esc.into());
        a.key(KeyCode::Esc.into());
        a.store.marked = vec!["opus".into(), "flash".into()];
        a.rebuild();
        type_in(&mut a, "C/eci");
        a.key(KeyCode::Enter.into());
        assert_eq!(status(&mut a), "column 1 of 1", "compare's columns, by their labels");
        type_in(&mut a, "/contxt");
        assert_eq!(status(&mut a), "no column matches contxt", "plain text, as every panel finds it");
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
        assert_eq!(row(&term, 0).trim_end(), " ╭────────╮");
        let tabs = row(&term, 1);
        assert!(
            tabs.starts_with(
                " │ mine m │ all a │ ✓ selected S │ ★ favorites F │ ✗ excluded E │ recommend R │ compare C │ theme t"
            ),
            "{tabs}"
        );
        let top = row(&term, 2);
        assert!(top.ends_with("─ ▼ by ECI ╮"), "{top}");
        assert!(top.starts_with("╭╯        ╰──────────"), "{top}");
        let (l, r) = top.split_once(" ECI: Epoch AI's overall index, grey = estimate ").unwrap();
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
        assert!(bottom.contains("models.dev + Epoch AI · data "), "{bottom}");
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
        assert_eq!(term.backend().buffer()[(0, 5)].bg, Color::Yellow);
        // On a marked row's fill the ▲ keeps its own.
        a.store.marked = a.data.models.iter().map(|m| m.key.clone()).collect();
        term.draw(|f| draw(&mut a, f)).unwrap();
        let top = &term.backend().buffer()[(0, 5)];
        assert_eq!((top.symbol(), top.fg, top.bg), ("▲", Color::Black, Color::Yellow));
        a.store.marked.clear();
        // A tall overlay stops above the status bar, which shows its keys.
        a.key(KeyCode::Char('?').into());
        term.draw(|f| draw(&mut a, f)).unwrap();
        let bar: String = (0..100).map(|x| edge(&term, x, 9)).collect();
        assert!(bar.starts_with(" HELP "), "{bar}");
    }

    #[test]
    fn a_frame_reaches_the_terminal_in_one_write() {
        // Each write as the terminal gets it, a write(2) on `Stdout`.
        struct Writes<'a>(&'a std::cell::RefCell<Vec<usize>>);
        impl std::io::Write for Writes<'_> {
            fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
                self.0.borrow_mut().push(b.len());
                Ok(b.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let writes = std::cell::RefCell::new(Vec::new());
        let viewport = ratatui::Viewport::Fixed(Rect::new(0, 0, 200, 50));
        let mut term =
            Terminal::with_options(buffered(Writes(&writes)), ratatui::TerminalOptions { viewport }).unwrap();
        term.draw(|f| draw(&mut app(), f)).unwrap();
        let lens = writes.take();
        assert_eq!(lens.len(), 1, "{lens:?}");
        assert!(lens[0] > 1024, "more than `Stdout` holds: {lens:?}");
    }

    #[test]
    fn overlays_fit_and_scroll() {
        let area = Rect::new(0, 0, 30, 6);
        let mut buf = Buffer::empty(area);
        let mut scroll = 99;
        overlay(&mut buf, area, "keys", help(&app()).0, &mut scroll, (0, 0, 0..0));
        assert_eq!(buf[(0, 0)].symbol(), "╭");
        assert_eq!(buf[(0, 0)].fg, Color::Reset, "a box's border is in the text's colour, unlike the grey table frame");
        assert_eq!(scroll as usize, help(&app()).0.len() - 4, "scroll is clamped to the content");
        assert_eq!((buf[(0, 1)].symbol(), buf[(0, 4)].symbol()), ("▲", "│"), "at the end: lines above only");
        scroll = 0;
        overlay(&mut buf, area, "keys", help(&app()).0, &mut scroll, (0, 0, 0..0));
        assert_eq!((buf[(0, 1)].symbol(), buf[(0, 4)].symbol()), ("│", "▼"), "at the top: lines below only");
        // A grid's heading is no row: the border counts the rows under it, 20 here in a box of 4 lines.
        let grid = |scroll: &mut u16| {
            let mut buf = Buffer::empty(area);
            let lines = (0..22).map(|i| Line::from(format!("{i:<20}"))).collect();
            overlay(&mut buf, area, "f", lines, scroll, (1, 1, 0..0));
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
        m.country = "USA".into();
        let text: Vec<String> =
            detail(&m, &a.store, a.any_available(), (None, None)).1.iter().map(ToString::to_string).collect();
        assert!(text.iter().any(|l| l == "  country:    USA"), "{text:?}");
        m.fit.insert("coding".into(), (50.0, crate::fit::Basis::Benches(1)));
        let lines = detail(&m, &a.store, a.any_available(), (None, None)).1;
        let coding = lines.iter().flat_map(|l| &l.spans).find(|s| s.content == "coding").expect("a coding fit line");
        assert_eq!(coding.style.fg, Some(task_color("coding")), "a task's name is in its colour");
        // A harness left out in Via is grey, the others not.
        let mut store = a.store.clone();
        (m.via, store.via_not) = (vec!["codex".into(), "pi".into()], vec!["codex".into()]);
        let lines = detail(&m, &store, true, (None, None)).1;
        let fg = |h: &str| lines.iter().flat_map(|l| &l.spans).find(|s| s.content == h).map(|s| s.style.fg);
        assert_eq!((fg("codex"), fg("pi")), (Some(Some(MUTED)), Some(None)), "{lines:?}");
        let rows = compare(
            &a.marked_models(),
            (a.any_available(), &|_| None, &a.store),
            (0, (0, 0), None),
            (200, None),
            "",
            |_| false,
            &mut vec![],
        )
        .lines;
        assert!(rows[0].to_string().starts_with("verdict"), "the verdict comes first");
        assert!(rows.iter().any(|l| l.to_string().starts_with("Model")));
    }

    #[test]
    fn a_box_over_a_wide_glyph_shows_its_left_border() {
        // A CJK note behind the box, its last glyph cut by the box's left edge: the terminal is
        // sent the border, not the glyph's right half.
        let area = Rect::new(0, 0, 30, 6);
        let lines = vec![Line::from("q confirms")];
        let r = overlay_rect(area, "quit?", &lines);
        let note = |buf: &mut Buffer| {
            for y in 0..6 {
                buf.set_string(r.x - 1, y, "日本語", Style::new());
            }
        };
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(30, 6)).unwrap();
        term.draw(|f| note(f.buffer_mut())).unwrap();
        term.draw(|f| {
            note(f.buffer_mut());
            overlay(f.buffer_mut(), area, "quit?", lines.clone(), &mut 0, (0, 0, 0..0));
        })
        .unwrap();
        let left: Vec<_> = (r.y..r.bottom()).map(|y| term.backend().buffer()[(r.x, y)].symbol()).collect();
        assert_eq!(left, ["╭", "│", "╰"]);
    }

    #[test]
    fn a_box_nearly_as_wide_as_the_screen_spans_it() {
        // Recommend wraps its text to the screen, which leaves its box a cell or so short of it
        // at many widths: there it spans the screen, so the table's border shows nowhere beside
        // the box's own, as `╮╮` or `││`.
        let mut a = app();
        a.view = View::Recommend;
        for w in 60..=200 {
            let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, 30)).unwrap();
            term.draw(|f| draw(&mut a, f)).unwrap();
            let row = text(term.backend().buffer()).into_iter().find(|l| l.contains("╭ recommend ")).unwrap();
            // Its corners, by cell: the row has no wide character.
            let cells: Vec<char> = row.chars().collect();
            let left = row[..row.find("╭ recommend ").unwrap()].chars().count();
            let right = usize::from(w) - 1 - (left..cells.len()).find(|&x| cells[x] == '╮').unwrap();
            assert!((left, right) == (0, 0) || left.min(right) >= 2, "{w} wide: {row}");
        }
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
            let lines = choice_lines(
                kind,
                &[(label.to_string(), effect)],
                &List::default(),
                80,
                &BTreeMap::new(),
                None,
                &[],
                &[],
                &[],
            );
            lines[0].spans.iter().map(|s| lines[0].style.patch(s.style).fg).collect::<Vec<_>>()
        };
        assert_eq!(fgs(Kind::Theme, "nord", Effect::Theme("nord")), [Some(Color::Reset)]);
        assert_eq!(fgs(Kind::Open, "epoch.ai", Effect::Open(String::new())), [Some(Color::Reset)]);
        let pi = fgs(Kind::Launch, "pi --model x", Effect::Launch(vec![]));
        assert_eq!(pi, [Some(dev_color("pi")), Some(MUTED)], "the harness as in Via, its command muted");
        let items = [("pi --model x".to_string(), Effect::Launch(vec!["pi".into()]))];
        let lines =
            choice_lines(Kind::Launch, &items, &List::default(), 80, &BTreeMap::new(), None, &["pi".into()], &[], &[]);
        assert_eq!(lines[0].style.patch(lines[0].spans[0].style).fg, Some(MUTED), "left out in Via, grey");
        // llama.cpp's command starts with the binary the machine has, and `H`'s list grey too.
        let llama = |bin: &str| (format!("{bin} -hf x"), Effect::Launch(vec![bin.into()]));
        let via = ("pi --model x".to_string(), Effect::Via("k".into(), Some("pi".into()), None));
        let items = [llama("llama-server"), llama("llama"), via];
        let out = [crate::data::LLAMA.into(), "pi".into()];
        let lines = choice_lines(Kind::Launch, &items, &List::default(), 80, &BTreeMap::new(), None, &out, &[], &[]);
        assert!(lines.iter().all(|l| l.style.patch(l.spans[0].style).fg == Some(MUTED)), "{lines:?}");
        let lines =
            choice_lines(Kind::Launch, &items[..1], &List::default(), 80, &BTreeMap::new(), None, &[], &[], &[]);
        assert_eq!(lines[0].style.patch(lines[0].spans[0].style).fg, Some(dev_color(crate::data::LLAMA)));
        let get = || Effect::Launch(["modelcmp", "get", "x"].map(String::from).to_vec());
        let size = |fits| {
            let items = [("ollama download and run (2.5 GB)".to_string(), get())];
            let lines = choice_lines(Kind::Launch, &items, &List::default(), 80, &BTreeMap::new(), fits, &[], &[], &[]);
            lines[0].spans.iter().map(|s| (s.content.to_string(), lines[0].style.patch(s.style).fg)).next_back()
        };
        assert_eq!(size(Some(true)), Some((" (2.5 GB) ".into(), None)), "a download that fits, in the text's colour");
        assert_eq!(size(Some(false)).unwrap().1, Some(MUTED), "one too big stays muted");
    }

    #[test]
    fn searches_show_what_matched() {
        let lit_text = |lines: &[Line]| -> Vec<String> {
            let hit = |s: &&Span| s.style.add_modifier.contains(Modifier::UNDERLINED);
            lines.iter().flat_map(|l| l.spans.iter().filter(hit).map(|s| s.content.to_string())).collect()
        };
        assert_eq!(found("Gruvbox gRUV", "ruv"), [1..4, 9..12]);
        let hits = lit_text(&lit_lines(help(&app()).0, "THEME").0);
        assert!(!hits.is_empty() && hits.iter().all(|h| h.eq_ignore_ascii_case("theme")), "{hits:?}");
        let items = vec![("nord".to_string(), Effect::Theme("nord")), ("gruvbox".into(), Effect::Theme("gruvbox"))];
        let search = |q: &str| List { query: q.into(), typing: true, ..Default::default() };
        let lines = choice_lines(Kind::Theme, &items, &search("uv"), 80, &BTreeMap::new(), None, &[], &[], &[]);
        assert_eq!(lit_text(&lines), ["uv"]);
        // Under the cursor too a hit is yellow, as the cursor keeps colours.
        let lines = choice_lines(
            Kind::Theme,
            &items,
            &List { sel: 1, ..search("gr") },
            80,
            &BTreeMap::new(),
            None,
            &[],
            &[],
            &[],
        );
        let hit = lines[1].spans.iter().find(|s| s.content == "gr").unwrap();
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
        let wide = " ↓ ↑ ← → move · / search · space enter toggle · H harness · esc close".chars().count();
        let task = |other: &str, about| What::Task { was: Some("x".into()), other: other.into(), about };
        assert_eq!(
            text(task("", false), "y", None),
            [" y a description".to_string(), format!("{:<wide$}", " enter description · tab other field · esc cancel")],
            "as wide as the hint it stands for"
        );
        assert_eq!(
            text(task("x", true), "", Some("no")),
            [" x a description".to_string(), format!("{:<wide$}", " no")]
        );
        let new = Edit {
            what: What::Task { was: None, other: String::new(), about: false },
            text: String::new(),
            cur: 0,
            err: None,
        };
        assert_eq!(edit_prefix(&new), " + ");
        // However long what is written, the box keeps its width, and a long name leaves what it is
        // about room to show.
        let widest = |lines: &[Line]| lines.iter().map(Line::width).max();
        let long = Some(Edit { what: task("", true), text: "y".repeat(300), cur: 300, err: None });
        let edited = fav_lines(&a, &items, &List { edit: long, ..Default::default() }, 200);
        assert_eq!(widest(&edited), widest(&fav_lines(&a, &items, &List::default(), 200)));
        let e = Edit { what: task(&"n".repeat(100), true), text: "bugs".into(), cur: 4, err: None };
        let (line, x) = edit_line(&e, Span::raw(""), Style::new(), 40);
        assert!(line.to_string().ends_with(" bugs") && x < 40, "{line}");
    }

    #[test]
    fn recommend_writes_a_tier_cap_on_its_header() {
        let mut a = app();
        let head = |a: &App| {
            let lines = recommend(a, 200, &mut vec![]).0;
            lines.iter().map(ToString::to_string).find(|l| l.trim_start().starts_with("free")).unwrap()
        };
        assert!(head(&a).contains("low ≤$2") && head(&a).contains("mid ≤$5"), "{}", head(&a));
        a.store.tiers.insert("mid".into(), 8.0);
        assert!(head(&a).contains("mid ≤$8"), "your cap");
        let mut spots = vec![];
        recommend(&a, 200, &mut spots);
        let caps: Vec<_> = spots.iter().filter_map(|p| if let Stop::Cap(c) = p.at { Some(c) } else { None }).collect();
        assert_eq!(caps, [1, 2], "low's and mid's headers take a click");
        // Narrow, each header fits its box and its click lands on its column.
        let mut spots = vec![];
        let lines = recommend(&a, 36, &mut spots).0;
        let at = |s: Stop| spots.iter().find(|p| p.at == s).map(|p| p.x.clone());
        for c in [1, 2] {
            assert_eq!(at(Stop::Cap(c)), at(Stop::Recommend(0, c + 1)), "tier {c}");
        }
        let line = |s: &str| lines.iter().map(ToString::to_string).find(|l| l.trim_start().starts_with(s)).unwrap();
        assert_eq!(line("free ").chars().count(), line("▌overall").chars().count(), "{}", line("free "));
        (a.view, a.among, a.task_sel) = (View::Recommend, None, 2);
        assert!(hints(&a, 300).contains(&"< price cap"), "on a box of low or mid");
        a.task_sel = 4;
        assert!(!hints(&a, 300).contains(&"< price cap"), "not on high's");
        (a.view, a.input) = (View::Recommend, Input::Cap { tier: 1, text: "35".into(), cur: 1 });
        assert!(head(&a).contains("▌low ≤ $35 "), "written in place, in the cursor's bars: {}", head(&a));
        let (lines, _, _, at) = recommend(&a, 200, &mut vec![]);
        let (l, x) = at.unwrap();
        let line = lines[l].to_string();
        let before: String = line.chars().take(x).collect();
        assert!(before.ends_with("low ≤ $3"), "the text cursor after what is typed up to it: {before:?}");
    }

    #[test]
    fn recommend_panel_is_a_grid_of_tasks_and_tiers() {
        let mut a = app();
        // Unscored, so no box has a model and every row is one line, at 60 columns too.
        let mut data = std::mem::take(&mut a.data);
        data.models.iter_mut().for_each(|m| m.fit.clear());
        a.set_data(data);
        (a.view, a.task_cur) = (View::Recommend, TASKS.iter().position(|t| t.name == "vision").unwrap());
        let lines = recommend(&a, 60, &mut vec![]).0;
        let text: Vec<String> = lines.iter().map(ToString::to_string).collect();
        assert_eq!(
            text[0], "from: ● mine · ○ all · ○ ✓ selected ",
            "the models it ranks come first, the dot on the ones in use"
        );
        let gap = text.iter().position(String::is_empty).unwrap();
        let warn = text.iter().position(|l| l.starts_with("warning: ")).unwrap();
        assert!(warn > 2 && text[1..warn].join(" ") == TIER_LEGEND, "the legend wraps: {:?}", &text[..warn]);
        // Then what it leaves out.
        let said = text[warn..gap].iter().map(|l| l.trim()).collect::<Vec<_>>().join(" ");
        assert_eq!(said, format!("warning: {UNRATED}"));
        let head = gap + 1;
        assert_eq!(
            text[head].split_whitespace().collect::<Vec<_>>(),
            ["free", "low", "≤$2", "mid", "≤$5", "high"],
            "a box per tier, with its cap"
        );
        // A row per task and + new task, the cursor on the cursor task's name alone.
        let names: Vec<&str> = text[head + 1..].iter().map_while(|l| l.split_whitespace().next()).collect();
        let bare: Vec<&str> = names.iter().map(|n| n.trim_matches(['▌', '▐'])).collect();
        assert_eq!(bare, TASKS.iter().map(|t| t.name).chain(["+"]).collect::<Vec<_>>());
        assert_eq!(names.iter().filter(|n| n.starts_with('▌')).collect::<Vec<_>>(), [&"▌vision▐"]);
        let cursor = &lines[head + 1 + a.task_cur].spans[1];
        assert_eq!(
            (cursor.content.as_ref(), cursor.style.fg, cursor.style.bg),
            ("vision", Some(TASK[a.task_cur]), Some(CURSOR))
        );
        // Under the grid, the task and when to use it, on one line.
        let under = head + 1 + TASKS.len() + 2;
        assert_eq!(
            [&text[under][..], &text[under + 1]],
            ["vision screenshots, UI mockups, diagrams as input ·", "       ranked by overall"],
            "the rank's words together, under the use"
        );
        let long: Vec<&String> = text.iter().filter(|l| l.chars().count() > 60).collect();
        assert!(long.is_empty(), "wrapped to the width: {long:?}");
        let mut b = app();
        let data = std::mem::take(&mut b.data);
        b.set_data(data);
        (b.view, b.task_sel) = (View::Recommend, 3);
        // On the `from` line it is on a tab alone, the one in use bold, and "(filtered)" says a search narrows them.
        (b.among, b.query) = (Some(1), "o".into());
        let (top, block, ..) = recommend(&b, 200, &mut vec![]);
        let on: Vec<&Span> = top.iter().flat_map(|l| l.spans.iter()).filter(|s| s.style.bg == Some(CURSOR)).collect();
        assert!(on.len() == 3 && on[1].content == "○ all" && block == Some(0..1), "{on:?} {block:?}");
        let mine = top[0].spans.iter().find(|s| s.content == "● mine").unwrap();
        assert!(mine.style == TAB_ON && top[0].to_string().ends_with(" (filtered)"), "{}", top[0]);
        let narrow = recommend(&b, 44, &mut vec![]).0;
        assert_eq!(narrow[1].to_string(), "      (filtered)", "it wraps where the tabs fill the line");
        // On a tier, the cursor is on its box, as wide as its widest entry, and not on the name.
        (b.among, b.query) = (None, String::new());
        let mut spots = vec![];
        let all = recommend(&b, 200, &mut spots).0;
        let bars: Vec<&str> = all
            .iter()
            .flat_map(|l| l.spans.iter())
            .filter(|s| s.style.bg == Some(CURSOR))
            .map(|s| &*s.content)
            .collect();
        assert_eq!(bars.concat(), "▌opus · $5.0 ·\u{a0}150 ▾▐", "mid of overall, with its dropdown's ▾");
        // A click on the ▾ opens the dropdown, the box's other cells move the cursor to it.
        let menu = spots.iter().find(|p| matches!(p.at, Stop::Menu(..))).expect("a spot on the ▾");
        let line = all[menu.lines.start].to_string();
        assert_eq!(line.chars().nth(menu.x.start), Some('▾'), "{line}");
        let marks: Vec<&Span> = all.iter().flat_map(|l| l.spans.iter()).filter(|s| s.content.ends_with('▾')).collect();
        let muted = marks.iter().filter(|s| s.style.fg == Some(MUTED)).count();
        assert!(marks.len() > 1 && muted == marks.len() - 1, "every box's ▾, muted off the cursor: {marks:?}");
        assert_eq!(spots.iter().filter(|p| matches!(p.at, Stop::Menu(..))).count(), marks.len(), "each ▾ clicks");
        // Narrow, the ▾ goes on a line with room for it, so it adds none: overall stays 2 lines.
        let text: Vec<String> = recommend(&b, 60, &mut vec![]).0.iter().map(ToString::to_string).collect();
        let at = text.iter().position(|l| l.starts_with(" overall")).unwrap();
        assert!(text[at + 1].contains("120 ▾") && text[at + 2].starts_with(" coding"), "{text:#?}");
        let row = all.iter().map(ToString::to_string).find(|l| l.starts_with(" overall")).unwrap();
        let picks: Vec<&str> = row.split_whitespace().filter(|w| w.ends_with(['s', 'h'])).collect();
        assert_eq!(picks, ["flash", "▌opus", "opus"], "what each tier picks: low, mid and high");
        assert!(
            all.iter().any(|l| l.to_string() == "  mid:         opus · $5.0 ·\u{a0}150"),
            "under the grid, the box's model"
        );
        // An entry wider than its box wraps between the name and its price, else between words, and a word
        // wider than it where it ends: nothing is cut.
        let text = |l: &Vec<Span>| l.iter().map(|s| &*s.content).collect::<String>();
        let wrap = |w| {
            wrap_spans(entry(&b, "overall", 2, b.tier_picks(0)[2], false, true), w, &[])
                .iter()
                .map(text)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            (wrap(17), wrap(12), wrap(9)),
            (
                vec!["opus · $5.0 ·\u{a0}150".to_string()],
                vec!["opus".into(), "$5.0 ·\u{a0}150".into()],
                vec!["opus".into(), "$5.0".into(), "·\u{a0}150".into()]
            ),
            "the dot before the price dropped where the price starts a line"
        );
        assert_eq!(wrap(3), ["opu", "s", "$5.", "0", "·\u{a0}1", "50"], "nothing cut");
        // In a narrow grid the row takes more lines, every name whole.
        let narrow: Vec<String> = recommend(&b, 40, &mut vec![]).0.iter().map(ToString::to_string).collect();
        assert!(
            narrow.iter().filter(|l| l.contains("opus")).count() > 1 && !narrow.concat().contains('…'),
            "{narrow:#?}"
        );
        // Among all, a model out of reach is grey, and says so under the grid; one of yours keeps its price's colour.
        let mut data = std::mem::take(&mut b.data);
        data.models[0].available = false;
        b.set_data(data);
        b.all = true;
        b.rebuild();
        let all = recommend(&b, 200, &mut vec![]).0;
        let of = |n: &str| all.iter().flat_map(|l| l.spans.iter()).find(|s| s.content == n).unwrap().style;
        assert!(of("opus").fg == Some(MUTED) && of("flash").fg != Some(MUTED), "{:?} {:?}", of("opus"), of("flash"));
        let said = |n: &str| all.iter().any(|l| l.to_string().contains(&format!("{n} not available")));
        assert!(said("·\u{a0}150") && !said("·\u{a0}120"), "opus says so, flash does not");
        // In a narrow panel the line under the grid wraps too, its name and what it says whole.
        let under: Vec<String> = recommend(&b, 32, &mut vec![]).0.iter().map(ToString::to_string).collect();
        let at = under.iter().position(|l| l.starts_with("  mid:")).unwrap();
        assert_eq!(
            under[at..at + 2],
            ["  mid:         opus · $5.0 ·\u{a0}150", "               not available"],
            "{under:#?}"
        );
        assert!(under[at..].iter().all(|l| Span::raw(l.as_str()).width() <= 32), "{under:#?}");
        // A search underlines in the name, and it wraps where it did.
        let long = || vec![Span::raw("Claude Opus 4.5"), Span::raw(" $5")];
        let lit = wrap_spans(long(), 12, &found("Claude Opus 4.5", "claude"));
        assert_eq!(
            lit.iter().map(text).collect::<Vec<_>>(),
            wrap_spans(long(), 12, &[]).iter().map(text).collect::<Vec<_>>()
        );
        assert!(lit[0][0].style.add_modifier.contains(Modifier::UNDERLINED), "{lit:?}");
        // A wide character takes two cells of the box.
        let wide: Vec<String> = wrap_spans(vec![Span::raw("通义千问 Max")], 5, &[]).iter().map(text).collect();
        assert_eq!(wide, ["通义", "千问", "Max"]);
        // A task of your own is the last row, in a colour of its own, each tier with the model you gave it; under
        // the grid, what you wrote it is about and that it is picked over a built-in task.
        a.store.toggle_favorite("debugging:mid", "opus");
        a.store.toggle_favorite("debugging:high", "opus");
        a.store.set_about("debugging", "finding and fixing a bug");
        (a.view, a.task_cur) = (View::Recommend, TASKS.len());
        let lines = recommend(&a, 80, &mut vec![]).0;
        let text: Vec<String> = lines.iter().map(ToString::to_string).collect();
        let own = text.iter().position(|l| l.starts_with("▌debugging▐")).unwrap();
        assert_eq!(
            (lines[own].spans[1].style.fg, text[own + 1].trim(), text[own + 2].as_str()),
            (Some(task_color("debugging")), "+ new task", ""),
            "+ new task after it"
        );
        assert_ne!(task_color("debugging"), task_color("docs"), "tasks of your own can differ in colour");
        assert!(
            OWN.iter().all(|c| !TASK.contains(c) && ![STAR, BAD].contains(c)),
            "and none has a built-in task's, nor the gold ★'s, nor the red ✗'s"
        );
        let models =
            |row: &str| row.split("★ ").skip(1).map(|s| s.trim_end_matches([' ', '▾']).to_string()).collect::<Vec<_>>();
        assert_eq!(models(&text[own]), ["opus · $5.0", "opus · $5.0"], "mid and high");
        assert_eq!(text[own + 3], "debugging finding and fixing a bug", "what you wrote is when to use it");
        // Its name and what it is about are written there, the text cursor after what is typed,
        // the row keeping its models.
        let what = What::Task { was: Some("debugging".into()), other: "debugging".into(), about: true };
        a.input = Input::Task(Edit { what, text: "bugs".into(), cur: 4, err: None });
        let (lines, _, _, at) = recommend(&a, 80, &mut vec![]);
        let text: Vec<String> = lines.iter().map(ToString::to_string).collect();
        assert_eq!((text[own + 3].as_str(), at), ("debugging bugs", Some((own + 3, 14))));
        assert!(text[own].contains("opus"), "{}", text[own]);
        // The name being typed shows on the row too, and a new task's on `+ new task`.
        let what = What::Task { was: Some("debugging".into()), other: "bugs".into(), about: false };
        a.input = Input::Task(Edit { what, text: "debug".into(), cur: 5, err: None });
        let text: Vec<String> = recommend(&a, 80, &mut vec![]).0.iter().map(ToString::to_string).collect();
        assert!(text[own].starts_with("▌debug▐") && text[own + 3] == "debug bugs", "{}", text[own]);
        let what = What::Task { was: None, other: String::new(), about: false };
        (a.input, a.task_cur) = (Input::Task(Edit { what, text: "quick".into(), cur: 5, err: None }), TASKS.len() + 1);
        let text: Vec<String> = recommend(&a, 80, &mut vec![]).0.iter().map(ToString::to_string).collect();
        assert!(text[own + 1].starts_with("▌+ quick"), "{}", text[own + 1]);
        (a.input, a.task_cur) = (Input::None, TASKS.len());
        // A tier's model is in that tier's box.
        a.store.toggle_favorite("debugging:low", "flash");
        let text: Vec<String> = recommend(&a, 80, &mut vec![]).0.iter().map(ToString::to_string).collect();
        assert_eq!(models(&text[own]), ["flash · $0.10", "opus · $5.0", "opus · $5.0"]);
    }

    #[test]
    fn a_wide_task_name_keeps_its_boxes_under_the_tiers() {
        let mut a = app();
        // 13 characters in 26 cells, more than the 20 the names' column takes: cut, its ▾s are
        // drawn where a click opens their dropdowns.
        a.store.set_about("コードレビューと修正の作業", "reviewing code");
        a.store.toggle_favorite("コードレビューと修正の作業:high", "opus");
        a.view = View::Recommend;
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(200, 60)).unwrap();
        term.draw(|f| draw(&mut a, f)).unwrap();
        let buf = term.backend().buffer();
        let off: Vec<String> = (a.spots.iter())
            .filter(|(r, s)| matches!(s, Stop::Menu(..)) && buf[(r.x, r.y)].symbol() != "▾")
            .map(|(r, s)| format!("{s:?} at {},{} is on {:?}", r.x, r.y, buf[(r.x, r.y)].symbol()))
            .collect();
        assert!(off.is_empty(), "{}", off.join("\n"));
        let lines = recommend(&a, 200, &mut vec![]).0;
        assert!(lines.iter().any(|l| l.to_string().contains(" コードレビューと修… ")), "cut where it says so");
    }

    #[test]
    fn a_kept_search_keeps_the_list_keys() {
        let theme = " ↓ ↑ preview · / search · space enter saves · esc t close";
        assert_eq!(kept_hint(theme, " · "), " ↓ ↑ preview · n N match · space enter saves · esc clear");
        let dev = "↓ ↑ move  / search  v range  space enter toggle  e leave out  esc close";
        assert_eq!(kept_hint(dev, "  "), "↓ ↑ move  n N match  v range  space enter toggle  e leave out  esc clear");
    }
}
