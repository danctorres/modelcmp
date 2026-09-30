//! Text shared by the CLI and the TUI: number formatting, the detail page, the comparison grid.

use crate::data::{Data, Model, Offer, norm};
use crate::fit::{self, TASKS};
use crate::store::Store;
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
/// developers and harnesses take theirs from. Their count has to divide the 210 of `dev_color`
/// to keep the five harness names apart (5, 10 and 14 do; 6 and 7 collide), and they have to stand
/// far enough apart to tell two developers by colour (`accents_are_told_apart`). Nord's own
/// palette is four blues, so its last two are tints of it rather than a fifth near-blue.
/// Every colour here is drawn as text on the background, so each has to read on it
/// (`every_theme_reads_on_its_own_background`); a status pill takes its text from `bg`
/// and its fill from the half of a colour furthest from it, so `ansi[0]` need not stand out.
pub struct Palette {
    pub ansi: [u32; 16],
    pub text: u32,
    /// Painted under everything, so a dark theme stays dark on a light terminal and the other way round.
    pub bg: u32,
    pub accents: &'static [u32],
}

/// `t` in the TUI: the terminal's own colours, or one of these palettes in their place, in
/// tuiman's order: the dark ones, then the light ones, then the retro machines. Everforest is
/// left out as nord's twin on the same slate. A theme's own palette is only the starting point:
/// what a colour is here has to read on the background and stay apart from its neighbours, so a
/// light theme keeps the darker half of a pair and a monochrome one drifts in hue as it ramps.
pub const THEMES: [(&str, Option<Palette>); 21] = [
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
                0x3b4252, 0xbf616a, 0xa3be8c, 0xebcb8b, 0x5e81ac, 0xb48ead, 0x88c0d0, 0xe5e9f0, 0x7b88a1, 0xd08770,
                0xb5d19c, 0xf0d399, 0xa3d0e8, 0xc895bf, 0x8fbcbb, 0xeceff4,
            ],
            text: 0xd8dee9,
            bg: 0x2e3440,
            accents: &[
                0xbf616a, 0xd08770, 0xebcb8b, 0xa3be8c, 0xb48ead, 0x88c0d0, 0x81a1c1, 0xe5e9f0, 0xd6c1d2, 0x6c86a1,
            ],
        }),
    ),
    (
        "catppuccin",
        Some(Palette {
            ansi: [
                0x45475a, 0xf38ba8, 0xa6e3a1, 0xf9e2af, 0x89b4fa, 0xf5c2e7, 0x94e2d5, 0xbac2de, 0x7f849c, 0xeba0ac,
                0xb9e8b4, 0xfae3c0, 0xa6c8ff, 0xf7d0ee, 0xa8e8dd, 0xa6adc8,
            ],
            text: 0xcdd6f4,
            bg: 0x1e1e2e,
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
                0x69ff94, 0xffffa5, 0xd6acff, 0xff92df, 0xa4ffff, 0xffffff,
            ],
            text: 0xf8f8f2,
            bg: 0x282a36,
            accents: &[
                0xff5555, 0x50fa7b, 0xf1fa8c, 0xbd93f9, 0xff79c6, 0x8be9fd, 0xffb86c, 0xf8f8f2, 0x7f8bb8, 0xd6acff,
            ],
        }),
    ),
    (
        "tokyonight",
        Some(Palette {
            ansi: [
                0x15161e, 0xf7768e, 0x9ece6a, 0xe0af68, 0x7aa2f7, 0xbb9af7, 0x7dcfff, 0xa9b1d6, 0x565f89, 0xff7a93,
                0xb9f27c, 0xff9e64, 0x7da6ff, 0xc7a9ff, 0x8ddfff, 0xc0caf5,
            ],
            text: 0xc0caf5,
            bg: 0x1a1b26,
            accents: &[
                0xf7768e, 0x9ece6a, 0xe0af68, 0x7aa2f7, 0xbb9af7, 0x7dcfff, 0x2ac3de, 0x73daca, 0xc0caf5, 0xff007c,
            ],
        }),
    ),
    (
        "kanagawa",
        Some(Palette {
            ansi: [
                0x16161d, 0xc34043, 0x76946a, 0xc0a36e, 0x7e9cd8, 0x957fb8, 0x6a9589, 0xc8c093, 0x727169, 0xe82424,
                0x98bb6c, 0xe6c384, 0x7fb4ca, 0x938aa9, 0x7aa89f, 0xdcd7ba,
            ],
            text: 0xdcd7ba,
            bg: 0x1f1f28,
            accents: &[
                0xe46876, 0x98bb6c, 0xe6c384, 0x957fb8, 0xffa066, 0xdcd7ba, 0x7fb4ca, 0xc0a36e, 0x727169, 0x6a9589,
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
            accents: &[
                0xf92672, 0xa6e22e, 0xf4bf75, 0x66d9ef, 0xae81ff, 0xa1efe4, 0xfd971f, 0xf8f8f2, 0x75715e, 0xff5c9b,
            ],
        }),
    ),
    (
        "rose-pine",
        Some(Palette {
            ansi: [
                0x26233a, 0xeb6f92, 0x31748f, 0xf6c177, 0x9ccfd8, 0xc4a7e7, 0xebbcba, 0xe0def4, 0x6e6a86, 0xf2809f,
                0x4fa3c0, 0xffd39a, 0xb5e2e8, 0xd6c0f0, 0xf3d0ce, 0xffffff,
            ],
            text: 0xe0def4,
            bg: 0x191724,
            accents: &[
                0xf6c177, 0xebbcba, 0xc4a7e7, 0x6e6a86, 0xf2809f, 0xb5e2e8, 0x3e8fb0, 0x908caa, 0xf2e9e1, 0x9bc98c,
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
            accents: &[
                0xff7b72, 0x3fb950, 0x58a6ff, 0xbc8cff, 0x39c5cf, 0xf0883e, 0xdb61a2, 0xb1bac4, 0x6e7681, 0xe3b341,
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
            accents: &[
                0xfcee0c, 0xff0055, 0x00ff9c, 0xea00d9, 0xff8a00, 0xb14aeb, 0x3dd8ff, 0xff5cf0, 0x5fffb8, 0xd7d7d5,
            ],
        }),
    ),
    (
        "github-light",
        Some(Palette {
            ansi: [
                0xffffff, 0xcf222e, 0x116329, 0x4d2d00, 0x0969da, 0x8250df, 0x1b7c83, 0x6e7781, 0x57606a, 0xa40e26,
                0x1a7f37, 0x633c01, 0x218bff, 0xa475f9, 0x3192aa, 0x1f2328,
            ],
            text: 0x1f2328,
            bg: 0xffffff,
            accents: &[
                0xcf222e, 0x9a6700, 0x1a7f37, 0x0969da, 0x8250df, 0xbf3989, 0x1b7c83, 0x953800, 0x57606a, 0x4d2d00,
            ],
        }),
    ),
    (
        "gruvbox-light",
        Some(Palette {
            ansi: [
                0xfbf1c7, 0x9d0006, 0x79740e, 0xa86800, 0x076678, 0x8f3f71, 0x427b58, 0x3c3836, 0x7c6f64, 0xcc241d,
                0x5f7a1e, 0xb57614, 0x076678, 0xb16286, 0x3d7a4f, 0x282828,
            ],
            text: 0x3c3836,
            bg: 0xfbf1c7,
            accents: &[
                0x9d0006, 0x79740e, 0xb57614, 0x076678, 0x8f3f71, 0xaf3a03, 0x7c6f64, 0x3c3836, 0x458588, 0xb16286,
            ],
        }),
    ),
    (
        "paper",
        Some(Palette {
            ansi: [
                0xffffff, 0xc62828, 0x2e7d32, 0xb8860b, 0x1565c0, 0x6a1b9a, 0x00695c, 0x1a1a1a, 0x8a8a8a, 0x8e0000,
                0x1b5e20, 0x8f6b00, 0x0d47a1, 0x4a148c, 0x004d40, 0x000000,
            ],
            text: 0x1a1a1a,
            bg: 0xffffff,
            accents: &[
                0xc62828, 0x2e7d32, 0xb8860b, 0x1565c0, 0x6a1b9a, 0x00695c, 0xd84315, 0xad1457, 0x424242, 0x8e0000,
            ],
        }),
    ),
    (
        "sepia",
        Some(Palette {
            ansi: [
                0xf4ecd8, 0xa33a2a, 0x5f7a2e, 0x9a6a00, 0x2f5f8a, 0x7b4a7a, 0x3f7570, 0x5b4636, 0xa08c74, 0x8b2f22,
                0x4c631f, 0x8b4513, 0x24506f, 0x663c66, 0x33605c, 0x3b2d22,
            ],
            text: 0x5b4636,
            bg: 0xf4ecd8,
            accents: &[
                0xa33a2a, 0x9a6a00, 0x7b4a7a, 0x3f7570, 0x5b4636, 0x24506f, 0x4c631f, 0x2f6b4f, 0x6b4f9e, 0x3b2d22,
            ],
        }),
    ),
    (
        "amber",
        Some(Palette {
            ansi: [
                0x1a1000, 0xcc5500, 0xffd966, 0xffcc00, 0xe07b00, 0xff9900, 0xffe8a3, 0xffb000, 0x8f6200, 0xff7700,
                0xfff3d0, 0xffe066, 0xffe8a3, 0xffb84d, 0xfff8e7, 0xfffbf0,
            ],
            text: 0xffb000,
            bg: 0x1a1000,
            accents: &[
                0xffcc00, 0xffe8a3, 0xff9900, 0xcc5500, 0xfff3d0, 0xb87333, 0xff7700, 0x8f6200, 0xffe066, 0xffffff,
            ],
        }),
    ),
    (
        "phosphor",
        Some(Palette {
            ansi: [
                0x0a0f0a, 0x0f9f3f, 0x33ff66, 0x99ffaa, 0x1a7f33, 0x66ff99, 0xccffcc, 0x33ff66, 0x1a7f33, 0x2fbf55,
                0x66ff99, 0xccffcc, 0x2ecc71, 0x99ffaa, 0xe6ffe6, 0xffffff,
            ],
            text: 0x33ff66,
            bg: 0x0a0f0a,
            accents: &[
                0x33ff66, 0x99ffaa, 0xccffcc, 0x1a7f33, 0x66ff99, 0x2ecc71, 0x00cc44, 0xa0d8a0, 0xffffff, 0x7fbf8f,
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
            accents: &[
                0xedf171, 0xa9ff9f, 0x75cec8, 0xb2b2b2, 0xffffff, 0xd08ad3, 0xe8958c, 0xb8b0ff, 0xcfffc9, 0xf8fbb0,
            ],
        }),
    ),
    (
        "gameboy",
        Some(Palette {
            ansi: [
                0x9bbc0f, 0x553311, 0x145214, 0x6b4a00, 0x0d4f3c, 0x3b2d0c, 0x073b26, 0x0f380f, 0x306230, 0x5c3a10,
                0x1f5c2a, 0x4a5a10, 0x0b2b0b, 0x46360a, 0x2f5d3a, 0x0b2b0b,
            ],
            text: 0x0f380f,
            bg: 0x9bbc0f,
            // Five, not ten: the DMG screen is four shades of green, and ten colours that far
            // apart would have to leave it for navy and purple. Five still divide the 210 and
            // keep the harnesses apart, as the terminal's own five do.
            accents: &[0x0f380f, 0x306230, 0x3b2d0c, 0x13463a, 0x704214],
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

pub fn task_score(m: &Model, task: &str) -> Option<f64> {
    fit::fit(m, fit::task(task)?)
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
/// left out, as are those under the `low` tier's floor: the frontier is a recommendation, and
/// cheap alone is not one. The `favorites` join the line whether or not they earn a place
/// on it, and are ranked only if they are among `models`; with no score for the task, a
/// favorite's score is NaN, which `priced` shows as `-` and no tier floor reaches.
pub fn task_frontier<'a>(
    models: impl Iterator<Item = &'a Model>,
    t: &fit::Task,
    favorites: &[&'a Model],
) -> Vec<(&'a Model, f64)> {
    let ranked = fit::rank(models, t);
    let ranked: Vec<_> = ranked.into_iter().filter(|(_, s)| s.round() >= TIERS[0].1).collect();
    let mut v = frontier(&ranked, |(m, _)| m, |m| fit::fit(m, t));
    let by_cost = |v: &mut Vec<(&Model, f64)>| {
        v.sort_by(|a, b| b.0.cost().partial_cmp(&a.0.cost()).unwrap_or(std::cmp::Ordering::Equal));
    };
    // Dearest first so dedup keeps the best of each level, then back to cheapest first.
    by_cost(&mut v);
    v.dedup_by_key(|(m, _)| level(m.cost().unwrap_or(0.0)));
    for &f in favorites {
        if !v.iter().any(|(m, _)| m.key == f.key) {
            v.push((f, fit::fit(f, t).unwrap_or(f64::NAN)));
        }
    }
    by_cost(&mut v);
    v.reverse();
    v
}

/// Whether `key` is on the task's frontier among `models` on its merits, not only as the favorite.
pub fn recommended<'a>(models: impl Iterator<Item = &'a Model>, t: &fit::Task, key: &str) -> bool {
    task_frontier(models, t, &[]).iter().any(|(m, _)| m.key == key)
}

/// `--tier` names and their score floors. A tier picks the cheapest frontier entry at or
/// above its floor, scores compared as shown like the frontier does, or the best entry when
/// none reaches it; `high` always picks the best.
// ponytail: fixed floors on a percentile, tune them if the picks look off.
pub const TIERS: [(&str, f64); 3] = [("low", 50.0), ("mid", 75.0), ("high", f64::INFINITY)];

/// The entry of a cheapest-first frontier that `tier` picks; `None` for an empty frontier.
pub fn pick<'a, T>(front: &'a [(T, f64)], tier: &str) -> Option<&'a (T, f64)> {
    let floor = TIERS.iter().find(|t| t.0 == tier).map_or(f64::INFINITY, |t| t.1);
    front.iter().find(|(_, s)| s.round() >= floor).or(front.last())
}

/// `$1.5`, or `free`.
pub fn usd(x: f64) -> String {
    if x == 0.0 { "free".into() } else { format!("${}", money(x)) }
}

/// What a frontier line shows, for the recommend panel and `modelcmp recommend`, which adds the key.
pub fn frontier_legend(keyed: bool) -> String {
    format!(
        "best per price: the top model at each price level, cheapest first, as name{} $/1M tokens (the task's column), \
         plus ★ your favorite, marked not recommended when it is not one",
        if keyed { " [key]" } else { "" }
    )
}

/// `name $price (score)` for a frontier entry, `name [key] $price (score)` with `keyed`,
/// `★ name ...` when it is your favorite for the task, `(-)` for a favorite with no score.
pub fn priced(m: &Model, s: f64, keyed: bool, favorite: bool) -> String {
    let key = if keyed { format!(" [{}]", m.key) } else { String::new() };
    let flag = if favorite { "★ " } else { "" };
    let s = if s.is_nan() { "-".into() } else { format!("{s:.0}") };
    format!("{flag}{}{key} {} ({s})", m.name, m.cost().map_or("-".into(), usd))
}

pub fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.into() } else { s.chars().take(n - 1).chain(['…']).collect() }
}

/// Where a model or offer is available, "-" if nowhere: `claude, opencode, env`.
pub fn via(v: &[String]) -> String {
    if v.is_empty() { "-".into() } else { v.join(", ") }
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
    for t in q.split_whitespace().map(norm).filter(|t| !t.is_empty()) {
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

/// Models the user should see: the ones they have access to, unless `all` or none is available.
pub fn visible<'a>(
    data: &'a Data,
    store: &'a Store,
    all: bool,
    marked_only: bool,
) -> impl Iterator<Item = (usize, &'a Model)> {
    let any = data.models.iter().any(|m| m.available);
    data.models
        .iter()
        .enumerate()
        .filter(move |(_, m)| (all || !any || m.available) && (!marked_only || store.is_marked(&m.key)))
}

/// Everything about one model, one line per entry.
pub fn detail_lines(m: &Model, store: &Store) -> Vec<String> {
    let yes = |b: bool| if b { "yes" } else { "no" };
    let mut v = vec![
        format!("{}{}", m.name, if store.is_excluded(&m.key) { " (excluded)" } else { "" }),
        format!("  developer:  {}", or_dash(&m.developer)),
        format!("  via:        {}", via(&m.via)),
        format!("  context:    {} (max output {})", ctx(m.context), ctx(m.max_output)),
        format!(
            "  features:   tools {} · reasoning {} · vision {} · open weights {}",
            yes(m.tool_call),
            yes(m.reasoning),
            yes(m.vision),
            yes(m.open_weights)
        ),
        format!("  released:   {}   knowledge: {}", or_dash(&m.release), or_dash(&m.knowledge)),
        format!("  pages:      {}", m.links().into_iter().map(|(_, u)| u).collect::<Vec<_>>().join("  ")),
        format!("  note:       {}", store.note(&m.key).unwrap_or("-")),
        format!(
            "  favorite:   {}",
            Some(store.favorite_for(&m.key).join(", ")).filter(|s| !s.is_empty()).unwrap_or("-".into())
        ),
        String::new(),
        format!("  {} {}", crate::data::source().index().0, score(m.eci)),
        "  task fit (percentile vs all evaluated models):".into(),
    ];
    if m.tps.is_some() || m.ttft.is_some() {
        let n = |v: Option<f64>, f: fn(f64) -> String| v.map_or("-".into(), f);
        let speed = format!(
            "  speed:      {} tokens/s, first token in {} (median across providers)",
            n(m.tps, |v| format!("{v:.0}")),
            n(m.ttft, |v| format!("{v:.1}s"))
        );
        // Under context: what the model costs in time, next to what it holds.
        v.insert(4, speed);
    }
    for t in TASKS {
        if let Some(s) = fit::fit(m, t) {
            v.push(format!("    {:<13}{:>4.0}  {}", t.name, s, t.about));
        }
    }
    if !m.scores.is_empty() {
        v.push(String::new());
        v.push(format!("  benchmarks ({}, best effort setting):", crate::data::source().label()));
        for (b, s) in &m.scores {
            v.push(format!("    {:<36}{:>5.1}%", b, s * 100.0));
        }
    }
    v.push(String::new());
    v.push("  providers (model id, $ per 1M tokens in / cached in / out):".into());
    let mut offers: Vec<&Offer> = m.offers.iter().collect();
    // Available first, then cheapest; unknown price ("-") last.
    let cost = |o: &Offer| if o.unpriced { f64::MAX } else { o.blended() };
    offers.sort_by(|a, b| b.available.cmp(&a.available).then(cost(a).total_cmp(&cost(b))));
    for o in offers {
        v.push(format!(
            "    {} {:<20}{:<34}{:>8} {:>8} {:>8}  {}",
            if o.available { "●" } else { " " },
            truncate(&o.provider_name, 19),
            truncate(&o.id, 33),
            if o.unpriced { "-".into() } else { money(o.input) },
            o.cache_read.map_or("-".into(), money),
            if o.unpriced { "-".into() } else { money(o.output) },
            o.via.join(", ")
        ));
    }
    v
}

pub struct Row {
    pub label: String,
    pub cells: Vec<String>,
    /// Index of the best cell, if the row is comparable.
    pub best: Option<usize>,
    /// The topic the row belongs to, named in a rule above its first row; `""` for the
    /// model, price and context rows at the top.
    pub section: &'static str,
}

/// Rows for side-by-side comparison.
pub fn compare_rows(models: &[&Model]) -> Vec<Row> {
    fn row(label: &str, vals: Vec<Option<f64>>, fmt: impl Fn(f64) -> String, higher: bool) -> Row {
        let best = vals
            .iter()
            .enumerate()
            .filter_map(|(i, v)| Some((i, (*v)?)))
            .max_by(|a, b| if higher { a.1.total_cmp(&b.1) } else { b.1.total_cmp(&a.1) })
            .map(|(i, _)| i)
            .filter(|_| vals.iter().flatten().count() > 1);
        let cells = vals.into_iter().map(|v| v.map_or("-".into(), &fmt)).collect();
        Row { label: label.into(), cells, best, section: "" }
    }
    let price = |f: fn(&Offer) -> f64| -> Vec<Option<f64>> { models.iter().map(|m| m.priced_offer().map(f)).collect() };
    let mut rows = vec![
        Row { label: "model".into(), cells: models.iter().map(|m| m.name.clone()).collect(), best: None, section: "" },
        Row { label: "via".into(), cells: models.iter().map(|m| via(&m.via)).collect(), best: None, section: "" },
        row("$ in / 1M", price(|o| o.input), money, false),
        // No cache discount: cached input costs full price.
        row("$ cached in / 1M", price(|o| o.cache_read.unwrap_or(o.input)), money, false),
        row("$ out / 1M", price(|o| o.output), money, false),
        row("context", models.iter().map(|m| Some(m.context as f64)).collect(), |c| ctx(c as u64), true),
        Row {
            section: "scores",
            ..row(crate::data::source().index().0, models.iter().map(|m| m.eci).collect(), |v| format!("{v:.1}"), true)
        },
    ];
    // Under context, before the scores' rule: the ECI row is the last so far.
    if models.iter().any(|m| m.tps.is_some() || m.ttft.is_some()) {
        let at = rows.len() - 1;
        rows.insert(at, row("first token", models.iter().map(|m| m.ttft).collect(), |v| format!("{v:.1}s"), false));
        rows.insert(at, row("tokens/s", models.iter().map(|m| m.tps).collect(), |v| format!("{v:.0}"), true));
    }
    for t in TASKS.iter().filter(|t| t.name != "overall") {
        let vals: Vec<Option<f64>> = models.iter().map(|m| fit::fit(m, t)).collect();
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
    vec![
        best(models, "cheaper", Model::cost, false, usd),
        best(models, "better at coding", coding, true, |v| format!("{v:.0}")),
        best(models, "coding per $", |m| Some(coding(m)? / m.blended()?), true, |v| format!("{v:.1}")),
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
    fn tiers_pick_the_cheapest_good_enough() {
        let front = [("free", 30.0), ("mini", 60.0), ("sonnet", 74.8), ("opus", 90.0)];
        let key = |t| pick(&front, t).map(|e| e.0);
        assert_eq!(key("low"), Some("mini"));
        assert_eq!(key("mid"), Some("sonnet"), "74.8 shows as 75");
        assert_eq!(key("high"), Some("opus"));
        assert_eq!(pick(&front[..2], "mid").map(|e| e.0), Some("mini"), "none reaches it: the best");
        assert_eq!(pick::<&str>(&[], "low"), None);
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
        let rows = compare_rows(&[&a, &b]);
        let find = |l: &str| rows.iter().find(|r| r.label == l).unwrap();
        assert_eq!(find("context").best, Some(1));
        assert_eq!(find("ECI").best, None, "a single value is not a comparison");
        assert_eq!(find("ECI").cells, vec!["150.0", "-"]);
        assert_eq!((find("context").section, find("ECI").section), ("", "scores"));
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
        let (big, small, new) = (mk("big", Some(90.0), 10.0), mk("small", Some(60.0), 2.0), mk("new", None, 1.0));
        let v = verdict(&[&big, &small, &new]);
        let s = |r: [&str; 3]| r.map(String::from);
        assert_eq!(v[0], s(["cheaper", "new", "$1.0 vs $2.0"]));
        assert_eq!(v[1], s(["better at coding", "big", "90 vs 60"]), "a model without the score is skipped");
        assert_eq!(v[2], s(["coding per $", "small", "30.0 vs 9.0"]));
        assert_eq!(verdict(&[&big, &new])[1], s(["better at coding", "-", "not enough data"]));
        assert_eq!(verdict(&[&small, &small])[0], s(["cheaper", "tie: small, small", "$2.0 each"]));
        let twin = mk("twin", Some(60.0), 2.0);
        assert_eq!(verdict(&[&big, &small, &twin])[0], s(["cheaper", "tie: small, twin", "$2.0 each, next $10"]));
    }
}
