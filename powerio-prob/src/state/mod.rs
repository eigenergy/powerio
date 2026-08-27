//! Operating point state: complete instantaneous electrical states over an
//! immutable network handle.
//!
//! An [`OperatingPoint`] is a small owning handle: one cheap to clone network
//! handle, one shared column store, and one row index. Building a series
//! resolves every stable element identity once into a private layout; the
//! points share the layout and the columns, so retaining one point after
//! dropping its parent collection copies nothing and materializes no network.
//!
//! Quantities are the instantaneous state set: voltages, injections,
//! in-service and switch states, tap positions, phase shifts, and capacitor
//! or regulator settings. Parameters, inventory, connectivity, time varying
//! bounds, availability, commitment, reserves, and costs are network or
//! calculation data, never operating point fields.
//!
//! Storage is private and comes in two spellings a builder can supply: dense
//! point major columns, and sparse per point overrides over one base state.
//! The accessors read identically from either.

mod balanced;
mod multiconductor;

pub use balanced::{BALANCED_STATE_QUANTITIES, BalancedOperatingPoints, BalancedStateBuilder};
pub use multiconductor::{MulticonductorOperatingPoints, MulticonductorStateBuilder};

/// One complete instantaneous state: a small owning handle over the
/// shared network and the series' shared columns. Cloning it or retaining it
/// after the parent series drops copies no table and no column.
#[derive(Clone, Debug)]
pub struct OperatingPoint<N> {
    pub(crate) network: N,
    pub(crate) columns: SharedColumns,
    pub(crate) index: usize,
}

impl<N> OperatingPoint<N> {
    /// The network this state instantiates. Borrowed; never a copy.
    pub fn network(&self) -> &N {
        &self.network
    }

    fn value(&self, quantity: &'static str, identity: &str) -> Option<f64> {
        self.columns
            .quantities
            .get(quantity)?
            .value(self.index, identity)
    }

    fn stated(&self, quantity: &'static str) -> bool {
        self.columns.quantities.contains_key(quantity)
    }

    /// The identities one stated quantity's columns follow, in the resolved
    /// stable order — the order every bulk constructor and bulk read uses.
    /// `None` when the series does not state the quantity.
    pub fn identity_order(&self, quantity: &str) -> Option<impl ExactSizeIterator<Item = &str>> {
        Some(self.columns.quantities.get(quantity)?.layout.order())
    }

    /// One stated quantity's complete values for this point, in identity
    /// order. Dense storage copies the row once; sparse storage materializes
    /// it from the base and this point's overrides.
    pub fn quantity_values(&self, quantity: &str) -> Option<Vec<f64>> {
        let entry = self.columns.quantities.get(quantity)?;
        let mut scratch = Vec::new();
        Some(
            entry
                .storage
                .row(self.index, entry.layout.len(), &mut scratch)
                .to_vec(),
        )
    }
}

use std::collections::HashMap;
use std::sync::Arc;

use powerio_core::Error;

use crate::diagnostics::codes;

/// One quantity's resolved column block: the identity order the network
/// tables state, with a hash lookup so keyed access never scans.
#[derive(Debug, Default)]
pub(crate) struct QuantityLayout {
    /// Column position by resolved identity.
    index: HashMap<Box<str>, u32>,
    /// The identities in stable network table order; `index` inverts this.
    order: Vec<Box<str>>,
}

impl QuantityLayout {
    pub(crate) fn from_order(
        quantity: &'static str,
        order: impl IntoIterator<Item = String>,
    ) -> Result<Self, Error> {
        let mut layout = Self::default();
        for identity in order {
            let column = u32::try_from(layout.order.len()).map_err(|_| {
                Error::new(
                    &codes::BUILD_STATE_SHAPE_MISMATCH,
                    format!("{quantity}: more than u32::MAX elements"),
                )
            })?;
            if layout
                .index
                .insert(identity.clone().into_boxed_str(), column)
                .is_some()
            {
                return Err(Error::new(
                    &codes::BUILD_STATE_IDENTITY_UNKNOWN,
                    format!("{quantity}: duplicate element identity `{identity}`"),
                ));
            }
            layout.order.push(identity.into_boxed_str());
        }
        Ok(layout)
    }

    pub(crate) fn len(&self) -> usize {
        self.order.len()
    }

    pub(crate) fn column(&self, identity: &str) -> Option<usize> {
        self.index.get(identity).map(|column| *column as usize)
    }

    pub(crate) fn order(&self) -> impl ExactSizeIterator<Item = &str> {
        self.order.iter().map(AsRef::as_ref)
    }
}

/// One point's sparse overrides: `(column, value)` pairs sorted by column.
pub(crate) type PointChanges = Box<[(u32, f64)]>;

/// One quantity's values across every point: dense rows, or sparse overrides
/// over one base row. Both address `(point, column)`.
#[derive(Debug)]
pub(crate) enum QuantityStorage {
    Dense {
        /// `point_count * width` values, point major.
        values: Box<[f64]>,
    },
    Sparse {
        /// The base row every point starts from, `width` long.
        base: Box<[f64]>,
        /// Per point overrides, each sorted by column.
        changes: Box<[PointChanges]>,
    },
}

