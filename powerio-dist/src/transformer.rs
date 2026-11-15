//! Transformer winding physics: the pairwise leakage and its reduction, each
//! winding's coil voltage, connection and delta orientation, the neutral
//! grounding and magnetizing branch a unit states, and the finite leakage
//! primitive admittance over its physical terminals.
//!
//! Every multiconductor calculation reads its transformer quantities here:
//! the nodal admittance and its ideal constraints, the multiconductor OPF
//! descriptor, the multiconductor to balanced lowering and the SINCAL
//! writer's expected primitive.

use std::collections::BTreeMap;

use num_complex::Complex64;

use crate::diagnostics::codes;
use crate::{
    Configuration, ConnectionChannel, DistTransformer, DistWinding, DistWindingConn,
    MulticonductorNetwork, calc_element_connection, orient_delta_ring,
};

type Result<T> = std::result::Result<T, powerio_core::Error>;

fn unsupported(name: &str, reason: impl std::fmt::Display) -> powerio_core::Error {
    powerio_core::Error::new(
        &codes::BUILD_DIST_TRANSFORMER_UNSUPPORTED,
        format!("transformer `{name}` is unsupported: {reason}"),
    )
}

fn invalid(
    name: &str,
    field: impl std::fmt::Display,
    reason: impl std::fmt::Display,
) -> powerio_core::Error {
    powerio_core::Error::new(
        &codes::BUILD_DIST_TRANSFORMER_INVALID,
        format!("transformer `{name}` has invalid {field}: {reason}"),
    )
}

fn invalid_leakage(transformer: &DistTransformer, reason: &str) -> powerio_core::Error {
    invalid(&transformer.name, "leakage data", reason)
}

/// A transformer extra as a number: typed producers write numbers and the
/// DSS reader keeps written extras as text. A present value that is neither
/// reads as NaN, so it reaches the caller's finite value checks rather than
/// becoming an omitted zero. The one reader of a transformer's numeric
/// extras.
#[must_use]
pub fn transformer_extra_number(
    extras: &BTreeMap<String, serde_json::Value>,
    key: &str,
) -> Option<f64> {
    extras.get(key).map(|value| {
        value
            .as_f64()
            .or_else(|| value.as_str()?.trim().parse().ok())
            .unwrap_or(f64::NAN)
    })
}

fn subtype(transformer: &DistTransformer) -> Option<&str> {
    transformer
        .extras
        .get("bmopf_subtype")
        .and_then(serde_json::Value::as_str)
}

fn is_autotransformer(transformer: &DistTransformer) -> bool {
    subtype(transformer) == Some("single_phase_autotransformer")
}

/// Whether the unit is a BMOPF regulator (a single phase autotransformer or
/// an open delta regulator leg), whose tap is the regulated over source
/// voltage ratio.
#[must_use]
pub fn is_transformer_regulator(transformer: &DistTransformer) -> bool {
    matches!(
        subtype(transformer),
        Some("single_phase_autotransformer" | "open_delta_regulator")
    )
}

/// The unit system a transformer states its leakage in, which
/// [`calc_transformer_leakage_impedances`] reads.
///
/// The physical quantity is the same in every case: for windings `a` and
/// `b`, the series impedance of the pair referred to the first winding,
/// each winding's own resistance referred through its base and the pair's
/// short circuit reactance. Only the base each value is stated on differs.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum TransformerLeakageBase {
    /// OpenDSS nameplate data: each winding's `r_pct` on its own rating
    /// `S_a`, and `xsc_pct` on the first winding's rating. The result is per
    /// unit on `S_1`.
    Nameplate,
    /// BMOPF n-winding data: each winding's `r_pct` on its phase base
    /// `S_a / phases[a]`, and `xsc_pct` on `power_base / phases[0]`. The
    /// result is per unit on `power_base / phases[0]`.
    WindingPhase {
        /// The pair reactance's power base, volt amperes (`bmopf_power_base`,
        /// the first winding's rating when the source states none).
        power_base: f64,
        /// Each winding's phase count, in winding order.
        phases: Vec<f64>,
    },
    /// A two winding regulator stating its series impedance in ohms on each
    /// side (`r_series_from`, `x_series_from`, `r_series_to`, `x_series_to`
    /// in the extras), with the second winding's `tap` its regulated over
    /// source voltage ratio. The second side is referred through
    /// `n = 1 / tap`, and the result is ohms on the first side:
    /// `z = (r_from + n² r_to) + j (x_from + n² x_to)`.
    StatedOhms,
}

/// The pairwise leakage impedances of a multiwinding transformer, referred
/// to the first winding: for windings `a` and `b`, on a per winding base
/// `B_a` and a reference base `B_ref`,
///
/// ```text
/// z[a][b] = (r_a · B_ref / B_a + r_b · B_ref / B_b) / 100 + j · xsc_ab / 100
/// ```
///
/// with `xsc_pct` in pair order `(1,2), (1,3), …, (2,3), …`. The
/// [`TransformerLeakageBase`] states `B_a` and `B_ref` (the winding rating
/// and the first winding's rating for [`TransformerLeakageBase::Nameplate`],
/// the phase bases for [`TransformerLeakageBase::WindingPhase`]), or reads a
/// regulator's stated ohms ([`TransformerLeakageBase::StatedOhms`]). The
/// diagonal is zero. The one leakage derivation: the multiconductor OPF
/// transformer descriptor scales it to its ohmic base,
/// [`calc_transformer_winding_admittance`] inverts it,
/// [`calc_transformer_primitive_admittance`] stamps it, and the
/// multiconductor to balanced lowering reads a two winding unit's pair.
///
/// # Errors
/// Fewer than two windings, a winding rating that is not finite and
/// positive, a resistance that is not finite, or an `xsc_pct` whose length is
/// not one value per winding pair. For a phase base, a power base that is not
/// finite and positive or a phase count per winding that is missing or not
/// finite and positive. For stated ohms, a transformer without exactly two
/// windings, a second winding tap that is not finite and positive, or a
/// stated value that is not a finite nonnegative number.
pub fn calc_transformer_leakage_impedances(
    transformer: &DistTransformer,
    base: &TransformerLeakageBase,
) -> Result<Vec<Vec<Complex64>>> {
    let windings = &transformer.windings;
    let count = windings.len();
    if count < 2 {
        return Err(invalid_leakage(transformer, "fewer than two windings"));
    }
    if let TransformerLeakageBase::StatedOhms = base {
        return stated_ohms_leakage(transformer);
    }
    if windings
        .iter()
        .any(|w| !w.s_rating.is_finite() || w.s_rating <= 0.0 || !w.r_pct.is_finite())
    {
        return Err(invalid_leakage(
            transformer,
            "a winding rating is not finite and positive or a resistance is not finite",
        ));
    }
    if transformer.xsc_pct.len() != count * (count - 1) / 2 {
        return Err(invalid_leakage(
            transformer,
            "xsc_pct does not state one reactance per winding pair",
        ));
    }
    let (reference_rating, winding_base): (f64, Vec<f64>) = match base {
        TransformerLeakageBase::WindingPhase { power_base, phases } => {
            if !power_base.is_finite() || *power_base <= 0.0 {
                return Err(invalid_leakage(
                    transformer,
                    "the power base is not finite and positive",
                ));
            }
            if phases.len() != count || phases.iter().any(|p| !p.is_finite() || *p <= 0.0) {
                return Err(invalid_leakage(
                    transformer,
                    "a winding phase count is missing or not finite and positive",
                ));
            }
            (
                power_base / phases[0],
                windings
                    .iter()
                    .zip(phases)
                    .map(|(w, p)| w.s_rating / p)
                    .collect(),
            )
        }
        _ => (
            windings[0].s_rating,
            windings.iter().map(|w| w.s_rating).collect(),
        ),
    };
    let resistance: Vec<f64> = windings
        .iter()
        .zip(&winding_base)
        .map(|(w, base)| w.r_pct / 100.0 * reference_rating / base)
        .collect();
    let mut impedance = vec![vec![Complex64::new(0.0, 0.0); count]; count];
    let mut pair = 0;
    for a in 0..count {
        for b in a + 1..count {
            let z = Complex64::new(
                resistance[a] + resistance[b],
                transformer.xsc_pct[pair] / 100.0,
            );
            impedance[a][b] = z;
            impedance[b][a] = z;
            pair += 1;
        }
    }
    Ok(impedance)
}

