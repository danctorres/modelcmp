//! Text shared by the CLI and the TUI: number formatting, the detail page, the comparison grid.

use crate::data::{Data, Model, Offer, norm};
use crate::fit::{self, TASKS};
use crate::store::{Store, slot};
use std::ops::Range;

pub fn money(x: f64) -> String {
    match x {
        0.0 => "free".into(),
        x if x < 1.0 => format!("{x:.2}"),
        x if x < 10.0 => format!("{x:.1}"),
        x => format!("{x:.0}"),
    }
}

/// A theme's colours: the terminal's 16 in ANSI order (black, red, green, yellow, blue,
/// magenta, cyan, white, then the bright ones), default text, the background, and the accents
/// developers and harnesses take theirs from. There have to be at least as many as Via names
/// (`data::vias`, less omp, which has pi's), their count has to divide the 210 of `dev_color` (6, 10 and 14 do), and
/// they have to stand far enough apart to tell two developers by colour
/// (`accents_are_told_apart`). Nord's own palette is four blues, so its last two are tints of it rather than a fifth near-blue.
/// Every colour here is drawn as text on the background, so each has to read on it
/// (`every_theme_reads_on_its_own_background`); a status pill takes its text from `bg`
/// and its fill from the half of a colour furthest from it, so `ansi[0]` need not stand out.
/// What a colour says has to stay apart from what another says (`roles_are_told_apart`): red,
/// green, `text` and the muted `ansi[8]` as a worst, a best, a plain and a greyed value, the
/// six task colours from each other, and every accent from the muted one.
pub struct Palette {
    pub ansi: [u32; 16],
    pub text: u32,
    /// Painted under everything, so a dark theme stays dark on a light terminal and the other way round.
    pub bg: u32,
    /// The share of the mark colour in a marked row's fill, in percent (`tui::fill`): 12, but on
    /// sepia's cream, where that much blue is the grey of the cursor's fill.
    pub wash: u32,
    pub accents: &'static [u32],
}

