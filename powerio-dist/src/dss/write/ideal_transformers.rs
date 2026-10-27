//! Target-only exact circuit for ideal single-phase voltage ratios.
use super::{
    C, DistTransformer, DistWindingConn, DssWriter, MulticonductorNetwork, is_positive_finite,
};
use std::collections::BTreeSet;

fn ideal(t: &DistTransformer) -> bool {
    t.phases == 1
        && t.windings.len() == 2
        && t.xsc_pct.as_slice() == [0.0]
        && t.windings.iter().all(|w| {
            w.r_pct == 0.0
                && w.terminal_map.len() == 2
                && w.conn == DistWindingConn::Wye
                && is_positive_finite(w.v_ref)
                && is_positive_finite(w.s_rating)
                && is_positive_finite(w.tap)
        })
}

pub(super) fn used_names(net: &MulticonductorNetwork) -> BTreeSet<String> {
    net.buses()
        .iter()
        .map(|b| &b.id)
        .chain(net.lines().iter().map(|l| &l.name))
        .chain(net.switches().iter().map(|s| &s.name))
        .chain(net.transformers().iter().map(|t| &t.name))
        .map(|name| name.to_ascii_lowercase())
        .collect()
}

#[allow(clippy::float_cmp)] // Exact shared-base requirement, not a numeric tolerance.
pub(super) fn validate(net: &MulticonductorNetwork) -> Result<(), String> {
    for t in net.transformers().iter().filter(|t| ideal(t)) {
        let b = &t.windings[1];
        let secondary_bus = net
            .buses()
            .iter()
            .find(|bus| bus.id.eq_ignore_ascii_case(&b.bus));
        let r_pct = 100.0 * b.s_rating / (b.v_ref * b.tap).powi(2);
        if t.phases != 1
            || t.windings.iter().any(|w| {
                w.conn != DistWindingConn::Wye
                    || w.terminal_map.len() != 2
                    || !is_positive_finite(w.v_ref)
                    || !is_positive_finite(w.tap)
                    || !is_positive_finite(w.s_rating)
                    || w.r_neutral.is_some_and(|v| v != 0.0)
                    || w.x_neutral.is_some_and(|v| v != 0.0)
            })
            || t.windings[0].s_rating != b.s_rating
            || !is_positive_finite(r_pct)
            || !secondary_bus.is_some_and(|bus| {
                b.terminal_map
                    .iter()
                    .all(|terminal| bus.terminals.contains(terminal))
                    && !bus.grounded.contains(&b.terminal_map[0])
                    && b.terminal_map[0] != b.terminal_map[1]
            })
            || t.extras.iter().any(|(k, v)| match k.as_str() {
                "ppm_antifloat" => v.as_f64().or_else(|| v.as_str()?.parse().ok()) != Some(0.0),
                "rating_is_normalization_base" => v.as_bool() != Some(true),
                _ => true,
            })
        {
            return Err(format!(
                "ideal transformer {}: exact DSS projection currently requires two single-phase coils, equal finite ratings, distinct secondary terminals and no core/neutral shunts",
                t.name
            ));
        }
    }
    Ok(())
}

