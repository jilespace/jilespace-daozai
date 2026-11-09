//! Windows 的非激活窗口、形状区域与命中测试。
//!
//! 对齐原版实测（`BUILD-ORDER-v1.md` §1/§5）：
//! - `WS_POPUP` + `WS_EX_TOPMOST|TOOLWINDOW|NOREDIRECTIONBITMAP|NOACTIVATE`；
//! - `SetWindowRgn` 用三种矩形：收起 179×34@(230,0)、展开 512×186@(64,0)、
//!   极简收起 112×14@(264,0)，坐标是 640×190 逻辑点；
//! - 命中区与 region 分开：收起 143×22@(253,5)、展开 416×132@(110,5)，
//!   命中区外 `HTTRANSPARENT`，真实点击落给下层窗口；
//! - 前台窗口切走（`EVENT_SYSTEM_FOREGROUND`，且不是本进程）时通知主线程收起；
//! - 原版没有可用的 Esc 收起入口（§2.5），这里也不注册键盘钩子。

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;
use crate::poll_clock::PollClock;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{CreateRectRgn, DeleteObject, SetWindowRgn};
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState,VK_LBUTTON,VK_RBUTTON};
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::*;

const SUBCLASS_ID: usize = 0x4E55_5341; // "DAOZAI"
#[allow(dead_code)]
const WS_EX_NOREDIRECTIONBITMAP: isize = 0x0020_0000;

/// 命中区（物理像素，窗口坐标，left/top/right/bottom）。
static HIT: OnceLock<Mutex<Vec<(i32, i32, i32, i32)>>> = OnceLock::new();
/// 前台窗口切到别的进程时置位，由主线程消费。
static FG_CHANGED: AtomicBool = AtomicBool::new(false);
/// 全局剪贴板历史热键。
const CLIPBOARD_HOTKEY_ID: i32 = 0xDA0A;
static CLIPBOARD_HOTKEY: AtomicBool = AtomicBool::new(false);
/// 全局命令面板热键。
const COMMAND_HOTKEY_ID: i32 = 0xDA0B;
static COMMAND_HOTKEY: AtomicBool = AtomicBool::new(false);

// `WS_EX_NOREDIRECTIONBITMAP` 只能在 CreateWindowEx 时设置（运行时 SetWindowLongPtrW
// 会回 ERROR_INVALID_PARAMETER），本项目用 GL 合成窗口，加上这一位后画面上什么都
// 看不到，因此不使用；具体见 `apply_style` 注释与交付说明。

fn hit() -> &'static Mutex<Vec<(i32, i32, i32, i32)>> {
    HIT.get_or_init(|| Mutex::new(Vec::new()))
}

fn inside_hit(x: i32, y: i32) -> bool {
    hit().lock().unwrap_or_else(|e|e.into_inner()).iter().any(|&(l,t,r,b)|x>=l&&x<r&&y>=t&&y<b)
}

fn file_drag_region(region:(i32,i32,i32,i32),zone:Option<(i32,i32,i32,i32)>,capture:bool)->(i32,i32,i32,i32){
    match zone.filter(|_|capture) {
        Some(zone)=>(region.0.min(zone.0),region.1.min(zone.1),region.2.max(zone.2),region.3.max(zone.3)),
        None=>region,
    }
}

#[derive(Default)]
struct DragCapture {
    origin: Option<(i32,i32)>,
    moved: bool,
}

impl DragCapture {
    fn update(&mut self,down:bool,cursor:Option<(i32,i32)>,zone:Option<(i32,i32,i32,i32)>,threshold:(i32,i32))->bool {
        let Some(cursor)=cursor.filter(|_|down && zone.is_some()) else {
            *self=Self::default();
            return false;
        };
        let origin=*self.origin.get_or_insert(cursor);
        self.moved|=(cursor.0-origin.0).abs()>threshold.0 || (cursor.1-origin.1).abs()>threshold.1;
        self.moved && zone.is_some_and(|(l,t,r,b)|cursor.0>=l && cursor.0<r && cursor.1>=t && cursor.1<b)
    }
}

/// 命中区外 `HTTRANSPARENT`（鼠标投给下层窗口），命中区内 `HTCLIENT`。
unsafe extern "system" fn subclass(
    hwnd: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    // 保留框架的焦点通知；lParam=-1 禁止系统在激活时重画透明窗口的顶边。
    if msg==WM_NCACTIVATE {return DefSubclassProc(hwnd,msg,wp,LPARAM(-1));}
    if msg==WM_NCPAINT {return LRESULT(0);}
    if msg == WM_HOTKEY && wp.0 as i32 == CLIPBOARD_HOTKEY_ID {
        CLIPBOARD_HOTKEY.store(true, Ordering::Relaxed);
        return LRESULT(0);
    }
    if msg == WM_HOTKEY && wp.0 as i32 == COMMAND_HOTKEY_ID {
        COMMAND_HOTKEY.store(true, Ordering::Relaxed);
        return LRESULT(0);
    }
    if msg == WM_MOUSEACTIVATE
        && GetWindowLongPtrW(hwnd, GWL_EXSTYLE) & WS_EX_NOACTIVATE.0 as isize != 0
    {
        return LRESULT(MA_NOACTIVATE as isize);
    }
    if msg == WM_NCHITTEST {
        let x = (lp.0 & 0xffff) as u16 as i16 as i32;
        let y = ((lp.0 >> 16) & 0xffff) as u16 as i16 as i32;
        let mut rect = RECT::default();
        if GetWindowRect(hwnd, &mut rect).is_ok() {
            let px = x - rect.left;
            let py = y - rect.top;
            return if inside_hit(px, py) {
                LRESULT(HTCLIENT as isize)
            } else {
                LRESULT(HTTRANSPARENT as isize)
            };
        }
    }
    DefSubclassProc(hwnd, msg, wp, lp)
}

