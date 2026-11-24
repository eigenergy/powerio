//! Shared OPF preparation semantics.

use std::collections::HashSet;

use powerio_prob::{ConstraintSelection, Objective, ObjectiveTerm, ReferenceBuses};
use powerio_tx::{BalancedNetwork, BusId, BusType, IndexedNetwork};

use crate::{Error, Result};

/// The objective compiled into a balanced OPF preparation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum PreparedObjective {
    /// A feasibility problem with an identically zero objective.
    Feasibility,
    /// The sum of the in service generators' network cost curves.
    #[default]
    NetworkGeneratorCost,
}

/// One convex piecewise linear generator cost in preparation units.
///
/// `power[i]` and `value[i]` are the supplied breakpoint coordinates after
/// scaling the power coordinate into the preparation's [`Units`](crate::Units).
/// Objective values do not scale with the power unit. The preparation builder
/// validates that both columns have the same length, power is strictly
/// increasing, and adjacent segment slopes are nondecreasing.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub struct PiecewiseLinearCost {
    /// Breakpoint powers in the preparation's selected power unit.
    pub power: Vec<f64>,
    /// Objective values at the corresponding breakpoints.
    pub value: Vec<f64>,
}

/// The source component represented by one branch row in a lowered analysis
/// network.
///
/// Ordinary branches map to their row in `BalancedNetwork::branches()`. Each
/// in service three winding transformer contributes three analysis branches,
/// one for each winding in the transformer's declared terminal order. Those
/// rows stay distinct from the source branch table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
#[non_exhaustive]
pub enum AnalysisBranchSource {
    Branch {
        row: usize,
    },
    ThreeWindingTransformerWinding {
        transformer_row: usize,
        /// Winding position in `0..3`.
        winding: usize,
    },
}

pub(crate) fn analysis_branch_sources(source: &BalancedNetwork) -> Vec<AnalysisBranchSource> {
    let mut sources = (0..source.branches().len())
        .map(|row| AnalysisBranchSource::Branch { row })
        .collect::<Vec<_>>();
    for (transformer_row, transformer) in source.transformers_3w().iter().enumerate() {
        if !transformer.in_service {
            continue;
        }
        sources.extend((0..3).map(|winding| {
            AnalysisBranchSource::ThreeWindingTransformerWinding {
                transformer_row,
                winding,
            }
        }));
    }
    sources
}

pub(crate) fn compile_objective(objective: &Objective) -> Result<PreparedObjective> {
    match objective.terms() {
        [] => Ok(PreparedObjective::Feasibility),
        [ObjectiveTerm::NetworkGeneratorCost] => Ok(PreparedObjective::NetworkGeneratorCost),
        [ObjectiveTerm::ActivePowerDispatchCost] => Err(Error::UnsupportedOpfObjective {
            reason: "`active_power_dispatch_cost` belongs to multiconductor OPF".to_owned(),
        }),
        _ => Err(Error::UnsupportedOpfObjective {
            reason: "balanced OPF preparation supports either an empty objective or exactly one `network_generator_cost` term".to_owned(),
        }),
    }
}

pub(crate) fn row_identity(uid: Option<&str>, table: &str, row: usize) -> String {
    uid.map_or_else(|| format!("{table}:{row}"), str::to_owned)
}

/// Dense bus rows used by a balanced OPF preparation. A bus explicitly typed
/// isolated states no equation, so it and every incident element stay out of
/// the numerical problem while its source row remains in the PowerIO model.
pub(crate) struct ActiveBusIndex {
    pub analysis_rows: Vec<usize>,
    pub dense_by_analysis: Vec<Option<usize>>,
    pub bus_ids: Vec<BusId>,
    pub reference_buses: ReferenceBuses,
}

/// Select non-isolated buses and check reference coverage on the topology that
/// the OPF preparation will actually contain. In-service branches touching an
/// isolated bus are excluded with that bus, matching PowerIO normalization.
pub(crate) fn active_bus_index(case: &IndexedNetwork<'_>) -> Result<ActiveBusIndex> {
    let mut analysis_rows = Vec::new();
    let mut dense_by_analysis = vec![None; case.n()];
    let mut bus_ids = Vec::new();
    let mut reference_rows = Vec::new();
    for (analysis_row, bus) in case.network().buses().iter().enumerate() {
        if bus.kind == BusType::Isolated {
            continue;
        }
        let dense = analysis_rows.len();
        analysis_rows.push(analysis_row);
        dense_by_analysis[analysis_row] = Some(dense);
        bus_ids.push(bus.id);
        if bus.kind == BusType::Ref {
            reference_rows.push(dense);
        }
    }

    // The islands of the dense buses over the in-service branches between
    // them, by the one island labeling in powerio-tx.
    let (from_bus, to_bus): (Vec<usize>, Vec<usize>) = case
        .in_service_branches()
        .filter_map(|(_, branch)| {
            let from = dense_by_analysis[case.bus_index(branch.from)?]?;
            let to = dense_by_analysis[case.bus_index(branch.to)?]?;
            Some((from, to))
        })
        .unzip();
    let labels = powerio_tx::calc_dense_island_labels(analysis_rows.len(), &from_bus, &to_bus);
    let islands = labels.iter().max().map_or(0, |&last| last + 1);
    let mut grounded = vec![false; islands];
    for &reference in &reference_rows {
        grounded[labels[reference]] = true;
    }
    let ungrounded = grounded.iter().filter(|&&grounded| !grounded).count();
    if ungrounded > 0 {
        return Err(powerio_tx::Error::UngroundedComponent {
            components: ungrounded,
        }
        .into());
    }

    Ok(ActiveBusIndex {
        analysis_rows,
        dense_by_analysis,
        bus_ids,
        reference_buses: ReferenceBuses::new(reference_rows),
    })
}

/// Validate a selection against the complete family and return one flag per
/// active analysis row.
pub(crate) fn constraint_mask(
    family: &'static str,
    selection: &ConstraintSelection,
    all_identities: &[String],
    active_identities: &[String],
) -> Result<Vec<bool>> {
    let mut declared = HashSet::with_capacity(all_identities.len());
    for identity in all_identities {
        if !declared.insert(identity.as_str()) {
            return Err(Error::DuplicateElementIdentity {
                family,
                identity: identity.clone(),
            });
        }
    }
    let ConstraintSelection::Only(selected) = selection else {
        return Ok(active_identities
            .iter()
            .map(|identity| selection.selects(identity))
            .collect());
    };
    for identity in selected {
        if !declared.contains(identity.as_str()) {
            return Err(Error::UnknownConstraintIdentity {
                family,
                identity: identity.clone(),
            });
        }
    }
    // `ConstraintSelection::selects` scans the list per identity; a set
    // gives the same membership in time linear in the element count.
    let selected: HashSet<&str> = selected.iter().map(String::as_str).collect();
    Ok(active_identities
        .iter()
        .map(|identity| selected.contains(identity.as_str()))
        .collect())
}

// Keep prepared numeric bounds round-trippable without changing finite JSON
// numbers or the binary representation. Use the shared PowerIO spelling.
pub(crate) mod nonfinite_vec {
    use powerio_core::__implementation::nonfinite::{NonFiniteDe, NonFiniteSer};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(values: &[f64], serializer: S) -> Result<S::Ok, S::Error> {
        values.serialize(NonFiniteSer(serializer))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<f64>, D::Error> {
        Vec::<f64>::deserialize(NonFiniteDe(deserializer))
    }
}