/// `t` in the TUI: the terminal's own colours, or one of these palettes in their place, in
/// tuiman's order: the dark ones, then the light ones, then the retro machines. Left out as
/// twins of one that is in: everforest (nord, the same slate), rose-pine (catppuccin, the same
/// dark plum), paper (github-light, the same white) and gruvbox-light (sepia, the same cream).
/// A theme's own palette is only the starting point:
/// what a colour is here has to read on the background and stay apart from its neighbours, so a
/// light theme keeps the darker half of a pair and a monochrome one drifts in hue as it ramps.
pub const THEMES: [(&str, Option<Palette>); 19] = [
    ("terminal", None),
    (
        "gruvbox",
        Some(Palette {
            ansi: [
                0x282828, 0xea6962, 0x98971a, 0xd79921, 0x458588, 0xb16286, 0x689d6a, 0xa89984, 0x928374, 0xfb4934,
                0xb8bb26, 0xfabd2f, 0x9ecfe0, 0xd3869b, 0x8ec07c, 0xebdbb2,
            ],
            text: 0xebdbb2,
            bg: 0x282828,
            wash: 12,
            accents: &[
                0xfb4934, 0xb8bb26, 0xfabd2f, 0xd3869b, 0xfe8019, 0xea6962, 0x98971a, 0xd79921, 0x458588, 0xb16286,
                0x689d6a, 0xd65d0e, 0xa89984, 0x7daea3,
            ],
        }),
    ),
    (
        "nord",
        Some(Palette {
            ansi: [
                0x3b4252, 0xbf616a, 0xa3be8c, 0xebcb8b, 0x6f9bd6, 0xb48ead, 0x88c0d0, 0xe5e9f0, 0x7b88a1, 0xd08770,
                0xc0dba2, 0xf0d399, 0xa3d0e8, 0xc895bf, 0x8fbcbb, 0xeceff4,
            ],
            text: 0xd8dee9,
            bg: 0x2e3440,
            wash: 12,
            accents: &[
                0xbf616a, 0xd08770, 0xebcb8b, 0xa3be8c, 0xb48ead, 0x88c0d0, 0x81a1c1, 0xe5e9f0, 0xd6c1d2, 0x5fa8a0,
            ],
        }),
    ),
    (
        "catppuccin",
        Some(Palette {
            ansi: [
                0x45475a, 0xf38ba8, 0xa6e3a1, 0xf9e2af, 0x89b4fa, 0xf5c2e7, 0x94e2d5, 0xbac2de, 0x7f849c, 0xeba0ac,
                0xd0f0a0, 0xfae3c0, 0x74c7ec, 0xf7d0ee, 0xa8e8dd, 0xa6adc8,
            ],
            text: 0xcdd6f4,
            bg: 0x1e1e2e,
            wash: 12,
            accents: &[
                0xf5e0dc, 0xf5c2e7, 0xcba6f7, 0xf38ba8, 0xfab387, 0xf9e2af, 0xa6e3a1, 0x94e2d5, 0x89b4fa, 0xbac2de,
            ],
        }),
    ),
    (
        "dracula",
        Some(Palette {
            ansi: [
                0x21222c, 0xff5555, 0x50fa7b, 0xf1fa8c, 0xbd93f9, 0xff79c6, 0x8be9fd, 0xf8f8f2, 0x7f8bb8, 0xff6e6e,
                0x8affa6, 0xffffa5, 0xd6acff, 0xff92df, 0xa4ffff, 0xffffff,
            ],
            text: 0xf8f8f2,
            bg: 0x282a36,
            wash: 12,
            accents: &[
                0xff5555, 0x50fa7b, 0xf1fa8c, 0xbd93f9, 0xff79c6, 0x8be9fd, 0xffb86c, 0xf8f8f2, 0x6a9fe8, 0xd6acff,
            ],
        }),
    ),
    (
        "tokyonight",
        Some(Palette {
            ansi: [
                0x15161e, 0xf7768e, 0x9ece6a, 0xe0af68, 0x7aa2f7, 0xbb9af7, 0x7dcfff, 0xa9b1d6, 0x565f89, 0xff7a93,
                0xb9f27c, 0xff9e64, 0x2ac3de, 0xc7a9ff, 0x8ddfff, 0xc0caf5,
            ],
            text: 0xc0caf5,
            bg: 0x1a1b26,
            wash: 12,
            accents: &[
                0xf7768e, 0x9ece6a, 0xe0af68, 0x7aa2f7, 0xbb9af7, 0x7dcfff, 0x2ac3de, 0x73daca, 0xc0caf5, 0xff007c,
            ],
        }),
    ),
    (
        "kanagawa",
        Some(Palette {
            ansi: [
                0x16161d, 0xc34043, 0x98bb6c, 0xc0a36e, 0x7e9cd8, 0x957fb8, 0x6a9589, 0xc8c093, 0x727169, 0xe82424,
                0x76946a, 0xe6c384, 0xa3d4d5, 0x938aa9, 0x7aa89f, 0xdcd7ba,
            ],
            text: 0xdcd7ba,
            bg: 0x1f1f28,
            wash: 12,
            accents: &[
                0xe46876, 0x98bb6c, 0xe6c384, 0x957fb8, 0xffa066, 0xdcd7ba, 0x7fb4ca, 0xc0a36e, 0xd27e99, 0x6a9589,
            ],
        }),
    ),
    (
        "monokai",
        Some(Palette {
            ansi: [
                0x272822, 0xf92672, 0xa6e22e, 0xf4bf75, 0x66d9ef, 0xae81ff, 0xa1efe4, 0xf8f8f2, 0x75715e, 0xff5c9b,
                0xc3ee5e, 0xfd971f, 0x8ee7fa, 0xc9a6ff, 0xc4f7f0, 0xf9f8f5,
            ],
            text: 0xf8f8f2,
            bg: 0x272822,
            wash: 12,
            accents: &[
                0xf92672, 0xa6e22e, 0xf4bf75, 0x66d9ef, 0xae81ff, 0xa1efe4, 0xfd971f, 0xf8f8f2, 0xcc7833, 0xff5c9b,
            ],
        }),
    ),
    (
        "github",
        Some(Palette {
            ansi: [
                0x484f58, 0xff7b72, 0x3fb950, 0xd29922, 0x58a6ff, 0xbc8cff, 0x39c5cf, 0xb1bac4, 0x6e7681, 0xffa198,
                0x56d364, 0xe3b341, 0x79c0ff, 0xd2a8ff, 0x56d4dd, 0xf0f6fc,
            ],
            text: 0xe6edf3,
            bg: 0x0d1117,
            wash: 12,
            accents: &[
                0xff7b72, 0x3fb950, 0x58a6ff, 0xbc8cff, 0x39c5cf, 0xf0883e, 0xdb61a2, 0xb1bac4, 0xa5d6ff, 0xe3b341,
            ],
        }),
    ),
    (
        "solarized",
        Some(Palette {
            ansi: [
                0x073642, 0xdc322f, 0x859900, 0xb58900, 0x268bd2, 0xd33682, 0x2aa198, 0xeee8d5, 0x657b83, 0xcb4b16,
                0xa4b81c, 0xd4a520, 0x6c9fd8, 0x6c71c4, 0x48c4b8, 0xfdf6e3,
            ],
            text: 0x93a1a1,
            bg: 0x002b36,
            wash: 12,
            accents: &[
                0xdc322f, 0xd33682, 0x6c71c4, 0x268bd2, 0x2aa198, 0x859900, 0xd4a520, 0x48c4b8, 0x93a1a1, 0x6c9fd8,
            ],
        }),
    ),
    (
        "synthwave",
        Some(Palette {
            ansi: [
                0x262335, 0xfe4450, 0x72f1b8, 0xfede5d, 0x2de2e6, 0xff7edb, 0x36f9f6, 0xffffff, 0x848bbd, 0xf97e72,
                0x9cf7cf, 0xfff08a, 0x6be8ff, 0xffa3e8, 0x8ffcfa, 0xffffff,
            ],
            text: 0xf0eff1,
            bg: 0x241b2f,
            wash: 12,
            accents: &[
                0xff7edb, 0xfede5d, 0xf97e72, 0x72f1b8, 0xfe4450, 0xb893ce, 0x2de2e6, 0x6be8ff, 0x9cf7cf, 0xff8b39,
            ],
        }),
    ),
    (
        "cyberpunk",
        Some(Palette {
            ansi: [
                0x000b1e, 0xff0055, 0x00ff9c, 0xfcee0c, 0x0abdc6, 0xea00d9, 0x00e8ff, 0xd7d7d5, 0x5a6a8a, 0xff3377,
                0x5fffb8, 0xfff35c, 0x3dd8ff, 0xff5cf0, 0x7cf3ff, 0xffffff,
            ],
            text: 0x0abdc6,
            bg: 0x000b1e,
            wash: 12,
            accents: &[
                0xfcee0c, 0xff0055, 0x00ff9c, 0xea00d9, 0xff8a00, 0xb14aeb, 0x3dd8ff, 0xff5cf0, 0x5fffb8, 0xd7d7d5,
            ],
        }),
    ),
    (
        "github-light",
        Some(Palette {
            ansi: [
                0xffffff, 0xca212d, 0x116329, 0x8f5f00, 0x0550ae, 0x7b4cd3, 0x0f6b5f, 0x6e7781, 0x57606a, 0xa40e26,
                0x4d6f00, 0x633c01, 0x0865d3, 0xb53682, 0x1a6e96, 0x1f2328,
            ],
            text: 0x1f2328,
            bg: 0xffffff,
            wash: 12,
            accents: &[
                0xca212d, 0x8f5f00, 0x187834, 0x0865d3, 0x7b4cd3, 0xb53682, 0x197379, 0x953800, 0x6639ba, 0x4d2d00,
            ],
        }),
    ),
    (
        "sepia",
        Some(Palette {
            ansi: [
                0xf4ecd8, 0xa33a2a, 0x2f6a1a, 0x7e5600, 0x2f5f8a, 0x7b4a7a, 0x1f6b55, 0x5b4636, 0x8a7660, 0xaa2e57,
                0x5a5c00, 0x8b4513, 0x24506f, 0x8a3a6a, 0x33605c, 0x3b2d22,
            ],
            text: 0x5b4636,
            bg: 0xf4ecd8,
            wash: 16,
            accents: &[
                0xa33a2a, 0x7e5600, 0x7b4a7a, 0x376662, 0x5b4636, 0x1f4f8f, 0x4c631f, 0xaa2e57, 0x6b4f9e, 0x3b2d22,
            ],
        }),
    ),
    (
        "catppuccin-light",
        Some(Palette {
            ansi: [
                0xeff1f5, 0xc10d34, 0x296e1b, 0x795b00, 0x1957d2, 0x7b33d9, 0x106a6f, 0x6c6f85, 0x7a7d91, 0xae343f,
                0x546600, 0xa34207, 0x07669a, 0x9a3c7f, 0x4858b7, 0x4c4f69,
            ],
            text: 0x4c4f69,
            bg: 0xeff1f5,
            wash: 12,
            accents: &[
                0xc10d34, 0xa34207, 0x795b00, 0x296e1b, 0x106a6f, 0x1957d2, 0x7b33d9, 0x9a3c7f, 0x4858b7, 0x8b5042,
            ],
        }),
    ),
    (
        "amber",
        Some(Palette {
            ansi: [
                0x1a1000, 0xcc5500, 0xffd966, 0xffcc00, 0xe07b00, 0xff9900, 0xffe8a3, 0xffb000, 0x8f6200, 0xc9a227,
                0xff7700, 0xffee99, 0xffe8a3, 0xffb84d, 0xfff8e7, 0xfffbf0,
            ],
            text: 0xffb000,
            bg: 0x1a1000,
            wash: 12,
            accents: &[
                0xffcc00, 0xffe8a3, 0xff9900, 0xcc5500, 0xfff3d0, 0xb87333, 0xff7700, 0xe0c080, 0xffe066, 0xffffff,
            ],
        }),
    ),
    (
        "phosphor",
        Some(Palette {
            ansi: [
                0x0a0f0a, 0x00cc44, 0xccffcc, 0x99ffaa, 0x33ccaa, 0x66ff99, 0xccffcc, 0x33ff66, 0x1a7f33, 0x2fbf55,
                0x66ff99, 0xccff66, 0xe6ffe6, 0x99ffaa, 0xffffff, 0xffffff,
            ],
            text: 0x33ff66,
            bg: 0x0a0f0a,
            wash: 12,
            accents: &[
                0x33ff66, 0x99ffaa, 0xccffcc, 0xccff66, 0x66ff99, 0x2ecc71, 0x00cc44, 0xa0d8a0, 0xffffff, 0x7fbf8f,
            ],
        }),
    ),
    (
        "c64",
        Some(Palette {
            ansi: [
                0x40318d, 0xe8958c, 0xa9ff9f, 0xedf171, 0x9a97ff, 0xd08ad3, 0x75cec8, 0xb8b0ff, 0x9a8fd8, 0xffb3ab,
                0xcfffc9, 0xf8fbb0, 0xffffff, 0xe4b3e6, 0xa8e6e2, 0xffffff,
            ],
            text: 0xb8b0ff,
            bg: 0x40318d,
            wash: 12,
            accents: &[
                0xedf171, 0xa9ff9f, 0x75cec8, 0xb2b2b2, 0xffffff, 0xd08ad3, 0xe8958c, 0xb8b0ff, 0xcfffc9, 0xf8fbb0,
            ],
        }),
    ),
    (
        "gameboy",
        Some(Palette {
            ansi: [
                0x9bbc0f, 0x553311, 0x046800, 0x6b4a00, 0x0d4f3c, 0x3b2d0c, 0x073b26, 0x0f380f, 0x306230, 0x0b2b0b,
                0x5c3a10, 0x4a5a10, 0x00355f, 0x46360a, 0x7a2e0e, 0x0b2b0b,
            ],
            text: 0x0f380f,
            bg: 0x9bbc0f,
            wash: 12,
            // Seven, not ten: the DMG screen is four shades of green, and ten colours that far
            // apart would have to leave it for navy and purple. Seven still divide the 210 and
            // keep the Via names apart, as the terminal's own seven do. Only the mark (slot 12)
            // is a blue, as a green or a brown would give its fill the cursor's.
            accents: &[0x0f380f, 0x5a5a00, 0x3b2d0c, 0x13463a, 0x704214, 0x046800, 0x8a1c00],
        }),
    ),
];

