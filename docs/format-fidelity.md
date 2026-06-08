# Format fidelity and validation

How powerio's readers and writers are validated, the conventions they follow, and
the known limits. The headline fidelity table is in the [README](../README.md);
this document covers the conventions and the proof behind it.

## Conventions

powerio's numeric conventions match MATPOWER and PowerModels.jl. The reference
implementations and the matching powerio code:

| Quantity | Convention | Reference | powerio |
| --- | --- | --- | --- |
| Bus type codes | 1=PQ, 2=PV, 3=ref, 4=isolated | MATPOWER `idx_bus` | `network::BusType` |
| Impedance, susceptance | per unit on `baseMVA`, never rescaled | MATPOWER `idx_brch` (`BR_B` already per unit) | `format::matpower` |
| Line charging `b` | split half to each end (`b_fr = b_to = BR_B/2`) | PowerModels `matpower.jl` | `format::powermodels` |
| Tap ratio | `0` means a line (treated as `1`); nonzero is a transformer | MATPOWER `idx_brch` `TAP` | `Branch::effective_tap` |
| Phase shift, angle | degrees in the model; PowerModels JSON carries radians | PowerModels `make_per_unit!` | `format::powermodels` |
| Angle limits | `angmin`/`angmax` default ±360 (unconstrained) | MATPOWER `idx_brch` `ANGMIN`/`ANGMAX` | `Branch::has_angle_limits` |
| dcline `Pt`/`Qf`/`Qt` | sign flips vs MATPOWER | PowerModels `matpower.jl` | `format::powermodels` |
| Generator cost | `c2 p² + c1 p` → `q = 2c2`, `c = c1`; coefficients high order first | MATPOWER `idx_cost`, EGRET `matpower_parser` | `GenCost::quadratic` |
| `source_id` | `["bus", id]` for bus-tied elements | PowerModels `matpower.jl` | `format::powermodels` |

EGRET's own MATPOWER parser uses the same reductions (bus type as
`matpower_bustype`, polynomial coefficients reversed to a `{degree: coefficient}`
map, piecewise to `[[mw, cost], ...]`, impedances left per unit), which is why a
MATPOWER case taken through powerio to EGRET matches egret's direct import.

## Validation

The harness `benchmarks/run_validation.sh` checks powerio against four independent
tools. Every reader and writer, and every conversion pair, is exercised.

- **PowerModels.jl** (`validate_powermodels.jl`, `validate_psse.jl`,
  `core_json.jl`). Reads MATPOWER, PowerModels JSON, and PSS/E. The MATPOWER to
  PowerModels JSON path is checked field by field after per unit normalization;
  the others by element counts and demand/generation/shunt totals.
- **egret** (`validate_egret.py`). The oracle for EGRET output, which PowerModels
  cannot read: it loads powerio's EGRET JSON with `egret.data.model_data.ModelData`
  and compares counts, totals, and generator cost curves.
- **ExaPowerIO.jl** (`validate_exapowerio.jl`). Reads MATPOWER through powerio's C
  ABI and compares value for value.
- **pandapower** (`validate_pandapower.py`). Cross-checks the parse and the `Y_bus`.

### The conversion matrix

`benchmarks/validate_matrix.py` converts each source to every target and checks
the electrical core of the output (bus/branch/generator counts and the per unit
demand, generation, and shunt totals) against the source's own core, read by an
independent oracle. The diagonal is checked byte-exact: writing back to the source
format reproduces the file. Sources use the real native files where they exist
(the vendored PSS/E `.raw` and EGRET `.json`) and representative MATPOWER cases
otherwise: basic (`case9`), shunts and transformers (`case14`, `case30`), size
(`case118`, `case2869pegase`), HVDC with a mixed piecewise/polynomial gencost
(`t_case9_dcline`), and a piecewise-cost case (`pglib_opf_case5_pjm`).

All 65 cells pass (13 source cases × 5 targets). The core is preserved by every
writer regardless of fidelity tier, so it is the invariant checked across the
whole matrix; cost, HVDC, and angle limits are tier-specific and covered by the
dedicated checks above and the Rust suite.

### Running it

```
cargo build --release -p powerio-capi
maturin develop --release                          # the powerio wheel
julia --project=benchmarks -e 'using Pkg; Pkg.instantiate()'
pip install -r benchmarks/requirements.txt         # pandapower + egret oracles
bash benchmarks/run_validation.sh
```

The oracle tools (PowerModels.jl, egret, ExaPowerIO.jl, pandapower) are
benchmark-scoped: they are declared in `benchmarks/Project.toml` and
`benchmarks/requirements.txt`, never as dependencies of the powerio package.

## Known limits

These are reported in `Conversion::warnings`, not dropped silently.

- **PSS/E** reads revision 33. 3-winding transformers and two-terminal DC are
  skipped. A switched shunt is read as a fixed shunt at its steady-state
  susceptance `BINIT` (the same reduction PowerModels makes); block and step
  control is not modeled. Impedances are assumed on the system base (`CZ = CW = 1`).
- **PowerWorld** `.aux` carries no system base, so the reader defaults to 100 MVA.
  No third-party `.aux` reader exists, so that writer is validated by powerio's own
  read-back plus a PowerModels JSON bridge.
- **MATPOWER** canonical output (for a case that did not originate as MATPOWER)
  omits dcline and storage; the byte-exact echo path keeps them when the case was
  read from MATPOWER.
- **EGRET** output drops HVDC and storage. The reader takes the power flow
  ModelData subset (numeric bus ids, scalar values); unit commitment cases
  (`system.time_keys`) are rejected.
