//! Private native-input evidence and mapping disposition. This never returns
//! a partial network, repairs input, or interprets a successful component as
//! proof that SINCAL accepts a project. Each mapping error is the first guard
//! for that component, while the selected zero-sequence fields are audited
//! independently. This is not an exhaustive audit of every native field.

use std::collections::BTreeMap;

use rusqlite::{OptionalExtension, types::ValueRef};
use serde::Serialize;

use super::{format_error, mapping::CircuitDraft, schema::NativeDatabase};
use crate::Result;

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "state", content = "value", rename_all = "snake_case")]
pub(super) enum ObservedInput {
    MissingTable,
    UnsupportedObject,
    MissingColumn,
    MissingRow,
    AmbiguousRows,
    Null,
    Integer(i64),
    Real(f64),
    Nonfinite,
    InvalidType(&'static str),
}

impl ObservedInput {
    fn number_present(&self) -> bool {
        matches!(self, Self::Integer(_) | Self::Real(_))
    }
}

#[derive(Debug, Serialize)]
pub(super) struct InputFinding {
    /// Native table and column; the containing component supplies element ID
    /// and the report supplies the selected variant.
    pub field: String,
    pub reason: &'static str,
}

#[derive(Debug, Serialize)]
pub(super) struct ZeroSequenceEvidence {
    pub fields: BTreeMap<String, ObservedInput>,
    /// Requirements of the current explicit-input mapping profile. Stored
    /// numbers in an inactive category are not treated as declared inputs.
    pub findings: Vec<InputFinding>,
}

#[derive(Debug, Serialize)]
pub(super) struct ComponentMapping {
    pub element: i64,
    #[serde(rename = "type")]
    pub element_type: String,
    pub component_maps: bool,
    pub first_error: Option<String>,
    pub zero_sequence: Option<ZeroSequenceEvidence>,
}

#[derive(Debug, Serialize)]
pub(super) struct MappingReport {
    pub schema_version: f64,
    pub variant: i64,
    pub components: Vec<ComponentMapping>,
}

impl MappingReport {
    pub fn all_components_map(&self) -> bool {
        self.components
            .iter()
            .all(|component| component.component_maps)
    }
}

impl NativeDatabase {
    /// Global context must decode before components can be evaluated. Once it
    /// does, every element uses the same assembler as `network`, even after
    /// other elements fail. Source bytes and the query-only DB are untouched.
    pub fn mapping_report(&self) -> Result<MappingReport> {
        let context = self.mapping_context()?;
        let mut components = Vec::with_capacity(self.elements.len());
        for (&element, kind) in &self.elements {
            let mut scratch = CircuitDraft::new(context.frequency);
            let result = self.map_component(element, &context, &mut scratch);
            components.push(ComponentMapping {
                element,
                element_type: kind.clone(),
                component_maps: result.is_ok(),
                first_error: result.err().map(|error| error.to_string()),
                zero_sequence: self.zero_sequence_evidence(element, kind)?,
            });
        }
        Ok(MappingReport {
            schema_version: self.version,
            variant: self.variant,
            components,
        })
    }

