//! Source-free, explicitly multiconductor candidate authoring. Native desktop
//! acceptance remains unverified. Never calls the balanced backend.

use super::write_topology::Topology;
use crate::{MulticonductorNetwork, diagnostics::codes};
use num_complex::Complex64;
use powerio_core::{Diagnostic, Error};
use powerio_sincal::{
    DatabaseSnapshot,
    authoring::{ColumnKind, InputDatabase},
};
use rusqlite::types::Value;
use std::collections::BTreeMap;

pub(super) type Result<T> = std::result::Result<T, Error>;
pub(super) type Row = BTreeMap<&'static str, Value>;

/// Nominal native voltage levels are explicit authoring input. The generic
/// distribution model stores SI electrical values, not a bus nominal voltage.
#[derive(Default)]
pub struct ExperimentalMulticonductorOptions {
    /// One finite positive line-line voltage (volts) for every typed bus.
    pub nominal_ll_volts: BTreeMap<String, f64>,
}

pub struct ExperimentalMulticonductorOutput {
    pub database: Vec<u8>,
    pub diagnostics: Vec<Diagnostic>,
    /// Original bus IDs to the freshly assigned native node IDs. Closed
    /// ideal switches can make this map many-to-one; phases remain distinct.
    pub bus_ids: BTreeMap<String, i64>,
}

pub(super) fn error(message: impl std::fmt::Display) -> Error {
    Error::new(
        &codes::EMIT_SINCAL_MULTICONDUCTOR_UNSUPPORTED,
        message.to_string(),
    )
}
pub(super) fn row(inactive: &'static str) -> Row {
    let mut row = Row::from([("Variant_ID", Value::Integer(1))]);
    for name in inactive.split_ascii_whitespace() {
        row.insert(name, Value::Integer(0));
    }
    row
}
pub(super) fn integer(row: &mut Row, name: &'static str, value: i64) {
    row.insert(name, Value::Integer(value));
}
pub(super) fn real(row: &mut Row, name: &'static str, value: f64) {
    row.insert(name, Value::Real(value));
}
pub(super) fn text(row: &mut Row, name: &'static str, value: &str) {
    row.insert(name, Value::Text(value.to_owned()));
}

pub(super) struct ExpectedLoad {
    pub id: i64,
    pub p: f64,
    pub q: f64,
    pub voltage: f64,
    pub mode: i64,
    pub node: i64,
    pub phases: Vec<String>,
}

