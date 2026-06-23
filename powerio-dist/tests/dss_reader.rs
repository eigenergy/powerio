//! Typed model from the vendored fixtures, checked against the OpenDSS
//! engine's own bus and node sets (dumped with opendssdirect 0.9.4 via
//! `dss.Circuit.AllBusNames()` and `dss.Bus.Nodes()` per bus after a
//! Redirect; tools/solve_dss.py documents the staging to reuse when the
//! engine changes).

use std::collections::BTreeMap;
use std::path::PathBuf;

use powerio_dist::dss::parse_dss_file;
use powerio_dist::{Configuration, DistNetwork, WindingConn};

fn fixture(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/data/dist")
        .join(rel)
}

fn parse(rel: &str) -> DistNetwork {
    parse_dss_file(fixture(rel)).expect("fixture parses")
}

/// Bus id (lowercased) → phase terminal names, excluding the materialized
/// grounded neutral, matching what the engine reports as the bus's nodes.
fn phase_terminals(net: &DistNetwork) -> BTreeMap<String, Vec<String>> {
    net.buses
        .iter()
        .map(|b| {
            (
                b.id.to_ascii_lowercase(),
                b.terminals
                    .iter()
                    .filter(|t| !b.grounded.contains(t))
                    .cloned()
                    .collect(),
            )
        })
        .collect()
}

#[test]
fn ieee13_matches_the_engine_bus_map() {
    let net = parse("opendss/ieee13/IEEE13Nodeckt.dss");
    // dss.Circuit.AllBusNames() + dss.Bus.Nodes() on the same fixture.
    let expected: BTreeMap<String, Vec<String>> = [
        ("611", vec!["3"]),
        ("632", vec!["1", "2", "3"]),
        ("633", vec!["1", "2", "3"]),
        ("634", vec!["1", "2", "3"]),
        ("645", vec!["2", "3"]),
        ("646", vec!["2", "3"]),
        ("650", vec!["1", "2", "3"]),
        ("652", vec!["1"]),
        ("670", vec!["1", "2", "3"]),
        ("671", vec!["1", "2", "3"]),
        ("675", vec!["1", "2", "3"]),
        ("680", vec!["1", "2", "3"]),
        ("684", vec!["1", "3"]),
        ("692", vec!["1", "2", "3"]),
        ("rg60", vec!["1", "2", "3"]),
        ("sourcebus", vec!["1", "2", "3"]),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.into_iter().map(String::from).collect()))
    .collect();
    assert_eq!(phase_terminals(&net), expected);

    assert_eq!(net.name.as_deref(), Some("IEEE13Nodeckt"));
    assert_eq!(net.sources.len(), 1);
    assert_eq!(net.transformers.len(), 5);
    assert_eq!(net.loads.len(), 15);
    assert_eq!(net.switches.len(), 1);
    assert_eq!(net.shunts.len(), 2);
    assert_eq!(net.lines.len(), 11); // 12 line objects minus the switch

    // Source: 115 kV, pu=1.0001, 30 degrees.
    let vs = &net.sources[0];
    assert_eq!(vs.bus, "SourceBus");
    let vln = 115_000.0 / 3f64.sqrt() * 1.0001;
    assert!((vs.v_magnitude[0] - vln).abs() < 1e-6);
    assert!((vs.v_angle[0] - 30f64.to_radians()).abs() < 1e-12);
    assert!((vs.v_angle[1] - (-90f64).to_radians()).abs() < 1e-12);

    // Line 650632: mtx601 (ohm per mile), 2000 ft. r11 = 0.3465/1609.344
    // ohm/m; length = 2000*0.3048 m. Product must match the engine.
    let line = net.lines.iter().find(|l| l.name == "650632").unwrap();
    assert!((line.length - 2000.0 * 0.3048).abs() < 1e-9);
    let code = net.linecode(&line.linecode).unwrap();
    let r11_total = code.r_series[0][0] * line.length;
    assert!((r11_total - 0.3465 * 2000.0 / 5280.0).abs() < 1e-9);

    // The switch line 671692 carries its ampacity.
    let sw = &net.switches[0];
    assert_eq!(sw.name, "671692");
    assert!(!sw.open);

    // Bus coordinates landed as extras.
    let b = net.bus("611").unwrap();
    assert!(b.extras.contains_key("x"));

    // Load 671 is 3 phase delta: 1155 kW total, 660 kvar.
    let l671 = net.loads.iter().find(|l| l.name == "671").unwrap();
    assert_eq!(l671.configuration, Configuration::Delta);
    assert_eq!(l671.terminal_map, vec!["1", "2", "3"]);
    let p: f64 = l671.p_nom.iter().sum();
    assert!((p - 1_155_000.0).abs() < 1e-6);

    // Load 611 is single phase wye on node 3 with grounded return.
    let l611 = net.loads.iter().find(|l| l.name == "611").unwrap();
    assert_eq!(l611.configuration, Configuration::SinglePhase);
    assert_eq!(l611.terminal_map, vec!["3", "4"]);
    let b611 = net.bus("611").unwrap();
    assert_eq!(b611.grounded, vec!["4"]);

    // Substation transformer: delta primary, wye secondary.
    let sub = net
        .transformers
        .iter()
        .find(|t| t.name.eq_ignore_ascii_case("sub"))
        .unwrap();
    assert_eq!(sub.windings.len(), 2);
    assert_eq!(sub.windings[0].conn, WindingConn::Delta);
    assert_eq!(sub.windings[1].conn, WindingConn::Wye);
    assert!((sub.windings[0].v_ref - 115_000.0).abs() < 1e-9);
    assert!((sub.windings[1].v_ref - 4160.0).abs() < 1e-9);
}

