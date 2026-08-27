//! The gridfm Parquet read side against the matrix crate's write side. The
//! facade owns the reader, so the write to read round trips live here; the
//! writer's own structural tests stay in `powerio-matrix`.

use powerio::gridfm::{
    gridfm_base_case, read_gridfm_dataset, read_gridfm_network, read_gridfm_scenario_set,
    read_gridfm_scenarios,
};
use powerio::{BalancedNetwork, Branch, Bus, BusId, BusType, GenCost, Generator, SourceFormat};
use powerio_matrix::{
    GridfmOptions, GridfmSnapshot, gridfm_record_batches_single, write_gridfm_batch,
    write_gridfm_dataset,
};

fn case14() -> BalancedNetwork {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/data/case14.m");
    powerio::format::parse(powerio_core::Source::open(path).unwrap())
        .unwrap()
        .into_value()
}

fn scaled(net: &BalancedNetwork, factor: f64) -> BalancedNetwork {
    let mut s = net.clone();
    for l in s.loads_mut() {
        l.p *= factor;
        l.q *= factor;
    }
    for g in s.generators_mut() {
        g.pg *= factor;
        g.qg *= factor;
    }
    s
}

fn bus(id: usize, kind: BusType) -> Bus {
    Bus::new(BusId(id), kind, 1.0)
}

fn branch(from: usize, to: usize, r: f64, x: f64) -> Branch {
    Branch::new(BusId(from), BusId(to), r, x)
}

fn gencost(model: u8, ncost: usize, coeffs: Vec<f64>) -> GenCost {
    GenCost::with_ncost(model, 0.0, 0.0, ncost, coeffs)
}

fn gen_at(bus: usize, cost: GenCost) -> Generator {
    let mut generator = Generator::new(BusId(bus));
    generator.pmax = 100.0;
    generator.qmax = 50.0;
    generator.qmin = -50.0;
    generator.mbase = 100.0;
    generator.cost = Some(cost);
    generator
}

fn assert_fingerprint_close(got: &BalancedNetwork, want: &BalancedNetwork) {
    let (g, w) = (fingerprint(got), fingerprint(want));
    assert_eq!(
        (g.0, g.1, g.2, g.3),
        (w.0, w.1, w.2, w.3),
        "bus/branch/gen/ref counts differ"
    );
    for (a, b, label) in [
        (g.4, w.4, "load P"),
        (g.5, w.5, "load Q"),
        (g.6, w.6, "gen P"),
        (g.7, w.7, "sum r"),
        (g.8, w.8, "sum x"),
        (g.9, w.9, "sum b"),
        (g.10, w.10, "base_mva"),
    ] {
        assert!((a - b).abs() < 1e-9, "{label} differs: {a} vs {b}");
    }
}

#[allow(clippy::type_complexity)]
fn fingerprint(
    net: &BalancedNetwork,
) -> (
    usize,
    usize,
    usize,
    usize,
    f64,
    f64,
    f64,
    f64,
    f64,
    f64,
    f64,
) {
    (
        net.buses().len(),
        net.branches().len(),
        net.generators().len(),
        net.buses()
            .iter()
            .filter(|b| b.kind == BusType::Ref)
            .count(),
        net.loads().iter().map(|l| l.p).sum(),
        net.loads().iter().map(|l| l.q).sum(),
        net.generators().iter().map(|g| g.pg).sum(),
        net.branches().iter().map(|b| b.r).sum(),
        net.branches().iter().map(|b| b.x).sum(),
        net.branches().iter().map(|b| b.b).sum(),
        net.base_mva(),
    )
}

#[test]
fn read_round_trips_power_flow_fingerprint() {
    let net = case14();
    let dir = tempfile::tempdir().unwrap();
    write_gridfm_dataset(&net, 0, dir.path(), &GridfmOptions::default()).unwrap();

    let read = read_gridfm_dataset(dir.path().join("case14").join("raw"), 0).unwrap();
    assert_eq!(read.scenario, 0);
    assert_eq!(read.network.source_format(), SourceFormat::Gridfm);
    assert_eq!(read.network.name(), "case14");
    assert_fingerprint_close(&read.network, &net);
    // The reconstruction is structurally valid (validate() already ran inside).
    read.network.validate().unwrap();
}

