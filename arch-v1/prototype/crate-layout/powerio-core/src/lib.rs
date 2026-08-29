#![forbid(unsafe_code)]

use std::sync::Arc;
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct Source {
    name: String,
    bytes: Arc<[u8]>,
}

impl Source {
    pub fn from_bytes(name: impl Into<String>, bytes: impl Into<Arc<[u8]>>) -> Self {
        Self {
            name: name.into(),
            bytes: bytes.into(),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

#[derive(Debug, Default)]
struct ModuleRecords {
    diagnostics: Vec<String>,
    source: Option<Source>,
}

#[derive(Debug)]
pub struct PioModule<T> {
    value: T,
    records: ModuleRecords,
}

impl<T> PioModule<T> {
    pub fn new(value: T) -> Self {
        Self {
            value,
            records: ModuleRecords::default(),
        }
    }

    pub fn value(&self) -> &T {
        &self.value
    }

    pub fn into_value(self) -> T {
        self.value
    }

    pub fn diagnostics(&self) -> &[String] {
        &self.records.diagnostics
    }

    pub fn source(&self) -> Option<&Source> {
        self.records.source.as_ref()
    }

    pub fn with_diagnostic(mut self, diagnostic: impl Into<String>) -> Self {
        self.records.diagnostics.push(diagnostic.into());
        self
    }

    pub fn with_source(mut self, source: Source) -> Self {
        self.records.source = Some(source);
        self
    }

    /// Changes only the typed value and moves the common records unchanged.
    pub fn map_value<U>(self, map: impl FnOnce(T) -> U) -> PioModule<U> {
        PioModule {
            value: map(self.value),
            records: self.records,
        }
    }

    /// Internal cross-crate support for a recoverable value conversion.
    #[doc(hidden)]
    pub fn __try_map_value<U>(
        self,
        map: impl FnOnce(T) -> Result<U, T>,
    ) -> Result<PioModule<U>, PioModule<T>> {
        let Self { value, records } = self;
        match map(value) {
            Ok(value) => Ok(PioModule { value, records }),
            Err(value) => Err(PioModule { value, records }),
        }
    }
}

#[derive(Clone, Debug)]
pub struct TimePoint {
    pub label: String,
    pub duration: Option<Duration>,
}

#[derive(Clone, Debug)]
pub struct TimeSeries<T> {
    time_points: Arc<[TimePoint]>,
    values: Arc<[T]>,
}

impl<T> TimeSeries<T> {
    pub fn new(time_points: Vec<TimePoint>, values: Vec<T>) -> Result<Self, &'static str> {
        if time_points.len() != values.len() {
            return Err("time point and value lengths differ");
        }
        if time_points.iter().any(|point| point.label.is_empty()) {
            return Err("time point label cannot be empty");
        }
        Ok(Self {
            time_points: time_points.into(),
            values: values.into(),
        })
    }

    pub fn get(&self, index: usize) -> Option<(&TimePoint, &T)> {
        Some((self.time_points.get(index)?, self.values.get(index)?))
    }

    pub fn time_point(&self, index: usize) -> Option<&TimePoint> {
        self.time_points.get(index)
    }

    pub fn value(&self, index: usize) -> Option<&T> {
        self.values.get(index)
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::{TimePoint, TimeSeries};

    #[test]
    fn time_point_labels_are_nonempty() {
        let result = TimeSeries::new(
            vec![TimePoint {
                label: String::new(),
                duration: None,
            }],
            vec![()],
        );
        assert_eq!(result.unwrap_err(), "time point label cannot be empty");
    }
}

#[derive(Clone, Debug)]
pub struct ScenarioSet<T> {
    values: Arc<[T]>,
}

impl<T> ScenarioSet<T> {
    pub fn new(values: Vec<T>) -> Self {
        Self {
            values: values.into(),
        }
    }

    pub fn get(&self, index: usize) -> Option<&T> {
        self.values.get(index)
    }
}
