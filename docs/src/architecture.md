# Architecture

PowerIO treats case IO as a compiler pipeline: source formats parse into typed
models, passes derive normalized or lowered views, and writers emit target
artifacts.

- [Compiler IR](https://eigenergy.github.io/powerio/guide/compiler-ir.html): the IR layers, the `BalancedNetwork` and
  `MulticonductorNetwork` model families, and the `NetworkPackage` (`.pio.json`)
  envelope — explicit model kind, provenance, source maps, structured
  diagnostics, validation, operating points, and lowering.
- [PIO JSON schema](https://eigenergy.github.io/powerio/guide/pio-json-schema.html): the `.pio.json` field reference and
  the stability policy. The envelope and the IR payload are versioned
  independently (`schema_version` vs `payload_schema_version`); the payload
  shape follows the Rust models.

The package is implemented in the `powerio-pkg` crate.
GOC3 package construction uses `operating_points` to preserve the source time
series while keeping the payload itself static.
