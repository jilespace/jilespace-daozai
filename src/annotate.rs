//! 截图标注：框选后的图片进入一个轻量编辑器，可画笔、矩形、箭头，撤销后复制或保存。
//!
//! 绘制分两层：
//! - 界面里用 egui painter 把矢量形状实时叠在截图纹理上；
//! - 复制/保存时用纯函数 `rasterize` 把形状合成进 RGBA，输出与截图同尺寸的图片。

use eframe::egui::{self, Color32, Pos2, Rect, Stroke, Vec2};
use crate::ocr::Image;

pub const MAX_CANVAS: (f32, f32) = (860.0, 600.0);
pub const MIN_WINDOW_W: f32 = 760.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Pen,
    Rect,
    Arrow,
    Text,
    Mosaic,
    Highlight,
    Number,
}

impl Tool {
    pub fn label(self) -> &'static str {
        match self {
            Self::Pen => "画笔",
            Self::Rect => "矩形",
            Self::Arrow => "箭头",
            Self::Text => "文字",
            Self::Mosaic => "马赛克",
            Self::Highlight => "高亮",
            Self::Number => "序号",
        }
    }
}

pub const COLORS: [[u8; 3]; 5] = [[255, 69, 58], [255, 214, 10], [10, 132, 255], [255, 255, 255], [20, 20, 22]];
pub const WIDTHS: [f32; 3] = [3.0, 6.0, 11.0];
/// 文字工具的字号（像素），与线宽档位共用三个位置。
pub const TEXT_SIZES: [f32; 3] = [20.0, 30.0, 42.0];
/// 马赛克的块大小（原始像素）。
pub const MOSAIC_BLOCKS: [f32; 3] = [8.0, 14.0, 22.0];

#[derive(Clone, Debug)]
pub struct Shape {
    pub tool: Tool,
    pub color: [u8; 3],
    pub width: f32,
    /// 画笔是连续轨迹，矩形/箭头只取首尾两点，文字点表示左上角。
    pub points: Vec<(f32, f32)>,
    /// 文字工具的内容。
    pub text: Option<String>,
}

impl Shape {
    fn new(tool: Tool, color: [u8; 3], width: f32, point: (f32, f32)) -> Self {
        Self { tool, color, width, points: vec![point], text: None }
    }
}

/// 显示用缩略图：超过 egui 纹理上限时按整数倍做块平均，最多缩到 2048 像素。
pub fn display_image(image: &Image) -> Image {
    const LIMIT: u32 = 2048;
    let longest = image.width.max(image.height);
    if longest <= LIMIT {
        return image.clone();
    }
    let factor = longest.div_ceil(LIMIT);
    let width = image.width.div_ceil(factor);
    let height = image.height.div_ceil(factor);
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            let mut sum = [0u32; 4];
            let mut count = 0u32;
            for dy in 0..factor {
                for dx in 0..factor {
                    let source_x = x * factor + dx;
                    let source_y = y * factor + dy;
                    if source_x >= image.width || source_y >= image.height {
                        continue;
                    }
                    let index = ((source_y * image.width + source_x) * 4) as usize;
                    for channel in 0..4 {
                        sum[channel] += image.rgba[index + channel] as u32;
                    }
                    count += 1;
                }
            }
            let count = count.max(1);
            for channel in 0..4 {
                rgba.push((sum[channel] / count) as u8);
            }
        }
    }
    Image { width, height, rgba }
}

/// 把形状合成到图片上；返回新图片，不改动原图。
pub fn rasterize(base: &Image, shapes: &[Shape]) -> Image {
    let mut out = base.clone();
    for shape in shapes {
        match shape.tool {
            Tool::Pen => {
                for pair in shape.points.windows(2) {
                    line(&mut out, pair[0], pair[1], shape.width, shape.color);
                }
                if shape.points.len() == 1 {
                    disk(&mut out, shape.points[0], shape.width / 2.0, shape.color);
                }
            }
            Tool::Rect => {
                if let Some((start, end)) = ends(shape) {
                    let (x0, y0) = (start.0.min(end.0), start.1.min(end.1));
                    let (x1, y1) = (start.0.max(end.0), start.1.max(end.1));
                    line(&mut out, (x0, y0), (x1, y0), shape.width, shape.color);
                    line(&mut out, (x1, y0), (x1, y1), shape.width, shape.color);
                    line(&mut out, (x1, y1), (x0, y1), shape.width, shape.color);
                    line(&mut out, (x0, y1), (x0, y0), shape.width, shape.color);
                }
            }
            Tool::Arrow => {
                if let Some((start, end)) = ends(shape) {
                    line(&mut out, start, end, shape.width, shape.color);
                    let length = (shape.width * 4.0).max(14.0);
                    let angle = (end.1 - start.1).atan2(end.0 - start.0);
                    for side in [0.55f32, -0.55] {
                        let point = (end.0 - length * (angle + side).cos(), end.1 - length * (angle + side).sin());
                        line(&mut out, end, point, shape.width, shape.color);
                    }
                }
            }
            Tool::Text => {
                if let (Some(start), Some(text)) = (shape.points.first(), shape.text.as_deref()) {
                    if let Some(glyphs) = text_bitmap(text, shape.width, shape.color) {
                        blend_bitmap(&mut out, *start, &glyphs);
                    }
                }
            }
            Tool::Number => {
                if let Some(point) = shape.points.first() {
                    let radius = number_radius(base.width);
                    disk(&mut out, *point, radius, [255, 255, 255]);
                    disk(&mut out, *point, (radius - 2.0).max(1.0), shape.color);
                    if let Some(number) = shape.text.as_deref() {
                        if let Some(glyphs) = text_bitmap(number, radius * 1.15, [255, 255, 255]) {
                            blend_bitmap_centered(&mut out, *point, &glyphs);
                        }
                    }
                }
            }
            Tool::Mosaic => {
                if let Some((start, end)) = ends(shape) {
                    let (x0, y0) = (start.0.min(end.0), start.1.min(end.1));
                    let (x1, y1) = (start.0.max(end.0), start.1.max(end.1));
                    mosaic(&mut out, (x0, y0, x1, y1), shape.width);
                }
            }
            Tool::Highlight => {
                if let Some((start, end)) = ends(shape) {
                    let (x0, y0) = (start.0.min(end.0), start.1.min(end.1));
                    let (x1, y1) = (start.0.max(end.0), start.1.max(end.1));
                    highlight(&mut out, (x0, y0, x1, y1), shape.color);
                }
            }
        }
    }
    out
}

