//! Marks, exclusions, notes, per-task favorites, the theme and the columns left out, keyed by model key. ~/.config/modelcmp/user.json

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Serialize, Deserialize, Default, Debug, PartialEq)]
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
    pub notes: BTreeMap<String, String>,
    /// `slot` -> your favorite model for it: the task's, or one `--tier`'s of it, which `--tier`
    /// picks over the task's and both over the computed one. A key that is no built-in task
    /// is a task of your own (`custom_tasks`).
    #[serde(alias = "preferred")]
    pub favorite: BTreeMap<String, String>,
    /// `slot` -> the harness you run its favorite on (`fav --via`, `v` in `f`'s grid): `--id`
    /// gives the id that one takes. Gone with the favorite.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub via: BTreeMap<String, String>,
    /// The harness `--cmd` and `--id` go by when none is asked for and it has the model
    /// (`modelcmp harness`); a favorite's own comes first.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub harness: String,
    /// The folder of the `.gguf` files you downloaded (`modelcmp models-dir`), read when
    /// llama.cpp's own `LLAMA_ARG_MODELS_DIR` is not set.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub models_dir: String,
    /// What each task of your own is about, in your words, by its name: agents choose it by that.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub about: BTreeMap<String, String>,
    /// A `view::THEMES` name, picked with `t`; empty is the terminal's colours.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub theme: String,
    /// The columns left out of the TUI's table with `|`, by `app::col_id`.
    #[serde(skip_serializing_if = "BTreeSet::is_empty")]
    pub hide: BTreeSet<String>,
    /// The version the TUI last opened as: a different one plays the intro again.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub seen: String,
    /// What a newer modelcmp wrote and this one does not know, kept for it.
    #[serde(flatten)]
    extra: BTreeMap<String, serde_json::Value>,
}

/// The `favorite` key of a task, or of one `--tier` of it: `coding`, `coding:low`.
pub fn slot(task: &str, tier: Option<&str>) -> String {
    tier.map_or_else(|| task.to_string(), |x| format!("{task}:{x}"))
}

/// Every favorite slot as (task, tier), in `TASKS` order, each task before its tiers.
pub fn slots() -> impl Iterator<Item = (&'static str, Option<&'static str>)> {
    crate::fit::TASKS.iter().flat_map(|t| {
        std::iter::once(None).chain(crate::view::TIERS.iter().map(|x| Some(x.0))).map(move |x| (t.name, x))
    })
}

/// The slots of one task: its own, then one per tier.
pub fn task_slots(task: &str) -> impl Iterator<Item = String> {
    std::iter::once(None).chain(crate::view::TIERS.iter().map(|x| Some(x.0))).map(move |x| slot(task, x))
}

