//! Native FBS series equivalent for independent ideal phase ratios.
use super::{Writer, flags, phase_name, unsupported};
use crate::{DistLocationTarget, DistTransformer, DistWindingConn, MulticonductorNetwork};
use powerio_core::Error;
use std::collections::BTreeMap;
use std::fmt::Write as _;

#[derive(Default)]
pub(super) struct Regulators<'a> {
    pub groups: BTreeMap<&'a str, Group<'a>>,
    pub leaders: BTreeMap<&'a str, &'a str>,
}
pub(super) struct Group<'a> {
    pub from: &'a str,
    pub to: &'a str,
    pub members: Vec<&'a DistTransformer>,
}

pub(super) fn phase(t: &DistTransformer) -> Result<usize, Error> {
    Ok(super::phases(&t.windings[0].terminal_map[..1])?[0])
}
fn ratio(t: &DistTransformer) -> f64 {
    let a = &t.windings[0];
    let b = &t.windings[1];
    a.v_ref * a.tap / (b.v_ref * b.tap)
}
impl<'a> Regulators<'a> {
    pub fn discover(net: &'a MulticonductorNetwork) -> Result<Self, Error> {
        let mut candidates: BTreeMap<(&str, &str), Vec<&DistTransformer>> = BTreeMap::new();
        for t in net.transformers() {
            if t.phases != 1
                || t.windings.len() != 2
                || t.xsc_pct != [0.0]
                || t.windings.iter().any(|w| w.r_pct != 0.0)
            {
                continue;
            }
            if t.extras.iter().any(|(k, v)| match k.as_str() {
                "ppm_antifloat" => v.as_f64() != Some(0.0),
                "rating_is_normalization_base" => v.as_bool() != Some(true),
                _ => true,
            }) || t.windings.iter().any(|w| {
                w.conn != DistWindingConn::Wye
                    || w.terminal_map.len() != 2
                    || w.terminal_map[1] != "n"
                    || !w.v_ref.is_finite()
                    || w.v_ref <= 0.0
                    || !w.tap.is_finite()
                    || w.tap <= 0.0
                    || !w.s_rating.is_finite()
                    || w.s_rating <= 0.0
                    || w.r_neutral.is_some_and(|r| r != 0.0)
                    || w.x_neutral.is_some_and(|x| x != 0.0)
            }) || t.windings[0].terminal_map != t.windings[1].terminal_map
                || !ratio(t).is_finite()
                || ratio(t) <= 0.0
            {
                return Err(unsupported(format!(
                    "ideal transformer {} needs another phase/neutral/core mapping",
                    t.name
                )));
            }
            phase(t)?;
            candidates
                .entry((&t.windings[0].bus, &t.windings[1].bus))
                .or_default()
                .push(t);
        }
        let mut plan = Self::default();
        for ((from, to), mut members) in candidates {
            members.sort_by_key(|t| t.windings[0].terminal_map[0].as_str());
            if members
                .windows(2)
                .any(|pair| pair[0].windings[0].terminal_map == pair[1].windings[0].terminal_map)
            {
                return Err(unsupported(
                    "parallel ideal transformers on the same phase need another exact output equivalent",
                ));
            }
            let leader = members[0].name.as_str();
            for t in &members {
                plan.leaders.insert(t.name.as_str(), leader);
            }
            plan.groups.insert(leader, Group { from, to, members });
        }
        Ok(plan)
    }
}
impl Writer<'_> {
    pub(super) fn ideal_regulator_group(&mut self, leader: &str) -> Result<(), Error> {
        let group = &self.regulators.groups[leader];
        let members = group.members.clone();
        let reverse = self
            .ideal_plan
            .as_ref()
            .is_some_and(|p| p.reversed.contains(leader));
        let (from, to) = if reverse {
            (group.to, group.from)
        } else {
            (group.from, group.to)
        };
        let phases = members
            .iter()
            .map(|t| phase(t))
            .collect::<Result<Vec<_>, _>>()?;
        let selected = selected_members(self, &members);
        let mut previous = from.to_owned();
        for (index, t) in selected.iter().enumerate() {
            let next = if index + 1 == selected.len() {
                to.to_owned()
            } else {
                let bus = self.generated_name(&format!("{}__glm_ratio_bus_{index}", t.name));
                let _ = writeln!(
                    self.out,
                    "object node {{ name \"{bus}\"; phases {}N; nominal_voltage {}; maximum_voltage_error 1e-9; }}",
                    flags(&phases),
                    self.nominal[from]
                );
                bus
            };
            let config = self.generated_name(&format!("{}__configuration", t.name));
            let r = if reverse { 1.0 / ratio(t) } else { ratio(t) };
            if !r.is_finite() || r <= 0.0 {
                return Err(unsupported(
                    "reversed ideal ratio is nonfinite or underflows",
                ));
            }
            let (kind, amount) = if r >= 0.5 {
                ("B", 1.0 - r)
            } else {
                ("A", 1.0 / r - 1.0)
            };
            if !amount.is_finite() {
                return Err(unsupported(
                    "native ideal regulator ratio settings overflow",
                ));
            }
            let tap = if amount > 0.0 {
                1
            } else if amount < 0.0 {
                -1
            } else {
                0
            };
            let _ = writeln!(
                self.out,
                "object regulator_configuration {{ name \"{config}\"; connect_type WYE_WYE; Control MANUAL; Type {kind}; raise_taps 1; lower_taps 1; regulation {};",
                amount.abs()
            );
            let active = phase(t)?;
            for &p in &phases {
                let _ = writeln!(
                    self.out,
                    "tap_pos_{} {};",
                    phase_name(p),
                    if p == active { tap } else { 0 }
                );
            }
            let _ = writeln!(
                self.out,
                "}}\nobject regulator {{ name \"{}\"; phases {}N; from \"{previous}\"; to \"{next}\"; configuration \"{config}\"; regulator_resistance 0;",
                t.name,
                flags(&phases)
            );
            self.coordinates(DistLocationTarget::Transformer, &t.name);
            self.out.push_str("}\n");
            previous = next;
        }
        for t in members {
            self.written_locations
                .insert((DistLocationTarget::Transformer, t.name.as_str()));
            if !t.extras.contains_key("rating_is_normalization_base") {
                self.dropped(format!("transformer {}: native regulator configurations have no winding power-rating field; declared winding ratings are not emitted",t.name));
            }
            self.diagnostics.push(&crate::diagnostics::codes::EMIT_GLM_VALUE_REBASED,format!("transformer {}: ideal phase ratio becomes a source-oriented series regulator stage; other bank phases have exact unity pass-through; redundant unity coil identities are grouped, and physical bank ports exclude internal stage buses",t.name));
        }
        Ok(())
    }
}

fn selected_members<'a>(
    writer: &Writer<'_>,
    members: &[&'a DistTransformer],
) -> Vec<&'a DistTransformer> {
    let mut selected: Vec<_> = members
        .iter()
        .copied()
        .filter(|t| ratio(t).to_bits() != 1.0_f64.to_bits())
        .collect();
    if selected.is_empty() {
        selected.push(members[0]);
    }
    // A native series stage carries every bank phase. Keep additional unit
    // stages only when needed to retain a distinct point attachment.
    for t in members {
        if selected.iter().any(|s| s.name == t.name) {
            continue;
        }
        if let Some(point) = writer
            .locations
            .get(&(DistLocationTarget::Transformer, t.name.as_str()))
            && !selected.iter().any(|s| {
                writer
                    .locations
                    .get(&(DistLocationTarget::Transformer, s.name.as_str()))
                    .is_some_and(|p| p.point == point.point && p.kind == point.kind)
            })
        {
            selected.push(t);
        }
    }
    selected
}
