use super::{Writer, flags, unsupported};
use crate::{DistLocationTarget, DistWindingConn};
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
                || t.extras
                    .iter()
                    .any(|(k, v)| k != "ppm_antifloat" || v.as_f64() != Some(0.0))
            {
                return Err(fail());
            }
            let [a, b] = t.windings.as_slice() else {
                return Err(fail());
            };
            let [x] = t.xsc_pct.as_slice() else {
                return Err(fail());
            };
            if !x.is_finite() || *x <= 0.0 || a.s_rating != b.s_rating {
                return Err(fail());
            }
            for w in [a, b] {
                if w.conn != DistWindingConn::Wye
                    || w.tap != 1.0
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
}
