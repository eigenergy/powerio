# Changelog

## 0.9.0

The API and C ABI that 1.0.0 ships. Everything here exists so a later change can be additive, so this release takes the breaks: C ABI 5, one version number across every document powerio authors, a DC susceptance that reads the whole series impedance, and the deprecated names removed. Read the headings below before upgrading; each one changes what a working consumer sees.

**C ABI 5.** Fourteen symbols, zero renamed. `PIO_ABI_VERSION` is 5 and bindings gate on it by equality, so a binding built against 4 must move with it. `PIO_DIST_ABI_VERSION` stays 1, frozen rather than removed: thirteen distribution call sites in PowerIO.jl resolve it, and deleting it would make a library that fully supports distribution refuse every distribution call.

**Conversion warnings come back through an out pointer.** `pio_to_format`, `pio_convert_file`, `pio_convert_str`, `pio_write_dir` and their three `pio_dist_*` twins took a caller `warnbuf` and truncated into it silently — the length that would have said anything was lost was discarded, and the header advertised 256 bytes as sufficient. They take `char **out_warnings` instead: NULL when the conversion lost nothing, otherwise an owned string freed with `pio_string_free`, written before the call does any work so a stale value from an earlier call cannot be read as this one's. `pio_warnings` and `pio_dist_warnings` keep their caller buffer; they use the size-then-fill idiom and cannot truncate.

**Every per-bus and per-branch extractor reports the star-lowered space.** An in-service 3-winding transformer gains a bus before the dense extractors run. Through ABI 4, `pio_n_buses` and `pio_bus_ids` counted the unexpanded case table while `pio_bus_demand`, `pio_bus_shunt` and `pio_n_islands` counted the expansion, so a buffer sized from `pio_n_buses` read short and its trailing entries had no id. `pio_n_buses`, `pio_bus_ids`, `pio_n_branches`, `pio_branches` and `pio_branch_charging` all report the lowered space now. `length(bus_ids) == n_buses` is the migration test, and the closure a caller actually depends on holds: every branch endpoint is a bus the API reports, and every bus has an incident branch.

**Five ABI-visible JSON documents changed shape while their symbols kept their signatures.** `pio_schema_versions_json` dropped four keys. `pio_dist_capabilities_json`, `pio_arrow_catalog_json` and `pio_scopf_to_json` renamed `schema_version` to `powerio_version`. `pio_summary_json` gained `topology.n_buses` and `topology.n_branches`; its `counts` block stays the case file's own inventory, so a 3-winding transformer is one row there rather than the bus and three branches it lowers to. The Arrow metadata key `powerio.schema_version` is `powerio.version`. A binding built against ABI 4 passes the handshake and then reads `null` for keys it mirrors, which is why the integer moved.

**Deleted and added.** The three `pio_acopf_*` symbols are gone: no C consumer, and `acopf` appears nowhere in PowerIO.jl. Re-cut them additively when a consumer exists and can say what shape it needs. `pio_build_info` returns one document — version, ABI integer, compiled features, foreign schema versions, and the `ErrorCategory` tokens — shaped after `curl_version_info`. `pio_parse_bytes` takes `(const uint8_t *, size_t)` and every `pio_parse_str` format name plus `pwb`.

**`parse_bytes` on all four surfaces.** Rust, C, Python and Julia. It is the only in-memory route to the PowerWorld `.pwb` binary reader, and it opens nothing, which is what makes it the right entry point for input you do not control.

**The DC susceptance reads the whole series impedance, and states its sign once.** `DcConvention` offered `b = 1/x` and MATPOWER's `b = 1/(x·τ)`; neither reads the branch resistance, so a case with a real r/x ratio had no convention describing it and every consumer computed one by hand. `SeriesImpedance` is `x/(r² + x²)` with phase shift injections and no tap scaling, and it is the new default — a caller that passed no convention gets different numbers, and the gap grows with r/x, so it is small on transmission cases and large on distribution ones. `branch_susceptance` returns a **positive** Laplacian edge weight. PowerModels and tellegen write the negative one; that is their convention. A test holds all three variants to one sign, so a caller that negates once cannot get a sign flipped matrix from the choice of variant. `PaperPure` is now `ReactanceOnly`, which names the formula rather than a paper, and it is not deprecated: `b = 1/x` is the textbook DC linearization, so reproducing a published result needs it exactly as written. The CLI takes `--convention series` and Python takes `"series"`.

