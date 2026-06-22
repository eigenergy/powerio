# Distribution validation and coverage

How `powerio-dist` is validated against real OpenDSS feeders, which cases it
converts today, and which need more typed support. The per-fixture fidelity
counts live in [conversion-matrix.md](conversion-matrix.md); this page records
the dataset coverage and the validation gates behind it.

## Validation gates

Every fixture passes three independent checks, run from `tests/` and `tools/`:

- **Round trip** (`tests/matrix.rs`). Each case goes dss → model → {dss, BMOPF,
  PMD} and back. Writing to the source format reproduces the file byte for byte
  (the `echo` diagonal); every cross-format write reports each dropped field as a
  warning. The 3×3 matrix is regenerated into `conversion-matrix.md`.
- **OpenDSS re-solve** (`tools/physics_check.py`, oracle `opendssdirect`). The
  emitted `.dss` is re-solved and compared node by node against the source; the
  gate is a worst-case voltage deviation at or below 1e-8 pu, with documented
  exceptions for known transforms (e.g. RegControl to a fixed tap).
- **PMD cross-check** (`tools/pmd/pmdtool.jl`, oracle PowerModelsDistribution
  v0.16). The emitted PMD ENGINEERING JSON parses and agrees on topology. Cases
  with ragged transformer connections (delta-wye) are excluded: PMD's JSON reader
  `hcat`s connection lists and throws on ragged input (an upstream PMD bug).
- **Local DSS corpus BMOPF gate** (`tests/local_dss_corpus.rs`, opt in with
  `POWERIO_DIST_LOCAL_DSS_CORPUS`). Every `.dss` under the supplied tree parses,
  writes BMOPF JSON, validates against the vendored schema, reparses, writes DSS,
  reparses that DSS, and writes a second BMOPF JSON. The second BMOPF document
  must be schema valid and stable up to JSON numeric rounding. Against
  electricdss r4161 `IEEETestCases`, the gate passes all 91 `.dss` files:
  12,274 parse warnings, 290,393 BMOPF warnings, and 9,756 counted real network
  losses.

## Dataset coverage

Against the OpenDSS `IEEETestCases` distribution (electricdss r4161; the set
Frederik Geth pointed to, also at `github.com/tshort/OpenDSS`).

### Converts today (typed: line, linecode, load, transformer, vsource, capacitor, generator, reactor, switch/swtcontrol, regcontrol)

| Case | Status | BMOPF JSON |
|---|---|---|
| IEEE 13 | vendored fixture, all gates pass | `tests/data/dist/bmopf/example_ieee13.json` |
| IEEE 34 | vendored fixture, all gates pass | `examples/bmopf/ieee34.json` |
| IEEE 123 | vendored fixture, all gates pass | `examples/bmopf/ieee123.json` |
| 4Bus DY / YD / GrdYD / YY (Bal) | delta, wye, grounded transformer connections; all gates pass | `examples/bmopf/4bus_dy.json` (delta wye) |
| 4Bus OYOD (open wye, open delta) | the open connection units are single phase wye/delta transformers; they convert to `single_phase` with both delta phase terminals preserved | generate on demand |

The open delta leg is the single phase wye/delta path. The `single_phase`
subtype carries its terminals and impedance faithfully but has no field for the
wye/delta connection, so the writer flags each one (the consumer reads it as a
wye-wye unit). The dss re-solve is exact; the BMOPF re-solve differs by the
floating delta's ground reference, which the connection label would pin. See the
`xfmr_open_wye_open_delta` and `xfmr_1ph_delta_wye` micro-fixtures.

The transformer micro-cases (single phase, center tap, delta wye, wye delta, open
wye / open delta, single phase delta wye) and the switch, four wire linecode, and
ten conductor micro-fixtures exercise the same gates; see `conversion-matrix.md`.

### Needs more typed support (documented gaps)

| Case | Missing today | Behaviour now |
|---|---|---|
| 4wire-Delta | the three winding (wye/delta/delta) unit | the open wye/open delta units now convert as `single_phase`; fixed OpenDSS generators encode as negative BMOPF loads; the three winding bank still drops, pending the transformer model extension |
| IEEE 37 | delta-delta transformers and regulators | BMOPF JSON is schema valid and stable through DSS regeneration; unsupported transformer buses and terminals are pruned with warnings |
| 8500-Node | transformer model extension, plus `Fuse`/`Recloser`/`Relay`/`CapControl` instrumentation | center tap secondaries preserve grounding through BMOPF → DSS; unsupported banks, control, and protection elements drop with warnings |
| NEVTestCase | transformer model extension for the remaining transformer banks; series and impedance form reactors | grounding reactors with `kvar`/`kv` map to BMOPF `shunt`; the remaining reactor forms and transformer banks drop with warnings |
| LVTestCase / LVTestCaseNorthAmerican | `LoadShape`/`Monitor`/`EnergyMeter` time series; `CNData`/`LineGeometry`/`LineSpacing` geometry | core network converts; instrumentation and geometry drop with warnings |

Unsupported OpenDSS classes never fail a parse: they fall through to the `untyped`
store and the writer reports them, so coverage grows additively without silent
loss. Protection and instrumentation (Monitor, EnergyMeter, Recloser, Relay, Fuse,
TCC_Curve) are out of scope for a power flow / OPF data model.

The remaining transformer losses require a BMOPF schema extension; tracked
upstream in `frederikgeth/bmopf-report#9`.
