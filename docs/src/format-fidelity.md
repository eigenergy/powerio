# Format fidelity and validation

PowerIO validates readers and writers against independent tools and committed
round trip tests. The
[top level format table](https://github.com/eigenergy/powerio#supported-values-and-formats)
summarizes the supported directions; the conventions and evidence are below.

## Conventions

powerio's numeric conventions match MATPOWER and PowerModels.jl. The reference
implementations and the matching powerio code:

| Quantity | Convention | Reference | powerio |
| --- | --- | --- | --- |
| Bus type codes | \\(1 = \mathrm{PQ}\\), \\(2 = \mathrm{PV}\\), \\(3 = \mathrm{ref}\\), \\(4 = \mathrm{isolated}\\) | MATPOWER `idx_bus` | `network::BusType` |
| Impedance, susceptance | per unit on `baseMVA`, never rescaled | MATPOWER `idx_brch` (`BR_B` already per unit) | `matpower` |
| Branch terminal admittance | MATPOWER `BR_B` splits half to each end; richer sources use canonical `g_fr`/`b_fr`/`g_to`/`b_to`; one-value targets receive the total susceptance projection | PowerModels `matpower.jl`; MATPOWER `idx_brch` | `network::BranchCharging`, `Branch::calc_terminal_charging` |
| Tap ratio | `0` means a line (treated as `1`); nonzero is a transformer | MATPOWER `idx_brch` `TAP` | `Branch::calc_effective_tap` |
| Phase shift, angle | degrees in the model; PowerModels JSON carries radians | PowerModels `make_per_unit!` | `powermodels-json` |
| Angle limits | `angmin`/`angmax` default ±360 (unconstrained) | MATPOWER `idx_brch` `ANGMIN`/`ANGMAX` | `Branch::has_angle_limits` |
| pandapower/PyPSA impedance | line `r/x` are converted between per unit and ohms with \\(Z_{\mathrm{base}} = V_{\mathrm{kV}}^2 / \mathrm{baseMVA}\\); pandapower line charging is capacitance per km (`c_nf_per_km`, converted via \\(2\pi f \ell Z_{\mathrm{base}}\\)); PyPSA line `b` is siemens | pandapower PPC conversion, PyPSA static components | `pandapower-json`, `pypsa-csv` |
| dcline `Pt`/`Qf`/`Qt` | sign flips vs MATPOWER | PowerModels `matpower.jl` | `powermodels-json` |
| Generator cost | \\(c_2 p^2 + c_1 p\\) maps to \\(q = 2c_2\\), \\(c = c_1\\); coefficients high order first | MATPOWER `idx_cost`, egret `matpower_parser` | `GenCost::calc_quadratic` |
| `source_id` | `["bus", id]` for bus-tied elements | PowerModels `matpower.jl` | `powermodels-json` |
| PSLF shunts | EPC `pu_mw`/`pu_mvar` are per unit on `sbase`; `Shunt` stores MW/MVAr at \\(V = 1\\) | paired EPC/RAW case checks | `pslf` |
| DOE GO Challenge 3 | an input/problem data file parses to `AcScucInstance`; one `Source` containing that file and its matching output/solution data file parses to `AcScucSolution`; `instance.network()` returns the shared `BalancedNetwork` | pinned GO-3 data model, C3DataUtilities, and GOC3Benchmark.jl D1/D2/D3 files | `powerio::parse`, `powerio::emit` |
| Surge angles | Surge JSON carries voltage angles, phase shifts, and angle limits in radians; `BalancedNetwork` stores degrees | Rust Surge round trip tests | `surge-json` |
| DeepMind OPFData JSON | DeepMind OPFData carries p.u. powers and radian angles; `BalancedNetwork` stores the solved snapshot in MW/MVAr and degrees, with zero based links mapped to one based bus IDs | Paper Appendix A, the PyG loader, the smallest complete official fixture, and size independent FullTop and N-1 property tests | `opfdata-json` |

egret's own MATPOWER parser uses the same reductions (bus type as
`matpower_bustype`, polynomial coefficients reversed to a `{degree: coefficient}`
map, piecewise to `[[mw, cost], ...]`, impedances left per unit), which is why a
MATPOWER case taken through powerio to egret JSON matches egret's direct import.

## Validation