fn rect_bounds(image: &Image, area: (f32, f32, f32, f32)) -> Option<(i32, i32, i32, i32)> {
    let left = area.0.floor().max(0.0) as i32;
    let top = area.1.floor().max(0.0) as i32;
    let right = area.2.ceil().min(image.width as f32) as i32;
    let bottom = area.3.ceil().min(image.height as f32) as i32;
    (right > left && bottom > top).then_some((left, top, right, bottom))
}

/// 导出用马赛克：按块大小取区域平均色再填回，越界自动裁剪。
fn mosaic(image: &mut Image, area: (f32, f32, f32, f32), block: f32) {
    let Some((left, top, right, bottom)) = rect_bounds(image, area) else { return };
    let block = block.max(4.0) as i32;
    let mut y = top;
    while y < bottom {
        let mut x = left;
        while x < right {
            let x_end = (x + block).min(right);
            let y_end = (y + block).min(bottom);
            let mut sum = [0u32; 4];
            let mut count = 0u32;
            for py in y..y_end {
                for px in x..x_end {
                    let index = ((py as u32 * image.width + px as u32) * 4) as usize;
                    for channel in 0..4 {
                        sum[channel] += image.rgba[index + channel] as u32;
                    }
                    count += 1;
                }
            }
            if count > 0 {
                let color = [
                    (sum[0] / count) as u8,
                    (sum[1] / count) as u8,
                    (sum[2] / count) as u8,
                ];
                for py in y..y_end {
                    for px in x..x_end {
                        let index = ((py as u32 * image.width + px as u32) * 4) as usize;
                        image.rgba[index] = color[0];
                        image.rgba[index + 1] = color[1];
                        image.rgba[index + 2] = color[2];
                        image.rgba[index + 3] = 255;
                    }
                }
            }
            x += block;
        }
        y += block;
    }
}

/// 导出用高亮：半透明颜色叠加到矩形区域。
fn highlight(image: &mut Image, area: (f32, f32, f32, f32), color: [u8; 3]) {
    let Some((left, top, right, bottom)) = rect_bounds(image, area) else { return };
    for y in top..bottom {
        for x in left..right {
            blend(image, x, y, color, 0.36);
        }
    }
}

/// 把一张带 alpha 的小图叠加到目标图左上角坐标处。
fn blend_bitmap(target: &mut Image, at: (f32, f32), glyphs: &Image) {
    let left = at.0.round() as i32;
    let top = at.1.round() as i32;
    for y in 0..glyphs.height as i32 {
        for x in 0..glyphs.width as i32 {
            let index = ((y as u32 * glyphs.width + x as u32) * 4) as usize;
            let alpha = glyphs.rgba[index + 3] as f32 / 255.0;
            if alpha <= 0.0 {
                continue;
            }
            let color = [glyphs.rgba[index], glyphs.rgba[index + 1], glyphs.rgba[index + 2]];
            blend(target, left + x, top + y, color, alpha);
        }
    }
}

/// 用 GDI 把文字渲染成「选定颜色 + 抗锯齿 alpha」的小图；无 Windows 时返回 None。
#[cfg(windows)]
fn text_bitmap(text: &str, font_px: f32, color: [u8; 3]) -> Option<Image> {
    use windows::core::PCWSTR;
    use windows::Win32::Graphics::Gdi::{
        CreateCompatibleDC, CreateDIBSection, CreateFontW, DeleteDC, DeleteObject, GetTextExtentPoint32W,
        SelectObject, SetBkMode, SetTextColor, TextOutW, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
        DIB_RGB_COLORS, TRANSPARENT,
    };
    use windows::Win32::Foundation::{COLORREF, SIZE};
    let text: Vec<u16> = text.encode_utf16().collect();
    if text.is_empty() {
        return None;
    }
    unsafe {
        let dc = CreateCompatibleDC(None);
        let font = CreateFontW(
            -(font_px.max(8.0) as i32),
            0,
            0,
            0,
            600,
            0,
            0,
            0,
            1,
            0,
            0,
            4,
            0,
            PCWSTR("Microsoft YaHei UI\0".encode_utf16().collect::<Vec<_>>().as_ptr()),
        );
        let old_font = SelectObject(dc, font);
        let mut size = SIZE::default();
        let _ = GetTextExtentPoint32W(dc, &text, &mut size);
        let width = (size.cx.max(1) + 6) as u32;
        let height = (size.cy.max(1) + 6) as u32;
        let mut info = BITMAPINFO::default();
        info.bmiHeader = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width as i32,
            biHeight: -(height as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        };
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let Ok(bitmap) = CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, None, 0) else {
            let _ = SelectObject(dc, old_font);
            let _ = DeleteObject(font);
            let _ = DeleteDC(dc);
            return None;
        };
        let old_bitmap = SelectObject(dc, bitmap);
        SetBkMode(dc, TRANSPARENT);
        SetTextColor(dc, COLORREF(0x00FF_FFFF));
        let _ = TextOutW(dc, 3, 3, &text);
        let mut rgba = vec![0u8; (width * height * 4) as usize];
        if !bits.is_null() {
            let source = std::slice::from_raw_parts(bits as *const u8, rgba.len());
            for index in (0..rgba.len()).step_by(4) {
                let blue = source[index];
                let green = source[index + 1];
                let red = source[index + 2];
                let alpha = red.max(green).max(blue);
                rgba[index] = color[0];
                rgba[index + 1] = color[1];
                rgba[index + 2] = color[2];
                rgba[index + 3] = alpha;
            }
        }
        SelectObject(dc, old_bitmap);
        let _ = DeleteObject(bitmap);
        SelectObject(dc, old_font);
        let _ = DeleteObject(font);
        let _ = DeleteDC(dc);
        Some(Image { width, height, rgba })
    }
}

