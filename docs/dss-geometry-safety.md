# Safe handling of deferred OpenDSS line geometry

## Problem

OpenDSS line impedance can be defined indirectly through the geometry family:
`LineGeometry`, `LineSpacing`, `WireData`, `CNData`, and `TSData`. These classes
are not yet lowered into the canonical multiconductor impedance model.

A deferred source concept must not be normalized as though the user had omitted
impedance data. In particular, the OpenDSS `Line` constructor defaults
(`R1=0.058`, `X1=0.1206`, `R0=0.1784`, `X0=0.4047`) are not valid substitutes for
a geometry-defined line.

## Safety invariant

Until Carson/geometry impedance calculation is implemented:

1. Detect a `Line` that references `geometry=`, `spacing=`, or `wires=`.
2. Do not synthesize a `DistLineCode` from OpenDSS `Line` factory defaults.
3. Do not infer/fabricate conductor count from the balanced `phases` default.
4. Emit a **parse-time diagnostic** identifying the deferred geometry source.
5. Preserve the original source object so same-format DSS echo remains lossless.
6. Keep explicit `linecode=` behavior unchanged.
7. A one-conductor SWER geometry must never become a three-conductor normalized line.

The preferred long-term representation is an explicit unresolved/deferred line
reference in the canonical model. If that representation is not yet available,
the reader should fail closed rather than manufacture electrical parameters.

## Regression cases

The regression harness in `powerio-dist/tests/dss_geometry_safety.rs` covers:

- a 3-phase overhead `LineGeometry` backed by `WireData`;
- a single-conductor SWER `LineGeometry`;
- parse-time diagnostics for deferred geometry;
- protection against OpenDSS factory-default `R1` leaking into a geometry line.

The tests are currently `#[ignore]` because they encode the target invariant while
the canonical model's deferred-line representation is being finalized. They are
intended to become ordinary tests in the implementation PR.

## Upstream context

This work addresses the safety boundary described in
[eigenergy/powerio#479](https://github.com/eigenergy/powerio/issues/479), while
remaining separate from full typed geometry support tracked in #84.
