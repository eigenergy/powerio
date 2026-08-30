//! The dynamic value boundary: [`PioValue`], its [`PioValueKind`] register,
//! and the checked narrowing back to a concrete module.

use std::fmt;

use powerio_core::PioModule;
use powerio_dist::MulticonductorNetwork;
use powerio_tx::BalancedNetwork;

/// The register of built in dynamic value kinds, one per [`PioValue`]
/// variant. The string spelling is permanent and shared by every surface:
/// Rust, C, Python, Julia, and the stored `.pio.json` document.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum PioValueKind {
    BalancedNetwork,
    MulticonductorNetwork,
    BalancedNetworkTimeSeries,
    BalancedOperatingPointTimeSeries,
    MulticonductorOperatingPointTimeSeries,
    BalancedNetworkScenarioSet,
    DcPfInstance,
    AcPfInstance,
    DcOpfInstance,
    AcOpfInstance,
    McAcPfInstance,
    McAcOpfInstance,
    AcScucInstance,
    DcPfSolution,
    AcPfSolution,
    DcOpfSolution,
    AcOpfSolution,
    McAcPfSolution,
    McAcOpfSolution,
    AcScucSolution,
}

impl PioValueKind {
    /// The kind's permanent string identifier.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::BalancedNetwork => "balanced_network",
            Self::BalancedNetworkTimeSeries => "balanced_network_time_series",
            Self::BalancedOperatingPointTimeSeries => "balanced_operating_point_time_series",
            Self::MulticonductorOperatingPointTimeSeries => {
                "multiconductor_operating_point_time_series"
            }
            Self::BalancedNetworkScenarioSet => "balanced_network_scenario_set",
            Self::MulticonductorNetwork => "multiconductor_network",
            Self::DcPfInstance => "dc_pf_instance",
            Self::AcPfInstance => "ac_pf_instance",
            Self::DcOpfInstance => "dc_opf_instance",
            Self::AcOpfInstance => "ac_opf_instance",
            Self::McAcPfInstance => "mc_ac_pf_instance",
            Self::McAcOpfInstance => "mc_ac_opf_instance",
            Self::AcScucInstance => "ac_scuc_instance",
            Self::DcPfSolution => "dc_pf_solution",
            Self::AcPfSolution => "ac_pf_solution",
            Self::DcOpfSolution => "dc_opf_solution",
            Self::AcOpfSolution => "ac_opf_solution",
            Self::McAcPfSolution => "mc_ac_pf_solution",
            Self::McAcOpfSolution => "mc_ac_opf_solution",
            Self::AcScucSolution => "ac_scuc_solution",
        }
    }
}

impl fmt::Display for PioValueKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// The dynamic form of a compiled value: the finite set of built in concrete
/// types a parse can produce, discovered at run time through
/// [`PioValue::kind`]. `PioModule<PioValue>` is what [`crate::parse_file`]
/// returns; ordinary callers inspect `module.value()` with enum matching.
/// [`try_into_typed`] is the advanced owned conversion when generic code needs
/// a concrete `PioModule<T>`. Application defined types stay typed Rust
/// (`PioModule<MyValue>`) and never enter this enum.
#[derive(Debug)]
#[non_exhaustive]
// The exact 20 variants are the architecture's fixed dynamic surface; the
// larger calculation values stay unboxed because the enum is a transport
// wrapper moved whole, never stored in bulk.
#[allow(clippy::large_enum_variant)]
pub enum PioValue {
    BalancedNetwork(BalancedNetwork),
    MulticonductorNetwork(MulticonductorNetwork),
    /// One balanced network per time point, static tables shared.
    BalancedNetworkTimeSeries(powerio_core::TimeSeries<BalancedNetwork>),
    /// One fixed balanced network under an operating point per time point.
    BalancedOperatingPointTimeSeries(powerio_prob::BalancedOperatingPoints),
    /// One fixed multiconductor network under an operating point per time
    /// point.
    MulticonductorOperatingPointTimeSeries(powerio_prob::MulticonductorOperatingPoints),
    /// One balanced network per scenario, shared element tables.
    BalancedNetworkScenarioSet(powerio_core::ScenarioSet<BalancedNetwork>),
    DcPfInstance(powerio_prob::DcPfInstance),
    AcPfInstance(powerio_prob::AcPfInstance),
    DcOpfInstance(powerio_prob::DcOpfInstance),
    AcOpfInstance(powerio_prob::AcOpfInstance),
    McAcPfInstance(powerio_prob::McAcPfInstance),
    McAcOpfInstance(powerio_prob::McAcOpfInstance),
    AcScucInstance(powerio_prob::AcScucInstance),
    DcPfSolution(powerio_prob::DcPfSolution),
    AcPfSolution(powerio_prob::AcPfSolution),
    DcOpfSolution(powerio_prob::DcOpfSolution),
    AcOpfSolution(powerio_prob::AcOpfSolution),
    McAcPfSolution(powerio_prob::McAcPfSolution),
    McAcOpfSolution(powerio_prob::McAcOpfSolution),
    AcScucSolution(powerio_prob::AcScucSolution),
}

