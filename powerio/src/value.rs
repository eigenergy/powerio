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
}

impl PioValueKind {
    /// The kind's permanent string identifier.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::BalancedNetwork => "balanced_network",
            Self::MulticonductorNetwork => "multiconductor_network",
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
/// [`PioValue::kind`]. `PioModule<PioValue>` is what [`crate::parse`]
/// returns; a caller that expects one concrete type narrows with
/// [`try_into_typed`]. Application defined types stay typed Rust
/// (`PioModule<MyValue>`) and never enter this enum.
#[derive(Debug)]
#[non_exhaustive]
pub enum PioValue {
    BalancedNetwork(BalancedNetwork),
    MulticonductorNetwork(MulticonductorNetwork),
}

impl PioValue {
    #[must_use]
    pub fn kind(&self) -> PioValueKind {
        match self {
            Self::BalancedNetwork(_) => PioValueKind::BalancedNetwork,
            Self::MulticonductorNetwork(_) => PioValueKind::MulticonductorNetwork,
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

/// Check the dynamic kind and move the value and module records into a typed
/// module. Successful narrowing moves the value, the retained source owner,
/// and the common records without allocation.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn narrowing_moves_the_value_and_records_without_allocation() {
        let network = small_balanced();
        let bus_ptr = network.buses.as_ptr();
        let module = PioModule::new(PioValue::from(network));
        let typed: PioModule<BalancedNetwork> = try_into_typed(module).unwrap();
        assert_eq!(typed.value().buses.as_ptr(), bus_ptr);
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
