//! Marks, exclusions, notes, per-task favorites, the theme, the columns left out and your keys, keyed by model key. ~/.config/modelcmp/user.json

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// A bound typed with `>` (`min`) or `<` (`max`) in the TUI, on the column `--min` and `--max`
/// call `col`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Bound {
    pub col: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
}

#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Store {
    #[serde(skip)]
    path: PathBuf,
    /// The file's stamp when this process last read or wrote it, to spot another writer.
    #[serde(skip)]
    mtime: Option<Stamp>,
    /// Why the file could not be read, when it exists: saving over it would lose it.
    #[serde(skip)]
    unreadable: Option<String>,
    /// What went wrong reading the file, for the caller to show: the TUI's screen hides stderr.
    #[serde(skip)]
    pub warning: Option<String>,
    /// Pins from older files, read once and loaded as marks.
    #[serde(alias = "favorites", skip_serializing)]
    pinned: BTreeSet<String>,
    /// Marks (space in the TUI), kept until deselected.
    pub marked: Vec<String>,
    /// Models you have but cannot use; recommendations skip them.
    pub excluded: BTreeSet<String>,
    /// Developers and countries left out with `e` in the TUI's Dev dropdown: their models are
    /// excluded too.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub dev_not: Vec<String>,
    /// Harnesses left out with `e` in the Via dropdown: a model only they have is excluded too.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub via_not: Vec<String>,
    /// The price level (`view::LEVELS`) left out with `e` in the Price dropdown: models at or
    /// under it are excluded too.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub price_not: Option<usize>,
    /// Developers and countries picked in the TUI's Dev dropdown: the models of the others are
    /// excluded, unless `--dev` is given.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub dev_pick: Vec<String>,
    /// Entries picked in the Via dropdown, as `dev_pick`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub via_pick: Vec<String>,
    /// The price level (`view::LEVELS`) picked in the Price dropdown, as `dev_pick`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub price_pick: Option<usize>,
    /// The bounds typed with `>` and `<` in the TUI, as `dev_pick`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub bounds: Vec<Bound>,
    /// Column id -> the benchmark picked in its dropdown in the TUI, which the column shows and
    /// ranks a task's line by, in the TUI as in the CLI.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub bench: BTreeMap<String, String>,
    pub notes: BTreeMap<String, String>,
    /// `slot` -> your favorite model for it, one `--tier`'s of a task, which `--tier` picks
    /// over the computed one. A task in a key that is no built-in task is a task of your own
    /// (`custom_tasks`).
    #[serde(alias = "preferred")]
    pub favorite: BTreeMap<String, String>,
    /// Model key -> the harness you run it on (`H`, `modelcmp harness <harness> <model>`):
    /// `--cmd` and `--id` go by it when none is asked for.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub runs_on: BTreeMap<String, String>,
    /// The harness `--cmd` and `--id` go by when none is asked for and it has the model
    /// (`modelcmp harness`); a model's own comes first.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub harness: String,
    /// The folder of the `.gguf` files you downloaded (`modelcmp models-dir`), read when
    /// llama.cpp's own `LLAMA_ARG_MODELS_DIR` is not set.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub models_dir: String,
    /// The memory a model runs in on this machine, in GB (`modelcmp memory`), in place of the
    /// one found (`data::memory`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory: Option<f64>,
    /// What each task of your own is about, in your words, by its name: agents choose it by that.
    /// One with no model yet is a task by it alone.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub about: BTreeMap<String, String>,
    /// A `view::THEMES` name, picked with `t`; empty is the terminal's colours.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub theme: String,
    /// The columns left out of the TUI's table with `|`, by `app::col_id`.
    #[serde(skip_serializing_if = "BTreeSet::is_empty")]
    pub hide: BTreeSet<String>,
    /// The keys you bound in the TUI's `?`: each key the TUI has by default -> the
    /// one you gave its action. Only the changed ones.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub keys: BTreeMap<char, char>,
    /// Your price caps of tiers, in blended $/1M (`modelcmp tier`), over `view::TIERS`': only
    /// `low`'s and `mid`'s, as `free` takes none that costs and `high` any.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub tiers: BTreeMap<String, f64>,
    /// The version the TUI last opened as: a different one plays the intro again.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub seen: String,
    /// What a newer modelcmp wrote and this one does not know, kept for it.
    #[serde(flatten)]
    extra: BTreeMap<String, serde_json::Value>,
}

