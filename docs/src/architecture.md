# Architecture

PowerIO treats case IO as a compiler pipeline: source formats parse into typed
models, passes derive normalized or lowered views, and writers emit target
artifacts.

- [Compiler IR](https://eigenergy.github.io/powerio/guide/compiler-ir.html): the IR layers, the `BalancedNetwork` and
  `MulticonductorNetwork` model families, and the `NetworkPackage` (`.pio.json`)
  envelope — explicit model kind, provenance, source maps, structured
  diagnostics, validation, operating points, and lowering.
- [PIO JSON schema](https://eigenergy.github.io/powerio/guide/pio-json-schema.html): the `.pio.json` field reference and
  the stability policy. The envelope is versioned; the nested IR payloads still
  follow the Rust models.
- [v0.4 release direction](https://eigenergy.github.io/powerio/guide/v0.4-release-direction.html): the design direction
  for explicit `MulticonductorNetwork` to `BalancedNetwork` lowering.

The package is implemented in the `powerio-pkg` crate.
GOC3 package construction uses `operating_points` to preserve the source time
series while keeping the payload itself static.
