//! Native two-winding connection and fixed-tap inputs.
//!
//! The April 2014 Load Flow manual, printed pp. 34–35, defines coil pairing.
//! These are conductor incidence rows, not positive-sequence phase shifters.
//! Leakage, magnetizing/zero-sequence circuits and grounding still need mapping.

use super::{
    format_error,
    schema::{NativeDatabase, require_table},
    semantics::{ElectricalTerminal, State, require_input_categories},
};
use crate::Result;
use rusqlite::Row;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum WindingKind {
    Delta,
    /// Native N marking enables solid or impedance grounding; the neutral
    /// point reference must still be resolved before constructing a circuit.
    Wye {
        grounding_enabled: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct VectorGroup {
    pub primary: WindingKind,
    pub secondary: WindingKind,
    pub clock: u8,
    /// Conductive connection between the two sides; never an isolated YY/DD.
    pub autotransformer: bool,
}

/// Incidence over A, B, C and the winding star point, in that order. The
/// fourth coordinate does not imply that a star point is exposed or grounded.
pub(super) type CoilIncidence = [i8; 4];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct CoilPair {
    pub winding: usize,
    pub primary: CoilIncidence,
    pub secondary: CoilIncidence,
}

impl VectorGroup {
    pub fn decode(code: i64) -> Result<Self> {
        use WindingKind::{Delta as D, Wye};
        const Y: WindingKind = Wye {
            grounding_enabled: false,
        };
        const YN: WindingKind = Wye {
            grounding_enabled: true,
        };
        let autotransformer = matches!(code, 71..=73);
        let ordinary_code = match code {
            71 => 6,
            72 => 5,
            73 => 1,
            _ => code,
        };
        let (primary, secondary, clock) = match ordinary_code {
            1 => (D, D, 0),
            4 => (YN, Y, 0),
            5 => (YN, YN, 0),
            6 => (Y, Y, 0),
            7 => (Y, YN, 0),
            10 => (D, YN, 1),
            13 => (Y, D, 1),
            14 => (YN, D, 1),
            23 => (D, Y, 5),
            24 => (D, YN, 5),
            25 => (Y, D, 5),
            26 => (YN, D, 5),
            35 => (D, D, 6),
            38 => (YN, Y, 6),
            39 => (YN, YN, 6),
            40 => (Y, Y, 6),
            41 => (Y, YN, 6),
            44 => (D, Y, 7),
            45 => (D, YN, 7),
            48 => (Y, D, 7),
            49 => (YN, D, 7),
            58 => (D, Y, 11),
            59 => (D, YN, 11),
            60 => (Y, D, 11),
            61 => (YN, D, 11),
            70 => (D, Y, 1),
            _ => {
                return Err(format_error(format!(
                    "unsupported transformer vector group {code}"
                )));
            }
        };
        Ok(Self {
            primary,
            secondary,
            clock,
            autotransformer,
        })
    }

    /// Pair actual coils. Delta connections cancel zero-sequence voltage;
    /// real incidence gives opposite clock rotation for negative sequence.
    pub fn coils(self, selected: &[usize]) -> Result<Vec<CoilPair>> {
        use WindingKind::{Delta, Wye};
        let mut result = Vec::new();
        for &winding in selected {
            if winding >= 3 || result.iter().any(|c: &CoilPair| c.winding == winding) {
                return Err(format_error("invalid or duplicate transformer winding"));
            }
            let next = (winding + 1) % 3;
            let previous = (winding + 2) % 3;
            let (primary, secondary, reverse) = match (self.primary, self.secondary, self.clock) {
                (Wye { .. }, Wye { .. }, 0 | 6) => (
                    incidence(winding, 3),
                    incidence(winding, 3),
                    self.clock == 6,
                ),
                (Delta, Delta, 0 | 6) => (
                    incidence(winding, next),
                    incidence(winding, next),
                    self.clock == 6,
                ),
                (Wye { .. }, Delta, 1 | 7) => (
                    incidence(winding, 3),
                    incidence(winding, next),
                    self.clock == 7,
                ),
                (Wye { .. }, Delta, 5 | 11) => (
                    incidence(winding, 3),
                    incidence(winding, previous),
                    self.clock == 5,
                ),
                (Delta, Wye { .. }, 1 | 7) => (
                    incidence(winding, next),
                    incidence(next, 3),
                    self.clock == 1,
                ),
                (Delta, Wye { .. }, 5 | 11) => (
                    incidence(winding, next),
                    incidence(winding, 3),
                    self.clock == 5,
                ),
                _ => return Err(format_error("unsupported winding/clock combination")),
            };
            result.push(CoilPair {
                winding,
                primary,
                secondary: secondary.map(|value| if reverse { -value } else { value }),
            });
        }
        if result.is_empty() {
            return Err(format_error("transformer has no active windings"));
        }
        Ok(result)
    }
}

fn incidence(positive: usize, negative: usize) -> CoilIncidence {
    let mut row = [0; 4];
    row[positive] = 1;
    row[negative] = -1;
    row
}

pub(super) struct FixedTapInput {
    /// Zero-based transformer side containing the adjustable winding.
    pub side: usize,
    /// Only installed coils have a position. Inactive columns are not read.
    pub positions: [Option<f64>; 3],
    pub midpoint: f64,
    pub step_fraction: f64,
    pub boost_angle_rad: f64,
    pub rotation_per_step_rad: f64,
}

pub(super) struct TransformerConnectionInput {
    pub element: i64,
    pub ports: Vec<ElectricalTerminal>,
    pub state: State,
    pub vector_group: VectorGroup,
    pub coils: Vec<CoilPair>,
    pub rated_ll_volts: [f64; 2],
    pub rated_va: f64,
    /// Independent native rotation, not already folded into the vector group.
    pub additional_rotation_rad: f64,
    pub neutral_points: [Option<i64>; 2],
    pub tap: FixedTapInput,
}

impl NativeDatabase {
    /// Topology does not resolve regulator state or electrical impedances.
    /// Unsupported operating modes still fail atomic electrical assembly.
    pub fn transformer_topology(
        &self,
        element: i64,
    ) -> Result<(Vec<ElectricalTerminal>, Vec<CoilPair>)> {
        let ports = self.electrical_terminals(element)?;
        if ports.len() != 2
            || ports[0].position != 1
            || ports[1].position != 2
            || ports[0].connection != ports[1].connection
        {
            return Err(format_error(
                "transformer requires two matching winding selections",
            ));
        }
        let selected = ports[0]
            .connection
            .phases()
            .ok_or_else(|| format_error("neutral-only transformer winding selection"))?;
        require_table(
            &self.connection,
            "TwoWindingTransformer",
            &["Element_ID", "Variant_ID", "VecGrp"],
        )?;
        let mut stmt = self
            .connection
            .prepare(
                "SELECT VecGrp FROM TwoWindingTransformer WHERE Element_ID=?1 AND Variant_ID=?2",
            )
            .map_err(format_error)?;
        let mut rows = stmt.query([element, self.variant]).map_err(format_error)?;
        let group = rows
            .next()
            .map_err(format_error)?
            .ok_or_else(|| format_error("missing transformer row"))?
            .get::<_, i64>(0)
            .map_err(format_error)?;
        let coils = VectorGroup::decode(group)?.coils(selected)?;
        if rows.next().map_err(format_error)?.is_some() {
            return Err(format_error("duplicate transformer row"));
        }
        Ok((ports, coils))
    }

    /// Decode the winding connection only, not a complete DistTransformer.
    pub fn transformer_connection(&self, element: i64) -> Result<TransformerConnectionInput> {
        if self.elements.get(&element).map(String::as_str) != Some("TwoWindingTransformer") {
            return Err(format_error(format!(
                "Element {element} is not a TwoWindingTransformer"
            )));
        }
        let ports = self.electrical_terminals(element)?;
        if ports.len() != 2
            || ports[0].position != 1
            || ports[1].position != 2
            || ports[0].connection != ports[1].connection
        {
            return Err(format_error(
                "transformer requires two matching winding selections",
            ));
        }
        // For a transformer this enum denotes W1/W2/W3/W12/.../W123, not
        // its final bus conductor set. YNd1 W1, for example, needs A and B
        // at the secondary even though the stored terminal code is 1.
        let selected = ports[0]
            .connection
            .phases()
            .ok_or_else(|| format_error("neutral-only transformer winding selection"))?;
        require_table(
            &self.connection,
            "TwoWindingTransformer",
            &["Element_ID", "Variant_ID"],
        )?;
        let mut statement = self.connection.prepare(
            "SELECT e.Flag_Input AS ElementInput, t.* FROM TwoWindingTransformer t JOIN Element e
             ON e.Element_ID=t.Element_ID AND e.Variant_ID=t.Variant_ID
             WHERE t.Element_ID=?1 AND t.Variant_ID=?2").map_err(format_error)?;
        let mut rows = statement
            .query([element, self.variant])
            .map_err(format_error)?;
        let row = rows.next().map_err(format_error)?.ok_or_else(|| {
            format_error(format!("missing transformer row for Element {element}"))
        })?;
        require_input_categories(integer(row, "ElementInput")?, 2)?;
        Self::materialized_type(row)?;
        for field in [
            "Macro_ID",
            "MasterElm_ID",
            "TransformerTap_ID",
            "TransformerCon_ID",
        ] {
            if reference(row, field)?.is_some() {
                return Err(format_error(format!(
                    "transformer requires resolution of {field}"
                )));
            }
        }
        require_no_center_tap(row, self)?;
        if self.legacy_integer(row, "Flag_Ct", 0)? != 0 {
            return Err(format_error(
                "center-tapped transformer requires a separate circuit",
            ));
        }
        let vector_group = VectorGroup::decode(integer(row, "VecGrp")?)?;
        let coils = vector_group.coils(selected)?;
        let rated_ll_volts = [
            positive_scaled(row, "Un1", 1000.0)?,
            positive_scaled(row, "Un2", 1000.0)?,
        ];
        let rated_va = positive_scaled(row, "Sn", 1e6)?;
        let additional_rotation_rad = self
            .legacy_transformer_number(row, "AddRotate")?
            .to_radians();
        let neutral_points = [reference(row, "Stp_ID1")?, reference(row, "Stp_ID2")?];
        let tap = fixed_tap(row, selected, self)?;
        if rows.next().map_err(format_error)?.is_some() {
            return Err(format_error(format!(
                "duplicate transformer row for Element {element}"
            )));
        }
        Ok(TransformerConnectionInput {
            element,
            ports,
            state: self.element_state(element)?,
            vector_group,
            coils,
            rated_ll_volts,
            rated_va,
            additional_rotation_rad,
            neutral_points,
            tap,
        })
    }
}

// Database Description (April 2014), printed p.46: Flag_Ct defaults to
// inactive. The schema-11.5 legacy profile admits an explicit NULL only
// when both centre-tap measurements are NULL/zero, as in CSIRO03. A missing
// column, newer schema or nonzero ambiguous centre-tap data still rejects.
fn require_no_center_tap(row: &Row<'_>, db: &NativeDatabase) -> Result<()> {
    if row
        .get::<_, Option<i64>>("Flag_Ct")
        .map_err(format_error)?
        .is_some()
    {
        return Ok(());
    }
    db.legacy_integer(row, "Flag_Ct", 0)?;
    if reference(row, "StpCt_ID")?.is_some() {
        return Err(format_error(
            "NULL Flag_Ct with a centre-tap neutral reference is unresolved",
        ));
    }
    for field in ["uk_Ct", "ur_Ct"] {
        let value: Option<f64> = row.get(field).map_err(format_error)?;
        if value.is_some_and(|value| value != 0.0) {
            return Err(format_error(format!(
                "NULL Flag_Ct requires absent or zero {field}; centre-tap interpretation is unresolved"
            )));
        }
    }
    Ok(())
}

fn fixed_tap(row: &Row<'_>, selected: &[usize], db: &NativeDatabase) -> Result<FixedTapInput> {
    // Database Description (April 2014), p.46: fixed tap status defaults
    // to 1. The schema-11.5 adapter retains a stored NULL and records this
    // interpretation; active controller modes still require resolution.
    if db.legacy_integer(row, "Flag_roh", 1)? != 1 {
        return Err(format_error(
            "transformer regulator requires operating-state resolution",
        ));
    }
    let side = match db.legacy_integer(row, "Flag_ConNode", 1)? {
        1 => 0,
        2 => 1,
        _ => return Err(format_error("unknown transformer tap side")),
    };
    let mut positions = [None; 3];
    let individual = db.legacy_integer(row, "Flag_Tap", 0)?;
    for &coil in selected {
        let field = match individual {
            0 => "roh",
            1 => ["roh1", "roh2", "roh3"][coil],
            _ => {
                return Err(format_error(
                    "unknown individual transformer regulator flag",
                ));
            }
        };
        positions[coil] = Some(db.legacy_transformer_number(row, field)?);
    }
    Ok(FixedTapInput {
        side,
        positions,
        midpoint: db.legacy_transformer_number(row, "rohm")?,
        step_fraction: db.legacy_transformer_number(row, "ukr")? / 100.0,
        boost_angle_rad: db.legacy_transformer_number(row, "alpha")?.to_radians(),
        rotation_per_step_rad: db.legacy_transformer_number(row, "phi")?.to_radians(),
    })
}

pub(super) fn integer(row: &Row<'_>, field: &str) -> Result<i64> {
    row.get(field).map_err(format_error)
}

pub(super) fn number(row: &Row<'_>, field: &str) -> Result<f64> {
    let value: f64 = row.get(field).map_err(format_error)?;
    if !value.is_finite() {
        return Err(format_error(format!("nonfinite {field}")));
    }
    Ok(value)
}

fn positive_scaled(row: &Row<'_>, field: &str, scale: f64) -> Result<f64> {
    let value = number(row, field)? * scale;
    if value <= 0.0 || !value.is_finite() {
        return Err(format_error(format!("invalid transformer {field}")));
    }
    Ok(value)
}

pub(super) fn reference(row: &Row<'_>, field: &str) -> Result<Option<i64>> {
    match row.get(field).map_err(format_error)? {
        None | Some(0) => Ok(None),
        Some(value) if value > 0 => Ok(Some(value)),
        _ => Err(format_error(format!("invalid {field} reference"))),
    }
}
