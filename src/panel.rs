//! 独立浮层（剪贴板、命令面板等）共用的视觉令牌与小组件。
//!
//! 岛体本身是一块纯黑「刘海」，浮层则用石墨底 + 轻描边 + 柔和阴影，
//! 深浅色跟随系统。把配色、键帽、图标按钮、空状态、搜索图标等放在这里，
//! 保证多个面板的圆角、字号、间距和交互反馈一致。

use crate::theme::{self, W};
use eframe::egui::{
    self, Color32, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Ui, Vec2,
};

/// 一套面板配色。
pub(crate) struct Palette {
    pub surface: Color32,
    pub raised: Color32,
    pub hover: Color32,
    pub line: Color32,
    pub line_soft: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub faint: Color32,
    pub accent: Color32,
    pub danger: Color32,
    pub warn: Color32,
    pub scroll_idle: Color32,
    pub scroll_hover: Color32,
    pub scroll_active: Color32,
}

impl Palette {
    pub(crate) fn new(dark: bool) -> Self {
        if dark {
            Self {
                surface: rgb(30, 31, 34),
                raised: rgb(39, 40, 44),
                hover: rgb(47, 48, 53),
                line: rgb(64, 65, 70),
                line_soft: rgb(52, 53, 58),
                text: rgb(246, 247, 249),
                muted: rgb(166, 168, 175),
                faint: rgb(134, 136, 143),
                accent: theme::BLUE,
                danger: rgb(255, 112, 112),
                warn: theme::YELLOW,
                scroll_idle: rgb(78, 80, 86),
                scroll_hover: rgb(98, 100, 107),
                scroll_active: rgb(120, 122, 130),
            }
        } else {
            Self {
                surface: rgb(251, 252, 253),
                raised: rgb(244, 246, 249),
                hover: rgb(236, 239, 244),
                line: rgb(222, 226, 232),
                line_soft: rgb(232, 235, 240),
                text: rgb(30, 34, 41),
                muted: rgb(104, 110, 120),
                faint: rgb(124, 129, 139),
                accent: rgb(28, 118, 208),
                danger: rgb(196, 54, 64),
                warn: rgb(180, 118, 10),
                scroll_idle: rgb(205, 209, 217),
                scroll_hover: rgb(188, 193, 202),
                scroll_active: rgb(168, 174, 185),
            }
        }
    }
}

/// 给浮层设置统一 style，并返回对应配色。
pub(crate) fn apply_style(ctx: &egui::Context, dark: bool) -> Palette {
    let palette = Palette::new(dark);
    let mut style = (*ctx.style()).clone();
    style.visuals = if dark { egui::Visuals::dark() } else { egui::Visuals::light() };
    style.visuals.window_fill = palette.raised;
    style.visuals.window_stroke = Stroke::new(1.0_f32, palette.line);
    style.visuals.window_corner_radius = egui::CornerRadius::same(12);
    style.visuals.menu_corner_radius = egui::CornerRadius::same(10);
    style.visuals.panel_fill = palette.surface;
    style.visuals.extreme_bg_color = palette.raised;
    style.visuals.faint_bg_color = palette.raised;
    style.visuals.code_bg_color = palette.raised;
    style.visuals.hyperlink_color = palette.accent;
    style.visuals.override_text_color = Some(palette.text);
    style.visuals.text_cursor.stroke = Stroke::new(1.5_f32, palette.accent);
    style.visuals.selection.bg_fill = with_alpha(palette.accent, 0.32);
    style.visuals.selection.stroke = Stroke::new(1.0_f32, palette.accent);
    let widgets = &mut style.visuals.widgets;
    widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
    widgets.hovered.weak_bg_fill = palette.hover;
    widgets.active.weak_bg_fill = palette.hover;
    widgets.inactive.bg_fill = palette.scroll_idle;
    widgets.hovered.bg_fill = palette.scroll_hover;
    widgets.active.bg_fill = palette.scroll_active;
    widgets.inactive.bg_stroke = Stroke::new(1.0_f32, palette.line_soft);
    widgets.hovered.bg_stroke = Stroke::new(1.0_f32, palette.line_soft);
    widgets.active.bg_stroke = Stroke::new(1.0_f32, palette.line_soft);
    widgets.inactive.fg_stroke = Stroke::new(1.0_f32, palette.text);
    widgets.hovered.fg_stroke = Stroke::new(1.0_f32, palette.text);
    widgets.active.fg_stroke = Stroke::new(1.0_f32, palette.text);
    widgets.inactive.corner_radius = egui::CornerRadius::same(7);
    widgets.hovered.corner_radius = egui::CornerRadius::same(7);
    widgets.active.corner_radius = egui::CornerRadius::same(7);
    widgets.open = widgets.hovered;
    style.spacing.item_spacing = Vec2::new(8.0, 6.0);
    style.spacing.button_padding = Vec2::new(10.0, 5.0);
    style.spacing.scroll.floating = false;
    style.spacing.scroll.bar_width = 5.0;
    style.spacing.scroll.bar_inner_margin = 3.0;
    style.spacing.scroll.foreground_color = false;
    ctx.set_style(style);
    palette
}

