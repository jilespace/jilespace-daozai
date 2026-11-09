//! 框选取字层：全屏半透明遮罩 + 拖拽选框，松开后把选区交给 OCR。
//!
//! 打开之前外壳已经抓好了整屏位图，这一层只负责「选哪里」，所以选中区域
//! 不会把遮罩拍进去，也没有先隐藏窗口再截屏的时序问题。

use crate::region_capture::MonitorRect;
use crate::theme::{self, W};
use eframe::egui::{self, Color32, Pos2, Rect, Stroke, Vec2};
use std::time::Instant;

const FADE_SECS: f64 = 0.10;
const MIN_SELECTION: f32 = 8.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    None,
    Cancel,
    /// 相对显示器左上角的物理像素选区。
    Capture { x: i32, y: i32, width: u32, height: u32 },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Purpose {
    #[default]
    Ocr,
    Screenshot,
    Scroll,
}

impl Purpose {
    fn hint(self) -> &'static str {
        match self {
            Self::Ocr => "拖动框选文字区域，松开后识别",
            Self::Screenshot => "拖动框选截图区域，松开后复制并保存",
            Self::Scroll => "拖动框选滚动截图区域，松开后滚动页面自动拼接",
        }
    }
}

pub struct Overlay {
    pub open: bool,
    purposes: Purpose,
    monitor: MonitorRect,
    start: Option<Pos2>,
    current: Option<Pos2>,
    shown: bool,
    shown_at: Option<f64>,
    had_focus: bool,
    focus_lost_at: Option<Instant>,
    fade: f32,
}

impl Default for Overlay {
    fn default() -> Self {
        Self {
            open: false,
            purposes: Purpose::Ocr,
            monitor: MonitorRect { x: 0, y: 0, width: 1, height: 1 },
            start: None,
            current: None,
            shown: false,
            shown_at: None,
            had_focus: false,
            focus_lost_at: None,
            fade: 0.0,
        }
    }
}

impl Overlay {
    pub fn open(&mut self, monitor: MonitorRect, purpose: Purpose) {
        self.open = true;
        self.purposes = purpose;
        self.monitor = monitor;
        self.start = None;
        self.current = None;
        self.shown = false;
        self.shown_at = None;
        self.had_focus = false;
        self.focus_lost_at = None;
        self.fade = 0.0;
    }

    pub fn close(&mut self) {
        self.open = false;
        self.start = None;
        self.current = None;
        self.shown = false;
        self.shown_at = None;
    }

    pub fn monitor(&self) -> MonitorRect {
        self.monitor
    }

