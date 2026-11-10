//! 剪贴板历史快捷面板：搜索、选择、预览与粘贴。
//!
//! 面板是一个独立的无边框窗口，视觉语言与设置页一致：石墨底、轻描边、
//! 圆角与柔和阴影；所有交互状态（未开启、暂停、空列表、无匹配、写入失败、
//! 删除撤销）都在同一块面板里给足反馈，不把错误丢到岛体上。
use crate::clipboard::Item;
use crate::panel::{self, chip_button, draw_search_icon, hint, icon_button, keycap, link_button, mix, section_label, with_alpha, Palette};
use crate::theme::{self, W};
use eframe::egui::{
    self, Align, Color32, FontId, Key, Layout, Modifiers, Pos2, Rect, RichText, Sense, Stroke,
    StrokeKind, TextFormat, Ui, UiBuilder, Vec2,
};
use std::sync::Arc;
use std::time::{Duration, Instant};

const WIDTH: f32 = 560.0;
const HEIGHT: f32 = 520.0;
const PAD: f32 = 10.0;
const INNER: f32 = 18.0;
/// 底栏高度：固定贴在面板下沿，列表再长也不会把它挤出窗口。
const FOOTER_H: f32 = 76.0;
const ROW_H: f32 = 54.0;
const UNDO_TTL: Duration = Duration::from_secs(6);
const FADE_SECS: f64 = 0.13;
const MAX_PREVIEW_CHARS: usize = 24_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    None,
    Paste(String),
    CopyOnly(String),
    Delete(String),
    Restore(Item),
    Pin(String, bool),
    Enable,
    Resume,
    SetPaused(bool),
    Clear,
    OpenSettings,
    OcrClipboard,
    /// 识别历史图片里的文字。
    OcrImage(String),
    CropOcr,
    Close { restore: bool },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Recent,
    Pinned,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ItemKind {
    Text,
    Link,
    Code,
    MultiLine,
    Image,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum EmptyKind {
    Lines,
    Search,
    Pin,
    Lock,
}

struct Undo {
    item: Item,
    at: Instant,
}

pub struct Popup {
    pub open: bool,
    pub query: String,
    pub selected: usize,
    selected_id: Option<String>,
    pub items: Vec<Item>,
    pub enabled: bool,
    pub paused: bool,
    pub hotkey_error: Option<String>,
    target_label: String,
    tab: Tab,
    preview_open: bool,
    preview_reset_scroll: bool,
    /// 预览图片的懒加载纹理（key 与 TextureHandle）。
    preview_texture: Option<(String, egui::TextureHandle)>,
    /// 列表图片缩略图纹理。
    thumbnails: std::collections::HashMap<String, egui::TextureHandle>,
    thumb_rx: Option<std::sync::mpsc::Receiver<(String, egui::ColorImage)>>,
    thumb_sig: Vec<(String, u32, u32)>,
    undo: Option<Undo>,
    confirm_clear: bool,
    error: Option<String>,
    position: Option<egui::Pos2>,
    request_focus: bool,
    search_focused: bool,
    scroll_to_selected: bool,
    shown: bool,
    shown_at: Option<f64>,
    had_focus: bool,
    focus_lost_at: Option<Instant>,
    fade: f32,
}

impl Default for Popup {
    fn default() -> Self {
        Self {
            open: false,
            preview_texture: None,
            thumbnails: std::collections::HashMap::new(),
            thumb_rx: None,
            thumb_sig: Vec::new(),
            query: String::new(),
            selected: 0,
            selected_id: None,
            items: Vec::new(),
            enabled: false,
            paused: false,
            hotkey_error: None,
            target_label: "当前应用".into(),
            tab: Tab::Recent,
            preview_open: false,
            preview_reset_scroll: false,
            undo: None,
            confirm_clear: false,
            error: None,
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

impl Popup {
    pub fn open_state(&mut self, enabled: bool, paused: bool, hotkey_error: Option<String>) {
        self.open = true;
        self.query.clear();
        self.selected = 0;
        self.selected_id = None;
        self.enabled = enabled;
        self.paused = paused;
        self.hotkey_error = hotkey_error;
        self.tab = Tab::Recent;
        self.preview_open = false;
        self.preview_reset_scroll = false;
        self.undo = None;
        self.confirm_clear = false;
        self.error = None;
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

    pub fn close(&mut self) {
        self.open = false;
        self.shown = false;
        self.shown_at = None;
        self.position = None;
        self.preview_open = false;
        self.confirm_clear = false;
        self.undo = None;
    }

    pub fn sync(&mut self, enabled: bool, paused: bool, hotkey_error: Option<String>) {
        self.enabled = enabled;
        self.paused = paused;
        self.hotkey_error = hotkey_error;
    }

    pub fn set_target_label(&mut self, label: Option<String>) {
        self.target_label = label
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "当前应用".into());
    }

    /// 写入剪贴板失败等面板内错误：显示在搜索框下方，不自动消失。
    pub fn set_error(&mut self, message: impl Into<String>) {
        let message = message.into();
        if !message.trim().is_empty() {
            self.error = Some(message);
        }
    }

    pub fn clear_error(&mut self) {
        self.error = None;
    }

    pub fn set_items(&mut self, items: Vec<Item>) {
        let keep_id = self.selected_id.clone();
        self.items = items;
        self.selected_id = keep_id.filter(|id| self.items.iter().any(|item| &item.id == id));
        if self.preview_open && self.selected_item().is_none() {
            self.preview_open = false;
        }
        self.sync_selection();
        self.refresh_thumbnails();
    }

    /// 重新生成图片条目缩略图（后台线程解密/解码，不阻塞面板）。
    fn refresh_thumbnails(&mut self) {
        let keys: Vec<(String, crate::clipboard::ImageRef)> = self
            .items
            .iter()
            .filter_map(|item| item.image.as_ref().map(|image| (item.id.clone(), image.clone())))
            .take(crate::clipboard::MAX_IMAGE_ITEMS)
            .collect();
        let sig: Vec<(String, u32, u32)> = keys
            .iter()
            .map(|(key, image)| (key.clone(), image.width, image.height))
            .collect();
        if sig == self.thumb_sig {
            return;
        }
        self.thumb_sig = sig;
        self.thumbnails.clear();
        if keys.is_empty() {
            self.thumb_rx = None;
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        self.thumb_rx = Some(rx);
        std::thread::Builder::new()
            .name("daozai-clipboard-thumbs".into())
            .spawn(move || {
                for (key, image_ref) in keys {
                    let Some(image) = crate::clipboard::load_image(&image_ref) else {
                        continue;
                    };
                    let thumb = crate::clipboard::thumbnail(&image, 72);
                    let color = egui::ColorImage::from_rgba_unmultiplied(
                        [thumb.width as usize, thumb.height as usize],
                        &thumb.rgba,
                    );
                    if tx.send((key, color)).is_err() {
                        break;
                    }
                }
            })
            .ok();
    }

    /// 收取后台缩略图并上传纹理。
    fn poll_thumbnails(&mut self, ctx: &egui::Context) {
        let mut received = Vec::new();
        let mut disconnected = false;
        if let Some(rx) = self.thumb_rx.as_ref() {
            loop {
                match rx.try_recv() {
                    Ok(item) => received.push(item),
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }
        }
        for (key, pixels) in received {
            let texture = ctx.load_texture(
                format!("clipboard-thumb-{key}"),
                pixels,
                egui::TextureOptions::LINEAR,
            );
            self.thumbnails.insert(key, texture);
        }
        if disconnected {
            self.thumb_rx = None;
        }
    }
    pub fn filtered(&self) -> Vec<usize> {
        let needle = self.query.trim().to_lowercase();
        self.items
            .iter()
            .enumerate()
            .filter(|(_, item)| {
                if self.tab == Tab::Pinned && !item.pinned {
                    return false;
                }
                needle.is_empty() || item.text.to_lowercase().contains(&needle)
            })
            .map(|(index, _)| index)
            .collect()
    }

    pub fn clamp(&mut self) {
        self.sync_selection();
    }

    fn sync_selection(&mut self) {
        let indices = self.filtered();
        if indices.is_empty() {
            self.selected = 0;
            self.selected_id = None;
            return;
        }
        if let Some(id) = self.selected_id.as_ref() {
            if let Some(position) = indices
                .iter()
                .position(|index| self.items[*index].id == *id)
            {
                self.selected = position;
                return;
            }
        }
        self.selected = self.selected.min(indices.len() - 1);
        self.selected_id = Some(self.items[indices[self.selected]].id.clone());
    }

    pub fn move_selection(&mut self, delta: isize) {
        let indices = self.filtered();
        if indices.is_empty() {
            self.selected = 0;
            self.selected_id = None;
            return;
        }
        let next = (self.selected as isize + delta).clamp(0, indices.len() as isize - 1) as usize;
        self.selected = next;
        self.selected_id = Some(self.items[indices[next]].id.clone());
    }

    pub fn selected_id(&self) -> Option<String> {
        self.selected_id.clone()
    }

    pub fn selected_item(&self) -> Option<&Item> {
        let id = self.selected_id.as_ref()?;
        self.items.iter().find(|item| &item.id == id)
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

    fn stage_delete(&mut self, item: &Item) -> Action {
        self.undo = Some(Undo { item: item.clone(), at: Instant::now() });
        self.selected_id = Some(item.id.clone());
        self.sync_selection();
        Action::Delete(item.id.clone())
    }

    fn take_undo(&mut self) -> Action {
        match self.undo.take() {
            Some(undo) => Action::Restore(undo.item),
            None => Action::None,
        }
    }

    fn expire_undo(&mut self) {
        if self.undo.as_ref().is_some_and(|undo| undo.at.elapsed() >= UNDO_TTL) {
            self.undo = None;
        }
    }

    /// 独立窗口容器：负责定位、焦点、失焦关闭和入场淡入。
    pub fn show(&mut self, ctx: &egui::Context, dark: bool) -> Action {
        if !self.open {
            return Action::None;
        }
        self.expire_undo();
        let viewport_id = egui::ViewportId::from_hash_of("daozai-clipboard");
        let ppp = ctx.pixels_per_point();
        #[cfg(windows)]
        let initial_position = crate::native::clipboard_popup_position(WIDTH, HEIGHT, ppp)
            .map(|(x, y)| egui::pos2(x, y))
            .unwrap_or_else(|| egui::pos2(120.0, 70.0));
        #[cfg(not(windows))]
        let initial_position = {
            let _ = ppp;
            egui::pos2(120.0, 70.0)
        };
        let mut action = Action::None;
        let position = *self.position.get_or_insert(initial_position);
        let builder = egui::ViewportBuilder::default()
            .with_title("剪贴板 · 岛仔")
            .with_icon(theme::app_icon())
            .with_inner_size([WIDTH, HEIGHT])
            .with_min_inner_size([420.0, 420.0])
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
            let shown_at = *self.shown_at.get_or_insert(now);
            let progress = ((now - shown_at) / FADE_SECS).clamp(0.0, 1.0) as f32;
            self.fade = if progress >= 1.0 { 1.0 } else { crate::anim::ease_out_cubic(progress) };
            if progress < 1.0 {
                ctx.request_repaint();
            }
            if ctx.input(|input| input.viewport().close_requested()) {
                action = Action::Close { restore: false };
            }
            let focused = ctx.input(|input| input.viewport().focused.unwrap_or(false));
            if self.observe_focus(focused, Instant::now()) {
                action = Action::Close { restore: false };
            }
            let panel_action = self.panel_ui(ctx, dark);
            if action == Action::None {
                action = panel_action;
            }
        });
        if matches!(action, Action::Paste(_) | Action::CopyOnly(_) | Action::Restore(_)) {
            self.error = None;
        }
        action
    }

    /// 面板本体。拆出来是为了让录制器与测试不必真的创建窗口也能渲染和交互。
    pub fn panel_ui(&mut self, ctx: &egui::Context, dark: bool) -> Action {
        self.poll_thumbnails(ctx);
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
        let focused = self.search_box(ui, p);
        if !focused {
            let (delete, space) = ctx.input_mut(|input| {
                (
                    input.consume_key(Modifiers::NONE, Key::Delete),
                    input.consume_key(Modifiers::NONE, Key::Space),
                )
            });
            if delete && self.enabled && action == Action::None {
                if let Some(item) = self.selected_item().cloned() {
                    action = self.stage_delete(&item);
                }
            }
            if space && self.enabled {
                if self.selected_item().is_some() {
                    self.preview_open = !self.preview_open;
                    self.preview_reset_scroll = true;
                }
            }
        }
        self.status_line(ui, p);
        if self.enabled {
            ui.add_space(10.0);
            let tab_action = self.tabs(ui, p);
            if action == Action::None {
                action = tab_action;
            }
            ui.add_space(8.0);
        } else {
            ui.add_space(12.0);
        }
        let left = ui.max_rect().left();
        let right = ui.max_rect().right();
        let bottom = ui.max_rect().bottom();
        let line_y = ui.cursor().top();
        ui.painter().line_segment(
            [Pos2::new(left, line_y), Pos2::new(right, line_y)],
            Stroke::new(1.0_f32, p.line_soft),
        );
        ui.add_space(7.0);
        let body_top = ui.cursor().top();
        let footer_top = (bottom - FOOTER_H).max(body_top + 96.0);
        let body = Rect::from_min_max(
            Pos2::new(left, body_top),
            Pos2::new(right, (footer_top - 8.0).max(body_top + 64.0)),
        );
        let body_action = if !self.enabled {
            if self.empty_state(
                ui,
                body,
                p,
                EmptyKind::Lock,
                "剪贴板历史未开启",
                "开启后，复制的文本会加密保存在本机",
                Some("开启剪贴板历史"),
            ) {
                Action::Enable
            } else {
                Action::None
            }
        } else if self.preview_open {
            self.preview_body(ui, body, p)
        } else {
            self.list_body(ui, body, p)
        };
        if action == Action::None {
            action = body_action;
        }
        let footer = Rect::from_min_max(Pos2::new(left, footer_top), Pos2::new(right, bottom));
        let footer_action = self.footer(ui, footer, p);
        if action == Action::None {
            action = footer_action;
        }
        action
    }

    /// ↑↓ 选择、Enter 粘贴、Ctrl+Enter 仅复制、Esc 关闭；要在输入框之前消费，
    /// 否则搜索框会把方向键和回车先吃掉。
    fn consume_navigation(&mut self, ctx: &egui::Context) -> Action {
        let (up, down, enter, ctrl_enter, escape) = ctx.input_mut(|input| {
            (
                input.consume_key(Modifiers::NONE, Key::ArrowUp),
                input.consume_key(Modifiers::NONE, Key::ArrowDown),
                input.consume_key(Modifiers::NONE, Key::Enter),
                input.consume_key(Modifiers::CTRL, Key::Enter),
                input.consume_key(Modifiers::NONE, Key::Escape),
            )
        });
        let mut action = Action::None;
        if escape {
            if self.confirm_clear {
                self.confirm_clear = false;
            } else if self.preview_open {
                self.preview_open = false;
            } else {
                action = Action::Close { restore: true };
            }
        }
        if up {
            self.move_selection(-1);
            self.scroll_to_selected = true;
            self.preview_reset_scroll = true;
        }
        if down {
            self.move_selection(1);
            self.scroll_to_selected = true;
            self.preview_reset_scroll = true;
        }
        if ctrl_enter {
            if self.enabled {
                if let Some(id) = self.selected_id() {
                    action = Action::CopyOnly(id);
                }
            }
        } else if enter && self.enabled {
            if let Some(id) = self.selected_id() {
                action = Action::Paste(id);
            }
        }
        action
    }

    fn header(&mut self, ui: &mut Ui, p: &Palette) -> Action {
        let mut action = Action::None;
        let paused = self.paused;
        let mut confirm_clear = false;
        ui.horizontal(|ui| {
            let (icon_rect, _) = ui.allocate_exact_size(Vec2::splat(23.0), Sense::hover());
            draw_clipboard_icon(ui, icon_rect, p.accent);
            ui.add_space(2.0);
            ui.label(RichText::new("剪贴板").font(theme::font(16.0, W::Semibold)).color(p.text));
            ui.add_space(6.0);
            keycap(ui, "Ctrl+Alt+V", p);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if icon_button(ui, "×", p.muted, p.text, p.hover, 16.0) {
                    action = Action::Close { restore: true };
                }
                ui.add_space(2.0);
                ui.menu_button(
                    RichText::new("•••").font(theme::font(13.0, W::Semibold)).color(p.muted),
                    |ui| {
                        ui.set_min_width(176.0);
                        ui.spacing_mut().button_padding = Vec2::new(10.0, 7.0);
                        if ui
                            .button(if paused { "恢复记录" } else { "暂停记录" })
                            .clicked()
                        {
                            action = Action::SetPaused(!paused);
                            ui.close_menu();
                        }
                        if ui.button("清空全部…").clicked() {
                            confirm_clear = true;
                            ui.close_menu();
                        }
                        if ui.button("识别图片文字…").clicked() {
                            action = Action::OcrClipboard;
                            ui.close_menu();
                        }
                        if ui.button("截屏框选取字…").clicked() {
                            action = Action::CropOcr;
                            ui.close_menu();
                        }
                        ui.separator();
                        if ui.button("打开设置").clicked() {
                            action = Action::OpenSettings;
                            ui.close_menu();
                        }
                        if ui.button("关闭").clicked() {
                            action = Action::Close { restore: true };
                            ui.close_menu();
                        }
                    },
                );
            });
        });
        if confirm_clear {
            self.confirm_clear = true;
        }
        action
    }

    fn search_box(&mut self, ui: &mut Ui, p: &Palette) -> bool {
        // 搜索框没焦点时，用户直接敲字也应当进入搜索：把这一帧的文字事件手动拼进来，
        // 同时请求焦点，下一帧起恢复正常输入。
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
                self.selected = 0;
                self.selected_id = None;
                self.preview_open = false;
                self.sync_selection();
                self.request_focus = true;
            }
        }
        let previous = self.query.clone();
        let mut focused = false;
        let mut clear = false;
        let frame = egui::Frame::new()
            .fill(p.raised)
            .stroke(Stroke::NONE)
            .corner_radius(10.0)
            .inner_margin(egui::Margin::symmetric(11, 9));
        let inner = frame.show(ui, |ui| {
            ui.horizontal(|ui| {
                let (icon_rect, _) = ui.allocate_exact_size(Vec2::splat(20.0), Sense::hover());
                draw_search_icon(ui, icon_rect, p.faint);
                ui.add_space(1.0);
                // 有内容时为右侧清除按钮留位置；没有内容时占满整行，避免视觉上多出一段空白。
                let reserve = if self.query.is_empty() { 8.0 } else { 40.0 };
                let edit_width = (ui.available_width() - reserve).max(80.0);
                let response = ui.add(
                    egui::TextEdit::singleline(&mut self.query)
                        .id_source("clipboard-search")
                        .hint_text(
                            RichText::new("搜索复制过的内容…")
                                .font(theme::font(13.0, W::Regular))
                                .color(p.faint),
                        )
                        .font(theme::font(13.0, W::Regular))
                        .text_color(p.text)
                        .frame(false)
                        .desired_width(edit_width),
                );
                focused = response.has_focus();
                if self.request_focus {
                    response.request_focus();
                    self.request_focus = false;
                    self.search_focused = true;
                    focused = true;
                }
                if !self.query.is_empty() {
                    ui.add_space(2.0);
                    if icon_button(ui, "×", p.faint, p.muted, p.hover, 12.0) {
                        clear = true;
                    }
                }
            });
        });
        let rect = inner.response.rect;
        let border = if focused { p.accent } else { p.line };
        ui.painter().rect_stroke(
            rect,
            10.0,
            Stroke::new(if focused { 1.2_f32 } else { 1.0_f32 }, border),
            StrokeKind::Inside,
        );
        if clear {
            self.query.clear();
            self.selected = 0;
            self.selected_id = None;
            self.sync_selection();
            self.request_focus = true;
        }
        if self.query != previous {
            self.selected = 0;
            self.selected_id = None;
            self.preview_open = false;
            self.sync_selection();
        }
        self.search_focused = focused;
        focused
    }

    fn status_line(&mut self, ui: &mut Ui, p: &Palette) {
        let error = self.error.clone();
        let is_error = error.is_some();
        let hotkey = self.hotkey_error.clone().filter(|_| error.is_none());
        let Some((message, color)) = error
            .map(|message| (message, p.danger))
            .or_else(|| hotkey.map(|message| (message, p.warn)))
        else {
            return;
        };
        let mut dismiss = false;
        ui.add_space(7.0);
        ui.horizontal(|ui| {
            ui.add_space(1.0);
            let (dot_rect, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
            ui.painter().circle_filled(dot_rect.center(), 2.6, color);
            ui.label(RichText::new(&message).font(theme::font(11.0, W::Regular)).color(color));
            if is_error {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui
                        .add(egui::Button::new(RichText::new("知道了").font(theme::font(10.5, W::Regular)).color(p.muted)).frame(false))
                        .clicked()
                    {
                        dismiss = true;
                    }
                });
            }
        });
        if dismiss {
            self.error = None;
        }
    }

    fn tabs(&mut self, ui: &mut Ui, p: &Palette) -> Action {
        let mut action = Action::None;
        let total = self.items.len();
        let pinned = self.items.iter().filter(|item| item.pinned).count();
        let mut next: Option<Tab> = None;
        let mut resume = false;
        let paused = self.paused;
        ui.horizontal(|ui| {
            if tab_button(ui, p, "最近", total, self.tab == Tab::Recent) {
                next = Some(Tab::Recent);
            }
            ui.add_space(10.0);
            if tab_button(ui, p, "置顶", pinned, self.tab == Tab::Pinned) {
                next = Some(Tab::Pinned);
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if paused {
                    if chip_button(ui, "已暂停 · 恢复", p.warn, with_alpha(p.warn, 0.14)) {
                        resume = true;
                    }
                } else if total > 0 {
                    ui.label(RichText::new(format!("共 {total} 条")).font(theme::font(10.5, W::Regular)).color(p.faint));
                }
            });
        });
        if let Some(tab) = next {
            if tab != self.tab {
                self.tab = tab;
                self.selected = 0;
                self.selected_id = None;
                self.preview_open = false;
                self.sync_selection();
                self.scroll_to_selected = true;
            }
        }
        if resume {
            action = Action::Resume;
        }
        action
    }

    fn list_body(&mut self, ui: &mut Ui, rect: Rect, p: &Palette) -> Action {
        let mut action = Action::None;
        let query = self.query.clone();
        let now = chrono::Local::now().timestamp();
        let indices = self.filtered();
        if indices.is_empty() {
            return self.empty_for_query(ui, rect, p);
        }
        let pinned_count = indices.iter().filter(|index| self.items[**index].pinned).count();
        let show_recent_label = pinned_count > 0;
        let mut needs_fade = false;
        ui.scope_builder(UiBuilder::new().max_rect(rect), |ui| {
            ui.set_width(rect.width());
            ui.spacing_mut().item_spacing = Vec2::new(0.0, 2.0);
            ui.style_mut().spacing.scroll.floating = false;
            let output = egui::ScrollArea::vertical()
                .id_salt("clipboard-list")
                .auto_shrink([false, false])
                .max_height(rect.height())
                .scroll_bar_visibility(
                    egui::containers::scroll_area::ScrollBarVisibility::VisibleWhenNeeded,
                )
                .show(ui, |ui| {
                    let mut pinned_label = false;
                    let mut recent_label = false;
                    for index in indices {
                        // 避免每帧克隆整条文本（单条可达 32 KB、200 条时非常可观）。
                        let item = &self.items[index];
                        if item.pinned && !pinned_label {
                            section_label(ui, "置顶", p);
                            pinned_label = true;
                        }
                        if !item.pinned && !recent_label && show_recent_label {
                            section_label(ui, "最近复制", p);
                            recent_label = true;
                        }
                        let selected = self.selected_id.as_deref() == Some(item.id.as_str());
                        let scroll_into_view = selected && self.scroll_to_selected;
                        let thumbnail = self.thumbnails.get(&item.id).cloned();
                        let next = row(ui, item, thumbnail.as_ref(), selected, p, &query, now, scroll_into_view);
                        match next {
                            Some(RowAction::Select) => {
                                let id = item.id.clone();
                                self.selected_id = Some(id);
                                self.sync_selection();
                                self.preview_reset_scroll = true;
                            }
                            Some(RowAction::Paste) => {
                                let id = item.id.clone();
                                self.selected_id = Some(id.clone());
                                action = Action::Paste(id);
                            }
                            Some(RowAction::CopyOnly) => {
                                let id = item.id.clone();
                                self.selected_id = Some(id.clone());
                                action = Action::CopyOnly(id);
                            }
                            Some(RowAction::Preview) => {
                                let id = item.id.clone();
                                self.selected_id = Some(id);
                                self.sync_selection();
                                self.preview_open = true;
                                self.preview_reset_scroll = true;
                            }
                            Some(RowAction::Pin) => {
                                let id = item.id.clone();
                                let pinned = item.pinned;
                                action = Action::Pin(id, !pinned);
                            }
                            Some(RowAction::Delete) => {
                                let copy = item.clone();
                                action = self.stage_delete(&copy);
                            }
                            Some(RowAction::OcrImage) => {
                                action = Action::OcrImage(item.id.clone());
                            }
                            None => {}
                        }
                    }
                    ui.add_space(4.0);
                });
            needs_fade = output.content_size.y > output.inner_rect.height() + 1.0;
        });
        if needs_fade {
            for step in 0..6 {
                let k = step as f32 / 5.0;
                let strip_h = 3.0;
                let y = rect.bottom() - 18.0 + step as f32 * strip_h;
                ui.painter().rect_filled(
                    Rect::from_min_max(Pos2::new(rect.left(), y), Pos2::new(rect.right(), y + strip_h)),
                    0.0,
                    with_alpha(p.surface, 0.04 + 0.10 * k),
                );
            }
        }
        self.scroll_to_selected = false;
        action
    }

    fn empty_for_query(&mut self, ui: &mut Ui, rect: Rect, p: &Palette) -> Action {
        let trimmed = self.query.trim().to_string();
        if trimmed.is_empty() {
            if self.tab == Tab::Pinned {
                self.empty_state(
                    ui,
                    rect,
                    p,
                    EmptyKind::Pin,
                    "还没有置顶的内容",
                    "在条目上点右键，选择「置顶」",
                    None,
                );
            } else {
                self.empty_state(
                    ui,
                    rect,
                    p,
                    EmptyKind::Lines,
                    "还没有记录",
                    "复制一段文字后，它会出现在这里",
                    None,
                );
            }
            return Action::None;
        }
        let subtitle = format!("没有找到包含「{trimmed}」的记录");
        let clicked = self.empty_state(
            ui,
            rect,
            p,
            EmptyKind::Search,
            "没有匹配的内容",
            &subtitle,
            Some("清除搜索"),
        );
        if clicked {
            self.query.clear();
            self.tab = Tab::Recent;
            self.selected = 0;
            self.selected_id = None;
            self.sync_selection();
            self.request_focus = true;
        }
        Action::None
    }

    #[allow(clippy::too_many_arguments)]
    fn empty_state(
        &mut self,
        ui: &mut Ui,
        rect: Rect,
        p: &Palette,
        kind: EmptyKind,
        title: &str,
        subtitle: &str,
        action_label: Option<&str>,
    ) -> bool {
        let mut clicked = false;
        ui.scope_builder(
            UiBuilder::new().max_rect(rect).layout(Layout::top_down(Align::Center)),
            |ui| {
                ui.set_width(rect.width());
                ui.add_space((rect.height() * 0.28).clamp(12.0, 86.0));
                let (icon_rect, _) = ui.allocate_exact_size(Vec2::splat(46.0), Sense::hover());
                draw_empty_icon(ui.painter(), icon_rect, p, kind);
                ui.add_space(12.0);
                ui.label(RichText::new(title).font(theme::font(14.0, W::Semibold)).color(p.text));
                ui.add_space(4.0);
                ui.label(RichText::new(subtitle).font(theme::font(11.5, W::Regular)).color(p.muted));
                if let Some(label) = action_label {
                    ui.add_space(14.0);
                    let button = egui::Button::new(
                        RichText::new(label).font(theme::font(12.0, W::Semibold)).color(Color32::WHITE),
                    )
                    .fill(p.accent)
                    .corner_radius(8.0)
                    .min_size(Vec2::new(0.0, 30.0));
                    if ui.add(button).clicked() {
                        clicked = true;
                    }
                }
            },
        );
        clicked
    }

    fn preview_body(&mut self, ui: &mut Ui, rect: Rect, p: &Palette) -> Action {
        let mut action = Action::None;
        let Some(item) = self.selected_item().cloned() else {
            self.preview_open = false;
            return Action::None;
        };
        let kind = if item.is_image() { ItemKind::Image } else { item_kind(&item.text) };
        ui.scope_builder(UiBuilder::new().max_rect(rect), |ui| {
            ui.set_width(rect.width());
            egui::Frame::new()
                .fill(p.raised)
                .stroke(Stroke::new(1.0_f32, p.line_soft))
                .corner_radius(12.0)
                .inner_margin(13.0)
                .show(ui, |ui| {
                    ui.set_min_size(ui.available_size());
                    ui.horizontal(|ui| {
                        if icon_button(ui, "←", p.muted, p.text, p.hover, 14.0) {
                            self.preview_open = false;
                        }
                        ui.add_space(2.0);
                        ui.label(RichText::new("预览").font(theme::font(12.0, W::Semibold)).color(p.text));
                        ui.add_space(2.0);
                        ui.label(
                            RichText::new(meta_label(&item, kind_label(kind)))
                                .font(theme::font(10.5, W::Regular))
                                .color(p.faint),
                        );
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if icon_button(ui, "×", p.muted, p.text, p.hover, 14.0) {
                                self.preview_open = false;
                            }
                            if item.image.is_some()
                                && ui
                                    .add(
                                        egui::Button::new(
                                            RichText::new("识别文字")
                                                .font(theme::font(10.5, W::Semibold))
                                                .color(p.accent),
                                        )
                                        .frame(false),
                                    )
                                    .on_hover_text("用 Windows 本地 OCR 识别这张图片")
                                    .clicked()
                            {
                                action = Action::OcrImage(item.id.clone());
                            }
                        });
                    });
                    ui.add_space(9.0);
                    let width = ui.available_width();
                    let y = ui.cursor().top();
                    ui.painter().line_segment(
                        [Pos2::new(ui.cursor().left(), y), Pos2::new(ui.cursor().left() + width, y)],
                        Stroke::new(1.0_f32, p.line_soft),
                    );
                    ui.add_space(10.0);
                    let height = ui.available_height().max(40.0);
                    if let Some(image_ref) = item.image.as_ref() {
                        let key = item.id.clone();
                        if self
                            .preview_texture
                            .as_ref()
                            .map(|(cached, _)| cached != &key)
                            .unwrap_or(true)
                        {
                            self.preview_texture = None;
                            if let Some(image) = crate::clipboard::load_image(image_ref) {
                                // 预览最大 1200px，既清晰又把纹理内存控制在小面板量级。
                                let display = crate::clipboard::thumbnail(&image, 1200);
                                let pixels = egui::ColorImage::from_rgba_unmultiplied(
                                    [display.width as usize, display.height as usize],
                                    &display.rgba,
                                );
                                let texture = ui.ctx().load_texture(
                                    format!("clipboard-image-{key}"),
                                    pixels,
                                    egui::TextureOptions::LINEAR,
                                );
                                self.preview_texture = Some((key, texture));
                            }
                        }
                        ui.vertical_centered(|ui| {
                            if let Some((_, texture)) = &self.preview_texture {
                                let available = Vec2::new(width, (height - 26.0).max(60.0));
                                let size = texture.size_vec2();
                                let scale = (available.x / size.x)
                                    .min(available.y / size.y)
                                    .min(1.0)
                                    .max(0.02);
                                ui.add(
                                    egui::Image::new(texture)
                                        .fit_to_exact_size(size * scale)
                                        .corner_radius(8.0),
                                );
                                ui.add_space(6.0);
                                ui.label(
                                    RichText::new("粘贴时写入原图；图片经 DPAPI 加密保存在本机")
                                        .font(theme::font(10.5, W::Regular))
                                        .color(p.faint),
                                );
                            } else {
                                ui.add_space((height * 0.3).min(80.0));
                                ui.label(
                                    RichText::new("图片读取失败，可能已被清理")
                                        .font(theme::font(12.0, W::Regular))
                                        .color(p.muted),
                                );
                            }
                        });
                    } else {
                        let text = if item.text.chars().count() > MAX_PREVIEW_CHARS {
                            let mut text: String = item.text.chars().take(MAX_PREVIEW_CHARS).collect();
                            text.push_str("\n…");
                            text
                        } else {
                            item.text.clone()
                        };
                        let mut area = egui::ScrollArea::vertical()
                            .id_salt("clipboard-preview")
                            .auto_shrink([false, false])
                            .max_height(height);
                        if std::mem::take(&mut self.preview_reset_scroll) {
                            area = area.vertical_scroll_offset(0.0);
                        }
                        area.show(ui, |ui| {
                            let font = match kind {
                                ItemKind::Code | ItemKind::Link => FontId::monospace(12.0),
                                _ => theme::font(12.5, W::Regular),
                            };
                            ui.add(
                                egui::Label::new(RichText::new(text).font(font).color(p.text))
                                    .selectable(true)
                                    .wrap(),
                            );
                        });
                    }
                });
        });
        action
    }

    fn footer(&mut self, ui: &mut Ui, rect: Rect, p: &Palette) -> Action {
        let mut action = Action::None;
        let mut cancel_clear = false;
        let mut confirm = false;
        let mut undo = false;
        let mut resume = false;
        let selected = self.enabled && self.selected_id().is_some();
        let count = self.items.len();
        let confirm_clear = self.confirm_clear;
        let has_undo = self.undo.is_some();
        let paused = self.paused;
        ui.scope_builder(UiBuilder::new().max_rect(rect), |ui| {
            ui.set_width(rect.width());
            let left = rect.left();
            let right = left + rect.width();
            let top = rect.top();
            ui.painter().line_segment(
                [Pos2::new(left, top), Pos2::new(right, top)],
                Stroke::new(1.0_f32, p.line_soft),
            );
            ui.add_space(11.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("粘贴到").font(theme::font(11.5, W::Regular)).color(p.muted));
                target_chip(ui, &self.target_label, p);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let paste = egui::Button::new(
                        RichText::new("粘贴").font(theme::font(12.5, W::Semibold)).color(Color32::WHITE),
                    )
                    .fill(p.accent)
                    .corner_radius(8.0)
                    .min_size(Vec2::new(72.0, 30.0));
                    if ui.add_enabled(selected, paste).clicked() {
                        if let Some(id) = self.selected_id() {
                            action = Action::Paste(id);
                        }
                    }
                    keycap(ui, "Enter", p);
                    ui.add_space(10.0);
                    let copy = egui::Button::new(
                        RichText::new("仅复制").font(theme::font(11.5, W::Regular)).color(p.muted),
                    )
                    .frame(false);
                    if ui.add_enabled(selected, copy).clicked() {
                        if let Some(id) = self.selected_id() {
                            action = Action::CopyOnly(id);
                        }
                    }
                    keycap(ui, "Ctrl+Enter", p);
                });
            });
            ui.add_space(9.0);
            ui.horizontal(|ui| {
                if confirm_clear {
                    ui.label(
                        RichText::new(format!("清空全部 {count} 条记录？"))
                            .font(theme::font(11.5, W::Regular))
                            .color(p.text),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if chip_button(ui, "清空", p.danger, with_alpha(p.danger, 0.16)) {
                            confirm = true;
                        }
                        if chip_button(ui, "取消", p.muted, p.hover) {
                            cancel_clear = true;
                        }
                    });
                } else if has_undo {
                    ui.label(RichText::new("已删除 1 条记录").font(theme::font(11.5, W::Regular)).color(p.muted));
                    if link_button(ui, "撤销", p) {
                        undo = true;
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(RichText::new("6 秒内可恢复").font(theme::font(10.5, W::Regular)).color(p.faint));
                    });
                } else if !self.enabled {
                    ui.label(
                        RichText::new("开启剪贴板历史后，可使用粘贴、预览、置顶与撤销。")
                            .font(theme::font(10.5, W::Regular))
                            .color(p.faint),
                    );
                } else {
                    hint(ui, "↑↓", "选择", p);
                    ui.add_space(8.0);
                    hint(ui, "Del", "删除", p);
                    ui.add_space(8.0);
                    hint(ui, "Esc", "关闭", p);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if paused {
                            if chip_button(ui, "已暂停 · 恢复", p.warn, with_alpha(p.warn, 0.14)) {
                                resume = true;
                            }
                        } else {
                            ui.label(
                                RichText::new("右键条目查看更多操作")
                                    .font(theme::font(10.5, W::Regular))
                                    .color(p.faint),
                            );
                        }
                    });
                }
            });
        });
        if confirm {
            self.confirm_clear = false;
            action = Action::Clear;
        }
        if cancel_clear {
            self.confirm_clear = false;
        }
        if undo {
            action = self.take_undo();
        }
        if resume && action == Action::None {
            action = Action::Resume;
        }
        action
    }
}

