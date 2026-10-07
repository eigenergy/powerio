//! Finite three-phase transformer authoring. Winding leakage stays in the
//! conductor model; a balanced projection cannot validate this conversion.
use super::{
    write::{Result, Tables, error, integer, real, row},
    write_topology::Topology,
};
use crate::{DistTransformer, DistWindingConn, MulticonductorNetwork};
use num_complex::Complex64;

pub(super) struct ExpectedTransformer {
    pub id: i64,
    pub nodes: [i64; 2],
    pub primitive: [[Complex64; 6]; 6],
}

// OpenDSS-compatible generic winding convention: ANSI/Lag is the default;
// Lead/Euro reverses the HV/LV rotation. The order is determined by rated
// winding voltage, not operating voltage or fixed tap position.
fn group(t: &DistTransformer) -> Result<(i64, bool)> {
    let mut lead = false;
    for (key, value) in &t.extras {
        match key.as_str() {
            "leadlag" => {
                lead = match value.as_str().map(str::to_ascii_lowercase).as_deref() {
                    Some("lead" | "euro") => true,
                    Some("lag" | "ansi") => false,
                    _ => return Err(error("transformer has an unknown leadlag convention")),
                };
            }
            "%noloadloss" | "%imag" | "ppm_antifloat" if value.as_f64() == Some(0.0) => {}
            _ => {
                return Err(error(format!(
                    "transformer {}: extra {key} requires explicit electrical/metadata mapping",
                    t.name
                )));
            }
        }
    }
    let reverse = lead ^ (t.windings[0].v_ref < t.windings[1].v_ref);
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
    Ok((code, reverse))
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
        let (code, reverse) = group(t)?;
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
        let primitive = coil_primitive(t, reverse)?;
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

/// Direct winding-incidence construction, independent of the native reader's
/// symmetrical-component transformation. Zero-sequence current is eliminated
/// on delta ports by the coil incidence itself, not by balancing the circuit.
fn coil_primitive(t: &DistTransformer, reverse: bool) -> Result<[[Complex64; 6]; 6]> {
    let mut incidence = [[0.0; 6]; 3];
    let mixed = t.windings[0].conn != t.windings[1].conn;
    for (side, w) in t.windings.iter().enumerate() {
        let is_delta = w.conn == DistWindingConn::Delta;
        let v = w.v_ref * w.tap / if is_delta { 1.0 } else { 3.0_f64.sqrt() };
        let scale = if side == 0 { 1.0 / v } else { -1.0 / v };
        // For mixed windings, Delta->Wye lag uses preceding phases;
        // Wye->Delta lag uses following phases. Lead reverses that choice.
        let previous = mixed && ((side == 0) ^ reverse);
        for (coil, row) in incidence.iter_mut().enumerate() {
            row[3 * side + coil] = scale;
            if is_delta {
                row[3 * side + (coil + if previous { 2 } else { 1 }) % 3] = -scale;
            }
        }
    }
    let z = Complex64::new(t.windings[0].r_pct + t.windings[1].r_pct, t.xsc_pct[0])
        * (0.03 / t.windings[0].s_rating);
    let y = Complex64::new(1.0, 0.0) / z;
    let mut result = [[Complex64::new(0.0, 0.0); 6]; 6];
    for (i, row) in result.iter_mut().enumerate() {
        for (j, value) in row.iter_mut().enumerate() {
            *value = y * incidence.iter().map(|a| a[i] * a[j]).sum::<f64>();
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
