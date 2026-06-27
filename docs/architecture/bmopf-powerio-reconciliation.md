# BMOPF ↔ PowerIO reconciliation

A field-level map between the IEEE PES Task Force benchmark distribution OPF
format ("BMOPF JSON") and PowerIO's `MulticonductorNetwork`
(`powerio_dist::DistNetwork`). BMOPF is a frontend and an emission target, not
the internal IR (see ADR 0002). This table is the basis for the dist reader and
writer and for the diagnostics that Phase 4 adds.

Two reference points are used, and they disagree in places:

- the **normative draft**: the LaTeX spec (`DataModel.tex`, `MathModel.tex`,
  `ListOfSymbols.tex`, `Appendix.tex`, and `Tables/`), title "Mathematical Model
  and Data Model for Up-To-Four-Wire Distribution System OPF";
- the **executable interpretation**: BMOPFTools.jl (`schema.jl`,
  `parse_bmopf.jl`, `write_bmopf.jl`, `migrate.jl`) plus the bundled
  `draft_bmopf_schema.json`.

Where they conflict, this document records both and prefers a diagnostic over an
irreversible choice. The version marker is `meta.$schema` (a URI); there is no
top-level `version`. BMOPFTools maps the URI
`https://raw.githubusercontent.com/frederikgeth/bmopf-report/main/schema/bmopf.json`
to spec tag `:draft`. PowerIO's writer emits `meta.$schema` and
`meta.generator`.

Legend: ✓ carried; ~ carried with a representational gap; ✗ dropped or
unsupported (with the proposed diagnostic).

## Root object and metadata

| BMOPF key | MulticonductorNetwork | status | notes |
|---|---|---|---|
| `name` | `name` | ✓ | |
| `meta` | `extras` (read drops it; writer regenerates) | ~ | reader does not retain `meta`; writer emits `{$schema, generator:{tool,version}}` only. `authors`, `sources`, `license`, `created`, `modified`, `title`, `description` are not retained. |
| `bus` | `buses` | ✓ | id-keyed object → `Vec<DistBus>` keyed by `id` |
| `line` | `lines` | ✓ | |
| `linecode` | `linecodes` | ✓ | |
| `voltage_source` | `sources` | ✓ | BMOPF allows exactly one; the model allows a `Vec` and warns beyond the first |
| `generator` | `generators` | ✓ | |
| `load` | `loads` | ✓ | |
| `shunt` | `shunts` | ✓ | |
| `switch` | `switches` | ✓ | |
| `transformer.{subtype}` | `transformers` | ~ | nested by subtype; PowerIO stores the subtype in `extras["bmopf_subtype"]` on a generic winding model (see transformers below) |
| `capacitor` (BMOPFTools extension) | `shunts` | ~ | BMOPFTools-only; lowered to a shunt admittance |
| `inverter`, `control_profile`, `time_series` (extensions) | `untyped` / `extras` | ✗ | not in the draft schema; preserved untyped, not modeled. `EMIT.BMOPF.UNSUPPORTED_ELEMENT` |

The metadata not modeled by `MulticonductorNetwork` is the kind of producer and
source information the compiler package carries instead, in `producer`,
`origin`, and `sources`. The right long-term home for `meta.authors`/`sources`
is the package envelope, not the electrical payload.

## Buses, terminals, neutral, grounding

| BMOPF field | DistBus field | status | notes |
|---|---|---|---|
| `terminal_names` (`N_i`) | `terminals` | ✓ | ordered strings; `["a","b","c","n"]`, OpenDSS `["1","2","3","4"]`, etc. |
| `perfectly_grounded_terminals` (`G_i`) | `grounded` | ✓ | entries must be members of `terminals` |
| `v_min` / `v_max` (`U_i^{min/max}`, 4×1 per-terminal) | `v_min` / `v_max` (`Option<f64>` scalar) | ~ | **shape mismatch.** The draft schema declares scalars; BMOPFTools requires per-terminal arrays and rejects scalars. PowerIO stores a scalar. `PARTNER.BMOPF.V_BOUND_SHAPE` |
| `vpn_min` / `vpn_max` | `vpn_min` / `vpn_max` | ✓ | phase-to-neutral, `Option<Vec<f64>>` |
| `vpp_min` / `vpp_max` | `vpp_min` / `vpp_max` | ✓ | phase-to-phase (delta) |
| `vsym_min` / `vsym_max` | `vsym_min` / `vsym_max` | ~ | draft names `vsym_*`; BMOPFTools uses `vpos_*` plus `vneg_max`/`vzero_max` extensions. `PARTNER.BMOPF.VSYM_VS_VPOS` |
| `neutral_terminal` (BMOPFTools proposal) | — (heuristic) | ✗ | no field; the reader infers the neutral by name or position |
| `longitude` / `latitude` (extension) | `extras` | ~ | preserved in extras, not typed |

