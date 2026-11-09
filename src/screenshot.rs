//! 截图：把框选出来的 RGBA 图片保存为 PNG 并复制到系统剪贴板。
//!
//! - `dibv5` / `dib32` 是纯函数，负责生成剪贴板需要的 DIB 数据，便于测试；
//! - PNG 编码复用 `image` crate；
//! - 保存目录优先「图片\岛仔截图」，取不到系统图片目录时回退到数据目录。

use std::path::PathBuf;

pub fn file_name(now: chrono::DateTime<chrono::Local>) -> String {
    format!("岛仔截图-{}.png", now.format("%Y%m%d-%H%M%S"))
}

/// 保存目录：系统「图片」文件夹下的 `岛仔截图`，失败时回退到数据目录。
pub fn pictures_dir() -> PathBuf {
    #[cfg(windows)]
    {
        use windows::Win32::System::Com::CoTaskMemFree;
        use windows::Win32::UI::Shell::{SHGetKnownFolderPath, FOLDERID_Pictures, KF_FLAG_DEFAULT};
        if let Ok(path) = unsafe { SHGetKnownFolderPath(&FOLDERID_Pictures, KF_FLAG_DEFAULT, None) } {
            let text = unsafe { path.to_string() }.unwrap_or_default();
            unsafe { CoTaskMemFree(Some(path.0 as *const _)) };
            if !text.is_empty() {
                return PathBuf::from(text).join("岛仔截图");
            }
        }
    }
    crate::model::config_dir().join("screenshots")
}

pub fn encode_png(image: &crate::ocr::Image) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    let encoder = image::codecs::png::PngEncoder::new(&mut bytes);
    image::ImageEncoder::write_image(
        encoder,
        &image.rgba,
        image.width,
        image.height,
        image::ColorType::Rgba8.into(),
    )
    .map_err(|error| format!("PNG 编码失败：{error}"))?;
    Ok(bytes)
}

pub fn save_png(image: &crate::ocr::Image) -> Result<PathBuf, String> {
    let dir = pictures_dir();
    std::fs::create_dir_all(&dir).map_err(|error| format!("无法创建截图目录：{error}"))?;
    let path = dir.join(file_name(chrono::Local::now()));
    let bytes = encode_png(image)?;
    crate::model::atomic_write(&path, &bytes).map_err(|error| format!("无法保存截图：{error}"))?;
    Ok(path)
}

/// CF_DIBV5：BITMAPV5HEADER + 自底向上的 BGRA 行。
pub fn dibv5(image: &crate::ocr::Image) -> Vec<u8> {
    use windows::Win32::Graphics::Gdi::{BITMAPV5HEADER, BI_BITFIELDS};
    let header_size = std::mem::size_of::<BITMAPV5HEADER>();
    let pixels = image.width as usize * image.height as usize * 4;
    let mut out = vec![0u8; header_size + pixels];
    let header = BITMAPV5HEADER {
        bV5Size: header_size as u32,
        bV5Width: image.width as i32,
        bV5Height: image.height as i32,
        bV5Planes: 1,
        bV5BitCount: 32,
        bV5Compression: BI_BITFIELDS,
        bV5SizeImage: pixels as u32,
        bV5RedMask: 0x00FF_0000,
        bV5GreenMask: 0x0000_FF00,
        bV5BlueMask: 0x0000_00FF,
        bV5AlphaMask: 0xFF00_0000,
        // 'sRGB' 色彩空间常量（windows crate 未导出 LCS_sRGB）。
        bV5CSType: 0x7352_4742,
        ..Default::default()
    };
    let raw = unsafe {
        std::slice::from_raw_parts(&header as *const BITMAPV5HEADER as *const u8, header_size)
    };
    out[..header_size].copy_from_slice(raw);
    write_bgra_bottom_up(image, &mut out[header_size..]);
    out
}

/// CF_DIB：BITMAPINFOHEADER + 自底向上的 BGRA 行（32 位、BI_RGB）。
pub fn dib32(image: &crate::ocr::Image) -> Vec<u8> {
    use windows::Win32::Graphics::Gdi::{BITMAPINFOHEADER, BI_RGB};
    let header_size = std::mem::size_of::<BITMAPINFOHEADER>();
    let pixels = image.width as usize * image.height as usize * 4;
    let mut out = vec![0u8; header_size + pixels];
    let header = BITMAPINFOHEADER {
        biSize: header_size as u32,
        biWidth: image.width as i32,
        biHeight: image.height as i32,
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB.0,
        biSizeImage: pixels as u32,
        ..Default::default()
    };
    let raw = unsafe {
        std::slice::from_raw_parts(&header as *const BITMAPINFOHEADER as *const u8, header_size)
    };
    out[..header_size].copy_from_slice(raw);
    write_bgra_bottom_up(image, &mut out[header_size..]);
    out
}