#[derive(Default)]
pub(super) struct Tables {
    rows: BTreeMap<&'static str, Vec<Row>>,
    pub loads: Vec<ExpectedLoad>,
    pub sources: Vec<(i64, usize, bool)>,
    pub lines: Vec<(i64, usize)>,
}
impl Tables {
    pub fn push(&mut self, name: &'static str, row: Row) {
        self.rows.entry(name).or_default().push(row);
    }
    pub fn element(
        &mut self,
        kind: &'static str,
        mut input: Row,
        ports: &[(i64, i64)],
        name: &str,
    ) -> Result<i64> {
        let id = i64::try_from(self.rows.get("Element").map_or(0, Vec::len) + 1).map_err(error)?;
        integer(&mut input, "Element_ID", id);
        self.push(kind, input);
        let mut element = row("");
        integer(&mut element, "Element_ID", id);
        integer(&mut element, "Flag_Input", 6);
        integer(&mut element, "Flag_State", 1);
        integer(&mut element, "VoltLevel_ID", ports[0].0);
        text(&mut element, "Name", name);
        text(&mut element, "Type", kind);
        self.push("Element", element);
        for (position, &(node, connection)) in ports.iter().enumerate() {
            let mut port = row("");
            let terminal =
                i64::try_from(self.rows.get("Terminal").map_or(0, Vec::len) + 1).map_err(error)?;
            for (key, value) in [
                ("Terminal_ID", terminal),
                ("Element_ID", id),
                ("Node_ID", node),
                ("TerminalNo", i64::try_from(position + 1).map_err(error)?),
                ("Flag_Terminal", connection),
                ("Flag_State", 1),
            ] {
                integer(&mut port, key, value);
            }
            self.push("Terminal", port);
        }
        Ok(id)
    }
    fn database(&self) -> Result<Vec<u8>> {
        let mut db = InputDatabase::new().map_err(error)?;
        for (name, columns) in [
            (
                "LineSeg",
                vec![
                    ("Line_ID", ColumnKind::Integer),
                    ("Variant_ID", ColumnKind::Integer),
                ],
            ),
            (
                "Element",
                vec![
                    ("Element_ID", ColumnKind::Integer),
                    ("Variant_ID", ColumnKind::Integer),
                    ("Type", ColumnKind::Text),
                ],
            ),
            (
                "Terminal",
                vec![
                    ("Terminal_ID", ColumnKind::Integer),
                    ("Variant_ID", ColumnKind::Integer),
                    ("Element_ID", ColumnKind::Integer),
                    ("Node_ID", ColumnKind::Integer),
                    ("TerminalNo", ColumnKind::Integer),
                ],
            ),
        ] {
            if !self.rows.contains_key(name) {
                db.create_table(name, &columns).map_err(error)?;
            }
        }
        for (name, rows) in &self.rows {
            let columns = rows[0]
                .iter()
                .map(|(&key, value)| {
                    (
                        key,
                        match value {
                            Value::Integer(_) => ColumnKind::Integer,
                            Value::Real(_) => ColumnKind::Real,
                            Value::Text(_) => ColumnKind::Text,
                            _ => unreachable!("only concrete authored cells"),
                        },
                    )
                })
                .collect::<Vec<_>>();
            db.create_table(name, &columns).map_err(error)?;
            for input in rows {
                if !input.keys().copied().eq(columns.iter().map(|c| c.0)) {
                    return Err(error(format!("inconsistent authored {name} columns")));
                }
                for (field, value) in input {
                    if matches!(value,Value::Real(v) if !v.is_finite()) {
                        return Err(error(format!("{name}.{field}: nonfinite authored value")));
                    }
                }
                db.insert(name, &input.values().cloned().collect::<Vec<_>>())
                    .map_err(error)?;
            }
        }
        db.finish().map_err(error)
    }
}