/// [`TransformerLeakageBase::StatedOhms`]: a regulator's series impedance,
/// ohms on the first side.
fn stated_ohms_leakage(transformer: &DistTransformer) -> Result<Vec<Vec<Complex64>>> {
    if transformer.windings.len() != 2 {
        return Err(invalid_leakage(
            transformer,
            "a stated ohmic impedance needs exactly two windings",
        ));
    }
    let tap = transformer.windings[1].tap;
    if !tap.is_finite() || tap <= 0.0 {
        return Err(invalid_leakage(
            transformer,
            "the regulated winding tap is not finite and positive",
        ));
    }
    let n = 1.0 / tap;
    let [r_from, x_from, r_to, x_to] = calc_regulator_series_ohms(transformer)?;
    let z = Complex64::new(r_from + n.powi(2) * r_to, x_from + n.powi(2) * x_to);
    let zero = Complex64::new(0.0, 0.0);
    Ok(vec![vec![zero, z], vec![z, zero]])
}

/// A regulator's stated series impedance, ohms on each side:
/// `[r_series_from, x_series_from, r_series_to, x_series_to]`, each zero
/// when omitted. The one reading of these extras: the stated ohm leakage
/// ([`TransformerLeakageBase::StatedOhms`]) and the multiconductor OPF tap
/// rows read it.
///
/// # Errors
/// A stated value that is not a finite nonnegative number.
pub fn calc_regulator_series_ohms(transformer: &DistTransformer) -> Result<[f64; 4]> {
    let read = |key: &str| -> Result<f64> {
        let value = transformer_extra_number(&transformer.extras, key).unwrap_or(0.0);
        if !value.is_finite() || value < 0.0 {
            return Err(invalid_leakage(
                transformer,
                &format!("{key} must be finite and nonnegative"),
            ));
        }
        Ok(value)
    };
    Ok([
        read("r_series_from")?,
        read("x_series_from")?,
        read("r_series_to")?,
        read("x_series_to")?,
    ])
}

/// The pairwise leakage of [`calc_transformer_leakage_impedances`] reduced
/// against the `reference` winding to the winding difference impedance
/// matrix, over every other winding in winding order:
///
/// ```text
/// ZB[i][j] = z[i][r]                          (i = j)
///          = (z[i][r] + z[j][r] - z[i][j]) / 2  (i ≠ j)
/// ```
///
/// Row `i` is the voltage of winding `i` less the reference winding's. The
/// one reduction: [`calc_transformer_winding_admittance`] inverts it against
/// the last winding, and the multiconductor OPF descriptor keeps it in
/// impedance form against the first.
///
/// # Panics
/// A `reference` outside the matrix.
#[must_use]
pub fn calc_reduced_leakage(leakage: &[Vec<Complex64>], reference: usize) -> Vec<Vec<Complex64>> {
    assert!(reference < leakage.len(), "reference winding out of range");
    let others: Vec<usize> = (0..leakage.len()).filter(|&w| w != reference).collect();
    others
        .iter()
        .map(|&i| {
            others
                .iter()
                .map(|&j| {
                    if i == j {
                        leakage[i][reference]
                    } else {
                        (leakage[i][reference] + leakage[j][reference] - leakage[i][j]) / 2.0
                    }
                })
                .collect()
        })
        .collect()
}

/// The winding domain admittance `Y_1Volt` of a multiwinding transformer,
/// siemens at a one volt winding base (OpenDSS `CalcY_Terminal`): the
/// pairwise nameplate leakage impedances of
/// [`calc_transformer_leakage_impedances`] scaled by `phases / S_1`, reduced
/// against the last winding ([`calc_reduced_leakage`]), inverted, and lifted
/// back through the winding difference incidence `A` (row `i` is
/// `u_i - u_n`): `Y_1Volt = Aᵀ ZB⁻¹ A`. A two winding unit reduces to
/// `y · [[1, -1], [-1, 1]]`. `None` when the reduced matrix is exactly
/// singular or not finite: one volt base impedances of high rating units sit
/// far below any fixed pivot threshold, so only exact singularity is
/// refused.
///
/// # Errors
/// The leakage data [`calc_transformer_leakage_impedances`] refuses, or a
/// phase count of zero.
pub fn calc_transformer_winding_admittance(
    transformer: &DistTransformer,
) -> Result<Option<Vec<Vec<Complex64>>>> {
    let leakage =
        calc_transformer_leakage_impedances(transformer, &TransformerLeakageBase::Nameplate)?;
    if transformer.phases == 0 {
        return Err(invalid_leakage(transformer, "zero phases"));
    }
    let count = leakage.len();
    let reference = count - 1;
    let base = transformer.phases as f64 / transformer.windings[0].s_rating;
    let scaled: Vec<Vec<Complex64>> = leakage
        .iter()
        .map(|row| row.iter().map(|z| z * base).collect())
        .collect();
    let reduced = calc_reduced_leakage(&scaled, reference);
    let finite = |value: &Complex64| value.re.is_finite() && value.im.is_finite();
    if reduced.iter().flatten().any(|value| !finite(value)) {
        return Ok(None);
    }
    let Some(inverse) =
        crate::dense::invert_dense_matrix(&reduced, crate::dense::DensePivot::Partial, 0.0, 0.0)
    else {
        return Ok(None);
    };
    if inverse.iter().flatten().any(|value| !finite(value)) {
        return Ok(None);
    }
    let mut admittance = vec![vec![Complex64::new(0.0, 0.0); count]; count];
    for i in 0..reference {
        for j in 0..reference {
            let value = inverse[i][j];
            admittance[i][j] += value;
            admittance[i][reference] -= value;
            admittance[reference][j] -= value;
            admittance[reference][reference] += value;
        }
    }
    Ok(Some(admittance))
}

