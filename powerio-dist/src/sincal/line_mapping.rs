//! Phase selection and line-end switching using typed circuit components.
//!
//! An open terminal isolates the native bus from an auxiliary line-end bus.
//! The full pi circuit remains intact, including both charging halves.

use std::collections::BTreeMap;

use super::{
    format_error,
    schema::{NativeDatabase, require_table},
    semantics::{Connection, State},
    sequence::LineOperatingPoint,
    transformer::{integer, number},
};
use crate::{DistBus, DistLine, DistLineCode, DistSwitch, Result};

pub(super) struct LineCircuit {
    pub line: DistLine,
    pub code: DistLineCode,
    pub auxiliary_buses: Vec<DistBus>,
    pub terminal_switches: Vec<DistSwitch>,
    pub frequency_hz: f64,
}

impl NativeDatabase {
    pub fn line_circuit(
        &self,
        element: i64,
        buses: &BTreeMap<i64, DistBus>,
    ) -> Result<LineCircuit> {
        let operating_point = self.line_mapping_context(element)?;
        if self.element_state(element)? != State::On {
            return Err(format_error(
                "out-of-service line requires inactive-equipment retention",
            ));
        }
        let ports = self.electrical_terminals(element)?;
        if ports.len() != 2 || ports[0].position != 1 || ports[1].position != 2 {
            return Err(format_error("line requires ordered start/end ports"));
        }
        let positions = ports[0]
            .connection
            .phases()
            .ok_or_else(|| format_error("neutral-only line requires separate mapping"))?;
        if ports[1].connection != ports[0].connection {
            return Err(format_error(
                "different phase selections at line ends require separate mapping",
            ));
        }
        let phases: Vec<String> = positions.iter().map(|p| (p + 1).to_string()).collect();
        for port in &ports {
            let bus = buses
                .get(&port.node)
                .ok_or_else(|| format_error("line port references missing bus"))?;
            if bus.id != port.node.to_string() || phases.iter().any(|p| !bus.terminals.contains(p))
            {
                return Err(format_error(
                    "line bus identity or conductor declaration does not match its port",
                ));
            }
        }
        let mut decoded = self
            .sequence_line(element)?
            .at_operating_point(&operating_point)?;
        if ports[0].connection != Connection::L123 {
            select_phases(&mut decoded.code, positions)?;
        }
        let mut auxiliary_buses = Vec::new();
        let mut terminal_switches = Vec::new();
        let mut ends = [ports[0].node.to_string(), ports[1].node.to_string()];
        for (index, port) in ports.iter().enumerate() {
            if port.state == State::Off {
                // Native bus IDs are integer strings, so this namespace cannot
                // collide with a native bus. Terminal IDs are variant-unique.
                let internal = format!("sincal:line:{element}:terminal:{}", port.id);
                let provenance = serde_json::json!({"element":element,"terminal":port.id,"port":port.position,"node":port.node});
                let mut bus = DistBus::new(internal.clone(), phases.clone());
                bus.extras
                    .insert("sincal_line_end".into(), provenance.clone());
                auxiliary_buses.push(bus);
                let mut switch = DistSwitch::new(
                    format!("sincal:terminal:{}", port.id),
                    ends[index].clone(),
                    internal.clone(),
                    phases.clone(),
                    phases.clone(),
                    true,
                );
                switch.extras.insert("sincal_line_end".into(), provenance);
                terminal_switches.push(switch);
                ends[index] = internal;
            }
        }
        let mut line = DistLine::new(
            element.to_string(),
            ends[0].clone(),
            ends[1].clone(),
            phases.clone(),
            phases,
            decoded.code.name.clone(),
            decoded.length_m,
        );
        line.extras.insert(
            "sincal".into(),
            serde_json::json!({"element":element,"terminal_ids":[ports[0].id,ports[1].id]}),
        );
        Ok(LineCircuit {
            line,
            code: decoded.code,
            auxiliary_buses,
            terminal_switches,
            frequency_hz: operating_point.frequency_hz,
        })
    }

    fn require_line_mapping_tables(&self) -> Result<()> {
        require_table(&self.connection, "Line", &["Element_ID", "Variant_ID"])?;
        require_table(
            &self.connection,
            "VoltageLevel",
            &[
                "VoltLevel_ID",
                "Variant_ID",
                "Temp_Line",
                "Temp_Cable",
                "Un",
                "f",
                "Flag_Volt",
            ],
        )?;
        require_table(
            &self.connection,
            "CalcParameter",
            &["Variant_ID", "f", "Temp_Cond"],
        )?;
        require_table(&self.connection, "LineSeg", &["Variant_ID", "Line_ID"])?;
        Ok(())
    }

