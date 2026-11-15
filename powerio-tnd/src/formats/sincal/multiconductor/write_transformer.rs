//! Finite three-phase transformer authoring. Winding leakage stays in the
//! conductor model; a balanced projection cannot validate this conversion.
use super::{
    write::{Result, Tables, error, integer, real, row},
    write_topology::Topology,
};
use num_complex::Complex64;
use powerio_dist::{DistTransformer, DistWindingConn, MulticonductorNetwork};

pub(super) struct ExpectedTransformer {
    pub id: i64,
    pub nodes: [i64; 2],
    pub primitive: [[Complex64; 6]; 6],
}

// The vector group follows the one delta orientation
// (`powerio_dist::transformer::calc_transformer_phase_shift`): OpenDSS's
// ANSI/Lag default, Lead/Euro reversing the HV/LV rotation, the order set by
// rated winding voltage. `reverse` is a negative shift from the first
// winding to the second.
fn group(t: &DistTransformer) -> Result<i64> {
    for (key, value) in &t.extras {
        match key.as_str() {
            "leadlag" => {}
            "%noloadloss" | "%imag" | "ppm_antifloat" if value.as_f64() == Some(0.0) => {}
            _ => {
                return Err(error(format!(
                    "transformer {}: extra {key} requires explicit electrical/metadata mapping",
                    t.name
                )));
            }
        }
    }
    let reverse = powerio_dist::transformer::calc_transformer_phase_shift(t)
        .map_err(|_| error("transformer has an unknown leadlag convention"))?
        < 0.0;
    let code = match (&t.windings[0].conn, &t.windings[1].conn, reverse) {
        (DistWindingConn::Delta, DistWindingConn::Delta, _) => 1,
        (DistWindingConn::Delta, DistWindingConn::Wye, false) => 10,
        (DistWindingConn::Delta, DistWindingConn::Wye, true) => 59,
        (DistWindingConn::Wye, DistWindingConn::Delta, false) => 14,
        (DistWindingConn::Wye, DistWindingConn::Delta, true) => 61,
        _ => {
            return Err(error(
                "Wye/Wye zero-sequence authoring requires a separate native representation",
            ));
        }
    };
    Ok(code)
}

fn validate_windings(net: &MulticonductorNetwork, t: &DistTransformer) -> Result<()> {
    if t.windings.len() != 2 || t.phases != 3 || t.xsc_pct.len() != 1 {
        return Err(error(
            "candidate transformer requires two complete three-phase windings",
        ));
    }
    for w in &t.windings {
        if ![w.v_ref, w.s_rating, w.tap]
            .iter()
            .all(|v| v.is_finite() && *v > 0.0)
            || !w.r_pct.is_finite()
            || w.r_pct < 0.0
            || [w.r_neutral, w.x_neutral]
                .into_iter()
                .flatten()
                .any(|v| v != 0.0)
        {
            return Err(error(
                "invalid transformer rating, tap, leakage or non-solid neutral impedance",
            ));
        }
        let bus = net
            .bus(&w.bus)
            .ok_or_else(|| error("missing transformer winding bus"))?;
        let count = if w.conn == DistWindingConn::Wye { 4 } else { 3 };
        if w.terminal_map.len() != count
            || w.terminal_map[..3] != ["1", "2", "3"]
            || (count == 4 && !bus.grounded.contains(&w.terminal_map[3]))
        {
            return Err(error(
                "transformer requires ordered phases and a solidly grounded Wye star",
            ));
        }
    }
    Ok(())
}

pub(super) fn transformers(
    net: &MulticonductorNetwork,
    topology: &Topology,
    tables: &mut Tables,
) -> Result<()> {
    for t in net.transformers() {
        validate_windings(net, t)?;
        let [p, s] = [&t.windings[0], &t.windings[1]];
        // Equal ratings avoid changing per-winding current capability and the
        // OpenDSS two-winding kVA convention. Fixed taps enter both the turns
        // ratio and terminal-referred impedance through the effective kV.
        if p.s_rating.to_bits() != s.s_rating.to_bits()
            || !t.xsc_pct[0].is_finite()
            || t.xsc_pct[0] < 0.0
        {
            return Err(error(
                "transformer needs equal winding ratings and nonnegative finite leakage reactance",
            ));
        }
        let r = p.r_pct + s.r_pct;
        let x = t.xsc_pct[0];
        if !r.hypot(x).is_finite() || r.hypot(x) == 0.0 {
            return Err(error(
                "zero/overflowing transformer leakage needs a separate ideal constraint",
            ));
        }
        let code = group(t)?;
        let volts = [p.v_ref * p.tap, s.v_ref * s.tap];
        if volts.iter().any(|v| !v.is_finite() || *v <= 0.0) {
            return Err(error(
                "transformer effective winding voltage overflows/underflows",
            ));
        }
        let nodes = [topology.bus_ids[&p.bus], topology.bus_ids[&s.bus]];
        let mut input = row(
            "Typ_ID Flag_Typ Macro_ID MasterElm_ID TransformerTap_ID TransformerCon_ID Flag_Ct Stp_ID1 Stp_ID2 Flag_Tap Flag_Macro Flag_Boost ElemLoading_ID Ctrl_OpSer_ID Ctrl_OpPnt_ID CompImp_ID CtrlRange_ID",
        );
        for (key, value) in [
            ("VecGrp", code),
            ("Flag_roh", 1),
            ("Flag_ConNode", 1),
            ("Flag_Lf", 1),
            ("Flag_Z0_Input", 2),
        ] {
            integer(&mut input, key, value);
        }
        let grounded = usize::from(p.conn != DistWindingConn::Wye);
        let zbase = volts[grounded] * volts[grounded] / p.s_rating;
        for (key, value) in [
            ("Un1", volts[0] / 1000.0),
            ("Un2", volts[1] / 1000.0),
            ("Sn", p.s_rating / 1e6),
            ("ur", r),
            ("uk", r.hypot(x)),
            ("R0", r / 100.0 * zbase),
            ("X0", x / 100.0 * zbase),
            ("Vfe", 0.0),
            ("i0", 0.0),
            ("AddRotate", 0.0),
            ("roh", 0.0),
            ("rohm", 0.0),
            ("ukr", 0.0),
            ("alpha", 0.0),
            ("phi", 0.0),
            ("C01", 0.0),
            ("C02", 0.0),
        ] {
            real(&mut input, key, value);
        }
        let primitive = phase_primitive(net, t)?;
        let id = tables.element(
            "TwoWindingTransformer",
            input,
            &[(nodes[0], 7), (nodes[1], 7)],
            &t.name,
        )?;
        tables.transformers.push(ExpectedTransformer {
            id,
            nodes,
            primitive,
        });
    }
    Ok(())
}