#[cfg(not(windows))]
fn text_bitmap(_text: &str, _font_px: f32, _color: [u8; 3]) -> Option<Image> {
    None
}

fn number_radius(base_width: u32) -> f32 {
    (base_width as f32 * 0.014).clamp(14.0, 30.0)
}

fn next_number(shapes: &[Shape]) -> usize {
    shapes.iter().filter(|shape| shape.tool == Tool::Number).count() + 1
}

fn blend_bitmap_centered(image: &mut Image, center: (f32, f32), bitmap: &Image) {
    let origin = (
        center.0 - bitmap.width as f32 / 2.0,
        center.1 - bitmap.height as f32 / 2.0,
    );
    blend_bitmap(image, origin, bitmap);
}

fn ends(shape: &Shape) -> Option<((f32, f32), (f32, f32))> {
    let first = *shape.points.first()?;
    let last = *shape.points.last()?;
    Some((first, last))
}

fn blend(image: &mut Image, x: i32, y: i32, color: [u8; 3], alpha: f32) {
    if x < 0 || y < 0 || x >= image.width as i32 || y >= image.height as i32 {
        return;
    }
    let index = (y as usize * image.width as usize + x as usize) * 4;
    let a = alpha.clamp(0.0, 1.0);
    for channel in 0..3 {
        let old = image.rgba[index + channel] as f32;
        image.rgba[index + channel] = (old * (1.0 - a) + color[channel] as f32 * a).round().clamp(0.0, 255.0) as u8;
    }
    image.rgba[index + 3] = 255;
}

fn disk(image: &mut Image, center: (f32, f32), radius: f32, color: [u8; 3]) {
    let radius = radius.max(0.6);
    let x0 = (center.0 - radius - 1.0).floor() as i32;
    let x1 = (center.0 + radius + 1.0).ceil() as i32;
    let y0 = (center.1 - radius - 1.0).floor() as i32;
    let y1 = (center.1 + radius + 1.0).ceil() as i32;
    for y in y0..=y1 {
        for x in x0..=x1 {
            let distance = ((x as f32 + 0.5 - center.0).powi(2) + (y as f32 + 0.5 - center.1).powi(2)).sqrt();
            let coverage = (radius + 0.5 - distance).clamp(0.0, 1.0);
            if coverage > 0.0 {
                blend(image, x, y, color, coverage);
            }
        }
    }
}

fn line(image: &mut Image, from: (f32, f32), to: (f32, f32), width: f32, color: [u8; 3]) {
    let radius = (width / 2.0).max(0.5);
    let x0 = (from.0.min(to.0) - radius - 1.0).floor() as i32;
    let x1 = (from.0.max(to.0) + radius + 1.0).ceil() as i32;
    let y0 = (from.1.min(to.1) - radius - 1.0).floor() as i32;
    let y1 = (from.1.max(to.1) + radius + 1.0).ceil() as i32;
    let dx = to.0 - from.0;
    let dy = to.1 - from.1;
    let length_squared = dx * dx + dy * dy;
    for y in y0..=y1 {
        for x in x0..=x1 {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            let t = if length_squared <= f32::EPSILON {
                0.0
            } else {
                (((px - from.0) * dx + (py - from.1) * dy) / length_squared).clamp(0.0, 1.0)
            };
            let nearest_x = from.0 + dx * t;
            let nearest_y = from.1 + dy * t;
            let distance = ((px - nearest_x).powi(2) + (py - nearest_y).powi(2)).sqrt();
            let coverage = (radius + 0.5 - distance).clamp(0.0, 1.0);
            if coverage > 0.0 {
                blend(image, x, y, color, coverage);
            }
        }
    }
}

/// 拖动时：画笔追加轨迹，矩形/箭头更新终点。
fn extend(shape: &mut Shape, point: (f32, f32)) {
    match shape.tool {
        Tool::Pen => {
            if shape.points.last().is_some_and(|last| {
                let dx = last.0 - point.0;
                let dy = last.1 - point.1;
                dx * dx + dy * dy >= 0.6
            }) {
                shape.points.push(point);
            }
        }
        Tool::Rect | Tool::Arrow | Tool::Mosaic | Tool::Highlight => {
            if shape.points.len() < 2 {
                shape.points.push(point);
            } else {
                shape.points[1] = point;
            }
        }
        Tool::Text | Tool::Number => {}
    }
}

pub enum Action {
    None,
    Close,
    Copy(Image),
    Save(Image),
    Pin(Image),
}

pub struct Editor {
    pub open: bool,
    base: Option<Image>,
    texture: Option<egui::TextureHandle>,
    shapes: Vec<Shape>,
    draft: Option<Shape>,
    tool: Tool,
    color: usize,
    width: usize,
    position: Option<egui::Pos2>,
    shown: bool,
    had_focus: bool,
    scale: f32,
    status: String,
    /// 文字工具待放置的位置、内容和是否需要聚焦输入框。
    text_pending: Option<(f32, f32, String)>,
    text_focus: bool,
}