/// A task's name as `fav` and `--task` take it: lowercase with `-` for spaces, so that it needs
/// no quoting. One that is no built-in task is a task of your own (`Store::custom_tasks`).
pub fn task_name(s: &str) -> Result<String, String> {
    let name = s.split_whitespace().collect::<Vec<_>>().join("-").to_lowercase();
    if name.is_empty() {
        Err("a task needs a name".into())
    } else if name.contains(':') {
        Err("a task's name takes no ':', which sets a tier apart".into())
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
                warning: Some(format!("cannot read {} ({e}); changes will not be saved", path.display())),
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
                        let aside = if copied { "a copy is in" } else { "no copy could be made in" };
                        let warning = Some(format!("{name} is partly not valid ({e}); the rest is kept, {aside} {to}"));
                        Store { warning, ..s }
                    }
                    None => {
                        let _ = std::fs::rename(&path, &bad);
                        Store { warning: Some(format!("{name} is not valid ({e}); moved to {to}")), ..Store::default() }
                    }
                }
            }),
        };
        s.pins_to_marks();
        // A slot of no tier is a task of your own with no model to show, and could not be cleared.
        let tier = |x: &str| crate::view::TIERS.iter().any(|t| t.0 == x);
        s.favorite.retain(|k, _| k.split_once(':').is_none_or(|(_, x)| tier(x)));
        // A harness is its favorite's: one left by a hand edit has no model to run.
        let Store { favorite, via, .. } = &mut s;
        via.retain(|k, _| favorite.contains_key(k));
        // A task gone with its models leaves what it was about, in case one comes back before
        // the file is closed; no longer than that.
        let tasks: Vec<String> = s.custom_tasks().into_iter().map(String::from).collect();
        s.about.retain(|t, _| tasks.contains(t));
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

    /// The harness you run the slot's favorite on, when you chose one.
    pub fn via(&self, slot: &str) -> Option<&str> {
        self.via.get(slot).map(String::as_str)
    }

    /// The harness of `key` as a favorite of the task: with a tier, the tier's when it is that
    /// one's favorite, else the task's, as `--tier` picks; with none, of the first slot it has.
    pub fn task_via(&self, task: &str, tier: Option<&str>, key: &str) -> Option<&str> {
        let slots: Vec<String> = match tier {
            Some(x) => vec![slot(task, Some(x)), slot(task, None)],
            None => task_slots(task).collect(),
        };
        slots.iter().find(|s| self.favorite(s) == Some(key)).and_then(|s| self.via(s))
    }

    /// What `--tier` tries before the computed pick, in order: the tier's favorite, then the task's.
    pub fn tier_favorites(&self, task: &str, tier: &str) -> impl Iterator<Item = &str> {
        [self.favorite(&slot(task, Some(tier))), self.favorite(task)].into_iter().flatten()
    }

    /// Every model that is a favorite of the task or of one of its tiers: all join its line.
    pub fn task_favorites(&self, task: &str) -> Vec<&str> {
        let mut v: Vec<&str> = Vec::new();
        for k in task_slots(task).filter_map(|s| self.favorite(&s)) {
            if !v.contains(&k) {
                v.push(k);
            }
        }
        v
    }

    /// Your own tasks, by name: the favorites, of the task or a tier of it, of no built-in
    /// task. One has no ranking, only the models you gave it, and is gone when they are cleared.
    pub fn custom_tasks(&self) -> Vec<&str> {
        let mut v: Vec<&str> = self.favorite.keys().map(|k| k.split(':').next().unwrap_or(k)).collect();
        v.retain(|t| crate::fit::task(t).is_none());
        v.sort_unstable();
        v.dedup();
        v
    }

    /// Give a task of your own another name, which no task has yet; its models stay.
    pub fn rename_task(&mut self, old: &str, new: &str) -> Result<(), String> {
        if !self.custom_tasks().contains(&old) {
            return Err(format!("{old} is not a task of your own"));
        }
        if crate::fit::task(new).is_some() || self.custom_tasks().contains(&new) {
            return Err(format!("there is a task {new} already"));
        }
        for (from, to) in task_slots(old).zip(task_slots(new)) {
            if let Some(via) = self.via.remove(&from) {
                self.via.insert(to.clone(), via);
            }
            if let Some(model) = self.favorite.remove(&from) {
                self.favorite.insert(to, model);
            }
        }
        if let Some(about) = self.about.remove(old) {
            self.about.insert(new.to_string(), about);
        }
        Ok(())
    }

    /// Every favorite slot there is: the built-in tasks' in `slots` order, then those of your
    /// own tasks, each task before its tiers.
    pub fn all_slots(&self) -> impl Iterator<Item = String> {
        slots().map(|(t, x)| slot(t, x)).chain(self.custom_tasks().into_iter().flat_map(task_slots))
    }

    /// The slots the model is the favorite for, in `all_slots` order, as its ★s are drawn.
    pub fn favorite_for(&self, key: &str) -> Vec<String> {
        self.all_slots().filter(|s| self.favorite(s) == Some(key)).collect()
    }

    /// Whether the model is a favorite of the task or its tiers, or with no task of any.
    pub fn is_favorite(&self, task: Option<&crate::fit::Task>, key: &str) -> bool {
        match task {
            // Asked per row and per frame, so without `task_favorites`' list.
            Some(t) => self.favorite.iter().any(|(s, k)| k == key && s.split(':').next() == Some(t.name)),
            None => self.favorite.values().any(|k| k == key),
        }
    }

    /// Make `key` the favorite for the slot, or nothing when it already was: `f` toggles.
    pub fn toggle_favorite(&mut self, slot: &str, key: &str) {
        if self.favorite.get(slot).is_some_and(|k| k == key) {
            self.clear_favorite(slot);
        } else {
            self.set_favorite(slot, key, None);
        }
    }

    /// Make `key` the slot's favorite, run on the harness `via` when one is given.
    pub fn set_favorite(&mut self, slot: &str, key: &str, via: Option<&str>) {
        self.favorite.insert(slot.to_string(), key.to_string());
        set_text(&mut self.via, slot, via.unwrap_or(""));
    }

    /// The slot left with no favorite, nor its harness; false when it had none.
    pub fn clear_favorite(&mut self, slot: &str) -> bool {
        self.via.remove(slot);
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
        s.toggle_favorite("coding", "gpt55");
        s.toggle_favorite("agentic", "gpt55");
        s.toggle_favorite("agentic", "gpt55");
        s.seen = "0.1.0".into();
        s.save().unwrap();
        let back = Store::load_from(p.clone());
        assert_eq!(back, s);
        assert_eq!(back.note("gpt55"), Some("fast"));
        assert!(back.is_excluded("llama"));
        assert!(back.note("x").is_none());
        assert_eq!(back.favorite("coding"), Some("gpt55"));
        assert_eq!(back.favorite_for("gpt55"), ["coding"], "toggling twice clears it");
        s.toggle_favorite(&slot("coding", Some("low")), "flash");
        s.toggle_favorite(&slot("coding", Some("high")), "gpt55");
        assert_eq!(s.tier_favorites("coding", "low").collect::<Vec<_>>(), ["flash", "gpt55"], "the tier's first");
        assert_eq!(s.tier_favorites("coding", "mid").collect::<Vec<_>>(), ["gpt55"], "else the task's");
        assert_eq!(s.task_favorites("coding"), ["gpt55", "flash"], "each model once");
        assert_eq!(s.favorite_for("gpt55"), ["coding", "coding:high"]);
        s.set_favorite("coding", "gpt55", Some("codex"));
        assert_eq!(s.task_via("coding", Some("mid"), "gpt55"), Some("codex"), "the task's harness");
        assert_eq!(s.task_via("coding", Some("high"), "gpt55"), None, "the tier's favorite has none of its own");
        assert_eq!(s.task_via("coding", Some("low"), "gpt55"), Some("codex"), "the tier's hidden: the task's");
        assert_eq!(s.task_via("coding", None, "gpt55"), Some("codex"), "no tier: the slot it is the favorite of");
        s.toggle_favorite("coding", "flash");
        assert_eq!(s.via("coding"), None, "another model: the harness goes with the one it was for");
        // Files written before the renames call favorites "preferred" and pins "favorites".
        let file = br#"{"preferred":{"coding":"old","debugging":"old","debugging:fast":"x","long-context:low":"x"},"favorites":["old"]}"#;
        std::fs::write(&p, file).unwrap();
        let old = Store::load_from(p.clone());
        assert!(old.favorite("coding") == Some("old") && old.is_marked("old"));
        // A task that is not built in is your own, whether its model is the task's or a tier's.
        assert_eq!(old.custom_tasks(), ["debugging", "long-context"]);
        assert_eq!(old.favorite_for("old"), ["coding", "debugging"], "after the built-in ones");
        assert_eq!(
            (old.favorite_for("x"), old.favorite("debugging:fast")),
            (vec!["long-context:low".to_string()], None)
        );
        assert_eq!(task_name(" Tool  Dispatch ").as_deref(), Ok("tool-dispatch"), "typed without quotes");
        // A task of your own takes another name, one no task has; a built-in one keeps its own.
        let mut old = old;
        assert!(old.rename_task("coding", "code").is_err() && old.rename_task("debugging", "coding").is_err());
        old.set_about("debugging", " finding and fixing a bug ");
        old.favorite.remove("long-context:low");
        old.toggle_favorite("debugging:low", "flash");
        assert_eq!(old.rename_task("debugging", "bug-hunt"), Ok(()));
        assert_eq!((old.custom_tasks(), old.favorite("bug-hunt")), (vec!["bug-hunt"], Some("old")));
        assert_eq!(old.tier_favorites("bug-hunt", "low").collect::<Vec<_>>(), ["flash", "old"], "its tiers go with it");
        old.toggle_favorite("bug-hunt:low", "flash");
        assert_eq!((old.about("bug-hunt"), old.about("debugging")), (Some("finding and fixing a bug"), None));
        // What a task was about goes with it, the next time the file is read.
        old.toggle_favorite("bug-hunt", "old");
        old.save().unwrap();
        assert_eq!(Store::load_from(p.clone()).about("bug-hunt"), None);
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
        let partly = br#"{"favorite": {"coding": "opus"}, "excluded": ["mini"], "favorites": ["old"], "theme": 7}"#;
        std::fs::write(&p, partly).unwrap();
        let s = Store::load_from(p.clone());
        assert_eq!(
            (s.favorite.get("coding").map(String::as_str), s.excluded.len(), s.theme.as_str()),
            (Some("opus"), 1, "")
        );
        assert!(p.exists() && s.warning.is_some_and(|w| w.contains("the rest is kept")));
        let again = Store::load_from(p.clone());
        assert!(again.warning.is_none(), "written as kept, so said once");
        assert_eq!(again.marked, ["old"], "an older file's pins are written as marks, not dropped");
        std::fs::remove_dir_all(p.parent().unwrap()).unwrap();
    }
}
