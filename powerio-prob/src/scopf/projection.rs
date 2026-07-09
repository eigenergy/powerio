use std::collections::HashSet;

use powerio::{BusId, Goc3Document};
use serde_json::{Map, Value};

use super::error::ScopfResult;
use super::goc3::{
    Goc3Adapter, cost_cube, float_matrix, float_vec, initial_status, json_error, require_field,
    require_num, require_str,
};
use super::types::{
    ScopfAcContingencySurvivors, ScopfAcLineRow, ScopfAcLineSurvivorRow, ScopfActiveReserveRow,
    ScopfActiveReserveSetRow, ScopfBusRow, ScopfCostRow, ScopfDcContingencyFlowRow, ScopfDcLineRow,
    ScopfDeviceRow, ScopfEnergyWindowMaxCsRow, ScopfEnergyWindowMaxPrRow,
    ScopfEnergyWindowMinCsRow, ScopfEnergyWindowMinPrRow, ScopfEnergyWindowPeriodMaxCsRow,
    ScopfEnergyWindowPeriodMaxPrRow, ScopfEnergyWindowPeriodMinCsRow,
    ScopfEnergyWindowPeriodMinPrRow, ScopfEnergyWindows, ScopfFixedPhaseRow, ScopfFixedRatioRow,
    ScopfInstance, ScopfLengths, ScopfPriceBlockRow, ScopfPriceBlocks, ScopfReactiveReserveRow,
    ScopfReactiveReserveSetRow, ScopfShuntRow, ScopfStaticData, ScopfStaticDataProjection,
    ScopfTransformerRow, ScopfTransformerSurvivorRow, ScopfVariablePhaseRow, ScopfVariableRatioRow,
};

type Result<T> = ScopfResult<T>;

fn validate_period_len(
    kind: &str,
    uid: &str,
    field: &str,
    actual: usize,
    expected: usize,
) -> Result<()> {
    if actual == expected {
        return Ok(());
    }
    Err(json_error(format!(
        "{kind} `{uid}` `{field}` has {actual} periods; expected {expected}"
    )))
}

impl Goc3Adapter {
    fn cost_vector(&self, device_type: &str) -> Result<Vec<ScopfCostRow>> {
        let mut rows = Vec::new();
        for uid in self.sdd_order() {
            let val = self.sdd.get(&uid)?;
            if require_str(val, "device_type")? != device_type {
                continue;
            }
            let ts_val = self.sdd_ts.get(&uid)?;
            let bus = self.goc3_bus_id(require_str(val, "bus")?)?;
            let cost = ts_val.get("cost").ok_or_else(|| {
                json_error(format!(
                    "simple_dispatchable_device time series `{uid}` missing `cost`"
                ))
            })?;
            let cost = cost_cube(cost)?;
            validate_period_len(
                "simple_dispatchable_device time series",
                &uid,
                "cost",
                cost.len(),
                self.dt.len(),
            )?;
            rows.push(ScopfCostRow { bus, uid, cost });
        }
        Ok(rows)
    }

    fn twt_variable_phase(&self) -> Result<Vec<ScopfVariablePhaseRow>> {
        let mut rows = Vec::new();
        for uid in self.twt.uids() {
            let val = self.twt.get(uid)?;
            let (lb, ub) = (require_num(val, "ta_lb")?, require_num(val, "ta_ub")?);
            if lb < ub {
                rows.push(ScopfVariablePhaseRow {
                    j_xf: self.twt.index(uid)?,
                    phi_min: lb,
                    phi_max: ub,
                });
            }
        }
        rows.sort_by_key(|r| r.j_xf);
        Ok(rows)
    }

    fn twt_fixed_phase(&self) -> Result<Vec<ScopfFixedPhaseRow>> {
        let mut rows = Vec::new();
        for uid in self.twt.uids() {
            let val = self.twt.get(uid)?;
            let (lb, ub) = (require_num(val, "ta_lb")?, require_num(val, "ta_ub")?);
            if lb >= ub {
                let phi_o = require_num(initial_status(val)?, "ta")?;
                rows.push(ScopfFixedPhaseRow {
                    j_xf: self.twt.index(uid)?,
                    phi_o,
                });
            }
        }
        rows.sort_by_key(|r| r.j_xf);
        Ok(rows)
    }

    fn twt_variable_ratio(&self) -> Result<Vec<ScopfVariableRatioRow>> {
        let mut rows = Vec::new();
        for uid in self.twt.uids() {
            let val = self.twt.get(uid)?;
            let (lb, ub) = (require_num(val, "tm_lb")?, require_num(val, "tm_ub")?);
            if lb < ub {
                rows.push(ScopfVariableRatioRow {
                    j_xf: self.twt.index(uid)?,
                    tau_min: lb,
                    tau_max: ub,
                });
            }
        }
        rows.sort_by_key(|r| r.j_xf);
        Ok(rows)
    }

