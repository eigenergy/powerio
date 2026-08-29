//! Operating point state: the #196 completion behaviors — identity
//! permutations, sparse and dense equivalence, retained point validity,
//! shared network identity, unknown and duplicate identities, and repeated
//! access without allocation.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use powerio_core::{Source, TimePoint};
use powerio_prob::{BalancedStateBuilder, MulticonductorStateBuilder};
use powerio_tx::BusId;

struct CountingAllocator;

static ALLOCS: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    /// Counting is scoped to the measuring thread, so the other tests
    /// running in parallel never pollute the assertion.
    static MEASURING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn count_this_thread() {
    let _ = MEASURING.try_with(|flag| flag.set(true));
}

fn stop_counting_this_thread() {
    let _ = MEASURING.try_with(|flag| flag.set(false));
}

fn measuring() -> bool {
    MEASURING.try_with(std::cell::Cell::get).unwrap_or(false)
}

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if measuring() {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if measuring() {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static COUNTING: CountingAllocator = CountingAllocator;

fn case9() -> powerio_tx::BalancedNetwork {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/data/case9.m");
    powerio_tx::parse(Source::open(path).unwrap())
        .expect("case9 parses")
        .into_value()
}

fn labels(count: usize) -> Vec<TimePoint> {
    (0..count)
        .map(|index| TimePoint::new(index.to_string(), None).unwrap())
        .collect()
}

#[test]
fn dense_columns_read_back_in_identity_order() {
    let net = case9();
    let n = net.buses().len();
    // Two points: point 0 holds 1.00..; point 1 holds 2.00.. per bus.
    let vm: Vec<f64> = (0..2 * n)
        .map(|i| 1.0 + (i / n) as f64 + (i % n) as f64 / 100.0)
        .collect();
    let series = BalancedStateBuilder::new(net.clone(), labels(2))
        .bus_voltage_magnitudes(vm.clone())
        .build()
        .expect("dense series builds");
    let (_, first) = series.get(0).unwrap();
    let (_, second) = series.get(1).unwrap();
    for (offset, bus) in net.buses().iter().enumerate() {
        assert_eq!(
            first.bus_voltage_magnitude(bus.id).map(f64::to_bits),
            Some(vm[offset].to_bits())
        );
        assert_eq!(
            second.bus_voltage_magnitude(bus.id).map(f64::to_bits),
            Some(vm[n + offset].to_bits())
        );
    }
    // Bulk read matches the identity order the builder consumed.
    let order: Vec<String> = first
        .identity_order("bus_voltage_magnitude")
        .unwrap()
        .map(str::to_owned)
        .collect();
    let expected_order: Vec<String> = net.buses().iter().map(|b| b.id.0.to_string()).collect();
    assert_eq!(order, expected_order);
    assert_eq!(
        first
            .quantity_values("bus_voltage_magnitude")
            .unwrap()
            .len(),
        n
    );
}

#[test]
fn sparse_and_dense_storage_read_identically() {
    let net = case9();
    let loads = net.loads().len();
    let base: Vec<f64> = (0..loads).map(|i| 10.0 + i as f64).collect();
    // Dense spelling of "base everywhere except one override at point 1".
    let mut dense = Vec::new();
    for point in 0..3 {
        for (i, value) in base.iter().enumerate() {
            if point == 1 && i == 0 {
                dense.push(99.0);
            } else {
                dense.push(*value);
            }
        }
    }
    let first_load = "loads:0".to_string();
    let dense_series = BalancedStateBuilder::new(net.clone(), labels(3))
        .load_active_powers(dense)
        .build()
        .unwrap();
    let sparse_series = BalancedStateBuilder::new(net.clone(), labels(3))
        .sparse_load_active_powers(
            base,
            vec![Vec::new(), vec![(first_load.clone(), 99.0)], Vec::new()],
        )
        .build()
        .unwrap();
    for point in 0..3 {
        let (_, d) = dense_series.get(point).unwrap();
        let (_, s) = sparse_series.get(point).unwrap();
        for row in 0..loads {
            let identity = format!("loads:{row}");
            assert_eq!(
                d.load_active_power(&identity),
                s.load_active_power(&identity),
                "point {point} load {identity}"
            );
        }
        assert_eq!(
            d.quantity_values("load_active_power"),
            s.quantity_values("load_active_power")
        );
    }
}

#[test]
fn a_retained_point_survives_its_dropped_series_without_copying_the_network() {
    let net = case9();
    let n = net.buses().len();
    let bus_ptr = net.buses().as_ptr();
    let series = BalancedStateBuilder::new(net, labels(2))
        .bus_voltage_angles(vec![0.25; 2 * n])
        .build()
        .unwrap();
    let (_, point) = series.get(1).unwrap();
    let retained = point.clone();
    drop(series);
    assert_eq!(retained.bus_voltage_angle(BusId(1)), Some(0.25));
    // The retained point shares the same network tables: no materialization.
    assert_eq!(retained.network().buses().as_ptr(), bus_ptr);
}

#[test]
fn every_point_shares_one_network_identity() {
    let net = case9();
    let n = net.buses().len();
    let bus_ptr = net.buses().as_ptr();
    let series = BalancedStateBuilder::new(net, labels(4))
        .bus_voltage_magnitudes(vec![1.0; 4 * n])
        .build()
        .unwrap();
    for (_, point) in series.iter() {
        assert_eq!(point.network().buses().as_ptr(), bus_ptr);
    }
}

#[test]
fn unknown_and_duplicate_identities_are_refused() {
    let net = case9();
    let loads = net.loads().len();
    let error = BalancedStateBuilder::new(net.clone(), labels(1))
        .sparse_load_active_powers(vec![0.0; loads], vec![vec![("nope".to_owned(), 1.0)]])
        .build()
        .expect_err("unknown identity refuses");
    assert!(error.to_string().contains("nope"), "{error}");

    // A network with a duplicated uid cannot resolve a layout.
    let mut duplicated = net;
    for load in duplicated.loads_mut() {
        load.uid = Some("same".to_owned());
    }
    let error = BalancedStateBuilder::new(duplicated, labels(1))
        .load_active_powers(vec![0.0; loads])
        .build()
        .expect_err("duplicate identity refuses");
    assert!(error.to_string().contains("duplicate"), "{error}");
}

#[test]
fn shape_mismatches_are_refused_loudly() {
    let net = case9();
    let n = net.buses().len();
    let error = BalancedStateBuilder::new(net.clone(), labels(2))
        .bus_voltage_magnitudes(vec![1.0; n]) // one point's worth for two points
        .build()
        .expect_err("shape mismatch refuses");
    assert!(error.to_string().contains("values supplied"), "{error}");

    let error = BalancedStateBuilder::new(net, Vec::new())
        .build()
        .expect_err("an empty time axis refuses");
    assert!(error.to_string().contains("time point"), "{error}");
}

#[test]
fn an_unstated_quantity_reads_as_none_not_zero() {
    let net = case9();
    let n = net.buses().len();
    let series = BalancedStateBuilder::new(net, labels(1))
        .bus_voltage_magnitudes(vec![1.0; n])
        .build()
        .unwrap();
    let (_, point) = series.get(0).unwrap();
    assert_eq!(point.bus_voltage_angle(BusId(1)), None);
    assert!(point.states("bus_voltage_magnitude"));
    assert!(!point.states("bus_voltage_angle"));
    assert_eq!(point.bus_voltage_magnitude(BusId(9999)), None);
}

#[test]
fn repeated_keyed_access_does_not_allocate() {
    let net = case9();
    let n = net.buses().len();
    let n_gen = net.generators().len();
    let series = BalancedStateBuilder::new(net, labels(2))
        .bus_voltage_magnitudes(vec![1.0; 2 * n])
        .generator_active_powers(vec![10.0; 2 * n_gen])
        .build()
        .unwrap();
    let (_, point) = series.get(0).unwrap();
    // Warm every code path once (the BusId spelling allocates its lookup
    // string; the layout hash itself allocates nothing).
    let _ = point.quantity_values("generator_active_power");
    let key = point
        .identity_order("generator_active_power")
        .unwrap()
        .next()
        .expect("case9 states generators")
        .to_owned();
    let key = key.as_str();
    // The hot accessor really reads a stored column.
    assert!(point.generator_active_power(key).is_some());
    let direct = |p: &powerio_prob::OperatingPoint<powerio_tx::BalancedNetwork>| {
        // Keyed access through a preformed key string: no per-call layout or
        // column allocation.
        p.generator_active_power(key)
    };
    count_this_thread();
    let before = ALLOCS.load(Ordering::Relaxed);
    for _ in 0..1000 {
        std::hint::black_box(direct(point));
    }
    let after = ALLOCS.load(Ordering::Relaxed);
    stop_counting_this_thread();
    assert_eq!(after - before, 0, "keyed reads allocate nothing");
}

#[test]
fn multiconductor_points_address_terminals_and_share_the_network() {
    let dss = "New Circuit.c basekv=12.47 pu=1 phases=3 bus1=a\n\
               New Line.l1 bus1=a.1.2.3 bus2=b.1.2.3 phases=3 r1=0.1 x1=0.2 length=1 units=km\n\
               New Load.ld bus1=b.1.2.3 phases=3 conn=wye kv=7.2 kw=30 kvar=9\n";
    let source = Source::from_bytes("<memory>", dss.as_bytes().to_vec())
        .unwrap()
        .with_format(powerio_core::FormatId::new("dss").unwrap());
    let net = powerio_dist::parse(source).unwrap().into_value();
    let terminals: usize = net.buses().iter().map(|b| b.terminals.len()).sum();
    let load_conductors: usize = net.loads().iter().map(|l| l.terminal_map.len()).sum();
    let bus_ptr = net.buses().as_ptr();

    // Distinct values per position: a wrong bus/terminal or load/conductor
    // index reads back a different number, so addressing errors surface.
    let vm: Vec<f64> = (0..2 * terminals).map(|k| 7200.0 + k as f64).collect();
    let lp: Vec<f64> = (0..2 * load_conductors)
        .map(|k| 10_000.0 + k as f64)
        .collect();
    let series = MulticonductorStateBuilder::new(net.clone(), labels(2))
        .terminal_voltage_magnitudes(vm.clone())
        .load_active_powers(lp.clone())
        .build()
        .expect("multiconductor series builds");
    let (_, point) = series.get(0).unwrap();
    let bus = &net.buses()[0];
    for (position, terminal) in bus.terminals.iter().enumerate() {
        assert_eq!(
            point.terminal_voltage_magnitude(&bus.id, terminal.as_str()),
            Some(vm[position]),
            "terminal {terminal}"
        );
    }
    let load = &net.loads()[0];
    for (position, conductor) in load.terminal_map.iter().enumerate() {
        assert_eq!(
            point.load_active_power(&load.name, conductor.as_str()),
            Some(lp[position]),
            "conductor {conductor}"
        );
    }
    let retained = point.clone();
    drop(series);
    assert_eq!(retained.network().buses().as_ptr(), bus_ptr);
}
