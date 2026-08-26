use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use crate::Error;
use crate::validation::valid_nonempty_text;

/// One ordered position in a time series.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimePoint {
    label: Box<str>,
    duration: Option<Duration>,
}

impl TimePoint {
    pub fn new(label: impl Into<String>, duration: Option<Duration>) -> Result<Self, Error> {
        let label = label.into();
        if !valid_nonempty_text(&label) {
            return Err(Error::new(
                &crate::codes::VALIDATE_TIME_POINT_INVALID_LABEL,
                "a time point label must be nonempty and bounded",
            ));
        }
        Ok(Self {
            label: label.into_boxed_str(),
            duration,
        })
    }

    /// Construct an exact stored duration, rejecting a normalized nanosecond
    /// value instead of silently carrying into the seconds field.
    pub fn from_duration_parts(
        label: impl Into<String>,
        seconds: u64,
        nanoseconds: u32,
    ) -> Result<Self, Error> {
        if nanoseconds >= 1_000_000_000 {
            return Err(Error::new(
                &crate::codes::VALIDATE_TIME_POINT_INVALID_DURATION,
                format!("duration nanosecond remainder {nanoseconds} is at least one billion"),
            ));
        }
        Self::new(label, Some(Duration::new(seconds, nanoseconds)))
    }

    #[must_use]
    pub fn label(&self) -> &str {
        &self.label
    }

    #[must_use]
    pub const fn duration(&self) -> Option<Duration> {
        self.duration
    }
}

/// Ordered complete values of one Rust type.
pub struct TimeSeries<T> {
    time_points: Arc<[TimePoint]>,
    values: Arc<[T]>,
}

impl<T> TimeSeries<T> {
    pub fn new(time_points: Vec<TimePoint>, values: Vec<T>) -> Result<Self, Error> {
        if time_points.len() != values.len() {
            return Err(Error::new(
                &crate::codes::VALIDATE_TIME_SERIES_SHAPE,
                format!(
                    "time series has {} values for {} time points",
                    values.len(),
                    time_points.len()
                ),
            ));
        }
        Ok(Self {
            time_points: time_points.into(),
            values: values.into(),
        })
    }

    #[must_use]
    pub fn time_points(&self) -> &[TimePoint] {
        &self.time_points
    }

    #[must_use]
    pub fn values(&self) -> &[T] {
        &self.values
    }

    #[must_use]
    pub fn get(&self, index: usize) -> Option<(&TimePoint, &T)> {
        Some((self.time_points.get(index)?, self.values.get(index)?))
    }

    #[must_use]
    pub fn time_point(&self, index: usize) -> Option<&TimePoint> {
        self.time_points.get(index)
    }

    #[must_use]
    pub fn value(&self, index: usize) -> Option<&T> {
        self.values.get(index)
    }

    pub fn iter(&self) -> impl ExactSizeIterator<Item = (&TimePoint, &T)> {
        self.time_points.iter().zip(self.values.iter())
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.values.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
}

impl<T> Clone for TimeSeries<T> {
    fn clone(&self) -> Self {
        Self {
            time_points: Arc::clone(&self.time_points),
            values: Arc::clone(&self.values),
        }
    }
}

impl<T: fmt::Debug> fmt::Debug for TimeSeries<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TimeSeries")
            .field("time_points", &self.time_points)
            .field("values", &self.values)
            .finish()
    }
}

/// Checked flattened dimension used by type specific column builders.
#[doc(hidden)]
pub fn checked_dimension_product(what: &str, rows: usize, columns: usize) -> Result<usize, Error> {
    rows.checked_mul(columns).ok_or_else(|| {
        Error::new(
            &crate::codes::VALIDATE_TIME_SERIES_DIMENSION_OVERFLOW,
            format!("{what} dimensions {rows} by {columns} overflow usize"),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_and_exact_duration_parts_are_checked() {
        assert!(TimePoint::new("", None).is_err());
        assert!(TimePoint::new("x".repeat(65_537), None).is_err());
        assert!(TimePoint::from_duration_parts("t0", u64::MAX, 999_999_999).is_ok());
        assert!(TimePoint::from_duration_parts("t0", 0, 1_000_000_000).is_err());
    }

    #[test]
    fn shape_and_dimension_overflow_are_errors() {
        let point = TimePoint::new("t0", None).unwrap();
        assert!(TimeSeries::<u8>::new(vec![point], Vec::new()).is_err());
        assert!(checked_dimension_product("column", usize::MAX, 2).is_err());
    }

    #[test]
    fn lookup_and_iteration_preserve_value_identity() {
        let points = vec![
            TimePoint::new("t0", Some(Duration::from_secs(1))).unwrap(),
            TimePoint::new("t1", Some(Duration::from_secs(2))).unwrap(),
        ];
        let values = vec![String::from("a"), String::from("b")];
        let series = TimeSeries::new(points, values).unwrap();
        let value_pointer = series.value(1).unwrap().as_ptr();
        assert_eq!(series.get(1).unwrap().0.label(), "t1");
        assert_eq!(series.iter().count(), 2);
        assert_eq!(series.value(1).unwrap().as_ptr(), value_pointer);
    }
}
