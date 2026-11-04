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

    pub fn to_json(&self) -> String {
        let items: Vec<serde_json::Value> = self
            .items
            .iter()
            .map(|item| {
                serde_json::json!({
                    "id": item.id,
                    "text": item.text,
                    "copied_at": item.copied_at,
                    "pinned": item.pinned,
                })
            })
            .collect();
        serde_json::json!({ "version": 1, "items": items }).to_string()
    }

    pub fn from_json(raw: &str, limit: usize, days: u32, now: i64) -> Result<Self, String> {
        let value: serde_json::Value =
            serde_json::from_str(raw).map_err(|error| format!("JSON 解析失败：{error}"))?;
        if value["version"].as_u64() != Some(1) {
            return Err("不支持的剪贴板历史版本".into());
        }
        let items = value["items"]
            .as_array()
            .ok_or_else(|| "剪贴板历史缺少 items".to_string())?;
        let mut history = Self::new(limit, days);
        for item in items {
            let (Some(id), Some(text)) = (item["id"].as_str(), item["text"].as_str()) else {
                continue;
            };
            if text.trim().is_empty() || text.len() > MAX_ITEM_BYTES {
                continue;
            }
            history.items.push(Item {
                id: id.to_string(),
                text: text.to_string(),
                copied_at: item["copied_at"].as_i64().unwrap_or(0),
                pinned: item["pinned"].as_bool().unwrap_or(false),
            });
        }
        history.cleanup(now);
        Ok(history)
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

#[derive(Clone, Debug)]
pub struct RawClipboard {
    pub text: String,
    pub excluded: bool,
    pub own_process: bool,
}

pub fn accepts(raw: &RawClipboard) -> bool {
    !raw.excluded
        && !raw.own_process
        && !raw.text.trim().is_empty()
        && raw.text.len() <= MAX_ITEM_BYTES
}

#[cfg(windows)]
pub fn clipboard_sequence() -> u32 {
    unsafe { windows::Win32::System::DataExchange::GetClipboardSequenceNumber() }
}

#[cfg(windows)]
pub fn clipboard_owner_is_self() -> bool {
    use windows::Win32::{
        System::DataExchange::GetClipboardOwner,
        System::Threading::GetCurrentProcessId,
        UI::WindowsAndMessaging::GetWindowThreadProcessId,
    };
    unsafe {
        let Ok(owner) = GetClipboardOwner() else { return false };
        if owner.0.is_null() {
            return false;
        }
        let mut process = 0u32;
        let _ = GetWindowThreadProcessId(owner, Some(&mut process));
        process == GetCurrentProcessId()
    }
}

#[cfg(windows)]
pub fn read_clipboard() -> Option<String> {
    use windows::Win32::{
        Foundation::{HGLOBAL, HWND},
        System::{
            DataExchange::{CloseClipboard, GetClipboardData, OpenClipboard},
            Memory::{GlobalLock, GlobalUnlock},
            Ole::CF_UNICODETEXT,
        },
    };
    unsafe {
        if OpenClipboard(HWND::default()).is_err() {
            return None;
        }
        let result = (|| {
            if clipboard_excluded() {
                return None;
            }
            let handle = GetClipboardData(CF_UNICODETEXT.0 as u32).ok()?;
            let pointer = GlobalLock(HGLOBAL(handle.0)) as *const u16;
            if pointer.is_null() {
                return None;
            }
            let limit = MAX_ITEM_BYTES / 2;
            let mut length = 0usize;
            while length < limit && *pointer.add(length) != 0 {
                length += 1;
            }
            let text = String::from_utf16_lossy(std::slice::from_raw_parts(pointer, length));
            let _ = GlobalUnlock(HGLOBAL(handle.0));
            Some(text)
        })();
        let _ = CloseClipboard();
        result
    }
}

#[cfg(windows)]
unsafe fn clipboard_excluded() -> bool {
    use windows::core::HSTRING;
    use windows::Win32::{
        Foundation::HGLOBAL,
        System::{
            DataExchange::{
                GetClipboardData, IsClipboardFormatAvailable, RegisterClipboardFormatW,
            },
            Memory::{GlobalLock, GlobalUnlock},
        },
    };
    for name in [
        "ExcludeClipboardContentFromMonitorProcessing",
        "CanIncludeInClipboardHistory",
        "CanUploadToCloudClipboard",
    ] {
        let format = RegisterClipboardFormatW(&HSTRING::from(name));
        if format == 0 || IsClipboardFormatAvailable(format).is_err() {
            continue;
        }
        if name == "ExcludeClipboardContentFromMonitorProcessing" {
            return true;
        }
        if let Ok(handle) = GetClipboardData(format) {
            let pointer = GlobalLock(HGLOBAL(handle.0)) as *const u32;
            if !pointer.is_null() {
                let value = *pointer;
                let _ = GlobalUnlock(HGLOBAL(handle.0));
                if value == 0 {
                    return true;
                }
            }
        }
    }
    false
}

#[cfg(windows)]
pub fn write_clipboard(text: &str) -> bool {
    use windows::Win32::{
        Foundation::{HANDLE, HWND},
        System::{
            DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData},
            Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE},
            Ole::CF_UNICODETEXT,
        },
    };
    unsafe {
        if OpenClipboard(HWND::default()).is_err() {
            return false;
        }
        let ok = (|| {
            if EmptyClipboard().is_err() {
                return false;
            }
            let units: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
            let Ok(handle) = GlobalAlloc(GMEM_MOVEABLE, units.len() * 2) else {
                return false;
            };
            let pointer = GlobalLock(handle) as *mut u16;
            if pointer.is_null() {
                let _ = windows::Win32::Foundation::GlobalFree(handle);
                return false;
            }
            std::ptr::copy_nonoverlapping(units.as_ptr(), pointer, units.len());
            let _ = GlobalUnlock(handle);
            if SetClipboardData(CF_UNICODETEXT.0 as u32, HANDLE(handle.0)).is_err() {
                let _ = windows::Win32::Foundation::GlobalFree(handle);
                return false;
            }
            true
        })();
        let _ = CloseClipboard();
        ok
    }
}

