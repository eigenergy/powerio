# Distribution network fixtures

Vendored upstream cases for `powerio-dist`. Per CONTRIBUTING.md, fixture bytes
are pinned exactly as committed; do not reformat or re-encode them.

## bmopf/

Draft BMOPF schema and example networks from the IEEE PES Task Force on
Benchmarking Multiconductor OPF.

- Source: <https://github.com/distribution-system-opt/bmopf-resources>, commit
  `f2e368470a5012dd264d1f5a2f867867fb926615`. The schema comes from
  `draft_schema_and_networks/draft_bmopf_schema.json` and the two examples from
  `draft_schema_and_networks/network_examples/`, all three unchanged with
  nothing applied on top.
- `draft_bmopf_schema.json` sha256
  `a74f4d2be151e4b250a47a1730445301c093572fce8de609e9af15b76c67ef73`
- `example_ieee13.json` sha256
  `48707ea839c20032c88df715587e50d097637cd0cc8a17b2d213d4591eea8bc7`
- `example_enwl_n1_f2.json` sha256
  `24d2c054b70b5e09d179f785cb09ff90cddbf73c859756154e9eb604782b69f1`
- Versioned schemas are under `powerio-dist/schemas/bmopf/`, with their exact
  source revisions, hashes, and CC BY 4.0 license. The historical schema stays
  beside the examples for source-fidelity tests. Its rules match archived
  0.1.0; only the embedded identifier differs. Archived 0.2.0 is a proposal.

## cim/

`micro_feeder.xml` is a synthetic, hand written distribution CIM feeder
original to this repository (CIM100 namespace, 243 lines, 13,829 bytes). It
exercises the mapped element set with hand computed expected values, pinned in
`powerio-dist/tests/cim.rs`. `tank_feeder.xml` is a synthetic distribution
CIM feeder, also original to this repository (CIM100 namespace, 533 lines,
30,526 bytes), of transformers in the `TransformerTank` form, pinned in the
same test file and compared with native OpenDSS by
`evals/validation/validate_cim_tanks.py`. Both are MIT OR Apache-2.0
(`cim/License.md`). See `cim/README.md`.

## opendss/

IEEE 13, 34, and 123 bus test feeders from the official OpenDSS distribution,
vendored via the dss-extensions mirror of the EPRI test case tree. The
feeders are the IEEE PES Distribution Test Feeder Working Group cases as
distributed with OpenDSS; they are vendored unchanged under the distribution
license in `opendss/License.txt`, with no relicensing.

- Source: <https://github.com/dss-extensions/electricdss-tst>, commit
  `3b208397160213cae4a9e2d0a7d1aa3528ce26e1`, directory
  `Version8/Distrib/IEEETestCases/`.
- `ieee13/`: `IEEE13Nodeckt.dss`, `IEEELineCodes.DSS`, `IEEE13Node_BusXY.csv`
  (from `13Bus/`).
