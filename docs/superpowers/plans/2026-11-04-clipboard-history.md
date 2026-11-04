# 剪贴板历史实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 为岛仔加入纯文本剪贴板历史：`Ctrl+Alt+V` 唤起、搜索、置顶、删除，历史用 Windows DPAPI 加密保存在本机。

**Architecture:** 新建 `clipboard`（纯逻辑 History + 后台 Service）和 `clipboard_ui`（独立视口弹窗）两个模块；Service 通过 channel 与主线程通信，所有 Win32 剪贴板读写都在后台线程；`native` 负责热键、前台窗口和光标显示器工作区；设置页与托盘作为入口和兜底。

**Tech Stack:** Rust 2021、egui/eframe 0.31、windows 0.58、ring、serde_json、chrono。

---

## 文件结构

| 文件 | 职责 |
| --- | --- |
| `src/clipboard.rs` | `Item` / `History` 纯逻辑；JSON；DPAPI；Win32 剪贴板读写；`Service` 后台线程 |
| `src/clipboard_ui.rs` | `Popup` 状态、搜索/选择逻辑、独立视口渲染 |
| `src/native.rs` | `RegisterHotKey`、`WM_HOTKEY`、前台窗口恢复、光标工作区 |
| `src/main.rs` | Service/Popup 接线、热键消费、事件分发 |
| `src/model.rs` | 4 个设置字段及持久化 |
| `src/app.rs` | `Cmd::ClipboardClear`、`clipboard_status` |
| `src/extras.rs` | 「剪贴板」设置标签页 |
| `src/tray.rs` | 「剪贴板历史」菜单项与事件 |
| `src/paint.rs` | `search`、`pin` 矢量图标 |
| `assets/settings-icons/system/clipboard.svg` | 设置页图标 |
| `src/ui_tests.rs` | 设置与弹窗状态测试 |

---

### Task 1: History 纯逻辑

**Files:**
- Create: `src/clipboard.rs`
- Modify: `src/lib.rs:8-45`

- [ ] **Step 1: 注册模块并写失败测试**

在 `src/lib.rs` 的模块列表中加入：

```rust
pub mod clipboard;
```

在 `src/clipboard.rs` 写入以下测试骨架（实现暂时留空，测试应编译失败或断言失败）：

```rust
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
```

- [ ] **Step 2: 运行测试确认失败**

```powershell
cargo test --locked --lib clipboard::tests::newest_item_goes_to_top -- --test-threads=1
```

Expected: 编译失败，提示 `History` / `id_for` 未定义。

- [ ] **Step 3: 实现 History**

在测试模块之前写入：

```rust
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
```

- [ ] **Step 4: 运行全部 History 测试**

```powershell
cargo test --locked --lib clipboard::tests -- --test-threads=1
```

Expected: 9 个测试全部 PASS。

- [ ] **Step 5: 提交**

```powershell
git add src/lib.rs src/clipboard.rs
git commit -m "feat(clipboard): add history model"
```

---

### Task 2: JSON 与 DPAPI 持久化

**Files:**
- Modify: `src/clipboard.rs`

- [ ] **Step 1: 写失败测试**

在 `clipboard::tests` 中追加：

```rust
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
```

- [ ] **Step 2: 运行测试确认失败**

```powershell
cargo test --locked --lib clipboard::tests::json_round_trip_keeps_order_and_pins -- --test-threads=1
```

Expected: 编译失败，`to_json` / `from_json` / `protect` 未定义。

- [ ] **Step 3: 实现 JSON 与加密**

在 `impl History` 中追加：

```rust
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
```

在 `impl History` 之后追加：

```rust
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
```

- [ ] **Step 4: 运行测试**

```powershell
cargo test --locked --lib clipboard::tests -- --test-threads=1
```

Expected: 全部 PASS。

- [ ] **Step 5: 提交**

```powershell
git add src/clipboard.rs
git commit -m "feat(clipboard): persist encrypted history"
```

---

### Task 3: 设置字段

**Files:**
- Modify: `src/model.rs:87-127`（字段与默认值）
- Modify: `src/model.rs:129-215`（加载与保存）
- Test: `src/ui_tests.rs`

- [ ] **Step 1: 写失败测试**

在 `src/ui_tests.rs` 追加：

```rust
#[test]
fn clipboard_settings_default_off_and_round_trip() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut h = H::new();

    assert!(!h.app.settings.clipboard_enabled);
    assert!(!h.app.settings.clipboard_paused);
    assert_eq!(h.app.settings.clipboard_limit, 200);
    assert_eq!(h.app.settings.clipboard_days, 7);

    h.app.settings.clipboard_enabled = true;
    h.app.settings.clipboard_paused = true;
    h.app.settings.clipboard_limit = 80;
    h.app.settings.clipboard_days = 3;
    h.app.settings.save();

    let loaded = crate::model::Settings::load();
    assert!(loaded.clipboard_enabled);
    assert!(loaded.clipboard_paused);
    assert_eq!(loaded.clipboard_limit, 80);
    assert_eq!(loaded.clipboard_days, 3);

    std::fs::write(
        crate::model::config_dir().join("settings.txt"),
        "clipboard_limit=9999\nclipboard_days=0\n",
    )
    .unwrap();
    let clamped = crate::model::Settings::load();
    assert_eq!(clamped.clipboard_limit, 500);
    assert_eq!(clamped.clipboard_days, 1);
}
```

- [ ] **Step 2: 运行测试确认失败**

```powershell
cargo test --locked --lib ui_tests::clipboard_settings_default_off_and_round_trip -- --test-threads=1
```

Expected: 编译失败，字段不存在。

- [ ] **Step 3: 添加字段、默认值、加载与保存**

在 `Settings` 结构体末尾加入：

```rust
    pub clipboard_enabled: bool,
    pub clipboard_paused: bool,
    pub clipboard_limit: u32,
    pub clipboard_days: u32,
```

在 `Default` 的结构体初始化末尾加入：

```rust
        clipboard_enabled: false,
        clipboard_paused: false,
        clipboard_limit: 200,
        clipboard_days: 7,
```

在 `Settings::load` 的 `match` 中加入：

```rust
                "clipboard_enabled" => s.clipboard_enabled = v == "1",
                "clipboard_paused" => s.clipboard_paused = v == "1",
                "clipboard_limit" => {
                    s.clipboard_limit = v.parse::<u32>().unwrap_or(200).clamp(50, 500)
                }
                "clipboard_days" => {
                    s.clipboard_days = v.parse::<u32>().unwrap_or(7).clamp(1, 30)
                }
```

在 `Settings::save` 的 `write_kv` 数组末尾加入：

```rust
                ("clipboard_enabled".into(), (self.clipboard_enabled as u8).to_string()),
                ("clipboard_paused".into(), (self.clipboard_paused as u8).to_string()),
                ("clipboard_limit".into(), self.clipboard_limit.to_string()),
                ("clipboard_days".into(), self.clipboard_days.to_string()),
```

- [ ] **Step 4: 运行测试**

```powershell
cargo test --locked --lib ui_tests::clipboard_settings_default_off_and_round_trip -- --test-threads=1
```

Expected: PASS。

- [ ] **Step 5: 提交**

```powershell
git add src/model.rs src/ui_tests.rs
git commit -m "feat(settings): add clipboard options"
```

---

### Task 4: Win32 剪贴板读取、写入与隐私过滤

**Files:**
- Modify: `Cargo.toml:55-87`（windows features）
- Modify: `src/clipboard.rs`

- [ ] **Step 1: 写失败测试**

在 `clipboard::tests` 中追加：

```rust
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
```

- [ ] **Step 2: 运行测试确认失败**

```powershell
cargo test --locked --lib clipboard::tests::raw_clipboard_filter_rejects_private_empty_and_oversized -- --test-threads=1
```

