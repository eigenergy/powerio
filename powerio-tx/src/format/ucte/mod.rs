//! Parse and emit UCTE-DEF `.uct` (revisions 2003.09.01 and 2007.05.01).
//!
//! UCTE-DEF is the ENTSO-E (formerly UCTE) exchange format for load flow and
//! short circuit studies of the continental European transmission grid. A file
//! is US-ASCII text made of seven fixed column blocks, each introduced by a
//! `##` key line: `##C` comments headed by the revision date, `##N` nodes in
//! `##Z<cc>` country groups, `##L` lines, `##T` two winding transformers,
//! `##R` their regulation, `##TT` their special description, and `##E`
//! scheduled exchange powers. Both revisions share the column layout PowSybl
//! Core's `UcteRecordParser` reads; a 2003.09.01 file leaves the element name
//! columns blank. Fresh output uses revision 2007.05.01.
//!
//! Units are physical: ohm, microsiemens, kV, MW, MVAr, and ampere. UCTE-DEF
//! states no system base, so the balanced view uses 100 MVA, and the
//! synchronous area runs at 50 Hz. Generation in a node record follows the
//! load sign convention (an injection is negative), and the permissible
//! generation limits carry the same sign, so the reader negates all six.
//!
//! The mapping: a node is a bus named by its 8 character node code with a bus
//! id equal to its position in the node block; a country is an area, and the
//! fictitious cross border nodes (country letter `X`) form their own area so
//! a tie line keeps both ends. A node's load and generation become one load
//! and one generator. A line is a branch on the node voltage level base; a
//! busbar coupler (status 2 or 7) is a switch. A transformer is a branch from
//! the regulated winding (node 2) to the non regulated winding (node 1), with
//! the rated voltages and any `##R` phase and angle regulation folded into
//! the tap ratio and phase shift, and the regulation itself kept in the
//! branch extras so fresh UCTE output replays it. `##TT` and `##E` records
//! stay in the retained source; `##TT` rows also ride on the transformer
//! extras. Same format emission reproduces the retained input through
//! [`crate::emit`]; this module's writer builds a document from the typed
//! network.

use std::collections::{BTreeSet, HashMap};

use serde_json::{Value, json};

use super::{TextEmission, warn_extra_branch_rating_sets};
use crate::diagnostics::codes::EMIT_UCTE as F;
use crate::diagnostics::{DiagnosticInfo, Diagnostics, codes};
use crate::network::{
    Area, BalancedNetwork, Branch, BranchCharging, Bus, BusId, BusType, Extras, Generator,
    GeneratorEnergySource, Load, SourceFormat, Switch, TransformerControl, TransformerControlMode,
};
use crate::{Error, Result};

mod read;
mod write;

pub(crate) use read::parse_ucte_source;
pub(crate) use write::write_ucte;

const FMT: &str = "UCTE-DEF .uct";
/// The revision fresh output declares.
pub(crate) const REVISION: &str = "2007.05.01";
const REVISIONS: [&str; 2] = ["2003.09.01", "2007.05.01"];
/// The balanced view's system base; UCTE-DEF states physical units only.
const BASE_MVA: f64 = 100.0;
/// The continental European synchronous area frequency.
const BASE_FREQUENCY: f64 = 50.0;
/// The reactance floor PowSybl applies to a real or equivalent element, in
/// ohm; a smaller reactance reads as this value with the same sign.
const MIN_REACTANCE_OHM: f64 = 0.05;
/// The generation limit a node record leaves unstated, in MW or MVAr.
const DEFAULT_POWER_LIMIT: f64 = 9999.0;
/// The nominal voltage of each voltage level code, indexed by the code digit.
const VOLTAGE_LEVELS_KV: [f64; 10] = [
    750.0, 380.0, 220.0, 150.0, 120.0, 110.0, 70.0, 27.0, 330.0, 500.0,
];
/// Country letter and ISO 3166-1 alpha-2 code, in PowSybl's table order.
const COUNTRIES: [(char, &str); 37] = [
    ('O', "AT"),
    ('A', "AL"),
    ('B', "BE"),
    ('V', "BG"),
    ('W', "BA"),
    ('3', "BY"),
    ('S', "CH"),
    ('C', "CZ"),
    ('D', "DE"),
    ('K', "DK"),
    ('E', "ES"),
    ('F', "FR"),
    ('5', "GB"),
    ('G', "GR"),
    ('M', "HU"),
    ('H', "HR"),
    ('I', "IT"),
    ('1', "LU"),
    ('6', "LT"),
    ('2', "MA"),
    ('7', "MD"),
    ('Y', "MK"),
    ('9', "NO"),
    ('N', "NL"),
    ('P', "PT"),
    ('Z', "PL"),
    ('R', "RO"),
    ('4', "RU"),
    ('8', "SE"),
    ('Q', "SK"),
    ('L', "SI"),
    ('T', "TR"),
    ('U', "UA"),
    ('0', "ME"),
    ('J', "RS"),
    ('_', "KS"),
    ('X', "XX"),
];
/// The area type of the country areas.
const CONTROL_AREA: &str = "ControlArea";
/// The area type of the cross border node area.
const CROSS_BORDER_AREA: &str = "CrossBorder";
/// The order codes an element id may carry, in the order fresh output tries
/// them for parallel elements.
const ORDER_CODES: &str = "123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ0_-.";
/// The busbar characters fresh output tries when a derived node code collides.
const BUSBARS: &str = "123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";
const PLANT_TYPES: &str = "HNLCGOWF";
const SQRT_3: f64 = 1.732_050_807_568_877_2;

