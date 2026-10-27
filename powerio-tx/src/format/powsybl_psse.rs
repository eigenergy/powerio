//! The rules PowSybl's psse-converter applies between IIDM and PSS/E, shared by
//! the XIIDM and PSS/E readers and writers so the two directions stay inverse.
//!
//! Each rule cites the powsybl-core `psse-converter` class and method it
//! follows (package `com.powsybl.psse.converter`).

use std::collections::HashMap;

use powerio_core::ComponentId;

use crate::network::{BalancedNetwork, BranchRatingSet, BusId, LoadingLimits};

/// The apparent power of a current limit at a nominal voltage:
/// `√3 · Vnom · I / 1000` MVA for `I` in A and `Vnom` in kV
/// (`AbstractConverter.convertToMva`).
pub(super) fn current_limit_mva(current_a: f64, nominal_kv: f64) -> f64 {
    current_a / 1000.0 * 3f64.sqrt() * nominal_kv
}

/// The MVA ratings of one set of current limits, permanent and temporary,
/// sorted ascending as PowSybl fills `RATE1` to `RATE12`
/// (`AbstractConverter.getSortedRates(CurrentLimits, double)` and
/// `setSortedRatesToPsseRates`). A temporary limit with no value, which IIDM
/// reads as unlimited, states no rating.
pub(super) fn sorted_current_rates(limits: &LoadingLimits, nominal_kv: f64) -> Vec<f64> {
    let mut rates = limits
        .permanent_limit
        .into_iter()
        .chain(limits.temporary_limits.iter().map(|limit| limit.value))
        .filter(|value| value.is_finite() && *value > 0.0 && *value < f64::MAX)
        .map(|value| current_limit_mva(value, nominal_kv))
        .collect::<Vec<_>>();
    rates.sort_by(f64::total_cmp);
    rates
}

/// The MVA ratings PSS/E states for each branch that carries current limits
/// and no MVA rating. PowSybl's exporter takes the selected current limits of
/// side 1 at its nominal voltage, and side 2's only when side 1 has none
/// (`LineConverter.createRates`, `TransformerConverter.createRates`). The
/// selected groups retained from an IIDM source give every temporary limit;
/// otherwise the branch's own current ratings stand in, at the side 1 bus
/// voltage.
///
/// Returns the projected network and the number of branches it rated, or
/// `None` when no branch needs it.
pub(super) fn rate_branches_from_current_limits(
    net: &BalancedNetwork,
) -> Option<(BalancedNetwork, usize)> {
    let detailed = net.detailed_connectivity().as_deref();
    let bus_kv = net
        .buses()
        .iter()
        .map(|bus| (bus.id, bus.base_kv))
        .collect::<HashMap<BusId, f64>>();
    let mut rated = Vec::new();
    for (index, branch) in net.branches().iter().enumerate() {
        if branch.rate_a != 0.0
            || branch.rate_b != 0.0
            || branch.rate_c != 0.0
            || !branch.rating_sets.is_empty()
        {
            continue;
        }
        let side_kv = |side: u8| {
            let bus = if side == 1 { branch.from } else { branch.to };
            bus_kv.get(&bus).copied().unwrap_or(0.0)
        };
        let exact = detailed.and_then(|detailed| {
            let component = ComponentId::new("branch", branch.uid.as_deref()?).ok()?;
            (1..=2).find_map(|side| {
                detailed
                    .operational_limit_groups
                    .iter()
                    .filter(|group| {
                        group.equipment == component && group.terminal == side && group.selected
                    })
                    .find_map(|group| group.current_limits.as_ref())
                    .map(|limits| sorted_current_rates(limits, side_kv(side)))
                    .filter(|rates| !rates.is_empty())
            })
        });
        let rates = exact.or_else(|| {
            let ratings = branch.current_ratings?;
            let limits = LoadingLimits {
                permanent_limit: Some(ratings.c_rating_a),
                permanent_limit_name: None,
                temporary_limits: [ratings.c_rating_b, ratings.c_rating_c]
                    .into_iter()
                    .map(|value| crate::network::TemporaryLimit {
                        name: String::new(),
                        value,
                        acceptable_duration_seconds: 0,
                        fictitious: false,
                    })
                    .collect(),
            };
            Some(sorted_current_rates(&limits, side_kv(1))).filter(|rates| !rates.is_empty())
        });
        if let Some(rates) = rates {
            rated.push((index, rates));
        }
    }
    if rated.is_empty() {
        return None;
    }
    let count = rated.len();
    let mut projected = net.clone();
    let branches = projected.branches_mut();
    for (index, rates) in rated {
        let branch = &mut branches[index];
        let rate = |slot: usize| rates.get(slot).copied().unwrap_or(0.0);
        branch.rate_a = rate(0);
        branch.rate_b = rate(1);
        branch.rate_c = rate(2);
        branch.rating_sets = rates
            .iter()
            .enumerate()
            .skip(3)
            .map(|(slot, value)| BranchRatingSet::new(format!("RATE{}", slot + 1), *value))
            .collect();
        branch.current_ratings = None;
    }
    Some((projected, count))
}
