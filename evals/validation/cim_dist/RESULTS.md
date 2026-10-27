## Leg D: schema conformance

Vocabulary errors against `cim17v40` and profile warnings against `rc4_2021` (CIM-Graph profile classes), per case.

| Case | Status | Vocabulary errors | Profile warnings |
|---|---|---|---|
| `tests/data/dist/bmopf/example_enwl_n1_f2.json output` | ok; not written: `PARSE.CLI.ERRORS_REPORTED` | | |
| `tests/data/dist/bmopf/example_ieee13.json output` | ok | 0 | 22 |
| `tests/data/dist/cim/micro_feeder.xml source` | ok | 6 | 13 |
| `tests/data/dist/cim/micro_feeder.xml output` | ok | 0 | 11 |
| `tests/data/dist/micro/defaults_degenerate.dss output` | ok | 0 | 9 |
| `tests/data/dist/micro/fourwire_linecode.dss output` | ok | 0 | 6 |
| `tests/data/dist/micro/ibr_pv_control.dss output` | ok | 0 | 6 |
| `tests/data/dist/micro/lindist3flow_oracle.dss output` | ok | 0 | 4 |
| `tests/data/dist/micro/linecode_10x10.dss output` | ok | 0 | 4 |
| `tests/data/dist/micro/neutral_grounding_reactor.dss output` | ok | 0 | 7 |
| `tests/data/dist/micro/onephase_cvr_load.dss output` | ok | 0 | 4 |
| `tests/data/dist/micro/onephase_zip_load.dss output` | ok | 0 | 4 |
| `tests/data/dist/micro/switch.dss output` | ok | 0 | 8 |
| `tests/data/dist/micro/xfmr_1ph_delta_wye.dss output` | ok | 0 | 6 |
| `tests/data/dist/micro/xfmr_center_tap.dss output` | ok | 0 | 10 |
| `tests/data/dist/micro/xfmr_delta_wye.dss output` | ok | 0 | 6 |
| `tests/data/dist/micro/xfmr_open_wye_open_delta.dss output` | ok | 0 | 10 |
| `tests/data/dist/micro/xfmr_single_phase.dss output` | ok | 0 | 6 |
| `tests/data/dist/micro/xfmr_wye_delta.dss output` | ok | 0 | 6 |
| `tests/data/dist/pmd/fourwire_linecode.json output` | ok | 0 | 6 |
| `tests/data/dist/pmd/ieee13.json output` | ok | 0 | 54 |

External corpus: not requested (fixture-only run).
