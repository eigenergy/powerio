//! Exact ideal WYE constraints over explicit winding terminals.

use std::collections::BTreeMap;

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
