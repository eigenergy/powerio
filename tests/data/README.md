# Test fixtures: provenance and licenses

Everything in this tree is vendored for testing and validation only; none of
it ships in any released artifact.

## MATPOWER cases (`*.m` in this directory)

`case9.m`, `case14.m`, `case30.m`, `case57.m`, `case118.m`,
`case2869pegase.m`, `t_case9_dcline.m`, and `t_case9_oos.m` are vendored
byte exact from the [MATPOWER repository](https://github.com/MATPOWER/matpower)
(the `t_*` cases from its test suite). MATPOWER's code is BSD 3-Clause,
(c) the Power Systems Engineering Research Center (PSERC) and individual
contributors. Its [upstream license notice](LICENSE-MATPOWER.txt) explicitly
excludes case data from that license and describes the data as included with
permission or converted from public sources. Each file keeps its original
header and data citation. The PEGASE case is
fictitious data representing the size and complexity of the European
transmission network (stated in its header).

`case33bw.m` is byte-exact from MATPOWER commit
[`80b6e16f760f58721ad31221c7287c28259cd4ad`](https://github.com/MATPOWER/matpower/blob/80b6e16f760f58721ad31221c7287c28259cd4ad/data/case33bw.m)
(upstream Git blob `97e510bee25c543de0453959672fe86b40a999c8`). Its original
header cites the Baran and Wu 1989 distribution-network data. The upstream
license notice above is preserved verbatim from the same commit.
It pins the original kW/kVAr and ohm literals, the post-literal conversion
statements, and exact-byte source echo. The numerical regression checks
3.715 MW, 2.300 MVAr, and `Zbase = 12.66^2 / 10` ohms.

## `pio-ir/`

PowerIO IR documents written by published PowerIO releases, original to this
repository under its code license. `pio-ir/0.11.6/hvdc-dc-pf-instance.pio.json`
and `pio-ir/0.11.6/hvdc-ac-pf-instance.pio.json` are the DC and AC power flow
instances the `powerio` 0.11.6 wheel from PyPI wrote (`parse`,
`to_dc_pf_instance` or `to_ac_pf_instance`, then `serialize`) from a hand
written two bus MATPOWER case with 40 MW of load and one `dcline` sending
10 MW and delivering 9.5 MW. They are kept byte exact as that release wrote
them, without the `hvdc_treatment` field later releases write, and pin the
calculation such a document denotes.

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

## `cgmes/`

Hand written CGMES documents original to this repository under its code
license. `cgmes/node-breaker/` is a CGMES 2.4.15 EQ and SSH set without a TP
profile; `cgmes/node-breaker/README.md` describes its equipment and the
calculated buses it exercises. No ENTSO-E conformity data is vendored.

## `ieee-cdf/`

The IEEE 14 and 30 bus test cases in the IEEE Common Data Format, copied
without edits from PowSybl Core (MPL-2.0), which carries the University of
Washington archive files. Source path, checksums, and the archive quirk in
the 30 bus file are in `ieee-cdf/README.md`.

## `psse/` and `egret/`

Original to this repository: the PSS/E RAW files are hand written minimal
cases (their title lines say what each exercises), except the two PowSybl
Core revision 32 cases whose source paths, license, and digests are listed in
`psse/README.md`; the egret JSON files
are renderings of the MATPOWER cases above (plus a small dcline case) in
egret's ModelData schema.

Everything else in this tree (`capi_matrix/`, `angle_bounds_clamp.m`, and the
other unlisted fixtures) is original to this
repository under its code license.

## `large/` (not committed)

Gitignored; `evals/validation/fetch_cases.sh` downloads the large benchmark cases
on demand from the MATPOWER repository and
[goghino/opf_benchmarks](https://github.com/goghino/opf_benchmarks) and
documents their origins.