/// The transformer primitive
/// (`powerio_dist::transformer::calc_transformer_primitive_admittance`)
/// over each winding's three phase terminals, first winding then second. A
/// WYE star is solidly grounded, so its neutral row and column drop exactly;
/// zero-sequence current is eliminated on delta ports by the coil incidence
/// itself, not by balancing the circuit.
fn phase_primitive(
    net: &MulticonductorNetwork,
    t: &DistTransformer,
) -> Result<[[Complex64; 6]; 6]> {
    let primitive = powerio_dist::transformer::calc_transformer_primitive_admittance(net, t)
        .map_err(|e| error(format!("transformer {}: {e}", t.name)))?;
    let mut rows = [0usize; 6];
    for (side, w) in t.windings.iter().enumerate() {
        for phase in 0..3 {
            rows[3 * side + phase] = primitive
                .terminals
                .iter()
                .position(|&(bus, terminal)| {
                    bus.eq_ignore_ascii_case(&w.bus) && terminal == w.terminal_map[phase]
                })
                .ok_or_else(|| error("transformer primitive lost a phase terminal"))?;
        }
    }
    let mut result = [[Complex64::new(0.0, 0.0); 6]; 6];
    for (i, row) in result.iter_mut().enumerate() {
        for (j, value) in row.iter_mut().enumerate() {
            *value = primitive.primitive[rows[i]][rows[j]];
            if !value.re.is_finite() || !value.im.is_finite() {
                return Err(error(
                    "transformer conductor admittance overflows/underflows",
                ));
            }
        }
    }
    if (0..6).any(|i| result[i][i].norm() == 0.0) {
        return Err(error("transformer conductor admittance underflows"));
    }
    Ok(result)
}

/// [`phase_primitive`] of a candidate unit on its own: each winding on a bus
/// of its own declaring the winding's terminals.
pub(super) fn coil_primitive(t: &DistTransformer) -> Result<[[Complex64; 6]; 6]> {
    let mut unit = t.clone();
    let mut net = MulticonductorNetwork::new();
    for (side, w) in unit.windings.iter_mut().enumerate() {
        w.bus = format!("winding-{side}");
        net.buses_mut().push(powerio_dist::DistBus::new(
            w.bus.clone(),
            w.terminal_map.clone(),
        ));
    }
    phase_primitive(&net, &unit)
}

pub(super) fn verify(net: &MulticonductorNetwork, expected: &[ExpectedTransformer]) -> Result<()> {
    if net.shunts().len() != expected.len() || !net.transformers().is_empty() {
        return Err(error("candidate changes transformer primitive count"));
    }
    for t in expected {
        let shunt = net
            .shunts()
            .iter()
            .find(|s| s.name == t.id.to_string())
            .ok_or_else(|| error("missing transformer primitive"))?;
        let coordinates = ["p1", "p2", "p3", "s1", "s2", "s3"];
        if shunt.terminal_map != coordinates || shunt.g.len() != 6 || shunt.b.len() != 6 {
            return Err(error("candidate changes transformer coordinates"));
        }
        for i in 0..6 {
            if shunt.g[i].len() != 6 || shunt.b[i].len() != 6 {
                return Err(error("invalid transformer primitive size"));
            }
            for j in 0..6 {
                // Scale each port block separately: a high turns ratio must
                // not hide loss of the smaller primary or transfer block.
                let scale = t.primitive[3 * (i / 3)..3 * (i / 3) + 3]
                    .iter()
                    .flat_map(|row| &row[3 * (j / 3)..3 * (j / 3) + 3])
                    .map(|z| z.norm())
                    .fold(0.0_f64, f64::max);
                let actual = Complex64::new(shunt.g[i][j], shunt.b[i][j]);
                if !actual.re.is_finite()
                    || !actual.im.is_finite()
                    || (actual - t.primitive[i][j]).norm() > 1e-10 * scale
                {
                    return Err(error("candidate changes transformer conductor admittance"));
                }
            }
        }
        for side in 0..2 {
            if !net.switches().iter().any(|s| {
                !s.open
                    && s.bus_from == t.nodes[side].to_string()
                    && s.bus_to == shunt.bus
                    && s.terminal_map_from == ["1", "2", "3"]
                    && s.terminal_map_to == coordinates[3 * side..3 * side + 3]
            }) {
                return Err(error("candidate changes transformer port connectivity"));
            }
        }
    }
    Ok(())
}
