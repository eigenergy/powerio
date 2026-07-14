# Changelog

## 0.7.1

- The SCOPF Julia wire conversion is structural (#252): every struct reaching
  the wire classifies its fields (index, renamed, value) through an exhaustive
  destructure, so a new field fails to compile until classified and a value
  field reusing an index name is never renumbered. Wire output is unchanged.
- GOC3 parses once per read (#250): the reader hands its parsed document
  forward as `Parsed::document`, and the package boundary derives the
  operating point series from it instead of reparsing the retained text.
- `DcOpfInstance` carries the constant cost term `c0` (generator and nodal
  data), and the DC OPF bundle writes `c0.mtx`/`c0_gen.mtx` (bundle schema
  0.3.0).
- Balanced formats harvest and emit coordinates (#183): PowerWorld aux
  `Latitude:1`/`Longitude:1`, pandapower bus `geo` Point strings, PyPSA
  `buses.csv` x/y, each in both directions. Writers with no geometry concept
  warn that locations were dropped.
- The standalone geographic document (#184): `GeoLayer`/`ElementKey` with
  tolerant reads (headerless buscoords CSV, aliased CSV/JSON records, GeoJSON)
  and canonical GeoJSON writes (`.geo.json`, the `powerio_geo` foreign
  member); `DisplayFormat::GeoJson`/`DisplayData::Geo`;
  `Network::geo_layer()`/`apply_geo_layer()` with multiconductor equivalents
  in `powerio-pkg`; `Branch.route`/`DistLine.route` polyline routing (payload
  schemas 1.2.0); `.pwd` promotion (`geo_layer_from_pwd`,
  `apply_substation_points`, `pwd_mercator_to_lonlat`); and
  `powerio geo extract | apply | convert` in the CLI.
- C ABI and Python bindings for the new surfaces (#249, #185), all additive
  (`PIO_ABI_VERSION` stays 4): `pio_acopf_from_network` / `pio_acopf_to_json`
  / `pio_acopf_instance_free`, `pio_geo_parse` / `pio_geo_extract` /
  `pio_geo_apply`, and `pio_dist_geo_extract` / `pio_dist_geo_apply`. Python
  gains `parse_geo`, `Network.geo_layer()/apply_geo_layer()/acopf_instance()`,
  and the distribution equivalents.
- DeepMind OPFData reader (#258): `SourceFormat::DeepMindOpfDataJson` reads
  one raw OPFData JSON document as a solved snapshot, echoes the same format
  byte for byte, and converts through the standard surface (CLI, C ABI,
  Python).

## 0.7.0

- Add `powerio-prob` for complete numerical problem instances (#238). Its
  default build is matrix free; the `matrix` feature adds sparse projections,
  DC OPF bundle output.
- Move DC OPF instance types and bundle output out of `powerio-matrix`.
- Keep solver formulations and KKT operators outside `powerio-prob`.
- Demote `powerio-json` from the public case format surface (#229). It leaves
  the CLI format help and the generated format tables; `pio_parse_str(...,
  "powerio-json", ...)` and `pio_to_format(..., "powerio-json", ...)` keep
  working as ABI v4 compatibility aliases, and `pio_to_json` / `pio_from_json`
  are the documented balanced model JSON API.
- `NetworkPackage::from_balanced` is format neutral. Source adapters, such as
  GOC3 operating point extraction, run only for parsed reader input through
  `from_parsed_balanced`.
- Building a SCOPF instance from text requires an explicit source format.
- Add AC OPF problem instances (#248): `powerio-prob` gains `AcOpfInstance`
  and `build_ac_opf_instance`, carrying pi model branch data with separate tap
  and shift, per terminal charging, bus shunts, active and reactive demand,
  voltage bands, generator PQ bounds, and quadratic cost including the
  constant term (`GenCost::quadratic_with_constant`). Relaxations such as SOC
  forms consume the same instance. Matrix free; C ABI and Python exposure is
  #249.
- `powerio-prob` first publish review fixes: reserve membership sets now
  assign zone indices from the same document order as the reserve rows
  (sorted order previously crossed `n_p`/`n_q` between the two tables and
  diverged from `src/goc3.jl` past nine zones); a GOC3 branch with
  `r = x = 0` is rejected by name instead of writing NaN into the wire
  rows; a missing `device_type` defaults to `producer`, the balanced
  reader's rule; the AC instance folds self-loop branch admittance into
  the bus shunt vectors, matching `build_ybus`; both instance builders
  reject a non-positive base MVA before scaling; the DC OPF bundle
  directory name stays confined to the output directory, and the bundle
  manifest reports the powerio core version (`powerio::VERSION`, new).
  SCOPF row structs and `DcOpfOutputs` are `#[non_exhaustive]`.
- DSS reader: `linecode=` now sets a line's conductor count the way the
  engine's FetchLineCode does, the later of `phases=`/`linecode=` winning.
  A 4-wire line without an explicit `phases=` keeps its neutral instead of
  truncating the terminal map to 3 against a 4x4 linecode
  (frederikgeth/BMOPFTools.jl#332).

## 0.6.3

- Arrow matrix axes (#234): the C ABI Arrow export gains a table catalog and
  dense matrix axis maps. `pio_to_arrow` exposes `matrix_bus` and
  `matrix_branch` axis tables alongside the `ybus`, `incidence`, `bprime`, and
  `bdoubleprime` COO tables, which carry `powerio.row_axis` / `powerio.col_axis`
  schema metadata so a consumer maps dense matrix rows and incidence columns
  back to source bus and branch rows. Matrix rows are labeled with the
  `matrix_bus` axis, which stays correct when 3-winding transformer star-point
  lowering expands the bus set past the handle bus order.
- FDPF matrices (#234): `bprime` / `bdoubleprime` follow MATPOWER `makeB`
  semantics, with self-loop handling and asymmetric Matrix Market writes pinned
  by Rust, C ABI, and Python coverage.
- Summary JSON (#234): the C ABI exposes balanced (`pio_summary_json`) and
  distribution summary JSON, so a binding can render network summaries without
  materializing the full model payload.
- Matrix Arrow export (#234): the numeric matrix export path fills Julia-owned
  buffers directly for the copy-free fast path.

## 0.6.2

- Normalization (#210): angle bound clamping now keeps every repaired branch
  interval ordered. One sided intervals wholly outside the supported window are
  widened to the configured pad instead of producing `angmin > angmax`; Rust,
  C ABI, and Python normalize option coverage pin the behavior.
- Binding coverage (#185): the already shipped study block and distribution
  graph projection now have C ABI and Python accessors. The remaining geo
  binding symbols stay in the v0.6.3 follow through.
- BMOPF diagnostics (#219): distribution conversions now carry structured
  diagnostics alongside warning strings, and transformer export losses expose
  stable `EMIT.BMOPF.*` codes for downstream tests and capability checks.
- BMOPF transformer fidelity (#214, #215, #216, #217): OpenDSS fixed
  transformer taps, center tap convention fields, delta_wye leakage referral,
  and n_winding `delta_roll` now export directly in BMOPF form with regression
  coverage against schema valid output and unaffected fixture byte identity.
- BMOPF source fidelity (#218): per phase OpenDSS voltage sources on the same
  bus merge into one BMOPF `voltage_source` when their phase angles are
  coherent; ambiguous, bounded, priced, or conflicting source banks stay split
  with warnings.
- Distribution capabilities (#213): the C ABI `dist` feature exposes
  `pio_dist_capabilities_json`, reporting the six BMOPF fidelity flags that
  PowerIO.jl and downstream tools can probe at runtime.
- Geographic fields (#180): balanced and distribution models now share typed
  `GeoMeta` / `Location` JSON shapes. `Network.geo`, `Bus.location`,
  `DistNetwork.geo`, and `DistBus.location` are optional and omitted when
  absent. OpenDSS Buscoords and BMOPF longitude/latitude sideloads promote into
  typed bus locations; OpenDSS writes a Buscoords sidecar when locations are
  present, while BMOPF longitude/latitude output remains opt in and only emits
  declared geographic coordinates.
- `.pio.json` model JSON: the balanced and multiconductor payload schema
  versions move from `1.0.0` to `1.1.0` for the additive geographic fields.
  The package metadata schema, C ABI version, and Python package surface stay
  in the 0.6 compatibility band.
- JSON strategy: `.pio.json` docs now state that it is PowerIO's compiled
  artifact, not a case format; payload schemas are for validating model JSON
  inside `.pio.json` documents; `powerio-json` remains supported, is deprecated
  for CLI file handoffs, and is no longer shown in the PR conversion matrix.

## 0.6.1

- CI: added wasm32 coverage for the core Rust crates (#186), external BMOPF
  JSON Schema validation for emitted distribution documents (#192), and
  generated `.pio.json` / model JSON schema drift checks (#178).
- Distribution fidelity (#197): OpenDSS and BMOPF writers preserve transformer
  winding voltage bases, no load admittance, tap settings, neutral impedances,
  and multi winding transformer structure. Roundtripped OpenDSS decks now run
  through a solve oracle that checks voltage agreement, load voltage model
  behavior, and neutral return handling.
- Distribution DER mapping (#197): typed IBR and control profile data now round
  trips through OpenDSS `PVSystem` / `Generator` / `InvControl` and BMOPF
  `ibr` / `control_profile` records, with warnings for unsupported control
  details.
- `.pio.json` documents (#181, #193): added the study block for replayable
  balanced model edits, materialization helpers, deterministic uid stamping,
  and balanced reader warnings as structured `.pio.json` diagnostics.
- Normalization (#188): added an opt-in angle bound clamp pass with Rust, C ABI,
  and Python entry points; existing normalization behavior is unchanged.
- Distribution graph projection (#182): added a bus and terminal graph view for
  `DistNetwork`, including transformers, open switches, and terminal metadata.
- Matrix bindings (#190): added Arrow C ABI matrix exports as COO triplet tables
  for Ybus, incidence, MATPOWER Bp, and MATPOWER Bpp, with C and Python golden
  coverage.
- Sensitivities (#8): added sparse and iterative PTDF/LODF export paths with
  drop tolerance metadata, while retaining the dense path as the small case
  oracle.
- Documentation (#191): standardized released docs, READMEs, and crate metadata
  around `.pio.json` document, model JSON, and metadata terminology.
  `powerio-py` continues to inherit the workspace version; no separate Python
  version bump is needed.

## 0.6.0

- Breaking (#175): `ElementRef.row` is `Option<usize>`, the honest form of the
  0.5.1 wire semantics. `None` addresses by identity alone (refs built with
  `by_source_uid`); the private wire-presence shim (`wire_row()`) is gone, and
  `row` itself says whether the wire carried one. The `.pio.json` wire format
  is unchanged. The other break collected in #175, keyed-object addressing for
  multiconductor operating point updates, needs design and moves to the 1.0
  window (#196).
- C ABI: the package payload extraction inverses land as additive symbols (no
  ABI version change; probe the symbols like the other feature surfaces):
  `pio_package_to_balanced_network` and `pio_package_to_multiconductor_network`
  materialize an owned network handle from a parsed `.pio.json` package handle,
  the inverses of the `pio_package_from_*` constructors. A handle built from a
  payload retains no source text, so a same-format write is a fresh
  serialization rather than a byte-exact echo; the multiconductor payload's
  parse warnings ride along.
- C ABI: `pio_to_json` / `pio_from_json` are the function form of the balanced
  model JSON (byte identical to the `powerio-json` writer); the format token
  remains as a compatibility alias for file based workflows. This is the
  additive half of #194; retiring the token waits for 1.0.
- C ABI: `pio_dist_to_json` / `pio_dist_from_json` serialize a distribution
  handle to its model JSON and back in one call each: the same object a
  `.pio.json` document carries under `model.multiconductor_network`, without
  the surrounding document. Bindings materialize element tables with this
  instead of building a throwaway package; it is not a case format (the
  converter, CLI, and inference do not know it), so BMOPF JSON remains the
  distribution JSON exchanged with other tools.
- C ABI: `pio_classify_str` classifies in-memory JSON by the same top level
  markers the transmission parser's `.json` sniffing uses, and recognizes
  `.pio.json` envelopes: `transmission:<format>`, `distribution:<format>`,
  `package`, `ambiguous`, or `unknown`, size-then-fill. Bindings can route a
  bare `.json` before choosing a parser instead of matching error text.
- The JSON classifier reports a `.pio.json` envelope as its own outcome
  (`routing::JsonClass`), so every consumer handles it: the CLI, the Python
  readers, and the Python `classify_json_text` now name the package surface
  for an envelope instead of a generic cannot-infer error (or, for the Python
  string reader, a MATPOWER syntax error). Envelope detection requires
  `model_kind` to be `balanced` or `multiconductor`, so a case document
  carrying those key names with other values still classifies as a case, and
  classification parses the document once.
- Directed errors at the transmission boundary: a `.dss` path, a distribution
  `from` token (`dss`/`pmd`/`bmopf`), and a `.pio.json` envelope handed to the
  balanced parser now name the surface that reads them instead of a generic
  unknown-format message.

## 0.5.1

- `.pio.json` payload schema declared (#173): new optional envelope fields
  `payload_schema` and `payload_schema_version` name and version the IR payload
  schema id per model kind (`pio-payload-balanced/1`,
  `pio-payload-multiconductor/1`, both `1.0.0`), independent of the envelope
  `schema_version` (now `0.1.1`). A reader rejects a foreign payload major;
  packages without the fields (0.5.0 and earlier) read unchanged. The JSON
  shape of `model` is untouched.
- Payload row identity: balanced IR elements gained `uid: Option<String>`
  (serde additive). The GOC3 parser keeps source uids on buses, devices,
  branches, and dc lines; package construction synthesizes `{table}:{row}` uids
  for the rest, so every powerio built payload row has a stable identity.
- Operating point updates resolve by identity: `ElementRef.source_uid` is
  authoritative when the payload table carries uids — a present `row` must
  agree with the resolved row, unknown or duplicated identities are rejected
  (at materialization and by `pio_package_validate` via the
  `VALIDATE.PACKAGE.OPERATING_IDENTITY` pass), and `row` may be omitted on the
  wire (`ElementRef::by_source_uid`). Tables without uids keep the pre-0.5.1
  row-only semantics, so existing packages materialize as before. Provenance
  cleanup paths now come from the resolved row, not the wire row.
- Python: network table dicts expose `uid`; unknown identities raise
  `ValueError` from `Package.materialize_operating_point`. C ABI: no signature
  changes; materialization reports identity failures through `errbuf`.
- `powerio-pkg`: `ElementRef.row` is meaningful only when
  `ElementRef::wire_row()` is `Some`; refs built by `by_source_uid` serialize
  without `row`.

## 0.5.0

- `powerio-pkg`: `NetworkPackage` is the one package type name (`CompilerPackage`
  is gone); the Julia binding already leads with `NetworkPackage`. The `.pio.json`
  format is unchanged.
- Python API: the seven module level `package_*` functions are replaced by the
  `powerio.Package` handle class, which parses the envelope once and exposes
  `model_kind`, `operating_points()`, `materialize_operating_point()`,
  `as_balanced()`/`as_multiconductor()`, `validate()`, `validation()`,
  `diagnostics()`, and the multiconductor to balanced preflight and lowering.
- `.pio.json` operating points: the per point `label` and `duration_hours` fields
  are gone; `time_axis.labels` and `time_axis.duration_hours` (indexed by
  `points[].index`) are the one source of truth. Readers ignore the old fields.
- Transmission formats: added GOC3 JSON input and Surge JSON read and write paths.
  GOC3 packages lift source time series into `.pio.json` `operating_points`,
  and package APIs can materialize one point into a static package.
- GOC3 reader fixes: branches with `additional_shunt` keep the line charging
  (`b/2` per terminal added to the extra shunts, per the GO Challenge 3
  formulation); `ta_lb`/`ta_ub` map to an `ActiveFlow` transformer control
  range instead of fabricating `angmin`/`angmax` bus angle limits; producers
  and consumers honor `initial_status.on_status` like every other record type;
  object form section keys sort under a total order (mixed numeric and non
  numeric keys no longer risk a sort panic).
- `powerio-pkg`: GOC3 operating point extraction now consumes the parser's own
  document walking (`device_rows`, `section`, `cost_at` shared through a
  bridge), so update row indices match the payload by construction, including
  devices without a `uid`. A failed extraction attaches a
  `READ.GOC3.OPERATING_POINTS_DROPPED` diagnostic instead of silently
  producing a static only package. Materialized packages clear `package_id`
  (the parent id lives in `origin.parent_package_id`).
- PSS/E `.raw`: revision aware record layouts for v34/v35 transformer winding
  lines (twelve ratings, `NODE`), v35 generator records (`NREG`, `BASLOD`),
  and v35 switched shunts (`NREG`, per block status triples), on both read and
  write; the 2W/3W transformer split accepts float form `K` fields. The
  `case14_v34.raw`/`case14_v35.raw` fixtures are regenerated in the genuine
  layouts.
- PSLF `.epc` writer: parallel loads and shunts on one bus get distinct ids
  (`extras["id"]` preferred, positional fallback); the reader captures load,
  shunt, and SVD ids into `extras["id"]` so they survive cross format writes.
- PowerWorld `.pwb`: the table location search runs under a work budget, so a
  crafted file fails with a read error instead of pinning a core for hours.
- Surge JSON writer warns when named branch rating sets are dropped, like every
  other lossy writer.
- Writing a read only format (`goc3-json`) returns the new
  `Error::WriteUnsupported` instead of a misleading `UnknownFormat`.
- C ABI: the panic guard now covers index construction in the parse entry
  points; `pio_package_validate` documents its exclusive access requirement
  (the one non `const` entry point) and the header preamble scopes the
  concurrent read guarantee accordingly; `PioDistNetwork` gains the same
  compile time `Send + Sync` assertion as the other handles.
- `SourceFormat::name()` is the one source format name mapping; the package,
  CLI, and Python copies are gone.

## 0.4.0

- `powerio-pkg`: `.pio.json` reads now enforce the envelope compatibility rule:
  same major `schema_version` values load, while incompatible major versions
  fail before payload use. The mdBook schema page documents the rule.
- `powerio-pkg`: balanced package output now emits source maps for stable bus,
  load, shunt, branch, and generator fields; validation diagnostics attach the
  matching source reference where a map exists. Format folded fields use
  mapping kinds such as `split`, and defaulted fields are not marked as exact
  source fields.
- Converter tests now compare stable per element values across writable legacy
  formats, not only counts and totals. PSLF export now warns when transformer
  charging admittance is dropped.
- `powerio-dist` BMOPF: OpenDSS fixed P/Q generators now emit as BMOPF
  `generator.*` entries with pinned P/Q bounds instead of negative `load.*`
  entries. The old negative load warning is gone; generators without source
  costs keep the existing cost 0 warning.
- Python API: removed the one release `powerio.Case` and
  `powerio.dist.DistCase` compatibility aliases. Use `powerio.Network` /
  `powerio.BalancedNetwork` and `powerio.dist.MulticonductorNetwork` /
  `powerio.dist.DistNetwork`.
- No C ABI rename in this migration slice: `PIO_ABI_VERSION` stays 4 and
  `PIO_DIST_ABI_VERSION` stays 1.

## 0.3.3

- MCP server: **unified the advertised tool surface** to semantic verbs:
  `convert`, `save`, `summary`, `parse`, `normalize`, `matrix`, and `display`.
  The tools route transmission cases, distribution cases, PyPSA CSV folders, and
  gridfm datasets by format. Distribution `parse` returns canonical `bmopf-json`
  as its serial transport; transmission `parse` returns `powerio-json`.
  `summary` now has one canonical JSON schema across MCP and the CLI's new
  `powerio summary` command. Gridfm is a format behind `parse`/`save`, not its
  own MCP tool. PowerWorld `.pwd` display files parse through `display`, leaving
  room for a future open display format without renaming the tool. Older case,
  matrix, and PyPSA helper names stay as direct Python compatibility callables
  for one release, but they are no longer advertised as MCP tools.
- Python API: restored the undocumented `powerio.Case = Network` alias for one
  release, but left it out of `__all__` and docs; remove it in 0.4.0. The
  **experimental** distribution surface now uses `powerio.dist.DistNetwork` as
  the primary name to match the native `DistNetwork` hub type, while the
  exported `powerio.dist.DistCase = DistNetwork` alias stays for one release.
  `powerio.dist` is gated on the draft BMOPF schema (`PIO_DIST_ABI_VERSION` = 1)
  and not yet under the stability guarantee.
- No C ABI change: `PIO_ABI_VERSION` stays 4 and `PIO_DIST_ABI_VERSION` stays 1,
  and the matrix builders are unchanged. The native extension's internal pyclass
  names changed (`PyCase → PyNetwork`, `_DistCase → _DistNetwork`) so `repr()`
  now renders the public `Network(...)` / `DistNetwork(...)` form directly; a
  rebuilt wheel is required.

## 0.3.2

- `powerio-dist` OpenDSS: grounding reactors written from a bus terminal to the
  same bus's node 0 now type as shunts in BMOPF instead of staying untyped.
  Impedance form reactors use the equivalent admittance matrix, so neutral
  grounding resistors survive DSS to BMOPF conversion.
- `powerio-dist` OpenDSS: three phase and single phase line to line delta
  capacitor and reactor banks now type as shunt admittance matrices, including
  off diagonal branch terms, instead of being dropped as untyped objects. Two
  phase open delta banks stay untyped with a warning.
- DSS writing now regenerates conductance bearing shunts as grounding reactors
  and preserves delta shunts as `conn=delta` where the typed model identifies
  them. The PMD shunt writer labels delta banks `DELTA` instead of `WYE`.
- Shunt conversion hardening: a `kv` that squares to zero, a non-finite stashed
  token, and a reactor `r`/`x` that fails to evaluate no longer leak infinities,
  literal `NaN`/`inf`, or a silent zero into the output; each keeps the object
  untyped or drops it with a warning. The BMOPF writer no longer warns that a
  delta shunt's `conn` marker was dropped.
- No core or distribution C ABI break; `PIO_ABI_VERSION` stays 4 and
  `PIO_DIST_ABI_VERSION` stays 1.

## 0.3.1

- Parser warnings: PSS/E and PowerWorld `.aux` parse warnings now surface
  through `Parsed::warnings` and the C ABI's `pio_warnings` path instead of
  living only in docs or writer warnings.
- PSS/E: hardened record tokenization and continuation handling. Slash
  characters inside quoted fields are no longer treated as comments; incomplete
  transformer and two-terminal DC continuation records now error clearly instead
  of consuming section terminators; transformer records with non-unit `CW`/`CZ`
  now warn that impedance and turns values were read without conversion.
- PSS/E: load ZIP components and v34/v35 load tail fields are retained in extras
  and replayed on write. If typed load `p/q` no longer match retained
  `PL/QL/IP/IQ/YP/YQ`, the writer emits typed constant power and reports the
  stale extras instead of replaying wrong source components.
- PSS/E: quoted IDs, names, and HVDC names are sanitized before duplicate ID
  allocation, so collisions created by replacing quotes or `/` are handled
  deterministically and reported in conversion warnings.
- Normalization: generator cost per-unit scaling now dispatches through explicit
  cost models, and slack bus selection ignores `NaN` generator `pmax` values
  when choosing among candidate reference buses.
- PSLF and PowerWorld AUX tokenization: quoted `/` and `//` text is kept as data
  rather than treated as continuation or comments. PowerWorld `.aux` now reports
  unmodeled `DATA` blocks as parse warnings while retaining source text for
  same-format writeback.
- `powerio-dist` OpenDSS: quoted comment markers are preserved in lexer values,
  indented block comments are skipped, capacitor and reactor kvar shunts share
  validation, reactors with kvar/kv map to typed shunts with negative
  susceptance, and invalid shunt forms stay untyped with explicit warnings.
- `powerio-dist` BMOPF: fixed OpenDSS generators with fixed P/Q setpoints now
  encode as negative BMOPF loads with warnings. The vendored draft schema was
  refreshed for multi-digit matrix keys, corrected `$id`, and nonnegative
  switch `i_max`, so 10-conductor linecode output validates without the old
  schema warning.
- C distribution ABI v1 (`PIO_DIST_ABI_VERSION` 1): direct `pio_dist_*` callers
  get a separate version check; the supported one-shot conversion order is
  `(input, from, to, ...)`.
- C ABI tests now reject the old target-before-source conversion order for both
  `pio_convert_*` and `pio_dist_convert_*`, including the compiled C smoke test
  against `powerio.h`.
- C ABI hardening: unit tests pin every public `PIO_*` macro, opaque typedef,
  and `pio_*` prototype in `powerio.h`; Cargo now checks Rust source/header
  symbol parity; CI builds the no-default core ABI plus the release
  `arrow,gridfm,dist` feature smoke test and C++ header/link sanity checks.
- No core C ABI break; `PIO_ABI_VERSION` stays 4. No existing Rust or Python
  API was removed or reordered.

## 0.3.0

- Distribution systems: new `powerio-dist` crate for multi conductor unbalanced
  networks. Reads OpenDSS and the PowerModelsDistribution engineering JSON, and
  reads/writes the IEEE BMOPF Taskforce JSON (schema v0.0.1). First crates.io
  release of `powerio-dist`.
- PSS/E: read and write support for v34 and v35 alongside v33.
- GE PSLF: an `.epc` writer, with better interoperability between PSLF and PSS/E.
- Transformers with three or more windings.
- C ABI v4 (`PIO_ABI_VERSION` 4): a smaller canonical surface designed so future
  changes stay additive. Breaking ABI change in this release.
- Memory safety hardening across the readers.

## 0.2.4

- PSLF `.epc`: read support for GE PSLF power flow cases, including `.epc`
  extension inference and `pslf` / `epc` input aliases. The reader is read only
  and keeps source text plus warnings for sections outside `Network`.
- PowerWorld `.pwb`: expanded binary reader coverage across older and newer
  header constants, with stricter record probes, companion format parity checks,
  and clearer rejection of unsupported vintages.
- PowerWorld `.pwd`: display parsing keeps the separate display API path and
  retains the malformed input invariant: corrupt or truncated display files
  return a structured error or a parsed display, not a panic.
- No C ABI break; `PIO_ABI_VERSION` stays 3.

## 0.2.3

- Normalization: `Network::to_normalized` preserves source bus ids instead of
  renumbering surviving buses to dense 1-based ids. Dense row mapping remains
  available through `IndexedNetwork` and the C ABI table order.

## 0.2.2

- Display API: `parse_display_file` / `parse_display_bytes` read display
  artifacts separately from network cases. PowerWorld `.pwd` returns
  `DisplayData::PowerWorld(PwdDisplay)` in Rust and
  `DisplayData("powerworld", PwdDisplay(...))` in Python. `parse_file`
  remains Network only and points `.pwd` callers at the display API.
- PowerWorld AUX: name keyed complete case exports can resolve
  `BusName_NomVolt` labels for loads, shunts, generators, and branches.
- PSS/E: the reader accepts comment headers, system wide records before
  `BEGIN BUS DATA`, and v34 named branch records without misclassifying
  long v33 branch rows.
- MCP: add dedicated tools for PyPSA CSV folders and gridfm Parquet datasets.
- DC sensitivities: PTDF/LODF fall back to dense Gaussian elimination for
  invertible indefinite grounded Laplacians.

## 0.2.1

Hardening fixes only; no API or ABI change (`PIO_ABI_VERSION` stays 3).

- MATPOWER: a crafted `gencost` NCOST (e.g. `1e20`) overflowed the row
  width arithmetic and panicked on every build profile, a denial of
  service on untrusted input through the Rust API and the CLI. The width
  now saturates and the row is rejected as a `ShortRow` parse error.
  Found by malformed input fuzzing.
- C ABI: error and warning messages were clipped at a raw byte count,
  which could split a multibyte UTF-8 character and hand the caller an
  invalid string. Truncation now lands on a character boundary.
- PowerWorld `.pwd`: the reader's byte accessors return `Option` instead
  of indexing, so an out of range offset from a corrupt file rejects the
  record instead of panicking. A corruption sweep test pins the
  invariant; the differential oracle tests pass unchanged.
- `powerio.h`: a doc comment contained a literal `*/` that terminated
  the generated block comment, so compiling with `-DPIO_GRIDFM` against
  the shipped 0.2.0 header failed with `unknown type name 'raw'`.

## 0.2.0

- PowerWorld `.pwb` binary reader (#95, #102, #105): read only, covering
  June 2016 through 2022 era exports under header constants 425, 483, 508,
  537, 550, and 551, parity tested against same vintage `.aux`/`.RAW`/`.m`
  siblings up to the 6717 bus Texas7k. Unsupported writer vintages are
  rejected with the format constant named.
- pandapower JSON converter (#106): read and write `pandapowerNet` JSON.
  Written trafo parameters reproduce the source Y_bus exactly through
  pandapower 3.x's transformer model, ZIP load columns go out in both the
  <= 3.1 and >= 3.2 namings, and CI validates the converter against
  pandapower itself over the vendored fixtures.
- PyPSA CSV folder converter (#106): read and write the static network
  CSV folder, CI validated against PyPSA over the vendored fixtures.
  Folders parse through `parse_file(..., "pypsa-csv")`, auto-detected for
  a directory holding `network.csv`; the CLI takes `--from pypsa-csv` and
  `--to pypsa-csv -o <dir>`.
- Read fidelity channel (#106): `parse_file`/`parse_str` return
  `Parsed { network, warnings }`, so what a reader cannot carry is
  itemized instead of dropped silently. Python exposes
  `Network.read_warnings` and the MCP tools report it; the C ABI gains
  `pio_parse_warnings` and `pio_write_pypsa_csv_folder` (additive, ABI
  version stays 3).
- PowerWorld `.pwd` display reader (#102): substation diagram coordinates,
  matched 1-1 against the aux substations on every probed save with a same
  vintage aux (the v19 resave matches 1248/1250 against the published
  case, a vintage skew).
- Full `.aux` fidelity (#95): all three field naming generations through
  Simulator 21+, validated against the vendored ACTIVSg200 set.
- The PowerWorld guide records the decode evidence, mapping notes, and the
  coverage matrix the corpus tests assert.

## 0.1.1

- File extension detection is case-insensitive (#97, #101): `parse_file`
  accepts `.RAW`/`.M`/`.JSON`/`.AUX` and any mixed case alongside the
  lowercase forms, and the CLI batch discovery and TUI file browser find
  such files too. Reported by @jd-foster.
- MCP server error hardening (#93): an unreadable input file surfaces as
  the documented ValueError shape instead of a raw `PermissionError`, with
  defensive guards on the JSON load and matrix dispatch paths.

## 0.1.0

- gridfm read path (#70): `read_gridfm_dataset` / `read_gridfm_scenarios` /
  `gridfm_base_case` in `powerio-matrix`, `pio_read_gridfm` /
  `pio_gridfm_scenario_ids` in the C ABI behind `--features gridfm`, and
  `powerio.read_gridfm` / `read_gridfm_scenarios` in Python. Release tarballs
  now build the C ABI with the gridfm feature, so the symbols ship to the
  Julia bindings.
- `convert_str` (#88): in-memory conversion through the hub in Rust and
  Python; the MCP server's inline conversion no longer stages temp files.
  Closes #66.
- The MCP server grows from two tools to eight (#90): parse and normalization
  helpers emit the JSON transport, the matrix helper returns nine sparse kinds
  in COO form, the dense table export returns copied tables, and the save
  helper writes converted cases to disk; conversion and summary helpers are
  unchanged.
- Docs (#92): Pages landing page with the released/development split, guide
  links, and the logo; the crate homepage points at the docs site; release
  drafts carry the CHANGELOG section instead of a bare title.

## 0.0.1

First release.

- Parsers and writers for MATPOWER `.m`, PSS/E RAW, PowerWorld AUX,
  PowerModels JSON, and egret JSON; byte-exact same-format round trips,
  maximal-fidelity conversion between formats.
- `Network`, the one canonical model, with `to_normalized` deriving a
  per-unit / radian / filtered / reindexed form.
- C ABI (`powerio-capi`, ABI version 3): parse, query, convert, JSON
  transport, and Arrow C Data Interface export behind `--features arrow`;
  cbindgen-generated header, version handshake, panic-safe boundary.
- Python bindings (`pip install powerio`) with `matrix`, `graph`, and
  `gridfm` extras, plus an MCP convert/validate server.
- `powerio-matrix`: admittance and Laplacian builders over the parsed
  tables; gridfm Parquet export behind `--features gridfm`.
- `powerio-cli`: convert and validate from the shell.

The C ABI history (versions 1 through 3) is tracked in
`powerio-capi/README.md`.