Expected: 编译失败，`RawClipboard` / `accepts` 未定义。

- [ ] **Step 3: 添加 windows feature**

在 `Cargo.toml` 的 `[target.'cfg(windows)'.dependencies]` 的 `windows` features 列表中，加入：

```toml
  "Win32_System_Memory",
```

- [ ] **Step 4: 实现过滤与 Win32 读写**

在 `clipboard.rs` 的 `id_for` 之后加入：

```rust
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
        System::Threading::{GetCurrentProcessId, GetWindowThreadProcessId},
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
        Foundation::HGLOBAL,
        System::{
            DataExchange::{
                CloseClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
                RegisterClipboardFormatW,
            },
            Memory::{GlobalLock, GlobalUnlock},
            Ole::CF_UNICODETEXT,
        },
    };
    unsafe {
        if OpenClipboard(None).is_err() {
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
        Foundation::HANDLE,
        System::{
            DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData},
            Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE},
            Ole::CF_UNICODETEXT,
        },
    };
    unsafe {
        if OpenClipboard(None).is_err() {
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
```

- [ ] **Step 5: 运行过滤测试与编译检查**

```powershell
cargo test --locked --lib clipboard::tests::raw_clipboard_filter_rejects_private_empty_and_oversized -- --test-threads=1
cargo check --locked --all-targets --features record
```

Expected: 测试 PASS；编译无错误。若 `OpenClipboard(None)` 的类型推断失败，改为 `OpenClipboard(Option::<windows::Win32::Foundation::HWND>::None)`。

- [ ] **Step 6: 提交**

```powershell
git add Cargo.toml Cargo.lock src/clipboard.rs
git commit -m "feat(clipboard): read and write Windows clipboard"
```

---

### Task 5: 后台捕获服务

**Files:**
- Modify: `src/clipboard.rs`

- [ ] **Step 1: 写失败测试**

在 `clipboard::tests` 中追加：

```rust
use std::sync::{
    atomic::{AtomicU32, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant};

struct FakeCapture {
    sequence: AtomicU32,
    text: Mutex<Option<String>>,
    written: Mutex<Vec<String>>,
}

impl FakeCapture {
    fn new() -> Self {
        Self {
            sequence: AtomicU32::new(1),
            text: Mutex::new(None),
            written: Mutex::new(Vec::new()),
        }
    }

    fn set(&self, text: &str) {
        *self.text.lock().unwrap() = Some(text.into());
        self.sequence.fetch_add(1, Ordering::Relaxed);
    }
}

impl Capture for FakeCapture {
    fn sequence(&self) -> u32 {
        self.sequence.load(Ordering::Relaxed)
    }

    fn read(&self) -> Option<String> {
        self.text.lock().unwrap().clone()
    }

    fn write(&self, text: &str) -> bool {
        self.written.lock().unwrap().push(text.into());
        self.sequence.fetch_add(1, Ordering::Relaxed);
        true
    }

    fn own_process(&self) -> bool {
        false
    }
}

fn next_event(service: &Service, timeout: Duration) -> Event {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(event) = service.try_recv() {
            return event;
        }
        if Instant::now() > deadline {
            panic!("等待剪贴板事件超时");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn test_history_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "daozai-clipboard-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    std::env::set_var("DAOZAI_DATA_DIR", &dir);
    dir
}

#[test]
fn service_emits_initial_snapshot_and_captures_new_text() {
    let _dir = test_history_dir();
    let capture = Arc::new(FakeCapture::new());
    let service = Service::start_with(capture.clone());

    let initial = next_event(&service, Duration::from_secs(1));
    assert!(matches!(initial, Event::Changed(items) if items.is_empty()));

    capture.set("hello clipboard");
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match next_event(&service, Duration::from_millis(300)) {
            Event::Changed(items) if items.iter().any(|item| item.text == "hello clipboard") => break,
            _ if Instant::now() > deadline => panic!("新复制内容没有进入历史"),
            _ => {}
        }
    }
}

#[test]
fn copy_command_writes_clipboard_and_does_not_recapture() {
    let _dir = test_history_dir();
    let capture = Arc::new(FakeCapture::new());
    let service = Service::start_with(capture.clone());
    let _ = next_event(&service, Duration::from_secs(1));

    capture.set("from another app");
    let deadline = Instant::now() + Duration::from_secs(2);
    let item = loop {
        if let Event::Changed(items) = next_event(&service, Duration::from_millis(300)) {
            if let Some(item) = items.first() {
                if item.text == "from another app" {
                    break item.clone();
                }
            }
        }
        assert!(Instant::now() < deadline, "待复制内容没有进入历史");
    };

    service.copy(&item.id);
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        match next_event(&service, Duration::from_millis(300)) {
            Event::Copied(id) if id == item.id => break,
            _ if Instant::now() > deadline => panic!("复制命令没有回执"),
            _ => {}
        }
    }
    assert_eq!(capture.written.lock().unwrap().as_slice(), ["from another app"]);
}

#[test]
fn delete_pin_and_clear_update_the_snapshot() {
    let _dir = test_history_dir();
    let capture = Arc::new(FakeCapture::new());
    let service = Service::start_with(capture.clone());
    let _ = next_event(&service, Duration::from_secs(1));

    capture.set("keep me");
    let deadline = Instant::now() + Duration::from_secs(2);
    let item = loop {
        if let Event::Changed(items) = next_event(&service, Duration::from_millis(300)) {
            if let Some(item) = items.first().filter(|item| item.text == "keep me") {
                break item.clone();
            }
        }
        assert!(Instant::now() < deadline, "待测内容没有进入历史");
    };

    let wanted = item.id.clone();
    service.pin(&wanted, true);
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        if let Event::Changed(items) = next_event(&service, Duration::from_millis(300)) {
            if items.first().is_some_and(|entry| entry.id == wanted && entry.pinned) {
                break;
            }
        }
        assert!(Instant::now() < deadline, "置顶状态没有更新");
    }

    service.delete(&wanted);
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        if let Event::Changed(items) = next_event(&service, Duration::from_millis(300)) {
            if items.is_empty() {
                break;
            }
        }
        assert!(Instant::now() < deadline, "删除后历史没有清空");
    }
}
```

- [ ] **Step 2: 运行测试确认失败**

```powershell
cargo test --locked --lib clipboard::tests::service_emits_initial_snapshot_and_captures_new_text -- --test-threads=1
```

Expected: 编译失败，`Service` / `Event` / `Capture` 未定义。

- [ ] **Step 3: 实现 Service**

在 `clipboard.rs` 末尾（测试模块之前）加入：

