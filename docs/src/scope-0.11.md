# Known limits and versions

## Known limits

- Format profiles are bounded. PyPSA support covers the CSV electrical
  profile; multi carrier components, investment periods, and stochastic data
  are retained and reported rather than typed, and PyPSA NetCDF does not
  parse. Egret support is the scalar network profile with time series, with
  unit commitment fields outside it. OpenDSS support is the static circuit;
  load shapes and solve instructions are retained and reported.
- DOE GO Challenge 3 problem data, DeepMind OPFData, PowerWorld PWB, and the
  IEEE Common Data Format are parse only, though a complete `AcScucSolution`
  does emit the official GO Challenge 3 solution file.
- PowerIO does not solve anything; instances feed external solvers.
- Balanced to multiconductor construction, load linearized multiconductor
  admittance from an operating point, and a general multi period planning
  instance are waiting on a later release, and dynamic simulation data has no
  representation.
- The multiconductor to balanced transformation is Rust and Python only.
  Multiconductor admittance assembly,
  `powerio_matrix::calc_multiconductor_admittance_matrix`, is Rust only.
- Only the MATPOWER and PSS/E readers attach source spans, marking the record
  each finding is about. Findings from the other readers, from
  transformations, and from writers have no byte range, and the text rendering
  of a diagnostic is still `CODE: message`.
- There is no one call `convert` in the library; you compose `parse` with
  `emit`. The command line does have `powerio convert`.
- The MATPOWER, PSS/E, and PowerWorld AUX readers tokenize without owned
  strings. PyPSA CSV, PSLF, and OpenDSS tokenize through owned strings, and
  the Egret, GO Challenge 3, OPFData, and pandapower readers decode through a
  generic JSON value tree first.
- Two sets of field names coexist. Rust records and Python dict rows use the
  MATPOWER derived names (`vm`, `pg`, `rate_a`, `tap`); C ABI 7 and Julia
  spell out the quantity and unit (`vm_pu`, `active_power_mw`, `rate_a_mva`,
  `tap_ratio`). Settling on one set of names for all four languages is a 1.0
  decision, and it is separate from the PowerIO IR keys, which change only
  with a generation.
- Python tables return dict rows; typed element views like Julia's are not
  offered yet.
- `to_balanced` and `to_balanced_report` live in `powerio::transform` rather
  than the facade root, and `serialize_diagnostics` returns a `String` where
  `serialize` returns an `EmitResult`.
- `SocwrOpfSolution` calls its branch flows `branch_from_active_power` where
  the other solutions say `branch_from_active_flow`, and its `bus_order` and
  `branch_order` are iterators rather than vectors. PowerIO IR generation 2
  has the same split in its keys.

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
has to match. The IR generation identifies the serialized document; its rule
is in [PowerIO IR](pio-json-schema.md). The MCP server passes PowerIO IR
documents through and defines no electrical data shape of its own.

The 0.11.x line is for compatible fixes, performance work, and additive
change; a break in the public Rust API that cannot be avoided goes into 0.12.
The Rust API stays pre-1.0 as long as public signatures expose types from the
pre-1.0 `sprs` and `petgraph` crates.
