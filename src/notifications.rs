//! 通知历史：收拢 Windows 通知镜像与岛内提醒，持久化到本机。
//!
//! 只保存应用名、正文、时间与是否已读，不含任何来源应用的数据或附件；
//! 与剪贴板一样使用原子写入，读取失败时回退为空列表。

use serde::{Deserialize, Serialize};

pub const LIMIT: usize = 60;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Notification {
    pub app: String,
    pub body: String,
    /// 本地时间戳（秒）。
    pub at: i64,
    /// 应用源的稳定标识（Windows AUMID）；用于按应用静音。
    #[serde(default)]
    pub source_id: String,
    /// 条目自身的唯一 id（应用 + 正文摘要），用于删除与去重。
    #[serde(default)]
    pub id: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Store {
    pub items: Vec<Notification>,
    /// 最近一次打开面板时最上面的那条；它之前的条目算未读。
    #[serde(default)]
    pub last_seen: String,
}

impl Store {
    pub fn load() -> Self {
        let mut store: Self = std::fs::read_to_string(path())
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        store.items.truncate(LIMIT);
        store
    }

    fn save(&self) {
        if let Ok(text) = serde_json::to_string(self) {
            let _ = crate::model::atomic_write(&path(), text.as_bytes());
        }
    }

    /// 记录一条通知；同一应用、同一正文的重复回调只保留一份，不同消息都会保留。
    pub fn push(&mut self, app: String, source_id: String, body: String) -> bool {
        let app = if app.trim().is_empty() { "Windows 通知".to_string() } else { app.trim().to_string() };
        let body = body.trim().to_string();
        if body.is_empty() {
            return false;
        }
        let source_id = source_id.trim().to_string();
        let id = format!("{}#{:08x}", if source_id.is_empty() { app.as_str() } else { source_id.as_str() }, fnv1a(body.as_bytes()));
        let now = chrono::Local::now().timestamp();
        self.items.retain(|item| item.id != id);
        self.items.insert(0, Notification { app, body, at: now, source_id, id });
        self.items.truncate(LIMIT);
        self.save();
        true
    }

    /// 岛内提醒（喝水、专注、计时等）也进入历史，方便回看。
    pub fn push_alert(&mut self, title: &str, body: &str) {
        let app = "岛仔".to_string();
        let body = body.trim().to_string();
        if body.is_empty() {
            return;
        }
        let now = chrono::Local::now().timestamp();
        let id = format!("{title}:{}", now / 60);
        self.items.retain(|item| !(item.id == id && item.app == app));
        self.items.insert(0, Notification { app, body: format!("{title} · {body}"), at: now, source_id: String::new(), id });
        self.items.truncate(LIMIT);
        self.save();
    }

    pub fn remove(&mut self, id: &str) {
        let before = self.items.len();
        self.items.retain(|item| item.id != id);
        if self.items.len() != before {
            self.save();
        }
    }

    pub fn clear(&mut self) {
        if self.items.is_empty() {
            return;
        }
        self.items.clear();
        self.save();
    }

    pub fn unread(&self) -> usize {
        if self.last_seen.is_empty() {
            return self.items.len();
        }
        self.items.iter().position(|item| item.id == self.last_seen).unwrap_or(self.items.len())
    }

    pub fn mark_seen(&mut self) {
        if let Some(top) = self.items.first() {
            if self.last_seen != top.id {
                self.last_seen = top.id.clone();
                self.save();
            }
        }
    }

    /// 面板按时段分组用：最近一条的序号。
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

fn fnv1a(bytes: &[u8]) -> u32 {
    let mut hash = 0x811c9dc5u32;
    for byte in bytes {
        hash ^= *byte as u32;
        hash = hash.wrapping_mul(0x01000193);
    }
    hash
}

fn path() -> std::path::PathBuf {
    crate::model::config_dir().join("notifications.json")
}

/// 通知时间的人性化标签：今天/昨天显示时分，更早显示日期。
pub fn time_label(at: i64, now: chrono::DateTime<chrono::Local>) -> String {
    let Some(time) = chrono::DateTime::from_timestamp(at, 0).map(|time| time.with_timezone(&chrono::Local)) else {
        return String::new();
    };
    if time.date_naive() == now.date_naive() {
        time.format("%H:%M").to_string()
    } else if Some(time.date_naive()) == now.date_naive().pred_opt() {
        format!("昨天 {}", time.format("%H:%M"))
    } else {
        time.format("%m-%d %H:%M").to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> Store {
        Store::default()
    }

    #[test]
    fn pushes_dedupes_and_caps_history() {
        let mut store = setup();
        assert!(store.push("微信".into(), "1".into(), "你好".into()));
        assert!(store.push("微信".into(), "2".into(), "在吗".into()));
        assert!(!store.push("微信".into(), "3".into(), "   ".into()));
        // 相同 id 只保留最新正文，且移到最前。
        // 同一应用的不同消息都要保留；完全相同的回调只保留一份。
        store.push("微信".into(), "1".into(), "你好呀".into());
        assert_eq!(store.items.len(), 3);
        assert_eq!(store.items[0].body, "你好呀");
        store.push("微信".into(), "1".into(), "你好呀".into());
        assert_eq!(store.items.len(), 3);
        for index in 0..(LIMIT + 10) {
            store.push("应用".into(), format!("id-{index}"), format!("第 {index} 条"));
        }
        assert_eq!(store.items.len(), LIMIT);
        assert!(store.items[0].body.contains("第 69 条"));
    }

    #[test]
    fn alert_push_remove_clear_and_unread() {
        let mut store = setup();
        store.push_alert("喝水提醒", "该喝水了");
        store.push_alert("计时结束", "时间到了");
        assert_eq!(store.items.len(), 2);
        let alert = store.items[1].id.clone();
        store.remove(&alert);
        assert_eq!(store.items.len(), 1);
        store.mark_seen();
        assert_eq!(store.unread(), 0);
        store.push("微信".into(), "new".into(), "新消息".into());
        assert_eq!(store.unread(), 1);
        store.clear();
        assert!(store.is_empty());
    }

    #[test]
    fn time_labels_cover_today_yesterday_and_older() {
        use chrono::TimeZone;
        let now = chrono::Local.with_ymd_and_hms(2026, 11, 10, 15, 0, 0).unwrap();
        let today = chrono::Local.with_ymd_and_hms(2026, 11, 10, 9, 30, 0).unwrap();
        let yesterday = chrono::Local.with_ymd_and_hms(2026, 11, 9, 21, 5, 0).unwrap();
        let older = chrono::Local.with_ymd_and_hms(2026, 11, 1, 8, 0, 0).unwrap();
        assert_eq!(time_label(today.timestamp(), now), "09:30");
        assert_eq!(time_label(yesterday.timestamp(), now), "昨天 21:05");
        assert_eq!(time_label(older.timestamp(), now), "11-01 08:00");
    }

    #[test]
    fn corrupted_file_falls_back_to_empty() {
        if let Some(dir) = path().parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        std::fs::write(path(), "{not json").unwrap();
        let store = Store::load();
        assert!(store.items.is_empty());
        std::fs::remove_file(path()).ok();
    }
}