// ---------------------------------------------------------------- 行与菜单

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RowAction {
    Select,
    Paste,
    CopyOnly,
    Delete,
    Pin,
    Preview,
    OcrImage,
}

fn row(
    ui: &mut Ui,
    item: &Item,
    thumbnail: Option<&egui::TextureHandle>,
    selected: bool,
    p: &Palette,
    query: &str,
    now: i64,
    scroll_into_view: bool,
) -> Option<RowAction> {
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, ROW_H), Sense::click());
    if !ui.is_rect_visible(rect) {
        return None;
    }
    if scroll_into_view {
        ui.scroll_to_rect(rect, Some(Align::Center));
    }
    let ctx = ui.ctx().clone();
    let hover = ctx.animate_bool_with_time(
        egui::Id::new(("clipboard-row-hover", item.id.as_str())),
        response.hovered() && !selected,
        0.10,
    );
    let painter = ui.painter().with_clip_rect(rect);
    let radius = 9.0;
    let body = rect.shrink2(Vec2::new(0.0, 1.0));
    if selected {
        painter.rect_filled(body, radius, mix(p.surface, p.accent, 0.13));
        painter.rect_stroke(
            body,
            radius,
            Stroke::new(1.0_f32, with_alpha(p.accent, 0.35)),
            StrokeKind::Inside,
        );
        painter.rect_filled(
            Rect::from_min_size(Pos2::new(rect.left() + 1.5, rect.top() + 12.0), Vec2::new(3.0, ROW_H - 24.0)),
            1.5,
            p.accent,
        );
    } else if hover > 0.01 {
        painter.rect_filled(body, radius, mix(p.surface, p.hover, hover));
    }
    let kind = if item.is_image() { ItemKind::Image } else { item_kind(&item.text) };
    let icon_rect = Rect::from_min_size(
        Pos2::new(rect.left() + 10.0, rect.center().y - 17.0),
        Vec2::splat(34.0),
    );
    let tile = if selected { mix(p.raised, p.accent, 0.20) } else { p.raised };
    painter.rect_filled(icon_rect, 9.0, tile);
    if let Some(texture) = thumbnail {
        let image_rect = icon_rect.shrink(4.0);
        painter.image(
            texture.id(),
            image_rect,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            Color32::WHITE,
        );
        painter.rect_stroke(
            image_rect,
            6.0,
            Stroke::new(1.0_f32, with_alpha(p.line, 0.9)),
            StrokeKind::Inside,
        );
    } else {
        draw_kind_icon(&painter, icon_rect, kind, if selected { p.accent } else { p.faint });
    }
    let (line, range) = display_line(&item.text, query);
    let title_x = icon_rect.right() + 11.0;
    let chip_space = if item.pinned { 42.0 } else { 6.0 };
    let max_title = (rect.right() - 14.0 - chip_space - title_x).max(60.0);
    let font = theme::font(13.0, if selected { W::Semibold } else { W::Regular });
    let galley = title_galley(ui, &line, range, font, p.text, p.accent, max_title);
    let truncated = galley.text() != line;
    painter.galley(Pos2::new(title_x, rect.center().y - 13.0), galley, p.text);
    painter.text(
        Pos2::new(title_x, rect.center().y + 5.0),
        egui::Align2::LEFT_TOP,
        meta_line(item, kind, now),
        theme::font(10.5, W::Regular),
        p.faint,
    );
    if item.pinned {
        let chip_rect = Rect::from_min_max(
            Pos2::new(rect.right() - 40.0, rect.center().y - 8.0),
            Pos2::new(rect.right() - 12.0, rect.center().y + 8.0),
        );
        painter.rect_filled(chip_rect, 4.0, with_alpha(p.warn, 0.15));
        painter.text(
            chip_rect.center(),
            egui::Align2::CENTER_CENTER,
            "置顶",
            theme::font(9.5, W::Semibold),
            p.warn,
        );
    }
    let mut out = None;
    if response.clicked() {
        out = Some(RowAction::Select);
    }
    if response.double_clicked() {
        out = Some(RowAction::Paste);
    }
    let _ = response.clone().on_hover_cursor(egui::CursorIcon::PointingHand);
    if truncated {
        let full: String = item.text.chars().take(600).collect();
        response.clone().on_hover_text(full);
    }
    response.context_menu(|ui| {
        ui.set_min_width(150.0);
        ui.spacing_mut().button_padding = Vec2::new(10.0, 6.0);
        if ui.button("粘贴").clicked() {
            out = Some(RowAction::Paste);
            ui.close_menu();
        }
        if ui.button("仅复制").clicked() {
            out = Some(RowAction::CopyOnly);
            ui.close_menu();
        }
        if ui.button("预览").clicked() {
            out = Some(RowAction::Preview);
            ui.close_menu();
        }
        if item.image.is_some() && ui.button("识别图片文字").clicked() {
            out = Some(RowAction::OcrImage);
            ui.close_menu();
        }
        ui.separator();
        if ui.button(if item.pinned { "取消置顶" } else { "置顶" }).clicked() {
            out = Some(RowAction::Pin);
            ui.close_menu();
        }
        ui.separator();
        if ui
            .add(egui::Button::new(
                RichText::new("删除").font(theme::font(12.0, W::Regular)).color(p.danger),
            ))
            .clicked()
        {
            out = Some(RowAction::Delete);
            ui.close_menu();
        }
    });
    out
}

