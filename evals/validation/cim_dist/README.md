# Distribution CIM validation

Validation of the `cim-dist` reader and writer against independent CIM
tooling, run on PowerIO's own fixtures and on public distribution feeders.
The CIM schema workflow runs the committed fixtures and false-pass regressions
in CI. The external corpus remains optional and is reported separately.
These checks do not run in `cargo test` and do not establish electrical equivalence.

This directory holds leg D, schema conformance. The census, CIM-Graph
structural comparison, OpenDSS electrical comparison and benchmark legs join
it in later changes (#456).

## Leg D: schema conformance

```
bash evals/validation/cim_dist/run_schema_check.sh
```

`schema_check.py` reads each CIM RDF/XML document and checks it in two passes
against the profile classes CIM-Graph generates from the CIM UML:

- **Vocabulary, against `cim17v40` (errors).** Every class exists; every
  property is declared on the object's class or an ancestor, under the class
  that qualifies it; attributes are literals and associations are references;
  enumeration values name a member of the declared enumeration; integer,
  float and boolean literals parse; a reference to an object in the document
  names an object of the declared target class.
- **Profile, against `rc4_2021` (warnings).** The classes and properties the
  document uses that the GridAPPS-D CIM100 profile leaves out. A valid CIM
  document can use classes a profile omits, so these are recorded, not
  refused.

It checks:

- PowerIO's fresh `cim-dist` output for every distribution fixture under
  `tests/data/dist/{micro,pmd,bmopf,cim}` that parses as a case, and for every
  corpus feeder present. Fresh output is `powerio serialize` then
  `powerio convert` from the IR document, which carries no retained source
  bytes, so a CIM source is written canonically rather than echoed.
- The CIM sources themselves (`tests/data/dist/cim/*.xml` and the corpus
  feeders), as a control. Their findings are upstream quirks, recorded once.

Expected fixture paths come from the baseline as well as file discovery: a
deleted fixture/directory or failed serialization cannot remove a case from
the run. Non-network JSON documents are explicitly listed with reasons in
`fixture_skips`; no parse failure is silently treated as a non-case. Refusals
must match the recorded diagnostic. New cases need an explicit baseline even
if they have zero findings. Unsupported nested RDF/XML property values produce
`VOCAB.NESTED_VALUE_UNSUPPORTED`; this checker does not recursively validate
inline objects.

For the reproducible CI scope, pass `--fixtures-only`. Run the regressions with
`.venv-cim/bin/python -m unittest discover -s evals/validation/cim_dist -v`.
Use `--require-corpus` for an external qualification run that must include all
six feeders. The normal optional-corpus run reports how many are present;
missing feeders have not been validated. A baseline update refuses to erase
missing expected fixture cases.

Every case's findings, counted per code and qualified name, are compared with
`schema_expected.json`; a new finding or one that disappears fails the run.
After a deliberate change, rerun with `--update-expected` and review the diff.
A vocabulary finding on PowerIO output is accepted into that file only with a
note in its `notes` block saying why.

### The five things

| | |
|---|---|
| Oracle | CIM-Graph 0.5.0a12 (PNNL, BSD-3), profiles `cim17v40` (vocabulary) and `rc4_2021` (profile); pinned in `requirements-cim.txt` |
| Settings | PowerIO CLI from the working tree (`POWERIO`); CIM100 namespace `http://iec.ch/TC57/CIM100#` |
| Identity mapping | None needed: each document is checked on its own; findings are keyed by code and CIM qualified name, with up to three example mRIDs |
| Tolerances | Exact: finding counts per key equal the expected counts |
| Expected codes | `schema_expected.json`, per case, with notes for every accepted vocabulary finding |

### Corpus

The feeders are GridAPPS-D Powergrid-Models `releases/2024.06.0`
`platform/cimxml` (IEEE 13, IEEE 13 Assets, IEEE 37, IEEE 123, IEEE 8500, and
the 9500 node balanced feeder), read from `tests/data/large/cim_dist/`
(gitignored, or `CIM_DIST_CORPUS`). They are fetched, never vendored: the
release branch has no licence file at its root. Absent feeders are skipped,
so the fixture cases run with no download. Until the corpus fetch script
lands, download them by hand:

```
base=https://raw.githubusercontent.com/GRIDAPPSD/Powergrid-Models/releases/2024.06.0/platform/cimxml
mkdir -p tests/data/large/cim_dist
for f in IEEE13 IEEE13_Assets IEEE37 IEEE123 IEEE8500 final9500nodebalanced; do
    curl -sSfL -o tests/data/large/cim_dist/$f.xml "$base/$f.xml"
done
```

### What it found first

- PowerIO's writer before #648 used `PhaseImpedanceData.PerLengthPhaseImpedance`,
  which `cim17v40` does not declare (`VOCAB.PROPERTY_UNKNOWN`, one per
  impedance entry). With #648 PowerIO's output has no vocabulary findings.
- `tests/data/dist/cim/micro_feeder.xml` states shunt sections as
  `ShuntCompensatorPhase.bPerSection` and `gPerSection`; `cim17v40` declares
  them on `LinearShuntCompensatorPhase`, which is what the writer emits.
- The Powergrid-Models sources use `PhotovoltaicUnit` (`PhotoVoltaicUnit` in
  `cim17v40`), `BatteryState.Charging` (`BatteryStateKind.charging`),
  `SynchronousMachine.p` (declared on `RotatingMachine`), and
  `RegulatingControl.discrete` and `ConnectivityNode.OperationalLimitSet`,
  which `cim17v40` does not declare.

`RESULTS.md` holds the rendered per case table from the last run.

## Native tank electrical qualification

`../validate_cim_tanks.py --powerio <current-cli>` runs in the main Validation
workflow with OpenDSSDirect.py. It compares nine transformer cases against
committed native DSS decks and hand-stated regulator nameplates, through both
CIM reading and fresh CIM writing. The comparison covers the grounded active
terminal admittance stamp (ground node 0 is eliminated) and loaded complex
coil voltages, with rtol/atol of 1e-10/1e-8 S and 1e-9/1e-6 V respectively.
Nonzero resistance, center taps, open-delta legs, no-load losses, three-phase
connections and regulator taps are included. An independent native lead/lag
control distinguishes clocks 1 and 11; inactive/disconnected and unsupported
clock inputs must refuse conversion. This synthetic qualification does not
claim validation of the optional external feeder corpus or X-in-magnitude
producer dialects.
