# Architecture map

Two checked diagrams: the component map and the parse-to-write data flow. Their sources live in `docs/diagrams/*.dot`; `scripts/check-architecture-map.py` validates the drawn crate edges against `cargo metadata` and fails CI when a committed image drifts from its source. The prose equivalent of the component map is [Crate graph and dependency rules](crate-graph.md), so the architecture reads without the graphics.

## Components

![The PowerIO component map: powerio-core at the bottom; powerio-tx and powerio-dist as independent siblings above it; powerio-prob over both networks; powerio-matrix over the networks and prob; the powerio facade over the component crates with an optional matrix feature edge; powerio-cli, powerio-capi, and powerio-py over the facade; solvers consume instances from powerio-prob and the GridFM pipeline reads Parquet datasets from powerio-matrix.](assets/architecture.svg)

What the picture is drawn to show: the facade does not force a consumer to depend on every component crate (a distribution only consumer uses `powerio-dist` over `powerio-core` and nothing else), and the value families are not one universal format (the two network crates are siblings with no edge between them). Solvers sit outside: they consume instances, and PowerIO never grows solver state.

## Data flow

![The PowerIO data flow: a Source of named immutable bytes enters parse, which produces a PioModule holding the typed value, retained source, diagnostics, and history; state selection loops modules over series and scenario sets; the explicit lossy lowering turns a multiconductor module into a balanced one; matrix data with element mappings and format writing (byte exact same format, reported losses cross format) leave the module; the stored .pio.json document round trips with it.](assets/dataflow.svg)

Every arrow out of the module is an explicit operation returning diagnostics; nothing transforms as a side effect of something else.
