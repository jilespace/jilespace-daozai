//! 命令面板：`Ctrl+Alt+K` 打开，键盘优先地跳转页面、设置、启动应用，
//! 并顺手完成计算与单位换算。
//!
//! 面板与剪贴板共用 `panel` 里的视觉令牌：石墨浮层、轻描边、柔和阴影、
//! 跟随系统深浅色。数据侧刻意保持纯函数（应用扫描、匹配排序、计算、换算），
//! 这样可以在无窗口环境里完整回归。

use crate::panel::{
    self, hint, icon_button, keycap, mix, with_alpha, Palette,
};
use crate::theme::{self, W};
use eframe::egui::{
    self, Align, Color32, Key, Layout, Modifiers, Pos2, Rect, RichText, Sense, Stroke, Ui,
    UiBuilder, Vec2,
};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const WIDTH: f32 = 560.0;
const HEIGHT: f32 = 468.0;
const PAD: f32 = 10.0;
const INNER: f32 = 16.0;
const FOOTER_H: f32 = 40.0;
const ROW_H: f32 = 48.0;
const FADE_SECS: f64 = 0.13;
const MAX_RESULTS: usize = 40;
const MAX_APPS: usize = 400;
const APP_CACHE: Duration = Duration::from_secs(300);
pub const HOTKEY_TEXT: &str = "Ctrl+Alt+K";

/// 命令面板交回外壳执行的副作用。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    None,
    Close,
    OpenSettings(usize),
    OpenPage(usize),
    OpenCalendar,
    OpenClipboard,
    LaunchApp(PathBuf),
    CopyText(String),
    SetClipboardPaused(bool),
    SetClipboardEnabled(bool),
    ToggleFocus,
    OcrClipboard,
    CropOcr,
    StartCountdown(u32),
    ToggleTimer,
    ResetTimer,
    SetStayAwake(bool),
    RefreshWeather,
    OpenNotifications,
    ScreenshotRegion,
    CopyClipboard(String),
    ClosePins,
    ScrollCapture,
    VolumeUp,
    VolumeDown,
    VolumeMute,
    MediaPlayPause,
    MediaNext,
    MediaPrev,
    BrightnessUp,
    BrightnessDown,
}

/// 打开面板时由岛体提供的上下文。
#[derive(Clone, Default)]
pub struct Snapshot {
    pub pages: Vec<PageInfo>,
    pub clipboard_enabled: bool,
    pub clipboard_paused: bool,
    pub focus_running: bool,
    pub timer_running: bool,
    pub timer_remaining: f64,
    pub stay_awake: bool,
    pub weather_enabled: bool,
    /// 当前桌面贴图数量，用于「关闭所有贴图」。
    pub pins: usize,
    pub media_title: String,
    pub media_source: String,
    pub media_playing: bool,
}

#[derive(Clone)]
pub struct PageInfo {
    pub index: usize,
    pub name: &'static str,
    pub description: &'static str,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Section {
    Recent,
    Action,
    Clipboard,
    Page,
    Setting,
    App,
    Calc,
}

impl Section {
    fn label(self) -> &'static str {
        match self {
            Self::Recent => "最近",
            Self::Action => "操作",
            Self::Clipboard => "剪贴板",
            Self::Page => "页面",
            Self::Setting => "设置",
            Self::App => "应用",
            Self::Calc => "计算",
        }
    }

    fn order(self) -> usize {
        match self {
            Self::Calc => 0,
            Self::Recent => 1,
            Self::Action => 2,
            Self::Clipboard => 3,
            Self::Page => 4,
            Self::Setting => 5,
            Self::App => 6,
        }
    }
}

#[derive(Clone, Copy)]
enum Glyph {
    Clipboard,
    Timer,
    Calendar,
    Info,
    Page,
    Gear,
    App,
    Calc,
    Scan,
}

#[derive(Clone)]
struct Entry {
    id: String,
    section: Section,
    title: String,
    subtitle: String,
    haystack: String,
    action: Action,
    glyph: Glyph,
}

#[derive(Clone)]
struct AppEntry {
    name: String,
    folder: String,
    path: PathBuf,
}

/// 最近从命令面板启动的应用（持久化，最多 12 条，展示前 5 条）。
#[derive(Clone, Debug, Default)]
pub struct RecentApps {
    items: Vec<(String, std::path::PathBuf)>,
}

impl RecentApps {
    pub fn load() -> Self {
        let text = match std::fs::read_to_string(Self::path()) {
            Ok(text) => text,
            Err(_) => return Self::default(),
        };
        let value: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
        let items = value
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|item| {
                let name = item.get("name")?.as_str()?.to_string();
                let path = std::path::PathBuf::from(item.get("path")?.as_str()?);
                Some((name, path))
            })
            .take(12)
            .collect();
        Self { items }
    }

    pub fn items(&self) -> Vec<(String, std::path::PathBuf)> {
        self.items.iter().take(5).cloned().collect()
    }

    pub fn record(&mut self, name: &str, path: &std::path::Path) {
        let name = name.trim();
        if name.is_empty() {
            return;
        }
        self.items.retain(|(_, existing)| existing != path);
        self.items.insert(0, (name.to_string(), path.to_path_buf()));
        self.items.truncate(12);
        self.save();
    }

    fn save(&self) {
        let value: Vec<serde_json::Value> = self
            .items
            .iter()
            .map(|(name, path)| serde_json::json!({"name": name, "path": path.to_string_lossy()}))
            .collect();
        if let Ok(text) = serde_json::to_string(&value) {
            let _ = crate::model::atomic_write(&Self::path(), text.as_bytes());
        }
    }

    fn path() -> std::path::PathBuf {
        crate::model::config_dir().join("recent-apps.json")
    }
}

/// 命令面板里可搜索的剪贴板条目（只带预览，不复制整段大文本）。
#[derive(Clone, Debug)]
pub struct ClipboardItem {
    pub id: String,
    pub text: String,
}

pub struct Panel {
    pub open: bool,
    query: String,
    selected: usize,
    selected_id: Option<String>,
    base: Vec<Entry>,
    snapshot: Snapshot,
    clipboard: Vec<ClipboardItem>,
    recent: Vec<(String, std::path::PathBuf)>,
    results: Vec<Entry>,
    apps: Vec<AppEntry>,
    apps_at: Option<Instant>,
    hotkey_error: Option<String>,
    position: Option<Pos2>,
    request_focus: bool,
    search_focused: bool,
    scroll_to_selected: bool,
    shown: bool,
    shown_at: Option<f64>,
    had_focus: bool,
    focus_lost_at: Option<Instant>,
    fade: f32,
}

impl Default for Panel {
    fn default() -> Self {
        Self {
            open: false,
            query: String::new(),
            selected: 0,
            selected_id: None,
            base: Vec::new(),
            snapshot: Snapshot::default(),
            clipboard: Vec::new(),
            recent: Vec::new(),
            results: Vec::new(),
            apps: Vec::new(),
            apps_at: None,
            hotkey_error: None,
            position: None,
            request_focus: false,
            search_focused: false,
            scroll_to_selected: false,
            shown: false,
            shown_at: None,
            had_focus: false,
            focus_lost_at: None,
            fade: 0.0,
        }
    }
}

impl Panel {
    /// 最近启动的应用；空查询时排在最前。
    pub fn set_recent(&mut self, recent: Vec<(String, std::path::PathBuf)>) {
        self.recent = recent.into_iter().take(5).collect();
    }

    /// 更新命令面板可搜索的剪贴板条目；内容没变时不重建。
    pub fn set_clipboard(&mut self, items: Vec<ClipboardItem>) {
        if self.clipboard.len() == items.len()
            && self.clipboard.iter().zip(items.iter()).all(|(old, new)| old.id == new.id && old.text == new.text)
        {
            return;
        }
        self.clipboard = items;
        if self.open {
            let snapshot = self.snapshot.clone();
            self.base = self.build_base(&snapshot);
            self.rebuild();
        }
    }

    pub fn open(&mut self, snapshot: &Snapshot) {
        self.open = true;
        self.query.clear();
        self.selected = 0;
        self.selected_id = None;
        self.snapshot = snapshot.clone();
        let snapshot = snapshot.clone();
        self.base = self.build_base(&snapshot);
        self.rebuild();
        self.position = None;
        self.request_focus = true;
        self.search_focused = false;
        self.scroll_to_selected = true;
        self.shown = false;
        self.shown_at = None;
        self.had_focus = false;
        self.focus_lost_at = None;
        self.fade = 0.0;
    }

    pub fn toggle(&mut self, snapshot: &Snapshot) {
        if self.open {
            self.close();
        } else {
            self.open(snapshot);
        }
    }

    pub fn close(&mut self) {
        self.open = false;
        self.shown = false;
        self.shown_at = None;
        self.position = None;
        // The application catalog stays cached, but the query-specific copies
        // (including clipboard snippets) are rebuilt on the next open.
        self.base = Vec::new();
        self.results = Vec::new();
        self.clipboard = Vec::new();
        self.query = String::new();
        self.selected_id = None;
    }

