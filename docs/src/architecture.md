# Architecture

How PowerIO is organized as a compiler for power system data: distinct model
families and a `.pio.json` compiler package.

- [Compiler IR](https://eigenergy.github.io/powerio/guide/compiler-ir.html): the IR layers, the `BalancedNetwork` and
  `MulticonductorNetwork` model families, and the `CompilerPackage` (`.pio.json`)
  envelope — explicit model kind, provenance, source maps, structured
  diagnostics, validation, and lowering.
- [PIO JSON schema](https://eigenergy.github.io/powerio/guide/pio-json-schema.html): the `.pio.json` field reference and
  the stability policy — the envelope is versioned, the nested IR payloads are
  experimental.
- [v0.4 release direction](https://eigenergy.github.io/powerio/guide/v0.4-release-direction.html): the design direction
  for explicit `MulticonductorNetwork` to `BalancedNetwork` lowering.

The package is implemented in the `powerio-pkg` crate.