#[test]
fn read_gridfm_network_pure_path_matches_disk() {
    // The in-memory inverse of gridfm_record_batches_single reproduces the same
    // fingerprint with no disk I/O.
    let net = case14();
    let tables = gridfm_record_batches_single(&net, 0, &GridfmOptions::default()).unwrap();
    let read = read_gridfm_network(
        &tables.bus,
        &tables.generator,
        &tables.branch,
        0,
        net.base_mva(),
        net.name(),
    )
    .unwrap();
    assert_fingerprint_close(&read.network, &net);
}

#[test]
fn read_recovers_shunt_at_base_mva() {
    // case14 has a single bus shunt (Bs = 19 at bus 9). The writer divides by
    // base_mva; the reader must multiply it back.
    let net = case14();
    let want_b: f64 = net.shunts().iter().map(|s| s.b).sum();
    assert!(want_b.abs() > 1.0, "fixture should have a real shunt");

    let tables = gridfm_record_batches_single(&net, 0, &GridfmOptions::default()).unwrap();
    let read = read_gridfm_network(
        &tables.bus,
        &tables.generator,
        &tables.branch,
        0,
        net.base_mva(),
        net.name(),
    )
    .unwrap();
    let got_b: f64 = read.network.shunts().iter().map(|s| s.b).sum();
    assert!(
        (got_b - want_b).abs() < 1e-9,
        "shunt b not recovered at base_mva: {got_b} vs {want_b}"
    );
}

#[test]
fn read_scenarios_yields_distinct_networks() {
    // A 2-scenario batch (base + load×1.1): each scenario reads back to its own
    // BalancedNetwork, and gridfm_base_case picks scenario 0.
    let base = case14();
    let up = scaled(&base, 1.1);
    let snaps = [GridfmSnapshot::new(&base, 0), GridfmSnapshot::new(&up, 1)];
    let dir = tempfile::tempdir().unwrap();
    let out = write_gridfm_batch(&snaps, dir.path(), &GridfmOptions::default()).unwrap();

    let reads = read_gridfm_scenarios(&out.dir).unwrap();
    assert_eq!(reads.len(), 2);
    assert_eq!((reads[0].scenario, reads[1].scenario), (0, 1));

    let load0: f64 = reads[0].network.loads().iter().map(|l| l.p).sum();
    let load1: f64 = reads[1].network.loads().iter().map(|l| l.p).sum();
    assert!(load0 > 0.0);
    assert!(
        (load1 - 1.1 * load0).abs() < 1e-6,
        "scenario 1 load should be 1.1× scenario 0: {load1} vs {load0}"
    );

    let base_case = gridfm_base_case(&out.dir).unwrap();
    assert_fingerprint_close(&base_case.network, &reads[0].network);
}

#[test]
fn read_resolves_lenient_directory_layouts() {
    // resolve_raw_dir accepts the leaf raw/ dir, the <case>/ dir, and the
    // parent out/ dir.
    let net = case14();
    let dir = tempfile::tempdir().unwrap();
    write_gridfm_dataset(&net, 0, dir.path(), &GridfmOptions::default()).unwrap();
    let out = dir.path(); // parent
    let case_dir = out.join("case14");
    let raw_dir = case_dir.join("raw");
    for d in [raw_dir.clone(), case_dir, out.to_path_buf()] {
        let read = read_gridfm_dataset(&d, 0)
            .unwrap_or_else(|e| panic!("failed to resolve {}: {e}", d.display()));
        assert_eq!(read.network.buses().len(), net.buses().len());
    }
}

#[test]
fn read_missing_scenario_errors() {
    let net = case14();
    let tables = gridfm_record_batches_single(&net, 0, &GridfmOptions::default()).unwrap();
    let err = read_gridfm_network(
        &tables.bus,
        &tables.generator,
        &tables.branch,
        99,
        net.base_mva(),
        net.name(),
    )
    .unwrap_err();
    assert!(
        matches!(
            err,
            powerio_tx::Error::FormatRead {
                format: "gridfm",
                ..
            }
        ),
        "got {err:?}"
    );
}

#[test]
fn read_no_dataset_errors() {
    // An empty (but existing) directory: read_dir succeeds, finds nothing.
    let dir = tempfile::tempdir().unwrap();
    let err = read_gridfm_dataset(dir.path(), 0).unwrap_err();
    assert!(
        matches!(
            err,
            powerio_tx::Error::FormatRead {
                format: "gridfm",
                ..
            }
        ),
        "got {err:?}"
    );
    // A non-existent directory: read_dir's IO error is surfaced, not masked.
    let missing = dir.path().join("does-not-exist");
    let err = read_gridfm_dataset(&missing, 0).unwrap_err();
    assert!(
        matches!(
            err,
            powerio_tx::Error::FormatRead {
                format: "gridfm",
                ..
            }
        ),
        "got {err:?}"
    );
}