#[cfg(not(windows))]
pub fn clipboard_sequence() -> u32 {
    0
}

#[cfg(not(windows))]
pub fn clipboard_owner_is_self() -> bool {
    false
}

#[cfg(not(windows))]
pub fn read_clipboard() -> Option<String> {
    None
}

#[cfg(not(windows))]
pub fn write_clipboard(_text: &str) -> bool {
    false
}

pub fn history_path() -> std::path::PathBuf {
    crate::model::config_dir().join("clipboard.bin")
}

/// 读取历史；返回历史与需要展示给用户的状态消息。
pub fn load(limit: usize, days: u32, now: i64) -> (History, Option<String>) {
    let path = history_path();
    let Ok(raw) = std::fs::read(&path) else {
        return (History::new(limit, days), None);
    };
    let Some(protected) = raw.strip_prefix(b"DPAPI") else {
        backup_unreadable(&path);
        return (
            History::new(limit, days),
            Some("剪贴板历史格式无法识别，已备份并重新开始".into()),
        );
    };
    let parsed = unprotect(protected)
        .and_then(|bytes| String::from_utf8(bytes).map_err(|_| "剪贴板历史不是有效文本".to_string()))
        .and_then(|text| History::from_json(&text, limit, days, now));
    match parsed {
        Ok(history) => (history, None),
        Err(_) => {
            backup_unreadable(&path);
            (
                History::new(limit, days),
                Some("剪贴板历史无法解密，已备份并重新开始".into()),
            )
        }
    }
}

