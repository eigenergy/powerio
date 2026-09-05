# Electrical readiness before analysis

Parsing, preservation and numerical readiness are separate results.
`powerio_dist::audit_electrical_readiness` reports structural blockers and
source defaults. `require_electrical_readiness` rejects the first blocker.
Multiconductor calculation-instance constructors and admittance assembly call
that check before numerical use.

## OpenDSS line geometry

PowerIO does not calculate conductor impedances from OpenDSS `geometry`,
`spacing`, `wires`, `cncables` or `tscables`. The reader retains a line using any
of these properties as an untyped source object and reports
`READ.DSS.GEOMETRY_UNRESOLVED` with error severity. It creates neither a typed
line nor a synthetic linecode or terminal map for that object. This also
applies when geometry properties and an explicit linecode occur together.

The original, unmodified source module can still emit its exact source bytes.
IR retains the untyped object, but canonical OpenDSS, PMD and BMOPF conversion
reject unresolved geometry because they cannot reconstruct its electrical
meaning. Retain the source module or provide explicit conductor impedances
before requesting those conversions. Numerical calculations reject both these
untyped records and geometry-dependent typed lines from earlier IR documents.

This intentionally rejects calculations and canonical conversions that could
otherwise use incomplete electrical data. It does not implement Carson's
equations or infer conductor count from a phase count. Explicit linecode data
without geometry-dependent properties remains eligible when the remaining
checks pass.

## Other structural checks

The audit checks source-specific identifier equality, unresolved line endpoints
and linecodes, terminal references, line lengths, conductor counts, matrix
shapes and finite matrix entries. It reports source defaults as warnings.

```rust
let readiness = powerio_dist::audit_electrical_readiness(network);
for finding in readiness.findings {
    eprintln!("{} {}: {}", finding.code, finding.element, finding.message);
}
powerio_dist::require_electrical_readiness(network)?;
```

A passing audit establishes only these structural properties. The selected
calculation still checks its support for transformer connections, controls,
loads and other required physics. PowerIO preserves data that a particular
calculation may not support.