```rust
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::{Duration, Instant};

pub enum Cmd {
    SetEnabled(bool),
    SetLimits(usize, u32),
    Copy(String),
    Delete(String),
    Pin(String, bool),
    Clear,
    Refresh,
}

#[derive(Clone, Debug)]
pub enum Event {
    Changed(std::sync::Arc<Vec<Item>>),
    Copied(String),
    Status(String),
}

pub trait Capture: Send + Sync {
    fn sequence(&self) -> u32;
    fn read(&self) -> Option<String>;
    fn write(&self, text: &str) -> bool;
    fn own_process(&self) -> bool;
}

struct SystemCapture;

impl Capture for SystemCapture {
    fn sequence(&self) -> u32 {
        clipboard_sequence()
    }

    fn read(&self) -> Option<String> {
        read_clipboard()
    }

    fn write(&self, text: &str) -> bool {
        write_clipboard(text)
    }

    fn own_process(&self) -> bool {
        clipboard_owner_is_self()
    }
}

pub struct Service {
    cmd: Sender<Cmd>,
    event: Receiver<Event>,
}

impl Service {
    pub fn start() -> Self {
        Self::spawn(std::sync::Arc::new(SystemCapture))
    }

    #[cfg(test)]
    pub(crate) fn start_with(capture: std::sync::Arc<dyn Capture>) -> Self {
        Self::spawn(capture)
    }

    fn spawn(capture: std::sync::Arc<dyn Capture>) -> Self {
        let (event_tx, event_rx) = channel();
        let (cmd_tx, cmd_rx) = channel();
        let _ = std::thread::Builder::new()
            .name("daozai-clipboard".into())
            .spawn(move || supervisor(capture, cmd_rx, event_tx));
        Self { cmd: cmd_tx, event: event_rx }
    }

    pub fn set_enabled(&self, enabled: bool) {
        let _ = self.cmd.send(Cmd::SetEnabled(enabled));
    }

    pub fn set_limits(&self, limit: usize, days: u32) {
        let _ = self.cmd.send(Cmd::SetLimits(limit, days));
    }

    pub fn copy(&self, id: &str) {
        let _ = self.cmd.send(Cmd::Copy(id.into()));
    }

    pub fn delete(&self, id: &str) {
        let _ = self.cmd.send(Cmd::Delete(id.into()));
    }

    pub fn pin(&self, id: &str, pinned: bool) {
        let _ = self.cmd.send(Cmd::Pin(id.into(), pinned));
    }

    pub fn clear(&self) {
        let _ = self.cmd.send(Cmd::Clear);
    }

    pub fn refresh(&self) {
        let _ = self.cmd.send(Cmd::Refresh);
    }

    pub fn try_recv(&self) -> Option<Event> {
        self.event.try_recv().ok()
    }
}

fn now_seconds() -> i64 {
    chrono::Local::now().timestamp()
}

fn supervisor(capture: std::sync::Arc<dyn Capture>, commands: Receiver<Cmd>, events: Sender<Event>) {
    let (mut history, load_status) = load(DEFAULT_LIMIT, DEFAULT_DAYS, now_seconds());
    if let Some(status) = load_status {
        let _ = events.send(Event::Status(status));
    }
    let _ = events.send(Event::Changed(std::sync::Arc::new(history.items().to_vec())));
    let mut enabled = true;
    let mut last_sequence = capture.sequence();
    let mut last_poll = Instant::now();
    let mut last_prune = Instant::now();

    loop {
        match commands.recv_timeout(Duration::from_millis(50)) {
            Ok(Cmd::SetEnabled(next)) => {
                enabled = next;
                if enabled {
                    // 开启时不要把当前剪贴板里的旧内容当成新复制。
                    last_sequence = capture.sequence();
                }
            }
            Ok(Cmd::SetLimits(limit, days)) => {
                history.set_limits(limit, days, now_seconds());
                if let Err(error) = save(&history) {
                    let _ = events.send(Event::Status(error));
                }
                let _ = events.send(Event::Changed(std::sync::Arc::new(history.items().to_vec())));
            }
            Ok(Cmd::Copy(id)) => {
                if let Some(text) = history.items().iter().find(|item| item.id == id).map(|item| item.text.clone()) {
                    if capture.write(&text) {
                        last_sequence = capture.sequence();
                        let _ = events.send(Event::Copied(id));
                    } else {
                        let _ = events.send(Event::Status("无法写入剪贴板".into()));
                    }
                }
            }
            Ok(Cmd::Delete(id)) => {
                history.remove(&id);
                if let Err(error) = save(&history) {
                    let _ = events.send(Event::Status(error));
                }
                let _ = events.send(Event::Changed(std::sync::Arc::new(history.items().to_vec())));
            }
            Ok(Cmd::Pin(id, pinned)) => {
                history.set_pinned(&id, pinned);
                if let Err(error) = save(&history) {
                    let _ = events.send(Event::Status(error));
                }
                let _ = events.send(Event::Changed(std::sync::Arc::new(history.items().to_vec())));
            }
            Ok(Cmd::Clear) => {
                history.clear();
                if let Err(error) = save(&history) {
                    let _ = events.send(Event::Status(error));
                }
                let _ = events.send(Event::Changed(std::sync::Arc::new(history.items().to_vec())));
            }
            Ok(Cmd::Refresh) => {
                let _ = events.send(Event::Changed(std::sync::Arc::new(history.items().to_vec())));
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
        }

        if !enabled {
            continue;
        }
        if last_poll.elapsed() >= Duration::from_millis(500) {
            last_poll = Instant::now();
            let sequence = capture.sequence();
            if sequence != last_sequence {
                last_sequence = sequence;
                if let Some(text) = capture.read() {
                    let raw = RawClipboard {
                        text,
                        excluded: false,
                        own_process: capture.own_process(),
                    };
                    if accepts(&raw) && history.insert(raw.text, now_seconds()) {
                        if let Err(error) = save(&history) {
                            let _ = events.send(Event::Status(error));
                        }
                        let _ = events.send(Event::Changed(std::sync::Arc::new(history.items().to_vec())));
                    }
                }
            }
        }
        if last_prune.elapsed() >= Duration::from_secs(3600) {
            last_prune = Instant::now();
            history.cleanup(now_seconds());
            if let Err(error) = save(&history) {
                let _ = events.send(Event::Status(error));
            }
            let _ = events.send(Event::Changed(std::sync::Arc::new(history.items().to_vec())));
        }
    }
}
```

说明：`FakeCapture` 的 `read` 已经做了排除格式模拟；真实 `SystemCapture` 的 `read_clipboard` 内部会检查排除格式，因此 `supervisor` 只需要补 owner 与长度/空白判断。

- [ ] **Step 4: 运行 Service 测试**

```powershell
cargo test --locked --lib clipboard::tests -- --test-threads=1
```

Expected: 全部 PASS。若测试偶发失败，检查 `recv_timeout` 与 500ms 轮询窗口，保持超时至少 2 秒。

- [ ] **Step 5: 提交**

```powershell
git add src/clipboard.rs
git commit -m "feat(clipboard): add background capture service"
```

---

### Task 6: 原生热键与窗口辅助

**Files:**
- Modify: `src/native.rs:26-130`（静态量、子类过程）
- Modify: `src/native.rs:178-205`（Window 结构体与 new）
- Modify: `src/native.rs:260-270`（Drop）
- Modify: `src/native.rs:319`（测试构造器）

- [ ] **Step 1: 写失败测试**

在 `native::tests` 中追加：

```rust
#[test]
fn clipboard_hotkey_flag_is_consumed_once() {
    test_clipboard_hotkey_press();
    assert!(take_clipboard_hotkey());
    assert!(!take_clipboard_hotkey());
}
```

- [ ] **Step 2: 运行测试确认失败**

```powershell
cargo test --locked --lib native::tests::clipboard_hotkey_flag_is_consumed_once -- --test-threads=1
```

Expected: 编译失败，相关函数未定义。

- [ ] **Step 3: 实现热键与辅助函数**

在 `native.rs` 顶部静态量旁加入：