#[test]
fn read_defaults_unusable_base_mva_to_100() {
    // A manifest base_mva of 0 (or negative/NaN) is unusable — shunt recovery
    // scales by it — so the reader defaults to 100 and warns instead of
    // silently producing a network with zeroed shunts.
    let net = case14();
    let dir = tempfile::tempdir().unwrap();
    let out = write_gridfm_dataset(&net, 0, dir.path(), &GridfmOptions::default()).unwrap();
    let meta_path = out.dir.join("gridfm_meta.json");
    let mut meta: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&meta_path).unwrap()).unwrap();
    meta["base_mva"] = serde_json::json!(0.0);
    std::fs::write(&meta_path, serde_json::to_string(&meta).unwrap()).unwrap();

    let read = read_gridfm_dataset(&out.dir, 0).unwrap();
    assert!(
        (read.network.base_mva() - 100.0).abs() < 1e-9,
        "base_mva should default to 100, got {}",
        read.network.base_mva()
    );
    assert!(
        read.warnings.iter().any(|w| w.contains("base_mva")),
        "expected a base_mva warning, got {:?}",
        read.warnings
    );
}

#[test]
fn read_surfaces_fidelity_warnings() {
    let net = case14();
    let tables = gridfm_record_batches_single(&net, 0, &GridfmOptions::default()).unwrap();
    let read = read_gridfm_network(
        &tables.bus,
        &tables.generator,
        &tables.branch,
        0,
        net.base_mva(),
        net.name(),
    )
    .unwrap();
    assert!(!read.warnings.is_empty());
    assert!(
        read.warnings
            .iter()
            .any(|w| w.contains("synthesized bus ids")),
        "expected the bus-id synthesis warning, got {:?}",
        read.warnings
    );
    // case14 has loads and a shunt, so those folding warnings appear too.
    assert!(read.warnings.iter().any(|w| w.contains("nodal load")));
    assert!(read.warnings.iter().any(|w| w.contains("nodal shunts")));
}

#[test]
fn read_recovers_gen_vg_from_bus_vm() {
    // gridfm has no gen vg column; vg is recovered from the gen's bus Vm.
    // case14's slack bus 1 sits at Vm = 1.06, so its generator reads vg ≈ 1.06
    // (not the old hard-coded 1.0).
    let net = case14();
    let tables = gridfm_record_batches_single(&net, 0, &GridfmOptions::default()).unwrap();
    let read = read_gridfm_network(
        &tables.bus,
        &tables.generator,
        &tables.branch,
        0,
        net.base_mva(),
        net.name(),
    )
    .unwrap();
    for g in read.network.generators() {
        let bus = read
            .network
            .buses()
            .iter()
            .find(|b| b.id == g.bus)
            .expect("gen references a known bus");
        assert!(
            (g.vg - bus.vm).abs() < 1e-12,
            "vg should equal the bus Vm: {} vs {}",
            g.vg,
            bus.vm
        );
    }
    assert!(
        read.network
            .generators()
            .iter()
            .any(|g| (g.vg - 1.0).abs() > 1e-3),
        "expected a generator with vg != 1.0 (case14's slack is at 1.06)"
    );
}

#[test]
fn read_maps_unit_tap_lines_back_to_zero() {
    // The writer stores effective tap (a line's raw 0 becomes 1.0). The reader
    // must map unit tap + no shift back to raw tap 0 so lines stay lines;
    // otherwise every line reads as a transformer and a read→write to PSS/E /
    // PowerWorld misclassifies them. case14 has both lines and off-nominal
    // transformers, so the line/transformer split must survive the round trip.
    let net = case14();
    let n_lines = net
        .branches()
        .iter()
        .filter(|b| !b.is_transformer())
        .count();
    let n_xfmr = net.branches().iter().filter(|b| b.is_transformer()).count();
    assert!(
        n_lines > 0 && n_xfmr > 0,
        "fixture needs both lines and transformers"
    );

    let tables = gridfm_record_batches_single(&net, 0, &GridfmOptions::default()).unwrap();
    let read = read_gridfm_network(
        &tables.bus,
        &tables.generator,
        &tables.branch,
        0,
        net.base_mva(),
        net.name(),
    )
    .unwrap();
    let read_lines = read
        .network
        .branches()
        .iter()
        .filter(|b| !b.is_transformer())
        .count();
    let read_xfmr = read
        .network
        .branches()
        .iter()
        .filter(|b| b.is_transformer())
        .count();
    assert_eq!(
        read_lines, n_lines,
        "lines must read back as lines (raw tap 0)"
    );
    assert_eq!(
        read_xfmr, n_xfmr,
        "transformers must keep their off-nominal ratio"
    );
    assert!(
        read.warnings.iter().any(|w| w.contains("read as lines")),
        "expected the unit-tap warning, got {:?}",
        read.warnings
    );
}