    fn twt_fixed_ratio(&self) -> Result<Vec<ScopfFixedRatioRow>> {
        let mut rows = Vec::new();
        for uid in self.twt.uids() {
            let val = self.twt.get(uid)?;
            let (lb, ub) = (require_num(val, "tm_lb")?, require_num(val, "tm_ub")?);
            if lb >= ub {
                let tau_o = require_num(initial_status(val)?, "tm")?;
                rows.push(ScopfFixedRatioRow {
                    j_xf: self.twt.index(uid)?,
                    tau_o,
                });
            }
        }
        rows.sort_by_key(|r| r.j_xf);
        Ok(rows)
    }

    fn sdd_row(&self, uid: &str) -> Result<ScopfDeviceRow> {
        const SDD: &str = "simple_dispatchable_device";
        const SDD_TS: &str = "simple_dispatchable_device time series";
        let val = self.sdd.get(uid)?;
        let ts_val = self.sdd_ts.get(uid)?;
        let initial = initial_status(val)?;
        let ts = |key| require_field(ts_val, SDD_TS, uid, key);
        let row = ScopfDeviceRow {
            bus: self.goc3_bus_id(require_str(val, "bus")?)?,
            uid: uid.to_owned(),
            c_on: require_num(val, "on_cost")?,
            c_su: require_num(val, "startup_cost")?,
            c_sd: require_num(val, "shutdown_cost")?,
            p_ru: require_num(val, "p_ramp_up_ub")?,
            p_rd: require_num(val, "p_ramp_down_ub")?,
            p_ru_su: require_num(val, "p_startup_ramp_ub")?,
            p_rd_sd: require_num(val, "p_shutdown_ramp_ub")?,
            c_rgu: float_vec(ts("p_reg_res_up_cost")?)?,
            c_rgd: float_vec(ts("p_reg_res_down_cost")?)?,
            c_scr: float_vec(ts("p_syn_res_cost")?)?,
            c_nsc: float_vec(ts("p_nsyn_res_cost")?)?,
            c_rru_on: float_vec(ts("p_ramp_res_up_online_cost")?)?,
            c_rru_off: float_vec(ts("p_ramp_res_up_offline_cost")?)?,
            c_rrd_on: float_vec(ts("p_ramp_res_down_online_cost")?)?,
            c_rrd_off: float_vec(ts("p_ramp_res_down_offline_cost")?)?,
            c_qru: float_vec(ts("q_res_up_cost")?)?,
            c_qrd: float_vec(ts("q_res_down_cost")?)?,
            p_rgu_max: require_num(val, "p_reg_res_up_ub")?,
            p_rgd_max: require_num(val, "p_reg_res_down_ub")?,
            p_scr_max: require_num(val, "p_syn_res_ub")?,
            p_nsc_max: require_num(val, "p_nsyn_res_ub")?,
            p_rru_on_max: require_num(val, "p_ramp_res_up_online_ub")?,
            p_rru_off_max: require_num(val, "p_ramp_res_up_offline_ub")?,
            p_rrd_on_max: require_num(val, "p_ramp_res_down_online_ub")?,
            p_rrd_off_max: require_num(val, "p_ramp_res_down_offline_ub")?,
            p_0: require_num(initial, "p")?,
            q_0: require_num(initial, "q")?,
            p_max: float_vec(ts("p_ub")?)?,
            p_min: float_vec(ts("p_lb")?)?,
            q_max: float_vec(ts("q_ub")?)?,
            q_min: float_vec(ts("q_lb")?)?,
            sus: float_matrix(require_field(val, SDD, uid, "startup_states")?)?,
        };
        for (field, actual) in [
            ("p_reg_res_up_cost", row.c_rgu.len()),
            ("p_reg_res_down_cost", row.c_rgd.len()),
            ("p_syn_res_cost", row.c_scr.len()),
            ("p_nsyn_res_cost", row.c_nsc.len()),
            ("p_ramp_res_up_online_cost", row.c_rru_on.len()),
            ("p_ramp_res_up_offline_cost", row.c_rru_off.len()),
            ("p_ramp_res_down_online_cost", row.c_rrd_on.len()),
            ("p_ramp_res_down_offline_cost", row.c_rrd_off.len()),
            ("q_res_up_cost", row.c_qru.len()),
            ("q_res_down_cost", row.c_qrd.len()),
            ("p_ub", row.p_max.len()),
            ("p_lb", row.p_min.len()),
            ("q_ub", row.q_max.len()),
            ("q_lb", row.q_min.len()),
        ] {
            validate_period_len(SDD_TS, uid, field, actual, self.dt.len())?;
        }
        Ok(row)
    }

