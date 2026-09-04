//! The AC security constrained unit commitment instance, following the DOE
//! GO Challenge 3 mathematical definition and data format.
//!
//! `AcScucInstance` contains the balanced electrical network plus the
//! inputs used by the Challenge 3 formulation: interval durations, initial
//! commitment durations, time varying bounds, cost blocks, reserve zones and memberships,
//! energy requirements,
//! contingencies, and violation costs — carried by [`ScucInputs`]. The GO
//! Challenge 3 grid exchange format also defines optional data outside that
//! formulation; such fields do not change the meaning of this type. Asking only for the
//! network discards scheduling, reserve, and contingency data; the
//! transformations below report those omissions.

use std::collections::HashSet;

use powerio_core::{ComponentId, Error};
use powerio_tx::BalancedNetwork;

use crate::diagnostics::codes;
use crate::instance::balanced::{
    AcOpfInstance, AcPfInstance, DcOpfInstance, DcPfInstance, transform_discarded,
};
use crate::instance::scuc_inputs::ScucInputs;

/// The AC security constrained unit commitment instance: the shared balanced
/// network plus the Challenge 3 formulation inputs.
#[derive(Clone, Debug)]
pub struct AcScucInstance {
    network: BalancedNetwork,
    inputs: ScucInputs,
}

impl AcScucInstance {
    /// Assemble the instance from a network and typed AC SCUC inputs. The
    /// GO Challenge 3 reader that builds both halves from one
    /// document owns identity reconciliation; this constructor checks the
    /// identities and time dimensions that must agree for the pair to describe
    /// one calculation.
    ///
    /// # Errors
    /// A referenced component is absent, an identity is repeated, or a time
    /// varying record disagrees with the horizon.
    pub fn new(network: BalancedNetwork, inputs: ScucInputs) -> Result<Self, Error> {
        validate_inputs(&network, &inputs)?;
        Ok(Self { network, inputs })
    }

    /// The balanced network this instance schedules. Borrowed; never a copy.
    #[must_use]
    pub fn network(&self) -> &BalancedNetwork {
        &self.network
    }

    /// The Challenge 3 formulation inputs.
    #[must_use]
    pub const fn inputs(&self) -> &ScucInputs {
        &self.inputs
    }

    /// The single period AC OPF instance this problem's network implies. The
    /// scheduling categories are discarded and the discard is recorded.
    ///
    /// # Errors
    /// As [`AcOpfInstance::from_network`].
    pub fn to_ac_opf(&self) -> Result<(AcOpfInstance, Vec<powerio_core::Diagnostic>), Error> {
        let instance = AcOpfInstance::from_network(self.network.clone())?;
        Ok((instance, vec![scheduling_discarded()]))
    }

    /// The single period DC OPF instance this problem's network implies.
    ///
    /// # Errors
    /// As [`DcOpfInstance::from_network`].
    pub fn to_dc_opf(&self) -> Result<(DcOpfInstance, Vec<powerio_core::Diagnostic>), Error> {
        let instance = DcOpfInstance::from_network(self.network.clone())?;
        Ok((instance, vec![scheduling_discarded()]))
    }

    /// The AC power flow instance for this problem's network at its stated
    /// injections and setpoints.
    ///
    /// # Errors
    /// As [`AcPfInstance::from_network`].
    pub fn to_ac_pf(&self) -> Result<(AcPfInstance, Vec<powerio_core::Diagnostic>), Error> {
        let instance = AcPfInstance::from_network(self.network.clone())?;
        Ok((instance, vec![scheduling_discarded()]))
    }

    /// The DC power flow instance for this problem's network at its stated
    /// injections.
    ///
    /// # Errors
    /// As [`DcPfInstance::from_network`].
    pub fn to_dc_pf(&self) -> Result<(DcPfInstance, Vec<powerio_core::Diagnostic>), Error> {
        let instance = DcPfInstance::from_network(self.network.clone())?;
        Ok((instance, vec![scheduling_discarded()]))
    }
}

