//! The checked, explicit zero impedance resolution.
//!
//! Networks and instances preserve zero impedance branches; a finite matrix
//! or problem projection refuses them rather than silently skipping rows.
//! [`merge_zero_impedance_buses`] is the explicit resolution: buses joined by
//! an in service branch with zero series impedance merge into one electrical
//! node, and the transformation returns the complete mapping plus diagnostics
//! for the branch behavior the merge removes. It is
//! [`BalancedNetwork::merge_buses`] under
//! [`ZeroImpedanceRule::Exact`](powerio_tx::ZeroImpedanceRule::Exact) with
//! closed switches left alone.

use std::collections::BTreeMap;

use powerio_core::{Diagnostic, Error};
use powerio_tx::{BalancedNetwork, BusId, BusMergeRule, RemovalReason, ZeroImpedanceRule};

use crate::diagnostics::codes;

/// What one merge did: which buses now name which surviving bus, and which
/// branches the merge removed.
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct ZeroImpedanceMerge {
    /// Every merged bus to the bus that now carries it. Buses that survived
    /// unchanged are absent.
    pub merged_buses: BTreeMap<BusId, BusId>,
    /// The removed branches, by stable identity (`uid`, else
    /// `branches:{row}` of the source network): the zero impedance branches
    /// and any branch whose two buses the merge joined through them.
    pub removed_branches: Vec<String>,
}

/// Merge every group of buses joined by in service branches with zero series
/// impedance (`r == 0` and `x == 0`, self loops and off-nominal transformers
/// excluded) into one surviving bus, rewriting every element reference and
/// dropping the merged buses and the zero impedance branches.
///
/// The survivor is the group's reference bus, else a bus hosting an
/// in-service generator, else a bus a generator regulates, with the smallest
/// id breaking each tie. A branch whose two buses the merge joined through
/// other branches is removed as well, and a removed branch's line charging
/// becomes a fixed shunt at the survivor.
///
/// The flow through a removed branch is no longer a variable of any derived
/// calculation, and merged buses may have stated different attributes; both
/// are reported as diagnostics, one per removed branch and one per group whose
/// base voltages differ. The input network is never mutated.
/// [`BalancedNetwork::merge_buses`] is the general form, which also merges
/// closed switches, applies a stated threshold, and recovers removed flows.
///
/// # Errors
/// A zero impedance branch naming a bus the network does not declare.
pub fn merge_zero_impedance_buses(
    network: &BalancedNetwork,
) -> Result<(BalancedNetwork, ZeroImpedanceMerge, Vec<Diagnostic>), Error> {
    let rule = BusMergeRule::new(false, Some(ZeroImpedanceRule::Exact));
    let merge = network
        .merge_buses(&rule)
        .map_err(|error| Error::new(error.code(), error.to_string()))?;

    let mut diagnostics = Vec::with_capacity(merge.removed_branches.len());
    for removed in &merge.removed_branches {
        if removed.reason == RemovalReason::ZeroImpedance {
            diagnostics.push(Diagnostic::of(
                &codes::CANONICALIZE_MERGE_ZERO_IMPEDANCE,
                format!(
                    "zero impedance branch `{}` between buses {} and {} was merged; its flow is not a variable of any derived calculation",
                    removed.identity, removed.from, removed.to
                ),
            ));
        }
    }
    // The per branch findings above replace the merge's one summary finding.
    diagnostics.extend(
        merge
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.code() != codes::CANONICALIZE_MERGE_ZERO_IMPEDANCE.code)
            .cloned(),
    );
    let removed_branches = merge
        .removed_branches
        .iter()
        .map(|removed| removed.identity.clone())
        .collect();
    Ok((
        merge.network,
        ZeroImpedanceMerge {
            merged_buses: merge.merged_buses,
            removed_branches,
        },
        diagnostics,
    ))
}
