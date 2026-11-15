//! Transformers in the multiconductor nodal system: exact ideal WYE
//! constraints over explicit winding terminals for a unit that states no
//! leakage, and the finite leakage primitive admittance (with the leakage
//! derivation it inverts) for every other unit.

use std::collections::BTreeMap;

use num_complex::Complex64;
use powerio_dist::{DistTransformer, DistWinding, DistWindingConn, MulticonductorNetwork};

use super::{MulticonductorNodeIndex, NodeRef};
use crate::{Error, Result, diagnostics::codes};

pub(super) struct IdealConstraint {
    pub label: String,
    pub coefficients: BTreeMap<usize, f64>,
}

fn unsupported(transformer: &DistTransformer, reason: &str) -> Error {
    powerio_core::Error::new(
        &codes::BUILD_MULTI_PHYSICS_UNSUPPORTED,
        format!(
            "transformer `{}` is outside the ideal WYE admittance profile: {reason}",
            transformer.name
        ),
    )
    .into()
}

fn validate(transformer: &DistTransformer) -> Result<()> {
    let supported = transformer.windings.len() == 2
        && (1..=3).contains(&transformer.phases)
        && transformer.xsc_pct.len() == 1
        && transformer.xsc_pct[0] == 0.0
        && transformer.windings.iter().all(|w| {
            w.conn == DistWindingConn::Wye
                && w.r_pct == 0.0
                && w.r_neutral.is_none_or(|r| r == 0.0)
                && w.x_neutral.is_none_or(|x| x == 0.0)
                && ((transformer.phases == 1 && w.terminal_map.len() == 1)
                    || w.terminal_map.len() == transformer.phases + 1)
        })
        && transformer.extras.get("no_load_shunt").is_none_or(|shunt| {
            ["g", "b"]
                .iter()
                .all(|key| shunt.get(key).and_then(serde_json::Value::as_f64) == Some(0.0))
        })
        && ["g_no_load", "b_no_load", "%noloadloss", "%imag"]
            .iter()
            .all(|key| {
                transformer
                    .extras
                    .get(*key)
                    .is_none_or(|value| value.as_f64() == Some(0.0))
            })
        && !["tap_min", "tap_max", "tap_ratio_min", "tap_ratio_max"]
            .iter()
            .any(|key| transformer.extras.contains_key(*key));
    if supported {
        Ok(())
    } else {
        Err(unsupported(
            transformer,
            "requires two complete ideal WYE windings without leakage, implicit neutral impedance, core loss or tap control",
        ))
    }
}

fn winding_nodes(
    winding: &DistWinding,
    transformer: &DistTransformer,
    index: &MulticonductorNodeIndex,
) -> Result<(Vec<NodeRef>, NodeRef)> {
    let mut nodes = winding
        .terminal_map
        .iter()
        .map(|terminal| {
            index.resolve(&winding.bus, terminal).ok_or_else(|| {
                Error::Mtx(format!(
                    "transformer `{}` names terminal `{terminal}` bus `{}` does not declare",
                    transformer.name, winding.bus
                ))
            })
        })
        .collect::<Result<Vec<_>>>()?;
    // A one-terminal single-phase winding is the existing implicit-ground
    // spelling. Otherwise the last terminal is its stated neutral, which
    // must not be grounded merely because it is a winding's last terminal.
    let neutral = if nodes.len() == 1 {
        NodeRef::Ground
    } else {
        nodes.pop().expect("validated nonempty winding")
    };
    if neutral != NodeRef::Ground && (winding.r_neutral.is_some() || winding.x_neutral.is_some()) {
        return Err(unsupported(
            transformer,
            "an explicit winding-neutral grounding parameter needs its own circuit",
        ));
    }
    Ok((nodes, neutral))
}

