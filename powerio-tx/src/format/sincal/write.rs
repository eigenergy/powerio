//! Candidate positive-sequence input authoring. No retained source is consulted.
//! Native desktop acceptance is a separate, still outstanding validation gate.

// Eligibility compares declared controls/parameters exactly: tolerances here
// would silently approximate a different electrical model. Numeric readback
// uses its own explicitly bounded relative error.
#![allow(clippy::float_cmp)]

use super::read_balanced_snapshot;

use crate::{
    diagnostics::codes,
    network::{BalancedNetwork, Branch, BranchCharging, Bus, BusId, BusType, LoadVoltageModel},
};
use powerio_core::{Diagnostic, Error};
use powerio_sincal::{
    DatabaseSnapshot,
    authoring::{ColumnKind, InputDatabase},
};
use rusqlite::types::Value;
use std::collections::BTreeMap;

type Result<T> = std::result::Result<T, Error>;
type Row = BTreeMap<&'static str, Value>;

/// Experimental bytes, accompanied by explicit losses and acceptance limitations.
/// This internal staging API is not the universal format emitter.
pub struct ExperimentalBalancedOutput {
    pub database: Vec<u8>,
    pub diagnostics: Vec<Diagnostic>,
}

fn error(message: impl std::fmt::Display) -> Error {
    Error::new(
        &codes::EMIT_SINCAL_EXPERIMENTAL_UNSUPPORTED,
        message.to_string(),
    )
}
fn loss(diagnostics: &mut Vec<Diagnostic>, message: impl Into<String>) {
    diagnostics.push(Diagnostic::of(
        &codes::EMIT_SINCAL_EXPERIMENTAL_LOSS,
        message,
    ));
}
fn row(inactive: &'static str) -> Row {
    let mut row = Row::from([("Variant_ID", Value::Integer(1))]);
    // These explicitly named fields are inactive controls in this authored
    // profile, never defaults supplied for unknown input data.
    for key in inactive.split_ascii_whitespace() {
        row.insert(key, Value::Integer(0));
    }
    row
}
fn integer(row: &mut Row, key: &'static str, value: i64) {
    row.insert(key, Value::Integer(value));
}
fn real(row: &mut Row, key: &'static str, value: f64) {
    row.insert(key, Value::Real(value));
}
fn text(row: &mut Row, key: &'static str, value: impl Into<String>) {
    row.insert(key, Value::Text(value.into()));
}

