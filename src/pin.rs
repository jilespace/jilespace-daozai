//! 贴图：把截图钉在桌面上（置顶、可拖动、右键/双击关闭）。
//!
//! 每张贴图是一个独立的无边框置顶窗口；窗口内按原始像素显示图片，
//! 拖动交给系统窗口拖拽（`ViewportCommand::StartDrag`），不占用主界面。

use eframe::egui::{self, Color32, Pos2, Rect, Stroke, Vec2};

use crate::ocr::Image;

/// 逻辑尺寸：按根窗口的 DPI 把物理像素换算成逻辑点，尽量 1:1 显示。
pub fn window_size(image: &Image, pixels_per_point: f32) -> Vec2 {
    let ppp = pixels_per_point.max(0.5);
    let (mut width, mut height) = (image.width as f32 / ppp, image.height as f32 / ppp);
    // 先按比例缩小到屏幕内，再按比例放大到可点击的最小尺寸，避免拉伸变形。
    let shrink = (1800.0 / width).min(1200.0 / height).min(1.0);
    width *= shrink;
    height *= shrink;
    let grow = (40.0 / width).max(40.0 / height).max(1.0);
    width *= grow;
    height *= grow;
    Vec2::new(width, height)
}

static SERIAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

pub struct Pin {
    image: Image,
    texture: Option<egui::TextureHandle>,
    size: Vec2,
    position: Option<Pos2>,
    viewport: egui::ViewportId,
    shown: bool,
}

impl Pin {
    pub fn new(image: Image, pixels_per_point: f32) -> Self {
        let serial = SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Self {
            size: window_size(&image, pixels_per_point),
            image,
            texture: None,
            position: None,
            viewport: egui::ViewportId::from_hash_of(("daozai-pin", serial)),
            shown: false,
        }
    }

