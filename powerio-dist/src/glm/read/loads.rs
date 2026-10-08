use super::{Reader, allocate_name};
use crate::glm::ObjectKey;
use crate::glm::connections::{EXPLICIT, reference};
use crate::{Configuration, DistLoad, DistLoadVoltageModel, DistLocationTarget};
use num_complex::Complex64;
use powerio_core::Error;

/// Canonical complex property, optional real/imaginary part, and native unit.
pub(super) fn property(
    name: &str,
) -> Option<(
    &str,
    Option<bool>,
    crate::glm::GlmQuantityKind,
    &'static str,
)> {
    use crate::glm::GlmQuantityKind::{Complex, Real};
    let (canonical, part) = if let Some(base) = name.strip_suffix("_real") {
        (base, Some(false))
    } else if let Some(base) = name.strip_suffix("_reac") {
        (base, Some(true))
    } else {
        (name, None)
    };
    for (prefix, unit) in [
        ("constant_power_", "VA"),
        ("constant_current_", "A"),
        ("constant_impedance_", "Ohm"),
    ] {
        if canonical.strip_prefix(prefix).is_some_and(|suffix| {
            matches!(
                suffix,
                "A" | "B" | "C" | "AB" | "BC" | "CA" | "AN" | "BN" | "CN"
            )
        }) {
            let unit = match (unit, part) {
                ("VA", Some(false)) => "W",
                ("VA", Some(true)) => "VAr",
                _ => unit,
            };
            return Some((
                canonical,
                part,
                if part.is_some() { Real } else { Complex },
                unit,
            ));
        }
    }
    None
}

impl Reader<'_> {
    pub(super) fn explicit_load_term_count(&self, key: ObjectKey) -> usize {
        ["power", "current", "impedance"]
            .iter()
            .map(|law| {
                EXPLICIT
                    .iter()
                    .filter(|(suffix, _, _)| {
                        self.quantity(key, &format!("constant_{law}_{suffix}"))
                            .is_some()
                    })
                    .count()
            })
            .sum()
    }

    pub(super) fn connected_load_terms(
        &mut self,
        key: ObjectKey,
        root: ObjectKey,
        phases: &[usize],
        nominal: f64,
        delta: bool,
    ) -> Result<(), Error> {
        let legacy_present = !delta
            && ["power", "current", "impedance"].iter().any(|law| {
                phases.iter().any(|&p| {
                    self.quantity(key, &format!("constant_{law}_{}", super::phase_name(p)))
                        .is_some()
                })
            });
        let channels: Vec<_> = EXPLICIT
            .into_iter()
            .chain(
                delta
                    .then_some([("A", 0, Some(1)), ("B", 1, Some(2)), ("C", 2, Some(0))])
                    .into_iter()
                    .flatten(),
            )
            .collect();
        let term_count: usize = ["power", "current", "impedance"]
            .iter()
            .map(|law| {
                channels
                    .iter()
                    .filter(|(suffix, _, _)| {
                        self.quantity(key, &format!("constant_{law}_{suffix}"))
                            .is_some()
                    })
                    .count()
            })
            .sum();
        let sole_term = term_count == 1 && !legacy_present;
        for law in ["power", "current", "impedance"] {
            for &(suffix, from, to) in &channels {
                let Some(value) = self.quantity(key, &format!("constant_{law}_{suffix}")) else {
                    continue;
                };
                if !phases.contains(&from) || to.is_some_and(|p| !phases.contains(&p)) {
                    return Err(self.error(
                        key,
                        format!("explicit load connection {suffix} uses an absent phase"),
                    ));
                }
                let voltage = reference(nominal, from, to);
                let power = match law {
                    "power" => value,
                    "current" => voltage * value.conj(),
                    _ if value == Complex64::default() => Complex64::default(),
                    _ => voltage.norm_sqr() / value.conj(),
                };
                if !power.re.is_finite()
                    || !power.im.is_finite()
                    || (value != Complex64::default() && power == Complex64::default())
                {
                    return Err(self.error(
                        key,
                        "explicit load power normalization overflows or underflows",
                    ));
                }
                let name = if key != root && sole_term {
                    self.names[&key].clone()
                } else {
                    allocate_name(
                        &mut self.identities,
                        &format!("{}__glm_{law}_{suffix}", self.names[&key]),
                    )
                };
                let terminals = vec![
                    (from + 1).to_string(),
                    to.map_or_else(|| "n".into(), |p| (p + 1).to_string()),
                ];
                let mut load = DistLoad::new(
                    name,
                    &self.names[&root],
                    terminals,
                    Configuration::SinglePhase,
                    vec![power.re],
                    vec![power.im],
                );
                let v_nom = vec![voltage.norm()];
                load.voltage_model = match law {
                    "power" => DistLoadVoltageModel::ConstantPower { v_nom },
                    "current" => DistLoadVoltageModel::ConstantCurrent { v_nom },
                    _ => DistLoadVoltageModel::ConstantImpedance { v_nom },
                };
                if key != root {
                    self.attach(key, DistLocationTarget::Load, load.name.clone());
                }
                self.net.loads_mut().push(load);
            }
        }
        Ok(())
    }
}