#[derive(Default)]
struct Tables(BTreeMap<&'static str, Vec<Row>>);
impl Tables {
    fn push(&mut self, name: &'static str, row: Row) {
        self.0.entry(name).or_default().push(row);
    }
    fn finish(self) -> Result<Vec<u8>> {
        let mut db = InputDatabase::new().map_err(error)?;
        // These are required structural catalogs, even for a bus-only network.
        for (name, columns) in [
            ("NetworkGroup", vec!["Group_ID", "Variant_ID", "Flag_Temp"]),
            ("Element", vec!["Element_ID", "Variant_ID", "Type"]),
            (
                "Terminal",
                vec![
                    "Terminal_ID",
                    "Variant_ID",
                    "Element_ID",
                    "Node_ID",
                    "TerminalNo",
                ],
            ),
        ] {
            if !self.0.contains_key(name) {
                let columns = columns
                    .into_iter()
                    .map(|c| {
                        (
                            c,
                            if c == "Type" {
                                ColumnKind::Text
                            } else {
                                ColumnKind::Integer
                            },
                        )
                    })
                    .collect::<Vec<_>>();
                db.create_table(name, &columns).map_err(error)?;
            }
        }
        for (name, rows) in self.0 {
            let columns = rows[0]
                .iter()
                .map(|(&key, value)| {
                    (
                        key,
                        match value {
                            Value::Integer(_) => ColumnKind::Integer,
                            Value::Real(_) => ColumnKind::Real,
                            Value::Text(_) => ColumnKind::Text,
                            _ => unreachable!("authoring uses concrete typed values"),
                        },
                    )
                })
                .collect::<Vec<_>>();
            db.create_table(name, &columns).map_err(error)?;
            for row in rows {
                for (field, value) in &row {
                    if matches!(value, Value::Real(v) if !v.is_finite()) {
                        return Err(error(format!("{name}.{field}: finite number required")));
                    }
                }
                if !row.keys().copied().eq(columns.iter().map(|(key, _)| *key)) {
                    return Err(error(format!("inconsistent authored {name} columns")));
                }
                db.insert(name, &row.into_values().collect::<Vec<_>>())
                    .map_err(error)?;
            }
        }
        db.finish().map_err(error)
    }
    fn element(
        &mut self,
        kind: &'static str,
        input: Row,
        buses: &[BusId],
        name: &str,
        active: bool,
    ) -> Result<()> {
        let id = i64::try_from(self.0.get("Element").map_or(0, Vec::len) + 1).map_err(error)?;
        let mut input = input;
        integer(&mut input, "Element_ID", id);
        self.push(kind, input);
        let mut element = row("");
        integer(&mut element, "Element_ID", id);
        integer(&mut element, "Flag_Input", 2);
        integer(&mut element, "Flag_State", i64::from(active));
        integer(&mut element, "VoltLevel_ID", bus_id(buses[0])?);
        text(&mut element, "Type", kind);
        text(&mut element, "Name", name);
        self.push("Element", element);
        for (position, &bus) in buses.iter().enumerate() {
            let mut terminal = row("");
            integer(
                &mut terminal,
                "Terminal_ID",
                i64::try_from(self.0.get("Terminal").map_or(0, Vec::len) + 1).map_err(error)?,
            );
            integer(&mut terminal, "Element_ID", id);
            integer(&mut terminal, "Node_ID", bus_id(bus)?);
            integer(
                &mut terminal,
                "TerminalNo",
                i64::try_from(position + 1).map_err(error)?,
            );
            integer(&mut terminal, "Flag_Terminal", 7);
            integer(&mut terminal, "Flag_State", 1);
            integer(&mut terminal, "Flag_Switch", 0);
            self.push("Terminal", terminal);
        }
        Ok(())
    }
}
fn bus_id(bus: BusId) -> Result<i64> {
    let id = i64::try_from(bus.0).map_err(error)?;
    if id <= 0 {
        return Err(error("candidate native Node_ID must be positive"));
    }
    Ok(id)
}

/// Construct a fresh schema-14.8 static balanced input database and validate it
/// through the balanced reader. This does not establish native SINCAL acceptance.
///
/// # Errors
/// Rejects unsupported electrical components/controls and invalid values atomically.
pub fn write_experimental_balanced(net: &BalancedNetwork) -> Result<ExperimentalBalancedOutput> {
    net.check_base_mva().map_err(error)?;
    net.validate().map_err(error)?;
    if net.is_normalized() {
        return Err(error(
            "normalized solver networks require explicit restoration of MW/degrees",
        ));
    }
    if net.buses().is_empty() {
        return Err(error("candidate profile requires at least one bus"));
    }
    if !net.base_frequency().is_finite() || net.base_frequency() <= 0.0 {
        return Err(error("base frequency must be positive and finite"));
    }
    if !net.shunts().is_empty()
        || !net.static_var_compensators().is_empty()
        || !net.storage().is_empty()
        || !net.hvdc().is_empty()
        || !net.transformers_3w().is_empty()
        || !net.switches().is_empty()
    {
        return Err(error(
            "candidate balanced profile supports buses, sources, loads, lines and two-winding transformers; shunts, SVCs, storage, HVDC, three-winding transformers and switches remain unsupported",
        ));
    }
    if net.detailed_connectivity().is_some() {
        return Err(error(
            "detailed connectivity requires a declared topology projection",
        ));
    }
    let mut diagnostics = vec![Diagnostic::of(
        &codes::EMIT_SINCAL_EXPERIMENTAL,
        "Candidate static balanced schema-14.8 output; native SINCAL open/save/calculation has not been validated. Fresh component identities are generated; readback uses a 100 MVA conversion base.",
    )];
    // Metadata is deliberately outside the electrical candidate profile. This
    // warning is unconditional so future metadata additions are not silently lost.
    loss(
        &mut diagnostics,
        "Fresh candidate output omits case/solver/area metadata, geometry, source provenance and extras; native component UIDs are regenerated. Bus and branch names are retained.",
    );
    let mut tables = Tables::default();
    let mut settings = row(
        "Flag_UseLA OpSer_ID IncrSer_ID Scenario_ID Flag_UseScenario Flag_UseTimeSer Flag_UseOpSer Flag_UseIncSer Flag_Unit Flag_ABW",
    );
    integer(&mut settings, "CalcParameter_ID", 1);
    real(&mut settings, "f", net.base_frequency());
    real(&mut settings, "ull", net.buses()[0].vmin * 100.0);
    real(&mut settings, "uul", net.buses()[0].vmax * 100.0);
    tables.push("CalcParameter", settings);
    let buses = net
        .buses()
        .iter()
        .map(|b| (b.id, b))
        .collect::<BTreeMap<_, _>>();
    write_buses(net, &mut tables, &mut diagnostics)?;
    write_loads(net, &mut tables)?;
    write_generators(net, &buses, &mut tables, &mut diagnostics)?;
    write_branches(net, &buses, &mut tables, &mut diagnostics)?;
    let database = tables.finish()?;
    let snapshot = DatabaseSnapshot::decode(&database, None).map_err(error)?;
    let readback = read_balanced_snapshot(&snapshot, net.name()).map_err(error)?;
    verify_topology(net, &readback)?;
    verify_electrical(net, &readback)?;
    Ok(ExperimentalBalancedOutput {
        database,
        diagnostics,
    })
}

// Readability alone is insufficient: native percentage/magnitude fields can
// round a small reactance to zero, or unit conversion can underflow. Check the
// electrical quantities actually recovered, including a changed MVA basis.
fn verify_electrical(original: &BalancedNetwork, recovered: &BalancedNetwork) -> Result<()> {
    fn same(a: f64, b: f64, context: &str) -> Result<()> {
        let scale = a.abs().max(b.abs());
        if !a.is_finite()
            || !b.is_finite()
            || (a != b && (a == 0.0 || b == 0.0 || (a / scale - b / scale).abs() > 1e-9))
        {
            return Err(error(format!(
                "{context}: candidate numeric conversion loses electrical value ({a} -> {b})"
            )));
        }
        Ok(())
    }
    let buses = recovered
        .buses()
        .iter()
        .map(|b| (b.id, b))
        .collect::<BTreeMap<_, _>>();
    for bus in original.buses() {
        let out = buses
            .get(&bus.id)
            .ok_or_else(|| error("missing authored bus"))?;
        if bus.kind != out.kind {
            return Err(error("candidate changes bus kind"));
        }
        for (a, b, context) in [
            (bus.base_kv, out.base_kv, "bus nominal voltage"),
            (bus.vm, out.vm, "bus start voltage"),
            (bus.va, out.va, "bus angle"),
            (bus.vmin, out.vmin, "bus lower limit"),
            (bus.vmax, out.vmax, "bus upper limit"),
        ] {
            same(a, b, context)?;
        }
    }
    for (load, out) in original.loads().iter().zip(recovered.loads()) {
        same(load.p, out.p, "load active power")?;
        same(load.q, out.q, "load reactive power")?;
    }
    for (generator, out) in original.generators().iter().zip(recovered.generators()) {
        if generator.voltage_regulation_on {
            same(generator.vg, out.vg, "source voltage")?;
        } else {
            same(generator.pg, out.pg, "converter active power")?;
            same(generator.qg, out.qg, "converter reactive power")?;
        }
    }
    for (branch, out) in original.branches().iter().zip(recovered.branches()) {
        let y = branch.charging.unwrap_or(BranchCharging::new(
            0.0,
            branch.b / 2.0,
            0.0,
            branch.b / 2.0,
        ));
        let oy = out
            .charging
            .unwrap_or(BranchCharging::new(0.0, out.b / 2.0, 0.0, out.b / 2.0));
        for (a, b, context) in [
            (
                branch.r,
                out.r * (original.base_mva() / recovered.base_mva()),
                "branch resistance",
            ),
            (
                branch.x,
                out.x * (original.base_mva() / recovered.base_mva()),
                "branch reactance",
            ),
            (
                y.g_fr,
                oy.g_fr * (recovered.base_mva() / original.base_mva()),
                "branch conductance",
            ),
            (
                y.b_fr,
                oy.b_fr * (recovered.base_mva() / original.base_mva()),
                "branch susceptance",
            ),
            (
                branch.calc_effective_tap(),
                out.calc_effective_tap(),
                "branch ratio",
            ),
            (branch.shift, out.shift, "branch shift"),
            (branch.rate_a, out.rate_a, "branch thermal limit"),
        ] {
            same(a, b, context)?;
        }
    }
    Ok(())
}

fn write_loads(net: &BalancedNetwork, tables: &mut Tables) -> Result<()> {
    for load in net.loads() {
        let mode = match &load.voltage_model {
            None | Some(LoadVoltageModel::ConstantPower) => 2,
            Some(LoadVoltageModel::Exponential {
                p,
                q,
                v_nom: None,
                gamma_p,
                gamma_q,
            }) if *p == load.p && *q == load.q && gamma_p == gamma_q => match *gamma_p {
                0.0 => 2,
                1.0 => 3,
                2.0 => 1,
                _ => return Err(error(format!("load at {}: unsupported exponent", load.bus))),
            },
            _ => {
                return Err(error(format!(
                    "load at {}: unsupported voltage model",
                    load.bus
                )));
            }
        };
        let mut input = row(
            "DayOpSer_ID YearOpSer_ID WeekOpSer_ID IncrSer_ID Mpl_ID Macro_ID Flag_Macro Load_ID TransformerTap_ID Stp_ID Typ_ID Gang_ID Flag_Typified Flag_ShdU Flag_ShdP Ireg pk Flag_LA",
        );
        integer(&mut input, "Flag_Load", 1);
        integer(&mut input, "Flag_Lf", 1);
        integer(&mut input, "Flag_LoadType", mode);
        for (key, value) in [
            ("P", load.p),
            ("Q", load.q),
            ("fP", 1.0),
            ("fQ", 1.0),
            ("u", 100.0),
        ] {
            real(&mut input, key, value);
        }
        tables.element("Load", input, &[load.bus], "", load.in_service)?;
    }
    Ok(())
}

fn write_generators(
    net: &BalancedNetwork,
    buses: &BTreeMap<BusId, &Bus>,
    tables: &mut Tables,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<()> {
    for generator in net.generators() {
        let bus = buses[&generator.bus];
        if !generator.pg.is_finite() || !generator.qg.is_finite() {
            return Err(error(format!(
                "generator at {}: finite P/Q required",
                bus.id
            )));
        }
        if generator.regulating_terminal.is_some()
            || generator.regulated_bus.is_some()
            || generator.active_power_control.is_some()
        {
            return Err(error(format!(
                "generator at {}: unsupported regulation/control",
                bus.id
            )));
        }
        let external = generator.voltage_regulation_on;
        if external && (bus.kind != BusType::Ref || generator.vg != bus.vm) {
            return Err(error(format!(
                "generator at {}: ideal source requires reference bus and matching voltage",
                bus.id
            )));
        }
        let mut input = row(
            "DayOpSer_ID YearOpSer_ID WeekOpSer_ID Typ_ID Flag_Typ_ID Mpl_ID Flag_Macro Macro_ID Flag_LimitType PowerLimit_ID Flag_Pctrl Flag_Qctrl Flag_CtrlPrior Qctrl_U_P_ID Qctrl_PF_U_ID Qctrl_PF_P_ID Qctrl_P_Q_ID Qctrl_U_Q_ID Node_ID MasterElm_ID Flag_ShdU Flag_ShdP Rlf Xlf Kr Flag_LfLimit",
        );
        integer(&mut input, "Flag_ChkType", 1);
        let kind = if external {
            for field in ["IncrSer_ID", "Flag_LfCtrl", "xi"] {
                integer(&mut input, field, 0);
            }
            integer(&mut input, "Flag_Lf", 3);
            real(&mut input, "u", generator.vg * 100.0);
            real(&mut input, "delta", bus.va);
            if generator.pg != 0.0 || generator.qg != 0.0 {
                loss(
                    diagnostics,
                    format!(
                        "source at {}: stored P/Q dispatch omitted; ideal reference injection is determined by the solve",
                        bus.id
                    ),
                );
            }
            "Infeeder"
        } else {
            for field in [
                "IslandOp",
                "Flag_Converter",
                "EnergyStorage_ID",
                "TransformerTap_ID",
                "Sn_Inverter",
                "Ireg",
                "pk",
                "Flag_LA",
                "Boost_Idc",
            ] {
                integer(&mut input, field, 0);
            }
            for field in ["Flag_Lf", "Flag_Connect", "Flag_I", "Flag_DCtyp"] {
                integer(&mut input, field, 1);
            }
            for (field, value) in [
                ("P", generator.pg),
                ("Q", generator.qg),
                ("fP", 1.0),
                ("fQ", 1.0),
            ] {
                real(&mut input, field, value);
            }
            "DCInfeeder"
        };
        loss(
            diagnostics,
            format!(
                "generator at {}: capability limits, costs, MVA base, inactive voltage setpoints and energy-source metadata omitted; native limits are inactive",
                bus.id
            ),
        );
        tables.element(kind, input, &[bus.id], "", generator.in_service)?;
    }
    Ok(())
}

fn write_branches(
    net: &BalancedNetwork,
    buses: &BTreeMap<BusId, &Bus>,
    tables: &mut Tables,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<()> {
    for (index, branch) in net.branches().iter().enumerate() {
        let from = buses[&branch.from];
        let to = buses[&branch.to];
        let y = branch.charging.unwrap_or(BranchCharging::new(
            0.0,
            branch.b / 2.0,
            0.0,
            branch.b / 2.0,
        ));
        if branch.control.is_some() || y.g_fr != y.g_to || y.b_fr != y.b_to {
            return Err(error(format!(
                "branch {index}: control or asymmetric terminal admittance unsupported"
            )));
        }
        if !branch.r.is_finite()
            || branch.r < 0.0
            || !branch.x.is_finite()
            || (branch.r == 0.0 && branch.x == 0.0)
            || !branch.calc_effective_tap().is_finite()
            || branch.calc_effective_tap() <= 0.0
            || !branch.rate_a.is_finite()
            || branch.rate_a < 0.0
        {
            return Err(error(format!(
                "branch {index}: invalid series impedance, tap or rating"
            )));
        }
        let line = from.base_kv == to.base_kv
            && branch.calc_effective_tap() == 1.0
            && branch.shift == 0.0
            && y.g_fr == 0.0
            && y.b_fr >= 0.0;
        let (kind, input) = if line {
            let input = line_row(net, branch, from);
            ("Line", input)
        } else {
            let input = transformer_row(net, branch, from, to, y, index)?;
            ("TwoWindingTransformer", input)
        };
        if branch.rate_b != 0.0
            || branch.rate_c != 0.0
            || !branch.rating_sets.is_empty()
            || branch.current_ratings.is_some()
            || branch.angmin != -360.0
            || branch.angmax != 360.0
            || branch.solution.is_some()
        {
            loss(
                diagnostics,
                format!(
                    "branch {index}: extra/current ratings, angle bounds and stored terminal flows omitted; rate_a is retained"
                ),
            );
        }
        tables.element(
            kind,
            input,
            &[branch.from, branch.to],
            branch.name.as_deref().unwrap_or(""),
            branch.in_service,
        )?;
    }
    Ok(())
}

fn write_buses(
    net: &BalancedNetwork,
    tables: &mut Tables,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<()> {
    for bus in net.buses() {
        if !matches!(bus.kind, BusType::Pq | BusType::Ref) {
            return Err(error(format!(
                "bus {}: PV/isolated semantics unsupported",
                bus.id
            )));
        }
        if [bus.base_kv, bus.vm, bus.vmin, bus.vmax]
            .iter()
            .any(|v| !v.is_finite() || *v <= 0.0)
            || !bus.va.is_finite()
            || bus.vmin > bus.vmax
        {
            return Err(error(format!("bus {}: invalid voltage/angle", bus.id)));
        }
        if bus.vmin != net.buses()[0].vmin || bus.vmax != net.buses()[0].vmax {
            return Err(error(
                "heterogeneous bus voltage limits not supported by candidate global limits",
            ));
        }
        if bus.evhi.is_some() || bus.evlo.is_some() {
            loss(
                diagnostics,
                format!("bus {}: emergency voltage limits omitted", bus.id),
            );
        }
        if bus.kind == BusType::Ref
            && !net
                .generators()
                .iter()
                .any(|g| g.bus == bus.id && g.in_service && g.voltage_regulation_on)
        {
            return Err(error(format!(
                "reference bus {} requires an active voltage source",
                bus.id
            )));
        }
        let id = bus_id(bus.id)?;
        let mut level = row("Flag_DCInfeeder");
        integer(&mut level, "VoltLevel_ID", id);
        integer(&mut level, "Flag_Volt", 1);
        real(&mut level, "Un", bus.base_kv);
        real(&mut level, "f", net.base_frequency());
        tables.push("VoltageLevel", level);
        let mut node = row("Flag_Volt RefNode_ID Stp_ID Uul Ull");
        integer(&mut node, "Node_ID", id);
        integer(&mut node, "VoltLevel_ID", id);
        real(&mut node, "Un", bus.vm * bus.base_kv);
        real(&mut node, "Phi", bus.va);
        text(&mut node, "Name", bus.name.as_deref().unwrap_or(""));
        tables.push("Node", node);
    }
    Ok(())
}

fn line_row(net: &BalancedNetwork, branch: &Branch, from: &Bus) -> Row {
    let y = branch.charging.unwrap_or(BranchCharging::new(
        0.0,
        branch.b / 2.0,
        0.0,
        branch.b / 2.0,
    ));
    let zbase = from.base_kv.powi(2) / net.base_mva();
    let mut input = row(
        "Flag_Typ_ID Typ_ID Flag_Ll CoupData_ID Flag_Macro Macro_ID LineTemp_ID va ElemLoading_ID",
    );
    for field in [
        "Flag_LineTyp",
        "Flag_ESB",
        "Flag_Lf",
        "Flag_Vart",
        "Flag_Cond",
    ] {
        integer(&mut input, field, 1);
    }
    for (key, value) in [
        ("l", 1.0),
        ("ParSys", 1.0),
        ("fr", 1.0),
        ("fn", net.base_frequency()),
        ("r", branch.r * zbase),
        ("x", branch.x * zbase),
        (
            "c",
            2.0 * y.b_fr / (std::f64::consts::TAU * net.base_frequency() * 1e-9 * zbase),
        ),
        ("Ith", branch.rate_a / (3.0_f64.sqrt() * from.base_kv)),
    ] {
        real(&mut input, key, value);
    }
    input
}

fn transformer_row(
    net: &BalancedNetwork,
    branch: &Branch,
    from: &Bus,
    to: &Bus,
    y: BranchCharging,
    index: usize,
) -> Result<Row> {
    if branch.x < 0.0 || y.g_fr < 0.0 || y.b_fr > 0.0 || branch.rate_a <= 0.0 {
        return Err(error(format!(
            "branch {index}: transformer needs nonnegative R/X/core loss, inductive magnetization and positive MVA rating"
        )));
    }
    let mut input = row(
        "Flag_Tap Typ_ID Flag_Typ_ID Flag_Macro Macro_ID Flag_Ct Flag_Boost TransformerTap_ID Ctrl_OpSer_ID Ctrl_OpPnt_ID MasterElm_ID Node_ID TransformerCon_ID CompImp_ID CtrlRange_ID SatChar_ID alpha phi ukl uku ElemLoading_ID",
    );
    for field in ["Flag_Lf", "Flag_roh", "Flag_ConNode"] {
        integer(&mut input, field, 1);
    }
    // YY0 is a canonical positive-sequence representation only. No
    // zero-sequence winding/grounding meaning is claimed or emitted.
    integer(&mut input, "VecGrp", 6);
    for (key, value) in [
        ("Un1", from.base_kv * branch.calc_effective_tap()),
        ("Un2", to.base_kv),
        ("Sn", branch.rate_a),
        ("ur", 100.0 * branch.r * branch.rate_a / net.base_mva()),
        (
            "uk",
            100.0 * branch.r.hypot(branch.x) * branch.rate_a / net.base_mva(),
        ),
        ("roh", 0.0),
        ("rohm", 0.0),
        ("ukr", 0.0),
        ("AddRotate", branch.shift),
        ("Vfe", 2.0 * y.g_fr * net.base_mva() * 1000.0),
        (
            "i0",
            200.0 * y.g_fr.hypot(y.b_fr) * net.base_mva() / branch.rate_a,
        ),
    ] {
        real(&mut input, key, value);
    }
    Ok(input)
}

fn verify_topology(original: &BalancedNetwork, recovered: &BalancedNetwork) -> Result<()> {
    if original.buses().len() != recovered.buses().len()
        || original.loads().len() != recovered.loads().len()
        || original.generators().len() != recovered.generators().len()
        || original.branches().len() != recovered.branches().len()
    {
        return Err(error("candidate changes component counts"));
    }
    for (a, b) in original.loads().iter().zip(recovered.loads()) {
        if a.bus != b.bus || a.in_service != b.in_service {
            return Err(error("candidate changes load connectivity/service state"));
        }
    }
    for (a, b) in original.generators().iter().zip(recovered.generators()) {
        if a.bus != b.bus
            || a.in_service != b.in_service
            || a.voltage_regulation_on != b.voltage_regulation_on
        {
            return Err(error(
                "candidate changes source connectivity/service/control mode",
            ));
        }
    }
    for (a, b) in original.branches().iter().zip(recovered.branches()) {
        if a.from != b.from || a.to != b.to || a.in_service != b.in_service {
            return Err(error("candidate changes branch connectivity/service state"));
        }
    }
    Ok(())
}
