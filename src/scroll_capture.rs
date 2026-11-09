//! 滚动截图：框选固定区域后，用户滚动页面，程序反复截取该区域并按重叠行拼接成长图。
//!
//! - 拼接核心 `Stitcher` 是纯函数式逻辑（灰度下采样 + 重叠匹配），带确定性测试；
//! - 采集在界面线程按 350ms 间隔进行（区域较小），HUD 用独立置顶小窗显示进度，
//!   并提供「完成 / 取消」；HUD 会尽量放在选区外，且尝试排除出屏幕捕获。

use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, Pos2, Stroke};

use crate::ocr::Image;
use crate::region_capture::{capture_region, MonitorRect};

pub const MAX_HEIGHT: u32 = 8000;
pub const MAX_OVERLAP_BAND: u32 = 64;
pub const MAX_MATCH_ERROR: f32 = 14.0;
pub const INTERVAL: Duration = Duration::from_millis(350);
pub const HUD_W: f32 = 320.0;
pub const HUD_H: f32 = 84.0;
const HUD_TITLE: &str = "滚动截图 · 岛仔";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Append {
    Added(u32),
    NoChange,
    NoOverlap,
    Full,
}

pub struct Stitcher {
    image: Image,
}

impl Stitcher {
    pub fn new(first: Image) -> Self {
        Self { image: first }
    }

    pub fn height(&self) -> u32 {
        self.image.height
    }

    pub fn image(&self) -> &Image {
        &self.image
    }

    pub fn append(&mut self, next: &Image) -> Append {
        if next.width != self.image.width || next.height == 0 {
            return Append::NoOverlap;
        }
        if self.image.height >= MAX_HEIGHT {
            return Append::Full;
        }
        let band = MAX_OVERLAP_BAND.min(next.height).min(self.image.height) as usize;
        if band < 8 {
            return Append::NoOverlap;
        }
        let Some((offset, error)) = self.best_match(next, band) else {
            return Append::NoOverlap;
        };
        if error > MAX_MATCH_ERROR {
            return Append::NoOverlap;
        }
        let new_start = self.image.height.saturating_sub(offset as u32) as usize;
        if new_start >= next.height as usize {
            return Append::NoChange;
        }
        let room = (MAX_HEIGHT - self.image.height) as usize;
        let rows = (next.height as usize - new_start).min(room);
        if rows == 0 {
            return Append::Full;
        }
        let stride = self.image.width as usize * 4;
        let start = new_start * stride;
        let end = start + rows * stride;
        self.image.rgba.extend_from_slice(&next.rgba[start..end]);
        self.image.height += rows as u32;
        Append::Added(rows as u32)
    }

    /// 在已拼接图底部搜索新图第一行对应的位置，返回 (offset, 平均灰度误差)。
    fn best_match(&self, next: &Image, band: usize) -> Option<(usize, f32)> {
        let total = self.image.height as usize;
        if total < band {
            return None;
        }
        let low = total.saturating_sub(next.height as usize);
        let high = total - band;
        if low > high {
            return None;
        }
        let width = self.image.width as usize;
        let stride_x = (width / 160).max(1);
        let stride_y = (band / 40).max(1);
        let mut best: Option<(usize, f32)> = None;
        for offset in low..=high {
            let mut sum = 0u64;
            let mut count = 0u64;
            for row in (0..band).step_by(stride_y) {
                let next_row = row * width * 4;
                let stitched_row = (offset + row) * width * 4;
                for column in (0..width).step_by(stride_x) {
                    let a = next_row + column * 4;
                    let b = stitched_row + column * 4;
                    let gray_a = (next.rgba[a] as u32 * 3 + next.rgba[a + 1] as u32 * 4 + next.rgba[a + 2] as u32 * 2) / 9;
                    let gray_b = (self.image.rgba[b] as u32 * 3 + self.image.rgba[b + 1] as u32 * 4 + self.image.rgba[b + 2] as u32 * 2) / 9;
                    sum += gray_a.abs_diff(gray_b) as u64;
                    count += 1;
                }
            }
            if count == 0 {
                continue;
            }
            let error = sum as f32 / count as f32;
            if best.is_none_or(|(_, best_error)| error < best_error) {
                best = Some((offset, error));
            }
        }
        best
    }
}