```rust
/// 全局剪贴板历史热键。
const CLIPBOARD_HOTKEY_ID: i32 = 0xDA0A;
static CLIPBOARD_HOTKEY: AtomicBool = AtomicBool::new(false);

pub const CLIPBOARD_HOTKEY_TEXT: &str = "Ctrl+Alt+V";

pub fn register_clipboard_hotkey(hwnd: HWND) -> Result<(), String> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        RegisterHotKey, HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT,
    };
    unsafe {
        RegisterHotKey(
            hwnd,
            CLIPBOARD_HOTKEY_ID,
            HOT_KEY_MODIFIERS(MOD_CONTROL.0 | MOD_ALT.0 | MOD_NOREPEAT.0),
            'V' as u32,
        )
        .map_err(|_| "全局热键 Ctrl+Alt+V 被其他程序占用".to_string())
    }
}

pub fn unregister_clipboard_hotkey(hwnd: HWND) {
    use windows::Win32::UI::Input::KeyboardAndMouse::UnregisterHotKey;
    unsafe {
        let _ = UnregisterHotKey(hwnd, CLIPBOARD_HOTKEY_ID);
    }
}

pub fn take_clipboard_hotkey() -> bool {
    CLIPBOARD_HOTKEY.swap(false, Ordering::Relaxed)
}

#[cfg(test)]
pub(crate) fn test_clipboard_hotkey_press() {
    CLIPBOARD_HOTKEY.store(true, Ordering::Relaxed);
}

pub fn foreground_window() -> Option<HWND> {
    let hwnd = unsafe { GetForegroundWindow() };
    (!hwnd.0.is_null()).then_some(hwnd)
}

pub fn restore_foreground(hwnd: Option<HWND>) {
    let Some(hwnd) = hwnd else { return };
    unsafe {
        if IsWindow(hwnd).as_bool() {
            let _ = SetForegroundWindow(hwnd);
        }
    }
}

/// 光标所在显示器的工作区，换算成逻辑点。
pub fn cursor_work_area(pixels_per_point: f32) -> Option<(f32, f32, f32, f32)> {
    use windows::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    };
    unsafe {
        let mut cursor = POINT::default();
        GetCursorPos(&mut cursor).ok()?;
        let monitor = MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST);
        if monitor.0.is_null() {
            return None;
        }
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if !GetMonitorInfoW(monitor, &mut info).as_bool() {
            return None;
        }
        let area = info.rcWork;
        let scale = pixels_per_point.max(0.1);
        Some((
            area.left as f32 / scale,
            area.top as f32 / scale,
            (area.right - area.left) as f32 / scale,
            (area.bottom - area.top) as f32 / scale,
        ))
    }
}
```

在 `subclass` 函数的 `WM_NCACTIVATE` 分支之前加入：

```rust
    if msg == WM_HOTKEY && wp.0 as i32 == CLIPBOARD_HOTKEY_ID {
        CLIPBOARD_HOTKEY.store(true, Ordering::Relaxed);
        return LRESULT(0);
    }
```

- [ ] **Step 4: 在 Window 中注册与注销**

在 `pub struct Window` 中加入字段：

```rust
    pub hotkey_error: Option<String>,
```

在 `Window::new` 的 `Some(Self { ... })` 之前加入：

```rust
        let hotkey_error = register_clipboard_hotkey(hwnd).err();
```

并在结构体初始化中加入：

```rust
            hotkey_error,
```

在 `impl Drop for Window` 中加入：

```rust
            unregister_clipboard_hotkey(self.hwnd);
```

在测试构造器 `let mut native=Window{...}` 中加入：

```rust
hotkey_error: None,
```

- [ ] **Step 5: 运行测试与编译检查**

```powershell
cargo test --locked --lib native::tests::clipboard_hotkey_flag_is_consumed_once -- --test-threads=1
cargo check --locked --all-targets --features record
```

Expected: 测试 PASS；编译无错误。

- [ ] **Step 6: 提交**

```powershell
git add src/native.rs
git commit -m "feat(native): register clipboard hotkey"
```

---

### Task 7: 弹窗纯状态与时间格式

**Files:**
- Create: `src/clipboard_ui.rs`
- Modify: `src/lib.rs`

- [ ] **Step 1: 注册模块并写失败测试**

在 `src/lib.rs` 加入：

```rust
pub mod clipboard_ui;
```

在 `src/clipboard_ui.rs` 写入测试与骨架：

```rust
//! 剪贴板历史弹窗：独立视口、搜索与键盘选择。
use crate::clipboard::Item;
use std::time::{Duration, Instant};

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
        assert_eq!(popup.selected_id().as_deref(), Some("c"));
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
```

- [ ] **Step 2: 运行测试确认失败**

```powershell
cargo test --locked --lib clipboard_ui::tests -- --test-threads=1
```

Expected: 编译失败，`Popup` / `relative_time` 未定义。

- [ ] **Step 3: 实现状态与时间格式**

在测试模块之前写入：

```rust
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
```

- [ ] **Step 4: 运行测试**

```powershell
cargo test --locked --lib clipboard_ui::tests -- --test-threads=1
```

Expected: 全部 PASS。

- [ ] **Step 5: 提交**

```powershell
git add src/lib.rs src/clipboard_ui.rs
git commit -m "feat(clipboard): add popup state"
```

---

### Task 8: 弹窗渲染

**Files:**
- Modify: `src/clipboard_ui.rs`
- Modify: `src/paint.rs`

- [ ] **Step 1: 添加矢量图标**

在 `src/paint.rs` 的图标函数区域加入：

```rust
pub fn search(c: &Card, cx: f32, cy: f32, r: f32, color: Color32) {
    let center = c.pos(cx, cy);
    c.painter
        .circle_stroke(center, r * c.s, Stroke::new(1.4 * c.s, color));
    c.painter.line_segment(
        [
            center + Vec2::new(r * 0.7, r * 0.7) * c.s,
            center + Vec2::new(r * 1.35, r * 1.35) * c.s,
        ],
        Stroke::new(1.4 * c.s, color),
    );
}

pub fn pin(c: &Card, cx: f32, cy: f32, size: f32, filled: bool, color: Color32) {
    let center = c.pos(cx, cy);
    let s = size * c.s;
    if filled {
        c.painter
            .circle_filled(center + Vec2::new(0.0, -s * 0.18), s * 0.22, color);
    } else {
        c.painter.circle_stroke(
            center + Vec2::new(0.0, -s * 0.18),
            s * 0.22,
            Stroke::new(1.3 * c.s, color),
        );
    }
    c.painter.line_segment(
        [
            center + Vec2::new(0.0, s * 0.04),
            center + Vec2::new(0.0, s * 0.45),
        ],
        Stroke::new(1.5 * c.s, color),
    );
    c.painter.line_segment(
        [
            center + Vec2::new(-s * 0.18, s * 0.16),
            center + Vec2::new(s * 0.18, s * 0.16),
        ],
        Stroke::new(1.3 * c.s, color),
    );
}
```

- [ ] **Step 2: 实现 `Popup::show`**

在 `clipboard_ui.rs` 顶部补充导入：

```rust
use crate::settings_theme::color as settings_color;
use crate::theme::{self, W};
use eframe::egui::{self, Color32, Key, Modifiers, Sense, Vec2};

const WIDTH: f32 = 420.0;
const HEIGHT: f32 = 440.0;
```

在 `impl Popup` 中加入：