/// Author a declared multiconductor static subset using explicit native voltage
/// levels. This internal staging API does not enable universal format emission.
///
/// # Errors
/// Invalid typed circuits, unsupported active physics, missing voltage levels,
/// or numerical/topological loss during fresh native write/read.
pub fn write_experimental_multiconductor(
    net: &MulticonductorNetwork,
    options: &ExperimentalMulticonductorOptions,
) -> Result<ExperimentalMulticonductorOutput> {
    crate::require_electrical_readiness(net).map_err(error)?;
    super::write_validate::validate(net)?;
    if net.buses().is_empty() {
        return Err(error("candidate output requires at least one bus"));
    }
    if !net.transformers().is_empty()
        || !net.shunts().is_empty()
        || !net.capacitors().is_empty()
        || !net.generators().is_empty()
        || !net.ibrs().is_empty()
        || !net.control_profiles().is_empty()
        || !net.untyped_objects().is_empty()
    {
        return Err(error(
            "candidate multiconductor writer currently implements ideal sources, sequence-representable lines and static loads; transformer, shunt, generation and control authoring remains unfinished",
        ));
    }
    let topology = Topology::new(net, options)?;
    let mut tables = Tables::default();
    let mut settings = row("");
    integer(&mut settings, "CalcParameter_ID", 1);
    integer(&mut settings, "Flag_LFZ0", 1);
    // Database Description (April 2014), p. 56; these fields are also
    // present in the authentic schema-14.8 catalog. Explicitly request
    // phase-domain unbalanced calculation and retain asymmetric elements.
    integer(&mut settings, "Flag_LFmet", 8);
    integer(&mut settings, "Flag_UsymElm", 3);
    integer(&mut settings, "Flag_DIType", 0);
    integer(&mut settings, "Flag_ABW", 1);
    real(&mut settings, "f", net.base_frequency());
    real(&mut settings, "Temp_Cond", 20.0);
    tables.push("CalcParameter", settings);
    for (&id, (voltage, _)) in &topology.nodes {
        let mut level = row("");
        integer(&mut level, "VoltLevel_ID", id);
        integer(&mut level, "Flag_Volt", 1);
        for (key, value) in [
            ("Un", voltage / 1000.0),
            ("f", net.base_frequency()),
            ("Temp_Line", 20.0),
            ("Temp_Cable", 20.0),
        ] {
            real(&mut level, key, value);
        }
        tables.push("VoltageLevel", level);
        let mut node = row("Stp_ID");
        integer(&mut node, "Node_ID", id);
        integer(&mut node, "VoltLevel_ID", id);
        text(&mut node, "Name", &format!("node-{id}"));
        tables.push("Node", node);
    }
    super::write_equipment::sources(net, &topology, &mut tables)?;
    super::write_equipment::loads(net, &topology, &mut tables)?;
    super::write_equipment::lines(net, &topology, &mut tables)?;
    let database = tables.database()?;
    let snapshot = DatabaseSnapshot::decode(&database, None).map_err(error)?;
    let recovered = super::read_snapshot(snapshot).map_err(error)?;
    crate::require_electrical_readiness(&recovered).map_err(error)?;
    verify(net, &recovered, &topology, &tables)?;
    let diagnostics = vec![
        Diagnostic::of(
            &codes::EMIT_SINCAL_MULTICONDUCTOR_EXPERIMENTAL,
            "Candidate conductor-resolved schema-14.8 output; native SINCAL open/save/calculation is unverified. Nominal voltage levels were supplied explicitly.",
        ),
        Diagnostic::of(
            &codes::EMIT_SINCAL_MULTICONDUCTOR_LOSS,
            "Fresh IDs are assigned; closed ideal switches are collapsed without joining additional conductors; load branches become separate native elements. Metadata, geometry, bounds, costs, switch ampacities, apparent-power ratings and extras are omitted. Native readback may introduce device-local buses and switches.",
        ),
    ];
    Ok(ExperimentalMulticonductorOutput {
        database,
        diagnostics,
        bus_ids: topology.bus_ids,
    })
}

pub(super) fn near(actual: f64, expected: f64, context: &str) -> Result<()> {
    let scale = actual.abs().max(expected.abs());
    if !actual.is_finite()
        || !expected.is_finite()
        || (actual.to_bits() != expected.to_bits()
            && !(actual == 0.0 && expected == 0.0)
            && (actual == 0.0
                || expected == 0.0
                || (actual / scale - expected / scale).abs() > 1e-10))
    {
        return Err(error(format!(
            "{context}: candidate numerical/electrical mismatch {actual} versus {expected}"
        )));
    }
    Ok(())
}

fn verify(
    original: &MulticonductorNetwork,
    recovered: &MulticonductorNetwork,
    topology: &Topology,
    tables: &Tables,
) -> Result<()> {
    if recovered.lines().len() != tables.lines.len()
        || recovered.loads().len() != tables.loads.len()
        || recovered.sources().len() != tables.sources.len()
    {
        return Err(error("candidate changes component counts"));
    }
    for (&id, (_, phases)) in &topology.nodes {
        let bus = recovered
            .bus(&id.to_string())
            .ok_or_else(|| error("missing native node"))?;
        if bus
            .terminals
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            != phases.iter().collect()
        {
            return Err(error(
                "unattached phase terminals require native retention support",
            ));
        }
    }
    verify_loads(recovered, &tables.loads)?;
    for &(id, index, floating) in &tables.sources {
        let source = recovered
            .sources()
            .iter()
            .find(|s| s.name == id.to_string())
            .ok_or_else(|| error("missing authored source"))?;
        let input = &original.sources()[index];
        verify_port(
            recovered,
            &source.bus,
            topology.bus_ids[&input.bus],
            &input.terminal_map,
        )?;
        if source.terminal_map != input.terminal_map
            || source.reference_terminal.is_some() != floating
        {
            return Err(error("candidate changes source reference"));
        }
        for i in 0..3 {
            let a = Complex64::from_polar(source.v_magnitude[i], source.v_angle[i]);
            let b = Complex64::from_polar(input.v_magnitude[i], input.v_angle[i]);
            if (a - b).norm() > 1e-10 * b.norm() {
                return Err(error("candidate changes source phasors"));
            }
        }
    }
    verify_lines(original, recovered, topology, tables)?;
    Ok(())
}

