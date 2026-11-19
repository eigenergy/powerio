//! Interpret 0.11's zero ratings at the document version boundary.

use super::dto::StoredValue;
use crate::BalancedNetwork;

fn rating(value: &mut f64) {
    if *value == 0.0 {
        *value = f64::INFINITY;
    }
}

fn network(network: &mut BalancedNetwork) {
    for branch in network.branches_mut() {
        for value in [&mut branch.rate_a, &mut branch.rate_b, &mut branch.rate_c] {
            rating(value);
        }
        for set in &mut branch.rating_sets {
            rating(&mut set.rate_mva);
        }
    }
    for transformer in network.transformers_3w_mut() {
        for winding in &mut transformer.windings {
            for value in [
                &mut winding.rate_a,
                &mut winding.rate_b,
                &mut winding.rate_c,
            ] {
                rating(value);
            }
        }
        for index in 0..3 {
            let mut sets = transformer.winding_rating_sets(index);
            if sets.iter().any(|set| set.rate_mva == 0.0) {
                for set in &mut sets {
                    rating(&mut set.rate_mva);
                }
                transformer.set_winding_rating_sets(index, sets);
            }
        }
    }
}

pub(super) fn read_ratings(value: &mut StoredValue) {
    match value {
        StoredValue::BalancedNetwork(n) => network(n),
        StoredValue::BalancedOperatingPoint(p) => network(&mut p.network),
        StoredValue::BalancedNetworkTimeSeries(s) => {
            for n in &mut s.values {
                network(n);
            }
        }
        StoredValue::BalancedOperatingPointTimeSeries(s) => {
            if let Some(n) = &mut s.network {
                network(n);
            }
        }
        StoredValue::BalancedNetworkScenarioSet(s) => {
            for entry in &mut s.scenarios {
                network(&mut entry.value);
            }
        }
        StoredValue::BalancedOperatingPointScenarioSet(s) => {
            if let Some(n) = &mut s.network {
                network(n);
            }
        }
        StoredValue::BalancedNetworkTimeSeriesScenarioSet(s) => {
            for entry in &mut s.scenarios {
                for n in &mut entry.value.values {
                    network(n);
                }
            }
        }
        StoredValue::BalancedOperatingPointTimeSeriesScenarioSet(s) => {
            for entry in &mut s.scenarios {
                if let Some(n) = &mut entry.value.network {
                    network(n);
                }
            }
        }
        StoredValue::DcPfInstance(i) => network(&mut i.network),
        StoredValue::AcPfInstance(i) => network(&mut i.network),
        StoredValue::DcOpfInstance(i) => network(&mut i.network),
        StoredValue::AcOpfInstance(i) => network(&mut i.network),
        StoredValue::AcOtsInstance(i) => network(&mut i.base.network),
        StoredValue::AcScucInstance(i) => network(&mut i.network),
        StoredValue::DcPfSolution(s) => network(&mut s.instance.network),
        StoredValue::AcPfSolution(s) => network(&mut s.instance.network),
        StoredValue::DcOpfSolution(s) => network(&mut s.instance.network),
        StoredValue::AcOpfSolution(s) => network(&mut s.instance.network),
        StoredValue::AcOtsSolution(s) => {
            network(&mut s.instance.base.network);
            network(&mut s.ac_solution.instance.network);
        }
        StoredValue::SocwrOpfSolution(s) => network(&mut s.instance.network),
        StoredValue::AcScucSolution(s) => network(&mut s.instance.network),
        StoredValue::MulticonductorNetwork(_)
        | StoredValue::GeoLayer(_)
        | StoredValue::ContingencySet(_)
        | StoredValue::SubsystemSet(_)
        | StoredValue::MonitoredSet(_)
        | StoredValue::MulticonductorOperatingPoint(_)
        | StoredValue::MulticonductorNetworkTimeSeries(_)
        | StoredValue::MulticonductorOperatingPointTimeSeries(_)
        | StoredValue::MulticonductorNetworkScenarioSet(_)
        | StoredValue::MulticonductorOperatingPointScenarioSet(_)
        | StoredValue::MulticonductorNetworkTimeSeriesScenarioSet(_)
        | StoredValue::MulticonductorOperatingPointTimeSeriesScenarioSet(_)
        | StoredValue::McAcPfInstance(_)
        | StoredValue::McAcOpfInstance(_)
        | StoredValue::LinDist3FlowPfInstance(_)
        | StoredValue::LinDist3FlowOpfInstance(_)
        | StoredValue::McAcPfSolution(_)
        | StoredValue::McAcOpfSolution(_)
        | StoredValue::LinDist3FlowPfSolution(_)
        | StoredValue::LinDist3FlowOpfSolution(_) => {}
    }
}
