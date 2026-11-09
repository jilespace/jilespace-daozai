//! 剪贴板图片 OCR：读取剪贴板里的 DIB 图片，用 Windows 本地 OCR 识别文字。
//!
//! 识别完全在本机完成，不上传图片。DIB 解析是纯函数，便于用合成数据回归；
//! WinRT 部分只在 Windows 上编译。

pub const MAX_PIXELS: usize = 16_000_000;
pub const HOTKEY_TEXT: &str = "Ctrl+Alt+O";
const MAX_SIDE: i64 = 10_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    /// RGBA8，alpha 固定为 255，方便直接交给 OCR。
    pub rgba: Vec<u8>,
}

fn u16_at(bytes: &[u8], offset: usize) -> Option<u16> {
    let slice = bytes.get(offset..offset + 2)?;
    Some(u16::from_le_bytes([slice[0], slice[1]]))
}

fn u32_at(bytes: &[u8], offset: usize) -> Option<u32> {
    let slice = bytes.get(offset..offset + 4)?;
    Some(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

fn i32_at(bytes: &[u8], offset: usize) -> Option<i32> {
    u32_at(bytes, offset).map(|value| value as i32)
}

/// 把剪贴板里的 `CF_DIB` 字节解析成 RGBA。
///
/// 支持 1/4/8/24/32 位、调色板、自底向上与自顶向下，以及 BI_RGB / BI_BITFIELDS。
pub fn decode_dib(bytes: &[u8]) -> Result<Image, String> {
    let header_size = u32_at(bytes, 0).ok_or("图片数据不完整")? as usize;
    if header_size < 40 {
        return Err("图片格式暂不支持（需要 BITMAPINFOHEADER）".into());
    }
    let width = i32_at(bytes, 4).ok_or("图片数据不完整")?;
    let height_raw = i32_at(bytes, 8).ok_or("图片数据不完整")?;
    let planes = u16_at(bytes, 12).ok_or("图片数据不完整")?;
    let bit_count = u16_at(bytes, 14).ok_or("图片数据不完整")?;
    let compression = u32_at(bytes, 16).ok_or("图片数据不完整")?;
    if planes != 1 {
        return Err("图片平面数异常".into());
    }
    if width <= 0 || height_raw == 0 {
        return Err("图片尺寸无效".into());
    }
    let width = width as i64;
    let top_down = height_raw < 0;
    let height = height_raw.unsigned_abs() as i64;
    if width > MAX_SIDE || height > MAX_SIDE || width * height > MAX_PIXELS as i64 {
        return Err("图片太大，请先裁剪再识别".into());
    }
    if !matches!(bit_count, 1 | 4 | 8 | 24 | 32) {
        return Err(format!("暂不支持 {bit_count} 位图片"));
    }
    if !matches!(compression, 0 | 3) {
        return Err("图片压缩格式暂不支持（仅支持未压缩 BMP/DIB）".into());
    }
    if compression == 3 && bit_count != 32 {
        return Err("暂不支持该位深的分段掩码图片".into());
    }

    // 32 位分段掩码：读取头部之后的三个掩码，缺省按 BGRA 处理。
    let (red_mask, green_mask, blue_mask) = if compression == 3 {
        // BI_BITFIELDS 的三个掩码固定在 BITMAPINFOHEADER 之后（offset 40），
        // 即使 DIBV5 的 header 更长，也仍然读这里。
        let offset = if header_size >= 52 { 40 } else { header_size };
        (
            u32_at(bytes, offset).unwrap_or(0x00FF_0000),
            u32_at(bytes, offset + 4).unwrap_or(0x0000_FF00),
            u32_at(bytes, offset + 8).unwrap_or(0x0000_00FF),
        )
    } else if bit_count == 24 || bit_count == 32 {
        (0x00FF_0000, 0x0000_FF00, 0x0000_00FF)
    } else {
        (0, 0, 0)
    };

    let mut data_offset = header_size;
    // 40 字节头 + BI_BITFIELDS 时，三个掩码存放在头部之后、像素数据之前。
    if compression == 3 && header_size == 40 {
        data_offset += 12;
    }
    let mut palette: Vec<(u8, u8, u8)> = Vec::new();
    if bit_count <= 8 {
        let colors = u32_at(bytes, 32).unwrap_or(0) as usize;
        let entries = if colors == 0 { 1usize << bit_count } else { colors.min(1 << bit_count) };
        data_offset += entries * 4;
        for index in 0..entries {
            let base = header_size + index * 4;
            let blue = bytes.get(base).copied().unwrap_or(0);
            let green = bytes.get(base + 1).copied().unwrap_or(0);
            let red = bytes.get(base + 2).copied().unwrap_or(0);
            palette.push((red, green, blue));
        }
    }
    let stride = ((width * bit_count as i64 + 31) / 32) * 4;
    let needed = data_offset as i64 + stride * height;
    if needed > bytes.len() as i64 {
        return Err("图片数据不完整".into());
    }

    let mut rgba = vec![0u8; (width * height * 4) as usize];
    for row in 0..height {
        let source_row = if top_down { row } else { height - 1 - row };
        let row_start = data_offset + (source_row * stride) as usize;
        for column in 0..width {
            let pixel = match bit_count {
                1 | 4 | 8 => {
                    let bit_index = column * bit_count as i64;
                    let byte_index = row_start + (bit_index / 8) as usize;
                    let shift = 8 - bit_count as i32 - (bit_index % 8) as i32;
                    let byte = bytes.get(byte_index).copied().unwrap_or(0);
                    let mask = (1u16 << bit_count) - 1;
                    let value = ((byte as u16 >> shift.max(0)) & mask) as usize;
                    palette.get(value).copied().unwrap_or((0, 0, 0))
                }
                24 => {
                    let index = row_start + (column * 3) as usize;
                    let blue = bytes.get(index).copied().unwrap_or(0);
                    let green = bytes.get(index + 1).copied().unwrap_or(0);
                    let red = bytes.get(index + 2).copied().unwrap_or(0);
                    (red, green, blue)
                }
                32 => {
                    let index = row_start + (column * 4) as usize;
                    let value = u32_at(bytes, index).unwrap_or(0);
                    (
                        mask_component(value, red_mask),
                        mask_component(value, green_mask),
                        mask_component(value, blue_mask),
                    )
                }
                _ => unreachable!(),
            };
            let index = ((row * width + column) * 4) as usize;
            rgba[index] = pixel.0;
            rgba[index + 1] = pixel.1;
            rgba[index + 2] = pixel.2;
            rgba[index + 3] = 255;
        }
    }
    Ok(Image { width: width as u32, height: height as u32, rgba })
}

fn mask_component(pixel: u32, mask: u32) -> u8 {
    if mask == 0 {
        return 255;
    }
    let shift = mask.trailing_zeros();
    let max = (mask >> shift) as u64;
    if max == 0 {
        return 0;
    }
    let value = ((pixel & mask) >> shift) as u64;
    ((value * 255 + max / 2) / max).min(255) as u8
}

/// 解码剪贴板里的 PNG（部分浏览器和截图工具只提供 PNG 格式）。
pub fn decode_png(bytes: &[u8]) -> Result<Image, String> {
    let decoded = image::load_from_memory_with_format(bytes, image::ImageFormat::Png)
        .map_err(|_| "PNG 图片解码失败".to_string())?;
    let rgba = decoded.to_rgba8();
    let (width, height) = rgba.dimensions();
    if width == 0 || height == 0 {
        return Err("图片尺寸无效".into());
    }
    if width as usize * height as usize > MAX_PIXELS {
        return Err("图片太大，请先裁剪再识别".into());
    }
    Ok(Image { width, height, rgba: rgba.into_raw() })
}

/// OCR 结果清洗：统一换行、去掉行尾空白、压缩连续空行。
pub fn clean_text(raw: &str) -> String {
    let normalized = raw.replace("\r\n", "\n").replace('\r', "\n");
    let mut lines: Vec<String> = Vec::new();
    let mut blank = 0usize;
    for line in normalized.lines() {
        let trimmed = line.replace('\u{feff}', "").trim().to_string();
        if trimmed.trim().is_empty() {
            blank += 1;
            if blank > 1 {
                continue;
            }
            lines.push(String::new());
        } else {
            blank = 0;
            lines.push(trimmed);
        }
    }
    while lines.first().is_some_and(|line| line.is_empty()) {
        lines.remove(0);
    }
    while lines.last().is_some_and(|line| line.is_empty()) {
        lines.pop();
    }
    lines.join("\n")
}

/// 单个汉字/字母的大小，用来判断识别结果是否可用。
pub fn char_count(text: &str) -> usize {
    text.chars().filter(|ch| !ch.is_whitespace()).count()
}

#[cfg(windows)]
mod windows_impl {
    use super::{char_count, clean_text, Image};
    use std::time::Duration;

    use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
    use windows::Media::Ocr::OcrEngine;
    use windows::Security::Cryptography::CryptographicBuffer;
    use windows::Win32::Foundation::{HGLOBAL, HWND};
    use windows::Win32::System::DataExchange::{
        CloseClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
    };
    use windows::core::w;
    use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
    use windows::Win32::System::Ole::{CF_DIB, CF_DIBV5};
    use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};

    /// 从剪贴板读取图片。按 DIB → DIBV5 → 注册格式 PNG 的顺序尝试，
    /// 兼容截图工具、浏览器与图像编辑器各自的剪贴板格式。
    pub fn read_clipboard_image() -> Result<Image, String> {
        let mut last_error = "剪贴板正被占用，请重试".to_string();
        for attempt in 0..5 {
            let opened = unsafe { OpenClipboard(HWND::default()) }.is_ok();
            if !opened {
                std::thread::sleep(Duration::from_millis(20));
                continue;
            }
            let result = (|| -> Result<Image, String> {
                let dib = CF_DIB.0 as u32;
                let dibv5 = CF_DIBV5.0 as u32;
                let png = unsafe { windows::Win32::System::DataExchange::RegisterClipboardFormatW(w!("PNG")) };
                let has = |format: u32| unsafe { IsClipboardFormatAvailable(format) }.is_ok();
                let (format, is_png) = if has(dib) {
                    (dib, false)
                } else if has(dibv5) {
                    (dibv5, false)
                } else if png != 0 && has(png) {
                    (png, true)
                } else {
                    return Err("剪贴板里没有图片".into());
                };
                let handle = unsafe { GetClipboardData(format) }
                    .map_err(|_| "无法读取剪贴板图片".to_string())?;
                if handle.0.is_null() {
                    return Err("剪贴板图片为空".into());
                }
                let global = HGLOBAL(handle.0);
                let pointer = unsafe { GlobalLock(global) } as *const u8;
                if pointer.is_null() {
                    return Err("无法锁定剪贴板图片".into());
                }
                let size = unsafe { GlobalSize(global) };
                let bytes = if size == 0 { Vec::new() } else { unsafe { std::slice::from_raw_parts(pointer, size).to_vec() } };
                unsafe {
                    let _ = GlobalUnlock(global);
                }
                if bytes.is_empty() {
                    return Err("剪贴板图片为空".into());
                }
                if is_png {
                    super::decode_png(&bytes)
                } else {
                    super::decode_dib(&bytes)
                }
            })();
            unsafe {
                let _ = CloseClipboard();
            }
            match result {
                Ok(image) => return Ok(image),
                Err(error) => {
                    last_error = error;
                    if attempt == 4 {
                        return Err(last_error);
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
        }
        Err(last_error)
    }

    /// OCR 引擎是否可用（取决于系统已安装的语言包）。
    pub fn engine_available() -> bool {
        OcrEngine::TryCreateFromUserProfileLanguages().is_ok()
    }

    /// 直接识别 RGBA 图像，供端到端测试与截图取字复用。
    pub fn recognize_rgba(rgba: &[u8], width: u32, height: u32) -> Result<String, String> {
        if width == 0 || height == 0 || rgba.len() < (width * height * 4) as usize {
            return Err("图片数据无效".into());
        }
        let _ = unsafe { RoInitialize(RO_INIT_MULTITHREADED) };
        let mut bgra = Vec::with_capacity((width * height * 4) as usize);
        for pixel in rgba.chunks_exact(4) {
            bgra.extend_from_slice(&[pixel[2], pixel[1], pixel[0], 255]);
        }
        let buffer =
            CryptographicBuffer::CreateFromByteArray(&bgra).map_err(|error| format!("无法准备图片：{error}"))?;
        let bitmap = SoftwareBitmap::CreateCopyFromBuffer(
            &buffer,
            BitmapPixelFormat::Bgra8,
            width as i32,
            height as i32,
        )
        .map_err(|error| format!("无法创建位图：{error}"))?;
        let engine = OcrEngine::TryCreateFromUserProfileLanguages()
            .map_err(|_| "Windows OCR 语言包不可用，请在系统设置中安装语言包".to_string())?;
        let result = engine
            .RecognizeAsync(&bitmap)
            .and_then(|operation| operation.get())
            .map_err(|error| format!("文字识别失败：{error}"))?;
        let text = result.Text().map_err(|error| format!("无法读取识别结果：{error}"))?;
        Ok(clean_text(&text.to_string_lossy()))
    }

    /// 读取剪贴板图片并识别文字。
    pub fn recognize_clipboard() -> Result<String, String> {
        let image = read_clipboard_image()?;
        let text = recognize_rgba(&image.rgba, image.width, image.height)?;
        if char_count(&text) == 0 {
            return Err("没有识别到文字".into());
        }
        Ok(text)
    }
}

#[cfg(windows)]
pub use windows_impl::{engine_available, read_clipboard_image, recognize_clipboard, recognize_rgba};

#[cfg(not(windows))]
pub fn read_clipboard_image() -> Result<Image, String> {
    Err("图片识别仅支持 Windows".into())
}

#[cfg(not(windows))]
pub fn recognize_rgba(_rgba: &[u8], _width: u32, _height: u32) -> Result<String, String> {
    Err("图片识别仅支持 Windows".into())
}

#[cfg(not(windows))]
pub fn recognize_clipboard() -> Result<String, String> {
    Err("图片识别仅支持 Windows".into())
}

#[cfg(not(windows))]
pub fn engine_available() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造一个最简单的 DIB：BITMAPINFOHEADER + 调色板/像素。
    fn dib(width: i32, height: i32, bit_count: u16, pixels: &[u8], palette: &[(u8, u8, u8)]) -> Vec<u8> {
        let stride = (((width.abs() as usize * bit_count as usize) + 31) / 32) * 4;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&40u32.to_le_bytes());
        bytes.extend_from_slice(&width.to_le_bytes());
        bytes.extend_from_slice(&height.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&bit_count.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&((stride * height.unsigned_abs() as usize) as u32).to_le_bytes());
        bytes.extend_from_slice(&2835i32.to_le_bytes());
        bytes.extend_from_slice(&2835i32.to_le_bytes());
        bytes.extend_from_slice(&(palette.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        for (red, green, blue) in palette {
            bytes.extend_from_slice(&[*blue, *green, *red, 0]);
        }
        bytes.extend_from_slice(pixels);
        bytes
    }

    #[test]
    fn decodes_bottom_up_24_bit_frames() {
        // 2x2：底行红、绿；顶行蓝、白（BGR 顺序 + 4 字节行对齐）。
        let pixels = [0u8, 0, 255, 0, 255, 0, 0, 0, 255, 0, 0, 255, 255, 255, 0, 0];
        let image = decode_dib(&dib(2, 2, 24, &pixels, &[])).unwrap();
        assert_eq!((image.width, image.height), (2, 2));
        assert_eq!(&image.rgba[0..4], &[0, 0, 255, 255], "左上应为蓝色");
        assert_eq!(&image.rgba[4..8], &[255, 255, 255, 255], "右上应为白色");
        assert_eq!(&image.rgba[8..12], &[255, 0, 0, 255], "左下应为红色");
        assert_eq!(&image.rgba[12..16], &[0, 255, 0, 255], "右下应为绿色");
    }

    #[test]
    fn decodes_top_down_32_bit_frames_and_forces_opaque_alpha() {
        let red = [0u8, 0, 255, 0];
        let green = [0u8, 255, 0, 0];
        let mut pixels = Vec::new();
        pixels.extend_from_slice(&red);
        pixels.extend_from_slice(&green);
        let image = decode_dib(&dib(2, -1, 32, &pixels, &[])).unwrap();
        assert_eq!(&image.rgba[0..4], &[255, 0, 0, 255]);
        assert_eq!(&image.rgba[4..8], &[0, 255, 0, 255]);
    }

    #[test]
    fn decodes_paletted_8_bit_frames() {
        let palette = [(255u8, 0u8, 0u8), (0, 0, 255)];
        let image = decode_dib(&dib(2, 1, 8, &[1, 0, 0, 0], &palette)).unwrap();
        assert_eq!(&image.rgba[0..4], &[0, 0, 255, 255]);
        assert_eq!(&image.rgba[4..8], &[255, 0, 0, 255]);
    }

    #[test]
    fn decodes_32_bit_bitfield_frames_with_explicit_masks() {
        // 1x1 蓝色：像素 0x000000FF，BI_BITFIELDS 掩码放在头部之后。
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&40u32.to_le_bytes());
        bytes.extend_from_slice(&1i32.to_le_bytes());
        bytes.extend_from_slice(&1i32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&32u16.to_le_bytes());
        bytes.extend_from_slice(&3u32.to_le_bytes());
        bytes.extend_from_slice(&4u32.to_le_bytes());
        bytes.extend_from_slice(&2835i32.to_le_bytes());
        bytes.extend_from_slice(&2835i32.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&0x00FF_0000u32.to_le_bytes());
        bytes.extend_from_slice(&0x0000_FF00u32.to_le_bytes());
        bytes.extend_from_slice(&0x0000_00FFu32.to_le_bytes());
        bytes.extend_from_slice(&0x0000_00FFu32.to_le_bytes());
        let image = decode_dib(&bytes).unwrap();
        assert_eq!(&image.rgba[0..4], &[0, 0, 255, 255]);
    }

    #[test]
    fn random_or_truncated_dib_bytes_never_panic() {
        let mut state = 0x1234_5678_9abc_def0u64;
        let mut next = || {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (state >> 33) as u8
        };
        for len in 0..384 {
            let bytes: Vec<u8> = (0..len).map(|_| next()).collect();
            let _ = decode_dib(&bytes);
        }
        let valid = dib(6, 4, 24, &[0u8; 96], &[]);
        for cut in 0..valid.len() {
            let _ = decode_dib(&valid[..cut]);
        }
        assert!(decode_dib(&valid).is_ok());
    }

    #[test]
    fn decodes_png_images_from_browsers() {
        let mut source = image::RgbaImage::new(3, 2);
        source.put_pixel(0, 0, image::Rgba([255, 0, 0, 255]));
        source.put_pixel(2, 1, image::Rgba([0, 128, 255, 255]));
        let mut cursor = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(source).write_to(&mut cursor, image::ImageFormat::Png).unwrap();
        let decoded = decode_png(&cursor.into_inner()).unwrap();
        assert_eq!((decoded.width, decoded.height), (3, 2));
        assert_eq!(&decoded.rgba[0..4], &[255, 0, 0, 255]);
        assert!(decode_png(&[0, 1, 2, 3]).is_err());
    }

    #[test]
    fn rejects_truncated_oversized_and_compressed_images() {
        assert!(decode_dib(&[]).is_err());
        assert!(decode_dib(&dib(4, 4, 24, &[0; 4], &[])).is_err(), "像素数据不完整应报错");
        let mut compressed = dib(1, 1, 24, &[0; 4], &[]);
        compressed[16..20].copy_from_slice(&1u32.to_le_bytes());
        assert!(decode_dib(&compressed).is_err());
        let huge = dib(9000, 9000, 32, &[], &[]);
        assert!(decode_dib(&huge).is_err(), "超大图片应被拒绝");
    }

    #[test]
    fn cleans_ocr_text_newlines_and_blank_lines() {
        assert_eq!(clean_text("  hello  \r\n\r\n\r\nworld \n"), "hello\n\nworld");
        assert_eq!(clean_text("\u{feff}第一行\n第二行"), "第一行\n第二行");
        assert_eq!(clean_text(""), "");
        assert_eq!(char_count("你 好\nabc "), 5);
    }

    #[cfg(windows)]
    #[test]
    fn windows_ocr_recognizes_rendered_text_end_to_end() {
        use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Vec2};
        if !engine_available() {
            eprintln!("本机没有 OCR 语言包，跳过端到端识别测试");
            return;
        }
        let ctx = egui::Context::default();
        crate::theme::load_fonts(&ctx);
        let (width, height) = (520u32, 140u32);
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(width as f32, height as f32))),
                viewports: std::iter::once((
                    egui::ViewportId::ROOT,
                    egui::ViewportInfo { native_pixels_per_point: Some(1.0), ..Default::default() },
                ))
                .collect(),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::new().fill(Color32::BLACK))
                    .show(ctx, |ui| {
                        ui.painter().text(
                            Pos2::new(24.0, height as f32 / 2.0),
                            Align2::LEFT_CENTER,
                            "HELLO 12345",
                            FontId::proportional(56.0),
                            Color32::WHITE,
                        );
                    });
            },
        );
        let mut raster = crate::preview_raster::Raster::new(width as usize, height as usize);
        raster.update_textures(&output.textures_delta);
        let primitives = ctx.tessellate(output.shapes, 1.0);
        raster.draw(&primitives, 1.0);
        let mut rgba = Vec::new();
        raster.to_rgba8(&mut rgba);
        let text = recognize_rgba(&rgba, width, height).expect("OCR 应成功");
        assert!(
            text.to_ascii_uppercase().contains("12345") || text.contains("12345"),
            "OCR 结果应包含渲染的数字：{text:?}"
        );
    }
}