#[allow(clippy::too_many_lines)]
fn validate_inputs(network: &BalancedNetwork, inputs: &ScucInputs) -> Result<(), Error> {
    let periods = inputs.interval_durations.len();
    if periods == 0
        || inputs
            .interval_durations
            .iter()
            .any(|duration| !duration.is_finite() || *duration <= 0.0)
    {
        return Err(Error::new(
            &codes::BUILD_INSTANCE_SHAPE_MISMATCH,
            "the AC SCUC horizon must contain positive finite interval durations",
        ));
    }

    let mut ids = HashSet::new();
    for device in &inputs.devices {
        require_unique(&mut ids, &device.id)?;
        if device.periods.len() != periods {
            return Err(shape_error(format!(
                "device {} has {} intervals; expected {periods}",
                device.id,
                device.periods.len()
            )));
        }
        let found = match device.kind {
            super::scuc_inputs::ScucDeviceKind::Producer => {
                device.id.component_type() == "generator"
                    && network
                        .generators()
                        .iter()
                        .any(|component| component.uid.as_deref() == Some(device.id.local_id()))
            }
            super::scuc_inputs::ScucDeviceKind::Consumer => {
                device.id.component_type() == "load"
                    && network
                        .loads()
                        .iter()
                        .any(|component| component.uid.as_deref() == Some(device.id.local_id()))
            }
        };
        if !found {
            return Err(shape_error(format!(
                "device {} has no matching component in the balanced network",
                device.id
            )));
        }
    }
    for shunt in &inputs.shunts {
        require_unique(&mut ids, &shunt.id)?;
        if shunt.id.component_type() != "shunt"
            || !network
                .shunts()
                .iter()
                .any(|component| component.uid.as_deref() == Some(shunt.id.local_id()))
        {
            return Err(shape_error(format!(
                "shunt {} has no matching component in the balanced network",
                shunt.id
            )));
        }
    }
    for switching in &inputs.branch_switching_costs {
        require_branch(network, &switching.id)?;
    }
    for control in &inputs.transformer_controls {
        require_branch(network, &control.id)?;
    }
    for zone in &inputs.active_reserve_zones {
        require_unique(&mut ids, &zone.id)?;
        require_periods(
            &zone.id,
            "ramping up requirement",
            zone.ramping_up_requirement.len(),
            periods,
        )?;
        require_periods(
            &zone.id,
            "ramping down requirement",
            zone.ramping_down_requirement.len(),
            periods,
        )?;
        require_buses(network, &zone.buses)?;
    }
    for zone in &inputs.reactive_reserve_zones {
        require_unique(&mut ids, &zone.id)?;
        require_periods(
            &zone.id,
            "reactive up requirement",
            zone.reactive_up_requirement.len(),
            periods,
        )?;
        require_periods(
            &zone.id,
            "reactive down requirement",
            zone.reactive_down_requirement.len(),
            periods,
        )?;
        require_buses(network, &zone.buses)?;
    }
    for contingency in &inputs.contingencies {
        require_unique(&mut ids, &contingency.id)?;
        if contingency.components.is_empty() {
            return Err(shape_error(format!(
                "contingency {} contains no component",
                contingency.id
            )));
        }
        for component in &contingency.components {
            require_branch(network, component)?;
        }
    }
    Ok(())
}

fn scheduling_discarded() -> powerio_core::Diagnostic {
    transform_discarded(
        "the scheduling, reserve, contingency, energy window, and violation cost categories",
    )
}

fn shape_error(message: impl Into<String>) -> Error {
    Error::new(&codes::BUILD_INSTANCE_SHAPE_MISMATCH, message)
}

fn require_unique(ids: &mut HashSet<ComponentId>, id: &ComponentId) -> Result<(), Error> {
    if ids.insert(id.clone()) {
        Ok(())
    } else {
        Err(shape_error(format!("component identity {id} is repeated")))
    }
}

fn require_periods(
    id: &ComponentId,
    field: &str,
    actual: usize,
    expected: usize,
) -> Result<(), Error> {
    if actual == expected {
        Ok(())
    } else {
        Err(shape_error(format!(
            "{id} {field} has {actual} intervals; expected {expected}"
        )))
    }
}

fn require_buses(network: &BalancedNetwork, buses: &[ComponentId]) -> Result<(), Error> {
    for id in buses {
        if id.component_type() != "bus"
            || !network
                .buses()
                .iter()
                .any(|component| component.uid.as_deref() == Some(id.local_id()))
        {
            return Err(shape_error(format!(
                "reserve zone bus {id} has no matching bus in the balanced network"
            )));
        }
    }
    Ok(())
}

fn require_branch(network: &BalancedNetwork, id: &ComponentId) -> Result<(), Error> {
    let component_type = id.component_type();
    let found = match component_type {
        "branch" | "transformer" => network
            .branches()
            .iter()
            .any(|component| component.uid.as_deref() == Some(id.local_id())),
        "hvdc" => network
            .hvdc()
            .iter()
            .any(|component| component.uid.as_deref() == Some(id.local_id())),
        _ => false,
    };
    if found {
        Ok(())
    } else {
        Err(shape_error(format!(
            "{id} has no matching branch in the balanced network"
        )))
    }
}
