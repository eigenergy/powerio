//! Operating points: alternate electrical assignments over an immutable
//! network handle.
//!
//! An [`OperatingPoint`] is a small owning handle: one cheap to clone network
//! handle, one shared column store, and one row index. Building a series
//! resolves every stable element identity once into a private layout; the
//! points share the layout and the columns, so retaining one point after
//! dropping its parent collection copies no network.
//!
//! Quantities include voltages, injections, in-service flags, switch
//! positions, tap positions, phase shifts, and capacitor
//! or regulator settings. Parameters, the equipment set, connectivity, time varying
//! bounds, availability, commitment, reserves, and costs are network or
//! calculation data, never operating point fields.
//!
//! Builders accept dense point major columns or sparse per point overrides
//! over one base assignment. Both forms have the same accessors.

pub(crate) mod balanced;
pub(crate) mod multiconductor;

pub use balanced::{
    BalancedOperatingPointBuilder, BalancedOperatingPointFlag, BalancedOperatingPointQuantity,
};
pub use multiconductor::{
    MulticonductorOperatingPointBuilder, MulticonductorOperatingPointFlag,
    MulticonductorOperatingPointQuantity,
};

/// One possibly partial alternate electrical assignment: a small owning handle over the
/// shared network and the series' shared columns. Cloning it or retaining it
/// after the parent series drops copies no table and no column.
#[derive(Clone, Debug)]
pub struct OperatingPoint<N> {
    pub(crate) network: N,
    pub(crate) columns: SharedColumns,
    pub(crate) index: usize,
}

impl<N> OperatingPoint<N> {
    /// The network whose equipment identities and defaults this point uses.
    pub fn network(&self) -> &N {
        &self.network
    }

    fn value(&self, quantity: &'static str, identity: &str) -> Option<f64> {
        self.columns
            .quantities
            .get(quantity)?
            .value(self.index, identity)
    }

