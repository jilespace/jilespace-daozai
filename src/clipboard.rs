//! 剪贴板历史：纯数据逻辑与后台服务。

pub const MAX_ITEM_BYTES: usize = 32 * 1024;
pub const MAX_TOTAL_BYTES: usize = 1024 * 1024;
pub const DEFAULT_LIMIT: usize = 200;
pub const DEFAULT_DAYS: u32 = 7;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub id: String,
    pub text: String,
    pub copied_at: i64,
    pub pinned: bool,
}

#[derive(Clone, Debug)]
pub struct History {
    items: Vec<Item>,
    limit: usize,
    days: u32,
}

impl Default for History {
    fn default() -> Self {
        Self::new(DEFAULT_LIMIT, DEFAULT_DAYS)
    }
}

impl History {
    pub fn new(limit: usize, days: u32) -> Self {
        Self { items: Vec::new(), limit: limit.max(1), days: days.max(1) }
    }

    pub fn items(&self) -> &[Item] {
        &self.items
    }

    pub fn set_limits(&mut self, limit: usize, days: u32, now: i64) {
        self.limit = limit.max(1);
        self.days = days.max(1);
        self.cleanup(now);
    }

    pub fn insert(&mut self, text: String, now: i64) -> bool {
        if text.trim().is_empty() || text.len() > MAX_ITEM_BYTES {
            return false;
        }
        let id = id_for(&text);
        let mut item = match self.items.iter().position(|item| item.id == id) {
            Some(position) => self.items.remove(position),
            None => Item { id, text, copied_at: now, pinned: false },
        };
        item.copied_at = now;
        self.items.insert(0, item);
        self.cleanup(now);
        true
    }

    pub fn remove(&mut self, id: &str) {
        self.items.retain(|item| item.id != id);
    }

    pub fn set_pinned(&mut self, id: &str, pinned: bool) {
        if let Some(item) = self.items.iter_mut().find(|item| item.id == id) {
            item.pinned = pinned;
        }
        self.order();
    }

    pub fn clear(&mut self) {
        self.items.clear();
    }

    pub fn cleanup(&mut self, now: i64) {
        let cutoff = now - self.days as i64 * 86_400;
        self.items.retain(|item| item.pinned || item.copied_at >= cutoff);

        let mut unpinned = 0usize;
        self.items.retain(|item| {
            if item.pinned {
                true
            } else {
                unpinned += 1;
                unpinned <= self.limit
            }
        });

        let mut bytes = 0usize;
        self.items.retain(|item| {
            if item.pinned {
                true
            } else {
                bytes += item.text.len();
                bytes <= MAX_TOTAL_BYTES
            }
        });

        self.order();
    }

    fn order(&mut self) {
        self.items.sort_by(|a, b| {
            b.pinned.cmp(&a.pinned).then_with(|| b.copied_at.cmp(&a.copied_at))
        });
    }

    pub fn visible(&self, query: &str) -> Vec<&Item> {
        let needle = query.trim().to_lowercase();
        self.items
            .iter()
            .filter(|item| needle.is_empty() || item.text.to_lowercase().contains(&needle))
            .collect()
    }
}

pub fn id_for(text: &str) -> String {
    use std::fmt::Write;
    let digest = ring::digest::digest(&ring::digest::SHA256, text.as_bytes());
    let mut id = String::with_capacity(16);
    for byte in &digest.as_ref()[..8] {
        let _ = write!(id, "{byte:02x}");
    }
    id
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find<'a>(history: &'a History, text: &str) -> Option<&'a Item> {
        history.items().iter().find(|item| item.text == text)
    }

    #[test]
    fn newest_item_goes_to_top() {
        let mut history = History::new(10, 7);
        assert!(history.insert("first".into(), 1));
        assert!(history.insert("second".into(), 2));
        assert_eq!(history.items()[0].text, "second");
        assert_eq!(history.items()[1].text, "first");
    }

    #[test]
    fn duplicate_moves_to_top_without_duplicating() {
        let mut history = History::new(10, 7);
        history.insert("alpha".into(), 1);
        history.insert("beta".into(), 2);
        assert!(history.insert("alpha".into(), 3));
        assert_eq!(history.items().len(), 2);
        assert_eq!(history.items()[0].text, "alpha");
        assert_eq!(history.items()[0].copied_at, 3);
    }

    #[test]
    fn pinned_items_survive_count_and_expiry() {
        let mut history = History::new(2, 1);
        history.insert("pinned".into(), 1_000);
        let id = id_for("pinned");
        history.set_pinned(&id, true);
        history.insert("recent".into(), 200_000);
        history.insert("newest".into(), 200_001);
        history.cleanup(200_002);
        assert!(find(&history, "pinned").is_some());
    }

    #[test]
    fn count_limit_keeps_newest_unpinned() {
        let mut history = History::new(2, 7);
        for (index, text) in ["one", "two", "three", "four"].iter().enumerate() {
            history.insert((*text).into(), index as i64);
        }
        assert_eq!(history.items().len(), 2);
        assert_eq!(history.items()[0].text, "four");
        assert_eq!(history.items()[1].text, "three");
    }

    #[test]
    fn expired_unpinned_items_are_removed() {
        let mut history = History::new(10, 1);
        history.insert("old".into(), 0);
        history.insert("new".into(), 2 * 86_400);
        history.cleanup(2 * 86_400 + 1);
        assert!(find(&history, "old").is_none());
        assert!(find(&history, "new").is_some());
    }

    #[test]
    fn byte_limit_drops_oldest_unpinned() {
        let mut history = History::new(100, 7);
        let prefix = "x".repeat(MAX_ITEM_BYTES - 4);
        for index in 0..40 {
            assert!(history.insert(format!("{prefix}{index:04}"), index as i64));
        }
        let total: usize = history
            .items()
            .iter()
            .filter(|item| !item.pinned)
            .map(|item| item.text.len())
            .sum();
        assert!(total <= MAX_TOTAL_BYTES);
        assert!(history.items().len() < 40);
    }

    #[test]
    fn search_is_case_insensitive() {
        let mut history = History::new(10, 7);
        history.insert("Hello World".into(), 1);
        history.insert("另一个".into(), 2);
        assert_eq!(history.visible("hello").len(), 1);
        assert_eq!(history.visible("世界").len(), 0);
        assert_eq!(history.visible("").len(), 2);
    }

    #[test]
    fn empty_and_oversized_text_are_ignored() {
        let mut history = History::new(10, 7);
        assert!(!history.insert("   ".into(), 1));
        assert!(!history.insert("x".repeat(MAX_ITEM_BYTES + 1), 2));
        assert!(history.items().is_empty());
    }

    #[test]
    fn id_is_stable_for_same_text() {
        assert_eq!(id_for("same"), id_for("same"));
        assert_ne!(id_for("same"), id_for("different"));
    }
}
