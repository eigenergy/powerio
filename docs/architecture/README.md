# Architecture

The v1 compiler-IR architecture: how PowerIO is organized as a compiler for power
system data, with distinct model families and a `.pio.json` compiler package.

- [compiler-ir.md](compiler-ir.md): the IR layers, the `BalancedNetwork` and
  `MulticonductorNetwork` model families, and the `CompilerPackage` (`.pio.json`)
  envelope — explicit model kind, provenance, source maps, structured
  diagnostics, validation, and lowering.
- [pio-json-schema.md](pio-json-schema.md): the `.pio.json` field reference and
  the stability policy — the envelope is versioned, the nested IR payloads are
  experimental.
- [pr143-reconciliation.md](pr143-reconciliation.md): how this package work was
  rebuilt on top of PR #143 (what #143 owns, what was dropped as duplicate, what
  is carried forward).
- [bmopf-powerio-reconciliation.md](bmopf-powerio-reconciliation.md): the
  field-level map between BMOPF JSON and `MulticonductorNetwork`, grounded in the
  normative draft and BMOPFTools, with the gaps and the diagnostics that cover
  them.
- [migration-v1.md](migration-v1.md): the naming migration across Rust, Python, C
  ABI, Julia, MCP, and CLI.
- [recon.md](recon.md): the original pre-#143 reconnaissance (historical).

## ADRs

- [adr-0001-compiler-package-and-model-families.md](adr-0001-compiler-package-and-model-families.md):
  a compiler package wraps distinct model families.
- [adr-0002-bmopf-and-cim-are-boundaries.md](adr-0002-bmopf-and-cim-are-boundaries.md):
  BMOPF, CIM, and RAVENS are frontends/backends, not the internal IR.

The implementation of the package lives in the `powerio-pkg` crate.
