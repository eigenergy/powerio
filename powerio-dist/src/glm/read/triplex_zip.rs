//! Native triplex base-power ZIP initialization, before voltage-law projection.
use super::Reader;
use crate::glm::{GlmQuantityKind, ObjectKey};
use num_complex::Complex64;
use powerio_core::{Diagnostic, Error};
use std::collections::BTreeMap;

pub(super) fn quantity_spec(class: &str, name: &str) -> Option<(GlmQuantityKind, &'static str)> {
    if class != "triplex_load" {
        return None;
    }
    let (prefix, suffix) = name.rsplit_once('_')?;
    if !matches!(suffix, "1" | "2" | "12") {
        return None;
    }
    let unit = match prefix {
        "base_power" => "VA",
        "power_fraction" | "current_fraction" | "impedance_fraction" | "power_pf"
        | "current_pf" | "impedance_pf" => "pu",
        _ => return None,
    };
    Some((GlmQuantityKind::Real, unit))
}
impl Reader<'_> {
    #[allow(clippy::float_cmp)] // The native fraction normalization uses exact equality.
    pub(super) fn triplex_zip(
        &mut self,
        key: ObjectKey,
    ) -> Result<BTreeMap<String, Complex64>, Error> {
        let mut terms = BTreeMap::new();
        for suffix in ["1", "2", "12"] {
            let base = self
                .real(key, &format!("base_power_{suffix}"))
                .unwrap_or(0.0);
            if base == 0.0 {
                continue;
            }
            let mut fractions = ["power", "current", "impedance"].map(|law| {
                self.real(key, &format!("{law}_fraction_{suffix}"))
                    .unwrap_or(0.0)
            });
            if fractions.iter().sum::<f64>() != 1.0 {
                fractions[0] = 1.0 - fractions[1] - fractions[2];
            }
            for (law, fraction) in ["power", "current", "impedance"].into_iter().zip(fractions) {
                let pf = self.real(key, &format!("{law}_pf_{suffix}")).unwrap_or(1.0);
                if !(-1.0..=1.0).contains(&pf) {
                    return Err(self.error(key, "triplex ZIP power factors must lie in -1..1"));
                }
                let apparent = base * fraction;
                let power = if pf == 0.0 {
                    Complex64::new(0.0, apparent)
                } else {
                    Complex64::new(
                        apparent * pf.abs(),
                        apparent * (1.0 - pf * pf).sqrt() * pf.signum(),
                    )
                };
                if !power.re.is_finite() || !power.im.is_finite() {
                    return Err(self.error(key, "triplex ZIP initialization overflows"));
                }
                terms.insert(format!("constant_{law}_{suffix}"), power);
            }
        }
        if !terms.is_empty() {
            self.diagnostics.push(Diagnostic::of(&crate::diagnostics::codes::READ_GLM_LOAD_NORMALIZED,
                format!("{}: nonzero triplex base_power overrides explicit P/I/Z coefficients; native power fraction normalization and signed power factors define the typed voltage-law terms",self.names[&key])));
        }
        Ok(terms)
    }
}
