//! Native Windows composition regression check (requires an unlocked desktop).
//! Run: cargo run --locked --example viewport_transparency
//! Uses synthetic content only; never starts services or reads clipboard history.

#[cfg(windows)]
fn main() -> eframe::Result {
    use eframe::egui::{self, Color32, Pos2, Vec2};
    use std::time::{Duration, Instant};

    const BACKGROUND: Color32 = Color32::from_rgb(40, 100, 150);
    struct Probe {
        started: Instant,
        phase: usize,
        clipboard: daozai::clipboard_ui::Popup,
        failures: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    }

    impl eframe::App for Probe {
        fn clear_color(&self, _: &egui::Visuals) -> [f32; 4] {
            BACKGROUND.to_normalized_gamma_f32()
        }

        fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
            // A dropped viewport between phases exercises close/reopen as well.
            if self.started.elapsed() < Duration::from_millis(250) {
                ctx.request_repaint_after(Duration::from_millis(50));
                return;
            }
            let origin = ctx.input(|i| i.viewport().inner_rect.unwrap().min);
            let transparent = self.phase != 1;
            let id = egui::ViewportId::from_hash_of(("transparency-probe", self.phase));
            let mut geometry = None;
            if self.phase >= 3 {
                let _ = self.clipboard.show(ctx, self.phase == 4);
                geometry = ctx
                    .input(|i| {
                        i.raw
                            .viewports
                            .get(&egui::ViewportId::from_hash_of("daozai-clipboard"))
                            .and_then(|v| v.inner_rect)
                    })
                    .map(|rect| (rect, ctx.pixels_per_point()));
                if let Some((rect, _)) = geometry {
                    // Put the synthetic backdrop behind the actual cursor-positioned popup.
                    let position = rect.min - Vec2::splat(50.0);
                    if origin.distance(position) > 1.0 {
                        ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(position));
                    }
                }
            } else {
                ctx.show_viewport_immediate(
                id,
                egui::ViewportBuilder::default()
                    .with_title("Daozai transparency regression")
                    .with_position(origin + Vec2::splat(50.0))
                    .with_inner_size([560.0, 520.0])
                    .with_decorations(false)
                    .with_resizable(false)
                    .with_transparent(transparent)
                    .with_always_on_top(),
                |ctx, _| {
                    geometry = ctx.input(|i| i.viewport().inner_rect)
                        .map(|rect| (rect, ctx.pixels_per_point()));
                    egui::CentralPanel::default()
                        .frame(egui::Frame::new().fill(Color32::TRANSPARENT).inner_margin(10.0))
                        .show(ctx, |ui| {
                            egui::Frame::new()
                                .fill(Color32::WHITE)
                                .corner_radius(18.0)
                                .inner_margin(18.0)
                                .show(ui, |ui| {
                                    ui.set_min_size(ui.available_size());
                                    ui.heading("Native transparent viewport regression");
                                    ui.label("The blue backdrop must show through the padding and corners.");
                                });
                        });
                },
                );
            }
            if self.started.elapsed() >= Duration::from_secs(2) {
                let (rect, ppp) = geometry.expect("child viewport geometry");
                let capture =
                    daozai::region_capture::capture_region(daozai::region_capture::MonitorRect {
                        x: (rect.min.x * ppp).round() as i32,
                        y: (rect.min.y * ppp).round() as i32,
                        width: (rect.width() * ppp).round() as u32,
                        height: (rect.height() * ppp).round() as u32,
                    })
                    .expect("capture native composition");
                let pixel = |point: Pos2| {
                    let x = (point.x * ppp).round() as u32;
                    let y = (point.y * ppp).round() as u32;
                    let offset = ((y * capture.width + x) * 4) as usize;
                    capture.rgba[offset..offset + 3].to_vec()
                };
                let expected = if transparent {
                    vec![40, 100, 150]
                } else {
                    vec![0, 0, 0]
                };
                // All four edges and rounded corners, plus an opaque panel control.
                let w = rect.width();
                let h = rect.height();
                for point in [
                    Pos2::new(5.0, h / 2.0),
                    Pos2::new(w - 5.0, h / 2.0),
                    Pos2::new(w / 2.0, 5.0),
                    Pos2::new(w / 2.0, h - 5.0),
                    Pos2::new(12.0, 12.0),
                    Pos2::new(w - 12.0, 12.0),
                    Pos2::new(12.0, h - 12.0),
                    Pos2::new(w - 12.0, h - 12.0),
                    Pos2::new(w / 2.0, h / 2.0),
                ] {
                    let center = point == Pos2::new(w / 2.0, h / 2.0);
                    if center && self.phase >= 3 {
                        continue; // Real clipboard content has its own theme and list colors.
                    }
                    let want = if center {
                        vec![255; 3]
                    } else {
                        expected.clone()
                    };
                    let actual = pixel(point);
                    if actual != want {
                        self.failures.lock().unwrap().push(format!(
                            "phase {} transparent={transparent}, {point:?}: expected {want:?}, got {actual:?}", self.phase,
                        ));
                    }
                }
                std::fs::create_dir_all("preview/transparency").unwrap();
                image::save_buffer(
                    format!("preview/transparency/phase-{}.png", self.phase),
                    &capture.rgba,
                    capture.width,
                    capture.height,
                    image::ColorType::Rgba8,
                )
                .unwrap();
                println!("phase {}: transparent={transparent}, ppp={ppp}", self.phase);
                self.phase += 1;
                self.started = Instant::now();
                if (3..5).contains(&self.phase) {
                    self.clipboard.open_state(true, false, None);
                }
                if self.phase == 5 {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
            ctx.request_repaint_after(Duration::from_millis(50));
        }
    }

    let failures = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let result = failures.clone();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Daozai synthetic transparency backdrop")
            .with_position([80.0, 80.0])
            .with_inner_size([680.0, 640.0])
            .with_decorations(false)
            .with_transparent(true)
            .with_always_on_top(),
        ..Default::default()
    };
    eframe::run_native(
        "Transparency regression",
        options,
        Box::new(move |cc| {
            daozai::theme::load_fonts(&cc.egui_ctx);
            let mut clipboard = daozai::clipboard_ui::Popup::default();
            clipboard.set_target_label(Some("透明窗口回归检查".into()));
            clipboard.set_items(vec![daozai::clipboard::Item {
                id: "synthetic-transparency-check".into(),
                text: "透明边缘测试 · 示例内容".into(),
                copied_at: 0,
                pinned: false,
                image: None,
            }]);
            Ok(Box::new(Probe {
                started: Instant::now(),
                phase: 0,
                clipboard,
                failures,
            }))
        }),
    )?;
    let failures = result.lock().unwrap();
    assert!(
        failures.is_empty(),
        "native composition failed:\n{}",
        failures.join("\n")
    );
    println!("PASS: transparent edges/corners, opaque control, recreated viewport, and light/dark clipboard");
    Ok(())
}

#[cfg(not(windows))]
fn main() {
    eprintln!("This native composition check requires Windows.");
}
