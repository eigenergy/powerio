# Changelog

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
- `docs/powerworld.md` records the decode evidence, mapping notes, and the
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
- The MCP server grows from two tools to eight (#90): `parse_case`,
  `normalize_case`, and `case_to_json` emit the JSON transport,
  `compute_matrix` returns nine sparse kinds in COO form, `dense_view`
  returns the dense table view, and `save_case` writes converted cases to
  disk; `convert_case` and `case_summary` are unchanged.
- Docs (#92): Pages landing page with the released/development split, guide
  links, and the logo; the crate homepage points at the docs site; release
  drafts carry the CHANGELOG section instead of a bare title.

## 0.0.1

First release.

- Parsers and writers for MATPOWER `.m`, PSS/E RAW, PowerWorld AUX,
  PowerModels JSON, and egret JSON; byte-exact same-format round trips,
  maximal-fidelity conversion between formats.
- `Network`, the one canonical model, with `to_normalized` deriving a
  per-unit / radian / filtered / reindexed view.
- C ABI (`powerio-capi`, ABI version 3): parse, query, convert, JSON
  transport, and Arrow C Data Interface export behind `--features arrow`;
  cbindgen-generated header, version handshake, panic-safe boundary.
- Python bindings (`pip install powerio`) with `matrix`, `graph`, and
  `gridfm` extras, plus an MCP convert/validate server.
- `powerio-matrix`: admittance and Laplacian builders over the parsed
  tables; gridfm Parquet export behind `--features gridfm`.
- `powerio-cli`: convert and validate from the shell.

The C ABI history (versions 1 through 3) is tracked in
[powerio-capi/README.md](powerio-capi/README.md).