    pub fn set_hotkey_error(&mut self, error: Option<String>) {
        self.hotkey_error = error.filter(|message| !message.trim().is_empty());
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    fn ensure_apps(&mut self) {
        if self.apps_at.is_some_and(|at| at.elapsed() < APP_CACHE) {
            return;
        }
        self.apps = scan_apps(&default_app_roots());
        self.apps_at = Some(Instant::now());
    }

    fn build_base(&mut self, snapshot: &Snapshot) -> Vec<Entry> {
        self.ensure_apps();
        let mut entries = Vec::new();
        let action = |id: &str, title: &str, subtitle: &str, keywords: &str, glyph, action| Entry {
            id: id.into(),
            section: Section::Action,
            title: title.into(),
            subtitle: subtitle.into(),
            haystack: format!("{title} {subtitle} {keywords}").to_lowercase(),
            action,
            glyph,
        };
        entries.push(action(
            "action-clipboard-open",
            "打开剪贴板历史",
            "搜索、预览并粘贴最近复制的文本",
            "剪贴板 复制 粘贴 clipboard paste",
            Glyph::Clipboard,
            Action::OpenClipboard,
        ));
        if !snapshot.clipboard_enabled {
            entries.push(action(
                "action-clipboard-enable",
                "开启剪贴板历史",
                "复制过的文本会用 DPAPI 加密保存在本机",
                "剪贴板 开启 记录 clipboard enable",
                Glyph::Clipboard,
                Action::SetClipboardEnabled(true),
            ));
        } else if snapshot.clipboard_paused {
            entries.push(action(
                "action-clipboard-resume",
                "恢复剪贴板记录",
                "继续记录新复制的文本",
                "剪贴板 恢复 记录 clipboard resume",
                Glyph::Clipboard,
                Action::SetClipboardPaused(false),
            ));
        } else {
            entries.push(action(
                "action-clipboard-pause",
                "暂停剪贴板记录",
                "暂时不记录新复制的文本",
                "剪贴板 暂停 记录 clipboard pause",
                Glyph::Clipboard,
                Action::SetClipboardPaused(true),
            ));
        }
        if snapshot.focus_running {
            entries.push(action(
                "action-focus-pause",
                "暂停专注计时",
                "当前专注计时会停在剩余时间",
                "专注 番茄钟 暂停 focus pomodoro pause",
                Glyph::Timer,
                Action::ToggleFocus,
            ));
        } else {
            entries.push(action(
                "action-focus-start",
                "开始专注计时",
                "进入专注页并开始番茄钟",
                "专注 番茄钟 开始 focus pomodoro start",
                Glyph::Timer,
                Action::ToggleFocus,
            ));
        }
        entries.push(action(
            "action-ocr",
            "识别剪贴板图片文字",
            "读取剪贴板里的图片，用 Windows 本地 OCR 识别文字",
            "OCR 图片 文字 识别 截图 提取 image text extract clipboard",
            Glyph::Scan,
            Action::OcrClipboard,
        ));
        entries.push(action(
            "action-crop",
            "截屏框选取字",
            "框选屏幕上的任意区域，本地 OCR 识别文字",
            "截屏 框选 取字 截图 OCR 图片 识别 screenshot crop region",
            Glyph::Scan,
            Action::CropOcr,
        ));
        if snapshot.timer_running {
            entries.push(action(
                "action-timer-toggle",
                "暂停计时",
                &format!("当前剩余 {}", crate::world_clock::clock_label(snapshot.timer_remaining)),
                "计时 倒计时 秒表 暂停 timer pause",
                Glyph::Timer,
                Action::ToggleTimer,
            ));
            entries.push(action(
                "action-timer-reset",
                "重置计时",
                "把计时器恢复到初始状态",
                "计时 倒计时 秒表 重置 timer reset",
                Glyph::Timer,
                Action::ResetTimer,
            ));
        } else {
            for minutes in [5u32, 25] {
                entries.push(action(
                    &format!("action-timer-{minutes}"),
                    &format!("开始 {minutes} 分钟倒计时"),
                    "在岛上开始倒计时，收起时药丸时钟会显示剩余时间",
                    "计时 倒计时 分钟 timer countdown",
                    Glyph::Timer,
                    Action::StartCountdown(minutes),
                ));
            }
        }
        if snapshot.stay_awake {
            entries.push(action(
                "action-stay-awake-off",
                "关闭防睡眠",
                "恢复系统默认的睡眠与息屏策略",
                "防睡眠 休眠 息屏 caffeine stay awake sleep",
                Glyph::Gear,
                Action::SetStayAwake(false),
            ));
        } else {
            entries.push(action(
                "action-stay-awake-on",
                "开启防睡眠",
                "演示、下载或长时间任务期间保持唤醒",
                "防睡眠 休眠 息屏 caffeine stay awake sleep",
                Glyph::Gear,
                Action::SetStayAwake(true),
            ));
        }
        if snapshot.weather_enabled {
            entries.push(action(
                "action-weather-refresh",
                "刷新天气",
                "重新获取当前城市天气",
                "天气 weather forecast refresh 刷新",
                Glyph::Gear,
                Action::RefreshWeather,
            ));
        }
        for (id, title, subtitle, keywords, palette_action) in [
            (
                "action-volume-up",
                "音量 +5%",
                "调高系统默认输出音量",
                "音量 声音 volume up 调高",
                Action::VolumeUp,
            ),
            (
                "action-volume-down",
                "音量 −5%",
                "调低系统默认输出音量",
                "音量 声音 volume down 调低",
                Action::VolumeDown,
            ),
            (
                "action-volume-mute",
                "静音 / 取消静音",
                "切换系统默认输出设备的静音状态",
                "静音 声音 mute volume",
                Action::VolumeMute,
            ),
        ] {
            entries.push(action(id, title, subtitle, keywords, Glyph::Gear, palette_action));
        }
        let media_label = if snapshot.media_title.trim().is_empty() {
            "没有正在播放的媒体".to_string()
        } else if snapshot.media_source.trim().is_empty() {
            snapshot.media_title.clone()
        } else {
            format!("{} · {}", snapshot.media_source.trim(), snapshot.media_title.trim())
        };
        for (id, title, subtitle, keywords, palette_action) in [
            (
                "action-brightness-up",
                "亮度 +10%",
                "调高鼠标所在显示器的亮度",
                "亮度 屏幕 brightness up 调高",
                Action::BrightnessUp,
            ),
            (
                "action-brightness-down",
                "亮度 −10%",
                "调低鼠标所在显示器的亮度",
                "亮度 屏幕 brightness down 调低",
                Action::BrightnessDown,
            ),
        ] {
            entries.push(action(id, title, subtitle, keywords, Glyph::Gear, palette_action));
        }
        entries.push(action(
            "action-media-play",
            if snapshot.media_playing { "暂停播放" } else { "继续播放" },
            &media_label,
            "播放 暂停 继续 媒体 media play pause",
            Glyph::Gear,
            Action::MediaPlayPause,
        ));
        entries.push(action(
            "action-media-next",
            "下一首",
            &media_label,
            "下一首 切歌 媒体 media next",
            Glyph::Gear,
            Action::MediaNext,
        ));
        entries.push(action(
            "action-media-prev",
            "上一首",
            &media_label,
            "上一首 切歌 媒体 media previous",
            Glyph::Gear,
            Action::MediaPrev,
        ));
        if snapshot.pins > 0 {
            entries.push(action(
                "action-close-pins",
                "关闭所有贴图",
                &format!("当前桌面上有 {} 张贴图", snapshot.pins),
                "贴图 关闭 pin close",
                Glyph::Gear,
                Action::ClosePins,
            ));
        }
        entries.push(action(
            "action-scroll-capture",
            "滚动截图…",
            "框选区域后滚动页面，自动拼接成长图",
            "滚动截图 长截图 scroll capture long screenshot",
            Glyph::Gear,
            Action::ScrollCapture,
        ));
        entries.push(action(
            "action-screenshot",
            "截屏保存…",
            "框选屏幕区域，复制图片并保存到图片目录",
            "截屏 截图 保存 screenshot capture save",
            Glyph::Gear,
            Action::ScreenshotRegion,
        ));
        entries.push(action(
            "action-notifications",
            "通知历史",
            "查看最近的 Windows 通知与提醒",
            "通知 历史 notification history alert",
            Glyph::Gear,
            Action::OpenNotifications,
        ));
        entries.push(action(
            "action-calendar",
            "打开日程",
            "月历、邮件邀请与提醒",
            "日程 日历 会议 calendar meeting",
            Glyph::Calendar,
            Action::OpenCalendar,
        ));
        entries.push(action(
            "action-settings",
            "打开设置",
            "常规、外观、页面与各功能配置",
            "设置 配置 选项 settings preferences",
            Glyph::Gear,
            Action::OpenSettings(0),
        ));
        entries.push(action(
            "action-about",
            "关于岛仔与检查更新",
            "版本、更新说明与开源声明",
            "关于 更新 版本 about update version",
            Glyph::Info,
            Action::OpenSettings(9),
        ));
        for page in &snapshot.pages {
            let title = format!("前往页面 · {}", page.name);
            let keywords = page_english_keywords(page.index);
            entries.push(Entry {
                id: format!("page-{}", page.index),
                section: Section::Page,
                title: title.clone(),
                subtitle: page.description.to_string(),
                haystack: format!("{title} {} {keywords}", page.description).to_lowercase(),
                action: Action::OpenPage(page.index),
                glyph: Glyph::Page,
            });
        }
        for (tab, title, subtitle, keywords) in SETTINGS_TABS {
            let label = format!("设置 · {title}");
            entries.push(Entry {
                id: format!("setting-{tab}"),
                section: Section::Setting,
                title: label.clone(),
                subtitle: (*subtitle).to_string(),
                haystack: format!("{label} {subtitle} {keywords}").to_lowercase(),
                action: Action::OpenSettings(*tab),
                glyph: Glyph::Gear,
            });
        }
        for (name, path, keywords) in BUILTIN_APPS {
            let label = format!("启动 · {name}");
            entries.push(Entry {
                id: format!("builtin-{path}"),
                section: Section::App,
                title: label,
                subtitle: "Windows 系统工具".into(),
                haystack: format!("{name} {path} {keywords} 系统工具 app launch").to_lowercase(),
                action: Action::LaunchApp(PathBuf::from(path)),
                glyph: Glyph::App,
            });
        }
        for app in &self.apps {
            entries.push(Entry {
                id: format!("app-{}", app.path.display()),
                section: Section::App,
                title: app.name.clone(),
                subtitle: if app.folder.is_empty() { "开始菜单应用".into() } else { app.folder.clone() },
                haystack: format!("{} {} 应用 启动 app launch", app.name, app.folder).to_lowercase(),
                action: Action::LaunchApp(app.path.clone()),
                glyph: Glyph::App,
            });
        }
        for (name, path) in self.recent.iter() {
            entries.push(Entry {
                id: format!("recent-{}", path.display()),
                section: Section::Recent,
                title: name.clone(),
                subtitle: "最近启动 · Enter 打开".into(),
                haystack: format!("{name} {} 最近 recent app launch", path.display()).to_lowercase(),
                action: Action::LaunchApp(path.clone()),
                glyph: Glyph::App,
            });
        }
        for item in self.clipboard.iter().take(30) {
            let one_line = item.text.lines().next().unwrap_or("").trim();
            let title = if one_line.chars().count() > 68 {
                format!("{}…", one_line.chars().take(68).collect::<String>())
            } else if one_line.is_empty() {
                "（空白）".to_string()
            } else {
                one_line.to_string()
            };
            entries.push(Entry {
                id: format!("clipboard:{}", item.id),
                section: Section::Clipboard,
                title,
                subtitle: "剪贴板 · Enter 复制到剪贴板".into(),
                haystack: item.text.chars().take(4000).collect::<String>().to_lowercase(),
                action: Action::CopyClipboard(item.id.clone()),
                glyph: Glyph::Clipboard,
            });
        }
        entries
    }

    fn rebuild(&mut self) {
        let needle = self.query.trim().to_lowercase();
        let mut matches: Vec<(u16, usize, usize, Entry)> = Vec::new();
        for (index, entry) in self.base.iter().enumerate() {
            if needle.is_empty() && matches!(entry.section, Section::App | Section::Clipboard) {
                continue;
            }
            if !needle.is_empty() && entry.section == Section::Recent {
                continue;
            }
            if let Some(score) = match_score(entry, &needle) {
                matches.push((score, entry.section.order(), index, entry.clone()));
            }
        }
        matches.sort_by(|a, b| (a.0, a.1, a.2).cmp(&(b.0, b.1, b.2)));
        let mut results: Vec<Entry> = matches.into_iter().map(|(_, _, _, entry)| entry).collect();
        results.truncate(MAX_RESULTS);
        let calc = calculations(&self.query);
        if !calc.is_empty() {
            let mut combined = calc;
            combined.extend(results);
            combined.truncate(MAX_RESULTS);
            results = combined;
        }
        self.results = results;
        self.sync_selection();
    }

    fn sync_selection(&mut self) {
        if self.results.is_empty() {
            self.selected = 0;
            self.selected_id = None;
            return;
        }
        if let Some(id) = self.selected_id.as_ref() {
            if let Some(position) = self.results.iter().position(|entry| &entry.id == id) {
                self.selected = position;
                return;
            }
        }
        self.selected = self.selected.min(self.results.len() - 1);
        self.selected_id = Some(self.results[self.selected].id.clone());
    }

    fn move_selection(&mut self, delta: isize) {
        if self.results.is_empty() {
            return;
        }
        let next = (self.selected as isize + delta).clamp(0, self.results.len() as isize - 1) as usize;
        self.selected = next;
        self.selected_id = Some(self.results[next].id.clone());
        self.scroll_to_selected = true;
    }

    fn selected_action(&self) -> Action {
        self.results
            .get(self.selected)
            .map(|entry| entry.action.clone())
            .unwrap_or(Action::None)
    }

    #[cfg(test)]
    fn selected_id(&self) -> Option<String> {
        self.selected_id.clone()
    }

    pub fn show(&mut self, ctx: &egui::Context, dark: bool) -> Action {
        if !self.open {
            return Action::None;
        }
        let viewport_id = egui::ViewportId::from_hash_of("daozai-command-palette");
        let ppp = ctx.pixels_per_point();
        let (size, min_size) = crate::panel::fitted_window_ctx(ctx, egui::vec2(WIDTH, HEIGHT), egui::vec2(440.0, 340.0));
        #[cfg(windows)]
        let initial_position = crate::native::clipboard_popup_position(size.x, size.y, ppp)
            .map(|(x, y)| egui::pos2(x, y))
            .unwrap_or_else(|| egui::pos2(160.0, 96.0));
        #[cfg(not(windows))]
        let initial_position = {
            let _ = ppp;
            egui::pos2(160.0, 96.0)
        };
        let mut action = Action::None;
        let position = *self.position.get_or_insert(initial_position);
        let builder = egui::ViewportBuilder::default()
            .with_title("命令面板 · 岛仔")
            .with_icon(theme::app_icon())
            .with_inner_size([size.x, size.y])
            .with_min_inner_size([min_size.x, min_size.y])
            .with_decorations(false)
            .with_transparent(true)
            .with_resizable(false)
            .with_taskbar(false)
            .with_always_on_top()
            .with_active(false)
            .with_visible(false)
            .with_position(position);
        ctx.show_viewport_immediate(viewport_id, builder, |ctx, _| {
            if !self.shown {
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                self.shown = true;
            }
            let now = ctx.input(|input| input.time);
            if self.update_fade(now, crate::bar_layout::animations_enabled()) {
                ctx.request_repaint();
            }
            if ctx.input(|input| input.viewport().close_requested()) {
                action = Action::Close;
            }
            let focused = ctx.input(|input| input.viewport().focused.unwrap_or(false));
            if self.observe_focus(focused) {
                action = Action::Close;
            }
            let panel_action = self.panel_ui(ctx, dark);
            if action == Action::None {
                action = panel_action;
            }
        });
        action
    }

    fn observe_focus(&mut self, focused: bool) -> bool {
        if focused {
            self.had_focus = true;
            self.focus_lost_at = None;
            return false;
        }
        if !self.had_focus {
            return false;
        }
        let lost = *self.focus_lost_at.get_or_insert(Instant::now());
        Instant::now().duration_since(lost) >= Duration::from_millis(100)
    }

    /// 更新入场淡入；关闭动效时直接显示完整面板。返回是否还需要继续刷新。
    fn update_fade(&mut self, now: f64, motion: bool) -> bool {
        if !motion {
            self.fade = 1.0;
            return false;
        }
        let shown_at = *self.shown_at.get_or_insert(now);
        let progress = ((now - shown_at) / FADE_SECS).clamp(0.0, 1.0) as f32;
        self.fade = if progress >= 1.0 { 1.0 } else { crate::anim::ease_out_cubic(progress) };
        progress < 1.0
    }

    /// 无窗口渲染入口：录制器与测试不需要真的创建窗口也能画同一份界面。
    pub fn panel_ui(&mut self, ctx: &egui::Context, dark: bool) -> Action {
        let palette = panel::apply_style(ctx, dark);
        let mut action = Action::None;
        let fade = self.fade.clamp(0.0, 1.0);
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(Color32::TRANSPARENT).inner_margin(PAD))
            .show(ctx, |ui| {
                ui.set_opacity(fade);
                egui::Frame::new()
                    .fill(palette.surface)
                    .stroke(Stroke::new(1.0_f32, palette.line))
                    .corner_radius(18.0)
                    .inner_margin(INNER)
                    .shadow(panel::shadow(dark))
                    .show(ui, |ui| {
                        ui.set_min_size(ui.available_size());
                        action = self.panel_content(ui, &palette);
                    });
            });
        action
    }

