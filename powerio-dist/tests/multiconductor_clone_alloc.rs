//! #440: `MulticonductorNetwork` is a thin handle over one
//! `Arc<MulticonductorNetworkTables>`, so cloning it is a refcount bump,
//! never a deep copy. One test per binary so no parallel test inflates the
//! count.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use powerio_dist::MulticonductorNetwork;

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

#[test]
fn clone_is_zero_allocation() {
    let network = MulticonductorNetwork::default();

    let before = ALLOCATIONS.load(Ordering::Relaxed);
    let cloned = network.clone();
    let after = ALLOCATIONS.load(Ordering::Relaxed);

    assert_eq!(
        after - before,
        0,
        "MulticonductorNetwork::clone allocated; it should only bump the \
         shared table's reference count"
    );
    drop(cloned);
    drop(network);
}
