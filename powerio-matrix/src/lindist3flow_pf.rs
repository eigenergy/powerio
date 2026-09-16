//! Fixed-dispatch LinDist3Flow compilation and semantic SI limit evaluation.

use powerio_prob::{LinDist3FlowLimitCheck, LinDist3FlowOpfValues, LinDist3FlowPfInstance};

use crate::{
    LinDist3FlowConicProblem, LinDist3FlowStandardForm, LinDist3FlowStandardFormOptions, Result,
    build_lindist3flow_conic_problem, build_lindist3flow_standard_form_with_options,
    lindist3flow_values_from_primal, lindist3flow_values_from_standard_primal,
};

/// Compile the fixed-dispatch electrical equations. The instance constructor
/// has already removed conductor limits from the enforced constraint set and
/// replaced the objective with zero.
///
/// # Errors
/// As [`build_lindist3flow_conic_problem`].
pub fn build_lindist3flow_pf_conic_problem(
    instance: &LinDist3FlowPfInstance,
) -> Result<LinDist3FlowConicProblem> {
    build_lindist3flow_conic_problem(instance.formulation())
}

/// Compile the fixed-dispatch problem in the default scaled standard form.
///
/// # Errors
/// As [`build_lindist3flow_standard_form_with_options`].
pub fn build_lindist3flow_pf_standard_form(
    instance: &LinDist3FlowPfInstance,
) -> Result<LinDist3FlowStandardForm> {
    build_lindist3flow_pf_standard_form_with_options(
        instance,
        LinDist3FlowStandardFormOptions::default(),
    )
}

/// Compile the fixed-dispatch problem with explicit numerical scaling.
///
/// # Errors
/// As [`build_lindist3flow_standard_form_with_options`].
pub fn build_lindist3flow_pf_standard_form_with_options(
    instance: &LinDist3FlowPfInstance,
    options: LinDist3FlowStandardFormOptions,
) -> Result<LinDist3FlowStandardForm> {
    build_lindist3flow_standard_form_with_options(instance.formulation(), options)
}

/// Decode a canonical SI primal for a fixed-dispatch instance.
///
/// # Errors
/// The primal does not match the canonical variable axis.
pub fn lindist3flow_pf_values_from_primal(
    problem: &LinDist3FlowConicProblem,
    primal: &[f64],
) -> Result<LinDist3FlowOpfValues> {
    lindist3flow_values_from_primal(problem, primal)
}

/// Decode a scaled standard-form primal for a fixed-dispatch instance.
///
/// # Errors
/// The primal does not match the standard-form variable axis.
pub fn lindist3flow_pf_values_from_standard_primal(
    form: &LinDist3FlowStandardForm,
    primal: &[f64],
) -> Result<LinDist3FlowOpfValues> {
    lindist3flow_values_from_standard_primal(form, primal)
}

/// Evaluate every supported line thermal limit at a fixed-dispatch point.
/// Values and limits are returned in SI units: volt amperes and amperes.
/// Current is evaluated independently at both physical line endpoints as
/// `hypot(P,Q) / |V|`. Parallel switch contacts remain separate prepared
/// lines, so crossed ratings cannot be relaxed by aggregation.
///
/// # Errors
/// The values do not match the formulation axes, a rating is invalid, or an
/// endpoint squared voltage is nonpositive or non-finite.
pub fn evaluate_lindist3flow_pf_limits(
    instance: &LinDist3FlowPfInstance,
    values: &LinDist3FlowOpfValues,
) -> Result<Vec<LinDist3FlowLimitCheck>> {
    powerio_prob::evaluate_lindist3flow_pf_limits(instance, values).map_err(crate::Error::Commit)
}