    fn panel_content(&mut self, ui: &mut Ui, p: &Palette) -> Action {
        let ctx = ui.ctx().clone();
        let mut action = self.consume_navigation(&ctx);
        let header = self.header(ui, p);
        if action == Action::None {
            action = header;
        }
        ui.add_space(12.0);
        let previous = self.query.clone();
        let (focused, clear) = self.search_box(ui, p);
        if clear {
            self.query.clear();
        }
        if self.query != previous {
            self.selected = 0;
            self.selected_id = None;
            self.rebuild();
        }
        if let Some(error) = self.hotkey_error.clone() {
            ui.add_space(6.0);
            let (dot, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
            ui.painter().circle_filled(dot.center(), 2.6, p.warn);
            ui.label(RichText::new(error).font(theme::font(10.5, W::Regular)).color(p.warn));
        }
        ui.add_space(10.0);
        let left = ui.max_rect().left();
        let right = ui.max_rect().right();
        let bottom = ui.max_rect().bottom();
        let footer_top = (bottom - FOOTER_H).max(ui.cursor().top() + 80.0);
        let list = Rect::from_min_max(
            Pos2::new(left, ui.cursor().top()),
            Pos2::new(right, (footer_top - 4.0).max(ui.cursor().top() + 60.0)),
        );
        let list_action = self.list_body(ui, list, p);
        if action == Action::None {
            action = list_action;
        }
        let footer = Rect::from_min_max(Pos2::new(left, footer_top), Pos2::new(right, bottom));
        let footer_action = self.footer(ui, footer, p);
        if action == Action::None {
            action = footer_action;
        }
        let _ = focused;
        action
    }

    fn consume_navigation(&mut self, ctx: &egui::Context) -> Action {
        if ctx.memory(|memory| memory.any_popup_open()) {
            return Action::None;
        }
        let (up, down, enter, escape) = ctx.input_mut(|input| {
            (
                input.consume_key(Modifiers::NONE, Key::ArrowUp),
                input.consume_key(Modifiers::NONE, Key::ArrowDown),
                input.consume_key(Modifiers::NONE, Key::Enter),
                input.consume_key(Modifiers::NONE, Key::Escape),
            )
        });
        let mut action = Action::None;
        if escape {
            if self.query.is_empty() {
                return Action::Close;
            }
            self.query.clear();
            self.selected = 0;
            self.selected_id = None;
            self.rebuild();
            self.request_focus = true;
            return Action::None;
        }
        if up {
            self.move_selection(-1);
        }
        if down {
            self.move_selection(1);
        }
        if enter {
            let selected = self.selected_action();
            if selected != Action::None {
                action = selected;
            }
        }
        action
    }

    fn header(&mut self, ui: &mut Ui, p: &Palette) -> Action {
        let mut action = Action::None;
        ui.horizontal(|ui| {
            let (icon_rect, _) = ui.allocate_exact_size(Vec2::splat(23.0), Sense::hover());
            draw_compass_icon(ui, icon_rect, p.accent);
            ui.add_space(2.0);
            ui.label(RichText::new("命令面板").font(theme::font(16.0, W::Semibold)).color(p.text));
            ui.add_space(6.0);
            keycap(ui, "Ctrl+Alt+K", p);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if icon_button(ui, "×", "关闭窗口", p.muted, p.text, p.hover, 16.0) {
                    action = Action::Close;
                }
            });
        });
        action
    }

    fn search_box(&mut self, ui: &mut Ui, p: &Palette) -> (bool, bool) {
        if !self.search_focused {
            let typed = ui.ctx().input(|input| {
                let mut text = String::new();
                for event in &input.events {
                    if let egui::Event::Text(chunk) = event {
                        text.push_str(chunk);
                    }
                }
                text
            });
            if !typed.is_empty() {
                self.query.push_str(&typed);
                self.rebuild();
                self.request_focus = true;
            }
        }
        let response = panel::search_field(ui, "command-palette-search", &mut self.query,
            "搜索页面、设置、应用，或输入算式…", p, &mut self.request_focus, false);
        self.search_focused = response.focused;
        (response.focused, response.cleared)
    }

    fn list_body(&mut self, ui: &mut Ui, rect: Rect, p: &Palette) -> Action {
        let mut action = Action::None;
        if self.results.is_empty() {
            self.empty_results(ui, rect, p);
            return Action::None;
        }
        let searching = !self.query.trim().is_empty();
        let mut previous_section: Option<Section> = None;
        ui.scope_builder(UiBuilder::new().max_rect(rect), |ui| {
            ui.set_width(rect.width());
            ui.spacing_mut().item_spacing = Vec2::new(0.0, 2.0);
            let scroll = egui::ScrollArea::vertical()
                .id_salt("command-palette-list")
                .auto_shrink([false, false])
                .max_height(rect.height())
                .scroll_bar_visibility(
                    egui::containers::scroll_area::ScrollBarVisibility::VisibleWhenNeeded,
                )
                .show(ui, |ui| {
                    for (index, entry) in self.results.iter().enumerate() {
                        if !searching && previous_section != Some(entry.section) {
                            panel::section_label(ui, entry.section.label(), p);
                            previous_section = Some(entry.section);
                        }
                        let selected = self.selected_id.as_deref() == Some(entry.id.as_str());
                        let scroll = selected && self.scroll_to_selected;
                        let clicked = entry_row(ui, entry, selected, p, searching, scroll);
                        if clicked {
                            if action == Action::None {
                                action = entry.action.clone();
                            }
                            self.selected = index;
                            self.selected_id = Some(entry.id.clone());
                        }
                    }
                    ui.add_space(4.0);
                });
            if scroll.content_size.y > scroll.inner_rect.height() + 1.0 {
                let area = scroll.inner_rect;
                for step in 0..6 {
                    let k = step as f32 / 5.0;
                    let y = area.bottom() - 18.0 + step as f32 * 3.0;
                    let strip = Rect::from_min_size(Pos2::new(area.left(), y), Vec2::new(area.width(), 3.0));
                    ui.painter().rect_filled(strip, 0.0, Color32::from_rgba_unmultiplied(p.surface.r(), p.surface.g(), p.surface.b(), (16.0 + 26.0 * k) as u8));
                }
            }
        });
        self.scroll_to_selected = false;
        action
    }

    fn empty_results(&self, ui: &mut Ui, rect: Rect, p: &Palette) {
        ui.scope_builder(
            UiBuilder::new().max_rect(rect).layout(Layout::top_down(Align::Center)),
            |ui| {
                ui.set_width(rect.width());
                ui.add_space((rect.height() * 0.28).clamp(12.0, 80.0));
                let (icon_rect, _) = ui.allocate_exact_size(Vec2::splat(44.0), Sense::hover());
                draw_empty_search(ui.painter(), icon_rect, p);
                ui.add_space(12.0);
                ui.label(RichText::new("没有匹配的命令").font(theme::font(14.0, W::Semibold)).color(p.text));
                ui.add_space(4.0);
                ui.label(
                    RichText::new("试试「设置」「待办」「12*8」或应用名")
                        .font(theme::font(11.5, W::Regular))
                        .color(p.muted),
                );
            },
        );
    }

    fn footer(&mut self, ui: &mut Ui, rect: Rect, p: &Palette) -> Action {
        let count = self.results.len();
        ui.scope_builder(UiBuilder::new().max_rect(rect), |ui| {
            ui.set_width(rect.width());
            ui.painter().line_segment(
                [Pos2::new(rect.left(), rect.top()), Pos2::new(rect.right(), rect.top())],
                Stroke::new(1.0_f32, p.line_soft),
            );
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                hint(ui, "↑↓", "选择", p);
                ui.add_space(8.0);
                hint(ui, "Enter", "执行", p);
                ui.add_space(8.0);
                hint(ui, "Esc", if self.query.is_empty() { "关闭" } else { "清除搜索" }, p);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let text = if self.query.trim().is_empty() {
                        "输入可搜索应用、设置与换算".to_string()
                    } else {
                        format!("{count} 个结果")
                    };
                    ui.label(RichText::new(text).font(theme::font(10.5, W::Regular)).color(p.faint));
                });
            });
        });
        Action::None
    }
}