    fn sdd_rows(&self, device_type: &str) -> Result<Vec<ScopfDeviceRow>> {
        let mut rows = Vec::new();
        for uid in self.sdd_order() {
            if require_str(self.sdd.get(&uid)?, "device_type")? == device_type {
                rows.push(self.sdd_row(&uid)?);
            }
        }
        Ok(rows)
    }

    /// One (bus, zone, device) membership set: `ids` is the sorted reserve
    /// zone uid list (`azr_ids`/`rzr_ids`), `uids_key` names the bus field
    /// listing its zone uids, `device_type` filters the zone's devices. The
    /// Rust equivalent of `reserve_set` in `src/goc3.jl`, iterating buses in
    /// [`Goc3Adapter::bus_order`] and devices in
    /// [`Goc3Adapter::devices_by_bus`] order (see the module-level order
    /// note; `src/goc3.jl` iterates both as `Dict`s here).
    fn reserve_set<R>(
        &self,
        ids: &[String],
        uids_key: &str,
        device_type: &str,
        mkrow: impl Fn(BusId, usize, String) -> R,
    ) -> Result<Vec<R>> {
        let devices_by_bus = self.devices_by_bus()?;
        let bus_order = self.bus_order();
        let mut rows = Vec::new();
        for (zone_index, id) in ids.iter().enumerate() {
            for bus_uid in &bus_order {
                let bus_obj = self.bus.get(bus_uid)?;
                let member = bus_obj
                    .get(uids_key)
                    .and_then(Value::as_array)
                    .is_some_and(|zones| zones.iter().any(|z| z.as_str() == Some(id.as_str())));
                if !member {
                    continue;
                }
                let Some(devices) = devices_by_bus.get(bus_uid) else {
                    continue;
                };
                for dev_uid in devices {
                    let device = self.sdd.get(dev_uid)?;
                    if require_str(device, "device_type")? == device_type {
                        rows.push(mkrow(
                            self.goc3_bus_id(bus_uid)?,
                            zone_index,
                            dev_uid.clone(),
                        ));
                    }
                }
            }
        }
        Ok(rows)
    }
}

