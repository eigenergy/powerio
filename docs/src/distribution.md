# Distribution networks

A multiconductor network is the conductor level distribution model. OpenDSS,
PowerModelsDistribution engineering JSON, and BMOPF JSON parse to it.
Construct `McAcPfInstance` or `McAcOpfInstance` explicitly when a calculation
is required ([Calculation instances and solutions](instances.md)).

```julia
using PowerIO
feeder = parse("IEEE13Nodeckt.dss")        # PioModule{MulticonductorNetwork}
net = feeder.value
net.lines[1]                               # terminal maps and the line code reference
net.linecodes[1]                           # per length impedance matrices, SI units
feeder.diagnostics                         # what the reader kept, assumed, or refused
```

The model keeps conductor identity everywhere. Buses carry ordered terminals
and explicit grounding. Lines and switches map conductors between terminal
sets. Transformers state windings with connection kinds. Loads and generators
attach per terminal. Impedance and shunt matrices are SI, per unit length on
line codes. An element the reader has no typed slot for stays verbatim in the
`untyped` table and is reported.

The OpenDSS profile is the static circuit: element definitions and their
electrical data. Load shapes, solve commands, monitors, and other calculation
instructions are outside it. They stay in the retained source, are reported
as uninterpreted, and survive a same format write byte for byte.

```julia
emit(feeder, "dss", "copy.dss")            # the source bytes, sidecars included
result = emit(feeder, "pmd")               # fresh PMD JSON
result.text
result.diagnostics
```

There is no implicit conversion between the multiconductor and balanced
models. The balanced positive sequence equivalent is an explicit
transformation that states its assumptions, such as voltage bases per zone,
phase aggregation, and switch merging, and refuses what it cannot state:

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
