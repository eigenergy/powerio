//! Steady-state ideal connection lines. General Input Data (April 2014),
//! printed p. 153: Flag_LineTyp=3 joins nodes in the power-flow topology;
//! stored impedances/capacitances apply to dynamics, protection and export.
//! Keep source values for native echo, but never stamp them as a finite line
//! in this electrical profile. A typed switch supplies the exact constraint.

use std::collections::BTreeMap;

use super::{
    format_error,
    schema::{NativeDatabase, require_table},
    semantics::{Connection, State, require_input_categories},
    transformer::{integer, number, reference},
};
use crate::{DistBus, DistSwitch, Result};

impl NativeDatabase {
    pub fn connection_switch(
        &self,
        element: i64,
        buses: &BTreeMap<i64, DistBus>,
    ) -> Result<Option<(DistSwitch, Vec<&'static str>)>> {
        require_table(
            &self.connection,
            "Line",
            &["Element_ID", "Variant_ID", "Flag_LineTyp"],
        )?;
        let mut statement = self
            .connection
            .prepare(
                "SELECT e.Flag_Input AS ElementInput, l.* FROM Line l JOIN Element e
             ON e.Element_ID=l.Element_ID AND e.Variant_ID=l.Variant_ID
             WHERE l.Element_ID=?1 AND l.Variant_ID=?2",
            )
            .map_err(format_error)?;
        let mut rows = statement
            .query([element, self.variant])
            .map_err(format_error)?;
        let row = rows
            .next()
            .map_err(format_error)?
            .ok_or_else(|| format_error("missing line row"))?;
        if integer(row, "Flag_LineTyp")? != 3 {
            return Ok(None);
        }
        let mut defaulted = Vec::new();
        let limit = self.connection_inputs(row, &mut defaulted)?;
        if rows.next().map_err(format_error)?.is_some() {
            return Err(format_error("duplicate ideal-connection line row"));
        }
        require_table(&self.connection, "LineSeg", &["Line_ID", "Variant_ID"])?;
        let segments: bool = self
            .connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM LineSeg WHERE Line_ID=?1 AND Variant_ID=?2)",
                [element, self.variant],
                |r| r.get(0),
            )
            .map_err(format_error)?;
        if segments {
            return Err(format_error(
                "ideal connection with segments requires separate assembly",
            ));
        }
        let ports = self.electrical_terminals(element)?;
        if ports.len() != 2
            || ports[0].position != 1
            || ports[1].position != 2
            || ports.iter().any(|p| p.connection != Connection::L123)
        {
            return Err(format_error(
                "ideal connection requires two full three-phase ports",
            ));
        }
        let phases = ["1", "2", "3"].map(str::to_owned).to_vec();
        for port in &ports {
            let bus = buses
                .get(&port.node)
                .ok_or_else(|| format_error("ideal connection bus missing"))?;
            if bus.id != port.node.to_string() || phases.iter().any(|p| !bus.terminals.contains(p))
            {
                return Err(format_error("ideal connection bus or conductor mismatch"));
            }
        }
        let in_service = self.element_state(element)? == State::On;
        let terminal_closed = ports
            .iter()
            .map(|p| p.state == State::On)
            .collect::<Vec<_>>();
        let mut switch = DistSwitch::new(
            element.to_string(),
            ports[0].node.to_string(),
            ports[1].node.to_string(),
            phases.clone(),
            phases,
            !in_service || terminal_closed.contains(&false),
        );
        switch.i_max = (limit > 0.0).then(|| vec![limit; 3]);
        switch.extras.insert(
            "sincal_connection".into(),
            serde_json::json!({
                "element":element, "terminal_ids":ports.iter().map(|p|p.id).collect::<Vec<_>>(),
                "terminal_closed":terminal_closed, "in_service":in_service,
                "profile":"steady_state_ideal_connection",
            }),
        );
        Ok(Some((switch, defaulted)))
    }
    fn connection_inputs(
        &self,
        row: &rusqlite::Row<'_>,
        defaulted: &mut Vec<&'static str>,
    ) -> Result<f64> {
        require_input_categories(integer(row, "ElementInput")?, 2)?;
        Self::materialized_type(row)?;
        for field in ["Flag_Ll", "Flag_Ground", "Flag_Macro"] {
            if self.line_optional_flag(row, field, defaulted)? != 0 {
                return Err(format_error(format!(
                    "ideal connection requires resolution of {field}"
                )));
            }
        }
        if self.newer_integer(row, "Flag_Lf", 1)? != 1 {
            return Err(format_error(
                "ideal connection requires the steady-state line profile",
            ));
        }
        for field in ["Macro_ID", "LineTemp_ID", "ElemLoading_ID"] {
            if self.newer_reference(row, field)?.is_some() {
                return Err(format_error(format!(
                    "ideal connection requires resolution of {field}"
                )));
            }
        }
        if reference(row, "CoupData_ID")?.is_some() {
            return Err(format_error(
                "ideal connection has unresolved coupling data",
            ));
        }
        let amperes = number(row, "Ith")? * 1000.0;
        let parallel = self.line_optional_number(row, "ParSys", defaulted)?;
        let reduction = self.line_optional_number(row, "fr", defaulted)?;
        let limit = amperes * parallel * reduction;
        if amperes < 0.0
            || parallel <= 0.0
            || reduction <= 0.0
            || !limit.is_finite()
            || (amperes > 0.0 && limit <= 0.0)
        {
            return Err(format_error("invalid ideal-connection current rating"));
        }
        Ok(limit)
    }
}
