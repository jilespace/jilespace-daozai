//! 剪贴板历史弹窗：独立视口、搜索与键盘选择。
use crate::clipboard::Item;
use crate::settings_theme::color as settings_color;
use crate::theme::{self, W};
use eframe::egui::{self, Color32, Key, Modifiers, Sense, Vec2};
use std::time::{Duration, Instant};

const WIDTH: f32 = 420.0;
const HEIGHT: f32 = 440.0;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    None,
    Copy(String),
    Delete(String),
    Pin(String, bool),
    Enable,
    Resume,
    Close { restore: bool },
}

pub struct Popup {
    pub open: bool,
    pub query: String,
    pub selected: usize,
    pub items: Vec<Item>,
    pub enabled: bool,
    pub paused: bool,
    pub hotkey_error: Option<String>,
    request_focus: bool,
    shown: bool,
    had_focus: bool,
    focus_lost_at: Option<Instant>,
}

impl Default for Popup {
    fn default() -> Self {
        Self {
            open: false,
            query: String::new(),
            selected: 0,
            items: Vec::new(),
            enabled: false,
            paused: false,
            hotkey_error: None,
            request_focus: false,
            shown: false,
            had_focus: false,
            focus_lost_at: None,
        }
    }
}

impl Popup {
    pub fn open_state(&mut self, enabled: bool, paused: bool, hotkey_error: Option<String>) {
        self.open = true;
        self.query.clear();
        self.selected = 0;
        self.enabled = enabled;
        self.paused = paused;
        self.hotkey_error = hotkey_error;
        self.request_focus = true;
        self.shown = false;
        self.had_focus = false;
        self.focus_lost_at = None;
    }

    pub fn close(&mut self) {
        self.open = false;
        self.shown = false;
    }

    pub fn sync(&mut self, enabled: bool, paused: bool, hotkey_error: Option<String>) {
        self.enabled = enabled;
        self.paused = paused;
        self.hotkey_error = hotkey_error;
    }

    pub fn set_items(&mut self, items: Vec<Item>) {
        self.items = items;
        self.clamp();
    }

    pub fn filtered(&self) -> Vec<usize> {
        let needle = self.query.trim().to_lowercase();
        self.items
            .iter()
            .enumerate()
            .filter(|(_, item)| needle.is_empty() || item.text.to_lowercase().contains(&needle))
            .map(|(index, _)| index)
            .collect()
    }

    pub fn clamp(&mut self) {
        let len = self.filtered().len();
        self.selected = self.selected.min(len.saturating_sub(1));
    }

    pub fn move_selection(&mut self, delta: isize) {
        let len = self.filtered().len();
        if len == 0 {
            self.selected = 0;
            return;
        }
        let next = self.selected as isize + delta;
        self.selected = next.clamp(0, len as isize - 1) as usize;
    }

    pub fn selected_id(&self) -> Option<String> {
        let indices = self.filtered();
        indices
            .get(self.selected)
            .map(|index| self.items[*index].id.clone())
    }

    pub fn observe_focus(&mut self, focused: bool, now: Instant) -> bool {
        if focused {
            self.had_focus = true;
            self.focus_lost_at = None;
            return false;
        }
        if !self.had_focus {
            return false;
        }
        let lost = *self.focus_lost_at.get_or_insert(now);
        now.duration_since(lost) >= Duration::from_millis(100)
    }

