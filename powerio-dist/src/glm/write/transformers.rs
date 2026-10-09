use super::{Writer, flags, unsupported};
use crate::{DistLocationTarget, DistTransformer, DistWindingConn};
use powerio_core::Error;
use std::fmt::Write as _;

impl Writer<'_> {
    #[allow(clippy::float_cmp)] // Exact base/tap restrictions; no approximate projection.
    pub(super) fn validate_transformers(&self) -> Result<(), Error> {
        for t in self.net.transformers() {
            let fail = || {
                unsupported(format!(
                    "transformer {} needs an additional winding/connection/tap/shunt mapping",
                    t.name
                ))
            };
            if self.ideal_plan.is_some()
                || !matches!(t.phases, 1 | 3)
                || t.extras.iter().any(|(k, v)| match k.as_str() {
                    "ppm_antifloat" => v.as_f64() != Some(0.0),
                    "rating_is_normalization_base" => v.as_bool() != Some(true),
                    _ => true,
                })
            {
                return Err(fail());
            }
            let [a, b] = t.windings.as_slice() else {
                return Err(fail());
            };
            let [x] = t.xsc_pct.as_slice() else {
                return Err(fail());
            };
            let regulator = t.phases == 1 && *x == 0.0 && a.r_pct == 0.0 && b.tap == 1.0;
            if !x.is_finite() || *x < 0.0 || (*x == 0.0 && !regulator) || a.s_rating != b.s_rating {
                return Err(fail());
            }
            for w in [a, b] {
                if w.conn != DistWindingConn::Wye
                    || !w.tap.is_finite()
                    || w.tap <= 0.0
                    || (!regulator && w.tap != 1.0)
                    || !w.v_ref.is_finite()
                    || w.v_ref <= 0.0
                    || !w.s_rating.is_finite()
                    || w.s_rating <= 0.0
                    || !w.r_pct.is_finite()
                    || w.r_pct < 0.0
                    || w.r_neutral.is_some_and(|r| r != 0.0)
                    || w.x_neutral.is_some_and(|x| x != 0.0)
                    || w.terminal_map.len() != t.phases + 1
                    || w.terminal_map.last().is_none_or(|n| n != "n")
                {
                    return Err(fail());
                }
            }
            if a.r_pct + b.r_pct <= 0.0 {
                return Err(fail());
            }
            self.link_phases(
                &t.name,
                &a.bus,
                &b.bus,
                &a.terminal_map[..t.phases],
                &b.terminal_map[..t.phases],
            )?;
        }
        Ok(())
    }

    pub(super) fn transformers(&mut self) -> Result<(), Error> {
        for t in self.net.transformers() {
            let a = &t.windings[0];
            let b = &t.windings[1];
            let phases = super::phases(&a.terminal_map[..t.phases])?;
            let config = self.generated_name(&format!("{}__configuration", t.name));
            if t.xsc_pct[0] == 0.0 {
                self.regulator(t, &config, &phases)?;
                continue;
            }
            if t.extras.contains_key("rating_is_normalization_base") {
                self.dropped(format!("transformer {}: GLM power_rating carries the winding normalization base; absence of a source nameplate rating has no configuration field", t.name));
            }
            let connection = if t.phases == 1 {
                "SINGLE_PHASE"
            } else {
                "WYE_WYE"
            };
            let _ = writeln!(
                self.out,
                "object transformer_configuration {{ name \"{config}\"; connect_type {connection}; primary_voltage {}; secondary_voltage {}; power_rating {}; resistance {}; reactance {}; }}",
                a.v_ref,
                b.v_ref,
                a.s_rating / 1000.0,
                (a.r_pct + b.r_pct) / 100.0,
                t.xsc_pct[0] / 100.0
            );
            let _ = writeln!(
                self.out,
                "object transformer {{ name \"{}\"; phases {}N; from \"{}\"; to \"{}\"; configuration \"{config}\";",
                t.name,
                flags(&phases),
                a.bus,
                b.bus
            );
            self.coordinates(DistLocationTarget::Transformer, &t.name);
            self.out.push_str("}\n");
        }
        Ok(())
    }
    fn regulator(
        &mut self,
        t: &DistTransformer,
        config: &str,
        phases: &[usize],
    ) -> Result<(), Error> {
        let a = &t.windings[0];
        let b = &t.windings[1];
        let ratio = a.v_ref * a.tap / b.v_ref;
        let resistance = b.r_pct / 100.0 * b.v_ref.powi(2) / b.s_rating;
        if !ratio.is_finite() || ratio <= 0.0 || !resistance.is_finite() || resistance <= 0.0 {
            return Err(unsupported(format!(
                "regulator equivalent {} has an invalid turns ratio or resistance",
                t.name
            )));
        }
        let amount = 1.0 - ratio;
        let tap = if amount > 0.0 {
            1
        } else if amount < 0.0 {
            -1
        } else {
            0
        };
        let phase = super::super::read::phase_name(phases[0]);
        let _ = writeln!(
            self.out,
            "object regulator_configuration {{ name \"{config}\"; connect_type WYE_WYE; Control MANUAL; Type B; raise_taps 1; lower_taps 1; regulation {}; tap_pos_{phase} {tap}; }}",
            amount.abs()
        );
        let _ = writeln!(
            self.out,
            "object regulator {{ name \"{}\"; phases {}N; from \"{}\"; to \"{}\"; configuration \"{config}\"; regulator_resistance {resistance};",
            t.name,
            flags(phases),
            a.bus,
            b.bus
        );
        self.coordinates(DistLocationTarget::Transformer, &t.name);
        self.out.push_str("}\n");
        self.diagnostics.push(&crate::diagnostics::codes::EMIT_GLM_VALUE_REBASED,format!("{}: canonical manual regulator settings encode the current winding ratio and resistance; no controller is emitted",t.name));
        Ok(())
    }
}
