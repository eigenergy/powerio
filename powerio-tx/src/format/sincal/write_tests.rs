use super::{DatabaseSnapshot, read_balanced_snapshot, write_experimental_balanced};
use crate::network::*;

fn constructed() -> BalancedNetwork {
    // Original synthetic circuit, deliberately not on the reader's 100 MVA base.
    let mut net = BalancedNetwork::new("fresh synthetic", 25.0);
    *net.base_frequency_mut() = 60.0;
    for (id, kind, kv) in [
        (90, BusType::Ref, 20.0),
        (12, BusType::Pq, 0.4),
        (44, BusType::Pq, 0.4),
    ] {
        net.buses_mut().push(Bus::new(BusId(id), kind, kv));
    }
    net.buses_mut()[0].vm = 1.03;
    net.buses_mut()[0].va = -4.0;
    net.buses_mut()[1].name = Some("O'Brien Δ; DROP TABLE Node;".into());
    let mut source = Generator::new(BusId(90));
    source.vg = 1.03;
    net.generators_mut().push(source);
    let mut generator = Generator::new(BusId(44));
    generator.voltage_regulation_on = false;
    generator.pg = 0.002;
    generator.qg = -0.0004;
    net.generators_mut().push(generator);
    let mut transformer = Branch::new(BusId(90), BusId(12), 0.8, 2.5);
    transformer.rate_a = 0.4;
    transformer.tap = 1.025;
    transformer.shift = 30.0;
    transformer.charging = Some(BranchCharging::new(1e-5, -2e-5, 1e-5, -2e-5));
    net.branches_mut().push(transformer);
    let mut line = Branch::new(BusId(12), BusId(44), 1.0, 0.3);
    line.b = 1e-6;
    line.rate_a = 0.1;
    net.branches_mut().push(line);
    net.loads_mut().push(Load::new(BusId(44), 0.02, 0.004));
    net
}
fn reread(net: &BalancedNetwork) -> BalancedNetwork {
    let output = write_experimental_balanced(net).unwrap();
    let snapshot = DatabaseSnapshot::decode(&output.database, None).unwrap();
    read_balanced_snapshot(&snapshot, "fresh").unwrap()
}
fn close(a: f64, b: f64) {
    assert!(
        (a - b).abs() <= 1e-12 * a.abs().max(b.abs()).max(1e-9),
        "{a} != {b}"
    );
}

#[test]
fn fresh_physical_units_ratios_and_losses_survive_nonstandard_base() {
    let net = constructed();
    let result = write_experimental_balanced(&net).unwrap();
    let db = DatabaseSnapshot::decode(&result.database, None).unwrap();
    let values = db
        .connection
        .query_row(
            "SELECT Un1, Un2, Sn, ur, uk, Vfe, i0, AddRotate FROM TwoWindingTransformer",
            [],
            |r| {
                Ok((
                    r.get::<_, f64>(0)?,
                    r.get::<_, f64>(1)?,
                    r.get::<_, f64>(2)?,
                    r.get::<_, f64>(3)?,
                    r.get::<_, f64>(4)?,
                    r.get::<_, f64>(5)?,
                    r.get::<_, f64>(6)?,
                    r.get::<_, f64>(7)?,
                ))
            },
        )
        .unwrap();
    close(values.0, 20.5);
    close(values.1, 0.4);
    close(values.2, 0.4);
    close(values.3, 1.28);
    close(values.4, 4.199_809_519_490_14);
    close(values.5, 0.5);
    close(values.6, 0.279_508_497_187_473_7);
    close(values.7, 30.0);
    let (r, x, amps) = db
        .connection
        .query_row("SELECT r,x,Ith FROM Line", [], |r| {
            Ok((
                r.get::<_, f64>(0)?,
                r.get::<_, f64>(1)?,
                r.get::<_, f64>(2)?,
            ))
        })
        .unwrap();
    close(r, 0.0064);
    close(x, 0.00192);
    close(amps, 0.144_337_567_297_406_46);
    let out = reread(&net);
    close(out.base_mva(), 100.0);
    close(out.branches()[0].r, 3.2);
    close(out.branches()[0].x, 10.0);
    close(out.branches()[0].charging.unwrap().g_fr, 2.5e-6);
    close(out.branches()[1].b, 2.5e-7);
    assert_eq!(
        out.buses().iter().find(|b| b.id == BusId(12)).unwrap().name,
        net.buses()[1].name
    );
    assert!(
        result
            .diagnostics
            .iter()
            .any(|d| d.message().contains("native SINCAL"))
    );
}

