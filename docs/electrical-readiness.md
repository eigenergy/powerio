# Electrical readiness before analysis

`powerio-dist` separates source parsing from the decision to hand a
multiconductor network to numerical analysis. Parsing establishes a typed
model; [`audit_electrical_readiness`](../powerio-dist/src/readiness.rs) checks
whether that model is electrically complete enough for a solver or semantic
lowering pass.

## Fail-closed rule for deferred OpenDSS geometry

OpenDSS can define a line through `geometry=`, `spacing=`, `wires=`,
`cncables=`, or `tscables=`. The geometry family is not yet lowered into
PowerIO's canonical conductor impedance matrices. Until that work is
implemented, these properties are **blockers**, not warnings.

The readiness audit therefore refuses a geometry-backed line even when the
parser has produced a syntactically valid `DistLine`. Applications should
call the audit before numerical lowering or solving and stop when
`is_ready()` is false.

The regression contract explicitly covers **all five** deferred geometry
properties. A future geometry-lowering implementation should change the
corresponding readiness behavior deliberately and update these tests and the
scope documentation together; a parser accepting one of these properties is
not evidence that numerical impedance data exists.

This is intentionally a non-breaking safety boundary. It does not attempt to
implement Carson's equations, infer conductor count from `phases`, or replace
geometry-derived impedance with OpenDSS `Line` factory defaults. Explicit
`linecode=` data remains eligible for analysis when the rest of the network
passes the audit.

## Example

```rust
use powerio_dist::{audit_electrical_readiness, parse};

let module = parse(source)?;
let readiness = audit_electrical_readiness(module.value());
if !readiness.is_ready() {
    for finding in readiness.blockers() {
        eprintln!("{}: {}", finding.code, finding.message);
    }
    return Err("electrical model is not ready for analysis".into());
}
```

The audit is read-only. It also catches unresolved line endpoints, unresolved
linecodes, invalid line lengths, duplicate identifiers, zero-conductor
linecodes, malformed impedance matrices, and non-finite matrix values.

## Scope

This gate addresses the safety half of the deferred-geometry problem tracked
upstream in `eigenergy/powerio#479`. Full typed geometry support remains a
separate implementation: it should calculate the conductor impedance from
the relevant OpenDSS geometry family and preserve the source units and
conductor topology without relying on parser defaults.