    /// 返回 `true` 表示用户关闭了这张贴图。
    pub fn show(&mut self, ctx: &egui::Context, dark: bool) -> bool {
        let ppp = ctx.pixels_per_point();
        let position = *self.position.get_or_insert_with(|| {
            #[cfg(windows)]
            {
                crate::native::clipboard_popup_position(self.size.x, self.size.y, ppp)
                    .map(|(x, y)| egui::pos2(x, y))
                    .unwrap_or_else(|| egui::pos2(120.0, 80.0))
            }
            #[cfg(not(windows))]
            {
                let _ = ppp;
                egui::pos2(120.0, 80.0)
            }
        });
        let mut closed = false;
        let builder = egui::ViewportBuilder::default()
            .with_title("贴图 · 岛仔")
            .with_icon(crate::theme::app_icon())
            .with_inner_size([self.size.x, self.size.y])
            .with_min_inner_size([self.size.x, self.size.y])
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
            if ctx.input(|input| input.viewport().close_requested())
                || ctx.input(|input| input.key_pressed(egui::Key::Escape))
            {
                closed = true;
            }
            if self.paint(ctx, dark) {
                closed = true;
            }
        });
        closed
    }

    /// 绘制贴图内容；拆出来便于离屏渲染测试。
    pub fn paint(&mut self, ctx: &egui::Context, dark: bool) -> bool {
        let _ = dark;
        let mut closed = false;
        if self.texture.is_none() {
            // 与标注编辑器一致：超过纹理上限的截图先缩略显示，贴图窗口本身保持比例。
            let display = crate::annotate::display_image(&self.image);
            let pixels = egui::ColorImage::from_rgba_unmultiplied(
                [display.width as usize, display.height as usize],
                &display.rgba,
            );
            self.texture = Some(ctx.load_texture("pinned-shot", pixels, egui::TextureOptions::LINEAR));
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(Color32::TRANSPARENT))
            .show(ctx, |ui| {
                let rect = ui.max_rect();
                let response = ui.interact(rect, egui::Id::new("pin-surface"), egui::Sense::click_and_drag());
                if let Some(texture) = &self.texture {
                    ui.painter().add(
                        egui::epaint::RectShape::filled(rect, 8.0, Color32::WHITE)
                            .with_texture(texture.id(), Rect::from_min_max(Pos2::ZERO, egui::pos2(1.0, 1.0))),
                    );
                }
                ui.painter().rect_stroke(
                    rect.shrink(0.5),
                    8.0,
                    Stroke::new(1.0_f32, Color32::from_white_alpha(46)),
                    egui::StrokeKind::Inside,
                );
                if response.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
                    let close = Rect::from_min_size(rect.max - Vec2::splat(22.0), Vec2::splat(16.0));
                    let hit = ui.interact(close, egui::Id::new("pin-close"), egui::Sense::click());
                    ui.painter().circle_filled(close.center(), 8.0, Color32::from_black_alpha(150));
                    ui.painter().line_segment(
                        [close.center() + Vec2::new(-3.0, -3.0), close.center() + Vec2::new(3.0, 3.0)],
                        Stroke::new(1.4_f32, Color32::WHITE),
                    );
                    ui.painter().line_segment(
                        [close.center() + Vec2::new(3.0, -3.0), close.center() + Vec2::new(-3.0, 3.0)],
                        Stroke::new(1.4_f32, Color32::WHITE),
                    );
                    if hit.clicked() {
                        closed = true;
                    }
                }
                if response.clicked() || response.drag_started() {
                    if ui.input(|input| input.pointer.primary_pressed()) {
                        ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
                    }
                }
                if response.double_clicked() || ui.input(|input| input.pointer.secondary_clicked() && response.hovered()) {
                    closed = true;
                }
                if ui.input(|input| input.key_pressed(egui::Key::Escape)) {
                    closed = true;
                }
            });
        closed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient(width: u32, height: u32) -> Image {
        let mut rgba = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            for x in 0..width {
                let r = (40 + x * 160 / width.max(1)) as u8;
                let g = (80 + y * 120 / height.max(1)) as u8;
                rgba.extend_from_slice(&[r, g, 190, 255]);
            }
        }
        Image { width, height, rgba }
    }

    #[test]
    fn window_size_matches_physical_pixels_at_common_dpi() {
        let image = gradient(400, 300);
        assert_eq!(window_size(&image, 1.0), Vec2::new(400.0, 300.0));
        assert_eq!(window_size(&image, 2.0), Vec2::new(200.0, 150.0));
        let small = gradient(10, 10);
        assert_eq!(window_size(&small, 1.0), Vec2::new(40.0, 40.0), "贴图窗口不应小到无法点击");
    }

    #[test]
    fn huge_pins_shrink_preserving_aspect_ratio() {
        let image = gradient(3840, 2160);
        let size = window_size(&image, 1.0);
        assert!(size.x <= 1800.0 && size.y <= 1200.0);
        let ratio = size.x / size.y;
        assert!((ratio - 3840.0 / 2160.0).abs() < 0.01, "贴图窗口不应变形：{ratio}");
        let tall = gradient(400, 4000);
        let size = window_size(&tall, 1.0);
        assert!(size.y <= 1200.0);
        assert!((size.x / size.y - 0.1).abs() < 0.01);
    }

    #[test]
    fn pin_renders_and_paints_the_image_offscreen() {
        let ctx = egui::Context::default();
        crate::theme::load_fonts(&ctx);
        let mut pin = Pin::new(gradient(240, 160), 2.0);
        let size = pin.size;
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
            time: Some(0.0),
            viewports: std::iter::once((
                egui::ViewportId::ROOT,
                egui::ViewportInfo { native_pixels_per_point: Some(2.0), ..Default::default() },
            ))
            .collect(),
            ..Default::default()
        };
        let output = ctx.run(raw, |ctx| {
            let _ = pin.paint(ctx, true);
        });
        assert!(!output.shapes.is_empty(), "贴图应绘制图片与描边");
        if let Some(dest) = std::env::var_os("DAOZAI_PIN_PREVIEW_DIR").filter(|value| !value.is_empty()) {
            let dest = std::path::PathBuf::from(dest);
            std::fs::create_dir_all(&dest).unwrap();
            let width = (size.x * 2.0) as usize;
            let height = (size.y * 2.0) as usize;
            let mut raster = crate::preview_raster::Raster::new(width, height);
            raster.update_textures(&output.textures_delta);
            let primitives = ctx.tessellate(output.shapes.clone(), output.pixels_per_point);
            raster.draw(&primitives, output.pixels_per_point);
            let mut rgba = Vec::new();
            raster.to_rgba8(&mut rgba);
            image::save_buffer(
                dest.join("pinned-shot.png"),
                &rgba,
                width as u32,
                height as u32,
                image::ColorType::Rgba8,
            )
            .unwrap();
        }
    }
}