The harness script `evals/validation/run_validation.sh` checks powerio against five independent
tools. Every classic text reader and writer runs under an oracle: the conversion
matrix covers MATPOWER, PSS/E, and egret sources against all five legacy text
targets, every PowerWorld output is read back and bridged to PowerModels JSON,
and the PMread leg covers the PowerModels JSON read side. pandapower JSON and
PyPSA CSV folders have dedicated import validators because pandapower has its
own JSON schema and PyPSA is a directory format; both validate the write
direction only, because the pandapower JSON and PyPSA readers have no external
oracle.
DOE GO Challenge 3 has a separate pinned reference job described below. Surge
JSON and the remaining source/target pairs (PowerModels JSON and PowerWorld
sources into the non-PowerModels targets) rest on the Rust round trip suite.

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

`evals/validation/validate_matrix.py` converts each source to every legacy text target and checks
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
.venv/bin/python -m pip install --upgrade pip maturin -r evals/validation/requirements.txt
env VIRTUAL_ENV=$PWD/.venv .venv/bin/maturin develop --release
julia --project=evals/validation -e 'using Pkg; Pkg.instantiate()'
bash evals/validation/run_validation.sh
```

The oracle tools (PowerModels.jl, egret, ExaPowerIO.jl, pandapower, PyPSA) are
benchmark scoped: they are declared only in `evals/validation/Project.toml` and
`evals/validation/requirements.txt`, and the powerio package itself has no
dependency on them.
`evals/validation/run_validation.sh` requires the Python oracles to import in the
selected Python 3.11+ environment; a missing PyPSA, pandapower, or egret import
is a setup failure.

## Known limits

Every loss is a coded diagnostic. Readers itemize what they keep only in the
retained source (naming the table and counting the affected rows), writers
report what a target cannot represent, and `emit` returns the writer's findings
on the parsed module. Codes name the format and reason, such as
`READ.CGMES.RECORD_UNMAPPED`, `READ.CGMES.FIELD_UNMAPPED`,
`READ.XIIDM.FIELD_UNMAPPED`, `READ.DGS.CLASS_UNMAPPED`, or
`EMIT.PSSE.FIELD_DROPPED` for RAW and RAWX;
there is no generic
parse warning that hides the cause.

- **XIIDM** reads PowSybl XIIDM 1.12 through 1.17 and writes 1.17. It maps
  substations, voltage levels, bus breaker and node breaker topology, busbar
  sections, switches, lines, tie and boundary lines, loads, generators,
  batteries, shunts, static VAR compensators, two- and three-winding
  transformers, tap changers and controls, operational limits, reactive
  limits, HVDC converters, aliases, properties, and PowSybl active power
  control. Areas, one level of nested XIIDM networks, nonlinear shunt section
  models, physical DC equipment, and VSC/LCC converters also parse and survive
  fresh emission. Unknown extension subtrees remain available for byte exact same
  format emission and produce a diagnostic; they do not become unnamed fields
  on the network. Fresh emission preserves detailed connectivity when present
  and allocates missing local node numbers without changing stable PowerIO
  identities. A three winding transformer whose `ratedU0` differs from
  `ratedU1` keeps that leg impedance base for fresh emission. XIIDM states no
  system MVA base, so the balanced calculation view
  uses 100 MVA and reports that assumption.
- **CGMES** reads 2.4.15 on CIM16 and 3.0 on CIM100. CGMES 2.4.15 uses the
  namespace `http://iec.ch/TC57/2013/CIM-schema-cim16#` with the ENTSO-E
  extension namespace `http://entsoe.eu/CIM/SchemaExtension/3/1#` (IEC TS
  61970-600-1/-2:2017); CGMES 3.0 uses `http://iec.ch/TC57/CIM100#` and
  `http://iec.ch/TC57/CIM100-European#` (IEC 61970-600-1/-2:2021). Both use
  the IEC 61970-552 CIMXML instance syntax: `rdf:ID` defines a record,
  `rdf:about` extends one, and `md:FullModel` heads each profile document.
  EQ and TP are required;
  SSH, SV, and boundary profile data are used when present. SSH assignments
  take precedence over SV observations; an SV shunt section count that differs
  from the SSH assignment is reported and not retained. Each document's
  `Model.modelingAuthoritySet` is read for the boundary and state variable
  authority checks, reported, and not retained; fresh output states PowerIO's
  own modeling authority. A source can be an
  XML profile directory, a directory containing profile ZIP files, or one ZIP
  containing the profiles. The mapping covers hierarchy, AC and DC equipment,
  detailed connectivity, current, active power, and apparent power operating
  limits, tap changers and controls, reactive
  limits, and the operating and solution values in SSH and SV. Diagram,
  geography, dynamics, and unrecognized CIM classes are counted in
  diagnostics. Fields present on a recognized class but not consumed by the
  mapping receive grouped `READ.CGMES.FIELD_UNMAPPED` diagnostics naming the
  class, field, count, and sample identities. Fresh output is a deterministic
  CGMES 3.0 EQ, TP, SSH, and SV profile set. Imported UUID mRIDs survive for
  mapped equipment, terminals, tap changers, operational limit sets, hierarchy,
  and topology records.
  Missing identities use UUIDv5 derived from the component type and stable
  identity. Tap changer tables, tap controls and table points, reactive curve
  points, and individual limit values retain their electrical values and
  relationships but receive deterministic subordinate mRIDs on fresh emission;
  parsing reports this identity change. The source neutral limit model retains
  permanent and temporary limits. Fresh output therefore uses PATL and TATL;
  parsing reports PATLT or TCT substitution and any fractional duration rounded
  to whole seconds. Distinct same-kV `BaseVoltage` identities also receive a
  precise collapse diagnostic before fresh output uses one voltage keyed
  record. Busbar `VoltageLimit`
  records are combined with the enclosing voltage level into the most
  restrictive valid low/high voltage range. An inconsistent pair is diagnosed
  and ignored; fresh emission writes the resulting `VoltageLevel` fields rather
  than recreating the individual `VoltageLimit` records. CGMES states physical
  units but no system MVA base, so PowerIO uses and reports 100 MVA.
