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

1. Detect a `Line` that references `geometry=`, `spacing=`, `wires=`,
   `cncables=`, or `tscables=`.
2. Do not synthesize a `DistLineCode` from OpenDSS `Line` factory defaults.
3. Do not infer/fabricate conductor count from the balanced `phases` default.
4. Emit a **parse-time diagnostic** identifying the deferred geometry source.
5. Preserve the original source object so same-format DSS echo remains lossless.
6. Keep explicit `linecode=` behavior unchanged.
7. A one-conductor SWER geometry must never become a three-conductor normalized line.

PowerIO represents this state explicitly: the `DistLine` remains in the
network, with its source-derived terminal maps and `linecode: None`. Parsing
records an error diagnostic because the typed network is not electrically
complete. A same-format DSS echo still returns the retained source exactly;
analysis and semantic format writers refuse the unresolved impedance, and no
writer supplies a placeholder linecode or electrical default.

## Regression cases

The regression harness in `powerio-dist/tests/dss_geometry_safety.rs` covers:

- a 3-phase overhead `LineGeometry` backed by `WireData`;
- a single-conductor SWER `LineGeometry`;
- a 4-conductor `LineSpacing` plus `wires` definition;
- parse-time diagnostics for deferred geometry;
- protection against OpenDSS factory-default `R1` leaking into a geometry line.

These are ordinary regression tests. They also verify that retained DSS echoes
exactly and that every semantic output path reports unresolved impedance as an
error instead of inventing data.

## Upstream context

This work addresses the safety boundary described in
[eigenergy/powerio#479](https://github.com/eigenergy/powerio/issues/479), while
remaining separate from full typed geometry support tracked in #84.