/// The ratio of a winding's rated voltage (`v_ref`) to its coil voltage:
/// `√3` for a WYE winding of a two or three phase unit (a line to line
/// rating over a line to neutral coil), one otherwise. The BMOPF reader
/// forms `v_ref` from a coil voltage `v_nom` with it, and
/// [`calc_coil_voltage`] divides by it.
#[must_use]
pub fn calc_coil_voltage_ratio(conn: DistWindingConn, phases: usize) -> f64 {
    if conn == DistWindingConn::Wye && matches!(phases, 2 | 3) {
        crate::electrical::SQRT_3
    } else {
        1.0
    }
}

/// A winding's coil voltage, volts: line to neutral (`v_ref / √3`) for a
/// polyphase WYE winding, the rated voltage otherwise
/// ([`calc_coil_voltage_ratio`] over the unit's phases). Times the winding's
/// tap, it is the coil's turns in volts, the quantity every turns ratio of
/// the unit is formed from, and the `v_nom` a BMOPF n-winding unit states.
#[must_use]
pub fn calc_coil_voltage(transformer: &DistTransformer, winding: usize) -> f64 {
    let w = &transformer.windings[winding];
    w.v_ref / calc_coil_voltage_ratio(w.conn, transformer.phases)
}

/// The number of phases a winding's per phase power base divides over: a
/// WYE winding's terminals less its neutral (mapped last, as the BMOPF and
/// OpenDSS readers state it), at least one; one for a two terminal DELTA
/// winding (a single coil); one per terminal of a DELTA ring.
#[must_use]
pub fn calc_winding_phase_count(conn: DistWindingConn, terminal_map: &[String]) -> usize {
    match conn {
        DistWindingConn::Wye => terminal_map.len().saturating_sub(1).max(1),
        DistWindingConn::Delta => {
            if terminal_map.len() == 2 {
                1
            } else {
                terminal_map.len().max(1)
            }
        }
    }
}

/// A winding's impedance base, ohms: its coil voltage
/// ([`calc_coil_voltage`]) squared over its per phase power
/// `power / phases` ([`calc_winding_phase_count`]),
/// [`powerio_core::electrical::calc_impedance_base`]. The base a BMOPF
/// n-winding unit states its winding resistances (on the winding's rating)
/// and pair reactances (on the unit's power base) on, and the multiconductor
/// OPF descriptor's reference base. `None` for an unusable voltage or power.
#[must_use]
pub fn calc_winding_impedance_base(
    transformer: &DistTransformer,
    winding: usize,
    power: f64,
) -> Option<f64> {
    let w = &transformer.windings[winding];
    let phases = calc_winding_phase_count(w.conn, &w.terminal_map);
    powerio_core::electrical::calc_impedance_base(
        calc_coil_voltage(transformer, winding),
        power / phases as f64,
    )
}

/// Whether the source states the European convention (`leadlag` `lead` or
/// `euro`, OpenDSS's `LeadLag`): the low voltage winding leads the high
/// voltage winding (Dy11). The default, `lag` or `ansi`, has the high
/// voltage winding leading (Dy1).
fn low_voltage_leads(transformer: &DistTransformer) -> Result<bool> {
    let Some(value) = transformer.extras.get("leadlag") else {
        return Ok(false);
    };
    match value.as_str().map(str::to_ascii_lowercase).as_deref() {
        Some("lead" | "euro") => Ok(true),
        Some("lag" | "ansi") => Ok(false),
        _ => Err(invalid(
            &transformer.name,
            "leadlag",
            "expected lead, lag, ansi or euro",
        )),
    }
}

/// The orientation of a DELTA winding's coil ring, `+1` (phase `k` to phase
/// `k + 1`) or `-1` (phase `k` to phase `k - 1`):
///
/// - a stated BMOPF roll (`bmopf_delta_rolls`, one based winding keys);
/// - otherwise `+1` for a BMOPF n-winding unit (BMOPFTools' schema
///   default), `+1` for `wye_delta` and `-1` for `delta_wye`;
/// - otherwise OpenDSS's rule over the first two windings: `+1` when they
///   share a connection, else `-1` exactly when the high voltage winding is
///   DELTA with the high voltage winding leading (the default) or WYE with
///   the low voltage winding leading (`leadlag` `lead` or `euro`).
///
/// The one delta orientation: the transformer primitive, the multiconductor
/// OPF descriptor, the connection phase shift
/// ([`calc_transformer_phase_shift`]) and the SINCAL writer read it.
///
/// # Errors
/// A stated roll that is not `+1` or `-1`, a `bmopf_delta_rolls` that is
/// not an object, or an unknown `leadlag` convention.
pub fn calc_delta_direction(transformer: &DistTransformer, winding: usize) -> Result<i8> {
    if let Some(rolls) = transformer.extras.get("bmopf_delta_rolls") {
        let rolls = rolls
            .as_object()
            .ok_or_else(|| invalid(&transformer.name, "bmopf_delta_rolls", "must be an object"))?;
        if let Some(roll) = rolls.get(&(winding + 1).to_string()) {
            return match roll.as_i64() {
                Some(1) => Ok(1),
                Some(-1) => Ok(-1),
                _ => Err(invalid(
                    &transformer.name,
                    "bmopf_delta_rolls",
                    "a delta roll must be +1 or -1",
                )),
            };
        }
    }
    match subtype(transformer) {
        Some("n_winding" | "wye_delta") => return Ok(1),
        Some("delta_wye") => return Ok(-1),
        _ => {}
    }
    let lv_leads = low_voltage_leads(transformer)?;
    let [a, b, ..] = transformer.windings.as_slice() else {
        return Ok(1);
    };
    if a.conn == b.conn {
        return Ok(1);
    }
    let hv = if a.v_ref >= b.v_ref { a } else { b };
    Ok(match (hv.conn, lv_leads) {
        (DistWindingConn::Delta, false) | (DistWindingConn::Wye, true) => -1,
        _ => 1,
    })
}

/// The positive sequence phase shift of a two winding unit, degrees: the
/// angle by which the first winding's voltage leads the second's
/// ([`calc_winding_phase_shift`] of winding 1).
///
/// # Errors
/// Fewer than two windings, or what [`calc_delta_direction`] refuses.
pub fn calc_transformer_phase_shift(transformer: &DistTransformer) -> Result<f64> {
    if transformer.windings.len() < 2 {
        return Err(unsupported(
            &transformer.name,
            "a phase shift needs two windings",
        ));
    }
    calc_winding_phase_shift(transformer, 1)
}

/// The positive sequence phase shift of winding `winding` against the first
/// winding, degrees: the angle by which the first winding's voltage leads
/// it. Zero when both share a connection; otherwise thirty degrees, signed
/// by the DELTA winding's orientation ([`calc_delta_direction`]): `-30 d`
/// when the first winding is the DELTA, `+30 d` when `winding` is. The one
/// vector group shift: [`calc_transformer_phase_shift`] and the CIM and
/// MG-RAVENS phase clocks read it.
///
/// # Errors
/// A winding outside the unit, or what [`calc_delta_direction`] refuses.
pub(crate) fn calc_winding_phase_shift(
    transformer: &DistTransformer,
    winding: usize,
) -> Result<f64> {
    let (Some(a), Some(b)) = (
        transformer.windings.first(),
        transformer.windings.get(winding),
    ) else {
        return Err(unsupported(
            &transformer.name,
            format!("winding {} is outside the unit", winding + 1),
        ));
    };
    if a.conn == b.conn {
        return Ok(0.0);
    }
    if a.conn == DistWindingConn::Delta {
        Ok(-30.0 * f64::from(calc_delta_direction(transformer, 0)?))
    } else {
        Ok(30.0 * f64::from(calc_delta_direction(transformer, winding)?))
    }
}