/// Build the static SCOPF index sets from parsed GOC3 tables
/// (`_build_static_projection` in `src/goc3.jl`). Pure function of `tables`; no unit
/// commitment solution is used.
// One flat builder mirroring `_build_static_projection`'s single `sc_data` literal
// in `src/goc3.jl`; splitting it into a builder per row family would scatter
// the one-to-one correspondence with the Julia source this port is checked
// against. `additional_shunt` is a discrete 0/1 flag read straight from
// JSON, not an accumulated float, so the exact comparison is intentional.
#[allow(clippy::too_many_lines, clippy::float_cmp)]
fn build_static_projection(tables: &Goc3Adapter) -> Result<ScopfStaticDataProjection> {
    let l_j_xf = tables.twt.uids().len();
    let l_j_ln = tables.ac_line.uids().len();
    let l_j_ac = l_j_ln + l_j_xf;
    let l_j_dc = tables.dc_line.uids().len();
    let l_j_br = l_j_ac + l_j_dc;
    let l_j_cs = tables.sdd_ids_consumer.len();
    let l_j_pr = tables.sdd_ids_producer.len();
    let l_j_cspr = l_j_cs + l_j_pr;
    let l_j_sh = tables.shunt.uids().len();
    let i = tables.bus.uids().len();
    let l_t = tables.dt.len();
    let l_n_p = tables.azr.uids().len();
    let l_n_q = tables.rzr.uids().len();

    let lengths = ScopfLengths {
        l_j_xf,
        l_j_ln,
        l_j_ac,
        l_j_dc,
        l_j_br,
        l_j_cs,
        l_j_pr,
        l_j_cspr,
        l_j_sh,
        i,
        l_t,
        l_n_p,
        l_n_q,
    };

    let mut bus: Vec<ScopfBusRow> = tables
        .bus
        .uids()
        .iter()
        .map(|uid| {
            let val = tables.bus.get(uid)?;
            Ok(ScopfBusRow {
                i: tables.goc3_bus_id(uid)?,
                uid: uid.clone(),
                v_min: require_num(val, "vm_lb")?,
                v_max: require_num(val, "vm_ub")?,
            })
        })
        .collect::<Result<_>>()?;
    bus.sort_by_key(|r| r.i);

    let shunt: Vec<ScopfShuntRow> = tables
        .shunt
        .uids()
        .iter()
        .map(|uid| {
            let val = tables.shunt.get(uid)?;
            Ok(ScopfShuntRow {
                uid: uid.clone(),
                bus: tables.goc3_bus_id(require_str(val, "bus")?)?,
                g_sh: require_num(val, "gs")?,
                b_sh: require_num(val, "bs")?,
            })
        })
        .collect::<Result<_>>()?;
    let mut acl_branch: Vec<ScopfAcLineRow> = tables
        .ac_line
        .uids()
        .iter()
        .enumerate()
        .map(|(j_ln, uid)| {
            let val = tables.ac_line.get(uid)?;
            let (g_sr, b_sr, b_ch, g_fr, g_to, b_fr, b_to) = branch_admittance(val)?;
            Ok(ScopfAcLineRow {
                j_ln,
                uid: uid.clone(),
                to_bus: tables.goc3_bus_id(require_str(val, "to_bus")?)?,
                fr_bus: tables.goc3_bus_id(require_str(val, "fr_bus")?)?,
                c_su: require_num(val, "connection_cost")?,
                c_sd: require_num(val, "disconnection_cost")?,
                s_max: require_num(val, "mva_ub_nom")?,
                g_sr,
                b_sr,
                b_ch,
                g_fr,
                g_to,
                b_fr,
                b_to,
            })
        })
        .collect::<Result<_>>()?;
    acl_branch.sort_by_key(|r| r.j_ln);

    let mut acx_branch: Vec<ScopfTransformerRow> = tables
        .twt
        .uids()
        .iter()
        .enumerate()
        .map(|(j_xf, uid)| {
            let val = tables.twt.get(uid)?;
            let (g_sr, b_sr, b_ch, g_fr, g_to, b_fr, b_to) = branch_admittance(val)?;
            Ok(ScopfTransformerRow {
                j_xf,
                uid: uid.clone(),
                to_bus: tables.goc3_bus_id(require_str(val, "to_bus")?)?,
                fr_bus: tables.goc3_bus_id(require_str(val, "fr_bus")?)?,
                c_su: require_num(val, "connection_cost")?,
                c_sd: require_num(val, "disconnection_cost")?,
                s_max: require_num(val, "mva_ub_nom")?,
                g_sr,
                b_sr,
                b_ch,
                g_fr,
                g_to,
                b_fr,
                b_to,
            })
        })
        .collect::<Result<_>>()?;
    acx_branch.sort_by_key(|r| r.j_xf);

    let mut dc_branch: Vec<ScopfDcLineRow> = tables
        .dc_line
        .uids()
        .iter()
        .enumerate()
        .map(|(j_dc, uid)| {
            let val = tables.dc_line.get(uid)?;
            Ok(ScopfDcLineRow {
                j_dc,
                uid: uid.clone(),
                pdc_max: require_num(val, "pdc_ub")?,
                qdc_fr_min: require_num(val, "qdc_fr_lb")?,
                qdc_to_min: require_num(val, "qdc_to_lb")?,
                qdc_fr_max: require_num(val, "qdc_fr_ub")?,
                qdc_to_max: require_num(val, "qdc_to_ub")?,
                to_bus: tables.goc3_bus_id(require_str(val, "to_bus")?)?,
                fr_bus: tables.goc3_bus_id(require_str(val, "fr_bus")?)?,
            })
        })
        .collect::<Result<_>>()?;
    dc_branch.sort_by_key(|r| r.j_dc);

    let cost_vector_pr = tables.cost_vector("producer")?;
    let cost_vector_cs = tables.cost_vector("consumer")?;
    let prod = tables.sdd_rows("producer")?;
    let cons = tables.sdd_rows("consumer")?;

    let mut active_reserve: Vec<ScopfActiveReserveRow> = tables
        .azr
        .uids()
        .iter()
        .enumerate()
        .map(|(n_p, uid)| {
            let val = tables.azr.get(uid)?;
            let ts_val = tables.azr_ts.get(uid)?;
            let row = ScopfActiveReserveRow {
                n_p,
                uid: uid.clone(),
                c_rgu: require_num(val, "REG_UP_vio_cost")?,
                c_rgd: require_num(val, "REG_DOWN_vio_cost")?,
                c_scr: require_num(val, "SYN_vio_cost")?,
                c_nsc: require_num(val, "NSYN_vio_cost")?,
                c_rru: require_num(val, "RAMPING_RESERVE_UP_vio_cost")?,
                c_rrd: require_num(val, "RAMPING_RESERVE_DOWN_vio_cost")?,
                sigma_rgu: require_num(val, "REG_UP")?,
                sigma_rgd: require_num(val, "REG_DOWN")?,
                sigma_scr: require_num(val, "SYN")?,
                sigma_nsc: require_num(val, "NSYN")?,
                p_rru_min: float_vec(ts_val.get("RAMPING_RESERVE_UP").ok_or_else(|| {
                    json_error(format!(
                        "active_zonal_reserve time series `{uid}` missing `RAMPING_RESERVE_UP`"
                    ))
                })?)?,
                p_rrd_min: float_vec(ts_val.get("RAMPING_RESERVE_DOWN").ok_or_else(|| {
                    json_error(format!(
                        "active_zonal_reserve time series `{uid}` missing `RAMPING_RESERVE_DOWN`"
                    ))
                })?)?,
            };
            validate_period_len(
                "active_zonal_reserve time series",
                uid,
                "RAMPING_RESERVE_UP",
                row.p_rru_min.len(),
                tables.dt.len(),
            )?;
            validate_period_len(
                "active_zonal_reserve time series",
                uid,
                "RAMPING_RESERVE_DOWN",
                row.p_rrd_min.len(),
                tables.dt.len(),
            )?;
            Ok(row)
        })
        .collect::<Result<_>>()?;
    active_reserve.sort_by_key(|r| r.n_p);

    let mut reactive_reserve: Vec<ScopfReactiveReserveRow> = tables
        .rzr
        .uids()
        .iter()
        .enumerate()
        .map(|(n_q, uid)| {
            let val = tables.rzr.get(uid)?;
            let ts_val = tables.rzr_ts.get(uid)?;
            let row = ScopfReactiveReserveRow {
                n_q,
                uid: uid.clone(),
                c_qru: require_num(val, "REACT_UP_vio_cost")?,
                c_qrd: require_num(val, "REACT_DOWN_vio_cost")?,
                q_qru_min: float_vec(ts_val.get("REACT_UP").ok_or_else(|| {
                    json_error(format!(
                        "reactive_zonal_reserve time series `{uid}` missing `REACT_UP`"
                    ))
                })?)?,
                q_qrd_min: float_vec(ts_val.get("REACT_DOWN").ok_or_else(|| {
                    json_error(format!(
                        "reactive_zonal_reserve time series `{uid}` missing `REACT_DOWN`"
                    ))
                })?)?,
            };
            validate_period_len(
                "reactive_zonal_reserve time series",
                uid,
                "REACT_UP",
                row.q_qru_min.len(),
                tables.dt.len(),
            )?;
            validate_period_len(
                "reactive_zonal_reserve time series",
                uid,
                "REACT_DOWN",
                row.q_qrd_min.len(),
                tables.dt.len(),
            )?;
            Ok(row)
        })
        .collect::<Result<_>>()?;
    reactive_reserve.sort_by_key(|r| r.n_q);

    let active_reserve_set_pr = tables.reserve_set(
        &tables.azr_ids,
        "active_reserve_uids",
        "producer",
        |i, n_p, uid| ScopfActiveReserveSetRow { i, n_p, uid },
    )?;
    let active_reserve_set_cs = tables.reserve_set(
        &tables.azr_ids,
        "active_reserve_uids",
        "consumer",
        |i, n_p, uid| ScopfActiveReserveSetRow { i, n_p, uid },
    )?;
    let reactive_reserve_set_pr = tables.reserve_set(
        &tables.rzr_ids,
        "reactive_reserve_uids",
        "producer",
        |i, n_q, uid| ScopfReactiveReserveSetRow { i, n_q, uid },
    )?;
    let reactive_reserve_set_cs = tables.reserve_set(
        &tables.rzr_ids,
        "reactive_reserve_uids",
        "consumer",
        |i, n_q, uid| ScopfReactiveReserveSetRow { i, n_q, uid },
    )?;

    let static_data = ScopfStaticData {
        bus,
        shunt,
        acl_branch,
        acx_branch,
        vpd: tables.twt_variable_phase()?,
        fpd: tables.twt_fixed_phase()?,
        vwr: tables.twt_variable_ratio()?,
        fwr: tables.twt_fixed_ratio()?,
        dc_branch,
        prod,
        cons,
        active_reserve,
        reactive_reserve,
        active_reserve_set_pr,
        active_reserve_set_cs,
        reactive_reserve_set_pr,
        reactive_reserve_set_cs,
    };

    Ok(ScopfStaticDataProjection {
        static_data,
        lengths,
        cost_vector_pr,
        cost_vector_cs,
    })
}

