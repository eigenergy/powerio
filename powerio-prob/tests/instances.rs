//! The #412 completion behaviors: seven instances and seven solutions with
//! shared immutable networks and instances, typed objectives, exact bus
//! specifications, refusal of inconsistent input, transformation
//! diagnostics, and the explicit zero impedance merge.

use std::sync::Arc;

use powerio_core::Source;
use powerio_prob::{
    AcBusSpecification, AcOpfInstance, AcPfInstance, DcOpfInstance, DcPfInstance, DcPfSolution,
    McAcOpfInstance, McAcPfInstance, McAcPfSolution, ObjectiveTerm, Termination,
    merge_zero_impedance_buses,
};
use powerio_tx::{BalancedNetwork, BusId};

fn case9() -> BalancedNetwork {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/data/case9.m");
    powerio_tx::parse(Source::open(path).unwrap())
        .expect("case9 parses")
        .into_value()
}

fn dss_network() -> powerio_dist::MulticonductorNetwork {
    let dss = "New Circuit.c basekv=12.47 pu=1 phases=3 bus1=a\n\
               New Line.l1 bus1=a.1.2.3 bus2=b.1.2.3 phases=3 r1=0.1 x1=0.2 length=1 units=km\n\
               New Load.ld bus1=b.1.2.3 phases=3 conn=wye kv=7.2 kw=30 kvar=9\n";
    let source = Source::from_bytes("<memory>", dss.as_bytes().to_vec())
        .unwrap()
        .with_format(powerio_core::FormatId::new("dss").unwrap());
    powerio_dist::parse(source).unwrap().into_value()
}

#[test]
fn every_instance_shares_the_network_without_copying_tables() {
    let net = case9();
    let bus_ptr = net.buses().as_ptr();

    let dc_pf = DcPfInstance::from_network(net.clone()).unwrap();
    let ac_pf = AcPfInstance::from_network(net.clone()).unwrap();
    let dc_opf = DcOpfInstance::from_network(net.clone()).unwrap();
    let ac_opf = AcOpfInstance::from_network(net.clone()).unwrap();
    for network in [
        dc_pf.network(),
        ac_pf.network(),
        dc_opf.network(),
        ac_opf.network(),
    ] {
        assert_eq!(network.buses().as_ptr(), bus_ptr);
    }

    // Cloning an instance clones no network table.
    let cloned = ac_opf.clone();
    assert_eq!(cloned.network().buses().as_ptr(), bus_ptr);
}

#[test]
fn pf_specifications_state_the_boundary_exactly() {
    let net = case9();
    let ac = AcPfInstance::from_network(net.clone()).unwrap();
    assert_eq!(ac.specifications().len(), net.buses().len());
    // case9: bus 1 is the slack with a generator; buses without elements are
    // PQ with zero net injection; generator buses 2 and 3 are PV.
    assert!(matches!(
        ac.specifications()[0],
        AcBusSpecification::Reference { .. }
    ));
    assert!(matches!(
        ac.specifications()[1],
        AcBusSpecification::Pv { .. }
    ));
    let AcBusSpecification::Pq { p, q } = ac.specifications()[3] else {
        panic!("bus 4 is a PQ bus")
    };
    assert_eq!(p.to_bits(), 0.0f64.to_bits());
    assert_eq!(q.to_bits(), 0.0f64.to_bits());

    // The DC projection of the same problem discards reactive data and
    // records the discard and the flat voltage assumption.
    let (dc, diagnostics) = ac.to_dc_pf();
    assert_eq!(dc.specifications().len(), net.buses().len());
    assert_eq!(diagnostics.len(), 2);
    assert!(
        diagnostics
            .iter()
            .any(|d| d.code() == "TRANSFORM.INSTANCE.DATA_DISCARDED")
    );
    assert!(
        diagnostics
            .iter()
            .any(|d| d.code() == "TRANSFORM.INSTANCE.ASSUMPTION")
    );
}

#[test]
fn conflicting_voltage_controllers_are_refused_at_construction() {
    let mut net = case9();
    // A second in service generator at the slack bus with a different
    // setpoint: conflicting active voltage controllers.
    let mut second = net.generators()[0].clone();
    second.vg += 0.05;
    net.generators_mut().push(second);
    let error = AcPfInstance::from_network(net).unwrap_err();
    assert_eq!(
        error.info().map(|info| info.code),
        Some("BUILD.INSTANCE.VOLTAGE_CONTROL_CONFLICT")
    );
}

#[test]
fn the_objective_edit_never_copies_the_shared_network() {
    let net = case9();
    let bus_ptr = net.buses().as_ptr();
    let instance = DcOpfInstance::from_network(net)
        .unwrap()
        .with_objective_term(ObjectiveTerm::DifferentiabilityRegularization { weight: 1e-6 });
    assert_eq!(instance.objective().terms().len(), 2);
    assert_eq!(instance.network().buses().as_ptr(), bus_ptr);
    let (pf, diagnostics) = instance.to_dc_pf().unwrap();
    assert_eq!(pf.network().buses().as_ptr(), bus_ptr);
    assert_eq!(diagnostics.len(), 1);
}