    /// 独立的全屏窗口。`root_ppp` 用于把显示器物理矩形换算成 egui 点。
    pub fn show(&mut self, ctx: &egui::Context, root_ppp: f32) -> Action {
        if !self.open {
            return Action::None;
        }
        let scale = if root_ppp > 0.0 { root_ppp } else { 1.0 };
        let position = egui::pos2(self.monitor.x as f32 / scale, self.monitor.y as f32 / scale);
        let size = egui::vec2(self.monitor.width as f32 / scale, self.monitor.height as f32 / scale);
        let viewport_id = egui::ViewportId::from_hash_of("daozai-capture-overlay");
        let builder = egui::ViewportBuilder::default()
            .with_title("框选取字 · 岛仔")
            .with_decorations(false)
            .with_transparent(true)
            .with_resizable(false)
            .with_taskbar(false)
            .with_always_on_top()
            .with_active(false)
            .with_visible(false)
            .with_position(position)
            .with_inner_size(size);
        let mut action = Action::None;
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
                action = Action::Cancel;
            }
            let focused = ctx.input(|input| input.viewport().focused.unwrap_or(false));
            if self.observe_focus(focused) {
                action = Action::Cancel;
            }
            let panel_action = self.panel_ui(ctx);
            if action == Action::None {
                action = panel_action;
            }
            if action != Action::None {
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            }
        });
        action
    }

    fn observe_focus(&mut self, focused: bool) -> bool {
        if focused {
            self.had_focus = true;
            self.focus_lost_at = None;
            return false;
        }
        if !self.had_focus {
            return false;
        }
        let lost = *self.focus_lost_at.get_or_insert(Instant::now());
        Instant::now().duration_since(lost) >= std::time::Duration::from_millis(120)
    }

    /// 遮罩本体。拆出来便于无窗口渲染与手势回归。
    pub fn panel_ui(&mut self, ctx: &egui::Context) -> Action {
        let screen = ctx.screen_rect();
        let fade = self.fade.clamp(0.0, 1.0);
        let pointer = ctx.input(|input| {
            (
                input.pointer.latest_pos(),
                input.pointer.primary_down(),
                input.pointer.primary_pressed(),
                input.pointer.primary_released(),
                input.key_pressed(egui::Key::Escape),
            )
        });
        let (latest, down, pressed, released, escape) = pointer;
        ctx.set_cursor_icon(egui::CursorIcon::Crosshair);

        if pressed {
            if let Some(position) = latest {
                self.start = Some(position);
                self.current = Some(position);
            }
        } else if down {
            if let Some(position) = latest {
                self.current = Some(position);
            }
        }

        let mut action = if escape { Action::Cancel } else { Action::None };
        let selection = self.selection_rect();
        if released {
            if let Some(rect) = selection {
                if rect.width() >= MIN_SELECTION && rect.height() >= MIN_SELECTION {
                    let ppp = ctx.pixels_per_point();
                    let monitor = self.monitor;
                    let x = (rect.left() * ppp).round() as i32;
                    let y = (rect.top() * ppp).round() as i32;
                    let width = ((rect.width() * ppp).round() as u32).min(monitor.width.saturating_sub(x.max(0) as u32));
                    let height = ((rect.height() * ppp).round() as u32).min(monitor.height.saturating_sub(y.max(0) as u32));
                    if width >= 4 && height >= 4 {
                        action = Action::Capture { x: x.max(0), y: y.max(0), width, height };
                    }
                }
            }
            self.start = None;
            self.current = None;
        }

        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("daozai-capture-overlay"),
        ));
        let dim = Color32::from_black_alpha((168.0 * fade) as u8);
        match selection {
            Some(rect) => {
                // 只压暗选区外，中间保持原屏幕亮度。
                painter.rect_filled(
                    Rect::from_min_max(screen.min, egui::pos2(screen.right(), rect.top())),
                    0.0,
                    dim,
                );
                painter.rect_filled(
                    Rect::from_min_max(egui::pos2(screen.left(), rect.bottom()), screen.max),
                    0.0,
                    dim,
                );
                painter.rect_filled(
                    Rect::from_min_max(egui::pos2(screen.left(), rect.top()), egui::pos2(rect.left(), rect.bottom())),
                    0.0,
                    dim,
                );
                painter.rect_filled(
                    Rect::from_min_max(egui::pos2(rect.right(), rect.top()), egui::pos2(screen.right(), rect.bottom())),
                    0.0,
                    dim,
                );
                let border = Rect::from_min_max(rect.min - Vec2::splat(0.5), rect.max + Vec2::splat(0.5));
                painter.rect_stroke(
                    border,
                    2.0,
                    Stroke::new(1.6_f32, theme::BLUE),
                    egui::StrokeKind::Outside,
                );
                // 四角白色刻度，弱化整圈描边。
                let tick = 14.0_f32.min(rect.width() / 3.0).min(rect.height() / 3.0);
                if tick > 3.0 {
                    let corners = [
                        (rect.left_top(), Vec2::new(1.0, 0.0), Vec2::new(0.0, 1.0)),
                        (rect.right_top(), Vec2::new(-1.0, 0.0), Vec2::new(0.0, 1.0)),
                        (rect.left_bottom(), Vec2::new(1.0, 0.0), Vec2::new(0.0, -1.0)),
                        (rect.right_bottom(), Vec2::new(-1.0, 0.0), Vec2::new(0.0, -1.0)),
                    ];
                    for (corner, dx, dy) in corners {
                        painter.line_segment(
                            [corner, corner + dx * tick],
                            Stroke::new(2.0_f32, Color32::WHITE),
                        );
                        painter.line_segment(
                            [corner, corner + dy * tick],
                            Stroke::new(2.0_f32, Color32::WHITE),
                        );
                    }
                }
                self.draw_size_label(&painter, rect, ctx.pixels_per_point(), fade);
            }
            None => {
                painter.rect_filled(screen, 0.0, dim);
                if let Some(position) = latest {
                    // 取景十字线，帮助对齐边缘。
                    let line = Stroke::new(1.0_f32, Color32::from_white_alpha((90.0 * fade) as u8));
                    painter.line_segment(
                        [egui::pos2(screen.left(), position.y), egui::pos2(screen.right(), position.y)],
                        line,
                    );
                    painter.line_segment(
                        [egui::pos2(position.x, screen.top()), egui::pos2(position.x, screen.bottom())],
                        line,
                    );
                    painter.circle_stroke(position, 5.0, Stroke::new(1.2_f32, Color32::from_white_alpha(150)));
                }
            }
        }
        self.draw_hint(&painter, screen, fade);
        action
    }

    fn selection_rect(&self) -> Option<Rect> {
        match (self.start, self.current) {
            (Some(start), Some(current)) => {
                let rect = Rect::from_two_pos(start, current);
                (rect.width() > 0.5 && rect.height() > 0.5).then_some(rect)
            }
            _ => None,
        }
    }

    fn draw_size_label(&self, painter: &egui::Painter, rect: Rect, ppp: f32, fade: f32) {
        let width = (rect.width() * ppp).round() as i32;
        let height = (rect.height() * ppp).round() as i32;
        let galley = painter.layout_no_wrap(
            format!("{width} × {height} 像素"),
            theme::font(11.0, W::Semibold),
            Color32::WHITE,
        );
        let size = galley.size() + Vec2::new(18.0, 8.0);
        let y = if rect.top() > size.y + 12.0 {
            rect.top() - size.y - 10.0
        } else {
            rect.top() + 10.0
        };
        let clip = painter.clip_rect();
        let x = rect.left().min(clip.right() - size.x).max(clip.left());
        let pill = Rect::from_min_size(egui::pos2(x, y), size);
        painter.rect_filled(pill, 6.0, Color32::from_black_alpha((205.0 * fade) as u8));
        painter.rect_stroke(
            pill,
            6.0,
            Stroke::new(1.0_f32, Color32::from_white_alpha((30.0 * fade) as u8)),
            egui::StrokeKind::Inside,
        );
        painter.galley(
            pill.center() - galley.size() / 2.0,
            galley,
            Color32::WHITE,
        );
    }

    fn draw_hint(&self, painter: &egui::Painter, screen: Rect, fade: f32) {
        let hint = painter.layout_no_wrap(
            self.purposes.hint().to_owned(),
            theme::font(11.5, W::Semibold),
            Color32::WHITE,
        );
        let key = painter.layout_no_wrap(
            "Esc 取消".to_owned(),
            theme::font(10.0, W::Regular),
            Color32::from_gray(190),
        );
        let padding = Vec2::new(14.0, 8.0);
        let gap = 10.0;
        let size = Vec2::new(hint.size().x + gap + key.size().x, hint.size().y.max(key.size().y)) + padding * 2.0;
        let pill = Rect::from_center_size(
            egui::pos2(screen.center().x, screen.top() + 34.0),
            size,
        );
        painter.rect_filled(pill, pill.height() / 2.0, Color32::from_black_alpha((188.0 * fade) as u8));
        painter.rect_stroke(
            pill,
            pill.height() / 2.0,
            Stroke::new(1.0_f32, Color32::from_white_alpha((26.0 * fade) as u8)),
            egui::StrokeKind::Inside,
        );
        let content_left = pill.left() + padding.x;
        let hint_width = hint.size().x;
        let text_y = pill.center().y - hint.size().y / 2.0;
        painter.galley(egui::pos2(content_left, text_y), hint, Color32::WHITE);
        painter.galley(
            egui::pos2(content_left + hint_width + gap, pill.center().y - key.size().y / 2.0),
            key,
            Color32::from_gray(190),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> egui::Context {
        let ctx = egui::Context::default();
        crate::theme::load_fonts(&ctx);
        ctx
    }

    fn monitor() -> MonitorRect {
        MonitorRect { x: 0, y: 0, width: 800, height: 500 }
    }

    fn new_overlay() -> Overlay {
        let mut overlay = Overlay::default();
        overlay.open(monitor(), Purpose::Ocr);
        overlay.fade = 1.0;
        overlay
    }

    fn frame(overlay: &mut Overlay, ctx: &egui::Context, time: f64, events: Vec<egui::Event>) -> Action {
        let mut action = Action::None;
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 500.0))),
            time: Some(time),
            events,
            focused: true,
            viewports: std::iter::once((
                egui::ViewportId::ROOT,
                egui::ViewportInfo { native_pixels_per_point: Some(1.0), ..Default::default() },
            ))
            .collect(),
            ..Default::default()
        };
        let _ = ctx.run(raw, |ctx| {
            action = overlay.panel_ui(ctx);
        });
        action
    }

    fn button(position: Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos: position,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        }
    }

    #[test]
    fn dragging_produces_a_physical_capture_rect() {
        let ctx = context();
        let mut overlay = new_overlay();
        frame(&mut overlay, &ctx, 0.0, vec![egui::Event::PointerMoved(Pos2::new(100.0, 120.0))]);
        frame(&mut overlay, &ctx, 0.05, vec![button(Pos2::new(100.0, 120.0), true)]);
        frame(&mut overlay, &ctx, 0.10, vec![egui::Event::PointerMoved(Pos2::new(320.0, 260.0))]);
        let action = frame(&mut overlay, &ctx, 0.15, vec![button(Pos2::new(320.0, 260.0), false)]);
        assert_eq!(action, Action::Capture { x: 100, y: 120, width: 220, height: 140 });
    }

    #[test]
    fn tiny_or_cancelled_selections_do_not_capture() {
        let ctx = context();
        let mut overlay = new_overlay();
        frame(&mut overlay, &ctx, 0.0, vec![egui::Event::PointerMoved(Pos2::new(50.0, 50.0))]);
        frame(&mut overlay, &ctx, 0.05, vec![button(Pos2::new(50.0, 50.0), true)]);
        frame(&mut overlay, &ctx, 0.08, vec![egui::Event::PointerMoved(Pos2::new(54.0, 56.0))]);
        let action = frame(&mut overlay, &ctx, 0.10, vec![button(Pos2::new(54.0, 56.0), false)]);
        assert_eq!(action, Action::None, "过小的拖动不应触发识别");

        let mut overlay = new_overlay();
        frame(&mut overlay, &ctx, 1.0, vec![egui::Event::PointerMoved(Pos2::new(10.0, 10.0))]);
        frame(&mut overlay, &ctx, 1.05, vec![button(Pos2::new(10.0, 10.0), true)]);
        frame(&mut overlay, &ctx, 1.08, vec![egui::Event::PointerMoved(Pos2::new(300.0, 200.0))]);
        let action = frame(
            &mut overlay,
            &ctx,
            1.10,
            vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            }],
        );
        assert_eq!(action, Action::Cancel);
    }

    #[test]
    fn selection_is_clamped_inside_the_monitor() {
        let ctx = context();
        let mut overlay = new_overlay();
        frame(&mut overlay, &ctx, 0.0, vec![egui::Event::PointerMoved(Pos2::new(790.0, 490.0))]);
        frame(&mut overlay, &ctx, 0.05, vec![button(Pos2::new(790.0, 490.0), true)]);
        frame(&mut overlay, &ctx, 0.08, vec![egui::Event::PointerMoved(Pos2::new(795.0, 495.0))]);
        // 反向拖动：从右下到左上，坐标与尺寸仍为正。
        frame(&mut overlay, &ctx, 0.10, vec![egui::Event::PointerMoved(Pos2::new(760.0, 460.0))]);
        let action = frame(&mut overlay, &ctx, 0.15, vec![button(Pos2::new(760.0, 460.0), false)]);
        match action {
            Action::Capture { x, y, width, height } => {
                assert_eq!((x, y), (760, 460));
                assert_eq!((width, height), (30, 30));
            }
            other => panic!("应得到捕获动作，实际 {other:?}"),
        }
    }

    #[test]
    fn overlay_renders_dimmed_selection_and_hints() {
        let ctx = context();
        crate::theme::load_fonts(&ctx);
        let mut overlay = new_overlay();
        overlay.start = Some(Pos2::new(120.0, 140.0));
        overlay.current = Some(Pos2::new(440.0, 300.0));
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 500.0))),
            time: Some(0.0),
            focused: true,
            viewports: std::iter::once((
                egui::ViewportId::ROOT,
                egui::ViewportInfo { native_pixels_per_point: Some(1.0), ..Default::default() },
            ))
            .collect(),
            ..Default::default()
        };
        let output = ctx.run(raw, |ctx| {
            overlay.panel_ui(ctx);
        });
        let shapes = output.shapes;
        if let Some(dest) = std::env::var_os("DAOZAI_CAPTURE_PREVIEW_DIR").filter(|value| !value.is_empty()) {
            let dest = std::path::PathBuf::from(dest);
            std::fs::create_dir_all(&dest).unwrap();
            let mut raster = crate::preview_raster::Raster::new(800, 500);
            raster.update_textures(&output.textures_delta);
            let primitives = ctx.tessellate(shapes.clone(), output.pixels_per_point);
            raster.draw(&primitives, output.pixels_per_point);
            let mut rgba = Vec::new();
            raster.to_rgba8(&mut rgba);
            image::save_buffer(
                dest.join("capture-overlay.png"),
                &rgba,
                800,
                500,
                image::ColorType::Rgba8,
            )
            .unwrap();
        }
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
        for clipped in &shapes {
            collect(&clipped.shape, &mut texts);
        }
        assert!(texts.iter().any(|text| text.galley.text().contains("拖动框选文字区域")));
        assert!(texts.iter().any(|text| text.galley.text().contains("Esc 取消")));
        assert!(texts.iter().any(|text| text.galley.text().contains("320 × 160")));
    }

    #[test]
    fn screenshot_purpose_shows_save_hint() {
        let ctx = context();
        let mut overlay = new_overlay();
        overlay.purposes = Purpose::Screenshot;
        overlay.start = Some(Pos2::new(120.0, 140.0));
        overlay.current = Some(Pos2::new(440.0, 300.0));
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 500.0))),
            time: Some(0.0),
            focused: true,
            viewports: std::iter::once((
                egui::ViewportId::ROOT,
                egui::ViewportInfo { native_pixels_per_point: Some(1.0), ..Default::default() },
            ))
            .collect(),
            ..Default::default()
        };
        let output = ctx.run(raw, |ctx| {
            overlay.panel_ui(ctx);
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
        assert!(texts.iter().any(|text| text.galley.text().contains("拖动框选截图区域，松开后复制并保存")));
        assert!(Purpose::Scroll.hint().contains("滚动截图区域"));
    }
}