# PowerModelsACDC MATPOWER-extension qualification

This harness qualifies the MATPOWER-extension variant of PowerModelsACDC through
`source → PowerIO → PowerIO IR → fresh .m`. The native JSON variant based on
PowerModelsACDC IR/data-dictionary serialization is separate future work.
`Manifest.toml` pins Julia dependencies, including PowerModelsACDC commit
`9ada4082ac087aed649f48fb5f977680b0fe06c7`, JuMP and Ipopt. These are validation
only dependencies; no Julia or solver dependency is added to PowerIO.

## Reproduce

From the repository root, run the vendored case5 and negative controls:

```sh
julia --project=evals/validation/acdc -e 'using Pkg; Pkg.instantiate()'
python3 evals/validation/acdc/run.py /tmp/powerio-acdc-validation
```

For all four cases, supply a directory containing `case24_7_jb.m`,
`case39_10_he.m` and `case67.m` from
[PGLib-HVDC commit f928f95c](https://github.com/power-grid-lib/pglib-opf-hvdc/tree/f928f95cde06d035554d92854f03950e074850e0).
The runner checks all source SHA-256 hashes before starting. Only case5 is vendored.

```sh
python3 evals/validation/acdc/run.py /tmp/powerio-acdc-validation --pglib-dir /path/to/pglib-opf-hvdc
```

Individual cases can also be run directly:

```sh
cargo run -p powerio --example acdc_validation -- tests/data/pglib-hvdc/case5_3_he.m /tmp/powerio-acdc-oracle
julia --project=evals/validation/acdc evals/validation/acdc/compare.jl tests/data/pglib-hvdc/case5_3_he.m /tmp/powerio-acdc-oracle
julia --project=evals/validation/acdc evals/validation/acdc/solve.jl tests/data/pglib-hvdc/case5_3_he.m /tmp/powerio-acdc-oracle pf,opf
```

## What is checked

`compare.jl` compares every AC/DC electrical input, including AC topology,
generators, costs, bounds, taps, statuses, controls and starts, both before and
after PowerModelsACDC preprocessing. All dictionary keys and array sizes must
match. Numeric tolerance is `atol=1e-12`, `rtol=1e-10` in reference coordinates.
The narrow exclusions in `common.jl` are file name/type/version, component source
provenance, and previous solve duals. Every excluded path and value is recorded;
indices, electrical fields and unknown fields are not excluded. Case67 contains
previous `mu_pmax` results, which fresh output does not preserve.

The native DC owner is also checked field by field against the reference parser,
including PCC signs and valve-side limits. PGLib aliases are renamed explicitly
in reference dictionaries. Source declarations are never corrected in place.
`reference.json` saves both source and effective dictionaries: for example, the
reference enlarges some `Imax` values while PowerIO preserves the source limits.

`solve.jl` uses `ACPPowerModel`, `conv_losses_mp=true`, Ipopt/MUMPS, a single BLAS
thread, the same declared/default starts on both sides, solver tolerance `1e-9`,
and no bound relaxation. It checks successful termination and primal feasibility
of both solves, objective agreement (`atol=1e-7`, `rtol=1e-8`), and both solved
points against the other model's constraints and variable bounds (`1e-7`).
PF voltages, dispatch, converter P/Q, DC flows and derived losses must match
(`atol=1e-7`, `rtol=1e-6`, per-unit reference coordinates). OPF differences in all
these quantities are recorded rather than requiring a unique optimal dispatch.

`solve.json` contains versions, manifest/input/output hashes, settings, starts,
solutions, residuals, cross-feasibility results and field-level differences.
`summary.json` collects the case results. These generated files stay outside the
repository. `test_checks.jl` and `test_solve_checks.jl` verify that changed costs,
limits, topology, statuses, unknown electrical fields and infeasible points fail.

The qualified corpus runs PF and OPF for case5 and case24, and OPF for case39 and
case67. Preliminary ACP PF solves of the original case39 and case67 inputs return
`LOCALLY_INFEASIBLE` under these settings; that is not a proof of global
infeasibility and is not counted as a passing PF comparison. The runner does not
silently convert failed solves into passes. See [RESULTS.md](RESULTS.md) for the
recorded run and [FOLLOW_UP.md](FOLLOW_UP.md) for native-model validation work.

## Frozen-reader compatibility

Run independently of the current extension:

```sh
python3 -m venv /tmp/powerio-acdc-legacy
/tmp/powerio-acdc-legacy/bin/pip install powerio==0.11.0
/tmp/powerio-acdc-legacy/bin/python evals/validation/acdc/check_legacy_ir.py
# Repeat in a separate environment with powerio==0.11.9.
```

Eight historical raw/nested network shapes must load. The proof reproduces loss
of an unguarded network field and verifies that `required_capabilities` forces
rejection on each shape. Current Rust tests exercise actual equipment, capability
mismatches, malformed declarations, COW edits, explicit subsets and unsupported
calculation paths.