#[test]
fn several_solutions_share_one_immutable_instance() {
    let net = case9();
    let buses = net.buses().len();
    let branches = net.branches().len();
    let instance = Arc::new(DcPfInstance::from_network(net).unwrap());

    let first = DcPfSolution::new(
        Arc::clone(&instance),
        Termination::Converged,
        vec![0.0; buses],
        vec![1.0; buses],
        vec![2.0; branches],
        vec![-2.0; branches],
    )
    .unwrap()
    .with_producer("solver-a");
    let second = DcPfSolution::new(
        Arc::clone(&instance),
        Termination::IterationLimit,
        vec![0.5; buses],
        vec![1.5; buses],
        vec![3.0; branches],
        vec![-3.0; branches],
    )
    .unwrap();

    // Both solutions read through one instance and one network allocation.
    assert!(std::ptr::eq(first.instance(), second.instance()));
    assert_eq!(
        first.network().buses().as_ptr(),
        second.network().buses().as_ptr()
    );
    // Cloning a solution duplicates neither its instance nor its network.
    let cloned = first.clone();
    assert!(std::ptr::eq(cloned.instance(), first.instance()));

    // Values read back by stable identity.
    assert_eq!(first.bus_voltage_angle(BusId(1)), Some(0.0));
    assert_eq!(first.bus_active_injection(BusId(1)), Some(1.0));
    let branch_identity = first.branch_identity_order().next().unwrap();
    assert_eq!(first.branch_from_active_flow(&branch_identity), Some(2.0));
    assert_eq!(first.bus_voltage_angle(BusId(9999)), None);
    assert_eq!(first.producer(), Some("solver-a"));
    assert_eq!(*second.termination(), Termination::IterationLimit);
}

#[test]
fn dimensionally_inconsistent_solutions_are_refused() {
    let net = case9();
    let buses = net.buses().len();
    let branches = net.branches().len();
    let instance = Arc::new(DcPfInstance::from_network(net).unwrap());
    let error = DcPfSolution::new(
        instance,
        Termination::Converged,
        vec![0.0; buses + 1], // one angle too many
        vec![1.0; buses],
        vec![2.0; branches],
        vec![-2.0; branches],
    )
    .unwrap_err();
    assert_eq!(
        error.info().map(|info| info.code),
        Some("BUILD.SOLUTION.SHAPE_MISMATCH")
    );
}

#[test]
fn multiconductor_instances_and_solutions_share_the_network() {
    let net = dss_network();
    let bus_ptr = net.buses().as_ptr();
    let pf = McAcPfInstance::from_network(net.clone()).unwrap();
    assert_eq!(pf.network().buses().as_ptr(), bus_ptr);
    assert_eq!(pf.loads().len(), 1);
    assert_eq!(pf.loads()[0].p_w.len(), 3);
    assert_eq!(pf.sources().len(), 1);

    let opf = McAcOpfInstance::from_network(net.clone()).unwrap();
    let (derived, diagnostics) = opf.to_mc_ac_pf().unwrap();
    assert_eq!(derived.network().buses().as_ptr(), bus_ptr);
    assert_eq!(diagnostics.len(), 1);

    let terminals: usize = net.buses().iter().map(|bus| bus.terminals.len()).sum();
    let source_terminals: usize = net
        .sources()
        .iter()
        .map(|source| source.terminal_map.len())
        .sum();
    let instance = Arc::new(pf);
    let solution = McAcPfSolution::new(
        Arc::clone(&instance),
        Termination::Converged,
        vec![7200.0; terminals],
        vec![0.0; terminals],
        vec![10_000.0; source_terminals],
    )
    .unwrap();
    let bus = &net.buses()[0];
    assert_eq!(
        solution.terminal_voltage_magnitude(&bus.id, bus.terminals[0].as_str()),
        Some(7200.0)
    );
    assert!(std::ptr::eq(solution.instance(), instance.as_ref()));
}

#[test]
fn zero_impedance_branches_are_preserved_until_the_explicit_merge() {
    let mut net = case9();
    // A zero impedance tie between buses 5 and 6.
    let mut tie = net.branches()[0].clone();
    tie.from = BusId(5);
    tie.to = BusId(6);
    tie.r = 0.0;
    tie.x = 0.0;
    tie.uid = Some("tie-5-6".to_owned());
    net.branches_mut().push(tie);
    let branch_count = net.branches().len();

    // The instance preserves the branch; the finite projection's refusal by
    // default (rather than skipping) is covered beside the private assembly
    // in `dcopf_tests`.
    let instance = DcOpfInstance::from_network(net.clone()).unwrap();
    assert_eq!(instance.network().branches().len(), branch_count);

    // The explicit merge returns the mapping and the diagnostics, and the
    // merged network projects.
    let (merged, mapping, diagnostics) = merge_zero_impedance_buses(&net).unwrap();
    assert_eq!(mapping.merged_buses.get(&BusId(6)), Some(&BusId(5)));
    assert_eq!(mapping.removed_branches, vec!["tie-5-6".to_owned()]);
    assert!(
        diagnostics
            .iter()
            .any(|d| d.code() == "CANONICALIZE.MERGE.ZERO_IMPEDANCE")
    );
    assert_eq!(merged.buses().len(), net.buses().len() - 1);
    assert_eq!(merged.branches().len(), branch_count - 1);
    assert!(
        merged
            .branches()
            .iter()
            .all(|branch| { branch.from != BusId(6) && branch.to != BusId(6) })
    );
    // The untouched input network still carries the branch.
    assert_eq!(net.branches().len(), branch_count);
}