```rust
    pub fn show(&mut self, ctx: &egui::Context, dark: bool) -> Action {
        if !self.open {
            return Action::None;
        }
        let viewport_id = egui::ViewportId::from_hash_of("daozai-clipboard");
        let ppp = ctx.pixels_per_point();
        #[cfg(windows)]
        let position = crate::native::cursor_work_area(ppp)
            .map(|(x, y, width, _)| egui::pos2(x + (width - WIDTH) / 2.0, y + 60.0))
            .unwrap_or_else(|| egui::pos2(160.0, 80.0));
        #[cfg(not(windows))]
        let position = {
            let _ = ppp;
            egui::pos2(160.0, 80.0)
        };
        let mut action = Action::None;

        ctx.show_viewport_immediate(
            viewport_id,
            egui::ViewportBuilder::default()
                .with_title("剪贴板历史 · 岛仔")
                .with_icon(theme::app_icon())
                .with_inner_size([WIDTH, HEIGHT])
                .with_min_inner_size([360.0, 300.0])
                .with_decorations(false)
                .with_transparent(true)
                .with_resizable(false)
                .with_taskbar(false)
                .with_always_on_top()
                .with_active(false)
                .with_visible(false)
                .with_position(position),
            |ctx, _| {
                let original = ctx.style();
                let mut style = (*original).clone();
                style.visuals = if dark { egui::Visuals::dark() } else { egui::Visuals::light() };
                ctx.set_style(style);
                ctx.send_viewport_cmd(egui::ViewportCommand::SetTheme(if dark {
                    egui::SystemTheme::Dark
                } else {
                    egui::SystemTheme::Light
                }));

                if !self.shown {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                    self.shown = true;
                }
                if ctx.input(|input| input.viewport().close_requested()) {
                    action = Action::Close { restore: false };
                }
                let focused = ctx.input(|input| input.viewport().focused.unwrap_or(false));
                if self.observe_focus(focused, Instant::now()) {
                    action = Action::Close { restore: false };
                }

                let (up, down, enter, escape, delete) = ctx.input_mut(|input| {
                    (
                        input.consume_key(Modifiers::NONE, Key::ArrowUp),
                        input.consume_key(Modifiers::NONE, Key::ArrowDown),
                        input.consume_key(Modifiers::NONE, Key::Enter),
                        input.consume_key(Modifiers::NONE, Key::Escape),
                        input.consume_key(Modifiers::NONE, Key::Delete),
                    )
                });
                if escape {
                    action = Action::Close { restore: true };
                }
                if up {
                    self.move_selection(-1);
                }
                if down {
                    self.move_selection(1);
                }
                if enter {
                    if let Some(id) = self.selected_id() {
                        action = Action::Copy(id);
                    }
                }
                if delete {
                    if let Some(id) = self.selected_id() {
                        action = Action::Delete(id);
                    }
                }

                let previous_query = self.query.clone();
                egui::CentralPanel::default()
                    .frame(egui::Frame::new().inner_margin(10))
                    .show(ctx, |ui| {
                        let background =
                            settings_color(ctx, Color32::from_rgb(35, 35, 38), Color32::WHITE);
                        egui::Frame::new()
                            .fill(background)
                            .corner_radius(14.0)
                            .inner_margin(14)
                            .show(ui, |ui| {
                                ui.set_min_size(ui.available_size());
                                ui.horizontal(|ui| {
                                    let (icon_rect, _) = ui
                                        .allocate_exact_size(Vec2::splat(16.0), Sense::hover());
                                    let card =
                                        crate::paint::Card::new(ui, icon_rect.min, icon_rect);
                                    crate::paint::search(
                                        &card,
                                        7.0,
                                        7.0,
                                        5.0,
                                        settings_color(
                                            ctx,
                                            Color32::from_gray(150),
                                            Color32::from_gray(105),
                                        ),
                                    );
                                    let response = ui.add(
                                        egui::TextEdit::singleline(&mut self.query)
                                            .hint_text("搜索剪贴板…")
                                            .font(theme::font(13.0, W::Regular))
                                            .frame(false)
                                            .desired_width(f32::INFINITY),
                                    );
                                    if self.request_focus {
                                        response.request_focus();
                                        self.request_focus = false;
                                    }
                                });
                                if self.query != previous_query {
                                    self.selected = 0;
                                    self.clamp();
                                }
                                if let Some(error) = &self.hotkey_error {
                                    ui.label(
                                        egui::RichText::new(error)
                                            .font(theme::font(10.5, W::Regular))
                                            .color(theme::YELLOW),
                                    );
                                }
                                ui.add_space(6.0);
                                ui.separator();
                                if !self.enabled {
                                    ui.add_space(10.0);
                                    ui.vertical_centered(|ui| {
                                        ui.label(
                                            egui::RichText::new("剪贴板历史未开启")
                                                .font(theme::font(13.0, W::Regular))
                                                .color(settings_color(
                                                    ctx,
                                                    Color32::WHITE,
                                                    Color32::from_rgb(30, 34, 41),
                                                )),
                                        );
                                        ui.add_space(4.0);
                                        ui.label(
                                            egui::RichText::new(
                                                "开启后，复制的文本会加密保存在本机",
                                            )
                                            .font(theme::font(11.0, W::Regular))
                                            .color(settings_color(
                                                ctx,
                                                Color32::from_gray(180),
                                                Color32::from_gray(75),
                                            )),
                                        );
                                        ui.add_space(10.0);
                                        if ui.button("开启剪贴板历史").clicked() {
                                            action = Action::Enable;
                                        }
                                    });
                                    return;
                                }
                                if self.paused {
                                    ui.add_space(8.0);
                                    ui.horizontal(|ui| {
                                        ui.label(
                                            egui::RichText::new("已暂停记录")
                                                .font(theme::font(11.5, W::Regular))
                                                .color(settings_color(
                                                    ctx,
                                                    Color32::from_gray(180),
                                                    Color32::from_gray(75),
                                                )),
                                        );
                                        if ui.button("恢复").clicked() {
                                            action = Action::Resume;
                                        }
                                    });
                                }

                                let indices = self.filtered();
                                self.clamp();
                                egui::ScrollArea::vertical()
                                    .auto_shrink([false, false])
                                    .show(ui, |ui| {
                                        if indices.is_empty() {
                                            ui.add_space(70.0);
                                            ui.vertical_centered(|ui| {
                                                ui.label(
                                                    egui::RichText::new(if self.query.trim().is_empty() {
                                                        "还没有记录"
                                                    } else {
                                                        "没有匹配的内容"
                                                    })
                                                    .font(theme::font(12.0, W::Regular))
                                                    .color(settings_color(
                                                        ctx,
                                                        Color32::from_gray(150),
                                                        Color32::from_gray(105),
                                                    )),
                                                );
                                            });
                                            return;
                                        }
                                        for (position, index) in indices.iter().enumerate() {
                                            if let Some(next) = row(
                                                ui,
                                                &self.items[*index],
                                                position == self.selected,
                                                chrono::Local::now().timestamp(),
                                            ) {
                                                action = next;
                                            }
                                        }
                                    });
                                ui.add_space(6.0);
                                ui.label(
                                    egui::RichText::new("↑↓ 选择 · Enter 复制 · Esc 关闭")
                                        .font(theme::font(10.0, W::Regular))
                                        .color(settings_color(
                                            ctx,
                                            Color32::from_gray(150),
                                            Color32::from_gray(105),
                                        )),
                                );
                            });
                    });
            },
        );
        action
    }
```

在 `impl Popup` 之后加入行渲染函数：

