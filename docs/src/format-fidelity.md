# Format fidelity and validation

How powerio's readers and writers are validated, the conventions they follow, and
the known limits. The headline fidelity table is in the
[top level README](https://github.com/eigenergy/powerio#current-format-fidelity); this
document covers the conventions and the proof behind it.

## Conventions

powerio's numeric conventions match MATPOWER and PowerModels.jl. The reference
implementations and the matching powerio code:

| Quantity | Convention | Reference | powerio |
| --- | --- | --- | --- |
| Bus type codes | \\(1 = \mathrm{PQ}\\), \\(2 = \mathrm{PV}\\), \\(3 = \mathrm{ref}\\), \\(4 = \mathrm{isolated}\\) | MATPOWER `idx_bus` | `network::BusType` |
| Impedance, susceptance | per unit on `baseMVA`, never rescaled | MATPOWER `idx_brch` (`BR_B` already per unit) | `format::matpower` |
| Branch terminal admittance | MATPOWER `BR_B` splits half to each end; richer sources use canonical `g_fr`/`b_fr`/`g_to`/`b_to`; one-value targets receive the total susceptance projection | PowerModels `matpower.jl`; MATPOWER `idx_brch` | `network::BranchCharging`, `Branch::terminal_charging` |
| Tap ratio | `0` means a line (treated as `1`); nonzero is a transformer | MATPOWER `idx_brch` `TAP` | `Branch::effective_tap` |
| Phase shift, angle | degrees in the model; PowerModels JSON carries radians | PowerModels `make_per_unit!` | `format::powermodels` |
| Angle limits | `angmin`/`angmax` default ±360 (unconstrained) | MATPOWER `idx_brch` `ANGMIN`/`ANGMAX` | `Branch::has_angle_limits` |
| pandapower/PyPSA impedance | line `r/x` are converted between per unit and ohms with \\(Z_{\mathrm{base}} = V_{\mathrm{kV}}^2 / \mathrm{baseMVA}\\); pandapower line charging is capacitance per km (`c_nf_per_km`, converted via \\(2\pi f \ell Z_{\mathrm{base}}\\)); PyPSA line `b` is siemens | pandapower PPC conversion, PyPSA static components | `format::pandapower`, `format::pypsa` |
| dcline `Pt`/`Qf`/`Qt` | sign flips vs MATPOWER | PowerModels `matpower.jl` | `format::powermodels` |
| Generator cost | \\(c_2 p^2 + c_1 p\\) maps to \\(q = 2c_2\\), \\(c = c_1\\); coefficients high order first | MATPOWER `idx_cost`, egret `matpower_parser` | `GenCost::quadratic` |
| `source_id` | `["bus", id]` for bus-tied elements | PowerModels `matpower.jl` | `format::powermodels` |
| PSLF shunts | EPC `pu_mw`/`pu_mvar` are per unit on `sbase`; `Network::Shunt` stores MW/MVAr at \\(V = 1\\) | paired EPC/RAW case checks | `format::pslf` |
| GO Challenge 3 time series | `Network` stores the first interval as a static case; `.pio.json` packages carry replayable later intervals in `operating_points` | Rust GOC3 package tests | `format::goc3`, `powerio_pkg::operating` |
| Surge angles | Surge JSON carries voltage angles, phase shifts, and angle limits in radians; `Network` stores degrees | Rust Surge round trip tests | `format::surge` |

egret's own MATPOWER parser uses the same reductions (bus type as
`matpower_bustype`, polynomial coefficients reversed to a `{degree: coefficient}`
map, piecewise to `[[mw, cost], ...]`, impedances left per unit), which is why a
MATPOWER case taken through powerio to egret JSON matches egret's direct import.

## Validation

The harness script `benchmarks/run_validation.sh` checks powerio against five independent
tools. Every classic text reader and writer runs under an oracle: the conversion
matrix covers MATPOWER, PSS/E, and egret sources against all five legacy text
targets, every PowerWorld output is read back and bridged to PowerModels JSON,
and the PMread leg covers the PowerModels JSON read side. pandapower JSON and
PyPSA CSV folders have dedicated import validators because pandapower has its
own JSON schema and PyPSA is a directory format; both validate the write
direction only — the pandapower JSON and PyPSA readers have no external oracle.
They, GO Challenge 3 JSON, Surge JSON, and the remaining source/target pairs
(PowerModels JSON and PowerWorld sources into the non-PowerModels targets) rest
on the Rust round trip suite.

- **PowerModels.jl** (`validate_powermodels.jl`, `validate_psse.jl`,
  `core_json.jl`). Reads MATPOWER, PowerModels JSON, and PSS/E. The MATPOWER to
  PowerModels JSON path is checked field by field after per unit normalization;
  the others by element counts and demand/generation/shunt totals.
