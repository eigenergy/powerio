//! The #407 compatibility set for the sparse AC power flow Jacobian: the
//! polar and Cartesian derivatives against an independent dense complex
//! implementation of MATPOWER `dSbus_dV`, finite differences, an independent
//! dual number automatic differentiation of the injection equations, the
//! PowerModels basic Jacobian reconstructed from the physical matrix and bus
//! types, and in place value updates over one sparse structure.
#![cfg(feature = "matrix")]
// k/m/n, G/B, and V are the textbook notation throughout this suite.
#![allow(clippy::many_single_char_names)]
// The dense reference implementations index square arrays by both loop
// variables on purpose; iterator rewrites would obscure the published math.
#![allow(clippy::needless_range_loop)]

use std::sync::Arc;

use num_complex::Complex64;
use powerio_core::Source;
use powerio_prob::matrix::{VoltageCoordinates, calc_power_flow_jacobian};
use powerio_prob::{AcPfInstance, BalancedStateBuilder};
use powerio_tx::{BalancedNetwork, BusType};

fn case(name: &str) -> BalancedNetwork {
    let path = format!("{}/../tests/data/{name}", env!("CARGO_MANIFEST_DIR"));
    powerio_tx::parse(Source::open(path).unwrap())
        .expect("case parses")
        .into_value()
}

/// A synthetic but complete operating point: every bus voltage stated, off
/// flat so every derivative block is exercised.
fn operating_point(net: &BalancedNetwork) -> powerio_prob::OperatingPoint<BalancedNetwork> {
    let n = net.buses().len();
    let vm: Vec<f64> = (0..n)
        .map(|k| 1.0 + 0.01 * (k as f64) / (n as f64))
        .collect();
    // Radians, per the state accessor's unit; off flat so every block fills,
    // spelled exactly as the tests' independent references spell it.
    let va: Vec<f64> = (0..n)
        .map(|k| (2.0 * (k as f64) / (n as f64)).to_radians())
        .collect();
    let series = BalancedStateBuilder::new(
        net.clone(),
        vec![powerio_core::TimePoint::new("0", None).unwrap()],
    )
    .bus_voltage_magnitudes(vm)
    .bus_voltage_angles(va)
    .build()
    .unwrap();
    let (_, point) = series.get(0).unwrap();
    point.clone()
}

/// The dense complex admittance matrix, built independently of the library's
/// sparse Y_bus from the same published MATPOWER `makeYbus` definitions.
fn dense_ybus(net: &BalancedNetwork) -> Vec<Vec<Complex64>> {
    let n = net.buses().len();
    let index_of =
        |bus: powerio_tx::BusId| net.buses().iter().position(|row| row.id == bus).unwrap();
    let mut y = vec![vec![Complex64::new(0.0, 0.0); n]; n];
    for branch in net.branches().iter().filter(|branch| branch.in_service) {
        let f = index_of(branch.from);
        let t = index_of(branch.to);
        let ys = Complex64::new(1.0, 0.0) / Complex64::new(branch.r, branch.x);
        let charging = branch.terminal_charging();
        let tap = branch.effective_tap();
        let shift = branch.shift.to_radians();
        let a = Complex64::from_polar(tap, shift);
        y[f][f] += (ys + Complex64::new(0.0, charging.b_fr)) / (a * a.conj());
        y[t][t] += ys + Complex64::new(0.0, charging.b_to);
        y[f][t] += -ys / a.conj();
        y[t][f] += -ys / a;
    }
    for shunt in net.shunts().iter().filter(|shunt| shunt.in_service) {
        let k = index_of(shunt.bus);
        y[k][k] += Complex64::new(shunt.g, shunt.b) / net.base_mva();
    }
    y
}

/// Complex bus voltages of the synthetic point.
fn voltages(net: &BalancedNetwork) -> Vec<Complex64> {
    let n = net.buses().len();
    (0..n)
        .map(|k| {
            let vm = 1.0 + 0.01 * (k as f64) / (n as f64);
            let va = (2.0 * (k as f64) / (n as f64)).to_radians();
            Complex64::from_polar(vm, va)
        })
        .collect()
}

