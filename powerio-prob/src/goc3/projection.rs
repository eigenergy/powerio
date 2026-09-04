use powerio_core::ComponentId;
use powerio_tx::__internal::Goc3Document;
use serde_json::{Map, Value};

use super::decode::{
    Goc3Adapter, cost_cube, float_matrix, float_vec, initial_status, json_error, require_binary,
    require_field, require_num, require_str,
};
#[cfg(test)]
use super::error::Goc3Error;
use super::error::Goc3Result;
use crate::instance::scuc_inputs::{
    ScucActiveReserveZone, ScucBranchSwitchingCost, ScucContingency, ScucDevice, ScucDeviceKind,
    ScucDevicePeriod, ScucEnergyCostBlock, ScucEnergyRequirement, ScucInitialCommitment,
    ScucInputs, ScucRampLimits, ScucReactiveCapability, ScucReactiveReserveZone, ScucReserveCosts,
    ScucReserveLimits, ScucShunt, ScucStartupCostAdjustment, ScucStartupLimit,
    ScucTransformerControl, ScucViolationCosts,
};

type Result<T> = Goc3Result<T>;

fn component_id(component_type: &str, uid: &str) -> Result<ComponentId> {
    ComponentId::new(component_type, uid).map_err(|error| json_error(error.to_string()))
}

fn device_kind(obj: &Map<String, Value>) -> ScucDeviceKind {
    if obj.get("device_type").and_then(Value::as_str) == Some("consumer") {
        ScucDeviceKind::Consumer
    } else {
        ScucDeviceKind::Producer
    }
}

fn require_flag(obj: &Map<String, Value>, uid: &str, key: &str) -> Result<bool> {
    match obj.get(key).and_then(Value::as_u64) {
        Some(0) => Ok(false),
        Some(1) => Ok(true),
        _ => Err(json_error(format!(
            "simple_dispatchable_device `{uid}` `{key}` is not 0 or 1"
        ))),
    }
}

fn binary_vec(value: &Value, what: &str) -> Result<Vec<bool>> {
    value
        .as_array()
        .ok_or_else(|| json_error(format!("{what} is not an array")))?
        .iter()
        .enumerate()
        .map(|(index, value)| match value.as_u64() {
            Some(0) => Ok(false),
            Some(1) => Ok(true),
            _ => Err(json_error(format!(
                "{what}[{index}] is not the binary integer 0 or 1"
            ))),
        })
        .collect()
}

fn startup_cost_adjustments(value: &Value, uid: &str) -> Result<Vec<ScucStartupCostAdjustment>> {
    float_matrix(value)?
        .into_iter()
        .map(|fields| match fields.as_slice() {
            [cost, maximum_down_time] => Ok(ScucStartupCostAdjustment {
                cost: *cost,
                maximum_down_time: *maximum_down_time,
            }),
            _ => Err(json_error(format!(
                "simple_dispatchable_device `{uid}` `startup_states` entry does not have two fields"
            ))),
        })
        .collect()
}

fn startup_limits(value: &Value, uid: &str) -> Result<Vec<ScucStartupLimit>> {
    value
        .as_array()
        .ok_or_else(|| {
            json_error(format!(
                "simple_dispatchable_device `{uid}` `startups_ub` is not an array"
            ))
        })?
        .iter()
        .enumerate()
        .map(|(index, window)| {
            let fields = window.as_array().ok_or_else(|| {
                json_error(format!(
                    "simple_dispatchable_device `{uid}` `startups_ub[{index}]` is not an array"
                ))
            })?;
            if fields.len() != 3 {
                return Err(json_error(format!(
                    "simple_dispatchable_device `{uid}` `startups_ub[{index}]` has {} fields; expected 3",
                    fields.len()
                )));
            }
            Ok(ScucStartupLimit {
                start_time: fields[0]
                    .as_f64()
                    .expect("input validation established a finite number"),
                end_time: fields[1]
                    .as_f64()
                    .expect("input validation established a finite number"),
                maximum_startups: fields[2]
                    .as_u64()
                    .expect("input validation established a nonnegative integer"),
            })
        })
        .collect()
}

