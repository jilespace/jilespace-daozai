#![cfg_attr(windows, windows_subsystem = "windows")]
//! Windows 外壳：无边框透明窗口 + 托盘 + 系统数据，界面全部交给 `daozai::app::App`。

use eframe::egui::{self, Color32, Pos2};
use daozai::app::{self, App, Cmd, Env, Options};
use daozai::smtc;
use daozai::poll_clock::PollClock;
use std::time::{Duration, Instant};

mod diagnostics;

struct Shell {
    app: App,
    t0: Instant,
    #[cfg(windows)]
    frame_at: Instant,
    media: smtc::Shared,
    placed: bool,
    win_x: f32,
    sys: (Option<u8>, bool, PollClock),
    #[cfg(windows)]
    system_state: daozai::system::Snapshot,
    #[cfg(windows)]
    system_rx: std::sync::mpsc::Receiver<daozai::system::Observation>,
    fullscreen: bool,
    input_state:daozai::input::Monitor,
    #[cfg(windows)]
    input_keys:Option<daozai::input::Shortcuts>,
    #[cfg(windows)]
    incoming: Option<std::sync::mpsc::Receiver<daozai::system::Incoming>>,
    #[cfg(windows)]
    hidden: bool,
    #[cfg(windows)]
    native: Option<daozai::native::Window>,
    #[cfg(windows)]
    tray: Option<daozai::tray::Tray>,
    #[cfg(windows)]
    autostart: bool,
    /// v0.5.4：蓝牙音频设备（后台线程推送）与音量回声的静默窗口。
    #[cfg(windows)]
    bt_rx: Option<std::sync::mpsc::Receiver<Vec<daozai::bt::Device>>>,
    bt: Vec<daozai::bt::Device>,
    bt_ready: bool,
    volume_quiet: PollClock,
    /// v0.5.5：邮件收信服务（工作线程），以及上次同步给它的账号/开关快照。
    mail: daozai::mail::Service,
    mail_sig: (bool, u32, Vec<daozai::mail::Account>),
    /// v0.5.6：日程（CalDAV）服务与上次下发的配置。
    cal: daozai::calendar::Service,
    cal_sig: (Vec<daozai::calendar::Config>, bool),
    weather: daozai::weather::Service,
    weather_sig: (bool, String, u32),
    /// 音频可视化服务：开启设置时才启动。
    audio: Option<daozai::audio::Service>,
    audio_visualizer: bool,
    quota: daozai::quota::Service,
    quota_sig: Option<(bool,Vec<daozai::quota::Account>)>,
    quota_at: PollClock,
    clipboard: daozai::clipboard::Service,
    clipboard_popup: daozai::clipboard_ui::Popup,
    /// 最近一次剪贴板历史快照，供命令面板搜索。
    clipboard_items: Vec<daozai::clipboard::Item>,
    clipboard_sig: (bool, usize, u32, bool),
    #[cfg(windows)]
    clipboard_prev: Option<windows::Win32::Foundation::HWND>,
    clipboard_transfer: Option<(String, bool)>,
    clipboard_status: String,
    /// 剪贴板图片 OCR 的后台结果。
    ocr_rx: Option<std::sync::mpsc::Receiver<Result<String, String>>>,
    /// 框选取字：整屏位图、目标显示器与全屏遮罩。
    capture_overlay: daozai::capture_ui::Overlay,
    capture_purpose: daozai::capture_ui::Purpose,
    /// 截图标注编辑器。
    annotate: daozai::annotate::Editor,
    /// 贴在桌面上的截图（置顶小窗）。
    pins: Vec<daozai::pin::Pin>,
    /// 通知历史面板与数据版本。
    notifications_popup: daozai::notifications_ui::Popup,
    notifications_rev: u64,
    capture_image: Option<daozai::ocr::Image>,
    capture_monitor: Option<daozai::region_capture::MonitorRect>,
    command_palette: daozai::command_palette::Panel,
    /// 系统页采样器与最近一次结果。
    perf: daozai::perf::Sampler,
    perf_at: PollClock,
    perf_value: daozai::perf::Performance,
    /// 当前已下发给系统的防睡眠状态。
    stay_awake: bool,
}