Neutral and grounding: the spec models ground as a 0 V reference plate. PowerIO
materializes an implicit ground connection as an explicit grounded neutral
terminal on the bus (named `4` on a three-phase bus), matching the PMD and public
BMOPF examples. Impedance grounding is modeled as a shunt (a single-phase shunt
with `terminal_map = ["n"]`), in both the spec and PowerIO.

Bus-pair and intra-bus voltage **angle** bounds exist in the math model
(`Θ_{ij}`, `Θ_i^{Y}`) but have no DataModel JSON field, and PowerIO has no field
for them either. Both omit; nothing to reconcile.

## Lines and linecodes

| BMOPF field | field | status | notes |
|---|---|---|---|
| line `length` (m) | `DistLine::length` | ✓ | |
| line `linecode` | `DistLine::linecode` | ✓ | name reference |
| line `bus_from` / `bus_to` | `bus_from` / `bus_to` | ✓ | |
| line `terminal_map_from` / `terminal_map_to` | same | ✓ | |
| linecode `R_series_i_j` (Ω/m) | `DistLineCode::r_series` (`Mat`) | ✓ | row-first, 1-indexed; full or upper-triangular accepted |
| linecode `X_series_i_j` | `x_series` | ✓ | |
| linecode `G_from_i_j` / `G_to_i_j` (S/m) | `g_from` / `g_to` | ✓ | Π-model, half shunt each end |
| linecode `B_from_i_j` / `B_to_i_j` | `b_from` / `b_to` | ✓ | |
| linecode `i_max` (A) / `s_max` (VA) | `DistLineCode::i_max` / `s_max` | ✓ | per conductor; the line may also carry `i_max`/`s_max` in BMOPF, copied from the linecode |

Impedance and admittance are complex `n×n` matrices, `1 ≤ n ≤ 4`, stored as
separate real and imaginary `Mat = Vec<Vec<f64>>`. There is no complex type;
`r`/`g` is the real part, `x`/`b` the imaginary part.

## Switches

| BMOPF field | DistSwitch field | status |
|---|---|---|
| `bus_from` / `bus_to` | `bus_from` / `bus_to` | ✓ |
| `open_switch` (bool) | `open` | ✓ |
| `terminal_map_from` / `terminal_map_to` | same | ✓ |
| `i_max` (A) | `i_max` | ✓ |

Ideal lossless open/closed conducting section. Closed: equal voltages, balanced
current; open: voltages unlinked, zero current.

## Voltage sources

| BMOPF field | VoltageSource field | status |
|---|---|---|
| `v_magnitude` (V) | `v_magnitude` | ✓ |
| `v_angle` (rad) | `v_angle` | ✓ |
| `bus` | `bus` | ✓ |
| `terminal_map` | `terminal_map` | ✓ |

Exactly one source is permitted by the spec; voltage bounds are not applied at
the reference bus. PowerIO allows several and warns past the first
(`EMIT.BMOPF.MULTIPLE_SOURCES`). The spec's uncosted source makes the objective
degenerate; BMOPFTools synthesizes a costed slack generator. PowerIO does not
synthesize a slack; that is a benchmark-augmentation concern owned by
BMOPFTools.

## Loads and generators