    fn zero_sequence_evidence(
        &self,
        element: i64,
        kind: &str,
    ) -> Result<Option<ZeroSequenceEvidence>> {
        let (table, parameters): (&str, &[&str]) = match kind {
            "Load" => ("Load", &["Z0_Z1", "R0_X0", "R0", "X0"]),
            "Line" => ("Line", &["R0_R1", "X0_X1", "r0", "x0", "c0"]),
            _ => return Ok(None),
        };
        let mut fields = self.observed_fields("Element", element, &["Flag_Input"])?;
        fields.extend(self.observed_fields(table, element, &["Flag_Z0_Input"])?);
        fields.extend(self.observed_fields(table, element, parameters)?);
        let mut findings = Vec::new();
        match fields["Element.Flag_Input"] {
            ObservedInput::Integer(value) if value >= 0 && value & 4 != 0 => {
                let selector = format!("{table}.Flag_Z0_Input");
                let required: &[&str] = match (&fields[&selector], table) {
                    (ObservedInput::Integer(1), "Load") => &["Z0_Z1", "R0_X0"],
                    (ObservedInput::Integer(2), "Load") => &["R0", "X0"],
                    (ObservedInput::Integer(3), "Load") => &[],
                    (ObservedInput::Integer(1), "Line") => &["R0_R1", "X0_X1", "c0"],
                    (ObservedInput::Integer(2), "Line") => &["r0", "x0", "c0"],
                    _ => {
                        findings.push(InputFinding { field: selector,
                            reason: "active zero-sequence category requires a supported integer selector" });
                        &[]
                    }
                };
                for field in required {
                    let field = format!("{table}.{field}");
                    if !fields[&field].number_present() {
                        findings.push(InputFinding { field,
                            reason: "selected zero-sequence mode requires an explicit finite numeric input" });
                    }
                }
            }
            ObservedInput::Integer(value) if value >= 0 => findings.push(InputFinding {
                field: "Element.Flag_Input".into(),
                reason: "zero-sequence category bit 0x4 is absent; stored values are inactive",
            }),
            _ => findings.push(InputFinding {
                field: "Element.Flag_Input".into(),
                reason: "input categories require a nonnegative integer",
            }),
        }
        Ok(Some(ZeroSequenceEvidence { fields, findings }))
    }

    /// Table and field names are internal constants, never native SQL text.
    fn observed_fields(
        &self,
        table: &str,
        element: i64,
        fields: &[&str],
    ) -> Result<BTreeMap<String, ObservedInput>> {
        let kind: Option<String> = self
            .connection
            .query_row(
                "SELECT type FROM pragma_table_list WHERE schema='main' AND name=?1 COLLATE NOCASE",
                [table],
                |row| row.get(0),
            )
            .optional()
            .map_err(format_error)?;
        let fill = |value: ObservedInput| {
            fields
                .iter()
                .map(|field| (format!("{table}.{field}"), value.clone()))
                .collect()
        };
        match kind.as_deref() {
            None => return Ok(fill(ObservedInput::MissingTable)),
            Some("table") => {}
            // Views and virtual/shadow tables are not native schema records.
            // Do not execute them while gathering evidence after a rejection.
            Some(_) => return Ok(fill(ObservedInput::UnsupportedObject)),
        }
        let mut statement = self
            .connection
            .prepare(&format!(
                "SELECT * FROM {table} WHERE Element_ID=?1 AND Variant_ID=?2"
            ))
            .map_err(format_error)?;
        let indices: Vec<_> = fields
            .iter()
            .map(|field| statement.column_index(field).ok())
            .collect();
        let mut rows = statement
            .query([element, self.variant])
            .map_err(format_error)?;
        let Some(row) = rows.next().map_err(format_error)? else {
            return Ok(fill(ObservedInput::MissingRow));
        };
        let mut output = BTreeMap::new();
        for (field, index) in fields.iter().zip(indices) {
            let value = if let Some(index) = index {
                match row.get_ref(index).map_err(format_error)? {
                    ValueRef::Null => ObservedInput::Null,
                    ValueRef::Integer(value) => ObservedInput::Integer(value),
                    ValueRef::Real(value) if value.is_finite() => ObservedInput::Real(value),
                    ValueRef::Real(_) => ObservedInput::Nonfinite,
                    ValueRef::Text(_) => ObservedInput::InvalidType("text"),
                    ValueRef::Blob(_) => ObservedInput::InvalidType("blob"),
                }
            } else {
                ObservedInput::MissingColumn
            };
            output.insert(format!("{table}.{field}"), value);
        }
        if rows.next().map_err(format_error)?.is_some() {
            return Ok(fill(ObservedInput::AmbiguousRows));
        }
        Ok(output)
    }
}