impl Default for Editor {
    fn default() -> Self {
        Self {
            open: false,
            base: None,
            texture: None,
            shapes: Vec::new(),
            draft: None,
            tool: Tool::Pen,
            color: 0,
            width: 1,
            position: None,
            shown: false,
            had_focus: false,
            scale: 1.0,
            status: String::new(),
            text_pending: None,
            text_focus: false,
        }
    }
}

impl Editor {
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// 打开编辑器并上传截图纹理；窗口按图片大小自适应，超大截图按比例缩小显示。
    pub fn open(&mut self, ctx: &egui::Context, image: Image) {
        let (max_w, max_h) = MAX_CANVAS;
        let available_w = (max_w - 24.0).max(200.0);
        let available_h = (max_h - 120.0).max(120.0);
        self.scale = (available_w / image.width as f32).min(available_h / image.height as f32).min(1.0);
        let _ = ctx;
        self.texture = None;
        self.base = Some(image);
        self.shapes.clear();
        self.draft = None;
        self.text_pending = None;
        self.text_focus = false;
        self.status.clear();
        self.open = true;
        self.shown = false;
        self.had_focus = false;
        self.position = None;
    }

    pub fn close(&mut self) {
        self.open = false;
        self.shown = false;
        self.texture = None;
        self.base = None;
        self.shapes.clear();
        self.draft = None;
        self.text_pending = None;
        self.text_focus = false;
    }

    fn window_size(&self) -> Vec2 {
        let (w, h) = self
            .base
            .as_ref()
            .map(|base| (base.width as f32 * self.scale, base.height as f32 * self.scale))
            .unwrap_or((300.0, 200.0));
        Vec2::new((w + 24.0).max(MIN_WINDOW_W), h + 120.0)
    }