#[derive(Clone, Debug)]
pub enum Event {
    None,
    Finished(Image),
    Cancelled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HudAction {
    None,
    Finish,
    Cancel,
}

pub struct Capture {
    active: bool,
    stitcher: Stitcher,
    region: MonitorRect,
    status: String,
    frames: u32,
    last_capture: Instant,
    failures: u32,
    viewport: egui::ViewportId,
    position: Option<Pos2>,
    shown: bool,
    excluded: bool,
}

impl Capture {
    /// 开始滚动截图；`first` 是选区当前画面。
    pub fn start(ctx: &egui::Context, monitor: MonitorRect, selection: (i32, i32, u32, u32), first: Image) -> Option<Self> {
        if selection.2 < 16 || selection.3 < 16 || first.width != selection.2 || first.height != selection.3 {
            return None;
        }
        let ppp = ctx.pixels_per_point().max(0.5);
        let region = MonitorRect {
            x: monitor.x + selection.0,
            y: monitor.y + selection.1,
            width: selection.2,
            height: selection.3,
        };
        // HUD 放在选区上方；空间不足时放下方，尽量不遮挡被截取的区域。
        let selection_left = region.x as f32 / ppp;
        let selection_top = region.y as f32 / ppp;
        let selection_bottom = (region.y + region.height as i32) as f32 / ppp;
        let monitor_left = monitor.x as f32 / ppp;
        let monitor_top = monitor.y as f32 / ppp;
        let monitor_bottom = (monitor.y + monitor.height as i32) as f32 / ppp;
        let mut x = selection_left + region.width as f32 / ppp / 2.0 - HUD_W / 2.0;
        let mut y = selection_top - HUD_H - 10.0;
        if y < monitor_top + 6.0 {
            y = selection_bottom + 10.0;
        }
        if y + HUD_H > monitor_bottom - 6.0 {
            y = (monitor_top + 6.0).max(monitor_bottom - HUD_H - 6.0);
        }
        x = x.clamp(monitor_left + 6.0, (monitor_left + monitor.width as f32 / ppp - HUD_W - 6.0).max(monitor_left + 6.0));
        Some(Self {
            active: true,
            stitcher: Stitcher::new(first),
            region,
            status: "滚动页面，自动拼接".into(),
            frames: 1,
            last_capture: Instant::now(),
            failures: 0,
            viewport: egui::ViewportId::from_hash_of("daozai-scroll-capture"),
            position: Some(egui::pos2(x, y)),
            shown: false,
            excluded: false,
        })
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    /// 每帧调用：按间隔补拍、更新 HUD，返回完成/取消事件。
    pub fn poll(&mut self, ctx: &egui::Context) -> Event {
        if !self.active {
            return Event::None;
        }
        let mut event = Event::None;
        if self.last_capture.elapsed() >= INTERVAL {
            self.last_capture = Instant::now();
            match capture_region(self.region) {
                Ok(frame) => {
                    self.frames += 1;
                    match self.stitcher.append(&frame) {
                        Append::Added(rows) => {
                            self.failures = 0;
                            self.status = format!("已拼接 +{rows}px · 共 {}px", self.stitcher.height());
                        }
                        Append::NoChange => {
                            self.failures = 0;
                            self.status = "页面未移动，等待滚动…".into();
                        }
                        Append::NoOverlap => {
                            self.failures += 1;
                            self.status = "未检测到重叠，请放慢滚动速度".into();
                        }
                        Append::Full => {
                            self.status = "已达到最大长度，正在完成…".into();
                            event = Event::Finished(self.stitcher.image.clone());
                        }
                    }
                }
                Err(error) => {
                    self.failures += 1;
                    self.status = format!("截取失败：{error}");
                }
            }
        }
        let action = self.show_hud(ctx);
        if matches!(event, Event::None) {
            event = match action {
                HudAction::Finish => Event::Finished(self.stitcher.image.clone()),
                HudAction::Cancel => Event::Cancelled,
                HudAction::None => Event::None,
            };
        }
        if !matches!(event, Event::None) {
            self.active = false;
        }
        event
    }

    /// 外部（再次按热键）结束并取回拼接结果。
    pub fn finish(&mut self) -> Option<Image> {
        if !self.active {
            return None;
        }
        self.active = false;
        Some(self.stitcher.image.clone())
    }

    fn hud_ui(&self, ui: &mut egui::Ui, action: &mut HudAction) {
        egui::Frame::new()
            .fill(Color32::from_rgb(38, 40, 46))
            .stroke(Stroke::new(1.0_f32, Color32::from_rgb(66, 68, 76)))
            .corner_radius(12.0)
            .inner_margin(egui::Margin::symmetric(12, 8))
            .show(ui, |ui| {
                ui.set_min_width(HUD_W - 24.0);
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("滚动截图")
                            .size(12.5)
                            .strong()
                            .color(Color32::WHITE),
                    );
                    ui.label(
                        egui::RichText::new(format!("{}×{} · {}", self.region.width, self.region.height, self.frames))
                            .size(10.0)
                            .color(Color32::from_gray(150)),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add(egui::Button::new(egui::RichText::new("取消").size(10.5)).frame(false))
                            .clicked()
                        {
                            *action = HudAction::Cancel;
                        }
                        if ui
                            .add(
                                egui::Button::new(
                                    egui::RichText::new("完成").size(10.5).strong().color(Color32::from_rgb(120, 190, 255)),
                                )
                                .frame(false),
                            )
                            .clicked()
                        {
                            *action = HudAction::Finish;
                        }
                    });
                });
                ui.label(
                    egui::RichText::new(&self.status)
                        .size(10.5)
                        .color(Color32::from_gray(190)),
                );
            });
    }

    fn show_hud(&mut self, ctx: &egui::Context) -> HudAction {
        let mut action = HudAction::None;
        let position = self.position.unwrap_or(egui::pos2(80.0, 80.0));
        let builder = egui::ViewportBuilder::default()
            .with_title(HUD_TITLE)
            .with_icon(crate::theme::app_icon())
            .with_inner_size([HUD_W, HUD_H])
            .with_min_inner_size([HUD_W, HUD_H])
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
            if !self.excluded {
                #[cfg(windows)]
                {
                    self.excluded = crate::native::exclude_from_capture(HUD_TITLE);
                }
                #[cfg(not(windows))]
                {
                    self.excluded = true;
                }
            }
            egui::CentralPanel::default()
                .frame(egui::Frame::new().fill(Color32::TRANSPARENT).inner_margin(6.0))
                .show(ctx, |ui| self.hud_ui(ui, &mut action));
        });
        action
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn content(height: u32) -> Image {
        let width = 60;
        let mut rgba = Vec::new();
        for y in 0..height {
            for x in 0..width {
                let value = ((y * 7 + x * 3) % 256) as u8;
                rgba.extend_from_slice(&[value, value / 2, 255 - value, 255]);
            }
        }
        Image { width, height, rgba }
    }

    fn slice(content: &Image, from: u32, height: u32) -> Image {
        let stride = content.width as usize * 4;
        Image {
            width: content.width,
            height,
            rgba: content.rgba[from as usize * stride..(from + height) as usize * stride].to_vec(),
        }
    }

    fn matches(stitched: &Image, content: &Image, rows: u32) -> bool {
        let stride = stitched.width as usize * 4;
        let start = (stitched.height - rows) as usize * stride;
        let end = stitched.height as usize * stride;
        stitched.rgba[start..] == content.rgba[start..end]
    }

    #[test]
    fn stitches_scrolled_frames_by_overlap() {
        let content = content(600);
        let mut stitcher = Stitcher::new(slice(&content, 0, 240));
        assert_eq!(stitcher.height(), 240);
        // 向下滚 80px：新截图顶部与已拼接顶部错开 80 行。
        assert_eq!(stitcher.append(&slice(&content, 80, 240)), Append::Added(80));
        assert_eq!(stitcher.height(), 320);
        assert!(matches(stitcher.image(), &content, 320));
        // 页面没动：不追加。
        assert_eq!(stitcher.append(&slice(&content, 80, 240)), Append::NoChange);
        assert_eq!(stitcher.height(), 320);
        // 再滚动 40px。
        assert_eq!(stitcher.append(&slice(&content, 120, 240)), Append::Added(40));
        assert_eq!(stitcher.height(), 360);
        // 画面完全不重叠时不追加也不 panic。
        let jump = slice(&content, 560, 40);
        assert_eq!(stitcher.append(&jump), Append::NoOverlap);
        assert_eq!(stitcher.height(), 360);
    }

    #[test]
    fn width_mismatch_and_max_height_are_rejected() {
        let content = content(300);
        let mut stitcher = Stitcher::new(slice(&content, 0, 120));
        let mut wrong = slice(&content, 10, 100);
        wrong.width += 1;
        wrong.rgba.extend(std::iter::repeat(0).take(100 * 4));
        assert_eq!(stitcher.append(&wrong), Append::NoOverlap);
        assert_eq!(stitcher.height(), 120);
    }

    #[test]
    fn hud_renders_progress_and_actions() {
        let ctx = egui::Context::default();
        crate::theme::load_fonts(&ctx);
        let first = slice(&content(400), 0, 240);
        let capture = Capture::start(
            &ctx,
            MonitorRect { x: 0, y: 0, width: 1920, height: 1080 },
            (100, 100, 60, 240),
            first,
        )
        .expect("应能开始滚动截图");
        assert!(capture.is_active());
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(HUD_W, HUD_H))),
            time: Some(0.0),
            viewports: std::iter::once((
                egui::ViewportId::ROOT,
                egui::ViewportInfo { native_pixels_per_point: Some(2.0), ..Default::default() },
            ))
            .collect(),
            ..Default::default()
        };
        let output = ctx.run(raw, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let mut action = HudAction::None;
                capture.hud_ui(ui, &mut action);
            });
        });
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
        for expected in ["滚动截图", "完成", "取消", "滚动页面"] {
            assert!(texts.iter().any(|text| text.galley.text().contains(expected)), "HUD 缺少 {expected}");
        }
        if let Some(dest) = std::env::var_os("DAOZAI_SCROLL_PREVIEW_DIR").filter(|value| !value.is_empty()) {
            let dest = std::path::PathBuf::from(dest);
            std::fs::create_dir_all(&dest).unwrap();
            let mut raster = crate::preview_raster::Raster::new(
                (HUD_W * 2.0) as usize,
                (HUD_H * 2.0) as usize,
            );
            raster.update_textures(&output.textures_delta);
            let primitives = ctx.tessellate(output.shapes.clone(), output.pixels_per_point);
            raster.draw(&primitives, output.pixels_per_point);
            let mut rgba = Vec::new();
            raster.to_rgba8(&mut rgba);
            image::save_buffer(
                dest.join("scroll-capture-hud.png"),
                &rgba,
                (HUD_W * 2.0) as u32,
                (HUD_H * 2.0) as u32,
                image::ColorType::Rgba8,
            )
            .unwrap();
        }
    }

    #[test]
    fn matching_handles_low_contrast_gradients() {
        let width = 40u32;
        let mut rgba = Vec::new();
        for y in 0..400u32 {
            for _ in 0..width {
                let value = (20 + y / 8) as u8;
                rgba.extend_from_slice(&[value, value, value, 255]);
            }
        }
        let content = Image { width, height: 400, rgba };
        let mut stitcher = Stitcher::new(slice(&content, 0, 160));
        assert_eq!(stitcher.append(&slice(&content, 60, 160)), Append::Added(60));
    }
}
