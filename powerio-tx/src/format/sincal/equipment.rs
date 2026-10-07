//! Positive-sequence physical units to the balanced network's MW/kV/pu basis.
use super::{error, rows::NativeRow};
use crate::Result;
use crate::network::{
    BalancedNetwork, Branch, BranchCharging, BranchCurrentRatings, BusId, BusType, Generator, Load,
    LoadVoltageModel,
};

fn kv(network: &BalancedNetwork, bus: BusId) -> f64 {
    network
        .buses()
        .iter()
        .find(|b| b.id == bus)
        .unwrap()
        .base_kv
}

fn finite(value: f64, context: &str) -> Result<f64> {
    if value.is_finite() {
        Ok(value)
    } else {
        Err(error(format!("{context}: arithmetic overflow")))
    }
}

fn quadrature(magnitude: f64, real: f64, context: &str) -> Result<f64> {
    if real > magnitude {
        return Err(error(format!("{context}: real part exceeds magnitude")));
    }
    // Avoid cancellation/overflow from subtracting separately squared values.
    finite(((magnitude - real) * (magnitude + real)).sqrt(), context)
}

pub(super) fn line(
    row: &NativeRow,
    ports: &[BusId],
    network: &BalancedNetwork,
    uid: &str,
) -> Result<Branch> {
    for field in [
        "Flag_LineTyp",
        "Flag_Cond",
        "Flag_Vart",
        "Flag_ESB",
        "Flag_Lf",
    ] {
        row.equals(field, 1)?;
    }
    row.inactive(&[
        "Flag_Typ_ID",
        "Typ_ID",
        "Flag_Ll",
        "CoupData_ID",
        "Flag_Macro",
        "Macro_ID",
        "LineTemp_ID",
        "va",
        "ElemLoading_ID",
    ])?;
    if kv(network, ports[0]).to_bits() != kv(network, ports[1]).to_bits() {
        return Err(row.bad("Un", "line joins unlike voltage bases"));
    }
    let base_z = kv(network, ports[0]).powi(2) / network.base_mva();
    let length = row.positive("l")?;
    let parallel = row.positive("ParSys")?;
    let scale = finite(length / parallel / base_z, "line impedance base")?;
    let r = finite(
        row.nonnegative("r")? * row.positive("fr")? * scale,
        "line resistance",
    )?;
    let x = finite(row.number("x")? * scale, "line reactance")?;
    if r == 0.0 && x == 0.0 {
        return Err(row.bad("r/x", "zero series impedance requires an ideal connection"));
    }
    if row.positive("fn")?.to_bits() != network.base_frequency().to_bits() {
        return Err(row.bad("fn", "frequency conversion not implemented"));
    }
    let mut branch = Branch::new(ports[0], ports[1], r, x);
    branch.b = finite(
        std::f64::consts::TAU
            * network.base_frequency()
            * row.nonnegative("c")?
            * 1e-9
            * length
            * parallel
            * base_z,
        "line charging",
    )?;
    let amps = finite(
        row.nonnegative("Ith")? * 1000.0 * parallel,
        "line current limit",
    )?;
    branch.current_ratings = Some(BranchCurrentRatings::new(amps, 0.0, 0.0));
    branch.rate_a = finite(
        3.0_f64.sqrt() * kv(network, ports[0]) * amps / 1000.0,
        "line thermal rating",
    )?;
    branch.uid = Some(uid.to_owned());
    Ok(branch)
}

