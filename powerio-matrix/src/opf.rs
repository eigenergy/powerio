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

fn find(parent: &mut [usize], mut node: usize) -> usize {
    while parent[node] != node {
        parent[node] = parent[parent[node]];
        node = parent[node];
    }
    node
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

    let mut parent: Vec<usize> = (0..analysis_rows.len()).collect();
    for (_, branch) in case.in_service_branches() {
        let Some(from_analysis) = case.bus_index(branch.from) else {
            continue;
        };
        let Some(to_analysis) = case.bus_index(branch.to) else {
            continue;
        };
        let (Some(from), Some(to)) = (
            dense_by_analysis[from_analysis],
            dense_by_analysis[to_analysis],
        ) else {
            continue;
        };
        let from_root = find(&mut parent, from);
        let to_root = find(&mut parent, to);
        if from_root != to_root {
            parent[to_root] = from_root;
        }
    }

    let mut grounded = vec![false; parent.len()];
    for &reference in &reference_rows {
        let root = find(&mut parent, reference);
        grounded[root] = true;
    }
    let mut roots = std::collections::HashSet::with_capacity(parent.len());
    for bus in 0..parent.len() {
        roots.insert(find(&mut parent, bus));
    }
    let ungrounded = roots.iter().filter(|&&root| !grounded[root]).count();
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
    if let ConstraintSelection::Only(selected) = selection {
        for identity in selected {
            if !declared.contains(identity.as_str()) {
                return Err(Error::UnknownConstraintIdentity {
                    family,
                    identity: identity.clone(),
                });
            }
        }
    }
    Ok(active_identities
        .iter()
        .map(|identity| selection.selects(identity))
        .collect())
}