/// Index into `THEMES` of a saved name; an unknown or empty one is the terminal's.
pub fn theme(name: &str) -> usize {
    THEMES.iter().position(|t| t.0 == name).unwrap_or(0)
}

/// Upper edges of the price levels in blended $/1M: free, then up to 0.5, 2, 5 and 15, then above.
/// They fall between model families: Flash-Lite, Flash and Haiku, Sonnet and Pro, Opus, Fable.
pub const LEVELS: [f64; 5] = [0.0, 0.5, 2.0, 5.0, 15.0];

/// Price level of a blended price, 0 (free) to 5 (above the last edge).
pub fn level(x: f64) -> usize {
    LEVELS.iter().take_while(|&&e| x > e).count()
}

/// Name of price level `l`: `free`, `≤$0.5` ... `≤$15`, `>$15`.
pub fn level_label(l: usize) -> String {
    match LEVELS.get(l) {
        Some(0.0) => "free".into(),
        Some(e) => format!("≤${e}"),
        None => format!(">${}", LEVELS[LEVELS.len() - 1]),
    }
}

/// How long ago the data was fetched: `5m`, `3h`, `2d`.
pub fn age(d: std::time::Duration) -> String {
    match d.as_secs() {
        s if s < 3600 => format!("{}m", s / 60),
        s if s < 48 * 3600 => format!("{}h", s / 3600),
        s => format!("{}d", s / 86400),
    }
}

pub fn ctx(n: u64) -> String {
    match n {
        0 => "-".into(),
        n if n >= 1_000_000 => format!("{}M", (n as f64 / 1e6 * 10.0).round() / 10.0),
        n => format!("{}k", n / 1000),
    }
}

pub fn score(x: Option<f64>) -> String {
    x.map_or("-".into(), |v| format!("{v:.0}"))
}

/// The value a task shows for the model (`fit::shown`), when it qualifies.
pub fn task_score(m: &Model, task: &str) -> Option<f64> {
    let t = fit::task(task)?;
    Some(fit::shown(m, t, fit::fit(m, t)?))
}

/// The items whose model no other beats on both `score` and blended price: the cheapest at
/// every quality level. Scores compare as shown, whole points, so a fraction of a point never
/// justifies a higher price. Free models count at $0; ties on both keep every tied item.
pub fn frontier<'m, T: Copy>(
    items: &[T],
    model: impl Fn(T) -> &'m Model,
    score: impl Fn(&Model) -> Option<f64>,
) -> Vec<T> {
    let pts: Vec<Option<(f64, f64)>> =
        items.iter().map(|&i| Some((score(model(i))?.round(), model(i).cost()?))).collect();
    let beats = |(s, c): (f64, f64), (t, d): (f64, f64)| s >= t && c <= d && (s > t || c < d);
    // ponytail: O(n²), fine for the few hundred rows the harnesses list.
    items
        .iter()
        .zip(&pts)
        .filter(|(_, a)| a.is_some_and(|a| !pts.iter().any(|b| b.is_some_and(|b| beats(b, a)))))
        .map(|(&i, _)| i)
        .collect()
}

/// A task's price frontier, cheapest first: each entry costs more and scores higher, the last
/// being the best model for the task. Each price level keeps only its best entry, since models
/// that close in price are not worth choosing between. Models without a price or a score are
/// left out, as are those further behind the best of them than the `low` tier allows, on
/// coding for "value": the frontier is a recommendation, and cheap alone is not one. What a
/// tier picks stays on the line though its level has a better entry, so that the tier's is the
/// cheapest good enough for it, and so does a favorite of the frontier. The `favorites` join the line
/// whether or not they earn a place on it, and are ranked only if they are among `models`;
/// with no score for the task, a favorite's score is NaN, which `priced` shows as `-`, and no
/// tier picks it on merit.
/// With the line come the keys on it only for being favorites.
pub fn task_frontier<'a>(
    models: impl Iterator<Item = &'a Model>,
    t: &fit::Task,
    favorites: &[&'a Model],
) -> (Vec<(&'a Model, f64)>, Vec<&'a str>) {
    // Not one only your machine runs, whose scores and context are not that copy's: free, it
    // would take every tier it has the score for. A favorite is still on the line.
    let ranked = fit::rank(models.filter(|m| !m.local()), t);
    // Behind the best the frontier can take, which has a price; "value" on coding, with no
    // line while coding has no pace.
    let coder = matches!(t.need, fit::Need::Coder);
    let behind =
        behind(ranked.iter().map(|e| e.0).filter(|m| m.cost().is_some()), if coder { "coding" } else { t.name });
    let near = |m: &Model| behind(m).map_or(!coder, |b| b <= TIERS[0].1);
    let ranked: Vec<_> = ranked.iter().copied().filter(|(m, _)| near(m)).collect();
    let mut v = frontier(&ranked, |(m, _)| m, |m| task_score(m, t.name));
    let fav = |m: &Model| favorites.iter().any(|f| f.key == m.key);
    let dearest =
        |a: &(&Model, f64), b: &(&Model, f64)| b.0.cost().partial_cmp(&a.0.cost()).unwrap_or(std::cmp::Ordering::Equal);
    // A favorite ahead of the models it ties, as on the frontier the same price is the same score.
    let favored = |a: &(&Model, f64), b: &(&Model, f64)| fav(b.0).cmp(&fav(a.0));
    // What each tier picks of the whole frontier, cheapest first.
    v.sort_by(|a, b| dearest(b, a).then_with(|| favored(a, b)));
    let picked: Vec<&str> =
        TIERS.iter().filter_map(|x| pick(v.iter(), t.name, x.0)).map(|e| e.0.key.as_str()).collect();
    // Dearest first so each level keeps its best, then back to cheapest first.
    v.sort_by(|a, b| dearest(a, b).then_with(|| favored(a, b)));
    let mut last = None;
    v.retain(|(m, _)| {
        let l = Some(level(m.cost().unwrap_or(0.0)));
        std::mem::replace(&mut last, l) != l || fav(m) || picked.contains(&m.key.as_str())
    });
    let mut off = Vec::new();
    for &f in favorites {
        if !v.iter().any(|(m, _)| m.key == f.key) {
            v.push((f, fit::fit(f, t).unwrap_or(f64::NAN)));
            off.push(f.key.as_str());
        }
    }
    v.sort_by(dearest);
    v.reverse();
    (v, off)
}

/// `task_frontier` with the favorites `store` has for the task: the ones among `pool`.
pub fn task_line<'a>(
    models: impl Iterator<Item = &'a Model>,
    pool: impl Iterator<Item = &'a Model>,
    store: &Store,
    t: &fit::Task,
) -> (Vec<(&'a Model, f64)>, Vec<&'a str>) {
    let favs = store.task_favorites(t.name);
    let fav: Vec<&Model> = pool.filter(|m| favs.contains(&m.key.as_str())).collect();
    task_frontier(models, t, &fav)
}

/// The line of a task of your own: the models you gave it and its tiers, the ones among
/// `pool`, cheapest first as a built-in task's line.
pub fn custom_line<'a>(pool: impl Iterator<Item = &'a Model>, store: &Store, task: &str) -> Vec<&'a Model> {
    let favs = store.task_favorites(task);
    let mut v: Vec<&Model> = pool.filter(|m| favs.contains(&m.key.as_str())).collect();
    v.sort_by(|a, b| a.cost().partial_cmp(&b.cost()).unwrap_or(std::cmp::Ordering::Equal));
    v
}