fn write_bgra_bottom_up(image: &crate::ocr::Image, out: &mut [u8]) {
    let stride = image.width as usize * 4;
    for row in 0..image.height as usize {
        let source_row = image.height as usize - 1 - row;
        let source = &image.rgba[source_row * stride..(source_row + 1) * stride];
        let target = &mut out[row * stride..(row + 1) * stride];
        for (pixel, chunk) in source.chunks_exact(4).zip(target.chunks_exact_mut(4)) {
            chunk[0] = pixel[2];
            chunk[1] = pixel[1];
            chunk[2] = pixel[0];
            chunk[3] = pixel[3];
        }
    }
}

/// 把图片写入系统剪贴板：优先 CF_DIBV5，同时提供 CF_DIB 与注册的 PNG 格式。
#[cfg(windows)]
pub fn copy_to_clipboard(image: &crate::ocr::Image) {
    use windows::core::{HSTRING, PCWSTR};
    use windows::Win32::Foundation::{HANDLE, HWND};
    use windows::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
    };
    use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
    use windows::Win32::System::Ole::{CF_DIB, CF_DIBV5};
    unsafe {
        if OpenClipboard(HWND::default()).is_err() {
            return;
        }
        let _ = EmptyClipboard();
        let set = |format: u32, data: &[u8]| -> bool {
            let Ok(handle) = GlobalAlloc(GMEM_MOVEABLE, data.len()) else {
                return false;
            };
            let pointer = GlobalLock(handle);
            if pointer.is_null() {
                return false;
            }
            std::ptr::copy_nonoverlapping(data.as_ptr(), pointer as *mut u8, data.len());
            let _ = GlobalUnlock(handle);
            SetClipboardData(format, HANDLE(handle.0)).is_ok()
        };
        set(CF_DIBV5.0 as u32, &dibv5(image));
        set(CF_DIB.0 as u32, &dib32(image));
        if let Ok(png) = encode_png(image) {
            let name = HSTRING::from("PNG");
            let format = RegisterClipboardFormatW(PCWSTR(name.as_ptr()));
            if format != 0 {
                set(format, &png);
            }
        }
        let _ = CloseClipboard();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(width: u32, height: u32) -> crate::ocr::Image {
        let mut rgba = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            for x in 0..width {
                rgba.extend_from_slice(&[(x * 10) as u8, (y * 20) as u8, 0x40, 0x80]);
            }
        }
        crate::ocr::Image { width, height, rgba }
    }

    #[test]
    fn dib_headers_and_rows_are_bottom_up_bgra() {
        let image = sample(3, 2);
        let dib = dib32(&image);
        let header = unsafe { &*(dib.as_ptr() as *const windows::Win32::Graphics::Gdi::BITMAPINFOHEADER) };
        assert_eq!(header.biWidth, 3);
        assert_eq!(header.biHeight, 2);
        assert_eq!(header.biBitCount, 32);
        assert_eq!(dib.len(), std::mem::size_of::<windows::Win32::Graphics::Gdi::BITMAPINFOHEADER>() + 3 * 2 * 4);
        // 第一行 BGRA 对应图片最后一行 y=1：r=10*? 第一像素 r=(0*10)=0,g=20,b=0x40,a=0x80
        assert_eq!(&dib[dib.len() - 24..dib.len() - 20], &[0x40, 20, 0, 0x80]);
        // 第二行第一个像素对应 y=0。
        assert_eq!(&dib[dib.len() - 12..dib.len() - 8], &[0x40, 0, 0, 0x80]);
        let v5 = dibv5(&image);
        let header5 = unsafe { &*(v5.as_ptr() as *const windows::Win32::Graphics::Gdi::BITMAPV5HEADER) };
        assert_eq!(header5.bV5Width, 3);
        assert_eq!(header5.bV5AlphaMask, 0xFF00_0000);
        assert_eq!(v5.len(), std::mem::size_of::<windows::Win32::Graphics::Gdi::BITMAPV5HEADER>() + 3 * 2 * 4);
    }

    #[test]
    fn png_encoding_round_trips() {
        let image = sample(5, 4);
        let png = encode_png(&image).expect("PNG 编码应成功");
        assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]);
        let decoded = crate::ocr::decode_png(&png).expect("自己的 PNG 应能解码");
        assert_eq!((decoded.width, decoded.height), (5, 4));
    }

    /// 真实剪贴板往返：默认跳过，设置 DAOZAI_CLIPBOARD_TESTS=1 时运行。
    #[test]
    fn clipboard_round_trip_keeps_image_dimensions() {
        if std::env::var("DAOZAI_CLIPBOARD_TESTS").is_err() {
            return;
        }
        let image = sample(9, 7);
        copy_to_clipboard(&image);
        std::thread::sleep(std::time::Duration::from_millis(120));
        let back = crate::ocr::read_clipboard_image().expect("应能从剪贴板读回截图");
        assert_eq!((back.width, back.height), (9, 7));
    }

    #[test]
    fn file_names_are_sortable_and_chinese() {
        use chrono::TimeZone;
        let now = chrono::Local.with_ymd_and_hms(2026, 11, 10, 5, 12, 34).unwrap();
        assert_eq!(file_name(now), "岛仔截图-20261110-051234.png");
    }
}
