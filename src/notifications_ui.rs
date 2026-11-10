//! 通知历史面板：Windows 通知镜像与最近消息回看。
//!
//! 视觉与剪贴板/命令面板共用 `panel` 的配色、圆角与阴影；面板本体拆成
//! `panel_ui` 便于无窗口渲染与交互测试。

use crate::notifications::{time_label, Notification};
use crate::panel;
use crate::theme;
use eframe::egui::{self, Color32, RichText, Stroke, Vec2};

pub const WIDTH: f32 = 392.0;
pub const HEIGHT: f32 = 468.0;
const PAD: f32 = 10.0;
const INNER: f32 = 14.0;
const FADE_SECS: f64 = 0.13;

#[derive(Clone, Debug, Default)]
pub enum Action {
    #[default]
    None,
    Close,
    Copy(String),
    Delete(String),
    /// 不再提醒该应用。
    MuteApp(String, String),
    /// 打开通知来源应用（AUMID 或可执行文件路径）。
    OpenApp(String),
    Clear,
}

/// 按像素宽度截断文本并补 `…`（用于通知应用名等单行标签）。
fn truncate_to_width(ui: &egui::Ui, text: &str, font: egui::FontId, max_width: f32) -> String {
    let width = |value: &str| {
        ui.fonts(|fonts| {
            fonts
                .layout_no_wrap(value.to_owned(), font.clone(), Color32::WHITE)
                .size()
                .x
        })
    };
    if width(text) <= max_width {
        return text.to_owned();
    }
    let mut chars: Vec<char> = text.chars().collect();
    while !chars.is_empty() {
        chars.pop();
        let candidate: String = chars.iter().collect::<String>() + "…";
        if width(&candidate) <= max_width {
            return candidate;
        }
    }
    "…".into()
}

pub struct Popup {
    pub open: bool,
    items: Vec<Notification>,
    unread: usize,
    status: String,
    query: String,
    unread_only: bool,
    focus_search: bool,
    position: Option<egui::Pos2>,
    fade: f32,
    shown: bool,
    shown_at: Option<f64>,
    had_focus: bool,
    clear_confirm: bool,
}

impl Default for Popup {
    fn default() -> Self {
        Self {
            open: false,
            items: Vec::new(),
            unread: 0,
            status: String::new(),
            query: String::new(),
            unread_only: false,
            focus_search: false,
            position: None,
            fade: 0.0,
            shown: false,
            shown_at: None,
            had_focus: false,
            clear_confirm: false,
        }
    }
}

impl Popup {
    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn open(&mut self, items: Vec<Notification>, unread: usize, status: String) {
        self.items = items;
        self.unread = unread;
        self.status = status;
        self.open = true;
        self.fade = 0.0;
        self.shown = false;
        self.shown_at = None;
        self.had_focus = false;
        self.clear_confirm = false;
        self.query.clear();
        self.unread_only = false;
        self.focus_search = true;
    }

    pub fn close(&mut self) {
        self.open = false;
        self.shown = false;
        self.shown_at = None;
        self.had_focus = false;
        self.clear_confirm = false;
    }

    /// 面板打开期间有新通知到达时刷新列表。
    pub fn sync(&mut self, items: Vec<Notification>, unread: usize) {
        self.items = items;
        self.unread = unread;
    }

