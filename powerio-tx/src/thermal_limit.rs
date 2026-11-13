//! Numeric rating checks and format-boundary conversions.

use crate::{Error, Result};

pub(crate) fn validate(value: f64) -> Result<f64> {
    if value >= 0.0 {
        Ok(value)
    } else {
        Err(Error::InvalidThermalLimit { magnitude: value })
    }
}

pub(crate) fn finite(value: f64) -> Result<f64> {
    if value.is_finite() {
        validate(value)
    } else {
        Err(Error::InvalidThermalLimit { magnitude: value })
    }
}

/// Optional fields in exchange formats omit an infinite bound.
pub(crate) fn as_option(value: f64) -> Option<f64> {
    value.is_finite().then_some(value)
}

/// The inverse of [`as_option`]: an absent rating imposes no bound.
pub(crate) fn from_option(value: Option<f64>) -> Result<f64> {
    value.map_or(Ok(f64::INFINITY), finite)
}

/// Formats such as MATPOWER use zero to mean no bound.
pub(crate) fn from_zero_unlimited(value: f64) -> Result<f64> {
    validate(value).map(|value| if value == 0.0 { f64::INFINITY } else { value })
}

/// The inverse of [`from_zero_unlimited`]: an unlimited rating is written as
/// zero. A zero bound becomes indistinguishable from no bound, which
/// `append_emit_warnings` reports for these formats.
pub(crate) fn to_zero_unlimited(value: f64) -> f64 {
    as_option(value).unwrap_or(0.0)
}

pub(crate) fn scaled(value: f64, factor: f64) -> Result<f64> {
    validate(value)?;
    if !factor.is_finite() || factor <= 0.0 {
        return Err(Error::InvalidThermalLimit { magnitude: factor });
    }
    if value == f64::INFINITY {
        return Ok(value);
    }
    let result = finite(value * factor)?;
    if value > 0.0 && result == 0.0 {
        return Err(Error::InvalidThermalLimit { magnitude: result });
    }
    Ok(result)
}

#[cfg(test)]
pub(crate) fn assert_close(actual: f64, expected: f64, tolerance: f64) {
    assert!(
        (actual == f64::INFINITY && expected == f64::INFINITY)
            || (actual - expected).abs() < tolerance,
        "{actual} != {expected}"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_and_infinity_keep_their_meaning_when_scaled() {
        assert_eq!(scaled(0.0, 0.01).unwrap(), 0.0);
        assert_eq!(scaled(f64::INFINITY, 0.01).unwrap(), f64::INFINITY);
        assert_eq!(scaled(150.0, 0.01).unwrap(), 1.5);
        assert_eq!(from_zero_unlimited(0.0).unwrap(), f64::INFINITY);
        for value in [-1.0, f64::NAN, f64::NEG_INFINITY] {
            assert!(validate(value).is_err());
        }
        assert!(scaled(f64::MAX, 2.0).is_err());
        assert!(scaled(f64::from_bits(1), 0.25).is_err());
        for factor in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert!(scaled(0.0, factor).is_err());
            assert!(scaled(f64::INFINITY, factor).is_err());
        }
    }
}