// ---------------------------------------------------------------- 文案与度量

fn item_kind(text: &str) -> ItemKind {
    let trimmed = text.trim_start();
    if trimmed.starts_with("http://")
        || trimmed.starts_with("https://")
        || trimmed.starts_with("www.")
        || trimmed.starts_with("mailto:")
    {
        return ItemKind::Link;
    }
    if text.contains('\n') {
        return ItemKind::MultiLine;
    }
    if looks_like_code(text) {
        return ItemKind::Code;
    }
    ItemKind::Text
}

fn looks_like_code(text: &str) -> bool {
    let code_marks = ["fn ", "=>", "();", "{\"", "</", "def ", "import ", "SELECT ", "sudo "];
    if code_marks.iter().any(|mark| text.contains(mark)) {
        return true;
    }
    let path_like = text.contains('\\') || text.contains("~/") || text.contains("/usr/");
    let bracketed = (text.contains('{') && text.contains('}')) || (text.contains('(') && text.contains(')'));
    let spaced_command = text.split_whitespace().count() >= 2 && (text.contains(" --") || text.starts_with("git ") || text.starts_with("cargo "));
    path_like || bracketed && text.len() < 240 || spaced_command
}

fn kind_label(kind: ItemKind) -> &'static str {
    match kind {
        ItemKind::Text => "文本",
        ItemKind::Link => "链接",
        ItemKind::Code => "代码",
        ItemKind::MultiLine => "多行文本",
        ItemKind::Image => "图片",
    }
}

