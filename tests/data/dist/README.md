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
- The released schemas live in
  <https://github.com/distribution-system-opt/dsopt-schema>. `draft_bmopf_schema.json`
  is the 0.1.0 draft the two examples validate against, kept beside them.
- `bmopf-0.2.0.schema.json` is the 0.2.0 proposal, from
  <https://github.com/distribution-system-opt/dsopt-schema> at commit
  `202b7b59ae3ea97b81cded0b6d428234f6c536c2` on branch
  `propose-bmopf-0.2.0`, unchanged. sha256
  `2744afb88a036783a6d22f5e830fe254321a6a68e203d31c6ff2e084d27b20cc`. It is
  the version the writer targets by default, so the writer's output is
  validated against it; the proposal is CC BY 4.0. Re-vendor it when that
  proposal changes.

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
case with `PVSystem` and `InvControl` (`ibr_pv_control`). All fourteen solve in
OpenDSS (opendssdirect 0.9.4). `evals/validation/validate_opendss.py` compares the
thirteen solve fidelity fixtures against their canonical regenerated decks; it
excludes `defaults_degenerate` because that fixture intentionally relies on
constructor defaults, including omitted load voltage bounds.

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

## Licensing

Each directory carries its own license file next to the data it covers:
`bmopf/License.md`, `opendss/License.txt` (the BSD 3 clause notice retained
from the upstream distribution), `micro/License.md` (CC BY 4.0), and
`pmd/License.md` (derivatives carry their sources' licenses). The repository
code license does not apply to vendored data.

The two BMOPF examples carry different licenses, so `bmopf/License.md` states
each one: `example_enwl_n1_f2.json` is CC BY 4.0 from the CSIRO Data Collection
release it derives from, and `example_ieee13.json` is governed by the OpenDSS
distribution license in `opendss/License.txt`.
