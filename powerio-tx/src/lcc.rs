//! The steady state of a line-commutated (LCC) two-terminal DC line at its
//! AC terminal voltages.
//!
//! A two-terminal DC line read from PSS/E keeps its converter records
//! (`psse_dc_*` extras), and those, with the AC voltage at each converter
//! bus, fix the converters' firing and extinction angles, their commutation
//! overlap, and the reactive power each one draws from its AC bus. The
//! [`Hvdc`] record states no converter reactive power (`qf = qt = 0`), so a
//! calculation that wants the converters' physical demand computes it here.
//!
//! The model is the classic six-pulse bridge equations PSS/E states for its
//! two-terminal lines. With `NB` bridges in series, commutating resistance
//! `RC` and reactance `XC` per bridge (ohms), and the converter transformer's
//! primary base voltage `EBAS` (kV), ratio `TR`, and tap `TAP`, the valve-side
//! line-to-line voltage at AC magnitude `V` (per unit) is
//! `E = V·EBAS·TR/TAP`, and each converter's angle `θ` (firing angle `α` at
//! the rectifier, extinction angle `γ` at the inverter) satisfies
//!
//! ```text
//! Vd/NB = (3√2/π)·E·cos θ − (3·XC/π + 2·RC)·Id
//! cos(θ + μ) = cos θ − √2·XC·Id/E
//! Q = P·(2μ + sin 2θ − sin 2(θ + μ)) / (cos 2θ − cos 2(θ + μ))
//! ```
//!
//! with `P = Vd·Id`. The DC operating point comes from the record's
//! schedule: the scheduled compounded voltage `VSCHD` holds
//! `Vd_inverter + RCOMP·Id`, the rectifier sits `RDC·Id` above the inverter,
//! and `SETVL` states the power (at the rectifier, or at the inverter when
//! negative) or, under `MDC 2`, the current.

use serde_json::Value;

use crate::network::{Extras, Hvdc};

/// One converter of an LCC line at an operating point.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct LccConverterState {
    /// DC voltage at the converter, kV.
    pub dc_voltage_kv: f64,
    /// Firing angle at the rectifier, extinction angle at the inverter,
    /// degrees.
    pub angle_deg: f64,
    /// Commutation overlap angle, degrees.
    pub overlap_deg: f64,
    /// DC power through the converter, `Vd·Id`, MW.
    pub p_mw: f64,
    /// Reactive power the converter draws from its AC bus, MVAr. Positive.
    pub q_mvar: f64,
}

/// Both converters of an LCC line at an operating point.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct LccOperatingPoint {
    /// DC current, kA.
    pub dc_current_ka: f64,
    /// The rectifier, at the line's `from` bus.
    pub rectifier: LccConverterState,
    /// The inverter, at the line's `to` bus.
    pub inverter: LccConverterState,
}

/// The converter fields the reactive power depends on, revision independent:
/// the first eight converter tail fields and the commutating capacitor last.
struct Bridge {
    bridges: f64,
    rc: f64,
    xc: f64,
    ebas: f64,
    tr: f64,
    tap: f64,
    xcap: f64,
}

/// The default converter tail the PSS/E reader does not retain: one bridge
/// with no commutating impedance and no AC base, which states no converter.
const DEFAULT_BRIDGE: [f64; 8] = [1.0, 15.0, 5.0, 0.0, 0.0, 0.0, 1.0, 1.0];

fn number(token: &Value) -> Option<f64> {
    token
        .as_str()
        .and_then(|text| text.trim().parse::<f64>().ok())
        .or_else(|| token.as_f64())
        .filter(|value| value.is_finite())
}

fn tail(extras: &Extras, key: &str) -> Option<Vec<f64>> {
    extras.get(key).map(|value| {
        value
            .as_array()
            .map(|tokens| tokens.iter().map(|t| number(t).unwrap_or(0.0)).collect())
            .unwrap_or_default()
    })
}

