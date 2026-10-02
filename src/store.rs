//! Marks, exclusions, notes, per-task favorites, the theme and the benchmark source, keyed by model key. ~/.config/modelcmp/user.json

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
    /// Marks (space in the TUI); closing the TUI clears them.
    pub marked: Vec<String>,
    /// Models you have but cannot use; recommendations skip them.
    pub excluded: BTreeSet<String>,
    pub notes: BTreeMap<String, String>,
    /// `slot` -> your favorite model for it: the task's, or one `--tier`'s of it, which `--tier`
    /// picks over the task's and both over the computed one. A key that is no built-in task
    /// is a task of your own (`custom_tasks`).
    #[serde(alias = "preferred")]
    pub favorite: BTreeMap<String, String>,
    /// What each task of your own is about, in your words, by its name: agents choose it by that.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub about: BTreeMap<String, String>,
    /// A `view::THEMES` name, picked with `t`; empty is the terminal's colours.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub theme: String,
    /// A `data::Source` id, picked with `B`; empty is Epoch AI, never picked: the TUI asks.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub source: String,
    /// The version the TUI last opened as: a different one plays the intro again.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub seen: String,
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
    std::io::Write::write_all(&mut opts.open(&tmp)?, bytes)?;
    std::fs::rename(tmp, p)
}

impl Store {
    pub fn load() -> Self {
        Self::load_from(path())
    }

    /// A file that exists but does not parse is moved aside to `user.json.bad` and reported,
    /// instead of being silently replaced by the next save.
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
                let _ = std::fs::rename(&path, &bad);
                let warning = Some(format!("{} is not valid ({e}); moved to {}", path.display(), bad.display()));
                Store { warning, ..Store::default() }
            }),
        };
        for k in std::mem::take(&mut s.pinned) {
            if !s.marked.contains(&k) {
                s.marked.push(k);
            }
        }
        // A slot of no tier is a task of your own with no model to show, and could not be cleared.
        let tier = |x: &str| crate::view::TIERS.iter().any(|t| t.0 == x);
        s.favorite.retain(|k, _| k.split_once(':').is_none_or(|(_, x)| tier(x)));
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
        write_atomic(&self.path, serde_json::to_string_pretty(self)?.as_bytes())?;
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

    /// What `M` shows and `C` compares.
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
            self.favorite.remove(slot);
        } else {
            self.favorite.insert(slot.to_string(), key.to_string());
        }
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
        std::fs::remove_dir_all(p.parent().unwrap()).unwrap();
    }
}