- **egret** (`validate_egret.py`). The oracle for egret output, which PowerModels
  cannot read: it loads powerio's egret JSON with `egret.data.model_data.ModelData`
  and compares counts, totals, and generator cost curves.
- **ExaPowerIO.jl** (`validate_exapowerio.jl`). Reads MATPOWER through powerio's C
  ABI and compares value for value.
- **pandapower** (`validate_pandapower.py`,
  `validate_pandapower_converter.py`). Cross-checks MATPOWER parse/\\(Y_{\mathrm{bus}}\\) and
  imports powerio's pandapower JSON output back into pandapower, comparing counts
  and \\(Y_{\mathrm{bus}}\\).
- **PyPSA** (`validate_pypsa.py`). Imports powerio's PyPSA CSV folder output and
  checks counts, totals, line r/x/b rebased from ohms on the bus0 voltage, and
  transformer r/x/tap_ratio/s_nom rebased from the transformer `s_nom` base; a
  line/transformer split mismatch fails the case.

### The conversion matrix

`benchmarks/validate_matrix.py` converts each source to every legacy text target and checks
the electrical core of the output (bus/branch/generator counts and the per unit
demand, generation, and shunt totals) against the source's own core, read by an
independent oracle. The diagonal is checked byte exact: writing back to the source
format reproduces the file. Sources use the real native files where they exist
(the vendored PSS/E `.raw` and egret `.json`) and representative MATPOWER cases
otherwise: basic (`case9`), shunts and transformers (`case14`, `case30`), size
(`case118`, `case2869pegase`), HVDC with a mixed piecewise/polynomial gencost
(`t_case9_dcline`), and a piecewise-cost case (`pglib_opf_case5_pjm`).

All 65 legacy text cells pass (13 source cases × 5 targets). The core is preserved by every
writer regardless of fidelity tier, so it is the invariant checked across the
whole matrix; cost, HVDC, and angle limits are tier specific and covered by the
dedicated checks above and the Rust suite. The pandapower JSON and PyPSA CSV
validators run alongside this matrix and are reported as separate legs.

### Running it

```sh
cargo build --release -p powerio-capi
python3.12 -m venv .venv
.venv/bin/python -m pip install --upgrade pip maturin -r benchmarks/requirements.txt
env VIRTUAL_ENV=$PWD/.venv .venv/bin/maturin develop --release
julia --project=benchmarks -e 'using Pkg; Pkg.instantiate()'
bash benchmarks/run_validation.sh
```

The oracle tools (PowerModels.jl, egret, ExaPowerIO.jl, pandapower, PyPSA) are
benchmark scoped: they are declared in `benchmarks/Project.toml` and
`benchmarks/requirements.txt`, never as dependencies of the powerio package.
`benchmarks/run_validation.sh` requires the Python oracles to import in the
selected Python 3.11+ environment; a missing PyPSA, pandapower, or egret import
is a setup failure.

## Known limits

Write side losses are reported in `Conversion::warnings`; the pandapower and
PyPSA readers itemize what they ignore in `Parsed::warnings` (`read_warnings`
in Python), naming the table and counting the affected rows.
`convert_file`/`convert_str` fold the read warnings into `Conversion::warnings`.

- **PSS/E** reads revisions 33, 34, and 35. 3-winding transformers are kept as
  typed records and star-lowered into \\(Y_{\mathrm{bus}}\\)/connectivity by the indexed view;
  two-terminal DC lines map to the neutral HVDC model. A switched shunt keeps its
  steady-state susceptance `BINIT` as the shunt `b` and carries its mode, voltage
  band, regulated bus, and step blocks. A 2-winding transformer's magnetizing
  susceptance round-trips through `MAG2` (\\(\mathrm{CM} = 1\\)). Impedances are assumed on the
  system base (\\(\mathrm{CZ} = \mathrm{CW} = 1\\)).
- **PowerWorld** `.aux` carries no system base, so the reader defaults to 100 MVA.
  No third-party `.aux` reader exists, so that writer is validated by powerio's own
  read back plus a PowerModels JSON bridge.
- **PSLF** `.epc` is read and written. The reader maps the static power flow core:
  buses, lines, two- and three-winding transformers, generators, loads, fixed
  shunts, controlled shunts at initial `g/b`, and limited two-terminal DC records.
  Three-winding transformers are kept as typed records and star-lowered into
  \\(Y_{\mathrm{bus}}\\)/connectivity by the indexed view. Unsupported sections stay in the
  retained source text and emit warnings.
