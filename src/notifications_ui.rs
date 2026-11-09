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
const FADE_SECS: f64 = 0.12;

#[derive(Clone, Debug, Default)]
pub enum Action {
    #[default]
    None,
    Close,
    Copy(String),
    Delete(String),
    Clear,
}

pub struct Popup {
    pub open: bool,
    items: Vec<Notification>,
    unread: usize,
    status: String,
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
        #[cfg(windows)]
        let initial_position = crate::native::clipboard_popup_position(WIDTH, HEIGHT, ppp)
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
            .with_inner_size([WIDTH, HEIGHT])
            .with_min_inner_size([WIDTH, HEIGHT])
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
                action = Action::Close;
            }
            if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
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

    /// 面板本体：拆出来供测试与截图直接渲染。
    pub fn panel_ui(&mut self, ctx: &egui::Context, dark: bool) -> Action {
        let palette = panel::apply_style(ctx, dark);
        let mut action = Action::None;
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
                        ui.set_min_size(Vec2::new(WIDTH - (PAD + INNER) * 2.0, HEIGHT - (PAD + INNER) * 2.0));
                        ui.spacing_mut().item_spacing = Vec2::new(8.0, 6.0);
                        self.header(ui, &palette, &mut action);
                        ui.add_space(2.0);
                        ui.label(
                            RichText::new(if self.status.trim().is_empty() {
                                "Windows 通知会镜像到岛上，并保存在这里。"
                            } else {
                                self.status.as_str()
                            })
                            .size(10.5)
                            .color(palette.faint),
                        );
                        ui.add_space(4.0);
                        ui.separator();
                        ui.add_space(2.0);
                        self.list(ui, &palette, &now, &mut action);
                        let gap = (ui.available_height() - 30.0).max(0.0);
                        ui.add_space(gap);
                        ui.separator();
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("Esc 关闭 · 复制按钮保存正文").size(10.0).color(palette.faint));
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
            ui.label(RichText::new("通知").size(19.0).strong().color(palette.text));
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
                if ui
                    .add(egui::Button::new(RichText::new("关闭").size(11.0).color(palette.muted)).frame(false))
                    .clicked()
                {
                    *action = Action::Close;
                }
            });
        });
    }

    fn list(&mut self, ui: &mut egui::Ui, palette: &panel::Palette, now: &chrono::DateTime<chrono::Local>, action: &mut Action) {
        if self.items.is_empty() {
            ui.add_space(70.0);
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
        let height = (ui.available_height() - 58.0).max(60.0);
        egui::ScrollArea::vertical()
            .id_salt("notification-history")
            .auto_shrink([false, false])
            .max_height(height)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                for (index, item) in self.items.iter().enumerate() {
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
                                ui.label(
                                    RichText::new(&item.app)
                                        .size(12.0)
                                        .strong()
                                        .color(if unread { palette.text } else { palette.muted }),
                                );
                                ui.label(RichText::new(time_label(item.at, *now)).size(9.5).color(palette.faint));
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    if ui
                                        .add(
                                            egui::Button::new(RichText::new("×").size(12.0).color(palette.faint))
                                                .frame(false),
                                        )
                                        .on_hover_text("删除这条")
                                        .clicked()
                                    {
                                        *action = Action::Delete(id.clone());
                                    }
                                    if ui
                                        .add(
                                            egui::Button::new(RichText::new("复制").size(10.0).color(palette.accent))
                                                .frame(false),
                                        )
                                        .on_hover_text("复制正文")
                                        .clicked()
                                    {
                                        *action = Action::Copy(body.clone());
                                    }
                                });
                            });
                            ui.add_space(2.0);
                            ui.add(
                                egui::Label::new(RichText::new(&item.body).size(11.5).color(palette.muted))
                                    .wrap(),
                            );
                        });
                    ui.add_space(5.0);
                }
            });
    }
}

/// 供测试/截图：把面板渲染到离屏表面。
#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::{Pos2, Rect};

    struct Rendered {
        shapes: Vec<egui::epaint::ClippedShape>,
        primitives: Vec<egui::epaint::ClippedPrimitive>,
        textures: Vec<egui::TexturesDelta>,
        pixels_per_point: f32,
    }

    fn render(popup: &mut Popup, dark: bool) -> Rendered {
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
            Notification { app: "微信".into(), body: "项目群里有人 @ 你，记得看一下新的需求文档。".into(), at: now, id: "a".into() },
            Notification { app: "Outlook".into(), body: "周会提醒：今天 15:00 开始".into(), at: now - 3600, id: "b".into() },
        ]
    }

    #[test]
    fn renders_items_and_empty_state() {
        let mut popup = Popup::default();
        popup.open(sample(), 1, "正在监听 · 当前可见 2 条通知".into());
        let rendered = render(&mut popup, true);
        for expected in ["通知", "1 条未读", "微信", "项目群里有人", "Outlook", "正在监听"] {
            assert!(contains(&rendered.shapes, expected), "面板缺少 {expected}");
        }
        let mut popup = Popup::default();
        popup.open(Vec::new(), 0, String::new());
        let rendered = render(&mut popup, true);
        assert!(contains(&rendered.shapes, "还没有收到通知"));
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
