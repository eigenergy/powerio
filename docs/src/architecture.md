# Architecture

How PowerIO is organized as a compiler for power system data: distinct model
families and a `.pio.json` compiler package.

- [compiler-ir.md](compiler-ir.md): the IR layers, the `BalancedNetwork` and
  `MulticonductorNetwork` model families, and the `CompilerPackage` (`.pio.json`)
  envelope — explicit model kind, provenance, source maps, structured
  diagnostics, validation, and lowering.
- [pio-json-schema.md](pio-json-schema.md): the `.pio.json` field reference and
  the stability policy — the envelope is versioned, the nested IR payloads are
  experimental.
- [v0.4-release-direction.md](v0.4-release-direction.md): the design direction
  for explicit `MulticonductorNetwork` to `BalancedNetwork` lowering.

The package is implemented in the `powerio-pkg` crate.
