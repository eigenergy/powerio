//! The model kind and the single typed IR payload.
//!
//! The two IR families never merge. [`ModelKind`] is stored explicitly on the
//! package; [`ModelPayload`] is the tagged wrapper around exactly one payload.
//! The payload's `kind()` must agree with the package's `model_kind` (the
//! package asserts this), but the authoritative kind is the standalone field, so
//! a reader never infers the kind from which payload field is present.

use serde::{Deserialize, Serialize};

use powerio::BalancedNetwork;
use powerio_dist::MulticonductorNetwork;

/// Which concrete static-grid IR family the payload is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ModelKind {
    /// Scalar positive-sequence transmission model ([`powerio::BalancedNetwork`]).
    Balanced,
    /// Wire-coordinate distribution model ([`powerio_dist::MulticonductorNetwork`]).
    Multiconductor,
}

/// The one IR payload a package carries, tagged by `kind` in JSON so the payload
/// is self-describing in addition to the top-level `model_kind`.
///
/// The payload is a direct serde snapshot of the live PowerIO Rust IR
/// ([`powerio::Network`] / [`powerio_dist::DistNetwork`]). It is experimental: it
/// grows whenever the IR does, with no envelope version change. Only the envelope
/// is versioned. See `docs/src/pio-json-schema.md`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ModelPayload {
    Balanced {
        balanced_network: Box<BalancedNetwork>,
    },
    Multiconductor {
        multiconductor_network: Box<MulticonductorNetwork>,
    },
}

impl ModelPayload {
    pub fn balanced(net: BalancedNetwork) -> Self {
        Self::Balanced {
            balanced_network: Box::new(net),
        }
    }

    pub fn multiconductor(net: MulticonductorNetwork) -> Self {
        Self::Multiconductor {
            multiconductor_network: Box::new(net),
        }
    }

    pub fn kind(&self) -> ModelKind {
        match self {
            ModelPayload::Balanced { .. } => ModelKind::Balanced,
            ModelPayload::Multiconductor { .. } => ModelKind::Multiconductor,
        }
    }

    /// The balanced payload, if this is one.
    pub fn as_balanced(&self) -> Option<&BalancedNetwork> {
        match self {
            ModelPayload::Balanced { balanced_network } => Some(balanced_network),
            ModelPayload::Multiconductor { .. } => None,
        }
    }

    /// The multiconductor payload, if this is one.
    pub fn as_multiconductor(&self) -> Option<&MulticonductorNetwork> {
        match self {
            ModelPayload::Multiconductor {
                multiconductor_network,
            } => Some(multiconductor_network),
            ModelPayload::Balanced { .. } => None,
        }
    }
}