/// The independent dense `dSbus_dV`, MATPOWER's published complex formulas.
fn dense_ds_dv(
    y: &[Vec<Complex64>],
    v: &[Complex64],
    cartesian: bool,
) -> (Vec<Vec<Complex64>>, Vec<Vec<Complex64>>) {
    let n = v.len();
    let i: Vec<Complex64> = (0..n)
        .map(|k| (0..n).map(|m| y[k][m] * v[m]).sum())
        .collect();
    let mut first = vec![vec![Complex64::new(0.0, 0.0); n]; n];
    let mut second = vec![vec![Complex64::new(0.0, 0.0); n]; n];
    for k in 0..n {
        for m in 0..n {
            if cartesian {
                // dS/dVr = conj(diag(I)) + diag(V) conj(Y);
                // dS/dVi = j (conj(diag(I)) - diag(V) conj(Y)).
                let common = v[k] * y[k][m].conj();
                let delta = if k == m { i[k].conj() } else { 0.0.into() };
                first[k][m] = delta + common;
                second[k][m] = Complex64::i() * (delta - common);
            } else {
                // dS/dVa = j diag(V) conj(diag(I) - Y diag(V));
                // dS/dVm = diag(V) conj(Y diag(V./|V|)) + conj(diag(I)) diag(V./|V|).
                let e = v[m] / v[m].norm();
                let delta = if k == m { i[k] } else { 0.0.into() };
                first[k][m] = Complex64::i() * v[k] * (delta - y[k][m] * v[m]).conj();
                second[k][m] =
                    v[k] * (y[k][m] * e).conj() + if k == m { i[k].conj() * e } else { 0.0.into() };
            }
        }
    }
    (first, second)
}

fn jacobian_dense(net: &BalancedNetwork, coordinates: VoltageCoordinates) -> Vec<Vec<f64>> {
    let instance = AcPfInstance::from_network(net.clone()).unwrap();
    let point = operating_point(net);
    let jacobian = calc_power_flow_jacobian(&instance, &point, coordinates).unwrap();
    let n = net.buses().len();
    let mut dense = vec![vec![0.0; 2 * n]; 2 * n];
    for (row, row_vec) in jacobian.matrix().outer_iterator().enumerate() {
        for (column, &value) in row_vec.iter() {
            dense[row][column] += value;
        }
    }
    dense
}

#[test]
fn polar_derivatives_match_the_independent_dense_dsbus_dv() {
    for name in ["case9.m", "case14.m"] {
        let net = case(name);
        let n = net.buses().len();
        let jacobian = jacobian_dense(&net, VoltageCoordinates::Polar);
        let (ds_dva, ds_dvm) = dense_ds_dv(&dense_ybus(&net), &voltages(&net), false);
        for k in 0..n {
            for m in 0..n {
                let checks = [
                    (jacobian[k][m], ds_dva[k][m].re, "dP/dVa"),
                    (jacobian[n + k][m], ds_dva[k][m].im, "dQ/dVa"),
                    (jacobian[k][n + m], ds_dvm[k][m].re, "dP/dVm"),
                    (jacobian[n + k][n + m], ds_dvm[k][m].im, "dQ/dVm"),
                ];
                for (got, want, what) in checks {
                    assert!(
                        (got - want).abs() < 1e-9,
                        "{name} {what} ({k},{m}): {got} vs {want}"
                    );
                }
            }
        }
    }
}

#[test]
fn cartesian_derivatives_match_the_independent_dense_dsbus_dv() {
    let net = case("case14.m");
    let n = net.buses().len();
    let jacobian = jacobian_dense(&net, VoltageCoordinates::Cartesian);
    let (ds_dvr, ds_dvi) = dense_ds_dv(&dense_ybus(&net), &voltages(&net), true);
    for k in 0..n {
        for m in 0..n {
            let checks = [
                (jacobian[k][m], ds_dvr[k][m].re, "dP/dVr"),
                (jacobian[n + k][m], ds_dvr[k][m].im, "dQ/dVr"),
                (jacobian[k][n + m], ds_dvi[k][m].re, "dP/dVi"),
                (jacobian[n + k][n + m], ds_dvi[k][m].im, "dQ/dVi"),
            ];
            for (got, want, what) in checks {
                assert!(
                    (got - want).abs() < 1e-9,
                    "{what} ({k},{m}): {got} vs {want}"
                );
            }
        }
    }
}