impl PioValue {
    #[must_use]
    pub fn kind(&self) -> PioValueKind {
        match self {
            Self::BalancedNetwork(_) => PioValueKind::BalancedNetwork,
            Self::MulticonductorNetwork(_) => PioValueKind::MulticonductorNetwork,
            Self::BalancedNetworkTimeSeries(_) => PioValueKind::BalancedNetworkTimeSeries,
            Self::BalancedOperatingPointTimeSeries(_) => {
                PioValueKind::BalancedOperatingPointTimeSeries
            }
            Self::MulticonductorOperatingPointTimeSeries(_) => {
                PioValueKind::MulticonductorOperatingPointTimeSeries
            }
            Self::BalancedNetworkScenarioSet(_) => PioValueKind::BalancedNetworkScenarioSet,
            Self::DcPfInstance(_) => PioValueKind::DcPfInstance,
            Self::AcPfInstance(_) => PioValueKind::AcPfInstance,
            Self::DcOpfInstance(_) => PioValueKind::DcOpfInstance,
            Self::AcOpfInstance(_) => PioValueKind::AcOpfInstance,
            Self::McAcPfInstance(_) => PioValueKind::McAcPfInstance,
            Self::McAcOpfInstance(_) => PioValueKind::McAcOpfInstance,
            Self::AcScucInstance(_) => PioValueKind::AcScucInstance,
            Self::DcPfSolution(_) => PioValueKind::DcPfSolution,
            Self::AcPfSolution(_) => PioValueKind::AcPfSolution,
            Self::DcOpfSolution(_) => PioValueKind::DcOpfSolution,
            Self::AcOpfSolution(_) => PioValueKind::AcOpfSolution,
            Self::McAcPfSolution(_) => PioValueKind::McAcPfSolution,
            Self::McAcOpfSolution(_) => PioValueKind::McAcOpfSolution,
            Self::AcScucSolution(_) => PioValueKind::AcScucSolution,
        }
    }
}

/// The recoverable failure of [`try_into_typed`]: the expectation did not
/// match the parsed kind. The original dynamic module rides along untouched,
/// so the caller can inspect the actual kind and take the other route.
#[derive(Debug)]
pub struct ValueKindMismatch {
    expected: PioValueKind,
    module: PioModule<PioValue>,
}

impl ValueKindMismatch {
    #[must_use]
    pub fn expected(&self) -> PioValueKind {
        self.expected
    }

    #[must_use]
    pub fn actual(&self) -> PioValueKind {
        self.module.value().kind()
    }

    /// The dynamic module the narrowing was attempted on, unchanged.
    #[must_use]
    pub fn into_module(self) -> PioModule<PioValue> {
        self.module
    }
}

impl fmt::Display for ValueKindMismatch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "expected `{}`, found `{}`",
            self.expected.as_str(),
            self.actual().as_str()
        )
    }
}

impl std::error::Error for ValueKindMismatch {}

mod private {
    pub trait Sealed {}
}

/// Internal connection between one built in concrete type and its dynamic
/// [`PioValue`] variant. Implementations are closed because adding one also
/// adds a stored and binding value kind. `PioModule<T>` itself has no such
/// bound.
#[doc(hidden)]
pub trait FromPioValue: private::Sealed + Sized {
    const KIND: PioValueKind;
    // Boxing the failure would allocate and violate the recoverable no-copy
    // narrowing rule; the caller gets the original value back by value.
    #[allow(clippy::result_large_err)]
    fn try_from_pio_value(value: PioValue) -> Result<Self, PioValue>;
}

/// Move a dynamic module into one concrete `PioModule<T>` without cloning its
/// value or records.
///
/// Ordinary code can match directly on `module.value()`. This helper is for
/// generic code that requires an owned `PioModule<T>` after the match.
///
/// # Errors
/// [`ValueKindMismatch`] when the module holds another kind; it owns the
/// original dynamic module.
// Boxing the failure would allocate and violate the recoverable no-copy
// narrowing rule; the caller gets the original dynamic module back by value.
#[allow(clippy::result_large_err)]
pub fn try_into_typed<T: FromPioValue>(
    module: PioModule<PioValue>,
) -> Result<PioModule<T>, ValueKindMismatch> {
    match module.__try_map_value(T::try_from_pio_value) {
        Ok(module) => Ok(module),
        Err(module) => Err(ValueKindMismatch {
            expected: T::KIND,
            module,
        }),
    }
}

