//! The AC security constrained unit commitment solution, preserving the DOE
//! GO Challenge 3 output fields.

use std::sync::Arc;

use powerio_core::Error;

use crate::diagnostics::codes;
use crate::instance::AcScucInstance;
use crate::solution::{Producer, Residuals, Termination};

/// Per time point network state outputs: `values[t][row]` over the stated
/// element table order.
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct ScucNetworkOutputs {
    /// Bus voltage magnitude, per unit.
    pub bus_vm: Vec<Vec<f64>>,
    /// Bus voltage angle, radians.
    pub bus_va: Vec<Vec<f64>>,
    /// Shunt step counts.
    pub shunt_step: Vec<Vec<f64>>,
    /// AC line on status.
    pub ac_line_on_status: Vec<Vec<f64>>,
    /// Two winding transformer winding ratio.
    pub transformer_tm: Vec<Vec<f64>>,
    /// Two winding transformer phase shift, radians.
    pub transformer_ta: Vec<Vec<f64>>,
    /// Two winding transformer on status.
    pub transformer_on_status: Vec<Vec<f64>>,
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
    pub on_status: Vec<Vec<f64>>,
    /// Dispatched active power while on, per unit power.
    pub p_on: Vec<Vec<f64>>,
    /// Dispatched reactive power, per unit power.
    pub q: Vec<Vec<f64>>,
    /// Regulation reserve.
    pub p_reg_res_up: Vec<Vec<f64>>,
    /// Regulation down reserve.
    pub p_reg_res_down: Vec<Vec<f64>>,
    /// Synchronous reserve.
    pub p_syn_res: Vec<Vec<f64>>,
    /// Nonsynchronous reserve.
    pub p_nsyn_res: Vec<Vec<f64>>,
    /// Ramping reserve up.
    pub p_ramp_res_up_online: Vec<Vec<f64>>,
    /// Ramping reserve down.
    pub p_ramp_res_down_online: Vec<Vec<f64>>,
    /// Reactive reserve up.
    pub q_res_up: Vec<Vec<f64>>,
    /// Reactive reserve down.
    pub q_res_down: Vec<Vec<f64>>,
}

/// Every stored series of [`ScucNetworkOutputs`], one name per field. The
/// stored wire and this list stay in agreement through the exhaustive
/// destructure test below: adding a field breaks the build until the name
/// lands here and on the wire.
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
pub const SCUC_DEVICE_OUTPUT_SERIES: [&str; 11] = [
    "on_status",
    "p_on",
    "q",
    "p_reg_res_up",
    "p_reg_res_down",
    "p_syn_res",
    "p_nsyn_res",
    "p_ramp_res_up_online",
    "p_ramp_res_down_online",
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
    /// per time point of the instance's time axis; an empty series states
    /// that the producer omitted the category.
    ///
    /// # Errors
    /// An output series whose time axis disagrees with the instance.
    pub fn new(
        instance: Arc<AcScucInstance>,
        termination: Termination,
        network_outputs: ScucNetworkOutputs,
        device_outputs: ScucDeviceOutputs,
        objective: Option<f64>,
    ) -> Result<Self, Error> {
        let periods = instance.inputs().dt.len();
        let check = |what: &'static str, series: &Vec<Vec<f64>>| -> Result<(), Error> {
            if series.is_empty() || series.len() == periods {
                Ok(())
            } else {
                Err(Error::new(
                    &codes::BUILD_SOLUTION_SHAPE_MISMATCH,
                    format!(
                        "{what} carries {} time rows; the instance states {periods} intervals",
                        series.len()
                    ),
                ))
            }
        };
        check("bus vm", &network_outputs.bus_vm)?;
        check("bus va", &network_outputs.bus_va)?;
        check("shunt step", &network_outputs.shunt_step)?;
        check("ac line on status", &network_outputs.ac_line_on_status)?;
        check("transformer tm", &network_outputs.transformer_tm)?;
        check("transformer ta", &network_outputs.transformer_ta)?;
        check(
            "transformer on status",
            &network_outputs.transformer_on_status,
        )?;
        check("dc line pdc_fr", &network_outputs.dc_line_pdc_fr)?;
        check("dc line qdc_fr", &network_outputs.dc_line_qdc_fr)?;
        check("dc line qdc_to", &network_outputs.dc_line_qdc_to)?;
        check("device on status", &device_outputs.on_status)?;
        check("device p_on", &device_outputs.p_on)?;
        check("device q", &device_outputs.q)?;
        check("device p_reg_res_up", &device_outputs.p_reg_res_up)?;
        check("device p_reg_res_down", &device_outputs.p_reg_res_down)?;
        check("device p_syn_res", &device_outputs.p_syn_res)?;
        check("device p_nsyn_res", &device_outputs.p_nsyn_res)?;
        check(
            "device p_ramp_res_up_online",
            &device_outputs.p_ramp_res_up_online,
        )?;
        check(
            "device p_ramp_res_down_online",
            &device_outputs.p_ramp_res_down_online,
        )?;
        check("device q_res_up", &device_outputs.q_res_up)?;
        check("device q_res_down", &device_outputs.q_res_down)?;
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

    /// The per time point network state outputs.
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
            p_on: _,
            q: _,
            p_reg_res_up: _,
            p_reg_res_down: _,
            p_syn_res: _,
            p_nsyn_res: _,
            p_ramp_res_up_online: _,
            p_ramp_res_down_online: _,
            q_res_up: _,
            q_res_down: _,
        } = ScucDeviceOutputs::default();
        assert_eq!(SCUC_DEVICE_OUTPUT_SERIES.len(), 11);
    }
}
