//! 屏幕区域截取：给「框选取字」提供显示器几何、屏幕位图与裁剪。
//!
//! 截屏使用 GDI `BitBlt`，只在打开框选层之前抓一次整屏，之后从内存裁剪，
//! 这样框选界面不会被拍进识别图片，也没有隐藏窗口的时序竞争。

use crate::ocr::Image;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MonitorRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// 从整屏图片里裁出一块；坐标是相对整屏左上角的物理像素。
pub fn crop(image: &Image, x: i32, y: i32, width: u32, height: u32) -> Result<Image, String> {
    if width < 2 || height < 2 {
        return Err("选区太小，请重新框选".into());
    }
    let left = x.max(0);
    let top = y.max(0);
    let right = (x + width as i32).min(image.width as i32);
    let bottom = (y + height as i32).min(image.height as i32);
    if right - left < 2 || bottom - top < 2 {
        return Err("选区超出屏幕范围".into());
    }
    let (width, height) = ((right - left) as u32, (bottom - top) as u32);
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    for row in 0..height {
        let source = (((top as u32 + row) * image.width) + left as u32) as usize * 4;
        let end = source + (width * 4) as usize;
        rgba.extend_from_slice(&image.rgba[source..end]);
    }
    Ok(Image { width, height, rgba })
}

#[cfg(windows)]
mod windows_impl {
    use super::MonitorRect;
    use crate::ocr::Image;
    use windows::Win32::Foundation::POINT;
    use windows::Win32::Graphics::Gdi::{
        BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC,
        GetDIBits, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CAPTUREBLT,
        DIB_RGB_COLORS, HGDIOBJ, SRCCOPY,
    };
    use windows::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromPoint, MONITOR_DEFAULTTONEAREST, MONITORINFO,
    };
    use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;

    /// 光标所在显示器的物理矩形。
    pub fn monitor_at_cursor() -> Option<MonitorRect> {
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
            let rect = info.rcMonitor;
            Some(MonitorRect {
                x: rect.left,
                y: rect.top,
                width: (rect.right - rect.left).max(1) as u32,
                height: (rect.bottom - rect.top).max(1) as u32,
            })
        }
    }

    /// 抓取显示器上的一块区域，返回 RGBA（alpha 恒为 255）。
    pub fn capture_region(rect: MonitorRect) -> Result<Image, String> {
        let width = rect.width as i32;
        let height = rect.height as i32;
        if width <= 0 || height <= 0 {
            return Err("屏幕尺寸无效".into());
        }
        unsafe {
            let screen = GetDC(None);
            if screen.is_invalid() {
                return Err("无法获取屏幕设备上下文".into());
            }
            let memory = CreateCompatibleDC(screen);
            if memory.is_invalid() {
                let _ = ReleaseDC(None, screen);
                return Err("无法创建内存画布".into());
            }
            let bitmap = CreateCompatibleBitmap(screen, width, height);
            let previous = SelectObject(memory, HGDIOBJ(bitmap.0));
            let copied = BitBlt(
                memory,
                0,
                0,
                width,
                height,
                screen,
                rect.x,
                rect.y,
                SRCCOPY | CAPTUREBLT,
            );
            let mut info = BITMAPINFO::default();
            info.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
            info.bmiHeader.biWidth = width;
            // 负高度 = 自上而下，和我们的 RGBA 行序一致。
            info.bmiHeader.biHeight = -height;
            info.bmiHeader.biPlanes = 1;
            info.bmiHeader.biBitCount = 32;
            info.bmiHeader.biCompression = BI_RGB.0;
            let mut bgra = vec![0u8; (width * height * 4) as usize];
            let lines = GetDIBits(
                memory,
                bitmap,
                0,
                height as u32,
                Some(bgra.as_mut_ptr().cast()),
                &mut info,
                DIB_RGB_COLORS,
            );
            if !previous.0.is_null() {
                SelectObject(memory, previous);
            }
            let _ = DeleteObject(HGDIOBJ(bitmap.0));
            let _ = DeleteDC(memory);
            let _ = ReleaseDC(None, screen);
            if copied.is_err() || lines == 0 {
                return Err("无法截取屏幕".into());
            }
            let mut rgba = Vec::with_capacity(bgra.len());
            for pixel in bgra.chunks_exact(4) {
                rgba.extend_from_slice(&[pixel[2], pixel[1], pixel[0], 255]);
            }
            Ok(Image { width: rect.width, height: rect.height, rgba })
        }
    }
}

#[cfg(windows)]
pub use windows_impl::{capture_region, monitor_at_cursor};

#[cfg(not(windows))]
pub fn monitor_at_cursor() -> Option<MonitorRect> {
    None
}

#[cfg(not(windows))]
pub fn capture_region(_rect: MonitorRect) -> Result<Image, String> {
    Err("屏幕截取仅支持 Windows".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(width: u32, height: u32) -> Image {
        let mut rgba = Vec::new();
        for y in 0..height {
            for x in 0..width {
                rgba.extend_from_slice(&[x as u8, y as u8, (x + y) as u8, 255]);
            }
        }
        Image { width, height, rgba }
    }

    #[test]
    fn crop_extracts_the_requested_window() {
        let source = image(10, 8);
        let cropped = crop(&source, 2, 3, 4, 3).unwrap();
        assert_eq!((cropped.width, cropped.height), (4, 3));
        assert_eq!(&cropped.rgba[0..4], &[2, 3, 5, 255]);
        let last = (2 * 4 + 3) * 4;
        assert_eq!(&cropped.rgba[last..last + 4], &[5, 5, 10, 255]);
    }

    #[test]
    fn crop_clamps_partial_selections_and_rejects_tiny_ones() {
        let source = image(6, 6);
        let clamped = crop(&source, 4, 4, 10, 10).unwrap();
        assert_eq!((clamped.width, clamped.height), (2, 2));
        assert!(crop(&source, 0, 0, 1, 1).is_err(), "过小的选区应被拒绝");
        assert!(crop(&source, 100, 100, 10, 10).is_err(), "完全出界应被拒绝");
        assert!(crop(&source, 0, 0, 6, 6).is_ok());
    }

    #[test]
    fn random_crop_windows_stay_in_bounds_without_panicking() {
        let source = image(64, 48);
        let mut state = 7u64;
        let mut next = || {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            (state >> 40) as i64
        };
        for _ in 0..400 {
            let x = (next() % 220 - 110) as i32;
            let y = (next() % 220 - 110) as i32;
            let width = (next() % 180) as u32;
            let height = (next() % 180) as u32;
            if let Ok(out) = crop(&source, x, y, width, height) {
                assert!(out.width <= 64 && out.height <= 48);
                assert_eq!(out.rgba.len() as u32, out.width * out.height * 4);
            }
        }
    }

    #[cfg(windows)]
    #[test]
    fn captures_a_screen_region_with_opaque_pixels() {
        let region = MonitorRect { x: 8, y: 8, width: 96, height: 64 };
        match capture_region(region) {
            Ok(captured) => {
                assert_eq!((captured.width, captured.height), (96, 64));
                assert_eq!(captured.rgba.len(), 96 * 64 * 4);
                assert!(captured.rgba.chunks_exact(4).all(|pixel| pixel[3] == 255));
            }
            Err(error) => eprintln!("当前会话无法截屏，跳过：{error}"),
        }
    }
}
