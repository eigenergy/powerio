use super::{Writer, complex_literal, flags, unsupported};
use crate::diagnostics::codes;
use crate::glm::connections::EXPLICIT;
use crate::glm::line_physics::fbs_shunts_commute;
use crate::{DistLine, DistLineCode};
use num_complex::Complex64;
use powerio_core::Error;
use std::fmt::Write as _;

impl Writer<'_> {
    pub(super) fn expand_line_shunts(&self, code: &DistLineCode) -> bool {
        code.g_from
            .iter()
            .flatten()
            .chain(code.g_to.iter().flatten())
            .any(|&v| v != 0.0)
            || code.b_from != code.b_to
            || (self.ideal_plan.is_some() && !fbs_shunts_commute(code))
    }

    #[allow(clippy::float_cmp)] // Exact reciprocity; no unrequested symmetrization.
    pub(super) fn line_shunts(
        &mut self,
        line: &DistLine,
        code: &DistLineCode,
        phases: &[usize],
    ) -> Result<(), Error> {
        // Preserve physical endpoint attachment even when the series link is
        // reversed for FBS. Y = sum(y_ij m_ij m_ij') + diag(row_sums(Y)).
        for (side, bus, g, b) in [
            ("from", &line.bus_from, &code.g_from, &code.b_from),
            ("to", &line.bus_to, &code.g_to, &code.b_to),
        ] {
            let n = phases.len();
            let mut terms = Vec::new();
            for i in 0..n {
                let mut earth = Complex64::default();
                for j in 0..n {
                    if g[i][j] != g[j][i] || b[i][j] != b[j][i] {
                        return Err(unsupported(format!(
                            "line {} {side}: nonsymmetric shunt matrix needs a nonreciprocal equivalent",
                            line.name
                        )));
                    }
                    let coefficient = Complex64::new(g[i][j], b[i][j]);
                    let admittance = coefficient * line.length;
                    if coefficient != Complex64::default() && admittance == Complex64::default() {
                        return Err(unsupported("line end-shunt admittance underflows"));
                    }
                    earth += admittance;
                    if j > i {
                        terms.push((phases[i], Some(phases[j]), -admittance));
                    }
                }
                terms.push((phases[i], None, earth));
            }
            if terms.iter().all(|(_, _, y)| *y == Complex64::default()) {
                continue;
            }
            let name = self.generated_name(&format!("{}__glm_shunt_{side}", line.name));
            let _ = writeln!(
                self.out,
                "object load {{ name \"{name}\"; parent \"{bus}\"; phases {}N; nominal_voltage {};",
                flags(phases),
                self.nominal[bus.as_str()]
            );
            for (a, b, y) in terms {
                if y == Complex64::default() {
                    continue;
                }
                let suffix = EXPLICIT
                    .iter()
                    .find(|(_, x, z)| {
                        (*x == a && *z == b) || b.is_some_and(|b| *x == b && *z == Some(a))
                    })
                    .map(|v| v.0)
                    .ok_or_else(|| unsupported("line shunt has no explicit connection"))?;
                if !y.re.is_finite() || !y.im.is_finite() {
                    return Err(unsupported("line end-shunt expansion overflows"));
                }
                let impedance = Complex64::new(1.0, 0.0) / y;
                if impedance == Complex64::default() {
                    return Err(unsupported("line end-shunt impedance underflows"));
                }
                let _ = writeln!(
                    self.out,
                    "    constant_impedance_{suffix} {};",
                    complex_literal(impedance)?
                );
            }
            self.out.push_str("}\n");
            self.diagnostics.push(&codes::EMIT_GLM_LINE_SHUNT_EQUIVALENT,
                format!("line {} {side}: shunt admittance is materialized as load {name} at bus {bus}; physical line port currents and powers include this load as well as the series link; its location remains on the original line", line.name));
        }
        Ok(())
    }
}