fn interval_midpoints(dt: &[f64]) -> Vec<f64> {
    let mut a_end = 0.0;
    dt.iter()
        .map(|d| {
            let start = a_end;
            a_end += d;
            f64::midpoint(start, a_end)
        })
        .collect()
}

/// One `(window_index, uid, start, end, bound)` row, before it is packed
/// into a [`ScopfEnergyWindowMaxPrRow`]-family struct.
type EnergyWindowTuple = (usize, String, f64, f64, f64);
/// One `(window_index, uid, period, duration)` row, before it is packed into
/// a [`ScopfEnergyWindowPeriodMaxPrRow`]-family struct.
type EnergyWindowPeriodTuple = (usize, String, usize, f64);

/// One energy-requirement window set and its per-period membership rows in
/// one pass: `device_type`/`req_key` select the producer/consumer max/min
/// window list. The Rust equivalent of `windows` and `window_periods`
/// together in `src/goc3.jl`'s `_build_energy_windows` (there, two separate
/// passes over the same device/window set; fused here since a window row and
/// its period memberships come from the same parsed `(start, end, bound)`).
/// Device iteration uses [`Goc3Adapter::sdd_order`] (see the module-level
/// order note; `src/goc3.jl` iterates `keys(data.sdd_lookup)`, a `Dict`,
/// here).
fn sdd_windows(
    tables: &Goc3Adapter,
    a_mid: &[f64],
    device_type: &str,
    req_key: &str,
    eps: f64,
) -> Result<(Vec<EnergyWindowTuple>, Vec<EnergyWindowPeriodTuple>)> {
    let mut windows = Vec::new();
    let mut window_periods = Vec::new();
    let mut ind = 0usize;
    for uid in tables.sdd_order() {
        let val = tables.sdd.get(&uid)?;
        if require_str(val, "device_type")? != device_type {
            continue;
        }
        let req = require_field(val, "simple_dispatchable_device", &uid, req_key)?
            .as_array()
            .ok_or_else(|| {
                json_error(format!(
                    "simple_dispatchable_device `{uid}` `{req_key}` is not an array"
                ))
            })?;
        for w in req {
            let w = float_vec(w)?;
            let [start, end, bound] = w[..] else {
                return Err(json_error(format!(
                    "simple_dispatchable_device `{uid}` `{req_key}` window is not a 3-element array"
                )));
            };
            windows.push((ind, uid.clone(), start, end, bound));
            for (t0, &m) in a_mid.iter().enumerate() {
                if start + eps < m && m <= end + eps {
                    window_periods.push((ind, uid.clone(), t0, tables.dt[t0]));
                }
            }
            ind += 1;
        }
    }
    Ok((windows, window_periods))
}