// ---------------------------------------------------------------- 行绘制

fn entry_row(
    ui: &mut Ui,
    entry: &Entry,
    selected: bool,
    p: &Palette,
    show_section: bool,
    scroll_into_view: bool,
) -> bool {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_H), Sense::click());
    if !ui.is_rect_visible(rect) {
        return false;
    }
    if scroll_into_view {
        ui.scroll_to_rect(rect, Some(Align::Center));
    }
    let ctx = ui.ctx().clone();
    let hover = panel::animate_bool(&ctx,
        egui::Id::new(("command-row-hover", entry.id.as_str())),
        response.hovered() && !selected,
        0.09,
    );
    let painter = ui.painter().with_clip_rect(rect);
    let radius = 9.0;
    let body = rect.shrink2(Vec2::new(0.0, 1.0));
    if selected {
        painter.rect_filled(body, radius, mix(p.hover, p.accent, 0.05));
        painter.rect_stroke(
            body,
            radius,
            Stroke::new(1.0_f32, p.line_soft),
            egui::StrokeKind::Inside,
        );
        painter.rect_filled(
            Rect::from_min_size(Pos2::new(rect.left() + 1.5, rect.top() + 10.0), Vec2::new(3.0, ROW_H - 20.0)),
            1.5,
            p.accent,
        );
    } else if hover > 0.01 {
        painter.rect_filled(body, radius, mix(p.surface, p.hover, hover));
    }
    response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, ui.is_enabled(), selected, &entry.title));
    panel::focus_ring(ui, &response, p.accent, radius);
    let icon_rect = Rect::from_min_size(
        Pos2::new(rect.left() + 10.0, rect.center().y - 14.0),
        Vec2::splat(28.0),
    );
    let tint = entry_tint(entry);
    let (tile, ink) = match entry.glyph {
        Glyph::App => (
            with_alpha(tint, if selected { 0.34 } else { 0.20 }),
            if selected { mix(tint, p.text, 0.35) } else { tint },
        ),
        _ => (
            if selected { mix(p.raised, p.accent, 0.10) } else { p.raised },
            if selected { p.accent } else { p.muted },
        ),
    };
    painter.rect_filled(icon_rect, 8.0, tile);
    draw_glyph(&painter, icon_rect, entry, ink);
    let title_x = icon_rect.right() + 10.0;
    let tag_space = if show_section { 52.0 } else { 6.0 };
    let max_width = (rect.right() - 12.0 - tag_space - title_x).max(60.0);
    let title = elided(ui, &entry.title, theme::font(12.5, if selected { W::Semibold } else { W::Regular }), p.text, max_width);
    painter.galley(Pos2::new(title_x, rect.center().y - 12.0), title, p.text);
    let subtitle = elided(ui, &entry.subtitle, theme::font(10.5, W::Regular), p.faint, max_width);
    painter.galley(Pos2::new(title_x, rect.center().y + 5.0), subtitle, p.faint);
    if show_section {
        painter.text(
            Pos2::new(rect.right() - 12.0, rect.center().y),
            egui::Align2::RIGHT_CENTER,
            entry.section.label(),
            theme::font(9.5, W::Regular),
            with_alpha(if entry.section == Section::Calc { p.accent } else { p.faint }, 0.9),
        );
    }
    let _ = response.clone().on_hover_cursor(egui::CursorIcon::PointingHand);
    response.clicked()
}

fn elided(
    ui: &Ui,
    text: &str,
    font: egui::FontId,
    color: Color32,
    max_width: f32,
) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::default();
    job.wrap.max_width = max_width;
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    job.wrap.overflow_character = Some('…');
    job.append(text, 0.0, egui::TextFormat { font_id: font, color, ..Default::default() });
    ui.fonts(|fonts| fonts.layout_job(job))
}

fn entry_tint(entry: &Entry) -> Color32 {
    let palette = [
        theme::BLUE,
        theme::SOFT_GREEN,
        theme::YELLOW,
        theme::RED_TOP,
        theme::CYAN,
        crate::panel::rgb(186, 148, 255),
    ];
    let mut hash = 2166136261u32;
    for byte in entry.id.bytes() {
        hash ^= byte as u32;
        hash = hash.wrapping_mul(16777619);
    }
    palette[(hash as usize) % palette.len()]
}

fn draw_glyph(painter: &egui::Painter, rect: Rect, entry: &Entry, color: Color32) {
    let center = rect.center();
    match entry.glyph {
        Glyph::Clipboard => {
            let body = Rect::from_center_size(center + Vec2::new(0.0, 1.0), Vec2::new(12.0, 14.0));
            painter.rect_stroke(body, 2.5, Stroke::new(1.5_f32, color), egui::StrokeKind::Inside);
            painter.rect_filled(
                Rect::from_center_size(Pos2::new(center.x, body.top() + 1.0), Vec2::new(6.0, 3.0)),
                1.5,
                color,
            );
        }
        Glyph::Timer => {
            painter.circle_stroke(center, 6.5, Stroke::new(1.5_f32, color));
            painter.line_segment(
                [center, center + Vec2::new(0.0, -4.0)],
                Stroke::new(1.4_f32, color),
            );
            painter.line_segment(
                [center, center + Vec2::new(2.6, 1.6)],
                Stroke::new(1.4_f32, color),
            );
            painter.line_segment(
                [center + Vec2::new(-2.0, -9.0), center + Vec2::new(2.0, -9.0)],
                Stroke::new(1.4_f32, color),
            );
            painter.line_segment(
                [center + Vec2::new(0.0, -9.0), center + Vec2::new(0.0, -6.5)],
                Stroke::new(1.4_f32, color),
            );
        }
        Glyph::Calendar => {
            let body = Rect::from_center_size(center + Vec2::new(0.0, 1.0), Vec2::new(14.0, 12.0));
            painter.rect_stroke(body, 2.5, Stroke::new(1.5_f32, color), egui::StrokeKind::Inside);
            painter.line_segment(
                [Pos2::new(body.left() + 1.0, body.top() + 4.0), Pos2::new(body.right() - 1.0, body.top() + 4.0)],
                Stroke::new(1.2_f32, color),
            );
            for (x, y) in [(-3.0, 0.0), (1.0, 0.0), (-3.0, 4.0), (1.0, 4.0)] {
                painter.circle_filled(center + Vec2::new(x, y), 1.0, color);
            }
            painter.line_segment(
                [center + Vec2::new(-3.5, -8.5), center + Vec2::new(-3.5, -5.5)],
                Stroke::new(1.4_f32, color),
            );
            painter.line_segment(
                [center + Vec2::new(3.5, -8.5), center + Vec2::new(3.5, -5.5)],
                Stroke::new(1.4_f32, color),
            );
        }
        Glyph::Info => {
            painter.circle_stroke(center, 7.0, Stroke::new(1.5_f32, color));
            painter.circle_filled(center + Vec2::new(0.0, -3.4), 1.0, color);
            painter.line_segment(
                [center + Vec2::new(0.0, -0.8), center + Vec2::new(0.0, 4.0)],
                Stroke::new(1.5_f32, color),
            );
        }
        Glyph::Page => {
            painter.rect_stroke(
                Rect::from_center_size(center, Vec2::new(13.0, 15.0)),
                2.5,
                Stroke::new(1.5_f32, color),
                egui::StrokeKind::Inside,
            );
            for offset in [-3.0f32, 1.0, 5.0] {
                painter.line_segment(
                    [center + Vec2::new(-4.0, offset), center + Vec2::new(4.0, offset)],
                    Stroke::new(1.2_f32, color),
                );
            }
        }
        Glyph::Gear => {
            painter.circle_stroke(center, 6.0, Stroke::new(1.5_f32, color));
            painter.circle_stroke(center, 1.8, Stroke::new(1.3_f32, color));
            for step in 0..6 {
                let angle = std::f32::consts::TAU * step as f32 / 6.0;
                let direction = Vec2::angled(angle);
                painter.line_segment(
                    [center + direction * 6.0, center + direction * 8.6],
                    Stroke::new(1.6_f32, color),
                );
            }
        }
        Glyph::Calc => {
            painter.text(center, egui::Align2::CENTER_CENTER, "=", theme::font(15.0, W::Semibold), color);
        }
        Glyph::Scan => {
            // 取景框四角 + 两行文字，表示「截取图片里的文字」。
            for (sx, sy) in [(-1.0f32, -1.0f32), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
                let corner = center + Vec2::new(sx * 7.0, sy * 7.0);
                painter.line_segment([corner, corner - Vec2::new(sx * 4.0, 0.0)], Stroke::new(1.4_f32, color));
                painter.line_segment([corner, corner - Vec2::new(0.0, sy * 4.0)], Stroke::new(1.4_f32, color));
            }
            painter.line_segment([center + Vec2::new(-3.0, -1.5), center + Vec2::new(3.0, -1.5)], Stroke::new(1.3_f32, color));
            painter.line_segment([center + Vec2::new(-3.0, 2.0), center + Vec2::new(1.0, 2.0)], Stroke::new(1.3_f32, color));
        }
        Glyph::App => {
            let monogram: String = entry
                .title
                .chars()
                .next()
                .map(|ch| ch.to_uppercase().collect())
                .unwrap_or_default();
            painter.text(center, egui::Align2::CENTER_CENTER, monogram, theme::font(13.0, W::Semibold), color);
        }
    }
}

