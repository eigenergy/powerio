# BMOPF 0.2.0 design notes

Research record for the BMOPF 0.2.0 work of the PowerIO 0.11 release. It states
what the IEEE PES Task Force on Benchmarking Multiconductor OPF has accepted,
what remains proposal, where the archived 0.2.0 material lives, what PowerIO
already implements, and what the release plan still needs.

## The four Task Force repositories

`distribution-system-opt` holds four public repositories.

- `math-and-data-model-specifications` is the specification: SI units, the
  mathematical model, and the data dictionary, as Documenter.jl Markdown under
  `docs/src/`. Its README carries a work in progress warning: the content "has
  not yet been validated for consistency with the source specification", and
  "until then there is no released version". `docs/src/changelog.md` holds one
  empty `## Unreleased` section, and no `v*` tag exists. `docs/src/spec/index.md`
  is headed "Status: prototype".
- `bmopf-resources` holds the draft material: the specification PDF
  `bmopf_math_and_data_model_specification_v0.2.2.pdf`, the draft JSON schema
  `draft_schema_and_networks/draft_bmopf_schema.json`, two example networks, a
  `jschon` validation example, and the PowerUp 2026 workshop tutorial. Both
  readme files state the material is draft and not a Task Force output: the
  schema "is **NOT** the final schema", and `example_ieee13.json` "is NOT
  intended to represent the IEEE 13 Bus Test network in the BMOPF format".
- `task-force-meetings` holds one file, `PES GM 26 Slides.pdf`. It records no
  decision text, so it settles nothing about what is accepted.
- `dsopt-schema` is empty: size 0, an unborn `main`, and a description reading
  "Repository to host the JSON schemas released by the BMOPF taskforce". Both
  the specification repository's `README.md` and its `docs/src/index.md` name
  it as the canonical home of the data schema.

## What BMOPF 0.1.0 covers

The accepted model is the intersection of the specification prose and the
`0.1.0` draft schema. Element classes: `bus`, `line`, `linecode`, `switch`,
`load`, `generator`, `shunt`, `capacitor`, `voltage_source`, and `transformer`
in exactly four subtypes (`single_phase`, `center_tap`, `wye_delta`,
`delta_wye`). Document level: `name`, `meta`, `terminal_conventions`, and one
free-form `extras` object.

Conventions the specification fixes:

- Every quantity is SI and absolute. Cost rate is the one deliberate exception,
  in currency per kilowatt-hour. Per unit is out of scope.
- A complex quantity is a pair of real fields. A matrix is stored row first
  with a one-based underscore key, so entry `A_kj` is the field `A_k_j`. Matrix
  dimension `n` satisfies `1 <= n <= 4` for a four-wire three-phase network.
- A vector is a JSON array ordered to match the element's terminal map or phase
  order, as each component page states.
- An absent constraint field means the constraint is unbounded. An absent
  parameter field means zero.
- `terminal_conventions`, when present, is the authority on terminal roles and
  is matched exactly including case. Absent, roles are inferred: `n` or `N` is
  the neutral, `"4"` is the neutral when the bus terminal set is exactly
  `{1, 2, 3, 4}`, and every other terminal is a phase. The name `"g"` is
  reserved for the common ground.
- `meta.version` versions the dataset, never the schema. `meta.$schema` is the
  URI of the schema the file validates against.
- Exactly one voltage source. Snapshot solves only, with no inter-temporal
  coupling. The objective is linear active power dispatch cost, summed over
  generators, the voltage source, and inverter-based resources, from each
  element's per phase `cost` array; a scalar `cost` is rejected. A feasibility
  relaxation replaces it by the squared magnitude of nodal slack currents.

The specification states these present limitations: one reference bus, snapshot
solves, no transformer saturation, core loss, or tap changing, and no quadratic
cost term.

## What 0.2.0 must add

The specification names none of the following as accepted material, so every
item below is a normative change under the Task Force review tier of
`docs/src/contributing.md`, which requires an issue or discussion reaching rough
consensus before a pull request.

- Inverter-based resources. `docs/src/spec/objective.md` sums the objective over
  "generators, the voltage source, IBRs" and calls the injection sign
  convention uniform across the three, but no IBR element is defined anywhere,
  and `cost` exists only on `generator`. The objective is therefore written
  against elements the data model does not have.
- Control profiles. Not mentioned in the specification at all.
- Regulators. Not mentioned in the specification at all. Tap changing is named
  only as absent: "the turns ratio is fixed by nameplate voltages".