- **PSS/E** reads RAW revisions 33, 34, and 35 and RAWX revision 35. RAW and
  RAWX share one electrical mapping. RAW 34 maps its substation section. RAW
  35 and RAWX 35 map and freshly emit substations, nodes, switches, busbar
  sections, and equipment terminal references. Fresh RAW 34/35 and
  RAWX output preserves AC line and transformer names. Explicit RAW revisions
  outside 33 through 35, invalid system bases or frequencies, and nonfinite
  record values are rejected. Fresh output accepts only revisions 33 through
  35 and returns an error when detailed connectivity cannot form valid RAWX
  tables. Generator `IREG`/`NREG`,
  switched shunt `SWREG`/`NREG`, and transformer `CONT`/`NODE` resolve to exact
  terminal references, including an explicit target on the same bus. Every
  winding of a 3-winding transformer retains its control mode, regulated
  terminal, limits, tap position and range, and number of tap positions. A
  positive `COD` enables automatic adjustment; a negative `COD` retains the
  same mode with automatic adjustment disabled; zero is fixed. `|COD| = 4`
  controls a DC line quantity on a 2-winding transformer, and `|COD| = 5`
  controls asymmetric active power flow. Unsupported RAWX
  tables, including multi-terminal DC, FACTS, GNE, induction machine,
  multi-section line, zone, owner, and interarea transfer records, remain only
  in byte exact same format emission and produce counted diagnostics. Unknown
  RAWX tables and `caseid` fields receive the same retained source diagnostic;
  fresh output diagnoses detailed records and fields that RAWX cannot carry.
  3-winding transformers are kept as
  typed records and star-lowered into \\(Y_{\mathrm{bus}}\\)/connectivity by the indexed view;
  two-terminal DC lines map to the neutral HVDC model. A switched shunt keeps its
  steady-state susceptance `BINIT` as the shunt `b` and carries its mode, voltage
  band, regulated bus, and step blocks. A 2-winding transformer's magnetizing
  susceptance round-trips through `MAG2`. The reader converts `CW` 1/2/3,
  `CZ` 1/2/3, and `CM` 1/2 into the neutral tap ratio, system-base impedance,
  and magnetizing admittance. Fresh output uses the electrically equivalent
  canonical `CW = CZ = CM = 1` representation.
- **PowerWorld** `.aux` is read and written. `.pwb` binary cases are read
  only, and `.pwd` display files parse through the separate display API.
  `.aux` carries no system base, so the reader defaults to 100 MVA. No third party `.aux` reader
  exists, so that writer is validated by powerio's own read back plus a
  PowerModels JSON bridge. The `.pwb` layouts are reverse engineered; the decode
  evidence and coverage matrix are maintainer notes at
  [`powerio-tx/src/format/powerworld/FORMAT.md`](../../powerio-tx/src/format/powerworld/FORMAT.md).
