//! 媒体控制 HUD：播放/暂停、上一首/下一首时在屏幕底部弹出的轻量 OSD。

use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, Rect, Stroke, Vec2};

use crate::smtc::NowPlaying;

pub const WIDTH: f32 = 344.0;
pub const HEIGHT: f32 = 96.0;
const VISIBLE_FOR: Duration = Duration::from_millis(1600);

pub struct Hud {
    open: bool,
    state: NowPlaying,
    action: &'static str,
    shown_at: Instant,
    viewport: egui::ViewportId,
    shown: bool,
    artwork: Option<(String, egui::TextureHandle)>,
}

impl Default for Hud {
    fn default() -> Self {
        Self {
            open: false,
            state: NowPlaying::default(),
            action: "",
            shown_at: Instant::now(),
            viewport: egui::ViewportId::from_hash_of("daozai-media-hud"),
            shown: false,
            artwork: None,
        }
    }
}

impl Hud {
    pub fn show(&mut self, state: NowPlaying, action: &'static str) {
        self.state = state;
        self.action = action;
        self.open = true;
        self.shown_at = Instant::now();
        self.shown = false;
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn poll(&mut self, ctx: &egui::Context, state: NowPlaying) {
        if !self.open {
            return;
        }
        if self.shown_at.elapsed() >= VISIBLE_FOR {
            self.open = false;
            self.shown = false;
            return;
        }
        self.state = state;
        let key = format!("{}|{}", self.state.title, self.state.artist);
        if self.artwork.as_ref().map(|(cached, _)| cached != &key).unwrap_or(true) {
            self.artwork = None;
            if let Some(image) = self.state.artwork.as_ref() {
                let texture = ctx.load_texture(
                    format!("media-hud-{key}"),
                    image.as_ref().clone(),
                    egui::TextureOptions::LINEAR,
                );
                self.artwork = Some((key, texture));
            }
        }
        let monitor = ctx
            .input(|input| input.viewport().monitor_size)
            .unwrap_or(Vec2::new(1280.0, 800.0));
        let position = egui::pos2((monitor.x - WIDTH) / 2.0, monitor.y - HEIGHT - 190.0);
        let builder = egui::ViewportBuilder::default()
            .with_title("媒体 · 岛仔")
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
            .inner_margin(egui::Margin::symmetric(14, 10))
            .show(ui, |ui| {
                ui.set_min_width(WIDTH - 40.0);
                ui.horizontal(|ui| {
                    let (art_rect, _) = ui.allocate_exact_size(Vec2::splat(56.0), egui::Sense::hover());
                    if let Some((_, texture)) = &self.artwork {
                        ui.painter().image(
                            texture.id(),
                            art_rect,
                            Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                            Color32::WHITE,
                        );
                        ui.painter().rect_stroke(
                            art_rect,
                            9.0,
                            Stroke::new(1.0_f32, Color32::from_white_alpha(28)),
                            egui::StrokeKind::Inside,
                        );
                    } else {
                        ui.painter().rect_filled(art_rect, 9.0, Color32::from_rgb(52, 96, 150));
                        let center = art_rect.center();
                        if self.state.playing {
                            ui.painter().circle_filled(center, 11.0, Color32::from_white_alpha(220));
                            ui.painter().add(egui::Shape::convex_polygon(
                                vec![
                                    center + Vec2::new(-3.0, -5.0),
                                    center + Vec2::new(-3.0, 5.0),
                                    center + Vec2::new(5.0, 0.0),
                                ],
                                Color32::from_rgb(40, 78, 122),
                                Stroke::NONE,
                            ));
                        } else {
                            for offset in [-4.0f32, 2.0] {
                                ui.painter().rect_filled(
                                    Rect::from_center_size(center + Vec2::new(offset, 0.0), Vec2::new(3.0, 12.0)),
                                    1.5,
                                    Color32::from_white_alpha(220),
                                );
                            }
                        }
                    }
                    ui.add_space(10.0);
                    ui.vertical(|ui| {
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(if self.state.source.trim().is_empty() {
                                    "媒体"
                                } else {
                                    self.state.source.trim()
                                })
                                .size(9.5)
                                .color(Color32::from_gray(150)),
                            );
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                ui.label(
                                    egui::RichText::new(if self.state.playing {
                                        "播放中"
                                    } else {
                                        "已暂停"
                                    })
                                    .size(9.5)
                                    .color(if self.state.playing {
                                        Color32::from_rgb(120, 200, 140)
                                    } else {
                                        Color32::from_gray(155)
                                    }),
                                );
                            });
                        });
                        ui.add_space(2.0);
                        let title = if self.state.title.trim().is_empty() {
                            "没有正在播放的媒体"
                        } else {
                            self.state.title.trim()
                        };
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(title)
                                    .size(13.0)
                                    .strong()
                                    .color(Color32::WHITE),
                            )
                            .truncate(),
                        );
                        let artist = self.state.artist.trim();
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(if artist.is_empty() {
                                    self.action.to_string()
                                } else {
                                    artist.to_string()
                                })
                                .size(10.5)
                                .color(Color32::from_gray(175)),
                            )
                            .truncate(),
                        );
                    });
                });
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> NowPlaying {
        NowPlaying {
            title: "夜色温柔".into(),
            artist: "岛仔电台".into(),
            source: "Spotify".into(),
            playing: true,
            ..Default::default()
        }
    }

    #[test]
    fn hud_renders_title_artist_and_play_state() {
        let ctx = egui::Context::default();
        crate::theme::load_fonts(&ctx);
        let hud = Hud {
            state: sample(),
            action: "下一首",
            ..Default::default()
        };
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
        for expected in ["夜色温柔", "岛仔电台", "Spotify", "播放中"] {
            assert!(
                texts.iter().any(|text| text.galley.text().contains(expected)),
                "媒体 HUD 缺少 {expected}"
            );
        }
        if let Some(dest) = std::env::var_os("DAOZAI_MEDIA_PREVIEW_DIR")
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
            image::save_buffer(
                dest.join("media-hud.png"),
                &rgba,
                WIDTH as u32,
                HEIGHT as u32,
                image::ColorType::Rgba8,
            )
            .unwrap();
        }
    }
}