fn draw_compass_icon(ui: &Ui, rect: Rect, color: Color32) {
    let painter = ui.painter();
    let center = rect.center();
    painter.circle_stroke(center, 8.5, Stroke::new(1.7_f32, color));
    let needle = [
        center + Vec2::new(-4.0, 4.0),
        center + Vec2::new(4.0, -4.0),
        center + Vec2::new(1.4, 1.4),
        center + Vec2::new(-1.4, -1.4),
    ];
    painter.add(egui::Shape::convex_polygon(needle.to_vec(), color, Stroke::NONE));
}

fn draw_empty_search(painter: &egui::Painter, rect: Rect, p: &Palette) {
    painter.circle_filled(rect.center(), rect.width() / 2.0, p.raised);
    painter.circle_stroke(rect.center(), rect.width() / 2.0, Stroke::new(1.0_f32, p.line_soft));
    let center = rect.center();
    painter.circle_stroke(center - Vec2::new(2.0, 2.0), 7.0, Stroke::new(1.7_f32, p.faint));
    painter.line_segment(
        [center + Vec2::new(3.0, 3.0), center + Vec2::new(8.5, 8.5)],
        Stroke::new(1.7_f32, p.faint),
    );
}

// ---------------------------------------------------------------- 数据与匹配

/// 常用 Windows 工具。路径由系统目录约定而来，存在与否交给 ShellExecute 判断。
const BUILTIN_APPS: &[(&str, &str, &str)] = &[
    ("记事本", r"C:\Windows\System32\notepad.exe", "notepad text 文本"),
    ("计算器", "calc.exe", "calculator calc 计算"),
    ("画图", r"C:\Windows\System32\mspaint.exe", "paint 图片 绘图"),
    ("截图工具", r"C:\Windows\System32\SnippingTool.exe", "screenshot snip 截图"),
    ("任务管理器", r"C:\Windows\System32\Taskmgr.exe", "task manager 进程"),
    ("控制面板", r"C:\Windows\System32\control.exe", "control panel 设置"),
    ("系统设置", "ms-settings:", "settings 设置"),
    ("命令提示符", r"C:\Windows\System32\cmd.exe", "cmd terminal 终端"),
    ("PowerShell", r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe", "shell terminal 终端"),
    ("资源管理器", r"C:\Windows\explorer.exe", "explorer files 文件"),
];

const SETTINGS_TABS: &[(usize, &str, &str, &str)] = &[
    (0, "常规", "设置页外观与启动行为", "general theme appearance"),
    (10, "页面管理", "页面顺序、显隐与常驻组件", "pages layout order resident"),
    (1, "音乐与歌词", "歌词显示与播放来源", "music lyrics player"),
    (2, "专注", "专注与休息时长", "focus pomodoro timer"),
    (3, "饮水", "饮水目标、提醒间隔与时段", "water drink reminder"),
    (4, "待办", "待办提醒与清单", "todo task reminder"),
    (5, "置物", "文件置物架", "shelf files drag"),
    (6, "通知", "Windows 通知读取与提醒", "notification alert"),
    (7, "邮箱", "IMAP 账号、验证码与会议邀请", "mail imap otp email"),
    (8, "日程", "CalDAV 与本机日程", "calendar schedule"),
    (11, "AI 额度", "账号、额度与常驻圆环", "quota ai account"),
    (12, "剪贴板", "历史开关、容量与清空", "clipboard history"),
    (9, "关于", "版本、更新与开源声明", "about update version"),
    (13, "岛体外观", "不透明度、圆角与动效", "appearance opacity radius motion 外观 透明度 圆角 动效"),
    (14, "天气", "城市、自动刷新与数据来源", "weather forecast city 天气 城市 预报"),
];

fn page_english_keywords(page: usize) -> &'static str {
    match page {
        0 => "music player lyrics 音乐",
        1 => "focus pomodoro timer 专注",
        2 => "water drink 饮水",
        3 => "shelf files drag 置物 文件",
        4 => "todo task 待办",
        5 => "mail inbox otp 邮件 验证码",
        6 => "quota ai 额度",
        7 => "calendar schedule 日程 日历",
        8 => "system cpu memory network disk 系统 性能 内存 网络 磁盘",
        _ => "",
    }
}

fn match_score(entry: &Entry, needle: &str) -> Option<u16> {
    if needle.is_empty() {
        return Some(0);
    }
    let title = entry.title.to_lowercase();
    if title.starts_with(needle) {
        return Some(0);
    }
    if title.contains(needle) {
        return Some(100);
    }
    if entry.haystack.contains(needle) {
        return Some(300);
    }
    if needle.chars().count() >= 2 && is_subsequence(&title, needle) {
        return Some(600);
    }
    None
}

fn is_subsequence(haystack: &str, needle: &str) -> bool {
    let mut chars = haystack.chars();
    for wanted in needle.chars() {
        loop {
            match chars.next() {
                Some(ch) if ch == wanted => break,
                Some(_) => continue,
                None => return false,
            }
        }
    }
    true
}

// ---------------------------------------------------------------- 应用扫描

pub(crate) fn default_app_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(appdata) = std::env::var_os("APPDATA") {
        roots.push(
            PathBuf::from(appdata)
                .join("Microsoft")
                .join("Windows")
                .join("Start Menu")
                .join("Programs"),
        );
    }
    if let Some(program_data) = std::env::var_os("ProgramData") {
        roots.push(
            PathBuf::from(program_data)
                .join("Microsoft")
                .join("Windows")
                .join("Start Menu")
                .join("Programs"),
        );
    }
    roots.into_iter().filter(|root| root.is_dir()).collect()
}

fn scan_apps(roots: &[PathBuf]) -> Vec<AppEntry> {
    let mut found: Vec<AppEntry> = Vec::new();
    for root in roots {
        walk_apps(root, root, 0, &mut found);
    }
    found.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.path.cmp(&b.path))
    });
    let mut seen = std::collections::HashSet::new();
    found.retain(|app| seen.insert(app.name.to_lowercase()));
    found.truncate(MAX_APPS);
    found
}

fn walk_apps(root: &Path, dir: &Path, depth: usize, out: &mut Vec<AppEntry>) {
    if depth > 4 || out.len() >= MAX_APPS {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            walk_apps(root, &path, depth + 1, out);
            continue;
        }
        let extension = path
            .extension()
            .and_then(|value| value.to_str())
            .map(|value| value.to_ascii_lowercase());
        if !matches!(extension.as_deref(), Some("lnk") | Some("url")) {
            continue;
        }
        let Some(name) = path.file_stem().and_then(|value| value.to_str()) else {
            continue;
        };
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 64 {
            continue;
        }
        let lower = name.to_lowercase();
        if ["uninstall", "unins", "卸载", "帮助", "help", "readme", "website", "官网", "修复", "daozai", "岛仔"].iter().any(|word| lower.contains(word)) {
            continue;
        }
        let folder = path
            .parent()
            .and_then(|parent| parent.strip_prefix(root).ok())
            .map(|relative| relative.display().to_string())
            .unwrap_or_default();
        out.push(AppEntry { name: name.to_string(), folder, path });
        if out.len() >= MAX_APPS {
            return;
        }
    }
}

// ---------------------------------------------------------------- 计算与换算

fn calculations(query: &str) -> Vec<Entry> {
    let mut out = Vec::new();
    if let Some(value) = evaluate_math(query) {
        let text = format_number(value);
        out.push(Entry {
            id: "calc-math".into(),
            section: Section::Calc,
            title: format!("= {text}"),
            subtitle: "计算 · Enter 复制结果".into(),
            haystack: String::new(),
            action: Action::CopyText(text),
            glyph: Glyph::Calc,
        });
    }
    if let Some((title, copy)) = convert_units(query) {
        out.push(Entry {
            id: "calc-unit".into(),
            section: Section::Calc,
            title,
            subtitle: "单位换算 · Enter 复制结果".into(),
            haystack: String::new(),
            action: Action::CopyText(copy),
            glyph: Glyph::Calc,
        });
    }
    for (title, copy) in crate::world_clock::lookup(query) {
        out.push(Entry {
            id: format!("clock-{title}"),
            section: Section::Calc,
            title,
            subtitle: "世界时钟 · Enter 复制时间".into(),
            haystack: String::new(),
            action: Action::CopyText(copy),
            glyph: Glyph::Calc,
        });
    }
    out
}

fn format_number(value: f64) -> String {
    if !value.is_finite() {
        return String::new();
    }
    if value == 0.0 {
        return "0".into();
    }
    let abs = value.abs();
    if abs >= 1e15 || abs < 1e-6 {
        return format!("{value:.4e}");
    }
    if (value - value.round()).abs() < 1e-9 {
        return format!("{}", value.round() as i64);
    }
    let decimals = if abs >= 1000.0 {
        2
    } else if abs >= 1.0 {
        4
    } else {
        6
    };
    let mut text = format!("{value:.decimals$}");
    while text.ends_with('0') {
        text.pop();
    }
    if text.ends_with('.') {
        text.pop();
    }
    text
}

#[derive(Clone, Copy, PartialEq)]
enum Token {
    Number(f64),
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Pow,
    LeftParen,
    RightParen,
}

fn tokenize_math(input: &str) -> Option<Vec<Token>> {
    let mut tokens = Vec::new();
    let mut chars = input.chars().peekable();
    let mut saw_digit = false;
    while let Some(ch) = chars.next() {
        match ch {
            ' ' | '\t' => {}
            '0'..='9' | '.' => {
                saw_digit = true;
                let mut text = String::from(ch);
                while let Some(next) = chars.peek().copied() {
                    if next.is_ascii_digit() || next == '.' {
                        text.push(next);
                        chars.next();
                    } else {
                        break;
                    }
                }
                let value: f64 = text.parse().ok()?;
                tokens.push(Token::Number(value));
            }
            '+' => tokens.push(Token::Add),
            '-' | '−' => tokens.push(Token::Sub),
            '*' | '×' => tokens.push(Token::Mul),
            '/' | '÷' => tokens.push(Token::Div),
            '%' => tokens.push(Token::Rem),
            '^' => tokens.push(Token::Pow),
            '(' => tokens.push(Token::LeftParen),
            ')' => tokens.push(Token::RightParen),
            _ => return None,
        }
    }
    if !saw_digit || tokens.is_empty() {
        return None;
    }
    Some(tokens)
}

struct MathParser {
    tokens: Vec<Token>,
    at: usize,
}

impl MathParser {
    fn new(tokens: Vec<Token>) -> Self {
        Self { tokens, at: 0 }
    }

    fn peek(&self) -> Option<Token> {
        self.tokens.get(self.at).copied()
    }

    fn bump(&mut self) -> Option<Token> {
        let token = self.peek();
        if token.is_some() {
            self.at += 1;
        }
        token
    }

    fn expression(&mut self) -> Option<f64> {
        let mut value = self.term()?;
        loop {
            match self.peek() {
                Some(Token::Add) => {
                    self.bump();
                    value += self.term()?;
                }
                Some(Token::Sub) => {
                    self.bump();
                    value -= self.term()?;
                }
                _ => return Some(value),
            }
        }
    }

