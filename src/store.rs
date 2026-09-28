//! Marks, exclusions, notes and per-task favorites, keyed by model key. ~/.config/modelcmp/user.json

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize, Default, Debug, PartialEq)]
#[serde(default)]
pub struct Store {
    #[serde(skip)]
    path: PathBuf,
    /// Pins from older files, read once and loaded as marks.
    #[serde(alias = "favorites", skip_serializing)]
    pinned: BTreeSet<String>,
    /// Marks (`m` in the TUI) outlive a session.
    pub marked: Vec<String>,
    /// Models you have but cannot use; recommendations skip them.
    pub excluded: BTreeSet<String>,
    pub notes: BTreeMap<String, String>,
    /// Task name -> your favorite model for it: `--tier` picks it over the computed one.
    #[serde(alias = "preferred")]
    pub favorite: BTreeMap<String, String>,
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
        s.path = path;
        s
    }

    pub fn save(&self) -> std::io::Result<()> {
        write_atomic(&self.path, serde_json::to_string_pretty(self)?.as_bytes())
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

    /// The tasks the model is the favorite for, in `TASKS` order, as its ★s are drawn.
    pub fn favorite_for(&self, key: &str) -> Vec<&'static str> {
        crate::fit::TASKS.iter().map(|t| t.name).filter(|t| self.favorite(t) == Some(key)).collect()
    }

    /// Make `key` the favorite for the task, or nothing when it already was: `f` toggles.
    pub fn toggle_favorite(&mut self, task: &str, key: &str) {
        if self.favorite.get(task).is_some_and(|k| k == key) {
            self.favorite.remove(task);
        } else {
            self.favorite.insert(task.to_string(), key.to_string());
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
        // Files written before the renames call favorites "preferred" and pins "favorites".
        std::fs::write(&p, br#"{"preferred":{"coding":"old"},"favorites":["old"]}"#).unwrap();
        let old = Store::load_from(p.clone());
        assert!(old.favorite("coding") == Some("old") && old.is_marked("old"));
        // Pins were dropped: they load as marks, once each, and are not written back.
        std::fs::write(&p, br#"{"pinned":["a","c"],"marked":["a","b"]}"#).unwrap();
        let both = Store::load_from(p.clone());
        assert_eq!(both.marked, ["a", "b", "c"]);
        both.save().unwrap();
        assert!(!std::fs::read_to_string(&p).unwrap().contains("pinned"));
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