impl DssWriter {
    pub(super) fn ideal_transformer(
        &mut self,
        t: &DistTransformer,
        used: &mut BTreeSet<String>,
    ) -> Option<DistTransformer> {
        if !ideal(t) {
            return None;
        }
        let mut name = format!("_pio_xfmr_{}", t.name);
        while !used.insert(name.to_ascii_lowercase()) {
            name.push('_');
        }
        let b = &t.windings[1];
        let grounded_return = self
            .grounded
            .get(&b.bus.to_ascii_lowercase())
            .is_some_and(|grounded| grounded.contains(&b.terminal_map[1]));
        let port = self.bus_ref(
            &b.bus,
            if grounded_return {
                &b.terminal_map[..1]
            } else {
                &b.terminal_map
            },
        );
        // OpenDSS cannot invert a zero-leakage primitive. Add a finite one-ohm
        // secondary resistance and cancel it at an unloaded internal node.
        // Eliminating that node enforces the original ideal ratio exactly;
        // neither resistance is an approximation or an epsilon.
        let mut projected = t.clone();
        let secondary = &mut projected.windings[1];
        secondary.bus.clone_from(&name);
        secondary.terminal_map = vec!["1".into(), if grounded_return { "0" } else { "2" }.into()];
        secondary.r_pct = 100.0 * b.s_rating / (b.v_ref * b.tap).powi(2);
        projected
            .extras
            .insert("ppm_antifloat".into(), serde_json::json!(0.0));
        if grounded_return {
            self.line_out(&format!("New Line.{name} phases=1 bus1={name}.1 bus2={port} length=1 units=m rmatrix=(-1) xmatrix=(0) cmatrix=(0)"));
        } else {
            // Equal and opposite coil currents flow through both conductors.
            // Each half-ohm cancellation preserves the floating common mode.
            self.line_out(&format!("New Line.{name} phases=2 bus1={name}.1.2 bus2={port} length=1 units=m rmatrix=(-0.5 | 0 -0.5) xmatrix=(0 | 0 0) cmatrix=(0 | 0 0)"));
        }
        self.warn(&C::EMIT_DSS_IDEAL_TRANSFORMER_EQUIVALENT,format!("transformer {}: ideal ratio uses one-ohm secondary leakage cancelled by Line.{name}; external secondary ports and total loss include that line",t.name));
        Some(projected)
    }
}

#[cfg(test)]
mod tests {
    use super::{DistTransformer, DistWindingConn, MulticonductorNetwork, validate};
    use crate::dss::write::emit_dss_text;
    use crate::{DistBus, DistWinding};
    fn network() -> MulticonductorNetwork {
        let mut n = MulticonductorNetwork::new();
        for name in ["source", "sink", "_pio_xfmr_r"] {
            let mut b = DistBus::new(name, vec!["1".into(), "n".into()]);
            b.grounded = vec!["n".into()];
            n.buses_mut().push(b);
        }
        let a = DistWinding::new(
            "source",
            vec!["1".into(), "n".into()],
            DistWindingConn::Wye,
            240.0,
            1000.0,
        );
        let b = DistWinding::new(
            "sink",
            vec!["1".into(), "n".into()],
            DistWindingConn::Wye,
            120.0,
            1000.0,
        );
        n.transformers_mut()
            .push(DistTransformer::new("r", vec![a, b], vec![0.0], 1));
        n
    }
    #[test]
    fn generated_bus_avoids_existing_identity_and_typed_values_do_not_change() {
        let n = network();
        let before = n.transformers().clone();
        validate(&n).unwrap();
        let out = emit_dss_text(&n);
        assert!(
            out.text
                .contains("New Line._pio_xfmr_r_ phases=1 bus1=_pio_xfmr_r_.1")
        );
        assert_eq!(n.transformers(), &before);
    }
    #[test]
    fn floating_return_keeps_both_conductors() {
        let mut n = network();
        n.buses_mut()[1].grounded.clear();
        validate(&n).unwrap();
        let out = emit_dss_text(&n);
        assert!(
            out.text
                .contains("phases=2 bus1=_pio_xfmr_r_.1.2 bus2=sink.1.2")
        );
        assert!(out.text.contains("rmatrix=(-0.5 | 0 -0.5)"));
    }
    #[test]
    fn unqualified_ideal_terminals_or_core_are_rejected() {
        let mut n = network();
        n.transformers_mut()[0].windings[1].terminal_map[1] = "absent".into();
        assert!(validate(&n).is_err());
        let mut n = network();
        n.transformers_mut()[0]
            .extras
            .insert("%imag".into(), serde_json::json!(1.0));
        assert!(validate(&n).is_err());
        let mut n = network();
        n.transformers_mut()[0].windings[1].r_neutral = Some(1.0);
        assert!(validate(&n).is_err());
    }
}