**One version number in every document powerio authors.** powerio stamped fifteen; nine were invented for its own artifacts, six of those were write only, and two claimed 1.x stability inside a 0.x library. Every document powerio authors now states `powerio_version`, the release that wrote it, and `powerio::version` holds the acceptance rule: a document loads when it shares this build's lineage, the major once it reaches 1 and the major and minor pair while the major is 0. `.pio.json`, the SCOPF document, the DC OPF bundle manifest, the geo layer sidecar, the Arrow catalog and table metadata, and every summary document change their version field — **regenerate stored artifacts**. A `.pio.json` from 0.8.x or earlier states no version at all; it deserializes to the empty string rather than defaulting, so the gate stays closed and the reader names the release that wrote it. A foreign format keeps its own version: powerio implements case formats and authors none, so pandapower's `3.0.0` and the BMOPF `$schema` are reproduced, never set. The served JSON Schema path follows the same lineage the reader accepts, so `pio-package/0.9` is the current document and the retired identifiers stay published.

**The deprecated names are removed rather than carried to 1.0.** `powerio/src/lib.rs` re-exported only `BalancedNetwork`, so the aliases were reachable at `powerio::network::Network` and nowhere else and every existing `use powerio::Network;` got a compile error rather than the promised warning. Since the whole set was scheduled for deletion at 1.0 anyway, 0.9.0 takes the break: `Network` is `BalancedNetwork`, `DistNetwork` is `MulticonductorNetwork`, `build_scopf_instance_from_str` is `parse_scopf_str`, and `Branch::legacy_total_charging_b` is `total_charging_b` — nothing about it is legacy; it is the projection every MATPOWER shaped writer needs. `Network` named one of two models by the word for both, and `DistNetwork` named a crate rather than a model. Python's module level `__getattr__` raises a `DeprecationWarning` for `powerio.Network` and `powerio.dist.DistNetwork` and returns the renamed object. `scripts/deprecated-inventory.sh --assert-empty` is the gate that keeps them from returning.