unsafe extern "system" fn on_foreground(
    _hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    _object: i32,
    _child: i32,
    _thread: u32,
    _time: u32,
) {
    if event != EVENT_SYSTEM_FOREGROUND || hwnd.0.is_null() {
        return;
    }
    let mut pid = 0u32;
    let _ = GetWindowThreadProcessId(hwnd, Some(&mut pid));
    // 自己（含托盘窗口）拿到前台不算「切走」
    if pid != 0 && pid != GetCurrentProcessId() {
        FG_CHANGED.store(true, Ordering::Relaxed);
    }
}

/// 主线程每帧调用一次：这一帧是否发生了「前台切到别的程序」。
pub fn take_foreground_change() -> bool {
    FG_CHANGED.swap(false, Ordering::Relaxed)
}

pub fn register_clipboard_hotkey(hwnd: HWND) -> Result<(), String> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        RegisterHotKey, HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT,
    };
    unsafe {
        RegisterHotKey(
            hwnd,
            CLIPBOARD_HOTKEY_ID,
            HOT_KEY_MODIFIERS(MOD_CONTROL.0 | MOD_ALT.0 | MOD_NOREPEAT.0),
            'V' as u32,
        )
        .map_err(|_| "全局热键 Ctrl+Alt+V 被其他程序占用".to_string())
    }
}

pub fn unregister_clipboard_hotkey(hwnd: HWND) {
    use windows::Win32::UI::Input::KeyboardAndMouse::UnregisterHotKey;
    unsafe {
        let _ = UnregisterHotKey(hwnd, CLIPBOARD_HOTKEY_ID);
    }
}

pub fn take_clipboard_hotkey() -> bool {
    CLIPBOARD_HOTKEY.swap(false, Ordering::Relaxed)
}

#[cfg(test)]
pub(crate) fn test_clipboard_hotkey_press() {
    CLIPBOARD_HOTKEY.store(true, Ordering::Relaxed);
}

pub fn register_command_hotkey(hwnd: HWND) -> Result<(), String> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        RegisterHotKey, HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT,
    };
    unsafe {
        RegisterHotKey(
            hwnd,
            COMMAND_HOTKEY_ID,
            HOT_KEY_MODIFIERS(MOD_CONTROL.0 | MOD_ALT.0 | MOD_NOREPEAT.0),
            'K' as u32,
        )
        .map_err(|_| "全局热键 Ctrl+Alt+K 被其他程序占用".to_string())
    }
}

pub fn unregister_command_hotkey(hwnd: HWND) {
    use windows::Win32::UI::Input::KeyboardAndMouse::UnregisterHotKey;
    unsafe {
        let _ = UnregisterHotKey(hwnd, COMMAND_HOTKEY_ID);
    }
}

pub fn take_command_hotkey() -> bool {
    COMMAND_HOTKEY.swap(false, Ordering::Relaxed)
}

#[cfg(test)]
pub(crate) fn test_command_hotkey_press() {
    COMMAND_HOTKEY.store(true, Ordering::Relaxed);
}

pub fn foreground_window() -> Option<HWND> {
    let hwnd = unsafe { GetForegroundWindow() };
    (!hwnd.0.is_null()).then_some(hwnd)
}

fn window_process_id(hwnd: HWND) -> Option<u32> {
    if hwnd.0.is_null() { return None; }
    unsafe {
        let mut pid = 0u32;
        let thread = GetWindowThreadProcessId(hwnd, Some(&mut pid));
        (thread != 0 && pid != 0).then_some(pid)
    }
}

fn is_external_window(hwnd: HWND) -> bool {
    let current_pid = unsafe { GetCurrentProcessId() };
    (unsafe { IsWindow(hwnd).as_bool() })
        && window_process_id(hwnd).is_some_and(|pid| pid != current_pid)
}

pub fn external_foreground_window() -> Option<HWND> {
    foreground_window().filter(|&hwnd| is_external_window(hwnd))
}

pub fn foreground_window_label(hwnd: Option<HWND>) -> Option<String> {
    let hwnd = hwnd.filter(|&hwnd| is_external_window(hwnd))?;
    unsafe {
        let mut path = [0u16; 1024];
        let length = GetWindowModuleFileNameW(hwnd, &mut path);
        if length == 0 { return None; }
        let executable = String::from_utf16_lossy(&path[..length as usize]);
        let stem = Path::new(&executable).file_stem()?.to_str()?.trim();
        if stem.is_empty() { return None; }
        let label: String = stem.chars().filter(|character| !character.is_control()).take(64).collect();
        (!label.is_empty()).then_some(label)
    }
}

