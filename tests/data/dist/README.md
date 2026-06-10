# Distribution network fixtures

Vendored upstream cases for `powerio-dist`. Per CONTRIBUTING.md, fixture bytes
are pinned exactly as committed; do not reformat or re-encode them.

## bmopf/

Draft BMOPF schema and example networks from the IEEE PES Task Force on
Benchmarking Multiconductor OPF.

- Source: <https://github.com/frederikgeth/bmopf-report>, commit
  `f93bca69c59e47d08a727145277406ed3f11aa3f`, directory
  `draft_schema_and_networks/`.
- `draft_bmopf_schema.json` sha256
  `b28d712e32a467ad0b339c600f51562aa049574c86cd4323ab18c4fb2e45d089`
- `example_ieee13.json` sha256
  `dec886d0fcde8bb82ef3d4567d04c08eced87a84d30a041385cac97a936dd757`
- `example_enwl_n1_f2.json` sha256
  `c635a3a2a2783b3e0e8249e65ef17f217a464955977e2223ae8f7d39b6519d6c`

## opendss/

IEEE 13, 34, and 123 bus test feeders from the official OpenDSS distribution,
vendored via the dss-extensions mirror of the EPRI test case tree.

- Source: <https://github.com/dss-extensions/electricdss-tst>, commit
  `3b208397160213cae4a9e2d0a7d1aa3528ce26e1`, directory
  `Version8/Distrib/IEEETestCases/`.
- `ieee13/`: `IEEE13Nodeckt.dss`, `IEEELineCodes.DSS`, `IEEE13Node_BusXY.csv`
  (from `13Bus/`).
- `ieee34/`: `ieee34Mod1.dss`, `Run_IEEE34Mod1.dss`, `IEEELineCodes.DSS`
  (from `34Bus/`).
- `ieee123/`: `IEEE123Master.dss`, `IEEE123Loads.DSS`,
  `IEEE123Regulators.DSS`, `IEEELineCodes.DSS` (from `123Bus/`).
- `IEEELineCodes.DSS` at this directory's root is the shared linecode file
  the per-feeder 30 byte stubs redirect to (`redirect ../IEEELineCodes.DSS`),
  mirroring the upstream layout.

## micro/

Original cases written for this crate (no upstream source). Each isolates one
construct: the four BMOPF transformer subtypes (`xfmr_single_phase`,
`xfmr_center_tap`, `xfmr_wye_delta`, `xfmr_delta_wye`), switch state with
SwtControl (`switch`), an explicit four wire linecode (`fourwire_linecode`),
OpenDSS constructor defaults (`defaults_degenerate`), and a ten conductor
linecode with double digit matrix indices (`linecode_10x10`). All eight solve
in OpenDSS (opendssdirect 0.9.4); `powerio-dist/tools/solve_dss.py` reproduces
the reference solutions.
