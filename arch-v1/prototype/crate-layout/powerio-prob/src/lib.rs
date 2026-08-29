#![forbid(unsafe_code)]

use powerio_core_prototype::{TimePoint, TimeSeries};
use powerio_dist_prototype::MulticonductorNetwork;
use powerio_tx_prototype::BalancedNetwork;
use std::fmt;
use std::marker::PhantomData;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OperatingPointSeriesError {
    DimensionOverflow { point_count: usize, width: usize },
    ShapeMismatch { expected: usize, actual: usize },
}

impl fmt::Display for OperatingPointSeriesError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DimensionOverflow { point_count, width } => write!(
                f,
                "operating point shape {point_count} by {width} exceeds addressable memory"
            ),
            Self::ShapeMismatch { expected, actual } => write!(
                f,
                "operating point data has {actual} values; expected {expected}"
            ),
        }
    }
}

impl std::error::Error for OperatingPointSeriesError {}

#[derive(Clone, Debug)]
enum OperatingPointData {
    Balanced(Arc<[f64]>),
    Multiconductor(Arc<[f64]>),
}

#[derive(Clone, Debug)]
pub struct OperatingPoint<N> {
    network: N,
    data: OperatingPointData,
    index: usize,
    width: usize,
    marker: PhantomData<N>,
}

impl OperatingPoint<BalancedNetwork> {
    pub fn network(&self) -> &BalancedNetwork {
        &self.network
    }

    pub fn load_p(&self) -> &[f64] {
        let OperatingPointData::Balanced(values) = &self.data else {
            unreachable!("private constructor preserves the network family")
        };
        row(values, self.index, self.width)
    }
}

impl OperatingPoint<MulticonductorNetwork> {
    pub fn network(&self) -> &MulticonductorNetwork {
        &self.network
    }

    pub fn load_p(&self) -> &[f64] {
        let OperatingPointData::Multiconductor(values) = &self.data else {
            unreachable!("private constructor preserves the network family")
        };
        row(values, self.index, self.width)
    }
}

pub fn balanced_operating_points(
    network: BalancedNetwork,
    point_count: usize,
    load_p: Vec<f64>,
) -> Result<TimeSeries<OperatingPoint<BalancedNetwork>>, OperatingPointSeriesError> {
    let width = network.bus_ids().len();
    let expected = expected_len(point_count, width)?;
    if load_p.len() != expected {
        return Err(OperatingPointSeriesError::ShapeMismatch {
            expected,
            actual: load_p.len(),
        });
    }
    let columns: Arc<[f64]> = load_p.into();
    TimeSeries::new(
        prototype_time_points(point_count),
        (0..point_count)
            .map(|index| OperatingPoint {
                network: network.clone(),
                data: OperatingPointData::Balanced(Arc::clone(&columns)),
                index,
                width,
                marker: PhantomData,
            })
            .collect(),
    )
    .map_err(|_| OperatingPointSeriesError::ShapeMismatch {
        expected: point_count,
        actual: point_count,
    })
}

pub fn multiconductor_operating_points(
    network: MulticonductorNetwork,
    point_count: usize,
    load_p: Vec<f64>,
) -> Result<TimeSeries<OperatingPoint<MulticonductorNetwork>>, OperatingPointSeriesError> {
    let width = network.bus_ids().len();
    let expected = expected_len(point_count, width)?;
    if load_p.len() != expected {
        return Err(OperatingPointSeriesError::ShapeMismatch {
            expected,
            actual: load_p.len(),
        });
    }
    let columns: Arc<[f64]> = load_p.into();
    TimeSeries::new(
        prototype_time_points(point_count),
        (0..point_count)
            .map(|index| OperatingPoint {
                network: network.clone(),
                data: OperatingPointData::Multiconductor(Arc::clone(&columns)),
                index,
                width,
                marker: PhantomData,
            })
            .collect(),
    )
    .map_err(|_| OperatingPointSeriesError::ShapeMismatch {
        expected: point_count,
        actual: point_count,
    })
}

fn prototype_time_points(point_count: usize) -> Vec<TimePoint> {
    (0..point_count)
        .map(|index| TimePoint {
            label: index.to_string(),
            duration: None,
        })
        .collect()
}

fn expected_len(point_count: usize, width: usize) -> Result<usize, OperatingPointSeriesError> {
    point_count
        .checked_mul(width)
        .ok_or(OperatingPointSeriesError::DimensionOverflow { point_count, width })
}

fn row(values: &[f64], index: usize, width: usize) -> &[f64] {
    let start = index * width;
    &values[start..start + width]
}

#[derive(Clone, Debug)]
pub struct DcPfInstance {
    network: BalancedNetwork,
}

impl DcPfInstance {
    pub fn new(network: BalancedNetwork) -> Self {
        Self { network }
    }

    pub fn network(&self) -> &BalancedNetwork {
        &self.network
    }
}