    pub fn show(&mut self, ctx: &egui::Context, dark: bool) -> Action {
        if !self.open {
            return Action::None;
        }
        let viewport_id = egui::ViewportId::from_hash_of("daozai-clipboard");
        let ppp = ctx.pixels_per_point();
        #[cfg(windows)]
        let position = crate::native::cursor_work_area(ppp)
            .map(|(x, y, width, _)| egui::pos2(x + (width - WIDTH) / 2.0, y + 60.0))
            .unwrap_or_else(|| egui::pos2(160.0, 80.0));
        #[cfg(not(windows))]
        let position = {
            let _ = ppp;
            egui::pos2(160.0, 80.0)
        };
        let mut action = Action::None;

        ctx.show_viewport_immediate(
            viewport_id,
            egui::ViewportBuilder::default()
                .with_title("剪贴板历史 · 岛仔")
                .with_icon(theme::app_icon())
                .with_inner_size([WIDTH, HEIGHT])
                .with_min_inner_size([360.0, 300.0])
                .with_decorations(false)
                .with_transparent(true)
                .with_resizable(false)
                .with_taskbar(false)
                .with_always_on_top()
                .with_active(false)
                .with_visible(false)
                .with_position(position),
            |ctx, _| {
                let original = ctx.style();
                let mut style = (*original).clone();
                style.visuals = if dark { egui::Visuals::dark() } else { egui::Visuals::light() };
                ctx.set_style(style);
                ctx.send_viewport_cmd(egui::ViewportCommand::SetTheme(if dark {
                    egui::SystemTheme::Dark
                } else {
                    egui::SystemTheme::Light
                }));

                if !self.shown {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                    self.shown = true;
                }
                if ctx.input(|input| input.viewport().close_requested()) {
                    action = Action::Close { restore: false };
                }
                let focused = ctx.input(|input| input.viewport().focused.unwrap_or(false));
                if self.observe_focus(focused, Instant::now()) {
                    action = Action::Close { restore: false };
                }

                let (up, down, enter, escape, delete) = ctx.input_mut(|input| {
                    (
                        input.consume_key(Modifiers::NONE, Key::ArrowUp),
                        input.consume_key(Modifiers::NONE, Key::ArrowDown),
                        input.consume_key(Modifiers::NONE, Key::Enter),
                        input.consume_key(Modifiers::NONE, Key::Escape),
                        input.consume_key(Modifiers::NONE, Key::Delete),
                    )
                });
                if escape {
                    action = Action::Close { restore: true };
                }
                if up {
                    self.move_selection(-1);
                }
                if down {
                    self.move_selection(1);
                }
                if enter {
                    if let Some(id) = self.selected_id() {
                        action = Action::Copy(id);
                    }
                }
                if delete {
                    if let Some(id) = self.selected_id() {
                        action = Action::Delete(id);
                    }
                }

                let previous_query = self.query.clone();
                egui::CentralPanel::default()
                    .frame(egui::Frame::new().inner_margin(10))
                    .show(ctx, |ui| {
                        let background =
                            settings_color(ctx, Color32::from_rgb(35, 35, 38), Color32::WHITE);
                        egui::Frame::new()
                            .fill(background)
                            .corner_radius(14.0)
                            .inner_margin(14)
                            .show(ui, |ui| {
                                ui.set_min_size(ui.available_size());
                                ui.horizontal(|ui| {
                                    let (icon_rect, _) =
                                        ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
                                    let card =
                                        crate::paint::Card::new(ui, icon_rect.min, icon_rect);
                                    crate::paint::search(
                                        &card,
                                        7.0,
                                        7.0,
                                        5.0,
                                        settings_color(
                                            ctx,
                                            Color32::from_gray(150),
                                            Color32::from_gray(105),
                                        ),
                                    );
                                    let response = ui.add(
                                        egui::TextEdit::singleline(&mut self.query)
                                            .hint_text("搜索剪贴板…")
                                            .font(theme::font(13.0, W::Regular))
                                            .frame(false)
                                            .desired_width(f32::INFINITY),
                                    );
                                    if self.request_focus {
                                        response.request_focus();
                                        self.request_focus = false;
                                    }
                                });
                                if self.query != previous_query {
                                    self.selected = 0;
                                    self.clamp();
                                }
                                if let Some(error) = &self.hotkey_error {
                                    ui.label(
                                        egui::RichText::new(error)
                                            .font(theme::font(10.5, W::Regular))
                                            .color(theme::YELLOW),
                                    );
                                }
                                ui.add_space(6.0);
                                ui.separator();
                                if !self.enabled {
                                    ui.add_space(10.0);
                                    ui.vertical_centered(|ui| {
                                        ui.label(
                                            egui::RichText::new("剪贴板历史未开启")
                                                .font(theme::font(13.0, W::Regular))
                                                .color(settings_color(
                                                    ctx,
                                                    Color32::WHITE,
                                                    Color32::from_rgb(30, 34, 41),
                                                )),
                                        );
                                        ui.add_space(4.0);
                                        ui.label(
                                            egui::RichText::new(
                                                "开启后，复制的文本会加密保存在本机",
                                            )
                                            .font(theme::font(11.0, W::Regular))
                                            .color(settings_color(
                                                ctx,
                                                Color32::from_gray(180),
                                                Color32::from_gray(75),
                                            )),
                                        );
                                        ui.add_space(10.0);
                                        if ui.button("开启剪贴板历史").clicked() {
                                            action = Action::Enable;
                                        }
                                    });
                                    return;
                                }
                                if self.paused {
                                    ui.add_space(8.0);
                                    ui.horizontal(|ui| {
                                        ui.label(
                                            egui::RichText::new("已暂停记录")
                                                .font(theme::font(11.5, W::Regular))
                                                .color(settings_color(
                                                    ctx,
                                                    Color32::from_gray(180),
                                                    Color32::from_gray(75),
                                                )),
                                        );
                                        if ui.button("恢复").clicked() {
                                            action = Action::Resume;
                                        }
                                    });
                                }

                                let indices = self.filtered();
                                self.clamp();
                                egui::ScrollArea::vertical()
                                    .auto_shrink([false, false])
                                    .show(ui, |ui| {
                                        if indices.is_empty() {
                                            ui.add_space(70.0);
                                            ui.vertical_centered(|ui| {
                                                ui.label(
                                                    egui::RichText::new(
                                                        if self.query.trim().is_empty() {
                                                            "还没有记录"
                                                        } else {
                                                            "没有匹配的内容"
                                                        },
                                                    )
                                                    .font(theme::font(12.0, W::Regular))
                                                    .color(settings_color(
                                                        ctx,
                                                        Color32::from_gray(150),
                                                        Color32::from_gray(105),
                                                    )),
                                                );
                                            });
                                            return;
                                        }
                                        for (position, index) in indices.iter().enumerate() {
                                            if let Some(next) = row(
                                                ui,
                                                &self.items[*index],
                                                position == self.selected,
                                                chrono::Local::now().timestamp(),
                                            ) {
                                                action = next;
                                            }
                                        }
                                    });
                                ui.add_space(6.0);
                                ui.label(
                                    egui::RichText::new("↑↓ 选择 · Enter 复制 · Esc 关闭")
                                        .font(theme::font(10.0, W::Regular))
                                        .color(settings_color(
                                            ctx,
                                            Color32::from_gray(150),
                                            Color32::from_gray(105),
                                        )),
                                );
                            });
                    });
            },
        );
        action
    }
}

