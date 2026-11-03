# powerio-tnd

Integrated transmission/distribution modeling for PowerIO.

`IntegratedNetwork` composes a balanced transmission module with named
multiconductor feeder modules through checked `Interface` records. Interfaces
state phase order, voltage ratio and angle, and how feeder demand substitutes
transmission loads. `boundary_voltages` uses per-unit transmission voltage and
returns phase-to-earth volts; `boundary_power` returns MW + j MVAr.

`transform` owns the explicit multiconductor-to-balanced transformation.
`formats::sincal` provides the initial format adapters behind the optional
`sincal` feature. The default build needs no SQLite. Component modules retain
their source bytes and provenance. Coupled equation assembly and solvers are
separate consumers of this model.

See the [integration guide](https://eigenergy.github.io/powerio/tnd.html) for the
boundary rules, migration paths, and planned native/PowerModelsITD support.