fn meta_line(item: &Item, kind: ItemKind, now: i64) -> String {
    if let Some(image) = item.image.as_ref() {
        return format!(
            "图片 · {}×{} · {}",
            image.width,
            image.height,
            relative_time(now, item.copied_at)
        );
    }
    let mut parts = vec![kind_label(kind).to_string()];
    let lines = item.text.lines().count();
    if lines > 1 {
        parts.push(format!("{lines} 行"));
    }
    parts.push(format!("{} 字", item.text.chars().count()));
    parts.push(relative_time(now, item.copied_at));
    parts.join(" · ")
}

fn meta_label(item: &Item, label: &str) -> String {
    if let Some(image) = item.image.as_ref() {
        return format!(
            "{} · {}×{} · {}",
            label,
            image.width,
            image.height,
            relative_time(chrono::Local::now().timestamp(), item.copied_at)
        );
    }
    let mut parts = vec![label.to_string()];
    let lines = item.text.lines().count();
    if lines > 1 {
        parts.push(format!("{lines} 行"));
    }
    parts.push(format!("{} 字", item.text.chars().count()));
    parts.push(relative_time(chrono::Local::now().timestamp(), item.copied_at));
    parts.join(" · ")
}

/// 预览第一行；如果第一行没有命中关键词，就换成命中的那一行，避免搜到了却看不到。
fn display_line(text: &str, query: &str) -> (String, Option<(usize, usize)>) {
    let needle = query.trim();
    let lines: Vec<&str> = text.lines().collect();
    let mut chosen = 0usize;
    if !needle.is_empty() {
        if let Some(index) = lines.iter().position(|line| find_range(line, needle).is_some()) {
            chosen = index;
        }
    }
    let raw = lines.get(chosen).copied().unwrap_or_default();
    let mut line: String = raw.replace('\t', "    ").trim_start().to_string();
    if line.chars().count() > 400 {
        line = line.chars().take(400).collect::<String>();
        line.push('…');
    }
    let range = find_range(&line, needle);
    (line, range)
}

