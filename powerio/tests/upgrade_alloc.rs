//! SEC-6: the 0.9 upgrade used to size its allocations (upgraded time
//! points, per quantity sparse override columns) from the declared
//! `time_axis.periods` scalar, capped only by a flat MAX_UPGRADE_PERIODS —
//! a number that costs nothing to inflate in a tiny document, unlike the
//! labels/durations/points arrays that actually carry the data. A counting
//! allocator compares a document that inflates the scalar against one that
//! genuinely carries that many periods. One test per binary so no parallel
//! test inflates the count.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use powerio::stored::read_module;
use powerio::{BalancedNetwork, PioValue};
use powerio_tx::{Bus, BusId, BusType};

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

static MEASURE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn measure_allocated(work: impl FnOnce()) -> usize {
    let before = TOTAL_BYTES.load(Ordering::Relaxed);
    work();
    TOTAL_BYTES.load(Ordering::Relaxed) - before
}

fn small_network() -> BalancedNetwork {
    BalancedNetwork::in_memory(
        "upgrade-alloc",
        100.0,
        vec![
            Bus::new(BusId(1), BusType::Ref, 138.0),
            Bus::new(BusId(2), BusType::Pq, 138.0),
        ],
        Vec::new(),
    )
}

/// A legacy 0.9 package declaring `declared_periods` (the `time_axis.periods`
/// scalar) while its labels and durations stay empty and it carries exactly
/// one operating point: the shape of a document lying about its period
/// count, at negligible cost to the document's own size.
fn legacy_envelope(operating_points: serde_json::Value) -> String {
    serde_json::json!({
        "powerio_version": "0.9.0",
        "producer": {"tool": "powerio", "version": "0.9.0"},
        "model_kind": "balanced",
        "origin": {"kind": "in_memory"},
        "validation": {"status": "ok", "counts": {"fatal": 0, "error": 0, "warning": 0, "info": 0, "debug": 0}},
        "model": {
            "kind": "balanced",
            "balanced_network": serde_json::to_value(small_network()).unwrap(),
        },
        "operating_points": operating_points,
    })
    .to_string()
}

/// A legacy 0.9 package declaring `declared_periods` on its time axis while
/// carrying one real point: a ~2 KB document whatever it declares.
fn legacy_text_declaring(declared_periods: usize) -> String {
    legacy_envelope(serde_json::json!({
        "time_axis": {"periods": declared_periods, "duration_hours": [], "labels": []},
        "points": [{"index": 0}],
    }))
}

/// A legacy 0.9 package genuinely carrying `periods` periods: labels and
/// durations both sized to match, so the document's own byte size scales
/// with `periods` the way a real producer's output would.
fn legacy_text_carrying(periods: usize) -> String {
    let labels: Vec<String> = (0..periods).map(|i| format!("h{i}")).collect();
    let points: Vec<serde_json::Value> = (0..periods)
        .map(|i| serde_json::json!({"index": i}))
        .collect();
    legacy_envelope(serde_json::json!({
        "time_axis": {
            "periods": periods,
            "duration_hours": vec![1.0; periods],
            "labels": labels,
        },
        "points": points,
    }))
}

#[test]
fn an_inflated_declared_period_count_does_not_inflate_allocation() {
    let _serial = MEASURE_LOCK.lock().unwrap();
    let declared_text = legacy_text_declaring(131_072);
    assert!(
        declared_text.len() < 8192,
        "the declaring document should stay small: {} bytes",
        declared_text.len()
    );

    let declared_bytes = measure_allocated(|| {
        let module = read_module(&declared_text).unwrap();
        assert!(
            matches!(
                module.value(),
                PioValue::BalancedOperatingPointTimeSeries(_)
            ),
            "expected the series value, got {:?}",
            module.value().kind()
        );
        std::hint::black_box(&module);
    });

    // The honest comparison: a document that genuinely carries 500 periods
    // of real labels and durations.
    let carrying_text = legacy_text_carrying(500);
    let carrying_bytes = measure_allocated(|| {
        let module = read_module(&carrying_text).unwrap();
        std::hint::black_box(&module);
    });

    assert!(
        declared_bytes < carrying_bytes,
        "a document declaring 131,072 periods but carrying one point allocated \
         {declared_bytes} bytes, at least as much as a document genuinely carrying \
         500 real periods ({carrying_bytes} bytes): allocation is still driven by \
         the declared count rather than what the document actually carries"
    );
}

#[test]
fn a_document_genuinely_carrying_n_periods_still_decodes() {
    let _serial = MEASURE_LOCK.lock().unwrap();
    let text = legacy_text_carrying(500);
    let module = read_module(&text).unwrap();
    let PioValue::BalancedOperatingPointTimeSeries(series) = module.value() else {
        panic!("expected the series value, got {:?}", module.value().kind());
    };
    assert_eq!(series.len(), 500);
}