| BMOPF field | field | status | notes |
|---|---|---|---|
| load `p_nom` (W) / `q_nom` (var) | `DistLoad::p_nom` / `q_nom` | ✓ | per phase |
| load `configuration` (`WYE`/`DELTA`/`SINGLE_PHASE`) | `DistLoad::configuration` | ✓ | `Configuration::{Wye,Delta,SinglePhase}` |
| load `bus` / `terminal_map` | same | ✓ | |
| load `model` (constant_power/current/impedance/zip/exponential) | `DistLoad::voltage_model` (`DistLoadVoltageModel`) | ✓ | PR #143 added the typed `voltage_model`: `ConstantPower` / `ConstantCurrent` / `ConstantImpedance` / `Zip` / `Exponential`, with `v_nom` and ZIP/exponential coefficients. This gap (previously dropped to `extras`) is now closed |
| generator `bus` / `terminal_map` / `configuration` | same | ✓ | spec supports generator `WYE` only; delta/single-phase planned |
| generator `p_min`/`p_max`/`q_min`/`q_max` (3×1) | `Option<Vec<f64>>` | ✓ | `p_nom`/`q_nom` synthesized when bounds are pinned (a==b) |
| generator `cost` (3×1 per-phase, $/kWh) | `DistGenerator::cost` (`Option<f64>` scalar) | ~ | **shape mismatch.** BMOPFTools requires a per-conductor array; the reader collapses it to the first entry and warns if non-uniform; the writer broadcasts the scalar back per phase. `EMIT.BMOPF.GEN_COST_PER_PHASE_COLLAPSED` |
| generator `i_max` / `s_max` (BMOPFTools tolerated) | — | ✗ | no generator current/apparent-power field in PowerIO |

The objective is the only one defined: linear per-phase generation cost,
`min Σ_g C_g^T Re(S_g)`. A fixed real generator should be modeled as a negative
load; PowerIO's writer does exactly that for a fixed P/Q generator with no cost
from a non-BMOPF source.

## Shunts and capacitors

| BMOPF field | DistShunt field | status | notes |
|---|---|---|---|
| `G_i_j` (S) | `g` (`Mat`) | ✓ | up to 4×4 |
| `B_i_j` (S) | `b` (`Mat`) | ✓ | |
| `bus` / `terminal_map` | same | ✓ | |
| `capacitor` (BMOPFTools): `q_rated`, `v_rated`, `configuration` | `shunts` (lowered, `B = q/v²`) | ~ | no typed capacitor; OpenDSS `capacitor`/`reactor` both lower to `DistShunt` (delta banks tagged `extras["conn"]="delta"`) |

The spec has no separate capacitor element; capacitors are shunts. BMOPFTools
adds a typed `capacitor` as an extension. Note the spec's delta-capacitor
admittance `M^Δ` is asymmetric (a known erratum, BMOPFTools item 8); the correct
reciprocal form is `Y·[[2,-1,-1],[-1,2,-1],[-1,-1,2]]`. PowerIO should emit the
reciprocal form and may flag the discrepancy with `PARTNER.BMOPF.DELTA_CAP_ADMITTANCE`.

## Transformers

The normative draft defines four subtypes as distinct objects under
`transformer.{single_phase, wye_delta, delta_wye, center_tap}`. BMOPFTools adds
`single_phase_autotransformer`, `open_delta_regulator`, and `n_winding`.
PowerIO's `DistTransformer` is a generic OpenDSS-style winding model
(`windings: Vec<Winding>`, `xsc_pct`, `phases`) with the BMOPF subtype carried in
`extras["bmopf_subtype"]`; the writer's `classify` step selects the output
subtype.

| BMOPF subtype | terminal-map arity (from/to) | PowerIO read/write | status |
|---|---|---|---|
| `single_phase` | 2 / 2 | `Winding` pair; `v_ref_from/to` → `Winding::v_ref`; `s_rating`; `r/x_series_from/to` → percent | ✓ |
| `center_tap` | 2 / 3 | secondary expanded into two half-windings on read; collapsed on write (the `xht`/`xlt` split is dropped, with a warning) | ~ |
| `wye_delta` | 4 / 3 | `Winding` with `WindingConn::{Wye,Delta}`; `r_series`/`x_series` (or per-winding) | ✓ |
| `delta_wye` | 3 / 4 | mirror of wye_delta | ✓ |
| `single_phase_autotransformer` (ext) | 2 / 2 | read as a single-phase pair with a warning | ✗ `EMIT.BMOPF.UNSUPPORTED_TRANSFORMER_SUBTYPE` |
| `open_delta_regulator` (ext) | 4 / 4 | unsupported | ✗ same |
| `n_winding` (ext) | windings list | unsupported (PowerIO has up to 3-winding via `xsc_pct`) | ✗ same |