/// Build the multi-interval energy requirement window sets and their
/// per-period membership sets (`_build_energy_windows` in `src/goc3.jl`).
/// Pure function of `tables`.
// Four max/min x producer/consumer variants, each packed into its own
// distinctly-named row struct to keep Julia's exact field spelling on the
// wire (see the module doc comment); the packing is what pushes this over
// the line budget.
#[allow(clippy::too_many_lines)]
fn build_energy_windows(tables: &Goc3Adapter) -> Result<ScopfEnergyWindows> {
    const EPS_TIME: f64 = 1e-6;
    let a_mid = interval_midpoints(&tables.dt);

    let (max_pr, t_max_pr) = sdd_windows(tables, &a_mid, "producer", "energy_req_ub", EPS_TIME)?;
    let (max_cs, t_max_cs) = sdd_windows(tables, &a_mid, "consumer", "energy_req_ub", EPS_TIME)?;
    let (min_pr, t_min_pr) = sdd_windows(tables, &a_mid, "producer", "energy_req_lb", EPS_TIME)?;
    let (min_cs, t_min_cs) = sdd_windows(tables, &a_mid, "consumer", "energy_req_lb", EPS_TIME)?;

    let w_en_max_pr = max_pr
        .into_iter()
        .map(
            |(w_en_max_pr_ind, uid, a_en_max_start, a_en_max_end, e_max)| {
                ScopfEnergyWindowMaxPrRow {
                    w_en_max_pr_ind,
                    uid,
                    a_en_max_start,
                    a_en_max_end,
                    e_max,
                }
            },
        )
        .collect();
    let w_en_max_cs = max_cs
        .into_iter()
        .map(
            |(w_en_max_cs_ind, uid, a_en_max_start, a_en_max_end, e_max)| {
                ScopfEnergyWindowMaxCsRow {
                    w_en_max_cs_ind,
                    uid,
                    a_en_max_start,
                    a_en_max_end,
                    e_max,
                }
            },
        )
        .collect();
    let w_en_min_pr = min_pr
        .into_iter()
        .map(
            |(w_en_min_pr_ind, uid, a_en_min_start, a_en_min_end, e_min)| {
                ScopfEnergyWindowMinPrRow {
                    w_en_min_pr_ind,
                    uid,
                    a_en_min_start,
                    a_en_min_end,
                    e_min,
                }
            },
        )
        .collect();
    let w_en_min_cs = min_cs
        .into_iter()
        .map(
            |(w_en_min_cs_ind, uid, a_en_min_start, a_en_min_end, e_min)| {
                ScopfEnergyWindowMinCsRow {
                    w_en_min_cs_ind,
                    uid,
                    a_en_min_start,
                    a_en_min_end,
                    e_min,
                }
            },
        )
        .collect();

    let t_w_en_max_pr = t_max_pr
        .into_iter()
        .map(
            |(w_en_max_pr_ind, uid, t, dt)| ScopfEnergyWindowPeriodMaxPrRow {
                w_en_max_pr_ind,
                uid,
                t,
                dt,
            },
        )
        .collect();
    let t_w_en_max_cs = t_max_cs
        .into_iter()
        .map(
            |(w_en_max_cs_ind, uid, t, dt)| ScopfEnergyWindowPeriodMaxCsRow {
                w_en_max_cs_ind,
                uid,
                t,
                dt,
            },
        )
        .collect();
    let t_w_en_min_pr = t_min_pr
        .into_iter()
        .map(
            |(w_en_min_pr_ind, uid, t, dt)| ScopfEnergyWindowPeriodMinPrRow {
                w_en_min_pr_ind,
                uid,
                t,
                dt,
            },
        )
        .collect();
    let t_w_en_min_cs = t_min_cs
        .into_iter()
        .map(
            |(w_en_min_cs_ind, uid, t, dt)| ScopfEnergyWindowPeriodMinCsRow {
                w_en_min_cs_ind,
                uid,
                t,
                dt,
            },
        )
        .collect();

    Ok(ScopfEnergyWindows {
        w_en_max_pr,
        w_en_max_cs,
        w_en_min_pr,
        w_en_min_cs,
        t_w_en_max_pr,
        t_w_en_max_cs,
        t_w_en_min_pr,
        t_w_en_min_cs,
    })
}