    fn term(&mut self) -> Option<f64> {
        let mut value = self.unary()?;
        loop {
            match self.peek() {
                Some(Token::Mul) => {
                    self.bump();
                    value *= self.unary()?;
                }
                Some(Token::Div) => {
                    self.bump();
                    let divisor = self.unary()?;
                    if divisor == 0.0 {
                        return None;
                    }
                    value /= divisor;
                }
                Some(Token::Rem) => {
                    self.bump();
                    let divisor = self.unary()?;
                    if divisor == 0.0 {
                        return None;
                    }
                    value %= divisor;
                }
                _ => return Some(value),
            }
        }
    }

    fn unary(&mut self) -> Option<f64> {
        match self.peek() {
            Some(Token::Sub) => {
                self.bump();
                Some(-self.unary()?)
            }
            Some(Token::Add) => {
                self.bump();
                self.unary()
            }
            _ => self.power(),
        }
    }

    fn power(&mut self) -> Option<f64> {
        let base = self.primary()?;
        if self.peek() == Some(Token::Pow) {
            self.bump();
            let exponent = self.unary()?;
            return Some(base.powf(exponent));
        }
        Some(base)
    }

    fn primary(&mut self) -> Option<f64> {
        match self.bump() {
            Some(Token::Number(value)) => Some(value),
            Some(Token::LeftParen) => {
                let value = self.expression()?;
                if self.bump() != Some(Token::RightParen) {
                    return None;
                }
                Some(value)
            }
            _ => None,
        }
    }
}

fn evaluate_math(query: &str) -> Option<f64> {
    let query = query.trim();
    if query.is_empty() || query.chars().any(|ch| ch.is_alphabetic()) {
        return None;
    }
    // 纯数字（例如“2026”）不当成计算，避免搜索结果里多出无意义的等号条目。
    if query.chars().all(|ch| ch.is_ascii_digit() || ch == '.' || ch == ' ') {
        return None;
    }
    let tokens = tokenize_math(query)?;
    let mut parser = MathParser::new(tokens);
    let value = parser.expression()?;
    if parser.at != parser.tokens.len() || !value.is_finite() {
        return None;
    }
    Some(value)
}

const LINEAR_UNITS: &[(&str, &str, f64)] = &[
    ("mm", "length", 0.001),
    ("cm", "length", 0.01),
    ("m", "length", 1.0),
    ("km", "length", 1000.0),
    ("in", "length", 0.0254),
    ("ft", "length", 0.3048),
    ("yd", "length", 0.9144),
    ("mi", "length", 1609.344),
    ("mg", "mass", 0.000001),
    ("g", "mass", 0.001),
    ("kg", "mass", 1.0),
    ("t", "mass", 1000.0),
    ("oz", "mass", 0.028349523125),
    ("lb", "mass", 0.45359237),
    ("b", "data", 1.0),
    ("kb", "data", 1024.0),
    ("mb", "data", 1048576.0),
    ("gb", "data", 1073741824.0),
    ("tb", "data", 1099511627776.0),
    ("km/h", "speed", 1.0),
    ("m/s", "speed", 3.6),
    ("mph", "speed", 1.609344),
    ("kn", "speed", 1.852),
    ("s", "time", 1.0),
    ("min", "time", 60.0),
    ("h", "time", 3600.0),
    ("d", "time", 86400.0),
];

fn normalize_unit(raw: &str) -> Option<&'static str> {
    let cleaned = raw.trim().to_lowercase().replace('°', "").replace(' ', "");
    let canonical: &str = match cleaned.as_str() {
        "公里" | "千米" | "kilometer" | "kilometers" => "km",
        "米" | "meter" | "meters" => "m",
        "厘米" | "centimeter" | "centimeters" => "cm",
        "毫米" | "millimeter" | "millimeters" => "mm",
        "英寸" | "inch" | "inches" => "in",
        "英尺" | "foot" | "feet" => "ft",
        "码" | "yard" | "yards" => "yd",
        "英里" | "mile" | "miles" => "mi",
        "克" | "gram" | "grams" => "g",
        "毫克" => "mg",
        "千克" | "公斤" | "kilogram" | "kilograms" => "kg",
        "吨" | "ton" | "tons" => "t",
        "盎司" | "ounce" | "ounces" => "oz",
        "磅" | "pound" | "pounds" => "lb",
        "字节" | "byte" | "bytes" => "b",
        "千字节" | "kilobyte" | "kilobytes" => "kb",
        "兆字节" | "megabyte" | "megabytes" => "mb",
        "吉字节" | "千兆字节" | "gigabyte" | "gigabytes" => "gb",
        "太字节" | "terabyte" | "terabytes" => "tb",
        "公里/小时" | "公里每小时" | "千米每小时" | "kph" | "kmh" => "km/h",
        "米/秒" | "米每秒" | "mps" => "m/s",
        "英里/小时" | "英里每小时" => "mph",
        "节" | "knot" | "knots" => "kn",
        "秒" | "second" | "seconds" => "s",
        "分钟" | "minute" | "minutes" => "min",
        "小时" | "hour" | "hours" => "h",
        "天" | "day" | "days" => "d",
        "摄氏度" | "celsius" | "度" => "c",
        "华氏度" | "fahrenheit" => "f",
        "开尔文" | "kelvin" => "k",
        other => other,
    };
    if let Some((name, _, _)) = LINEAR_UNITS.iter().find(|(name, _, _)| *name == canonical) {
        return Some(*name);
    }
    match canonical {
        "c" => Some("c"),
        "f" => Some("f"),
        "k" => Some("k"),
        _ => None,
    }
}

fn unit_display(unit: &str) -> String {
    match unit {
        "c" => "°C".into(),
        "f" => "°F".into(),
        "k" => "K".into(),
        "b" => "B".into(),
        "kb" => "KB".into(),
        "mb" => "MB".into(),
        "gb" => "GB".into(),
        "tb" => "TB".into(),
        other => other.to_string(),
    }
}

fn parse_number_and_unit(input: &str) -> Option<(f64, &'static str)> {
    let input = input.trim();
    let mut end = 0usize;
    let mut saw_digit = false;
    for (index, ch) in input.char_indices() {
        let allowed = ch.is_ascii_digit()
            || ch == '.'
            || (ch == '-' || ch == '+') && index == 0;
        if allowed {
            if ch.is_ascii_digit() {
                saw_digit = true;
            }
            end = index + ch.len_utf8();
        } else {
            break;
        }
    }
    if !saw_digit {
        return None;
    }
    let number: f64 = input[..end].parse().ok()?;
    let unit = normalize_unit(input[end..].trim())?;
    Some((number, unit))
}

