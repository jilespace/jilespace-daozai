//! 音量 HUD：调整音量/静音时在屏幕底部居中弹出的轻量 OSD，1.4 秒后自动消失。

use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, Rect, Stroke, Vec2};

use crate::volume::State;

pub const WIDTH: f32 = 300.0;
pub const HEIGHT: f32 = 78.0;
const VISIBLE_FOR: Duration = Duration::from_millis(1400);

pub struct Hud {
    open: bool,
    state: State,
    shown_at: Instant,
    viewport: egui::ViewportId,
    shown: bool,
}

impl Default for Hud {
    fn default() -> Self {
        Self {
            open: false,
            state: State { level: 0.0, muted: false },
            shown_at: Instant::now(),
            viewport: egui::ViewportId::from_hash_of("daozai-volume-hud"),
            shown: false,
        }
    }
}

impl Hud {
    pub fn show(&mut self, state: State) {
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
            .with_title("音量 · 岛仔")
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
                    draw_speaker(ui, self.state.muted);
                    ui.add_space(8.0);
                    ui.vertical(|ui| {
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(if self.state.muted { "已静音" } else { "音量" })
                                    .size(12.5)
                                    .strong()
                                    .color(if self.state.muted {
                                        Color32::from_rgb(235, 176, 70)
                                    } else {
                                        Color32::WHITE
                                    }),
                            );
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                ui.label(
                                    egui::RichText::new(format!(
                                        "{}%",
                                        (self.state.level * 100.0).round() as i32
                                    ))
                                    .size(12.0)
                                    .color(Color32::from_gray(200)),
                                );
                            });
                        });
                        ui.add_space(6.0);
                        let (rect, _) =
                            ui.allocate_exact_size(Vec2::new(WIDTH - 92.0, 6.0), egui::Sense::hover());
                        ui.painter().rect_filled(rect, 3.0, Color32::from_gray(54));
                        let fill = if self.state.muted {
                            Color32::from_gray(120)
                        } else {
                            Color32::from_rgb(84, 160, 250)
                        };
                        if self.state.level > 0.005 {
                            let width = (rect.width() * self.state.level).max(3.0);
                            ui.painter().rect_filled(
                                Rect::from_min_size(rect.min, Vec2::new(width, rect.height())),
                                3.0,
                                fill,
                            );
                        }
                    });
                });
            });
    }
}

fn draw_speaker(ui: &mut egui::Ui, muted: bool) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(26.0), egui::Sense::hover());
    let painter = ui.painter();
    let center = rect.center();
    let color = if muted {
        Color32::from_rgb(235, 176, 70)
    } else {
        Color32::from_gray(225)
    };
    let body = Rect::from_center_size(center + Vec2::new(-5.0, 0.0), Vec2::new(6.0, 8.0));
    painter.rect_filled(body, 1.0, color);
    let tip = vec![
        center + Vec2::new(-2.0, -5.0),
        center + Vec2::new(5.0, -9.0),
        center + Vec2::new(5.0, 9.0),
        center + Vec2::new(-2.0, 5.0),
    ];
    painter.add(egui::Shape::convex_polygon(tip, color, Stroke::NONE));
    if muted {
        painter.line_segment(
            [center + Vec2::new(8.0, -7.0), center + Vec2::new(15.0, 7.0)],
            Stroke::new(1.6_f32, color),
        );
        painter.line_segment(
            [center + Vec2::new(15.0, -7.0), center + Vec2::new(8.0, 7.0)],
            Stroke::new(1.6_f32, color),
        );
    } else {
        painter.circle_stroke(center + Vec2::new(7.0, 0.0), 5.5, Stroke::new(1.3_f32, color));
        painter.circle_stroke(center + Vec2::new(7.0, 0.0), 9.0, Stroke::new(1.1_f32, color));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::Pos2;

    #[test]
    fn hud_renders_level_and_mute_state() {
        for (state, expected) in [
            (State { level: 0.42, muted: false }, "42%"),
            (State { level: 0.0, muted: true }, "已静音"),
        ] {
            let ctx = egui::Context::default();
            crate::theme::load_fonts(&ctx);
            let hud = Hud { state, ..Default::default() };
            let raw = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(WIDTH, HEIGHT))),
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
                texts.iter().any(|text| text.galley.text().contains(expected)),
                "HUD 缺少 {expected}"
            );
            if let Some(dest) = std::env::var_os("DAOZAI_VOLUME_PREVIEW_DIR")
                .filter(|value| !value.is_empty())
            {
                let dest = std::path::PathBuf::from(dest);
                std::fs::create_dir_all(&dest).unwrap();
                let width = WIDTH as usize;
                let height = HEIGHT as usize;
                let mut raster = crate::preview_raster::Raster::new(width, height);
                raster.update_textures(&output.textures_delta);
                let primitives = ctx.tessellate(output.shapes.clone(), output.pixels_per_point);
                raster.draw(&primitives, output.pixels_per_point);
                let mut rgba = Vec::new();
                raster.to_rgba8(&mut rgba);
                let name = if state.muted { "volume-hud-muted.png" } else { "volume-hud.png" };
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
