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
    /// 纹理创建后即释放原图，避免多张贴图各占一份全分辨率 RGBA。
    image: Option<Image>,
    texture: Option<egui::TextureHandle>,
    base_size: Vec2,
    zoom: f32,
    opacity: f32,
    position: Option<Pos2>,
    viewport: egui::ViewportId,
    shown: bool,
    /// 持久化文件名（不含扩展名）；为空表示不保存。
    persist_key: Option<String>,
}

pub const MIN_ZOOM: f32 = 0.4;
pub const MAX_ZOOM: f32 = 3.0;
pub const MIN_OPACITY: f32 = 0.25;

impl Pin {
    pub fn new(image: Image, pixels_per_point: f32) -> Self {
        let serial = SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Self {
            base_size: window_size(&image, pixels_per_point),
            zoom: 1.0,
            opacity: 1.0,
            image: Some(image),
            texture: None,
            position: None,
            viewport: egui::ViewportId::from_hash_of(("daozai-pin", serial)),
            shown: false,
            persist_key: None,
        }
    }

    /// 从持久化记录恢复一张贴图。
    pub fn restore(
        image: Image,
        pixels_per_point: f32,
        position: Pos2,
        zoom: f32,
        opacity: f32,
        key: String,
    ) -> Self {
        let mut pin = Self::new(image, pixels_per_point);
        pin.position = Some(position);
        pin.set_zoom(zoom);
        pin.set_opacity(opacity);
        pin.persist_key = Some(key);
        pin
    }

    pub fn set_persist_key(&mut self, key: String) {
        self.persist_key = Some(key);
    }

    pub fn persist_key(&self) -> Option<&str> {
        self.persist_key.as_deref()
    }

    pub fn clear_persist_key(&mut self) {
        self.persist_key = None;
    }

    /// 尚未定位时放到鼠标附近，保证首帧保存元数据也有坐标。
    pub fn place_near_cursor(&mut self, pixels_per_point: f32) {
        if self.position.is_some() {
            return;
        }
        let size = self.display_size();
        #[cfg(windows)]
        {
            self.position = crate::native::clipboard_popup_position(size.x, size.y, pixels_per_point)
                .map(|(x, y)| egui::pos2(x, y))
                .or(Some(egui::pos2(120.0, 80.0)));
        }
        #[cfg(not(windows))]
        {
            let _ = (size, pixels_per_point);
            self.position = Some(egui::pos2(120.0, 80.0));
        }
    }

    pub fn metadata(&self) -> Option<crate::pin_store::StoredPin> {
        let position = self.position?;
        Some(crate::pin_store::StoredPin {
            key: self.persist_key.clone()?,
            x: position.x,
            y: position.y,
            zoom: self.zoom,
            opacity: self.opacity,
        })
    }

    pub fn display_size(&self) -> Vec2 {
        self.base_size * self.zoom
    }

    pub fn set_zoom(&mut self, zoom: f32) {
        self.zoom = zoom.clamp(MIN_ZOOM, MAX_ZOOM);
    }

    pub fn set_opacity(&mut self, opacity: f32) {
        self.opacity = opacity.clamp(MIN_OPACITY, 1.0);
    }

    pub fn opacity(&self) -> f32 {
        self.opacity
    }

    pub fn zoom(&self) -> f32 {
        self.zoom
    }

