//! The generator cost curve policy, over real cases.
//!
//! The in-repo cases carry a cost row lifted from
//! `Texas7k_20210804.m`: six breakpoints whose segment slopes dip by about
//! 0.07 percent partway along. A source that tabulates a cost curve to two
//! decimals produces exactly that, and it is the shape the policy has to
//! decide about.
//!
//! Set `POWERIO_TEXAS7K` to that file to run the same checks over the whole
//! 731-generator case.

mod helpers;
#[allow(unused_imports)]
use helpers::*;

use powerio_matrix::{
    BalancedNetwork, CostCurveAction, CostCurveDeparture, CostCurvePolicy, DcOpfAssemblyOptions,
    DcOpfPreparation, Error, GenCost, build_dc_opf_preparation, cost_curve_diagnostics,
};
use powerio_prob::DcOpfInstance;

/// Generator 2 of `Texas7k_20210804.m`: the first slope, 85.7987, is above the
/// second, 85.7389.
const TEXAS7K_GENERATOR_2: [f64; 12] = [
    15.73, 2029.89, 23.38, 2686.25, 31.04, 3343.01, 38.69, 4000.17, 46.35, 4657.74, 54.0, 5315.71,
];

fn case5_with_texas7k_cost_row() -> BalancedNetwork {
    let mut network =
        parse_matpower_file("../tests/data/pglib/pglib_opf_case5_pjm.m").expect("parse case5");
    network.generators_mut()[0].cost =
        Some(GenCost::new(1, 0.0, 0.0, TEXAS7K_GENERATOR_2.to_vec()));
    network
}

fn prepare(
    network: &BalancedNetwork,
    policy: CostCurvePolicy,
) -> Result<DcOpfPreparation, powerio_matrix::Error> {
    let instance = DcOpfInstance::from_network(network.clone()).expect("DC OPF instance");
    build_dc_opf_preparation(
        &instance,
        &DcOpfAssemblyOptions::default().with_cost_curve_policy(policy),
    )
}

#[test]
fn the_default_policy_builds_and_reports_the_nonconvex_row() {
    let network = case5_with_texas7k_cost_row();
    let prepared = prepare(&network, CostCurvePolicy::Any).expect("the default policy builds");

    assert_eq!(prepared.cost_curve_projections.len(), 1);
    let projection = &prepared.cost_curve_projections[0];
    assert_eq!(projection.source_row, 0);
    assert_eq!(projection.departure, CostCurveDeparture::NonconvexPiecewise);
    assert_eq!(projection.action, CostCurveAction::Kept);
    assert_eq!(projection.projection_loss.to_bits(), 0.0_f64.to_bits());

    // The stated curve reaches the arrays unchanged: `Any` decides nothing
    // about the data, it only declines to refuse it.
    let curve = prepared.generators.piecewise_linear[0]
        .as_ref()
        .expect("a piecewise column");
    assert_eq!(curve.power.len(), 6);
    assert_eq!(curve.value[0].to_bits(), TEXAS7K_GENERATOR_2[1].to_bits());

    let diagnostics = cost_curve_diagnostics(&prepared.cost_curve_projections);
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(
        diagnostics[0].code(),
        "BUILD.INSTANCE.PIECEWISE_COST_NONCONVEX"
    );
    assert_eq!(
        diagnostics[0].severity(),
        powerio_core::DiagnosticSeverity::Warning
    );
}

#[test]
fn convex_only_still_refuses_the_nonconvex_row() {
    let network = case5_with_texas7k_cost_row();
    let error =
        prepare(&network, CostCurvePolicy::ConvexOnly).expect_err("convex_only refuses the row");
    assert!(
        matches!(
            error,
            Error::NonconvexPiecewiseCost {
                gen_index: 0,
                segment: 1
            }
        ),
        "{error}"
    );
    assert_eq!(error.code().code, "BUILD.INSTANCE.PIECEWISE_COST_NONCONVEX");
}

#[test]
fn the_lower_envelope_convexifies_the_row_and_prices_what_it_drops() {
    let network = case5_with_texas7k_cost_row();
    let prepared =
        prepare(&network, CostCurvePolicy::ConvexifyLowerEnvelope).expect("convexified build");

    let projection = &prepared.cost_curve_projections[0];
    assert_eq!(projection.action, CostCurveAction::LowerEnvelope);
    // Two decimals of tabulation, so the curve the envelope replaces is only
    // cents away from it over a row that spans thousands of dollars.
    assert!(
        projection.projection_loss > 0.0 && projection.projection_loss < 1.0,
        "loss {} is not the rounding-scale gap this row states",
        projection.projection_loss
    );

    let curve = prepared.generators.piecewise_linear[0]
        .as_ref()
        .expect("a piecewise column");
    assert_convex(&curve.power, &curve.value);
    // Both extreme breakpoints survive, so the range the curve prices is the
    // stated one. Powers scale into per unit; values never do.
    assert_eq!(curve.value.first(), Some(&TEXAS7K_GENERATOR_2[1]));
    assert_eq!(curve.value.last(), Some(&TEXAS7K_GENERATOR_2[11]));
}

#[test]
fn a_convex_case_records_nothing_under_every_policy() {
    let network =
        parse_matpower_file("../tests/data/pglib/pglib_opf_case14_ieee.m").expect("parse case14");
    let baseline = prepare(&network, CostCurvePolicy::Any).expect("a convex case builds");
    assert!(baseline.cost_curve_projections.is_empty());

    for policy in [
        CostCurvePolicy::ConvexOnly,
        CostCurvePolicy::ConvexifyLowerEnvelope,
    ] {
        let prepared = prepare(&network, policy).expect("a convex case builds");
        assert!(prepared.cost_curve_projections.is_empty());
        assert_eq!(
            prepared.generators.q, baseline.generators.q,
            "{policy} changed a convex case"
        );
        assert_eq!(prepared.generators.c, baseline.generators.c);
        assert_eq!(
            prepared.generators.piecewise_linear,
            baseline.generators.piecewise_linear
        );
    }
}