    pub fn show(&mut self, ctx: &egui::Context, dark: bool) -> Action {
        if !self.open {
            return Action::None;
        }
        let viewport_id = egui::ViewportId::from_hash_of("daozai-notifications");
        let ppp = ctx.pixels_per_point();
        let (size, min_size) = crate::panel::fitted_window_ctx(ctx, egui::vec2(WIDTH, HEIGHT), egui::vec2(340.0, 300.0));
        #[cfg(windows)]
        let initial_position = crate::native::clipboard_popup_position(size.x, size.y, ppp)
            .map(|(x, y)| egui::pos2(x, y))
            .unwrap_or_else(|| egui::pos2(140.0, 80.0));
        #[cfg(not(windows))]
        let initial_position = {
            let _ = ppp;
            egui::pos2(140.0, 80.0)
        };
        let mut action = Action::None;
        let position = *self.position.get_or_insert(initial_position);
        let builder = egui::ViewportBuilder::default()
            .with_title("通知 · 岛仔")
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
            if focused {
                self.had_focus = true;
            } else if self.had_focus {
                self.had_focus = false;
                action = Action::Close;
            }
            let panel_action = self.panel_ui(ctx, dark);
            if matches!(action, Action::None) {
                action = panel_action;
            }
        });
        if matches!(action, Action::Close | Action::Clear) {
            self.clear_confirm = false;
        }
        action
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

    /// 面板本体：拆出来供测试与截图直接渲染。
    pub fn panel_ui(&mut self, ctx: &egui::Context, dark: bool) -> Action {
        let palette = panel::apply_style(ctx, dark);
        let mut action = Action::None;
        if !ctx.memory(|memory| memory.any_popup_open())
            && ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            if self.clear_confirm {
                self.clear_confirm = false;
            } else if !self.query.is_empty() {
                self.query.clear();
                self.focus_search = true;
            } else {
                action = Action::Close;
            }
        }
        let fade = self.fade.clamp(0.0, 1.0);
        let now = chrono::Local::now();
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
                        ui.set_min_size(Vec2::new(
            (WIDTH - (PAD + INNER) * 2.0).min(ui.available_width().max(1.0)),
            (HEIGHT - (PAD + INNER) * 2.0).min(ui.available_height().max(1.0)),
        ));
                        ui.spacing_mut().item_spacing = Vec2::new(8.0, 6.0);
                        self.header(ui, &palette, &mut action);
                        ui.add_space(2.0);
                        if !self.status.trim().is_empty() {
                            ui.add(egui::Label::new(RichText::new(&self.status).size(10.5).color(palette.faint)).truncate())
                                .on_hover_text(&self.status);
                        }
                        self.search_row(ui, &palette);
                        ui.add_space(4.0);
                        ui.separator();
                        ui.add_space(2.0);
                        self.list(ui, &palette, &now, &mut action);
                        let gap = (ui.available_height() - 30.0).max(0.0);
                        ui.add_space(gap);
                        ui.separator();
                        ui.horizontal(|ui| {
                            panel::hint(ui, "Esc", if self.clear_confirm { "取消" } else if self.query.is_empty() { "关闭" } else { "清除搜索" }, &palette);
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if !self.items.is_empty() {
                                    if ui
                                        .add(
                                            egui::Button::new(
                                                RichText::new(if self.clear_confirm { "确认清空？" } else { "清空全部" })
                                                    .size(10.5)
                                                    .color(if self.clear_confirm { palette.danger } else { palette.muted }),
                                            )
                                            .frame(false),
                                        )
                                        .clicked()
                                    {
                                        if self.clear_confirm {
                                            self.items.clear();
                                            self.unread = 0;
                                            self.clear_confirm = false;
                                            action = Action::Clear;
                                        } else {
                                            self.clear_confirm = true;
                                        }
                                    }
                                }
                            });
                        });
                    });
            });
        action
    }

    fn header(&mut self, ui: &mut egui::Ui, palette: &panel::Palette, action: &mut Action) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("通知").font(theme::font(16.0, theme::W::Semibold)).color(palette.text));
            if self.unread > 0 {
                egui::Frame::new()
                    .fill(panel::with_alpha(palette.accent, 0.18))
                    .corner_radius(9.0)
                    .inner_margin(egui::Margin::symmetric(7, 2))
                    .show(ui, |ui| {
                        ui.label(
                            RichText::new(format!("{} 条未读", self.unread))
                                .size(10.0)
                                .color(palette.accent),
                        );
                    });
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if panel::icon_button(ui, "×", "关闭窗口", palette.muted, palette.text, palette.hover, 16.0) {
                    *action = Action::Close;
                }
            });
        });
    }

    fn search_row(&mut self, ui: &mut egui::Ui, palette: &panel::Palette) {
        panel::search_field(ui, "notifications-search", &mut self.query,
            "搜索应用或通知内容…", palette, &mut self.focus_search, false);
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            if panel::filter_tab(ui, "全部", self.items.len(), !self.unread_only, palette) {
                self.unread_only = false;
            }
            if panel::filter_tab(ui, "未读", self.unread, self.unread_only, palette) {
                self.unread_only = true;
            }
        });
    }

    fn list(&mut self, ui: &mut egui::Ui, palette: &panel::Palette, now: &chrono::DateTime<chrono::Local>, action: &mut Action) {
        let height = (ui.available_height() - 42.0).max(40.0);
        if self.items.is_empty() {
            ui.add_space(((height - 65.0) * 0.5).max(0.0));
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("还没有收到通知").size(14.0).strong().color(palette.muted));
                ui.add_space(4.0);
                ui.label(
                    RichText::new("在「设置 → 通知」连接 Windows 通知后，这里会收拢最近的消息。")
                        .size(11.0)
                        .color(palette.faint),
                );
            });
            return;
        }
        let needle = self.query.trim().to_lowercase();
        let unread = self.unread;
        let filtered: Vec<(usize, &Notification)> = self
            .items
            .iter()
            .enumerate()
            .filter(|(index, item)| {
                (!self.unread_only || *index < unread)
                    && (needle.is_empty()
                        || item.app.to_lowercase().contains(&needle)
                        || item.body.to_lowercase().contains(&needle))
            })
            .collect();
        if filtered.is_empty() {
            ui.add_space(((height - 55.0) * 0.5).max(0.0));
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("没有匹配的通知").size(13.0).strong().color(palette.muted));
                ui.add_space(3.0);
                ui.label(RichText::new("试试清空搜索，或切换到「全部」").size(10.5).color(palette.faint));
            });
            return;
        }
        let scroll = egui::ScrollArea::vertical()
            .id_salt("notification-history")
            .auto_shrink([false, false])
            .max_height(height)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                for (index, item) in filtered.iter().copied() {
                    let unread = index < self.unread;
                    let id = item.id.clone();
                    let body = item.body.clone();
                    egui::Frame::new()
                        .fill(if unread { palette.raised } else { Color32::TRANSPARENT })
                        .stroke(Stroke::new(
                            1.0_f32,
                            if unread { palette.line_soft } else { Color32::TRANSPARENT },
                        ))
                        .corner_radius(10.0)
                        .inner_margin(egui::Margin::symmetric(10, 8))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                if unread {
                                    let (dot, _) = ui.allocate_exact_size(Vec2::new(7.0, 7.0), egui::Sense::hover());
                                    ui.painter().circle_filled(dot.center(), 2.5, palette.accent);
                                }
                                // 应用名过长时按可用宽度截断，避免把右侧操作挤出行外。
                                let app_width = (ui.available_width() - 90.0).max(48.0);
                                let app_font = theme::font(12.0, theme::W::Semibold);
                                let app = truncate_to_width(ui, &item.app, app_font.clone(), app_width);
                                ui.label(
                                    RichText::new(app)
                                        .font(app_font)
                                        .color(if unread { palette.text } else { palette.muted }),
                                );
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    ui.menu_button(RichText::new("•••").size(10.0).color(palette.muted), |ui| {
                                        if ui.button("静音此应用").clicked() {
                                            *action = Action::MuteApp(item.source_id.clone(), item.app.clone());
                                            ui.close_menu();
                                        }
                                        if ui.button(RichText::new("删除这条").color(palette.danger)).clicked() {
                                            *action = Action::Delete(id.clone());
                                            ui.close_menu();
                                        }
                                    }).response.on_hover_text("更多操作");
                                    ui.label(RichText::new(time_label(item.at, *now)).size(9.5).color(palette.faint));
                                });
                            });
                            ui.add_space(2.0);
                            ui.add(
                                egui::Label::new(RichText::new(&item.body).size(11.5).color(palette.muted))
                                    .wrap(),
                            );
                            ui.horizontal(|ui| {
                                if panel::link_button(ui, "复制", palette) {
                                    *action = Action::Copy(body.clone());
                                }
                                if !item.source_id.trim().is_empty() && panel::link_button(ui, "打开应用", palette) {
                                    *action = Action::OpenApp(item.source_id.clone());
                                }
                            });
                        });
                    ui.add_space(5.0);
                }
            });
        if scroll.content_size.y > scroll.inner_rect.height() + 1.0 {
            let area = scroll.inner_rect;
            for step in 0..6 {
                let k = step as f32 / 5.0;
                let y = area.bottom() - 18.0 + step as f32 * 3.0;
                let strip = egui::Rect::from_min_size(egui::pos2(area.left(), y), egui::vec2(area.width(), 3.0));
                ui.painter().rect_filled(strip, 0.0, Color32::from_rgba_unmultiplied(palette.surface.r(), palette.surface.g(), palette.surface.b(), (16.0 + 26.0 * k) as u8));
            }
        }
    }
}