fn find_range(line: &str, needle: &str) -> Option<(usize, usize)> {
    if needle.is_empty() {
        return None;
    }
    if needle.is_ascii() {
        let hay = line.to_ascii_lowercase();
        let needle = needle.to_ascii_lowercase();
        let start = hay.find(&needle)?;
        return Some((start, start + needle.len()));
    }
    let hay = line.to_lowercase();
    let needle_lower = needle.to_lowercase();
    let start = hay.find(&needle_lower)?;
    let char_start = hay[..start].chars().count();
    let char_len = needle_lower.chars().count();
    let orig_start = line.char_indices().nth(char_start).map(|(index, _)| index)?;
    let orig_end = line
        .char_indices()
        .nth(char_start + char_len)
        .map(|(index, _)| index)
        .unwrap_or(line.len());
    Some((orig_start, orig_end))
}

fn title_galley(
    ui: &Ui,
    line: &str,
    range: Option<(usize, usize)>,
    font: FontId,
    text: Color32,
    accent: Color32,
    max_width: f32,
) -> Arc<egui::Galley> {
    let base = TextFormat { font_id: font.clone(), color: text, ..Default::default() };
    let mut job = egui::text::LayoutJob::default();
    job.wrap.max_width = max_width;
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    job.wrap.overflow_character = Some('…');
    match range {
        Some((start, end))
            if start < end && line.is_char_boundary(start) && line.is_char_boundary(end) =>
        {
            if start > 0 {
                job.append(&line[..start], 0.0, base.clone());
            }
            let hit = TextFormat {
                font_id: font,
                color: accent,
                background: with_alpha(accent, 0.20),
                ..Default::default()
            };
            job.append(&line[start..end], 0.0, hit);
            if end < line.len() {
                job.append(&line[end..], 0.0, base);
            }
        }
        _ => job.append(line, 0.0, base),
    }
    ui.fonts(|fonts| fonts.layout_job(job))
}