fn flatten_price_blocks(cost_vector: &[ScopfCostRow]) -> Vec<ScopfPriceBlockRow> {
    let mut rows = Vec::new();
    let mut flat_k = 0usize;
    for pc in cost_vector {
        for (t0, cost_t) in pc.cost.iter().enumerate() {
            for (m0, cost_tm) in cost_t.iter().enumerate() {
                let (c_en, p_max) = (cost_tm[0], cost_tm[1]);
                rows.push(ScopfPriceBlockRow {
                    flat_k,
                    uid: pc.uid.clone(),
                    t: t0,
                    m: m0,
                    c_en,
                    p_max,
                });
                flat_k += 1;
            }
        }
    }
    rows
}

/// Flatten the per-device energy cost curves into one row per (device,
/// period, cost block), unscaled in the GOC3 document's own per-unit
/// convention (`_build_price_blocks` in `src/goc3.jl`). Pure function of the
/// cost vectors [`build_static_projection`] returns; infallible, since
/// [`ScopfCostRow::cost`](ScopfCostRow) is already validated numeric data.
fn build_price_blocks(
    cost_vector_pr: &[ScopfCostRow],
    cost_vector_cs: &[ScopfCostRow],
) -> ScopfPriceBlocks {
    ScopfPriceBlocks {
        producer: flatten_price_blocks(cost_vector_pr),
        consumer: flatten_price_blocks(cost_vector_cs),
    }
}

fn b_sr(r: f64, x: f64) -> f64 {
    -x / (x * x + r * r)
}

/// Series admittance and terminal shunt parameters shared by AC lines and
/// transformers: `(g_sr, b_sr, b_ch, g_fr, g_to, b_fr, b_to)`, from
/// `r`/`x`/`b` and, when `additional_shunt` is set, `g_fr`/`g_to`/`b_fr`/
/// `b_to`. The common body of `acl_branch`/`acx_branch` in
/// `_build_static_projection` (`src/goc3.jl`). `additional_shunt` is a discrete 0/1
/// flag read straight from JSON, not an accumulated float, so the exact
/// comparison is intentional.
#[allow(clippy::type_complexity, clippy::float_cmp)]
fn branch_admittance(val: &Map<String, Value>) -> Result<(f64, f64, f64, f64, f64, f64, f64)> {
    let (r, x) = (require_num(val, "r")?, require_num(val, "x")?);
    let g_sr = r / (x * x + r * r);
    let additional_shunt = require_num(val, "additional_shunt")? == 1.0;
    let (g_fr, g_to, b_fr, b_to) = if additional_shunt {
        (
            require_num(val, "g_fr")?,
            require_num(val, "g_to")?,
            require_num(val, "b_fr")?,
            require_num(val, "b_to")?,
        )
    } else {
        (0.0, 0.0, 0.0, 0.0)
    };
    Ok((
        g_sr,
        b_sr(r, x),
        require_num(val, "b")?,
        g_fr,
        g_to,
        b_fr,
        b_to,
    ))
}

/// One contingency index and its outaged component UIDs.
fn contingency_outages(ctg_idx: usize, ctg: &Value) -> Result<(usize, HashSet<&str>)> {
    let ctg_obj = ctg
        .as_object()
        .ok_or_else(|| json_error("reliability.contingency item is not an object"))?;
    let ctg_uid = require_str(ctg_obj, "uid")?;
    let outaged = require_field(ctg_obj, "contingency", ctg_uid, "components")?
        .as_array()
        .ok_or_else(|| {
            json_error(format!(
                "contingency `{ctg_uid}` `components` is not an array"
            ))
        })?
        .iter()
        .map(|v| {
            v.as_str()
                .ok_or_else(|| json_error("component uid is not a string"))
        })
        .collect::<Result<_>>()?;
    Ok((ctg_idx, outaged))
}

