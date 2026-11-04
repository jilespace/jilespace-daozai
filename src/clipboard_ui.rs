//! 剪贴板历史弹窗：独立视口、搜索与键盘选择。
use crate::clipboard::Item;
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    None,
    Copy(String),
    Delete(String),
    Pin(String, bool),
    Enable,
    Resume,
    Close { restore: bool },
}

pub struct Popup {
    pub open: bool,
    pub query: String,
    pub selected: usize,
    pub items: Vec<Item>,
    pub enabled: bool,
    pub paused: bool,
    pub hotkey_error: Option<String>,
    request_focus: bool,
    shown: bool,
    had_focus: bool,
    focus_lost_at: Option<Instant>,
}

impl Default for Popup {
    fn default() -> Self {
        Self {
            open: false,
            query: String::new(),
            selected: 0,
            items: Vec::new(),
            enabled: false,
            paused: false,
            hotkey_error: None,
            request_focus: false,
            shown: false,
            had_focus: false,
            focus_lost_at: None,
        }
    }
}

impl Popup {
    pub fn open_state(&mut self, enabled: bool, paused: bool, hotkey_error: Option<String>) {
        self.open = true;
        self.query.clear();
        self.selected = 0;
        self.enabled = enabled;
        self.paused = paused;
        self.hotkey_error = hotkey_error;
        self.request_focus = true;
        self.shown = false;
        self.had_focus = false;
        self.focus_lost_at = None;
    }

    pub fn close(&mut self) {
        self.open = false;
        self.shown = false;
    }

    pub fn sync(&mut self, enabled: bool, paused: bool, hotkey_error: Option<String>) {
        self.enabled = enabled;
        self.paused = paused;
        self.hotkey_error = hotkey_error;
    }

    pub fn set_items(&mut self, items: Vec<Item>) {
        self.items = items;
        self.clamp();
    }

    pub fn filtered(&self) -> Vec<usize> {
        let needle = self.query.trim().to_lowercase();
        self.items
            .iter()
            .enumerate()
            .filter(|(_, item)| needle.is_empty() || item.text.to_lowercase().contains(&needle))
            .map(|(index, _)| index)
            .collect()
    }

    pub fn clamp(&mut self) {
        let len = self.filtered().len();
        self.selected = self.selected.min(len.saturating_sub(1));
    }

    pub fn move_selection(&mut self, delta: isize) {
        let len = self.filtered().len();
        if len == 0 {
            self.selected = 0;
            return;
        }
        let next = self.selected as isize + delta;
        self.selected = next.clamp(0, len as isize - 1) as usize;
    }

    pub fn selected_id(&self) -> Option<String> {
        let indices = self.filtered();
        indices
            .get(self.selected)
            .map(|index| self.items[*index].id.clone())
    }

    pub fn observe_focus(&mut self, focused: bool, now: Instant) -> bool {
        if focused {
            self.had_focus = true;
            self.focus_lost_at = None;
            return false;
        }
        if !self.had_focus {
            return false;
        }
        let lost = *self.focus_lost_at.get_or_insert(now);
        now.duration_since(lost) >= Duration::from_millis(100)
    }
}

pub fn relative_time(now: i64, at: i64) -> String {
    let delta = now.saturating_sub(at);
    if delta < 60 {
        "刚刚".into()
    } else if delta < 3_600 {
        format!("{} 分钟前", delta / 60)
    } else if delta < 86_400 {
        format!("{} 小时前", delta / 3_600)
    } else if delta < 7 * 86_400 {
        format!("{} 天前", delta / 86_400)
    } else {
        chrono::DateTime::from_timestamp(at, 0)
            .map(|time| time.with_timezone(&chrono::Local).format("%m-%d").to_string())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, text: &str, at: i64) -> Item {
        Item { id: id.into(), text: text.into(), copied_at: at, pinned: false }
    }

    #[test]
    fn open_resets_query_and_selection() {
        let mut popup = Popup::default();
        popup.query = "old".into();
        popup.selected = 3;
        popup.open_state(true, false, None);
        assert!(popup.open);
        assert!(popup.query.is_empty());
        assert_eq!(popup.selected, 0);
    }

    #[test]
    fn filtering_and_selection_stay_in_range() {
        let mut popup = Popup::default();
        popup.open_state(true, false, None);
        popup.set_items(vec![
            item("a", "Alpha", 1),
            item("b", "Beta", 2),
            item("c", "alpha two", 3),
        ]);
        popup.query = "alpha".into();
        assert_eq!(popup.filtered().len(), 2);
        popup.move_selection(5);
        assert_eq!(popup.selected, 1);
        popup.move_selection(-5);
        assert_eq!(popup.selected, 0);
        assert_eq!(popup.selected_id().as_deref(), Some("a"));
    }

    #[test]
    fn focus_loss_needs_a_grace_period() {
        let mut popup = Popup::default();
        popup.open_state(true, false, None);
        let start = Instant::now();
        assert!(!popup.observe_focus(false, start));
        assert!(!popup.observe_focus(true, start + Duration::from_millis(10)));
        assert!(!popup.observe_focus(false, start + Duration::from_millis(20)));
        assert!(!popup.observe_focus(false, start + Duration::from_millis(80)));
        assert!(popup.observe_focus(false, start + Duration::from_millis(130)));
    }

    #[test]
    fn relative_time_uses_compact_labels() {
        assert_eq!(relative_time(1000, 1000), "刚刚");
        assert_eq!(relative_time(1000, 880), "2 分钟前");
        assert_eq!(relative_time(10_000, 2_800), "2 小时前");
        assert_eq!(relative_time(200_000, 100_000), "1 天前");
    }
}