impl Shell {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let mut app = App::new(&cc.egui_ctx, Options::default());
        #[cfg(feature="record")] let quota_demo=std::env::var("DAOZAI_QUOTA_DEMO").is_ok();
        #[cfg(not(feature="record"))] let quota_demo=false;
        if !quota_demo{app.quota_initialize();}
        if std::env::args().any(|arg|arg=="--settings"){app.open_settings();}
        if std::env::args().any(|arg|arg=="--calendar"){app.open_calendar(None);}
        #[cfg(feature="record")]
        if std::env::var("DAOZAI_MAIL_DEMO").is_ok(){app.mail_demo();}
        #[cfg(feature="record")]
        if std::env::var("DAOZAI_CAL_DEMO").is_ok(){app.calendar_demo();}
        #[cfg(feature="record")]
        if let Ok(mode)=std::env::var("DAOZAI_QUOTA_DEMO"){app.quota_demo(&mode);}
        let media = smtc::shared();
        smtc::spawn_poller(media.clone(),cc.egui_ctx.clone());
        #[cfg(windows)]
        let autostart = daozai::tray::autostart_enabled();
        #[cfg(windows)]
        let tray = daozai::tray::Tray::start(daozai::tray::State {
            pet: app.settings.pet,
            minimal: app.settings.minimal,
            autostart,
        });
        #[cfg(windows)]
        let incoming=if app.settings.read_notifications {match daozai::system::connect(){Ok(rx)=>Some(rx),Err(e)=>{app.listener_status=format!("Windows 无法读取通知：{}",e.code());None}}}else{None};
        let initial_input=daozai::input::State::read();
        #[cfg(windows)] let system_state=daozai::system::Snapshot::read();
        #[cfg(windows)] let system_rx=daozai::system::observe(cc.egui_ctx.clone(),system_state.clone(),initial_input);
        let clipboard_enabled =
            app.settings.clipboard_enabled && !app.settings.clipboard_paused;
        let clipboard_limit = app.settings.clipboard_limit as usize;
        let clipboard_days = app.settings.clipboard_days;
        let clipboard_clean_links = app.settings.clipboard_clean_links;
        #[cfg(windows)]
        if app.settings.stay_awake {
            daozai::system::set_stay_awake(true);
        }
        let stay_awake = app.settings.stay_awake;
        let audio_visualizer = app.settings.audio_visualizer;
        let clipboard = daozai::clipboard::Service::start();
        clipboard.set_enabled(clipboard_enabled);
        clipboard.set_limits(clipboard_limit, clipboard_days);
        clipboard.set_clean_links(app.settings.clipboard_clean_links);
        Self {
            app,
            t0: Instant::now(),
            #[cfg(windows)]
            frame_at: Instant::now(),
            media,
            placed: false,
            win_x: 0.0,
            sys: (None, false, PollClock::default()),
            #[cfg(windows)]
            system_state,
            #[cfg(windows)] system_rx,
            fullscreen:false,
            input_state:daozai::input::Monitor::new(initial_input),
            #[cfg(windows)] input_keys:daozai::input::Shortcuts::start(),
            #[cfg(windows)] incoming,
            #[cfg(windows)] hidden:false,
            #[cfg(windows)]
            native: daozai::native::Window::new(cc),
            #[cfg(windows)]
            tray,
            #[cfg(windows)]
            autostart,
            #[cfg(windows)]
            bt_rx: Some(daozai::bt::spawn()),
            bt: Vec::new(),
            bt_ready: false,
            volume_quiet: PollClock::default(),
            mail: daozai::mail::Service::start(),
            mail_sig: (false, 0, Vec::new()),
            cal: daozai::calendar::Service::start(),
            cal_sig: (Vec::new(), true),
            weather: daozai::weather::Service::start(),
            weather_sig: (false, String::new(), 0),
            audio: if audio_visualizer { Some(daozai::audio::Service::start()) } else { None },
            audio_visualizer,
            quota: daozai::quota::Service::start(), quota_sig: None, quota_at: PollClock::default(),
            clipboard,
            clipboard_popup: daozai::clipboard_ui::Popup::default(),
            clipboard_items: Vec::new(),
            clipboard_sig: (clipboard_enabled, clipboard_limit, clipboard_days, clipboard_clean_links),
            #[cfg(windows)]
            clipboard_prev: None,
            clipboard_transfer: None,
            clipboard_status: String::new(),
            ocr_rx: None,
            capture_overlay: Default::default(),
            capture_purpose: Default::default(),
            annotate: Default::default(),
            pins: Vec::new(),
            notifications_popup: Default::default(),
            notifications_rev: 0,
            capture_image: None,
            capture_monitor: None,
            command_palette: daozai::command_palette::Panel::default(),
            perf: daozai::perf::Sampler::new(),
            perf_at: PollClock::default(),
            perf_value: Default::default(),
            stay_awake,
        }
    }

    #[cfg(windows)]
    fn poll_tray(&mut self, ctx: &egui::Context) {
        use daozai::tray::{self, Event};
        let events = self.tray.as_ref().map(|t| t.poll()).unwrap_or_default();
        for e in events {
            match e {
                Event::OpenSettings=>self.app.open_settings(),
                Event::OpenCalendar=>self.app.open_calendar(None),
                Event::OpenClipboard=>self.open_clipboard(ctx, false),
                Event::OpenCommandPalette=>self.open_command_palette(ctx),
                Event::OcrClipboard=>self.run_ocr(ctx),
                Event::CropOcr=>self.start_region_ocr(ctx),
                Event::OpenNotifications=>self.open_notifications(ctx),
                Event::Screenshot=>self.start_region_screenshot(ctx),
                Event::TogglePet => {
                    self.app.set_pet(!self.app.settings.pet);
                    self.app.say(if self.app.settings.pet { "吉祥物已打开" } else { "吉祥物已关闭" });
                }
                Event::ToggleMinimal => {
                    self.app.set_minimal(!self.app.settings.minimal);
                    self.app.say(if self.app.settings.minimal { "极简刘海已打开" } else { "极简刘海已关闭" });
                }
                Event::ToggleAutostart => {
                    let want = !self.autostart;
                    match tray::set_autostart(want) {
                        Ok(()) => {
                            self.autostart = want;
                            self.app.say(if want { "已设为开机启动" } else { "已取消开机启动" });
                        }
                        Err(e) => self.app.say(e),
                    }
                }
            }
        }
        if let Some(t) = &self.tray {
            t.sync(tray::State { pet: self.app.settings.pet, minimal: self.app.settings.minimal, autostart: self.autostart });
        }
    }

    fn open_clipboard(&mut self, ctx: &egui::Context, restore: bool) {
        self.annotate.close();
        self.close_notifications();
        self.command_palette.close();
        #[cfg(windows)]
        {
            let previous = if restore { daozai::native::external_foreground_window() } else { None };
            self.clipboard_prev = previous;
            self.clipboard_popup.set_target_label(previous.and_then(|hwnd| daozai::native::foreground_window_label(Some(hwnd))));
        }
        #[cfg(not(windows))]
        {
            let _ = restore;
            self.clipboard_popup.set_target_label(None);
        }
        let hotkey_error = self.clipboard_hotkey_error();
        self.clipboard_popup.open_state(
            self.app.settings.clipboard_enabled,
            self.app.settings.clipboard_paused,
            hotkey_error,
        );
        self.clipboard.refresh();
        ctx.request_repaint();
    }

    fn close_clipboard(&mut self, restore: bool) {
        self.clipboard_popup.close();
        #[cfg(windows)]
        {
            if restore {
                daozai::native::restore_foreground(self.clipboard_prev.take());
            }
        }
        #[cfg(not(windows))]
        let _ = restore;
    }

    fn clipboard_hotkey_error(&self) -> Option<String> {
        #[cfg(windows)]
        {
            self.native.as_ref().and_then(|native| native.hotkey_error.clone())
        }
        #[cfg(not(windows))]
        {
            None
        }
    }

    fn poll_clipboard(&mut self) {
        let settings = &self.app.settings;
        let signature = (
            settings.clipboard_enabled && !settings.clipboard_paused,
            settings.clipboard_limit as usize,
            settings.clipboard_days,
            settings.clipboard_clean_links,
        );
        if signature != self.clipboard_sig {
            self.clipboard.set_limits(signature.1, signature.2);
            self.clipboard.set_enabled(signature.0);
            self.clipboard.set_clean_links(signature.3);
            self.clipboard_sig = signature;
        }
        while let Some(event) = self.clipboard.try_recv() {
            match event {
                daozai::clipboard::Event::Changed(items) => {
                    self.clipboard_popup.set_items((*items).clone());
                    self.clipboard_items = (*items).clone();
                    if self.command_palette.is_open() {
                        let entries = self.palette_clipboard_entries();
                        self.command_palette.set_clipboard(entries);
                    }
                }
                daozai::clipboard::Event::Copied(_) => {
                    if let Some((id, paste)) = self.clipboard_transfer.take() {
                        if paste {
                            #[cfg(windows)] {
                                let target = self.clipboard_prev.take();
                                self.close_clipboard(false);
                                if let Err(error) = daozai::native::restore_and_paste(target) {
                                    self.app.say(format!("已复制，请手动粘贴：{error}"));
                                }
                            }
                            #[cfg(not(windows))]
                            {
                                let _ = id;
                                self.close_clipboard(false);
                                self.app.say("已复制，请手动粘贴");
                            }
                        } else {
                            let restore = self.clipboard_prev.is_some();
                            self.close_clipboard(restore);
                            let _ = id;
                        }
                    } else {
                        self.app.say("已复制");
                    }
                }
                daozai::clipboard::Event::CopyFailed(_, message) => {
                    self.clipboard_transfer = None;
                    self.clipboard_popup.set_error(message.clone());
                    self.app.say(message);
                }
                daozai::clipboard::Event::Status(message) => {
                    if !message.trim().is_empty() {
                        self.clipboard_popup.set_error(message.clone());
                    }
                    self.clipboard_status = message;
                }
            }
        }
        let hotkey_error = self.clipboard_hotkey_error();
        self.clipboard_status = hotkey_error.clone().unwrap_or_else(|| self.clipboard_status.clone());
        self.app.clipboard_status = self.clipboard_status.clone();
        self.clipboard_popup.sync(
            self.app.settings.clipboard_enabled,
            self.app.settings.clipboard_paused,
            hotkey_error,
        );
    }

    fn notification_status(&self) -> String {
        let base = if self.incoming.is_some() {
            if self.app.listener_status.trim().is_empty() {
                "正在监听 Windows 通知".to_string()
            } else {
                self.app.listener_status.clone()
            }
        } else {
            "未连接 Windows 通知 · 在「设置 → 通知」连接".to_string()
        };
        if self.app.settings.notifications {
            base
        } else {
            format!("岛上提醒已关闭 · {base}")
        }
    }

    fn open_notifications(&mut self, ctx: &egui::Context) {
        self.annotate.close();
        if self.capture_overlay.open {
            return;
        }
        if self.notifications_popup.is_open() {
            self.close_notifications();
            return;
        }
        self.close_clipboard(false);
        self.command_palette.close();
        self.app.notifications_mark_seen();
        self.notifications_popup.open(
            self.app.notifications_snapshot(),
            self.app.notifications_unread(),
            self.notification_status(),
        );
        self.notifications_rev = self.app.notifications_revision();
        ctx.request_repaint();
    }

    fn palette_clipboard_entries(&self) -> Vec<daozai::command_palette::ClipboardItem> {
        self.clipboard_items
            .iter()
            .map(|item| daozai::command_palette::ClipboardItem { id: item.id.clone(), text: item.text.clone() })
            .collect()
    }

    fn close_notifications(&mut self) {
        self.notifications_popup.close();
    }

    fn show_notifications(&mut self, ctx: &egui::Context) {
        if !self.notifications_popup.is_open() {
            return;
        }
        let revision = self.app.notifications_revision();
        if revision != self.notifications_rev {
            self.notifications_rev = revision;
            self.notifications_popup.sync(
                self.app.notifications_snapshot(),
                self.app.notifications_unread(),
            );
        }
        let dark = daozai::settings_theme::system_dark(ctx);
        match self.notifications_popup.show(ctx, dark) {
            daozai::notifications_ui::Action::None => {}
            daozai::notifications_ui::Action::Close => self.close_notifications(),
            daozai::notifications_ui::Action::Copy(text) => {
                ctx.copy_text(text);
                self.app.say("已复制通知内容");
            }
            daozai::notifications_ui::Action::Delete(id) => {
                self.app.notifications_delete(&id);
                self.notifications_popup.sync(
                    self.app.notifications_snapshot(),
                    self.app.notifications_unread(),
                );
            }
            daozai::notifications_ui::Action::Clear => {
                self.app.notifications_clear();
                self.notifications_popup.sync(Vec::new(), 0);
                self.app.say("已清空通知历史");
            }
        }
    }

    fn show_clipboard(&mut self, ctx: &egui::Context) {
        // 快捷面板跟随系统深浅色，和设置页保持同一套观感。
        let dark = daozai::settings_theme::system_dark(ctx);
        match self.clipboard_popup.show(ctx, dark) {
            daozai::clipboard_ui::Action::None => {}
            daozai::clipboard_ui::Action::Paste(id) => self.begin_clipboard_transfer(id, true),
            daozai::clipboard_ui::Action::CopyOnly(id) => self.begin_clipboard_transfer(id, false),
            daozai::clipboard_ui::Action::Delete(id) => self.clipboard.delete(&id),
            daozai::clipboard_ui::Action::Restore(item) => self.clipboard.restore(item),
            daozai::clipboard_ui::Action::Pin(id, pinned) => self.clipboard.pin(&id, pinned),
            daozai::clipboard_ui::Action::Enable => {
                self.app.settings.clipboard_enabled = true;
                self.app.settings.save();
            }
            daozai::clipboard_ui::Action::Resume => {
                self.app.settings.clipboard_paused = false;
                self.app.settings.save();
            }
            daozai::clipboard_ui::Action::SetPaused(paused) => {
                self.app.settings.clipboard_paused = paused;
                self.app.settings.save();
                self.clipboard_popup.sync(
                    self.app.settings.clipboard_enabled,
                    paused,
                    self.clipboard_hotkey_error(),
                );
            }
            daozai::clipboard_ui::Action::Clear => self.clipboard.clear(),
            daozai::clipboard_ui::Action::OcrClipboard => {
                self.close_clipboard(false);
                self.run_ocr(ctx);
            }
            daozai::clipboard_ui::Action::CropOcr => {
                self.close_clipboard(false);
                self.start_region_ocr(ctx);
            }
            daozai::clipboard_ui::Action::OpenSettings => {
                self.close_clipboard(false);
                self.app.open_settings();
                ctx.request_repaint();
            }
            daozai::clipboard_ui::Action::Close { restore } => self.close_clipboard(restore),
        }
    }

    fn begin_clipboard_transfer(&mut self, id: String, paste: bool) {
        if self.clipboard_transfer.is_some() {
            return;
        }
        self.clipboard_popup.clear_error();
        self.clipboard_transfer = Some((id.clone(), paste));
        self.clipboard.copy(&id);
    }

    /// 打开框选取字层：先抓整屏位图，再让用户框选，避免遮罩被拍进图片。
    fn start_region_ocr(&mut self, ctx: &egui::Context) {
        self.start_region_capture(ctx, daozai::capture_ui::Purpose::Ocr);
    }

    /// 打开框选截图层：框选后复制到剪贴板并保存 PNG。
    fn start_region_screenshot(&mut self, ctx: &egui::Context) {
        self.start_region_capture(ctx, daozai::capture_ui::Purpose::Screenshot);
    }

    fn start_region_capture(&mut self, ctx: &egui::Context, purpose: daozai::capture_ui::Purpose) {
        self.annotate.close();
        if self.capture_overlay.open {
            return;
        }
        if purpose == daozai::capture_ui::Purpose::Ocr && self.ocr_rx.is_some() {
            self.app.say("正在识别上一张图片，请稍候");
            return;
        }
        // 框选层全屏覆盖，先收起其它浮层，避免遮挡屏幕。
        if self.clipboard_popup.open {
            self.close_clipboard(false);
        }
        if self.command_palette.is_open() {
            self.command_palette.close();
        }
        if self.notifications_popup.is_open() {
            self.close_notifications();
        }
        let Some(monitor) = daozai::region_capture::monitor_at_cursor() else {
            self.app.say("没有找到可用显示器");
            return;
        };
        match daozai::region_capture::capture_region(monitor) {
            Ok(image) => {
                self.capture_image = Some(image);
                self.capture_monitor = Some(monitor);
                self.capture_purpose = purpose;
                self.capture_overlay.open(monitor, purpose);
                ctx.request_repaint();
            }
            Err(error) => self.app.say(format!("无法截取屏幕：{error}")),
        }
    }

    fn show_capture_overlay(&mut self, ctx: &egui::Context) {
        use daozai::capture_ui::Action as CaptureAction;
        if !self.capture_overlay.open {
            return;
        }
        let ppp = ctx.pixels_per_point();
        match self.capture_overlay.show(ctx, ppp) {
            CaptureAction::None => {}
            CaptureAction::Cancel => self.close_capture_overlay(),
            CaptureAction::Capture { x, y, width, height } => {
                self.capture_overlay.close();
                let purpose = self.capture_purpose;
                let Some(image) = self.capture_image.take() else {
                    self.capture_monitor = None;
                    return;
                };
                self.capture_monitor = None;
                match daozai::region_capture::crop(&image, x, y, width, height) {
                    Ok(cropped) => match purpose {
                        daozai::capture_ui::Purpose::Ocr => self.start_ocr_image(ctx, cropped),
                        daozai::capture_ui::Purpose::Screenshot => self.finish_screenshot(ctx, cropped),
                    },
                    Err(error) => self.app.say(error),
                }
            }
        }
    }

    /// 截图完成：进入标注编辑器，复制/保存由编辑器决定。
    fn finish_screenshot(&mut self, ctx: &egui::Context, image: daozai::ocr::Image) {
        self.annotate.open(ctx, image);
        ctx.request_repaint();
    }

    fn show_pins(&mut self, ctx: &egui::Context) {
        if self.pins.is_empty() {
            return;
        }
        let dark = daozai::settings_theme::system_dark(ctx);
        self.pins.retain_mut(|pin| !pin.show(ctx, dark));
    }

    fn show_annotate(&mut self, ctx: &egui::Context) {
        if !self.annotate.is_open() {
            return;
        }
        use daozai::annotate::Action as AnnotateAction;
        let dark = daozai::settings_theme::system_dark(ctx);
        match self.annotate.show(ctx, dark) {
            AnnotateAction::None => {}
            AnnotateAction::Close => self.annotate.close(),
            AnnotateAction::Copy(image) => {
                #[cfg(windows)]
                daozai::screenshot::copy_to_clipboard(&image);
                self.app.say("已复制标注截图");
                self.annotate.close();
            }
            AnnotateAction::Save(image) => {
                match daozai::screenshot::save_png(&image) {
                    Ok(path) => self.app.say(format!("标注截图已保存到 {}", path.display())),
                    Err(error) => self.app.say(format!("保存标注截图失败：{error}")),
                }
                self.annotate.close();
            }
            AnnotateAction::Pin(image) => {
                self.annotate.close();
                if self.pins.len() >= 6 {
                    self.app.say("最多同时贴 6 张图，先右键关闭一些吧");
                } else {
                    self.pins.push(daozai::pin::Pin::new(image, ctx.pixels_per_point()));
                    self.app.say("已贴到桌面，右键或双击关闭");
                }
            }
        }
    }


    fn close_capture_overlay(&mut self) {
        self.capture_overlay.close();
        self.capture_image = None;
        self.capture_monitor = None;
    }

    /// 对一段已经截好的 RGBA 图像跑 OCR（框选取字）。
    fn start_ocr_image(&mut self, ctx: &egui::Context, image: daozai::ocr::Image) {
        if self.ocr_rx.is_some() {
            self.app.say("正在识别上一张图片，请稍候");
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        self.ocr_rx = Some(rx);
        self.app.say("正在识别选区文字…");
        ctx.request_repaint();
        std::thread::Builder::new()
            .name("daozai-ocr-region".into())
            .spawn(move || {
                let _ = tx.send(daozai::ocr::recognize_rgba(&image.rgba, image.width, image.height));
            })
            .ok();
    }

    /// 识别剪贴板图片里的文字：后台线程跑 Windows OCR，主线程只收结果。
    fn run_ocr(&mut self, ctx: &egui::Context) {
        if self.ocr_rx.is_some() {
            self.app.say("正在识别上一张图片，请稍候");
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        self.ocr_rx = Some(rx);
        self.app.say("正在识别剪贴板图片…");
        ctx.request_repaint();
        std::thread::Builder::new()
            .name("daozai-ocr".into())
            .spawn(move || {
                let _ = tx.send(daozai::ocr::recognize_clipboard());
            })
            .ok();
    }

    fn poll_ocr(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.ocr_rx else { return };
        let mut finished = None;
        match rx.try_recv() {
            Ok(result) => finished = Some(result),
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                finished = Some(Err("识别线程已退出".into()))
            }
        }
        let Some(result) = finished else { return };
        self.ocr_rx = None;
        match result {
            Ok(text) => {
                let count = text.chars().filter(|ch| !ch.is_whitespace()).count();
                if count == 0 {
                    self.app.say("没有识别到文字");
                    return;
                }
                ctx.copy_text(text.clone());
                self.clipboard.insert(text);
                self.app.say(format!("已识别 {count} 个字，已复制到剪贴板"));
            }
            Err(message) => self.app.say(message),
        }
    }

    fn open_command_palette(&mut self, ctx: &egui::Context) {
        self.annotate.close();
        self.close_notifications();
        // 两个浮层互斥：同时打开会互相抢焦点。
        if self.clipboard_popup.open {
            self.close_clipboard(false);
        }
        let mut snapshot = self.app.palette_snapshot();
        snapshot.pins = self.pins.len();
        self.command_palette.open(&snapshot);
        let entries = self.palette_clipboard_entries();
        self.command_palette.set_clipboard(entries);
        #[cfg(windows)]
        let error = self.native.as_ref().and_then(|native| native.command_hotkey_error.clone());
        #[cfg(not(windows))]
        let error = None;
        self.command_palette.set_hotkey_error(error);
        ctx.request_repaint();
    }

    fn close_command_palette(&mut self) {
        self.command_palette.close();
        self.command_palette.set_clipboard(Vec::new());
    }

    fn show_command_palette(&mut self, ctx: &egui::Context) {
        use daozai::command_palette::Action as PaletteAction;
        if !self.command_palette.is_open() {
            return;
        }
        // 命令面板与剪贴板共用同一套浮层外观，跟随系统深浅色。
        let dark = daozai::settings_theme::system_dark(ctx);
        match self.command_palette.show(ctx, dark) {
            PaletteAction::None => {}
            PaletteAction::Close => self.close_command_palette(),
            PaletteAction::OpenSettings(tab) => {
                self.close_command_palette();
                self.app.open_settings_tab(tab);
                ctx.request_repaint();
            }
            PaletteAction::OpenPage(page) => {
                self.close_command_palette();
                self.app.show_page(page);
            }
            PaletteAction::OpenCalendar => {
                self.close_command_palette();
                self.app.open_calendar(None);
            }
            PaletteAction::OpenClipboard => {
                self.close_command_palette();
                self.open_clipboard(ctx, true);
            }
            PaletteAction::LaunchApp(path) => {
                self.close_command_palette();
                if let Err(error) = open_path(&path) {
                    self.app.say(format!("无法启动应用：{error}"));
                }
            }
            PaletteAction::CopyText(text) => {
                self.close_command_palette();
                ctx.copy_text(text);
                self.app.say("已复制计算结果");
            }
            PaletteAction::SetClipboardPaused(paused) => {
                self.close_command_palette();
                self.app.settings.clipboard_paused = paused;
                self.app.settings.save();
                self.app.say(if paused { "剪贴板记录已暂停" } else { "剪贴板记录已恢复" });
            }
            PaletteAction::SetClipboardEnabled(enabled) => {
                self.close_command_palette();
                self.app.settings.clipboard_enabled = enabled;
                if enabled {
                    self.app.settings.clipboard_paused = false;
                }
                self.app.settings.save();
                self.app.say(if enabled { "已开启剪贴板历史" } else { "已关闭剪贴板历史" });
            }
            PaletteAction::ToggleFocus => {
                self.close_command_palette();
                self.app.toggle_focus();
            }
            PaletteAction::OcrClipboard => {
                self.close_command_palette();
                self.run_ocr(ctx);
            }
            PaletteAction::CropOcr => {
                self.close_command_palette();
                self.start_region_ocr(ctx);
            }
            PaletteAction::StartCountdown(minutes) => {
                self.close_command_palette();
                self.app.start_countdown(minutes);
                self.app.say(format!("开始 {minutes} 分钟倒计时"));
            }
            PaletteAction::ToggleTimer => {
                self.close_command_palette();
                let running = self.app.toggle_timer();
                self.app.say(if running { "计时继续" } else { "计时已暂停" });
            }
            PaletteAction::ResetTimer => {
                self.close_command_palette();
                self.app.reset_timer();
                self.app.say("计时已重置");
            }
            PaletteAction::RefreshWeather => {
                self.close_command_palette();
                self.app.request_weather_refresh();
                self.app.say("正在刷新天气…");
            }
            PaletteAction::OpenNotifications => {
                self.close_command_palette();
                self.open_notifications(ctx);
            }
            PaletteAction::ScreenshotRegion => {
                self.close_command_palette();
                self.start_region_screenshot(ctx);
            }
            PaletteAction::CopyClipboard(id) => {
                self.close_command_palette();
                self.clipboard.copy(&id);
            }
            PaletteAction::ClosePins => {
                self.close_command_palette();
                let count = self.pins.len();
                self.pins.clear();
                self.app.say(format!("已关闭 {count} 张贴图"));
            }
            PaletteAction::SetStayAwake(awake) => {
                self.close_command_palette();
                self.app.settings.stay_awake = awake;
                self.app.settings.save();
                self.app.say(if awake { "已开启防睡眠" } else { "已关闭防睡眠" });
            }
        }
    }

    fn env(&mut self, ctx: &egui::Context) -> Env {
        let (clock, today, date) = local_now();
        // 电量变化很慢，每 5 秒读一次就够
        if self.sys.2.ready(Duration::from_secs(5)) {
            let (b, c) = read_power();
            if self.app.settings.show_battery && self.sys.0.is_some() && self.sys.1!=c && self.app.settings.system_hud {self.app.alert(if c {"电源已连接"}else{"已切换电池供电"},b.map(|v|format!("当前电量 {v}%")).unwrap_or_else(||"桌面电源状态已更新".into()),5,false);}
            if self.app.settings.show_battery && b.is_some_and(|v|v<=20) && !c && self.sys.0.is_some_and(|v|v>20) {self.app.alert("电量偏低",format!("剩余 {}% · 请连接电源",b.unwrap()),5,true);}
            self.sys = (b, c, PollClock::now());
        }
        let ppp = ctx.pixels_per_point();
        let origin = ctx.input(|i| i.viewport().outer_rect).map(|r| r.min).unwrap_or(Pos2::new(self.win_x, 0.0));
        let cursor = cursor_logical(ppp).map(|(x, y)| Pos2::new(x - origin.x, y - origin.y));
        // 系统页：按秒采样，CPU 百分比需要两次累计值之差。
        if self.perf_at.ready(Duration::from_millis(1000)) {
            self.perf_value = self.perf.sample();
            self.perf_at = PollClock::now();
        }
        Env {
            t: self.t0.elapsed().as_secs_f64(),
            clock,
            today,
            date,
            battery: self.sys.0,
            charging: self.sys.1,
            network: Some(self.system_state.network),
            cursor,
            idle_seconds: {
                #[cfg(windows)] {Some(daozai::system::idle_seconds())}
                #[cfg(not(windows))] {None}
            },
            browser_at_top: {
                #[cfg(windows)] {if self.app.settings.browser_minimal {self.native.as_mut().and_then(|native|native.browser_at_top())}else{None}}
                #[cfg(not(windows))] {None}
            },
            media: self.media.lock().map(|m| m.clone()).unwrap_or_default(),
            volume: self.system_state.volume,
            bt: self.bt.clone(),
            bt_ready: self.bt_ready,
            cpu: self.perf_value.cpu,
            memory: self.perf_value.memory,
            network_rates: self.perf_value.network,
            disk: self.perf_value.disk,
        }
    }
}

