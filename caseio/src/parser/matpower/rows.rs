//! MATPOWER matrix rows → [`Network`](crate::network) elements.
//!
//! Column layouts are 0-based per the MATPOWER manual. Each `*_row` reads one
//! parsed numeric row into the format-neutral element(s). A bus row fans out
//! into a [`Bus`] plus an optional [`Load`] and [`Shunt`] (MATPOWER folds
//! demand and shunts onto the bus row; the hub keeps them first-class).

use serde_json::Value;

use crate::network::{
    Branch, Bus, BusType, Extras, GenCost, Generator, Hvdc, Load, Shunt, Storage, GEN_EXTRA_KEYS,
};
use crate::{Error, Result};

fn num(x: f64) -> Value {
    serde_json::Number::from_f64(x).map_or(Value::Null, Value::Number)
}

/// Bus matrix column indices.
mod bus_col {
    pub const BUS_I: usize = 0;
    pub const BUS_TYPE: usize = 1;
    pub const PD: usize = 2;
    pub const QD: usize = 3;
    pub const GS: usize = 4;
    pub const BS: usize = 5;
    pub const BUS_AREA: usize = 6;
    pub const VM: usize = 7;
    pub const VA: usize = 8;
    pub const BASE_KV: usize = 9;
    pub const ZONE: usize = 10;
    pub const VMAX: usize = 11;
    pub const VMIN: usize = 12;
    pub const REQUIRED: usize = 13;
}

/// Branch matrix column indices.
mod branch_col {
    pub const F_BUS: usize = 0;
    pub const T_BUS: usize = 1;
    pub const BR_R: usize = 2;
    pub const BR_X: usize = 3;
    pub const BR_B: usize = 4;
    pub const RATE_A: usize = 5;
    pub const RATE_B: usize = 6;
    pub const RATE_C: usize = 7;
    pub const TAP: usize = 8;
    pub const SHIFT: usize = 9;
    pub const BR_STATUS: usize = 10;
    pub const ANGMIN: usize = 11;
    pub const ANGMAX: usize = 12;
    pub const REQUIRED: usize = 13;
}

/// DC line matrix column indices (MATPOWER 17-column layout).
mod dcline_col {
    pub const F_BUS: usize = 0;
    pub const T_BUS: usize = 1;
    pub const BR_STATUS: usize = 2;
    pub const PF: usize = 3;
    pub const PT: usize = 4;
    pub const QF: usize = 5;
    pub const QT: usize = 6;
    pub const VF: usize = 7;
    pub const VT: usize = 8;
    pub const PMIN: usize = 9;
    pub const PMAX: usize = 10;
    pub const QMINF: usize = 11;
    pub const QMAXF: usize = 12;
    pub const QMINT: usize = 13;
    pub const QMAXT: usize = 14;
    pub const LOSS0: usize = 15;
    pub const LOSS1: usize = 16;
    pub const REQUIRED: usize = 17;
}

/// Generator matrix column indices.
mod gen_col {
    pub const GEN_BUS: usize = 0;
    pub const PG: usize = 1;
    pub const QG: usize = 2;
    pub const QMAX: usize = 3;
    pub const QMIN: usize = 4;
    pub const VG: usize = 5;
    pub const MBASE: usize = 6;
    pub const GEN_STATUS: usize = 7;
    pub const PMAX: usize = 8;
    pub const PMIN: usize = 9;
    pub const REQUIRED: usize = 10;
}

/// Storage matrix column indices (PowerModels / pglib 17-column layout).
mod storage_col {
    pub const STORAGE_BUS: usize = 0;
    pub const PS: usize = 1;
    pub const QS: usize = 2;
    pub const ENERGY: usize = 3;
    pub const ENERGY_RATING: usize = 4;
    pub const CHARGE_RATING: usize = 5;
    pub const DISCHARGE_RATING: usize = 6;
    pub const CHARGE_EFFICIENCY: usize = 7;
    pub const DISCHARGE_EFFICIENCY: usize = 8;
    pub const THERMAL_RATING: usize = 9;
    pub const QMIN: usize = 10;
    pub const QMAX: usize = 11;
    pub const R: usize = 12;
    pub const X: usize = 13;
    pub const P_LOSS: usize = 14;
    pub const Q_LOSS: usize = 15;
    pub const STATUS: usize = 16;
    pub const REQUIRED: usize = 17;
}