fn bridge(extras: &Extras, key: &str) -> Bridge {
    let fields = tail(extras, key).unwrap_or_else(|| DEFAULT_BRIDGE.to_vec());
    let at = |i: usize| fields.get(i).copied().unwrap_or(DEFAULT_BRIDGE[i]);
    Bridge {
        bridges: at(0),
        rc: at(3),
        xc: at(4),
        ebas: at(5),
        tr: at(6),
        tap: at(7),
        xcap: if fields.len() > DEFAULT_BRIDGE.len() {
            fields.last().copied().unwrap_or(0.0)
        } else {
            0.0
        },
    }
}

impl Bridge {
    /// The converter at DC voltage `vd` (kV), current `id` (kA), and AC
    /// magnitude `vm` (per unit). `None` when the record states no AC base,
    /// a commutating capacitor (not modeled), or a point no angle reaches.
    fn calc_state(&self, vd: f64, id: f64, vm: f64) -> Option<LccConverterState> {
        if self.bridges < 1.0 || self.ebas <= 0.0 || self.tap <= 0.0 || self.xcap != 0.0 {
            return None;
        }
        let e = vm * self.ebas * self.tr / self.tap;
        if e.is_nan() || e <= 0.0 {
            return None;
        }
        let k = 3.0 * std::f64::consts::SQRT_2 / std::f64::consts::PI;
        let drop = (3.0 * self.xc / std::f64::consts::PI + 2.0 * self.rc) * id;
        let cos_theta = (vd / self.bridges + drop) / (k * e);
        if !(-1.0..=1.0).contains(&cos_theta) {
            return None;
        }
        let theta = cos_theta.acos();
        let cos_end = (cos_theta - std::f64::consts::SQRT_2 * self.xc * id / e).clamp(-1.0, 1.0);
        let mu = cos_end.acos() - theta;
        let p = vd * id;
        let denominator = (2.0 * theta).cos() - (2.0 * (theta + mu)).cos();
        let q = if denominator.abs() > 1e-12 {
            p * (2.0 * mu + (2.0 * theta).sin() - (2.0 * (theta + mu)).sin()) / denominator
        } else {
            // No overlap: the fundamental lags by the angle itself.
            p * theta.tan()
        };
        q.is_finite().then(|| LccConverterState {
            dc_voltage_kv: vd,
            angle_deg: theta.to_degrees(),
            overlap_deg: mu.to_degrees(),
            p_mw: p,
            q_mvar: q.abs(),
        })
    }
}

impl Hvdc {
    /// The LCC operating point of a line read from a PSS/E two-terminal DC
    /// record, at AC voltage magnitudes `vm_rectifier` (the `from` bus) and
    /// `vm_inverter` (the `to` bus), per unit: the DC current, and each
    /// converter's DC voltage, angle, overlap, and reactive demand. See the
    /// [module](crate::lcc) for the equations.
    ///
    /// `None` when the line carries no PSS/E converter record with an AC base
    /// (a line from another format, or a record left at the default converter
    /// fields), states no scheduled voltage, uses capacitor commutation, or
    /// schedules a point no firing or extinction angle reaches at these
    /// voltages. The in service flag is not consulted.
    #[must_use]
    pub fn calc_lcc_operating_point(
        &self,
        vm_rectifier: f64,
        vm_inverter: f64,
    ) -> Option<LccOperatingPoint> {
        let extras = &self.extras;
        let f64_extra = |key: &str| extras.get(key).and_then(Value::as_f64);
        let vschd = f64_extra("psse_dc_vschd")?;
        if vschd <= 0.0 {
            return None;
        }
        let rdc = f64_extra("psse_dc_rdc").unwrap_or(0.0);
        // The control tail starts at VCMOD; RCOMP follows it.
        let rcomp = tail(extras, "psse_dc_control_tail")
            .and_then(|fields| fields.get(1).copied())
            .unwrap_or(0.0);
        let mdc = extras
            .get("psse_dc_mdc")
            .and_then(Value::as_i64)
            .unwrap_or(1);
        let current = if mdc == 2 {
            // A current schedule the reader priced at the scheduled voltage.
            self.pf / vschd
        } else if extras.contains_key("psse_dc_setvl_at_inverter") {
            // (VSCHD − RCOMP·I)·I = P at the inverter.
            solve_current(-rcomp, vschd, self.pt)?
        } else {
            // (VSCHD + (RDC − RCOMP)·I)·I = P at the rectifier.
            solve_current(rdc - rcomp, vschd, self.pf)?
        };
        let vd_inverter = vschd - rcomp * current;
        let vd_rectifier = vd_inverter + rdc * current;
        Some(LccOperatingPoint {
            dc_current_ka: current,
            rectifier: bridge(extras, "psse_dc_rectifier_tail").calc_state(
                vd_rectifier,
                current,
                vm_rectifier,
            )?,
            inverter: bridge(extras, "psse_dc_inverter_tail").calc_state(
                vd_inverter,
                current,
                vm_inverter,
            )?,
        })
    }
}