- General transformers. Only the four fixed subtypes are specified. The assets
  directory carries unreferenced figures `nwinding.svg` and `nwinding_loss.svg`,
  which is artwork for material no page states.
- DC networks. Not mentioned in the specification at all.
- Line geometry and conductor data. `docs/src/spec/metadata.md` validates
  `meta.frequency` against "`line_geometry`/linecode derivation frequency" and
  `docs/src/spec/notation.md` names "wire data, line geometries" as libraries
  referenced by string id, but neither object is defined. Both references
  resolve to nothing.
- Time series. Not mentioned, and ruled out indirectly by "Snapshot solves" and
  by the objective page's instruction to multiply each snapshot rate by its
  duration before summing.

Beside the new classes, 0.2.0 must settle four things the accepted 0.1.0 text
leaves open, because a converter cannot be written against them:

1. Array dimensions. `docs/src/spec/bus.md` states `v_min` and `v_max` are "one
   per phase terminal", while the 0.1.0 schema describes them as "one entry per
   bus terminal". The published `example_ieee13.json` states three entries on a
   four-terminal bus, so the schema text is the error. `vpp_min` and `vpp_max`
   are per phase pair with no stated pair order. Load arrays are per sub-load,
   which is a different count from per phase for a delta load.
2. Conductor ordering. The specification fixes the matrix key spelling but not
   which conductor index `i` names on each class. A linecode shared by several
   lines has no ordering of its own unless every referencing line states a
   terminal map of the same width and order.
3. The transformer current limit. The data model table calls `i_max_from` and
   `i_max_to` "Per-conductor current limits"; the constraint in part 5 of
   `docs/src/spec/transformer.md` calls the same fields "Per-winding
   current-magnitude circles, per conductor k and side sigma" and bounds
   `I_x_sigma_k`, the winding current. For a delta winding the coil current is
   not the terminal conductor current, and for `center_tap` the to side has
   three terminals but two legs with a dependent centre-tap current, so neither
   the meaning nor the array length is determined.
4. Ideal equipment equations. The specification writes the ideal winding pair,
   the ideal closed and open switch, and the free slack current of the source as
   equations, but the schema states none of them, so a document that omits every
   impedance field has no stated behaviour.

## Where the archived 0.2.0 proposal lives

In the git history of `distribution-system-opt/bmopf-resources`. The draft
schema carried the full extension set until 2026-07-21, when two commits removed
it:

- `afda547` "remove regulators, taps, ibrs, dc"
- `9c9b26f` "remove time series"

The last commit holding the complete set is `2e0b1cb` (2026-07-18), in both
`draft_schema_and_networks/draft_bmopf_schema.json` and the since-deleted
`schema/bmopf.json`. That document declares the tables `ibr`, `control_profile`,
`wire_data`, `line_geometry`, `dc_bus`, `dc_branch`, `dc_grounding`, `dc_load`,
`dc_source`, and `time_series`, the transformer subtypes `n_winding`,
`single_phase_autotransformer`, and `open_delta_regulator`, and the definitions
`inverter_topology_type`, `prime_mover_type`, `dc_pole_type`, `dc_control_type`,
`voltage_reference_type`, `q_ref_type`, `p_ref_type`, `q_unit_type`,
`p_unit_type`, `regulator_type`, `open_delta_connection_type`,
`transformer_winding`, and the two regulator objects.

Its own text marks each extension as unsettled: the IBR table says "scope for
the July release is under Task Force review", the DC tables say "under Task
Force review", `wire_data` and `line_geometry` say "geometry-to-impedance
derivation is outside the core Task Force scope for the July release", and
`time_series` says "time-series support is under review".

Three parts of `2e0b1cb` are superseded by the later accepted 0.1.0 text and
must not be restored as they stand: its `capacitor` states `q_rated` as a per
phase array where 0.1.0 states one whole-bank scalar with a line-to-line
`v_nom`; its `load.model` enumeration is lower case where 0.1.0 is upper case;
and its `three_phase_transformer` keeps the legacy lumped `r_series`/`x_series`
beside the per side fields.

## What PowerIO already implements

`powerio-dist/src/bmopf/` reads and writes the format. `read.rs` types the whole
0.1.0 model plus, already, the `ibr` and `control_profile` tables, the
`n_winding` transformer, and the `single_phase_autotransformer` and
`open_delta_regulator` regulator subtypes. `write.rs` emits a strictly 0.1.0
document: because 0.1.0 sets `additionalProperties: false` and dropped those
tables, the writer relocates them under `extras` and the transformer taps,
winding neutral impedance, and no-load admittance to
`extras.transformer.<subtype>.<name>`, and reports each relocation. The reader
folds the same overlay back.