/// Generator cost matrix column indices.
mod gencost_col {
    pub const MODEL: usize = 0;
    pub const STARTUP: usize = 1;
    pub const SHUTDOWN: usize = 2;
    pub const NCOST: usize = 3;
    pub const COEFF0: usize = 4;
    pub const REQUIRED: usize = 4;
}

fn short_row(field: &'static str, row: usize, expected: usize, got: usize) -> Error {
    Error::ShortRow { field, row, expected, got }
}

/// Parse a bus row into a [`Bus`] plus an optional [`Load`] and [`Shunt`].
/// A load/shunt is emitted only when its values are nonzero, matching MATPOWER:
/// a bus with `Pd = Qd = 0` carries no load. `in_service` follows the bus type
/// (an isolated bus is out of service).
pub(super) fn bus_row(row: &[f64], i: usize) -> Result<(Bus, Option<Load>, Option<Shunt>)> {
    if row.len() < bus_col::REQUIRED {
        return Err(short_row("bus", i, bus_col::REQUIRED, row.len()));
    }
    let id = row[bus_col::BUS_I] as usize;
    let kind = BusType::from_f64(row[bus_col::BUS_TYPE]);
    let in_service = kind != BusType::Isolated;
    let bus = Bus {
        id,
        kind,
        vm: row[bus_col::VM],
        va: row[bus_col::VA],
        base_kv: row[bus_col::BASE_KV],
        vmax: row[bus_col::VMAX],
        vmin: row[bus_col::VMIN],
        area: row[bus_col::BUS_AREA] as usize,
        zone: row[bus_col::ZONE] as usize,
        name: None,
        extras: Extras::new(),
    };
    let (pd, qd) = (row[bus_col::PD], row[bus_col::QD]);
    let load = (pd != 0.0 || qd != 0.0)
        .then(|| Load { bus: id, p: pd, q: qd, in_service, extras: Extras::new() });
    let (gs, bs) = (row[bus_col::GS], row[bus_col::BS]);
    let shunt = (gs != 0.0 || bs != 0.0)
        .then(|| Shunt { bus: id, g: gs, b: bs, in_service, extras: Extras::new() });
    Ok((bus, load, shunt))
}

pub(super) fn branch_row(row: &[f64], i: usize) -> Result<Branch> {
    if row.len() < branch_col::REQUIRED {
        return Err(short_row("branch", i, branch_col::REQUIRED, row.len()));
    }
    Ok(Branch {
        from: row[branch_col::F_BUS] as usize,
        to: row[branch_col::T_BUS] as usize,
        r: row[branch_col::BR_R],
        x: row[branch_col::BR_X],
        b: row[branch_col::BR_B],
        rate_a: row[branch_col::RATE_A],
        rate_b: row[branch_col::RATE_B],
        rate_c: row[branch_col::RATE_C],
        tap: row[branch_col::TAP],
        shift: row[branch_col::SHIFT],
        in_service: row[branch_col::BR_STATUS] == 1.0,
        angmin: row[branch_col::ANGMIN],
        angmax: row[branch_col::ANGMAX],
        extras: Extras::new(),
    })
}

