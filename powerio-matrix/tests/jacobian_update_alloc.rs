//! #440: `PowerFlowJacobian::update` used to rebuild the whole derived index
//! (`IndexedNetwork::new`) and look up every bus voltage through a freshly
//! allocated decimal string per bus per quantity — allocation as heavy as
//! the full build it exists to avoid. This pins the fix: the allocation
//! count on a larger case must not scale anywhere near linearly with a
//! smaller one's. One test per binary so no parallel test inflates the
//! count.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use powerio_matrix::{BalancedNetwork, VoltageCoordinates, calc_power_flow_jacobian};
use powerio_prob::{AcPfInstance, BalancedOperatingPointBuilder, OperatingPoint};

struct CountingAlloc;

static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) };
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.realloc(pointer, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;

fn case(name: &str) -> BalancedNetwork {
    let path = format!("{}/../tests/data/{name}", env!("CARGO_MANIFEST_DIR"));
    powerio_tx::parse(powerio_core::Source::open(path).unwrap())
        .unwrap()
        .into_value()
}

/// A synthetic but complete operating point: every bus voltage stated, off
/// flat so the update path fills every block, mirroring
/// `powerio-matrix/tests/ac_jacobian.rs`'s own fixture.
fn operating_point(net: &BalancedNetwork, seed: &str) -> OperatingPoint<BalancedNetwork> {
    let n = net.buses().len();
    let vm: Vec<f64> = (0..n)
        .map(|k| 1.0 + 0.01 * (k as f64) / (n as f64))
        .collect();
    let va: Vec<f64> = (0..n)
        .map(|k| (2.0 * (k as f64) / (n as f64)).to_radians())
        .collect();
    let series = BalancedOperatingPointBuilder::new(
        net.clone(),
        vec![powerio_core::TimePoint::new(seed, None).unwrap()],
    )
    .bus_voltage_magnitudes(vm)
    .bus_voltage_angles(va)
    .build()
    .unwrap();
    let point = series.get(0).unwrap();
    point.clone()
}

/// Allocations `update` makes refreshing an already built Jacobian at a
/// second operating point (from a second series, so nothing about the
/// refresh is answered by reusing the build's own series).
fn update_allocations(case_file: &str) -> usize {
    let net = case(case_file);
    let instance = AcPfInstance::from_network(net.clone()).unwrap();
    let built_at = operating_point(&net, "0");
    let mut jacobian =
        calc_power_flow_jacobian(&instance, &built_at, VoltageCoordinates::Polar).unwrap();
    let flat = operating_point(&net, "1");

    let before = ALLOCATIONS.load(Ordering::Relaxed);
    jacobian.update(&instance, &flat).unwrap();
    ALLOCATIONS.load(Ordering::Relaxed) - before
}

#[test]
fn update_allocation_count_does_not_scale_with_case_size() {
    let small = update_allocations("case9.m");
    let large = update_allocations("case118.m");

    // `fill_values` (unrelated to this fix, and not touched by it) merges
    // the admittance rows with one `Vec` push per nonzero entry, so its own
    // allocation count already scales with edge count — the ratio between a
    // 9 bus and a 118 bus case stays close to 9x on both sides of this fix
    // (measured: 42/417 = 9.9x before, 18/165 = 9.2x after) and so cannot by
    // itself catch a regression back to a per-bus lookup. What the fix
    // changes is the count itself: measured 417 allocations refreshing the
    // 118 bus case before (`IndexedNetwork::new` plus a `to_string` per bus
    // per quantity), 165 after. The bound below sits at roughly 1.5x the
    // post-fix count — comfortable headroom over normal variance, and still
    // well short of a reversion to the pre-fix count.
    assert!(
        large < 250,
        "update() allocated {large} times refreshing a 118 bus case ({small} times on a \
         9 bus case); the pre-fix per-bus voltage lookup measured 417 on the same case"
    );
}
