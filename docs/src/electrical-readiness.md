# Electrical readiness before analysis

Parsing, preservation and numerical readiness are separate results.
`powerio_dist::audit_electrical_readiness` reports structural blockers and
source defaults. `require_electrical_readiness` rejects the first blocker.
Multiconductor calculation-instance constructors and admittance assembly call
that check before numerical use.

## OpenDSS line geometry

The dss reader calculates conductor impedances from OpenDSS `geometry`,
`spacing`, `wires`, `cncables` and `tscables` with the pinned engine's line
constants: overhead conductors, concentric-neutral cables (`CNData`) and
tape-shielded cables (`TSData`), under the earth model and resistivity the
source states. A line whose geometry falls outside that calculation, for
example one mixing `CNData` and `TSData` cables, is retained as an untyped
source object and reported as `READ.DSS.GEOMETRY_UNRESOLVED` with error
severity, naming the reason. The reader creates neither a typed line nor a
synthetic linecode or terminal map for that object.

The original, unmodified source module can still emit its exact source bytes.
IR retains the untyped object, but canonical OpenDSS, PMD and BMOPF conversion
reject unresolved geometry because they cannot reconstruct its electrical
meaning. Retain the source module or provide explicit conductor impedances
before requesting those conversions. Numerical calculations reject both these
untyped records and geometry-dependent typed lines from earlier IR documents.

This intentionally rejects calculations and canonical conversions that could
otherwise use incomplete electrical data. It does not infer conductor count
from a phase count. Explicit linecode data without geometry-dependent
properties remains eligible when the remaining checks pass.

## Distribution CIM segments without impedance

A CIM `ACLineSegment` whose impedance the reader cannot read, for example one
on a `WireSpacingInfo` with a position no `ACLineSegmentPhase` names a wire
for, parses with a zero placeholder
line code and an error finding, `READ.CIM_DIST.IMPEDANCE_UNRESOLVED`. The
audit blocks it as `READINESS.CIM_DIST.IMPEDANCE_UNRESOLVED`, because the
placeholder would be a short circuit between its buses. Numerical
calculations and conversion to another format reject it, as they reject an
OpenDSS geometry line, and a same format write of a single source document
still returns its bytes.

## MG-RAVENS equipment the reader cannot type

The MG-RAVENS reader keeps an in-service `ACLineSegment`, `Switch`,
`TransformerTank` or `EnergyConsumer` it cannot type (an incomplete wire
catalog, fewer than two terminals, a tank without a datasheet or with
phase clocks neither `leadlag` convention represents, an unresolvable delta
load) as an untyped object, with a `READ.RAVENS_DIST.RETAINED_SOURCE_ONLY`
warning naming the reason. It round trips to MG-RAVENS. The audit blocks
it as `READINESS.RAVENS_DIST.EQUIPMENT_UNRESOLVED`, so numerical
calculations and conversion to another format refuse it instead of writing
a feeder with the equipment missing. Out-of-service equipment is retained
the same way but not blocked, since it contributes no network equation.

## Other structural checks

The audit checks source-specific identifier equality, unresolved line endpoints
and linecodes, terminal references, line lengths, conductor counts, matrix
shapes and finite matrix entries. It reports source defaults as warnings.

```rust,ignore
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
