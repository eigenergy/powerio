# Architecture

PowerIO treats case IO as a compiler pipeline: source formats parse into typed
models, passes derive normalized or lowered views, and writers emit target
artifacts.

- [Compiler IR](compiler-ir.md): the IR layers, the `BalancedNetwork` and
  `MulticonductorNetwork` model families, and the `.pio.json` document
  metadata: explicit model kind, provenance, source maps, structured
  diagnostics, validation, operating points, and lowering.
- [The `.pio.json` format](pio-json-schema.md): what the document is for, the
  field reference, and the stability policy. The metadata and the model JSON
  are versioned independently (`schema_version` vs `payload_schema_version`);
  the model JSON shape follows the Rust models.

The `.pio.json` document APIs are implemented in the `powerio-pkg` crate.
GOC3 document construction uses `operating_points` to preserve the source time
series while keeping the model JSON itself static.
