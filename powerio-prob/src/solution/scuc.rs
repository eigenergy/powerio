//! The AC security constrained unit commitment solution, preserving the DOE
//! GO Challenge 3 output fields.

use std::sync::Arc;

use powerio_core::Error;

use crate::diagnostics::codes;
use crate::instance::AcScucInstance;
use crate::solution::{Producer, Residuals, Termination};

/// Per time point network outputs: `values[t][row]` over the stated
/// element table order.
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct ScucNetworkOutputs {
    /// Bus voltage magnitude, per unit.
    pub bus_vm: Vec<Vec<f64>>,
    /// Bus voltage angle, radians.
    pub bus_va: Vec<Vec<f64>>,
    /// Shunt step counts.
    pub shunt_step: Vec<Vec<i64>>,
    /// AC line on status.
    pub ac_line_on_status: Vec<Vec<bool>>,
    /// Two winding transformer winding ratio.
    pub transformer_tm: Vec<Vec<f64>>,
    /// Two winding transformer phase shift, radians.
    pub transformer_ta: Vec<Vec<f64>>,
    /// Two winding transformer on status.
    pub transformer_on_status: Vec<Vec<bool>>,
    /// DC line from-side active flow, per unit power.
    pub dc_line_pdc_fr: Vec<Vec<f64>>,
    /// DC line from-side reactive flow, per unit power.
    pub dc_line_qdc_fr: Vec<Vec<f64>>,
    /// DC line to-side reactive flow, per unit power.
    pub dc_line_qdc_to: Vec<Vec<f64>>,
}

/// Per time point simple dispatchable device outputs: `values[t][device]`
/// over the stated device order.
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct ScucDeviceOutputs {
    /// Commitment.
    pub on_status: Vec<Vec<bool>>,
    /// Startup status.
    pub startup_status: Vec<Vec<bool>>,
    /// Shutdown status.
    pub shutdown_status: Vec<Vec<bool>>,
    /// Dispatched active power while on, per unit power.
    pub p_on: Vec<Vec<f64>>,
    /// Dispatched reactive power, per unit power.
    pub q: Vec<Vec<f64>>,
    /// Regulation up reserve, per unit power.
    pub p_reg_res_up: Vec<Vec<f64>>,
    /// Regulation down reserve, per unit power.
    pub p_reg_res_down: Vec<Vec<f64>>,
    /// Synchronized reserve, per unit power.
    pub p_syn_res: Vec<Vec<f64>>,
    /// Non-synchronized reserve, per unit power.
    pub p_nsyn_res: Vec<Vec<f64>>,
    /// Ramp up reserve when online, per unit power.
    pub p_ramp_res_up_online: Vec<Vec<f64>>,
    /// Ramp up reserve when offline, per unit power.
    pub p_ramp_res_up_offline: Vec<Vec<f64>>,
    /// Ramp down reserve when online, per unit power.
    pub p_ramp_res_down_online: Vec<Vec<f64>>,
    /// Ramp down reserve when offline, per unit power.
    pub p_ramp_res_down_offline: Vec<Vec<f64>>,
    /// Reactive reserve up, per unit power.
    pub q_res_up: Vec<Vec<f64>>,
    /// Reactive reserve down, per unit power.
    pub q_res_down: Vec<Vec<f64>>,
}

/// Every stored series of [`ScucNetworkOutputs`], one name per field. The
/// serialized fields and this list stay in agreement through the exhaustive
/// destructure test below: adding a field breaks the build until the name
/// lands here and in the stored document.
pub const SCUC_NETWORK_OUTPUT_SERIES: [&str; 10] = [
    "bus_vm",
    "bus_va",
    "shunt_step",
    "ac_line_on_status",
    "transformer_tm",
    "transformer_ta",
    "transformer_on_status",
    "dc_line_pdc_fr",
    "dc_line_qdc_fr",
    "dc_line_qdc_to",
];

/// Every stored series of [`ScucDeviceOutputs`], as
/// [`SCUC_NETWORK_OUTPUT_SERIES`].
pub const SCUC_DEVICE_OUTPUT_SERIES: [&str; 15] = [
    "on_status",
    "startup_status",
    "shutdown_status",
    "p_on",
    "q",
    "p_reg_res_up",
    "p_reg_res_down",
    "p_syn_res",
    "p_nsyn_res",
    "p_ramp_res_up_online",
    "p_ramp_res_up_offline",
    "p_ramp_res_down_online",
    "p_ramp_res_down_offline",
    "q_res_up",
    "q_res_down",
];

