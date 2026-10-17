//! Native neutral-point topology, before conductor-domain circuit lowering.
//!
//! RE/XE and RG/XG are physical ohms, not sequence-network impedances. Shared
//! neutral paths must remain connected; summing every path into a local shunt
//! would lose coupling. The newer circuitry selector remains uninterpreted.

use std::collections::BTreeSet;

use num_complex::Complex64;

use super::{
    format_error,
    schema::{NativeDatabase, require_table},
    semantics::State,
    transformer::{integer, number, reference},
};
use crate::Result;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum NeutralOwner {
    Element(i64),
    Node(i64),
    Shared,
}

pub(super) struct NeutralPointInput {
    pub id: i64,
    pub owner: NeutralOwner,
    pub state: State,
    pub shared_neutral: Option<i64>,
    /// Component values, not an equivalent R+jX until circuitry is resolved.
    pub resistance_ohm: f64,
    pub reactance_ohm: f64,
    pub earth_ohm: Option<Complex64>,
    /// Present in observed schema 14.8, absent from the April 2014 manual.
    /// No value, including NULL or zero, has yet been assigned a circuit.
    pub circuitry_code: Option<i64>,
}

impl NativeDatabase {
    pub fn neutral_point(&self, id: i64) -> Result<NeutralPointInput> {
        require_table(
            &self.connection,
            "NeutralPointImp",
            &["Stp_ID", "Variant_ID", "Flag_CIrcitryZE"],
        )?;
        let mut statement = self
            .connection
            .prepare("SELECT * FROM NeutralPointImp WHERE Stp_ID=?1 AND Variant_ID=?2")
            .map_err(format_error)?;
        let mut rows = statement.query([id, self.variant]).map_err(format_error)?;
        let row = rows.next().map_err(format_error)?.ok_or_else(|| {
            format_error(format!(
                "missing neutral point {id} in variant {}",
                self.variant
            ))
        })?;
        let owner = match integer(row, "Flag_Type")? {
            1 => {
                let element = integer(row, "Element_ID")?;
                if !self.elements.contains_key(&element) {
                    return Err(format_error("neutral point references unknown element"));
                }
                NeutralOwner::Element(element)
            }
            2 => {
                let node = integer(row, "Node_ID")?;
                if !self.nodes.contains(&node) {
                    return Err(format_error("neutral point references unknown node"));
                }
                NeutralOwner::Node(node)
            }
            3 => NeutralOwner::Shared,
            _ => return Err(format_error("unknown neutral point owner kind")),
        };
        let impedance = |r, x| -> Result<Complex64> {
            let resistance = number(row, r)?;
            if resistance < 0.0 {
                return Err(format_error("negative neutral-point resistance"));
            }
            Ok(Complex64::new(resistance, number(row, x)?))
        };
        let neutral_components = impedance("RE", "XE")?;
        let result = NeutralPointInput {
            id,
            owner,
            state: State::decode(integer(row, "Flag_Switch")?)?,
            shared_neutral: reference(row, "ComStp_ID")?,
            resistance_ohm: neutral_components.re,
            reactance_ohm: neutral_components.im,
            earth_ohm: match State::decode(integer(row, "Flag_Ground")?)? {
                State::On => Some(impedance("RG", "XG")?),
                State::Off => None,
            },
            circuitry_code: row.get("Flag_CIrcitryZE").map_err(format_error)?,
        };
        if rows.next().map_err(format_error)?.is_some() {
            return Err(format_error(format!("duplicate neutral point {id}")));
        }
        Ok(result)
    }

    /// Resolve reference identity without choosing a grounding circuit.
    /// The first record is the requested point; later records must be shared
    /// points. Preserve each switch and impedance rather than collapsing them.
    pub fn neutral_chain(&self, id: i64) -> Result<Vec<NeutralPointInput>> {
        let mut visited = BTreeSet::new();
        let mut chain = Vec::new();
        let mut next = Some(id);
        while let Some(id) = next {
            if !visited.insert(id) {
                return Err(format_error("cycle in shared neutral references"));
            }
            if chain.len() >= 64 {
                return Err(format_error("shared neutral reference depth exceeds limit"));
            }
            let point = self.neutral_point(id)?;
            if !chain.is_empty() && point.owner != NeutralOwner::Shared {
                return Err(format_error(
                    "shared neutral reference targets a non-shared point",
                ));
            }
            next = point.shared_neutral;
            chain.push(point);
        }
        Ok(chain)
    }
}