/// The number of terminals of a winding its coils span: two for a single
/// phase unit (one for an implicitly grounded coil), the phases and neutral
/// of a polyphase WYE winding, the phases of a DELTA winding.
fn terminal_count(w: &DistWinding, phases: usize) -> usize {
    if phases == 1 {
        2.min(w.terminal_map.len())
    } else if w.conn == DistWindingConn::Wye {
        (phases + 1).min(w.terminal_map.len())
    } else {
        phases
    }
}

/// The coils of one winding as channels over its terminal map, in phase
/// order ([`calc_element_connection`] with the unit's phase count): each
/// polyphase WYE coil from its phase terminal to the neutral (to ground when
/// the map states no neutral), each DELTA coil around the ring in the
/// orientation of [`calc_delta_direction`], and a single phase coil across
/// its two terminals (to ground for one).
///
/// # Errors
/// A winding outside the unit, a terminal map shorter than its coils need,
/// an undeclared bus or ambiguous neutral, or what [`calc_delta_direction`]
/// refuses.
pub fn calc_winding_connection(
    network: &MulticonductorNetwork,
    transformer: &DistTransformer,
    winding: usize,
) -> Result<Vec<ConnectionChannel>> {
    let name = transformer.name.as_str();
    let w = transformer
        .windings
        .get(winding)
        .ok_or_else(|| invalid(name, format!("windings[{winding}]"), "is not stated"))?;
    let phases = transformer.phases;
    let configuration = if phases == 1 {
        Configuration::SinglePhase
    } else if w.conn == DistWindingConn::Wye {
        Configuration::Wye
    } else {
        Configuration::Delta
    };
    let count = terminal_count(w, phases);
    let map = w.terminal_map.get(..count).ok_or_else(|| {
        invalid(
            name,
            format!("windings[{winding}].terminal_map"),
            format!("expected {count} terminals"),
        )
    })?;
    let mut channels =
        calc_element_connection(network, &w.bus, map, configuration, Some(phases))
            .map_err(|reason| invalid(name, format!("windings[{winding}].terminal_map"), reason))?;
    if configuration == Configuration::Delta {
        orient_delta_ring(&mut channels, calc_delta_direction(transformer, winding)?);
    }
    Ok(channels)
}

/// A transformer's magnetizing branch: the per coil admittance, siemens,
/// stamped across each coil of one winding.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct TransformerMagnetizing {
    /// The winding whose coils carry the branch, zero based.
    pub winding: usize,
    /// Conductance plus susceptance per coil, siemens.
    pub admittance: Complex64,
}

/// The magnetizing branch a transformer states, in one of three spellings:
///
/// - `no_load_shunt` `{winding, g, b}`: per coil siemens on the stated one
///   based winding;
/// - `g_no_load` and `b_no_load` (BMOPF): siemens on the second winding, or
///   the first winding of a regulator, divided across the phases of a
///   `single_phase`, `wye_delta` or `delta_wye` unit, which state a total;
/// - OpenDSS `%noloadloss` and `%imag`: percent of the first winding's
///   rating over the second winding's coil voltage
///   ([`calc_coil_voltage`] times tap), on the second winding, the
///   susceptance inductive.
///
/// `None` when none is stated. The one magnetizing branch: the transformer
/// primitive stamps it and the multiconductor OPF descriptor writes it as
/// coil shunts.
///
/// # Errors
/// `no_load_shunt` beside `g_no_load` or `b_no_load`, a `no_load_shunt`
/// that is not an object of a winding, `g` and `b` alone, a winding outside
/// the unit, a negative or nonfinite conductance, a nonfinite susceptance,
/// or OpenDSS percentages that are negative or nonfinite.
#[allow(clippy::too_many_lines)] // three spellings, each validated in full
pub fn calc_transformer_magnetizing(
    transformer: &DistTransformer,
) -> Result<Option<TransformerMagnetizing>> {
    let name = transformer.name.as_str();
    let windings = transformer.windings.len();
    let explicit = transformer.extras.get("no_load_shunt");
    let legacy = transformer_extra_number(&transformer.extras, "g_no_load")
        .or_else(|| transformer_extra_number(&transformer.extras, "b_no_load"))
        .is_some();
    if explicit.is_some() && legacy {
        return Err(invalid(
            name,
            "no_load_shunt",
            "cannot coexist with g_no_load or b_no_load",
        ));
    }
    let checked = |field: &str, g: f64, b: f64| -> Result<Complex64> {
        if !g.is_finite() || g < 0.0 || !b.is_finite() {
            return Err(invalid(name, field, "requires finite g >= 0 and finite b"));
        }
        Ok(Complex64::new(g, b))
    };
    if let Some(shunt) = explicit {
        let object = shunt.as_object().ok_or_else(|| {
            invalid(
                name,
                "no_load_shunt",
                "expected an object containing winding, g, and b",
            )
        })?;
        if let Some(key) = object
            .keys()
            .find(|key| !["winding", "g", "b"].contains(&key.as_str()))
        {
            return Err(invalid(
                name,
                "no_load_shunt",
                format!("unknown field `{key}`"),
            ));
        }
        let winding = object
            .get("winding")
            .and_then(serde_json::Value::as_u64)
            .and_then(|w| usize::try_from(w).ok())
            .filter(|w| *w >= 1 && *w <= windings)
            .ok_or_else(|| {
                invalid(
                    name,
                    "no_load_shunt.winding",
                    format!("must be a one based winding between 1 and {windings}"),
                )
            })?;
        let component = |key: &str| {
            object
                .get(key)
                .and_then(serde_json::Value::as_f64)
                .ok_or_else(|| invalid(name, format!("no_load_shunt.{key}"), "expected siemens"))
        };
        return Ok(Some(TransformerMagnetizing {
            winding: winding - 1,
            admittance: checked("no_load_shunt", component("g")?, component("b")?)?,
        }));
    }
    if legacy {
        let g = transformer_extra_number(&transformer.extras, "g_no_load").unwrap_or(0.0);
        let b = transformer_extra_number(&transformer.extras, "b_no_load").unwrap_or(0.0);
        let admittance = checked("g_no_load/b_no_load", g, b)?;
        let winding = usize::from(!is_transformer_regulator(transformer));
        if winding >= windings {
            return Err(invalid(
                name,
                "g_no_load/b_no_load",
                "has no winding to sit on",
            ));
        }
        let divisor = if matches!(
            subtype(transformer),
            Some("single_phase" | "wye_delta" | "delta_wye")
        ) {
            transformer.phases.max(1) as f64
        } else {
            1.0
        };
        return Ok(Some(TransformerMagnetizing {
            winding,
            admittance: admittance / divisor,
        }));
    }
    let loss = transformer_extra_number(&transformer.extras, "%noloadloss");
    let imag = transformer_extra_number(&transformer.extras, "%imag");
    if loss.is_none() && imag.is_none() {
        return Ok(None);
    }
    let loss = loss.unwrap_or(0.0);
    let imag = imag.unwrap_or(0.0);
    if !loss.is_finite() || loss < 0.0 || !imag.is_finite() || imag < 0.0 {
        return Err(invalid(
            name,
            "%noloadloss/%imag",
            "requires finite nonnegative percentages",
        ));
    }
    if windings < 2 || transformer.phases == 0 {
        return Err(invalid(
            name,
            "%noloadloss/%imag",
            "needs a second winding and a phase",
        ));
    }
    let v = calc_coil_voltage(transformer, 1) * transformer.windings[1].tap;
    let ybase = crate::electrical::calc_rated_admittance(
        transformer.windings[0].s_rating / transformer.phases as f64,
        v,
    );
    Ok(Some(TransformerMagnetizing {
        winding: 1,
        admittance: Complex64::new(loss / 100.0 * ybase, -imag / 100.0 * ybase),
    }))
}