#[test]
fn deterministic_archives_and_source_free_edits() {
    let mut net = constructed();
    let first = write_experimental_balanced(&net).unwrap().database;
    assert_eq!(first, write_experimental_balanced(&net).unwrap().database);
    let archive = powerio_sincal::authoring::candidate_archive(&first, "synthetic").unwrap();
    assert_eq!(powerio_sincal::database_bytes(&archive).unwrap(), first);
    // Serde here tests the backend with no retained module source; full public
    // IR integration will have separate facade coverage.
    net = serde_json::from_slice(&serde_json::to_vec(&net).unwrap()).unwrap();
    net.loads_mut()[0].p = 0.035;
    let out = reread(&net);
    close(out.loads()[0].p, 0.035);
    assert_ne!(write_experimental_balanced(&net).unwrap().database, first);
}

#[test]
fn constant_current_and_impedance_loads_and_service_states() {
    for exponent in [0.0, 1.0, 2.0] {
        let mut net = constructed();
        let load = &mut net.loads_mut()[0];
        load.voltage_model = Some(LoadVoltageModel::Exponential {
            p: load.p,
            q: load.q,
            v_nom: None,
            gamma_p: exponent,
            gamma_q: exponent,
        });
        load.in_service = false;
        net.branches_mut()[1].in_service = false;
        net.generators_mut()[1].in_service = false;
        let out = reread(&net);
        assert!(!out.loads()[0].in_service);
        assert!(!out.branches()[1].in_service);
        assert!(!out.generators()[1].in_service);
        if exponent != 0.0 {
            assert_eq!(out.loads()[0].voltage_model, net.loads()[0].voltage_model);
        }
    }
}

#[test]
fn rejects_unsupported_physics_and_numeric_loss_atomically() {
    type Edit = fn(&mut BalancedNetwork);
    let cases: &[(Edit, &str)] = &[
        (|n| n.buses_mut()[1].kind = BusType::Pv, "PV"),
        (|n| n.buses_mut()[1].vmin = 0.8, "heterogeneous"),
        (|n| n.buses_mut()[1].id = BusId(90), "duplicate"),
        (|n| n.loads_mut()[0].p = f64::NAN, "finite"),
        (|n| n.branches_mut()[0].rate_a = 0.0, "positive MVA"),
        (|n| n.branches_mut()[0].x = -1.0, "nonnegative"),
        (|n| n.branches_mut()[0].x = 1e-30, "loses electrical"),
        (
            |n| n.branches_mut()[0].charging.as_mut().unwrap().g_to = 0.0,
            "asymmetric",
        ),
        (|n| n.generators_mut()[0].vg = 0.99, "matching voltage"),
        (
            |n| n.generators_mut()[0].in_service = false,
            "active voltage source",
        ),
        (
            |n| *n.source_format_mut() = SourceFormat::Normalized,
            "normalized",
        ),
        (|n| n.loads_mut()[0].bus = BusId(123), "unknown bus"),
    ];
    for (edit, expected) in cases {
        let mut net = constructed();
        edit(&mut net);
        let message = match write_experimental_balanced(&net) {
            Ok(_) => panic!("accepted {expected}"),
            Err(e) => e.to_string(),
        };
        assert!(message.contains(expected), "{expected}: {message}");
    }
}

#[test]
fn fresh_authentic_simbench_mapping_needs_no_template() {
    let archive = include_bytes!("../../../../tests/data/sincal/1-LV-rural1--0-sw.sinx");
    let bytes = powerio_sincal::database_bytes(archive).unwrap();
    let db = DatabaseSnapshot::decode(&bytes, None).unwrap();
    let mut net = read_balanced_snapshot(&db, "SimBench").unwrap();
    drop(db);
    drop(bytes);
    net.loads_mut()[0].p *= 1.2;
    let out = reread(&net);
    assert_eq!(out.buses().len(), 15);
    assert_eq!(out.loads().len(), 13);
    assert_eq!(out.branches().len(), 14);
    assert_eq!(out.generators().len(), 5);
    for (a, b) in net.loads().iter().zip(out.loads()) {
        close(a.p, b.p);
        close(a.q, b.q);
    }
}

#[test]
fn bus_only_and_unrated_lines_do_not_invent_sources_or_limits() {
    let mut net = BalancedNetwork::new("empty equipment", 100.0);
    net.buses_mut().push(Bus::new(BusId(7), BusType::Pq, 10.0));
    let out = reread(&net);
    assert!(out.generators().is_empty());
    assert!(out.branches().is_empty());
    net.buses_mut().push(Bus::new(BusId(8), BusType::Pq, 10.0));
    net.branches_mut()
        .push(Branch::new(BusId(7), BusId(8), 0.01, -0.02));
    let out = reread(&net);
    close(out.branches()[0].rate_a, 0.0);
    close(out.branches()[0].x, -0.02);
    close(out.branches()[0].calc_effective_tap(), 1.0);
}