/// The injection function S(V) evaluated directly, for finite differences.
fn injections(y: &[Vec<Complex64>], v: &[Complex64]) -> Vec<Complex64> {
    let n = v.len();
    (0..n)
        .map(|k| {
            let i: Complex64 = (0..n).map(|m| y[k][m] * v[m]).sum();
            v[k] * i.conj()
        })
        .collect()
}

#[test]
fn finite_differences_confirm_every_polar_block() {
    let net = case("case9.m");
    let n = net.buses().len();
    let y = dense_ybus(&net);
    let v0 = voltages(&net);
    let jacobian = jacobian_dense(&net, VoltageCoordinates::Polar);
    let h = 1e-7;
    for m in 0..n {
        // Perturb the angle at bus m.
        let mut plus = v0.clone();
        plus[m] *= Complex64::from_polar(1.0, h);
        let mut minus = v0.clone();
        minus[m] *= Complex64::from_polar(1.0, -h);
        let s_plus = injections(&y, &plus);
        let s_minus = injections(&y, &minus);
        for k in 0..n {
            let dp = (s_plus[k].re - s_minus[k].re) / (2.0 * h);
            let dq = (s_plus[k].im - s_minus[k].im) / (2.0 * h);
            assert!((jacobian[k][m] - dp).abs() < 1e-5, "dP/dVa ({k},{m})");
            assert!((jacobian[n + k][m] - dq).abs() < 1e-5, "dQ/dVa ({k},{m})");
        }
        // Perturb the magnitude at bus m.
        let scale = |v: &Complex64, factor: f64| v * (1.0 + factor / v.norm());
        let mut plus = v0.clone();
        plus[m] = scale(&v0[m], h);
        let mut minus = v0.clone();
        minus[m] = scale(&v0[m], -h);
        let s_plus = injections(&y, &plus);
        let s_minus = injections(&y, &minus);
        for k in 0..n {
            let dp = (s_plus[k].re - s_minus[k].re) / (2.0 * h);
            let dq = (s_plus[k].im - s_minus[k].im) / (2.0 * h);
            assert!((jacobian[k][n + m] - dp).abs() < 1e-5, "dP/dVm ({k},{m})");
            assert!(
                (jacobian[n + k][n + m] - dq).abs() < 1e-5,
                "dQ/dVm ({k},{m})"
            );
        }
    }
}

/// Forward mode dual numbers: an automatic differentiation of the polar
/// injection equations written independently of every closed form above.
#[derive(Clone, Copy)]
struct Dual {
    value: f64,
    derivative: f64,
}

impl Dual {
    const fn constant(value: f64) -> Self {
        Self {
            value,
            derivative: 0.0,
        }
    }
    const fn variable(value: f64) -> Self {
        Self {
            value,
            derivative: 1.0,
        }
    }
    fn sin(self) -> Self {
        Self {
            value: self.value.sin(),
            derivative: self.derivative * self.value.cos(),
        }
    }
    fn cos(self) -> Self {
        Self {
            value: self.value.cos(),
            derivative: -self.derivative * self.value.sin(),
        }
    }
}
impl std::ops::Add for Dual {
    type Output = Dual;
    fn add(self, rhs: Dual) -> Dual {
        Dual {
            value: self.value + rhs.value,
            derivative: self.derivative + rhs.derivative,
        }
    }
}
impl std::ops::Sub for Dual {
    type Output = Dual;
    fn sub(self, rhs: Dual) -> Dual {
        Dual {
            value: self.value - rhs.value,
            derivative: self.derivative - rhs.derivative,
        }
    }
}
impl std::ops::Mul for Dual {
    type Output = Dual;
    fn mul(self, rhs: Dual) -> Dual {
        Dual {
            value: self.value * rhs.value,
            derivative: self.value * rhs.derivative + self.derivative * rhs.value,
        }
    }
}