**Numerical guards across matrix assembly and sensitivities** ([#292](https://github.com/eigenergy/powerio/issues/292)). Each of these passed a value whose reciprocal is astronomical, or used a tolerance that could not separate the case it was written for from a nearby one, and the matrix came out wrong with nothing saying so. An impedance denominator is bounded on magnitude rather than tested against exact zero, so `x = 1e-300` no longer annihilates every real branch sharing a Laplacian diagonal. A tap the builder cannot divide by is `DegenerateTap`, a new `Error` variant, and the four admittances are checked after they are computed since each input can be in range while a product is not. LODF islanding is decided by an iterative Tarjan bridge finder over the branch endpoints rather than by a tolerance that passed a *near* bridge and amplified its column to about 1e9. Both Cholesky pivot floors are `n * f64::EPSILON * max|a_ij|` instead of one absolute constant that was at once too strict for a legitimately small scaled matrix and far too loose for one whose entries run to 1e12. The `mtx` writer emits a `symmetric` header only on bit equality with a stored mirror, so a matrix that was merely close no longer goes out under a header that makes a reader rebuild it changed. Ordinary cases are unchanged; the degenerate inputs get an error or a recorded skip.

**Errors belong to the crate that raises them.** `powerio::Error` had 35 variants and 15 were never constructed in `powerio`. `powerio_matrix::Error` and `powerio_prob::Error` now carry what each crate raises — `powerio-matrix` had no error type at all and re-exported the hub's — and a variant several crates raise stays in the hub as shared vocabulary. Each new type wraps the layer below through `#[error(transparent)]`, so a hub failure crossing the boundary keeps its `Display` text byte for byte; the C ABI reports errors as text and nothing else, so a wrapper that restated the message would change what every binding prints. `powerio-pkg` gets a real error type: `from_json` returned one opaque `serde_json::Error` for malformed JSON, an unreadable lineage, and a `model_kind` contradicting its payload, which are now `Envelope`, `UnsupportedVersion` and `ModelKindMismatch`. **Fixed:** `package_pyerr` mapped every `.pio.json` failure to a bare `ValueError`, so `except powerio.PowerIOError` did not catch a package failure although it caught every other parse failure.

**OPF and SCOPF instances carry what a model reads.** `ReferenceBuses` replaces `Vec<usize>`: a consumer that grounded one slack bus wrote `.first()`, which grounds one island of a network of several and leaves the rest singular, so the new type has no `first()` — walk it with `iter()`, or call `single()`, which errors unless the set holds exactly one bus. The serde form is unchanged. `nodal_generator_data` aggregates several generators at one bus instead of refusing, combining cost curves by the parallel rule `q = 1/Σ(1/qᵢ)`; `Error::MultipleGeneratorsAtBus` is gone and the method no longer returns a `Result`. `build_dc_opf_instance` reads bus shunt conductance, which it never did although the AC builder always had, and carries it as its own vector beside `p_d` rather than folded into it. `ScopfInstance` gains the contingency count, `ScopfShuntRow.j_sh`, the reactive capability block, `violation_cost`, and the `producers_first`/`device_classes_contiguous` layout flags a model needs to address a device by per class offset. **The SCOPF index rule is document order**; the uid suffix rule is unsound on real files, and the vendored GOCompetition 14 bus case proves it — its 17 dispatchable devices carry 13 distinct numbers, because a generator and a load at one bus collide.

**Fixed: a bus hosting a flat generator alongside a quadratic one overstated its cost.** `combine_costs` in `powerio-prob` dropped the merit-order offset in that case, adding a fixed constant to the reported bus cost at every dispatch. The `q` and `c` coefficients were right, so dispatch was unaffected and only the reported objective was wrong.

**Fixed: Python's `to_dense()` mixed two bus spaces.** It returned case-mirror tables beside a lowered-view `reference_bus`, `n_components` and `is_radial`, and a `bprime()` whose shape did not match the tables next to it. `_BalancedNetwork.lowered()` is new and `to_dense` builds from it. `reference_bus` is `None` rather than `-1` when there is no unique reference, matching PowerIO.jl.

**Geo.** A branch key no longer reads a bare `id`: GIS exports and RFC 7946 tooling write a feature row counter there, and a bare integer was read as a 1-based positional row alias, so a counter put the route on an unrelated branch whenever counter order and case branch order disagreed. `geo_layer_from_aux_substations` lifts the aux `Substation` table into a layer for a case with no display file, and the aux bus reader promotes a bare `Latitude`/`Longitude` pair. `GeoApplyReport` counts `unlocated_buses` and `unlocated_branches` over the whole model, so a caller can tell "no geo data supplied" from "geo data supplied and nothing matched" — `apply_substation_points` built its own report and returned both counts zero, so `require_located` passed whatever that join left behind.

**Repository.** `scripts/capi-header-regen.sh` regenerates `powerio.h` with cbindgen and diffs it, wired into the `c-abi` job and `scripts/ci-mirror.sh`. The existing parity script compares symbol *names* only, so a reordered argument, a changed type, or a new struct field passed it — the class of defect that once shipped `pio_convert_file` with two arguments reversed and still linking. An unlicensed GOCompetition fixture is excluded from the published `powerio-prob` crate; it stays for local tests.

**Known.** Two numerical guards are stricter than the inputs they judge: the incidence tap guard rejects a tap under conventions that never read it, and the sensitivity pivot floor uses a global scale that rejects a well-posed Laplacian with wide dynamic range ([#324](https://github.com/eigenergy/powerio/issues/324)). Both are loud, accurate errors rather than silent wrongness, so they are filed rather than changed under the freeze.

## 0.8.3

Correctness fixes for the OpenDSS writer and the BMOPF reader. No API, C ABI, or schema version changes: `PIO_ABI_VERSION` stays 4, `PIO_DIST_ABI_VERSION` stays 1, and `.pio.json` stays at schema 0.2.1. Two behaviors change in ways a consumer can observe, both below.

**A center tapped service exports OpenDSS that solves to the right answer.** Reported in [eigenergy/PowerIO.jl#79](https://github.com/eigenergy/PowerIO.jl/issues/79) against three 19 kV SWER feeders. A dss node list is positional, the phase conductors first and the return last, and a center tapped service maps as `[p1, n, p2]`. The writer emitted one `Load` record over that map, so the engine discarded the conductor it could not address: on the reported network the load drew 0.652 kW of its stated 2.608 and the second leg floated to 1.35 pu, converged, with nothing in the warnings. Such a load now emits one single phase `Load` per leg. The split keys on the conductor count rather than on unequal per phase power, which a center tapped consumer does not have, and the return conductor is located from the bus grounding rather than assumed last. A terminal map longer than the record's conductor count now warns, the mirror of the short map warning that already existed.

Three more silent paths from the same report: a three winding star whose third arm is zero cannot be solved by OpenDSS, which collapses the secondary legs to about half voltage instead, so the writer substitutes the split from the OpenDSS center tap example and says so; the BMOPF and PMD readers warn when a document states no frequency and carries line susceptance, since the 60 Hz default costs a 50 Hz feeder about a fifth of its line charging; and a `center_tap` `v_nom_to` at about twice the secondary bus's own phase to neutral band warns, that being the full span rather than the per leg voltage the convention asks for.

**A malformed BMOPF numeric field is refused rather than read as `NaN`.** `bmopf::read` mapped every numeric field through `as_f64().unwrap_or(f64::NAN)`, so a string, a null, an object, or an array holding one of those became `NaN` and the parse continued silently. Schema 0.1.0 spells no `null` anywhere and types the bounds and ratings as `nonnegative_number`, so every value that reached it was already invalid — and a `NaN` bound serializes into `.pio.json` as `null`, which the payload reader restores as ±Inf, an explicit "no limit" the source never stated. Each such field is now an `Error` finding carrying its JSON pointer, so `powerio convert` and `powerio package` exit nonzero on a document that used to parse quietly. The value itself still reads as `NaN`: telling absent from invalid per field is a real feature and it waits for the typed readers of #293.

**Python.** The MCP server is inside the ruff and mypy gates (#297); the exclusions #285 added are gone.

**Repository.** The normalize pass no longer clones the element fields it immediately overwrites, on a pass that runs before every solve.

## 0.8.2

Row provenance from the normalize pass, a PYPOWER bridge, `null` for a nonfinite float in the multiconductor payload, and distribution writer coverage for rated capacitor banks and unbalanced loads. No breaking API changes; two behaviors change in ways a consumer can observe, both below.

**`.pio.json` moves to schema 0.2.1.** `serde_json` writes a nonfinite `f64` as `null`, so a package holding an unbounded rating or an unstated line length wrote a file the payload reader refused — the library could not read its own output. The reader now restores `null` per field: an upper bound reads as +Inf, a lower bound as -Inf, and a length as NaN, the PMD convention. The writer is unchanged, so 0.2.0 and 0.2.1 documents load in each other's readers, which is what the patch bump says. The published schema documents the `null` spelling and keeps every required key required; the multiconductor to balanced pass refuses a line whose length is not a finite number rather than scaling an impedance by NaN.

**Solver table `*_source_rows` values change on an already-normalized input.** `NormalizedSolverTables` used to rebuild provenance in `solver_tables` by re-simulating the normalize filter against the source network. The normalize pass now reports the rows itself through `Network::to_normalized_with_source_rows`, so there is one map instead of two that could drift, and it covers the star-lowered view the matrix builders read. For a raw case the values are unchanged. For a network already flagged `SourceFormat::Normalized` the old map was wrong — an out-of-service element and an isolated bus resolved to the wrong row or to none — and the new values are the identity. A consumer that stored provenance from 0.8.1 output for such a case resolves a dense row to a different source element after upgrading; regenerate it.

**Distribution.**

- Rated capacitor banks convert to OpenDSS. A `DistCapacitor` states `q_rated` at `v_nom`, which is what a dss `Capacitor` takes; banks were dropped with a warning before.
- A load whose phases carry different power emits as one single phase `Load` per terminal. A dss `Load` divides its `kw` evenly across its phases, so one balanced object kept the total and lost the profile. A delta load keeps the balanced form and says what was lost, since its phases sit across terminal pairs.
- A missing winding `kv` is derived from the bus voltage estimate instead of writing a `NaN` token, line level `i_max` maps to `emergamps`, and the BMOPF writer authors `terminal_conventions` from the network's own terminal naming when the block is absent.
- A refused OpenDSS include is an `Error` finding, so `powerio convert` and `powerio package` exit nonzero on a case whose `Redirect` escapes the case directory. The output is still written, for inspection. An include the OS refuses to open for an unrelated reason stays a warning.

**Python.**

- `Network.to_ppc()` and `powerio.from_ppc(ppc)` bridge a PYPOWER case dict, so a downstream server no longer hand builds the tables and hand serializes them back to MATPOWER text.
- The generator view carries `caps`, the MATPOWER gen columns past `PMIN` in column order, `None` where the source stated nothing.
- The MCP server runs on the mcp 2.0 SDK.

**Repository.** ruff, mypy, and stubtest gate the pure Python layer; a fuzz smoke run and the book tests gate every PR.

## 0.8.1

Text-writer hardening, JSON reader fidelity, a document-version report
over the C ABI, and a release gate that tests the Julia binding before any
binary ships. No breaking changes: every 0.8.0 API, format token, and wire
version is unchanged.

**Security.** The psse, pslf, powerworld, and OpenDSS writers replace a
line terminator inside any quoted or free-text field. A name that held a
`\n` or `\r` ended the record and made the rest of the text parse as new
records, so a crafted case name, DC line label, circuit id, or bus name
could forge whole records in the written file. Five such paths are now
closed, each with a test that reads the written text back and counts the
records. Upgrade if you write text formats from names you do not control.

- The two C JSON report entry points, `pio_last_error_json` and
  `pio_schema_versions_json`, now run inside the panic guard the other
  entry points use. A panic in either one crossed the C boundary, which is
  undefined behavior.
- `build_ybus` refuses a case whose base MVA is zero or nonfinite. Such a
  case wrote a matrix of `NaN` and the CLI exited 0.
- The auto sensitivity solver reads the real matrix shape (branches by
  buses) instead of the reduced dimension alone, and holds the dense path
  to a 2 GiB memory budget. The dense threshold moves from 512 to 8192,
  because a dense solve is faster than conjugate gradient well past 512
  buses. Set `SensitivityOptions::auto_dense_threshold` to keep the old
  value. PTDF and LODF results are unchanged: both paths were verified
  bit-identical on case30.
- The psse field splitter, the powerworld auxiliary token splitter, and
  the pandapower split-frame reader reuse their buffers and move rows
  instead of copying them. Reader output is unchanged.
- New `pio_schema_versions_json` C entry point reports the schema version
  of every document format the library speaks (`.pio.json`, Arrow, the
  distribution capability document, and the BMOPF vintage). A key is
  `null` when the owning feature is not compiled in. `PIO_ABI_VERSION`
  does not cover document formats, so a binding that mirrors one of these
  versions can now read it from the library and refuse a mismatch at load
  or pin time instead of finding it downstream (#270, query half).
- `release-binaries.yml` gates every tag on PowerIO.jl: the workflow
  builds the tag, runs the binding's suite against it, and produces no
  tarballs and no draft release on failure. A planned binding break
  merges the paired PowerIO.jl change first, then re-runs the workflow.
- The retired schema documents under `docs/schema/` are pinned by a test.
  A `.pio.json` written before v0.8.0 declares those URLs, so they stay
  published even though the reader no longer accepts that lineage.
- The egret and pandapower readers keep unrecognized element fields as
  `extras` instead of dropping them silently, matching the PowerModels
  reader. A powerio-written file still reads back extras-free (#263).
- The PowerModels reader reports the taps it discards: a branch with an
  off-nominal `tap` but no `transformer: true` flag reads as a line, and
  one aggregated warning now names the branches and the total. The
  inference rule itself is unchanged.
- `.pio.json` validation diagnoses duplicate payload uids directly
  (`VALIDATE.BALANCED.PAYLOAD_IDENTITY`, a new always-present
  `balanced.payload_identity` pass) instead of leaving the ambiguity to
  surface later as a failed operating-point reference.
- The text-only C conversion entry points warn when a distribution
  writer produced a companion file they cannot return, naming the file:
  an OpenDSS deck referencing a `Buscoords` CSV the caller never received
  no longer fails to compile with nothing to explain why.
- `summary`, `package`, `convert` without `--from`, and `geo
  extract|apply` read and classify a `.json` once instead of twice. The
  distribution reader's own document rule still applies on that path, so
  a JSON that is not a distribution case is refused as before.
- Python `Network.ptdf()` / `lodf()` route through the auto sensitivity
  solver (dense below the reduced-dimension threshold, iterative
  conjugate gradient above it), matching the CLI `sensitivities`
  command. Both take `solver="auto"|"dense"|"iterative"`. On a case above
  the threshold results move from exact-dense to iterative-CG at a 1e-10
  relative residual (#273).

## 0.8.0

BMOPF schema 0.1.0 alignment, one version number for `.pio.json`, and distribution JSON reader validation. Two migration notes: `.pio.json` files written by 0.7.x and earlier are rejected with an error that says to regenerate them from their source case (convert with a 0.7.x install to migrate an orphaned file), and the BMOPF writer targets the published schema 0.1.0 `$id`, so consumers that key on the old `$schema` URI should update their accepted list.

- `.pio.json` carries one version number. `schema_version` (now `0.2.0`)
  covers the whole document, model JSON included: while the major is 0 an
  incompatible change bumps the minor, and the reader accepts exactly its own
  major.minor lineage, rejecting anything else with an error that says to
  regenerate the package from its source case. The `schema`,
  `payload_schema`, and `payload_schema_version` fields and the
  `PIO_PACKAGE_SCHEMA_URL` / `PIO_PAYLOAD_*` constants are gone; the payload
  schema documents under `docs/schema/pio-payload-*` are no longer published
  (the `pio-package/0.2` document embeds every model type). Files written by
  0.7.x and earlier are rejected with the regenerate error. `schema_version`
  is required: it used to default to the current version when absent, which
  let a document skip the lineage check by leaving the field out.
- powerio-dist JSON reader validation (#262):
  - PMD bound arrays keep their finite entries when another entry is a
    null-derived infinity (an unbounded phase): generator
    `pg_lb`/`pg_ub`/`qg_lb`/`qg_ub` and linecode `cm_ub`/`sm_ub` no longer
    vanish whole. The PMD writer spells nonfinite entries back as null; the
    BMOPF writer drops such a field with a warning instead of coercing to 0,
    and the dss writer skips `emergamps` with a warning when the first
    `i_max` entry is nonfinite.
  - A PMD matrix column that is not an array warns and stays zero; the
    parseable columns survive instead of the whole matrix dropping to
    nothing silently. A matrix field that is not an array of columns has no
    shape to keep, so it drops, but it now names itself in a warning.
  - Dangling cross-references warn: a BMOPF or PMD element referencing an
    undefined bus, or a line referencing an undefined linecode, names the
    reference instead of parsing silently into a phantom bus.
  - `parse_file`/`parse_str` no longer route arbitrary JSON to the BMOPF
    reader: a `.json` without the PMD `data_model` marker, and without a
    BMOPF `bus` table beside another BMOPF table, errors (a PowerModels
    document used to parse into a bogus near-empty network). A pre-0.1.0
    feeder fragment with no `voltage_source` still classifies, because the
    reader accepts it. An explicit format override still forces the reader.
- BMOPF schema 0.1.0 (bmopf-report#16). The writer targets the published
  schema `$id` and the reader keeps accepting the pre-0.1.0 spellings:
  - `meta` carries `case_study_generator` (was `generator`) and the system
    `frequency`; the reader also still accepts the legacy top-level
    `frequency`/`base_frequency`.
  - Load `model` strings are uppercase (`CONSTANT_POWER`, `ZIP`, ...).
  - Bus symmetrical component bounds are the per-sequence scalars
    `vpos_min`/`vpos_max`/`vneg_max`/`vzero_max`/`vn_max`; legacy
    `vsym_min`/`vsym_max` arrays map on read assuming zero/positive/negative
    order. `DistBus` renames the fields, part of the `.pio.json` 0.2.0 bump
    above.
  - Three phase transformers emit one lumped `r_series`/`x_series` pair on
    the wye base, with each winding's percent resistance referred to its own
    rating before the sum; the split `_from`/`_to` fields lost their slots.
  - Transformer taps, neutral impedance, and no load admittance relocate to
    `extras.transformer.<subtype>.<name>` (the schema's escape hatch); the
    IBR, control profile, DC, and time series tables relocate under `extras`
    the same way. The reader folds all of them back.
  - A typed `capacitor` element (`DistCapacitor`: `bus`, `terminal_map`,
    `configuration`, `q_rated`, `v_nom`). The DSS converter still lowers
    OpenDSS capacitors to shunt matrices; the dss and PMD writers drop typed
    capacitors with a warning.
  - Lines accept the inline impedance alternative to `linecode` + `length`
    (read into a synthesized linecode) and carry `i_max`/`s_max`. The line
    ratings map to the ENGINEERING line's own `cm_ub`/`sm_ub` (both
    directions; an inline line's ratings stay on the synthesized linecode),
    and the dss writer warns when it drops them (the `normamps`/`emergamps`
    mapping decision is #266).
  - One-triangle matrix spellings mirror on read, the shorthand
    BMOPFTools' reader also accepts; a spelled cell always wins, and both
    writers emit full matrices.
  - Generator `s_max`/`i_max` and linecode `source` are typed fields, read
    and written; the dss and PMD writers warn when they drop them.
  - The `meta` provenance fields (title, description, license, authors,
    data_sources, created, modified, provenance, version) survive a BMOPF
    round trip; the writer keeps owning `$schema`, `frequency`, and
    `case_study_generator`.
  - A grounded terminal counts as referenced, so the unused-terminal prune
    no longer silently drops standalone grounding.
  - The vendored schema and example networks track bmopf-report@f2e3684;
    `cargo run -p powerio-dist --example regen_bmopf_examples` regenerates
    the checked-in example outputs.
- Python: `Network.write_file(path, to)` / `DistNetwork.write_file(path, to)`
  and a `convert_file(..., out=...)` output path write the serialized case to
  disk exactly as produced. Writing `to_format` text through
  `open(path, "w")` corrupts a CRLF source echo on Windows (text mode turns
  each `\r\n` into `\r\r\n`, which PSS/E family tools reject as malformed
  records); the new paths bypass Python's newline translation.
  `DistNetwork.write_file` also writes any sidecar the writer produced beside
  the case, so a dss write that emits a `Buscoords` directive no longer names
  a file that does not exist.

## 0.7.3

Security fixes for parsing untrusted case files. The parsers are written to
reject malformed input with an error, never to crash, exhaust memory, or read
or write files outside the ones named on the command line; this release closes
several gaps in that model.

- A PowerWorld `.aux` legacy `DATA` header whose field list closes before it
  opens (`]` before `[`) no longer panics on an inverted slice. It returns a
  read error, matching the guard the parenthesized form already carried.
- OpenDSS `Redirect`/`Compile`/`Buscoords` no longer read arbitrary local
  files. Parsing from a string (`parse_dss_str`, and the C ABI and Python
  string entry points) disables filesystem includes entirely, so untrusted
  `.dss` text cannot pull in a file such as `/etc/passwd` and echo its
  contents back through bus names. Parsing from a file (`parse_dss_file`) still
  follows includes, now confined to the case directory: an include that
  resolves outside that directory is refused with a warning, whether it climbs
  out with `..` or is an absolute path outside the directory.
- OpenDSS `phases`, `windings`, and `wdg` counts are capped. A single small
  property could otherwise size a dense n by n conductor matrix or a
  per-winding vector into the gigabytes; an oversized value clamps to the
  supported maximum with a warning.
- A PMD ENGINEERING impedance or admittance matrix dimension is capped the same
  way, so an array of thousands of empty rows no longer demands an n by n
  allocation.
- The matrix pipeline and the CLI `sensitivities` command sanitize the case
  name before it forms an output filename. A name like `../../x` or an absolute
  path can no longer steer a write outside the chosen output directory.
- The PowerWorld `.pwd` reader groups drawing records by tag in logarithmic
  time and bounds its substation identity search with a probe budget, so a
  crafted file cannot force quadratic work.
- The OpenDSS include confinement holds for a case file parsed by bare
  filename: an empty case directory no longer passes every path as a prefix
  match, so `Redirect /etc/passwd` in `parse_dss_file("master.dss")` is
  refused. A case directory that itself starts with `..` now confines rather
  than refusing everything under it.
- OpenDSS includes are also checked after symlink resolution: a lexically
  contained include that is really a symlink out of the case directory is
  refused. `parse_raw_file` now confines includes exactly like
  `parse_dss_file` instead of following them anywhere on disk.
- `sanitize_stem` follows Windows filename rules (trailing dots trimmed,
  reserved device names like `con` prefixed, length capped) and appends a
  short hash of the original name whenever sanitization changed it, so two
  distinct case names that sanitize alike (`a/b` and `a_b`) cannot silently
  overwrite each other's files in a multi case export. The gridfm dataset
  writer routes its output directory through the same sanitizer.
- A `DistNetwork` arriving without reader caps (the model JSON C entry point
  deserializes one unchecked) can no longer force quadratic allocation out of
  linear-size input: the BMOPF writer caps the dense zero fill for an absent
  linecode/shunt matrix and the transformer `x_sc` pair expansion at 64
  conductors/windings, with a warning.
- A `Network` JSON bus id near `usize::MAX` is rejected when 3-winding
  transformers are present, closing an integer overflow in the synthetic star
  bus allocation that could alias an existing bus. An oversized piecewise
  cost `ncost` clamps instead of overflowing during normalization and Surge
  export.
- The Python `mcp` extra pins the SDK below 2.0, which removed the
  `mcp.server.fastmcp` module the server imports.
- OpenDSS `like=` splicing is capped per object. A self-referencing or
  mutually-referencing chain (`Edit Load.a like=a` repeated) otherwise doubled
  an object's property count each edit, so a few hundred bytes could exhaust
  memory; the splice is now refused with a warning past the cap.
- The PMD writer no longer panics on a transformer with no windings, reachable
  from a PMD or BMOPF document with the winding array absent. The emergency
  rating default derives from the first winding only when one exists.
- A PSS/E transformer `COD` field of an extreme magnitude (which saturates to
  `i64::MIN`) no longer overflows when the control mode is decoded; it reads as
  a fixed ratio.
- The PowerWorld `.aux` reader routes a bus's own `BusNum`/`Number` through the
  same validation as every bus reference, so a fractional or out-of-range value
  is a read error instead of a silently truncated or saturated id.
- A GOC3 bus uid of `bus_<usize::MAX>` no longer overflows when its suffix is
  mapped to a bus id; it falls back to the 1-based document position.
- The MCP server's allowed-roots write check follows a symbolic link in the
  output filename before deciding containment. A dangling link named as the
  output file, sitting inside an allowed root but pointing outside it, could
  otherwise pass the check while the write escaped the sandbox.
- Lifting a GOC3 time series into a package operating-point series binds the
  declared `time_periods` to the `interval_duration` array length (the same
  equality the SCOPF loader enforces). An oversized `time_periods` that does
  not match the data no longer drives an unbounded per-period allocation; the
  series is refused with a diagnostic and the package stays static.
- The PMD writer caps the conductor count it expands into square matrices. A
  switch or voltage source terminal map is a linear model array, and a 35 KB
  case file naming thousands of terminals drove a 360 MB document and 1.5 GB
  of resident memory; the matrices now emit at 64 conductors with a warning
  while the terminal list itself stays faithful. Reachable from any
  distribution case file, not only the unchecked C entry point.
- The dist graph builder caps the transformer winding count feeding its pair
  expansion, so a `DistNetwork` deserialized without reader caps cannot turn a
  linear winding array into a quadratic edge list.
- A degenerate model matrix (rows shorter than the row count) reads as zero in
  the PMD writer instead of panicking on an out of range index, matching the
  DSS writer.
- `sanitize_stem` hashes a name that already carries the disambiguating suffix
  shape, so the suffixed and unsuffixed name spaces stay disjoint: a case
  cannot be named to impersonate another case's disambiguated stem, which took
  no search to construct because the suffix derives from a published hash. The
  suffix widens to 64 bits, and the DC OPF bundle writer routes its output
  directory through the same sanitizer instead of a weaker local copy.
- The CLI refuses a conversion sidecar whose path is absolute or climbs out of
  the output directory, closing the join before a writer can reach it.
- Fuzz harnesses cover the distribution family (`dss`, `pmd_json`). Both parse
  and then write the network back, since a reader cap that does not hold shows
  up in the consumer that sizes an allocation from it.

## 0.7.2

- CLI case discovery is recursive and covers every supported format (#260):
  `powerio batch` and the TUI walk the input directory for `.m`, `.raw`,
  `.aux`, `.epc`, `.pwb`, `.json`, and `.dss` files, matched case
  insensitively, pruning hidden directories and the output directory. A file
  that fails to load during a scan is skipped with a warning instead of
  aborting the run, and the no-cases error names the supported extensions.
  Distribution cases (`.dss`, BMOPF/PMD JSON) parse to the multiconductor
  model and go through the explicit `lower_multiconductor_to_balanced` pass,
  whose approximations and dropped fields surface as warnings.
- A leading UTF-8 byte order mark no longer defeats the JSON classifier or
  any text reader. `classify_json_text`, the transmission read funnel, the
  distribution parsers (including `.dss` and redirected files), the PMD/BMOPF
  JSON split, `Network::from_json`, and `Package::from_json` all strip it,
  and the parse warnings itemize the removal (a same-format echo returns the
  text without the mark).
- `parse_str_with_name`: `parse_str` plus the name hint role the file stem
  plays in `parse_file`. The CLI now reads and classifies a `.json` case
  once and hands the text straight to the typed parser; a batch scan
  previously read each `.json` twice and DOM-parsed it three times.
- A nameless distribution case loaded by the CLI takes its file stem as the
  network name, so batch exports no longer collide on the lowering's
  `lowered-multiconductor` fallback.
- JSON reader hardening from a review pass over the family:
  - PowerModels: a status field written as a JSON boolean (`"br_status":
    false`) reads out of service instead of in service; `baseMVA` must be
    finite and positive; gencost rows padded to the MATPOWER matrix width are
    trimmed to the declared `ncost` before the per-unit unscale; an
    out-of-range cost model number passes through unscaled instead of
    wrapping into the piecewise/polynomial rescale.
  - pandapower: a trafo `sn_mva` of zero or below falls back to the system
    base instead of dividing the impedance by zero; string cells can no
    longer smuggle `"inf"`/`"nan"` into numeric columns.
  - egret: a polynomial cost exponent key is bounded, so a few bytes of JSON
    can no longer demand an arbitrarily large allocation or index out of
    bounds.
  - GOC3: a duplicate `simple_dispatchable_device` uid is rejected instead of
    silently taking another device's time series bounds and cost.
  - Surge: a float bus reference too large for an index is rejected like the
    equivalent integer instead of saturating to `usize::MAX`.
  - BMOPF: matrix key indices and winding counts are bounded (a crafted
    document could previously demand gigabytes or quadratic work), a missing
    line `length` warns instead of propagating NaN silently, and the `meta`
    block is kept in extras instead of dropped.
  - PMD: a `data_model` other than ENGINEERING (the index based MATHEMATICAL
    model) is rejected instead of being misread as ENGINEERING.
  - `.pio.json`: a semver build tag containing a hyphen (`1.0.0+build-x`) no
    longer fails the schema version check.

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