/// The whole Texas7k case, when the dataset is available.
#[test]
fn texas7k_builds_under_the_default_policy() {
    let Ok(path) = std::env::var("POWERIO_TEXAS7K") else {
        eprintln!("skipped: POWERIO_TEXAS7K is not set");
        return;
    };
    let network = parse_matpower_file(&path).expect("parse Texas7k");

    let prepared = prepare(&network, CostCurvePolicy::Any).expect("the default policy builds");
    assert_eq!(
        prepared.cost_curve_projections.len(),
        124,
        "the nonconvex generator count of Texas7k_20210804"
    );
    assert!(
        prepared
            .cost_curve_projections
            .iter()
            .all(|projection| projection.action == CostCurveAction::Kept)
    );

    assert!(prepare(&network, CostCurvePolicy::ConvexOnly).is_err());

    let convexified =
        prepare(&network, CostCurvePolicy::ConvexifyLowerEnvelope).expect("convexified build");
    assert_eq!(convexified.cost_curve_projections.len(), 124);
    for (column, curve) in convexified.generators.piecewise_linear.iter().enumerate() {
        let curve = curve.as_ref().expect("every Texas7k row is piecewise");
        assert_convex(&curve.power, &curve.value);
        assert_eq!(
            curve.value.last(),
            prepared.generators.piecewise_linear[column]
                .as_ref()
                .and_then(|stated| stated.value.last()),
            "column {column} moved its last breakpoint"
        );
    }
}

fn assert_convex(power: &[f64], value: &[f64]) {
    let slopes: Vec<f64> = (0..power.len() - 1)
        .map(|i| (value[i + 1] - value[i]) / (power[i + 1] - power[i]))
        .collect();
    for pair in slopes.windows(2) {
        assert!(
            pair[0] <= pair[1],
            "slopes {slopes:?} are not nondecreasing"
        );
    }
}

/// A piecewise cost case still writes the whole bundle: only the three bus
/// space cost files, which no bus space quadratic can carry, stay out.
#[test]
fn a_piecewise_case_writes_the_bundle_without_the_nodal_cost_files() {
    let network = case5_with_texas7k_cost_row();
    let instance = DcOpfInstance::from_network(network).expect("DC OPF instance");
    let output = tempfile::tempdir().expect("tempdir");
    let bundle = powerio_matrix::emit_dcopf_bundle(
        &instance,
        output.path(),
        &powerio_matrix::DcOpfBundleOptions::default(),
    )
    .expect("a piecewise case writes a bundle");

    for present in [
        "A.mtx",
        "L.mtx",
        "pmax.mtx",
        "pmin.mtx",
        "q_gen.mtx",
        "c_gen.mtx",
        "c0_gen.mtx",
        "dcopf_meta.json",
    ] {
        assert!(bundle.dir.join(present).is_file(), "{present} is missing");
    }
    for absent in ["q.mtx", "c.mtx", "c0.mtx"] {
        assert!(
            !bundle.dir.join(absent).exists(),
            "{absent} states a bus space cost this case has none of"
        );
    }

    let manifest: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(bundle.dir.join("dcopf_meta.json")).expect("manifest"),
    )
    .expect("manifest json");
    assert_eq!(manifest["nodal_cost"]["written"], false);
    assert_eq!(manifest["nodal_cost"]["omitted_because"], "piecewise");
    assert_eq!(manifest["cost_curve_policy"], "any");
    assert_eq!(
        manifest["cost_curve_projections"]
            .as_array()
            .expect("projections")
            .len(),
        1
    );

    let codes: Vec<&str> = bundle
        .diagnostics
        .iter()
        .map(powerio_core::Diagnostic::code)
        .collect();
    assert!(codes.contains(&"BUILD.INSTANCE.PIECEWISE_COST_NONCONVEX"));
    assert!(codes.contains(&"BUILD.OPF.NODAL_COST_UNSUPPORTED"));
    assert!(
        bundle
            .diagnostics
            .iter()
            .all(|d| d.severity() == powerio_core::DiagnosticSeverity::Warning)
    );
}

/// A quadratic case is unchanged: the bus space cost files are still written
/// and the bundle reports nothing.
#[test]
fn a_quadratic_case_still_writes_the_nodal_cost_files() {
    let network =
        parse_matpower_file("../tests/data/pglib/pglib_opf_case14_ieee.m").expect("parse case14");
    let instance = DcOpfInstance::from_network(network).expect("DC OPF instance");
    let output = tempfile::tempdir().expect("tempdir");
    let bundle = powerio_matrix::emit_dcopf_bundle(
        &instance,
        output.path(),
        &powerio_matrix::DcOpfBundleOptions::default(),
    )
    .expect("bundle");

    for present in ["q.mtx", "c.mtx", "c0.mtx"] {
        assert!(bundle.dir.join(present).is_file(), "{present} is missing");
    }
    assert!(bundle.diagnostics.is_empty());
    let manifest: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(bundle.dir.join("dcopf_meta.json")).expect("manifest"),
    )
    .expect("manifest json");
    assert_eq!(manifest["nodal_cost"]["written"], true);
    assert!(manifest["nodal_cost"].get("omitted_because").is_none());
}