fn energy_requirements(
    value: &Value,
    uid: &str,
    field: &str,
) -> Result<Vec<ScucEnergyRequirement>> {
    float_matrix(value)?
        .into_iter()
        .map(|fields| match fields.as_slice() {
            [start_time, end_time, energy] => Ok(ScucEnergyRequirement {
                start_time: *start_time,
                end_time: *end_time,
                energy: *energy,
            }),
            _ => Err(json_error(format!(
                "simple_dispatchable_device `{uid}` `{field}` entry does not have three fields"
            ))),
        })
        .collect()
}

fn reactive_capability(obj: &Map<String, Value>, uid: &str) -> Result<ScucReactiveCapability> {
    let linear = require_flag(obj, uid, "q_linear_cap")?;
    let bounded = require_flag(obj, uid, "q_bound_cap")?;
    match (linear, bounded) {
        (false, false) => Ok(ScucReactiveCapability::None),
        (true, false) => Ok(ScucReactiveCapability::Linear {
            reactive_power_at_zero_active_power: require_num(obj, "q_0")?,
            slope: require_num(obj, "beta")?,
        }),
        (false, true) => Ok(ScucReactiveCapability::Bounded {
            reactive_power_at_zero_active_power_min: require_num(obj, "q_0_lb")?,
            reactive_power_at_zero_active_power_max: require_num(obj, "q_0_ub")?,
            slope_min: require_num(obj, "beta_lb")?,
            slope_max: require_num(obj, "beta_ub")?,
        }),
        (true, true) => Err(json_error(format!(
            "simple_dispatchable_device `{uid}` selects two reactive capability forms"
        ))),
    }
}

fn device_periods(
    tables: &Goc3Adapter,
    uid: &str,
    series: &Map<String, Value>,
) -> Result<Vec<ScucDevicePeriod>> {
    const SECTION: &str = "simple_dispatchable_device time series";
    let series_field = |key| require_field(series, SECTION, uid, key);
    let on_status_min = binary_vec(
        series_field("on_status_lb")?,
        &format!("{SECTION} `{uid}` `on_status_lb`"),
    )?;
    let on_status_max = binary_vec(
        series_field("on_status_ub")?,
        &format!("{SECTION} `{uid}` `on_status_ub`"),
    )?;
    let active_power_min = float_vec(series_field("p_lb")?)?;
    let active_power_max = float_vec(series_field("p_ub")?)?;
    let reactive_power_min = float_vec(series_field("q_lb")?)?;
    let reactive_power_max = float_vec(series_field("q_ub")?)?;
    let costs = cost_cube(series_field("cost")?)?;
    let regulation_up = float_vec(series_field("p_reg_res_up_cost")?)?;
    let regulation_down = float_vec(series_field("p_reg_res_down_cost")?)?;
    let synchronized = float_vec(series_field("p_syn_res_cost")?)?;
    let nonsynchronized = float_vec(series_field("p_nsyn_res_cost")?)?;
    let ramping_up_online = float_vec(series_field("p_ramp_res_up_online_cost")?)?;
    let ramping_down_online = float_vec(series_field("p_ramp_res_down_online_cost")?)?;
    let ramping_up_offline = float_vec(series_field("p_ramp_res_up_offline_cost")?)?;
    let ramping_down_offline = float_vec(series_field("p_ramp_res_down_offline_cost")?)?;
    let reactive_up = float_vec(series_field("q_res_up_cost")?)?;
    let reactive_down = float_vec(series_field("q_res_down_cost")?)?;

    let periods = tables.dt.len();
    for (field, actual) in [
        ("on_status_lb", on_status_min.len()),
        ("on_status_ub", on_status_max.len()),
        ("p_lb", active_power_min.len()),
        ("p_ub", active_power_max.len()),
        ("q_lb", reactive_power_min.len()),
        ("q_ub", reactive_power_max.len()),
        ("cost", costs.len()),
        ("p_reg_res_up_cost", regulation_up.len()),
        ("p_reg_res_down_cost", regulation_down.len()),
        ("p_syn_res_cost", synchronized.len()),
        ("p_nsyn_res_cost", nonsynchronized.len()),
        ("p_ramp_res_up_online_cost", ramping_up_online.len()),
        ("p_ramp_res_down_online_cost", ramping_down_online.len()),
        ("p_ramp_res_up_offline_cost", ramping_up_offline.len()),
        ("p_ramp_res_down_offline_cost", ramping_down_offline.len()),
        ("q_res_up_cost", reactive_up.len()),
        ("q_res_down_cost", reactive_down.len()),
    ] {
        if actual != periods {
            return Err(json_error(format!(
                "{SECTION} `{uid}` `{field}` has {actual} periods; expected {periods}"
            )));
        }
    }

    Ok((0..periods)
        .map(|period| ScucDevicePeriod {
            on_status_min: on_status_min[period],
            on_status_max: on_status_max[period],
            active_power_min: active_power_min[period],
            active_power_max: active_power_max[period],
            reactive_power_min: reactive_power_min[period],
            reactive_power_max: reactive_power_max[period],
            energy_cost_blocks: costs[period]
                .iter()
                .map(|block| ScucEnergyCostBlock {
                    marginal_cost: block[0],
                    block_size: block[1],
                })
                .collect(),
            reserve_costs: ScucReserveCosts {
                regulation_up: regulation_up[period],
                regulation_down: regulation_down[period],
                synchronized: synchronized[period],
                nonsynchronized: nonsynchronized[period],
                ramping_up_online: ramping_up_online[period],
                ramping_down_online: ramping_down_online[period],
                ramping_up_offline: ramping_up_offline[period],
                ramping_down_offline: ramping_down_offline[period],
                reactive_up: reactive_up[period],
                reactive_down: reactive_down[period],
            },
        })
        .collect())
}