/// The AC security constrained unit commitment solution over the shared
/// instance.
#[derive(Clone, Debug)]
pub struct AcScucSolution {
    instance: Arc<AcScucInstance>,
    termination: Termination,
    residuals: Residuals,
    producer: Producer,
    network_outputs: ScucNetworkOutputs,
    device_outputs: ScucDeviceOutputs,
    objective: Option<f64>,
}

impl AcScucSolution {
    /// Assemble the solution. Every stated output series must carry one row
    /// per time point of the instance's time axis and one value per component
    /// in the corresponding instance table. Empty series remain permitted for
    /// producers that do not supply a category; GO Challenge 3 output requires
    /// every category and its writer checks that stronger requirement.
    ///
    /// # Errors
    /// An output series whose time axis disagrees with the instance.
    #[allow(clippy::too_many_lines)]
    pub fn new(
        instance: Arc<AcScucInstance>,
        termination: Termination,
        network_outputs: ScucNetworkOutputs,
        device_outputs: ScucDeviceOutputs,
        objective: Option<f64>,
    ) -> Result<Self, Error> {
        let periods = instance.inputs().interval_durations.len();
        let inputs = instance.inputs();
        let network = instance.network();
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

        check_finite_grid("bus vm", &network_outputs.bus_vm, periods, buses)?;
        check_finite_grid("bus va", &network_outputs.bus_va, periods, buses)?;
        check_grid("shunt step", &network_outputs.shunt_step, periods, shunts)?;
        check_grid(
            "ac line on status",
            &network_outputs.ac_line_on_status,
            periods,
            ac_lines,
        )?;
        check_finite_grid(
            "transformer tm",
            &network_outputs.transformer_tm,
            periods,
            transformers,
        )?;
        check_finite_grid(
            "transformer ta",
            &network_outputs.transformer_ta,
            periods,
            transformers,
        )?;
        check_grid(
            "transformer on status",
            &network_outputs.transformer_on_status,
            periods,
            transformers,
        )?;
        check_finite_grid(
            "dc line pdc_fr",
            &network_outputs.dc_line_pdc_fr,
            periods,
            dc_lines,
        )?;
        check_finite_grid(
            "dc line qdc_fr",
            &network_outputs.dc_line_qdc_fr,
            periods,
            dc_lines,
        )?;
        check_finite_grid(
            "dc line qdc_to",
            &network_outputs.dc_line_qdc_to,
            periods,
            dc_lines,
        )?;
        check_grid(
            "device on status",
            &device_outputs.on_status,
            periods,
            devices,
        )?;
        check_grid(
            "device startup status",
            &device_outputs.startup_status,
            periods,
            devices,
        )?;
        check_grid(
            "device shutdown status",
            &device_outputs.shutdown_status,
            periods,
            devices,
        )?;
        check_finite_grid("device p_on", &device_outputs.p_on, periods, devices)?;
        check_finite_grid("device q", &device_outputs.q, periods, devices)?;
        check_finite_grid(
            "device p_reg_res_up",
            &device_outputs.p_reg_res_up,
            periods,
            devices,
        )?;
        check_finite_grid(
            "device p_reg_res_down",
            &device_outputs.p_reg_res_down,
            periods,
            devices,
        )?;
        check_finite_grid(
            "device p_syn_res",
            &device_outputs.p_syn_res,
            periods,
            devices,
        )?;
        check_finite_grid(
            "device p_nsyn_res",
            &device_outputs.p_nsyn_res,
            periods,
            devices,
        )?;
        check_finite_grid(
            "device p_ramp_res_up_online",
            &device_outputs.p_ramp_res_up_online,
            periods,
            devices,
        )?;
        check_finite_grid(
            "device p_ramp_res_up_offline",
            &device_outputs.p_ramp_res_up_offline,
            periods,
            devices,
        )?;
        check_finite_grid(
            "device p_ramp_res_down_online",
            &device_outputs.p_ramp_res_down_online,
            periods,
            devices,
        )?;
        check_finite_grid(
            "device p_ramp_res_down_offline",
            &device_outputs.p_ramp_res_down_offline,
            periods,
            devices,
        )?;
        check_finite_grid(
            "device q_res_up",
            &device_outputs.q_res_up,
            periods,
            devices,
        )?;
        check_finite_grid(
            "device q_res_down",
            &device_outputs.q_res_down,
            periods,
            devices,
        )?;
        Ok(Self {
            instance,
            termination,
            residuals: Residuals::default(),
            producer: None,
            network_outputs,
            device_outputs,
            objective,
        })
    }