/// An entry of that line: `★ name $price`, `★ name [key] $price` with `keyed`, then the tiers
/// the model is for, `(low, mid)`, unless it is the task's, which every tier without one takes.
pub fn custom_priced(m: &Model, store: &Store, task: &str, keyed: bool) -> String {
    let is = |x: Option<&str>| store.favorite(&slot(task, x)) == Some(m.key.as_str());
    let tiers: Vec<&str> = TIERS.iter().map(|t| t.0).filter(|x| is(Some(x))).collect();
    let tiers = if is(None) || tiers.is_empty() { String::new() } else { format!(" ({})", tiers.join(", ")) };
    let key = if keyed { format!(" [{}]", m.key) } else { String::new() };
    format!("★ {}{key} {}{tiers}", m.name, price(m))
}

/// Two values of a column in order, the highest first with `desc`: blanks last either way.
pub fn by_value(x: Option<f64>, y: Option<f64>, desc: bool) -> std::cmp::Ordering {
    use std::cmp::Ordering::{Equal, Greater, Less};
    match (x, y) {
        (Some(x), Some(y)) if desc => y.total_cmp(&x),
        (Some(x), Some(y)) => x.total_cmp(&y),
        (Some(_), None) => Less,
        (None, Some(_)) => Greater,
        (None, None) => Equal,
    }
}

/// `--tier` names and how far behind your best model each lets its pick be, in months of
/// progress (`fit::add_lag`). A tier picks the cheapest frontier entry no further behind than
/// that, so `high` the best, to the whole point as the frontier compares; the best too on a
/// task with no such scale.
// ponytail: fixed months, picked from how the tiers fell on 2026-10's data; tune them if the
// picks look off.
pub const TIERS: [(&str, f64); 3] = [("low", 8.0), ("mid", 3.0), ("high", 0.0)];

/// How far a model is behind the best of `models` on `task`, in months of progress
/// (`Model::lag`); none for a model, or a task, without it.
fn behind<'a>(models: impl Iterator<Item = &'a Model>, task: &str) -> impl Fn(&Model) -> Option<f64> {
    let best = models.filter_map(|m| m.lag.get(task)).fold(f64::INFINITY, |a, &b| a.min(b));
    move |m| m.lag.get(task).map(|l| l - best)
}

/// The entry of a task's cheapest-first frontier that `tier` picks; `None` for an empty frontier.
pub fn pick<'a, 'm: 'a>(
    front: impl Iterator<Item = &'a (&'m Model, f64)> + Clone,
    task: &str,
    tier: &str,
) -> Option<&'a (&'m Model, f64)> {
    let limit = TIERS.iter().find(|t| t.0 == tier).map_or(0.0, |t| t.1);
    let behind = behind(front.clone().map(|e| e.0), task);
    front.clone().find(|(m, _)| behind(m).is_some_and(|b| b <= limit)).or(front.last())
}

/// The entry of a task's line (`task_line`) that `tier` picks, as `--tier` does: your favorite
/// for the tier, else for the task, beats the pick on merit, which a favorite on the line only
/// for being one (`off`) is neither best nor good enough for; one the line lacks gives way to
/// the next.
pub fn tier_pick<'m>(
    front: &[(&'m Model, f64)],
    off: &[&str],
    store: &Store,
    task: &str,
    tier: &str,
) -> Option<(&'m Model, f64)> {
    let fav = store.tier_favorites(task, tier).find_map(|k| front.iter().find(|(m, _)| m.key == k)).copied();
    fav.or_else(|| pick(front.iter().filter(|(m, _)| !off.contains(&m.key.as_str())), task, tier).copied())
}

/// `$1.5`, or `free`.
pub fn usd(x: f64) -> String {
    if x == 0.0 { "free".into() } else { format!("${}", money(x)) }
}

/// A model's blended price as `usd`: after a `~` when it is the list one (`Model::listed`), `-` when unknown.
pub fn price(m: &Model) -> String {
    m.quoted().map_or("-".into(), |(o, listed)| format!("{}{}", if listed { "~" } else { "" }, usd(o.blended())))
}

/// What a frontier line of `modelcmp recommend` shows.
pub const FRONTIER_LEGEND: &str = "best per price: the top model at each price level, cheapest first, as name [key] $/1M tokens (score on the task), \
     plus ★ your favorite, marked not recommended when it is not one";

/// What a task of your own says of itself, where a built-in one says what it measures.
pub const CUSTOM_ABOUT: &str = "your own task";

/// When a task of your own is the one to pick, where a built-in one has its `when`.
pub const CUSTOM_WHEN: &str = "work that fits it, even when a built-in task fits too";

/// `name $price (score)` for a frontier entry, `name [key] $price (score)` with `keyed`,
/// `★ name ...` when it is your favorite for the task, `(-)` for a favorite with no score.
pub fn priced(m: &Model, s: f64, keyed: bool, favorite: bool) -> String {
    let key = if keyed { format!(" [{}]", m.key) } else { String::new() };
    let flag = if favorite { "★ " } else { "" };
    let s = if s.is_nan() { "-".into() } else { format!("{s:.0}") };
    format!("{flag}{}{key} {} ({s})", m.name, price(m))
}

pub fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.into() } else { s.chars().take(n - 1).chain(['…']).collect() }
}

fn or_dash(s: &str) -> &str {
    if s.is_empty() { "-" } else { s }
}

/// Where every word of the search `q` starts a word of one of `fields`, any case, punctuation
/// ignored: char ranges in each field, for highlighting. "opus 4.5", "anthropic opus" and
/// "gpt5" match a name and developer; "mini" misses Gemini. None when a word misses them all.
/// With `typos`, a word of 3+ letters that starts no word may be one slip off one ("opsu").
pub fn hits<const N: usize>(q: &str, fields: [&str; N], typos: bool) -> Option<[Vec<Range<usize>>; N]> {
    let mut out = [(); N].map(|_| vec![]);
    // A "+" is dropped, not spelled out as in a key: `word_hit` reads the fields without it.
    for t in q.split_whitespace().map(|w| norm(&w.replace('+', ""))).filter(|t| !t.is_empty()) {
        let find = |typo| fields.iter().enumerate().find_map(|(i, s)| Some((i, word_hit(s, &t, typo)?)));
        let fuzzy = typos && t.len() >= 3 && t.bytes().all(|b| b.is_ascii_alphabetic());
        let (i, r) = find(false).or_else(|| fuzzy.then(|| find(true)).flatten())?;
        out[i].push(r);
    }
    Some(out)
}

/// The chars of `s` that normalized `t` covers where it starts a word of `s`: at the beginning,
/// after a space or punctuation, or where letters turn to digits or back ("Qwen3", "4o").
/// With `typo`, what it covers is instead one slip away from `t`, see `one_typo`.
fn word_hit(s: &str, t: &str, typo: bool) -> Option<Range<usize>> {
    // `s` normalized, the char index in `s` of each of its bytes, and where its words start.
    let (mut n, mut pos, mut starts, mut prev) = (String::new(), vec![], vec![], ' ');
    for (i, c) in s.chars().enumerate() {
        if c.is_ascii_alphanumeric() {
            let c = c.to_ascii_lowercase();
            if !prev.is_ascii_alphanumeric() || prev.is_ascii_digit() != c.is_ascii_digit() {
                starts.push(n.len());
            }
            n.push(c);
            pos.push(i);
        }
        prev = c;
    }
    let (at, len) = if typo {
        let (n, t) = (n.as_bytes(), t.as_bytes());
        starts.into_iter().find_map(|k| {
            [t.len(), t.len() + 1, t.len() - 1]
                .into_iter()
                .find(|&l| k + l <= n.len() && one_typo(&n[k..k + l], t))
                .map(|l| (k, l))
        })?
    } else {
        (starts.into_iter().find(|&k| n[k..].starts_with(t))?, t.len())
    };
    Some(pos[at]..pos[at + len - 1] + 1)
}

