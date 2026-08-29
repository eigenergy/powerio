# LLVM and MLIR lessons

PowerIO's design borrows from LLVM and MLIR where their problems genuinely overlap with reading, transforming, and writing power system data. This page records each adopted lesson against the shipped design, and the mechanisms deliberately not adopted. Primary references: the MLIR [language reference](https://mlir.llvm.org/docs/LangRef/), [diagnostics](https://mlir.llvm.org/docs/Diagnostics/), [interfaces](https://mlir.llvm.org/docs/Interfaces/), [pass management](https://mlir.llvm.org/docs/PassManagement/), and [dialect definition](https://mlir.llvm.org/docs/DefiningDialects/) documents.

## Adopted

**A small shared foundation under acyclic higher layers.** LLVM's library layering puts Support under IR under the producers. PowerIO's `powerio-core` owns sources, diagnostics, errors, the module, and the generic containers; the network crates, the calculation crate, and the matrix crate stack over it in one direction, and CI asserts the edges from `cargo metadata` ([Crate graph](crate-graph.md)).

**Source ownership that survives parsing.** MLIR's source manager keeps buffers alive so locations mean something after parsing. A `PioModule` retains its source, and the byte exact same format echo reads it back; the diagnostic wire form carries a source identifier plus a byte range into those exact bytes end to end, though 0.10 parsers do not yet emit a span on any diagnostic.

**Structured diagnostics with stable severities and attached context.** The four severities (`error`, `warning`, `remark`, `note`) are MLIR's, with the same meanings: a remark reports on success, a note attaches context to another finding. PowerIO adds the stable dotted code as the identity a consumer branches on, plus targets, related records, and suggested actions.

**Typed representations at more than one abstraction level.** The value families span reusable networks, calculation instances, and solutions the way a compiler holds IR at several levels; nothing forces the richer levels through the poorer ones.

**Explicit transformations with testable boundaries.** Every transformation names its input and output types, returns diagnostics, and refuses what it cannot state (the balanced lowering's readiness report is the preflight MLIR's dialect conversion legality check suggests). Nothing rewrites as a side effect. A writer has no matching legality preflight yet; a format's losses surface in the write result itself rather than in a check a caller can run beforehand.

**Verification at representation boundaries.** Parsers and transformations verify what they produce and report findings rather than repairing silently; repairs are explicit operations that leave history records.

**Shared operations instead of per format switches.** The parse and write dispatchers route once, at the facade; matrix builders, writers, and inspectors consume the concrete typed values, so a new format adds one parser and one writer rather than a case in every consumer.

**Analysis caches.** Factorizations and prepared solver arrays are derived data behind the public results, invalidated when their inputs change, the way pass manager analyses are; `IndexedNetwork`, the derived index view, stays public in 0.10 because downstream consumers build matrices through it directly.

**Registries checked mechanically where tables drift.** The twenty kind strings, the format tokens, the diagnostic codes, and the drawn architecture edges are each held to one source by a CI gate, which is the maintainable slice of MLIR's declarative dialect definitions.

**Serialization versioned apart from memory.** `.pio.json` version 1 is a schema with its own upgrade rules, in the spirit of MLIR bytecode versioning; the Rust structs never derive the wire layout.

**Scrutiny proportional to permanence.** A new core concept (a value family, a common module record) needs the promotion checklist: variant, stable string, stored DTO, and binding coverage. A new format adapter needs none of that.

## Not adopted

PowerIO 0.10 has no SSA values, no generic operation tree, no region nesting, no global context, no open runtime dialect registry, no generic pass manager, and no bytecode. The existing Rust types state power system data more directly than an operation tree would, and none of those mechanisms has a PowerIO use with measured benefit. Public names stay power system names: a bus is a bus, a lowering names its concrete result, and no Rust struct is renamed an operation to resemble MLIR. A `PioContext` would be justified only by measured interning or shared allocation needs, and none has appeared.