fn build_devices(tables: &Goc3Adapter) -> Result<Vec<ScucDevice>> {
    const SECTION: &str = "simple_dispatchable_device";
    tables
        .sdd_order()
        .into_iter()
        .map(|uid| {
            let value = tables.sdd.get(&uid)?;
            let series = tables.sdd_ts.get(&uid)?;
            let initial = initial_status(value)?;
            let kind = device_kind(value);
            let component_type = match kind {
                ScucDeviceKind::Producer => "generator",
                ScucDeviceKind::Consumer => "load",
            };
            Ok(ScucDevice {
                id: component_id(component_type, &uid)?,
                kind,
                on_cost: require_num(value, "on_cost")?,
                startup_cost: require_num(value, "startup_cost")?,
                startup_cost_adjustments: startup_cost_adjustments(
                    require_field(value, SECTION, &uid, "startup_states")?,
                    &uid,
                )?,
                shutdown_cost: require_num(value, "shutdown_cost")?,
                startup_limits: startup_limits(
                    require_field(value, SECTION, &uid, "startups_ub")?,
                    &uid,
                )?,
                energy_upper_bounds: energy_requirements(
                    require_field(value, SECTION, &uid, "energy_req_ub")?,
                    &uid,
                    "energy_req_ub",
                )?,
                energy_lower_bounds: energy_requirements(
                    require_field(value, SECTION, &uid, "energy_req_lb")?,
                    &uid,
                    "energy_req_lb",
                )?,
                minimum_up_time: require_num(value, "in_service_time_lb")?,
                minimum_down_time: require_num(value, "down_time_lb")?,
                ramp_limits: ScucRampLimits {
                    up: require_num(value, "p_ramp_up_ub")?,
                    down: require_num(value, "p_ramp_down_ub")?,
                    startup: require_num(value, "p_startup_ramp_ub")?,
                    shutdown: require_num(value, "p_shutdown_ramp_ub")?,
                },
                reserve_limits: ScucReserveLimits {
                    regulation_up: require_num(value, "p_reg_res_up_ub")?,
                    regulation_down: require_num(value, "p_reg_res_down_ub")?,
                    synchronized: require_num(value, "p_syn_res_ub")?,
                    nonsynchronized: require_num(value, "p_nsyn_res_ub")?,
                    ramping_up_online: require_num(value, "p_ramp_res_up_online_ub")?,
                    ramping_down_online: require_num(value, "p_ramp_res_down_online_ub")?,
                    ramping_up_offline: require_num(value, "p_ramp_res_up_offline_ub")?,
                    ramping_down_offline: require_num(value, "p_ramp_res_down_offline_ub")?,
                },
                initial_on_status: require_binary(initial, "on_status")?,
                initial_commitment: ScucInitialCommitment {
                    accumulated_up_time: require_num(initial, "accu_up_time")?,
                    accumulated_down_time: require_num(initial, "accu_down_time")?,
                },
                reactive_capability: reactive_capability(value, &uid)?,
                periods: device_periods(tables, &uid, series)?,
            })
        })
        .collect()
}