// ---------------------------------------------------------------- 绘制部件

fn draw_clipboard_icon(ui: &Ui, rect: Rect, color: Color32) {
    let painter = ui.painter();
    let body = rect.shrink(4.0);
    painter.rect_stroke(body, 3.0, Stroke::new(1.7_f32, color), StrokeKind::Inside);
    painter.rect_filled(
        Rect::from_center_size(egui::pos2(rect.center().x, body.top() + 1.0), egui::vec2(9.0, 4.0)),
        2.0,
        color,
    );
}


fn draw_kind_icon(painter: &egui::Painter, rect: Rect, kind: ItemKind, color: Color32) {
    let center = rect.center();
    match kind {
        ItemKind::Text => {
            painter.text(center, egui::Align2::CENTER_CENTER, "T", theme::font(15.0, W::Semibold), color);
        }
        ItemKind::Link => {
            let dir = Vec2::new(3.4, 3.4);
            painter.circle_stroke(center - dir, 3.6, Stroke::new(1.4_f32, color));
            painter.circle_stroke(center + dir, 3.6, Stroke::new(1.4_f32, color));
            painter.line_segment([center - Vec2::new(1.0, 1.0), center + Vec2::new(1.0, 1.0)], Stroke::new(1.4_f32, color));
        }
        ItemKind::Code => {
            painter.line_segment([center + Vec2::new(-3.0, -5.0), center + Vec2::new(-7.0, 0.0)], Stroke::new(1.4_f32, color));
            painter.line_segment([center + Vec2::new(-7.0, 0.0), center + Vec2::new(-3.0, 5.0)], Stroke::new(1.4_f32, color));
            painter.line_segment([center + Vec2::new(3.0, -5.0), center + Vec2::new(7.0, 0.0)], Stroke::new(1.4_f32, color));
            painter.line_segment([center + Vec2::new(7.0, 0.0), center + Vec2::new(3.0, 5.0)], Stroke::new(1.4_f32, color));
            painter.line_segment([center + Vec2::new(1.5, -6.0), center + Vec2::new(-1.5, 6.0)], Stroke::new(1.4_f32, color));
        }
        ItemKind::Image => {
            let frame = Rect::from_center_size(center, Vec2::new(15.0, 12.0));
            painter.rect_stroke(frame, 2.0, Stroke::new(1.4_f32, color), StrokeKind::Inside);
            painter.circle_filled(center + Vec2::new(4.0, -3.0), 1.6, color);
            painter.line_segment(
                [center + Vec2::new(-6.0, 4.0), center + Vec2::new(-1.0, -1.0)],
                Stroke::new(1.3_f32, color),
            );
            painter.line_segment(
                [center + Vec2::new(-1.0, -1.0), center + Vec2::new(3.0, 3.0)],
                Stroke::new(1.3_f32, color),
            );
        }
        ItemKind::MultiLine => {
            for (offset, length) in [(-5.0f32, 14.0f32), (0.0, 17.0), (5.0, 10.0)] {
                painter.line_segment(
                    [center + Vec2::new(-9.0, offset), center + Vec2::new(-9.0 + length, offset)],
                    Stroke::new(1.5_f32, color),
                );
            }
        }
    }
}