fn convert_units(query: &str) -> Option<(String, String)> {
    let lower = query.trim().to_lowercase();
    if lower.is_empty() {
        return None;
    }
    let (left, right) = [" to ", " in ", "->", "→"]
        .iter()
        .find_map(|separator| lower.split_once(separator))
        .map(|(left, right)| (left.trim().to_string(), right.trim().to_string()))?;
    let (value, from) = parse_number_and_unit(&left)?;
    let to = normalize_unit(&right)?;
    let (result, from_display, to_display) = if matches!(from, "c" | "f" | "k") && matches!(to, "c" | "f" | "k") {
        let celsius = match from {
            "c" => value,
            "f" => (value - 32.0) * 5.0 / 9.0,
            "k" => value - 273.15,
            _ => return None,
        };
        let result = match to {
            "c" => celsius,
            "f" => celsius * 9.0 / 5.0 + 32.0,
            "k" => celsius + 273.15,
            _ => return None,
        };
        (result, unit_display(from), unit_display(to))
    } else {
        if matches!(from, "c" | "f" | "k") || matches!(to, "c" | "f" | "k") {
            return None;
        }
        let (_, from_group, from_factor) = LINEAR_UNITS.iter().find(|(name, _, _)| *name == from)?;
        let (_, to_group, to_factor) = LINEAR_UNITS.iter().find(|(name, _, _)| *name == to)?;
        if from_group != to_group {
            return None;
        }
        (value * from_factor / to_factor, unit_display(from), unit_display(to))
    };
    if !result.is_finite() {
        return None;
    }
    let input_text = format!("{} {}", format_number(value), from_display);
    let output_text = format!("{} {}", format_number(result), to_display);
    Some((format!("{input_text} = {output_text}"), output_text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_close_drops_search_results_and_reopens() {
        let mut panel = palette_for_test("剪贴板");
        assert!(!panel.base.is_empty() && !panel.results.is_empty());
        panel.close();
        assert!(panel.base.is_empty() && panel.results.is_empty(), "closed panel must release duplicated search strings");
        let snapshot = panel.snapshot.clone();
        panel.open(&snapshot);
        assert!(!panel.results.is_empty(), "reopening must rebuild the commands");
    }

    #[test]
    fn polish_escape_clears_results_before_closing() {
        let ctx = egui::Context::default();
        let mut palette = palette_for_test("不存在xyz");
        assert!(palette.results.is_empty());
        for expected in [Action::None, Action::Close] {
            let mut action = Action::None;
            let _ = ctx.run(egui::RawInput {
                events: vec![egui::Event::Key { key: Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }],
                ..Default::default()
            }, |ctx| action = palette.consume_navigation(ctx));
            assert_eq!(action, expected);
            assert!(palette.query.is_empty());
            assert!(!palette.results.is_empty(), "清除后立即恢复默认操作");
        }
    }

    #[test]
    fn reduced_motion_skips_palette_fade() {
        let mut palette = Panel::default();
        palette.open(&snapshot());
        palette.shown_at = Some(0.0);
        palette.fade = 0.0;
        assert!(!palette.update_fade(0.01, false));
        assert_eq!(palette.fade, 1.0);
    }

    fn page(index: usize, name: &'static str, description: &'static str) -> PageInfo {
        PageInfo { index, name, description }
    }

    fn snapshot() -> Snapshot {
        Snapshot {
            pages: vec![
                page(0, "音乐", "播放控制与同步歌词"),
                page(4, "待办", "任务清单与到期提醒"),
                page(7, "日程", "月历、邮件邀请与工作日程"),
            ],
            clipboard_enabled: true,
            clipboard_paused: false,
            focus_running: false,
            timer_running: false,
            timer_remaining: 0.0,
            stay_awake: false,
            weather_enabled: false,
            pins: 0,
            media_title: "夜色温柔".into(),
            media_source: "Spotify".into(),
            media_playing: true,
        }
    }

    fn fixture_apps() -> Vec<AppEntry> {
        vec![
            AppEntry {
                name: "QQ".into(),
                folder: "腾讯".into(),
                path: PathBuf::from(r"C:\fixture\QQ.lnk"),
            },
            AppEntry {
                name: "微信".into(),
                folder: "腾讯".into(),
                path: PathBuf::from(r"C:\fixture\WeChat.lnk"),
            },
        ]
    }

    fn palette_for_snapshot(snapshot: Snapshot, query: &str) -> Panel {
        let mut palette = Panel::default();
        palette.apps = fixture_apps();
        palette.apps_at = Some(Instant::now());
        palette.open(&snapshot);
        palette.query = query.into();
        palette.fade = 1.0;
        palette.rebuild();
        palette
    }

    fn palette_for_test(query: &str) -> Panel {
        let mut palette = Panel::default();
        palette.apps = fixture_apps();
        palette.apps_at = Some(Instant::now());
        palette.open(&snapshot());
        palette.query = query.into();
        palette.fade = 1.0;
        palette.rebuild();
        palette
    }

    struct Rendered {
        shapes: Vec<egui::epaint::ClippedShape>,
        primitives: Vec<egui::epaint::ClippedPrimitive>,
        textures: Vec<egui::TexturesDelta>,
        pixels_per_point: f32,
    }

    fn render_palette(palette: &mut Panel, dark: bool) -> Rendered {
        render_palette_sized(palette, dark, Vec2::new(WIDTH, HEIGHT))
    }

    fn render_palette_sized(palette: &mut Panel, dark: bool, size: Vec2) -> Rendered {
        palette.fade = 1.0;
        let ctx = egui::Context::default();
        crate::theme::load_fonts(&ctx);
        let mut textures = Vec::new();
        let mut last = None;
        for frame in 0..3 {
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
                    time: Some(frame as f64 / 60.0),
                    viewports: std::iter::once((
                        egui::ViewportId::ROOT,
                        egui::ViewportInfo { native_pixels_per_point: Some(2.0), ..Default::default() },
                    ))
                    .collect(),
                    ..Default::default()
                },
                |ctx| {
                    let _ = palette.panel_ui(ctx, dark);
                },
            );
            textures.push(output.textures_delta.clone());
            last = Some(output);
        }
        let output = last.expect("至少渲染一帧");
        let shapes = output.shapes;
        let primitives = ctx.tessellate(shapes.clone(), output.pixels_per_point);
        Rendered { shapes, primitives, textures, pixels_per_point: output.pixels_per_point }
    }

    fn collect_text<'a>(shape: &'a egui::Shape, out: &mut Vec<&'a egui::epaint::TextShape>) {
        match shape {
            egui::Shape::Text(text) => out.push(text),
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect_text(shape, out);
                }
            }
            _ => {}
        }
    }

    fn text_shapes(shapes: &[egui::epaint::ClippedShape]) -> Vec<&egui::epaint::TextShape> {
        let mut out = Vec::new();
        for shape in shapes {
            collect_text(&shape.shape, &mut out);
        }
        out
    }

    fn contains(shapes: &[egui::epaint::ClippedShape], needle: &str) -> bool {
        text_shapes(shapes).iter().any(|text| text.galley.text().contains(needle))
    }

    fn write_png(rendered: &Rendered, path: &Path) {
        let width = (WIDTH * rendered.pixels_per_point) as usize;
        let height = (HEIGHT * rendered.pixels_per_point) as usize;
        let mut raster = crate::preview_raster::Raster::new(width, height);
        for delta in &rendered.textures {
            raster.update_textures(delta);
        }
        raster.draw(&rendered.primitives, rendered.pixels_per_point);
        let mut rgba = Vec::new();
        raster.to_rgba8(&mut rgba);
        image::save_buffer(path, &rgba, width as u32, height as u32, image::ColorType::Rgba8).unwrap();
    }

    #[test]
    fn open_resets_query_and_selection() {
        let mut palette = palette_for_test("旧关键词");
        palette.open(&snapshot());
        assert!(palette.open);
        assert!(palette.query.is_empty());
        assert_eq!(palette.selected, 0);
        assert!(!palette.results.is_empty());
    }

    #[test]
    fn empty_query_lists_actions_pages_and_settings_but_not_apps() {
        let palette = palette_for_test("");
        assert!(palette.results.iter().all(|entry| entry.section != Section::App));
        assert!(palette.results.iter().any(|entry| entry.title == "打开剪贴板历史"));
        assert!(palette.results.iter().any(|entry| entry.title.contains("前往页面 · 音乐")));
        assert!(palette.results.iter().any(|entry| entry.title.contains("设置 · 剪贴板")));
    }

    #[test]
    fn typing_filters_and_ranks_title_matches_first() {
        let palette = palette_for_test("设置");
        assert!(!palette.results.is_empty());
        assert!(palette.results[0].title.contains("设置"));
        let palette = palette_for_test("qq");
        assert_eq!(palette.results[0].title, "QQ", "应用名应以标题优先命中");
        let palette = palette_for_test("音乐");
        assert!(palette.results.iter().any(|entry| entry.title.contains("音乐")));
    }

    #[test]
    fn ocr_and_crop_actions_are_searchable_from_the_commands() {
        for query in ["ocr", "识别", "图片"] {
            let palette = palette_for_test(query);
            assert!(
                palette.results.iter().any(|entry| entry.action == Action::OcrClipboard),
                "查询 {query} 应找到图片文字识别"
            );
        }
        for query in ["截屏", "框选"] {
            let palette = palette_for_test(query);
            assert!(
                palette.results.iter().any(|entry| entry.action == Action::CropOcr),
                "查询 {query} 应找到截屏取字"
            );
        }
    }
    #[test]
    fn close_all_pins_appears_only_when_pins_exist() {
        let mut snapshot = snapshot();
        assert!(snapshot.pins == 0);
        let palette = palette_for_snapshot(snapshot.clone(), "贴图");
        assert!(palette.results.iter().all(|entry| entry.action != Action::ClosePins));
        snapshot.pins = 2;
        let palette = palette_for_snapshot(snapshot, "贴图");
        assert!(palette.results.iter().any(|entry| entry.action == Action::ClosePins));
    }

    #[test]
    fn recent_apps_appear_only_on_empty_query_and_keep_order() {
        let mut palette = Panel::default();
        palette.set_recent(vec![
            ("微信".into(), std::path::PathBuf::from("C:/Apps/WeChat.exe")),
            ("Code".into(), std::path::PathBuf::from("C:/Apps/Code.exe")),
        ]);
        palette.open(&snapshot());
        assert!(palette.results.first().is_some_and(|entry| entry.section == Section::Recent));
        assert!(palette.results.iter().any(|entry| entry.title == "微信"));
        assert!(palette.results.iter().any(|entry| entry.title == "Code"));
        palette.query = "微信".into();
        palette.rebuild();
        assert!(palette.results.iter().all(|entry| entry.section != Section::Recent), "输入后应由应用分组统一匹配");
        assert!(palette.results.iter().any(|entry| matches!(entry.action, Action::LaunchApp(_))));
    }

    #[test]
    fn recent_apps_dedupe_and_cap() {
        let mut recent = RecentApps::default();
        for index in 0..20 {
            recent.items.push((format!("App{index}"), std::path::PathBuf::from(format!("C:/A{index}.exe"))));
        }
        recent.items.truncate(12);
        let mut recent = RecentApps { items: recent.items };
        recent.record("App5", std::path::Path::new("C:/A5.exe"));
        assert_eq!(recent.items[0].0, "App5");
        assert_eq!(recent.items.iter().filter(|(_, path)| path.ends_with("A5.exe")).count(), 1);
        assert!(recent.items.len() <= 12);
        assert!(recent.items().len() <= 5);
    }

    #[test]
    fn media_actions_show_the_current_track() {
        let palette = palette_for_snapshot(snapshot(), "");
        let play = palette
            .results
            .iter()
            .find(|entry| entry.action == Action::MediaPlayPause)
            .expect("空查询应包含播放/暂停");
        assert!(play.subtitle.contains("Spotify") && play.subtitle.contains("夜色温柔"));
        assert!(palette.results.iter().any(|entry| entry.action == Action::MediaNext));
        assert!(palette.results.iter().any(|entry| entry.action == Action::MediaPrev));
    }

    #[test]
    fn palette_renders_at_reduced_window_size() {
        let mut palette = palette_for_test("");
        let size = Vec2::new(440.0, 340.0);
        let rendered = render_palette_sized(&mut palette, true, size);
        assert!(contains(&rendered.shapes, "命令面板"));
        assert!(contains(&rendered.shapes, "操作"));
        if let Some(dest) = std::env::var_os("DAOZAI_COMMAND_SMALL_PREVIEW").filter(|value| !value.is_empty()) {
            let dest = std::path::PathBuf::from(dest);
            std::fs::create_dir_all(&dest).unwrap();
            let ppp=rendered.pixels_per_point;
            let mut raster = crate::preview_raster::Raster::new((size.x*ppp) as usize, (size.y*ppp) as usize);
            for delta in &rendered.textures { raster.update_textures(delta); }
            raster.draw(&rendered.primitives, ppp);
            let mut rgba = Vec::new();
            raster.to_rgba8(&mut rgba);
            image::save_buffer(dest.join("command-small.png"), &rgba, (size.x*ppp) as u32, (size.y*ppp) as u32, image::ColorType::Rgba8).unwrap();
        }
    }

    #[test]
    fn clipboard_entries_are_searchable_but_hidden_from_empty_query() {
        let mut palette = Panel::default();
        palette.open(&snapshot());
        palette.set_clipboard(vec![
            ClipboardItem { id: "c1".into(), text: "https://example.com 设计文档".into() },
            ClipboardItem { id: "c2".into(), text: "git rebase --onto main".into() },
        ]);
        palette.query = "设计".into();
        palette.fade = 1.0;
        palette.rebuild();
        let entry = palette
            .results
            .iter()
            .find(|entry| entry.section == Section::Clipboard)
            .expect("应能搜到剪贴板条目");
        assert!(entry.title.contains("设计文档"));
        assert_eq!(entry.action, Action::CopyClipboard("c1".into()));
        // 空查询不展示剪贴板内容，避免面板一打开就暴露历史。
        palette.query.clear();
        palette.rebuild();
        assert!(palette.results.iter().all(|entry| entry.section != Section::Clipboard));
    }

    #[test]
    fn screenshot_action_is_searchable() {
        let palette = palette_for_test("截屏");
        assert!(
            palette.results.iter().any(|entry| entry.action == Action::ScreenshotRegion),
            "应能搜索到截屏保存"
        );
    }

    #[test]
    fn notification_history_action_is_searchable() {
        let palette = palette_for_test("通知");
        assert!(
            palette.results.iter().any(|entry| entry.action == Action::OpenNotifications),
            "应能搜索到通知历史"
        );
    }

    #[test]
    fn weather_refresh_action_is_searchable_when_enabled() {
        let mut snapshot = snapshot();
        snapshot.weather_enabled = true;
        let mut palette = Panel::default();
        palette.apps = fixture_apps();
        palette.apps_at = Some(Instant::now());
        palette.open(&snapshot);
        palette.query = "天气".into();
        palette.fade = 1.0;
        palette.rebuild();
        assert!(palette.results.iter().any(|entry| entry.action == Action::RefreshWeather), "开启天气后应能搜到刷新天气");
    }

    #[test]
    fn timer_and_stay_awake_actions_are_searchable() {
        let palette = palette_for_test("防睡眠");
        assert!(
            palette.results.iter().any(|entry| entry.action == Action::SetStayAwake(true)),
            "应能搜索到开启防睡眠"
        );
        let palette = palette_for_test("分钟");
        assert!(
            palette.results.iter().any(|entry| matches!(entry.action, Action::StartCountdown(_))),
            "应能搜索到倒计时"
        );
        let palette = palette_for_test("东京");
        assert!(
            palette.results.iter().any(|entry| entry.title.starts_with("东京")),
            "世界时钟应出现在计算结果里"
        );
    }

    #[test]
    fn builtin_system_tools_are_searchable_while_typing() {
        let palette = palette_for_test("记事本");
        assert!(!palette.results.is_empty());
        assert!(palette.results.iter().any(|entry| entry.title.contains("记事本")));
        let palette = palette_for_test("任务管理器");
        assert!(palette.results.iter().any(|entry| entry.title.contains("任务管理器")));
        let palette = palette_for_test("settings");
        assert!(palette.results.iter().any(|entry| entry.title.contains("系统设置")));
    }

    #[test]
    fn keyboard_selection_stays_within_results() {
        let mut palette = palette_for_test("");
        palette.move_selection(3);
        assert_eq!(palette.selected, 3);
        palette.move_selection(1000);
        assert_eq!(palette.selected, palette.results.len() - 1);
        palette.move_selection(-1000);
        assert_eq!(palette.selected, 0);
        assert_eq!(palette.selected_id(), Some(palette.results[0].id.clone()));
        assert_eq!(palette.selected_action(), palette.results[0].action);
    }

    #[test]
    fn calculation_entries_copy_their_result() {
        let palette = palette_for_test("12*8");
        let first = &palette.results[0];
        assert_eq!(first.section, Section::Calc);
        assert_eq!(first.action, Action::CopyText("96".into()));
        assert!(first.title.contains("= 96"));
        let palette = palette_for_test("(1+2)*3");
        assert_eq!(palette.results[0].action, Action::CopyText("9".into()));
        let palette = palette_for_test("2^10");
        assert_eq!(palette.results[0].action, Action::CopyText("1024".into()));
    }

    #[test]
    fn math_eval_handles_edge_cases() {
        assert_eq!(evaluate_math("1+2*3"), Some(7.0));
        assert_eq!(evaluate_math("-3^2"), Some(-9.0));
        assert_eq!(evaluate_math("10/4"), Some(2.5));
        assert_eq!(evaluate_math("10/0"), None);
        assert_eq!(evaluate_math("2026"), None, "纯数字不是计算");
        assert_eq!(evaluate_math("abc+1"), None);
        assert_eq!(evaluate_math("(1+2"), None);
        assert_eq!(evaluate_math("1+2+"), None);
        assert_eq!(evaluate_math(""), None);
    }

    #[test]
    fn unit_conversion_covers_length_mass_data_and_temperature() {
        let (title, copy) = convert_units("90f to c").unwrap();
        assert_eq!(copy, "32.2222 °C");
        assert!(title.contains("90 °F"));
        let (_, copy) = convert_units("100 c to f").unwrap();
        assert_eq!(copy, "212 °F");
        let (_, copy) = convert_units("1 gb to mb").unwrap();
        assert_eq!(copy, "1024 MB");
        let (_, copy) = convert_units("10 km to mi").unwrap();
        assert_eq!(copy, "6.2137 mi");
        let (_, copy) = convert_units("10 公里 to 英里").unwrap();
        assert_eq!(copy, "6.2137 mi");
        assert!(convert_units("10 km to kg").is_none(), "不同量纲不能换算");
        assert!(convert_units("90 to c").is_none());
        assert!(convert_units("10 km to").is_none());
        assert!(convert_units("abc to km").is_none());
    }

    #[test]
    fn conversion_results_appear_for_typed_queries() {
        let palette = palette_for_test("90f to c");
        assert_eq!(palette.results[0].section, Section::Calc);
        assert_eq!(palette.results[0].action, Action::CopyText("32.2222 °C".into()));
        let palette = palette_for_test("你好");
        assert!(!palette.results.iter().any(|entry| entry.section == Section::Calc));
    }

    #[test]
    fn scanning_apps_skips_uninstallers_and_dedupes() {
        let root = std::env::temp_dir().join(format!("daozai-apps-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("腾讯")).unwrap();
        std::fs::create_dir_all(root.join("工具")).unwrap();
        for (dir, name) in [
            ("腾讯", "QQ.lnk"),
            ("腾讯", "Uninstall QQ.lnk"),
            ("腾讯", "帮助文档.url"),
            ("工具", "计算器.lnk"),
            ("工具", "QQ.lnk"),
        ] {
            std::fs::write(root.join(dir).join(name), b"placeholder").unwrap();
        }
        let apps = scan_apps(&[root.clone()]);
        let _ = std::fs::remove_dir_all(&root);
        let names: Vec<&str> = apps.iter().map(|app| app.name.as_str()).collect();
        assert!(names.contains(&"QQ"));
        assert!(names.contains(&"计算器"));
        assert!(!names.iter().any(|name| name.contains("Uninstall")));
        assert!(!names.iter().any(|name| name.contains("帮助")));
        assert_eq!(names.iter().filter(|name| **name == "QQ").count(), 1);
        let qq = apps.iter().find(|app| app.name == "QQ").unwrap();
        assert!(qq.folder == "腾讯" || qq.folder == "工具", "保留其中一份快捷方式即可");
        assert!(qq.path.ends_with("QQ.lnk"));
    }

    #[test]
    fn match_score_prefers_prefix_and_supports_subsequence() {
        let entry = Entry {
            id: "x".into(),
            section: Section::Action,
            title: "打开剪贴板历史".into(),
            subtitle: "复制与粘贴".into(),
            haystack: "打开剪贴板历史 复制与粘贴 clipboard paste".into(),
            action: Action::None,
            glyph: Glyph::Clipboard,
        };
        assert_eq!(match_score(&entry, "打开"), Some(0));
        assert_eq!(match_score(&entry, "剪贴板"), Some(100));
        assert_eq!(match_score(&entry, "clipboard"), Some(300));
        assert!(match_score(&entry, "拿铁").is_none());
        let latin = Entry { title: "Visual Studio Code".into(), haystack: "visual studio code".into(), ..entry.clone() };
        assert_eq!(match_score(&latin, "vsc"), Some(600));
    }

    fn assert_inside(rendered: &Rendered, label: &str) {
        let window = Rect::from_min_size(Pos2::ZERO, Vec2::new(WIDTH, HEIGHT));
        for shape in &rendered.shapes {
            let clip = shape.clip_rect;
            assert!(window.expand(2.0).contains_rect(clip), "{label} 裁剪区超出面板：{clip:?}");
            let mut texts = Vec::new();
            collect_text(&shape.shape, &mut texts);
            for text in texts {
                if text.galley.text().is_empty() || !text.galley.size().x.is_finite() {
                    continue;
                }
                let visible = text.visual_bounding_rect().intersect(clip);
                if visible.width() > 0.0 && visible.height() > 0.0 {
                    assert!(
                        window.expand(2.0).contains_rect(visible),
                        "{label} 文字超出面板：{:?} ← {:?}",
                        visible,
                        text.galley.text()
                    );
                }
            }
        }
    }

    #[test]
    fn palette_renders_states_inside_the_window() {
        let mut palette = palette_for_test("");
        let dark = render_palette(&mut palette, true);
        assert_inside(&dark, "深色列表");
        for expected in ["命令面板", "Ctrl+Alt+K", "打开剪贴板历史", "识别剪贴板图片文字", "操作", "页面", "设置", "选择", "执行"] {
            assert!(contains(&dark.shapes, expected), "缺少关键元素：{expected}");
        }
        let light = render_palette(&mut palette, false);
        for expected in ["命令面板", "打开剪贴板历史"] {
            assert!(contains(&light.shapes, expected), "浅色面板缺少：{expected}");
        }
        let mut calc = palette_for_test("12*8");
        let calc = render_palette(&mut calc, true);
        assert_inside(&calc, "计算结果");
        if std::env::var_os("DAOZAI_DEBUG_PANEL").is_some() {
            for shape in &calc.shapes {
                if let egui::Shape::Rect(rect) = &shape.shape {
                    if rect.rect.width() > 200.0 {
                        eprintln!("rect {:?} fill {:?}", rect.rect, rect.fill);
                    }
                }
            }
        }
        assert!(contains(&calc.shapes, "= 96"));
        assert!(contains(&calc.shapes, "计算"));
        let mut empty = palette_for_test("没有这条命令xyz");
        let empty = render_palette(&mut empty, true);
        assert_inside(&empty, "空结果");
        assert!(contains(&empty.shapes, "没有匹配的命令"));
        assert_inside(&light, "浅色列表");
        let mut app = palette_for_test("qq");
        let app = render_palette(&mut app, true);
        assert_inside(&app, "应用搜索");
        assert!(contains(&app.shapes, "QQ"));
        let mut unit = palette_for_test("90f to c");
        let unit = render_palette(&mut unit, true);
        assert_inside(&unit, "单位换算");
        assert!(contains(&unit.shapes, "32.2222"));
        let mut recent_panel = Panel::default();
        recent_panel.set_recent(vec![
            ("微信".into(), std::path::PathBuf::from("C:/Apps/WeChat.exe")),
            ("Visual Studio Code".into(), std::path::PathBuf::from("C:/Apps/Code.exe")),
        ]);
        recent_panel.open(&snapshot());
        recent_panel.fade = 1.0;
        let recent_panel = render_palette(&mut recent_panel, true);
        assert_inside(&recent_panel, "最近启动");
        assert!(contains(&recent_panel.shapes, "最近启动"));
        assert!(contains(&recent_panel.shapes, "微信"));
        let mut clipboard = Panel::default();
        clipboard.open(&snapshot());
        clipboard.set_clipboard(vec![
            ClipboardItem { id: "c1".into(), text: "https://example.com/design 设计文档".into() },
            ClipboardItem { id: "c2".into(), text: "git rebase --onto main".into() },
        ]);
        clipboard.query = "example".into();
        clipboard.fade = 1.0;
        clipboard.rebuild();
        let clipboard = render_palette(&mut clipboard, true);
        assert_inside(&clipboard, "剪贴板结果");
        assert!(contains(&clipboard.shapes, "剪贴板"));
        assert!(contains(&clipboard.shapes, "设计文档"));
        if let Some(dest) = std::env::var_os("DAOZAI_COMMAND_PREVIEW_DIR").filter(|value| !value.is_empty()) {
            let dest = PathBuf::from(dest);
            std::fs::create_dir_all(&dest).unwrap();
            write_png(&dark, &dest.join("command-dark.png"));
            write_png(&light, &dest.join("command-light.png"));
            write_png(&calc, &dest.join("command-calc.png"));
            write_png(&empty, &dest.join("command-empty.png"));
            write_png(&app, &dest.join("command-app.png"));
            write_png(&unit, &dest.join("command-unit.png"));
            write_png(&clipboard, &dest.join("command-clipboard.png"));
            write_png(&recent_panel, &dest.join("command-recent.png"));
        }
    }
}