impl eframe::App for Shell {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0; 4]
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.app.settings.stay_awake != self.stay_awake {
            self.stay_awake = self.app.settings.stay_awake;
            #[cfg(windows)]
            daozai::system::set_stay_awake(self.stay_awake);
            self.app.say(if self.stay_awake { "已开启防睡眠" } else { "已关闭防睡眠" });
        }
        #[cfg(windows)]
        {
            // 用帧间隔限制刷新，绕过部分 Intel 驱动在垂直同步中的忙等待。
            let delay=Duration::from_secs_f64(1.0/60.0).saturating_sub(self.frame_at.elapsed());
            if !delay.is_zero(){std::thread::sleep(delay);}
            self.frame_at=Instant::now();
        }
        #[cfg(windows)]
        self.poll_tray(ctx);
        #[cfg(windows)]
        {
            // 一次只消费一次热键；框选层打开期间不再响应其他浮层热键。
            let screen_ocr = daozai::native::take_crop_hotkey();
            let screen_shot = daozai::native::take_screenshot_hotkey();
            let clipboard_hotkey = daozai::native::take_clipboard_hotkey();
            let command_hotkey = daozai::native::take_command_hotkey();
            let image_ocr = daozai::native::take_ocr_hotkey();
            let notification_hotkey = daozai::native::take_notification_hotkey();
            if screen_shot {
                self.start_region_screenshot(ctx);
            } else if screen_ocr {
                self.start_region_ocr(ctx);
            } else if !self.capture_overlay.open && !self.annotate.is_open() {
                if clipboard_hotkey {
                    if self.clipboard_popup.open {
                        self.close_clipboard(true);
                    } else {
                        self.open_clipboard(ctx, true);
                    }
                }
                if command_hotkey {
                    if self.command_palette.is_open() {
                        self.command_palette.close();
                    } else {
                        self.open_command_palette(ctx);
                    }
                }
                if image_ocr {
                    self.run_ocr(ctx);
                }
                if notification_hotkey {
                    self.open_notifications(ctx);
                }
            }
        }
        self.poll_clipboard();
        self.poll_ocr(ctx);
        #[cfg(windows)]
        if daozai::native::take_foreground_change() {
            // 切到别的程序：收起，并在光标离开前不再自动展开
            self.app.foreground_changed();
        }
        if !self.placed {
            if let Some(mon) = ctx.input(|i| i.viewport().monitor_size) {
                self.placed = true;
                self.win_x = ((mon.x - app::WIN_W) / 2.0).round();
                ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(egui::pos2(self.win_x, 0.0)));
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            }
        }
        #[cfg(windows)]
        {
            while let Ok(observation)=self.system_rx.try_recv(){
                let next=observation.input;
                let manual=next.is_some_and(|state|self.input_keys.as_ref().is_some_and(|keys|keys.allows(state.context)));
                if let Some(label)=self.input_state.observe(next,manual){
                    if let Some(keys)=&self.input_keys{keys.clear();}
                    if self.app.settings.system_hud{self.app.alert("键盘输入语言",label,5,false);}
                }
                let new=observation.system;
                // 切换输出设备时，系统会恢复该设备的音量；等待它稳定后再提示调节。
                if new.audio_endpoint!=self.system_state.audio_endpoint {self.volume_quiet=PollClock::now();}
                if self.app.settings.system_hud {
                    for (title,body) in new.changes(&self.system_state) {
                        // 拖动音量条刚改过音量时，系统回声不再单独弹一条。
                        if title=="系统音量" && !self.volume_quiet.ready(Duration::from_millis(900)) { continue; }
                        self.app.alert(title,body,5,false);
                    }
                }
                self.system_state=new;
                self.fullscreen=observation.fullscreen;
            }
            self.hidden=self.app.settings.hide_fullscreen&&self.fullscreen;
            if let Some(rx)=&self.bt_rx {
                while let Ok(found)=rx.try_recv(){ self.bt=found; self.bt_ready=true; }
            }
            if let Some(rx)=&self.incoming {
                while let Ok(event)=rx.try_recv(){match event {daozai::system::Incoming::Status(s)=>self.app.listener_status=s,daozai::system::Incoming::Message(a,id,b)=>self.app.receive_notification(a,id,b)}}
            }
        }
        let env = self.env(ctx);
        // v0.5.5 邮件：开关 / 间隔 / 账号清单变了就重新对齐收信线程，再把事件交给界面。
        let sig=(self.app.settings.mail_enabled,self.app.settings.mail_interval,self.app.mail_accounts().to_vec());
        if sig!=self.mail_sig{
            let accounts:&[daozai::mail::Account]=if sig.0{ &sig.2 }else{ &[] };
            self.mail.sync(accounts,sig.1);
            self.mail_sig=sig;
        }
        while let Some(ev)=self.mail.try_recv(){self.app.mail_event(ev);}
        // 日程连接或自动同步开关变化时下发给工作线程。
        // 录制/截图演示用的是假账号，别让真服务去连（会立刻失败、把状态改成红色）。
        #[cfg(feature="record")]
        let cal_demo=std::env::var("DAOZAI_CAL_DEMO").is_ok();
        #[cfg(not(feature="record"))]
        let cal_demo=false;
        let cal=if cal_demo{Vec::new()}else{self.app.calendar_configs()};
        let auto_sync=self.app.settings.calendar_auto_sync;
        if (&cal,auto_sync)!=(&self.cal_sig.0,self.cal_sig.1){
            self.cal.configure(cal.clone(),auto_sync);
            self.cal_sig=(cal,auto_sync);
        }
        while let Some(ev)=self.cal.try_recv(){self.app.calendar_event(ev);}
        let weather_cfg=(self.app.settings.weather_enabled,self.app.settings.weather_city.clone(),self.app.settings.weather_refresh_minutes);
        if weather_cfg!=self.weather_sig {
            self.weather.configure(weather_cfg.0,weather_cfg.1.clone(),weather_cfg.2);
            self.weather_sig=weather_cfg;
        }
        while let Some(event)=self.weather.try_recv(){self.app.weather_event(event);}
        if self.app.settings.audio_visualizer != self.audio_visualizer {
            self.audio_visualizer = self.app.settings.audio_visualizer;
            self.audio = if self.audio_visualizer { Some(daozai::audio::Service::start()) } else { None };
        }
        if let Some(service) = self.audio.as_ref() {
            while let Some(event) = service.try_recv() {
                match event {
                    daozai::audio::Event::Spectrum(spectrum) => self.app.audio_spectrum(spectrum),
                    daozai::audio::Event::Unavailable(message) => self.app.say(format!("音频可视化不可用：{message}")),
                }
            }
        }
        #[cfg(feature="record")] let quota_demo=std::env::var("DAOZAI_QUOTA_DEMO").is_ok();
        #[cfg(not(feature="record"))] let quota_demo=false;
        if !quota_demo {
            let config=self.app.quota_config();let sig=(config.enabled,config.accounts.iter().cloned().map(|mut a|{a.alias.clear();a.pinned=false;a}).collect::<Vec<_>>());
            if self.quota_sig.as_ref()!=Some(&sig) || self.quota_at.ready(Duration::from_secs(180)){
                let accounts:Vec<_>=sig.1.iter().filter(|a|sig.0 && a.enabled).cloned().collect();
                if self.quota.refresh(accounts.clone()){self.app.quota_pending(&accounts);self.quota_sig=Some(sig);self.quota_at=PollClock::now();}
            }
            while let Some(ev)=self.quota.try_recv(){self.app.quota_event(ev);}
        }
        let mut out = None;
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(Color32::TRANSPARENT))
            .show(ctx, |ui| {
                let win = egui::Rect::from_min_size(ui.max_rect().min,egui::vec2(app::WIN_W,app::WIN_H));
                out = Some(self.app.frame(ui, win, &env));
            });
        let Some(out) = out else { return };

        #[cfg(windows)]
        if let Some(native) = &mut self.native {
            let r = |r: egui::Rect| (r.left(), r.top(), r.width(), r.height());
            // 保留主窗口的事件循环，避免全屏隐藏后设置窗口无法关闭或重新打开。
            let region=if self.hidden {(0.0,0.0,0.0,0.0)} else {r(out.region)};
            let hit=if self.hidden {(0.0,0.0,0.0,0.0)} else {r(out.hit)};
            native.update(region, hit, if self.hidden{None}else{out.notice_hit.map(r)},if self.hidden{None}else{out.file_drop_zone.map(r)},ctx.pixels_per_point(), out.editing, !self.hidden && out.expanded);
        }
        for cmd in out.cmds {
            match cmd {
                Cmd::Media(t) => {
                    let source=self.media.lock().map(|media|media.source.clone()).unwrap_or_default();
                    std::thread::spawn(move || smtc::control(t,&source));
                }
                Cmd::Open(p) => {if let Err(e)=open_path(&p){self.app.say(e);}},
                Cmd::OpenUrl(url)=>{
                    if let Some(url)=daozai::mail::web_url(&url) {
                        if open_path(std::path::Path::new(&url)).is_err(){self.app.say("无法打开默认浏览器，请检查 Windows 默认应用设置");}
                    }
                },
                Cmd::AddFiles=>{
                    #[cfg(windows)] match daozai::system::pick_files(){Ok(files)=>if !files.is_empty(){self.app.on_drop(files);self.app.show_page(3);},Err(e)=>self.app.say(format!("无法选择文件：{}",e.code()))}
                },
                Cmd::DragFile(p)=>{
                    #[cfg(windows)] if let Err(e)=daozai::system::drag_file(&p){self.app.say(format!("无法拖出文件：{}",e.code()));}
                    ctx.input_mut(|i| i.pointer=Default::default());
                },
                Cmd::DisconnectNotifications=>{#[cfg(windows)]{self.incoming=None;self.app.settings.read_notifications=false;self.app.settings.save();self.app.listener_status="已断开，不再读取 Windows 通知".into();}},
                Cmd::Reveal(p)=>{let _=std::process::Command::new("explorer.exe").arg(format!("/select,{}",p.display())).spawn();},
                Cmd::QuotaChooseFile=>{
                    #[cfg(windows)] match daozai::system::pick_files_for("选择 AI 账号的登录 JSON 文件",false){Ok(files)=>if let Some(path)=files.into_iter().next(){self.app.quota_set_source(path);},Err(_)=>self.app.say("无法选择登录文件")}
                },
                Cmd::QuotaRefresh=>{
                    if self.quota_at.ready(Duration::from_secs(15)) {
                        let config=self.app.quota_config();let accounts:Vec<_>=config.accounts.iter().filter(|a|config.enabled && a.enabled).cloned().collect();
                        if self.quota.refresh(accounts.clone()){self.app.quota_pending(&accounts);self.quota_at=PollClock::now();}
                    }else{self.app.say("请稍后再刷新");}
                },
                Cmd::QuotaConnected=>{self.quota_sig=None;},
                Cmd::ToggleAutostart=>{
                    #[cfg(windows)] {let want=!self.autostart;match daozai::tray::set_autostart(want){Ok(())=>self.autostart=want,Err(e)=>self.app.say(e)}}
                },
                Cmd::SetVolume(v)=>{
                    #[cfg(windows)] daozai::system::set_volume(v);
                    self.volume_quiet=PollClock::now();
                },
                Cmd::ToggleMute=>{
                    #[cfg(windows)] daozai::system::toggle_mute();
                    self.volume_quiet=PollClock::now();
                },
                Cmd::Copy(text)=>{ctx.copy_text(text);self.app.say("已复制");},
                Cmd::Notify(title,body)=>{
                    #[cfg(windows)] if let Some(tray)=&self.tray{tray.notify(&title,&body,self.app.settings.sound);}
                },
                Cmd::RequestNotifications=>{
                    #[cfg(windows)] {self.app.settings.read_notifications=true;self.app.settings.save();self.incoming=None;match daozai::system::connect(){Ok(rx)=>{self.incoming=Some(rx);self.app.listener_status="正在请求 Windows 通知权限…".into();},Err(e)=>self.app.listener_status=format!("Windows 未允许读取通知：{}。需要相应的系统权限或应用身份。",e.code())}}
                },
                Cmd::MailSync=>{
                    let enabled=self.app.settings.mail_enabled;
                    let accounts=if enabled{self.app.mail_accounts().to_vec()}else{Vec::new()};
                    self.mail.sync(&accounts,self.app.settings.mail_interval);
                    self.mail_sig=(enabled,self.app.settings.mail_interval,self.app.mail_accounts().to_vec());
                },
                Cmd::MailTest(account,password)=>self.mail.test(account,password),
                Cmd::CalendarTest(cfg,password)=>self.cal.test(cfg,password),
                Cmd::CalendarSync=>self.cal.sync_now(),
                Cmd::CalendarMonth(month)=>self.cal.view_month(month),
                Cmd::CalendarForget=>{
                    if let Err(error)=daozai::calendar::forget_config(){self.app.say(&error);continue;}
                    if let Some(old)=self.app.forget_calendar(){daozai::calendar::secrets::delete(&old);}
                    let configs=self.app.calendar_configs();
                    self.cal.configure(configs.clone(),self.app.settings.calendar_auto_sync);
                    self.cal_sig=(configs,self.app.settings.calendar_auto_sync);
                },
                Cmd::Focus => ctx.send_viewport_cmd(egui::ViewportCommand::Focus),
                Cmd::ClipboardClear => self.clipboard.clear(),
                Cmd::WeatherRefresh => self.weather.refresh(),
                Cmd::OpenNotifications => self.open_notifications(ctx),
            }
        }


        #[cfg(windows)] let autostart = self.autostart;
        #[cfg(not(windows))] let autostart = false;
        self.app.settings_ui(ctx, autostart);
        self.app.mail_reader_ui(ctx);
        self.show_clipboard(ctx);
        self.show_command_palette(ctx);
        self.show_notifications(ctx);
        self.show_capture_overlay(ctx);
        self.show_annotate(ctx);
        self.show_pins(ctx);
        #[cfg(windows)] let repaint_after=if self.hidden{Duration::from_secs(1)}else{out.repaint_after};
        #[cfg(not(windows))] let repaint_after=out.repaint_after;
        let repaint_after = if self.clipboard_popup.open || self.command_palette.is_open() || self.capture_overlay.open || self.notifications_popup.is_open() || self.annotate.is_open() || !self.pins.is_empty() {
            Duration::from_millis(16)
        } else {
            repaint_after
        };
        ctx.request_repaint_after(repaint_after);
    }
}