/// The `favorite` key of one `--tier` of a task: `coding:low`.
pub fn slot(task: &str, tier: &str) -> String {
    format!("{task}:{tier}")
}

/// The slots of one task: one per tier.
pub fn task_slots(task: &str) -> impl Iterator<Item = String> {
    crate::view::TIERS.iter().map(move |x| slot(task, x.0))
}

/// A task's name as `fav` and `--task` take it: lowercase with `-` for spaces, so that it needs
/// no quoting. One that is no built-in task is a task of your own (`Store::custom_tasks`).
pub fn task_name(s: &str) -> Result<String, String> {
    let name = s.split_whitespace().collect::<Vec<_>>().join("-").to_lowercase();
    if name.is_empty() {
        Err("a task needs a name".into())
    } else if name.contains(':') {
        Err("a task's name cannot contain ':', which separates the task from the tier".into())
    } else {
        Ok(name)
    }
}

/// mtime and inode: each save renames a new file in, so the inode tells two saves apart
/// within one tick of the filesystem's coarse clock.
type Stamp = (SystemTime, u64);

fn mtime(p: &Path) -> Option<Stamp> {
    let m = std::fs::metadata(p).ok()?;
    #[cfg(unix)]
    let ino = std::os::unix::fs::MetadataExt::ino(&m);
    #[cfg(not(unix))]
    let ino = 0;
    Some((m.modified().ok()?, ino))
}

/// Hold it from reading the file to saving it, so two writers (parallel `modelcmp select`s,
/// the TUI and an agent) cannot both change the same old copy and drop one's change.
pub fn lock(p: &Path) -> std::io::Result<std::fs::File> {
    std::fs::create_dir_all(p.parent().unwrap_or(Path::new(".")))?;
    let f = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(p.with_extension("lock"))?;
    f.lock()?;
    Ok(f)
}

pub fn path() -> PathBuf {
    dirs::config_dir().unwrap_or_else(std::env::temp_dir).join("modelcmp").join("user.json")
}

/// Write through a per-process temp file and rename, so neither a crash nor a concurrent
/// writer (TUI and an agent script at once) leaves a half-written file behind.
pub fn write_atomic(p: &Path, bytes: &[u8]) -> std::io::Result<()> {
    write_mode(p, bytes, 0o666)
}

/// `write_atomic`, readable by you alone: for a secret.
pub fn write_private(p: &Path, bytes: &[u8]) -> std::io::Result<()> {
    write_mode(p, bytes, 0o600)
}

fn write_mode(p: &Path, bytes: &[u8], mode: u32) -> std::io::Result<()> {
    std::fs::create_dir_all(p.parent().unwrap_or(Path::new(".")))?;
    // Per call too: two refreshes in the TUI can write one cache at once.
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = p.with_extension(format!("{}.{n}.tmp", std::process::id()));
    // `mode` applies only to a new file, so not to one a crash left behind.
    let _ = std::fs::remove_file(&tmp);
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, mode);
    #[cfg(not(unix))]
    let _ = mode;
    let mut f = opts.open(&tmp)?;
    std::io::Write::write_all(&mut f, bytes)?;
    // On disk before it takes the name: a power cut must not leave an empty file under it.
    f.sync_all()?;
    // A file you made private stays so.
    if let Ok(m) = std::fs::metadata(p) {
        let _ = std::fs::set_permissions(&tmp, m.permissions());
    }
    std::fs::rename(tmp, p)
}

impl Store {
    pub fn load() -> Self {
        Self::load_from(path())
    }

    /// Pins from an older file become marks, once each.
    fn pins_to_marks(&mut self) {
        for k in std::mem::take(&mut self.pinned) {
            if !self.marked.contains(&k) {
                self.marked.push(k);
            }
        }
    }