pub(super) fn ideal_wye_constraints(
    network: &MulticonductorNetwork,
    index: &MulticonductorNodeIndex,
) -> Result<Vec<IdealConstraint>> {
    let mut result = Vec::new();
    // A unit stating leakage stamps its primitive instead.
    for transformer in network
        .transformers()
        .iter()
        .filter(|transformer| states_no_leakage(transformer))
    {
        validate(transformer)?;
        let primary = &transformer.windings[0];
        let secondary = &transformer.windings[1];
        if [primary.v_ref, secondary.v_ref, primary.tap, secondary.tap]
            .iter()
            .any(|v| !v.is_finite() || *v <= 0.0)
        {
            return Err(unsupported(
                transformer,
                "invalid rated voltage or fixed tap",
            ));
        }
        // Both windings have the same phase count and connection, so their
        // line-line to coil-voltage factors cancel. Fixed taps are included.
        let ratio = (primary.v_ref / secondary.v_ref) * (primary.tap / secondary.tap);
        if !ratio.is_finite() || ratio <= 0.0 {
            return Err(unsupported(
                transformer,
                "winding ratio overflows or underflows",
            ));
        }
        let (primary_nodes, primary_neutral) = winding_nodes(primary, transformer, index)?;
        let (secondary_nodes, secondary_neutral) = winding_nodes(secondary, transformer, index)?;
        for (phase, (&p, &s)) in primary_nodes.iter().zip(&secondary_nodes).enumerate() {
            let mut coefficients = BTreeMap::new();
            // (Vp - Vpn) - ratio * (Vs - Vsn) = 0. Aggregate after switch
            // merging; a shorted or redundant winding must not create an
            // all-zero constraint and a spurious free ideal-current unknown.
            for (node, coefficient) in [
                (p, 1.0),
                (primary_neutral, -1.0),
                (s, -ratio),
                (secondary_neutral, ratio),
            ] {
                if let NodeRef::Node(column) = node {
                    *coefficients.entry(column).or_insert(0.0) += coefficient;
                }
            }
            coefficients.retain(|_, value| *value != 0.0);
            if !coefficients.is_empty() {
                result.push(IdealConstraint {
                    label: format!("transformer:{}:{phase}", transformer.name),
                    coefficients,
                });
            }
        }
    }
    Ok(result)
}

fn invalid_leakage(transformer: &DistTransformer, reason: &str) -> Error {
    Error::Mtx(format!(
        "transformer `{}` states no usable leakage data: {reason}",
        transformer.name
    ))
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
/// [`calc_transformer_winding_admittance`] inverts it, and
/// [`calc_transformer_primitive_admittance`] stamps it.
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
    let impedance = |key: &str| -> Result<f64> {
        let value = extra_number(&transformer.extras, key).unwrap_or(0.0);
        if !value.is_finite() || value < 0.0 {
            return Err(invalid_leakage(
                transformer,
                &format!("{key} must be finite and nonnegative"),
            ));
        }
        Ok(value)
    };
    let n = 1.0 / tap;
    let z = Complex64::new(
        impedance("r_series_from")? + n.powi(2) * impedance("r_series_to")?,
        impedance("x_series_from")? + n.powi(2) * impedance("x_series_to")?,
    );
    let zero = Complex64::new(0.0, 0.0);
    Ok(vec![vec![zero, z], vec![z, zero]])
}

/// A transformer extra as a number: typed producers write numbers and the
/// DSS reader keeps written extras as text. A present value that is neither
/// reads as NaN, so it reaches the finite value checks rather than becoming
/// an omitted zero.
fn extra_number(extras: &BTreeMap<String, serde_json::Value>, key: &str) -> Option<f64> {
    extras.get(key).map(|value| {
        value
            .as_f64()
            .or_else(|| value.as_str()?.trim().parse().ok())
            .unwrap_or(f64::NAN)
    })
}

/// The winding domain admittance `Y_1Volt` of a multiwinding transformer,
/// siemens at a one volt winding base (OpenDSS `CalcY_Terminal`): the
/// pairwise nameplate leakage impedances of
/// [`calc_transformer_leakage_impedances`] scaled by `phases / S_1`, reduced against the last winding,
///
/// ```text
/// ZB[i][j] = z[i][n]                          (i = j)
///          = (z[i][n] + z[j][n] - z[i][j]) / 2  (i ≠ j)
/// ```
///
/// inverted, and lifted back through the winding difference incidence
/// `A` (row `i` is `u_i - u_n`): `Y_1Volt = Aᵀ ZB⁻¹ A`. A two winding unit
/// reduces to `y · [[1, -1], [-1, 1]]`. `None` when the reduced matrix is
/// exactly singular or not finite: one volt base impedances of high rating
/// units sit far below any fixed pivot threshold, so only exact singularity
/// is refused.
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
    let z = |a: usize, b: usize| leakage[a][b] * base;
    let reduced: Vec<Vec<Complex64>> = (0..reference)
        .map(|i| {
            (0..reference)
                .map(|j| {
                    if i == j {
                        z(i, reference)
                    } else {
                        (z(i, reference) + z(j, reference) - z(i, j)) / 2.0
                    }
                })
                .collect()
        })
        .collect();
    let finite = |value: &Complex64| value.re.is_finite() && value.im.is_finite();
    if reduced.iter().flatten().any(|value| !finite(value)) {
        return Ok(None);
    }
    let Some(inverse) = super::invert_above(&reduced, 0.0) else {
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
    /// WYE neutral terminals the source grounds solidly (`r_neutral` and
    /// `x_neutral` both zero): exact zero voltage constraints, not an
    /// admittance.
    pub grounded: Vec<(&'a str, &'a str)>,
    /// The delta phase orientation of the coil map, `+1` or `-1`.
    pub delta_direction: i8,
    /// The winding domain admittance `Y_1Volt` behind `series`
    /// ([`calc_transformer_winding_admittance`]), or the referred winding
    /// primitive of a single phase autotransformer.
    pub winding_admittance: Vec<Vec<Complex64>>,
}