- `ieee34/`: `ieee34Mod1.dss`, `IEEELineCodes.DSS` (from `34Bus/`; the
  upstream Run wrapper is not vendored, it references a coordinates csv and
  show/plot commands outside the converter's scope).
- `ieee123/`: `IEEE123Master.dss`, `IEEE123Loads.DSS`,
  `IEEE123Regulators.DSS`, `IEEELineCodes.DSS` (from `123Bus/`).
- `IEEELineCodes.DSS` at this directory's root is the shared linecode file
  the per-feeder 30 byte stubs redirect to (`redirect ../IEEELineCodes.DSS`),
  mirroring the upstream layout.

## micro/

Original cases written for this crate (no upstream source). Each isolates one
construct: the four BMOPF transformer subtypes (`xfmr_single_phase`,
`xfmr_center_tap`, `xfmr_wye_delta`, `xfmr_delta_wye`), two additional
single phase transformer wiring cases, switch state with SwtControl
(`switch`), an explicit four wire linecode (`fourwire_linecode`), OpenDSS
constructor defaults (`defaults_degenerate`), and a ten conductor linecode
with double digit matrix indices (`linecode_10x10`), plus a four wire feeder
whose neutral is grounded through an explicit reactor
(`neutral_grounding_reactor`) and two single phase load model cases
(`onephase_cvr_load`, `onephase_zip_load`), plus one inverter based resource
case with `PVSystem` and `InvControl` (`ibr_pv_control`), plus the one-phase
LinDist3Flow oracle described below. All fifteen solve in OpenDSS
(opendssdirect 0.9.4). `evals/validation/validate_opendss.py` compares the
thirteen general solve fidelity fixtures against their canonical regenerated
decks; it excludes `defaults_degenerate` because that fixture intentionally
relies on constructor defaults, including omitted load voltage bounds, and the
LinDist3Flow case is checked by its dedicated validator.

`lindist3flow_oracle.dss` is an original one-phase radial constant-power case
for comparing the lossless LinDist3Flow voltage equation with an independent
nonlinear OpenDSS solve. `lindist3flow_oracle.json` records the OpenDSSDirect.py
0.9.4 result. `evals/validation/validate_lindist3flow_opendss.py` regenerates
that result, while `powerio-matrix/tests/lindist3flow.rs` checks the compiled
linear model and its declared approximation error against it.

## pmd/

PMD ENGINEERING JSON generated from the fixtures above with
PowerModelsDistribution v0.16.0 (lanl-ansi/PowerModelsDistribution.jl,
commit 87dc18b0) via the committed oracle:

    julia powerio-dist/tools/pmd/pmdtool.jl dss2json \
        tests/data/dist/opendss/ieee13/IEEE13Nodeckt.dss \
        tests/data/dist/pmd/ieee13.json

`fourwire_linecode.json` comes from `micro/fourwire_linecode.dss` the same
way. PMD's `parse_file` ran with `kron_reduce=false`; `print_file` wrote the
dict. Regenerate with the same command when bumping the PMD version.

`case3_balanced.json` and `case3_balanced_multinetwork.json` reproduce issue
#646. Both come from `test/data/opendss/case3_balanced.dss` as shipped in the
PowerModelsDistribution v0.16.0 package (JSON.jl 0.21.4, Julia 1.13.1), read
with PMD's defaults:

```julia
using PowerModelsDistribution
eng = parse_file(joinpath(pkgdir(PowerModelsDistribution),
    "test/data/opendss/case3_balanced.dss"); data_model=ENGINEERING)
print_file("case3_balanced.json", eng)
# The multinetwork file keeps the first two points of the load shape.
ts = eng["time_series"]["ls1"]
ts["values"] = ts["values"][1:2]; ts["time"] = ts["time"][1:2]
print_file("case3_balanced_multinetwork.json", make_multinetwork(eng))
```

One edit follows `print_file` in each: the `files` entry, an absolute path
into the local Julia depot, is replaced with the package relative
`test/data/opendss/case3_balanced.dss`. `case3_balanced.json` is 509 lines,
8,283 bytes; `case3_balanced_multinetwork.json` is 994 lines, 20,121 bytes.

### pmd/math/

PMD MATHEMATICAL model exports beside the ENGINEERING documents they were
transformed from, the oracle for the `pmd-math-json` reader and writer. All
nine files are PowerModelsDistribution v0.16.0 `print_file` output (JSON.jl
0.21.4, Julia 1.13.1) from the `.dss` cases of the same name in the PMD
package's `test/data/opendss/`, read with PMD's defaults, so the MATHEMATICAL
documents are Kron reduced:

```julia
using PowerModelsDistribution
eng = parse_file(dss; data_model=ENGINEERING)
print_file("<case>_eng.json", eng)
print_file("<case>_math.json", transform_data_model(eng))
```

`case3_balanced_eng.json` is not repeated here: `../case3_balanced.json`
above is that export. The one edit after `print_file` replaces each `files`
entry, an absolute path into the local Julia depot, with the package relative
`test/data/opendss/<case>.dss`.

| File | Lines | Bytes |
|---|---|---|
| `case3_balanced_math.json` | 803 | 13,815 |
| `case3_balanced_cap_eng.json` | 515 | 8,349 |
| `case3_balanced_cap_math.json` | 857 | 14,657 |
| `case3_unbalanced_switch_eng.json` | 723 | 11,770 |
| `case3_unbalanced_switch_math.json` | 1,292 | 22,125 |
| `case3_unbalanced_ZIPloads_eng.json` | 526 | 8,450 |
| `case3_unbalanced_ZIPloads_math.json` | 1,031 | 18,089 |
| `ut_trans_2w_yy_eng.json` | 651 | 10,940 |
| `ut_trans_2w_yy_math.json` | 1,601 | 26,993 |

## ravens/

MG-RAVENS multiconductor JSON example from the schema project, vendored
unchanged as the reader/writer round-trip fixture.

- Source: <https://github.com/lanl-ansi/MG-RAVENS>, commit
  `094ad1163cb111df676884635aa03faa0538b688`, file
  `examples/case3_balanced.json` (a three-bus, two-line,
  three-single-phase-load feeder with a Thevenin `EnergySource`).
- `case3_balanced.json` sha256
  `4a66f801aa2506562b8611215314d36d000055d4ac9a1bec7ee84693e0b9e6e7`

## Licensing

Each directory carries its own license file next to the data it covers:
`bmopf/License.md`, `opendss/License.txt` (the BSD 3 clause notice retained
from the upstream distribution), `micro/License.md` (CC BY 4.0),
`cim/License.md` (MIT OR Apache-2.0), `pmd/License.md` (derivatives carry
their sources' licenses), and `ravens/License.md` (the MG-RAVENS Apache-2.0
notice, Copyright 2020 Triad National Security, LLC). The repository
code license does not apply to vendored data.

The two BMOPF examples carry different licenses, so `bmopf/License.md` states
each one: `example_enwl_n1_f2.json` is CC BY 4.0 from the CSIRO Data Collection
release it derives from, and `example_ieee13.json` is governed by the OpenDSS
distribution license in `opendss/License.txt`.

`micro/lindist3flow-solution.pio.json` is a synthetic two-bus, one-phase
PowerIO IR version 2 fixture for typed binding access and lifetime tests.
It is original test data, released under the micro directory's CC BY 4.0
license, with no upstream case data.
