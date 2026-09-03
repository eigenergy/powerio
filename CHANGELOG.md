# Changelog

## 1.0.0 (unreleased)

PowerIO 1.0 has four representation operations: `parse`, `emit`, `serialize`,
and `deserialize`. Paths, streams, and memory enter through `Source`; separate
file, text, and byte parsing functions are gone. `PioModule<T>` stores the
value, diagnostics, sources, provenance, and history. Rust matches `PioValue`,
Python uses `isinstance`, Julia uses multiple dispatch, and C uses structural
type names and typed borrowed accessors.

PowerIO IR has one document shape: `"schema": "powerio.module"` and
`"version": 1`. The 0.10 beta document readers and upgrade code are removed.
C ABI 7 is the only C ABI and contains no aliases for earlier ABI generations.

`BalancedNetwork` and `MulticonductorNetwork` are the two electrical network
types. `OperatingPoint<T>`, `TimeSeries<T>`, and `ScenarioSet<T>` compose over
both. Calculation data uses explicit PF, OPF, and SCUC instance and solution
types. `SocwrOpfSolution` records the PowerModels SOCWR relaxation and its
objective lower bound without claiming an AC feasible solution.
Multiconductor matrices use the descriptive Rust name `ConductorMatrix`; the
beta abbreviation `Mat` is removed.

`OperatingPointUpdate`, `NetworkUpdate`, and `CalculationUpdate` target stable
component identities and absolute values with units. `apply_updates` validates
the full batch before changing a value. `UpdateReport` lists the changed fields
and reports whether energized connectivity changed.

The public DC bundle and “DC branch coefficient” vocabulary are removed.
Callers use `calc_incidence_matrix`, `calc_branch_susceptances`,
`calc_bus_susceptance_matrix`, `calc_branch_flow_matrix`,
`calc_branch_phase_shift_injection`, `calc_bus_phase_shift_injection`,
`calc_branch_flow_dc`, and `calc_bus_injection_dc`. Incidence is branches by
buses, with `+1` at the from bus and `-1` at the to bus, matching PowerModels
and MATPOWER signs.

OPF preparation preserves the instance objective, active constraints, source
identities, and source row mappings. Convex piecewise linear costs remain
piecewise linear. AC and DC solutions store separate nonnegative terminal
thermal limit multipliers and objective derivatives without assuming a
currency. `Termination` distinguishes convergence, iteration limit,
infeasibility, unboundedness, failure, and an unreported result.

Python and Julia expose `.value` and `.diagnostics`, the same four
representation operations, ordinary collection indexing, typed updates, and
the named `calc_*` functions. PowerIO.jl uses ABI 7 and owner rooted views;
Julia does not encode and reparse values to inspect them.

DOE GO Challenge 3 problem and solution files use the one public `parse`
operation. A source containing the problem returns
`PioModule<AcScucInstance>`; a directory or named memory source containing the
problem and matching solution returns `PioModule<AcScucSolution>` and retains
both files. A solution file alone is rejected because it contains neither the
component definitions nor the time axis. `emit` writes a complete
`AcScucSolution` as the official GO Challenge 3 output file; unchanged problem
data still uses exact same format echo.

PowSybl XIIDM 1.12 through 1.17 and CIM CGMES 2.4.15 and 3.0 now parse and
emit through `BalancedNetwork`; fresh output uses XIIDM 1.17 and CGMES 3.0.
The CGMES reader and writer build on Mohamed Numair's original contribution;
`evals/powsybl/cgmes-contribution-audit.md` records how each part of that work
appears in 1.0.
The source neutral model retains detailed bus breaker and node breaker
connectivity, hierarchy, terminals, switches, operational limits, tap changer
steps and controls, reactive limits, external identities, aliases, AC and DC
equipment, and the operating and solution quantities those formats carry.
The interoperability gate removes retained source bytes before writing and
loads every fresh result with PowSybl. An external RTE 7k check compares all
29 PyPowSybl tables without committing the 33 MB source case.

PSS/E RAW 33, 34, and 35 and RAWX 35 now use one mapping for transformer
controls and detailed connectivity. Fresh RAW 34/35 and RAWX output preserves
AC line and transformer names, exact generator, switched shunt, and transformer
regulated node references, and the control data for every winding of a
3-winding transformer. The sign of `COD` records whether automatic adjustment
is enabled without losing its control mode. `|COD| = 4` means control of a DC
line quantity on a 2-winding transformer, and `|COD| = 5` means asymmetric
active power flow control.

The `powerio` command composes with shell pipelines. `-` as the input of
`convert`, `summary`, `serialize`, `verify`, `dcopf`, and `sensitivities` reads
the case from standard input with a declared `--from` format. The exit status
follows the PowerIO error category: 2 `request`, 3 `io`, 4 `parse`, 5 `data`,
6 `output`, and 1 for a failure without a category. Failures the command line
raises itself carry registered `*.CLI.*` diagnostic codes.
`--diagnostics-format json` replaces every stderr line with one JSON array of
PowerIO IR diagnostic records covering the whole run, warnings and the failure
alike, with each reason in a failure's cause chain as a `note` record related
to the failure record; `powerio::serialize_diagnostics` is that encoding as a
library function. `EMIT.MULTICONDUCTOR.SIDECAR_DROPPED`, registered but never
raised before, is the warning `convert` reports when standard output cannot
carry a sidecar file.

PSS/E RAW revision 32 is read. Revision 32 records end before the bus voltage
limits (`NVHI`, `NVLO`, `EVHI`, `EVLO`), the load `INTRPT` field, the
transformer `VECGRP` field, and the winding `CNXA` field that revision 33
added; the reader lays every record out by the header revision and defaults
those fields, and a revision 32 record that ends before its last typed field
is reported as `READ.PSSE.VALUE_DEFAULTED` with the record's byte range in the
retained source. A header revision outside 32 through 35 keeps its coded
rejection. Fresh output still uses revisions 33 through 35: no emission target
names revision 32, so a revision 32 source written back as PSS/E produces
fresh revision 33 text, and its unmodeled sections survive only in the
retained source. Two PowSybl Core revision 32 cases join the fixtures under
their MPL-2.0 license, and the PowSybl interoperability gate loads the fresh
revision 33 output written from each.

The guide gains a PowerIO IR reference (`docs/src/ir-reference.md`) that
defines every structural value type field by field: type, unit, sign
convention, invariant, and the value a reader takes when a field is absent;
`powerio/tests/ir_reference.rs` holds the page to the generated schema in both
directions. `serialize` is deterministic: one module serializes to identical
text every time, and serializing what that text deserializes to reproduces
it. The MATPOWER and PSS/E readers attach the byte range of the record a
finding is about, into the retained source, to the diagnostics they raise:
the failure that ends a read and the warnings alike. `powerio_core::Error`
gains `with_span`, which attaches a range to the diagnostic that ended an
operation.

