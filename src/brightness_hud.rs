//! 亮度 HUD：调整屏幕亮度时在屏幕底部居中的轻量 OSD。

use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, Rect, Stroke, Vec2};

use crate::brightness::State;

pub const WIDTH: f32 = 300.0;
pub const HEIGHT: f32 = 78.0;
const VISIBLE_FOR: Duration = Duration::from_millis(1400);

pub struct Hud {
    open: bool,
    state: Option<State>,
    shown_at: Instant,
    viewport: egui::ViewportId,
    shown: bool,
}

impl Default for Hud {
    fn default() -> Self {
        Self {
            open: false,
            state: None,
            shown_at: Instant::now(),
            viewport: egui::ViewportId::from_hash_of("daozai-brightness-hud"),
            shown: false,
        }
    }
}

impl Hud {
    pub fn show(&mut self, state: Option<State>) {
        self.state = state;
        self.open = true;
        self.shown_at = Instant::now();
        self.shown = false;
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn poll(&mut self, ctx: &egui::Context) {
        if !self.open {
            return;
        }
        if self.shown_at.elapsed() >= VISIBLE_FOR {
            self.open = false;
            self.shown = false;
            return;
        }
        let monitor = ctx
            .input(|input| input.viewport().monitor_size)
            .unwrap_or(Vec2::new(1280.0, 800.0));
        let position = egui::pos2((monitor.x - WIDTH) / 2.0, monitor.y - HEIGHT - 96.0);
        let builder = egui::ViewportBuilder::default()
            .with_title("亮度 · 岛仔")
            .with_icon(crate::theme::app_icon())
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
        ctx.show_viewport_immediate(self.viewport, builder, |ctx, _| {
            if !self.shown {
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                self.shown = true;
            }
            egui::CentralPanel::default()
                .frame(egui::Frame::new().fill(Color32::TRANSPARENT).inner_margin(6.0))
                .show(ctx, |ui| self.panel_ui(ui));
        });
    }

    /// 可离屏渲染的 HUD 内容。
    pub fn panel_ui(&self, ui: &mut egui::Ui) {
        egui::Frame::new()
            .fill(Color32::from_rgba_unmultiplied(32, 34, 40, 242))
            .stroke(Stroke::new(1.0_f32, Color32::from_white_alpha(22)))
            .corner_radius(16.0)
            .inner_margin(egui::Margin::symmetric(18, 12))
            .show(ui, |ui| {
                ui.set_min_width(WIDTH - 48.0);
                ui.horizontal(|ui| {
                    draw_sun(ui);
                    ui.add_space(8.0);
                    ui.vertical(|ui| {
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new("亮度")
                                    .size(12.5)
                                    .strong()
                                    .color(Color32::WHITE),
                            );
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                let text = match self.state {
                                    Some(state) => {
                                        format!("{}%", (state.level * 100.0).round() as i32)
                                    }
                                    None => "不支持".to_string(),
                                };
                                ui.label(
                                    egui::RichText::new(text)
                                        .size(12.0)
                                        .color(if self.state.is_some() {
                                            Color32::from_gray(200)
                                        } else {
                                            Color32::from_rgb(235, 176, 70)
                                        }),
                                );
                            });
                        });
                        ui.add_space(4.0);
                        let level = self.state.map(|state| state.level).unwrap_or(0.0);
                        let (rect, _) =
                            ui.allocate_exact_size(Vec2::new(WIDTH - 92.0, 6.0), egui::Sense::hover());
                        ui.painter().rect_filled(rect, 3.0, Color32::from_gray(54));
                        if level > 0.005 {
                            ui.painter().rect_filled(
                                Rect::from_min_size(rect.min, Vec2::new(rect.width() * level, 6.0)),
                                3.0,
                                Color32::from_rgb(240, 190, 70),
                            );
                        }
                        ui.add_space(3.0);
                        ui.label(
                            egui::RichText::new(if self.state.is_some() {
                                "Ctrl+Alt+Shift+↑/↓ 调节"
                            } else {
                                "当前显示器不支持亮度调节"
                            })
                            .size(9.5)
                            .color(Color32::from_gray(150)),
                        );
                    });
                });
            });
    }
}

fn draw_sun(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(26.0), egui::Sense::hover());
    let painter = ui.painter();
    let center = rect.center();
    let color = Color32::from_rgb(245, 196, 84);
    painter.circle_filled(center, 5.5, color);
    for index in 0..8 {
        let angle = std::f32::consts::TAU * index as f32 / 8.0;
        let direction = Vec2::new(angle.cos(), angle.sin());
        painter.line_segment(
            [center + direction * 8.0, center + direction * 12.0],
            Stroke::new(1.5_f32, color),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hud_renders_level_and_unsupported_state() {
        for (state, expected) in [
            (Some(State { level: 0.6 }), "60%"),
            (None, "不支持"),
        ] {
            let ctx = egui::Context::default();
            crate::theme::load_fonts(&ctx);
            let hud = Hud { state, ..Default::default() };
            let raw = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(WIDTH, HEIGHT))),
                ..Default::default()
            };
            let output = ctx.run(raw, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| hud.panel_ui(ui));
            });
            let mut texts = Vec::new();
            fn collect<'a>(shape: &'a egui::Shape, out: &mut Vec<&'a egui::epaint::TextShape>) {
                match shape {
                    egui::Shape::Text(text) => out.push(text),
                    egui::Shape::Vec(items) => {
                        for item in items {
                            collect(item, out);
                        }
                    }
                    _ => {}
                }
            }
            for clipped in &output.shapes {
                collect(&clipped.shape, &mut texts);
            }
            assert!(
                texts.iter().any(|text| text.galley.text().contains("亮度")),
                "亮度 HUD 缺少标题"
            );
            assert!(
                texts.iter().any(|text| text.galley.text().contains(expected)),
                "亮度 HUD 缺少 {expected}"
            );
            if let Some(dest) = std::env::var_os("DAOZAI_BRIGHTNESS_PREVIEW_DIR")
                .filter(|value| !value.is_empty())
            {
                let dest = std::path::PathBuf::from(dest);
                std::fs::create_dir_all(&dest).unwrap();
                let mut raster = crate::preview_raster::Raster::new(WIDTH as usize, HEIGHT as usize);
                raster.update_textures(&output.textures_delta);
                let primitives = ctx.tessellate(output.shapes.clone(), output.pixels_per_point);
                raster.draw(&primitives, output.pixels_per_point);
                let mut rgba = Vec::new();
                raster.to_rgba8(&mut rgba);
                let name = if state.is_some() {
                    "brightness-hud.png"
                } else {
                    "brightness-hud-unsupported.png"
                };
                image::save_buffer(
                    dest.join(name),
                    &rgba,
                    WIDTH as u32,
                    HEIGHT as u32,
                    image::ColorType::Rgba8,
                )
                .unwrap();
            }
        }
    }
}