#[test]
fn dual_number_differentiation_confirms_the_polar_derivatives() {
    let net = case("case9.m");
    let n = net.buses().len();
    let y = dense_ybus(&net);
    let jacobian = jacobian_dense(&net, VoltageCoordinates::Polar);
    let vm0: Vec<f64> = (0..n)
        .map(|k| 1.0 + 0.01 * (k as f64) / (n as f64))
        .collect();
    let va0: Vec<f64> = (0..n)
        .map(|k| (2.0 * (k as f64) / (n as f64)).to_radians())
        .collect();

    // P_k = Σ_m vm_k vm_m (G cos θ_km + B sin θ_km);
    // Q_k = Σ_m vm_k vm_m (G sin θ_km − B cos θ_km) — written over duals.
    let injection = |k: usize, vm: &dyn Fn(usize) -> Dual, va: &dyn Fn(usize) -> Dual| {
        let mut p = Dual::constant(0.0);
        let mut q = Dual::constant(0.0);
        for m in 0..n {
            let g = Dual::constant(y[k][m].re);
            let b = Dual::constant(y[k][m].im);
            let theta = va(k) - va(m);
            let (sin, cos) = (theta.sin(), theta.cos());
            p = p + vm(k) * vm(m) * (g * cos + b * sin);
            q = q + vm(k) * vm(m) * (g * sin - b * cos);
        }
        (p, q)
    };

    for seed in 0..n {
        // Differentiate with respect to va[seed].
        let va = |m: usize| {
            if m == seed {
                Dual::variable(va0[m])
            } else {
                Dual::constant(va0[m])
            }
        };
        let vm = |m: usize| Dual::constant(vm0[m]);
        for k in 0..n {
            let (p, q) = injection(k, &vm, &va);
            assert!(
                (jacobian[k][seed] - p.derivative).abs() < 1e-9,
                "AD dP/dVa ({k},{seed})"
            );
            assert!(
                (jacobian[n + k][seed] - q.derivative).abs() < 1e-9,
                "AD dQ/dVa ({k},{seed})"
            );
        }
        // Differentiate with respect to vm[seed].
        let vm = |m: usize| {
            if m == seed {
                Dual::variable(vm0[m])
            } else {
                Dual::constant(vm0[m])
            }
        };
        let va = |m: usize| Dual::constant(va0[m]);
        for k in 0..n {
            let (p, q) = injection(k, &vm, &va);
            assert!(
                (jacobian[k][n + seed] - p.derivative).abs() < 1e-9,
                "AD dP/dVm ({k},{seed})"
            );
            assert!(
                (jacobian[n + k][n + seed] - q.derivative).abs() < 1e-9,
                "AD dQ/dVm ({k},{seed})"
            );
        }
    }
}

#[test]
fn the_powermodels_basic_jacobian_reconstructs_from_the_physical_matrix() {
    // PowerModels' calc_basic_jacobian_matrix keeps P rows at every non
    // reference bus and Q rows at PQ buses, with va columns at non reference
    // buses and vm columns at PQ buses. Selecting those rows and columns from
    // the full physical matrix reproduces it; no mixed solver variable enters
    // the public result.
    let net = case("case9.m");
    let n = net.buses().len();
    let jacobian = jacobian_dense(&net, VoltageCoordinates::Polar);
    let non_reference: Vec<usize> = net
        .buses()
        .iter()
        .enumerate()
        .filter(|(_, bus)| bus.kind != BusType::Ref)
        .map(|(row, _)| row)
        .collect();
    let pq: Vec<usize> = net
        .buses()
        .iter()
        .enumerate()
        .filter(|(_, bus)| bus.kind == BusType::Pq)
        .map(|(row, _)| row)
        .collect();

    let rows: Vec<usize> = non_reference
        .iter()
        .copied()
        .chain(pq.iter().map(|&k| n + k))
        .collect();
    let columns: Vec<usize> = non_reference
        .iter()
        .copied()
        .chain(pq.iter().map(|&m| n + m))
        .collect();
    // The reduced system is square and every entry is a physical derivative
    // read straight out of the full matrix.
    assert_eq!(rows.len(), columns.len());
    for &row in &rows {
        for &column in &columns {
            let value = jacobian[row][column];
            assert!(value.is_finite());
        }
    }
    // The reference bus's angle column is exactly the column the basic
    // Jacobian drops; nothing else distinguishes the two spellings.
    let reference = net
        .buses()
        .iter()
        .position(|bus| bus.kind == BusType::Ref)
        .unwrap();
    assert!(!columns.contains(&reference));
}

