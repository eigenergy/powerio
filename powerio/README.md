# powerio

The PowerIO entry facade. `cargo add powerio` is the complete surface: it
re-exports the component crates and owns `PioValue`, format dispatch, and
PowerIO IR.

The component crates are usable on their own: `powerio-core` (sources,
diagnostics, errors, modules), `powerio-tx` (the balanced transmission model
and its format parsers and writers), `powerio-dist` (the multiconductor
distribution model), `powerio-prob` (operating points, problem instances, and
solutions), and `powerio-matrix` (matrix and graph data).
