# Entry points

Where two names reach the same result, which one is canonical and what the
other is for.

## The default-and-options pair

Most operations come as a short name with the settled defaults and a longer
name taking an options struct. The short name calls the long one; neither
duplicates the work.

| Default | Options | Options struct |
| --- | --- | --- |
| `parse` | `parse_with_options` | `ParseOptions` |
| `DcOperators::build` | `DcOperators::build_with` | `DcOperatorOptions` |
| `calc_ptdf_lodf` | `calc_ptdf_lodf_with_options` | `SensitivityOptions` |
| `build_lindist3flow_standard_form` | `build_lindist3flow_standard_form_with_options` | `LinDist3FlowStandardFormOptions` |

`calc_ptdf` and `calc_lodf` are the same operation asking for one of the two
matrices; they run the shared `build_parts` with `Want::Ptdf` or `Want::Lodf`
so a caller that needs only one does not pay for both. A caller that needs both
calls `calc_ptdf_lodf`, which factors once.

## Building an instance

`DcOpfInstance::from_network` states the whole instance: every stated limit
active, the network's cost curves as the objective when any generator carries
one. The `with_*` methods each replace one part of an instance that already
exists, consuming it; they are not an alternative way to build one.

`with_network` replaces the network while keeping the objective, the constraint
selections, the susceptance formula, and a compatible initial point. It is the
checked path for a parameter edit such as a rating change: rebuilding from
scratch drops those semantics silently.

## Deriving the numerical arrays

`build_dc_opf_preparation` is the one numerical assembly. Everything else reads
it:

- `calc_dc_opf_matrices` calls it and projects the arrays into sparse matrices.
- `emit_dcopf_bundle` calls it and writes the files.
- `DcOpfPreparation::calc_nodal_generator_data` projects its generator columns
  into bus space.

A consumer that wants the contiguous arrays calls `build_dc_opf_preparation`
directly; one that wants the sparse operators calls `calc_dc_opf_matrices`.
`AcOpfPreparation` and `AcPfPreparation` follow the same shape through
`build_ac_opf_preparation` and `build_ac_pf_preparation`.

## `IndexedNetwork` against `DcOperators`

These answer different questions and neither replaces the other.

`IndexedNetwork::new` is a dense index view over a borrowed network: it maps
source bus ids into `[0, n)` and iterates the in service elements. It computes
no electrical quantity and allocates only the index.

`DcOperators::build` is a reusable DC model: the incidence matrix, the branch
susceptances, the phase shift injections, and the axis identities behind them,
built once and updated in place by `DcOperators::update` when only parameters
change. A contingency screen that rebuilds the same topology hundreds of times
holds one `DcOperators`; a caller that needs one matrix once takes the
`IndexedNetwork` and calls the matrix builder.

## The one DC branch rule

Three builders fill branch columns: `matrix::incidence::build_incidence`
(behind the sparse matrix and sensitivity paths), `DcOperators::build_with`,
and `preparation_from_view` (behind the DC OPF preparation). They carry
different columns, so they stay separate loops, but they must agree on which
branches are carried and what each branch's susceptance is. Both decisions have
one owner:

- `BranchSusceptanceFormula::calc_branch_susceptance` and
  `calc_solver_edge_weight` state the susceptance. No builder rolls its own.
- `BranchSusceptanceFormula::impedance_is_degenerate` states which branches are
  too small to divide by. The bound reads only the denominator the selected
  formula divides by: `hypot(r, x)` for `SeriesSusceptance`, `|x|` for the
  reciprocal rules.

Before 0.11.3 the preparation bounded `|x|` under every formula, so a purely
resistive branch (`x = 0`, `r > 0`) under the default `SeriesSusceptance` built
a `DcOperators` and a PTDF but not a DC OPF preparation. It has
`b = -x/(r² + x²) = 0`, which is a reading of the DC model, not a failure: the
branch carries no angle-driven flow. Every builder now reads it that way.

## Nothing is deprecated here

No pair in this document computes the same thing twice, so no name is retired.
The duplication that remains is the three branch column loops above, which
share their two decisions and differ only in what each carries alongside.