#[test]
fn ieee34_and_ieee123_bus_counts_match_the_engine() {
    let net34 = parse("opendss/ieee34/ieee34Mod1.dss");
    assert_eq!(net34.buses.len(), 37);
    let t34 = phase_terminals(&net34);
    assert_eq!(t34["810"], vec!["2"]);
    assert_eq!(t34["864"], vec!["1"]);
    assert_eq!(t34["890"], vec!["1", "2", "3"]);

    let net123 = parse("opendss/ieee123/IEEE123Master.dss");
    assert_eq!(net123.buses.len(), 132);
    let t123 = phase_terminals(&net123);
    assert_eq!(t123["25r"], vec!["1", "3"]);
    assert_eq!(t123["36"], vec!["1", "2"]);
    assert_eq!(t123["94_open"], vec!["1"]);
    assert_eq!(net123.loads.len(), 91);
}

#[test]
fn defaults_materialize_with_provenance() {
    let net = parse("micro/defaults_degenerate.dss");

    // New Line.l_default bus1=sourcebus bus2=b2: every electrical value is
    // the constructor default, materialized and recorded.
    let line = net.lines.iter().find(|l| l.name == "l_default").unwrap();
    assert!((line.length - 1.0).abs() < 1e-12);
    let code = net.linecode(&line.linecode).unwrap();
    // Sequence defaults: diag (2*0.058 + 0.1784)/3, off diag (0.1784-0.058)/3.
    assert!((code.r_series[0][0] - 0.098_133_333_333_333_33).abs() < 1e-12);
    assert!((code.r_series[0][1] - 0.040_133_333_333_333_33).abs() < 1e-12);
    assert!((code.x_series[0][0] - 0.2153).abs() < 1e-12);
    let d = &net.defaulted["line.l_default"];
    assert!(d.contains(&"length") && d.contains(&"r1"));

    // New Load.ld_default bus1=b2: kv, kw, pf all defaulted.
    let load = net.loads.iter().find(|l| l.name == "ld_default").unwrap();
    let p: f64 = load.p_nom.iter().sum();
    let q: f64 = load.q_nom.iter().sum();
    assert!((p - 10_000.0).abs() < 1e-9);
    // q = kw * tan(acos(0.88))
    assert!((q - 10_000.0 * 0.88f64.acos().tan()).abs() < 1e-6);
    let d = &net.defaulted["load.ld_default"];
    assert!(d.contains(&"kv") && d.contains(&"kw") && d.contains(&"pf"));

    // New Transformer.t_default buses=(b2, b3): 12.47 kV / 1000 kVA wye-wye.
    let t = net
        .transformers
        .iter()
        .find(|t| t.name == "t_default")
        .unwrap();
    assert_eq!(t.windings.len(), 2);
    assert!((t.windings[0].v_ref - 12_470.0).abs() < 1e-9);
    assert!((t.windings[0].s_rating - 1_000_000.0).abs() < 1e-9);
    assert_eq!(t.windings[0].conn, WindingConn::Wye);
    assert!((t.xsc_pct[0] - 7.0).abs() < 1e-12);
    let d = &net.defaulted["transformer.t_default"];
    assert!(d.contains(&"kv") && d.contains(&"kva") && d.contains(&"xhl"));

    // The default circuit source.
    let vs = &net.sources[0];
    assert!((vs.v_magnitude[0] - 115_000.0 / 3f64.sqrt()).abs() < 1e-9);
    assert_eq!(vs.bus, "sourcebus");
}