```rust
fn row(
    ui: &mut egui::Ui,
    item: &Item,
    selected: bool,
    now: i64,
) -> Option<Action> {
    let ctx = ui.ctx().clone();
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), 56.0),
        Sense::click(),
    );
    if !ui.is_rect_visible(rect) {
        return None;
    }
    let hover = ctx.animate_bool_with_time(
        ui.id().with(("clipboard-row", item.id.as_str())),
        response.hovered(),
        0.12,
    );
    let text_color = settings_color(&ctx, Color32::WHITE, Color32::from_rgb(30, 34, 41));
    let muted = settings_color(&ctx, Color32::from_gray(150), Color32::from_gray(105));
    let painter = ui.painter().with_clip_rect(rect);
    if selected {
        painter.rect_filled(rect, 8.0, theme::alpha_blue(0.16));
        painter.rect_filled(
            egui::Rect::from_min_size(rect.min, Vec2::new(2.0, rect.height())),
            1.0,
            theme::BLUE,
        );
    } else if hover > 0.0 {
        painter.rect_filled(
            rect,
            8.0,
            settings_color(
                &ctx,
                Color32::from_rgb(43, 43, 46),
                Color32::from_rgb(214, 221, 232),
            )
            .gamma_multiply(hover),
        );
    }

    let preview = item.text.replace(['\r', '\n'], " ");
    let mut job = egui::text::LayoutJob::simple(
        preview,
        theme::font(13.0, W::Regular),
        text_color,
        rect.width() - 76.0,
    );
    job.wrap.max_rows = 2;
    job.wrap.break_anywhere = true;
    let galley = ui.fonts(|fonts| fonts.layout_job(job));
    painter.galley(rect.min + Vec2::new(10.0, 7.0), galley, text_color);
    painter.text(
        egui::pos2(rect.left() + 10.0, rect.bottom() - 8.0),
        egui::Align2::LEFT_BOTTOM,
        relative_time(now, item.copied_at),
        theme::font(11.0, W::Regular),
        muted,
    );

    let pin_rect = egui::Rect::from_center_size(
        egui::pos2(rect.right() - 22.0, rect.center().y),
        Vec2::splat(22.0),
    );
    let pin_response = ui.interact(
        pin_rect,
        ui.id().with(("clipboard-pin", item.id.as_str())),
        Sense::click(),
    );
    if item.pinned || pin_response.hovered() {
        let card = crate::paint::Card::new(ui, pin_rect.min, pin_rect);
        crate::paint::pin(
            &card,
            11.0,
            11.0,
            14.0,
            item.pinned,
            if item.pinned { theme::YELLOW } else { muted },
        );
    }

    let delete_rect = egui::Rect::from_center_size(
        egui::pos2(rect.right() - 48.0, rect.center().y),
        Vec2::splat(22.0),
    );
    let delete_response = ui.interact(
        delete_rect,
        ui.id().with(("clipboard-delete", item.id.as_str())),
        Sense::click(),
    );
    if delete_response.hovered() || selected {
        let card = crate::paint::Card::new(ui, delete_rect.min, delete_rect);
        crate::paint::close(
            &card,
            11.0,
            11.0,
            4.5,
            if delete_response.hovered() { theme::RED } else { muted },
        );
    }

    if pin_response.clicked() {
        return Some(Action::Pin(item.id.clone(), !item.pinned));
    }
    if delete_response.clicked() {
        return Some(Action::Delete(item.id.clone()));
    }
    if response.clicked() {
        return Some(Action::Copy(item.id.clone()));
    }
    None
}
```

- [ ] **Step 3: 编译检查**

```powershell
cargo check --locked --all-targets --features record
```

Expected: 编译无错误。若 `painter.rect_filled` 的圆角参数类型不符，改为 `egui::CornerRadius::same(8)`；若 `LayoutJob::simple` 的边长参数顺序不符，按编译器提示调整。

**透明回退**：如果目标 GPU 或远程桌面下透明视口出现黑角/闪烁，把 `with_transparent(true)` 改为 `false`，让卡片背景填满整个窗口，并把外层圆角设为 0；颜色、字体、行高和交互保持不变，不另起一套视觉。

**多显示器**：`cursor_work_area` 按当前 `pixels_per_point` 换算；混合 DPI 多屏下如果位置偏移，先记录实际显示器与缩放比例，再在 Task 11 的手动验收中修正，不要改动岛体窗口的定位逻辑。

- [ ] **Step 4: 提交**

```powershell
git add src/paint.rs src/clipboard_ui.rs
git commit -m "feat(clipboard): render history popup"
```

---

### Task 9: 主线程与托盘接线

**Files:**
- Modify: `src/app.rs:110-150`（Cmd）
- Modify: `src/app.rs:295-380`（App 字段）
- Modify: `src/app.rs:480-495`（App 初始化）
- Modify: `src/main.rs:12-54`（Shell 字段）
- Modify: `src/main.rs:85-118`（Shell 初始化）
- Modify: `src/main.rs:196-380`（update 与 Cmd 处理）
- Modify: `src/tray.rs:47-100`、`:176-200`、`:285-300`、`:581-610`

- [ ] **Step 1: 添加 Cmd 与状态字段**

在 `src/app.rs` 的 `pub enum Cmd` 中加入：

```rust
    /// 清空剪贴板历史。
    ClipboardClear,
```

在 `App` 结构体中 `listener_status` 附近加入：

```rust
    /// 剪贴板热键冲突或历史读写状态，设置页展示。
    pub clipboard_status: String,
```

在 `App::new` 的结构体初始化中加入：

```rust
            clipboard_status: String::new(),
```

- [ ] **Step 2: 托盘菜单**

在 `src/tray.rs` 常量区加入：

```rust
const CMD_CLIPBOARD: u32 = 7;
```

在 `Event` 枚举中加入：

```rust
    OpenClipboard,
```

将 `labels` 改为：

```rust
fn labels() -> [&'static str; 7] {
    ["开机启动", "吉祥物", "极简刘海", "关于岛仔", "退出", "设置…", "剪贴板历史"]
}
```

在 `popup_menu` 的 `CMD_SETTINGS` 行之后加入：

```rust
        append(MF_STRING, CMD_CLIPBOARD, PCWSTR(wide[6].as_ptr()));
```

在菜单命令的 `match` 中加入：

```rust
            CMD_CLIPBOARD => { let _ = ctx.tx.send(Event::OpenClipboard); }
```

更新托盘测试：

```rust
            assert_eq!(items.len(), 8, "7 项 + 1 分隔线");
            let texts: Vec<_> = items.iter().filter(|i| !i.2).map(|i| i.0.as_str()).collect();
            assert_eq!(texts, ["开机启动", "吉祥物", "极简刘海", "设置…", "剪贴板历史", "关于岛仔", "退出"]);
```

```rust
        assert_eq!(
            labels(),
            ["开机启动", "吉祥物", "极简刘海", "关于岛仔", "退出", "设置…", "剪贴板历史"]
        );
```

```rust
        assert_eq!(
            [CMD_EXIT, CMD_START, CMD_MASCOT, CMD_ABOUT, CMD_MINIMAL, CMD_CLIPBOARD],
            [1, 2, 3, 4, 5, 7]
        );
```

- [ ] **Step 3: Shell 字段与初始化**

在 `Shell` 结构体中加入：

```rust
    clipboard: daozai::clipboard::Service,
    clipboard_popup: daozai::clipboard_ui::Popup,
    clipboard_sig: (bool, usize, u32),
    #[cfg(windows)]
    clipboard_prev: Option<windows::Win32::Foundation::HWND>,
    clipboard_status: String,
```

在 `Shell::new` 中、结构体字面量之前加入：

```rust
        let clipboard_enabled =
            app.settings.clipboard_enabled && !app.settings.clipboard_paused;
        let clipboard_limit = app.settings.clipboard_limit as usize;
        let clipboard_days = app.settings.clipboard_days;
        let clipboard = daozai::clipboard::Service::start();
        clipboard.set_enabled(clipboard_enabled);
        clipboard.set_limits(clipboard_limit, clipboard_days);
```

在 `Shell::new` 的结构体初始化中加入：

```rust
            clipboard,
            clipboard_popup: daozai::clipboard_ui::Popup::default(),
            clipboard_sig: (clipboard_enabled, clipboard_limit, clipboard_days),
            #[cfg(windows)]
            clipboard_prev: None,
            clipboard_status: String::new(),
```

确认 `lib.rs` 中是 `pub mod clipboard_ui;`，且 `Action`、`Popup` 已标记为 `pub`。

- [ ] **Step 4: 热键、事件与弹窗**

在 `Shell::update` 中 `poll_tray()` 之后加入：

```rust
        #[cfg(windows)]
        if daozai::native::take_clipboard_hotkey() {
            if self.clipboard_popup.open {
                self.close_clipboard(true);
            } else {
                self.open_clipboard(ctx, true);
            }
        }
        self.poll_clipboard();
```

在 `impl Shell` 中加入：

