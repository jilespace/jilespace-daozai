# Clipboard Redesign Implementation Plan

**Goal:** Implement the selected first clipboard visual with reliable search, preview and paste-back behavior.

**Architecture:** Preserve encrypted history and its background worker. Keep rendering and selection in `clipboard_ui.rs`, platform helpers in `native.rs`, and acknowledgment-based dispatch in `main.rs`. Add isolated synthetic rendering and behavior tests.

**Tech Stack:** Rust, egui 0.31, Windows APIs, existing software raster preview.

## Tasks

- [ ] Add failing behavior tests for stable selection across updates and editing-safe key handling in `src/clipboard_ui.rs`.
- [ ] Implement the graphite command palette, fixed footer, grouping, preview, secondary menus and undo in `src/clipboard_ui.rs`.
- [ ] Add bounded native target/position/paste helpers in `src/native.rs`, with focused tests and foreground checks.
- [ ] Integrate copy acknowledgments and visible errors in `src/main.rs`; defer close and paste until write success.
- [ ] Add synthetic dark/light/narrow render and interaction checks, run focused tests then `cargo test --locked --all-targets --features record -- --test-threads=1`, build `daozai`.
- [ ] Update usage and changelog, inspect source diffs and final repository state; report verified and blocked checks separately. No commits or branch changes.
