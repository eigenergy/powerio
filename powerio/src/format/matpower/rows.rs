//! MATPOWER matrix rows → [`Network`](crate::network) elements.
//!
//! Column layouts are 0-based per the MATPOWER manual. Each `*_row` reads one
//! parsed numeric row into the format-neutral element(s). A bus row fans out
//! into a [`Bus`] plus an optional [`Load`] and [`Shunt`] (MATPOWER folds
//! demand and shunts onto the bus row; the hub keeps them first-class).

use crate::network::{
    Branch, Bus, BusId, BusType, Extras, GEN_EXTRA_KEYS, GenCaps, GenCost, Generator, Hvdc, Load,
    Shunt, Storage,
};
use crate::{Error, Result};

/// MATPOWER in-service flag: the status column is exactly 0 or 1 in the file,
/// so the equality is the intended exact compare.
#[allow(clippy::float_cmp)]
fn is_in_service(status: f64) -> bool {
    status == 1.0
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
    /// Minimum width; the cost coefficients are everything from here on.
    pub const REQUIRED: usize = 4;
}

/// Guard a row's width before indexing it. `field` names the matrix for the
/// error; `i` is the row's 0-based position.
fn require(field: &'static str, row: &[f64], i: usize, expected: usize) -> Result<()> {
    if row.len() < expected {
        return Err(Error::ShortRow {
            field,
            row: i,
            expected,
            got: row.len(),
        });
    }
    Ok(())
}

/// Parse a bus row into a [`Bus`] plus an optional [`Load`] and [`Shunt`].
/// A load/shunt is emitted only when its values are nonzero, matching MATPOWER:
/// a bus with `Pd = Qd = 0` carries no load. `in_service` follows the bus type
/// (an isolated bus is out of service).
pub(super) fn bus_row(row: &[f64], i: usize) -> Result<(Bus, Option<Load>, Option<Shunt>)> {
    require("bus", row, i, bus_col::REQUIRED)?;
    let id = BusId(row[bus_col::BUS_I] as usize);
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
    let load = (pd != 0.0 || qd != 0.0).then(|| Load {
        bus: id,
        p: pd,
        q: qd,
        in_service,
        extras: Extras::new(),
    });
    let (gs, bs) = (row[bus_col::GS], row[bus_col::BS]);
    let shunt = (gs != 0.0 || bs != 0.0).then(|| Shunt {
        bus: id,
        g: gs,
        b: bs,
        in_service,
        extras: Extras::new(),
    });
    Ok((bus, load, shunt))
}

pub(super) fn branch_row(row: &[f64], i: usize) -> Result<Branch> {
    require("branch", row, i, branch_col::REQUIRED)?;
    Ok(Branch {
        from: BusId(row[branch_col::F_BUS] as usize),
        to: BusId(row[branch_col::T_BUS] as usize),
        r: row[branch_col::BR_R],
        x: row[branch_col::BR_X],
        b: row[branch_col::BR_B],
        rate_a: row[branch_col::RATE_A],
        rate_b: row[branch_col::RATE_B],
        rate_c: row[branch_col::RATE_C],
        tap: row[branch_col::TAP],
        shift: row[branch_col::SHIFT],
        in_service: is_in_service(row[branch_col::BR_STATUS]),
        angmin: row[branch_col::ANGMIN],
        angmax: row[branch_col::ANGMAX],
        extras: Extras::new(),
    })
}

/// Parse a generator row. The cost curve is folded in later from `mpc.gencost`.
/// The MATPOWER capability/ramp columns past `PMIN` go into the fixed
/// [`GenCaps`] array, one slot per name in [`GEN_EXTRA_KEYS`] (the 11 of them),
/// so they survive cross-format writes. Any columns beyond those are not
/// retained — the byte-exact MATPOWER round-trip echoes the source.
pub(super) fn gen_row(row: &[f64], i: usize) -> Result<Generator> {
    require("gen", row, i, gen_col::REQUIRED)?;
    // The capability/ramp columns past PMIN, by position, into the fixed GenCaps
    // array — no per-key string allocation. A row that stops early leaves the
    // remaining slots `None`.
    let mut caps: GenCaps = [None; GEN_EXTRA_KEYS.len()];
    for (slot, &v) in caps.iter_mut().zip(&row[gen_col::REQUIRED..]) {
        *slot = Some(v);
    }
    Ok(Generator {
        bus: BusId(row[gen_col::GEN_BUS] as usize),
        pg: row[gen_col::PG],
        qg: row[gen_col::QG],
        qmax: row[gen_col::QMAX],
        qmin: row[gen_col::QMIN],
        vg: row[gen_col::VG],
        mbase: row[gen_col::MBASE],
        pmax: row[gen_col::PMAX],
        pmin: row[gen_col::PMIN],
        in_service: is_in_service(row[gen_col::GEN_STATUS]),
        cost: None,
        caps,
    })
}

pub(super) fn gencost_row(row: &[f64], i: usize) -> Result<GenCost> {
    require("gencost", row, i, gencost_col::REQUIRED)?;
    let model = row[gencost_col::MODEL] as u8;
    let ncost = row[gencost_col::NCOST] as usize;
    // This row's own cost values: `2·ncost` (mw, cost) breakpoints for piecewise
    // (model 1), `ncost` polynomial coefficients (model 2). A gencost matrix that
    // mixes the two is padded with trailing zeros to stay rectangular, so take
    // only this row's values, not the padding. Require the row to actually hold
    // them: a NCOST larger than the row is malformed, and silently truncating it
    // would misrepresent the cost curve.
    let want = if model == 1 { 2 * ncost } else { ncost };
    let start = gencost_col::REQUIRED;
    require("gencost", row, i, start + want)?;
    Ok(GenCost {
        model,
        startup: row[gencost_col::STARTUP],
        shutdown: row[gencost_col::SHUTDOWN],
        ncost,
        coeffs: row[start..start + want].to_vec(),
    })
}

pub(super) fn storage_row(row: &[f64], i: usize) -> Result<Storage> {
    require("storage", row, i, storage_col::REQUIRED)?;
    Ok(Storage {
        bus: BusId(row[storage_col::STORAGE_BUS] as usize),
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
        in_service: is_in_service(row[storage_col::STATUS]),
        extras: Extras::new(),
    })
}

pub(super) fn hvdc_row(row: &[f64], i: usize) -> Result<Hvdc> {
    require("dcline", row, i, dcline_col::REQUIRED)?;
    Ok(Hvdc {
        from: BusId(row[dcline_col::F_BUS] as usize),
        to: BusId(row[dcline_col::T_BUS] as usize),
        in_service: is_in_service(row[dcline_col::BR_STATUS]),
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