fn build_shunts(tables: &Goc3Adapter) -> Result<Vec<ScucShunt>> {
    tables
        .shunt
        .uids()
        .iter()
        .map(|uid| {
            let value = tables.shunt.get(uid)?;
            Ok(ScucShunt {
                id: component_id("shunt", uid)?,
                conductance_per_step: require_num(value, "gs")?,
                susceptance_per_step: require_num(value, "bs")?,
                step_min: super::decode::require_i64(value, "step_lb")?,
                step_max: super::decode::require_i64(value, "step_ub")?,
                initial_step: super::decode::require_i64(initial_status(value)?, "step")?,
            })
        })
        .collect()
}

fn build_branch_switching_costs(tables: &Goc3Adapter) -> Result<Vec<ScucBranchSwitchingCost>> {
    let mut rows = Vec::with_capacity(tables.ac_line.uids().len() + tables.twt.uids().len());
    for (section, component_type) in [(&tables.ac_line, "branch"), (&tables.twt, "transformer")] {
        for uid in section.uids() {
            let value = section.get(uid)?;
            rows.push(ScucBranchSwitchingCost {
                id: component_id(component_type, uid)?,
                connection_cost: require_num(value, "connection_cost")?,
                disconnection_cost: require_num(value, "disconnection_cost")?,
            });
        }
    }
    Ok(rows)
}

fn build_transformer_controls(tables: &Goc3Adapter) -> Result<Vec<ScucTransformerControl>> {
    tables
        .twt
        .uids()
        .iter()
        .map(|uid| {
            let value = tables.twt.get(uid)?;
            Ok(ScucTransformerControl {
                id: component_id("transformer", uid)?,
                tap_ratio_min: require_num(value, "tm_lb")?,
                tap_ratio_max: require_num(value, "tm_ub")?,
                phase_shift_min: require_num(value, "ta_lb")?,
                phase_shift_max: require_num(value, "ta_ub")?,
            })
        })
        .collect()
}

fn zone_buses(tables: &Goc3Adapter, zone_uid: &str, field: &str) -> Result<Vec<ComponentId>> {
    let mut buses = Vec::new();
    for uid in tables.bus.uids() {
        let value = tables.bus.get(uid)?;
        let member = value
            .get(field)
            .and_then(Value::as_array)
            .is_some_and(|zones| zones.iter().any(|zone| zone.as_str() == Some(zone_uid)));
        if member {
            buses.push(component_id("bus", uid)?);
        }
    }
    Ok(buses)
}

fn build_active_reserve_zones(tables: &Goc3Adapter) -> Result<Vec<ScucActiveReserveZone>> {
    tables
        .azr
        .uids()
        .iter()
        .map(|uid| {
            let value = tables.azr.get(uid)?;
            let series = tables.azr_ts.get(uid)?;
            Ok(ScucActiveReserveZone {
                id: component_id("active_reserve_zone", uid)?,
                buses: zone_buses(tables, uid, "active_reserve_uids")?,
                regulation_up_requirement_fraction: require_num(value, "REG_UP")?,
                regulation_down_requirement_fraction: require_num(value, "REG_DOWN")?,
                synchronized_requirement_fraction: require_num(value, "SYN")?,
                nonsynchronized_requirement_fraction: require_num(value, "NSYN")?,
                ramping_up_requirement: float_vec(require_field(
                    series,
                    "active_zonal_reserve time series",
                    uid,
                    "RAMPING_RESERVE_UP",
                )?)?,
                ramping_down_requirement: float_vec(require_field(
                    series,
                    "active_zonal_reserve time series",
                    uid,
                    "RAMPING_RESERVE_DOWN",
                )?)?,
                regulation_up_violation_cost: require_num(value, "REG_UP_vio_cost")?,
                regulation_down_violation_cost: require_num(value, "REG_DOWN_vio_cost")?,
                synchronized_violation_cost: require_num(value, "SYN_vio_cost")?,
                nonsynchronized_violation_cost: require_num(value, "NSYN_vio_cost")?,
                ramping_up_violation_cost: require_num(value, "RAMPING_RESERVE_UP_vio_cost")?,
                ramping_down_violation_cost: require_num(value, "RAMPING_RESERVE_DOWN_vio_cost")?,
            })
        })
        .collect()
}