pub(super) fn load(row: &NativeRow, bus: BusId, base_kv: f64, uid: &str) -> Result<Load> {
    row.equals("Flag_Load", 1)?;
    row.inactive(&[
        "Mpl_ID",
        "Macro_ID",
        "Flag_Macro",
        "Load_ID",
        "DayOpSer_ID",
        "WeekOpSer_ID",
        "YearOpSer_ID",
        "IncrSer_ID",
        "TransformerTap_ID",
        "Stp_ID",
        "Typ_ID",
        "Gang_ID",
        "Flag_Typified",
        "Flag_ShdU",
        "Flag_ShdP",
        "Ireg",
        "pk",
        "Flag_LA",
    ])?;
    // These modes explicitly supply total three-phase power. Phase-resolved
    // modes (13/14/15) must never become balanced by adding their values.
    let mode = row.integer("Flag_Lf")?;
    let (p, q) = match mode {
        1 | 2 => (
            row.number("P")? * row.number("fP")?,
            row.number("Q")? * row.number("fQ")?,
        ),
        3 | 4 => {
            let s = row.nonnegative("S")? * row.nonnegative("fS")?;
            let pf = row.number("cosphi")?;
            if !(0.0..=1.0).contains(&pf) {
                return Err(row.bad("cosphi", "unsupported power factor"));
            }
            (s * pf, s * (1.0 - pf * pf).sqrt())
        }
        11 | 12 => {
            let p = row.number("P")? * row.number("fP")?;
            let pf = row.positive("cosphi")?;
            if pf > 1.0 {
                return Err(row.bad("cosphi", "power factor exceeds one"));
            }
            (p, p / pf * (1.0 - pf * pf).sqrt())
        }
        _ => return Err(row.bad("Flag_Lf", "unsupported balanced load mode")),
    };
    let mut load = Load::new(bus, finite(p, "load P")?, finite(q, "load Q")?);
    let exponent = match row.integer("Flag_LoadType")? {
        1 => 2.0,
        2 => 0.0,
        3 => 1.0,
        _ => return Err(row.bad("Flag_LoadType", "unknown voltage dependence")),
    };
    // Normalize voltage-dependent demand to the bus nominal voltage. This
    // gives the same v_nom=None semantics as XIIDM/CGMES and avoids leaking
    // a format-specific absolute-voltage convention into the neutral model.
    let v_nom = if mode % 2 == 0 {
        row.positive("Ul")? / base_kv
    } else {
        row.positive("u")? / 100.0
    };
    if exponent != 0.0 {
        load.p = finite(p / v_nom.powf(exponent), "nominal load P")?;
        load.q = finite(q / v_nom.powf(exponent), "nominal load Q")?;
        load.voltage_model = Some(LoadVoltageModel::Exponential {
            p: load.p,
            q: load.q,
            v_nom: None,
            gamma_p: exponent,
            gamma_q: exponent,
        });
    }
    load.uid = Some(uid.to_owned());
    Ok(load)
}

pub(super) fn generator(
    row: &NativeRow,
    bus: BusId,
    external: bool,
    uid: &str,
    network: &mut BalancedNetwork,
    in_service: bool,
) -> Result<Generator> {
    row.inactive(&[
        "Typ_ID",
        "Flag_Typ_ID",
        "Mpl_ID",
        "Flag_Macro",
        "Macro_ID",
        "DayOpSer_ID",
        "WeekOpSer_ID",
        "YearOpSer_ID",
        "Flag_LfLimit",
        "Flag_LimitType",
        "PowerLimit_ID",
        "Flag_Pctrl",
        "Flag_Qctrl",
        "Flag_CtrlPrior",
        "Qctrl_U_P_ID",
        "Qctrl_PF_U_ID",
        "Qctrl_PF_P_ID",
        "Qctrl_P_Q_ID",
        "Qctrl_U_Q_ID",
        "Node_ID",
        "MasterElm_ID",
        "Flag_ShdU",
        "Flag_ShdP",
        "Rlf",
        "Xlf",
        "Kr",
    ])?;
    row.equals("Flag_ChkType", 1)?;
    let mut generator = Generator::new(bus);
    generator.uid = Some(uid.to_owned());
    generator.in_service = in_service;
    // Inactive native capability limits do not mean zero output capability.
    generator.pmin = f64::NEG_INFINITY;
    generator.pmax = f64::INFINITY;
    generator.qmin = f64::NEG_INFINITY;
    generator.qmax = f64::INFINITY;
    if external {
        row.equals("Flag_Lf", 3)?;
        row.inactive(&["IncrSer_ID", "Flag_LfCtrl", "xi"])?;
        generator.vg = row.positive("u")? / 100.0;
        let angle = row.number("delta")?;
        if in_service {
            let bus = network
                .buses_mut()
                .iter_mut()
                .find(|b| b.id == bus)
                .unwrap();
            if bus.kind == BusType::Ref
                && ((bus.vm - generator.vg).abs() > 1e-12 || (bus.va - angle).abs() > 1e-12)
            {
                return Err(row.bad("u/delta", "inconsistent reference voltages on one bus"));
            }
            bus.kind = BusType::Ref;
            bus.vm = generator.vg;
            bus.va = angle;
        }
    } else {
        row.equals("Flag_Lf", 1)?;
        row.equals("Flag_Connect", 1)?;
        row.equals("Flag_I", 1)?;
        row.inactive(&[
            "IslandOp",
            "Flag_Converter",
            "EnergyStorage_ID",
            "TransformerTap_ID",
            "Sn_Inverter",
            "Ireg",
            "pk",
            "Flag_LA",
            "Boost_Idc",
        ])?;
        if !(1..=7).contains(&row.integer("Flag_DCtyp")?) {
            return Err(row.bad("Flag_DCtyp", "unknown converter type"));
        }
        generator.voltage_regulation_on = false;
        generator.pg = finite(row.number("P")? * row.number("fP")?, "converter P")?;
        generator.qg = finite(row.number("Q")? * row.number("fQ")?, "converter Q")?;
    }
    Ok(generator)
}

