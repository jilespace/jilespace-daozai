//! 贴图持久化：把置顶截图加密保存到本机，下次启动恢复位置、缩放与透明度。
//!
//! 图片以 PNG 编码后用 Windows DPAPI 加密（与剪贴板历史一致），
//! 元数据仅保存窗口位置与比例；是否启用由设置 `remember_pins` 控制。

use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq)]
pub struct StoredPin {
    pub key: String,
    pub x: f32,
    pub y: f32,
    pub zoom: f32,
    pub opacity: f32,
}

pub fn dir() -> PathBuf {
    crate::model::config_dir().join("pins")
}

fn metadata_path(root: &Path) -> PathBuf {
    root.join("pins.json")
}

/// 新的贴图图片文件名（不含扩展名）。
pub fn new_key() -> String {
    static SERIAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let serial = SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!(
        "pin-{}-{serial}",
        chrono::Local::now().format("%Y%m%d-%H%M%S")
    )
}

pub fn serialize(records: &[StoredPin]) -> String {
    let value: Vec<serde_json::Value> = records
        .iter()
        .map(|record| {
            serde_json::json!({
                "key": record.key,
                "x": record.x,
                "y": record.y,
                "zoom": record.zoom,
                "opacity": record.opacity,
            })
        })
        .collect();
    serde_json::to_string(&value).unwrap_or_else(|_| "[]".into())
}

pub fn parse(text: &str) -> Vec<StoredPin> {
    let value: serde_json::Value = serde_json::from_str(text).unwrap_or_default();
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let key = item.get("key")?.as_str()?.trim().to_string();
            if key.is_empty() || key.contains(['/', '\\', '.']) {
                return None;
            }
            Some(StoredPin {
                key,
                x: item.get("x")?.as_f64()? as f32,
                y: item.get("y")?.as_f64()? as f32,
                zoom: (item.get("zoom")?.as_f64()? as f32).clamp(0.4, 3.0),
                opacity: (item.get("opacity")?.as_f64()? as f32).clamp(0.25, 1.0),
            })
        })
        .take(6)
        .collect()
}

pub fn load_records() -> Vec<StoredPin> {
    std::fs::read_to_string(metadata_path(&dir()))
        .map(|text| parse(&text))
        .unwrap_or_default()
}

pub fn save_records(records: &[StoredPin]) -> Result<(), String> {
    let root = dir();
    std::fs::create_dir_all(&root).map_err(|error| format!("无法创建贴图目录：{error}"))?;
    crate::model::atomic_write(&metadata_path(&root), serialize(records).as_bytes())
        .map_err(|error| format!("无法保存贴图位置：{error}"))?;
    // 清掉不再被引用的孤儿图片（例如异常退出遗留）。
    let referenced: std::collections::HashSet<&str> =
        records.iter().map(|record| record.key.as_str()).collect();
    if let Ok(entries) = std::fs::read_dir(&root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("bin") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };
            if !referenced.contains(stem) {
                let _ = std::fs::remove_file(path);
            }
        }
    }
    Ok(())
}

fn image_path(root: &Path, key: &str) -> PathBuf {
    root.join(format!("{key}.bin"))
}

pub fn save_image(root: &Path, key: &str, image: &crate::ocr::Image) -> Result<(), String> {
    std::fs::create_dir_all(root).map_err(|error| format!("无法创建贴图目录：{error}"))?;
    let png = crate::screenshot::encode_png(image)?;
    let protected = crate::clipboard::protect(&png)?;
    let mut body = b"DPAPI".to_vec();
    body.extend_from_slice(&protected);
    crate::model::atomic_write(&image_path(root, key), &body)
        .map_err(|error| format!("无法保存贴图：{error}"))
}

pub fn load_image(root: &Path, key: &str) -> Option<crate::ocr::Image> {
    let bytes = std::fs::read(image_path(root, key)).ok()?;
    let protected = bytes.strip_prefix(b"DPAPI")?;
    let png = crate::clipboard::unprotect(protected).ok()?;
    let image = image::load_from_memory(&png).ok()?.to_rgba8();
    Some(crate::ocr::Image {
        width: image.width(),
        height: image.height(),
        rgba: image.into_raw(),
    })
}

pub fn remove_image(root: &Path, key: &str) {
    let _ = std::fs::remove_file(image_path(root, key));
}

pub fn clear() {
    let _ = std::fs::remove_dir_all(dir());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "daozai-pin-store-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn metadata_round_trip_clamps_and_rejects_bad_keys() {
        let records = vec![StoredPin {
            key: "pin-a".into(),
            x: 120.0,
            y: -40.0,
            zoom: 99.0,
            opacity: 0.0,
        }];
        let parsed = parse(&serialize(&records));
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].key, "pin-a");
        assert_eq!(parsed[0].zoom, 3.0);
        assert_eq!(parsed[0].opacity, 0.25);
        let bad = parse(r#"[{"key":"../evil","x":0,"y":0,"zoom":1,"opacity":1},{"key":"ok","x":1,"y":2,"zoom":1,"opacity":1}]"#);
        assert_eq!(bad.len(), 1);
        assert_eq!(bad[0].key, "ok");
        assert!(parse("not json").is_empty());
    }

    /// 开发用：向当前数据目录写入一张演示贴图，便于手工验证重启恢复。
    #[test]
    #[ignore]
    fn seed_demo_pin_for_manual_check() {
        let root = dir();
        let image = crate::ocr::Image {
            width: 240,
            height: 160,
            rgba: (0..240 * 160 * 4)
                .map(|value| {
                    let pixel = value / 4;
                    match value % 4 {
                        0 => (pixel % 200 + 40) as u8,
                        1 => (pixel / 3 % 180 + 60) as u8,
                        2 => 200,
                        _ => 255,
                    }
                })
                .collect(),
        };
        let key = "pin-demo-restore".to_string();
        save_image(&root, &key, &image).unwrap();
        save_records(&[StoredPin { key, x: 320.0, y: 140.0, zoom: 1.0, opacity: 1.0 }]).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn image_round_trip_is_encrypted() {
        let root = temp_dir("image");
        let image = crate::ocr::Image {
            width: 4,
            height: 3,
            rgba: (0..4 * 3 * 4).map(|value| (value * 7) as u8).collect(),
        };
        save_image(&root, "pin-test", &image).unwrap();
        let raw = std::fs::read(root.join("pin-test.bin")).unwrap();
        assert!(raw.starts_with(b"DPAPI"));
        assert!(!raw.windows(4).any(|window| window == b"\x89PNG"), "磁盘上不能出现明文 PNG");
        let loaded = load_image(&root, "pin-test").unwrap();
        assert_eq!(loaded.width, image.width);
        assert_eq!(loaded.height, image.height);
        assert_eq!(loaded.rgba, image.rgba);
        remove_image(&root, "pin-test");
        assert!(load_image(&root, "pin-test").is_none());
        let _ = std::fs::remove_dir_all(root);
    }
}