/// How a winding's neutral is grounded, as the source states it.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub enum WindingNeutralGrounding {
    /// No neutral impedance is stated: the neutral terminal connects as its
    /// bus declares it.
    Unstated,
    /// A negative `r_neutral`, OpenDSS's floating neutral: no grounding
    /// branch.
    Floating,
    /// `r_neutral` and `x_neutral` both zero (one may be omitted): the
    /// neutral is solidly grounded, an exact zero voltage.
    Solid,
    /// A finite neutral to ground admittance, siemens:
    /// `1 / (r_neutral + j x_neutral)`.
    Impedance(Complex64),
}

/// The neutral grounding a winding's `r_neutral` and `x_neutral` state,
/// OpenDSS's reading: unstated when both are omitted, floating for a
/// negative resistance, solid when both are zero (an omitted one reads
/// zero), else the admittance of the stated impedance (`NaN` parts for a
/// nonfinite one). The one neutral grounding rule:
/// [`calc_winding_neutral_grounding`] checks it, and the MG-RAVENS and BMOPF
/// writers and the multiconductor ideal transformer profile read it as
/// stated.
#[must_use]
pub fn calc_stated_neutral_grounding(winding: &DistWinding) -> WindingNeutralGrounding {
    if winding.r_neutral.is_none() && winding.x_neutral.is_none() {
        return WindingNeutralGrounding::Unstated;
    }
    let r = winding.r_neutral.unwrap_or(0.0);
    let x = winding.x_neutral.unwrap_or(0.0);
    if r < 0.0 {
        WindingNeutralGrounding::Floating
    } else if r == 0.0 && x == 0.0 {
        WindingNeutralGrounding::Solid
    } else {
        WindingNeutralGrounding::Impedance(Complex64::new(1.0, 0.0) / Complex64::new(r, x))
    }
}

/// A winding's neutral grounding, [`calc_stated_neutral_grounding`] checked
/// for the transformer primitive and the multiconductor OPF descriptor.
///
/// # Errors
/// A nonfinite neutral impedance, or a DELTA winding that states a solid or
/// finite neutral grounding (it has no neutral terminal).
pub fn calc_winding_neutral_grounding(
    transformer: &DistTransformer,
    winding: usize,
) -> Result<WindingNeutralGrounding> {
    let name = transformer.name.as_str();
    let w = &transformer.windings[winding];
    let stated = [w.r_neutral, w.x_neutral];
    if stated.iter().any(Option::is_some) && !stated.iter().flatten().all(|v| v.is_finite()) {
        return Err(invalid(
            name,
            format!("windings[{winding}].neutral impedance"),
            "expected finite resistance and reactance",
        ));
    }
    let grounding = calc_stated_neutral_grounding(w);
    if w.conn == DistWindingConn::Delta
        && !matches!(
            grounding,
            WindingNeutralGrounding::Floating | WindingNeutralGrounding::Unstated
        )
    {
        return Err(unsupported(
            name,
            "neutral impedance on a delta winding has no physical neutral terminal",
        ));
    }
    Ok(grounding)
}

/// The primitive of an admittance `y` across one coil: `y · [[1, -1], [-1,
/// 1]]` over its two terminals, or `[[y]]` when the coil returns through
/// ground. The one coil shunt stamp: the transformer primitive's magnetizing
/// branch and the multiconductor OPF's core and neutral shunts read it.
#[must_use]
pub fn calc_coil_shunt_primitive(
    y: Complex64,
    returns_through_ground: bool,
) -> Vec<Vec<Complex64>> {
    if returns_through_ground {
        vec![vec![y]]
    } else {
        vec![vec![y, -y], vec![-y, y]]
    }
}

/// A finite leakage transformer's primitive admittance, siemens, over its
/// physical terminals.
///
/// A terminal shared by several coils (a common WYE neutral, a centre tap)
/// appears once in `terminals`, in the order the windings first name it.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct TransformerPrimitive<'a> {
    pub name: &'a str,
    /// `(bus, terminal)` of each primitive row, in primitive order.
    pub terminals: Vec<(&'a str, &'a str)>,
    /// Leakage and winding neutral impedance contributions.
    pub series: Vec<Vec<Complex64>>,
    /// Magnetizing and core loss contributions.
    pub shunt: Vec<Vec<Complex64>>,
    /// `series + shunt`, the element's primitive admittance.
    pub primitive: Vec<Vec<Complex64>>,
    /// WYE neutral terminals the source grounds solidly
    /// ([`WindingNeutralGrounding::Solid`]): exact zero voltage constraints,
    /// not an admittance.
    pub grounded: Vec<(&'a str, &'a str)>,
    /// The coil ring orientation of the first DELTA winding
    /// ([`calc_delta_direction`]), `+1` when the unit has none.
    pub delta_direction: i8,
    /// The winding domain admittance `Y_1Volt` behind `series`
    /// ([`calc_transformer_winding_admittance`]), or the referred winding
    /// primitive of a single phase autotransformer.
    pub winding_admittance: Vec<Vec<Complex64>>,
}

/// Whether [`calc_transformer_primitive_admittance`] refuses this
/// transformer for stating no leakage at all: no short circuit reactance and
/// no winding resistance, or, for a single phase autotransformer, no stated
/// series impedance. Such a unit is an ideal coupling, which needs exact
/// voltage constraints rather than an admittance.
#[must_use]
pub fn transformer_states_no_leakage(transformer: &DistTransformer) -> bool {
    if is_autotransformer(transformer) {
        return [
            "r_series_from",
            "x_series_from",
            "r_series_to",
            "x_series_to",
        ]
        .iter()
        .all(|key| {
            transformer_extra_number(&transformer.extras, key).is_none_or(|value| value == 0.0)
        });
    }
    transformer.xsc_pct.iter().all(|value| *value == 0.0)
        && transformer.windings.iter().all(|w| w.r_pct == 0.0)
}

fn terminal_index(terminals: &[(&str, &str)], bus: &str, terminal: &str) -> Option<usize> {
    terminals
        .iter()
        .position(|(b, t)| b.eq_ignore_ascii_case(bus) && *t == terminal)
}