    /// The immutable instance this solution solves. Borrowed; never a copy.
    #[must_use]
    pub fn instance(&self) -> &AcScucInstance {
        &self.instance
    }

    /// The shared instance owner, for another solution of the same problem.
    #[must_use]
    pub fn shared_instance(&self) -> Arc<AcScucInstance> {
        Arc::clone(&self.instance)
    }

    /// How the producing calculation ended.
    #[must_use]
    pub const fn termination(&self) -> &Termination {
        &self.termination
    }

    /// The reported numerical residuals.
    #[must_use]
    pub const fn residuals(&self) -> &Residuals {
        &self.residuals
    }

    /// The producer or solver identity, when recorded.
    #[must_use]
    pub fn producer(&self) -> Option<&str> {
        self.producer.as_deref()
    }

    /// Record the producer identity.
    #[must_use]
    pub fn with_producer(mut self, producer: impl Into<String>) -> Self {
        self.producer = Some(producer.into());
        self
    }

    /// Record the numerical residuals.
    #[must_use]
    pub fn with_residuals(mut self, residuals: Residuals) -> Self {
        self.residuals = residuals;
        self
    }

    /// The per time point network outputs.
    #[must_use]
    pub const fn network_outputs(&self) -> &ScucNetworkOutputs {
        &self.network_outputs
    }

    /// The per time point device outputs.
    #[must_use]
    pub const fn device_outputs(&self) -> &ScucDeviceOutputs {
        &self.device_outputs
    }

    /// The reported objective value, when the producer states one.
    #[must_use]
    pub const fn objective(&self) -> Option<f64> {
        self.objective
    }
}

fn check_grid<T>(what: &str, series: &[Vec<T>], periods: usize, width: usize) -> Result<(), Error> {
    if series.is_empty() {
        return Ok(());
    }
    if series.len() != periods {
        return Err(Error::new(
            &codes::BUILD_SOLUTION_SHAPE_MISMATCH,
            format!(
                "{what} carries {} time rows; the instance states {periods} intervals",
                series.len()
            ),
        ));
    }
    if let Some((time, row)) = series
        .iter()
        .enumerate()
        .find(|(_, row)| row.len() != width)
    {
        return Err(Error::new(
            &codes::BUILD_SOLUTION_SHAPE_MISMATCH,
            format!(
                "{what} time row {time} carries {} values; the instance states {width} components",
                row.len()
            ),
        ));
    }
    Ok(())
}

fn check_finite_grid(
    what: &str,
    series: &[Vec<f64>],
    periods: usize,
    width: usize,
) -> Result<(), Error> {
    check_grid(what, series, periods, width)?;
    if let Some((time, column, value)) = series.iter().enumerate().find_map(|(time, row)| {
        row.iter()
            .copied()
            .enumerate()
            .find(|(_, value)| !value.is_finite())
            .map(|(column, value)| (time, column, value))
    }) {
        return Err(Error::new(
            &codes::BUILD_SOLUTION_SHAPE_MISMATCH,
            format!("{what}[{time}][{column}] is not finite: {value}"),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod series_vocabulary_tests {
    use super::*;

    /// Exhaustive destructures with no rest binding: a field added to either
    /// struct fails this build until its name joins the series constant.
    #[test]
    fn every_output_field_is_named_in_the_series_constants() {
        let ScucNetworkOutputs {
            bus_vm: _,
            bus_va: _,
            shunt_step: _,
            ac_line_on_status: _,
            transformer_tm: _,
            transformer_ta: _,
            transformer_on_status: _,
            dc_line_pdc_fr: _,
            dc_line_qdc_fr: _,
            dc_line_qdc_to: _,
        } = ScucNetworkOutputs::default();
        assert_eq!(SCUC_NETWORK_OUTPUT_SERIES.len(), 10);

        let ScucDeviceOutputs {
            on_status: _,
            startup_status: _,
            shutdown_status: _,
            p_on: _,
            q: _,
            p_reg_res_up: _,
            p_reg_res_down: _,
            p_syn_res: _,
            p_nsyn_res: _,
            p_ramp_res_up_online: _,
            p_ramp_res_up_offline: _,
            p_ramp_res_down_online: _,
            p_ramp_res_down_offline: _,
            q_res_up: _,
            q_res_down: _,
        } = ScucDeviceOutputs::default();
        assert_eq!(SCUC_DEVICE_OUTPUT_SERIES.len(), 15);
    }
}