fn row(ui: &mut egui::Ui, item: &Item, selected: bool, now: i64) -> Option<Action> {
    let ctx = ui.ctx().clone();
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 56.0), Sense::click());
    if !ui.is_rect_visible(rect) {
        return None;
    }
    let hover = ctx.animate_bool_with_time(
        ui.id().with(("clipboard-row", item.id.as_str())),
        response.hovered(),
        0.12,
    );
    let text_color = settings_color(&ctx, Color32::WHITE, Color32::from_rgb(30, 34, 41));
    let muted = settings_color(&ctx, Color32::from_gray(150), Color32::from_gray(105));
    let painter = ui.painter().with_clip_rect(rect);
    if selected {
        painter.rect_filled(rect, 8.0, theme::alpha_blue(0.16));
        painter.rect_filled(
            egui::Rect::from_min_size(rect.min, Vec2::new(2.0, rect.height())),
            1.0,
            theme::BLUE,
        );
    } else if hover > 0.0 {
        painter.rect_filled(
            rect,
            8.0,
            settings_color(
                &ctx,
                Color32::from_rgb(43, 43, 46),
                Color32::from_rgb(214, 221, 232),
            )
            .gamma_multiply(hover),
        );
    }

    let preview = item.text.replace(['\r', '\n'], " ");
    let mut job = egui::text::LayoutJob::simple(
        preview,
        theme::font(13.0, W::Regular),
        text_color,
        rect.width() - 76.0,
    );
    job.wrap.max_rows = 2;
    job.wrap.break_anywhere = true;
    let galley = ui.fonts(|fonts| fonts.layout_job(job));
    painter.galley(rect.min + Vec2::new(10.0, 7.0), galley, text_color);
    painter.text(
        egui::pos2(rect.left() + 10.0, rect.bottom() - 8.0),
        egui::Align2::LEFT_BOTTOM,
        relative_time(now, item.copied_at),
        theme::font(11.0, W::Regular),
        muted,
    );

    let pin_rect = egui::Rect::from_center_size(
        egui::pos2(rect.right() - 22.0, rect.center().y),
        Vec2::splat(22.0),
    );
    let pin_response = ui.interact(
        pin_rect,
        ui.id().with(("clipboard-pin", item.id.as_str())),
        Sense::click(),
    );
    if item.pinned || pin_response.hovered() {
        let card = crate::paint::Card::new(ui, pin_rect.min, pin_rect);
        crate::paint::pin(
            &card,
            11.0,
            11.0,
            14.0,
            item.pinned,
            if item.pinned { theme::YELLOW } else { muted },
        );
    }

    let delete_rect = egui::Rect::from_center_size(
        egui::pos2(rect.right() - 48.0, rect.center().y),
        Vec2::splat(22.0),
    );
    let delete_response = ui.interact(
        delete_rect,
        ui.id().with(("clipboard-delete", item.id.as_str())),
        Sense::click(),
    );
    if delete_response.hovered() || selected {
        let card = crate::paint::Card::new(ui, delete_rect.min, delete_rect);
        crate::paint::close(
            &card,
            11.0,
            11.0,
            4.5,
            if delete_response.hovered() { theme::RED } else { muted },
        );
    }

    if pin_response.clicked() {
        return Some(Action::Pin(item.id.clone(), !item.pinned));
    }
    if delete_response.clicked() {
        return Some(Action::Delete(item.id.clone()));
    }
    if response.clicked() {
        return Some(Action::Copy(item.id.clone()));
    }
    None
}

