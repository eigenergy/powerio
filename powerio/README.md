# powerio

The PowerIO facade crate. `cargo add powerio` gets you everything: it
re-exports the component crates and owns `PioValue`, format dispatch, and
PowerIO IR.

You can also use the component crates on their own: `powerio-core` (sources,
diagnostics, errors, modules), `powerio-tx` (the balanced transmission model
and its format parsers and writers), `powerio-dist` (the multiconductor
distribution model), `powerio-prob` (operating points, problem instances, and
solutions), and `powerio-matrix` (matrix and graph data).
