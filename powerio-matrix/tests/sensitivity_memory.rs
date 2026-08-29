//! #291: the sparse sensitivity path holds bounded working memory — the
//! factorization plus one block of right hand sides — never the dense reduced
//! inverse or the dense PTDF the dense path forms. The counting allocator
//! measures peak live bytes on the measuring thread; the dense run on the same
//! case proves the measurement bites.

use powerio_matrix::{
    BalancedNetwork, Branch, Bus, BusId, BusType, IndexedNetwork, SensitivityOptions,
    SensitivitySolver, build_ptdf_lodf_with_options,
};

/// Peak live byte tracking global allocator, scoped to the one measuring
/// thread so the harness's other test threads never pollute a measurement.
struct PeakAllocator;

static LIVE_BYTES: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);
static PEAK_BYTES: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

thread_local! {
    static MEASURING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// `Layout` caps allocation sizes at `isize::MAX`, so the fallback never runs.
fn to_isize(size: usize) -> isize {
    isize::try_from(size).unwrap_or(isize::MAX)
}

fn record(delta: isize) {
    let live = LIVE_BYTES.fetch_add(delta, std::sync::atomic::Ordering::Relaxed) + delta;
    PEAK_BYTES.fetch_max(live, std::sync::atomic::Ordering::Relaxed);
}

/// Run `work` and return its value plus the peak live bytes it held.
fn measured_peak<T>(work: impl FnOnce() -> T) -> (T, usize) {
    let _ = MEASURING.try_with(|flag| flag.set(true));
    LIVE_BYTES.store(0, std::sync::atomic::Ordering::Relaxed);
    PEAK_BYTES.store(0, std::sync::atomic::Ordering::Relaxed);
    let value = work();
    let peak = PEAK_BYTES.load(std::sync::atomic::Ordering::Relaxed);
    let _ = MEASURING.try_with(|flag| flag.set(false));
    (value, usize::try_from(peak).unwrap_or(0))
}

// SAFETY: delegates every operation to the system allocator; the counters are
// a side effect on the measuring thread only.
unsafe impl std::alloc::GlobalAlloc for PeakAllocator {
    unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
        if MEASURING.try_with(std::cell::Cell::get).unwrap_or(false) {
            record(to_isize(layout.size()));
        }
        unsafe { std::alloc::System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: std::alloc::Layout) {
        if MEASURING.try_with(std::cell::Cell::get).unwrap_or(false) {
            record(-to_isize(layout.size()));
        }
        unsafe { std::alloc::System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: std::alloc::Layout, new_size: usize) -> *mut u8 {
        if MEASURING.try_with(std::cell::Cell::get).unwrap_or(false) {
            record(to_isize(new_size) - to_isize(layout.size()));
        }
        unsafe { std::alloc::System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static COUNTING: PeakAllocator = PeakAllocator;

/// Ring of `n` buses with a skip chord at every bus: 2n branches, none of
/// them bridges, positive susceptances, one reference.
fn ring_with_chords(n: usize) -> BalancedNetwork {
    let mut buses = Vec::with_capacity(n);
    buses.push(Bus::new(BusId(1), BusType::Ref, 345.0));
    for id in 2..=n {
        buses.push(Bus::new(BusId(id), BusType::Pq, 345.0));
    }
    let mut branches = Vec::with_capacity(2 * n);
    for i in 1..=n {
        let next = i % n + 1;
        let skip = (i + 1) % n + 1;
        branches.push(Branch::new(BusId(i), BusId(next), 0.0, 0.1));
        branches.push(Branch::new(BusId(i), BusId(skip), 0.0, 0.2));
    }
    BalancedNetwork::in_memory("ring_with_chords", 100.0, buses, branches)
}

#[test]
fn the_sparse_path_never_holds_a_dense_buffer() {
    let n = 700usize;
    let case = ring_with_chords(n);
    let view = IndexedNetwork::new(&case);
    let nr = n - 1;
    let dense_buffer = nr * nr * size_of::<f64>();

    // The drop tolerance keeps the retained output small on both paths, so
    // the measurement sees the solver's working memory rather than the CSR
    // matrices being returned.
    let sparse_options = SensitivityOptions {
        solver: SensitivitySolver::Sparse,
        drop_tolerance: 0.9,
        ..SensitivityOptions::default()
    };
    let (sparse, sparse_peak) =
        measured_peak(|| build_ptdf_lodf_with_options(&view, &sparse_options).unwrap());
    assert_eq!(sparse.lodf.rows(), 2 * n);
    // Every ring flow lands under the drop tolerance; the dropped count is
    // what proves the full PTDF sweep ran.
    assert!(sparse.metadata.ptdf.dropped_entries > 0);

    // The factorization of a near-banded network graph plus one 32-column
    // block of right hand sides is far smaller than even one dense reduced
    // buffer (about 3.9 MB here), let alone the dense factor plus dense
    // PTDF the dense path forms.
    assert!(
        sparse_peak < dense_buffer / 2,
        "sparse peak {sparse_peak} bytes, dense reduced buffer {dense_buffer}"
    );

    // Same case down the dense path: the peak must cross the dense reduced
    // buffer, which is what proves this measurement would catch the sparse
    // path regressing into a dense materialization.
    let dense_options = SensitivityOptions {
        solver: SensitivitySolver::Dense,
        drop_tolerance: 0.9,
        ..SensitivityOptions::default()
    };
    let (dense, dense_peak) =
        measured_peak(|| build_ptdf_lodf_with_options(&view, &dense_options).unwrap());
    assert_eq!(dense.lodf.rows(), 2 * n);
    assert!(
        dense_peak > dense_buffer,
        "dense peak {dense_peak} bytes, dense reduced buffer {dense_buffer}"
    );
}