// ---------------- 系统信息 ----------------

#[cfg(windows)]
fn local_now() -> (String, String, String) {
    use windows::Win32::System::SystemInformation::GetLocalTime;
    let st = unsafe { GetLocalTime() };
    let wd = ["周日", "周一", "周二", "周三", "周四", "周五", "周六"];
    (
        format!("{:02}:{:02}:{:02}", st.wHour, st.wMinute, st.wSecond),
        format!("{:04}-{:02}-{:02}", st.wYear, st.wMonth, st.wDay),
        format!("{}月{}日 {}", st.wMonth, st.wDay, wd[(st.wDayOfWeek as usize).min(6)]),
    )
}

#[cfg(not(windows))]
fn local_now() -> (String, String, String) {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let (h, m, s) = ((secs / 3600) % 24, (secs / 60) % 60, secs % 60);
    (format!("{h:02}:{m:02}:{s:02}"), "1970-01-01".into(), "1月1日 周四".into())
}

#[cfg(windows)]
fn read_power() -> (Option<u8>, bool) {
    use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
    let mut st = SYSTEM_POWER_STATUS::default();
    if unsafe { GetSystemPowerStatus(&mut st) }.is_ok() {
        let pct = if st.BatteryLifePercent == 255 { None } else { Some(st.BatteryLifePercent) };
        (pct, st.ACLineStatus == 1)
    } else {
        (None, false)
    }
}

