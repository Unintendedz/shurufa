//! 内联 Emoji 本地局部偏好表（MRU）：仅记录「中文锚定词 → 用户偏爱的 Emoji」，绝不写入主拼音词频或语言模型。
//!
//! 落盘为 `<user_data>/emoji_mru.json`（JSON 对象，上限 512 条）；磁盘不可写或文件损坏时静默退化为内存表，
//! 不影响正常组字与上屏。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

const MAX_MRU_ENTRIES: usize = 512;

#[derive(Default)]
pub(crate) struct EmojiMruStore {
    path: Option<PathBuf>,
    loaded: bool,
    entries: HashMap<String, String>,
}

impl EmojiMruStore {
    pub(crate) fn configure(&mut self, path: Option<PathBuf>) {
        if self.path != path {
            self.path = path;
            self.loaded = false;
            self.entries.clear();
        }
    }

    pub(crate) fn ensure_loaded(&mut self) {
        if self.loaded {
            return;
        }
        self.loaded = true;
        let Some(path) = self.path.as_deref() else {
            return;
        };
        let Ok(bytes) = std::fs::read(path) else {
            return;
        };
        if let Ok(parsed) = serde_json::from_slice::<HashMap<String, String>>(&bytes) {
            self.entries = parsed.into_iter().take(MAX_MRU_ENTRIES).collect();
        }
    }

    pub(crate) fn preferred(&self, anchor_word: &str) -> Option<&str> {
        self.entries.get(anchor_word).map(String::as_str)
    }

    pub(crate) fn record(&mut self, anchor_word: &str, emoji: &str) {
        if anchor_word.is_empty() || emoji.is_empty() {
            return;
        }
        self.ensure_loaded();
        if self.entries.get(anchor_word).map(String::as_str) == Some(emoji) {
            return;
        }
        if !self.entries.contains_key(anchor_word) && self.entries.len() >= MAX_MRU_ENTRIES {
            if let Some(evict) = self.entries.keys().next().cloned() {
                self.entries.remove(&evict);
            }
        }
        self.entries
            .insert(anchor_word.to_owned(), emoji.to_owned());
        if let Some(path) = self.path.as_deref() {
            let _ = persist_atomic(path, &self.entries);
        }
    }
}

fn persist_atomic(path: &Path, entries: &HashMap<String, String>) -> std::io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let bytes = serde_json::to_vec(entries).map_err(std::io::Error::other)?;
    let tmp = parent.join(".emoji_mru.json.tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mru_persists_and_reloads_anchor_preference() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("emoji_mru.json");
        let mut store = EmojiMruStore::default();
        store.configure(Some(path.clone()));
        assert_eq!(store.preferred("大笑"), None);
        store.record("大笑", "🤣");
        assert_eq!(store.preferred("大笑"), Some("🤣"));

        let mut reloaded = EmojiMruStore::default();
        reloaded.configure(Some(path));
        reloaded.ensure_loaded();
        assert_eq!(reloaded.preferred("大笑"), Some("🤣"));
    }
}
