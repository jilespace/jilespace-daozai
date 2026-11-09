//! 剪贴板历史：纯数据逻辑与后台服务。

pub const MAX_ITEM_BYTES: usize = 32 * 1024;
pub const MAX_TOTAL_BYTES: usize = 1024 * 1024;
pub const DEFAULT_LIMIT: usize = 200;
pub const DEFAULT_DAYS: u32 = 7;
pub const HOTKEY_TEXT: &str = "Ctrl+Alt+V";

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
        Self {
            items: Vec::new(),
            limit: limit.max(1),
            days: days.max(1),
        }
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
            None => Item {
                id,
                text,
                copied_at: now,
                pinned: false,
            },
        };
        item.copied_at = now;
        self.items.insert(0, item);
        self.cleanup(now);
        true
    }

    pub fn remove(&mut self, id: &str) {
        self.items.retain(|item| item.id != id);
    }

    pub fn restore(&mut self, mut item: Item, now: i64) {
        if item.text.trim().is_empty() || item.text.len() > MAX_ITEM_BYTES {
            return;
        }
        item.id = id_for(&item.text);
        if !self.items.iter().any(|existing| existing.id == item.id) {
            self.items.push(item);
            self.order();
            self.cleanup(now);
        }
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
        self.items
            .retain(|item| item.pinned || item.copied_at >= cutoff);

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
            b.pinned
                .cmp(&a.pinned)
                .then_with(|| b.copied_at.cmp(&a.copied_at))
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

/// 去掉常见营销/分享跟踪参数；只处理「整段就是一条链接」的文本，其它内容原样返回。
pub fn clean_tracking(text: &str) -> String {
    let trimmed = text.trim();
    if !(trimmed.starts_with("http://") || trimmed.starts_with("https://"))
        || trimmed.chars().any(char::is_whitespace)
    {
        return text.to_string();
    }
    let (before_fragment, fragment) = match trimmed.split_once('#') {
        Some((before, fragment)) => (before, Some(fragment)),
        None => (trimmed, None),
    };
    let Some((base, query)) = before_fragment.split_once('?') else {
        return text.to_string();
    };
    let pairs: Vec<&str> = query.split('&').filter(|pair| !pair.is_empty()).collect();
    let kept: Vec<&str> = pairs
        .iter()
        .copied()
        .filter(|pair| !is_tracking_param(pair.split('=').next().unwrap_or("")))
        .collect();
    if kept.len() == pairs.len() {
        return text.to_string();
    }
    let mut out = base.to_string();
    if !kept.is_empty() {
        out.push('?');
        out.push_str(&kept.join("&"));
    }
    if let Some(fragment) = fragment {
        out.push('#');
        out.push_str(fragment);
    }
    out
}

fn is_tracking_param(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    key.starts_with("utm_")
        || key.starts_with("share_")
        || matches!(
            key.as_str(),
            "fbclid"
                | "gclid"
                | "dclid"
                | "msclkid"
                | "yclid"
                | "igshid"
                | "mc_cid"
                | "mc_eid"
                | "_hsenc"
                | "_hsmi"
                | "spm"
                | "scm"
                | "si"
                | "vero_id"
                | "oly_enc_id"
                | "oly_anon_id"
                | "wk_via"
        )
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
        System::DataExchange::GetClipboardOwner, System::Threading::GetCurrentProcessId,
        UI::WindowsAndMessaging::GetWindowThreadProcessId,
    };
    unsafe {
        let Ok(owner) = GetClipboardOwner() else {
            return false;
        };
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
        .and_then(|bytes| {
            String::from_utf8(bytes).map_err(|_| "剪贴板历史不是有效文本".to_string())
        })
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
        Foundation::{LocalFree, HLOCAL},
        Security::Cryptography::{CryptProtectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB},
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
        let bytes = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        let _ = LocalFree(HLOCAL(output.pbData.cast()));
        Ok(bytes)
    }
}