    /// 返回 `true` 表示用户关闭了这张贴图。
    pub fn show(&mut self, ctx: &egui::Context, dark: bool) -> bool {
        let ppp = ctx.pixels_per_point();
        let size = self.display_size();
        let position = *self.position.get_or_insert_with(|| {
            #[cfg(windows)]
            {
                crate::native::clipboard_popup_position(size.x, size.y, ppp)
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
            .with_inner_size([size.x, size.y])
            .with_min_inner_size([size.x, size.y])
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
            // 系统拖动后同步窗口位置，退出时写入持久化元数据。
            if let Some(rect) = ctx.input(|input| input.viewport().outer_rect) {
                self.position = Some(rect.min);
            }
        });
        closed
    }

    /// 绘制贴图内容；拆出来便于离屏渲染测试。
    pub fn paint(&mut self, ctx: &egui::Context, dark: bool) -> bool {
        let _ = dark;
        let mut closed = false;
        if self.texture.is_none() {
            if let Some(image) = self.image.take() {
                // 与标注编辑器一致：超过纹理上限的截图先缩略显示，贴图窗口本身保持比例。
                let display = crate::annotate::display_image(&image);
                let pixels = egui::ColorImage::from_rgba_unmultiplied(
                    [display.width as usize, display.height as usize],
                    &display.rgba,
                );
                self.texture = Some(ctx.load_texture("pinned-shot", pixels, egui::TextureOptions::LINEAR));
            }
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(Color32::TRANSPARENT))
            .show(ctx, |ui| {
                let rect = ui.max_rect();
                let response = ui.interact(rect, egui::Id::new("pin-surface"), egui::Sense::click_and_drag());
                let tint = Color32::from_white_alpha((self.opacity * 255.0).round() as u8);
                if let Some(texture) = &self.texture {
                    ui.painter().add(
                        egui::epaint::RectShape::filled(rect, 8.0, tint)
                            .with_texture(texture.id(), Rect::from_min_max(Pos2::ZERO, egui::pos2(1.0, 1.0))),
                    );
                }
                ui.painter().rect_stroke(
                    rect.shrink(0.5),
                    8.0,
                    Stroke::new(1.0_f32, Color32::from_white_alpha((46.0 * self.opacity) as u8)),
                    egui::StrokeKind::Inside,
                );
                if response.hovered() {
                    let scroll = ui.input(|input| input.raw_scroll_delta.y);
                    if scroll.abs() > 0.01 {
                        if ui.input(|input| input.modifiers.ctrl) {
                            self.set_opacity(self.opacity + scroll * 0.002);
                        } else {
                            self.set_zoom(self.zoom * (1.0 + scroll * 0.0015));
                            ui.ctx().send_viewport_cmd(egui::ViewportCommand::InnerSize(self.display_size()));
                        }
                    }
                }
                if response.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
                    let hint = format!("{:.0}%  ·  不透明度 {:.0}%", self.zoom * 100.0, self.opacity * 100.0);
                    let galley = ui.painter().layout_no_wrap(hint, egui::FontId::proportional(9.0), Color32::WHITE);
                    let hint_rect = Rect::from_min_size(
                        rect.left_bottom() + Vec2::new(6.0, -galley.size().y - 6.0),
                        galley.size() + Vec2::new(10.0, 4.0),
                    );
                    ui.painter().rect_filled(hint_rect, 5.0, Color32::from_black_alpha(160));
                    ui.painter().galley(hint_rect.min + Vec2::new(5.0, 2.0), galley, Color32::WHITE);
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
                // 只在真正拖动时交给系统拖窗口，普通点击（关闭按钮/双击）不被吞掉。
                if response.drag_started() {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
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
    fn pin_releases_source_image_after_uploading_texture() {
        let ctx = egui::Context::default();
        crate::theme::load_fonts(&ctx);
        let mut pin = Pin::new(gradient(120, 80), 2.0);
        assert!(pin.image.is_some());
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, pin.display_size())),
            ..Default::default()
        };
        let _ = ctx.run(raw, |ctx| {
            let _ = pin.paint(ctx, true);
        });
        assert!(pin.image.is_none(), "上传纹理后应释放全分辨率原图");
        assert!(pin.texture.is_some());
    }

    #[test]
    fn persisted_pins_expose_metadata_and_restore_geometry() {
        let ctx = egui::Context::default();
        let mut pin = Pin::new(gradient(120, 80), 2.0);
        pin.place_near_cursor(2.0);
        assert!(pin.metadata().is_none(), "未设置持久化 key 时不写元数据");
        pin.set_persist_key("pin-test".into());
        let record = pin.metadata().expect("定位后应可生成元数据");
        assert_eq!(record.key, "pin-test");
        pin.set_zoom(1.5);
        pin.set_opacity(0.6);
        let restored = Pin::restore(gradient(120, 80), 2.0, egui::pos2(record.x, record.y), 1.5, 0.6, "pin-test".into());
        assert_eq!(restored.persist_key(), Some("pin-test"));
        assert!((restored.zoom() - 1.5).abs() < 1e-6);
        assert!((restored.opacity() - 0.6).abs() < 1e-6);
        let meta = restored.metadata().expect("恢复的贴图应带元数据");
        assert!((meta.zoom - 1.5).abs() < 1e-6);
        let _ = ctx;
    }

    #[test]
    fn zoom_and_opacity_are_clamped() {
        let mut pin = Pin::new(gradient(200, 100), 2.0);
        let base = pin.display_size();
        pin.set_zoom(2.0);
        assert_eq!(pin.display_size(), base * 2.0);
        pin.set_zoom(99.0);
        assert_eq!(pin.zoom(), MAX_ZOOM);
        pin.set_zoom(0.01);
        assert_eq!(pin.zoom(), MIN_ZOOM);
        pin.set_opacity(0.5);
        assert!((pin.opacity() - 0.5).abs() < 1e-6);
        pin.set_opacity(0.0);
        assert!((pin.opacity() - MIN_OPACITY).abs() < 1e-6);
        pin.set_opacity(5.0);
        assert!((pin.opacity() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn pin_renders_and_paints_the_image_offscreen() {
        let ctx = egui::Context::default();
        crate::theme::load_fonts(&ctx);
        let mut pin = Pin::new(gradient(240, 160), 2.0);
        pin.set_zoom(1.25);
        pin.set_opacity(0.85);
        let size = pin.display_size();
        let mut last = None;
        let mut textures = Vec::new();
        for frame in 0..2 {
            let events = if frame == 1 {
                vec![egui::Event::PointerMoved(Pos2::new(size.x / 2.0, size.y / 2.0))]
            } else {
                Vec::new()
            };
            let raw = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
                time: Some(frame as f64 / 60.0),
                events,
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
            textures.push(output.textures_delta.clone());
            last = Some(output);
        }
        let output = last.expect("至少渲染一帧");
        assert!(!output.shapes.is_empty(), "贴图应绘制图片与描边");
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
        let mut texts = Vec::new();
        for clipped in &output.shapes {
            collect(&clipped.shape, &mut texts);
        }
        assert!(texts.iter().any(|text| text.galley.text().contains("不透明度")), "悬停应显示缩放/透明度提示");
        if let Some(dest) = std::env::var_os("DAOZAI_PIN_PREVIEW_DIR").filter(|value| !value.is_empty()) {
            let dest = std::path::PathBuf::from(dest);
            std::fs::create_dir_all(&dest).unwrap();
            let width = (size.x * 2.0) as usize;
            let height = (size.y * 2.0) as usize;
            let mut raster = crate::preview_raster::Raster::new(width, height);
            for delta in &textures {
                raster.update_textures(delta);
            }
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