/// 供测试/截图：把面板渲染到离屏表面。
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn polish_more_menu_keeps_mute_and_delete_reachable() {
        let ctx = egui::Context::default();
        crate::theme::load_fonts(&ctx);
        let mut popup = Popup::default();
        popup.open(sample(), 1, String::new());
        popup.fade = 1.0;
        let mut frame = |events| {
            let mut action = Action::None;
            let output = ctx.run(egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(340.0, 300.0))),
                events,
                ..Default::default()
            }, |ctx| action = popup.panel_ui(ctx, false));
            (action, output)
        };
        let locate = |output: &egui::FullOutput, label: &str| {
            let mut texts = Vec::new();
            text_shapes(&output.shapes, &mut texts);
            texts.iter().find(|text| text.galley.text() == label)
                .map(|text| text.pos + text.galley.size() * 0.5)
                .unwrap_or_else(|| panic!("缺少控件 {label}"))
        };
        let pointer = |pos, pressed| egui::Event::PointerButton {
            pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE,
        };
        frame(vec![]);
        let (_, output) = frame(vec![]);
        let more = locate(&output, "•••");
        for label in ["静音此应用", "删除这条"] {
            frame(vec![egui::Event::PointerMoved(more), pointer(more, true)]);
            frame(vec![pointer(more, false)]);
            let (_, output) = frame(vec![]);
            let item = locate(&output, label);
            frame(vec![egui::Event::PointerMoved(item), pointer(item, true)]);
            let (action, _) = frame(vec![pointer(item, false)]);
            if label == "静音此应用" {
                assert!(matches!(action, Action::MuteApp(ref id, ref app) if id == "wechat.exe" && app == "微信"));
            } else {
                assert!(matches!(action, Action::Delete(ref id) if id == "a"));
            }
        }
    }

    #[test]
    fn polish_escape_cancels_clear_then_search_then_closes() {
        let ctx = egui::Context::default();
        crate::theme::load_fonts(&ctx);
        let mut popup = Popup::default();
        popup.open(sample(), 1, String::new());
        popup.query = "需求".into();
        popup.clear_confirm = true;
        for step in 0..3 {
            let mut action = Action::None;
            let _ = ctx.run(egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(WIDTH, HEIGHT))),
                events: vec![egui::Event::Key { key: egui::Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE }],
                ..Default::default()
            }, |ctx| action = popup.panel_ui(ctx, false));
            assert!(!popup.clear_confirm, "Esc 应先取消清空确认");
            if step == 0 { assert_eq!(popup.query, "需求"); }
            if step == 1 { assert!(popup.query.is_empty()); }
            assert_eq!(matches!(action, Action::Close), step == 2);
        }
    }

    #[test]
    fn reduced_motion_skips_notification_fade() {
        let mut popup = Popup::default();
        popup.open(sample(), 0, String::new());
        popup.shown_at = Some(0.0);
        popup.fade = 0.0;
        assert!(!popup.update_fade(0.01, false));
        assert_eq!(popup.fade, 1.0);
    }
    use eframe::egui::{Pos2, Rect};

    struct Rendered {
        shapes: Vec<egui::epaint::ClippedShape>,
        primitives: Vec<egui::epaint::ClippedPrimitive>,
        textures: Vec<egui::TexturesDelta>,
        pixels_per_point: f32,
    }

    fn render(popup: &mut Popup, dark: bool) -> Rendered {
        render_sized(popup, dark, Vec2::new(WIDTH, HEIGHT))
    }

    fn render_sized(popup: &mut Popup, dark: bool, size: Vec2) -> Rendered {
        popup.fade = 1.0;
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
                    let _ = popup.panel_ui(ctx, dark);
                },
            );
            textures.push(output.textures_delta.clone());
            last = Some(output);
        }
        let output = last.expect("至少渲染一帧");
        let primitives = ctx.tessellate(output.shapes.clone(), output.pixels_per_point);
        Rendered { shapes: output.shapes, primitives, textures, pixels_per_point: output.pixels_per_point }
    }

    fn text_shapes<'a>(shapes: &'a [egui::epaint::ClippedShape], out: &mut Vec<&'a egui::epaint::TextShape>) {
        for clipped in shapes {
            fn walk<'a>(shape: &'a egui::Shape, out: &mut Vec<&'a egui::epaint::TextShape>) {
                match shape {
                    egui::Shape::Text(text) => out.push(text),
                    egui::Shape::Vec(shapes) => {
                        for shape in shapes {
                            walk(shape, out);
                        }
                    }
                    _ => {}
                }
            }
            walk(&clipped.shape, out);
        }
    }

    fn contains(shapes: &[egui::epaint::ClippedShape], needle: &str) -> bool {
        let mut texts = Vec::new();
        text_shapes(shapes, &mut texts);
        texts.iter().any(|text| text.galley.text().contains(needle))
    }

    fn sample() -> Vec<Notification> {
        let now = chrono::Local::now().timestamp();
        vec![
            Notification { app: "微信".into(), body: "项目群里有人 @ 你，记得看一下新的需求文档。".into(), at: now, source_id: "wechat.exe".into(), id: "a".into() },
            Notification { app: "Outlook".into(), body: "周会提醒：今天 15:00 开始".into(), at: now - 3600, source_id: "outlook.exe".into(), id: "b".into() },
        ]
    }

    #[test]
    fn renders_items_and_empty_state() {
        let mut popup = Popup::default();
        popup.open(sample(), 1, "正在监听 · 当前可见 2 条通知".into());
        let rendered = render(&mut popup, true);
        for expected in ["通知", "1 条未读", "微信", "项目群里有人", "Outlook", "正在监听", "打开"] {
            assert!(contains(&rendered.shapes, expected), "面板缺少 {expected}");
        }
        let mut popup = Popup::default();
        popup.open(Vec::new(), 0, String::new());
        let rendered = render(&mut popup, true);
        assert!(contains(&rendered.shapes, "还没有收到通知"));
    }

    #[test]
    fn search_and_unread_filter_narrow_the_list() {
        let mut popup = Popup::default();
        popup.open(sample(), 1, String::new());
        popup.query = "需求".into();
        let rendered = render(&mut popup, true);
        assert!(contains(&rendered.shapes, "微信"), "搜索命中应保留微信");
        assert!(!contains(&rendered.shapes, "Outlook"), "不匹配的条目应被过滤");
        popup.query = "不存在的关键字".into();
        let rendered = render(&mut popup, true);
        assert!(contains(&rendered.shapes, "没有匹配的通知"));
        popup.query.clear();
        popup.unread_only = true;
        let rendered = render(&mut popup, true);
        assert!(contains(&rendered.shapes, "微信"));
        assert!(!contains(&rendered.shapes, "Outlook"), "只看未读应隐藏已读条目");
    }

    #[test]
    fn renders_at_reduced_window_size() {
        let mut popup = Popup::default();
        popup.open(sample(), 1, String::new());
        let size = Vec2::new(340.0, 300.0);
        let rendered = render_sized(&mut popup, true, size);
        for expected in ["通知", "微信", "复制", "•••"] {
            assert!(contains(&rendered.shapes, expected), "小窗口缺少 {expected}");
        }
        if let Some(dest) = std::env::var_os("DAOZAI_NOTIFICATION_SMALL_PREVIEW").filter(|value| !value.is_empty()) {
            let dest = std::path::PathBuf::from(dest);
            std::fs::create_dir_all(&dest).unwrap();
            let ppp=rendered.pixels_per_point;
            let mut raster = crate::preview_raster::Raster::new((size.x*ppp) as usize, (size.y*ppp) as usize);
            for delta in &rendered.textures { raster.update_textures(delta); }
            raster.draw(&rendered.primitives, ppp);
            let mut rgba = Vec::new();
            raster.to_rgba8(&mut rgba);
            image::save_buffer(dest.join("notifications-small.png"), &rgba, (size.x*ppp) as u32, (size.y*ppp) as u32, image::ColorType::Rgba8).unwrap();
        }
    }

    #[test]
    fn long_app_names_are_truncated_in_narrow_panels() {
        let mut items = sample();
        items.insert(0, Notification {
            app: "一个非常非常长的通知应用名称用于测试截断效果".into(),
            body: "正文仍然完整显示".into(),
            at: chrono::Local::now().timestamp(),
            source_id: "long.app".into(),
            id: "long".into(),
        });
        let mut popup = Popup::default();
        popup.open(items, 3, String::new());
        let rendered = render_sized(&mut popup, true, Vec2::new(340.0, 300.0));
        let full = "一个非常非常长的通知应用名称用于测试截断效果";
        assert!(!contains(&rendered.shapes, full), "过长应用名应被截断");
        assert!(contains(&rendered.shapes, "一个"), "截断后仍保留应用名开头");
        let window = Rect::from_min_size(Pos2::ZERO, Vec2::new(340.0, 300.0));
        for clipped in &rendered.shapes {
            let mut texts = Vec::new();
            text_shapes(std::slice::from_ref(clipped), &mut texts);
            for text in texts {
                let bounds = text.visual_bounding_rect();
                if text.galley.text().is_empty() || !bounds.is_finite() || !bounds.intersects(clipped.clip_rect) {
                    continue;
                }
                // ScrollArea intentionally lays out rows below the viewport; only painted
                // pixels must remain inside the window. Horizontal text must still fit.
                assert!(bounds.left() >= -8.0 && bounds.right() <= window.right() + 8.0, "文字横向溢出：{:?}", text.galley.text());
                assert!(window.expand(8.0).contains_rect(bounds.intersect(clipped.clip_rect)), "文字超出窄面板：{:?}", text.galley.text());
            }
        }
    }

    #[test]
    fn renders_light_theme_and_writes_preview() {
        let mut popup = Popup::default();
        popup.open(sample(), 2, "正在监听 · 当前可见 2 条通知".into());
        let rendered = render(&mut popup, false);
        assert!(contains(&rendered.shapes, "微信"));
        if let Ok(dir) = std::env::var("DAOZAI_NOTIFICATION_PREVIEW_DIR") {
            let dir = std::path::PathBuf::from(dir);
            std::fs::create_dir_all(&dir).unwrap();
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
                dir.join("notifications-light.png"),
                &rgba,
                (WIDTH * rendered.pixels_per_point) as u32,
                (HEIGHT * rendered.pixels_per_point) as u32,
                image::ColorType::Rgba8,
            )
            .unwrap();
        }
    }
}
