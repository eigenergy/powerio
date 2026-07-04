# Distribution network fixtures

Vendored upstream cases for `powerio-dist`. Per CONTRIBUTING.md, fixture bytes
are pinned exactly as committed; do not reformat or re-encode them.

## bmopf/

Draft BMOPF schema and example networks from the IEEE PES Task Force on
Benchmarking Multiconductor OPF.

- Example source: <https://github.com/frederikgeth/bmopf-report>, commit
  `3a786e16c761981951f1deab72fd28624577dda6`, directory
  `draft_schema_and_networks/network_examples/`.
- Schema source: same commit, with the matrix key, stale `$id`, and switch
  `i_max` corrections from `june26-report-updates`
  (`72ae3e672784c81f7f75f3283e0138cb70e6ebaa`) applied on top.
- `draft_bmopf_schema.json` sha256
  `1868a7cb599d7bc348dbdae5c406d569fc1c568212828ef9b3cbcbdb616f8603`
- `example_ieee13.json` sha256
  `dec886d0fcde8bb82ef3d4567d04c08eced87a84d30a041385cac97a936dd757`
- `example_enwl_n1_f2.json` sha256
  `082660cc835419a8335f1afca43ee89eea61216d4120ebe8b171b01550afb0d8`

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
(`onephase_cvr_load`, `onephase_zip_load`). All thirteen solve in OpenDSS
(opendssdirect 0.9.4). `benchmarks/validate_opendss.py` compares the twelve
solve fidelity fixtures against their canonical regenerated decks; it excludes
`defaults_degenerate` because that fixture intentionally relies on constructor
defaults, including omitted load voltage bounds.

## pmd/

ENGINEERING model JSON generated from the fixtures above with
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
