//! Electrical inverse mappings for the declared candidate conductor profile.
use super::{
    write::{ExpectedLoad, Result, Row, Tables, error, integer, near, real, row},
    write_topology::{Topology, connection},
};
use crate::{ConductorMatrix, Configuration, DistLoadVoltageModel, MulticonductorNetwork};
use num_complex::Complex64;
use std::f64::consts::TAU;

pub(super) fn sources(
    net: &MulticonductorNetwork,
    topology: &Topology,
    tables: &mut Tables,
) -> Result<()> {
    for (index, source) in net.sources().iter().enumerate() {
        if source.terminal_map != ["1", "2", "3"] {
            return Err(error("candidate source requires ordered L1/L2/L3 phases"));
        }
        let magnitude = source.v_magnitude[0];
        if !magnitude.is_finite() || magnitude <= 0.0 {
            return Err(error("source magnitude must be positive and finite"));
        }
        let angle = source.v_angle[0];
        for (i, offset) in [0.0, -TAU / 3.0, TAU / 3.0].into_iter().enumerate() {
            let actual = Complex64::from_polar(source.v_magnitude[i], source.v_angle[i]);
            let canonical = Complex64::from_polar(magnitude, angle + offset);
            if !actual.re.is_finite()
                || !actual.im.is_finite()
                || (actual - canonical).norm() > 1e-10 * magnitude
            {
                return Err(error(
                    "unequal source phasors require additional native source modes",
                ));
            }
        }
        let bus = net.bus(&source.bus).unwrap();
        let floating = source
            .reference_terminal
            .as_ref()
            .is_some_and(|t| !bus.grounded.contains(t));
        let mut input = row(
            "Typ_ID Mpl_ID IncrSer_ID Macro_ID MasterElm_ID Node_ID PowerLimit_ID Flag_LfLimit Flag_LfCtrl Flag_Pctrl Flag_Qctrl Flag_Macro Kr Rlf Xlf xi DayOpSer_ID WeekOpSer_ID YearOpSer_ID Stp_ID R0 X0",
        );
        integer(&mut input, "Flag_Lf", 6);
        integer(&mut input, "Flag_Z0", i64::from(!floating));
        integer(&mut input, "Flag_Z0_Input", 2);
        real(&mut input, "Ug", magnitude * 3.0_f64.sqrt() / 1000.0);
        real(&mut input, "delta", angle.to_degrees());
        let id = tables.element(
            "Infeeder",
            input,
            &[(topology.bus_ids[&source.bus], 7)],
            &source.name,
        )?;
        tables.sources.push((id, index, floating));
    }
    Ok(())
}

pub(super) fn loads(
    net: &MulticonductorNetwork,
    topology: &Topology,
    tables: &mut Tables,
) -> Result<()> {
    for load in net.loads() {
        let bus = net.bus(&load.bus).unwrap();
        let (mode, volts) = match &load.voltage_model {
            DistLoadVoltageModel::ConstantPower { v_nom } => (2, v_nom),
            DistLoadVoltageModel::ConstantCurrent { v_nom } => (3, v_nom),
            DistLoadVoltageModel::ConstantImpedance { v_nom } => (1, v_nom),
            _ => {
                return Err(error(format!(
                    "load {}: ZIP/exponential authoring remains unfinished",
                    load.name
                )));
            }
        };
        let pairs = load_pairs(load, bus)?;
        let node = topology.bus_ids[&load.bus];
        for (i, pair) in pairs.iter().enumerate() {
            let phase_voltage = if let Some(&v) = volts.get(i) {
                v
            } else if volts.is_empty() && mode == 2 {
                let ll = topology.nodes[&node].0;
                if pair.len() == 1 {
                    ll / 3.0_f64.sqrt()
                } else {
                    ll
                }
            } else {
                return Err(error("load nominal voltage vector is incomplete"));
            };
            if !phase_voltage.is_finite() || phase_voltage <= 0.0 {
                return Err(error("load nominal voltage must be positive and finite"));
            }
            let input_ll = if pair.len() == 1 {
                phase_voltage * 3.0_f64.sqrt()
            } else {
                phase_voltage
            };
            let mut input = row(
                "Typ_ID Mpl_ID Gang_ID Load_ID IncrSer_ID Macro_ID Ireg Flag_Typified Stp_ID DayOpSer_ID WeekOpSer_ID YearOpSer_ID",
            );
            integer(&mut input, "Flag_Load", 1);
            integer(&mut input, "Flag_Lf", 2);
            integer(&mut input, "Flag_LoadType", mode);
            integer(&mut input, "Flag_Z0_Input", 3);
            for (key, value) in [
                ("Ul", input_ll / 1000.0),
                ("P", load.p_nom[i] / 1e6),
                ("Q", load.q_nom[i] / 1e6),
                ("fP", 1.0),
                ("fQ", 1.0),
            ] {
                real(&mut input, key, value);
            }
            let id = tables.element(
                "Load",
                input,
                &[(node, connection(pair)?)],
                &format!("{}:{i}", load.name),
            )?;
            tables.loads.push(ExpectedLoad {
                id,
                p: load.p_nom[i],
                q: load.q_nom[i],
                voltage: phase_voltage,
                mode,
                node,
                phases: pair.clone(),
            });
        }
    }
    Ok(())
}