#[cfg(not(windows))]
fn read_power() -> (Option<u8>, bool) {
    (None, true)
}

#[cfg(windows)]
fn cursor_logical(ppp: f32) -> Option<(f32, f32)> {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;
    let mut p = POINT::default();
    unsafe { GetCursorPos(&mut p) }.ok()?;
    Some((p.x as f32 / ppp, p.y as f32 / ppp))
}

#[cfg(not(windows))]
fn cursor_logical(_ppp: f32) -> Option<(f32, f32)> {
    None
}

#[cfg(windows)]
fn open_path(p: &std::path::Path)->Result<(),String> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let mut wide: Vec<u16> = p.as_os_str().encode_wide().collect();
    wide.push(0);
    let op: Vec<u16> = "open\0".encode_utf16().collect();
    unsafe {
        let result=ShellExecuteW(None, PCWSTR(op.as_ptr()), PCWSTR(wide.as_ptr()), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
        if result.0 as usize<=32 {return Err("无法打开，请检查文件位置和默认应用".into())}
    }
    Ok(())
}

#[cfg(not(windows))]
fn open_path(_p: &std::path::Path)->Result<(),String> {Ok(())}

fn main() -> std::process::ExitCode {
    diagnostics::install();
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            diagnostics::report("岛仔启动或运行失败。", &error.to_string());
            std::process::ExitCode::FAILURE
        }
    }
}

fn run() -> eframe::Result<()> {
    // 单实例（与原版同名互斥体）
    #[cfg(windows)]
    if !daozai::tray::single_instance() {
        if std::env::args().any(|arg|arg=="--settings"){daozai::tray::open_settings();}
        if std::env::args().any(|arg|arg=="--calendar"){daozai::tray::open_calendar();}
        return Ok(());
    }
    let viewport = egui::ViewportBuilder::default()
        .with_title("岛仔").with_icon(daozai::theme::app_icon())
        .with_inner_size([app::CANVAS_W, app::WIN_H])
        .with_decorations(false)
        .with_transparent(true)
        .with_active(false)
        .with_resizable(false)
        .with_taskbar(false)
        .with_always_on_top()
        .with_visible(false);
    let opts = eframe::NativeOptions { viewport, vsync: !cfg!(windows), ..Default::default() };
    eframe::run_native("岛仔", opts, Box::new(|cc| Ok(Box::new(Shell::new(cc)))))
}