#[cfg(windows)]
pub fn unprotect(data: &[u8]) -> Result<Vec<u8>, String> {
    use windows::Win32::{
        Foundation::{LocalFree, HLOCAL},
        Security::Cryptography::{
            CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
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
        let bytes = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
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

use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::{Duration, Instant};

pub enum Cmd {
    SetEnabled(bool),
    SetLimits(usize, u32),
    Copy(String),
    Delete(String),
    Restore(Item),
    Pin(String, bool),
    /// 外部识别/生成的结果直接插入历史（不写系统剪贴板）。
    Insert(String),
    SetCleanLinks(bool),
    Clear,
    Refresh,
}

#[derive(Clone, Debug)]
pub enum Event {
    Changed(std::sync::Arc<Vec<Item>>),
    Copied(String),
    CopyFailed(String, String),
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
        Self {
            cmd: cmd_tx,
            event: event_rx,
        }
    }

    pub fn set_enabled(&self, enabled: bool) {
        let _ = self.cmd.send(Cmd::SetEnabled(enabled));
    }

    pub fn set_limits(&self, limit: usize, days: u32) {
        let _ = self.cmd.send(Cmd::SetLimits(limit, days));
    }

    pub fn set_clean_links(&self, clean: bool) {
        let _ = self.cmd.send(Cmd::SetCleanLinks(clean));
    }

    pub fn copy(&self, id: &str) {
        let _ = self.cmd.send(Cmd::Copy(id.into()));
    }

    pub fn delete(&self, id: &str) {
        let _ = self.cmd.send(Cmd::Delete(id.into()));
    }

    pub fn restore(&self, item: Item) {
        let _ = self.cmd.send(Cmd::Restore(item));
    }

    pub fn pin(&self, id: &str, pinned: bool) {
        let _ = self.cmd.send(Cmd::Pin(id.into(), pinned));
    }

    /// 把一段外部结果（例如 OCR 文字）加入历史。
    pub fn insert(&self, text: String) {
        let _ = self.cmd.send(Cmd::Insert(text));
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

fn supervisor(
    capture: std::sync::Arc<dyn Capture>,
    commands: Receiver<Cmd>,
    events: Sender<Event>,
) {
    let (mut history, load_status) = load(DEFAULT_LIMIT, DEFAULT_DAYS, now_seconds());
    if let Some(status) = load_status {
        let _ = events.send(Event::Status(status));
    }
    let _ = events.send(Event::Changed(std::sync::Arc::new(
        history.items().to_vec(),
    )));
    let mut enabled = false;
    let mut clean_links = true;
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
            Ok(Cmd::SetCleanLinks(clean)) => {
                clean_links = clean;
            }
            Ok(Cmd::SetLimits(limit, days)) => {
                history.set_limits(limit, days, now_seconds());
                if let Err(error) = save(&history) {
                    let _ = events.send(Event::Status(error));
                }
                let _ = events.send(Event::Changed(std::sync::Arc::new(
                    history.items().to_vec(),
                )));
            }
            Ok(Cmd::Copy(id)) => {
                if let Some(text) = history
                    .items()
                    .iter()
                    .find(|item| item.id == id)
                    .map(|item| item.text.clone())
                {
                    if capture.write(&text) {
                        last_sequence = capture.sequence();
                        let _ = events.send(Event::Copied(id));
                    } else {
                        let _ = events.send(Event::CopyFailed(id, "剪贴板正被占用，请重试".into()));
                    }
                } else {
                    let _ =
                        events.send(Event::CopyFailed(id, "这条记录已被移除，请重新选择".into()));
                }
            }
            Ok(Cmd::Restore(item)) => {
                history.restore(item, now_seconds());
                if let Err(error) = save(&history) {
                    let _ = events.send(Event::Status(error));
                }
                let _ = events.send(Event::Changed(std::sync::Arc::new(
                    history.items().to_vec(),
                )));
            }
            Ok(Cmd::Delete(id)) => {
                history.remove(&id);
                if let Err(error) = save(&history) {
                    let _ = events.send(Event::Status(error));
                }
                let _ = events.send(Event::Changed(std::sync::Arc::new(
                    history.items().to_vec(),
                )));
            }
            Ok(Cmd::Pin(id, pinned)) => {
                history.set_pinned(&id, pinned);
                if let Err(error) = save(&history) {
                    let _ = events.send(Event::Status(error));
                }
                let _ = events.send(Event::Changed(std::sync::Arc::new(
                    history.items().to_vec(),
                )));
            }
            Ok(Cmd::Insert(text)) => {
                if history.insert(text, now_seconds()) {
                    if let Err(error) = save(&history) {
                        let _ = events.send(Event::Status(error));
                    }
                }
                let _ = events.send(Event::Changed(std::sync::Arc::new(history.items().to_vec())));
            }
            Ok(Cmd::Clear) => {
                history.clear();
                if let Err(error) = save(&history) {
                    let _ = events.send(Event::Status(error));
                }
                let _ = events.send(Event::Changed(std::sync::Arc::new(
                    history.items().to_vec(),
                )));
            }
            Ok(Cmd::Refresh) => {
                let _ = events.send(Event::Changed(std::sync::Arc::new(
                    history.items().to_vec(),
                )));
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
                if let Some(mut text) = capture.read() {
                    if clean_links {
                        text = clean_tracking(&text);
                    }
                    let raw = RawClipboard {
                        text,
                        excluded: false,
                        own_process: capture.own_process(),
                    };
                    if accepts(&raw) && history.insert(raw.text, now_seconds()) {
                        if let Err(error) = save(&history) {
                            let _ = events.send(Event::Status(error));
                        }
                        let _ = events.send(Event::Changed(std::sync::Arc::new(
                            history.items().to_vec(),
                        )));
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
            let _ = events.send(Event::Changed(std::sync::Arc::new(
                history.items().to_vec(),
            )));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find<'a>(history: &'a History, text: &str) -> Option<&'a Item> {
        history.items().iter().find(|item| item.text == text)
    }

    #[test]
    fn clean_tracking_strips_marketing_parameters() {
        assert_eq!(
            clean_tracking("https://example.com/a?utm_source=x&id=42&fbclid=abc#top"),
            "https://example.com/a?id=42#top"
        );
        // 全部都是跟踪参数时连问号一起去掉。
        assert_eq!(clean_tracking("https://example.com?utm_medium=mail&spm=a1"), "https://example.com");
        // 不是链接、或链接里带空格时保持原样。
        let plain = "https://example.com/a?utm_source=x 这里有空格";
        assert_eq!(clean_tracking(plain), plain);
        assert_eq!(clean_tracking("普通文本 utm_source=x"), "普通文本 utm_source=x");
        assert_eq!(clean_tracking("https://example.com/a?id=1"), "https://example.com/a?id=1");
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
    fn restoring_deleted_item_preserves_text_timestamp_and_pin() {
        let mut history = History::new(10, 7);
        history.insert("keep original".into(), 1000);
        let id = id_for("keep original");
        history.set_pinned(&id, true);
        let deleted = history.items()[0].clone();
        history.remove(&id);
        history.restore(deleted.clone(), 2000);
        assert_eq!(history.items(), &[deleted]);
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
        let normal = RawClipboard {
            text: "hello".into(),
            excluded: false,
            own_process: false,
        };
        assert!(accepts(&normal));
        assert!(!accepts(&RawClipboard {
            text: "hello".into(),
            excluded: true,
            own_process: false
        }));
        assert!(!accepts(&RawClipboard {
            text: "hello".into(),
            excluded: false,
            own_process: true
        }));
        assert!(!accepts(&RawClipboard {
            text: "   ".into(),
            excluded: false,
            own_process: false
        }));
        assert!(!accepts(&RawClipboard {
            text: "x".repeat(MAX_ITEM_BYTES + 1),
            excluded: false,
            own_process: false,
        }));
    }

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
        service.set_enabled(true);

        let initial = next_event(&service, Duration::from_secs(1));
        assert!(matches!(initial, Event::Changed(items) if items.is_empty()));

        capture.set("hello clipboard");
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match next_event(&service, Duration::from_millis(700)) {
                Event::Changed(items)
                    if items.iter().any(|item| item.text == "hello clipboard") =>
                {
                    break
                }
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
        service.set_enabled(true);
        let _ = next_event(&service, Duration::from_secs(1));

        capture.set("from another app");
        let deadline = Instant::now() + Duration::from_secs(2);
        let item = loop {
            if let Event::Changed(items) = next_event(&service, Duration::from_millis(700)) {
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
            match next_event(&service, Duration::from_millis(700)) {
                Event::Copied(id) if id == item.id => break,
                _ if Instant::now() > deadline => panic!("复制命令没有回执"),
                _ => {}
            }
        }
        assert_eq!(
            capture.written.lock().unwrap().as_slice(),
            ["from another app"]
        );
    }

    #[test]
    fn insert_command_adds_external_text_to_history() {
        let _dir = test_history_dir();
        let capture = Arc::new(FakeCapture::new());
        let service = Service::start_with(capture.clone());
        service.set_enabled(true);
        let _ = next_event(&service, Duration::from_secs(1));
        service.insert("OCR 识别出来的一段文字".into());
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            if let Event::Changed(items) = next_event(&service, Duration::from_millis(600)) {
                if items.iter().any(|item| item.text == "OCR 识别出来的一段文字") {
                    break;
                }
            }
            assert!(Instant::now() < deadline, "外部插入的文本应出现在历史里");
        }
    }

    #[test]
    fn delete_pin_and_clear_update_the_snapshot() {
        let _dir = test_history_dir();
        let capture = Arc::new(FakeCapture::new());
        let service = Service::start_with(capture.clone());
        service.set_enabled(true);
        let _ = next_event(&service, Duration::from_secs(1));

        capture.set("keep me");
        let deadline = Instant::now() + Duration::from_secs(2);
        let item = loop {
            if let Event::Changed(items) = next_event(&service, Duration::from_millis(700)) {
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
            if let Event::Changed(items) = next_event(&service, Duration::from_millis(700)) {
                if items
                    .first()
                    .is_some_and(|entry| entry.id == wanted && entry.pinned)
                {
                    break;
                }
            }
            assert!(Instant::now() < deadline, "置顶状态没有更新");
        }

        service.delete(&wanted);
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            if let Event::Changed(items) = next_event(&service, Duration::from_millis(700)) {
                if items.is_empty() {
                    break;
                }
            }
            assert!(Instant::now() < deadline, "删除后历史没有清空");
        }
    }
}
