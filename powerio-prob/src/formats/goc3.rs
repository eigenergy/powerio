//! Map DOE GO Challenge 3 JSON to its typed calculation values.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use powerio_core::{Diagnostic, Error, SourceBuffer};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

use super::source_text;
use crate::diagnostics::codes;
use crate::goc3::{Goc3Error, parse_goc3_document};
use crate::instance::AcScucInstance;
use crate::solution::{AcScucSolution, ScucDeviceOutputs, ScucNetworkOutputs, Termination};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SolutionDocument {
    time_series_output: TimeSeriesOutput,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TimeSeriesOutput {
    bus: Vec<BusOutput>,
    shunt: Vec<ShuntOutput>,
    simple_dispatchable_device: Vec<DeviceOutput>,
    ac_line: Vec<AcLineOutput>,
    two_winding_transformer: Vec<TransformerOutput>,
    dc_line: Vec<DcLineOutput>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BusOutput {
    uid: String,
    vm: Vec<f64>,
    va: Vec<f64>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ShuntOutput {
    uid: String,
    step: Vec<i64>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AcLineOutput {
    uid: String,
    on_status: Vec<BinaryStatus>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TransformerOutput {
    uid: String,
    tm: Vec<f64>,
    ta: Vec<f64>,
    on_status: Vec<BinaryStatus>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DcLineOutput {
    uid: String,
    pdc_fr: Vec<f64>,
    qdc_fr: Vec<f64>,
    qdc_to: Vec<f64>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeviceOutput {
    uid: String,
    on_status: Vec<BinaryStatus>,
    p_on: Vec<f64>,
    q: Vec<f64>,
    p_reg_res_up: Vec<f64>,
    p_reg_res_down: Vec<f64>,
    p_syn_res: Vec<f64>,
    p_nsyn_res: Vec<f64>,
    p_ramp_res_up_online: Vec<f64>,
    p_ramp_res_down_online: Vec<f64>,
    p_ramp_res_up_offline: Vec<f64>,
    p_ramp_res_down_offline: Vec<f64>,
    q_res_up: Vec<f64>,
    q_res_down: Vec<f64>,
}

#[derive(Clone, Copy)]
struct BinaryStatus(bool);

type StatusGrid = Vec<Vec<bool>>;

impl Serialize for BinaryStatus {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_u8(u8::from(self.0))
    }
}

impl<'de> Deserialize<'de> for BinaryStatus {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        match i64::deserialize(deserializer)? {
            0 => Ok(Self(false)),
            1 => Ok(Self(true)),
            value => Err(de::Error::custom(format!(
                "status must be the integer 0 or 1, found {value}"
            ))),
        }
    }
}

/// Decode the official GO Challenge 3 output file against the problem that
/// supplies its component identities and time axis.
///
/// This buffer level entry exists for the PowerIO source dispatcher. It does
/// not add another public representation operation.
#[doc(hidden)]
#[allow(clippy::too_many_lines)]
pub fn __parse_goc3_output_buffer(
    instance: Arc<AcScucInstance>,
    buffer: &SourceBuffer,
) -> Result<AcScucSolution, Error> {
    let document: SolutionDocument =
        serde_json::from_slice(buffer.content_bytes()).map_err(|error| {
            Error::new(
                &codes::PARSE_GOC3_MALFORMED,
                format!("{}: {error}", buffer.name()),
            )
        })?;
    let output = document.time_series_output;
    let inputs = instance.inputs();
    let periods = inputs.interval_durations.len();
    let network = instance.network();

    let bus_ids = network
        .buses()
        .iter()
        .map(|row| row.uid.as_deref())
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| invalid_solution("the instance contains a bus without a uid"))?;
    let shunt_ids: Vec<_> = inputs.shunts.iter().map(|row| row.id.local_id()).collect();
    let ac_line_ids: Vec<_> = inputs
        .branch_switching_costs
        .iter()
        .filter(|row| row.id.component_type() == "branch")
        .map(|row| row.id.local_id())
        .collect();
    let transformer_ids: Vec<_> = inputs
        .branch_switching_costs
        .iter()
        .filter(|row| row.id.component_type() == "transformer")
        .map(|row| row.id.local_id())
        .collect();
    let dc_line_ids = network
        .hvdc()
        .iter()
        .map(|row| row.uid.as_deref())
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| invalid_solution("the instance contains a dc_line without a uid"))?;
    let device_ids: Vec<_> = inputs.devices.iter().map(|row| row.id.local_id()).collect();
    if let Some(message) = repeated_output_uid(&[
        ("bus", &bus_ids),
        ("shunt", &shunt_ids),
        ("simple_dispatchable_device", &device_ids),
        ("ac_line", &ac_line_ids),
        ("two_winding_transformer", &transformer_ids),
        ("dc_line", &dc_line_ids),
    ]) {
        return Err(invalid_solution(message));
    }

    let network_outputs = ScucNetworkOutputs {
        bus_vm: aligned_finite_series(
            "bus",
            "vm",
            &output.bus,
            &bus_ids,
            periods,
            |row| &row.uid,
            |row| &row.vm,
        )?,
        bus_va: aligned_finite_series(
            "bus",
            "va",
            &output.bus,
            &bus_ids,
            periods,
            |row| &row.uid,
            |row| &row.va,
        )?,
        shunt_step: aligned_series(
            "shunt",
            "step",
            &output.shunt,
            &shunt_ids,
            periods,
            |row| &row.uid,
            |row| &row.step,
        )?,
        ac_line_on_status: aligned_status_series(
            "ac_line",
            "on_status",
            &output.ac_line,
            &ac_line_ids,
            periods,
            |row| &row.uid,
            |row| &row.on_status,
        )?,
        transformer_tm: aligned_finite_series(
            "two_winding_transformer",
            "tm",
            &output.two_winding_transformer,
            &transformer_ids,
            periods,
            |row| &row.uid,
            |row| &row.tm,
        )?,
        transformer_ta: aligned_finite_series(
            "two_winding_transformer",
            "ta",
            &output.two_winding_transformer,
            &transformer_ids,
            periods,
            |row| &row.uid,
            |row| &row.ta,
        )?,
        transformer_on_status: aligned_status_series(
            "two_winding_transformer",
            "on_status",
            &output.two_winding_transformer,
            &transformer_ids,
            periods,
            |row| &row.uid,
            |row| &row.on_status,
        )?,
        dc_line_pdc_fr: aligned_finite_series(
            "dc_line",
            "pdc_fr",
            &output.dc_line,
            &dc_line_ids,
            periods,
            |row| &row.uid,
            |row| &row.pdc_fr,
        )?,
        dc_line_qdc_fr: aligned_finite_series(
            "dc_line",
            "qdc_fr",
            &output.dc_line,
            &dc_line_ids,
            periods,
            |row| &row.uid,
            |row| &row.qdc_fr,
        )?,
        dc_line_qdc_to: aligned_finite_series(
            "dc_line",
            "qdc_to",
            &output.dc_line,
            &dc_line_ids,
            periods,
            |row| &row.uid,
            |row| &row.qdc_to,
        )?,
    };

    macro_rules! device_finite_series {
        ($field:ident) => {
            aligned_finite_series(
                "simple_dispatchable_device",
                stringify!($field),
                &output.simple_dispatchable_device,
                &device_ids,
                periods,
                |row| &row.uid,
                |row| &row.$field,
            )?
        };
    }
    let on_status = aligned_status_series(
        "simple_dispatchable_device",
        "on_status",
        &output.simple_dispatchable_device,
        &device_ids,
        periods,
        |row| &row.uid,
        |row| &row.on_status,
    )?;
    let (startup_status, shutdown_status) = derive_commitment_changes(inputs, &on_status, periods)?;
    let device_outputs = ScucDeviceOutputs {
        on_status,
        startup_status,
        shutdown_status,
        p_on: device_finite_series!(p_on),
        q: device_finite_series!(q),
        p_reg_res_up: device_finite_series!(p_reg_res_up),
        p_reg_res_down: device_finite_series!(p_reg_res_down),
        p_syn_res: device_finite_series!(p_syn_res),
        p_nsyn_res: device_finite_series!(p_nsyn_res),
        p_ramp_res_up_online: device_finite_series!(p_ramp_res_up_online),
        p_ramp_res_up_offline: device_finite_series!(p_ramp_res_up_offline),
        p_ramp_res_down_online: device_finite_series!(p_ramp_res_down_online),
        p_ramp_res_down_offline: device_finite_series!(p_ramp_res_down_offline),
        q_res_up: device_finite_series!(q_res_up),
        q_res_down: device_finite_series!(q_res_down),
    };

    AcScucSolution::new(
        instance,
        Termination::NotReported,
        network_outputs,
        device_outputs,
        None,
    )
}

fn aligned_series<'a, R, T: Copy + 'a>(
    section: &str,
    field: &str,
    rows: &'a [R],
    expected_ids: &[&str],
    periods: usize,
    uid: impl Fn(&'a R) -> &'a str,
    values: impl Fn(&'a R) -> &'a [T],
) -> Result<Vec<Vec<T>>, Error> {
    let mut by_id = HashMap::with_capacity(rows.len());
    for row in rows {
        let id = uid(row);
        if by_id.insert(id, row).is_some() {
            return Err(invalid_solution(format!(
                "{section} contains duplicate uid `{id}`"
            )));
        }
    }
    let expected: HashSet<_> = expected_ids.iter().copied().collect();
    if let Some(id) = by_id.keys().copied().find(|id| !expected.contains(id)) {
        return Err(invalid_solution(format!(
            "{section} names unknown uid `{id}`"
        )));
    }
    if let Some(id) = expected_ids
        .iter()
        .copied()
        .find(|id| !by_id.contains_key(id))
    {
        return Err(invalid_solution(format!(
            "{section} omits required uid `{id}`"
        )));
    }

    let mut result = (0..periods)
        .map(|_| Vec::with_capacity(expected_ids.len()))
        .collect::<Vec<_>>();
    for id in expected_ids {
        let row = by_id[id];
        let series = values(row);
        if series.len() != periods {
            return Err(invalid_solution(format!(
                "{section} uid `{id}` field `{field}` carries {} values; the instance states {periods} intervals",
                series.len()
            )));
        }
        for (time, value) in series.iter().copied().enumerate() {
            result[time].push(value);
        }
    }
    Ok(result)
}

fn aligned_finite_series<'a, R>(
    section: &str,
    field: &str,
    rows: &'a [R],
    expected_ids: &[&str],
    periods: usize,
    uid: impl Fn(&'a R) -> &'a str,
    values: impl Fn(&'a R) -> &'a [f64],
) -> Result<Vec<Vec<f64>>, Error> {
    let result = aligned_series(section, field, rows, expected_ids, periods, uid, values)?;
    if let Some((time, column, value)) = result.iter().enumerate().find_map(|(time, row)| {
        row.iter()
            .copied()
            .enumerate()
            .find(|(_, value)| !value.is_finite())
            .map(|(column, value)| (time, column, value))
    }) {
        return Err(invalid_solution(format!(
            "{section} field `{field}` value [{time}][{column}] is not finite: {value}"
        )));
    }
    Ok(result)
}

fn aligned_status_series<'a, R>(
    section: &str,
    field: &str,
    rows: &'a [R],
    expected_ids: &[&str],
    periods: usize,
    uid: impl Fn(&'a R) -> &'a str,
    values: impl Fn(&'a R) -> &'a [BinaryStatus],
) -> Result<Vec<Vec<bool>>, Error> {
    Ok(
        aligned_series(section, field, rows, expected_ids, periods, uid, values)?
            .into_iter()
            .map(|row| row.into_iter().map(|value| value.0).collect())
            .collect(),
    )
}

fn derive_commitment_changes(
    inputs: &crate::instance::ScucInputs,
    on_status: &[Vec<bool>],
    periods: usize,
) -> Result<(StatusGrid, StatusGrid), Error> {
    if on_status.len() != periods {
        return Err(invalid_solution(format!(
            "simple_dispatchable_device on_status carries {} time rows; the instance states {periods} intervals",
            on_status.len()
        )));
    }
    let devices = inputs.devices.len();
    let mut startup = vec![vec![false; devices]; periods];
    let mut shutdown = vec![vec![false; devices]; periods];
    for time in 0..periods {
        for device in 0..devices {
            let current = on_status[time][device];
            let previous = if time == 0 {
                inputs.devices[device].initial_on_status
            } else {
                on_status[time - 1][device]
            };
            startup[time][device] = current && !previous;
            shutdown[time][device] = !current && previous;
        }
    }
    Ok((startup, shutdown))
}

/// Encode a complete AC SCUC solution as the official GO Challenge 3 output
/// document. Startup and shutdown indicators are not fields in that document.
#[doc(hidden)]
#[allow(clippy::too_many_lines)]
pub fn __emit_goc3_output(solution: &AcScucSolution) -> Result<String, Error> {
    let instance = solution.instance();
    let inputs = instance.inputs();
    let network = instance.network();
    let periods = inputs.interval_durations.len();
    let buses = network.buses().len();
    let shunts = inputs.shunts.len();
    let ac_lines = inputs
        .branch_switching_costs
        .iter()
        .filter(|row| row.id.component_type() == "branch")
        .count();
    let transformers = inputs
        .branch_switching_costs
        .iter()
        .filter(|row| row.id.component_type() == "transformer")
        .count();
    let dc_lines = network.hvdc().len();
    let devices = inputs.devices.len();
    let network_output = solution.network_outputs();
    let device_output = solution.device_outputs();

    require_complete_finite_grid("bus.vm", &network_output.bus_vm, periods, buses)?;
    require_complete_finite_grid("bus.va", &network_output.bus_va, periods, buses)?;
    require_complete_grid("shunt.step", &network_output.shunt_step, periods, shunts)?;
    require_complete_grid(
        "ac_line.on_status",
        &network_output.ac_line_on_status,
        periods,
        ac_lines,
    )?;
    require_complete_finite_grid(
        "two_winding_transformer.tm",
        &network_output.transformer_tm,
        periods,
        transformers,
    )?;
    require_complete_finite_grid(
        "two_winding_transformer.ta",
        &network_output.transformer_ta,
        periods,
        transformers,
    )?;
    require_complete_grid(
        "two_winding_transformer.on_status",
        &network_output.transformer_on_status,
        periods,
        transformers,
    )?;
    require_complete_finite_grid(
        "dc_line.pdc_fr",
        &network_output.dc_line_pdc_fr,
        periods,
        dc_lines,
    )?;
    require_complete_finite_grid(
        "dc_line.qdc_fr",
        &network_output.dc_line_qdc_fr,
        periods,
        dc_lines,
    )?;
    require_complete_finite_grid(
        "dc_line.qdc_to",
        &network_output.dc_line_qdc_to,
        periods,
        dc_lines,
    )?;
    require_complete_grid(
        "simple_dispatchable_device.on_status",
        &device_output.on_status,
        periods,
        devices,
    )?;
    for (name, grid) in [
        ("p_on", &device_output.p_on),
        ("q", &device_output.q),
        ("p_reg_res_up", &device_output.p_reg_res_up),
        ("p_reg_res_down", &device_output.p_reg_res_down),
        ("p_syn_res", &device_output.p_syn_res),
        ("p_nsyn_res", &device_output.p_nsyn_res),
        ("p_ramp_res_up_online", &device_output.p_ramp_res_up_online),
        (
            "p_ramp_res_down_online",
            &device_output.p_ramp_res_down_online,
        ),
        (
            "p_ramp_res_up_offline",
            &device_output.p_ramp_res_up_offline,
        ),
        (
            "p_ramp_res_down_offline",
            &device_output.p_ramp_res_down_offline,
        ),
        ("q_res_up", &device_output.q_res_up),
        ("q_res_down", &device_output.q_res_down),
    ] {
        require_complete_finite_grid(
            &format!("simple_dispatchable_device.{name}"),
            grid,
            periods,
            devices,
        )?;
    }

    let bus_ids = network
        .buses()
        .iter()
        .map(|row| row.uid.as_deref())
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| invalid_output("the instance contains a bus without a uid"))?;
    let shunt_ids: Vec<_> = inputs.shunts.iter().map(|row| row.id.local_id()).collect();
    let ac_line_ids: Vec<_> = inputs
        .branch_switching_costs
        .iter()
        .filter(|row| row.id.component_type() == "branch")
        .map(|row| row.id.local_id())
        .collect();
    let transformer_ids: Vec<_> = inputs
        .branch_switching_costs
        .iter()
        .filter(|row| row.id.component_type() == "transformer")
        .map(|row| row.id.local_id())
        .collect();
    let dc_line_ids = network
        .hvdc()
        .iter()
        .map(|row| row.uid.as_deref())
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| invalid_output("the instance contains a dc_line without a uid"))?;
    let device_ids: Vec<_> = inputs.devices.iter().map(|row| row.id.local_id()).collect();
    if let Some(message) = repeated_output_uid(&[
        ("bus", &bus_ids),
        ("shunt", &shunt_ids),
        ("simple_dispatchable_device", &device_ids),
        ("ac_line", &ac_line_ids),
        ("two_winding_transformer", &transformer_ids),
        ("dc_line", &dc_line_ids),
    ]) {
        return Err(invalid_output(message));
    }

    let document = SolutionDocument {
        time_series_output: TimeSeriesOutput {
            bus: bus_ids
                .iter()
                .enumerate()
                .map(|(column, uid)| BusOutput {
                    uid: (*uid).to_owned(),
                    vm: column_values(&network_output.bus_vm, column),
                    va: column_values(&network_output.bus_va, column),
                })
                .collect(),
            shunt: shunt_ids
                .iter()
                .enumerate()
                .map(|(column, uid)| ShuntOutput {
                    uid: (*uid).to_owned(),
                    step: column_values(&network_output.shunt_step, column),
                })
                .collect(),
            simple_dispatchable_device: device_ids
                .iter()
                .enumerate()
                .map(|(column, uid)| DeviceOutput {
                    uid: (*uid).to_owned(),
                    on_status: status_column(&device_output.on_status, column),
                    p_on: column_values(&device_output.p_on, column),
                    q: column_values(&device_output.q, column),
                    p_reg_res_up: column_values(&device_output.p_reg_res_up, column),
                    p_reg_res_down: column_values(&device_output.p_reg_res_down, column),
                    p_syn_res: column_values(&device_output.p_syn_res, column),
                    p_nsyn_res: column_values(&device_output.p_nsyn_res, column),
                    p_ramp_res_up_online: column_values(
                        &device_output.p_ramp_res_up_online,
                        column,
                    ),
                    p_ramp_res_down_online: column_values(
                        &device_output.p_ramp_res_down_online,
                        column,
                    ),
                    p_ramp_res_up_offline: column_values(
                        &device_output.p_ramp_res_up_offline,
                        column,
                    ),
                    p_ramp_res_down_offline: column_values(
                        &device_output.p_ramp_res_down_offline,
                        column,
                    ),
                    q_res_up: column_values(&device_output.q_res_up, column),
                    q_res_down: column_values(&device_output.q_res_down, column),
                })
                .collect(),
            ac_line: ac_line_ids
                .iter()
                .enumerate()
                .map(|(column, uid)| AcLineOutput {
                    uid: (*uid).to_owned(),
                    on_status: status_column(&network_output.ac_line_on_status, column),
                })
                .collect(),
            two_winding_transformer: transformer_ids
                .iter()
                .enumerate()
                .map(|(column, uid)| TransformerOutput {
                    uid: (*uid).to_owned(),
                    tm: column_values(&network_output.transformer_tm, column),
                    ta: column_values(&network_output.transformer_ta, column),
                    on_status: status_column(&network_output.transformer_on_status, column),
                })
                .collect(),
            dc_line: dc_line_ids
                .iter()
                .enumerate()
                .map(|(column, uid)| DcLineOutput {
                    uid: (*uid).to_owned(),
                    pdc_fr: column_values(&network_output.dc_line_pdc_fr, column),
                    qdc_fr: column_values(&network_output.dc_line_qdc_fr, column),
                    qdc_to: column_values(&network_output.dc_line_qdc_to, column),
                })
                .collect(),
        },
    };
    let mut text = serde_json::to_string_pretty(&document)
        .map_err(|error| invalid_output(format!("cannot encode GOC3 output JSON: {error}")))?;
    text.push('\n');
    Ok(text)
}

fn column_values<T: Copy>(grid: &[Vec<T>], column: usize) -> Vec<T> {
    grid.iter().map(|row| row[column]).collect()
}

fn status_column(grid: &[Vec<bool>], column: usize) -> Vec<BinaryStatus> {
    grid.iter().map(|row| BinaryStatus(row[column])).collect()
}

fn require_complete_grid<T>(
    what: &str,
    grid: &[Vec<T>],
    periods: usize,
    width: usize,
) -> Result<(), Error> {
    if grid.len() != periods {
        return Err(invalid_output(format!(
            "{what} carries {} time rows; GOC3 output requires {periods}",
            grid.len()
        )));
    }
    if let Some((time, row)) = grid.iter().enumerate().find(|(_, row)| row.len() != width) {
        return Err(invalid_output(format!(
            "{what} time row {time} carries {} values; GOC3 output requires {width}",
            row.len()
        )));
    }
    Ok(())
}

fn require_complete_finite_grid(
    what: &str,
    grid: &[Vec<f64>],
    periods: usize,
    width: usize,
) -> Result<(), Error> {
    require_complete_grid(what, grid, periods, width)?;
    if let Some((time, column, value)) = grid.iter().enumerate().find_map(|(time, row)| {
        row.iter()
            .copied()
            .enumerate()
            .find(|(_, value)| !value.is_finite())
            .map(|(column, value)| (time, column, value))
    }) {
        return Err(invalid_output(format!(
            "{what}[{time}][{column}] is not finite: {value}"
        )));
    }
    Ok(())
}

fn invalid_output(message: impl Into<String>) -> Error {
    Error::new(&codes::BUILD_SOLUTION_SHAPE_MISMATCH, message)
}

fn repeated_output_uid(sections: &[(&str, &[&str])]) -> Option<String> {
    let mut owner = HashMap::new();
    for (section, ids) in sections {
        for id in *ids {
            if let Some(previous) = owner.insert(*id, *section) {
                return Some(format!(
                    "GOC3 output uid `{id}` is repeated in {previous} and {section}"
                ));
            }
        }
    }
    None
}

fn invalid_solution(message: impl Into<String>) -> Error {
    Error::new(&codes::READ_GOC3_INVALID_DOCUMENT, message)
}

/// Decode one official GO Challenge 3 problem input buffer.
///
/// This buffer level entry exists for the PowerIO source dispatcher. It does
/// not add another public representation operation.
#[doc(hidden)]
pub fn __parse_goc3_problem_buffer(
    buffer: &SourceBuffer,
) -> Result<(AcScucInstance, Vec<Diagnostic>), Error> {
    let content = source_text(buffer)?;
    let (network, mut diagnostics, document) =
        powerio_tx::format::goc3::parse_goc3_instance_network(content)
            .map_err(|error| Error::new(error.code(), format!("{}: {error}", buffer.name())))?;
    let inputs = parse_goc3_document(&document).map_err(|error| goc3_error(&error))?;
    diagnostics.extend(goc3_optional_field_diagnostics(&document));

    let instance = AcScucInstance::new(network, inputs)?;
    Ok((instance, diagnostics))
}

fn goc3_optional_field_diagnostics(
    document: &powerio_tx::format::goc3::Goc3Document,
) -> Vec<powerio_core::Diagnostic> {
    let mut diagnostics = Vec::new();
    push_goc3_general_optional_diagnostics(document, &mut diagnostics);
    push_goc3_bus_optional_diagnostics(document, &mut diagnostics);
    push_goc3_development_diagnostics(document, &mut diagnostics);
    push_goc3_device_optional_diagnostics(document, &mut diagnostics);
    diagnostics
}

fn push_goc3_general_optional_diagnostics(
    document: &powerio_tx::format::goc3::Goc3Document,
    diagnostics: &mut Vec<powerio_core::Diagnostic>,
) {
    const GENERAL_OPTIONAL: &[&str] = &[
        "timestamp_start",
        "timestamp_stop",
        "season",
        "electricity_demand",
        "vre_availability",
        "solar_availability",
        "wind_availability",
        "weather_temperature",
        "day_type",
        "net_load",
    ];
    if let Some(general) = document
        .root()
        .get("network")
        .and_then(serde_json::Value::as_object)
        .and_then(|network| network.get("general"))
        .and_then(serde_json::Value::as_object)
    {
        let present: Vec<_> = GENERAL_OPTIONAL
            .iter()
            .filter(|field| general.contains_key(**field))
            .copied()
            .collect();
        if !present.is_empty() {
            diagnostics.push(powerio_core::Diagnostic::of(
                &powerio_tx::diagnostics::codes::READ_GOC3_RETAINED_SOURCE_ONLY,
                format!(
                    "optional network.general fields retained in source only: {}",
                    present.join(", ")
                ),
            ));
        }
    }
}

fn push_goc3_bus_optional_diagnostics(
    document: &powerio_tx::format::goc3::Goc3Document,
    diagnostics: &mut Vec<powerio_core::Diagnostic>,
) {
    if let Some(buses) = document
        .root()
        .get("network")
        .and_then(serde_json::Value::as_object)
        .and_then(|network| network.get("bus"))
        .and_then(serde_json::Value::as_array)
    {
        for field in ["area", "zone", "city", "county", "state", "country"] {
            push_source_field_diagnostic(
                diagnostics,
                &powerio_tx::diagnostics::codes::READ_GOC3_OPTIONAL_FIELD_UNTYPED,
                buses,
                |bus| bus.contains_key(field),
                format!("`network.bus.{field}` has no typed PowerIO field"),
                "retained in `Bus.extras`",
                "bus",
            );
        }
        push_source_field_diagnostic(
            diagnostics,
            &powerio_tx::diagnostics::codes::READ_GOC3_OPTIONAL_FIELD_UNTYPED,
            buses,
            |bus| bus.contains_key("longitude") && !bus.contains_key("latitude"),
            "`network.bus.longitude` without `latitude` cannot form a typed bus location",
            "retained in `Bus.extras`",
            "bus",
        );
        push_source_field_diagnostic(
            diagnostics,
            &powerio_tx::diagnostics::codes::READ_GOC3_OPTIONAL_FIELD_UNTYPED,
            buses,
            |bus| bus.contains_key("latitude") && !bus.contains_key("longitude"),
            "`network.bus.latitude` without `longitude` cannot form a typed bus location",
            "retained in `Bus.extras`",
            "bus",
        );
        let affected: Vec<_> = buses
            .iter()
            .filter_map(serde_json::Value::as_object)
            .filter(|bus| bus.contains_key("con_loss_factor"))
            .filter_map(|bus| bus.get("uid").and_then(serde_json::Value::as_str))
            .collect();
        if !affected.is_empty() {
            let samples = affected
                .iter()
                .take(3)
                .copied()
                .collect::<Vec<_>>()
                .join(", ");
            diagnostics.push(powerio_core::Diagnostic::of(
                &powerio_tx::diagnostics::codes::READ_GOC3_RETAINED_SOURCE_ONLY,
                format!(
                    "legacy network.bus.con_loss_factor retained in source only for {} buses (sample uids: {samples}); GO Challenge 3 data format 1.1.1 removed this field",
                    affected.len()
                ),
            ));
        }
    }
}

fn push_goc3_development_diagnostics(
    document: &powerio_tx::format::goc3::Goc3Document,
    diagnostics: &mut Vec<powerio_core::Diagnostic>,
) {
    for parent in ["network", "time_series_input"] {
        if document
            .root()
            .get(parent)
            .and_then(serde_json::Value::as_object)
            .is_some_and(|object| object.contains_key("development"))
        {
            diagnostics.push(powerio_core::Diagnostic::of(
                &powerio_tx::diagnostics::codes::READ_GOC3_RETAINED_SOURCE_ONLY,
                format!("optional {parent}.development retained in source only"),
            ));
        }
    }
}

fn push_goc3_device_optional_diagnostics(
    document: &powerio_tx::format::goc3::Goc3Document,
    diagnostics: &mut Vec<powerio_core::Diagnostic>,
) {
    if let Some(devices) = document
        .root()
        .get("network")
        .and_then(serde_json::Value::as_object)
        .and_then(|network| network.get("simple_dispatchable_device"))
        .and_then(serde_json::Value::as_array)
    {
        push_source_field_diagnostic(
            diagnostics,
            &powerio_tx::diagnostics::codes::READ_GOC3_RETAINED_SOURCE_ONLY,
            devices,
            |device| {
                device
                    .get("device_type")
                    .and_then(serde_json::Value::as_str)
                    == Some("producer")
                    && device.contains_key("description")
            },
            "`network.simple_dispatchable_device.description` on a producer has no typed PowerIO field",
            "retained in the original source only",
            "device",
        );
        push_source_field_diagnostic(
            diagnostics,
            &powerio_tx::diagnostics::codes::READ_GOC3_OPTIONAL_FIELD_UNTYPED,
            devices,
            |device| {
                device
                    .get("device_type")
                    .and_then(serde_json::Value::as_str)
                    == Some("consumer")
                    && device.contains_key("description")
            },
            "`network.simple_dispatchable_device.description` on a consumer has no typed PowerIO field",
            "retained in `Load.extras`",
            "device",
        );
        for field in ["vm_setpoint", "nameplate_capacity"] {
            push_source_field_diagnostic(
                diagnostics,
                &powerio_tx::diagnostics::codes::READ_GOC3_OPTIONAL_FIELD_UNTYPED,
                devices,
                |device| {
                    device
                        .get("device_type")
                        .and_then(serde_json::Value::as_str)
                        == Some("consumer")
                        && device.contains_key(field)
                },
                format!(
                    "`network.simple_dispatchable_device.{field}` on a consumer has no typed PowerIO field"
                ),
                "retained in `Load.extras`",
                "device",
            );
        }
    }
}

fn push_source_field_diagnostic(
    diagnostics: &mut Vec<powerio_core::Diagnostic>,
    info: &'static powerio_core::DiagnosticInfo,
    records: &[serde_json::Value],
    predicate: impl Fn(&serde_json::Map<String, serde_json::Value>) -> bool,
    finding: impl std::fmt::Display,
    retention: &str,
    record_name: &str,
) {
    let affected: Vec<_> = records
        .iter()
        .filter_map(serde_json::Value::as_object)
        .filter(|record| predicate(record))
        .filter_map(|record| record.get("uid").and_then(serde_json::Value::as_str))
        .collect();
    if affected.is_empty() {
        return;
    }
    let samples = affected
        .iter()
        .take(3)
        .copied()
        .collect::<Vec<_>>()
        .join(", ");
    let plural = if affected.len() == 1 { "" } else { "s" };
    diagnostics.push(powerio_core::Diagnostic::of(
        info,
        format!(
            "{finding}; {retention} for {} {record_name}{plural} (sample uids: {samples})",
            affected.len()
        ),
    ));
}

fn goc3_error(error: &Goc3Error) -> Error {
    Error::new(error.code(), error.to_string())
}
