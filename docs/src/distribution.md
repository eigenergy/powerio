# Distribution Networks

A multiconductor network is the conductor level distribution model. OpenDSS and PowerModelsDistribution engineering JSON parse to it; BMOPF JSON, which also defines an optimization calculation, parses to the calculation input that shares one ([Problem Instances and Solutions](instances.md)).

```julia
using PowerIO
feeder = parse_file("IEEE13Nodeckt.dss")   # PioModule{MulticonductorNetwork}
net = feeder.value
net.data.lines[1]                   # terminal maps, linecode reference
net.data.linecodes[1]               # per length impedance matrices, SI units
feeder.diagnostics                  # what the reader kept, assumed, or refused
```

The model keeps conductor identity everywhere: buses carry ordered terminals and explicit grounding, lines and switches map conductors between terminal sets, transformers state windings with connection kinds, and loads and generators attach per terminal. Impedance and shunt matrices are SI, per unit length on line codes. Elements the reader has no typed slot for stay verbatim in the `untyped` table and are reported.

The OpenDSS profile is the static circuit: element definitions and their electrical data. Load shapes, solve commands, monitors, and other calculation instructions are outside that profile; they stay in the retained source, are reported as uninterpreted, and survive a same format write byte for byte.

```julia
emit(feeder, "dss", "copy.dss")                 # byte exact, sidecars included
result = emit(feeder, "pmd")                     # cross format + reported losses
result.text
result.diagnostics
```

There is no implicit conversion between the multiconductor and balanced models. The balanced positive sequence equivalent is an explicit transformation that states its assumptions (voltage bases per zone, phase aggregation, switch merging) and refuses what it cannot state:

```julia
report = to_balanced_report(feeder)         # ready plus assumptions, losses, and diagnostics
balanced = to_balanced(feeder)              # PioModule{BalancedNetwork}
```

Multiconductor admittance matrices build directly from the multiconductor network, without this transformation, through `powerio_matrix::calc_multiconductor_admittance_matrix`. This entry point is Rust only in 1.0: there is no C ABI entry point yet, so no Python or Julia binding exists either.