pub fn restore_foreground(hwnd: Option<HWND>) {
    let Some(hwnd) = hwnd else { return };
    unsafe {
        if IsWindow(hwnd).as_bool() {
            let _ = SetForegroundWindow(hwnd);
        }
    }
}

fn popup_position_in_work_area(cursor_px:(i32,i32),work_area_px:(i32,i32,i32,i32),width:f32,height:f32,pixels_per_point:f32)->Option<(f32,f32)> {
    if !width.is_finite() || !height.is_finite() || !pixels_per_point.is_finite()
        || width<=0.0 || height<=0.0 || pixels_per_point<=0.0 { return None; }
    let (left_px,top_px,right_px,bottom_px)=work_area_px;
    if right_px<=left_px || bottom_px<=top_px { return None; }
    let scale=pixels_per_point;
    let left=left_px as f32/scale;let top=top_px as f32/scale;
    let right=right_px as f32/scale;let bottom=bottom_px as f32/scale;
    let desired_x=cursor_px.0 as f32/scale+12.0;
    let desired_y=cursor_px.1 as f32/scale+12.0;
    Some((desired_x.clamp(left,(right-width).max(left)),desired_y.clamp(top,(bottom-height).max(top))))
}

pub fn clipboard_popup_position(width:f32,height:f32,pixels_per_point:f32)->Option<(f32,f32)> {
    use windows::Win32::Graphics::Gdi::{GetMonitorInfoW,MonitorFromPoint,MONITORINFO,MONITOR_DEFAULTTONEAREST};
    unsafe {
        let mut cursor=POINT::default();GetCursorPos(&mut cursor).ok()?;
        let monitor=MonitorFromPoint(cursor,MONITOR_DEFAULTTONEAREST);
        if monitor.0.is_null(){return None;}
        let mut info=MONITORINFO{cbSize:std::mem::size_of::<MONITORINFO>() as u32,..Default::default()};
        if !GetMonitorInfoW(monitor,&mut info).as_bool(){return None;}
        let area=info.rcWork;
        popup_position_in_work_area((cursor.x,cursor.y),(area.left,area.top,area.right,area.bottom),width,height,pixels_per_point)
    }
}

const PASTE_WAIT:Duration=Duration::from_millis(300);
const PASTE_POLL:Duration=Duration::from_millis(5);

fn modifier_keys_are_down()->bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::{VK_LCONTROL,VK_LMENU,VK_LSHIFT,VK_RCONTROL,VK_RMENU,VK_RSHIFT};
    [VK_LCONTROL,VK_RCONTROL,VK_LMENU,VK_RMENU,VK_LSHIFT,VK_RSHIFT].into_iter()
        .any(|key|unsafe{GetAsyncKeyState(key.0 as i32)<0})
}

fn wait_for_modifiers_released(deadline:std::time::Instant)->bool {
    while modifier_keys_are_down() {
        if std::time::Instant::now()>=deadline{return false;}
        std::thread::sleep(PASTE_POLL);
    }
    true
}

fn paste_release_keys(inserted:u32)->Vec<windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{VK_LCONTROL,VK_V};
    match inserted {1=>vec![VK_LCONTROL],2=>vec![VK_V,VK_LCONTROL],3=>vec![VK_LCONTROL],_=>Vec::new()}
}