/// One winding's coils as positions in the primitive's terminal list:
/// each coil's first terminal and its return (`None` through ground).
fn coil_positions(
    network: &MulticonductorNetwork,
    transformer: &DistTransformer,
    winding: usize,
    terminals: &[(&str, &str)],
) -> Result<Vec<(Option<usize>, Option<usize>)>> {
    let w = &transformer.windings[winding];
    let position = |terminal: usize| terminal_index(terminals, &w.bus, &w.terminal_map[terminal]);
    Ok(calc_winding_connection(network, transformer, winding)?
        .iter()
        .map(|channel| (position(channel.from), channel.to.and_then(position)))
        .collect())
}

fn add_coil_shunt(
    y: &mut [Vec<Complex64>],
    (from, to): (Option<usize>, Option<usize>),
    value: Complex64,
) {
    let stamp = calc_coil_shunt_primitive(value, to.is_none());
    let rows = [from, to];
    for (i, row) in stamp.iter().enumerate() {
        for (j, entry) in row.iter().enumerate() {
            if let (Some(a), Some(b)) = (rows[i], rows[j]) {
                y[a][b] += entry;
            }
        }
    }
}

fn checked_tap(w: &DistWinding, name: &str) -> Result<f64> {
    if !w.tap.is_finite() || w.tap <= 0.0 {
        return Err(invalid(name, "winding.tap", "must be positive and finite"));
    }
    Ok(w.tap)
}

fn sum_matrices(a: &[Vec<Complex64>], b: &[Vec<Complex64>]) -> Vec<Vec<Complex64>> {
    let mut sum = a.to_vec();
    for (row, other) in sum.iter_mut().zip(b) {
        for (value, add) in row.iter_mut().zip(other) {
            *value += add;
        }
    }
    sum
}

/// The primitive admittance of a finite leakage transformer, siemens, over
/// its physical terminals, following the OpenDSS `CalcY_Terminal` path: the
/// winding domain `Y_1Volt` ([`calc_transformer_winding_admittance`], the
/// inverse of the reduced pairwise leakage of
/// [`calc_transformer_leakage_impedances`]) is referred to each phase's
/// coils through the turns and base factors,
///
/// ```text
/// Y_series = Σ_phase Bᵀ Y_1Volt B,   B[w][a] = ±1 / (V_coil,w · tap_w)
/// ```
///
/// with `B` the incidence of winding `w`'s coil on terminal `a`
/// ([`calc_winding_connection`]) and `V_coil` its coil voltage
/// ([`calc_coil_voltage`]). A finite WYE neutral impedance stamps once from
/// the neutral terminal to ground, a solid one is returned in `grounded`,
/// and a floating one stamps nothing ([`calc_winding_neutral_grounding`]).
/// The magnetizing branch ([`calc_transformer_magnetizing`]) stamps across
/// each coil of its winding.
///
/// Two winding units of one to three phases and the three winding BMOPF
/// `center_tap` unit are supported. The BMOPF `single_phase_autotransformer`
/// states its impedance in ohms
/// ([`TransformerLeakageBase::StatedOhms`]); its primitive is
/// `y · w wᵀ`, with `w` the from winding incidence minus `n = 1 / tap` times
/// the to winding incidence, over the order from phase, to phase, from
/// reference, to reference. The one transformer primitive: the
/// multiconductor nodal admittance stamps it and the SINCAL writer checks its
/// candidate against it.
///
/// # Errors
/// A transformer stating no leakage (an ideal coupling, which needs voltage
/// constraints), another winding count or phase count, a raw OpenDSS
/// property the typed model does not carry (`xrconst`, `core`, a
/// nondefault `ppm_antifloat`), nameplate, tap, terminal map, connection,
/// neutral impedance or magnetizing data that is not usable, or a singular
/// leakage impedance.
#[allow(clippy::float_cmp)] // 0 and 1 are OpenDSS's exact anti-float defaults
#[allow(clippy::too_many_lines)] // the coil referral, then the neutral and magnetizing stamps
pub fn calc_transformer_primitive_admittance<'a>(
    network: &MulticonductorNetwork,
    transformer: &'a DistTransformer,
) -> Result<TransformerPrimitive<'a>> {
    // These raw DSS settings are not represented by the typed winding model.
    for key in ["xrconst", "core"] {
        if transformer.extras.contains_key(key) {
            return Err(unsupported(
                &transformer.name,
                format!("raw OpenDSS property `{key}` requires normalization"),
            ));
        }
    }
    if transformer_extra_number(&transformer.extras, "ppm_antifloat")
        .is_some_and(|value| value != 0.0 && value != 1.0)
    {
        return Err(unsupported(
            &transformer.name,
            "nondefault OpenDSS anti-float admittance is not supported",
        ));
    }
    if is_autotransformer(transformer) {
        return autotransformer_primitive(transformer);
    }
    validate_primitive_shape(transformer)?;
    let phases = transformer.phases;
    let name = transformer.name.as_str();
    let windings = transformer.windings.len();

    let turns: Vec<f64> = (0..windings)
        .map(|index| {
            Ok(calc_coil_voltage(transformer, index)
                * checked_tap(&transformer.windings[index], name)?)
        })
        .collect::<Result<_>>()?;
    let y_1volt = calc_transformer_winding_admittance(transformer)
        .map_err(|error| unsupported(name, error))?
        .ok_or_else(|| invalid(name, "leakage impedance", "is singular"))?;

    let mut terminals: Vec<(&str, &str)> = Vec::new();
    // The unique terminal list, once. Coils from all phases then stamp
    // through the same physical neutral column when a neutral is explicit.
    for winding in &transformer.windings {
        for terminal in winding
            .terminal_map
            .iter()
            .take(terminal_count(winding, phases))
        {
            if terminal_index(&terminals, &winding.bus, terminal).is_none() {
                terminals.push((winding.bus.as_str(), terminal.as_str()));
            }
        }
    }
    let nterm = terminals.len();
    if nterm == 0 {
        return Err(invalid(name, "terminal_map", "no physical terminals"));
    }
    let coils: Vec<Vec<(Option<usize>, Option<usize>)>> = (0..windings)
        .map(|index| coil_positions(network, transformer, index, &terminals))
        .collect::<Result<_>>()?;
    if coils.iter().any(|winding| winding.len() != phases) {
        return Err(invalid(
            name,
            "terminal_map",
            "every winding needs one coil per phase",
        ));
    }
    let mut y_series = vec![vec![Complex64::new(0.0, 0.0); nterm]; nterm];
    let mut y_shunt = vec![vec![Complex64::new(0.0, 0.0); nterm]; nterm];

    // The physical terminal matrix is the sum over independent phase copies
    // of Bᵀ Y_1Volt B, where B contains the turns and base factors.
    for phase in 0..phases {
        let mut b = vec![vec![Complex64::new(0.0, 0.0); nterm]; windings];
        for (index, winding_coils) in coils.iter().enumerate() {
            let (a, z) = winding_coils[phase];
            let k = Complex64::new(1.0 / turns[index], 0.0);
            if let Some(i) = a {
                b[index][i] += k;
            }
            if let Some(i) = z {
                b[index][i] -= k;
            }
        }
        for i in 0..nterm {
            for j in 0..nterm {
                let mut value = Complex64::new(0.0, 0.0);
                for p in 0..windings {
                    for q in 0..windings {
                        value += b[p][i] * y_1volt[p][q] * b[q][j];
                    }
                }
                y_series[i][j] += value;
            }
        }
    }

    // A neutral grounding is one branch from the physical WYE neutral
    // conductor to ground, shared by all phase coils.
    let mut grounded = Vec::new();
    for (index, winding) in transformer.windings.iter().enumerate() {
        let grounding = calc_winding_neutral_grounding(transformer, index)?;
        if matches!(
            grounding,
            WindingNeutralGrounding::Unstated | WindingNeutralGrounding::Floating
        ) {
            continue;
        }
        let neutral = calc_winding_connection(network, transformer, index)?
            .first()
            .and_then(|coil| coil.to)
            .ok_or_else(|| {
                unsupported(
                    name,
                    "neutral impedance is declared but the WYE neutral is implicit ground",
                )
            })?;
        match grounding {
            WindingNeutralGrounding::Solid => {
                grounded.push((winding.bus.as_str(), winding.terminal_map[neutral].as_str()));
            }
            WindingNeutralGrounding::Impedance(y) => {
                if let Some(i) =
                    terminal_index(&terminals, &winding.bus, &winding.terminal_map[neutral])
                {
                    y_series[i][i] += y;
                }
            }
            _ => {}
        }
    }

    if let Some(core) = calc_transformer_magnetizing(transformer)? {
        for &coil in &coils[core.winding] {
            add_coil_shunt(&mut y_shunt, coil, core.admittance);
        }
    }
    let y_prim = sum_matrices(&y_series, &y_shunt);
    let delta_direction = match transformer
        .windings
        .iter()
        .position(|w| w.conn == DistWindingConn::Delta)
    {
        Some(index) => calc_delta_direction(transformer, index)?,
        None => 1,
    };
    Ok(TransformerPrimitive {
        name,
        terminals,
        series: y_series,
        shunt: y_shunt,
        primitive: y_prim,
        grounded,
        delta_direction,
        winding_admittance: y_1volt,
    })
}