    pub fn show(&mut self, ctx: &egui::Context, dark: bool) -> Action {
        if !self.open {
            return Action::None;
        }
        let viewport_id = egui::ViewportId::from_hash_of("daozai-annotate");
        let size = self.window_size();
        let ppp = ctx.pixels_per_point();
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
        let mut action = Action::None;
        let builder = egui::ViewportBuilder::default()
            .with_title("截图标注 · 岛仔")
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
        ctx.show_viewport_immediate(viewport_id, builder, |ctx, _| {
            if !self.shown {
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                self.shown = true;
            }
            if ctx.input(|input| input.viewport().close_requested()) {
                action = Action::Close;
            }
            if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
                if self.text_pending.is_some() {
                    self.text_pending = None;
                } else {
                    action = Action::Close;
                }
            }
            if ctx.input(|input| input.key_pressed(egui::Key::Z) && input.modifiers.command) {
                self.shapes.pop();
            }
            let focused = ctx.input(|input| input.viewport().focused.unwrap_or(false));
            if focused {
                self.had_focus = true;
            } else if self.had_focus {
                self.had_focus = false;
                action = Action::Close;
            }
            let panel = self.panel_ui(ctx, dark);
            if matches!(action, Action::None) {
                action = panel;
            }
        });
        action
    }

    /// 编辑器面板；拆出来便于无窗口渲染与交互测试。
    pub fn panel_ui(&mut self, ctx: &egui::Context, dark: bool) -> Action {
        if self.texture.is_none() {
            if let Some(base) = self.base.as_ref() {
                let display = display_image(base);
                let pixels = egui::ColorImage::from_rgba_unmultiplied(
                    [display.width as usize, display.height as usize],
                    &display.rgba,
                );
                self.texture = Some(ctx.load_texture("annotate-base", pixels, egui::TextureOptions::LINEAR));
            }
        }
        let palette = crate::panel::apply_style(ctx, dark);
        let mut action = Action::None;
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(Color32::TRANSPARENT).inner_margin(10.0))
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(palette.surface)
                    .stroke(Stroke::new(1.0_f32, palette.line))
                    .corner_radius(16.0)
                    .inner_margin(12.0)
                    .shadow(crate::panel::shadow(dark))
                    .show(ui, |ui| {
                        self.toolbar(ui, &palette);
                        ui.add_space(6.0);
                        let canvas = self.canvas(ui);
                        if let Some(canvas) = canvas {
                            self.draw_canvas(ui, canvas, &palette);
                            if self.text_pending.is_some() {
                                let ctx = ui.ctx().clone();
                                self.text_input(&ctx, ui, canvas, &palette);
                            }
                        }
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(if self.status.is_empty() {
                                    "拖动绘制 · Ctrl+Z 撤销 · Esc 取消"
                                } else {
                                    self.status.as_str()
                                })
                                .size(10.5)
                                .color(palette.faint),
                            );
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.button(egui::RichText::new("取消").size(11.0)).clicked() {
                                    action = Action::Close;
                                }
                                if ui.button(egui::RichText::new("保存").size(11.0)).clicked() {
                                    if let Some(base) = self.base.clone() {
                                        action = Action::Save(rasterize(&base, &self.shapes));
                                    }
                                }
                                if ui.button(egui::RichText::new("复制").size(11.0).strong()).clicked() {
                                    if let Some(base) = self.base.clone() {
                                        action = Action::Copy(rasterize(&base, &self.shapes));
                                    }
                                }
                                if ui.button(egui::RichText::new("贴图").size(11.0)).clicked() {
                                    if let Some(base) = self.base.clone() {
                                        action = Action::Pin(rasterize(&base, &self.shapes));
                                    }
                                }
                                if ui
                                    .add_enabled(!self.shapes.is_empty(), egui::Button::new(egui::RichText::new("撤销").size(11.0)))
                                    .clicked()
                                {
                                    self.shapes.pop();
                                }
                                ui.label(
                                    egui::RichText::new(format!("{} × {}", self.base.as_ref().map_or(0, |b| b.width), self.base.as_ref().map_or(0, |b| b.height)))
                                        .size(10.0)
                                        .color(palette.faint),
                                );
                            });
                        });
                    });
            });
        action
    }

    fn toolbar(&mut self, ui: &mut egui::Ui, palette: &crate::panel::Palette) {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("截图标注").size(15.0).strong().color(palette.text));
            ui.add_space(6.0);
            for tool in [Tool::Pen, Tool::Rect, Tool::Arrow, Tool::Text, Tool::Mosaic, Tool::Highlight, Tool::Number] {
                let selected = self.tool == tool;
                let response = ui.add(
                    egui::Button::new(
                        egui::RichText::new(tool.label())
                            .size(11.0)
                            .color(if selected { palette.text } else { palette.muted }),
                    )
                    .fill(if selected { egui::Color32::from_rgba_unmultiplied(10, 132, 255, 46) } else { Color32::TRANSPARENT })
                    .stroke(if selected { Stroke::new(1.0_f32, palette.accent) } else { Stroke::NONE })
                    .corner_radius(7.0),
                );
                if response.clicked() {
                    self.tool = tool;
                }
            }
            ui.add_space(6.0);
            for (index, color) in COLORS.iter().enumerate() {
                let (rect, response) = ui.allocate_exact_size(Vec2::new(18.0, 18.0), egui::Sense::click());
                ui.painter().circle_filled(rect.center(), 8.0, Color32::from_rgb(color[0], color[1], color[2]));
                ui.painter().circle_stroke(rect.center(), 8.0, Stroke::new(1.0_f32, palette.line));
                if self.color == index {
                    ui.painter().circle_stroke(rect.center(), 9.8, Stroke::new(1.6_f32, palette.text));
                }
                if response.clicked() {
                    self.color = index;
                }
            }
            ui.add_space(6.0);
            let width_labels = if self.tool == Tool::Text { ["小", "中", "大"] } else { ["细", "中", "粗"] };
            for (index, label) in width_labels.iter().enumerate() {
                let selected = self.width == index;
                let response = ui.add(
                    egui::Button::new(
                        egui::RichText::new(*label)
                            .size(10.5)
                            .color(if selected { palette.text } else { palette.faint }),
                    )
                    .fill(if selected { palette.raised } else { Color32::TRANSPARENT })
                    .corner_radius(7.0),
                );
                if response.clicked() {
                    self.width = index;
                }
            }

        });
    }

    fn canvas(&self, ui: &mut egui::Ui) -> Option<Rect> {
        let (width, height) = self.base.as_ref().map(|base| (base.width as f32 * self.scale, base.height as f32 * self.scale))?;
        let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width().max(width), height), egui::Sense::hover());
        Some(Rect::from_min_size(
            egui::pos2(rect.center().x - width / 2.0, rect.top()),
            Vec2::new(width, height),
        ))
    }

    fn draw_canvas(&mut self, ui: &mut egui::Ui, canvas: Rect, palette: &crate::panel::Palette) {
        let response = ui.interact(canvas, egui::Id::new("annotate-canvas"), egui::Sense::drag());
        ui.painter().rect_filled(canvas.expand(2.0), 4.0, palette.line_soft);
        if let (Some(texture), Some(base)) = (self.texture.clone(), self.base.as_ref()) {
            let _ = base;
            ui.painter().add(
                egui::epaint::RectShape::filled(canvas, 0.0, Color32::WHITE)
                    .with_texture(texture.id(), Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0))),
            );
        }
        for shape in self.shapes.iter().chain(self.draft.iter()) {
            self.paint_shape(ui, canvas, shape);
        }
        let Some(base) = self.base.as_ref() else { return };
        let to_image = |point: Pos2| {
            (
                ((point.x - canvas.left()) / self.scale).clamp(0.0, base.width.saturating_sub(1) as f32),
                ((point.y - canvas.top()) / self.scale).clamp(0.0, base.height.saturating_sub(1) as f32),
            )
        };
        if self.tool == Tool::Number {
            if response.clicked() {
                if let Some(pointer) = response.interact_pointer_pos() {
                    let position = to_image(pointer);
                    let number = next_number(&self.shapes);
                    self.shapes.push(Shape {
                        tool: Tool::Number,
                        color: COLORS[self.color],
                        width: WIDTHS[self.width],
                        points: vec![position],
                        text: Some(number.to_string()),
                    });
                }
            }
            return;
        }
        if self.tool == Tool::Text {
            if response.clicked() {
                if let Some(pointer) = response.interact_pointer_pos() {
                    let position = to_image(pointer);
                    self.text_pending = Some((position.0, position.1, String::new()));
                    self.text_focus = true;
                }
            }
            return;
        }
        if response.drag_started() {
            if let Some(pointer) = response.interact_pointer_pos() {
                self.draft = Some(Shape::new(
                    self.tool,
                    COLORS[self.color],
                    WIDTHS[self.width],
                    to_image(pointer),
                ));
            }
        }
        if response.dragged() {
            if let (Some(pointer), Some(draft)) = (response.interact_pointer_pos(), self.draft.as_mut()) {
                extend(draft, to_image(pointer));
            }
        }
        if response.drag_stopped() {
            if let Some(shape) = self.draft.take() {
                if shape.points.len() >= 2 || shape.tool == Tool::Pen {
                    self.shapes.push(shape);
                }
            }
        }
    }

    fn text_input(&mut self, ctx: &egui::Context, _ui: &mut egui::Ui, canvas: Rect, palette: &crate::panel::Palette) {
        let Some((x, y, text)) = self.text_pending.as_mut() else { return };
        let position = egui::pos2(canvas.left() + *x * self.scale, canvas.top() + *y * self.scale);
        let mut focus = self.text_focus;
        egui::Area::new(egui::Id::new("annotate-text-input"))
            .order(egui::Order::Foreground)
            .fixed_pos(position)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(palette.raised)
                    .stroke(Stroke::new(1.0_f32, palette.accent))
                    .corner_radius(8.0)
                    .inner_margin(egui::Margin::symmetric(8, 5))
                    .shadow(crate::panel::shadow(true))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let response = ui.add(
                                egui::TextEdit::singleline(text)
                                    .hint_text("输入文字，Enter 放置")
                                    .desired_width(190.0)
                                    .font(egui::FontId::proportional(13.0)),
                            );
                            if focus {
                                response.request_focus();
                                focus = false;
                            }
                            ui.label(egui::RichText::new("Enter").size(9.0).color(palette.faint));
                        });
                    });
            });
        self.text_focus = focus;
        let (enter, escape) = ctx.input(|input| {
            (
                input.key_pressed(egui::Key::Enter),
                input.key_pressed(egui::Key::Escape),
            )
        });
        if escape {
            self.text_pending = None;
            return;
        }
        if enter {
            let Some((x, y, text)) = self.text_pending.take() else { return };
            let content = text.trim().to_string();
            if !content.is_empty() {
                self.shapes.push(Shape {
                    tool: Tool::Text,
                    color: COLORS[self.color],
                    width: TEXT_SIZES[self.width],
                    points: vec![(x, y)],
                    text: Some(content),
                });
            }
        }
    }

    fn paint_shape(&self, ui: &egui::Ui, canvas: Rect, shape: &Shape) {
        let map = |point: (f32, f32)| {
            egui::pos2(canvas.left() + point.0 * self.scale, canvas.top() + point.1 * self.scale)
        };
        let color = Color32::from_rgb(shape.color[0], shape.color[1], shape.color[2]);
        let stroke = Stroke::new((shape.width * self.scale).max(1.0), color);
        match shape.tool {
            Tool::Pen => {
                let points: Vec<Pos2> = shape.points.iter().map(|point| map(*point)).collect();
                if points.len() >= 2 {
                    ui.painter().add(egui::Shape::line(points, stroke));
                } else if let Some(point) = points.first() {
                    ui.painter().circle_filled(*point, stroke.width / 2.0, color);
                }
            }
            Tool::Rect => {
                if let (Some(start), Some(end)) = (shape.points.first(), shape.points.last()) {
                    ui.painter().rect_stroke(
                        Rect::from_two_pos(map(*start), map(*end)),
                        0.0,
                        stroke,
                        egui::StrokeKind::Inside,
                    );
                }
            }
            Tool::Arrow => {
                if let (Some(start), Some(end)) = (shape.points.first(), shape.points.last()) {
                    let (from, to) = (map(*start), map(*end));
                    ui.painter().line_segment([from, to], stroke);
                    let angle = (to.y - from.y).atan2(to.x - from.x);
                    let length = (shape.width * 4.0 * self.scale).max(10.0);
                    for side in [0.55f32, -0.55] {
                        let point = egui::pos2(to.x - length * (angle + side).cos(), to.y - length * (angle + side).sin());
                        ui.painter().line_segment([to, point], stroke);
                    }
                }
            }
            Tool::Text => {
                if let (Some(start), Some(text)) = (shape.points.first(), shape.text.as_deref()) {
                    ui.painter().text(
                        map(*start),
                        egui::Align2::LEFT_TOP,
                        text,
                        egui::FontId::proportional((shape.width * self.scale).max(8.0)),
                        color,
                    );
                }
            }
            Tool::Highlight => {
                if let (Some(start), Some(end)) = (shape.points.first(), shape.points.last()) {
                    let rect = Rect::from_two_pos(map(*start), map(*end));
                    ui.painter().rect_filled(
                        rect,
                        0.0,
                        Color32::from_rgba_unmultiplied(shape.color[0], shape.color[1], shape.color[2], 92),
                    );
                }
            }
            Tool::Number => {
                if let Some(point) = shape.points.first() {
                    let center = map(*point);
                    let radius = number_radius(self.base.as_ref().map(|base| base.width).unwrap_or(1))
                        * self.scale;
                    ui.painter().circle_filled(center, radius, color);
                    ui.painter().circle_stroke(
                        center,
                        radius - (1.0 * self.scale).max(0.8),
                        Stroke::new((1.8 * self.scale).max(1.0), Color32::WHITE),
                    );
                    if let Some(number) = shape.text.as_deref() {
                        ui.painter().text(
                            center,
                            egui::Align2::CENTER_CENTER,
                            number,
                            egui::FontId::proportional((radius * 1.15).max(9.0)),
                            Color32::WHITE,
                        );
                    }
                }
            }
            Tool::Mosaic => {
                if let (Some(start), Some(end)) = (shape.points.first(), shape.points.last()) {
                    let rect = Rect::from_two_pos(map(*start), map(*end));
                    let Some(base) = self.base.as_ref() else { return };
                    let area = rect.width() * rect.height();
                    let block = (shape.width * self.scale).max(3.0).max((area / 900.0).sqrt());
                    let mut y = rect.top();
                    while y < rect.bottom() {
                        let mut x = rect.left();
                        while x < rect.right() {
                            let cell = Rect::from_min_max(
                                egui::pos2(x, y),
                                egui::pos2((x + block).min(rect.right()), (y + block).min(rect.bottom())),
                            );
                            let center = cell.center();
                            let image_x = (((center.x - canvas.left()) / self.scale) as i32)
                                .clamp(0, base.width as i32 - 1) as u32;
                            let image_y = (((center.y - canvas.top()) / self.scale) as i32)
                                .clamp(0, base.height as i32 - 1) as u32;
                            let index = ((image_y * base.width + image_x) * 4) as usize;
                            ui.painter().rect_filled(
                                cell,
                                0.0,
                                Color32::from_rgb(
                                    base.rgba[index],
                                    base.rgba[index + 1],
                                    base.rgba[index + 2],
                                ),
                            );
                            x += block;
                        }
                        y += block;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(width: u32, height: u32) -> Image {
        Image { width, height, rgba: vec![20; (width * height * 4) as usize] }
    }

    fn pixel(image: &Image, x: u32, y: u32) -> [u8; 3] {
        let index = ((y * image.width + x) * 4) as usize;
        [image.rgba[index], image.rgba[index + 1], image.rgba[index + 2]]
    }

    #[test]
    fn pen_and_rect_change_pixels_and_keep_size() {
        let base = image(60, 40);
        let shapes = vec![
            Shape { tool: Tool::Pen, color: [255, 0, 0], width: 4.0, points: vec![(5.0, 5.0), (40.0, 20.0)], text: None },
            Shape { tool: Tool::Rect, color: [0, 255, 0], width: 3.0, points: vec![(10.0, 25.0), (50.0, 35.0)], text: None },
        ];
        let out = rasterize(&base, &shapes);
        assert_eq!((out.width, out.height), (60, 40));
        assert_eq!(pixel(&out, 5, 5), [255, 0, 0]);
        assert_eq!(pixel(&out, 10, 25), [0, 255, 0]);
        assert_eq!(pixel(&out, 30, 3), [20, 20, 20]);
    }

    #[test]
    fn arrow_draws_head_and_clips_at_edges() {
        let base = image(40, 30);
        let shapes = vec![Shape {
            tool: Tool::Arrow,
            color: [10, 132, 255],
            width: 2.0,
            points: vec![(2.0, 2.0), (80.0, 60.0)],
            text: None,
        }];
        let out = rasterize(&base, &shapes);
        // 起点一定被画到；出界点被裁掉而不是 panic。
        assert_eq!(pixel(&out, 2, 2), [10, 132, 255]);
        assert_eq!(out.rgba.len(), base.rgba.len());
    }

    #[test]
    fn draft_extension_and_undo_keep_shapes_vec_consistent() {
        let mut pen = Shape::new(Tool::Pen, [1, 2, 3], 6.0, (0.0, 0.0));
        extend(&mut pen, (0.1, 0.1));
        assert_eq!(pen.points.len(), 1, "过近的画笔点应被忽略");
        extend(&mut pen, (10.0, 10.0));
        assert_eq!(pen.points.len(), 2);
        let mut rect = Shape::new(Tool::Rect, [1, 2, 3], 3.0, (1.0, 1.0));
        extend(&mut rect, (5.0, 5.0));
        extend(&mut rect, (9.0, 9.0));
        assert_eq!(rect.points, vec![(1.0, 1.0), (9.0, 9.0)]);
    }

    #[cfg(windows)]
    #[test]
    fn gdi_text_bitmaps_have_antialiased_alpha() {
        let glyphs = text_bitmap("测试A", 30.0, [255, 69, 58]).expect("GDI 文字渲染应成功");
        assert!(glyphs.width > 20 && glyphs.height > 10);
        let opaque = glyphs.rgba.chunks_exact(4).filter(|pixel| pixel[3] > 180).count();
        assert!(opaque > 8, "文字应产生不透明像素，实际 {opaque}");
        assert!(glyphs.rgba.chunks_exact(4).any(|pixel| pixel[3] > 0 && pixel[0] == 255 && pixel[1] == 69));
    }

    #[cfg(windows)]
    #[test]
    fn text_shapes_export_into_the_image() {
        let base = image(240, 100);
        let shapes = vec![Shape {
            tool: Tool::Text,
            color: [255, 0, 0],
            width: 30.0,
            points: vec![(12.0, 16.0)],
            text: Some("Hello 岛仔".into()),
        }];
        let out = rasterize(&base, &shapes);
        let red = out
            .rgba
            .chunks_exact(4)
            .any(|pixel| pixel[0] > 200 && pixel[1] < 80 && pixel[2] < 80);
        assert!(red, "文字应被合成进导出的截图");
    }

    #[test]
    fn mosaic_averages_blocks_and_clips_to_bounds() {
        let mut base = image(32, 32);
        for y in 0..32u32 {
            for x in 0..32u32 {
                let index = ((y * 32 + x) * 4) as usize;
                if x < 16 {
                    base.rgba[index..index + 4].copy_from_slice(&[200, 0, 0, 255]);
                } else {
                    base.rgba[index..index + 4].copy_from_slice(&[0, 0, 200, 255]);
                }
            }
        }
        let shape = Shape {
            tool: Tool::Mosaic,
            color: [255, 255, 255],
            width: 16.0,
            points: vec![(-10.0, -10.0), (40.0, 40.0)],
            text: None,
        };
        let out = rasterize(&base, &[shape]);
        assert_eq!(pixel(&out, 4, 4), [200, 0, 0]);
        assert_eq!(pixel(&out, 20, 20), [0, 0, 200]);
        assert_eq!(out.rgba.len(), base.rgba.len());
    }

    #[test]
    fn highlight_blends_color_only_inside_the_rect() {
        let base = image(40, 30);
        let shape = Shape {
            tool: Tool::Highlight,
            color: [255, 214, 10],
            width: WIDTHS[1],
            points: vec![(10.0, 8.0), (24.0, 20.0)],
            text: None,
        };
        let out = rasterize(&base, &[shape]);
        let inside = pixel(&out, 15, 12);
        assert!(inside[0] > 80 && inside[1] > 60, "高亮区域内应混入黄色：{inside:?}");
        assert_eq!(pixel(&out, 2, 2), [20, 20, 20]);
    }

    #[test]
    fn oversized_textures_are_downsampled_for_display_only() {
        let base = image(3840, 2160);
        let display = display_image(&base);
        assert!(display.width.max(display.height) <= 2048);
        assert_eq!((display.width, display.height), (1920, 1080));
        assert_eq!(display.rgba.len(), (1920 * 1080 * 4) as usize);
        // 原图保持不变，导出仍按原始尺寸合成。
        assert_eq!((base.width, base.height), (3840, 2160));
        let small = image(800, 600);
        assert_eq!(display_image(&small), small);
    }

    fn gradient(width: u32, height: u32) -> Image {
        let mut rgba = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            for x in 0..width {
                let r = (40 + x * 120 / width.max(1)) as u8;
                let g = (60 + y * 120 / height.max(1)) as u8;
                rgba.extend_from_slice(&[r, g, 130, 255]);
            }
        }
        Image { width, height, rgba }
    }

    #[test]
    fn numbered_callouts_increment_and_rasterize() {
        assert_eq!(next_number(&[]), 1);
        let base = gradient(300, 200);
        let shapes = vec![
            Shape {
                tool: Tool::Number,
                color: [255, 69, 58],
                width: WIDTHS[1],
                points: vec![(80.0, 80.0)],
                text: Some("1".into()),
            },
            Shape {
                tool: Tool::Number,
                color: [10, 132, 255],
                width: WIDTHS[1],
                points: vec![(220.0, 140.0)],
                text: Some("2".into()),
            },
        ];
        assert_eq!(next_number(&shapes), 3);
        let out = rasterize(&base, &shapes);
        let radius = number_radius(300) as i32;
        // 白环
        let ring = pixel(&out, 80, (80 - radius + 1) as u32);
        assert!(ring.iter().all(|channel| *channel > 180), "白环应清晰：{ring:?}");
        // 圆内填色（避开中间数字）
        let fill = pixel(&out, 74, 80);
        assert!(fill[0] > 180 && fill[1] < 150, "应填充选中颜色：{fill:?}");
        // 远离标注的区域保持原图
        assert_eq!(pixel(&out, 20, 20), pixel(&base, 20, 20));
    }

    #[test]
    fn panel_renders_toolbar_and_preview() {
        let ctx = egui::Context::default();
        crate::theme::load_fonts(&ctx);
        let mut editor = Editor::default();
        editor.open(&ctx, gradient(480, 300));
        editor.tool = Tool::Arrow;
        editor.shapes.push(Shape {
            tool: Tool::Arrow,
            color: [255, 69, 58],
            width: 6.0,
            points: vec![(80.0, 90.0), (320.0, 200.0)],
            text: None,
        });
        editor.shapes.push(Shape {
            tool: Tool::Text,
            color: [255, 255, 255],
            width: TEXT_SIZES[1],
            points: vec![(64.0, 214.0)],
            text: Some("岛仔截图".into()),
        });
        editor.shapes.push(Shape {
            tool: Tool::Highlight,
            color: [255, 214, 10],
            width: WIDTHS[1],
            points: vec![(250.0, 40.0), (430.0, 90.0)],
            text: None,
        });
        editor.shapes.push(Shape {
            tool: Tool::Mosaic,
            color: [255, 255, 255],
            width: MOSAIC_BLOCKS[1],
            points: vec![(56.0, 196.0), (260.0, 300.0)],
            text: None,
        });
        editor.shapes.push(Shape {
            tool: Tool::Rect,
            color: [10, 132, 255],
            width: 3.0,
            points: vec![(60.0, 40.0), (240.0, 150.0)],
            text: None,
        });
        for (index, position) in [(0, (360.0, 70.0)), (1, (360.0, 150.0))] {
            editor.shapes.push(Shape {
                tool: Tool::Number,
                color: [10, 132, 255],
                width: WIDTHS[1],
                points: vec![position],
                text: Some((index + 1).to_string()),
            });
        }
        let size = editor.window_size();
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
            let _ = editor.panel_ui(ctx, true);
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
        for expected in ["截图标注", "画笔", "矩形", "箭头", "文字", "马赛克", "高亮", "序号", "撤销", "复制", "贴图", "保存"] {
            assert!(texts.iter().any(|text| text.galley.text().contains(expected)), "编辑器缺少 {expected}");
        }
        if let Some(dest) = std::env::var_os("DAOZAI_ANNOTATE_PREVIEW_DIR").filter(|value| !value.is_empty()) {
            let dest = std::path::PathBuf::from(dest);
            std::fs::create_dir_all(&dest).unwrap();
            let mut raster = crate::preview_raster::Raster::new(
                (size.x * 2.0) as usize,
                (size.y * 2.0) as usize,
            );
            raster.update_textures(&output.textures_delta);
            let primitives = ctx.tessellate(output.shapes.clone(), output.pixels_per_point);
            raster.draw(&primitives, output.pixels_per_point);
            let mut rgba = Vec::new();
            raster.to_rgba8(&mut rgba);
            image::save_buffer(
                dest.join("annotate-editor.png"),
                &rgba,
                (size.x * 2.0) as u32,
                (size.y * 2.0) as u32,
                image::ColorType::Rgba8,
            )
            .unwrap();
        }
    }

    #[test]
    fn editor_scale_fits_large_screenshots() {
        let mut editor = Editor::default();
        let ctx = egui::Context::default();
        crate::theme::load_fonts(&ctx);
        editor.open(&ctx, image(3840, 2160));
        let size = editor.window_size();
        assert!(size.x <= 900.0 && size.y <= 720.0, "窗口不应超出最大画布：{size:?}");
        assert!(editor.scale < 0.5);
        assert!(editor.is_open());
        editor.close();
        assert!(!editor.is_open());
    }
}
