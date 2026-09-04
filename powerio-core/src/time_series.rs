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
    time_points: Arc<Vec<TimePoint>>,
    values: Arc<Vec<T>>,
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
            time_points: Arc::new(time_points),
            values: Arc::new(values),
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
    pub fn get(&self, index: usize) -> Option<&T> {
        self.values.get(index)
    }

    /// Look up one complete entry when its time metadata is also needed.
    #[must_use]
    pub fn entry(&self, index: usize) -> Option<(&TimePoint, &T)> {
        Some((self.time_points.get(index)?, self.values.get(index)?))
    }

    #[must_use]
    pub fn time_point(&self, index: usize) -> Option<&TimePoint> {
        self.time_points.get(index)
    }

    #[must_use]
    pub fn value(&self, index: usize) -> Option<&T> {
        self.get(index)
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

impl<T: Clone> TimeSeries<T> {
    /// Mutably borrow all values through copy on write. Time metadata remains
    /// shared because editing a value never changes its position.
    pub fn values_mut(&mut self) -> &mut [T] {
        Arc::make_mut(&mut self.values).as_mut_slice()
    }

    /// Mutably borrow one value through copy on write.
    pub fn get_mut(&mut self, index: usize) -> Option<&mut T> {
        Arc::make_mut(&mut self.values).get_mut(index)
    }

    /// Mutably borrow one value together with its immutable time metadata.
    pub fn entry_mut(&mut self, index: usize) -> Option<(&TimePoint, &mut T)> {
        let time_point = self.time_points.get(index)?;
        let value = Arc::make_mut(&mut self.values).get_mut(index)?;
        Some((time_point, value))
    }

    /// Iterate over mutable values and immutable time metadata through one
    /// copy on write detachment.
    pub fn iter_mut(&mut self) -> impl ExactSizeIterator<Item = (&TimePoint, &mut T)> {
        self.time_points
            .iter()
            .zip(Arc::make_mut(&mut self.values).iter_mut())
    }

    /// Consume the series and map each complete value while retaining the
    /// time axis. A uniquely owned series moves its values into `map` without
    /// cloning them; a shared series first performs the copy on write split.
    #[must_use]
    pub fn map_values<U>(self, map: impl FnMut(T) -> U) -> TimeSeries<U> {
        let values = Arc::unwrap_or_clone(self.values)
            .into_iter()
            .map(map)
            .collect();
        TimeSeries {
            time_points: self.time_points,
            values: Arc::new(values),
        }
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
        assert_eq!(series.entry(1).unwrap().0.label(), "t1");
        assert_eq!(series.iter().count(), 2);
        assert_eq!(series.value(1).unwrap().as_ptr(), value_pointer);
    }

    #[test]
    fn mutable_access_is_copy_on_write_and_keeps_time_metadata() {
        let points = vec![
            TimePoint::new("t0", None).unwrap(),
            TimePoint::new("t1", None).unwrap(),
        ];
        let mut edited = TimeSeries::new(points, vec![1_u8, 2]).unwrap();
        let original = edited.clone();
        assert!(Arc::ptr_eq(&edited.time_points, &original.time_points));
        assert!(Arc::ptr_eq(&edited.values, &original.values));

        *edited.get_mut(1).unwrap() = 9;
        assert_eq!(edited.get(1), Some(&9));
        assert_eq!(original.get(1), Some(&2));
        assert!(Arc::ptr_eq(&edited.time_points, &original.time_points));
        assert!(!Arc::ptr_eq(&edited.values, &original.values));

        let (point, value) = edited.entry_mut(0).unwrap();
        assert_eq!(point.label(), "t0");
        *value = 7;
        assert_eq!(edited.iter_mut().map(|(_, value)| *value).sum::<u8>(), 16);
    }

    #[test]
    fn consuming_map_moves_unique_values_and_retains_the_axis() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        static CLONES: AtomicUsize = AtomicUsize::new(0);

        #[derive(Debug)]
        struct Counted(u8);

        impl Clone for Counted {
            fn clone(&self) -> Self {
                CLONES.fetch_add(1, Ordering::Relaxed);
                Self(self.0)
            }
        }

        CLONES.store(0, Ordering::Relaxed);
        let series =
            TimeSeries::new(vec![TimePoint::new("t0", None).unwrap()], vec![Counted(3)]).unwrap();
        let mapped = series.map_values(|value| usize::from(value.0));
        assert_eq!(mapped.get(0), Some(&3));
        assert_eq!(mapped.entry(0).unwrap().0.label(), "t0");
        assert_eq!(CLONES.load(Ordering::Relaxed), 0);
    }
}
