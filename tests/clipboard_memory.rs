//! Allocation regression: a tiny thumbnail must not copy its full-size source.
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

struct ObservedAllocator;
thread_local! {
    static OBSERVE: Cell<bool> = const { Cell::new(false) };
    static LARGEST: Cell<usize> = const { Cell::new(0) };
    static FULL_IMAGES: Cell<usize> = const { Cell::new(0) };
}
fn observe(size: usize) {
    let _ = OBSERVE.try_with(|enabled| {
        if enabled.get() {
            let _ = LARGEST.try_with(|max| max.set(max.get().max(size)));
            if size >= 3840 * 2160 * 4 {
                let _ = FULL_IMAGES.try_with(|count| count.set(count.get() + 1));
            }
        }
    });
}
unsafe impl GlobalAlloc for ObservedAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        observe(layout.size());
        System.alloc(layout)
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        observe(layout.size());
        System.alloc_zeroed(layout)
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        observe(size);
        System.realloc(ptr, layout, size)
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout)
    }
}
#[global_allocator]
static ALLOCATOR: ObservedAllocator = ObservedAllocator;

#[test]
fn small_thumbnail_does_not_allocate_another_full_resolution_image() {
    let source = daozai::ocr::Image {
        width: 3840,
        height: 2160,
        rgba: vec![180; 3840 * 2160 * 4],
    };
    LARGEST.set(0);
    OBSERVE.set(true);
    let thumbnail = daozai::clipboard::thumbnail(&source, 72);
    OBSERVE.set(false);
    assert_eq!((thumbnail.width, thumbnail.height), (72, 41));
    assert_eq!(thumbnail.rgba.len(), 72 * 41 * 4);
    assert!(
        LARGEST.get() < 1024 * 1024,
        "a 72px thumbnail allocated {} bytes",
        LARGEST.get()
    );
    assert_eq!(
        source.rgba.len(),
        3840 * 2160 * 4,
        "original image must remain available for paste/OCR"
    );
}

#[test]
fn rgba_png_decode_reuses_the_decoded_pixel_buffer() {
    let source = daozai::ocr::Image {
        width: 3840,
        height: 2160,
        rgba: vec![180; 3840 * 2160 * 4],
    };
    let png = daozai::screenshot::encode_png(&source).unwrap();
    FULL_IMAGES.set(0);
    OBSERVE.set(true);
    let decoded = daozai::ocr::decode_png(&png).unwrap();
    OBSERVE.set(false);
    assert_eq!(
        decoded, source,
        "memory optimization must preserve every original pixel"
    );
    assert_eq!(
        FULL_IMAGES.get(),
        1,
        "PNG decoding should allocate one RGBA image without a full-size conversion copy"
    );
}