/// 面板外层的柔和投影。
pub(crate) fn shadow(dark: bool) -> egui::epaint::Shadow {
    egui::epaint::Shadow {
        offset: [0, 14],
        blur: 34,
        spread: 0,
        color: Color32::from_black_alpha(if dark { 135 } else { 42 }),
    }
}

pub(crate) fn rgb(r: u8, g: u8, b: u8) -> Color32 {
    Color32::from_rgb(r, g, b)
}

pub(crate) fn with_alpha(color: Color32, alpha: f32) -> Color32 {
    Color32::from_rgba_unmultiplied(
        color.r(),
        color.g(),
        color.b(),
        (255.0 * alpha.clamp(0.0, 1.0)) as u8,
    )
}

pub(crate) fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let channel = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round().clamp(0.0, 255.0) as u8;
    Color32::from_rgba_unmultiplied(
        channel(a.r(), b.r()),
        channel(a.g(), b.g()),
        channel(a.b(), b.b()),
        channel(a.a(), b.a()),
    )
}

pub(crate) fn draw_search_icon(ui: &Ui, rect: Rect, color: Color32) {
    let painter = ui.painter();
    painter.circle_stroke(rect.center() - Vec2::new(1.5, 1.5), 5.5, Stroke::new(1.7_f32, color));
    painter.line_segment(
        [rect.center() + Vec2::new(2.2, 2.2), rect.right_bottom() - Vec2::new(2.0, 2.0)],
        Stroke::new(1.7_f32, color),
    );
}

pub(crate) fn keycap(ui: &mut Ui, label: &str, p: &Palette) {
    let font = theme::font(9.5, W::Regular);
    let galley = ui.fonts(|fonts| fonts.layout_no_wrap(label.to_owned(), font, p.muted));
    let width = (galley.size().x + 12.0).max(24.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 18.0), Sense::hover());
    ui.painter().rect_filled(rect, 5.0, p.raised);
    ui.painter().rect_stroke(rect, 5.0, Stroke::new(1.0_f32, p.line_soft), StrokeKind::Inside);
    ui.painter().galley(
        Pos2::new(rect.center().x - galley.size().x / 2.0, rect.center().y - galley.size().y / 2.0),
        galley,
        p.muted,
    );
}

pub(crate) fn hint(ui: &mut Ui, key: &str, label: &str, p: &Palette) {
    keycap(ui, key, p);
    ui.label(RichText::new(label).font(theme::font(10.5, W::Regular)).color(p.faint));
}

pub(crate) fn section_label(ui: &mut Ui, label: &str, p: &Palette) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 24.0), Sense::hover());
    ui.painter().text(
        Pos2::new(rect.left() + 4.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        theme::font(10.0, W::Semibold),
        p.faint,
    );
}

pub(crate) fn icon_button(
    ui: &mut Ui,
    glyph: &str,
    idle: Color32,
    hover_color: Color32,
    hover_bg: Color32,
    size: f32,
) -> bool {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(26.0), Sense::click());
    if response.hovered() {
        ui.painter().circle_filled(rect.center(), 13.0, hover_bg);
    }
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        glyph,
        theme::font(size, W::Regular),
        if response.hovered() { hover_color } else { idle },
    );
    let _ = response.clone().on_hover_cursor(egui::CursorIcon::PointingHand);
    response.clicked()
}