**IIDM 1.0 through 1.17 and JIIDM.** The PowSybl reader accepts every IIDM
serialization version, from the iTesla namespaced 1.0 to 1.17, with each
version's rules: busbar section voltages, inline tie lines, `targetV` ratio
tap changers, legacy shunt and static VAR compensator attributes, and loading
limits stated on the equipment. Each maps to the same tables as its 1.17 form.
The new `jiidm` token reads PowSybl's JSON encoding of any of those versions
and writes JIIDM 1.17 in PowSybl's field layout and order; a bare `.json`
whose first key is `version` classifies as `jiidm`. `SourceFormat::Jiidm`
names a module read from JIIDM, and `EMIT.JIIDM.*` codes report its writer.
XIIDM output stays 1.17. The XIIDM element mapping now reads one tree
representation produced by either the XML or JSON reader. In the PowSybl
gate, PyPowSybl reads PowerIO's JIIDM output and PowerIO reads PyPowSybl's.

CGMES sets without `TopologicalNode` data now read when their declared
profiles describe node breaker equipment (CGMES 2.4.15 EquipmentOperation,
or CGMES 3.0 CoreEquipment with `ConnectivityNode` data), matching PowSybl.
The reader calculates buses as connected components of the
`ConnectivityNode` graph joined by closed, in-service switches. SSH
`Switch.open` takes precedence over EQ `Switch.normalOpen`. Each calculated
bus takes the name of a busbar section on it and a UUIDv5 identity derived
from the joined `ConnectivityNode` mRIDs, without inventing a source
`TopologicalNode` mRID. `READ.CGMES.TOPOLOGY_CALCULATED` reports the
calculation, and `READ.CGMES.CONNECTIVITY_INSUFFICIENT` names missing data
when a set has neither `TopologicalNode` data nor calculable connectivity.
Bay containers resolve to their voltage level, and a `TopologicalNode` whose
container holds none of its `ConnectivityNode` data follows those nodes. The
PowSybl gate compares calculated bus assignments for the MiniGrid, SmallGrid,
and CGMES 3.0 MicroGrid node breaker sets with PyPowSybl.

ENTSO-E UCTE-DEF `.uct` files read and write through `BalancedNetwork` under
the format token `ucte` (alias `uct`). The reader accepts revisions
2003.09.01 and 2007.05.01 and maps `##N` nodes with their `##Z` country
groups, `##L` lines and busbar couplers, `##T` transformers, and `##R` phase
and angle regulation. `##TT` and `##E` records remain available for same
format emission and are reported with `READ.UCTE.RETAINED_SOURCE_ONLY` before
other format emission. A node's 8-character code is the bus name, each
country is an area, and cross-border `X` nodes form their own area so tie
lines keep both ends. New output uses revision 2007.05.01; a bus whose name is
not a UCTE node code receives a derived code and an
`EMIT.UCTE.VALUE_SUBSTITUTED` warning. Reader findings name their records with
source spans. The PowSybl gate loads PowerIO UCTE output from a MATPOWER case
with PyPowSybl and compares bus and branch counts with PyPowSybl for every
`.uct` fixture in the pinned PowSybl Core checkout.

The IEEE Common Data Format parses to `BalancedNetwork` under the token
`ieee-cdf` (alias `cdf`). A `.txt` or `.cdf` file opening with a CDF title
card is detected without a declared format. The reader maps the title card
MVA base and date, the bus, branch, and interchange sections, transformer
taps, phase shifts, and regulating control blocks, and reports what the
format does not state under `READ.IEEE_CDF.*` codes with record spans. Loss
zone names and tie lines survive in the retained source only. The format is
read only: `emit` to `ieee-cdf` is refused. The vendored IEEE 14 and 30 bus
cases are checked against `case14.m` and `case30.m`, and the PowSybl gate
compares every public IEEE CDF case with PyPowSybl's own CDF importer.

**One `parse`, one `emit`, and no display sibling.** `parse`, `emit`,
`serialize`, and `deserialize` take their input or output through
`powerio_core::IntoSource` and `powerio_core::IntoDestination`, implemented for
a file or directory name in every spelling a caller holds one, for content
already in memory, and for a built `Source` or `Destination`. The ordinary read
is `powerio::parse("case.raw")?` with one call and one failure point, following
`mlir::parseSourceFile(filename, block, config)` and
`llvm::object::createBinary`, which autodetects the file type. The optional
format argument every caller wrote as `None` is gone: optional configuration is
`ParseOptions`, passed to `parse_with_options`, as MLIR and LLVM pass a
defaulted config value and this workspace already pairs `emit` with
`emit_with_options`. Content in memory carries the name `<memory>`, which
identifies no format, so a format detected from a file extension is declared
through the options or named through `Source::from_memory`. `parse_display`,
`DisplayData`, and `DisplayFormat` are removed: a geographic layer is a module
value, `powerio.GeoLayer`. The canonical `.geo.json`, GeoJSON, aliased CSV or
JSON records, headerless buscoords CSV, and a PowerWorld `.pwd` display all
parse to it, `emit` writes one as `geo-json`, PowerIO IR carries it, and
`pio_value_geo_layer` takes it in C. Which value a format produces is data
rather than a choice of operation, so no reading verb stands beside `parse`. Python and Julia
keep their `format` keyword and C ABI 7 is unchanged. See
`docs/src/migration-v1.md` for the table of replacements.

## 0.10.0

PowerIO 0.10 is the public beta of the 1.0 API. API corrections may land before 1.0.0 as downstream integrations exercise the new design.

`powerio::parse(source)` returns `PioModule<PioValue>`. The value is a balanced network, multiconductor network, time series, scenario set, problem instance, or solution. DOE GO Challenge 3 JSON produces `AcScucInstance`, BMOPF JSON produces `McAcOpfInstance`, and DeepMind OPFData JSON produces `AcOpfSolution`. PyPSA snapshot axes, Egret time keys, and GridFM scenarios remain typed.

`.pio.json` stores one module with schema `powerio.module/1`. The beta document
contains the typed value, source descriptions, source map, diagnostics,
history, and extensions. Its reader and API are not part of PowerIO 1.0.

`powerio-core` defines sources, diagnostics, and modules. `powerio-tx` and `powerio-dist` define the two network families. `powerio-prob` defines problem instances and solutions. `powerio-matrix` builds sparse matrices and graph data. The `powerio` crate provides format dispatch and the combined public API.

C ABI 6 replaces ABI 5. The measured change removes 70 symbols, changes 7 signatures, adds 91 symbols, and leaves 6 unchanged. The new surface uses retained handles, structured errors, `pio_module_*` functions, and `pio_dc_data_*` functions. The removed `pio_package_*`, `pio_scopf_*`, and solver row tables are described in the Developer Guides.

DC branch susceptance now follows the PowerModels sign convention. With the
public bus by branch incidence matrix `A`, the matrices satisfy
`B = A Diagonal(b) A'`, `Bf = Diagonal(b) A'`,
`p_shift = A (b .* shift)`, and
`p_branch = -Bf va + b .* shift`. PTDF and LODF calculations use a sparse
direct factorization and retain a dense path for smaller systems.

Python uses `powerio.parse(source)`. Format and value kind detection are automatic; `value_type` is an optional assertion. Julia uses `parse_file(source)` after `using PowerIO`. The CLI reads `.pio.json` anywhere it accepts a source and writes stored modules with `powerio module`.

