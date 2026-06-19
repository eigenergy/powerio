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

## Dataset coverage

Against the OpenDSS `IEEETestCases` distribution (electricdss r4161; the set
Frederik Geth pointed to, also at `github.com/tshort/OpenDSS`).

### Converts today (typed: line, linecode, load, transformer, vsource, capacitor, generator, switch/swtcontrol, regcontrol)

| Case | Status | BMOPF JSON |
|---|---|---|
| IEEE 13 | vendored fixture, all gates pass | `tests/data/dist/bmopf/example_ieee13.json` |
| IEEE 34 | vendored fixture, all gates pass | `examples/bmopf/ieee34.json` |
| IEEE 123 | vendored fixture, all gates pass | `examples/bmopf/ieee123.json` |
| IEEE 37 | converts cleanly from the upstream dataset, dss echo byte exact | generate on demand |
| 4Bus DY / YD / GrdYD / YY (Bal) | delta, wye, grounded transformer connections; all gates pass | `examples/bmopf/4bus_dy.json` (delta wye) |
| 4Bus OYOD (open wye, open delta) | the open connection transformer is a single phase wye delta, outside the four BMOPF subtypes, so it drops with a warning | — |

The three transformer micro-cases (single phase, center tap, delta wye, wye delta)
and the switch, four wire linecode, and ten conductor micro-fixtures exercise the
same gates; see `conversion-matrix.md`.

### Needs more typed support (documented gaps)

| Case | Missing today | Behaviour now |
|---|---|---|
| 4wire-Delta | typed `Generator` modeling | generators land in `untyped`, dropped with a warning |
| 8500-Node | typed `Generator`, plus `Fuse`/`Recloser`/`Relay`/`CapControl` instrumentation | partial; control elements drop with warnings |
| NEVTestCase | typed `Reactor` (neutral-to-earth grounding) | **addressed by the stacked typed-reactor PR**, which maps a grounding reactor to a BMOPF `shunt` |
| LVTestCase / LVTestCaseNorthAmerican | `LoadShape`/`Monitor`/`EnergyMeter` time series; `CNData`/`LineGeometry`/`LineSpacing` geometry | core network converts; instrumentation and geometry drop with warnings |

Unsupported OpenDSS classes never fail a parse: they fall through to the `untyped`
store and the writer reports them, so coverage grows additively without silent
loss. Protection and instrumentation (Monitor, EnergyMeter, Recloser, Relay, Fuse,
TCC_Curve) are out of scope for a power flow / OPF data model.