fn verify_port(
    net: &MulticonductorNetwork,
    local: &str,
    node: i64,
    phases: &[String],
) -> Result<()> {
    if !net.switches().iter().any(|s| {
        !s.open
            && s.bus_from == node.to_string()
            && s.bus_to == local
            && s.terminal_map_from == phases
            && s.terminal_map_to == phases
    }) {
        return Err(error(
            "candidate changes a device port's phase connectivity",
        ));
    }
    Ok(())
}

fn verify_loads(net: &MulticonductorNetwork, expected: &[ExpectedLoad]) -> Result<()> {
    let by_name = net
        .loads()
        .iter()
        .map(|l| (l.name.clone(), l))
        .collect::<BTreeMap<_, _>>();
    for input in expected {
        let load = by_name
            .get(&input.id.to_string())
            .ok_or_else(|| error("missing authored load"))?;
        let mode = match load.voltage_model {
            crate::DistLoadVoltageModel::ConstantImpedance { .. } => 1,
            crate::DistLoadVoltageModel::ConstantPower { .. } => 2,
            crate::DistLoadVoltageModel::ConstantCurrent { .. } => 3,
            _ => return Err(error("candidate changes load voltage dependence")),
        };
        let mut terminals = input.phases.clone();
        if terminals.len() == 1 {
            terminals.push("0".into());
            if !net
                .bus(&load.bus)
                .is_some_and(|b| b.grounded.contains(&"0".into()))
            {
                return Err(error("candidate changes load earth connection"));
            }
        }
        if load.p_nom.len() != 1 || load.terminal_map != terminals || mode != input.mode {
            return Err(error("candidate changes load branches/voltage dependence"));
        }
        verify_port(net, &load.bus, input.node, &input.phases)?;
        near(load.p_nom[0], input.p, "load W")?;
        near(load.q_nom[0], input.q, "load var")?;
        near(
            load.voltage_model.v_nom()[0],
            input.voltage,
            "load nominal volts",
        )?;
    }
    Ok(())
}

fn verify_lines(
    original: &MulticonductorNetwork,
    recovered: &MulticonductorNetwork,
    topology: &Topology,
    tables: &Tables,
) -> Result<()> {
    for &(id, index) in &tables.lines {
        let line = recovered
            .lines()
            .iter()
            .find(|l| l.name == id.to_string())
            .ok_or_else(|| error("missing authored line"))?;
        let input = &original.lines()[index];
        let code = original
            .line_codes()
            .iter()
            .find(|c| c.name == input.linecode)
            .unwrap();
        let out = recovered
            .line_codes()
            .iter()
            .find(|c| c.name == line.linecode)
            .unwrap();
        if line.bus_from != topology.bus_ids[&input.bus_from].to_string()
            || line.bus_to != topology.bus_ids[&input.bus_to].to_string()
            || line.terminal_map_from != input.terminal_map_from
            || line.terminal_map_to != input.terminal_map_to
        {
            return Err(error("candidate changes line connectivity"));
        }
        near(line.length, input.length, "line length")?;
        let limits = input.i_max.as_ref().or(code.i_max.as_ref()).unwrap();
        for (a, b) in limits.iter().zip(
            out.i_max
                .as_ref()
                .ok_or_else(|| error("missing authored ampacity"))?,
        ) {
            near(*a, *b, "line ampacity")?;
        }
        for (a, b) in [
            &code.r_series,
            &code.x_series,
            &code.g_from,
            &code.b_from,
            &code.g_to,
            &code.b_to,
        ]
        .into_iter()
        .zip([
            &out.r_series,
            &out.x_series,
            &out.g_from,
            &out.b_from,
            &out.g_to,
            &out.b_to,
        ]) {
            for (a, b) in a.iter().flatten().zip(b.iter().flatten()) {
                near(*a, *b, "line matrix")?;
            }
        }
    }
    Ok(())
}