/// The fixed ratio BMOPF single phase autotransformer or regulator: its
/// impedance in referred ohms and its ratio the regulated over source
/// voltage, for both ANSI types (`regulator_type` records the hardware
/// only), as the BMOPF schema reads it. For Type B this is the BMOPFTools
/// calculation and the equivalent fixed tap OpenDSS two winding regulator.
#[allow(clippy::too_many_lines)] // the BMOPFTools primitive order, then its stamps
fn autotransformer_primitive(transformer: &DistTransformer) -> Result<TransformerPrimitive<'_>> {
    let name = transformer.name.as_str();
    if transformer.phases != 1 || transformer.windings.len() != 2 {
        return Err(unsupported(
            name,
            format!(
                "single_phase_autotransformer requires one phase and two windings; got {} phases and {} windings",
                transformer.phases,
                transformer.windings.len()
            ),
        ));
    }
    let from = &transformer.windings[0];
    let to = &transformer.windings[1];
    for (idx, winding) in [from, to].iter().enumerate() {
        if winding.conn != DistWindingConn::Wye || winding.terminal_map.len() != 2 {
            return Err(invalid(
                name,
                format!("windings[{idx}].terminal_map"),
                "autotransformer winding requires phase and reference terminals",
            ));
        }
    }
    let tap_ratio = checked_tap(to, name)?;
    let regulator_type = transformer
        .extras
        .get("regulator_type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("B")
        .trim()
        .to_ascii_uppercase();
    if !matches!(regulator_type.as_str(), "A" | "B") {
        return Err(invalid(
            name,
            "regulator_type",
            format!("expected A or B, got `{regulator_type}`"),
        ));
    }
    let n_eff = 1.0 / tap_ratio;
    let z = calc_transformer_leakage_impedances(transformer, &TransformerLeakageBase::StatedOhms)?
        [0][1];
    if z.norm() == 0.0 {
        return Err(unsupported(
            name,
            "zero series impedance requires an ideal regulator voltage constraint",
        ));
    }
    let y = z.finv();
    let y_1volt = vec![vec![y, -n_eff * y], vec![-n_eff * y, n_eff.powi(2) * y]];

    // The BMOPFTools primitive order: from phase, to phase, from reference,
    // to reference. Repeated physical terminals are coalesced.
    let mut terminals: Vec<(&str, &str)> = Vec::new();
    for (winding, terminal) in [
        (from, &from.terminal_map[0]),
        (to, &to.terminal_map[0]),
        (from, &from.terminal_map[1]),
        (to, &to.terminal_map[1]),
    ] {
        if terminal_index(&terminals, &winding.bus, terminal).is_none() {
            terminals.push((winding.bus.as_str(), terminal.as_str()));
        }
    }
    let nterm = terminals.len();
    let position = |winding: &DistWinding, terminal: usize| {
        terminal_index(&terminals, &winding.bus, &winding.terminal_map[terminal])
            .expect("every winding terminal is listed")
    };
    let mut from_incidence = vec![0.0; nterm];
    let mut to_incidence = vec![0.0; nterm];
    for (incidence, winding) in [(&mut from_incidence, from), (&mut to_incidence, to)] {
        incidence[position(winding, 0)] += 1.0;
        incidence[position(winding, 1)] -= 1.0;
    }
    let winding_difference: Vec<f64> = from_incidence
        .iter()
        .zip(&to_incidence)
        .map(|(from, to)| from - n_eff * to)
        .collect();
    let mut y_series = vec![vec![Complex64::default(); nterm]; nterm];
    for i in 0..nterm {
        for j in 0..nterm {
            y_series[i][j] = y * winding_difference[i] * winding_difference[j];
        }
    }
    let mut y_shunt = vec![vec![Complex64::default(); nterm]; nterm];
    if let Some(core) = calc_transformer_magnetizing(transformer)? {
        let winding = &transformer.windings[core.winding];
        add_coil_shunt(
            &mut y_shunt,
            (Some(position(winding, 0)), Some(position(winding, 1))),
            core.admittance,
        );
    }
    let y_prim = sum_matrices(&y_series, &y_shunt);
    Ok(TransformerPrimitive {
        name,
        terminals,
        series: y_series,
        shunt: y_shunt,
        primitive: y_prim,
        grounded: Vec::new(),
        delta_direction: 1,
        winding_admittance: y_1volt,
    })
}

