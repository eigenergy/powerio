//! Electrical inverse mappings for the declared candidate conductor profile.
use super::{
    write::{ExpectedLoad, Result, Tables, error, integer, near, real, row},
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
        let mut input = row(
            "Typ_ID Flag_Typ_ID CoupData_ID Flag_Ll Flag_Ground Flag_Macro Macro_ID LineTemp_ID ElemLoading_ID R0_R1 X0_X1 va alpha",
        );
        integer(&mut input, "Flag_LineTyp", 1);
        integer(&mut input, "Flag_ESB", 1);
        integer(&mut input, "Flag_Vart", 1);
        integer(&mut input, "Flag_Cond", 1);
        integer(&mut input, "Flag_Lf", 1);
        integer(&mut input, "Flag_Z0_Input", 2);
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
