#![forbid(unsafe_code)]

pub use powerio_core_prototype::{PioModule, ScenarioSet, TimePoint, TimeSeries};
pub use powerio_dist_prototype::MulticonductorNetwork;
pub use powerio_prob_prototype::{
    DcPfInstance, OperatingPoint, balanced_operating_points, multiconductor_operating_points,
};
pub use powerio_tx_prototype::BalancedNetwork;

use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum PioValueKind {
    BalancedNetwork,
    MulticonductorNetwork,
    BalancedNetworkTimeSeries,
    BalancedOperatingPointTimeSeries,
    MulticonductorOperatingPointTimeSeries,
    DcPfInstance,
}

impl PioValueKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::BalancedNetwork => "balanced_network",
            Self::MulticonductorNetwork => "multiconductor_network",
            Self::BalancedNetworkTimeSeries => "balanced_network_time_series",
            Self::BalancedOperatingPointTimeSeries => "balanced_operating_point_time_series",
            Self::MulticonductorOperatingPointTimeSeries => {
                "multiconductor_operating_point_time_series"
            }
            Self::DcPfInstance => "dc_pf_instance",
        }
    }
}

#[derive(Debug)]
#[non_exhaustive]
pub enum PioValue {
    BalancedNetwork(BalancedNetwork),
    MulticonductorNetwork(MulticonductorNetwork),
    BalancedNetworkTimeSeries(TimeSeries<BalancedNetwork>),
    BalancedOperatingPointTimeSeries(TimeSeries<OperatingPoint<BalancedNetwork>>),
    MulticonductorOperatingPointTimeSeries(TimeSeries<OperatingPoint<MulticonductorNetwork>>),
    DcPfInstance(DcPfInstance),
}

impl PioValue {
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
            Self::DcPfInstance(_) => PioValueKind::DcPfInstance,
        }
    }
}

#[derive(Debug)]
pub struct ValueKindMismatch {
    expected: PioValueKind,
    module: PioModule<PioValue>,
}

impl ValueKindMismatch {
    pub fn expected(&self) -> PioValueKind {
        self.expected
    }

    pub fn actual(&self) -> PioValueKind {
        self.module.value().kind()
    }

    pub fn into_module(self) -> PioModule<PioValue> {
        self.module
    }
}

impl fmt::Display for ValueKindMismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
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
/// `PioValue` variant. Implementations are closed because adding one also adds a
/// stored and binding value kind. `PioModule<T>` itself has no such bound.
#[doc(hidden)]
pub trait FromPioValue: private::Sealed + Sized {
    const KIND: PioValueKind;
    fn try_from_pio_value(value: PioValue) -> Result<Self, PioValue>;
}

/// Checks the dynamic kind and moves the value and module records into a typed
/// module. Callers do not need to import a conversion trait.
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
    TimeSeries<BalancedNetwork>,
    BalancedNetworkTimeSeries,
    BalancedNetworkTimeSeries
);
typed_module!(
    TimeSeries<OperatingPoint<BalancedNetwork>>,
    BalancedOperatingPointTimeSeries,
    BalancedOperatingPointTimeSeries
);
typed_module!(
    TimeSeries<OperatingPoint<MulticonductorNetwork>>,
    MulticonductorOperatingPointTimeSeries,
    MulticonductorOperatingPointTimeSeries
);
typed_module!(DcPfInstance, DcPfInstance, DcPfInstance);

/// The standard-library conversion cannot be implemented in this crate.
/// Both `TryFrom` and the outer `PioModule` type are foreign to the facade;
/// placing the local `PioValue` inside that foreign type does not satisfy
/// Rust's orphan rule.
///
/// ```compile_fail
/// use powerio_facade_prototype::{BalancedNetwork, PioModule, PioValue};
///
/// impl TryFrom<PioModule<PioValue>> for PioModule<BalancedNetwork> {
///     type Error = ();
///     fn try_from(_: PioModule<PioValue>) -> Result<Self, Self::Error> {
///         unimplemented!()
///     }
/// }
/// ```
pub struct StandardTryFromIsNotCoherent;

#[cfg(test)]
mod tests {
    use super::*;
    use powerio_core_prototype::Source;
    use std::sync::Arc;

    #[test]
    fn facade_narrowing_moves_value_and_records_across_crate_boundaries() {
        let network = BalancedNetwork::new(vec![1, 2]);
        let bus_ptr = network.bus_ids().as_ptr();
        let module = PioModule::new(PioValue::BalancedNetwork(network))
            .with_diagnostic("kept")
            .with_source(Source::from_bytes(
                "case.m",
                Arc::<[u8]>::from(&b"source"[..]),
            ));
        let diagnostics_ptr = module.diagnostics().as_ptr();
        let source_ptr = module.source().unwrap().bytes().as_ptr();

        let typed: PioModule<BalancedNetwork> = try_into_typed(module).unwrap();

        assert_eq!(typed.value().bus_ids().as_ptr(), bus_ptr);
        assert_eq!(typed.diagnostics().as_ptr(), diagnostics_ptr);
        assert_eq!(typed.source().unwrap().bytes().as_ptr(), source_ptr);
    }

    #[test]
    fn mismatch_returns_the_dynamic_module() {
        let module = PioModule::new(PioValue::MulticonductorNetwork(MulticonductorNetwork::new(
            vec![1],
        )));
        let error = try_into_typed::<BalancedNetwork>(module).unwrap_err();
        assert_eq!(error.actual(), PioValueKind::MulticonductorNetwork);
        assert_eq!(
            error.into_module().value().kind(),
            PioValueKind::MulticonductorNetwork
        );
    }

    #[test]
    fn specialized_operating_points_live_in_the_calculation_crate() {
        let network = BalancedNetwork::new(vec![1, 2]);
        let bus_ptr = network.bus_ids().as_ptr();
        let series = balanced_operating_points(network, 2, vec![1.0, 2.0, 3.0, 4.0]).unwrap();
        let retained = series.value(1).unwrap().clone();
        drop(series);
        assert_eq!(retained.network().bus_ids().as_ptr(), bus_ptr);
        assert_eq!(retained.load_p(), [3.0, 4.0]);
    }

    #[test]
    fn operating_point_shapes_return_errors_without_panicking() {
        let network = BalancedNetwork::new(vec![1, 2]);
        assert!(balanced_operating_points(network.clone(), 2, vec![1.0]).is_err());
        assert!(balanced_operating_points(network, usize::MAX, Vec::new()).is_err());
    }

    #[test]
    fn application_types_remain_typed_rust_only() {
        #[derive(Debug)]
        struct ApplicationValue(Arc<()>);

        let module = PioModule::new(ApplicationValue(Arc::new(())));
        assert_eq!(Arc::strong_count(&module.value().0), 1);
    }
}