fn country_iso(letter: char) -> Option<&'static str> {
    COUNTRIES
        .iter()
        .find(|(code, _)| *code == letter)
        .map(|(_, iso)| *iso)
}

fn country_letter(iso: &str) -> Option<char> {
    COUNTRIES
        .iter()
        .find(|(_, code)| code.eq_ignore_ascii_case(iso))
        .map(|(letter, _)| *letter)
}

/// The voltage level code whose nominal voltage is nearest `kv`, the rule
/// PowSybl's `UcteVoltageLevelCode.voltageLevelCodeFromVoltage` applies.
fn voltage_level_code(kv: f64) -> usize {
    if kv < 27.0 {
        return 7;
    }
    if kv > 750.0 {
        return 0;
    }
    let mut best = 0;
    for (index, level) in VOLTAGE_LEVELS_KV.iter().enumerate() {
        if (kv - level).abs() < (kv - VOLTAGE_LEVELS_KV[best]).abs() {
            best = index;
        }
    }
    best
}

/// One 8 character UCTE node code: country letter, five character
/// geographical spot, voltage level digit, and busbar character.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct NodeCode([char; 8]);

impl NodeCode {
    fn parse(text: &str) -> Option<Self> {
        let chars: Vec<char> = text.chars().collect();
        let code = Self(chars.try_into().ok()?);
        (country_iso(code.country()).is_some() && code.0[6].is_ascii_digit()).then_some(code)
    }

    fn country(self) -> char {
        self.0[0]
    }

    fn level(self) -> usize {
        usize::from(self.0[6] as u8 - b'0')
    }

    fn base_kv(self) -> f64 {
        VOLTAGE_LEVELS_KV[self.level()]
    }

    fn is_cross_border(self) -> bool {
        self.country() == 'X'
    }

    fn text(self) -> String {
        self.0.iter().collect()
    }
}

/// A line, transformer, or regulation identity: two node codes and an order
/// code.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct ElementId {
    node1: NodeCode,
    node2: NodeCode,
    order: char,
}

impl ElementId {
    fn text(self) -> String {
        format!("{} {} {}", self.node1.text(), self.node2.text(), self.order)
    }
}

#[derive(Clone, Debug, PartialEq)]
struct PhaseRegulation {
    du: f64,
    n: i64,
    np: i64,
    u: Option<f64>,
}

#[derive(Clone, Debug, PartialEq)]
struct AngleRegulation {
    du: f64,
    theta: f64,
    n: i64,
    np: i64,
    p: Option<f64>,
    symmetrical: bool,
    /// Whether the record states its type; a blank type reads as ASYM.
    type_stated: bool,
}

impl AngleRegulation {
    /// The voltage ratio and the phase angle in degrees at tap `position`, as
    /// PowSybl's importer computes them for a UCTE angle regulation.
    fn rho_alpha(&self, position: i64) -> (f64, f64) {
        let step = position as f64 * self.du / 100.0;
        let dx = step * self.theta.to_radians().cos();
        let dy = step * self.theta.to_radians().sin();
        if self.symmetrical {
            (1.0, 2.0 * (dy / 2.0).atan2(1.0 + dx).to_degrees())
        } else {
            (1.0 / dy.hypot(1.0 + dx), dy.atan2(1.0 + dx).to_degrees())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_codes_follow_the_eight_character_rule() {
        let code = NodeCode::parse("FFNGEN71").unwrap();
        assert_eq!(code.country(), 'F');
        assert_eq!(code.level(), 7);
        assert!((code.base_kv() - 27.0).abs() < f64::EPSILON);
        assert!(!code.is_cross_border());
        assert!(NodeCode::parse("XFRBE_11").unwrap().is_cross_border());
        assert_eq!(NodeCode::parse("ISSUE 21").unwrap().text(), "ISSUE 21");
        assert!(NodeCode::parse("?FNGEN71").is_none());
        assert!(NodeCode::parse("FFNGENA1").is_none());
        assert!(NodeCode::parse("FFNGEN7").is_none());
    }

    #[test]
    fn voltage_levels_map_to_the_nearest_code() {
        assert_eq!(voltage_level_code(400.0), 1);
        assert_eq!(voltage_level_code(230.0), 2);
        assert_eq!(voltage_level_code(16.5), 7);
        assert_eq!(voltage_level_code(800.0), 0);
        assert_eq!(voltage_level_code(345.0), 8);
        assert_eq!(voltage_level_code(500.0), 9);
    }

    #[test]
    fn angle_regulation_matches_the_reference_formulas() {
        let asym = AngleRegulation {
            du: 8.0,
            theta: 180.1,
            n: 13,
            np: 3,
            p: None,
            symmetrical: false,
            type_stated: true,
        };
        let (rho, alpha) = asym.rho_alpha(3);
        let step = 3.0 * 0.08;
        let dx = step * 180.1f64.to_radians().cos();
        let dy = step * 180.1f64.to_radians().sin();
        assert!((rho - 1.0 / dy.hypot(1.0 + dx)).abs() < 1e-12);
        assert!((alpha - dy.atan2(1.0 + dx).to_degrees()).abs() < 1e-12);
        let symm = AngleRegulation {
            symmetrical: true,
            theta: 90.0,
            du: 2.0,
            n: 10,
            np: -2,
            p: None,
            type_stated: true,
        };
        let (rho, alpha) = symm.rho_alpha(-2);
        assert!((rho - 1.0).abs() < f64::EPSILON);
        assert!((alpha - 2.0 * (-0.02f64).atan2(1.0).to_degrees()).abs() < 1e-12);
    }
}