fn build_reactive_reserve_zones(tables: &Goc3Adapter) -> Result<Vec<ScucReactiveReserveZone>> {
    tables
        .rzr
        .uids()
        .iter()
        .map(|uid| {
            let value = tables.rzr.get(uid)?;
            let series = tables.rzr_ts.get(uid)?;
            Ok(ScucReactiveReserveZone {
                id: component_id("reactive_reserve_zone", uid)?,
                buses: zone_buses(tables, uid, "reactive_reserve_uids")?,
                reactive_up_requirement: float_vec(require_field(
                    series,
                    "reactive_zonal_reserve time series",
                    uid,
                    "REACT_UP",
                )?)?,
                reactive_down_requirement: float_vec(require_field(
                    series,
                    "reactive_zonal_reserve time series",
                    uid,
                    "REACT_DOWN",
                )?)?,
                reactive_up_violation_cost: require_num(value, "REACT_UP_vio_cost")?,
                reactive_down_violation_cost: require_num(value, "REACT_DOWN_vio_cost")?,
            })
        })
        .collect()
}

fn build_contingencies(tables: &Goc3Adapter) -> Result<Vec<ScucContingency>> {
    let component_type = |uid: &str| {
        if tables
            .ac_line
            .uids()
            .iter()
            .any(|candidate| candidate == uid)
        {
            Some("branch")
        } else if tables.twt.uids().iter().any(|candidate| candidate == uid) {
            Some("transformer")
        } else if tables
            .dc_line
            .uids()
            .iter()
            .any(|candidate| candidate == uid)
        {
            Some("hvdc")
        } else {
            None
        }
    };
    tables
        .contingencies()
        .iter()
        .map(|value| {
            let object = value
                .as_object()
                .ok_or_else(|| json_error("reliability.contingency item is not an object"))?;
            let uid = require_str(object, "uid")?;
            let components = require_field(object, "contingency", uid, "components")?
                .as_array()
                .ok_or_else(|| {
                    json_error(format!("contingency `{uid}` `components` is not an array"))
                })?
                .iter()
                .map(|component| {
                    let local_id = component.as_str().ok_or_else(|| {
                        json_error(format!(
                            "contingency `{uid}` contains a component identity that is not a string"
                        ))
                    })?;
                    let kind = component_type(local_id).ok_or_else(|| {
                        json_error(format!(
                            "contingency `{uid}` names unknown branch `{local_id}`"
                        ))
                    })?;
                    component_id(kind, local_id)
                })
                .collect::<Result<_>>()?;
            Ok(ScucContingency {
                id: component_id("contingency", uid)?,
                components,
            })
        })
        .collect()
}

fn build_violation_costs(tables: &Goc3Adapter) -> ScucViolationCosts {
    let cost = |key: &str| {
        tables.violation_cost[key]
            .as_f64()
            .expect("input validation established a finite number")
    };
    ScucViolationCosts {
        active_power_balance: cost("p_bus_vio_cost"),
        reactive_power_balance: cost("q_bus_vio_cost"),
        branch_thermal_limit: cost("s_vio_cost"),
        energy_requirement: cost("e_vio_cost"),
    }
}

fn build_scuc_inputs(tables: &Goc3Adapter) -> Result<ScucInputs> {
    Ok(ScucInputs {
        interval_durations: tables.dt.clone(),
        devices: build_devices(tables)?,
        shunts: build_shunts(tables)?,
        branch_switching_costs: build_branch_switching_costs(tables)?,
        transformer_controls: build_transformer_controls(tables)?,
        active_reserve_zones: build_active_reserve_zones(tables)?,
        reactive_reserve_zones: build_reactive_reserve_zones(tables)?,
        contingencies: build_contingencies(tables)?,
        violation_costs: build_violation_costs(tables),
    })
}

pub(crate) fn parse_goc3_document(document: &Goc3Document) -> Result<ScucInputs> {
    let tables = Goc3Adapter::from_document(document)?;
    build_scuc_inputs(&tables)
}

#[cfg(test)]
pub(super) fn decode_goc3_str(text: &str, from: &str) -> Result<ScucInputs> {
    if from != "goc3-json" {
        return Err(Goc3Error::UnsupportedFormat(from.to_owned()));
    }
    let document = Goc3Document::parse(text)?;
    parse_goc3_document(&document)
}
