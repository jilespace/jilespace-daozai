//! 系统音频回环 + FFT：给岛体上的播放条提供真实频谱。
//!
//! - 纯逻辑（PCM 下混、FFT、分频段）与平台无关，带单元测试；
//! - Windows 侧用 WASAPI loopback 抓默认输出设备的混音，后台线程按 30 Hz
//!   推送 12 段频谱，界面只在音乐播放时使用，失败时静默回退到原来的装饰条。
//!
//! 这个模块不保存任何音频数据，只保留最近一帧频谱；关闭设置后线程退出。

use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub const BANDS: usize = 12;
pub const FFT_SIZE: usize = 1024;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Spectrum {
    /// 低频 → 高频，0..1。
    pub bands: [f32; BANDS],
    /// 整体响度，用于判断是否真的有声音。
    pub level: f32,
}

#[derive(Clone, Debug)]
pub enum Event {
    Spectrum(Spectrum),
    Unavailable(String),
}

/// 把交错 PCM 下混为单声道 f32。
pub fn downmix(samples: &[u8], bits_per_sample: u16, channels: u16, float: bool) -> Vec<f32> {
    let channels = channels.max(1) as usize;
    let frame_bytes = (bits_per_sample as usize / 8).max(1) * channels;
    if frame_bytes == 0 {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(samples.len() / frame_bytes);
    for frame in samples.chunks_exact(frame_bytes) {
        let mut sum = 0.0f32;
        for channel in 0..channels {
            let offset = channel * (bits_per_sample as usize / 8);
            let value = if bits_per_sample == 32 && float {
                let bytes = [frame[offset], frame[offset + 1], frame[offset + 2], frame[offset + 3]];
                f32::from_le_bytes(bytes)
            } else if bits_per_sample == 32 {
                let bytes = [frame[offset], frame[offset + 1], frame[offset + 2], frame[offset + 3]];
                i32::from_le_bytes(bytes) as f32 / i32::MAX as f32
            } else if bits_per_sample == 16 {
                let bytes = [frame[offset], frame[offset + 1]];
                i16::from_le_bytes(bytes) as f32 / i16::MAX as f32
            } else {
                0.0
            };
            sum += value;
        }
        out.push((sum / channels as f32).clamp(-1.0, 1.0));
    }
    out
}

/// 1024 点实数 FFT 的幅度谱（只返回前一半），输入不足时补零。
pub fn magnitudes(samples: &[f32]) -> Vec<f32> {
    let mut data = vec![(0.0f32, 0.0f32); FFT_SIZE];
    for (index, sample) in samples.iter().rev().take(FFT_SIZE).enumerate() {
        // 汉宁窗，减少频谱泄漏。
        let window = 0.5 - 0.5 * (std::f32::consts::TAU * index as f32 / (FFT_SIZE - 1) as f32).cos();
        data[FFT_SIZE - 1 - index].0 = sample * window;
    }
    fft_in_place(&mut data);
    data[1..FFT_SIZE / 2].iter().map(|(re, im)| (re * re + im * im).sqrt()).collect()
}

fn fft_in_place(data: &mut [(f32, f32)]) {
    let n = data.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j ^= bit;
        if i < j {
            data.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let angle = -std::f32::consts::TAU / len as f32;
        for start in (0..n).step_by(len * 2) {
            for k in 0..len / 2 {
                let (sin, cos) = (angle * k as f32).sin_cos();
                let (wr, wi) = (cos, sin);
                let (ar, ai) = data[start + k];
                let (br, bi) = data[start + k + len / 2];
                let (tr, ti) = (br * wr - bi * wi, br * wi + bi * wr);
                data[start + k] = (ar + tr, ai + ti);
                data[start + k + len / 2] = (ar - tr, ai - ti);
            }
        }
        len <<= 1;
    }
}

/// 把幅度谱映射成 12 个对数频段（约 40 Hz–12 kHz），并做峰值归一化。
pub fn bands(samples: &[f32], sample_rate: u32) -> Spectrum {
    let mags = magnitudes(samples);
    let nyquist = sample_rate.max(8000) as f32 / 2.0;
    let low = 40.0f32.min(nyquist * 0.5);
    let high = 12_000.0f32.min(nyquist * 0.95);
    let mut raw = [0.0f32; BANDS];
    let mut peak = 0.0f32;
    let bin_hz = sample_rate.max(8000) as f32 / FFT_SIZE as f32;
    for band in 0..BANDS {
        let from = low * (high / low).powf(band as f32 / BANDS as f32);
        let to = low * (high / low).powf((band + 1) as f32 / BANDS as f32);
        let start = (from / bin_hz).round() as usize;
        let end = ((to / bin_hz).round() as usize).max(start + 1);
        let mut sum = 0.0;
        let mut count = 0;
        for index in start..end.min(mags.len()) {
            sum += mags[index];
            count += 1;
        }
        let value = if count == 0 { 0.0 } else { sum / count as f32 };
        raw[band] = value;
        peak = peak.max(value);
    }
    // 视觉上压缩动态范围：能量开方后归一化，小信号也能看出起伏。
    let reference = peak.max(1e-5);
    let mut out = [0.0f32; BANDS];
    let mut level = 0.0;
    for band in 0..BANDS {
        let normalized = (raw[band] / reference).sqrt().clamp(0.0, 1.0);
        out[band] = normalized;
        level += normalized;
    }
    Spectrum { bands: out, level: (level / BANDS as f32).clamp(0.0, 1.0) }
}

pub struct Service {
    events: Receiver<Event>,
    stop: Arc<AtomicBool>,
}

impl Service {
    pub fn start() -> Self {
        let (tx, events) = channel();
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        std::thread::Builder::new()
            .name("daozai-audio".into())
            .spawn(move || {
                #[cfg(windows)]
                capture_loop(tx, worker_stop);
                #[cfg(not(windows))]
                {
                    let _ = tx.send(Event::Unavailable("当前平台不支持系统音频回环".into()));
                    let _ = worker_stop;
                }
            })
            .ok();
        Self { events, stop }
    }

    pub fn try_recv(&self) -> Option<Event> {
        self.events.try_recv().ok()
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

#[cfg(windows)]
fn capture_loop(tx: Sender<Event>, stop: Arc<AtomicBool>) {
    use windows::Win32::Media::Audio::{
        eConsole, eRender, IAudioCaptureClient, IAudioClient, IMMDeviceEnumerator, MMDeviceEnumerator,
        AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK,
    };
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_ALL, COINIT_MULTITHREADED,
    };

    let run = || -> windows::core::Result<()> {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
            let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole)?;
            let client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
            let format = client.GetMixFormat()?;
            let format_ref = &*format;
            let channels = format_ref.nChannels;
            let sample_rate = format_ref.nSamplesPerSec;
            let bits = format_ref.wBitsPerSample;
            let float = bits == 32;
            client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_LOOPBACK,
                10_000_000,
                0,
                format as *const _,
                None,
            )?;
            CoTaskMemFree(Some(format as *const _ as *const core::ffi::c_void));
            let capture: IAudioCaptureClient = client.GetService()?;
            client.Start()?;
            let mut samples: Vec<f32> = Vec::new();
            let mut tick = std::time::Instant::now();
            loop {
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                let Ok(mut packet) = capture.GetNextPacketSize() else {
                    break;
                };
                while packet > 0 {
                    let mut data = std::ptr::null_mut();
                    let mut frames = 0u32;
                    let mut flags = 0u32;
                    if capture
                        .GetBuffer(&mut data, &mut frames, &mut flags, None, None)
                        .is_err()
                    {
                        break;
                    }
                    if frames > 0 && !data.is_null() {
                        let bytes = frames as usize * (bits as usize / 8).max(1) * channels.max(1) as usize;
                        if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 == 0 {
                            let slice = std::slice::from_raw_parts(data as *const u8, bytes);
                            samples.extend(downmix(slice, bits as u16, channels, float));
                        } else {
                            samples.extend(std::iter::repeat(0.0).take(frames as usize));
                        }
                        if samples.len() > 8192 {
                            let excess = samples.len() - 8192;
                            samples.drain(0..excess);
                        }
                    }
                    let _ = capture.ReleaseBuffer(frames);
                    let Ok(next) = capture.GetNextPacketSize() else {
                        break;
                    };
                    packet = next;
                }
                if tick.elapsed().as_millis() >= 33 {
                    tick = std::time::Instant::now();
                    if !samples.is_empty() {
                        let spectrum = bands(&samples, sample_rate);
                        if tx.send(Event::Spectrum(spectrum)).is_err() {
                            break;
                        }
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            let _ = client.Stop();
            Ok(())
        }
    };
    if let Err(error) = run() {
        let _ = tx.send(Event::Unavailable(format!("无法初始化系统音频回环：{}", error.code())));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(frequency: f32, sample_rate: u32, len: usize) -> Vec<f32> {
        (0..len)
            .map(|index| (std::f32::consts::TAU * frequency * index as f32 / sample_rate as f32).sin() * 0.5)
            .collect()
    }

    #[test]
    fn downmix_handles_float_and_i16_stereo() {
        let mut bytes = Vec::new();
        for (left, right) in [(0.5f32, 0.1f32), (-0.5, -1.0)] {
            for value in [left, right] {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
        }
        let mono = downmix(&bytes, 32, 2, true);
        assert_eq!(mono.len(), 2);
        assert!((mono[0] - 0.3).abs() < 1e-6);
        assert!((mono[1] + 0.75).abs() < 1e-6);
        let pcm: Vec<u8> = [3277i16, 3277, -32768].iter().flat_map(|value| value.to_le_bytes()).collect();
        let mono = downmix(&pcm, 16, 1, false);
        assert_eq!(mono.len(), 3);
        assert!(mono[1] > 0.0 && mono[2] < 0.0);
    }

    #[test]
    fn fft_finds_the_dominant_sine_bin() {
        let sample_rate = 48_000;
        let samples = sine(1_500.0, sample_rate, 1024);
        let spectrum = bands(&samples, sample_rate);
        let loudest = spectrum
            .bands
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .map(|(index, _)| index)
            .unwrap();
        // 1.5 kHz 落在低中频段，不应落到最末的高频段。
        assert!(loudest < 8, "1.5kHz 的峰值位置异常：{loudest}");
        assert!(spectrum.level > 0.05);
    }

    #[test]
    fn silence_stays_flat() {
        let spectrum = bands(&vec![0.0; 2048], 44_100);
        assert!(spectrum.bands.iter().all(|band| *band < 0.001));
        assert!(spectrum.level < 0.001);
    }

    /// 真实回环冒烟：默认跳过，设置 DAOZAI_AUDIO_TESTS=1 时运行并播放系统提示音。
    #[test]
    fn live_loopback_hears_a_system_sound() {
        if std::env::var("DAOZAI_AUDIO_TESTS").is_err() {
            return;
        }
        let service = Service::start();
        #[cfg(windows)]
        unsafe {
            use windows::core::PCWSTR;
            use windows::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_FILENAME};
            let path: Vec<u16> = "C:\\Windows\\Media\\Windows Notify System Generic.wav\0".encode_utf16().collect();
            let _ = PlaySoundW(PCWSTR(path.as_ptr()), None, SND_FILENAME | SND_ASYNC);
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        let mut best = 0.0f32;
        while std::time::Instant::now() < deadline {
            while let Some(event) = service.try_recv() {
                if let Event::Spectrum(spectrum) = event {
                    best = best.max(spectrum.level);
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(best > 0.01, "回环没有捕捉到系统提示音（level={best}）");
    }
}