/// `a` is `b` with one letter wrong, missing, extra, or swapped with the next.
fn one_typo(a: &[u8], b: &[u8]) -> bool {
    let p = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    let (a, b) = (&a[p..], &b[p..]);
    match (a.len(), b.len()) {
        (0, 0) => false,
        (x, y) if x == y => a[1..] == b[1..] || (x >= 2 && a[..2] == [b[1], b[0]] && a[2..] == b[2..]),
        (x, y) if x == y + 1 => a[1..] == *b,
        (x, y) if x + 1 == y => *a == b[1..],
        _ => false,
    }
}

/// Via of a model you have no access to.
pub const OUT_OF_REACH: &str = "not available";

/// Said with access to no model, when every model is shown instead.
pub const NO_ACCESS: &str = "no harness models found, showing all models";

/// Said by a key that acts on the selected models when none is.
pub const NO_SELECTED: &str = "no selected models: space selects the one under the cursor";

/// Whether `m` is one to use: any with `all`, else the ones you have access to, or every model
/// when you have access to none (`any` is whether you have access to one).
pub fn in_reach(m: &Model, all: bool, any: bool) -> bool {
    all || m.available || !any
}

/// Via as shown: the harnesses, or `OUT_OF_REACH` for a model you have no access to.
pub fn shown_via(m: &Model, any: bool) -> Vec<&str> {
    if in_reach(m, false, any) { m.via.iter().map(String::as_str).collect() } else { vec![OUT_OF_REACH] }
}

/// `shown_via` on one line, "-" with no harness: `claude, opencode, pi`. `get` is the mark of a
/// download `x` offers in the TUI (`↓ 2.5 GB`, `App::download`), which follows the harnesses
/// and stands for `OUT_OF_REACH`, as in the table.
pub fn via(m: &Model, any: bool, get: Option<&str>) -> String {
    let harnesses = shown_via(m, any).into_iter().filter(|v| get.is_none() || *v != OUT_OF_REACH);
    let mut said = harnesses.collect::<Vec<_>>().join(", ");
    if let Some(get) = get {
        said = format!("{said} {get}").trim_start().into();
    }
    or_dash(&said).into()
}

/// Models the user should see: the ones in reach, with `marked_only` only the selected ones.
/// Unlike `S` in the TUI, it leaves out a selected model out of reach, as an agent cannot call it.
pub fn visible<'a>(
    data: &'a Data,
    store: &'a Store,
    all: bool,
    marked_only: bool,
) -> impl Iterator<Item = (usize, &'a Model)> {
    let any = data.any_available();
    data.models
        .iter()
        .enumerate()
        .filter(move |(_, m)| in_reach(m, all, any) && (!marked_only || store.is_marked(&m.key)))
}

/// Everything about one model, one line per entry; `any` is whether you have access to a model.
pub fn detail_lines(m: &Model, store: &Store, any: bool) -> Vec<String> {
    detail_rows(m, store, any, None).into_iter().map(|r| r.1).collect()
}

/// `detail_lines`, each task's fit line with the task's name, for the TUI to colour it.
pub fn detail_rows(m: &Model, store: &Store, any: bool, get: Option<&str>) -> Vec<(Option<&'static str>, String)> {
    let yes = |b: bool| if b { "yes" } else { "no" };
    let source = crate::data::source();
    // With Artificial Analysis a task's score is one benchmark, so none are listed under it.
    let benches = if source == crate::data::Source::Aa { "" } else { ", benchmarks at best effort" };
    let mut v = vec![
        format!("{}{}", m.name, if store.is_excluded(&m.key) { " (excluded)" } else { "" }),
        format!("  developer:  {}", or_dash(&m.developer)),
        format!("  via:        {}", via(m, any, get)),
        // What you'd pay and to whom, `~` as in the table; the list price when it is not yours.
        // Per 1M tokens, as the providers below say.
        format!(
            "  price:      {}",
            m.quoted().map_or("-".into(), |(o, listed)| {
                let whom: &str = if o.available { &o.provider_name } else { "list price" };
                if o.input + o.output == 0.0 {
                    return format!("free  ({whom})");
                }
                let p = |x| format!("{}{}", if listed { "~" } else { "" }, money(x));
                let cached = o.cache_read.map_or("-".into(), p);
                format!("{} in · {cached} cached · {} out  ({whom})", p(o.input), p(o.output))
            })
        ),
        format!("  context:    {} (max output {})", ctx(m.context), ctx(m.max_output)),
    ];
    // Under context: what the model costs in time, next to what it holds.
    if m.tps.is_some() || m.ttft.is_some() {
        let n = |v: Option<f64>, f: fn(f64) -> String| v.map_or("-".into(), f);
        v.push(format!(
            "  speed:      {} tokens/s, answer starts in {} (median)",
            n(m.tps, |v| format!("{v:.0}")),
            n(m.ttft, |v| format!("{v:.1}s"))
        ));
    }
    v.extend([
        format!(
            "  features:   tools {} · reasoning {} · vision {} · open weights {}",
            yes(m.tool_call),
            yes(m.reasoning),
            yes(m.vision),
            yes(m.open_weights)
        ),
        format!("  released:   {}   knowledge: {}", or_dash(&m.release), or_dash(&m.knowledge)),
    ]);
    v.extend([
        format!("  note:       {}", store.note(&m.key).unwrap_or("-")),
        format!(
            "  favorite:   {}",
            Some(store.favorite_for(&m.key).join(", ")).filter(|s| !s.is_empty()).unwrap_or("-".into())
        ),
        String::new(),
        format!("  {} {}", source.index().0, score(m.eci)),
        format!("  task fit ({}, value a percentile{benches}):", source.scale()),
    ]);
    // The benchmarks listed under a task, so the rest come after: one a task's score here does
    // not come from, as the fields a column's dropdown adds with Artificial Analysis, or the
    // benchmarks of a task the model has no score for.
    let mut used: Vec<&str> = vec![];
    let mut fits = vec![];
    for t in TASKS {
        if let Some(s) = fit::fit(m, t) {
            fits.push((v.len(), t.name));
            v.push(format!("    {:<13}{:>4.0}  {}", t.name, fit::shown(m, t, s), t.about));
            v.extend(task_benches(m, t));
            used.extend(used_benches(t));
        }
    }
    let mut other = m.scores.iter().filter(|(b, _)| !used.contains(&b.as_str())).peekable();
    if other.peek().is_some() {
        v.push(String::new());
        v.push(format!("  other benchmarks ({}, best effort setting):", source.label()));
        v.extend(other.map(|(b, s)| format!("    {:<36}{:>5.1}%", b, s * 100.0)));
    }
    v.push(String::new());
    // A page per line, as the details panel cuts a line at its width. Down here, as `o` opens them.
    let mut pages = m.links().into_iter().map(|(_, url)| url);
    v.push(format!("  pages:      {}", pages.next().unwrap_or("-".into())));
    v.extend(pages.map(|url| format!("              {url}")));
    v.push(String::new());
    v.push("  providers ($ per 1M tokens):".into());
    // Each label over its column of the rows below.
    v.push(format!("{:26}{:>8} {:>8} {:>8}  model id", "", "in", "cached", "out"));
    let mut offers: Vec<&Offer> = m.offers.iter().collect();
    // Available first, then cheapest, then by name; unknown price ("-") last.
    let cost = |o: &Offer| if o.unpriced { f64::MAX } else { o.blended() };
    // By name first, each lowercased once; the stable sort after keeps it at one price.
    offers.sort_by_cached_key(|&o| (o.provider_name.to_lowercase(), &o.id));
    offers.sort_by(|a, b| b.available.cmp(&a.available).then(cost(a).total_cmp(&cost(b))));
    for o in offers {
        // The prices before the id, which is what a panel 80 columns wide cuts.
        let line = format!(
            "    {} {:<20}{:>8} {:>8} {:>8}  {:<34}{}",
            if o.available { "●" } else { " " },
            truncate(&o.provider_name, 19),
            if o.unpriced { "-".into() } else { money(o.input) },
            o.cache_read.map_or("-".into(), money),
            if o.unpriced { "-".into() } else { money(o.output) },
            truncate(&o.id, 33),
            o.via.join(", ")
        );
        // No padding left after an id without a harness, for a terminal to wrap.
        v.push(line.trim_end().to_string());
    }
    let mut rows: Vec<_> = v.into_iter().map(|s| (None, s)).collect();
    for (i, t) in fits {
        rows[i].0 = Some(t);
    }
    rows
}

