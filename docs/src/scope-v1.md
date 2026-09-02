# 1.0 Scope and Known Limits

PowerIO 1.0 incorporates the API corrections found while exercising the 0.10
beta with external solver consumers. The documents under `docs/design/` are
dated design records, not current API authority; source, tests, and release
documentation state the shipped API.

## In 1.0

- One `parse` operation for every supported source, returning a typed module across networks, series, scenario sets, instances, and solutions.
- Byte exact same format writing, diagnosed cross format conversion, and the explicit multiconductor to balanced transformation.
- Structured diagnostics with stable codes and native record access in every language; the wire form carries span fields end to end, though 1.0 parsers do not yet emit them.
- Balanced matrices (Y bus, FDPF B' and B'', LACPF, incidence, DC operators, AC power flow Jacobians, PTDF and LODF), all carrying element mappings; direct multiconductor admittance assembles in Rust only this release (see Known limits).
- One stored `.pio.json` document with `"schema": "powerio.module"` and
  `"version": 1`. Prerelease PowerIO document shapes are not accepted.
- C ABI 7, the Python package, PowerIO.jl, the `powerio` command line tool,
  and the MCP server over one set of names and types.

## Known limits

- Format profiles are bounded. PyPSA support is the CSV electrical profile: multi carrier components, investment periods, and stochastic data are retained and reported, never typed. Egret support is the scalar network profile with time series; unit commitment fields stay outside it. OpenDSS support is the static circuit; load shapes and solve instructions are retained and reported. PyPSA NetCDF does not parse.
- DOE GO Challenge 3 problem data and DeepMind OPFData are parse only. A
  complete `AcScucSolution` emits the official GO Challenge 3 output file.
  PowerWorld PWB is a parse only binary.
- PowerIO does not solve: instances feed external solvers.
- Balanced to multiconductor construction, load linearized multiconductor admittance from an operating point, and a general multi period planning instance wait for after 1.0.
- Dynamic simulation data has no representation yet; QSTS interchange beyond complete sampled operating point series waits for named instance and solution types.
- There is no one call facade `convert(source, format, destination)`: library callers compose `parse` with `emit`, while the CLI provides `powerio convert`.
- Classifying an undeclared JSON source is one cheap typed pass, except that a document nesting its payload under a `network`, `grid`, `solution`, or `metadata` marker key (GO Challenge 3, Surge) still allocates that subtree once during classification; cost is linear in the nested payload and transient.
- The parser allocation rules in the architecture record (`docs/design/v1-architecture.md`) are implemented for MATPOWER, PSS/E, and PowerWorld AUX; PyPSA CSV, PSLF, and OpenDSS still tokenize through owned strings, and several JSON readers (Egret, GO Challenge 3, DeepMind OPFData, pandapower) decode through a `serde_json::Value` tree. Scheduled work, stated here so the architecture record is not read as already shipped.
- Multiconductor admittance assembly (`powerio_matrix::calc_multiconductor_admittance_matrix`) is Rust only in 1.0: no C entry point, and so no Python or Julia binding yet.
- The sparse direct DC sensitivity factorization trades memory for speed against the previous conjugate gradient path: dense band peak memory is up about 3x at 2000 to 3000 buses for an 8 to 10x wall time win, measured against the committed allocation baseline in `evals/allocation`.

## Version boundaries

One package version covers the Rust crates, the Python package, and PowerIO.jl. The independently checked boundaries are:

| Boundary | Value at 1.0 | Checked where | Moves when |
|---|---|---|---|
| package version | 1.0.0 | manifests, `powerio.versions()`, `build_info` | every release |
| C ABI | 7 | `pio_abi_version` handshake at load | an existing C signature or documented behavior changes |
| `.pio.json` schema | 1 | the stored document header | a future release deliberately defines another document shape |
| matrix Arrow tables | append only, no separate number | the Arrow catalog report, stamped with the package version | an existing table's identity or column order would change (a removed table's id is burned, never reused) |
| MCP electrical data | PowerIO IR version 1 | PowerIO deserialization | with the PowerIO IR schema |

These answer different questions: 1.0.0 is what you install, ABI 7 is what a
compiled consumer must match, schema 1 is what PowerIO documents declare, and
the Arrow catalog is what a table consumer reads before addressing columns.
MCP tools carry the same PowerIO documents rather than defining another
electrical data shape.