fn draw_empty_icon(painter: &egui::Painter, rect: Rect, p: &Palette, kind: EmptyKind) {
    painter.circle_filled(rect.center(), rect.width() / 2.0, p.raised);
    painter.circle_stroke(rect.center(), rect.width() / 2.0, Stroke::new(1.0_f32, p.line_soft));
    let center = rect.center();
    let color = p.faint;
    match kind {
        EmptyKind::Lines => {
            for (offset, length) in [(-7.0f32, 18.0f32), (0.0, 22.0), (7.0, 13.0)] {
                painter.line_segment(
                    [center + Vec2::new(-11.0, offset), center + Vec2::new(-11.0 + length, offset)],
                    Stroke::new(1.6_f32, color),
                );
            }
        }
        EmptyKind::Search => {
            painter.circle_stroke(center - Vec2::new(2.0, 2.0), 7.5, Stroke::new(1.7_f32, color));
            painter.line_segment(
                [center + Vec2::new(3.2, 3.2), center + Vec2::new(9.0, 9.0)],
                Stroke::new(1.7_f32, color),
            );
        }
        EmptyKind::Pin => {
            painter.circle_stroke(center - Vec2::new(0.0, 2.0), 5.5, Stroke::new(1.6_f32, color));
            painter.line_segment([center + Vec2::new(0.0, 4.0), center + Vec2::new(0.0, 10.0)], Stroke::new(1.6_f32, color));
        }
        EmptyKind::Lock => {
            painter.circle_stroke(center - Vec2::new(0.0, 3.0), 5.0, Stroke::new(1.6_f32, color));
            painter.rect_stroke(
                Rect::from_center_size(center + Vec2::new(0.0, 5.0), Vec2::new(1.0, 1.0)),
                2.0,
                Stroke::new(1.6_f32, color),
                StrokeKind::Inside,
            );
            painter.rect_filled(
                Rect::from_center_size(center + Vec2::new(0.0, 6.0), Vec2::new(16.0, 11.0)),
                3.0,
                color,
            );
            painter.circle_filled(center + Vec2::new(0.0, 5.0), 1.6, p.raised);
        }
    }
}


fn target_chip(ui: &mut Ui, label: &str, p: &Palette) {
    let font = theme::font(11.5, W::Semibold);
    let galley = ui.fonts(|fonts| fonts.layout_no_wrap(label.to_owned(), font, p.text));
    let width = (galley.size().x + 18.0).min(300.0);
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 24.0), Sense::hover());
    ui.painter().rect_filled(rect, 6.0, p.raised);
    ui.painter().rect_stroke(rect, 6.0, Stroke::new(1.0_f32, p.line_soft), StrokeKind::Inside);
    ui.painter().galley(
        Pos2::new(rect.center().x - galley.size().x / 2.0, rect.center().y - galley.size().y / 2.0),
        galley,
        p.text,
    );
    response.on_hover_text("会把内容写回这个窗口，再模拟一次 Ctrl+V");
}