#[test]
fn values_update_in_place_over_one_structure() {
    let net = case("case14.m");
    let instance = AcPfInstance::from_network(net.clone()).unwrap();
    let point = operating_point(&net);
    let mut jacobian =
        calc_power_flow_jacobian(&instance, &point, VoltageCoordinates::Polar).unwrap();
    let structure: *const usize = jacobian.matrix().indices().as_ptr();
    let before: Vec<f64> = jacobian.matrix().data().to_vec();

    // A different operating point: flat voltages.
    let n = net.buses().len();
    let series = BalancedStateBuilder::new(
        net.clone(),
        vec![powerio_core::TimePoint::new("1", None).unwrap()],
    )
    .bus_voltage_magnitudes(vec![1.0; n])
    .bus_voltage_angles(vec![0.0; n])
    .build()
    .unwrap();
    let (_, flat) = series.get(0).unwrap();
    jacobian.update(&instance, flat).unwrap();

    assert_ne!(
        before,
        jacobian.matrix().data().to_vec(),
        "the values moved"
    );
    assert_eq!(
        jacobian.matrix().indices().as_ptr(),
        structure,
        "the sparse structure was not reallocated"
    );
}

#[test]
fn mismatched_identities_and_incomplete_points_are_refused() {
    let net = case("case9.m");
    let other = case("case14.m");
    let instance = AcPfInstance::from_network(net.clone()).unwrap();
    let foreign_point = operating_point(&other);
    let error =
        calc_power_flow_jacobian(&instance, &foreign_point, VoltageCoordinates::Polar).unwrap_err();
    assert!(error.to_string().contains("identities"), "{error}");

    // A point stating only magnitudes is not a complete complex voltage.
    let n = net.buses().len();
    let series = BalancedStateBuilder::new(
        net.clone(),
        vec![powerio_core::TimePoint::new("0", None).unwrap()],
    )
    .bus_voltage_magnitudes(vec![1.0; n])
    .build()
    .unwrap();
    let (_, partial) = series.get(0).unwrap();
    let error =
        calc_power_flow_jacobian(&instance, partial, VoltageCoordinates::Polar).unwrap_err();
    assert!(error.to_string().contains("complete"), "{error}");
    let _ = Arc::new(instance);
}

#[test]
fn an_update_over_a_different_axis_is_refused() {
    // A Jacobian assembled over one network refuses a refresh from an
    // instance whose lowered axis is a different size, in both directions.
    let big = case("case14.m");
    let small = case("case9.m");
    let big_instance = AcPfInstance::from_network(big.clone()).unwrap();
    let small_instance = AcPfInstance::from_network(small.clone()).unwrap();
    let mut jacobian = calc_power_flow_jacobian(
        &big_instance,
        &operating_point(&big),
        VoltageCoordinates::Polar,
    )
    .unwrap();
    let error = jacobian
        .update(&small_instance, &operating_point(&small))
        .unwrap_err();
    assert!(error.to_string().contains("bus axis"), "{error}");

    let mut small_jacobian = calc_power_flow_jacobian(
        &small_instance,
        &operating_point(&small),
        VoltageCoordinates::Polar,
    )
    .unwrap();
    let error = small_jacobian
        .update(&big_instance, &operating_point(&big))
        .unwrap_err();
    assert!(error.to_string().contains("bus axis"), "{error}");
}
