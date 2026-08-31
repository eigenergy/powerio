//! Peak allocation of the stored target checks: validating RFC 6901 targets
//! creates at most one generic representation of the value. One test per
//! binary so no parallel test inflates the measured peak.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

struct PeakTracking;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for PeakTracking {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            let live = LIVE.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
            PEAK.fetch_max(live, Ordering::Relaxed);
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) };
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
    }
}

#[global_allocator]
static ALLOCATOR: PeakTracking = PeakTracking;

fn measure_peak(text: &str) -> usize {
    PEAK.store(LIVE.load(Ordering::Relaxed), Ordering::Relaxed);
    let module = powerio::stored::read_module(text).unwrap();
    let peak = PEAK.load(Ordering::Relaxed);
    drop(module);
    peak
}

/// A document that forces the target existence check (one `source_map`
/// entry) retains at most one generic representation of the value. Measured
/// on this 200,000 bus value: the single borrowed representation peaks at
/// 2.5x the no-target baseline (the `serde_json::Value` tree alone is 1.5x
/// the baseline peak), while the pre-fix second cloned copy peaked at 4.1x.
/// The 3.2x bound separates the two implementations with margin on both
/// sides.
#[test]
fn target_checks_reinflate_the_value_once() {
    use powerio::{BalancedNetwork, PioValue};
    use powerio_core::{PioModule, SourceDescriptor, SourceId, SourceMapEntry, SourceRelation};
    use powerio_tx::{Bus, BusId, BusType};

    const BUSES: usize = 200_000;
    let buses: Vec<Bus> = (0..BUSES)
        .map(|index| {
            Bus::new(
                BusId(index + 1),
                if index == 0 {
                    BusType::Ref
                } else {
                    BusType::Pq
                },
                115.0,
            )
        })
        .collect();
    let network = BalancedNetwork::in_memory("alloc-peak", 100.0, buses, Vec::new());

    let plain = PioModule::new(PioValue::BalancedNetwork(network.clone()));
    let plain_text = powerio::stored::emit_module(&plain).unwrap();

    let mut mapped = PioModule::new(PioValue::BalancedNetwork(network));
    mapped
        .add_source_descriptor(
            SourceDescriptor::new(SourceId::new("s1").unwrap(), "case.m", 64).unwrap(),
        )
        .unwrap();
    mapped
        .add_source_map_entry(
            SourceMapEntry::new("/buses/0/vm", SourceRelation::Defaulted, Vec::new()).unwrap(),
        )
        .unwrap();
    let mapped_text = powerio::stored::emit_module(&mapped).unwrap();

    let plain_peak = measure_peak(&plain_text);
    let mapped_peak = measure_peak(&mapped_text);
    let ratio = mapped_peak as f64 / plain_peak as f64;
    assert!(
        ratio <= 3.2,
        "peak with a source map entry is {mapped_peak} bytes, {ratio:.2}x the {plain_peak} \
         byte baseline: the target check is holding more than one generic representation"
    );
}
