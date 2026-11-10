//! 屏幕亮度：读写鼠标所在显示器的亮度（DDC/CI 或内置屏），并驱动亮度 HUD。
//!
//! Windows 通过 `Get/SetMonitorBrightness`（dxva2）实现；不支持 DDC/CI 的外接屏
//! 读取会失败，此时 HUD 给出「不支持」提示。数值统一为 0.0–1.0。

pub const STEP: f32 = 0.10;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct State {
    pub level: f32,
}

pub fn clamp_level(level: f32) -> f32 {
    level.clamp(0.0, 1.0)
}

#[cfg(windows)]
fn with_brightness(desired: Option<f32>) -> Option<State> {
    use windows::Win32::Devices::Display::{
        DestroyPhysicalMonitors, GetMonitorBrightness, GetNumberOfPhysicalMonitorsFromHMONITOR,
        GetPhysicalMonitorsFromHMONITOR, PHYSICAL_MONITOR, SetMonitorBrightness,
    };
    use windows::Win32::Foundation::POINT;
    use windows::Win32::Graphics::Gdi::{MonitorFromPoint, MONITOR_DEFAULTTONEAREST};
    use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;
    unsafe {
        let mut cursor = POINT::default();
        GetCursorPos(&mut cursor).ok()?;
        let monitor = MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST);
        if monitor.0.is_null() {
            return None;
        }
        let mut count = 0u32;
        GetNumberOfPhysicalMonitorsFromHMONITOR(monitor, &mut count).ok()?;
        if count == 0 {
            return None;
        }
        let mut monitors = vec![PHYSICAL_MONITOR::default(); count as usize];
        GetPhysicalMonitorsFromHMONITOR(monitor, &mut monitors).ok()?;
        let handle = monitors[0].hPhysicalMonitor;
        let result = (|| {
            let mut minimum = 0u32;
            let mut current = 0u32;
            let mut maximum = 0u32;
            if GetMonitorBrightness(handle, &mut minimum, &mut current, &mut maximum) == 0
                || maximum <= minimum
            {
                return None;
            }
            let target = match desired {
                Some(level) => {
                    let span = (maximum - minimum) as f64;
                    let target = minimum
                        + (clamp_level(level) as f64 * span).round() as u32;
                    let target = target.clamp(minimum, maximum);
                    SetMonitorBrightness(handle, target);
                    target
                }
                None => current.clamp(minimum, maximum),
            };
            Some(State {
                level: clamp_level(
                    target.saturating_sub(minimum) as f32 / (maximum - minimum) as f32,
                ),
            })
        })();
        let _ = DestroyPhysicalMonitors(&monitors);
        result
    }
}

#[cfg(windows)]
pub fn read() -> Option<State> {
    with_brightness(None)
}

#[cfg(windows)]
pub fn set_level(level: f32) -> Option<State> {
    with_brightness(Some(level))
}

#[cfg(not(windows))]
pub fn read() -> Option<State> {
    None
}

#[cfg(not(windows))]
pub fn set_level(_level: f32) -> Option<State> {
    None
}

pub fn adjust(delta: f32) -> Option<State> {
    let current = read()?;
    set_level(clamp_level(current.level + delta))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn live_brightness_probe() {
        if std::env::var("DAOZAI_AUDIO_TESTS").as_deref() != Ok("1") {
            return;
        }
        let state = read();
        eprintln!("LIVE_BRIGHTNESS={state:?}");
        if let Some(state) = state {
            assert!((0.0..=1.0).contains(&state.level));
        }
    }

    #[test]
    fn levels_are_clamped() {
        assert_eq!(clamp_level(-1.0), 0.0);
        assert_eq!(clamp_level(0.4), 0.4);
        assert_eq!(clamp_level(3.0), 1.0);
        assert!((STEP - 0.10).abs() < 1e-6);
    }
}
