# Test fixtures: provenance and licenses

Everything in this tree is vendored for testing and validation only; none of
it ships in any released artifact.

## MATPOWER cases (`*.m` in this directory)

`case9.m`, `case14.m`, `case30.m`, `case57.m`, `case118.m`,
`case2869pegase.m`, `t_case9_dcline.m`, and `t_case9_oos.m` are vendored
byte exact from the [MATPOWER repository](https://github.com/MATPOWER/matpower)
(the `t_*` cases from its test suite). MATPOWER is BSD 3-Clause,
(c) the Power Systems Engineering Research Center (PSERC) and individual
contributors; each file keeps its original header. The PEGASE case is
fictitious data representing the size and complexity of the European
transmission network (stated in its header).

## `pglib/`

From the [IEEE PES PGLib-OPF benchmark library](https://github.com/power-grid-lib/pglib-opf),
v23.07. MIT license, (c) 2017 IEEE PES Power Grid Benchmarks. Original
headers retained.

## `powerworld/`

ACTIVSg200 exports from the TAMU Electric Grid Test Case Repository;
synthetic, no CEII. See `powerworld/README.md` for the full citation and
retrieval date.

## `opfdataset/`

One solved case-14 example from DeepMind's OPFData `dataset_release_1`
(CC BY 4.0, (c) 2024 DeepMind Technologies Limited). See
`opfdataset/README.md` for the format specification, source archive, and hash.

## `dist/`

Distribution-side fixtures: the IEEE PES test feeders as distributed with
OpenDSS, the IEEE PES Task Force on Benchmarking Multiconductor OPF draft
schema and examples, PowerModelsDistribution renderings, and original micro
cases. Provenance and per-directory licenses in `dist/README.md`.

## `pandapower/` and `pypsa/`

Tool generated fixtures with regeneration scripts and license notes in their
own READMEs (`pandapower/README.md`, `pypsa/README.md`).

## `psse/` and `egret/`

Original to this repository: the PSS/E RAW files are hand written minimal
cases (their title lines say what each exercises), except the two PowSybl
Core revision 32 cases whose source paths, license, and digests are listed in
`psse/README.md`; the egret JSON files
are renderings of the MATPOWER cases above (plus a small dcline case) in
egret's ModelData schema.

Everything else in this tree (`model-json/`, `capi_matrix/`,
`angle_bounds_clamp.m`, and the other unlisted fixtures) is original to this
repository under its code license.

## `large/` (not committed)

Gitignored; `evals/validation/fetch_cases.sh` downloads the large benchmark cases
on demand from the MATPOWER repository and
[goghino/opf_benchmarks](https://github.com/goghino/opf_benchmarks) and
documents their origins.
