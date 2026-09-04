# Known limits and versions

## Known limits

- Format profiles are bounded. PyPSA support is the CSV electrical profile:
  multi carrier components, investment periods, and stochastic data are
  retained and reported, never typed; PyPSA NetCDF does not parse. Egret
  support is the scalar network profile with time series; unit commitment
  fields stay outside it. OpenDSS support is the static circuit; load shapes
  and solve instructions are retained and reported.
- DOE GO Challenge 3 problem data and DeepMind OPFData are parse only. A
  complete `AcScucSolution` emits the official GO Challenge 3 solution file.
  PowerWorld PWB and the IEEE Common Data Format are parse only.
- PowerIO does not solve. Instances feed external solvers.
- Balanced to multiconductor construction, load linearized multiconductor
  admittance from an operating point, and a general multi period planning
  instance wait for a later release. Dynamic simulation data has no
  representation.
- The multiconductor to balanced transformation is Rust and Python only.
  Multiconductor admittance assembly,
  `powerio_matrix::calc_multiconductor_admittance_matrix`, is Rust only.
- Diagnostic source spans come from the MATPOWER and PSS/E readers, which
  mark the record each finding is about. Findings from the other readers,
  from transformations, and from writers carry no byte range, and the text
  rendering of a diagnostic stays `CODE: message`.
- There is no one call library `convert`. Library callers compose `parse`
  with `emit`; the command line has `powerio convert`.
- The MATPOWER, PSS/E, and PowerWorld AUX readers tokenize without owned
  strings. PyPSA CSV, PSLF, and OpenDSS tokenize through owned strings, and
  the Egret, GO Challenge 3, OPFData, and pandapower readers decode through a
  generic JSON value tree first.

## Versions

One PowerIO release version covers the Rust crates, the Python distribution,
and PowerIO.jl. The boundaries checked independently are:

| Boundary | Value at 0.11.0 | Checked where | Moves when |
|---|---|---|---|
| PowerIO release | 0.11.0 | the manifests, `powerio::VERSION`, `powerio.versions()`, `pio_version` | every release |
| C ABI | 7 | the `pio_abi_version` handshake at load | an existing C signature or documented behavior changes |
| PowerIO IR generation | 2, and the reader accepts 2 | the document header, `powerio::IR_VERSION`, `powerio::IR_MIN_VERSION` | the serialized representation changes |
| Rust toolchain | 1.88 | `rust-version` in the workspace manifest, checked by CI | a dependency in the locked graph requires a newer compiler |
| Python | 3.9 or later; the `mcp` extra needs 3.10, the `bench` extra 3.11 | `pyproject.toml` | a dependency drops a version |

The release version is what you install. ABI 7 is what a compiled consumer
must match. The IR generation identifies the serialized document, and its
rule is stated in [PowerIO IR](pio-json-schema.md). The MCP server carries
PowerIO IR documents and defines no electrical data shape of its own.

The 0.11.x line is reserved for compatible fixes, performance work, and
additive change. An unavoidable break in the public Rust API moves to 0.12.
The Rust API stays pre-1.0 while public signatures expose types from the
pre-1.0 `sprs` and `petgraph` crates.