/// The benchmarks a task's score comes from, with the source in use.
pub fn used_benches(t: &fit::Task) -> Vec<&'static str> {
    let (own, all) = t.sourced();
    own.map_or(all.to_vec(), |b| vec![b])
}

/// Under a task's fit in the details: the model's score on each benchmark the task's score
/// comes from, the ones it was tested on. None for a task with no benchmarks of its own, whose
/// `about` says what ranks it, nor when the score is one benchmark, the same number again.
fn task_benches(m: &Model, t: &fit::Task) -> Vec<String> {
    let (own, all) = t.sourced();
    let all = if own.is_some() { &[] } else { all };
    all.iter().filter_map(|b| Some(format!("      {:<34}{:>5.1}%", b, m.scores.get(*b)? * 100.0))).collect()
}

#[derive(Default)]
pub struct Row {
    pub label: String,
    pub cells: Vec<String>,
    /// The value of each cell, empty for rows not compared.
    pub vals: Vec<Option<f64>>,
    /// The best and the worst value, colouring every cell that has one; none when they all agree.
    pub ext: Option<(f64, f64)>,
    /// The cells holding a list price (`Model::listed`), drawn muted; empty off the price rows.
    pub listed: Vec<bool>,
    /// The topic the row belongs to, named in a rule above its first row; `""` for the
    /// model, price and context rows at the top.
    pub section: &'static str,
}

/// The mark of a model's download, when `x` offers one in the TUI (`App::download`).
pub type Get<'a> = &'a dyn Fn(&Model) -> Option<String>;

/// Rows for side-by-side comparison; Via as the table shows it (`shown_via`).
pub fn compare_rows(models: &[&Model], any: bool, get: Get) -> Vec<Row> {
    fn row(label: &str, vals: Vec<Option<f64>>, fmt: impl Fn(f64) -> String, higher: bool) -> Row {
        let mut it = vals.iter().flatten();
        let ext = it.next().and_then(|&first| {
            let (lo, hi) = it.fold((first, first), |(lo, hi), &v| (lo.min(v), hi.max(v)));
            (lo != hi).then_some(if higher { (hi, lo) } else { (lo, hi) })
        });
        let cells = vals.iter().map(|v| v.map_or("-".into(), &fmt)).collect();
        Row { label: label.into(), cells, vals, ext, ..Default::default() }
    }
    // A list price, yours having none, after a `~` as in the table, and as there neither the
    // best nor the worst: it has no value to colour.
    let price = |label: &str, f: fn(&Offer) -> f64| {
        let quoted = models.iter().map(|m| m.quoted());
        let offers: Vec<_> = quoted.map(|q| (q.map(|q| f(q.0)), q.is_some_and(|q| q.1))).collect();
        let mut r = row(label, offers.iter().map(|&(v, listed)| v.filter(|_| !listed)).collect(), money, false);
        for (c, v) in r.cells.iter_mut().zip(&offers).filter_map(|(c, &(v, listed))| Some((c, v.filter(|_| listed)?))) {
            *c = format!("~{}", money(v));
        }
        r.listed = offers.iter().map(|o| o.1).collect();
        r
    };
    let mut rows = vec![
        Row { label: "model".into(), cells: models.iter().map(|m| m.name.clone()).collect(), ..Default::default() },
        Row {
            label: "via".into(),
            cells: models.iter().map(|m| via(m, any, get(m).as_deref())).collect(),
            ..Default::default()
        },
        price("$ in / 1M", |o| o.input),
        // No cache discount: cached input costs full price.
        price("$ cached in / 1M", |o| o.cache_read.unwrap_or(o.input)),
        price("$ out / 1M", |o| o.output),
        row("context", models.iter().map(|m| Some(m.context as f64)).collect(), |c| ctx(c as u64), true),
        Row {
            section: "scores",
            ..row(crate::data::source().index().0, models.iter().map(|m| m.eci).collect(), |v| format!("{v:.1}"), true)
        },
    ];
    // Under context, before the scores' rule: the ECI row is the last so far.
    if models.iter().any(|m| m.tps.is_some() || m.ttft.is_some()) {
        let at = rows.len() - 1;
        rows.insert(at, row("answer starts", models.iter().map(|m| m.ttft).collect(), |v| format!("{v:.1}s"), false));
        rows.insert(at, row("tokens/s", models.iter().map(|m| m.tps).collect(), |v| format!("{v:.0}"), true));
    }
    for t in TASKS.iter().filter(|t| t.name != "overall") {
        let vals: Vec<Option<f64>> = models.iter().map(|m| task_score(m, t.name)).collect();
        if vals.iter().any(Option::is_some) {
            rows.push(Row { section: "scores", ..row(t.name, vals, |v| format!("{v:.0}"), true) });
        }
    }
    let mut benches: Vec<&String> = models.iter().flat_map(|m| m.scores.keys()).collect();
    benches.sort();
    benches.dedup();
    for b in benches {
        let vals = models.iter().map(|m| m.scores.get(b).copied()).collect();
        rows.push(Row { section: "benchmarks", ..row(b, vals, |v| format!("{:.1}%", v * 100.0), true) });
    }
    rows
}

/// The conclusion above a comparison: for each question, which model wins and by how much
/// against the runner-up, as `[question, winner, margin]`. Values compare as shown, so two
/// that print the same tie, and the winner cell names every tied model. With fewer than two
/// models to compare, the winner is `-`.
pub fn verdict(models: &[&Model]) -> Vec<[String; 3]> {
    let coding = |m: &Model| task_score(m, "coding");
    // A list price is not one you'd pay: it wins nothing here, as in the rows below.
    let cost = |m: &Model| m.quoted().filter(|q| !q.1).map(|q| q.0.blended());
    // Value hangs on the price, so a list price wins nothing there either.
    let any_value = |m: &Model| m.fit.get("value").copied();
    let value = |m: &Model| any_value(m).filter(|_| !m.listed());
    // Left with fewer than two by a list price, when `any` counts two with it, the row says so,
    // not that data is missing.
    let on_price = |mut row: [String; 3], any: &dyn Fn(&Model) -> Option<f64>| {
        if row[1] == "-" && models.iter().filter(|m| any(m).is_some()).count() > 1 {
            row[2] = "only a list price (~)".into();
        }
        row
    };
    vec![
        on_price(best(models, "cheaper", cost, false, usd), &|m| m.cost()),
        best(models, "better at coding", coding, true, |v| format!("{v:.0}")),
        on_price(best(models, "better value", value, true, |v| format!("{v:.0}")), &any_value),
    ]
}

fn best(
    models: &[&Model],
    question: &str,
    val: impl Fn(&Model) -> Option<f64>,
    higher: bool,
    fmt: impl Fn(f64) -> String,
) -> [String; 3] {
    let mut vals: Vec<(&str, f64)> = models.iter().filter_map(|m| Some((m.name.as_str(), val(m)?))).collect();
    vals.sort_by(|a, b| if higher { b.1.total_cmp(&a.1) } else { a.1.total_cmp(&b.1) });
    if vals.len() < 2 {
        return [question.into(), "-".into(), "not enough data".into()];
    }
    let top = fmt(vals[0].1);
    let tied: Vec<&str> = vals.iter().take_while(|v| fmt(v.1) == top).map(|v| v.0).collect();
    let next = vals.get(tied.len()).map(|v| fmt(v.1));
    let (winner, margin) = match (tied.len(), next) {
        (1, Some(n)) => (tied[0].into(), format!("{top} vs {n}")),
        (_, Some(n)) => (format!("tie: {}", tied.join(", ")), format!("{top} each, next {n}")),
        (_, None) => (format!("tie: {}", tied.join(", ")), format!("{top} each")),
    };
    [question.into(), winner, margin]
}

