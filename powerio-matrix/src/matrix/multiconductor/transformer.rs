//! Transformers in the multiconductor nodal system: exact ideal WYE
//! constraints over explicit winding terminals for a unit that states no
//! leakage. Every other unit stamps the finite leakage primitive of
//! [`powerio_dist::transformer`].

use std::collections::BTreeMap;

use powerio_dist::transformer::calc_winding_connection;
use powerio_dist::{DistTransformer, DistWindingConn, MulticonductorNetwork};

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

/// Each phase coil of one winding as node references, the phase terminal
/// then its neutral, over the one winding connection
/// ([`calc_winding_connection`]). A coil without a return (a one terminal
/// single phase winding, the existing implicit ground spelling) returns
/// through ground. A stated neutral is not grounded merely because it is a
/// winding's last terminal.
fn winding_nodes(
    network: &MulticonductorNetwork,
    transformer: &DistTransformer,
    winding: usize,
    index: &MulticonductorNodeIndex,
) -> Result<Vec<(NodeRef, NodeRef)>> {
    let w = &transformer.windings[winding];
    let node = |terminal: usize| -> Result<NodeRef> {
        let terminal = &w.terminal_map[terminal];
        index.resolve(&w.bus, terminal).ok_or_else(|| {
            Error::Mtx(format!(
                "transformer `{}` names terminal `{terminal}` bus `{}` does not declare",
                transformer.name, w.bus
            ))
        })
    };
    let coils = calc_winding_connection(network, transformer, winding)?
        .iter()
        .map(|coil| Ok((node(coil.from)?, coil.to.map_or(Ok(NodeRef::Ground), node)?)))
        .collect::<Result<Vec<_>>>()?;
    let explicit_neutral = coils.iter().any(|&(_, neutral)| neutral != NodeRef::Ground);
    if explicit_neutral && (w.r_neutral.is_some() || w.x_neutral.is_some()) {
        return Err(unsupported(
            transformer,
            "an explicit winding-neutral grounding parameter needs its own circuit",
        ));
    }
    Ok(coils)
}

/// The exact constraint rows of the transformers that state no leakage
/// (the ideal transformers of
/// [`calc_passive_primitives`](super::calc_passive_primitives)); a unit
/// stating leakage stamps its primitive instead.
pub(super) fn ideal_wye_constraints(
    network: &MulticonductorNetwork,
    ideal: &[&DistTransformer],
    index: &MulticonductorNodeIndex,
) -> Result<Vec<IdealConstraint>> {
    let mut result = Vec::new();
    for &transformer in ideal {
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
        let primary_coils = winding_nodes(network, transformer, 0, index)?;
        let secondary_coils = winding_nodes(network, transformer, 1, index)?;
        for (phase, (&(p, primary_neutral), &(s, secondary_neutral))) in
            primary_coils.iter().zip(&secondary_coils).enumerate()
        {
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
