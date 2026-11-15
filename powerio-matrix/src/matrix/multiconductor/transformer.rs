//! Exact ideal WYE constraints over explicit winding terminals.

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
    for transformer in network.transformers() {
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

/// The pairwise leakage impedances of a multiwinding transformer, per unit on
/// the first winding's rating: for windings `a` and `b`,
///
/// ```text
/// z[a][b] = (r_a · S_1 / S_a + r_b · S_1 / S_b) / 100 + j · xsc_ab / 100
/// ```
///
/// with each winding's `r_pct` referred to the first winding's rating and
/// `xsc_pct` in pair order `(1,2), (1,3), …, (2,3), …`. The diagonal is zero.
/// The one leakage derivation from OpenDSS style nameplate data: the
/// multiconductor OPF transformer descriptor scales it to its ohmic base, and
/// [`calc_transformer_winding_admittance`] inverts it.
///
/// # Errors
/// Fewer than two windings, a winding rating that is not finite and
/// positive, a resistance that is not finite, or an `xsc_pct` whose length is
/// not one value per winding pair.
pub fn calc_transformer_leakage_impedances(
    transformer: &DistTransformer,
) -> Result<Vec<Vec<Complex64>>> {
    let windings = &transformer.windings;
    let count = windings.len();
    if count < 2 {
        return Err(invalid_leakage(transformer, "fewer than two windings"));
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
    let reference_rating = windings[0].s_rating;
    let resistance: Vec<f64> = windings
        .iter()
        .map(|w| w.r_pct / 100.0 * reference_rating / w.s_rating)
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

/// The winding domain admittance `Y_1Volt` of a multiwinding transformer,
/// siemens at a one volt winding base (OpenDSS `CalcY_Terminal`): the
/// pairwise leakage impedances of [`calc_transformer_leakage_impedances`]
/// scaled by `phases / S_1`, reduced against the last winding,
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
    let leakage = calc_transformer_leakage_impedances(transformer)?;
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