#[cfg(test)]
#[allow(clippy::single_range_in_vec_init)]
mod tests {
    use super::*;

    #[test]
    fn details_show_the_price_then_pages_before_the_providers_by_name() {
        let offer = |name: &str| Offer { provider_name: name.into(), input: 1.0, output: 2.0, ..Default::default() };
        let offers = vec![offer("Zed"), offer("abc")];
        let m =
            Model { md: Some("a/b".into()), epoch: Some("b".into()), tps: Some(50.0), offers, ..Default::default() };
        let lines = detail_lines(&m, &Store::default(), false);
        let at = |start: &str| lines.iter().position(|l| l.starts_with(start)).unwrap();
        assert_eq!(at("  speed:"), at("  context:") + 1, "speed under context");
        let pages = at("  pages:");
        assert_eq!(lines[pages], "  pages:      https://models.dev/models/a/b/");
        assert_eq!(lines[pages + 1], "              https://epoch.ai/models/b");
        assert_eq!(at("  providers"), pages + 3, "pages last before the providers");
        assert_eq!(lines[at("  price:")], "  price:      1.0 in · - cached · 2.0 out  (list price)");
        assert!(at("      abc") < at("      Zed"), "at one price, by name");
        let free = Offer { provider_name: "Zed".into(), available: true, ..Default::default() };
        let m = Model { offers: vec![free], ..Default::default() };
        assert!(detail_lines(&m, &Store::default(), true).contains(&"  price:      free  (Zed)".to_string()));
    }

    #[test]
    fn details_say_the_benchmarks_a_task_score_comes_from() {
        let coding = fit::task("coding").unwrap();
        let mut m = Model::default();
        assert!(task_benches(&m, fit::task("value").unwrap()).is_empty(), "no benchmarks of its own");
        m.scores = [("DeepSWE", 0.5), ("HLE", 0.25), ("scicode", 0.1)].map(|(b, s)| (b.to_string(), s)).into();
        m.fit.insert("coding".into(), 50.0);
        assert_eq!(task_benches(&m, coding), [format!("      {:<34} 50.0%", "DeepSWE")], "not another task's");
        // Each under its task; one no task uses comes after them.
        let lines = detail_lines(&m, &Store::default(), false);
        let at = |start: &str| lines.iter().position(|l| l.starts_with(start)).unwrap();
        assert_eq!(at("      DeepSWE"), at("    coding") + 1);
        assert_eq!(at("    scicode"), at("  other benchmarks") + 2);
        assert_eq!(at("    HLE"), at("  other benchmarks") + 1, "reasoning has no fit here, but the score shows");
    }

    #[test]
    fn tiers_pick_the_cheapest_good_enough() {
        let model = |key: &str, lag: f64| Model {
            key: key.into(),
            lag: [("coding".to_string(), lag)].into(),
            ..Default::default()
        };
        // Months behind the source's best, which is none of them: the tiers go by yours.
        let ms = [model("free", 14.0), model("mini", 9.5), model("sonnet", 5.0), model("opus", 2.0)];
        let front: Vec<(&Model, f64)> = ms.iter().map(|m| (m, 0.0)).collect();
        let key = |front: &[(&Model, f64)], task, tier| pick(front.iter(), task, tier).map(|e| e.0.key.clone());
        assert_eq!(key(&front, "coding", "low").as_deref(), Some("mini"), "7.5 months behind opus");
        assert_eq!(key(&front, "coding", "mid").as_deref(), Some("sonnet"), "3 months behind is still mid");
        assert_eq!(key(&front, "coding", "high").as_deref(), Some("opus"));
        assert_eq!(key(&front[..2], "coding", "mid").as_deref(), Some("mini"), "behind the best there is");
        assert_eq!(key(&front, "value", "low").as_deref(), Some("opus"), "no such scale: the best");
        assert_eq!(key(&[], "coding", "low"), None);
    }

    #[test]
    fn a_favorite_takes_the_place_of_a_model_it_ties() {
        let model = |key: &str, coding: f64, price: f64| Model {
            key: key.into(),
            fit: [("coding".to_string(), coding)].into(),
            offers: vec![Offer { input: price, output: price, ..Default::default() }],
            ..Default::default()
        };
        let ms = [model("a", 60.4, 1.0), model("b", 59.8, 1.0), model("weak", 55.0, 1.5), model("top", 90.0, 10.0)];
        fn keys<'a>(ms: &'a [Model], fav: &[&'a Model]) -> (Vec<&'a str>, Vec<&'a str>) {
            let (line, off) = task_frontier(ms.iter(), fit::task("coding").unwrap(), fav);
            (line.iter().map(|(m, _)| m.key.as_str()).collect(), off)
        }
        let line = |fav: &[usize]| keys(&ms, &fav.iter().map(|&i| &ms[i]).collect::<Vec<_>>());
        assert_eq!(line(&[]), (vec!["a", "top"], vec![]));
        assert_eq!(line(&[1]), (vec!["b", "top"], vec![]), "the same price and shown score: recommended");
        assert_eq!(line(&[2]), (vec!["a", "weak", "top"], vec!["weak"]), "beaten: there only as the favorite");
    }

    #[test]
    fn search_words_start_words_of_the_name_or_developer() {
        let matches = |q, f| hits(q, f, false).is_some();
        let (opus, pro, mini, qwen) = (
            ["Claude Opus 4.5", "anthropic"],
            ["Gemini 3.1 Pro", "google"],
            ["GPT-5 mini", "openai"],
            ["Qwen3-235B-A22B", "alibaba"],
        );
        for q in ["opus", "anthropic opus", "opus claude", "OPUS 4.5", "claudeopus45", ""] {
            assert!(matches(q, opus), "{q}");
        }
        assert!(matches("gemini pro", pro) && matches("gpt5", mini) && matches("qwen 3", qwen));
        assert!(matches("235b", qwen) && matches("mini", mini));
        assert!(!matches("mini", pro), "mini is not a word of Gemini");
        assert!(!matches("laude", opus) && !matches("opus sonnet", opus));
        assert_eq!(hits("4.5 anthropic", opus, false), Some([vec![12..15], vec![0..9]]), "the dot sits inside the hit");
        assert_eq!(hits("a22b", qwen, false), Some([vec![11..15], vec![]]));
    }

    #[test]
    fn typos_are_one_slip_in_a_word_of_letters() {
        let (opus, son) = (["Claude Opus 4.5", "anthropic"], ["Claude Sonnet 4.5", "anthropic"]);
        let typo = |q, f| hits(q, f, true);
        for q in ["opsu", "anthorpic opus", "claud sonet", "sonnnet", "snonet", "sonbet", "sonen"] {
            assert!(typo(q, son).is_some() || typo(q, opus).is_some(), "{q}");
        }
        assert_eq!(typo("sonet", son), Some([vec![7..13], vec![]]), "the whole word lights up");
        assert_eq!(typo("opsu", opus), Some([vec![7..11], vec![]]));
        assert!(hits("opsu", opus, false).is_none(), "only when asked");
        assert!(typo("4.6", opus).is_none() && typo("op", son).is_none(), "digits and short words stay exact");
        assert!(typo("sonnet", opus).is_none() && typo("osup", opus).is_none(), "one slip, not two");
    }

    #[test]
    fn formats() {
        assert_eq!(money(0.0), "free");
        assert_eq!(
            [0.0, 0.3, 0.5, 2.0, 10.0, 20.0].map(level),
            [0, 1, 1, 2, 4, 5],
            "an edge belongs to the level below"
        );
        assert_eq!(money(0.153), "0.15");
        assert_eq!(money(2.5), "2.5");
        assert_eq!(money(15.0), "15");
        assert_eq!(ctx(0), "-");
        assert_eq!(ctx(128_000), "128k");
        assert_eq!(ctx(1_048_576), "1M");
        assert_eq!(truncate("abcdef", 4), "abc…");
        assert_eq!(truncate("abc", 4), "abc");
    }

