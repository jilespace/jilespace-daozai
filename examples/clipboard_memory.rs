//! Reproducible, isolated clipboard memory probe. Never reads/writes the system clipboard.
//! cargo run --release --example clipboard_memory -- prepare <empty fixture directory>
//! cargo run --release --example clipboard_memory -- measure <fixture directory>
#[cfg(windows)]
fn main() {
    use daozai::{clipboard, clipboard_ui, ocr};
    use eframe::egui;
    use std::time::{Duration, Instant};
    use windows::Win32::System::{
        ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX},
        Threading::GetCurrentProcess,
    };
    fn memory() -> (usize, usize) {
        let mut value = PROCESS_MEMORY_COUNTERS_EX::default();
        unsafe {
            GetProcessMemoryInfo(
                GetCurrentProcess(),
                &mut value as *mut _ as _,
                std::mem::size_of_val(&value) as u32,
            )
            .unwrap();
        }
        (value.WorkingSetSize, value.PrivateUsage)
    }
    let args: Vec<String> = std::env::args().collect();
    let root = std::path::PathBuf::from(args.get(2).expect("fixture directory required"));
    let manifest = root.join("memory-fixture.json");
    std::env::set_var("DAOZAI_DATA_DIR", &root);
    if args.get(1).map(String::as_str) == Some("prepare") {
        assert!(!root.exists(), "prepare requires a new directory");
        std::fs::create_dir_all(&root).unwrap();
        let mut rgba = vec![0u8; 3840 * 2160 * 4];
        for (index, pixel) in rgba.chunks_exact_mut(4).enumerate() {
            pixel.copy_from_slice(&[(index % 251) as u8, ((index / 3840) % 253) as u8, 160, 255]);
        }
        let image = clipboard::store_image(&ocr::Image {
            width: 3840,
            height: 2160,
            rgba,
        })
        .unwrap();
        std::fs::write(
            manifest,
            serde_json::to_vec(
                &serde_json::json!({"key":image.key,"width":image.width,"height":image.height}),
            )
            .unwrap(),
        )
        .unwrap();
        println!("Prepared synthetic 4K image in {}", root.display());
        return;
    }
    assert_eq!(args.get(1).map(String::as_str), Some("measure"));
    let data: serde_json::Value =
        serde_json::from_slice(&std::fs::read(manifest).unwrap()).unwrap();
    let reference = clipboard::ImageRef {
        key: data["key"].as_str().unwrap().into(),
        width: 3840,
        height: 2160,
    };
    let ctx = egui::Context::default();
    daozai::theme::load_fonts(&ctx);
    let mut popup = clipboard_ui::Popup::default();
    let items = (0..24)
        .map(|index| clipboard::Item {
            id: format!("synthetic-{index}"),
            text: "图片 3840×2160".into(),
            copied_at: 0,
            pinned: false,
            image: Some(reference.clone()),
        })
        .collect();
    let start = Instant::now();
    let baseline = memory();
    popup.set_items(items);
    let mut records = Vec::new();
    let mut phase = 0;
    let mut peak = baseline;
    loop {
        let elapsed = start.elapsed();
        if popup.open {
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(560.0, 520.0),
                    )),
                    time: Some(elapsed.as_secs_f64()),
                    ..Default::default()
                },
                |ctx| {
                    popup.panel_ui(ctx, false);
                },
            );
        }
        let value = memory();
        peak = (peak.0.max(value.0), peak.1.max(value.1));
        if elapsed >= Duration::from_secs((phase + 1) * 5) {
            records.push(serde_json::json!({"phase": (["closed_history", "open_list", "closed_again"][phase as usize]), "rss_mib":value.0 as f64/1048576.0,"private_mib":value.1 as f64/1048576.0,"textures":ctx.tex_manager().read().num_allocated()}));
            phase += 1;
            if phase == 1 {
                popup.open_state(true, false, None);
            }
            if phase == 2 {
                popup.close();
            }
            if phase == 3 {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    println!("{}", serde_json::to_string_pretty(&serde_json::json!({"baseline_rss_mib":baseline.0 as f64/1048576.0,"baseline_private_mib":baseline.1 as f64/1048576.0,"sampled_peak_rss_mib":peak.0 as f64/1048576.0,"sampled_peak_private_mib":peak.1 as f64/1048576.0,"phases":records})).unwrap());
}
#[cfg(not(windows))]
fn main() {
    eprintln!("Windows-only memory probe");
}