pub(super) fn transformer(
    row: &NativeRow,
    ports: &[BusId],
    network: &BalancedNetwork,
    uid: &str,
) -> Result<Branch> {
    row.equals("Flag_Lf", 1)?;
    row.equals("Flag_roh", 1)?;
    row.equals("Flag_Tap", 0)?;
    row.inactive(&[
        "Typ_ID",
        "Flag_Typ_ID",
        "Flag_Macro",
        "Macro_ID",
        "Flag_Ct",
        "Flag_Boost",
        "TransformerTap_ID",
        "Ctrl_OpSer_ID",
        "Ctrl_OpPnt_ID",
        "MasterElm_ID",
        "Node_ID",
        "TransformerCon_ID",
        "CompImp_ID",
        "CtrlRange_ID",
        "SatChar_ID",
        "alpha",
        "phi",
        "ukl",
        "uku",
        "ElemLoading_ID",
    ])?;
    let clock = match row.integer("VecGrp")? {
        1 | 4..=7 => 0,
        10 | 13 | 14 | 70 => 1,
        23..=26 => 5,
        35 | 38..=41 => 6,
        44 | 45 | 48 | 49 => 7,
        58..=61 => 11,
        _ => return Err(row.bad("VecGrp", "unsupported winding group")),
    };
    let un1 = row.positive("Un1")?;
    let un2 = row.positive("Un2")?;
    let sn = row.positive("Sn")?;
    let magnitude = row.nonnegative("uk")? / 100.0;
    let resistance = row.nonnegative("ur")? / 100.0;
    let reactance = quadrature(magnitude, resistance, "transformer short circuit")?;
    if magnitude == 0.0 {
        return Err(row.bad("uk", "zero impedance transformer"));
    }
    let secondary_scale = (un2 / kv(network, ports[1])).powi(2) * network.base_mva() / sn;
    let mut branch = Branch::new(
        ports[0],
        ports[1],
        finite(resistance * secondary_scale, "transformer resistance")?,
        finite(reactance * secondary_scale, "transformer reactance")?,
    );
    let delta = row.number("roh")? - row.number("rohm")?;
    let tap = finite(1.0 + delta * row.number("ukr")? / 100.0, "transformer tap")?;
    if tap <= 0.0 {
        return Err(row.bad("roh/ukr", "nonpositive tap"));
    }
    let (primary, secondary) = match row.integer("Flag_ConNode")? {
        1 => (tap, 1.0),
        2 => (1.0, tap),
        _ => return Err(row.bad("Flag_ConNode", "unknown tap side")),
    };
    // Refer the native pi circuit to the actual secondary winding voltage.
    branch.r = finite(
        branch.r * secondary * secondary,
        "tapped transformer resistance",
    )?;
    branch.x = finite(
        branch.x * secondary * secondary,
        "tapped transformer reactance",
    )?;
    if branch.r == 0.0 && branch.x == 0.0 {
        return Err(row.bad("uk/Un2/Sn", "series impedance underflows"));
    }
    branch.tap = finite(
        un1 / un2 * kv(network, ports[1]) / kv(network, ports[0]) * primary / secondary,
        "transformer ratio",
    )?;
    if branch.tap <= 0.0 {
        return Err(row.bad("Un1/Un2", "voltage ratio underflows"));
    }
    branch.shift = finite(
        f64::from(clock) * 30.0 + row.number("AddRotate")?,
        "transformer rotation",
    )?;
    let loss_mw = row.nonnegative("Vfe")? / 1000.0;
    let no_load_mva = sn * row.nonnegative("i0")? / 100.0;
    let magnetizing_mvar = quadrature(no_load_mva, loss_mw, "transformer no-load loss")?;
    let y_scale = (kv(network, ports[1]) / un2 / secondary).powi(2) / network.base_mva();
    let g = finite(loss_mw * y_scale / 2.0, "transformer core conductance")?;
    let b = finite(
        -magnetizing_mvar * y_scale / 2.0,
        "transformer core susceptance",
    )?;
    branch.charging = Some(BranchCharging::new(g, b, g, b));
    branch.b = 2.0 * b;
    branch.rate_a = sn;
    branch.uid = Some(uid.to_owned());
    Ok(branch)
}
