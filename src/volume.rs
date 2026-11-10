//! 系统音量：读取/调整默认输出设备的音量与静音状态，并驱动音量 HUD。
//!
//! Windows 走 Core Audio（`IAudioEndpointVolume`）；其它平台返回 `None`。
//! 数值统一为 0.0–1.0 的标量，HUD 显示为百分比。

pub const STEP: f32 = 0.05;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct State {
    pub level: f32,
    pub muted: bool,
}

pub fn clamp_level(level: f32) -> f32 {
    level.clamp(0.0, 1.0)
}

#[cfg(windows)]
fn with_endpoint<T>(run: impl FnOnce(&windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume) -> Option<T>) -> Option<T> {
    use windows::Win32::Media::Audio::{
        eConsole, eRender, IMMDeviceEnumerator, MMDeviceEnumerator,
    };
    use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED};
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
        let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole).ok()?;
        let volume: IAudioEndpointVolume = device.Activate(CLSCTX_ALL, None).ok()?;
        run(&volume)
    }
}

#[cfg(windows)]
pub fn read() -> Option<State> {
    with_endpoint(|volume| unsafe {
        let level = volume.GetMasterVolumeLevelScalar().ok()?;
        let muted = volume.GetMute().ok()?.as_bool();
        Some(State { level: clamp_level(level), muted })
    })
}

#[cfg(windows)]
pub fn set_level(level: f32) -> Option<State> {
    let level = clamp_level(level);
    with_endpoint(|volume| unsafe {
        volume
            .SetMasterVolumeLevelScalar(level, std::ptr::null())
            .ok()?;
        let muted = volume.GetMute().ok()?.as_bool();
        Some(State { level, muted })
    })
}

#[cfg(windows)]
pub fn toggle_mute() -> Option<State> {
    with_endpoint(|volume| unsafe {
        let muted = volume.GetMute().ok()?.as_bool();
        volume
            .SetMute(!muted, std::ptr::null())
            .ok()?;
        let level = volume.GetMasterVolumeLevelScalar().ok()?;
        Some(State { level: clamp_level(level), muted: !muted })
    })
}

#[cfg(not(windows))]
pub fn read() -> Option<State> {
    None
}

#[cfg(not(windows))]
pub fn set_level(_level: f32) -> Option<State> {
    None
}

#[cfg(not(windows))]
pub fn toggle_mute() -> Option<State> {
    None
}

/// 在当前状态上加减；返回 None 表示系统音量接口不可用。
pub fn adjust(delta: f32) -> Option<State> {
    let current = read()?;
    set_level(clamp_level(current.level + delta))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn live_volume_endpoint_is_readable() {
        if std::env::var("DAOZAI_AUDIO_TESTS").as_deref() != Ok("1") {
            return;
        }
        let state = read().expect("真实环境应能读取默认输出设备音量");
        assert!((0.0..=1.0).contains(&state.level));
    }

    #[test]
    fn levels_are_clamped() {
        assert_eq!(clamp_level(-1.0), 0.0);
        assert_eq!(clamp_level(0.5), 0.5);
        assert_eq!(clamp_level(2.0), 1.0);
        assert!((STEP - 0.05).abs() < 1e-6);
    }
}
