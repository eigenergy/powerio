//! The AC security constrained unit commitment instance, following the DOE
//! GO Challenge 3 mathematical definition and data format.
//!
//! `AcScucInstance` contains the balanced electrical network plus the
//! scheduling categories the challenge defines: time points and interval
//! durations, initial states, time varying bounds and availability, price
//! blocks and device costs, reserve zones and memberships, energy windows,
//! contingencies, and violation costs — carried by [`ScucInputs`], the typed
//! GO Challenge 3 categories this crate already reads. Asking only for the
//! network discards scheduling, reserve, and contingency data; the
//! transformations below report those omissions.

use powerio_core::Error;
use powerio_tx::BalancedNetwork;

use crate::diagnostics::codes;
use crate::instance::balanced::{
    AcOpfInstance, AcPfInstance, DcOpfInstance, DcPfInstance, transform_discarded,
};
use crate::scopf::ScucInputs;

/// The AC security constrained unit commitment instance: the shared balanced
/// network plus the complete GO Challenge 3 input categories.
#[derive(Clone, Debug)]
pub struct AcScucInstance {
    network: BalancedNetwork,
    inputs: ScucInputs,
}

impl AcScucInstance {
    /// Assemble the instance from a network and the typed GO Challenge 3
    /// categories. The format mapping that builds both halves from one
    /// document owns identity reconciliation; this constructor checks the
    /// shapes that must agree for the pair to describe one system.
    ///
    /// # Errors
    /// A bus count disagreement between the network and the inputs.
    pub fn new(network: BalancedNetwork, inputs: ScucInputs) -> Result<Self, Error> {
        let stated = inputs.static_data.bus.len();
        if stated != network.buses().len() {
            return Err(Error::new(
                &codes::BUILD_INSTANCE_SHAPE_MISMATCH,
                format!(
                    "the GO Challenge 3 inputs state {stated} buses; the network states {}",
                    network.buses().len()
                ),
            ));
        }
        Ok(Self { network, inputs })
    }

    /// The balanced network this instance schedules. Borrowed; never a copy.
    #[must_use]
    pub fn network(&self) -> &BalancedNetwork {
        &self.network
    }

    /// The complete GO Challenge 3 input categories.
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

fn scheduling_discarded() -> powerio_core::Diagnostic {
    transform_discarded(
        "the scheduling, reserve, contingency, energy window, and violation cost categories",
    )
}
