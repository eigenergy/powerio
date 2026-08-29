# 0.10 Beta Scope and Known Limits

PowerIO 0.10 is the public beta of the 1.0 API. API corrections may land before 1.0.0 as downstream integrations exercise the new design. The architecture documents under `arch-v1/` describe the 1.0 target; 0.10 implements it and asks for real world use before the permanent freeze.

## In this beta

- One parse for every supported source, returning the typed module; twenty built in value kinds across networks, series, scenario sets, instances, and solutions.
- Byte exact same format writing, diagnosed cross format conversion, and the explicit multiconductor to balanced lowering.
- Structured diagnostics with stable codes, native record access in every language, and byte spans into the retained source.
- Balanced matrices (Y bus, FDPF B' and B'', LACPF, incidence, DC operators, AC power flow Jacobians, PTDF and LODF) and direct multiconductor admittance, all carrying element mappings.
- The stored `.pio.json` document, version 1, with the one way upgrade from released 0.9 documents.
- C ABI 6, the Python package, PowerIO.jl, the `powerio` command line tool, and the MCP server over one set of names.

## Known limits

- Format profiles are bounded. PyPSA support is the CSV electrical profile: multi carrier components, investment periods, and stochastic data are retained and reported, never typed. Egret support is the scalar network profile with time series; unit commitment fields stay outside it. OpenDSS support is the static circuit; load shapes and solve instructions are retained and reported. PyPSA NetCDF does not parse.
- DOE GO Challenge 3 and DeepMind OPFData are parse only; PowerWorld PWB is a parse only binary.
- Solving is out of scope permanently: instances feed external solvers.
- Balanced to multiconductor construction, load linearized multiconductor admittance from an operating point, and a general multi period planning instance wait for after 1.0.
- Dynamic simulation data has no representation yet; QSTS interchange beyond complete sampled operating point series waits for named instance and solution types.

## Version boundaries

One package version covers the Rust crates, the Python package, and PowerIO.jl. The independently versioned compatibility boundaries:

| Boundary | Value at 0.10 | Checked where | Moves when |
|---|---|---|---|
| package version | 0.10.0 | manifests, `powerio.versions()`, `build_info` | every release |
| C ABI | 6 | `pio_abi_version` handshake at load | an existing C signature or documented behavior changes |
| `.pio.json` schema | 1 | the stored document header | a document version 1 cannot represent is needed |
| matrix Arrow tables | append only, no separate number | the Arrow catalog report, stamped with the package version | an existing table's identity or column order would change (a removed table's id is burned, never reused) |

These answer different questions and never race each other: 0.10.0 is what you install, ABI 6 is what a compiled consumer must match, schema 1 is what stored documents declare, and the Arrow catalog is the one report a table consumer reads before addressing columns.
