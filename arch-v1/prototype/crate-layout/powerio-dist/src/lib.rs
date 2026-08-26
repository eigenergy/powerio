#![forbid(unsafe_code)]

use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct MulticonductorNetwork {
    bus_ids: Arc<[u64]>,
}

impl MulticonductorNetwork {
    pub fn new(bus_ids: Vec<u64>) -> Self {
        Self {
            bus_ids: bus_ids.into(),
        }
    }

    pub fn bus_ids(&self) -> &[u64] {
        &self.bus_ids
    }
}
