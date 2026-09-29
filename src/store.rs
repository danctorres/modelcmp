//! Marks, exclusions, notes, per-task favorites and the theme, keyed by model key. ~/.config/modelcmp/user.json

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Serialize, Deserialize, Default, Debug, PartialEq)]
#[serde(default)]
pub struct Store {
    #[serde(skip)]
    path: PathBuf,
    /// The file's mtime when this process last read or wrote it, to spot another writer.
    #[serde(skip)]
    mtime: Option<SystemTime>,
    /// Pins from older files, read once and loaded as marks.
    #[serde(alias = "favorites", skip_serializing)]
    pinned: BTreeSet<String>,
    /// Marks (space in the TUI); closing the TUI clears them.
    pub marked: Vec<String>,
    /// Models you have but cannot use; recommendations skip them.
    pub excluded: BTreeSet<String>,
    pub notes: BTreeMap<String, String>,
    /// `slot` -> your favorite model for it: the task's, or one `--tier`'s of it, which `--tier`
    /// picks over the task's and both over the computed one.
    #[serde(alias = "preferred")]
    pub favorite: BTreeMap<String, String>,
    /// A `view::THEMES` name, picked with `t`; empty is the terminal's colours.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub theme: String,
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

fn mtime(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

pub fn path() -> PathBuf {
    dirs::config_dir().unwrap_or_else(std::env::temp_dir).join("modelcmp").join("user.json")
}

/// Write through a per-process temp file and rename, so neither a crash nor a concurrent
/// writer (TUI and an agent script at once) leaves a half-written file behind.
pub fn write_atomic(p: &Path, bytes: &[u8]) -> std::io::Result<()> {
    std::fs::create_dir_all(p.parent().unwrap_or(Path::new(".")))?;
    let tmp = p.with_extension(format!("{}.tmp", std::process::id()));
    std::fs::write(&tmp, bytes)?;
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
            Err(_) => Store::default(),
            Ok(bytes) => serde_json::from_slice::<Store>(&bytes).unwrap_or_else(|e| {
                let bad = path.with_extension("json.bad");
                eprintln!("warning: {} is not valid ({e}); moved to {}", path.display(), bad.display());
                let _ = std::fs::rename(&path, &bad);
                Store::default()
            }),
        };
        for k in std::mem::take(&mut s.pinned) {
            if !s.marked.contains(&k) {
                s.marked.push(k);
            }
        }
        // Favorites of removed tasks (long-context) could not be cleared: the CLI rejects the name.
        s.favorite.retain(|k, _| crate::fit::task(k.split(':').next().unwrap_or(k)).is_some());
        s.mtime = mtime(&path);
        s.path = path;
        s
    }

    pub fn save(&mut self) -> std::io::Result<()> {
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
        let parses = std::fs::read(&self.path).map_or(true, |b| serde_json::from_slice::<Store>(&b).is_ok());
        if parses {
            *self = Store::load_from(std::mem::take(&mut self.path));
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
        for k in slots().filter(|s| s.0 == task).filter_map(|(t, x)| self.favorite(&slot(t, x))) {
            if !v.contains(&k) {
                v.push(k);
            }
        }
        v
    }

    /// The slots the model is the favorite for, in `slots` order, as its ★s are drawn.
    pub fn favorite_for(&self, key: &str) -> Vec<String> {
        slots().map(|(t, x)| slot(t, x)).filter(|s| self.favorite(s) == Some(key)).collect()
    }

    /// Whether the model is a favorite of the task or its tiers, or with no task of any.
    pub fn is_favorite(&self, task: Option<&crate::fit::Task>, key: &str) -> bool {
        match task {
            Some(t) => self.task_favorites(t.name).contains(&key),
            None => !self.favorite_for(key).is_empty(),
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
        let text = text.trim();
        if text.is_empty() {
            self.notes.remove(key);
        } else {
            self.notes.insert(key.to_string(), text.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("modelcmp-test-{}-{name}", std::process::id())).join("user.json")
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
        std::fs::write(&p, br#"{"preferred":{"coding":"old","long-context":"x"},"favorites":["old"]}"#).unwrap();
        let old = Store::load_from(p.clone());
        assert!(old.favorite("coding") == Some("old") && old.is_marked("old"));
        assert_eq!(old.favorite("long-context"), None, "a removed task's favorite is dropped");
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
        tui.toggle_marked("a");
        tui.save().unwrap();
        assert!(!tui.reload_if_changed(), "its own save is not a change");
        // An agent marks another model; the file's mtime moves on.
        std::thread::sleep(std::time::Duration::from_millis(20));
        let mut agent = Store::load_from(p.clone());
        agent.toggle_marked("b");
        agent.save().unwrap();
        assert!(tui.reload_if_changed());
        assert_eq!(tui.marked, ["a", "b"]);
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
        std::fs::remove_dir_all(p.parent().unwrap()).unwrap();
    }
}