#[test]
fn micro_transformers_type_correctly() {
    let net = parse("micro/xfmr_center_tap.dss");
    let t = net.transformers.iter().find(|t| t.name == "t1").unwrap();
    assert_eq!(t.windings.len(), 3);
    assert_eq!(t.phases, 1);
    assert!((t.windings[0].v_ref - 7200.0).abs() < 1e-9);
    assert!((t.windings[1].v_ref - 120.0).abs() < 1e-9);
    // Winding 2 is secondary.1.0, winding 3 is secondary.0.2 (reversed).
    assert_eq!(t.windings[1].terminal_map, vec!["1", "4"]);
    assert_eq!(t.windings[2].terminal_map, vec!["4", "2"]);
    assert_eq!(t.xsc_pct.len(), 3);

    let net = parse("micro/xfmr_wye_delta.dss");
    let t = net.transformers.iter().find(|t| t.name == "t1").unwrap();
    assert_eq!(t.windings[0].conn, WindingConn::Wye);
    assert_eq!(t.windings[1].conn, WindingConn::Delta);
    // Delta side lists only the phase conductors.
    assert_eq!(t.windings[1].terminal_map, vec!["1", "2", "3"]);
    // Wye side default neutral is grounded.
    assert_eq!(t.windings[0].terminal_map, vec!["1", "2", "3", "4"]);
}

#[test]
fn switch_states_follow_swtcontrol() {
    let net = parse("micro/switch.dss");
    let closed = net.switches.iter().find(|s| s.name == "sw_closed").unwrap();
    let open = net.switches.iter().find(|s| s.name == "sw_open").unwrap();
    assert!(!closed.open);
    assert!(open.open);
}

#[test]
fn swtcontrol_last_action_or_state_wins() {
    use powerio_dist::parse_dss_str;
    let base = "New Circuit.c basekv=12.47\nNew Line.sw bus1=sourcebus bus2=b2 switch=y\n";
    // The later `state` overrides the earlier `action`.
    let net = parse_dss_str(&format!(
        "{base}New SwtControl.s1 SwitchedObj=Line.sw action=close state=open"
    ));
    assert!(net.switches[0].open);
    // Source order reversed: `action` wins.
    let net = parse_dss_str(&format!(
        "{base}New SwtControl.s1 SwitchedObj=Line.sw state=open action=close"
    ));
    assert!(!net.switches[0].open);
    // `normal` applies only when neither action nor state is written.
    let net = parse_dss_str(&format!(
        "{base}New SwtControl.s1 SwitchedObj=Line.sw normal=open"
    ));
    assert!(net.switches[0].open);
    let net = parse_dss_str(&format!(
        "{base}New SwtControl.s1 SwitchedObj=Line.sw normal=open action=close"
    ));
    assert!(!net.switches[0].open);
}

#[test]
#[allow(clippy::float_cmp)]
fn four_wire_line_keeps_the_neutral() {
    let net = parse("micro/fourwire_linecode.dss");
    let line = net.lines.iter().find(|l| l.name == "l1").unwrap();
    assert_eq!(line.terminal_map_from, vec!["1", "2", "3", "4"]);
    assert_eq!(line.terminal_map_to, vec!["1", "2", "3", "4"]);
    let code = net.linecode("lc4").unwrap();
    assert_eq!(code.n_conductors, 4);
    // km units: 0.211 ohm/km = 2.11e-4 ohm/m on the diagonal.
    assert!((code.r_series[0][0] - 0.211e-3).abs() < 1e-12);
    assert_eq!(code.i_max.as_ref().unwrap()[0], 240.0);
    // The load on phase 1 returns through terminal 4, not ground.
    let la = net.loads.iter().find(|l| l.name == "la").unwrap();
    assert_eq!(la.terminal_map, vec!["1", "4"]);
}

#[test]
fn ten_conductor_linecode_types() {
    let net = parse("micro/linecode_10x10.dss");
    let code = net.linecode("lc10").unwrap();
    assert_eq!(code.n_conductors, 10);
    assert_eq!(code.r_series.len(), 10);
    assert!((code.r_series[9][9] - 0.25e-3).abs() < 1e-12);
    let line = net.lines.iter().find(|l| l.name == "l10").unwrap();
    assert_eq!(line.terminal_map_to.len(), 10);
}