- **MATPOWER** canonical output (for a case that did not originate as MATPOWER)
  omits dcline; the byte exact echo path keeps it when the case was read from
  MATPOWER. Storage is written as an `mpc.storage` block.
- **egret** output drops HVDC and storage. The reader takes the power flow
  ModelData subset (numeric bus ids, scalar values); unit commitment cases
  (`system.time_keys`) are rejected.
- **pandapower JSON** writes the power flow core as split oriented
  `pandapowerNet` tables. Line ohms are referred to the from bus voltage, as
  pandapower's `build_branch` reads them; a bus with baseKV 0 writes
  `vn_kv` set to \\(1\\) (warned) so the per unit impedances survive. A branch with a
  tap, a shift, or terminals on two voltage levels becomes a `trafo` row with
  `tap_changer_type = "Ratio"`; its MATPOWER charging b rides as one bus
  shunt per terminal (warned, \\(Y_{\mathrm{bus}}\\) exact) because pandapower's magnetizing
  model is inductive only.
  The file is labeled with `f_hz` set to \\(50\\) and `c_nf_per_km` compensated, so
  a 60 Hz source keeps its exact \\(Y_{\mathrm{bus}}\\). Reference buses without a generator
  get an `ext_grid` row, which reads back as a Ref generator. The writer also
  warns on dropped HVDC, storage, capability columns, angle limits, rate B/C,
  non-finite values (written as JSON null), and costs `poly_cost` cannot
  carry. The reader models ratio, ideal, and pandapower 2.x tap changers,
  off-nominal `vn_hv_kv`/`vn_lv_kv`, lv side taps, and shunt `vn_kv` scaling;
  ZIP load composition, line shunt conductance, magnetizing branches, tabular
  tap changers, reactive cost coefficients, and every other non-empty table
  warn with row counts.
- **PyPSA CSV folders** are canonicalized directory outputs, not byte exact
  text conversions. Covered: static buses, generators, loads, lines (ohms on
  the bus0 voltage, as PyPSA computes them), transformers (rebased between
  the system base and the transformer `s_nom`), shunts, storage units, and
  base MVA. The reader maps links to HVDC with a warning, requires `v_nom`
  and balanced CSV quoting, and warns on stores, nonzero `g`, and every CSV
  it does not read (time series, carriers). The writer keys tables by bus
  name, falling back to the numeric id when names collide (warned), and warns
  on dropped HVDC, q limits, mbase, transformer angle limits, rate B/C,
  isolated buses, non-finite p limits, and slackless or normalized networks.
  Nonnumeric bus names read back as dense synthetic ids with the originals on
  `Bus.name`.
- **GO Challenge 3 JSON** reads ARPA-E GO Competition Challenge 3 input data
  into the balanced transmission model. `Network` is static, so the reader maps
  the first time interval into generator/load bounds and status fields, keeps
  the original JSON for byte exact source echo, and warns about scheduling data
  left in the retained source. There is no canonical GOC3 writer from an
  arbitrary `Network`; `TargetFormat::Goc3Json` only succeeds as a same format
  source echo. When a GOC3 `Network` is wrapped in `.pio.json`, `powerio-pkg`
  extracts the full input time axis into `operating_points`. Materializing one
  point applies those updates to the static payload and clears the series.
- **Surge JSON** reads and writes the versioned `surge-json` network document.
  The reader maps buses, loads, fixed shunts, branches, generators, storage, and
  HVDC links into `Network`, retains the original source for same format echo,
  and warns about source sections that stay only in the retained document. The
  writer emits a canonical Surge network body for the supported power flow core;
  richer MATPOWER generator capability or ramp columns and unsupported cost
  shapes are reported in `Conversion::warnings`.
- **gridfm** (read, the `gridfm` feature in `powerio-matrix`) reconstructs a
  `Network` from the gridfm-datakit Parquet dataset: lossy, but it recovers
  everything a power flow needs. That is bus types/voltages/limits, nodal load
  and shunt totals, generator
  dispatch and bounds, branch `r/x/b/tap/shift/rate_a`/angle limits, and `baseMVA`;
  it can't recover original bus ids (synthesized `1..n`), per element load/shunt
  granularity (folded one synthetic element per bus), piecewise/cubic gen costs
  (read as none), or HVDC/storage. Because the writer stores the *effective* tap,
  a branch with unit tap and no phase shift is read back as a line (raw \\(\mathrm{tap} = 0\\));
  a unity ratio, zero shift transformer in the source is thus read as a line (the
  power flow is identical). The losses are returned as a warnings list on
  `GridfmRead`, mirroring `Conversion::warnings`. The same direction writer is
  documented in the
  [top level README](https://github.com/eigenergy/powerio#gridfm).