unsafe fn release_paste_keys(keys:&[windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY]) {
    use windows::Win32::UI::Input::KeyboardAndMouse::{SendInput,INPUT,INPUT_0,INPUT_KEYBOARD,KEYBDINPUT,KEYEVENTF_KEYUP};
    let inputs:Vec<INPUT>=keys.iter().map(|&key|INPUT{r#type:INPUT_KEYBOARD,Anonymous:INPUT_0{ki:KEYBDINPUT{wVk:key,dwFlags:KEYEVENTF_KEYUP,..Default::default()}}}).collect();
    if !inputs.is_empty(){let _=SendInput(&inputs,std::mem::size_of::<INPUT>() as i32);}
}

fn target_is_current_external_window(hwnd:HWND,expected_pid:u32)->bool {
    unsafe {IsWindow(hwnd).as_bool() && GetForegroundWindow()==hwnd && window_process_id(hwnd)==Some(expected_pid) && expected_pid!=GetCurrentProcessId()}
}

pub fn restore_and_paste(hwnd:Option<HWND>)->Result<(),String> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{SendInput,INPUT,INPUT_0,INPUT_KEYBOARD,KEYBDINPUT,KEYEVENTF_KEYUP,VK_LCONTROL,VK_V};
    let hwnd=hwnd.ok_or_else(||"没有可恢复的外部窗口".to_string())?;
    let pid=window_process_id(hwnd).ok_or_else(||"剪贴板目标窗口无效".to_string())?;
    if !is_external_window(hwnd){return Err("剪贴板目标必须属于外部进程".to_string());}
    let deadline=std::time::Instant::now()+PASTE_WAIT;
    unsafe {
        if IsIconic(hwnd).as_bool(){let _=ShowWindowAsync(hwnd,SW_RESTORE);}
        if !SetForegroundWindow(hwnd).as_bool() && GetForegroundWindow()!=hwnd{return Err("无法恢复剪贴板目标窗口".to_string());}
    }
    while !target_is_current_external_window(hwnd,pid) {
        if std::time::Instant::now()>=deadline{return Err("剪贴板目标窗口未在超时内成为前台".to_string());}
        std::thread::sleep(PASTE_POLL);
    }
    if !wait_for_modifiers_released(deadline){return Err("等待物理 Ctrl/Alt/Shift 释放超时".to_string());}
    if !target_is_current_external_window(hwnd,pid){return Err("剪贴板目标窗口已失去前台".to_string());}
    let inputs=[
        INPUT{r#type:INPUT_KEYBOARD,Anonymous:INPUT_0{ki:KEYBDINPUT{wVk:VK_LCONTROL,..Default::default()}}},
        INPUT{r#type:INPUT_KEYBOARD,Anonymous:INPUT_0{ki:KEYBDINPUT{wVk:VK_V,..Default::default()}}},
        INPUT{r#type:INPUT_KEYBOARD,Anonymous:INPUT_0{ki:KEYBDINPUT{wVk:VK_V,dwFlags:KEYEVENTF_KEYUP,..Default::default()}}},
        INPUT{r#type:INPUT_KEYBOARD,Anonymous:INPUT_0{ki:KEYBDINPUT{wVk:VK_LCONTROL,dwFlags:KEYEVENTF_KEYUP,..Default::default()}}},
    ];
    let sent=unsafe{SendInput(&inputs,std::mem::size_of::<INPUT>() as i32)};
    if sent!=inputs.len() as u32 {
        let inserted=paste_release_keys(sent.min(inputs.len() as u32));
        unsafe{release_paste_keys(&inserted);}
        return Err(format!("Ctrl+V 输入仅派发 {sent}/{} 个事件",inputs.len()));
    }
    Ok(())
}

/// 光标所在显示器的工作区，换算成逻辑点。
pub fn cursor_work_area(pixels_per_point: f32) -> Option<(f32, f32, f32, f32)> {
    use windows::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    };
    unsafe {
        let mut cursor = POINT::default();
        GetCursorPos(&mut cursor).ok()?;
        let monitor = MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST);
        if monitor.0.is_null() {
            return None;
        }
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if !GetMonitorInfoW(monitor, &mut info).as_bool() {
            return None;
        }
        let area = info.rcWork;
        let scale = pixels_per_point.max(0.1);
        Some((
            area.left as f32 / scale,
            area.top as f32 / scale,
            (area.right - area.left) as f32 / scale,
            (area.bottom - area.top) as f32 / scale,
        ))
    }
}

/// winit 会在显示/改属性时重写窗口样式，所以每帧按需兜一次底。
unsafe fn apply_style(hwnd: HWND, editing: bool, raise: bool) {
    let style = GetWindowLongPtrW(hwnd, GWL_STYLE);
    let want = (style | WS_POPUP.0 as isize)
        & !((WS_CAPTION.0 | WS_THICKFRAME.0 | WS_SYSMENU.0 | WS_MINIMIZEBOX.0 | WS_MAXIMIZEBOX.0)
            as isize);
    let mut frame_changed = false;
    if want != style {
        SetWindowLongPtrW(hwnd, GWL_STYLE, want);
        frame_changed = true;
    }
    let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
    // 冻结单要求 0x08200088（含 WS_EX_NOREDIRECTIONBITMAP）。该位只能在
    // CreateWindowEx 时设置，运行时 SetWindowLongPtrW 会回 ERROR_INVALID_PARAMETER；
    // 而给 GL/窗口渲染的 HWND 加上它之后画面完全不被合成（实测可见像素为 0）。
    // 因此这里只保留可用的 TOPMOST|TOOLWINDOW|NOACTIVATE，去掉会露在任务栏的
    // APPWINDOW；ACCEPTFILES（winit 默认）也去掉，拖放走的是 OLE RegisterDragDrop，
    // 不依赖 WM_DROPFILES，这样 ex style 就是 0x08000088。
    // 置顶由 SetWindowPos 管理，避免样式标记与实际窗口层级脱节。
    let mut want_ex = (ex | WS_EX_TOOLWINDOW.0 as isize)
        & !(WS_EX_APPWINDOW.0 as isize)
        & !(WS_EX_ACCEPTFILES.0 as isize);
    if editing {
        want_ex &= !(WS_EX_NOACTIVATE.0 as isize);
    } else {
        want_ex |= WS_EX_NOACTIVATE.0 as isize;
    }
    if want_ex != ex {
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, want_ex);
    }
    let topmost = ex & WS_EX_TOPMOST.0 as isize != 0;
    if frame_changed || !topmost || raise {
        // 只有真的 SetWindowPos(HWND_TOPMOST) 才会让 TOPMOST 位生效
        // 每次展开重新抬高实际层级；标记存在时仍可能被别的窗口盖住。
        let mut flags=SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE;
        // 输入状态只切换激活权限，避免重画无边框窗口的非客户区。
        if frame_changed {flags|=SWP_FRAMECHANGED;}
        let _ = SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            0,
            0,
            0,
            0,
            flags,
        );
    }
}

pub struct Window {
    hwnd: HWND,
    region: (i32, i32, i32, i32),
    expanded: bool,
    hook: HWINEVENTHOOK,
    browser_state: (Option<bool>,PollClock),
    drag_capture: DragCapture,
    pub hotkey_error: Option<String>,
    pub command_hotkey_error: Option<String>,
}

impl Window {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Option<Self> {
        let RawWindowHandle::Win32(handle) = cc.window_handle().ok()?.as_raw() else { return None };
        let hwnd = HWND(handle.hwnd.get() as *mut _);
        let hook = unsafe {
            apply_style(hwnd, false, false);
            let _ = SetWindowSubclass(hwnd, Some(subclass), SUBCLASS_ID, 0);
            SetWinEventHook(
                EVENT_SYSTEM_FOREGROUND,
                EVENT_SYSTEM_FOREGROUND,
                None,
                Some(on_foreground),
                0,
                0,
                WINEVENT_OUTOFCONTEXT,
            )
        };
        let hotkey_error = register_clipboard_hotkey(hwnd).err();
        let command_hotkey_error = register_command_hotkey(hwnd).err();
        Some(Self { hwnd, region: (0, 0, 0, 0), expanded: false, hook, browser_state:(None,PollClock::default()), drag_capture:DragCapture::default(), hotkey_error, command_hotkey_error })
    }

    pub fn browser_at_top(&mut self)->Option<bool> {
        if self.browser_state.1.ready(Duration::from_millis(100)) {
            self.browser_state=(crate::system::browser_at_top(self.hwnd),PollClock::now());
        }
        self.browser_state.0
    }

    /// 区域参数均为窗口内的逻辑坐标 (x, y, 宽, 高)。
    pub fn update(
        &mut self,
        region: (f32, f32, f32, f32),
        hit_rect: (f32, f32, f32, f32),
        notice_hit: Option<(f32,f32,f32,f32)>,
        file_drop_zone: Option<(f32,f32,f32,f32)>,
        ppp: f32,
        editing: bool,
        expanded: bool,
    ) {
        let px = |v: f32| (v * ppp).round() as i32;
        let r = (px(region.0), px(region.1), px(region.0 + region.2), px(region.1 + region.3));
        // 仅在拖动进入感应区后扩大 OLE 接收区域，避免普通点击改变透明窗口裁剪。
        let button_down=unsafe{GetAsyncKeyState(VK_LBUTTON.0 as i32)<0 || GetAsyncKeyState(VK_RBUTTON.0 as i32)<0};
        let zone=file_drop_zone.map(|(x,y,w,h)|(px(x),px(y),px(x+w),px(y+h)));
        let mut cursor=POINT::default();let mut window=RECT::default();
        let cursor=unsafe{(GetCursorPos(&mut cursor).is_ok() && GetWindowRect(self.hwnd,&mut window).is_ok()).then_some((cursor.x-window.left,cursor.y-window.top))};
        let threshold=unsafe{((GetSystemMetrics(SM_CXDRAG)/2).max(1),(GetSystemMetrics(SM_CYDRAG)/2).max(1))};
        let capture=self.drag_capture.update(button_down,cursor,zone,threshold);
        let r=file_drag_region(r,zone,capture);
        let h = (
            px(hit_rect.0),
            px(hit_rect.1),
            px(hit_rect.0 + hit_rect.2),
            px(hit_rect.1 + hit_rect.3),
        );
        let mut hits=vec![h];
        if let Some((x,y,w,h))=notice_hit {hits.push((px(x),px(y),px(x+w),px(y+h)));}
        *hit().lock().unwrap_or_else(|e| e.into_inner()) = hits;
        if self.region != r {
            unsafe {
                let rgn = CreateRectRgn(r.0, r.1, r.2, r.3);
                if SetWindowRgn(self.hwnd, rgn, true) != 0 {
                    self.region = r;
                } else {
                    let _ = DeleteObject(rgn);
                }
            }
        }
        unsafe { apply_style(self.hwnd, editing, expanded && !self.expanded) };
        self.expanded = expanded;
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        unregister_clipboard_hotkey(self.hwnd);
        unregister_command_hotkey(self.hwnd);
        unsafe {
            let _ = RemoveWindowSubclass(self.hwnd, Some(subclass), SUBCLASS_ID);
            if !self.hook.is_invalid() {
                let _ = UnhookWinEvent(self.hook);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clipboard_popup_geometry_follows_cursor_and_clamps_to_work_area() {
        assert_eq!(popup_position_in_work_area((100,200),(0,0,1920,1040),420.0,500.0,1.0),Some((112.0,212.0)));
        assert_eq!(popup_position_in_work_area((1900,1020),(0,0,1920,1040),420.0,500.0,1.0),Some((1500.0,540.0)));
        assert_eq!(popup_position_in_work_area((-1900,-1000),(-1920,-1080,0,0),420.0,500.0,2.0),Some((-938.0,-500.0)));
        assert_eq!(popup_position_in_work_area((-10,-10),(-1920,-1080,0,0),420.0,500.0,2.0),Some((-420.0,-500.0)));
        assert_eq!(popup_position_in_work_area((100,100),(0,40,300,240),420.0,500.0,1.0),Some((0.0,40.0)));
    }

    #[test]
    fn clipboard_popup_geometry_rejects_invalid_dimensions_and_scale() {
        for invalid in [0.0,-1.0,f32::NAN,f32::INFINITY] {
            assert!(popup_position_in_work_area((0,0),(0,0,1920,1040),invalid,500.0,1.0).is_none());
            assert!(popup_position_in_work_area((0,0),(0,0,1920,1040),420.0,invalid,1.0).is_none());
            assert!(popup_position_in_work_area((0,0),(0,0,1920,1040),420.0,500.0,invalid).is_none());
        }
        assert!(popup_position_in_work_area((0,0),(100,0,0,1040),420.0,500.0,1.0).is_none());
        assert!(popup_position_in_work_area((0,0),(0,40,1920,40),420.0,500.0,1.0).is_none());
    }

    #[test]
    fn clipboard_paste_rejects_missing_and_invalid_windows_without_input() {
        for target in [None,Some(HWND::default()),Some(HWND((-1isize) as *mut _))] {
            assert!(restore_and_paste(target).is_err());
            assert!(foreground_window_label(target).is_none());
        }
    }

    #[test]
    fn clipboard_paste_rejects_our_own_window_without_input() {
        unsafe {
            let hwnd=CreateWindowExW(WS_EX_NOACTIVATE,windows::core::w!("STATIC"),windows::core::w!("private document title"),WS_POPUP,0,0,1,1,None,None,None,None).unwrap();
            let result=restore_and_paste(Some(hwnd));
            let label=foreground_window_label(Some(hwnd));
            let _=DestroyWindow(hwnd);
            assert!(result.is_err());
            assert!(label.is_none());
        }
    }

    #[test]
    fn clipboard_partial_input_recovery_releases_only_inserted_keydowns() {
        use windows::Win32::UI::Input::KeyboardAndMouse::{VK_LCONTROL,VK_V};
        assert_eq!(paste_release_keys(0),Vec::new());
        assert_eq!(paste_release_keys(1),vec![VK_LCONTROL]);
        assert_eq!(paste_release_keys(2),vec![VK_V,VK_LCONTROL]);
        assert_eq!(paste_release_keys(3),vec![VK_LCONTROL]);
        assert_eq!(paste_release_keys(4),Vec::new());
    }

    #[test]
    fn drag_capture_ignores_clicks_and_pointer_jitter() {
        let mut capture=DragCapture::default();let zone=Some((110,0,530,120));
        assert!(!capture.update(true,Some((300,60)),zone,(4,4)),"感应区内按下不能直接扩大窗口");
        assert!(!capture.update(true,Some((302,61)),zone,(4,4)),"点击时的小幅抖动不算拖动");
        assert!(!capture.update(false,Some((302,61)),zone,(4,4)));
    }

    #[test]
    fn drag_capture_requires_movement_and_entry_and_resets_after_release() {
        let mut capture=DragCapture::default();let zone=Some((110,0,530,120));
        assert!(!capture.update(true,Some((700,400)),zone,(4,4)));
        assert!(!capture.update(true,Some((650,300)),zone,(4,4)),"远处拖动不能改变岛的区域");
        assert!(capture.update(true,Some((300,60)),zone,(4,4)),"拖到感应区后允许接收文件");
        assert!(!capture.update(true,Some((700,400)),zone,(4,4)),"离开感应区应恢复区域");
        assert!(capture.update(true,Some((300,60)),zone,(4,4)));
        assert!(!capture.update(false,Some((300,60)),zone,(4,4)));
        assert!(!capture.update(true,Some((300,60)),zone,(4,4)),"下一次点击不能沿用上次拖动状态");
        assert!(!capture.update(true,None,zone,(4,4)));
        assert!(!capture.update(true,Some((300,60)),None,(4,4)));
    }

    #[test]
    fn expanding_raises_the_island_without_activating_it(){
        struct TestWindow(HWND);
        impl Drop for TestWindow {fn drop(&mut self){unsafe{let _=DestroyWindow(self.0);}}}
        unsafe fn above(first:HWND,second:HWND)->bool {
            let mut current=first;
            while let Ok(next)=GetWindow(current,GW_HWNDNEXT){
                if next==second{return true;}
                current=next;
            }
            false
        }
        unsafe {
            // 窗口在屏幕外可见，才能验证实际桌面层级；不激活，也不遮挡用户桌面。
            let create=||TestWindow(CreateWindowExW(WS_EX_NOACTIVATE|WS_EX_TOOLWINDOW|WS_EX_TOPMOST,windows::core::w!("STATIC"),windows::core::w!(""),WS_POPUP|WS_VISIBLE,-10000,-10000,416,140,None,None,None,None).unwrap());
            let island=create();let cover=create();
            let flags=SWP_NOMOVE|SWP_NOSIZE|SWP_NOACTIVATE;
            apply_style(island.0,false,false);
            SetWindowPos(cover.0,HWND_TOPMOST,0,0,0,0,flags).unwrap();
            assert!(above(cover.0,island.0),"先复现其他窗口盖住岛体");
            let foreground=GetForegroundWindow();
            let saved=hit().lock().unwrap_or_else(|e|e.into_inner()).clone();
            let mut native=Window{hwnd:island.0,region:(0,0,0,0),expanded:false,hook:HWINEVENTHOOK::default(),browser_state:(None,PollClock::now()),drag_capture:DragCapture::default(),hotkey_error:None,command_hotkey_error:None};
            let update=|native:&mut Window,expanded|native.update((0.0,0.0,416.0,140.0),(0.0,0.0,416.0,140.0),None,None,1.0,false,expanded);
            update(&mut native,true);
            assert!(above(island.0,cover.0),"展开时必须重新抬高实际窗口层级，不能只检查置顶标记");
            assert_eq!(GetForegroundWindow(),foreground,"悬停展开不能抢走当前应用的输入焦点");
            SetWindowPos(cover.0,HWND_TOPMOST,0,0,0,0,flags).unwrap();
            update(&mut native,true);
            assert!(above(cover.0,island.0),"展开后不能每帧与其他弹窗争抢层级");
            update(&mut native,false);
            assert!(above(cover.0,island.0),"收起时不抬高层级");
            // 模拟只剩置顶标记、实际顺序落在普通窗口后面的情况。
            SetWindowPos(island.0,HWND_NOTOPMOST,0,0,0,0,flags).unwrap();
            SetWindowPos(cover.0,HWND_NOTOPMOST,0,0,0,0,flags).unwrap();
            SetWindowLongPtrW(island.0,GWL_EXSTYLE,GetWindowLongPtrW(island.0,GWL_EXSTYLE)|WS_EX_TOPMOST.0 as isize);
            assert!(above(cover.0,island.0));
            update(&mut native,true);
            assert!(above(island.0,cover.0),"再次展开也必须恢复实际置顶顺序");
            assert_eq!(GetForegroundWindow(),foreground);
            *hit().lock().unwrap_or_else(|e|e.into_inner())=saved;
        }
    }

    #[test]
    fn file_drag_expands_region_only_when_capture_is_active(){
        let island=(230,0,410,42);let zone=(110,0,530,120);
        assert_eq!(file_drag_region(island,Some(zone),false),island,"普通悬停不扩大原生区域");
        assert_eq!(file_drag_region(island,Some(zone),true),zone,"拖入感应区应覆盖岛下方");
        assert_eq!(file_drag_region(island,None,true),island,"关闭置物架或隐藏岛时不接收外围拖放");
        let expanded=(64,0,576,220);
        assert_eq!(file_drag_region(expanded,Some(zone),true),expanded,"展开后的文件区不应被感应区裁掉");
    }

    #[test]
    fn file_capture_below_the_island_still_returns_click_through(){
        unsafe {
            let saved=std::mem::replace(&mut *hit().lock().unwrap_or_else(|e|e.into_inner()),vec![(230,0,410,42)]);
            let hwnd=CreateWindowExW(WS_EX_NOACTIVATE,windows::core::w!("STATIC"),windows::core::w!(""),WS_POPUP,0,0,640,270,None,None,None,None).unwrap();
            SetWindowSubclass(hwnd,Some(subclass),SUBCLASS_ID,0).unwrap();
            let region=file_drag_region((230,0,410,42),Some((110,0,530,120)),true);
            let rgn=CreateRectRgn(region.0,region.1,region.2,region.3);assert_ne!(SetWindowRgn(hwnd,rgn,false),0);
            let mut rect=RECT::default();GetWindowRect(hwnd,&mut rect).unwrap();
            let point=|x:i32,y:i32|LPARAM((((rect.top+y) as u32)<<16|((rect.left+x) as u16 as u32)) as isize);
            assert_eq!(SendMessageW(hwnd,WM_NCHITTEST,WPARAM(0),point(500,80)).0,HTTRANSPARENT as isize,"感应区中的普通点击仍须穿透");
            assert_eq!(SendMessageW(hwnd,WM_NCHITTEST,WPARAM(0),point(320,15)).0,HTCLIENT as isize,"岛体本身仍可交互");
            let _=DestroyWindow(hwnd);*hit().lock().unwrap_or_else(|e|e.into_inner())=saved;
        }
    }

    #[test]
    fn right_notice_keeps_empty_space_below_the_main_bar_click_through(){
        let saved=std::mem::replace(&mut *hit().lock().unwrap_or_else(|e|e.into_inner()),vec![(100,0,400,30),(372,0,760,96)]);
        assert!(inside_hit(600,70));assert!(inside_hit(200,15));assert!(!inside_hit(200,70));
        *hit().lock().unwrap_or_else(|e|e.into_inner())=saved;
    }

    #[test]
    fn activation_keeps_focus_messages_but_suppresses_nonclient_paint(){
        #[derive(Default)]
        struct Capture{activation:Vec<(usize,isize)>,paint:u32}
        unsafe extern "system" fn observe(hwnd:HWND,msg:u32,wp:WPARAM,lp:LPARAM,_id:usize,data:usize)->LRESULT{
            let capture=&mut *(data as *mut Capture);
            if msg==WM_NCACTIVATE{capture.activation.push((wp.0,lp.0));}
            if msg==WM_NCPAINT{capture.paint+=1;return LRESULT(0);}
            DefSubclassProc(hwnd,msg,wp,lp)
        }
        unsafe{
            let mut capture=Capture::default();
            let hwnd=CreateWindowExW(WS_EX_NOACTIVATE,windows::core::w!("STATIC"),windows::core::w!(""),WS_POPUP,0,0,100,50,None,None,None,None).unwrap();
            SetWindowSubclass(hwnd,Some(observe),SUBCLASS_ID+2,&mut capture as *mut Capture as usize).unwrap();
            SetWindowSubclass(hwnd,Some(subclass),SUBCLASS_ID,0).unwrap();
            for active in [1,0]{let result=SendMessageW(hwnd,WM_NCACTIVATE,WPARAM(active),LPARAM(0));if active==0{assert_ne!(result.0,0,"仍须允许切换到其他窗口");}}
            SendMessageW(hwnd,WM_NCPAINT,WPARAM(1),LPARAM(0));
            let _=DestroyWindow(hwnd);
            assert_eq!(capture.activation,[(1,-1),(0,-1)],"仍需通知窗口框架焦点变化，同时阻止系统重画顶边");
            assert_eq!(capture.paint,0,"透明岛体不能交给系统绘制非客户区");
        }
    }

    #[test]
    fn editing_mode_does_not_recalculate_the_native_window_frame(){
        unsafe extern "system" fn count_frame(hwnd:HWND,msg:u32,wp:WPARAM,lp:LPARAM,_id:usize,data:usize)->LRESULT {
            if msg==WM_NCCALCSIZE {*(data as *mut u32)+=1;}
            DefSubclassProc(hwnd,msg,wp,lp)
        }
        struct TestWindow(HWND);
        impl Drop for TestWindow {fn drop(&mut self){unsafe{let _=DestroyWindow(self.0);}}}
        unsafe {
            let mut count=0u32;
            // 与实际岛体一样，从已置顶窗口验证输入状态切换。
            let window=TestWindow(CreateWindowExW(WS_EX_NOACTIVATE|WS_EX_TOPMOST,windows::core::w!("STATIC"),windows::core::w!(""),WS_POPUP,0,0,100,50,None,None,None,None).unwrap());
            SetWindowSubclass(window.0,Some(count_frame),SUBCLASS_ID+1,&mut count as *mut u32 as usize).unwrap();
            apply_style(window.0,false,false);count=0;
            for editing in [true,false,true,false] {apply_style(window.0,editing,false);}
            apply_style(window.0,false,true);
            assert_eq!(count,0,"添加和退出输入时不应触发原生边框重算，避免顶端白条闪现");
            assert_ne!(GetWindowLongPtrW(window.0,GWL_EXSTYLE)&WS_EX_TOPMOST.0 as isize,0,"仍应保持岛体置顶");
        }
    }

    #[test]
    fn mouse_click_keeps_overlay_inactive_except_when_editing() {
        struct TestWindow(HWND);
        impl Drop for TestWindow {
            fn drop(&mut self) { unsafe { let _ = DestroyWindow(self.0); } }
        }
        unsafe {
            let window = TestWindow(CreateWindowExW(
                WS_EX_NOACTIVATE, windows::core::w!("STATIC"), windows::core::w!(""),
                WS_POPUP, 0, 0, 100, 50, None, None, None, None,
            ).unwrap());
            SetWindowSubclass(window.0, Some(subclass), SUBCLASS_ID, 0).unwrap();
            let click = LPARAM(((WM_LBUTTONDOWN << 16) | HTCLIENT) as isize);
            assert_eq!(SendMessageW(window.0, WM_MOUSEACTIVATE, WPARAM(0), click).0, MA_NOACTIVATE as isize);
            let style = GetWindowLongPtrW(window.0, GWL_EXSTYLE);
            SetWindowLongPtrW(window.0, GWL_EXSTYLE, style & !(WS_EX_NOACTIVATE.0 as isize));
            assert_eq!(SendMessageW(window.0, WM_MOUSEACTIVATE, WPARAM(0), click).0, MA_ACTIVATE as isize);
        }
    }

    #[test]
    fn command_hotkey_flag_is_consumed_once() {
        test_command_hotkey_press();
        assert!(take_command_hotkey());
        assert!(!take_command_hotkey());
    }

    #[test]
    fn clipboard_hotkey_flag_is_consumed_once() {
        test_clipboard_hotkey_press();
        assert!(take_clipboard_hotkey());
        assert!(!take_clipboard_hotkey());
    }
}