The 0.10 release notes contain the complete API, format, ABI, performance, and migration details.

## 0.9.0

The API and C ABI that 1.0.0 ships. Everything here exists so a later change can be additive, so this release takes the breaks: C ABI 5, one version number across every document powerio authors, a registered code on every finding powerio reports, a DC susceptance that reads the whole series impedance, and one release of working compatibility names before 1.0 removes them. Read the headings below before upgrading; each one changes what a working consumer sees.

**`powerio corpus` runs the conversion invariants over a directory of case files.** `powerio corpus ingest | compare | report` holds an arbitrary directory of case files to the same four properties the conversion matrix holds its vendored fixtures to, and the engine behind both moved into `powerio-cli`'s `invariants` module so the CI gate and the harness cannot drift apart. Files group into buckets by an electrical fingerprint — bus count, base MVA, degree sequence, quantized impedance and demand — rather than by name, so a case and its siblings in other formats land together whatever they are called. Service status and generator capability stay out of that key on purpose: a reader that drops them is what the sibling comparison exists to catch, and a key that included them would split exactly the pair whose disagreement is the finding. The corpus directory is only read, the work directory is disposable, and `report` audits its own output against every string the corpus taught it before writing a byte, so a finding states codes, class ordinals and orders of magnitude and never a filename, an element name, or a line of a source file. DC terminal power is now a property of its own: an HVDC line enters no admittance matrix, so without it the whole two-terminal DC surface sat outside every electrical check.

**`powerio corpus walk` chains cases through random format cycles and learns which pairs still teach.** The pairwise compare runs every leg from the pristine case and cannot state what only a chain can: `walk` grades that the route does not change the destination, that conversion settles on a fixed point, and that an emptied element table stays empty. Both domains walk, every finding carries the format path and the seed that replays it, and a ledger in the work directory steers each next path toward the format pairs that have taught the least, stopping when `--settle` consecutive walks teach nothing. `ingest --max-bytes` keeps an interconnection scale outlier from owning the run, and `report` now takes whichever of `compare` and `walk` ran.