    /// A file that exists but does not parse is moved aside to `user.json.bad` and reported,
    /// instead of being silently replaced by the next save. One whose fields mostly read keeps
    /// those, with a copy of the file as it was left aside the same way.
    pub fn load_from(path: PathBuf) -> Self {
        let mut s = match std::fs::read(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Store::default(),
            Err(e) => Store {
                warning: Some(format!("cannot read {} ({e}), so changes will not be saved", path.display())),
                unreadable: Some(e.to_string()),
                ..Store::default()
            },
            Ok(bytes) => serde_json::from_slice::<Store>(&bytes).unwrap_or_else(|e| {
                let mut bad = path.with_extension("json.bad");
                // An earlier one may hold the last good favorites and notes: keep both.
                if bad.exists() {
                    let t = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs());
                    bad = path.with_extension(format!("json.{t}.bad"));
                }
                // One bad field must not take the favorites and exclusions with it.
                let fields = serde_json::from_slice::<serde_json::Map<String, serde_json::Value>>(&bytes).ok();
                let kept = fields.and_then(|mut f| {
                    let reads = |k: &String, v: &serde_json::Value| {
                        let one = serde_json::Map::from_iter([(k.clone(), v.clone())]);
                        serde_json::from_value::<Store>(one.into()).is_ok()
                    };
                    f.retain(|k, v| reads(k, v));
                    serde_json::from_value::<Store>(f.into()).ok()
                });
                let (name, to) = (path.display(), bad.display());
                match kept {
                    Some(mut s) => {
                        // Pins are not written back: they are marks before the file is.
                        s.pins_to_marks();
                        // With the copy aside, the file is written as kept: the next start
                        // would only warn and copy again.
                        let copied = std::fs::copy(&path, &bad).is_ok();
                        if copied && let Ok(json) = serde_json::to_string_pretty(&s) {
                            let _ = write_atomic(&path, json.as_bytes());
                        }
                        let aside =
                            if copied { "and a copy of the file is in" } else { "but no copy could be made in" };
                        let warning = Some(format!("{name} is partly not valid ({e}): the rest is kept, {aside} {to}"));
                        Store { warning, ..s }
                    }
                    None => {
                        let _ = std::fs::rename(&path, &bad);
                        Store {
                            warning: Some(format!("{name} is not valid ({e}), so it was moved to {to}")),
                            ..Store::default()
                        }
                    }
                }
            }),
        };
        s.pins_to_marks();
        // A slot of no tier, a task's own favorite of older versions, is no pick of a tier.
        let tier = |x: &str| crate::view::TIERS.iter().any(|t| t.0 == x);
        s.favorite.retain(|k, _| k.split_once(':').is_some_and(|(_, x)| tier(x)));
        // A cap left by a hand edit on a tier that takes none, or that is no price.
        s.tiers.retain(|t, c| crate::view::capped(t) && c.is_finite() && *c >= 0.0);
        // A price level left by a hand edit that is none would hide nothing, yet show as left out.
        s.price_not = s.price_not.filter(|&l| l < crate::view::LEVELS.len());
        s.price_pick = s.price_pick.filter(|&l| l < crate::view::LEVELS.len());
        // Keys left by a hand edit that are no action's, no key or two actions on one would put
        // an action out of reach: such an entry goes, the others stay, and the start says so.
        let actions = || crate::app::actions();
        let before = s.keys.clone();
        s.keys.retain(|&d, &mut k| d != k && crate::app::bindable_char(k) && actions().any(|a| a == d));
        // Of two actions on one key, the one there by default keeps it, else the first in `?`.
        // Dropping an entry puts its action back on its default key, which may clash in turn,
        // and each clash has an entry to drop, as one action alone has that key by default.
        loop {
            let mut on: BTreeMap<char, Vec<char>> = BTreeMap::new();
            for a in actions() {
                on.entry(s.keys.get(&a).copied().unwrap_or(a)).or_default().push(a);
            }
            let mut drop = vec![];
            for v in on.into_values().filter(|v| v.len() > 1) {
                let keep = v.iter().position(|a| !s.keys.contains_key(a)).unwrap_or(0);
                drop.extend(v.into_iter().enumerate().filter(|&(i, _)| i != keep).map(|(_, a)| a));
            }
            if drop.is_empty() {
                break;
            }
            for a in &drop {
                s.keys.remove(a);
            }
        }
        // Said, and written back as kept, so the next start does not say it again.
        if s.keys != before {
            let gone: Vec<String> =
                before.iter().filter(|(d, _)| !s.keys.contains_key(d)).map(|(d, k)| format!("{d}→{k}")).collect();
            let said = format!(
                "key bindings that name no action or share a key were dropped from {}: {}",
                path.display(),
                gone.join(" ")
            );
            s.warning = s.warning.or(Some(said));
            if let Ok(json) = serde_json::to_string_pretty(&s) {
                let _ = write_atomic(&path, json.as_bytes());
            }
        }
        s.mtime = mtime(&path);
        s.path = path;
        s
    }

    pub fn save(&mut self) -> std::io::Result<()> {
        if let Some(e) = &self.unreadable {
            let msg = format!("{} could not be read ({e}), so it is not overwritten", self.path.display());
            return Err(std::io::Error::other(msg));
        }
        // Through a link to the file it names: a dotfiles repo's copy, not a file in its place.
        let to = std::fs::canonicalize(&self.path).unwrap_or_else(|_| self.path.clone());
        write_atomic(&to, serde_json::to_string_pretty(self)?.as_bytes())
            .map_err(|e| std::io::Error::new(e.kind(), format!("cannot save {}: {e}", to.display())))?;
        self.mtime = mtime(&self.path);
        Ok(())
    }

    /// Read the file again if another process wrote it since this one last did (an agent's
    /// `modelcmp mark` while the TUI is open), so the TUI's next save does not undo that.
    /// A file that does not parse is left alone until the next start. True when reloaded.
    pub fn reload_if_changed(&mut self) -> bool {
        let now = mtime(&self.path);
        if now == self.mtime {
            return false;
        }
        let parses = match std::fs::read(&self.path) {
            Ok(b) => serde_json::from_slice::<Store>(&b).is_ok(),
            Err(e) => e.kind() == std::io::ErrorKind::NotFound,
        };
        if parses {
            // The start's warning is still to be said, unless the reload has one of its own.
            let warning = self.warning.take();
            *self = Store::load_from(std::mem::take(&mut self.path));
            self.warning = self.warning.take().or(warning);
        } else {
            self.mtime = now;
        }
        parses
    }

    /// The dearest `tier` takes, in blended $/1M: yours (`modelcmp tier`), else `view::TIERS`'.
    pub fn tier_cap(&self, tier: &str) -> f64 {
        let def = crate::view::TIERS.iter().find(|t| t.0 == tier).map_or(f64::INFINITY, |t| t.1);
        self.tiers.get(tier).copied().filter(|_| crate::view::capped(tier)).unwrap_or(def)
    }

    /// Set `tier`'s cap, `None` back to its default: not a price, or `low` dearer than `mid`, is refused.
    pub fn set_tier_cap(&mut self, tier: &str, cap: Option<f64>) -> Result<(), String> {
        let was = self.tiers.clone();
        match cap {
            Some(c) if !(c.is_finite() && c >= 0.0) => return Err(format!("{c} is not a valid price")),
            Some(c) => drop(self.tiers.insert(tier.into(), c)),
            None => drop(self.tiers.remove(tier)),
        }
        let (low, mid) = (self.tier_cap("low"), self.tier_cap("mid"));
        if low > mid {
            self.tiers = was;
            return Err(format!("low's cap (${low}) cannot be above mid's (${mid})"));
        }
        Ok(())
    }

    /// What `S` shows and `C` compares.
    pub fn is_marked(&self, key: &str) -> bool {
        self.marked.iter().any(|k| k == key)
    }

    pub fn toggle_marked(&mut self, key: &str) {
        match self.marked.iter().position(|k| k == key) {
            Some(i) => {
                self.marked.remove(i);
            }
            None => self.marked.push(key.to_string()),
        }
    }

    pub fn is_excluded(&self, key: &str) -> bool {
        self.excluded.contains(key)
    }

    pub fn toggle_excluded(&mut self, key: &str) {
        if !self.excluded.remove(key) {
            self.excluded.insert(key.to_string());
        }
    }

    pub fn note(&self, key: &str) -> Option<&str> {
        self.notes.get(key).map(String::as_str)
    }

    pub fn favorite(&self, task: &str) -> Option<&str> {
        self.favorite.get(task).map(String::as_str)
    }

    /// The harness you run the model on, when you chose one (`H`).
    pub fn runs_on(&self, key: &str) -> Option<&str> {
        self.runs_on.get(key).map(String::as_str)
    }

    /// Run the model on `harness`, or on none in particular with None.
    pub fn set_runs_on(&mut self, key: &str, harness: Option<&str>) {
        set_text(&mut self.runs_on, key, harness.unwrap_or(""));
    }

    /// What `--tier` tries before the computed pick: the tier's favorite.
    pub fn tier_favorite(&self, task: &str, tier: &str) -> Option<&str> {
        self.favorite(&slot(task, tier))
    }

    /// Your own tasks, by name: the favorites, of a tier of a task, and the abouts of no
    /// built-in task. One has no ranking, only the models you gave it, and is gone when they
    /// and what it is about are cleared.
    pub fn custom_tasks(&self) -> Vec<&str> {
        let favs = self.favorite.keys().filter_map(|k| Some(k.split_once(':')?.0));
        let mut v: Vec<&str> = favs.chain(self.about.keys().map(String::as_str)).collect();
        v.retain(|t| crate::fit::task(t).is_none());
        v.sort_unstable();
        v.dedup();
        v
    }

    /// Your own tasks agents see: the ones with a model, as one with none yet has nothing
    /// for them to run.
    pub fn tasks_with_models(&self) -> Vec<&str> {
        let mut v = self.custom_tasks();
        v.retain(|t| task_slots(t).any(|s| self.favorite.contains_key(&s)));
        v
    }

    /// Delete a task of your own: its models, their harnesses and what it is about.
    pub fn delete_task(&mut self, task: &str) {
        task_slots(task).for_each(|s| _ = self.clear_favorite(&s));
        self.about.remove(task);
    }

    /// Give a task of your own another name, which no task has yet; its models stay.
    pub fn rename_task(&mut self, old: &str, new: &str) -> Result<(), String> {
        if !self.custom_tasks().contains(&old) {
            return Err(format!("{old} is not a task of your own"));
        }
        if crate::fit::task(new).is_some() || self.custom_tasks().contains(&new) {
            return Err(format!("a task named {new} already exists"));
        }
        for (from, to) in task_slots(old).zip(task_slots(new)) {
            if let Some(model) = self.favorite.remove(&from) {
                self.favorite.insert(to, model);
            }
        }
        if let Some(about) = self.about.remove(old) {
            self.about.insert(new.to_string(), about);
        }
        Ok(())
    }

    /// Every favorite slot there is: the built-in tasks' in `TASKS` order, then those of your
    /// own tasks.
    pub fn all_slots(&self) -> impl Iterator<Item = String> {
        crate::fit::TASKS.iter().map(|t| t.name).chain(self.custom_tasks()).flat_map(task_slots)
    }

    /// The slots the model is the favorite for, in `all_slots` order, as its ★s are drawn.
    pub fn favorite_for(&self, key: &str) -> Vec<String> {
        // Most models are no favorite: no slot names to make for them.
        if !self.favorite.values().any(|k| k == key) {
            return vec![];
        }
        self.all_slots().filter(|s| self.favorite(s) == Some(key)).collect()
    }

    /// Whether the model is a favorite of the task's tiers, or with no task of any.
    pub fn is_favorite(&self, task: Option<&crate::fit::Task>, key: &str) -> bool {
        match task {
            // Asked per row and per frame, so without a list of them.
            Some(t) => self.favorite.iter().any(|(s, k)| k == key && s.split(':').next() == Some(t.name)),
            None => self.favorite.values().any(|k| k == key),
        }
    }

    /// Make `key` the favorite for the slot, or nothing when it already was: `f` toggles.
    pub fn toggle_favorite(&mut self, slot: &str, key: &str) {
        if self.favorite.get(slot).is_some_and(|k| k == key) {
            self.clear_favorite(slot);
        } else {
            self.set_favorite(slot, key);
        }
    }

    /// Make `key` the slot's favorite.
    pub fn set_favorite(&mut self, slot: &str, key: &str) {
        self.favorite.insert(slot.to_string(), key.to_string());
    }

    /// The slot left with no favorite; false when it had none.
    pub fn clear_favorite(&mut self, slot: &str) -> bool {
        self.favorite.remove(slot).is_some()
    }

    /// Empty text deletes the note.
    pub fn set_note(&mut self, key: &str, text: &str) {
        set_text(&mut self.notes, key, text);
    }

    /// What a task of your own is about, once you wrote it.
    pub fn about(&self, task: &str) -> Option<&str> {
        self.about.get(task).map(String::as_str)
    }

    /// Empty text deletes it.
    pub fn set_about(&mut self, task: &str, text: &str) {
        set_text(&mut self.about, task, text);
    }
}

