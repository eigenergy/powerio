//! Coherent phase-pair coils projected to native delta-grounded-wye banks.
use super::{Writer, complex_literal, unsupported};
use crate::{DistLocationTarget, DistTransformer, DistWindingConn, MulticonductorNetwork};
use num_complex::Complex64;
use powerio_core::Error;
use std::collections::BTreeMap;
use std::fmt::Write as _;

#[derive(Default)]
pub(super) struct CoilBanks<'a> {
    pub banks: BTreeMap<&'a str, Bank<'a>>,
    pub leaders: BTreeMap<&'a str, &'a str>,
}
pub(super) struct Bank<'a> {
    pub members: [&'a DistTransformer; 3],
    pub primary: f64,
    pub secondary: f64,
    pub rating: f64,
    pub impedance: Complex64,
}

impl<'a> CoilBanks<'a> {
    #[allow(clippy::float_cmp, clippy::too_many_lines)] // Complete native bank certificate; no approximate projection.
    pub fn discover(network: &'a MulticonductorNetwork) -> Result<Self, Error> {
        let mut candidates: BTreeMap<(&str, &str), Vec<&DistTransformer>> = BTreeMap::new();
        for transformer in network.transformers() {
            if transformer.phases == 1
                && transformer.windings.len() == 2
                && transformer.windings[0].terminal_map.len() == 2
                && transformer.windings[0]
                    .terminal_map
                    .iter()
                    .all(|t| matches!(t.as_str(), "1" | "2" | "3"))
            {
                let [primary, secondary] = transformer.windings.as_slice() else {
                    unreachable!()
                };
                candidates
                    .entry((&primary.bus, &secondary.bus))
                    .or_default()
                    .push(transformer);
            }
        }
        let mut plan = Self::default();
        for ((from, to), members) in candidates {
            let fail = || {
                unsupported(format!(
                    "phase-pair bank {from}->{to} requires a complete coherent native coil mapping"
                ))
            };
            if members.len() != 3 {
                return Err(fail());
            }
            let mut ordered = [None; 3];
            for transformer in members {
                let [primary, secondary] = transformer.windings.as_slice() else {
                    unreachable!()
                };
                if secondary.terminal_map.len() != 2 {
                    return Err(fail());
                }
                let phase = super::phases(&secondary.terminal_map[..1])?[0];
                if secondary.terminal_map.len() != 2
                    || secondary.terminal_map[1] != "n"
                    || primary.terminal_map[0] != secondary.terminal_map[0]
                    || ordered[phase].replace(transformer).is_some()
                    || transformer.xsc_pct.len() != 1
                    || transformer
                        .extras
                        .iter()
                        .any(|(k, v)| k != "ppm_antifloat" || v.as_f64() != Some(0.0))
                    || primary
                        .r_neutral
                        .is_some_and(|r| !r.is_finite() || r >= 0.0)
                    || primary.x_neutral.is_some_and(|x| !x.is_finite())
                    || secondary.r_neutral.is_some_and(|r| r != 0.0)
                    || secondary.x_neutral.is_some_and(|x| x != 0.0)
                    || [primary, secondary].iter().any(|w| {
                        w.conn != DistWindingConn::Wye
                            || !w.v_ref.is_finite()
                            || w.v_ref <= 0.0
                            || !w.tap.is_finite()
                            || w.tap <= 0.0
                            || !w.s_rating.is_finite()
                            || w.s_rating <= 0.0
                            || !w.r_pct.is_finite()
                            || w.r_pct < 0.0
                    })
                    || primary.s_rating != secondary.s_rating
                {
                    return Err(fail());
                }
            }
            let [Some(a), Some(b), Some(c)] = ordered else {
                return Err(fail());
            };
            let members = [a, b, c];
            let primary = a.windings[0].v_ref * a.windings[0].tap;
            let secondary = a.windings[1].v_ref * a.windings[1].tap * 3.0_f64.sqrt();
            let rating = a.windings[0].s_rating * 3.0;
            let resistance = a.windings[0].r_pct + a.windings[1].r_pct;
            let reactance = a.xsc_pct[0];
            let other = if primary > secondary {
                ["3", "1", "2"]
            } else {
                ["2", "3", "1"]
            };
            if [primary, secondary, rating, resistance, reactance]
                .iter()
                .any(|v| !v.is_finite() || *v <= 0.0)
                || members.iter().enumerate().any(|(phase, t)| {
                    t.windings[0].terminal_map[1] != other[phase]
                        || t.windings[0].v_ref * t.windings[0].tap != primary
                        || t.windings[1].v_ref * t.windings[1].tap * 3.0_f64.sqrt() != secondary
                        || t.windings[0].s_rating * 3.0 != rating
                        || t.windings[0].r_pct + t.windings[1].r_pct != resistance
                        || t.xsc_pct[0] != reactance
                })
            {
                return Err(fail());
            }
            for member in members {
                plan.leaders.insert(&member.name, &a.name);
            }
            plan.banks.insert(
                &a.name,
                Bank {
                    members,
                    primary,
                    secondary,
                    rating,
                    impedance: Complex64::new(resistance / 100.0, reactance / 100.0),
                },
            );
        }
        Ok(plan)
    }
}
impl Writer<'_> {
    pub(super) fn validate_coil_banks(&self) -> Result<(), Error> {
        if self.ideal_plan.is_none()
            && !self.coil_banks.banks.is_empty()
            && let Some(bus) =
                crate::glm::connections::source_with_zero_sequence(self.net.sources())
        {
            return Err(unsupported(format!(
                "NR DELTA_GWYE output with source zero-sequence voltage at {bus} requires an additional native primary-stamp mapping"
            )));
        }
        for bank in self.coil_banks.banks.values() {
            let mut point = None;
            for member in bank.members {
                if let Some(location) = self
                    .locations
                    .get(&(DistLocationTarget::Transformer, member.name.as_str()))
                {
                    if point.is_some_and(|p: &crate::DistComponentLocation| {
                        p.point != location.point || p.kind != location.kind
                    }) {
                        return Err(unsupported(
                            "grouped delta-grounded-wye coils have distinct geographic points",
                        ));
                    }
                    point = Some(*location);
                }
            }
        }
        Ok(())
    }
    pub(super) fn coil_bank(&mut self, leader: &str) -> Result<(), Error> {
        let bank = &self.coil_banks.banks[leader];
        let members = bank.members;
        let primary = bank.primary;
        let secondary = bank.secondary;
        let rating = bank.rating;
        let impedance = bank.impedance;
        let source_name = members[0].name.strip_suffix("__glm_A").filter(|prefix| {
            members[1].name == format!("{prefix}__glm_B")
                && members[2].name == format!("{prefix}__glm_C")
        });
        let name =
            source_name.map_or_else(|| leader.to_owned(), |prefix| self.generated_name(prefix));
        let config = self.generated_name(&format!("{leader}__configuration"));
        let _ = writeln!(
            self.out,
            "object transformer_configuration {{ name \"{config}\"; connect_type DELTA_GWYE; primary_voltage {primary}; secondary_voltage {secondary}; power_rating {}; impedance {}; }}",
            rating / 1000.0,
            complex_literal(impedance)?
        );
        let _ = writeln!(
            self.out,
            "object transformer {{ name \"{name}\"; phases ABCN; from \"{}\"; to \"{}\"; configuration \"{config}\";",
            members[0].windings[0].bus, members[0].windings[1].bus
        );
        if let Some(member) = members.iter().find(|t| {
            self.locations
                .contains_key(&(DistLocationTarget::Transformer, t.name.as_str()))
        }) {
            self.coordinates(DistLocationTarget::Transformer, &member.name);
        }
        self.out.push_str("}\n");
        for member in members {
            if let Some(point) = self
                .locations
                .get(&(DistLocationTarget::Transformer, member.name.as_str()))
            {
                self.written_locations
                    .insert((DistLocationTarget::Transformer, point.name.as_str()));
            }
            self.diagnostics.push(&crate::diagnostics::codes::EMIT_GLM_VALUE_REBASED,format!("transformer {}: current phase-pair coil becomes a member of native DELTA_GWYE {name}; coincident coordinates remain on the bank and its independent coil identity is grouped",member.name));
        }
        Ok(())
    }
}