pub(crate) fn chip_button(ui: &mut Ui, label: &str, color: Color32, background: Color32) -> bool {
    let font = theme::font(10.5, W::Semibold);
    let galley = ui.fonts(|fonts| fonts.layout_no_wrap(label.to_owned(), font, color));
    let size = Vec2::new(galley.size().x + 18.0, 20.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let background = if response.hovered() { with_alpha(background, 0.85) } else { background };
    ui.painter().rect_filled(rect, 10.0, background);
    ui.painter().galley(
        Pos2::new(rect.center().x - galley.size().x / 2.0, rect.center().y - galley.size().y / 2.0),
        galley,
        color,
    );
    response.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}

pub(crate) fn link_button(ui: &mut Ui, label: &str, p: &Palette) -> bool {
    let font = theme::font(11.5, W::Semibold);
    let galley = ui.fonts(|fonts| fonts.layout_no_wrap(label.to_owned(), font, p.accent));
    let (rect, response) = ui.allocate_exact_size(galley.size() + Vec2::new(4.0, 2.0), Sense::click());
    ui.painter().galley(rect.min, galley, p.accent);
    let hover = ui.ctx().animate_bool_with_time(egui::Id::new(("panel-link", label)), response.hovered(), 0.10);
    let width = rect.width() * hover;
    ui.painter().line_segment(
        [Pos2::new(rect.left(), rect.bottom()), Pos2::new(rect.left() + width, rect.bottom())],
        Stroke::new(1.0_f32, p.accent),
    );
    response.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}
/// 计算浮层窗口在当前显示器内可用的内尺寸与最小尺寸。
///
/// - 正常屏幕返回期望尺寸；
/// - 小屏/高缩放时按显示器可用区域收缩；
/// - 最小尺寸不会超过实际尺寸，避免窗口无法变小。
pub(crate) fn fitted_window(
    desired: egui::Vec2,
    minimum: egui::Vec2,
    monitor: egui::Vec2,
) -> (egui::Vec2, egui::Vec2) {
    let max_width = (monitor.x - 24.0).max(240.0);
    let max_height = (monitor.y - 48.0).max(180.0);
    let size = egui::vec2(desired.x.min(max_width), desired.y.min(max_height));
    let min = egui::vec2(minimum.x.min(size.x), minimum.y.min(size.y));
    (size, min)
}

/// 读取当前显示器尺寸并计算浮层窗口尺寸；未知显示器时保持期望尺寸。
pub(crate) fn fitted_window_ctx(
    ctx: &egui::Context,
    desired: egui::Vec2,
    minimum: egui::Vec2,
) -> (egui::Vec2, egui::Vec2) {
    let monitor = ctx
        .input(|input| input.viewport().monitor_size)
        .unwrap_or(desired);
    fitted_window(desired, minimum, monitor)
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fitted_windows_shrink_on_small_monitors() {
        let (size, min) = fitted_window(
            egui::vec2(560.0, 520.0),
            egui::vec2(420.0, 360.0),
            egui::vec2(1920.0, 1080.0),
        );
        assert_eq!(size, egui::vec2(560.0, 520.0));
        assert_eq!(min, egui::vec2(420.0, 360.0));
        let (size, min) = fitted_window(
            egui::vec2(560.0, 520.0),
            egui::vec2(420.0, 360.0),
            egui::vec2(683.0, 384.0),
        );
        assert!(size.x <= 659.0 && size.y <= 336.0, "小屏应收缩窗口：{size:?}");
        assert!(min.x <= size.x && min.y <= size.y, "最小尺寸不能超过窗口：{min:?}");
        let (tiny, tiny_min) = fitted_window(
            egui::vec2(560.0, 520.0),
            egui::vec2(420.0, 360.0),
            egui::vec2(300.0, 220.0),
        );
        assert!(tiny.x >= 240.0 && tiny.y >= 180.0, "极窄屏仍保留可用尺寸");
        assert!(tiny_min.x <= tiny.x && tiny_min.y <= tiny.y);
    }
}