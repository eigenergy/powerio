# Distribution networks

A multiconductor network is the conductor level distribution model, and
OpenDSS, PowerModelsDistribution engineering JSON, and BMOPF JSON all parse to
it. When you need a calculation, construct an `McAcPfInstance` or
`McAcOpfInstance` from it explicitly (see
[Calculation instances and solutions](instances.md)).

```julia
using PowerIO
feeder = parse("IEEE13Nodeckt.dss")        # PioModule{MulticonductorNetwork}
net = feeder.value
net.lines[1]                               # terminal maps and the line code reference
net.linecodes[1]                           # per length impedance matrices, SI units
feeder.diagnostics                         # what the reader kept, assumed, or refused
```

The model identifies individual conductors throughout. Buses have ordered
terminals and explicit grounding, lines and switches map conductors between
terminal sets, transformers list their windings with connection kinds, and
loads and generators attach per terminal. Impedance and shunt matrices are in
SI units, per unit length, on line codes. An element the reader has no typed
slot for stays verbatim in the `untyped` table and is reported.

The OpenDSS profile is the static circuit, meaning the element definitions
and their electrical data. Load shapes, solve commands, monitors, and other
calculation instructions are outside it; they stay in the retained source,
are reported as uninterpreted, and survive a same format write byte for byte.

```julia
emit(feeder, "dss", "copy.dss")            # the source bytes, sidecars included
result = emit(feeder, "pmd")               # fresh PMD JSON
result.text
result.diagnostics
```

Nothing converts between the multiconductor and balanced models implicitly.
The balanced positive sequence equivalent is an explicit transformation that
reports its assumptions, such as voltage bases per zone, phase aggregation,
and switch merging, and refuses what it cannot represent:

```rust,ignore
use powerio::transform::{to_balanced, to_balanced_report};

let report = to_balanced_report(&feeder)?;   // readiness, assumptions, losses, diagnostics
let balanced = to_balanced(&feeder)?;        // PioModule<BalancedNetwork>
```

Python exposes the same pair as `module.to_balanced_report()` and
`module.to_balanced()`. The transformation has no C entry point in 0.11, so
PowerIO.jl does not bind it.

Multiconductor admittance matrices build directly from the multiconductor
network through `powerio_matrix::calc_multiconductor_admittance_matrix`,
which is Rust only in 0.11.