fn primitive_unsupported(name: &str, reason: impl std::fmt::Display) -> Error {
    powerio_core::Error::new(
        &codes::BUILD_MULTI_PHYSICS_UNSUPPORTED,
        format!("transformer `{name}` is unsupported: {reason}"),
    )
    .into()
}

fn primitive_invalid(
    name: &str,
    field: impl std::fmt::Display,
    reason: impl std::fmt::Display,
) -> Error {
    Error::Mtx(format!(
        "transformer `{name}` has invalid {field}: {reason}"
    ))
}

/// Whether [`calc_transformer_primitive_admittance`] refuses this
/// transformer for stating no leakage at all: no short circuit reactance and
/// no winding resistance, or, for a single phase autotransformer, no stated
/// series impedance. Such a unit is an ideal coupling, which
/// [`calc_multiconductor_admittance_matrix`](super::calc_multiconductor_admittance_matrix)
/// writes as exact constraint rows.
pub(super) fn states_no_leakage(transformer: &DistTransformer) -> bool {
    if is_autotransformer(transformer) {
        return [
            "r_series_from",
            "x_series_from",
            "r_series_to",
            "x_series_to",
        ]
        .iter()
        .all(|key| extra_number(&transformer.extras, key).is_none_or(|value| value == 0.0));
    }
    transformer.xsc_pct.iter().all(|value| *value == 0.0)
        && transformer.windings.iter().all(|w| w.r_pct == 0.0)
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
/// with `B` the incidence of winding `w`'s coil on terminal `a` (WYE coils
/// from phase to neutral, DELTA coils around the ring in the stated
/// orientation). A finite WYE neutral impedance stamps once from the neutral
/// terminal to ground, a solid one (both zero) is returned in `grounded`, and
/// a negative `r_neutral` is OpenDSS's floating neutral. The magnetizing
/// branch (`no_load_shunt` siemens on a stated winding, `g_no_load` and
/// `b_no_load` per coil siemens on the first winding, or OpenDSS
/// `%noloadloss` and `%imag` on the second) stamps across each coil of its
/// winding.
///
/// Two winding units of one to three phases and the three winding BMOPF
/// `center_tap` unit are supported. The BMOPF `single_phase_autotransformer`
/// states its impedance in ohms
/// ([`TransformerLeakageBase::StatedOhms`]); its primitive is
/// `y · w wᵀ`, with `w` the from winding incidence minus `n = 1 / tap` times
/// the to winding incidence, over the order from phase, to phase, from
/// reference, to reference.
///
/// # Errors
/// A transformer stating no leakage (an ideal coupling, which needs voltage
/// constraints), another winding count or phase count, a raw OpenDSS
/// property the typed model does not carry (`leadlag`, `xrconst`, `core`, a
/// nondefault `ppm_antifloat`), nameplate, tap, terminal map, neutral
/// impedance or magnetizing data that is not usable, or a singular leakage
/// impedance.
#[allow(clippy::too_many_lines)] // the coil referral, then the neutral and magnetizing stamps
#[allow(clippy::float_cmp)] // 0 and 1 are OpenDSS's exact anti-float defaults
pub fn calc_transformer_primitive_admittance(
    transformer: &DistTransformer,
) -> Result<TransformerPrimitive<'_>> {
    // These raw DSS settings are not represented by the typed winding model.
    // In particular LeadLag must not silently use the default delta
    // orientation.
    for key in ["leadlag", "xrconst", "core"] {
        if transformer.extras.contains_key(key) {
            return Err(primitive_unsupported(
                &transformer.name,
                format!("raw OpenDSS property `{key}` requires normalization"),
            ));
        }
    }
    if extra_number(&transformer.extras, "ppm_antifloat")
        .is_some_and(|value| value != 0.0 && value != 1.0)
    {
        return Err(primitive_unsupported(
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

    let direction = delta_direction(transformer);
    let directions: Vec<_> = (0..transformer.windings.len())
        .map(|index| delta_direction_for(transformer, index, direction))
        .collect();
    let vbase: Vec<_> = transformer
        .windings
        .iter()
        .map(|winding| coil_voltage(winding, phases))
        .collect();
    let y_1volt = calc_transformer_winding_admittance(transformer)
        .map_err(|error| primitive_unsupported(name, error))?
        .ok_or_else(|| {
            Error::Mtx(format!(
                "transformer `{name}` has singular leakage impedance"
            ))
        })?;

    let mut terminals: Vec<(&str, &str)> = Vec::new();
    let mut grounded = Vec::new();
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
        return Err(primitive_invalid(
            name,
            "terminal_map",
            "no physical terminals",
        ));
    }
    let mut y_series = vec![vec![Complex64::new(0.0, 0.0); nterm]; nterm];
    let mut y_shunt = vec![vec![Complex64::new(0.0, 0.0); nterm]; nterm];

    // The physical terminal matrix is the sum over independent phase copies
    // of Bᵀ Y_1Volt B, where B contains the turns and base factors.
    for phase in 0..phases {
        let mut b = vec![vec![Complex64::new(0.0, 0.0); nterm]; transformer.windings.len()];
        for (index, winding) in transformer.windings.iter().enumerate() {
            let (a, z) = coil_refs(winding, phase, phases, directions[index], &terminals, name)?;
            let k = Complex64::new(1.0 / (vbase[index] * checked_tap(winding, name)?), 0.0);
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
                for p in 0..transformer.windings.len() {
                    for q in 0..transformer.windings.len() {
                        value += b[p][i] * y_1volt[p][q] * b[q][j];
                    }
                }
                y_series[i][j] += value;
            }
        }

        // An explicit neutral impedance is one shunt from the physical WYE
        // neutral conductor to ground, shared by all phase coils.
        if phase == 0 {
            for (idx, winding) in transformer.windings.iter().enumerate() {
                if winding.r_neutral.is_none() && winding.x_neutral.is_none() {
                    continue;
                }
                if winding.conn == DistWindingConn::Delta {
                    return Err(primitive_unsupported(
                        name,
                        "neutral impedance on a delta winding has no physical neutral terminal",
                    ));
                }
                if winding.conn != DistWindingConn::Wye {
                    continue;
                }
                let neutral = neutral_ref(winding, phases, &terminals, name)?;
                let r = winding.r_neutral.unwrap_or(0.0);
                let x = winding.x_neutral.unwrap_or(0.0);
                if r < 0.0 || !r.is_finite() || !x.is_finite() {
                    if r < 0.0 && r.is_finite() && x.is_finite() {
                        // OpenDSS uses a negative RNeut as the explicit
                        // floating neutral sentinel.
                        continue;
                    }
                    return Err(primitive_invalid(
                        name,
                        format!("windings[{idx}].neutral impedance"),
                        "expected finite nonnegative resistance and reactance",
                    ));
                }
                if r == 0.0 && x == 0.0 {
                    let terminal = winding.terminal_map.get(phases).ok_or_else(|| {
                        primitive_invalid(
                            name,
                            format!("windings[{idx}].terminal_map"),
                            "solid WYE neutral has no terminal",
                        )
                    })?;
                    grounded.push((winding.bus.as_str(), terminal.as_str()));
                } else if let Some(i) = neutral {
                    y_series[i][i] += Complex64::new(1.0, 0.0) / Complex64::new(r, x);
                }
            }
        }
    }

    add_excitation(transformer, &mut y_shunt, &terminals)?;
    let y_prim = sum_matrices(&y_series, &y_shunt);
    Ok(TransformerPrimitive {
        name,
        terminals,
        series: y_series,
        shunt: y_shunt,
        primitive: y_prim,
        grounded,
        delta_direction: direction,
        winding_admittance: y_1volt,
    })
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

/// The fixed ratio BMOPF single phase autotransformer or regulator: its
/// impedance in referred ohms and its ratio the regulated over source
/// voltage, for both ANSI types (`regulator_type` records the hardware
/// only), as the BMOPF schema reads it. For Type B this is the BMOPFTools
/// calculation and the equivalent fixed tap OpenDSS two winding regulator.
fn autotransformer_primitive(transformer: &DistTransformer) -> Result<TransformerPrimitive<'_>> {
    let name = transformer.name.as_str();
    if transformer.phases != 1 || transformer.windings.len() != 2 {
        return Err(primitive_unsupported(
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
            return Err(primitive_invalid(
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
        return Err(primitive_invalid(
            name,
            "regulator_type",
            format!("expected A or B, got `{regulator_type}`"),
        ));
    }
    let n_eff = 1.0 / tap_ratio;
    let z = calc_transformer_leakage_impedances(transformer, &TransformerLeakageBase::StatedOhms)?
        [0][1];
    if z.norm() == 0.0 {
        return Err(primitive_unsupported(
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
    let mut from_incidence = vec![0.0; nterm];
    let mut to_incidence = vec![0.0; nterm];
    for (incidence, winding) in [(&mut from_incidence, from), (&mut to_incidence, to)] {
        let phase = terminal_index(&terminals, &winding.bus, &winding.terminal_map[0])
            .expect("every winding terminal is listed");
        let reference = terminal_index(&terminals, &winding.bus, &winding.terminal_map[1])
            .expect("every winding terminal is listed");
        incidence[phase] += 1.0;
        incidence[reference] -= 1.0;
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
    add_excitation(transformer, &mut y_shunt, &terminals)?;
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
        return Err(primitive_unsupported(
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
        return Err(primitive_unsupported(
            name,
            format!(
                "BMOPF centre-tap primitives require one phase; got {}",
                t.phases
            ),
        ));
    }
    if t.phases == 0 || t.phases > 3 {
        return Err(primitive_unsupported(
            name,
            format!(
                "{} phases are outside the supported 1, 2, and 3 phase shapes",
                t.phases
            ),
        ));
    }
    let expected_xsc = if center_tap { 3 } else { 1 };
    if t.xsc_pct.len() != expected_xsc {
        return Err(primitive_invalid(
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
        return Err(primitive_invalid(
            name,
            "xsc_pct",
            "must be finite and nonnegative",
        ));
    }
    for (idx, w) in t.windings.iter().enumerate() {
        if !w.v_ref.is_finite() || w.v_ref <= 0.0 {
            return Err(primitive_invalid(
                name,
                format!("windings[{idx}].v_ref"),
                "must be positive and finite",
            ));
        }
        if !w.s_rating.is_finite() || w.s_rating <= 0.0 {
            return Err(primitive_invalid(
                name,
                format!("windings[{idx}].s_rating"),
                "must be positive and finite",
            ));
        }
        if !w.r_pct.is_finite() || w.r_pct < 0.0 {
            return Err(primitive_invalid(
                name,
                format!("windings[{idx}].r_pct"),
                "must be finite and nonnegative",
            ));
        }
        checked_tap(w, name)?;
        let expected = terminal_count(w, t.phases);
        if t.phases == 1 && w.conn == DistWindingConn::Delta && w.terminal_map.len() < 2 {
            return Err(primitive_invalid(
                name,
                format!("windings[{idx}].terminal_map"),
                "a one-phase DELTA winding requires two coil endpoints",
            ));
        }
        if w.terminal_map.len() != expected
            && !(w.conn == DistWindingConn::Wye && w.terminal_map.len() == t.phases)
        {
            return Err(primitive_invalid(
                name,
                format!("windings[{idx}].terminal_map"),
                format!(
                    "expected {expected} terminals (or {} phase terminals with an implicit WYE ground), got {}",
                    t.phases,
                    w.terminal_map.len()
                ),
            ));
        }
    }
    if states_no_leakage(t) {
        return Err(primitive_unsupported(
            name,
            "zero leakage requires ideal voltage constraints and cannot be represented by an admittance primitive",
        ));
    }
    Ok(())
}

fn checked_tap(w: &DistWinding, name: &str) -> Result<f64> {
    if !w.tap.is_finite() || w.tap <= 0.0 {
        return Err(primitive_invalid(
            name,
            "winding.tap",
            "must be positive and finite",
        ));
    }
    Ok(w.tap)
}

fn terminal_count(w: &DistWinding, phases: usize) -> usize {
    if phases == 1 {
        2.min(w.terminal_map.len())
    } else if w.conn == DistWindingConn::Wye {
        phases + 1
    } else {
        phases
    }
}

/// A winding's coil voltage: line to neutral for a polyphase WYE winding,
/// the rated voltage otherwise.
fn coil_voltage(w: &DistWinding, phases: usize) -> f64 {
    if w.conn == DistWindingConn::Wye && matches!(phases, 2 | 3) {
        w.v_ref / 3f64.sqrt()
    } else {
        w.v_ref
    }
}

fn terminal_index(terminals: &[(&str, &str)], bus: &str, terminal: &str) -> Option<usize> {
    terminals
        .iter()
        .position(|(b, t)| b.eq_ignore_ascii_case(bus) && *t == terminal)
}

/// The two terminal positions of one phase coil: phase terminal, then its
/// neutral (WYE), the next phase in `direction` (DELTA), or the second
/// terminal (single phase). `None` is an implicit ground.
fn coil_refs(
    w: &DistWinding,
    phase: usize,
    phases: usize,
    direction: i8,
    terminals: &[(&str, &str)],
    name: &str,
) -> Result<(Option<usize>, Option<usize>)> {
    let first = w.terminal_map.get(phase).ok_or_else(|| {
        primitive_invalid(
            name,
            "terminal_map",
            format!("missing phase terminal {phase}"),
        )
    })?;
    let second_name = if phases == 1 {
        w.terminal_map.get(1)
    } else if w.conn == DistWindingConn::Wye {
        w.terminal_map.get(phases)
    } else {
        // The next phase around the ring in the coil map's orientation.
        let next = if direction < 0 {
            (phase + phases - 1) % phases
        } else {
            (phase + 1) % phases
        };
        w.terminal_map.get(next)
    };
    let first_idx = terminal_index(terminals, &w.bus, first);
    let second_idx = second_name.and_then(|s| terminal_index(terminals, &w.bus, s));
    Ok((first_idx, second_idx))
}

fn neutral_ref(
    w: &DistWinding,
    phases: usize,
    terminals: &[(&str, &str)],
    name: &str,
) -> Result<Option<usize>> {
    if w.terminal_map.len() <= phases {
        return Err(primitive_unsupported(
            name,
            "neutral impedance is declared but the WYE neutral is implicit ground",
        ));
    }
    Ok(terminal_index(terminals, &w.bus, &w.terminal_map[phases]))
}

#[allow(clippy::too_many_lines)] // three magnetizing spellings, each validated in full
#[allow(clippy::many_single_char_names)] // g, b, y and the coil ends follow the textbook stamp
fn add_excitation(
    t: &DistTransformer,
    y: &mut [Vec<Complex64>],
    terminals: &[(&str, &str)],
) -> Result<()> {
    let name = t.name.as_str();
    let explicit = t.extras.get("no_load_shunt");
    let legacy = extra_number(&t.extras, "g_no_load")
        .or_else(|| extra_number(&t.extras, "b_no_load"))
        .is_some();
    if explicit.is_some() && legacy {
        return Err(primitive_invalid(
            name,
            "no_load_shunt",
            "cannot coexist with g_no_load or b_no_load",
        ));
    }
    let stamp_winding =
        |y: &mut [Vec<Complex64>], winding: usize, value: Complex64| -> Result<()> {
            let w = &t.windings[winding];
            let direction = delta_direction_for(t, winding, delta_direction(t));
            for p in 0..t.phases {
                let (a, z) = coil_refs(w, p, t.phases, direction, terminals, name)?;
                add_coil_shunt(y, a, z, value);
            }
            Ok(())
        };
    if let Some(shunt) = explicit {
        let obj = shunt.as_object().ok_or_else(|| {
            primitive_invalid(
                name,
                "no_load_shunt",
                "expected an object containing winding, g, and b",
            )
        })?;
        let winding = obj
            .get("winding")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| {
                primitive_invalid(name, "no_load_shunt.winding", "expected one-based integer")
            })?;
        let winding = usize::try_from(winding)
            .ok()
            .filter(|w| *w >= 1 && *w <= t.windings.len())
            .ok_or_else(|| {
                primitive_invalid(
                    name,
                    "no_load_shunt.winding",
                    format!("must be between 1 and {}", t.windings.len()),
                )
            })?;
        let g = obj
            .get("g")
            .and_then(serde_json::Value::as_f64)
            .ok_or_else(|| {
                primitive_invalid(
                    name,
                    "no_load_shunt.g",
                    "expected finite nonnegative siemens",
                )
            })?;
        let b = obj
            .get("b")
            .and_then(serde_json::Value::as_f64)
            .ok_or_else(|| primitive_invalid(name, "no_load_shunt.b", "expected finite siemens"))?;
        if !g.is_finite() || g < 0.0 || !b.is_finite() {
            return Err(primitive_invalid(
                name,
                "no_load_shunt",
                "requires finite g >= 0 and finite b",
            ));
        }
        return stamp_winding(y, winding - 1, Complex64::new(g, b));
    }
    if legacy {
        // The autotransformer fields are from winding per coil siemens.
        let g = extra_number(&t.extras, "g_no_load").unwrap_or(0.0);
        let b = extra_number(&t.extras, "b_no_load").unwrap_or(0.0);
        if !g.is_finite() || g < 0.0 || !b.is_finite() {
            return Err(primitive_invalid(
                name,
                "g_no_load/b_no_load",
                "requires finite g >= 0 and finite b",
            ));
        }
        return stamp_winding(y, 0, Complex64::new(g, b));
    }
    // OpenDSS percentages, which its reference implementation places on
    // winding 2.
    let loss = extra_number(&t.extras, "%noloadloss");
    let imag = extra_number(&t.extras, "%imag");
    if loss.is_none() && imag.is_none() {
        return Ok(());
    }
    let loss = loss.unwrap_or(0.0);
    let imag = imag.unwrap_or(0.0);
    if !loss.is_finite() || loss < 0.0 || !imag.is_finite() || imag < 0.0 {
        return Err(primitive_invalid(
            name,
            "%noloadloss/%imag",
            "requires finite nonnegative percentages",
        ));
    }
    let w = &t.windings[1];
    let v = coil_voltage(w, t.phases) * w.tap;
    let ybase = t.windings[0].s_rating / (t.phases as f64 * v * v);
    let shunt = Complex64::new(loss / 100.0 * ybase, -imag / 100.0 * ybase);
    stamp_winding(y, 1, shunt)
}

fn add_coil_shunt(y: &mut [Vec<Complex64>], a: Option<usize>, z: Option<usize>, value: Complex64) {
    if let Some(i) = a {
        y[i][i] += value;
    }
    if let Some(i) = z {
        y[i][i] += value;
    }
    if let (Some(i), Some(j)) = (a, z) {
        y[i][j] -= value;
        y[j][i] -= value;
    }
}

/// The delta phase orientation: the BMOPF conventional three phase subtypes
/// state it (`wye_delta` +1, `delta_wye` -1); otherwise OpenDSS's rule, which
/// reads the high voltage winding's connection and `hv_leads_lv`.
fn delta_direction(t: &DistTransformer) -> i8 {
    let a = &t.windings[0];
    let b = &t.windings[1];
    match subtype(t) {
        Some("wye_delta") => return 1,
        Some("delta_wye") => return -1,
        _ => {}
    }
    if a.conn == b.conn {
        return 1;
    }
    let hv = if a.v_ref >= b.v_ref { a } else { b };
    let hv_leads_lv = t
        .extras
        .get("hv_leads_lv")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    match (hv.conn, hv_leads_lv) {
        (DistWindingConn::Wye, true) | (DistWindingConn::Delta, false) => -1,
        _ => 1,
    }
}

/// A BMOPF n-winding unit states each DELTA winding's roll
/// (`bmopf_delta_rolls`, one based keys, +1 when absent as BMOPFTools'
/// schema defaults it); every other unit reads `fallback`.
fn delta_direction_for(t: &DistTransformer, winding: usize, fallback: i8) -> i8 {
    if subtype(t) != Some("n_winding") {
        return fallback;
    }
    t.extras
        .get("bmopf_delta_rolls")
        .and_then(serde_json::Value::as_object)
        .and_then(|rolls| rolls.get(&(winding + 1).to_string()))
        .and_then(serde_json::Value::as_i64)
        .filter(|roll| *roll == -1 || *roll == 1)
        .map_or(1, |roll| if roll < 0 { -1 } else { 1 })
}