#[test]
#[allow(clippy::float_cmp)]
fn grounding_reactor_types_as_an_inductive_shunt() {
    use powerio_dist::parse_dss_str;
    let net = parse_dss_str(
        "New Circuit.c basekv=4.16\n\
         New Reactor.rx bus1=b2 phases=3 kvar=900 kv=4.16\n",
    );
    let sh = net
        .shunts
        .iter()
        .find(|s| s.name.eq_ignore_ascii_case("rx"))
        .expect("reactor typed as a shunt");
    // Inductive: the diagonal susceptance is negative, the capacitor's mirror.
    let v_phase = 4.16e3 / 3f64.sqrt();
    let expected = -900e3 / 3.0 / (v_phase * v_phase);
    assert!((sh.b[0][0] - expected).abs() < 1e-12, "{}", sh.b[0][0]);
    assert_eq!(sh.g[0][0], 0.0);
    // No silent loss: nothing falls through to the untyped layer.
    assert!(net.untyped.is_empty(), "{:?}", net.untyped);
}

#[test]
fn reactor_defaults_are_materialized_and_recorded() {
    use powerio_dist::parse_dss_str;
    let net = parse_dss_str("New Circuit.c basekv=12.47\nNew Reactor.rd bus1=b2\n");
    assert!(net.shunts.iter().any(|s| s.name.eq_ignore_ascii_case("rd")));
    let recorded = net
        .defaulted
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("reactor.rd"))
        .map(|(_, v)| v)
        .expect("defaults recorded for the reactor");
    assert!(recorded.contains(&"kvar"), "{recorded:?}");
    assert!(recorded.contains(&"kv"), "{recorded:?}");
}

#[test]
#[allow(clippy::float_cmp)]
fn grounding_impedance_reactors_type_as_conductive_shunts() {
    use powerio_dist::parse_dss_str;
    let net = parse_dss_str(
        "New Circuit.c basekv=0.4\n\
         New Reactor.tx_busgrounding_B179 phases=1 bus1=B179.4 bus2=B179.0 r=0.3 x=0.0\n\
         New Reactor.loadbusgrounding_B3230 phases=1 bus1=B3230.4 bus2=B3230.0 r=10.0 x=0.0\n\
         New Reactor.loadbusgrounding_B2656 phases=1 bus1=B2656.4 bus2=B2656.0 r=10.0 x=0.0\n",
    );
    assert_eq!(net.shunts.len(), 3, "{:?}", net.warnings);
    assert!(
        net.untyped
            .iter()
            .all(|o| !o.class.eq_ignore_ascii_case("reactor"))
    );
    let first = net
        .shunts
        .iter()
        .find(|s| s.name == "tx_busgrounding_B179")
        .unwrap();
    assert_eq!(first.bus, "B179");
    assert_eq!(first.terminal_map, vec!["4"]);
    assert_eq!(first.g[0][0], 1.0 / 0.3);
    assert_eq!(first.b[0][0], 0.0);
    let second = net
        .shunts
        .iter()
        .find(|s| s.name == "loadbusgrounding_B3230")
        .unwrap();
    assert_eq!(second.terminal_map, vec!["4"]);
    assert_eq!(second.g[0][0], 0.1);
    assert_eq!(second.b[0][0], 0.0);
}

#[test]
fn grounding_reactor_with_rx_uses_admittance_inverse() {
    use powerio_dist::parse_dss_str;
    let net = parse_dss_str(
        "New Circuit.c basekv=4.16\nNew Reactor.rz bus1=b2.1 bus2=b2.0 phases=1 r=3 x=4\n",
    );
    let sh = net.shunts.iter().find(|s| s.name == "rz").unwrap();
    assert!((sh.g[0][0] - 0.12).abs() < 1e-12, "{}", sh.g[0][0]);
    assert!((sh.b[0][0] + 0.16).abs() < 1e-12, "{}", sh.b[0][0]);
}

#[test]
fn grounding_reactor_bus2_uses_the_dss_fill_rule() {
    use powerio_dist::parse_dss_str;
    let net = parse_dss_str(
        "New Circuit.c basekv=4.16\n\
         New Reactor.rz bus1=b2.1.2.3 bus2=b2.0 phases=3 r=3 x=4\n",
    );
    assert!(net.untyped.iter().any(|o| o.name == "rz"));
    assert!(net.shunts.iter().all(|s| s.name != "rz"));
    assert!(
        net.warnings
            .iter()
            .any(|w| w.contains("reactor rz") && w.contains("series"))
    );

    let net = parse_dss_str(
        "New Circuit.c basekv=4.16\n\
         New Reactor.rz bus1=b2.1.2.3 bus2=b2.0.0.0 phases=3 r=3 x=4\n",
    );
    let sh = net.shunts.iter().find(|s| s.name == "rz").unwrap();
    assert_eq!(sh.terminal_map, vec!["1", "2", "3"]);
}