- **PSLF** `.epc` is read and written. The reader maps the static power flow core:
  buses, lines, two- and three-winding transformers, generators, loads, fixed
  shunts, controlled shunts at initial `g/b`, and limited two-terminal DC records.
  Three-winding transformers are kept as typed records and star-lowered into
  \\(Y_{\mathrm{bus}}\\)/connectivity by the indexed view. Unsupported sections stay in the
  retained source text and emit diagnostics.
- **MATPOWER** canonical output (for a case that did not originate as MATPOWER)
  omits dcline; the byte exact echo path keeps it when the case was read from
  MATPOWER. Storage is written as an `mpc.storage` block.
- **egret** output writes HVDC as `dc_branch`, the element its reader already
  read, so the power, voltage, and loss fields survive a round trip; only a
  dcline cost curve is dropped, and storage. The reader takes the power flow
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
- **PyPSA CSV folders** are canonicalized directory outputs rather than byte
  exact text conversions. Covered: static buses, generators, loads, lines (ohms on
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
- **DOE GO Challenge 3 JSON** is a grid exchange format “for Challenge 3 and
  beyond,” not the name of one calculation type. PowerIO recognizes its
  Challenge 3 input/problem data file and returns `AcScucInstance`. The
  instance holds the declared time points and durations, initial commitment
  and dispatch, time varying bounds, costs and reserves, energy windows,
  contingencies, and one shared `BalancedNetwork`. Call `instance.network()`
  to access that network. One directory or memory `Source` containing both the
  input/problem data file and its matching output/solution data file returns
  `AcScucSolution`; an output/solution data file alone is rejected because it
  has neither component definitions nor the time axis. Problem data is parse
  only. A complete `AcScucSolution` emits the official output/solution data
  file, including bus voltage, shunt step, device commitment, dispatch and
  reserves, AC line status, transformer tap, phase shift and status, and DC
  line terminal power fields. The pinned GO-3 model validates PowerIO's small
  problem and output documents and all D1/D2/D3 input/problem data files.
  C3DataUtilities reports no data, ignored, or solution errors for those
  documents. The older pinned GO-3 model and D1/D2/D3 files still carry
  `network.bus.con_loss_factor`; version 1.1.1 of the data format removed that
  field. PowerIO keeps the original source and reports one bounded diagnostic
  instead of treating it as an electrical network or AC SCUC field. Optional
  bus location labels and incomplete coordinate pairs remain in `Bus.extras`;
  optional consumer descriptions, voltage setpoints, and nameplate capacities
  remain in `Load.extras`. Each reports
  `READ.GOC3.OPTIONAL_FIELD_UNTYPED`. A producer description has no generator
  metadata field and reports `READ.GOC3.RETAINED_SOURCE_ONLY`.
- **Surge JSON** reads and writes the versioned `surge-json` network document.
  The reader maps buses, loads, fixed shunts, branches, generators, storage, and
  HVDC links into `BalancedNetwork`, retains the original source for same format echo,
  and warns about source sections that stay only in the retained document. The
  writer emits a canonical Surge network body for the supported power flow core;
  richer MATPOWER generator capability or ramp columns and unsupported cost
  shapes are reported in the emission diagnostics. An HVDC link carries the
  terminal voltage setpoints, the reactive limits, and the loss model on its
  converter terminals; a Surge link states no terminal reactive flow, no cost
  curve, and no received power (the reader derives it from the setpoint and the
  loss model), so those are warned. A link that states converter or control
  detail beyond the neutral converter this writer emits (firing angles,
  converter transformer taps, commutation impedance, a DC voltage schedule) is
  warned on the way in.
- **DeepMind OPFData JSON** reads one raw JSON document from a FullTop or N-1
  release into the balanced transmission model. Topology, limits, loads,
  shunts, and quadratic costs come
  from `grid`; solved bus voltages, generator dispatch, and branch flows come
  from `solution`. Powers and ratings are converted from per unit, angles from
  radians, link indices from zero based to one based, and flow columns from
  `[pt, qt, pf, qf]` into the canonical terminal order. Original bus IDs/names,
  areas/zones, and frequency are absent, and solver initial generator values are
  distinct from the solved snapshot, so cross-format reads report those facts.
  The adapter is driven by feature widths and the row/link counts in each file,
  not by a case name registry or expected element counts. The same path therefore
  covers all published grid families (14 through 13,659 buses) and both FullTop
  and N-1 examples; generator and branch outages are represented by absent rows
  and links and are validated against that example's solution topology.
  The published releases are derived from PGLib-OPF cases, but the reader does
  not use PGLib case names or a case registry. A document from another source is
  accepted when it follows the same object layout, feature column order, units,
  and link rules. The paper's Appendix A is the published format definition and
  the PyTorch Geometric loader is the executable reference. No separate JSON
  Schema or format version marker is published, so documents that depart from that
  layout are rejected by the reader's shape and topology checks.
  Unrecognized object fields remain in the retained source and produce a
  projection warning instead of being silently discarded or making same format
  echo impossible.
  The raw source echoes byte exactly; there is no canonical writer, `.pt` cache
  reader, archive reader, downloader, or batch directory API.
- **PowerFactory DGS** reads DIgSILENT PowerFactory DGS V5 ASCII exports
  (token `dgs`, alias `powerfactory`, extension `.dgs`). The reader decodes
  the `$$Class;attr(type)` tables, including vector and matrix attributes and
  decimal commas, into an object index with the `fold_id` hierarchy, then
  resolves every `StaCubic` cubicle to its `ElmTerm` terminal, element, side,
  and `StaSwitch` state. The mapping follows the PowSybl PowerFactory
  converter, which reads the same export definitions. Per unit values use a
  100 MVA base and each terminal's `uknom`, because DGS carries no system
  base; the nominal frequency comes from `ElmNet.frnom`.
  The export decides its network family. Elements described by sequence data
  alone (three phase `ElmTerm`, `TypLne` with `nlnph = 3` and no neutral,
  three phase loads, machines, and transformers) parse to `BalancedNetwork`.
  An export stating a terminal phase technology other than ABC, a `TypLne`
  neutral or non three phase conductor count, per phase load demand
  (`i_sym = 1`) or a single or two phase load or generator technology, a
  single phase transformer, a phase specific cubicle connection, a conductor
  type class (`TypCon`, `TypGeo`, `TypCabsys`), or a phase domain matrix on a
  line or tower type parses through `powerio::parse` to
  `MulticonductorNetwork`, with a `READ.DGS.ROUTED_MULTICONDUCTOR` remark
  naming the attributes that decided it; the transmission crate's own parser
  refuses such an export with guidance to the facade. An export with no
  `ElmTerm` fails with `READ.DGS.ROUTE_UNDECIDED`.
  Balanced mapping: one bus per group of AC terminals joined by a closed, in
  service `ElmCoup`, with the group's smallest object id as the bus id, the
  lead terminal's `loc_name` as the bus name, solved `m:u`/`m:phiu` as the
  voltage when exported, and `GPSlat`/`GPSlon` as the location. Lines take
  `TypLne` per kilometer values times `dline`, divided across `nlnum`
  parallel circuits, with charging from `bline` or `2 pi f cline` and
  conductance from `gline` or `bline tline`; a line on an `ElmTow` tower
  takes the diagonal of the tower's positive sequence circuit matrices and
  the dropped inter circuit coupling is reported. `ElmZpu` common impedances
  are per unit on `Sn`. Two winding transformers run from the HV cubicle
  (side 0) to the LV cubicle with the leakage impedance from `uktr`/`pcutr`
  referred to the LV bus base, the magnetizing admittance from `curmg`/`pfe`
  on the LV terminal, the tap from `utrn_h`/`utrn_l` against the bus nominal
  voltages and the current `nntap` step (or the row of an explicit `mTaps`
  table), the phase shift from `phitr` per step, and `ntrcn`/`imldc`/`t2ldc`/
  `p_rem`/`ElmTapctrl` as the automatic control; the vector group clock
  `nt2ag` stays in `extras` and is reported. Three winding transformers keep
  typed pairwise impedances on the smaller rating of each pair, per winding
  taps, and the controlled winding's regulation. Loads resolve the
  `mode_inp` pair (`PQ`, `SP`, `SQ`, `PC`, `QC`, `SC`) and `scale0`;
  `ElmLodmv` generation becomes a fixed generator. Machines carry `pgini`,
  `qgini`, `usetp`, `iv_mode`/`av_mode`, `Pmin_uc`/`Pmax_uc`, and reactive
  limits from the element or type per `iqtype`, an `IntQlim` capability
  curve collapsing to its widest range; `ElmXnet` external grids are
  generators whose `bustp` selects PQ, PV, or the slack. The declared slack
  (`ip_ctrl = 1` or `bustp = SL`) is the reference bus; without one the
  largest voltage regulating machine's bus is chosen and reported. Shunts
  read `shtype` 1 (R-L from `rrea`/`xrea` or `ushnm`/`qcapn`) and 2 (C from
  `gparac`/`bcap`) per section times `ncapa`, and a switchable shunt keeps
  its voltage band. A DC island behind exactly two `ElmVsc` converters is one
  HVDC record from the first converter's AC bus to the second's, with the
  parallel DC line resistances combined; any other DC island is dropped and
  reported. Load type voltage dependence, unmapped element classes, dangling
  references, and elements open at one cubicle only are reported under
  `READ.DGS.FIELD_UNMAPPED`, `READ.DGS.CLASS_UNMAPPED`,
  `READ.DGS.REFERENCE_DROPPED`, and `READ.DGS.VALUE_COLLAPSED`; every finding
  carries the byte span of its source row.
  Multiconductor mapping: terminal conductor sets follow the `ElmTerm`
  phase technology with the neutral named `4`, line codes take phase domain
  matrices from the sequence values (`(z0 + 2 z1) / 3` on the diagonal and
  `(z0 - z1) / 3` off it) and the neutral from `rnline`/`xnline`/`rpnline`/
  `xpnline`, cubicle `it2p1..3` values map element phases to terminal phases,
  loads keep their per phase `plinir/s/t` demand or split a balanced value
  across their phases, `ElmXnet` is a voltage source, and transformers keep
  their winding connections and taps. Three winding and two winding
  transformers, shunts, machines, and switches map as in OpenDSS units.
  The format is read only: an unchanged module emits its retained source
  and every other target goes through that target's writer. An encrypted
  `.pfd` project file fails with `READ.DGS.ENCRYPTED_PROJECT`, which names the
  DGS export as the way in. Validation: `tests/data/powerfactory/ieee14.dgs`
  reproduces `case14.m` impedances, charging, taps, loads, and the capacitor
  bank; the PowSybl gate compares element counts and the IEEE 14 values with
  PyPowSybl on every DGS fixture in the pinned PowSybl Core checkout.
- **GridFM Parquet datasets** (the `gridfm` feature) parse to a scenario set
  of balanced networks over one shared element identity map: lossy, but each
  scenario recovers everything a power flow needs. That is bus
  types/voltages/limits, nodal load and shunt totals, generator dispatch and
  bounds, branch `r/x/b/tap/shift/rate_a`/angle limits, and `baseMVA`; it
  cannot recover original bus ids (synthesized `1..n`), per element
  load/shunt granularity (folded one synthetic element per bus),
  piecewise/cubic generator costs (read as none), or HVDC/storage. Because
  the writer stores the *effective* tap, a unity ratio, zero shift
  transformer in the source reads back as a line (the power flow is
  identical). The losses are the module's diagnostics. The same direction
  writer is documented in the
  [top level README](https://github.com/eigenergy/powerio#gridfm).

## Missing generator costs

PSS/E `.raw` files carry no generator cost curves. Converting a PSS/E case to
MATPOWER writes `mpc.gen` and omits `mpc.gencost` with a warning; powerio does
not invent zero costs. A workflow that needs costs must pick an explicit policy:

```sh
powerio convert case.raw --from psse --to matpower --missing-gen-cost zero -o case.m
powerio dcopf case.m -o out --missing-gen-cost quadratic --default-gen-cost 0.01,2.0,0.0
powerio gridfm case.raw --from psse -o out --missing-gen-cost zero
```

- `preserve`: leave missing costs absent (default for conversion and GridFM export);
- `require`: fail on an in-service generator without cost (default for DC OPF export);
- `zero`: fill missing rows with a MATPOWER polynomial cost `[0, 0, 0]`;
- `quadratic`: fill missing rows with `--default-gen-cost C2,C1,C0`.

`--gen-cost-csv` overrides costs by generator row before the missing-cost policy
runs. The header is `gen_index,bus,c2,c1,c0,startup,shutdown`: `gen_index` is
zero based in the current generator table, `bus` must match that generator's bus
id (catching stale tables after reordering), and `startup`/`shutdown` default to
zero. GridFM stores `cp0/cp1/cp2` columns; missing or unsupported costs still
write zero columns, and the manifest separates `missing_cost_gens`,
`unsupported_cost_gens`, `zeroed_cost_gens`, and `synthesized_gen_costs`.