So these parts of the release plan are already done:

- Parsing to `MulticonductorNetwork`, with `McAcOpfInstance` construction in
  `powerio-prob` and solved values in `McAcOpfSolution`. `McAcOpfInstance`
  carries `Objective` and `MulticonductorActiveConstraints`;
  `powerio::to_mc_ac_opf_instance` builds it through
  `McAcOpfInstance::from_network`, which selects
  `Objective::active_power_dispatch_cost` and leaves every stated limit active.
  That is exactly the specification's default objective and its rule that
  constraints are active for the elements present.
- Typed IBR, control profile, regulator, and n-winding transformer reading and
  writing.
- The diagnostic families `READ.BMOPF.*` and `EMIT.BMOPF.*` are registered in
  `powerio-dist/src/diagnostics.rs`.

These parts are not done:

- No 0.2.0 output profile and no profile selection. `BMOPF_SCHEMA_VERSION` is
  the constant `"0.1.0"` and `BMOPF_SCHEMA_ID` still names
  `frederikgeth/bmopf-report`, which is not where the schema lives.
- No profile detection from `meta.$schema`, and no diagnostic when it is
  absent. The published `example_enwl_n1_f2.json` states
  `"$schema": "https://github.com/frederikgeth/bmopf-report/draft_schema_and_networks"`,
  which is neither a schema `$id` nor version bearing, so detection has to
  report what it cannot resolve.
- `powerio-dist/src/model.rs` has no type for a DC bus, DC branch, DC
  grounding, DC load, DC source, line geometry, wire data, or time series.
  Those classes read into `untyped` and re-emit under `extras`.
- `DistIbr` types no `cost`, no `dc_bus`/`dc_terminal_map`/`dc_control`
  fields, no converter filter impedance, and no `grid_forming` flag.
  `VoltageSource` types no `cost`, `p_min`, or `p_max`; the writer carries
  `voltage_source.cost` as a passthrough extra.
- The fixture provenance in `tests/data/dist/README.md` is wrong on every
  count. It names the repository `frederikgeth/bmopf-report` and the commit
  `3a786e16c761981951f1deab72fd28624577dda6`, claims three corrections were
  applied to the schema, and records three hashes that no vendored file has.
  Measured, the three vendored files are identical to
  `distribution-system-opt/bmopf-resources` at
  `f2e368470a5012dd264d1f5a2f867867fb926615` with nothing applied on top:
  `draft_bmopf_schema.json`
  `a74f4d2be151e4b250a47a1730445301c093572fce8de609e9af15b76c67ef73`,
  `example_ieee13.json`
  `48707ea839c20032c88df715587e50d097637cd0cc8a17b2d213d4591eea8bc7`,
  `example_enwl_n1_f2.json`
  `24d2c054b70b5e09d179f785cb09ff90cddbf73c859756154e9eb604782b69f1`.
- No normative field mapping document. Closed issue #414 asked for one, along
  with a comparison against BMOPFTools covering admittance stamps,
  constraints, objective terms, conductor order, and regulator behaviour.

## What is blocked

- The BMOPFTools comparison. BMOPFTools.jl is the Task Force toolchain that
  generated `example_ieee13.json` (`meta.case_study_generator` names
  `BMOPFTools.jl` version `0.1.0`). It is not published in the
  `distribution-system-opt` organization, is not on this machine, and is not a
  registered Julia package here. Without it, a mapping document can compare
  against the two published example networks and the specification equations,
  which is what this work does, but it cannot check admittance stamps,
  constraint activation, or regulator behaviour against the reference
  implementation. That needs the toolchain, from the Task Force.
- Every 0.2.0 element class beyond what 0.1.0 accepts. The specification's own
  review tiers make each of them a normative change needing Task Force
  ratification, and no meeting record in `task-force-meetings` states one. A
  schema proposal is the correct step; treating any of it as accepted is not.
- The ENWL example's licence. The upstream data is CC BY 4.0 by its CSIRO Data
  Collection release and the derivative carries the same licence, which is
  recordable now. The IEEE 13 node fixture is distributed under the OpenDSS
  licence in `tests/data/dist/opendss/License.txt`, and `bmopf-resources` says
  the Task Force may replace that example; replacing it with a licensed
  synthetic case needs a case that does not exist yet, so the OpenDSS notice
  stays for now.