```rust
    fn open_clipboard(&mut self, ctx: &egui::Context, restore: bool) {
        #[cfg(windows)]
        {
            let previous = if restore { daozai::native::foreground_window() } else { None };
            self.clipboard_prev = previous;
        }
        #[cfg(not(windows))]
        let _ = restore;
        let hotkey_error = self.clipboard_hotkey_error();
        self.clipboard_popup.open_state(
            self.app.settings.clipboard_enabled,
            self.app.settings.clipboard_paused,
            hotkey_error,
        );
        self.clipboard.refresh();
        ctx.request_repaint();
    }

    fn close_clipboard(&mut self, restore: bool) {
        self.clipboard_popup.close();
        #[cfg(windows)]
        {
            if restore {
                daozai::native::restore_foreground(self.clipboard_prev.take());
            }
        }
        #[cfg(not(windows))]
        let _ = restore;
    }

    fn clipboard_hotkey_error(&self) -> Option<String> {
        #[cfg(windows)]
        {
            self.native.as_ref().and_then(|native| native.hotkey_error.clone())
        }
        #[cfg(not(windows))]
        {
            None
        }
    }

    fn poll_clipboard(&mut self) {
        let settings = &self.app.settings;
        let signature = (
            settings.clipboard_enabled && !settings.clipboard_paused,
            settings.clipboard_limit as usize,
            settings.clipboard_days,
        );
        if signature != self.clipboard_sig {
            self.clipboard.set_limits(signature.1, signature.2);
            self.clipboard.set_enabled(signature.0);
            self.clipboard_sig = signature;
        }
        while let Some(event) = self.clipboard.try_recv() {
            match event {
                daozai::clipboard::Event::Changed(items) => {
                    self.clipboard_popup.set_items((*items).clone());
                }
                daozai::clipboard::Event::Copied(_) => {
                    self.app.say("已复制");
                }
                daozai::clipboard::Event::Status(message) => {
                    self.clipboard_status = message;
                }
            }
        }
        let hotkey_error = self.clipboard_hotkey_error();
        self.clipboard_status = hotkey_error.clone().unwrap_or_else(|| self.clipboard_status.clone());
        self.app.clipboard_status = self.clipboard_status.clone();
        self.clipboard_popup.sync(
            self.app.settings.clipboard_enabled,
            self.app.settings.clipboard_paused,
            hotkey_error,
        );
    }

    fn show_clipboard(&mut self, ctx: &egui::Context) {
        let dark = self
            .app
            .settings
            .settings_theme
            .dark(daozai::settings_theme::system_dark(ctx));
        match self.clipboard_popup.show(ctx, dark) {
            daozai::clipboard_ui::Action::None => {}
            daozai::clipboard_ui::Action::Copy(id) => {
                self.close_clipboard(true);
                self.clipboard.copy(&id);
            }
            daozai::clipboard_ui::Action::Delete(id) => self.clipboard.delete(&id),
            daozai::clipboard_ui::Action::Pin(id, pinned) => self.clipboard.pin(&id, pinned),
            daozai::clipboard_ui::Action::Enable => {
                self.app.settings.clipboard_enabled = true;
                self.app.settings.save();
            }
            daozai::clipboard_ui::Action::Resume => {
                self.app.settings.clipboard_paused = false;
                self.app.settings.save();
            }
            daozai::clipboard_ui::Action::Close { restore } => self.close_clipboard(restore),
        }
    }
```

在 `Shell::update` 末尾、`ctx.request_repaint_after(...)` 之前加入：

```rust
        self.show_clipboard(ctx);
        let repaint_after = if self.clipboard_popup.open {
            Duration::from_millis(16)
        } else {
            repaint_after
        };
```

在 `Cmd` 处理 `match` 中加入：

```rust
                Cmd::ClipboardClear => self.clipboard.clear(),
```

在 `poll_tray` 的事件 `match` 中加入：

```rust
                Event::OpenClipboard => self.open_clipboard(ctx, false),
```

注意：`poll_tray(&mut self)` 当前没有 `ctx` 参数；将签名改为 `fn poll_tray(&mut self, ctx: &egui::Context)`，并更新调用处 `self.poll_tray(ctx);`。托盘入口不恢复前台窗口（菜单本身已抢走焦点）。

- [ ] **Step 5: 编译与测试**

```powershell
cargo check --locked --all-targets --features record
cargo test --locked --lib tray::tests -- --test-threads=1
```

Expected: 编译无错误；托盘测试 PASS。

- [ ] **Step 6: 提交**

```powershell
git add src/app.rs src/main.rs src/tray.rs src/lib.rs src/clipboard_ui.rs
git commit -m "feat(clipboard): wire service and tray"
```

---

### Task 10: 设置标签页与图标

**Files:**
- Modify: `src/extras.rs:760-770`（SETTINGS_GROUPS）
- Modify: `src/extras.rs:835`（titles）
- Modify: `src/extras.rs:836`（标题索引上限）
- Modify: `src/extras.rs:845-860`（图标与内容分发）
- Create: `assets/settings-icons/system/clipboard.svg`
- Modify: `src/extras.rs` 测试与 `src/ui_tests.rs:3363`

- [ ] **Step 1: 写失败测试**

在 `src/extras.rs` 的测试模块中更新 `settings_groups_keep_every_page_reachable`：

```rust
        let groups = [
            "常规", "音乐与歌词", "专注与饮水", "专注与饮水", "待办与置物",
            "待办与置物", "通知、邮箱与日程", "通知、邮箱与日程",
            "通知、邮箱与日程", "关于", "页面管理", "AI 额度", "剪贴板",
        ];
```

并在 `src/ui_tests.rs` 中把设置页遍历从 `for tab in 0..12` 改为 `for tab in 0..13`。

- [ ] **Step 2: 运行测试确认失败**

```powershell
cargo test --locked --lib extras::tests::settings_groups_keep_every_page_reachable -- --test-threads=1
```

Expected: FAIL，索引 12 不属于任何分组。

- [ ] **Step 3: 添加图标资源**

创建 `assets/settings-icons/system/clipboard.svg`：

```svg
<svg xmlns="http://www.w3.org/2000/svg" width="32" height="32" viewBox="0 0 32 32"><defs><linearGradient id="bg" x2="0" y2="1"><stop stop-color="#6D65FF"/><stop offset="1" stop-color="#6D65FF" stop-opacity=".88"/></linearGradient></defs><rect x=".5" y=".5" width="31" height="31" rx="8" fill="url(#bg)"/><rect x="1" y="1" width="30" height="30" rx="7.5" fill="none" stroke="white" stroke-opacity=".12"/><g transform="translate(6 4)" fill="white"><rect x="4" y="3" width="12" height="18" rx="2" fill="none" stroke="white" stroke-width="1.8"/><rect x="8" y="1.5" width="4" height="3" rx="1"/><rect x="7" y="8" width="8" height="1.6" rx=".8"/><rect x="7" y="12" width="8" height="1.6" rx=".8"/><rect x="7" y="16" width="5" height="1.6" rx=".8"/></g></svg>
```

- [ ] **Step 4: 加入设置分组与标签**

将 `SETTINGS_GROUPS` 改为 9 项：

```rust
const SETTINGS_GROUPS: [(&str,usize,usize);9] = [
    ("常规",0,0), ("页面管理",10,10), ("音乐与歌词",1,1), ("专注与饮水",2,3),
    ("待办与置物",4,5), ("通知、邮箱与日程",6,8), ("AI 额度",11,11),
    ("剪贴板",12,12), ("关于",9,9),
];
```

将 `titles` 数组改为 13 项：

```rust
                let titles=["常规","音乐与歌词","专注","饮水","待办","置物","通知","邮箱","日程","关于","页面管理","AI 额度","剪贴板"];
```

将标题索引上限改为：

```rust
                ui.label(egui::RichText::new(titles[self.settings_tab.min(12)]).size(18.0).strong().color(settings_color(ctx,Color32::WHITE,Color32::from_rgb(30,34,41))));
```

在侧栏图标 `match start` 中加入：

```rust
                        12=>egui::include_image!("../assets/settings-icons/system/clipboard.svg"),
```