#[test]
fn read_allows_a_case_with_no_generators() {
    // gen_data may be legitimately empty (a power flow case with no mpc.gen);
    // the scenario guard must not reject it — only a *partial* table (rows for
    // other scenarios but not this one) is an error.
    let net = BalancedNetwork::in_memory(
        "nogen",
        100.0,
        vec![bus(1, BusType::Ref), bus(2, BusType::Pq)],
        vec![branch(1, 2, 0.01, 0.1)],
    );
    let tables = gridfm_record_batches_single(&net, 0, &GridfmOptions::default()).unwrap();
    let read = read_gridfm_network(
        &tables.bus,
        &tables.generator,
        &tables.branch,
        0,
        net.base_mva(),
        net.name(),
    )
    .unwrap();
    assert!(read.network.generators().is_empty());
    assert_eq!(read.network.branches().len(), 1);
}

#[test]
fn read_all_zero_cost_reads_as_none_with_ambiguity_warning() {
    // A genuine zero polynomial cost writes (0,0,0), indistinguishable from a
    // no-cost generator or a zeroed unrepresentable cost; the reader reads None
    // and the warning describes the ambiguity (not a false "piecewise/cubic").
    let mut net = BalancedNetwork::in_memory(
        "zerocost",
        100.0,
        vec![bus(1, BusType::Ref), bus(2, BusType::Pq)],
        vec![branch(1, 2, 0.01, 0.1)],
    );
    net.generators_mut()
        .push(gen_at(1, gencost(2, 3, vec![0.0, 0.0, 0.0])));
    let tables = gridfm_record_batches_single(&net, 0, &GridfmOptions::default()).unwrap();
    let read = read_gridfm_network(
        &tables.bus,
        &tables.generator,
        &tables.branch,
        0,
        net.base_mva(),
        net.name(),
    )
    .unwrap();
    assert!(
        read.network.generators()[0].cost.is_none(),
        "all-zero cost should read back as None"
    );
    assert!(
        read.warnings
            .iter()
            .any(|w| w.contains("read with no cost")),
        "expected the no-cost ambiguity warning, got {:?}",
        read.warnings
    );
}

#[test]
fn scenario_set_shares_unchanged_tables() {
    let base = BalancedNetwork::in_memory(
        "shared-set",
        100.0,
        vec![bus(1, BusType::Ref), bus(2, BusType::Pq)],
        vec![branch(1, 2, 0.01, 0.1)],
    );
    let mut base = base;
    base.generators_mut()
        .push(gen_at(1, gencost(2, 3, vec![0.0, 5.0, 0.0])));
    let mut varied = base.clone();
    varied.generators_mut()[0].pg = 42.0;

    let dir = tempfile::tempdir().unwrap();
    let snapshots = [
        GridfmSnapshot::new(&base, 0),
        GridfmSnapshot::new(&varied, 1),
    ];
    write_gridfm_batch(&snapshots, dir.path(), &GridfmOptions::default()).unwrap();

    let (set, _diagnostics) = read_gridfm_scenario_set(dir.path()).unwrap();
    assert_eq!(set.len(), 2);
    let first = set.get("0").unwrap().value();
    let second = set.get("1").unwrap().value();
    // The unchanged topology is one allocation across the set; only the
    // varied generator table is held per scenario.
    assert!(std::ptr::eq(
        first.branches().as_ptr(),
        second.branches().as_ptr()
    ));
    assert!(!std::ptr::eq(
        first.generators().as_ptr(),
        second.generators().as_ptr()
    ));
    assert!((second.generators()[0].pg - 42.0).abs() < 1e-9);
    assert!((first.generators()[0].pg - second.generators()[0].pg).abs() > 1.0);
}