macro_rules! typed_module {
    ($ty:ty, $variant:ident, $kind:ident) => {
        impl private::Sealed for $ty {}

        impl FromPioValue for $ty {
            const KIND: PioValueKind = PioValueKind::$kind;

            fn try_from_pio_value(value: PioValue) -> Result<Self, PioValue> {
                match value {
                    PioValue::$variant(value) => Ok(value),
                    value => Err(value),
                }
            }
        }

        impl From<$ty> for PioValue {
            fn from(value: $ty) -> Self {
                Self::$variant(value)
            }
        }
    };
}

typed_module!(BalancedNetwork, BalancedNetwork, BalancedNetwork);
typed_module!(
    MulticonductorNetwork,
    MulticonductorNetwork,
    MulticonductorNetwork
);
typed_module!(
    powerio_core::TimeSeries<BalancedNetwork>,
    BalancedNetworkTimeSeries,
    BalancedNetworkTimeSeries
);
typed_module!(
    powerio_prob::BalancedOperatingPoints,
    BalancedOperatingPointTimeSeries,
    BalancedOperatingPointTimeSeries
);
typed_module!(
    powerio_core::ScenarioSet<BalancedNetwork>,
    BalancedNetworkScenarioSet,
    BalancedNetworkScenarioSet
);
typed_module!(
    powerio_prob::MulticonductorOperatingPoints,
    MulticonductorOperatingPointTimeSeries,
    MulticonductorOperatingPointTimeSeries
);
typed_module!(powerio_prob::DcPfInstance, DcPfInstance, DcPfInstance);
typed_module!(powerio_prob::AcPfInstance, AcPfInstance, AcPfInstance);
typed_module!(powerio_prob::DcOpfInstance, DcOpfInstance, DcOpfInstance);
typed_module!(powerio_prob::AcOpfInstance, AcOpfInstance, AcOpfInstance);
typed_module!(powerio_prob::McAcPfInstance, McAcPfInstance, McAcPfInstance);
typed_module!(
    powerio_prob::McAcOpfInstance,
    McAcOpfInstance,
    McAcOpfInstance
);
typed_module!(powerio_prob::AcScucInstance, AcScucInstance, AcScucInstance);
typed_module!(powerio_prob::DcPfSolution, DcPfSolution, DcPfSolution);
typed_module!(powerio_prob::AcPfSolution, AcPfSolution, AcPfSolution);
typed_module!(powerio_prob::DcOpfSolution, DcOpfSolution, DcOpfSolution);
typed_module!(powerio_prob::AcOpfSolution, AcOpfSolution, AcOpfSolution);
typed_module!(powerio_prob::McAcPfSolution, McAcPfSolution, McAcPfSolution);
typed_module!(
    powerio_prob::McAcOpfSolution,
    McAcOpfSolution,
    McAcOpfSolution
);
typed_module!(powerio_prob::AcScucSolution, AcScucSolution, AcScucSolution);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn narrowing_moves_the_value_and_records_without_allocation() {
        let network = small_balanced();
        let bus_ptr = network.buses().as_ptr();
        let module = PioModule::new(PioValue::from(network));
        let typed: PioModule<BalancedNetwork> = try_into_typed(module).unwrap();
        assert_eq!(typed.value().buses().as_ptr(), bus_ptr);
    }

    fn small_balanced() -> BalancedNetwork {
        use powerio_tx::{Bus, BusId, BusType};
        BalancedNetwork::in_memory(
            "facade",
            100.0,
            vec![Bus::new(BusId(1), BusType::Ref, 230.0)],
            Vec::new(),
        )
    }

    #[test]
    fn a_mismatch_returns_the_dynamic_module() {
        let module = PioModule::new(PioValue::from(MulticonductorNetwork::new()));
        let error = try_into_typed::<BalancedNetwork>(module).unwrap_err();
        assert_eq!(error.expected(), PioValueKind::BalancedNetwork);
        assert_eq!(error.actual(), PioValueKind::MulticonductorNetwork);
        assert_eq!(
            error.to_string(),
            "expected `balanced_network`, found `multiconductor_network`"
        );
        assert_eq!(
            error.into_module().value().kind(),
            PioValueKind::MulticonductorNetwork
        );
    }

    #[test]
    fn kind_strings_are_the_stored_spellings() {
        assert_eq!(PioValueKind::BalancedNetwork.as_str(), "balanced_network");
        assert_eq!(
            PioValueKind::MulticonductorNetwork.as_str(),
            "multiconductor_network"
        );
    }
}