#[allow(clippy::too_many_lines)] // one check per nameplate field, each with its own message
fn validate_primitive_shape(t: &DistTransformer) -> Result<()> {
    let name = t.name.as_str();
    let center_tap = subtype(t) == Some("center_tap");
    let expected_windings = if center_tap { 3 } else { 2 };
    if t.windings.len() != expected_windings {
        return Err(unsupported(
            name,
            format!(
                "{} windings are present; expected {expected_windings} for {}",
                t.windings.len(),
                if center_tap {
                    "a BMOPF centre-tap primitive"
                } else {
                    "a finite two-winding primitive"
                }
            ),
        ));
    }
    if center_tap && t.phases != 1 {
        return Err(unsupported(
            name,
            format!(
                "BMOPF centre-tap primitives require one phase; got {}",
                t.phases
            ),
        ));
    }
    if t.phases == 0 || t.phases > 3 {
        return Err(unsupported(
            name,
            format!(
                "{} phases are outside the supported 1, 2, and 3 phase shapes",
                t.phases
            ),
        ));
    }
    let expected_xsc = if center_tap { 3 } else { 1 };
    if t.xsc_pct.len() != expected_xsc {
        return Err(invalid(
            name,
            "xsc_pct",
            format!(
                "{} requires exactly {expected_xsc} pairwise short-circuit values",
                if center_tap {
                    "a centre-tap transformer"
                } else {
                    "a two-winding transformer"
                }
            ),
        ));
    }
    if t.xsc_pct
        .iter()
        .any(|value| !value.is_finite() || *value < 0.0)
    {
        return Err(invalid(name, "xsc_pct", "must be finite and nonnegative"));
    }
    for (idx, w) in t.windings.iter().enumerate() {
        if !w.v_ref.is_finite() || w.v_ref <= 0.0 {
            return Err(invalid(
                name,
                format!("windings[{idx}].v_ref"),
                "must be positive and finite",
            ));
        }
        if !w.s_rating.is_finite() || w.s_rating <= 0.0 {
            return Err(invalid(
                name,
                format!("windings[{idx}].s_rating"),
                "must be positive and finite",
            ));
        }
        if !w.r_pct.is_finite() || w.r_pct < 0.0 {
            return Err(invalid(
                name,
                format!("windings[{idx}].r_pct"),
                "must be finite and nonnegative",
            ));
        }
        checked_tap(w, name)?;
        let expected = terminal_count(w, t.phases);
        if t.phases == 1 && w.conn == DistWindingConn::Delta && w.terminal_map.len() < 2 {
            return Err(invalid(
                name,
                format!("windings[{idx}].terminal_map"),
                "a one-phase DELTA winding requires two coil endpoints",
            ));
        }
        let stated = w.terminal_map.len();
        let wye_expected = if t.phases == 1 { 2 } else { t.phases + 1 };
        let accepted = if w.conn == DistWindingConn::Wye {
            stated == wye_expected || stated == t.phases
        } else {
            stated == expected
        };
        if !accepted {
            return Err(invalid(
                name,
                format!("windings[{idx}].terminal_map"),
                format!(
                    "expected {} terminals (or {} phase terminals with an implicit WYE ground), got {stated}",
                    if w.conn == DistWindingConn::Wye {
                        wye_expected
                    } else {
                        expected
                    },
                    t.phases,
                ),
            ));
        }
    }
    if transformer_states_no_leakage(t) {
        return Err(unsupported(
            name,
            "zero leakage requires ideal voltage constraints and cannot be represented by an admittance primitive",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(conn: [DistWindingConn; 2], v: [f64; 2]) -> DistTransformer {
        let windings = conn
            .iter()
            .zip(v)
            .map(|(&conn, v_ref)| DistWinding::new("b", vec![], conn, v_ref, 1.0))
            .collect();
        DistTransformer::new("t", windings, vec![1.0], 3)
    }

    #[test]
    fn opendss_dy_orientation_and_shift_follow_leadlag() {
        let mut dy = unit(
            [DistWindingConn::Delta, DistWindingConn::Wye],
            [12.47e3, 0.48e3],
        );
        assert_eq!(calc_delta_direction(&dy, 0).unwrap(), -1);
        assert_eq!(calc_transformer_phase_shift(&dy).unwrap(), 30.0);
        dy.extras
            .insert("leadlag".into(), serde_json::json!("Euro"));
        assert_eq!(calc_delta_direction(&dy, 0).unwrap(), 1);
        assert_eq!(calc_transformer_phase_shift(&dy).unwrap(), -30.0);
        let yd = unit(
            [DistWindingConn::Wye, DistWindingConn::Delta],
            [12.47e3, 0.48e3],
        );
        assert_eq!(calc_delta_direction(&yd, 1).unwrap(), 1);
        assert_eq!(calc_transformer_phase_shift(&yd).unwrap(), 30.0);
        let mut bmopf = unit(
            [DistWindingConn::Delta, DistWindingConn::Wye],
            [12.47e3, 0.48e3],
        );
        bmopf
            .extras
            .insert("bmopf_subtype".into(), serde_json::json!("delta_wye"));
        assert_eq!(calc_delta_direction(&bmopf, 0).unwrap(), -1);
        bmopf
            .extras
            .insert("bmopf_delta_rolls".into(), serde_json::json!({"1": 2}));
        assert!(calc_delta_direction(&bmopf, 0).is_err());
    }

    #[test]
    fn reduced_leakage_against_either_reference() {
        let z = |re: f64| Complex64::new(re, 0.0);
        let leakage = vec![
            vec![z(0.0), z(1.0), z(2.0)],
            vec![z(1.0), z(0.0), z(4.0)],
            vec![z(2.0), z(4.0), z(0.0)],
        ];
        assert_eq!(
            calc_reduced_leakage(&leakage, 0),
            vec![vec![z(1.0), z(-0.5)], vec![z(-0.5), z(2.0)]]
        );
        assert_eq!(
            calc_reduced_leakage(&leakage, 2),
            vec![vec![z(2.0), z(2.5)], vec![z(2.5), z(4.0)]]
        );
    }

    #[test]
    fn neutral_grounding_reads_opendss_conventions() {
        let mut t = unit([DistWindingConn::Wye, DistWindingConn::Delta], [1.0, 1.0]);
        assert_eq!(
            calc_winding_neutral_grounding(&t, 0).unwrap(),
            WindingNeutralGrounding::Unstated
        );
        t.windings[0].r_neutral = Some(0.0);
        assert_eq!(
            calc_winding_neutral_grounding(&t, 0).unwrap(),
            WindingNeutralGrounding::Solid
        );
        t.windings[0].r_neutral = Some(-1.0);
        assert_eq!(
            calc_winding_neutral_grounding(&t, 0).unwrap(),
            WindingNeutralGrounding::Floating
        );
        t.windings[1].r_neutral = Some(2.0);
        assert!(calc_winding_neutral_grounding(&t, 1).is_err());
        t.windings[1].r_neutral = Some(-1.0);
        assert_eq!(
            calc_winding_neutral_grounding(&t, 1).unwrap(),
            WindingNeutralGrounding::Floating
        );
    }

    #[test]
    fn legacy_core_sits_on_the_bmopf_winding() {
        let mut t = unit([DistWindingConn::Wye, DistWindingConn::Delta], [1.0, 1.0]);
        t.extras
            .insert("bmopf_subtype".into(), serde_json::json!("wye_delta"));
        t.extras.insert("g_no_load".into(), serde_json::json!(3.0));
        let core = calc_transformer_magnetizing(&t).unwrap().unwrap();
        assert_eq!(core.winding, 1);
        assert_eq!(core.admittance, Complex64::new(1.0, 0.0));
        t.extras.insert(
            "no_load_shunt".into(),
            serde_json::json!({"winding": 1, "g": 1.0, "b": 0.0}),
        );
        assert!(calc_transformer_magnetizing(&t).is_err());
    }
}