#[test]
fn zero_impedance_grounding_reactor_stays_untyped() {
    use powerio_dist::parse_dss_str;
    let net = parse_dss_str(
        "New Circuit.c basekv=4.16\nNew Reactor.rz bus1=b2.1 bus2=b2.0 phases=1 r=0 x=0\n",
    );
    assert!(net.untyped.iter().any(|o| o.name == "rz"));
    assert!(net.shunts.iter().all(|s| s.name != "rz"));
    assert!(net.warnings.iter().any(|w| w.contains("zero impedance")));
}

#[test]
fn grounding_reactor_with_unparseable_rx_stays_untyped() {
    use powerio_dist::parse_dss_str;
    // A non-numeric `r` fails to evaluate; substituting 0 would emit a lossless
    // grounding reactor, so the object stays untyped with a warning instead.
    let net = parse_dss_str(
        "New Circuit.c basekv=4.16\n\
         New Reactor.rz bus1=b2.1 bus2=b2.0 phases=1 r=notanumber x=4\n",
    );
    assert!(net.untyped.iter().any(|o| o.name == "rz"));
    assert!(net.shunts.iter().all(|s| s.name != "rz"));
    assert!(
        net.warnings
            .iter()
            .any(|w| w.contains("reactor rz") && w.contains("does not evaluate"))
    );
}

#[test]
fn delta_capacitor_and_reactor_type_as_shunt_matrices() {
    use powerio_dist::parse_dss_str;
    let net = parse_dss_str(
        "New Circuit.c basekv=4.16\n\
         New Capacitor.capd bus1=b2.1.2.3 phases=3 conn=delta kvar=900 kv=4.16\n\
         New Reactor.rxd bus1=b3.1.2.3 phases=3 conn=delta kvar=600 kv=4.16\n",
    );
    assert_eq!(net.shunts.len(), 2, "{:?}", net.warnings);
    assert!(
        net.untyped
            .iter()
            .all(|o| o.name != "capd" && o.name != "rxd")
    );
    let cap = net.shunts.iter().find(|s| s.name == "capd").unwrap();
    assert_eq!(cap.terminal_map, vec!["1", "2", "3"]);
    assert!(cap.b[0][0] > 0.0, "{:?}", cap.b);
    assert!(cap.b[0][1] < 0.0, "{:?}", cap.b);
    assert!((cap.b[0][0] + cap.b[0][1] + cap.b[0][2]).abs() < 1e-12);
    let rx = net.shunts.iter().find(|s| s.name == "rxd").unwrap();
    assert!(rx.b[0][0] < 0.0, "{:?}", rx.b);
    assert!(rx.b[0][1] > 0.0, "{:?}", rx.b);
}

#[test]
fn series_and_non_ground_impedance_reactors_stay_untyped() {
    use powerio_dist::parse_dss_str;
    // Series reactor (bus2): deferred, like the series capacitor.
    let net = parse_dss_str(
        "New Circuit.c basekv=4.16\nNew Reactor.rs bus1=b2 bus2=b3 phases=3 kvar=900 kv=4.16\n",
    );
    assert!(net.untyped.iter().any(|o| o.name == "rs"));
    assert!(net.shunts.iter().all(|s| s.name != "rs"));
    assert!(
        net.warnings
            .iter()
            .any(|w| w.contains("reactor rs") && w.contains("series"))
    );
    // Impedance form without an explicit ground return is not a shunt.
    let net =
        parse_dss_str("New Circuit.c basekv=4.16\nNew Reactor.rz bus1=b2 phases=3 r=0.1 x=5\n");
    assert!(net.untyped.iter().any(|o| o.name == "rz"));
    assert!(
        net.warnings
            .iter()
            .any(|w| w.contains("reactor rz") && w.contains("impedance form"))
    );
    // `parallel` and `rp` are modifiers, not an impedance SpecType: a kvar
    // reactor that also sets them still types as an inductive shunt.
    let net = parse_dss_str(
        "New Circuit.c basekv=4.16\n\
         New Reactor.rmod bus1=b2 phases=3 kvar=900 kv=4.16 parallel=yes rp=1000\n",
    );
    assert!(net.shunts.iter().any(|s| s.name == "rmod"));
    assert!(net.untyped.iter().all(|o| o.name != "rmod"));
}

#[test]
fn regcontrol_warns_and_keeps_taps() {
    let net = parse("opendss/ieee13/IEEE13Nodeckt.dss");
    assert!(
        net.warnings
            .iter()
            .any(|w| w.contains("regcontrol") && w.contains("Reg1"))
    );
    let reg1 = net
        .transformers
        .iter()
        .find(|t| t.name.eq_ignore_ascii_case("reg1"))
        .unwrap();
    assert_eq!(reg1.phases, 1);
}