    #[test]
    fn compare_marks_best() {
        let mk = |n: &str, c: u64, eci: Option<f64>| Model { name: n.into(), context: c, eci, ..Default::default() };
        let (a, b) = (mk("a", 100, Some(150.0)), mk("b", 200, None));
        let rows = compare_rows(&[&a, &b], false, &|_| None);
        let find = |l: &str| rows.iter().find(|r| r.label == l).unwrap();
        // With access to no model, Via is empty; with access to another, it says so, as the table.
        assert_eq!(find("via").cells, vec!["-", "-"]);
        assert_eq!(compare_rows(&[&a], true, &|_| None)[1].cells, vec![OUT_OF_REACH]);
        assert_eq!(via(&a, true, Some("↓ 2.5 GB")), "↓ 2.5 GB", "a download stands for it");
        assert!(detail_lines(&a, &Store::default(), true).contains(&format!("  via:        {OUT_OF_REACH}")));
        assert_eq!(find("context").ext, Some((200.0, 100.0)));
        assert_eq!(find("ECI").ext, None, "a single value is not a comparison");
        assert_eq!(find("ECI").cells, vec!["150.0", "-"]);
        assert_eq!((find("context").section, find("ECI").section), ("", "scores"));
        let same = compare_rows(&[&a, &a], false, &|_| None);
        assert_eq!(same.iter().find(|r| r.label == "context").unwrap().ext, None, "equal values are not marked");
        // A list price is neither the best nor the worst, as in the table.
        let at = |p: f64| Offer { input: p, output: p, available: true, ..Default::default() };
        let yours = |p: f64| Model { offers: vec![at(p)], ..Default::default() };
        let unpriced = Offer { unpriced: true, ..at(0.0) };
        let listed = Model { offers: vec![unpriced, Offer { available: false, ..at(1.0) }], ..Default::default() };
        let rows = compare_rows(&[&yours(3.0), &yours(5.0), &listed], false, &|_| None);
        let price = rows.iter().find(|r| r.label == "$ in / 1M").unwrap();
        assert_eq!((price.cells[2].as_str(), price.ext), ("~1.0", Some((3.0, 5.0))));
    }

    #[test]
    fn verdict_picks_a_winner_per_question() {
        let offer = |p: f64| Offer { input: p, output: p, ..Default::default() };
        let mk = |n: &str, coding: Option<f64>, p: f64| Model {
            name: n.into(),
            fit: coding.map(|c| ("coding".to_string(), c)).into_iter().collect(),
            offers: vec![offer(p)],
            ..Default::default()
        };
        let mut ms = [mk("big", Some(90.0), 10.0), mk("small", Some(60.0), 2.0), mk("new", None, 1.0)];
        fit::add_value(&mut ms);
        let [big, small, new] = &ms;
        let v = verdict(&[big, small, new]);
        let s = |r: [&str; 3]| r.map(String::from);
        assert_eq!(v[0], s(["cheaper", "new", "$1.0 vs $2.0"]));
        assert_eq!(v[1], s(["better at coding", "big", "90 vs 60"]), "a model without the score is skipped");
        assert_eq!(v[2], s(["better value", "small", "75 vs 25"]), "the Value column's percentile");
        assert_eq!(verdict(&[big, new])[1], s(["better at coding", "-", "not enough data"]));
        assert_eq!(verdict(&[small, small])[0], s(["cheaper", "tie: small, small", "$2.0 each"]));
        let twin = mk("twin", Some(60.0), 2.0);
        assert_eq!(verdict(&[big, small, &twin])[0], s(["cheaper", "tie: small, twin", "$2.0 each, next $10"]));
        let unpriced = Offer { available: true, unpriced: true, ..Default::default() };
        let offers = vec![unpriced, Offer { available: false, ..offer(1.0) }];
        let mut ms = [ms[0].clone(), ms[1].clone(), Model { offers, ..mk("est", Some(95.0), 0.0) }];
        fit::add_value(&mut ms);
        let v = verdict(&[&ms[0], &ms[1], &ms[2]]);
        assert_eq!(v[0], s(["cheaper", "small", "$2.0 vs $10"]), "a list price is not one you'd pay");
        assert_eq!((v[2][0].as_str(), v[2][1].as_str()), ("better value", "small"), "nor one to divide by");
        let v = verdict(&[&ms[1], &ms[2]]);
        assert_eq!(v[0], s(["cheaper", "-", "only a list price (~)"]), "a pair with one says why it has no winner");
        assert_eq!(v[2], s(["better value", "-", "only a list price (~)"]));
        assert_eq!(v[1][1], "est", "coding does not hang on the price");
        let none = Model { offers: vec![], ..mk("none", None, 0.0) };
        let v = verdict(&[&ms[2], &none]);
        assert_eq!((v[0][2].as_str(), v[2][2].as_str()), ("not enough data", "not enough data"), "one price at all");
    }

    #[test]
    fn a_tier_picks_the_cheapest_of_the_whole_frontier() {
        let model = |key: &str, score: f64, price: f64, lag: f64| Model {
            key: key.into(),
            fit: [("coding".to_string(), score)].into(),
            shown: [("coding".to_string(), score)].into(),
            lag: [("coding".to_string(), lag)].into(),
            offers: vec![Offer { input: price, output: price, ..Default::default() }],
            ..Default::default()
        };
        fn keys<'a>(ms: &'a [Model], fav: &[&'a Model], task: &str) -> (Vec<&'a str>, Vec<&'a str>) {
            let (line, off) = task_frontier(ms.iter(), fit::task(task).unwrap(), fav);
            (line.iter().map(|(m, _)| m.key.as_str()).collect(), off)
        }
        let pick = |ms: &[Model], tier| {
            let (line, off) = task_frontier(ms.iter(), fit::task("coding").unwrap(), &[]);
            tier_pick(&line, &off, &Store::default(), "coding", tier).unwrap().0.key.clone()
        };
        // a and b share the ≤$2 level, c is beaten: mid is a, a third of b's price.
        let ms = [
            model("a", 162.0, 0.6, 3.0),
            model("c", 160.0, 0.8, 5.0),
            model("b", 164.0, 1.9, 1.0),
            model("top", 165.0, 10.0, 0.0),
        ];
        assert_eq!(keys(&ms, &[], "coding"), (vec!["a", "b", "top"], vec![]), "a stays, as what mid picks");
        assert_eq!((pick(&ms, "low"), pick(&ms, "mid"), pick(&ms, "high")), ("a".into(), "a".into(), "top".into()));
        let ms = [ms[0].clone(), ms[2].clone(), model("mini", 163.0, 1.0, 2.0), ms[3].clone()];
        assert_eq!(keys(&ms, &[], "coding").0, ["a", "b", "top"], "mini is no tier's: b is its level's best");
        assert_eq!(
            keys(&ms, &[&ms[2]], "coding"),
            (vec!["a", "mini", "b", "top"], vec![]),
            "a favorite of the frontier"
        );
        let mut ms = ms;
        ms[0].offers = vec![Offer { local: true, available: true, ..Default::default() }];
        assert_eq!(keys(&ms, &[], "coding").0, ["mini", "b", "top"], "one only your machine runs is not picked");
        assert_eq!(keys(&ms, &[&ms[0]], "coding"), (vec!["a", "mini", "b", "top"], vec!["a"]), "but as a favorite");
        // The level past the last edge has no top: $16 within 8 months, not the $75 best.
        let dear = [model("a", 157.0, 16.0, 8.0), model("top", 165.0, 75.0, 0.0)];
        assert_eq!(pick(&dear, "low"), "a");
        // Value goes by your best on coding, 9 months behind the source's, not by the source's.
        let mut old =
            [model("mine", 150.0, 1.0, 9.0), model("cheap", 147.0, 0.2, 12.0), model("weak", 140.0, 0.0, 19.0)];
        fit::add_value(&mut old);
        assert_eq!(keys(&old, &[], "value").0, ["cheap"], "weak is free, and 10 months behind yours");
    }
}