fn tab_button(ui: &mut Ui, p: &Palette, label: &str, count: usize, active: bool) -> bool {
    let label_font = theme::font(12.0, if active { W::Semibold } else { W::Regular });
    let count_font = theme::font(10.0, W::Regular);
    let label_width = ui.fonts(|fonts| fonts.layout_no_wrap(label.to_owned(), label_font.clone(), p.text).size().x);
    let count_width = ui.fonts(|fonts| fonts.layout_no_wrap(count.to_string(), count_font.clone(), p.faint).size().x);
    let width = label_width + 7.0 + count_width + 8.0;
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 24.0), Sense::click());
    let t = ui.ctx().animate_bool_with_time(egui::Id::new(("clipboard-tab", label)), active, 0.16);
    let idle = p.muted;
    let color = mix(idle, p.text, t);
    let count_color = mix(p.faint, p.muted, t);
    ui.painter().text(
        Pos2::new(rect.left() + 2.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        label_font,
        color,
    );
    ui.painter().text(
        Pos2::new(rect.left() + 2.0 + label_width + 7.0, rect.center().y + 1.0),
        egui::Align2::LEFT_CENTER,
        count.to_string(),
        count_font,
        count_color,
    );
    if t > 0.01 {
        let y = rect.bottom() - 2.0;
        let width = (label_width + 7.0 + count_width) * t;
        ui.painter().line_segment(
            [Pos2::new(rect.left() + 2.0, y), Pos2::new(rect.left() + 2.0 + width, y)],
            Stroke::new(2.0_f32, p.accent),
        );
    }
    let _ = response.clone().on_hover_cursor(egui::CursorIcon::PointingHand);
    response.clicked()
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
        Item { id: id.into(), text: text.into(), copied_at: at, pinned: false, image: None }
    }

    fn pinned(mut item: Item) -> Item {
        item.pinned = true;
        item
    }

    fn sample_items() -> Vec<Item> {
        let now = chrono::Local::now().timestamp();
        vec![
            pinned(item("pin-1", "https://github.com/jilespace/jilespace-daozai/releases/tag/v0.6.49", now - 3_600)),
            pinned(item("pin-2", "每周五 14:30 方案评审，会议室见", now - 7_200)),
            item("a", "cargo test --locked --all-targets --features record -- --test-threads=1", now - 20),
            item("b", "设计稿里的留白和字重再统一一次，然后把标题层级重新排一遍。\n第二行是补充说明\n第三行留给备注", now - 240),
            item("c", "tang@example.com", now - 3_600),
            item("d", "https://notchy.dev/features/", now - 86_400),
            item("e", "把剪贴板的撤销、置顶、预览都做成面板内的一等公民。", now - 172_800),
        ]
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
        popup.set_items(vec![item("a", "Alpha", 1), item("b", "Beta", 2), item("c", "alpha two", 3)]);
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

    #[test]
    fn refreshing_items_preserves_selected_item_identity() {
        let mut popup = Popup::default();
        popup.open_state(true, false, None);
        popup.set_items(vec![item("a", "Alpha", 1), item("b", "Beta", 2)]);
        popup.move_selection(1);
        popup.set_items(vec![item("new", "Newest", 3), item("a", "Alpha", 1), item("b", "Beta", 2)]);
        assert_eq!(popup.selected_id().as_deref(), Some("b"));
    }

    #[test]
    fn pinned_tab_only_lists_pinned_and_keeps_selection() {
        let mut popup = Popup::default();
        popup.open_state(true, false, None);
        popup.set_items(vec![item("a", "alpha", 1), pinned(item("b", "beta", 2)), item("c", "gamma", 3)]);
        popup.tab = Tab::Pinned;
        let filtered = popup.filtered();
        assert_eq!(filtered.len(), 1);
        assert_eq!(popup.items[filtered[0]].id, "b");
        popup.sync_selection();
        assert_eq!(popup.selected_id().as_deref(), Some("b"));
    }

    #[test]
    fn search_keeps_matching_line_and_highlight_range() {
        let (line, range) = display_line("第一行没有关键词\n第二行包含 needle 关键词\n第三行", "needle");
        assert_eq!(line, "第二行包含 needle 关键词");
        let (start, end) = range.expect("应当标出命中区间");
        assert_eq!(&line[start..end], "needle");
        let (first, none) = display_line("只有一行文本", "");
        assert_eq!(first, "只有一行文本");
        assert!(none.is_none());
        let (_, cjk) = display_line("这里有关键词", "关键词");
        assert!(cjk.is_some());
    }

    #[test]
    fn delete_stages_undo_and_restore_returns_the_same_item() {
        let mut popup = Popup::default();
        popup.open_state(true, false, None);
        let target = pinned(item("b", "beta", 2));
        popup.set_items(vec![item("a", "alpha", 1), target.clone()]);
        popup.move_selection(1);
        let staged = popup.stage_delete(&target);
        assert_eq!(staged, Action::Delete("b".into()));
        assert!(popup.undo.is_some());
        let restored = popup.take_undo();
        match restored {
            Action::Restore(item) => {
                assert_eq!(item.id, "b");
                assert!(item.pinned);
                assert_eq!(item.text, "beta");
            }
            other => panic!("撤销应恢复原条目，实际 {other:?}"),
        }
        assert!(popup.undo.is_none());
    }

    #[test]
    fn undo_expires_after_the_timeout() {
        let mut popup = Popup::default();
        popup.open_state(true, false, None);
        popup.set_items(vec![item("a", "alpha", 1)]);
        let target = popup.items[0].clone();
        popup.stage_delete(&target);
        popup.undo.as_mut().unwrap().at = Instant::now() - UNDO_TTL - Duration::from_millis(1);
        popup.expire_undo();
        assert!(popup.undo.is_none());
    }

    #[test]
    fn copy_error_is_shown_until_cleared() {
        let mut popup = Popup::default();
        popup.open_state(true, false, None);
        popup.set_error("剪贴板正被占用，请重试");
        assert_eq!(popup.error.as_deref(), Some("剪贴板正被占用，请重试"));
        popup.clear_error();
        assert!(popup.error.is_none());
    }

    #[test]
    fn kind_detection_is_stable_for_common_payloads() {
        assert_eq!(item_kind("https://example.com/a"), ItemKind::Link);
        assert_eq!(item_kind("hello world"), ItemKind::Text);
        assert_eq!(item_kind("第一行\n第二行"), ItemKind::MultiLine);
        assert_eq!(item_kind("C:\\Users\\tester\\Desktop"), ItemKind::Code);
        assert_eq!(item_kind("git rebase --onto main"), ItemKind::Code);
    }

    // ---------- 无窗口渲染：布局是否装得下、关键状态是否画出来 ----------

    struct Rendered {
        shapes: Vec<egui::epaint::ClippedShape>,
        primitives: Vec<egui::epaint::ClippedPrimitive>,
        textures: Vec<egui::TexturesDelta>,
        pixels_per_point: f32,
    }

    fn render_popup(popup: &mut Popup, dark: bool, query: &str) -> Rendered {
        popup.query = query.to_string();
        popup.sync_selection();
        popup.fade = 1.0;
        let ctx = egui::Context::default();
        crate::theme::load_fonts(&ctx);
        let mut textures = Vec::new();
        let mut last = None;
        for frame in 0..3 {
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(WIDTH, HEIGHT))),
                    time: Some(frame as f64 / 60.0),
                    viewports: std::iter::once((
                        egui::ViewportId::ROOT,
                        egui::ViewportInfo { native_pixels_per_point: Some(2.0), ..Default::default() },
                    ))
                    .collect(),
                    ..Default::default()
                },
                |ctx| {
                    let _ = popup.panel_ui(ctx, dark);
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

    fn text_shapes(shapes: &[egui::epaint::ClippedShape]) -> Vec<&egui::epaint::TextShape> {
        shapes
            .iter()
            .flat_map(|shape| {
                let mut out = Vec::new();
                collect_text(&shape.shape, &mut out);
                out
            })
            .collect()
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

    fn contains_text(shapes: &[egui::epaint::ClippedShape], needle: &str) -> bool {
        text_shapes(shapes).iter().any(|text| text.galley.text().contains(needle))
    }

    fn write_png(rendered: &Rendered, path: &std::path::Path) {
        let mut raster = crate::preview_raster::Raster::new(
            (WIDTH * rendered.pixels_per_point) as usize,
            (HEIGHT * rendered.pixels_per_point) as usize,
        );
        raster.update_textures(&rendered.textures[0]);
        for delta in rendered.textures.iter().skip(1) {
            raster.update_textures(delta);
        }
        raster.draw(&rendered.primitives, rendered.pixels_per_point);
        let mut rgba = Vec::new();
        raster.to_rgba8(&mut rgba);
        image::save_buffer(
            path,
            &rgba,
            (WIDTH * rendered.pixels_per_point) as u32,
            (HEIGHT * rendered.pixels_per_point) as u32,
            image::ColorType::Rgba8,
        )
        .unwrap();
    }

    #[test]
    fn popup_renders_states_inside_the_window() {
        let mut popup = Popup::default();
        popup.open_state(true, false, None);
        popup.set_target_label(Some("chrome".into()));
        popup.set_items(sample_items());
        let dark = render_popup(&mut popup, true, "");
        let window = Rect::from_min_size(Pos2::ZERO, Vec2::new(WIDTH, HEIGHT));
        for text in text_shapes(&dark.shapes) {
            if text.galley.text().is_empty() || !text.galley.size().x.is_finite() {
                continue;
            }
            let rect = text.visual_bounding_rect();
            assert!(
                window.expand(2.0).contains_rect(rect),
                "文字超出面板：{:?} ← {:?}",
                rect,
                text.galley.text()
            );
        }
        for expected in ["剪贴板", "最近", "置顶", "粘贴到", "chrome", "粘贴", "仅复制", "最近复制"] {
            assert!(contains_text(&dark.shapes, expected), "缺少关键元素：{expected}");
        }
        assert!(contains_text(&dark.shapes, "cargo test --locked"));
        assert!(contains_text(&dark.shapes, "刚刚") || contains_text(&dark.shapes, "分钟前"));

        let light = render_popup(&mut popup, false, "");
        for expected in ["剪贴板", "粘贴到", "chrome"] {
            assert!(contains_text(&light.shapes, expected), "浅色面板缺少：{expected}");
        }

        let mut preview = Popup::default();
        preview.open_state(true, false, None);
        preview.set_items(sample_items());
        preview.selected_id = Some("b".into());
        preview.sync_selection();
        preview.preview_open = true;
        let view = render_popup(&mut preview, true, "");
        assert!(contains_text(&view.shapes, "预览"));
        assert!(contains_text(&view.shapes, "第三行留给备注") || contains_text(&view.shapes, "第二行是补充说明"));

        if let Some(dest) = std::env::var_os("DAOZAI_CLIPBOARD_PREVIEW_DIR").filter(|value| !value.is_empty()) {
            let dest = std::path::PathBuf::from(dest);
            std::fs::create_dir_all(&dest).unwrap();
            write_png(&dark, &dest.join("clipboard-dark.png"));
            write_png(&light, &dest.join("clipboard-light.png"));
            write_png(&view, &dest.join("clipboard-preview.png"));
        }
    }

    #[test]
    fn image_items_render_metadata_and_preview_fallback() {
        let now = chrono::Local::now().timestamp();
        let image_item = Item {
            id: "img-demo".into(),
            text: "图片 300×200".into(),
            copied_at: now,
            pinned: false,
            image: Some(crate::clipboard::ImageRef {
                key: "missing-image".into(),
                width: 300,
                height: 200,
            }),
        };
        let mut popup = Popup::default();
        popup.open_state(true, false, None);
        popup.set_items(vec![image_item.clone()]);
        let list = render_popup(&mut popup, true, "");
        assert!(contains_text(&list.shapes, "图片 · 300×200"), "列表应显示图片尺寸");
        popup.selected_id = Some(image_item.id.clone());
        popup.sync_selection();
        popup.preview_open = true;
        let preview = render_popup(&mut popup, true, "");
        assert!(contains_text(&preview.shapes, "预览"));
        assert!(contains_text(&preview.shapes, "图片读取失败"), "文件缺失时应给出预览兜底");
        assert!(contains_text(&preview.shapes, "识别文字"), "图片预览应提供 OCR 入口");
    }

    /// 开发用：生成真实图片条目的预览截图。
    #[test]
    #[ignore]
    fn image_preview_preview_render() {
        let dir = std::env::temp_dir().join(format!(
            "daozai-clipboard-image-preview-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("DAOZAI_DATA_DIR", &dir);
        let image = crate::ocr::Image {
            width: 320,
            height: 200,
            rgba: (0..320 * 200 * 4)
                .map(|index| match index % 4 {
                    0 => ((index / 4) % 255) as u8,
                    1 => (((index / 4) / 2) % 255) as u8,
                    2 => 220,
                    _ => 255,
                })
                .collect(),
        };
        let image_ref = crate::clipboard::store_image(&image).unwrap();
        let item = Item {
            id: image_ref.key.clone(),
            text: format!("图片 {}×{}", image_ref.width, image_ref.height),
            copied_at: chrono::Local::now().timestamp(),
            pinned: false,
            image: Some(image_ref),
        };
        let mut popup = Popup::default();
        popup.open_state(true, false, None);
        popup.set_items(vec![item.clone()]);
        popup.selected_id = Some(item.id.clone());
        popup.preview_open = true;
        let rendered = render_popup(&mut popup, true, "");
        assert!(contains_text(&rendered.shapes, "预览"));
        if let Some(dest) = std::env::var_os("DAOZAI_CLIPBOARD_IMAGE_PREVIEW").filter(|value| !value.is_empty()) {
            let dest = std::path::PathBuf::from(dest);
            std::fs::create_dir_all(&dest).unwrap();
            write_png(&rendered, &dest.join("clipboard-image-preview.png"));
            let mut list = Popup::default();
            list.open_state(true, false, None);
            list.set_items(vec![item]);
            std::thread::sleep(std::time::Duration::from_millis(250));
            let list = render_popup(&mut list, true, "");
            write_png(&list, &dest.join("clipboard-image-list.png"));
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn popup_renders_disabled_and_empty_states() {
        let mut popup = Popup::default();
        popup.open_state(true, false, None);
        popup.set_items(Vec::new());
        let rendered = render_popup(&mut popup, true, "");
        assert!(contains_text(&rendered.shapes, "还没有记录"));
        let rendered = render_popup(&mut popup, true, "不存在的关键词");
        assert!(contains_text(&rendered.shapes, "没有匹配的内容"));
        assert!(contains_text(&rendered.shapes, "清除搜索"));
        let mut disabled = Popup::default();
        disabled.open_state(false, false, None);
        let rendered = render_popup(&mut disabled, true, "");
        assert!(contains_text(&rendered.shapes, "剪贴板历史未开启"));
        assert!(contains_text(&rendered.shapes, "开启剪贴板历史"));
        if let Some(dest) = std::env::var_os("DAOZAI_CLIPBOARD_PREVIEW_DIR").filter(|value| !value.is_empty()) {
            let dest = std::path::PathBuf::from(dest);
            std::fs::create_dir_all(&dest).unwrap();
            write_png(&rendered, &dest.join("clipboard-disabled.png"));
        }
    }
}
