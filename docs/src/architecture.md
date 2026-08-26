# Architecture

Source formats parse into typed network models. Normalization, lowering,
matrix projection, package construction, and problem instance assembly consume
those models without changing parser dependencies.

```text
powerio-tx          powerio-dist
   │                     │
   ├──────► powerio-matrix
   │
   ├──────► powerio (facade) ◄──── powerio-dist
   │
   └──────► powerio-prob
                 │
                 └── optional "matrix" ──► powerio-matrix

powerio-core ◄──── powerio-tx, powerio-dist, powerio
```

- `powerio-core` owns the diagnostic record, the code grammar, the stage
  namespaces, and the error categories. It is a leaf below both model crates so
  one finding type crosses all of them.
- `powerio-tx` owns the balanced network model, format routing, indexing,
  normalization, and shared GOC3 document parsing. The `powerio` facade
  re-exports it and adds the dynamic value boundary and universal parse.
- `powerio-dist` owns the multiconductor network model and distribution
  formats.
- `powerio-matrix` owns generic sparse matrix and graph projections from a
  balanced network. It does not depend on `powerio-prob`.
- the `powerio` facade's `package` module owns `.pio.json` packages,
  operating points, study commits, provenance, validation, and lowering
  between model families.
- `powerio-prob` owns complete numerical problem instances. Its default build
  depends on `powerio`; the optional `matrix` feature projects a DC OPF
  instance into sparse operators. It has no `powerio-dist` dependency because
  no distribution problem instance is implemented.
- `powerio-cli`, `powerio-py`, and `powerio-capi` depend on the layers they
  expose.

A problem instance contains the complete indexed input for a problem family:
coefficients, bounds, mappings, units, and conventions. It is not a source
network, matrix projection, solver formulation, or solution. The current crate
provides `DcOpfInstance` and `ScopfInstance`.

[Compiler model layers](compiler-ir.md) describes the balanced and
multiconductor payloads. [`.pio.json` format](pio-json-schema.md) defines the
package metadata and its independent payload versioning.