/// `text` trimmed as the entry of `key`, or no entry when it is empty.
fn set_text(map: &mut BTreeMap<String, String>, key: &str, text: &str) {
    let text = text.trim();
    if text.is_empty() {
        map.remove(key);
    } else {
        map.insert(key.to_string(), text.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("modelcmp-test-{}-{name}", std::process::id())).join("user.json")
    }

    /// Keys that would put an action out of reach, left by a hand edit, are dropped.
    #[test]
    fn keys_that_are_none_or_clash_are_dropped() {
        let p = tmp("keys");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        // Each file's keys, and what is kept of them, a key and the one it is on in each pair.
        for (keys, kept) in [
            (r#"{"s": "o", "o": "s"}"#, "os so"),
            (r#"{"s": "z", "j": "3", "z": "s", "k": " "}"#, "sz"),
            (r#"{"s": "o"}"#, ""),
            (r#"{"s": "z", "o": "z"}"#, "sz"),
            (r#"{"s": "z", "o": "z", "c": "s", "j": "P"}"#, "cs jP sz"),
        ] {
            std::fs::write(&p, format!(r#"{{"keys": {keys}}}"#)).unwrap();
            let kept = kept.split_whitespace().map(|w| (w.as_bytes()[0] as char, w.as_bytes()[1] as char));
            let s = Store::load_from(p.clone());
            let all = kept.clone().count() == keys.matches(':').count();
            assert_eq!(s.warning.is_none(), all, "a drop is said: {keys}");
            assert_eq!(s.keys, BTreeMap::from_iter(kept), "{keys}");
            let again = Store::load_from(p.clone());
            assert!(again.warning.is_none() && again.keys == s.keys, "written back as kept: {keys}");
        }
        std::fs::remove_dir_all(p.parent().unwrap()).unwrap();
    }

    /// A price level that is none, left by a hand edit, is dropped: it would hide nothing.
    #[test]
    fn a_price_level_that_is_none_is_dropped() {
        let p = tmp("price-not");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        for (json, kept) in [(&br#"{"price_not": 7}"#[..], None), (br#"{"price_not": 2}"#, Some(2))] {
            std::fs::write(&p, json).unwrap();
            assert_eq!(Store::load_from(p.clone()).price_not, kept);
        }
        std::fs::remove_dir_all(p.parent().unwrap()).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_private_file_is_yours_alone() {
        use std::os::unix::fs::PermissionsExt;
        let p = tmp("private").with_file_name("key");
        write_private(&p, b"one").unwrap();
        write_private(&p, b"two").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"two");
        assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
    }

    #[test]
    fn round_trip() {
        let p = tmp("rt");
        let mut s = Store::load_from(p.clone());
        s.toggle_marked("gpt55");
        s.toggle_excluded("llama");
        s.set_note("gpt55", "  fast  ");
        s.set_note("x", "");
        s.toggle_favorite("coding:mid", "gpt55");
        s.toggle_favorite("agentic:mid", "gpt55");
        s.toggle_favorite("agentic:mid", "gpt55");
        s.seen = "0.1.0".into();
        s.save().unwrap();
        let back = Store::load_from(p.clone());
        assert_eq!(back, s);
        assert_eq!(back.note("gpt55"), Some("fast"));
        assert!(back.is_excluded("llama"));
        assert!(back.note("x").is_none());
        assert_eq!(back.favorite("coding:mid"), Some("gpt55"));
        assert_eq!(back.favorite_for("gpt55"), ["coding:mid"], "toggling twice clears it");
        s.toggle_favorite(&slot("coding", "low"), "flash");
        s.toggle_favorite(&slot("coding", "high"), "gpt55");
        assert_eq!((s.tier_favorite("coding", "low"), s.tier_favorite("coding", "free")), (Some("flash"), None));
        assert_eq!(s.favorite_for("gpt55"), ["coding:mid", "coding:high"]);
        s.set_runs_on("gpt55", Some("codex"));
        assert_eq!(s.runs_on("gpt55"), Some("codex"));
        s.set_runs_on("gpt55", None);
        assert!(s.runs_on.is_empty(), "none in particular");
        // Files written before the renames call favorites "preferred" and pins "favorites", and
        // older ones have a task's own favorite, of no tier, which no tier picks now.
        let file = br#"{"preferred":{"coding":"old","coding:low":"old","debugging":"old","debugging:fast":"x","long-context:low":"x"},"via":{"coding":"claude"},"favorites":["old"]}"#;
        std::fs::write(&p, file).unwrap();
        let old = Store::load_from(p.clone());
        assert!(old.favorite("coding").is_none() && old.runs_on.is_empty() && old.is_marked("old"));
        // A task that is not built in is your own.
        assert_eq!(old.custom_tasks(), ["long-context"]);
        assert_eq!(old.favorite_for("old"), ["coding:low"]);
        assert_eq!(
            (old.favorite_for("x"), old.favorite("debugging:fast")),
            (vec!["long-context:low".to_string()], None)
        );
        assert_eq!(task_name(" Tool  Dispatch ").as_deref(), Ok("tool-dispatch"), "typed without quotes");
        // A task of your own takes another name, one no task has; a built-in one keeps its own.
        let mut old = old;
        old.favorite.remove("long-context:low");
        old.toggle_favorite("debugging:low", "flash");
        old.toggle_favorite("debugging:high", "old");
        assert!(old.rename_task("coding", "code").is_err() && old.rename_task("debugging", "coding").is_err());
        old.set_about("debugging", " finding and fixing a bug ");
        assert_eq!(old.rename_task("debugging", "bug-hunt"), Ok(()));
        assert_eq!((old.custom_tasks(), old.favorite("bug-hunt:high")), (vec!["bug-hunt"], Some("old")));
        assert_eq!(old.tier_favorite("bug-hunt", "low"), Some("flash"), "its tiers go with it");
        old.toggle_favorite("bug-hunt:low", "flash");
        assert_eq!((old.about("bug-hunt"), old.about("debugging")), (Some("finding and fixing a bug"), None));
        // With no model it is a task by what it is about, and gone with that.
        old.toggle_favorite("bug-hunt:high", "old");
        old.save().unwrap();
        assert_eq!(Store::load_from(p.clone()).custom_tasks(), ["bug-hunt"]);
        old.set_about("bug-hunt", "");
        assert!(old.custom_tasks().is_empty());
        assert!(old.rename_task("debugging", "x").is_err(), "gone under its old name");
        assert!(task_name(" ").is_err() && task_name("a:b").is_err());
        // Pins were dropped: they load as marks, once each, and are not written back.
        std::fs::write(&p, br#"{"pinned":["a","c"],"marked":["a","b"]}"#).unwrap();
        let mut both = Store::load_from(p.clone());
        assert_eq!(both.marked, ["a", "b", "c"]);
        both.save().unwrap();
        assert!(!std::fs::read_to_string(&p).unwrap().contains("pinned"));
        std::fs::remove_dir_all(p.parent().unwrap()).unwrap();
    }

    /// A save keeps what it did not write: a newer version's field, a link to the file, its mode.
    #[cfg(unix)]
    #[test]
    fn a_save_keeps_the_link_the_mode_and_unknown_fields() {
        use std::os::unix::fs::PermissionsExt;
        let p = tmp("link");
        let real = p.with_file_name("dotfiles.json");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&real, r#"{"marked": ["a"], "newer": {"x": 1}}"#).unwrap();
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o600)).unwrap();
        let _ = std::fs::remove_file(&p);
        std::os::unix::fs::symlink(&real, &p).unwrap();
        let mut s = Store::load_from(p.clone());
        s.toggle_marked("b");
        s.save().unwrap();
        assert!(std::fs::symlink_metadata(&p).unwrap().is_symlink());
        let saved = std::fs::read_to_string(&real).unwrap();
        assert!(saved.contains("\"b\"") && saved.contains("newer"), "{saved}");
        assert_eq!(std::fs::metadata(&real).unwrap().permissions().mode() & 0o777, 0o600);
        std::fs::remove_dir_all(p.parent().unwrap()).unwrap();
    }

    #[test]
    fn another_writer_is_picked_up() {
        let p = tmp("reload");
        let mut tui = Store::load_from(p.clone());
        tui.warning = Some("said at the start".into());
        tui.toggle_marked("a");
        tui.save().unwrap();
        assert!(!tui.reload_if_changed(), "its own save is not a change");
        // An agent marks another model, in the same tick of the clock as likely as not.
        let mut agent = Store::load_from(p.clone());
        agent.toggle_marked("b");
        agent.save().unwrap();
        assert!(tui.reload_if_changed());
        assert_eq!(tui.marked, ["a", "b"]);
        assert_eq!(tui.warning.as_deref(), Some("said at the start"), "a warning not yet said is kept");
        std::fs::remove_dir_all(p.parent().unwrap()).unwrap();
    }

    #[test]
    fn an_unreadable_file_is_not_saved_over() {
        let p = tmp("unreadable");
        std::fs::create_dir_all(&p).unwrap();
        let mut s = Store::load_from(p.clone());
        s.toggle_marked("a");
        assert!(s.save().is_err());
        assert!(p.is_dir());
        std::fs::remove_dir_all(p.parent().unwrap()).unwrap();
    }

    #[test]
    fn corrupt_file_is_kept_aside() {
        let p = tmp("bad");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, b"{not json").unwrap();
        let s = Store::load_from(p.clone());
        assert!(s.marked.is_empty());
        assert!(!p.exists());
        assert_eq!(std::fs::read(p.with_extension("json.bad")).unwrap(), b"{not json");
        assert!(s.warning.is_some_and(|w| w.contains("moved to")));
        std::fs::write(&p, b"{again").unwrap();
        Store::load_from(p.clone());
        assert_eq!(std::fs::read(p.with_extension("json.bad")).unwrap(), b"{not json", "the first is kept");
        // One field of the wrong type: the others are kept, and the file stays with a copy aside.
        let partly = br#"{"favorite": {"coding:mid": "opus"}, "excluded": ["mini"], "favorites": ["old"], "theme": 7}"#;
        std::fs::write(&p, partly).unwrap();
        let s = Store::load_from(p.clone());
        assert_eq!(
            (s.favorite.get("coding:mid").map(String::as_str), s.excluded.len(), s.theme.as_str()),
            (Some("opus"), 1, "")
        );
        assert!(p.exists() && s.warning.is_some_and(|w| w.contains("the rest is kept")));
        let again = Store::load_from(p.clone());
        assert!(again.warning.is_none(), "written as kept, so said once");
        assert_eq!(again.marked, ["old"], "an older file's pins are written as marks, not dropped");
        std::fs::remove_dir_all(p.parent().unwrap()).unwrap();
    }
}