    /// Guard modes/corrections not yet applied by the raw sequence helper.
    /// The 2014 Input Data pp. 152–154 uses the element's voltage-level
    /// temperature and calculation frequency, not a node's initial voltage.
    fn line_mapping_context(&self, element: i64) -> Result<LineOperatingPoint> {
        self.require_line_mapping_tables()?;
        let mut statement = self
            .connection
            .prepare(
                "SELECT v.Temp_Line AS LevelLineTemp, v.Temp_Cable AS LevelCableTemp,
             v.Un AS LevelVoltage, v.f AS LevelFrequency, v.Flag_Volt AS LevelVoltageKind, c.f AS CalcFrequency,
             c.Temp_Cond AS CalcTemperature, l.* FROM Line l JOIN Element e
             ON e.Element_ID=l.Element_ID AND e.Variant_ID=l.Variant_ID
             JOIN VoltageLevel v ON v.VoltLevel_ID=e.VoltLevel_ID AND v.Variant_ID=e.Variant_ID
             JOIN CalcParameter c ON c.Variant_ID=e.Variant_ID
             WHERE l.Element_ID=?1 AND l.Variant_ID=?2",
            )
            .map_err(format_error)?;
        let mut rows = statement
            .query([element, self.variant])
            .map_err(format_error)?;
        let row = rows
            .next()
            .map_err(format_error)?
            .ok_or_else(|| format_error("missing line voltage-level/calculation context"))?;
        let kind = integer(row, "Flag_LineTyp")?;
        self.line_line_voltage_basis(row.get("LevelVoltageKind").map_err(format_error)?)?;
        let temperature = match kind {
            1 => number(row, "LevelCableTemp")?,
            2 => number(row, "LevelLineTemp")?,
            _ => {
                return Err(format_error(
                    "ideal-connection or coupled line needs its own circuit mapping",
                ));
            }
        };
        for field in ["Flag_Ll", "Flag_Ground", "Flag_Macro"] {
            if integer(row, field)? != 0 {
                return Err(format_error(format!("unresolved line model {field}")));
            }
        }
        for field in ["Macro_ID", "LineTemp_ID", "ElemLoading_ID"] {
            if self.newer_reference(row, field)?.is_some() {
                return Err(format_error(format!("unresolved line reference {field}")));
            }
        }
        if self.newer_integer(row, "Flag_Lf", 1)? != 1 {
            return Err(format_error(
                "line load-flow mode requires additional mapping",
            ));
        }
        let frequency = number(row, "CalcFrequency")?;
        if frequency <= 0.0
            || frequency.to_bits() != number(row, "LevelFrequency")?.to_bits()
            || number(row, "CalcTemperature")?.to_bits() != 20.0_f64.to_bits()
        {
            return Err(format_error(
                "calculation temperature override or inconsistent network frequency outside the verified profile",
            ));
        }
        let level_voltage = number(row, "LevelVoltage")?;
        let rated_voltage = number(row, "Un")?;
        if level_voltage <= 0.0 || rated_voltage < level_voltage {
            return Err(format_error(
                "line rated voltage is below its voltage level",
            ));
        }
        let operating_point = corrections(row, temperature, frequency, rated_voltage)?;
        if rows.next().map_err(format_error)?.is_some() {
            return Err(format_error("ambiguous line calculation context"));
        }
        let has_segments: bool = self
            .connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM LineSeg WHERE Line_ID=?1 AND Variant_ID=?2)",
                [element, self.variant],
                |r| r.get(0),
            )
            .map_err(format_error)?;
        if has_segments {
            return Err(format_error("line segments require explicit assembly"));
        }
        Ok(operating_point)
    }
}

/// The 2014 Multiple Faults manual, pp. 8–9, eliminates absent series currents
/// by a Schur complement of the phase admittance. Equivalently, restrict the
/// impedance to the available phases. Nonzero pi shunts need a separate
/// contract: both native CSIRO coupled two-phase cases violate the charging
/// balance of principal-submatrix projection. Only independent phases admit
/// nonzero shunts here.
fn select_phases(code: &mut DistLineCode, phases: &[usize]) -> Result<()> {
    let has_shunt = [&code.g_from, &code.g_to, &code.b_from, &code.b_to]
        .into_iter()
        .flat_map(|matrix| matrix.iter().flatten())
        .any(|value| *value != 0.0);
    for matrix in [
        &mut code.r_series,
        &mut code.x_series,
        &mut code.g_from,
        &mut code.g_to,
        &mut code.b_from,
        &mut code.b_to,
    ] {
        if has_shunt
            && matrix.iter().enumerate().any(|(i, row)| {
                row.iter()
                    .enumerate()
                    .any(|(j, value)| i != j && *value != 0.0)
            })
        {
            return Err(format_error(
                "coupled reduced-phase line with shunts requires verified missing-conductor mapping",
            ));
        }
        *matrix = phases
            .iter()
            .map(|&i| phases.iter().map(|&j| matrix[i][j]).collect())
            .collect();
    }
    for limits in [&mut code.i_max, &mut code.s_max].into_iter().flatten() {
        *limits = phases.iter().map(|&i| limits[i]).collect();
    }
    code.n_conductors = phases.len();
    Ok(())
}

fn corrections(
    row: &rusqlite::Row<'_>,
    temperature: f64,
    frequency: f64,
    rated_voltage: f64,
) -> Result<LineOperatingPoint> {
    let resistance_factor = if temperature.to_bits() == 20.0_f64.to_bits() {
        1.0
    } else {
        1.0 + (temperature - 20.0) * number(row, "alpha")?
    };
    if !resistance_factor.is_finite() || resistance_factor <= 0.0 {
        return Err(format_error("invalid line temperature correction"));
    }
    let losses = number(row, "va")?;
    let parallel = number(row, "ParSys")?;
    // va is kW/km, Un is line-line kV. The manual's full shunt is
    // va*1e-3/Un² S/km, then convert km to metres and apply ParSys.
    let conductance = losses / rated_voltage / rated_voltage * 1e-6 * parallel;
    if losses < 0.0 || !conductance.is_finite() || (losses > 0.0 && conductance <= 0.0) {
        return Err(format_error(
            "invalid or unrepresentable line dielectric losses",
        ));
    }
    Ok(LineOperatingPoint {
        frequency_hz: frequency,
        resistance_factor,
        conductance_s_per_m: conductance,
    })
}