pub fn relative_time(now: i64, at: i64) -> String {
    let delta = now.saturating_sub(at);
    if delta < 60 {
        "刚刚".into()
    } else if delta < 3_600 {
        format!("{} 分钟前", delta / 60)
    } else if delta < 86_400 {
        format!("{} 小时前", delta / 3_600)
    } else if delta < 7 * 86_400 {
        format!("{} 天前", delta / 86_400)
    } else {
        chrono::DateTime::from_timestamp(at, 0)
            .map(|time| time.with_timezone(&chrono::Local).format("%m-%d").to_string())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, text: &str, at: i64) -> Item {
        Item { id: id.into(), text: text.into(), copied_at: at, pinned: false }
    }

    #[test]
    fn open_resets_query_and_selection() {
        let mut popup = Popup::default();
        popup.query = "old".into();
        popup.selected = 3;
        popup.open_state(true, false, None);
        assert!(popup.open);
        assert!(popup.query.is_empty());
        assert_eq!(popup.selected, 0);
    }

    #[test]
    fn filtering_and_selection_stay_in_range() {
        let mut popup = Popup::default();
        popup.open_state(true, false, None);
        popup.set_items(vec![
            item("a", "Alpha", 1),
            item("b", "Beta", 2),
            item("c", "alpha two", 3),
        ]);
        popup.query = "alpha".into();
        assert_eq!(popup.filtered().len(), 2);
        popup.move_selection(5);
        assert_eq!(popup.selected, 1);
        popup.move_selection(-5);
        assert_eq!(popup.selected, 0);
        assert_eq!(popup.selected_id().as_deref(), Some("a"));
    }

    #[test]
    fn focus_loss_needs_a_grace_period() {
        let mut popup = Popup::default();
        popup.open_state(true, false, None);
        let start = Instant::now();
        assert!(!popup.observe_focus(false, start));
        assert!(!popup.observe_focus(true, start + Duration::from_millis(10)));
        assert!(!popup.observe_focus(false, start + Duration::from_millis(20)));
        assert!(!popup.observe_focus(false, start + Duration::from_millis(80)));
        assert!(popup.observe_focus(false, start + Duration::from_millis(130)));
    }

    #[test]
    fn relative_time_uses_compact_labels() {
        assert_eq!(relative_time(1000, 1000), "刚刚");
        assert_eq!(relative_time(1000, 880), "2 分钟前");
        assert_eq!(relative_time(10_000, 2_800), "2 小时前");
        assert_eq!(relative_time(200_000, 100_000), "1 天前");
    }
}