pub fn save(history: &History) -> Result<(), String> {
    let protected = protect(history.to_json().as_bytes())?;
    let mut body = b"DPAPI".to_vec();
    body.extend_from_slice(&protected);
    crate::model::atomic_write(&history_path(), &body)
        .map_err(|error| format!("无法保存剪贴板历史：{error}"))
}

fn backup_unreadable(path: &std::path::Path) {
    let timestamp = chrono::Local::now().timestamp();
    let backup = path.with_file_name(format!("clipboard.bin.unreadable-{timestamp}"));
    let _ = std::fs::rename(path, backup);
}

#[cfg(windows)]
pub fn protect(data: &[u8]) -> Result<Vec<u8>, String> {
    use windows::Win32::{
        Foundation::{HLOCAL, LocalFree},
        Security::Cryptography::{
            CryptProtectData, CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN,
        },
    };
    let input = CRYPT_INTEGER_BLOB {
        cbData: data.len() as u32,
        pbData: data.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    unsafe {
        CryptProtectData(
            &input,
            None,
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
        .map_err(|_| "无法加密剪贴板历史".to_string())?;
        let bytes =
            std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        let _ = LocalFree(HLOCAL(output.pbData.cast()));
        Ok(bytes)
    }
}

#[cfg(windows)]
pub fn unprotect(data: &[u8]) -> Result<Vec<u8>, String> {
    use windows::Win32::{
        Foundation::{HLOCAL, LocalFree},
        Security::Cryptography::{
            CryptUnprotectData, CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN,
        },
    };
    let input = CRYPT_INTEGER_BLOB {
        cbData: data.len() as u32,
        pbData: data.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    unsafe {
        CryptUnprotectData(
            &input,
            None,
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
        .map_err(|_| "无法解密剪贴板历史".to_string())?;
        let bytes =
            std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        let _ = LocalFree(HLOCAL(output.pbData.cast()));
        Ok(bytes)
    }
}

#[cfg(not(windows))]
pub fn protect(_data: &[u8]) -> Result<Vec<u8>, String> {
    Err("剪贴板历史仅支持 Windows".into())
}

#[cfg(not(windows))]
pub fn unprotect(_data: &[u8]) -> Result<Vec<u8>, String> {
    Err("剪贴板历史仅支持 Windows".into())
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

    #[test]
    fn json_round_trip_keeps_order_and_pins() {
        let mut history = History::new(10, 7);
        history.insert("first".into(), 100);
        history.insert("second".into(), 200);
        let id = id_for("first");
        history.set_pinned(&id, true);

        let raw = history.to_json();
        let restored = History::from_json(&raw, 10, 7, 300).unwrap();
        assert_eq!(restored.items().len(), 2);
        assert_eq!(restored.items()[0].text, "first");
        assert!(restored.items()[0].pinned);
        assert_eq!(restored.items()[1].text, "second");
    }

    #[test]
    fn invalid_json_and_unknown_versions_are_rejected() {
        assert!(History::from_json("{", 200, 7, 0).is_err());
        assert!(History::from_json(r#"{"version":2,"items":[]}"#, 200, 7, 0).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn dpapi_round_trip_and_rejects_garbage() {
        let data = b"hello clipboard";
        let protected = protect(data).unwrap();
        assert_eq!(unprotect(&protected).unwrap(), data);
        assert!(unprotect(b"not-protected").is_err());
    }

    #[test]
    fn raw_clipboard_filter_rejects_private_empty_and_oversized() {
        let normal = RawClipboard { text: "hello".into(), excluded: false, own_process: false };
        assert!(accepts(&normal));
        assert!(!accepts(&RawClipboard { text: "hello".into(), excluded: true, own_process: false }));
        assert!(!accepts(&RawClipboard { text: "hello".into(), excluded: false, own_process: true }));
        assert!(!accepts(&RawClipboard { text: "   ".into(), excluded: false, own_process: false }));
        assert!(!accepts(&RawClipboard {
            text: "x".repeat(MAX_ITEM_BYTES + 1),
            excluded: false,
            own_process: false,
        }));
    }
}