Transformer impedance stays in percent/per-unit form internally (`r_pct` per
winding, `xsc_pct` between pairs) and converts to ohms on the wye side at BMOPF
emission. BMOPFTools migrates the lumped three-phase `r_series`/`x_series` to
per-winding `r_series_from`/`r_series_to` (setting `_to = 0`); PowerIO's reader
accepts both forms. The no-load branch (`g_no_load`/`b_no_load`) is a BMOPFTools
addition with no normative placement; PowerIO does not model it.

## Units and angle conventions

Both are SI internally: volts, watts, vars, VA, ohms, siemens, meters, radians;
cost in $/kWh (the only non-SI quantity). No per-value unit fields in JSON; units
are fixed by the spec. Matrices are row-first and 1-indexed in BMOPF JSON. This
matches PowerIO's dist model directly; no unit conversion is needed on the
balanced-vs-distribution boundary because the two models are never silently
mixed.

## Time series, profiles, results

The normative model is a single-snapshot OPF: no time series, profiles, or result
schema. BMOPFTools adds `time_series`, `control_profile`, and `inverter`
extensions, and a separate result document (`result_io.jl`, a plain dict in SI
units, with `NaN`/`Inf` for infeasible). PowerIO models none of these in the
static IR. Solver results belong in a future `ResultPackage` linked to a model
package by id, not in `MulticonductorNetwork`. Time series and DER/control data
belong in their own package families, not the static-grid IR.

## What BMOPF can represent that PowerIO drops

- `meta` authorship/source/license/timestamps (regenerated, not retained).
- per-phase generator cost (collapsed to a scalar on read).
- per-terminal `v_min`/`v_max` vectors (stored as a scalar pair).
- ~~non-constant-power load models and ZIP/exponential coefficients~~ — now
  carried by the typed `DistLoad::voltage_model` (PR #143); no longer dropped.
- BMOPFTools extensions: typed `capacitor`, `inverter`, `control_profile`,
  `time_series`, the three extra transformer subtypes, bus coordinates,
  `neutral_terminal`, `vneg`/`vzero`/`vn` bounds, per-element `status`.

## What PowerIO preserves that BMOPF cannot represent

- retained source text for a byte-exact same-format echo (not a BMOPF concept).
- `defaulted` provenance (which fields came from a format default).
- `untyped` objects, OpenDSS `commands` and `options` (verbs like `solve`,
  `set mode=...`).
- arbitrary source-specific `extras` per element.

These are exactly the provenance and fidelity facts the compiler package is built
to carry: retained source becomes `Origin::File { retained_source: true }`,
`defaulted` becomes source-map entries with `mapping_kind = defaulted`, and the
free-form warnings become structured diagnostics.

## Normative draft vs BMOPFTools: open discrepancies

| topic | draft | BMOPFTools | PowerIO stance |
|---|---|---|---|
| `v_min`/`v_max` shape | scalar (`nonnegative_number`) | per-phase array, rejects scalar | scalar today; diagnose on emit (`PARTNER.BMOPF.V_BOUND_SHAPE`) |
| symmetrical-component bounds | `vsym_*` | `vpos_*` + `vneg_max`/`vzero_max` | read both names; prefer `vsym_*` on emit |
| transformer subtypes | 4 | 7 (adds autotransformer, open-delta regulator, n-winding) | support the 4; diagnose the rest |
| delta capacitor admittance | asymmetric `M^Δ` (erratum) | reciprocal `(M^Δ)^T M^Δ` | emit reciprocal form |
| slack/reference | uncosted `voltage_source` | synthesized costed slack generator | keep the source uncosted; slack synthesis is BMOPFTools' job |
| neutral identification | no marker | heuristic (name `n`/`N`, else terminal `4`) | same heuristic; honor an explicit marker if the spec adds one |
| matrix storage | ambiguous full vs triangular | accepts both, reports triangular | accept both |

These are documented rather than resolved unilaterally. Each is a candidate for a
`PARTNER.BMOPF.*` diagnostic so a consumer can see exactly which interpretation a
package used.