/// Enumerate, for each contingency, the AC lines and transformers that
/// remain in service: the branch is not among the contingency's outaged
/// components (`_build_ac_contingency_survivors` in `src/goc3.jl`). The outer
/// vector follows `reliability.contingency`'s document order (which need not
/// match ascending `ctg`); rows within one contingency follow the section's
/// document order (see the module-level order note; `src/goc3.jl` iterates
/// `values(lookup)`, a `Dict`, here).
fn build_ac_contingency_survivors(tables: &Goc3Adapter) -> Result<ScopfAcContingencySurvivors> {
    let contingencies = tables.contingencies()?;

    let mut ln = Vec::with_capacity(contingencies.len());
    let mut xf = Vec::with_capacity(contingencies.len());
    for (ctg_idx, ctg) in contingencies.iter().enumerate() {
        let (ctg_idx, outaged) = contingency_outages(ctg_idx, ctg)?;

        let mut ln_rows = Vec::new();
        for uid in tables.ac_line.uids() {
            if outaged.contains(uid.as_str()) {
                continue;
            }
            let val = tables.ac_line.get(uid)?;
            let (r, x) = (require_num(val, "r")?, require_num(val, "x")?);
            ln_rows.push(ScopfAcLineSurvivorRow {
                ctg: ctg_idx,
                j_ln: tables.ac_line.index(uid)?,
                uid: uid.clone(),
                to_bus: tables.goc3_bus_id(require_str(val, "to_bus")?)?,
                fr_bus: tables.goc3_bus_id(require_str(val, "fr_bus")?)?,
                b_sr: b_sr(r, x),
                s_max_ctg: require_num(val, "mva_ub_em")?,
            });
        }
        ln.push(ln_rows);

        let mut xf_rows = Vec::new();
        for uid in tables.twt.uids() {
            if outaged.contains(uid.as_str()) {
                continue;
            }
            let val = tables.twt.get(uid)?;
            let (r, x) = (require_num(val, "r")?, require_num(val, "x")?);
            xf_rows.push(ScopfTransformerSurvivorRow {
                ctg: ctg_idx,
                j_xf: tables.twt.index(uid)?,
                uid: uid.clone(),
                to_bus: tables.goc3_bus_id(require_str(val, "to_bus")?)?,
                fr_bus: tables.goc3_bus_id(require_str(val, "fr_bus")?)?,
                b_sr: b_sr(r, x),
                s_max_ctg: require_num(val, "mva_ub_em")?,
            });
        }
        xf.push(xf_rows);
    }

    Ok(ScopfAcContingencySurvivors { ln, xf })
}

fn build_dc_contingency_flows(tables: &Goc3Adapter) -> Result<Vec<ScopfDcContingencyFlowRow>> {
    let contingencies = tables.contingencies()?;
    let mut rows = Vec::new();
    let mut flat_jtk_dc = 0usize;
    for (ctg_idx, ctg) in contingencies.iter().enumerate() {
        let (ctg_idx, outaged) = contingency_outages(ctg_idx, ctg)?;

        for (t0, &dt) in tables.dt.iter().enumerate() {
            for uid in tables.dc_line.uids() {
                if outaged.contains(uid.as_str()) {
                    continue;
                }
                let val = tables.dc_line.get(uid)?;
                rows.push(ScopfDcContingencyFlowRow {
                    flat_jtk_dc,
                    ctg: ctg_idx,
                    j_dc: tables.dc_line.index(uid)?,
                    to_bus: tables.goc3_bus_id(require_str(val, "to_bus")?)?,
                    fr_bus: tables.goc3_bus_id(require_str(val, "fr_bus")?)?,
                    t: t0,
                    dt,
                });
                flat_jtk_dc += 1;
            }
        }
    }
    Ok(rows)
}

fn project_scopf_instance(tables: &Goc3Adapter) -> Result<ScopfInstance> {
    let ScopfStaticDataProjection {
        static_data,
        lengths,
        cost_vector_pr,
        cost_vector_cs,
    } = build_static_projection(tables)?;
    Ok(ScopfInstance {
        static_data,
        lengths,
        energy_windows: build_energy_windows(tables)?,
        price_blocks: build_price_blocks(&cost_vector_pr, &cost_vector_cs),
        ac_contingency_survivors: build_ac_contingency_survivors(tables)?,
        dc_contingency_flows: build_dc_contingency_flows(tables)?,
    })
}

/// Build a matrix free SCOPF instance from one parsed GOC3 document.
pub fn build_scopf_instance(document: &Goc3Document) -> Result<ScopfInstance> {
    let tables = Goc3Adapter::from_document(document)?;
    project_scopf_instance(&tables)
}

/// Parse GOC3 JSON text and build its SCOPF instance.
pub fn build_scopf_instance_from_str(text: &str) -> Result<ScopfInstance> {
    let document = Goc3Document::parse(text)?;
    build_scopf_instance(&document)
}
