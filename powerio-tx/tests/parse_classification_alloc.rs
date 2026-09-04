//! #440: an undeclared JSON source used to classify twice over — once
//! through a full `serde_json::Value` materialization of the whole document,
//! then again identically in the facade above this crate — before the typed
//! reader ever ran. This pins the local half of that fix: classifying an
//! undeclared `.json` source here costs close to nothing extra over already
//! knowing the format, because [`powerio_tx::format::routing::classify_json_text`]
//! now reads a typed header instead of a generic value tree. One test per
//! binary so no parallel test inflates the count.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use powerio_tx::TargetFormat;

mod helpers;
use helpers::emit_value;

struct CountingAlloc;

static TOTAL_BYTES: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        TOTAL_BYTES.fetch_add(layout.size(), Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) };
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if new_size > layout.size() {
            TOTAL_BYTES.fetch_add(new_size - layout.size(), Ordering::Relaxed);
        }
        unsafe { System.realloc(pointer, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;

/// Total bytes allocated while running `work`, cumulative (never decremented
/// on free), so a transient materialization that classification builds and
/// drops still counts toward the total.
fn measure_allocated(work: impl FnOnce()) -> usize {
    let before = TOTAL_BYTES.load(Ordering::Relaxed);
    work();
    TOTAL_BYTES.load(Ordering::Relaxed) - before
}

fn powermodels_json_text() -> String {
    let path = format!("{}/../tests/data/case118.m", env!("CARGO_MANIFEST_DIR"));
    let net = powerio_tx::parse(powerio_core::Source::open(path).unwrap())
        .unwrap()
        .into_value();
    emit_value(&net, TargetFormat::PowerModelsJson)
        .unwrap()
        .text
}

#[test]
fn undeclared_json_parsing_costs_close_to_a_declared_parse() {
    let text = powermodels_json_text();

    let declared_bytes = measure_allocated(|| {
        let source = powerio_core::Source::from_memory("case", text.as_bytes().to_vec())
            .unwrap()
            .with_format(powerio_core::FormatId::new("powermodels-json").unwrap());
        let parsed = powerio_tx::parse(source).unwrap();
        std::hint::black_box(&parsed);
    });

    let undeclared_bytes = measure_allocated(|| {
        let source =
            powerio_core::Source::from_memory("case.json", text.as_bytes().to_vec()).unwrap();
        let parsed = powerio_tx::parse(source).unwrap();
        std::hint::black_box(&parsed);
    });

    let ratio = undeclared_bytes as f64 / declared_bytes as f64;
    assert!(
        ratio <= 2.0,
        "undeclared parse allocated {undeclared_bytes} bytes, {ratio:.2}x the \
         {declared_bytes} byte declared-format baseline ({} byte source text); \
         classification is materializing more than a typed header",
        text.len()
    );
}