/// Invert a transposed phase matrix. A one-phase primitive has a canonical
/// uncoupled extension; a phase-pair primitive uses the same diagonal/mutual
/// representation as the reader's verified missing-phase series reduction.
fn sequence(matrix: &ConductorMatrix, context: &str) -> Result<(f64, f64)> {
    let d = matrix[0][0];
    let m = if matrix.len() == 1 { 0.0 } else { matrix[0][1] };
    for (i, row) in matrix.iter().enumerate() {
        for (j, value) in row.iter().enumerate() {
            near(*value, if i == j { d } else { m }, context)?;
        }
    }
    let positive = d - m;
    let zero = d + 2.0 * m;
    if !positive.is_finite() || !zero.is_finite() || positive < 0.0 || zero < 0.0 {
        return Err(error(format!(
            "{context}: no supported nonnegative sequence representation"
        )));
    }
    Ok((positive, zero))
}

pub(super) fn lines(
    net: &MulticonductorNetwork,
    topology: &Topology,
    tables: &mut Tables,
) -> Result<()> {
    for (index, line) in net.lines().iter().enumerate() {
        if line.terminal_map_from != line.terminal_map_to {
            return Err(error("line conductor permutation is not yet writable"));
        }
        let flag = connection(&line.terminal_map_from)?;
        let from = topology.bus_ids[&line.bus_from];
        let to = topology.bus_ids[&line.bus_to];
        let voltage = topology.nodes[&from].0;
        if voltage.to_bits() != topology.nodes[&to].0.to_bits() {
            return Err(error("line joins different declared voltage levels"));
        }
        let code = net
            .line_codes()
            .iter()
            .find(|c| c.name == line.linecode)
            .unwrap();
        if code
            .g_from
            .iter()
            .flatten()
            .chain(code.g_to.iter().flatten())
            .any(|g| *g != 0.0)
        {
            return Err(error(
                "line dielectric conductance authoring remains unfinished",
            ));
        }
        if code.b_from != code.b_to {
            return Err(error(
                "asymmetric line charging is not representable by this profile",
            ));
        }
        let (r, r0) = sequence(&code.r_series, "line resistance")?;
        let (x, x0) = sequence(&code.x_series, "line reactance")?;
        let (b, b0) = sequence(&code.b_from, "line charging")?;
        if code.n_conductors < 3
            && (b != 0.0 || b0 != 0.0)
            && (r.to_bits() != r0.to_bits()
                || x.to_bits() != x0.to_bits()
                || b.to_bits() != b0.to_bits())
        {
            return Err(error(
                "coupled reduced-phase charging requires additional native semantics",
            ));
        }
        if line.i_max.is_some() && code.i_max.is_some() && line.i_max != code.i_max {
            return Err(error(
                "distinct line/linecode ampacities require explicit selection",
            ));
        }
        let limits = line
            .i_max
            .as_ref()
            .or(code.i_max.as_ref())
            .ok_or_else(|| error("candidate native line requires an explicit ampacity"))?;
        let amps = limits[0];
        if !amps.is_finite() || amps <= 0.0 || limits.iter().any(|v| v.to_bits() != amps.to_bits())
        {
            return Err(error(
                "candidate native line requires uniform positive ampacity",
            ));
        }
        let mut input = line_row(voltage, net.base_frequency());
        for (key, value) in [
            ("Un", voltage / 1000.0),
            ("ParSys", 1.0),
            ("fr", 1.0),
            ("l", line.length / 1000.0),
            ("Ith", amps / 1000.0),
            ("r", r * 1000.0),
            ("r0", r0 * 1000.0),
            ("x", x * 1000.0),
            ("x0", x0 * 1000.0),
            ("c", 2.0 * b / (TAU * net.base_frequency() * 1e-12)),
            ("c0", 2.0 * b0 / (TAU * net.base_frequency() * 1e-12)),
            ("fn", net.base_frequency()),
        ] {
            real(&mut input, key, value);
        }
        let id = tables.element("Line", input, &[(from, flag), (to, flag)], &line.name)?;
        tables.lines.push((id, index));
    }
    Ok(())
}