**The SCOPF instance rows carry their ordinals and initial status.** Device rows gain `j_dev` (position within the row's own class, document order) and `j_sdd` (position in the canonical device stacking: the producer block then the consumer block), reserve membership rows carry the member device's pair, and device, AC line, and transformer rows carry `u_0`, the document's `initial_status.on_status`. A consumer indexes its variable vectors from these fields; deriving an index from a uid spelling — which GOCompetition's own validation case breaks — is never needed. The instance also carries `dt`, the interval durations, so a model needs no raw-document read for its time axis. Rust, C, and Python emit 0-based ordinals by default; Rust's `IndexBase`, C's `pio_scopf_to_json_with_index_base`, and Python's `index_base` keyword select 1-based output for Julia and other consumers. The document always records the selected `index_base`. Its schema id drops the language suffix: `powerio.scopf`, previously `powerio.scopf.julia`.

**C ABI 5.** Twenty-five symbols, zero renamed — nine re-signatured, nine changed behavior, four removed, three added. Two changes reach further than that count and are stated rather than enumerated: the `CODE: ` prefix lands on every `errbuf` message, so every fallible symbol carries it, and seven JSON documents changed shape while their symbols kept their signatures. `PIO_ABI_VERSION` is 5 and bindings gate on it by equality, so a binding built against 4 must move with it. `PIO_DIST_ABI_VERSION` stays 1, frozen rather than removed: twelve distribution call sites in PowerIO.jl resolve it, and deleting it would make a library that fully supports distribution refuse every distribution call. [Migration guide](https://powerio.dev/guide/abi-v5.html).

**Conversion findings come back as records through an out pointer.** `pio_to_format`, `pio_convert_file`, `pio_convert_str`, `pio_write_dir` and their three `pio_dist_*` twins took a caller `warnbuf` and truncated into it silently — the length that would have said anything was lost was discarded, and the header advertised 256 bytes as sufficient. They take `char **out_diagnostics_json` instead: NULL when the conversion lost nothing, otherwise an owned JSON array of diagnostic records freed with `pio_string_free`, written before the call does any work so a stale value from an earlier call cannot be read as this one's. An error return leaves it NULL as well — the string the call returns is built before the findings are published, so a caller reading NULL as the error signal has nothing left to free. A warning is a record with severity `warning`, so one channel carries both rather than two carrying one fact twice, and a record adds what a line cannot: the code, the severity, and where known the element path, the source reference and a details object. `pio_warnings` and `pio_dist_warnings` keep their caller buffer and stay the plain text view for a caller with no JSON parser; they use the size-then-fill idiom and cannot truncate.

**Every `errbuf` message leads with its code.** `pio_build_info` has advertised `error_categories` as the tokens that prefix an error message since ABI 4, and nothing prefixed anything. Every message the boundary writes now reads `CODE: message` — `REQUEST.FORMAT.UNKNOWN: unknown format \`bogus\``, `BIND.CAPI.NULL_HANDLE: network handle is NULL` — from the raising error's own registry entry, or from `powerio-capi`'s `BIND.CAPI.*` registry for the failures the boundary detects itself from an argument's representation alone. Split at the first `": "` to branch on the code; the prose after it is under no stability promise. `pio_build_info` gains `diagnostic_namespaces`, the ten first segments powerio emits, so a consumer merging reports from several producers can tell powerio's findings from its own; `error_categories` stays as the coarse five bucket projection.

**`source_format` reports the same token `from` accepts.** The balanced network's reported source format was the bare Rust variant name — `Matpower`, `PowerModelsJson` — a spelling that existed nowhere else: every `from`/`to` parameter, the CLI, the package origin block and the distribution side all speak the lowercase token (`matpower`, `powermodels-json`). The property (Python `source_format`, `pio_source_format`, the summary documents and the model JSON field) now reports `SourceFormat::name()`'s token, so the value a document carries is valid input to every entry point that takes a format name. The read side still accepts the pre-0.9 variant spellings in model JSON, and format name lookup was already case and hyphen insensitive, so a stored `"Matpower"` keeps resolving; the written form is the token.

**The transmission write entry points take an options struct.** `pio_to_format`, `pio_convert_file`, `pio_convert_str` and `pio_write_dir` take `const PioWriteOptions *opts`, so the generator cost policies the CLI and Python have shipped for releases are finally reachable from C: `missing_gen_cost_mode` (preserve, require, fill), the five doubles the fill row carries, and `gen_cost_csv`. That last field is CSV **text**, never a path — a write entry point does not open a file the caller names, which is the behavior 0.7.3 removed from the string entry points, and reading the CSV yourself is what the other two surfaces already do. The struct is extensible in the Linux syscall convention of `openat2`: `size_t struct_size` first, the library reads `min(struct_size, sizeof)` bytes, a nonzero field past its own size fails the call rather than being dropped in silence, and NULL means every default, which is exactly what these entry points did before. Later write options are appended fields rather than new symbols. The three `pio_dist_*` write entry points do not take it: the multiconductor writers carry their own per format options and none of them is reachable from this policy set. `powerio::write_dir_with_options` is new on the Rust side so the directory writer runs the same policy as the text writers, and the CLI's `--to pypsa-csv` path routes through it.

**`pio_normalize` takes the options struct and `pio_normalize_with_options` is gone.** Two normalize symbols become one: `pio_normalize(net, opts, errbuf, errlen)` with `const PioNormalizeOptions *opts`, NULL meaning every default and reproducing exactly what ABI 4's `pio_normalize` did. The flat `clamp_angle_bounds` and `angle_bound_pad` arguments are fields under the same rules `PioWriteOptions` states, so the next normalize option is an appended field rather than a third symbol. `angle_bound_pad` has to be in `(0, pi/2)` when the clamp is on, so `0` was never a value a caller could pass and it means the default 1.0472 radians — a zero filled struct is the default options with no sentinel to encode on either side of the boundary. A binding that resolves `pio_normalize_with_options` through a runtime symbol lookup must re-key that lookup: the symbol is not there, and a lookup whose false branch falls back to the consumer's own repair will run that repair with nothing to report.

**`pio_geo_parse` returns the reader's notes.** The tolerant geo reader skips what it cannot use, and through ABI 4 the C entry point threw those skips away and the header said so, so a C caller could not tell a sidecar that read whole from one that read half. It takes `char **out_diagnostics_json` in the position the conversion entry points use, under the same discipline: written before the call does any work, NULL when every record was used, NULL for the parameter itself discards, and an error return leaves it NULL. `pio_geo_apply` is unchanged; it already carries the same notes on the handle it returns.

**Every per-bus and per-branch extractor reports the star-lowered space.** An in-service 3-winding transformer gains a bus before the dense extractors run. Through ABI 4, `pio_n_buses` and `pio_bus_ids` counted the unexpanded case table while `pio_bus_demand`, `pio_bus_shunt` and `pio_n_islands` counted the expansion, so a buffer sized from `pio_n_buses` read short and its trailing entries had no id. `pio_n_buses`, `pio_bus_ids`, `pio_n_branches`, `pio_branches` and `pio_branch_charging` all report the lowered space now. `length(bus_ids) == n_buses` is the migration test, and the closure a caller actually depends on holds: every branch endpoint is a bus the API reports, and every bus has an incident branch. The int64 id space the header advertises is enforced rather than assumed: `BusId::MAX` is `i64::MAX`, `validate` refuses an id above it and reserves the star expansion's headroom under the same ceiling, and readers refuse an id column value they cannot represent — the saturating cast that silently collided two distinct out of range ids on one reported value is gone, and the error names the column.

**Seven ABI-visible JSON documents changed shape while their symbols kept their signatures.** `pio_schema_versions_json` dropped four keys. `pio_dist_capabilities_json`, `pio_arrow_catalog_json`, `pio_scopf_to_json`, `pio_package_to_json` and `pio_dist_summary_json` renamed `schema_version` to `powerio_version`, and `pio_arrow_catalog_json` also dropped the per-table `schema_version`. `pio_summary_json` gained `topology.n_buses` and `topology.n_branches`; its `counts` block stays the case file's own inventory, so a 3-winding transformer is one row there rather than the bus and three branches it lowers to. The Arrow metadata key `powerio.schema_version` is `powerio.version`. A binding built against ABI 4 passes the handshake and then reads `null` for keys it mirrors, which is why the integer moved.

**A nonfinite float spells itself as a string in every document powerio authors.** JSON has no `Inf`/`NaN` literal, and through 0.8.x serde wrote a nonfinite field as `null`, which the reader refused on the way back — `powerio package` refused stock case9241pegase over seven generators whose absent reactive limit legitimately reads as `Inf`, and every consumer moving a parsed network as JSON hit the same wall. A float position now writes `"Infinity"`, `"-Infinity"`, or `"NaN"` and reads back either a number or one of those spellings, one convention across model JSON, the multiconductor payload, and the `.pio.json` document, so every document powerio writes reads back. The multiconductor bound fields keep accepting the `null` a pre-0.9 writer emitted, restored by field role as before; everywhere else a `null` is refused with a message naming this change. The published schema states the string arm on every float position, and `to_json_with_diagnostics` returns an empty record list — nothing degrades anymore, and the channel stays for whatever write-side finding comes next.

**The MCP server works over a real transport.** The mcp 2.0 SDK rewrites a string argument into a parsed object whenever its annotation is not exactly `str`, so every JSON-carrying argument (`json`, `content`, `package_json`) arrived as a dict and failed validation — a model could parse a case and could do nothing multi step with the result, and no test saw it because every test called the tool functions in process. The three arguments are bare `str = ""` now (empty means unset), a regression suite drives the server over stdio through the client SDK, and the closed argument sets advertise themselves: `json_format` and matrix `kind` carry `enum` in the tool schema, and the accepted case format names are in the tool descriptions. Errors lead with their diagnostic code instead of a prose prefix, and the Python exceptions carry the same code as a `.code` attribute. The directory writers (`pypsa-csv`, `gridfm`) fill a fresh staging directory and install it through the same containment resolution as a single file write, per target, with `overwrite=false` refusing before anything moves — they previously created children the containment policy never saw. `powerio.mcp.sandbox` exports `PathNotAllowed` so a consumer catches a containment refusal by type, and `python -m powerio.mcp` and the `powerio-mcp` console script are stated consumer entry points that do not move without a version bump.

**The OpenDSS reader takes an opt-in include root.** Confinement to the case file's own directory stays the default, but a case split across a feeder directory and a shared component directory — ordinary OpenDSS practice — was unreadable even when one configured root admitted both. `DssReadOptions { include_root }` widens the boundary to a directory the caller names, with every existing guard (byte budget, include count, nesting, the symlink closure) applied against the widened root, and the MCP server passes the allowed root that admitted the case, so an operator's configured containment is the one policy in force. String input parsing still resolves no includes at all.

**The first errors a new user hits say what happened.** `READ.IO.FAILED` names the file it could not read. A directory that is not a PyPSA CSV folder is refused as a directory instead of being diagnosed by its "extension". `CANONICALIZE.NORMALIZE.NO_REFERENCE_BUS` states the actual rule — a reference must host an in-service generator — instead of contradicting a bus table that plainly shows a REF bus. `REQUEST.FORMAT.UNKNOWN` lists every accepted format name, held to the routing matcher by test. And two silent semantic decisions now announce themselves at the same gateway, the normalize pass that prepares a solver-ready copy: `CANONICALIZE.NORMALIZE.REFERENCE_DESIGNATED` when it designates a slack the case did not state, and `CANONICALIZE.NORMALIZE.GEN_COST_ABSENT` when the copy has in-service generators and no cost data, so a zero objective stops masquerading as an optimization. Both live at normalize rather than at parse deliberately: whether a case carries costs is the case's business, a conversion leg must not count it, and the conversion matrix keeps measuring what a format can express rather than what a fixture happens to contain.

**An instance build refuses a concave cost row.** A negative quadratic coefficient passed through untouched for a lone generator and was silently flattened in the shared bus merge, so the same row priced a bus two ways depending on its neighbours. The readers keep the row — it is the case's data — and `powerio-prob` refuses it at the one point both builders read cost curves, with `BUILD.INSTANCE.CONCAVE_COST` naming the generator and the coefficient. A zero coefficient keeps its deliberate flat reading.

**OpenDSS regulator banks reach BMOPF as the regulator subtypes.** A BMOPF document carrying `single_phase_autotransformer` or `open_delta_regulator` already round tripped verbatim through the untyped passthrough; the loss was on the OpenDSS side, where a RegControl targeted regulator emitted as a plain `single_phase` transformer and an autotransformer dropped with a diagnostic. The dss reader now classifies a single phase series regulator into the autotransformer subtype and two line to line legs spelling ABBC, BCAC or CABA into one open delta regulator, the writer emits both with the field grammar BMOPFTools defines (`EMIT.BMOPF.TRANSFORMER_OPEN_DELTA_MERGED` names the absorbed leg), and every ambiguous pattern keeps today's typing — a wrong classification is worse than a drop. A consumer integration run over the branch found the adjacent hole, also closed: a fixed tap open delta bank (two one phase Delta/Delta legs, no RegControl, an ordinary deck) had no classify arm and both legs dropped silently; the one phase arm now takes every conn pairing. The subtypes are a schema extension the BMOPF task force has not yet absorbed; the writer documentation says so next to `voltage_source.cost`, which has the same standing.

**Arrow `solver_storage` carries the two efficiencies.** `charge_efficiency` and `discharge_efficiency` append after `q_loss` in table 13 — `SolverStorageRow` always carried both, the storage state constraint reads both, and appending columns is exactly what the catalog's append only rule is for.

**The `powerio-json` token is gone, and a bare model JSON document classifies as `model-json`.** Balanced model JSON is powerio's own document rather than a case format, and `to_json`/`from_json` (`pio_to_json`/`pio_from_json` in C) have carried it for releases, so the token routed one thing through two entry points and made powerio look like the author of a case format it never wrote. `TargetFormat::PowerioJson` and `TransmissionFormat::PowerioJson` are removed, `target_format_from_name` no longer answers to `powerio-json`, `powerio` or `json`, the hidden CLI `--to powerio-json` arm and its deprecation warning are gone, and `pio_parse_str`/`pio_to_format` refuse the three tokens with `REQUEST.FORMAT.UNKNOWN`. **The bare `json` alias goes with them**, so a caller that wrote `"json"` meaning "let it sniff" must name a format or parse by path. A `.json` file holding model JSON is no longer routed to a reader by `parse_file`: the sniffer refuses it and the message names `from_json`. The CLI `powerio.summary` document and the MCP server's responses state `"json_format": "model-json"`; the MCP validator still accepts `powerio-json` and its short spellings as inputs, since its schema is versioned with the Python package rather than the C ABI.

**The JSON classifier answers a closed set of families, and every fixture is held to it.** `JsonClass` gains `ModelJson` beside `Package`, so a consumer routing a bare `.json` can name a model JSON document without a case format token existing, and `JsonClass::family()` is the one place the family token is spelled. That spelling now crosses every surface unchanged: `pio_classify_str`'s label, the Python `classify_json_text` status, and the Julia family symbol are all one of `transmission`, `distribution`, `package`, `model-json`, `ambiguous`, `unknown`. The set is closed and additive — spellings are permanent, a family is never removed or redefined, and a new one is a minor release with a changelog line — because a file picker dispatches on it. `pio_build_info` reports it under `json_classes` and Python exposes `json_classes()`, so a binding reads the set rather than hardcoding it. A corpus test classifies every `.json` fixture in the tree against a stated expected class, so a new fixture must say what it is, and a fuzz target drives `classify_json_text` and `pio_classify_str` over arbitrary bytes asserting no panic and no undocumented answer. `Detection`, `JsonClass`, `JSON_CLASSES` and `classify_json_text` are re-exported at the crate root beside the parse entry points — the classifier is the surface consumers are told to adopt, so it is at least as reachable as the token it replaced — and the recognition rule is stated in prose in the schema README, so a consumer classifying a dropped file in TypeScript, Python, or Julia implements the rule instead of inventing one. `DiagnosticSeverity` and `DiagnosticCode` hoist to the crate root beside `StructuredDiagnostic`, whose fields they type.

**Generator cost reaches Arrow, and `solver_bus` carries area and zone.** Two tables append to the catalog: 21 `solver_gen_cost`, dense over solver generators in `solver_gen` order, carrying `model`, `startup`, `shutdown`, `ncost`, `coeff_count` and `coeff_offset`; and 22 `solver_gen_cost_coeff`, the flattened coefficient vector as `(gen_index, position, value)` that a header row slices with its offset and count. Values are per unit on the network's MVA base, which both tables state in schema metadata alongside the group axis. `model == 2` reads position `i` of a `coeff_count` long slice as the coefficient of `p^(coeff_count - 1 - i)`; `model == 1` reads even positions as breakpoint active power and odd positions as the curve value there; `model == 0` is a generator with no cost row, and its `coeff_offset` is the sentinel `-1`. `ncost` is the count the source declared and `coeff_count` the number of values stored, so a curve whose declared count outruns its coefficients stays visible instead of being repaired. `solver_bus` gains `area` and `zone` at the end of its column list in the same change. Both are append only: no existing table id, column order or `PIO_ABI_VERSION` moves, and a consumer addressing columns by name sees nothing shift.

**Deleted and added.** The three `pio_acopf_*` symbols are gone: no C consumer, and `acopf` appears nowhere in PowerIO.jl. Re-cut them additively when a consumer exists and can say what shape it needs. `pio_normalize_with_options` is the fourth removal, folded into `pio_normalize` above. `pio_build_info` returns one document — version, ABI integer, compiled features, foreign schema versions, `diagnostic_namespaces`, `error_categories` and `json_classes` — shaped after `curl_version_info`, and the last three are the sets a binding reads from it rather than hardcodes. `pio_parse_bytes` takes `(const uint8_t *, size_t)` and every `pio_parse_str` format name plus `pwb`.

**`parse_bytes` on all four surfaces, and byte entry points for every document owner.** Rust, C, Python and Julia. It is the only in-memory route to the PowerWorld `.pwb` binary reader, and it opens nothing, which is what makes it the right entry point for input you do not control. `BalancedNetwork::from_json_bytes`, `powerio_dist::parse_bytes`, and `NetworkPackage::from_json_bytes` complete the set: a browser or archive consumer that already holds bytes hands them over directly instead of decoding through `File.text()`, which replaces every invalid byte with U+FFFD and reports nothing — a CP1252 OpenDSS deck reached the reader silently mangled that way. `classify_json_bytes` applies the same rule before routing: a leading UTF-8 byte order mark is accepted and invalid UTF-8 is `unknown`; model JSON parsing returns the coded malformed source error instead of replacing a byte.

**The DC susceptance reads the whole series impedance, and states its sign once.** `DcConvention` offered `b = 1/x` and MATPOWER's `b = 1/(x·τ)`; neither reads the branch resistance, so a case with a real r/x ratio had no convention describing it and every consumer computed one by hand. `SeriesImpedance` is `x/(r² + x²)` with phase shift injections and no tap scaling, and it is the new default — a caller that passed no convention gets different numbers, and the gap grows with r/x, so it is small on transmission cases and large on distribution ones. `branch_susceptance` returns a **positive** Laplacian edge weight. PowerModels and tellegen write the negative one; that is their convention. A test holds all three variants to one sign, so a caller that negates once cannot get a sign flipped matrix from the choice of variant. `PaperPure` is now `ReactanceOnly`, which names the formula rather than a paper, and it is not deprecated: `b = 1/x` is the textbook DC linearization, so reproducing a published result needs it exactly as written. The CLI takes `--convention series` and Python takes `"series"`.

**The DC OPF instance and bundle state the complete affine model.** `fixed_nodal_withdrawal()` returns `p_d + g_s + p_shift` for `L theta = Cg pg - fixed`, and `branch_flow_offset()` returns `-b * shift` for `f = BAt theta + offset`. The instance retains `synthesize_unrated_limits` with a false serde default for older documents. Bundles add `shift.mtx`, `flow_offset.mtx`, and `fixed_withdrawal.mtx`, record the synthesize option, and give every emitted data file exactly one `operators[]` row, including `c0`, `c0_gen`, and `gs`.

**One version number in every document powerio authors.** powerio stamped fifteen; nine were invented for its own artifacts, six of those were write only, and two claimed 1.x stability inside a 0.x library. Every document powerio authors now states `powerio_version`, the release that wrote it, and `powerio::version` holds the acceptance rule: a document loads when it shares this build's lineage, the major once it reaches 1 and the major and minor pair while the major is 0. `.pio.json`, the SCOPF document, the DC OPF bundle manifest, the geo layer sidecar, the Arrow catalog and table metadata, and every summary document change their version field — **regenerate stored artifacts**. A `.pio.json` from 0.8.x or earlier states no version at all; it deserializes to the empty string rather than defaulting, so the gate stays closed and the reader names the release that wrote it. A foreign format keeps its own version: powerio implements case formats and authors none, so pandapower's `3.0.0` and the BMOPF `$schema` are reproduced, never set. The served JSON Schema path follows the same lineage the reader accepts, so `pio-package/0.9` is the current document and the retired identifiers stay published.

**The 1.0 API uses one spelling for each supported operation across surfaces.** The distribution model types use the `Dist` prefix (`DistWinding`, `DistWindingConn`, `DistLocation`, `DistCoordsKind`, `DistGeoMeta`, `DistCanvas`), which also removes the numbered names schemars had generated beside them. The source format parameter is `from` (`from_` in Python) across every string and bytes entry point instead of half `from` and half `format`, and `powerio.parse_str` requires the format like its siblings. `BalancedNetwork::to_canonical_format` is the balanced counterpart to the distribution operation, producing a regenerated write when byte exact echo is not wanted. `gridfm_record_batches` names the batch function and the one network wrapper carries the suffix. The study edit's rating field is `delta_mva` because it patches the apparent power fields `rate_a`, `rate_b`, and `rate_c`. The schema `$id` carries its filename so references constructed from the documentation resolve.

**0.9 is the bridge: the 0.8 names live for exactly one release behind warnings that work.** The 0.8.x deprecation aliases never actually deprecated anything — `powerio/src/lib.rs` re-exported only the new names, so `use powerio::Network;` got a compile error rather than the promised warning. 0.9.0 fixes the bridge instead of abandoning it: `Network` is `BalancedNetwork`, `DistNetwork` is `MulticonductorNetwork`, `build_scopf_instance_from_str` is `parse_scopf_str`, `Branch::legacy_total_charging_b` is `total_charging_b`, and `DcConvention::PaperPure` is an associated constant equal to `ReactanceOnly` that works in expression and pattern position — each reachable where 0.8 code looks for it, each warning naming its successor and the 1.0.0 removal. Python bridges the same way: `powerio.Network` and `powerio.dist.DistNetwork` resolve with a `DeprecationWarning`. `scripts/deprecated-inventory.sh` lists the whole set, and its `--assert-empty` run is the 1.0.0 gate that deletes it. The retired `powerio-json` token is the one name with no alias, since model JSON stopped being a conversion target; passing it now gets guidance naming `to_json`, the MCP `json_format` channel, and the `model-json` classification family. [Coming from 0.8.x](https://powerio.dev/guide/abi-v5.html).

**Numerical guards across matrix assembly and sensitivities** ([#292](https://github.com/eigenergy/powerio/issues/292)). Each of these passed a value whose reciprocal is astronomical, or used a tolerance that could not separate the case it was written for from a nearby one, and the matrix came out wrong with nothing saying so. An impedance denominator is bounded on magnitude rather than tested against exact zero, so `x = 1e-300` no longer annihilates every real branch sharing a Laplacian diagonal. A tap the builder cannot divide by is `DegenerateTap`, a new `Error` variant, and the four admittances are checked after they are computed since each input can be in range while a product is not. `branch_susceptance` returns `NaN` for a denominator that is not finite, which `SeriesImpedance` already did: `1/±inf` is `0.0`, so `ReactanceOnly` and `Matpower` read an infinite reactance as a zero weight edge and dropped the branch from a Laplacian that Y_bus rejects outright on the same handle, and because `Matpower` divides by `x · τ` two finite factors whose product overflows read the same way. LODF islanding is decided by an iterative Tarjan bridge finder over the branch endpoints rather than by a tolerance that passed a *near* bridge and amplified its column to about 1e9. Both Cholesky pivot floors are `n * f64::EPSILON * max|a_ij|` instead of one absolute constant that was at once too strict for a legitimately small scaled matrix and far too loose for one whose entries run to 1e12. The `mtx` writer emits a `symmetric` header only on bit equality with a stored mirror, so a matrix that was merely close no longer goes out under a header that makes a reader rebuild it changed. Ordinary cases are unchanged; the degenerate inputs get an error or a recorded skip.

**Errors belong to the crate that raises them.** `powerio::Error` had 35 variants and 15 were never constructed in `powerio`. `powerio_matrix::Error` and `powerio_prob::Error` now carry what each crate raises — `powerio-matrix` had no error type at all and re-exported the hub's — and a variant several crates raise stays in the hub as shared vocabulary. Each new type wraps the layer below through `#[error(transparent)]`, so a hub failure crossing the boundary keeps its `Display` text byte for byte; the C ABI reports errors as text and nothing else, so a wrapper that restated the message would change what every binding prints. `powerio-pkg` gets a real error type: `from_json` returned one opaque `serde_json::Error` for malformed JSON, an unreadable lineage, and a `model_kind` contradicting its payload, which are now `Malformed`, `UnsupportedVersion` and `ModelKindMismatch`. **Fixed:** `package_pyerr` mapped every `.pio.json` failure to a bare `ValueError`, so `except powerio.PowerIOError` did not catch a package failure although it caught every other parse failure.

**One diagnostic record, and the stage comes from the code.** `powerio-dist` and `powerio-pkg` each defined a `StructuredDiagnostic`, a `DiagnosticCode`, a `DiagnosticSeverity` and a `DiagnosticStage`, bridged by a lift that mapped any stage a newer distribution crate added onto `read`. There is one of each now, in the new `powerio-diag` crate below both, so a distribution finding reaches a package as itself and can carry the `source_ref` it previously had nowhere to put. `StructuredDiagnostic::new` takes no stage argument: the stage is the first segment of the code, read back through `StructuredDiagnostic::stage()`, so a record cannot state a stage its code contradicts — a refused OpenDSS include was emitted under `READ.DSS.INCLUDE_REFUSED` at stage `parse` and now reads as `read`. In `.pio.json` the `stage` field is optional, written from the code and ignored on read, so a producer whose namespace is outside powerio's ten omits it. The stage enum gains `build` (assembling an index, a matrix, or a solver table from a case that already parsed) and `request` (a call naming a format or an option powerio does not provide); both land before 1.0 freezes the set, since an addition after it is a value an older validator rejects.

**Every warning carries a code, and every code is registered.** A warning was a free-form string: useful for a person, opaque to a script. Every emission site across the workspace now names a registered entry instead of writing a literal, so the text channels are rendered from the records and read `CODE: message` — `EMIT.PSSE.FIELD_DROPPED: generator cost curves dropped: PSS/E .raw has no cost data`. Split at the first `": "`; the left side matches `NAMESPACE.SCOPE.SPECIFIC` and cannot contain a colon, the right side is prose under no stability promise. **Every consumer that matched a warning by its whole text must move**; a substring or a `is_empty` check is unaffected. Codes are families rather than one per site: what differs between two sites of a family is which field or record it was, which belongs in `details` where a script can read it. Each crate declares its own registry — `powerio::diagnostics::codes` and its siblings — and the entry carries the default severity, the `ErrorCategory` projection for the codes that can be fatal, and a one line summary. `Error::code()` lands on all five error types beside `category()`, exhaustive with no wildcard, so a new variant is a compile error until it is coded, and a test holds `code().category` to `category()` for every variant. `Error::ReferenceBusCount` is the one variant raised from two stages, so it carries the raising site's choice: build it with `Error::no_reference_bus` (canonicalization) or `Error::reference_bus_count` (index build) rather than the struct literal.

**The three catch-all codes are retired.** `READ.TRANSMISSION.PARSE_WARNING`, `READ.GRIDFM.FIDELITY_WARNING` and `READ.DIST.PARSE_WARNING` existed only because the strings they wrapped had no identity of their own. A reader's findings arrive coded now, so `NetworkPackage::from_balanced_with_read_warnings(net, code, warnings)` is `from_balanced_with_read_diagnostics(net, diagnostics)` and `record_read_warnings` is `record_read_diagnostics`; the two exported constants are gone from `powerio-pkg`. `from_multiconductor` takes the parse findings as they are, which also deletes the message equality filter that kept a typed finding and its warning twin from appearing twice — there is one list now and no twins. `READ.OPERATING_POINTS_DROPPED` (two segments, so outside the grammar) and its per format sibling become `READ.PACKAGE.OPERATING_POINTS_DROPPED`. Every retired code stays in its registry so its identity is never reassigned and a document carrying one still reads.

**The stored module's codes speak the module vocabulary.** The seven codes describing operations on the stored module carried the retired `PACKAGE` segment: `PARSE.PACKAGE.MALFORMED`, `PARSE.PACKAGE.UNSUPPORTED_VERSION`, `VALIDATE.PACKAGE.MODEL_KIND_MISMATCH`, `REQUEST.PACKAGE.NO_SUCH_INDEX`, `REQUEST.PACKAGE.WRONG_MODEL_KIND`, `BUILD.PACKAGE.PAYLOAD_FAILED` and `EMIT.PACKAGE.SERIALIZE_FAILED`. Each is now the same code under `MODULE`, and the `PACKAGE` spelling stays registered as retired so a document carrying one still reads and its identity is never reassigned. A consumer branching on one of the seven moves to the `MODULE` spelling. The four codes the legacy 0.9 reader emits about a package document keep `PACKAGE`: that is the document family's name.

**The registry has gates, and the workspace gate lives in `powerio-capi`.** Per crate: every code matches the grammar, its first segment is one of the ten namespaces, and no code appears twice. Workspace wide, behind `cfg(all(feature = "dist", "pkg", "prob", "matrix"))` — the feature set the release CI job already builds — every registry is concatenated and held to global uniqueness plus disjoint scope ownership, so two crates cannot both claim `EMIT.PSSE`. `scripts/diag-inventory.sh` extracts every code literal and every error variant from the tree, which is how the registries were built: a hand audit of the same tree missed two entries while specifically hunting for them.

**OPF and SCOPF instances carry what a model reads.** `ReferenceBuses` replaces `Vec<usize>`: a consumer that grounded one slack bus wrote `.first()`, which grounds one island of a network of several and leaves the rest singular, so the new type has no `first()` — walk it with `iter()`, or call `single()`, which errors unless the set holds exactly one bus. The serde form is unchanged. `nodal_generator_data` aggregates several generators at one bus instead of refusing, combining cost curves by the parallel rule `q = 1/Σ(1/qᵢ)`; `Error::MultipleGeneratorsAtBus` is gone and the method no longer returns a `Result`. `build_dc_opf_instance` reads bus shunt conductance, which it never did although the AC builder always had, and carries it as its own vector beside `p_d` rather than folded into it. `ScopfInstance` gains the contingency count, `ScopfShuntRow.j_sh`, the reactive capability block, `violation_cost`, and `device_class_layout`. **Every SCOPF ordinal comes from document order**: `j_dev` is the position within the row's device class and `j_sdd` is the canonical producer block followed by the consumer block. Serialization defaults to base 0 and can select base 1 without changing external identities or value fields. The explicit layout enum reports whether source device classes are contiguous without changing those ordinals.

**Fixed: a bus hosting a flat generator alongside a quadratic one overstated its cost.** `combine_costs` in `powerio-prob` dropped the merit-order offset in that case, adding a fixed constant to the reported bus cost at every dispatch. The `q` and `c` coefficients were right, so dispatch was unaffected and only the reported objective was wrong.

**Fixed: Python's `to_dense()` mixed two bus spaces.** It returned case-mirror tables beside a lowered-view `reference_bus`, `n_components` and `is_radial`, and a `bprime()` whose shape did not match the tables next to it. `_BalancedNetwork.lowered()` is new and `to_dense` builds every field from it. `reference_bus` keeps reporting `None` when there is no unique reference; what changed is the space the tables beside it describe.

**Geo.** A branch key no longer reads a bare `id`: GIS exports and RFC 7946 tooling write a feature row counter there, and a bare integer was read as a 1-based positional row alias, so a counter put the route on an unrelated branch whenever counter order and case branch order disagreed. `geo_layer_from_aux_substations` lifts the aux `Substation` table into a layer for a case with no display file, and the aux bus reader promotes a bare `Latitude`/`Longitude` pair. `GeoApplyReport` counts `unlocated_buses` and `unlocated_branches` over the whole model, so a caller can tell "no geo data supplied" from "geo data supplied and nothing matched" — `apply_substation_points` built its own report and returned both counts zero, so `require_located` passed whatever that join left behind.

**A PSS/E two-terminal DC record's received power is priced by the record itself.** The reader set `pf = pt = SETVL`; the record states the DC current (`SETVL/VSCHD`, kA at kV), which prices the line loss `I²·RDC` in MW exactly, so the inverter now receives the demand minus the line's own drop. A negative `SETVL` under `MDC = 1` is the same demand measured at the inverter, and under `MDC = 2` `SETVL` is a current demand in amps — previously read as MW. The writer emits `SETVL` in the stored mode's own unit, so a re-read lands on the same operating point. `Hvdc` values read from real PSS/E input that states a resistance and a voltage schedule change; cross-format writes synthesize `RDC = 0` and are unaffected.

**The conversion matrix stops taking losses the formats don't have to take, and green cells must earn it.** Writers dropped, or warned about, data the matching reader was already prepared to take back: the canonical MATPOWER writer now emits `mpc.dcline`, `mpc.dclinecost` (the reader now reads it; a `toggle_dcline` zero-padding row reads back as no cost), `mpc.bus_name`, and the standard 21-column gen row whenever a generator carries capability columns; egret states `startup_cost`/`shutdown_cost`; pandapower gains the `dcline` table its reader has read all along, and generator reactive output rides `res_gen.q_mvar` — pandapower states solved Q as a result, not an input, so every PV machine and the slack used to read back at `qg = 0` silently. Readers stop retaining what powerio's own writers synthesize (positional circuit ids, zero load-component splits, default DC converter tails, PSLF's raw record echoes), so cross-format hops stop warning about restatements; a pandapower `max_i_ka` is one fact with `rate_a`, not two; and a costless network writing no `mpc.gencost` is the source's own shape, not a loss. The matrix harness itself now holds every cell to the admittance matrix entry for entry and the per-bus injections — the power flow problem must survive whatever a format drops — and a cell claiming zero warnings must leave the full typed model bit-identical, so suppressing a warning without carrying the data fails the gate rather than greening a cell. Green cells go from 21 to 34 of 90; every remaining warning names a field the target format has no column for.

**Every writer declares what it drops, and the two PowerWorld readers agree on one id rule** ([#330](https://github.com/eigenergy/powerio/issues/330)). The MATPOWER writer declared dropped passthrough extras; the PSS/E, PSLF and PowerWorld writers dropped them silently, and PSS/E to PowerWorld dropped the area table with no warning either. Each writer spells out the keys it replays and carries one warning with the count of elements whose extras fall outside that rule, over every family that can hold extras — buses, branches, loads, shunts, switches, storage, HVDC and 3-winding transformers. The area table gets its own line, since it is a typed field rather than a passthrough. The readers disagreed too: the pwb reader kept the padded `" 1"` circuit and the trimmed ids the aux tokenizer hands back, while the aux reader dropped the positional default, so the pwb to aux leg diffed on whitespace. Device ids and circuits are stored trimmed now, and a value equal to the positional default the writer's allocator re-derives is not retained — **a consumer reading `extras["id"]` off a pwb case finds nothing where a default used to sit.** The conversion matrix baselines do not move; over a 178 file corpus the undeclared extras and area losses go from 34 to 0.

**PSS/E tokenizes both delimiter styles the same way.** The comma path trimmed every field after unquoting while the space path pushed quoted interiors verbatim, so `' 1'` read as `1` from a comma delimited record and as ` 1` from a space delimited one — the same record body, two different field lists. PSS/E string columns are fixed width and blank padded, so the trimmed form is the value and both paths trim. A quoted `/` is text, and a blank quoted field holds its column under either style rather than vanishing and shifting every column after it. Any record that padded a quoted field parses to different values.

**Fixed: an out of service load or shunt wrote as live MATPOWER demand.** A MATPOWER bus row states one demand and one shunt with no status of their own, and the canonical writer folded every element into the row regardless, so a solver reading the output saw load the source had switched off. Only in-service elements reach the row now, and the warning names the MW and MVAr left out. This changes the numbers a MATPOWER write produces for any source with a load status column (PSS/E, PSLF, PowerWorld, pandapower, PyPSA) that has an idle element; a source with none is unaffected. **Fixed: a Surge HVDC link's received power came back negative.** `read_hvdc_link` set `pt = -setpoint`, but `Hvdc::pt` is MATPOWER's PT column, the power arriving at the far end, positive. It is derived from the link's own loss model now, like every other reader here.

**The MCP path containment policy is importable.** `powerio.mcp.sandbox` holds what the server applies to every `path` and `out_path`: `checked_path` decodes a local path or `file://` URI, refuses remote schemes, resolves symlinks — including a dangling final component under `for_write` — and raises unless the result lands under the configured roots. It imports only the standard library, so a server built on another MCP SDK gets the same rules by calling it rather than by reimplementing them, and `powerio.mcp` resolves `main` lazily so reaching the sandbox does not pull in the SDK. `POWERIO_MCP_ALLOWED_ROOTS` is the primary variable; `POWERIO_MCP_ROOT` and `POWERIO_MCP_ALLOWED_ROOT`, an alternate legacy spelling, are read in that order when it is unset.

**Repository.** `scripts/capi-header-regen.sh` regenerates `powerio.h` with cbindgen and diffs it, wired into the `c-abi` job and `scripts/ci-mirror.sh`. The existing parity script compares symbol *names* only, so a reordered argument, a changed type, or a new struct field passed it — the class of defect that once shipped `pio_convert_file` with two arguments reversed and still linking. Published crate tests and benchmarks use only self-contained synthetic fixtures.

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
  bit-identical on case30. [0.10.0 returns the default to 512, measured
  against a sparse direct factorization instead of conjugate gradient;
  see the 0.10.0 entry above.]
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

- The SCOPF wire conversion is structural (#252): every struct reaching
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
  `.pio.json` documents: `transmission:<format>`, `distribution:<format>`,
  `package`, `ambiguous`, or `unknown`, size-then-fill. Bindings can route a
  bare `.json` before choosing a parser instead of matching error text.
- The JSON classifier reports a `.pio.json` document as its own outcome
  (`routing::JsonClass`), so every consumer handles it: the CLI, the Python
  readers, and the Python `classify_json_text` now name the package surface
  for a document instead of a generic cannot-infer error (or, for the Python
  string reader, a MATPOWER syntax error). Document detection requires
  `model_kind` to be `balanced` or `multiconductor`, so a case document
  carrying those key names with other values still classifies as a case, and
  classification parses the document once.
- Directed errors at the transmission boundary: a `.dss` path, a distribution
  `from` token (`dss`/`pmd`/`bmopf`), and a `.pio.json` document handed to the
  balanced parser now name the surface that reads them instead of a generic
  unknown-format message.

## 0.5.1

- `.pio.json` payload schema declared (#173): new optional document fields
  `payload_schema` and `payload_schema_version` name and version the IR payload
  schema id per model kind (`pio-payload-balanced/1`,
  `pio-payload-multiconductor/1`, both `1.0.0`), independent of the document
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
  `powerio.Package` handle class, which parses the document once and exposes
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

- `powerio-pkg`: `.pio.json` reads now enforce the document version rule:
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