    fn iter_values(&self, quantity: &'static str) -> Option<OperatingPointValues<'_>> {
        Some(OperatingPointValues {
            quantity: self.columns.quantities.get(quantity)?,
            point: self.index,
            column: 0,
        })
    }

    fn iter_flags(&self, quantity: &'static str) -> Option<OperatingPointFlags<'_>> {
        Some(OperatingPointFlags(self.iter_values(quantity)?))
    }

    pub(crate) fn identity_order(
        &self,
        quantity: &str,
    ) -> Option<impl ExactSizeIterator<Item = &str>> {
        Some(self.columns.quantities.get(quantity)?.layout.order())
    }
}

/// One operating point quantity in stable component identity order.
#[derive(Clone, Debug)]
pub struct OperatingPointValues<'a> {
    quantity: &'a Quantity,
    point: usize,
    column: usize,
}

impl<'a> Iterator for OperatingPointValues<'a> {
    type Item = (&'a str, f64);

    fn next(&mut self) -> Option<Self::Item> {
        let identity = self.quantity.layout.order.get(self.column)?;
        let value =
            self.quantity
                .storage
                .value(self.point, self.column, self.quantity.layout.len());
        self.column += 1;
        Some((identity.as_ref(), value))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.quantity.layout.len() - self.column;
        (remaining, Some(remaining))
    }
}

impl ExactSizeIterator for OperatingPointValues<'_> {}

/// One operating point flag in stable component identity order.
#[derive(Clone, Debug)]
pub struct OperatingPointFlags<'a>(OperatingPointValues<'a>);

impl<'a> Iterator for OperatingPointFlags<'a> {
    type Item = (&'a str, bool);

    fn next(&mut self) -> Option<Self::Item> {
        self.0
            .next()
            .map(|(identity, value)| (identity, value != 0.0))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.0.size_hint()
    }
}

impl ExactSizeIterator for OperatingPointFlags<'_> {}

use std::collections::HashMap;
use std::sync::Arc;

use powerio_core::Error;

use crate::diagnostics::codes;

/// One quantity's resolved column block: the identity order the network
/// tables define, with a hash lookup so keyed access never scans.
#[derive(Clone, Debug, Default)]
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
                    &codes::BUILD_OPERATING_POINT_SHAPE_MISMATCH,
                    format!("{quantity}: more than u32::MAX elements"),
                )
            })?;
            if layout
                .index
                .insert(identity.clone().into_boxed_str(), column)
                .is_some()
            {
                return Err(Error::new(
                    &codes::BUILD_OPERATING_POINT_IDENTITY_UNKNOWN,
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
#[derive(Clone, Debug)]
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

    pub(crate) fn replace(
        &mut self,
        point: usize,
        column: usize,
        width: usize,
        replacement: f64,
    ) -> bool {
        let previous = self.value(point, column, width);
        if previous.to_bits() == replacement.to_bits() {
            return false;
        }
        match self {
            Self::Dense { values } => values[point * width + column] = replacement,
            Self::Sparse { base, changes } => {
                let column = column as u32;
                let row = &mut changes[point];
                let mut updated = row.to_vec();
                match updated.binary_search_by_key(&column, |(entry, _)| *entry) {
                    Ok(found) if replacement.to_bits() == base[column as usize].to_bits() => {
                        updated.remove(found);
                    }
                    Ok(found) => updated[found].1 = replacement,
                    Err(_) if replacement.to_bits() == base[column as usize].to_bits() => {}
                    Err(insert_at) => updated.insert(insert_at, (column, replacement)),
                }
                *row = updated.into_boxed_slice();
            }
        }
        true
    }
}

/// One named quantity: its identity layout and its storage.
#[derive(Clone, Debug)]
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
#[derive(Clone, Debug)]
pub(crate) struct OperatingPointColumns {
    pub(crate) point_count: usize,
    pub(crate) quantities: HashMap<&'static str, Quantity>,
}

pub(crate) type SharedColumns = Arc<OperatingPointColumns>;

impl<N> OperatingPoint<N> {
    pub(crate) fn replace_value(
        &mut self,
        quantity_name: &'static str,
        layout: QuantityLayout,
        defaults: &[f64],
        identity: &str,
        replacement: f64,
    ) -> Result<bool, Error> {
        let columns = Arc::make_mut(&mut self.columns);
        if !columns.quantities.contains_key(quantity_name) {
            if defaults.len() != layout.len() {
                return Err(Error::new(
                    &codes::BUILD_OPERATING_POINT_SHAPE_MISMATCH,
                    format!(
                        "{quantity_name}: {} defaults supplied for {} components",
                        defaults.len(),
                        layout.len()
                    ),
                ));
            }
            let mut values = Vec::with_capacity(defaults.len() * columns.point_count);
            for _ in 0..columns.point_count {
                values.extend_from_slice(defaults);
            }
            columns.quantities.insert(
                quantity_name,
                dense_quantity(quantity_name, layout, columns.point_count, values)?,
            );
        }
        let quantity = columns
            .quantities
            .get_mut(quantity_name)
            .expect("the quantity was inserted above");
        let Some(column) = quantity.layout.column(identity) else {
            return Err(Error::new(
                &codes::BUILD_OPERATING_POINT_IDENTITY_UNKNOWN,
                format!("{quantity_name}: unknown component identity `{identity}`"),
            ));
        };
        Ok(quantity
            .storage
            .replace(self.index, column, quantity.layout.len(), replacement))
    }
}

/// Validates one dense column block against its layout.
pub(crate) fn dense_quantity(
    quantity: &'static str,
    layout: QuantityLayout,
    point_count: usize,
    values: Vec<f64>,
) -> Result<Quantity, Error> {
    let expected = point_count.checked_mul(layout.len()).ok_or_else(|| {
        Error::new(
            &codes::BUILD_OPERATING_POINT_SHAPE_MISMATCH,
            format!(
                "{quantity}: {point_count} points by {} elements exceeds addressable memory",
                layout.len()
            ),
        )
    })?;
    if values.len() != expected {
        return Err(Error::new(
            &codes::BUILD_OPERATING_POINT_SHAPE_MISMATCH,
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
            &codes::BUILD_OPERATING_POINT_SHAPE_MISMATCH,
            format!(
                "{quantity}: base row has {} values; the layout has {} elements",
                base.len(),
                layout.len()
            ),
        ));
    }
    if changes.len() != point_count {
        return Err(Error::new(
            &codes::BUILD_OPERATING_POINT_SHAPE_MISMATCH,
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
                    &codes::BUILD_OPERATING_POINT_IDENTITY_UNKNOWN,
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
