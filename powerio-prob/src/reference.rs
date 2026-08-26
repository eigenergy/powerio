use serde::{Deserialize, Serialize};

use crate::Result;

/// The reference (slack) buses of a problem instance, as dense bus indices in
/// ascending order.
///
/// Every island of a network needs a reference bus, and the instance builders
/// check that before they assemble, so a network of several islands gives
/// several entries. A formulation grounds every entry. One that can ground
/// only one bus calls [`single`](Self::single), which states the condition it
/// needs instead of taking the first entry and grounding one island of many.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ReferenceBuses(Vec<usize>);

impl ReferenceBuses {
    #[must_use]
    pub fn new(buses: Vec<usize>) -> Self {
        Self(buses)
    }

    pub fn iter(&self) -> std::slice::Iter<'_, usize> {
        self.0.iter()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The one reference bus.
    ///
    /// # Errors
    /// [`powerio_tx::Error::ReferenceBusCount`] unless the set holds exactly one bus.
    pub fn single(&self) -> Result<usize> {
        match self.0.as_slice() {
            [bus] => Ok(*bus),
            other => Err(powerio_tx::Error::reference_bus_count(other.len()).into()),
        }
    }
}

impl AsRef<[usize]> for ReferenceBuses {
    fn as_ref(&self) -> &[usize] {
        &self.0
    }
}

impl<'a> IntoIterator for &'a ReferenceBuses {
    type Item = &'a usize;
    type IntoIter = std::slice::Iter<'a, usize>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}
