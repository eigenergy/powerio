//! The AC optimal transmission switching solution.
//!
//! An [`AcOtsSolution`] pairs a final branch status vector with the AC
//! optimal power flow solution of the problem
//! [`AcOtsInstance::fix_topology`] derives from it. Construction checks that
//! the topology is valid for the instance, that the AC solution solves
//! exactly that fixed problem, and that open branches carry no flow. Finiteness
//! is [`AcOpfSolution`]'s own rule: values it accepts, such as the NaN rows of
//! an out-of-service generator or a non-converged result, are accepted here.
//! It does not check power flow residuals or optimality.

use std::sync::Arc;

use powerio_core::Error;
use powerio_tx::BalancedNetwork;

use crate::diagnostics::codes;
use crate::instance::{AcOpfInstance, AcOtsInstance, ObjectiveTerm};
use crate::solution::{AcOpfSolution, Residuals, Termination};

/// The AC optimal transmission switching result: the final branch statuses,
/// their switching cost, and the AC optimal power flow solution over that
/// topology.
#[derive(Clone, Debug)]
pub struct AcOtsSolution {
    instance: Arc<AcOtsInstance>,
    branch_closed: Vec<bool>,
    ac_solution: AcOpfSolution,
    switching_cost: f64,
}

impl AcOtsSolution {
    /// Attach the AC optimal power flow solution computed on
    /// `instance.fix_topology(&branch_closed)`.
    ///
    /// # Errors
    /// The topology fails [`AcOtsInstance::validate_topology`], the AC
    /// solution answers a different problem than the fixed topology states
    /// (its network, objective, or constraint selections differ), or an open
    /// branch carries flow. A NaN flow on an open branch means no flow was
    /// recorded and is accepted; a NaN anywhere else is left to
    /// [`AcOpfSolution`].
    pub fn new(
        instance: Arc<AcOtsInstance>,
        branch_closed: Vec<bool>,
        ac_solution: AcOpfSolution,
    ) -> Result<Self, Error> {
        let switching_cost = instance.validate_topology(&branch_closed)?;
        let (fixed, _) = instance.fix_topology(&branch_closed)?;
        require_same_problem(&fixed, ac_solution.instance())?;
        require_open_branches_idle(&instance, &branch_closed, &ac_solution)?;
        Ok(Self {
            instance,
            branch_closed,
            ac_solution,
            switching_cost,
        })
    }

    /// The switching instance this solution solves. Borrowed; never a copy.
    #[must_use]
    pub fn instance(&self) -> &AcOtsInstance {
        &self.instance
    }

    /// The shared instance owner.
    #[must_use]
    pub fn shared_instance(&self) -> Arc<AcOtsInstance> {
        Arc::clone(&self.instance)
    }

    /// The instance's network: every candidate branch at its initial
    /// status. The final statuses are [`branch_closed`](Self::branch_closed);
    /// the AC solution's instance holds the network with them applied.
    #[must_use]
    pub fn network(&self) -> &BalancedNetwork {
        self.instance.network()
    }

    /// The final branch statuses, in branch table order.
    #[must_use]
    pub fn branch_closed(&self) -> &[bool] {
        &self.branch_closed
    }

    /// The AC optimal power flow solution over the final topology.
    #[must_use]
    pub const fn ac_solution(&self) -> &AcOpfSolution {
        &self.ac_solution
    }

    /// The cost of the status changes against the initial topology.
    #[must_use]
    pub const fn switching_cost(&self) -> f64 {
        self.switching_cost
    }

    /// The number of branches open in the final topology.
    #[must_use]
    pub fn branches_open(&self) -> usize {
        self.branch_closed.iter().filter(|closed| !**closed).count()
    }

    /// The number of branches whose final status differs from the initial
    /// one.
    #[must_use]
    pub fn branches_switched(&self) -> usize {
        self.instance
            .branches()
            .iter()
            .zip(&self.branch_closed)
            .filter(|(branch, closed)| branch.initial_closed != **closed)
            .count()
    }

    /// The objective value: the AC solution's objective, plus the switching
    /// cost when the instance's objective states the switching cost term.
    #[must_use]
    pub fn objective(&self) -> f64 {
        let switching = if self
            .instance
            .objective()
            .contains(&ObjectiveTerm::BranchSwitchingCost)
        {
            self.switching_cost
        } else {
            0.0
        };
        self.ac_solution.objective() + switching
    }

    /// How the producing calculation ended, as the AC solution records it.
    #[must_use]
    pub fn termination(&self) -> &Termination {
        self.ac_solution.termination()
    }

    /// The AC solution's numerical residuals.
    #[must_use]
    pub fn residuals(&self) -> &Residuals {
        self.ac_solution.residuals()
    }

    /// The producer or solver identity the AC solution records.
    #[must_use]
    pub fn producer(&self) -> Option<&str> {
        self.ac_solution.producer()
    }
}

fn mismatch(message: String) -> Error {
    Error::new(&codes::BUILD_SOLUTION_TOPOLOGY_MISMATCH, message)
}

/// The two AC problems agree on the network, the objective, and the
/// constraint selections. The initial point is a solver hint, not part of
/// the problem, so it is not compared. Networks compare by their stored
/// form, which a PowerIO IR round trip preserves.
fn require_same_problem(fixed: &AcOpfInstance, solved: &AcOpfInstance) -> Result<(), Error> {
    if fixed.objective() != solved.objective() {
        return Err(mismatch(
            "the AC solution's objective differs from the fixed-topology problem's".to_owned(),
        ));
    }
    if fixed.constraints() != solved.constraints() {
        return Err(mismatch(
            "the AC solution's constraint selections differ from the fixed-topology problem's"
                .to_owned(),
        ));
    }
    let stored = |network: &BalancedNetwork| {
        serde_json::to_value(network)
            .map_err(|error| mismatch(format!("a network could not be compared: {error}")))
    };
    if stored(fixed.network())? != stored(solved.network())? {
        let detail = match fixed
            .network()
            .branches()
            .iter()
            .zip(solved.network().branches())
            .position(|(a, b)| a.in_service != b.in_service)
        {
            Some(row) => format!(", first at branch row {row}'s status"),
            None => String::new(),
        };
        return Err(mismatch(format!(
            "the AC solution's network differs from the fixed-topology network{detail}"
        )));
    }
    Ok(())
}

#[allow(clippy::float_cmp)] // an open branch carries exactly zero flow
fn require_open_branches_idle(
    instance: &AcOtsInstance,
    branch_closed: &[bool],
    solution: &AcOpfSolution,
) -> Result<(), Error> {
    let flows = [
        solution.branch_from_active_flows(),
        solution.branch_from_reactive_flows(),
        solution.branch_to_active_flows(),
        solution.branch_to_reactive_flows(),
    ];
    for (row, (branch, &closed)) in instance.branches().iter().zip(branch_closed).enumerate() {
        // A NaN compares unequal to zero, but records no flow.
        if !closed
            && flows
                .iter()
                .any(|column| !column[row].is_nan() && column[row] != 0.0)
        {
            return Err(Error::new(
                &codes::BUILD_SOLUTION_VALUE_INVALID,
                format!(
                    "branch `{}` is open in the final topology but carries flow",
                    branch.id
                ),
            ));
        }
    }
    Ok(())
}