/// The positive root of `a·I² + b·I = p` for the current `I` (kA), with
/// `b > 0` the scheduled voltage.
fn solve_current(a: f64, b: f64, p: f64) -> Option<f64> {
    if p <= 0.0 {
        return None;
    }
    let current = if a.abs() < 1e-12 {
        p / b
    } else {
        let discriminant = b.mul_add(b, 4.0 * a * p);
        if discriminant < 0.0 {
            return None;
        }
        (discriminant.sqrt() - b) / (2.0 * a)
    };
    (current.is_finite() && current > 0.0).then_some(current)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::BusId;

    /// A 500 kV, 1000 MW bipole-like line: two bridges per converter.
    fn line() -> Hvdc {
        let mut line = Hvdc::new(BusId(1), BusId(2));
        line.pf = 1000.0;
        let tail = |values: &[&str]| {
            Value::Array(values.iter().map(|v| Value::String((*v).into())).collect())
        };
        line.extras.insert("psse_dc_rdc".into(), Value::from(10.0));
        line.extras
            .insert("psse_dc_vschd".into(), Value::from(500.0));
        // VCMOD, RCOMP, DELTI, METER, DCVMIN, CCCITMX, CCCACC.
        line.extras.insert(
            "psse_dc_control_tail".into(),
            tail(&["0.0", "0.0", "0.1", "'I'", "0.0", "20", "1.0"]),
        );
        // NB, ANMX, ANMN, RC, XC, EBAS, TR, TAP, TMX, TMN, STP, IC, IF, IT, ID, XCAP.
        let converter = tail(&[
            "2", "17.5", "12.5", "0.3", "20.0", "345.0", "0.7", "1.0", "1.2", "0.9", "0.0125", "0",
            "0", "0", "'1'", "0.0",
        ]);
        line.extras
            .insert("psse_dc_rectifier_tail".into(), converter.clone());
        line.extras
            .insert("psse_dc_inverter_tail".into(), converter);
        line
    }

    #[test]
    fn the_operating_point_satisfies_the_bridge_equations() {
        let point = line().calc_lcc_operating_point(1.02, 0.98).unwrap();
        // (500 + 10·I)·I = 1000 at the rectifier.
        let i = point.dc_current_ka;
        assert!(((500.0 + 10.0 * i) * i - 1000.0).abs() < 1e-9);
        assert!((point.inverter.dc_voltage_kv - 500.0).abs() < 1e-12);
        assert!((point.rectifier.dc_voltage_kv - (500.0 + 10.0 * i)).abs() < 1e-12);
        let k = 3.0 * 2.0_f64.sqrt() / std::f64::consts::PI;
        for (state, vm) in [(point.rectifier, 1.02), (point.inverter, 0.98)] {
            let e = vm * 345.0 * 0.7;
            let theta = state.angle_deg.to_radians();
            let mu = state.overlap_deg.to_radians();
            let vd = 2.0 * (k * e * theta.cos() - (3.0 * 20.0 / std::f64::consts::PI + 0.6) * i);
            assert!((vd - state.dc_voltage_kv).abs() < 1e-9);
            let overlap = theta.cos() - (theta + mu).cos();
            assert!((overlap - 2.0_f64.sqrt() * 20.0 * i / e).abs() < 1e-12);
            // A lagging power factor between the angle's cosine and the
            // average of the two commutation cosines.
            let tan = state.q_mvar / state.p_mw;
            let pf = 1.0 / (1.0 + tan * tan).sqrt();
            assert!(
                pf < theta.cos() + 1e-12
                    && pf > f64::midpoint(theta.cos(), (theta + mu).cos()) - 0.02
            );
        }
        // A lower AC voltage leaves less margin: the firing angle shrinks.
        let low = line().calc_lcc_operating_point(0.97, 0.98).unwrap();
        assert!(low.rectifier.angle_deg < point.rectifier.angle_deg);
    }

    #[test]
    fn an_inverter_measured_schedule_and_a_current_schedule() {
        let mut line = line();
        line.extras
            .insert("psse_dc_setvl_at_inverter".into(), Value::Bool(true));
        line.pt = 900.0;
        let point = line.calc_lcc_operating_point(1.0, 1.0).unwrap();
        assert!(
            (point.dc_current_ka - 1.8).abs() < 1e-12,
            "900 MW at 500 kV"
        );

        let mut line = super::tests::line();
        line.extras.insert("psse_dc_mdc".into(), Value::from(2));
        line.pf = 1000.0;
        let point = line.calc_lcc_operating_point(1.0, 1.0).unwrap();
        assert!((point.dc_current_ka - 2.0).abs() < 1e-12);
    }

    #[test]
    fn a_pss_e_record_reads_into_an_operating_point() {
        // The converter fields the reader retains are the ones this reads.
        let raw = "0, 100.00, 33, 0, 0, 60.00 / x
CASE
COMMENT
1,'B1          ', 230.0,3,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
4,'B4          ', 345.0,1,1,1,1,1.02,0.0,1.1,0.9,1.1,0.9
5,'B5          ', 345.0,1,1,1,1,0.98,0.0,1.1,0.9,1.1,0.9
0 / END OF BUS DATA, BEGIN LOAD DATA
0 / END OF LOAD DATA, BEGIN FIXED SHUNT DATA
0 / END OF FIXED SHUNT DATA, BEGIN GENERATOR DATA
0 / END OF GENERATOR DATA, BEGIN BRANCH DATA
0 / END OF BRANCH DATA, BEGIN TRANSFORMER DATA
0 / END OF TRANSFORMER DATA, BEGIN AREA DATA
0 / END OF AREA DATA, BEGIN TWO-TERMINAL DC DATA
'POLE', 1, 10.0, 1000.0, 500.0, 0.0, 0.0, 0.1, 'I', 0.0, 20, 1.0
4, 2, 17.5, 12.5, 0.3, 20.0, 345.0, 0.7, 1.0, 1.2, 0.9, 0.0125, 0, 0, 0, '1', 0.0
5, 2, 17.5, 12.5, 0.3, 20.0, 345.0, 0.7, 1.0, 1.2, 0.9, 0.0125, 0, 0, 0, '1', 0.0
0 / END OF TWO-TERMINAL DC DATA, BEGIN VSC DC LINE DATA
Q
";
        let parsed = crate::parse_str(raw, "psse").unwrap();
        let read = &parsed.network.hvdc()[0];
        let point = read.calc_lcc_operating_point(1.02, 0.98).unwrap();
        assert_eq!(point, line().calc_lcc_operating_point(1.02, 0.98).unwrap());
    }

    #[test]
    fn lines_without_a_converter_record_have_no_operating_point() {
        let mut plain = Hvdc::new(BusId(1), BusId(2));
        plain.pf = 100.0;
        assert!(plain.calc_lcc_operating_point(1.0, 1.0).is_none());
        let mut default_tail = line();
        default_tail.extras.remove("psse_dc_rectifier_tail");
        assert!(default_tail.calc_lcc_operating_point(1.0, 1.0).is_none());
        // An AC voltage too low for the schedule leaves no angle.
        assert!(line().calc_lcc_operating_point(0.3, 1.0).is_none());
    }
}