在内容分发 `match self.settings_tab` 中加入：

```rust
                    12=>self.clipboard_settings(ui),
```

- [ ] **Step 5: 实现设置页内容**

在 `impl App` 中加入：

```rust
    fn clipboard_settings(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        ui.label(
            egui::RichText::new("记录复制的文本，使用 Windows DPAPI 加密保存在本机。")
                .size(12.0)
                .color(settings_color(&ctx, Color32::from_gray(180), Color32::from_gray(75))),
        );
        settings_label(ui, "记录");
        settings_card(ui, |ui| {
            setting_row(
                ui,
                "启用剪贴板历史",
                "复制文本后自动记录；关闭后停止记录并保留已有历史",
                &mut self.settings.clipboard_enabled,
            );
            ui.separator();
            setting_row(
                ui,
                "暂停记录",
                "临时停止记录，适合处理敏感信息时使用",
                &mut self.settings.clipboard_paused,
            );
            ui.separator();
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("全局热键")
                        .size(12.0)
                        .color(settings_color(&ctx, Color32::WHITE, Color32::from_rgb(30, 34, 41))),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new(crate::native::CLIPBOARD_HOTKEY_TEXT)
                            .size(12.0)
                            .strong()
                            .color(settings_color(&ctx, Color32::from_gray(210), Color32::from_gray(55))),
                    );
                });
            });
            if !self.clipboard_status.is_empty() {
                ui.label(
                    egui::RichText::new(&self.clipboard_status)
                        .size(11.0)
                        .color(settings_color(&ctx, Color32::from_gray(180), Color32::from_gray(80))),
                );
            }
        });
        settings_label(ui, "容量");
        settings_card(ui, |ui| {
            setting_number(ui, "保留条数", &mut self.settings.clipboard_limit, 50, 500, " 条", 1.0);
            ui.separator();
            setting_number(ui, "自动过期", &mut self.settings.clipboard_days, 1, 30, " 天", 1.0);
        });
        settings_label(ui, "隐私与数据");
        settings_card(ui, |ui| {
            ui.label(
                egui::RichText::new("密码管理器通过系统剪贴板格式标记的内容不会记录。历史文件位于 %APPDATA%\\Daozai\\clipboard.bin，使用当前 Windows 用户的 DPAPI 加密。")
                    .size(11.5)
                    .color(settings_color(&ctx, Color32::from_gray(180), Color32::from_gray(75)))
                    .wrap(),
            );
            ui.separator();
            let id = ui.id().with("clipboard-clear");
            let now = ui.input(|input| input.time);
            let armed = ui.data(|data| data.get_temp::<f64>(id).unwrap_or(0.0));
            let label = if now - armed < 3.0 { "再次点击确认清空" } else { "清空剪贴板历史" };
            if ui
                .add_sized([150.0, 30.0], egui::Button::new(egui::RichText::new(label).size(12.0).color(theme::RED)))
                .clicked()
            {
                if now - armed < 3.0 {
                    self.pending.push(Cmd::ClipboardClear);
                    ui.data_mut(|data| data.remove::<f64>(id));
                } else {
                    ui.data_mut(|data| data.insert_temp(id, now));
                }
            }
        });
    }
```

注意：`crate::native::CLIPBOARD_HOTKEY_TEXT` 只在 Windows 编译；为跨平台编译，在 `native.rs` 的 `#[cfg(not(windows))]` 区域也定义同名常量：

```rust
#[cfg(not(windows))]
pub const CLIPBOARD_HOTKEY_TEXT: &str = "Ctrl+Alt+V";
```

- [ ] **Step 6: 运行测试**

```powershell
cargo test --locked --lib extras::tests::settings_groups_keep_every_page_reachable -- --test-threads=1
cargo test --locked --lib ui_tests::clipboard_settings_default_off_and_round_trip -- --test-threads=1
```

Expected: PASS。

- [ ] **Step 7: 提交**

```powershell
git add src/extras.rs src/ui_tests.rs assets/settings-icons/system/clipboard.svg
git commit -m "feat(settings): add clipboard tab"
```

---

### Task 11: 全量验证与文档

**Files:**
- Modify: `README.md`（功能表）
- Modify: `docs/USAGE.md`（使用说明）
- Modify: `CHANGELOG.md`（顶部新增「未发布」条目）

- [ ] **Step 1: 运行全量测试**

```powershell
cargo test --locked --all-targets --features record -- --test-threads=1
```

Expected: 全部 PASS。若 `ui_tests` 的设置页遍历仍为 `0..12`，改为 `0..13` 后重跑。

- [ ] **Step 2: 构建 release 验证**

```powershell
cargo build --locked --release --bin daozai
```

Expected: 构建成功。

- [ ] **Step 3: 手动验收**

在真实桌面上逐项验证：

1. 启动 `target\release\daozai.exe`
2. 在记事本复制一段文字，按 `Ctrl+Alt+V`，确认 1 秒内出现
3. 输入关键词搜索，`↑/↓` 选择，`Enter` 复制，回到记事本 `Ctrl+V`
4. 点击星标置顶，确认置顶项在顶部
5. 点击 `×` 删除，确认列表更新
6. 用 KeePass/1Password 复制密码，确认不出现
7. 重启应用，确认历史保留
8. 关闭设置里的「启用剪贴板历史」，按热键，确认出现一键开启提示
9. 打开浅色/深色设置，确认弹窗配色与设置窗口一致
10. 若热键被占用，确认设置页提示且托盘菜单可打开

- [ ] **Step 4: 更新文档**

在 `README.md` 功能表加入一行：

```markdown
| 剪贴板历史 | `Ctrl+Alt+V` 唤起，搜索、置顶和删除复制的文本；DPAPI 加密保存 |
```

在 `docs/USAGE.md` 加入简短小节：

```markdown
## 剪贴板历史

按 `Ctrl+Alt+V` 打开剪贴板历史，输入关键词过滤；`↑/↓` 选择，`Enter` 复制，`Esc` 关闭。置顶项不会被自动清理。历史默认关闭，首次使用时在弹窗内开启；内容使用 Windows DPAPI 加密保存在本机。
```

在 `CHANGELOG.md` 顶部加入：

```markdown
## 未发布

- 新增剪贴板历史：`Ctrl+Alt+V` 唤起，支持搜索、置顶、删除和 DPAPI 加密保存。
```

- [ ] **Step 5: 提交**

```powershell
git add README.md docs/USAGE.md CHANGELOG.md
git commit -m "docs: document clipboard history"
```

---

## 规格覆盖检查

| 规格章节 | 对应任务 |
| --- | --- |
| 目标/非目标 | Task 1、Task 8（仅文本、无自动粘贴） |
| 架构与数据流 | Task 5、Task 9 |
| 数据模型与存储 | Task 1、Task 2、Task 3 |
| 捕获与隐私过滤 | Task 4、Task 5 |
| 热键、窗口与焦点 | Task 6、Task 8 |
| UI 设计（颜色/字体/布局/交互） | Task 8、Task 10 |
| 设置与托盘 | Task 9、Task 10 |
| 错误处理 | Task 2、Task 4、Task 5、Task 9 |
| 测试计划 | 每个任务的测试步骤 + Task 11 |
| 文件改动 | 各任务 Files 列表 |
| 风险 | Task 8 的透明视口、Task 11 的手动验收 |

## 执行顺序说明

- Task 1–5 只依赖标准库与现有依赖，可以独立跑测试。
- Task 6–8 需要 Windows 环境；Linux 上只做 `cargo check`（`clipboard_ui` 的 `native` 调用需 `#[cfg(windows)]` 包裹，非 Windows 返回 `None`）。
- Task 9–10 依赖前面的模块，最后统一跑全量测试。
- 每个任务结束都提交一次，保持可回退。