fn load_pairs(load: &crate::DistLoad, bus: &crate::DistBus) -> Result<Vec<Vec<String>>> {
    let branch_count = load.p_nom.len();
    let pairs = match load.configuration {
        Configuration::Wye => {
            let reference = load
                .terminal_map
                .last()
                .ok_or_else(|| error("missing load reference"))?;
            if !bus.grounded.contains(reference) {
                return Err(error(format!(
                    "load {}: floating/external neutral authoring remains unfinished",
                    load.name
                )));
            }
            load.terminal_map[..branch_count]
                .iter()
                .map(|p| vec![p.clone()])
                .collect()
        }
        Configuration::SinglePhase => {
            let a = &load.terminal_map[0];
            let b = &load.terminal_map[1];
            vec![if bus.grounded.contains(b) {
                vec![a.clone()]
            } else {
                vec![a.clone(), b.clone()]
            }]
        }
        Configuration::Delta if branch_count == 1 => vec![load.terminal_map.clone()],
        Configuration::Delta => (0..branch_count)
            .map(|i| {
                vec![
                    load.terminal_map[i].clone(),
                    load.terminal_map[(i + 1) % branch_count].clone(),
                ]
            })
            .collect(),
    };
    Ok(pairs)
}

// Share one concrete Line schema for finite lines and zero-impedance switch
// carriers. One canonical metre with exactly zero R/X/C introduces no finite
// electrical path; its first terminal is open. No small-impedance approximation.
fn line_row(voltage: f64, frequency: f64) -> Row {
    let mut input = row(
        "Typ_ID Flag_Typ_ID CoupData_ID Flag_Ll Flag_Ground Flag_Macro Macro_ID LineTemp_ID ElemLoading_ID R0_R1 X0_X1 va alpha",
    );
    for key in [
        "Flag_LineTyp",
        "Flag_ESB",
        "Flag_Vart",
        "Flag_Cond",
        "Flag_Lf",
    ] {
        integer(&mut input, key, 1);
    }
    integer(&mut input, "Flag_Z0_Input", 2);
    for (key, value) in [
        ("Un", voltage / 1000.0),
        ("ParSys", 1.0),
        ("fr", 1.0),
        ("l", 0.001),
        ("Ith", 0.0),
        ("r", 0.0),
        ("r0", 0.0),
        ("x", 0.0),
        ("x0", 0.0),
        ("c", 0.0),
        ("c0", 0.0),
        ("fn", frequency),
    ] {
        real(&mut input, key, value);
    }
    input
}

pub(super) fn open_switches(
    net: &MulticonductorNetwork,
    topology: &Topology,
    tables: &mut Tables,
) -> Result<()> {
    for (index, switch) in net.switches().iter().enumerate().filter(|(_, s)| s.open) {
        let flag = connection(&switch.terminal_map_from)?;
        let from = topology.bus_ids[&switch.bus_from];
        let to = topology.bus_ids[&switch.bus_to];
        let mut input = line_row(
            topology.nodes[&from].0.max(topology.nodes[&to].0),
            net.base_frequency(),
        );
        if let Some(limits) = &switch.i_max {
            let amperes = limits[0];
            if limits.iter().any(|a| a.to_bits() != amperes.to_bits()) {
                return Err(error(
                    "open switch requires uniform ampacity for native output",
                ));
            }
            real(&mut input, "Ith", amperes / 1000.0);
        }
        let id = tables.element("Line", input, &[(from, flag), (to, flag)], &switch.name)?;
        tables.open_port(id, 1)?;
        tables.open_switches.push((id, index));
    }
    Ok(())
}

pub(super) fn verify_open_switches(
    original: &MulticonductorNetwork,
    recovered: &MulticonductorNetwork,
    topology: &Topology,
    tables: &Tables,
) -> Result<()> {
    for &(id, index) in &tables.open_switches {
        let input = &original.switches()[index];
        let carrier = recovered
            .switches()
            .iter()
            .find(|s| s.name == id.to_string())
            .ok_or_else(|| error("missing open-switch carrier"))?;
        let phases = &input.terminal_map_from;
        if carrier.open
            || carrier.bus_to != topology.bus_ids[&input.bus_to].to_string()
            || &carrier.terminal_map_from != phases
            || &carrier.terminal_map_to != phases
        {
            return Err(error("candidate changes open-switch carrier connectivity"));
        }
        if !recovered.switches().iter().any(|s| {
            s.open
                && s.bus_from == topology.bus_ids[&input.bus_from].to_string()
                && s.bus_to == carrier.bus_from
                && &s.terminal_map_from == phases
                && &s.terminal_map_to == phases
        }) {
            return Err(error("candidate changes open-switch terminal state"));
        }
        match (&carrier.i_max, &input.i_max) {
            (None, None) => {}
            (Some(actual), Some(expected)) if actual.len() == expected.len() => {
                for (a, b) in actual.iter().zip(expected) {
                    near(*a, *b, "switch ampacity")?;
                }
            }
            _ => return Err(error("candidate changes open-switch ampacity")),
        }
    }
    Ok(())
}