impl QuantityStorage {
    pub(crate) fn value(&self, point: usize, column: usize, width: usize) -> f64 {
        match self {
            Self::Dense { values } => values[point * width + column],
            Self::Sparse { base, changes } => {
                let column32 = column as u32;
                match changes[point].binary_search_by_key(&column32, |(c, _)| *c) {
                    Ok(found) => changes[point][found].1,
                    Err(_) => base[column],
                }
            }
        }
    }

    /// The whole row for one point. Dense storage borrows; sparse storage
    /// materializes the row once into the caller's buffer.
    pub(crate) fn row<'a>(
        &'a self,
        point: usize,
        width: usize,
        scratch: &'a mut Vec<f64>,
    ) -> &'a [f64] {
        match self {
            Self::Dense { values } => &values[point * width..(point + 1) * width],
            Self::Sparse { base, changes } => {
                scratch.clear();
                scratch.extend_from_slice(base);
                for (column, value) in &changes[point] {
                    scratch[*column as usize] = *value;
                }
                scratch
            }
        }
    }
}

/// One named quantity: its identity layout and its storage.
#[derive(Debug)]
pub(crate) struct Quantity {
    pub(crate) layout: QuantityLayout,
    pub(crate) storage: QuantityStorage,
}

impl Quantity {
    pub(crate) fn value(&self, point: usize, identity: &str) -> Option<f64> {
        let column = self.layout.column(identity)?;
        Some(self.storage.value(point, column, self.layout.len()))
    }
}

/// The shared column store behind every point of one series.
#[derive(Debug)]
pub(crate) struct StateColumns {
    pub(crate) quantities: HashMap<&'static str, Quantity>,
}

pub(crate) type SharedColumns = Arc<StateColumns>;

/// Validates one dense column block against its layout.
pub(crate) fn dense_quantity(
    quantity: &'static str,
    layout: QuantityLayout,
    point_count: usize,
    values: Vec<f64>,
) -> Result<Quantity, Error> {
    let expected = point_count.checked_mul(layout.len()).ok_or_else(|| {
        Error::new(
            &codes::BUILD_STATE_SHAPE_MISMATCH,
            format!(
                "{quantity}: {point_count} points by {} elements exceeds addressable memory",
                layout.len()
            ),
        )
    })?;
    if values.len() != expected {
        return Err(Error::new(
            &codes::BUILD_STATE_SHAPE_MISMATCH,
            format!(
                "{quantity}: {} values supplied; {point_count} points by {} elements needs {expected}",
                values.len(),
                layout.len()
            ),
        ));
    }
    Ok(Quantity {
        layout,
        storage: QuantityStorage::Dense {
            values: values.into_boxed_slice(),
        },
    })
}

/// Validates one sparse column block: a base row plus per point keyed
/// overrides, resolved against the layout once.
pub(crate) fn sparse_quantity(
    quantity: &'static str,
    layout: QuantityLayout,
    point_count: usize,
    base: Vec<f64>,
    changes: Vec<Vec<(String, f64)>>,
) -> Result<Quantity, Error> {
    if base.len() != layout.len() {
        return Err(Error::new(
            &codes::BUILD_STATE_SHAPE_MISMATCH,
            format!(
                "{quantity}: base row has {} values; the layout has {} elements",
                base.len(),
                layout.len()
            ),
        ));
    }
    if changes.len() != point_count {
        return Err(Error::new(
            &codes::BUILD_STATE_SHAPE_MISMATCH,
            format!(
                "{quantity}: {} change sets supplied for {point_count} points",
                changes.len()
            ),
        ));
    }
    let mut resolved = Vec::with_capacity(point_count);
    for point in changes {
        let mut row: Vec<(u32, f64)> = Vec::with_capacity(point.len());
        for (identity, value) in point {
            let Some(column) = layout.column(&identity) else {
                return Err(Error::new(
                    &codes::BUILD_STATE_IDENTITY_UNKNOWN,
                    format!("{quantity}: unknown element identity `{identity}`"),
                ));
            };
            row.push((column as u32, value));
        }
        row.sort_unstable_by_key(|(column, _)| *column);
        row.dedup_by_key(|(column, _)| *column);
        resolved.push(row.into_boxed_slice());
    }
    Ok(Quantity {
        layout,
        storage: QuantityStorage::Sparse {
            base: base.into_boxed_slice(),
            changes: resolved.into_boxed_slice(),
        },
    })
}

/// The payload identity of one element row: its stated uid, or the
/// `{table}:{row}` value the stored document mints for a row without one.
/// Resolution never mutates the network.
pub(crate) fn row_identity(uid: Option<&str>, table: &str, row: usize) -> String {
    match uid {
        Some(uid) => uid.to_owned(),
        None => format!("{table}:{row}"),
    }
}