/// Parse a generator row. The cost curve is folded in later from `mpc.gencost`.
/// The MATPOWER capability/ramp columns past `PMIN` ride along as `extras` under
/// their canonical names (the 11 in [`GEN_EXTRA_KEYS`]) so they survive
/// cross-format writes. Any columns beyond those are not retained — same as the
/// pre-dissolution path; the byte-exact MATPOWER round-trip echoes the source.
pub(super) fn gen_row(row: &[f64], i: usize) -> Result<Generator> {
    if row.len() < gen_col::REQUIRED {
        return Err(short_row("gen", i, gen_col::REQUIRED, row.len()));
    }
    let extras: Extras = GEN_EXTRA_KEYS
        .iter()
        .zip(&row[gen_col::REQUIRED..])
        .map(|(&k, &v)| (k.to_string(), num(v)))
        .collect();
    Ok(Generator {
        bus: row[gen_col::GEN_BUS] as usize,
        pg: row[gen_col::PG],
        qg: row[gen_col::QG],
        qmax: row[gen_col::QMAX],
        qmin: row[gen_col::QMIN],
        vg: row[gen_col::VG],
        mbase: row[gen_col::MBASE],
        pmax: row[gen_col::PMAX],
        pmin: row[gen_col::PMIN],
        in_service: row[gen_col::GEN_STATUS] == 1.0,
        cost: None,
        extras,
    })
}

pub(super) fn gencost_row(row: &[f64], i: usize) -> Result<GenCost> {
    if row.len() < gencost_col::REQUIRED {
        return Err(short_row("gencost", i, gencost_col::REQUIRED, row.len()));
    }
    Ok(GenCost {
        model: row[gencost_col::MODEL] as u8,
        startup: row[gencost_col::STARTUP],
        shutdown: row[gencost_col::SHUTDOWN],
        ncost: row[gencost_col::NCOST] as usize,
        coeffs: row[gencost_col::COEFF0..].to_vec(),
    })
}

pub(super) fn storage_row(row: &[f64], i: usize) -> Result<Storage> {
    if row.len() < storage_col::REQUIRED {
        return Err(short_row("storage", i, storage_col::REQUIRED, row.len()));
    }
    Ok(Storage {
        bus: row[storage_col::STORAGE_BUS] as usize,
        ps: row[storage_col::PS],
        qs: row[storage_col::QS],
        energy: row[storage_col::ENERGY],
        energy_rating: row[storage_col::ENERGY_RATING],
        charge_rating: row[storage_col::CHARGE_RATING],
        discharge_rating: row[storage_col::DISCHARGE_RATING],
        charge_efficiency: row[storage_col::CHARGE_EFFICIENCY],
        discharge_efficiency: row[storage_col::DISCHARGE_EFFICIENCY],
        thermal_rating: row[storage_col::THERMAL_RATING],
        qmin: row[storage_col::QMIN],
        qmax: row[storage_col::QMAX],
        r: row[storage_col::R],
        x: row[storage_col::X],
        p_loss: row[storage_col::P_LOSS],
        q_loss: row[storage_col::Q_LOSS],
        in_service: row[storage_col::STATUS] == 1.0,
        extras: Extras::new(),
    })
}

pub(super) fn hvdc_row(row: &[f64], i: usize) -> Result<Hvdc> {
    if row.len() < dcline_col::REQUIRED {
        return Err(short_row("dcline", i, dcline_col::REQUIRED, row.len()));
    }
    Ok(Hvdc {
        from: row[dcline_col::F_BUS] as usize,
        to: row[dcline_col::T_BUS] as usize,
        in_service: row[dcline_col::BR_STATUS] == 1.0,
        pf: row[dcline_col::PF],
        pt: row[dcline_col::PT],
        qf: row[dcline_col::QF],
        qt: row[dcline_col::QT],
        vf: row[dcline_col::VF],
        vt: row[dcline_col::VT],
        pmin: row[dcline_col::PMIN],
        pmax: row[dcline_col::PMAX],
        qminf: row[dcline_col::QMINF],
        qmaxf: row[dcline_col::QMAXF],
        qmint: row[dcline_col::QMINT],
        qmaxt: row[dcline_col::QMAXT],
        loss0: row[dcline_col::LOSS0],
        loss1: row[dcline_col::LOSS1],
        extras: Extras::new(),
    })
}
