# Distribution Networks

A multiconductor network is the conductor level distribution model. OpenDSS and PowerModelsDistribution engineering JSON parse to it; BMOPF JSON, which also defines an optimization calculation, parses to the calculation input that shares one ([Problem Instances and Solutions](instances.md)).

```julia
using PowerIO
feeder = parse_file("IEEE13Nodeckt.dss")   # PioModule{MulticonductorNetwork}
net = feeder.value
net.data.lines[1]                   # terminal maps, linecode reference
net.data.linecodes[1]               # per length impedance matrices, SI units
diagnostics(feeder)                 # what the reader kept, assumed, or refused
```

The model keeps conductor identity everywhere: buses carry ordered terminals and explicit grounding, lines and switches map conductors between terminal sets, transformers state windings with connection kinds, and loads and generators attach per terminal. Impedance and shunt matrices are SI, per unit length on line codes. Elements the reader has no typed slot for stay verbatim in the `untyped` table and are reported.

The OpenDSS profile is the static circuit: element definitions and their electrical data. Load shapes, solve commands, monitors, and other calculation instructions are outside that profile; they stay in the retained source, are reported as uninterpreted, and survive a same format write byte for byte.

```julia
write_file(feeder, "copy.dss")                  # byte exact, sidecars included
text, findings = to_format(net, "pmd")          # cross format + reported losses
```

There is no implicit conversion between the multiconductor and balanced models. The balanced equivalent is an explicit lossy lowering that states its assumptions (voltage bases per zone, phase aggregation, switch merging) and refuses what it cannot state:

```julia
report = lowering_readiness(feeder)         # the losses, before transforming
low = lower_to_balanced(feeder)             # PioModule{BalancedNetwork}
```

Multiconductor admittance matrices build directly from the multiconductor network, without lowering, through `powerio_matrix::build_multiconductor_admittance`. This entry point is Rust only in 0.10: there is no C ABI entry point yet, so no Python or Julia binding exists either.
