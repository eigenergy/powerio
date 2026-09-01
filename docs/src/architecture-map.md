# Architecture map

Two checked diagrams: the component map and the parse-to-write data flow. Their sources live in `docs/diagrams/*.dot`; `scripts/check-architecture-map.py` validates the drawn crate edges against `cargo metadata` and fails CI when a committed image drifts from its source. The prose equivalent of the component map is [Crate graph and dependency rules](crate-graph.md), so the architecture reads without the graphics.

## Components

![The PowerIO component map: powerio-core at the bottom; powerio-tx and powerio-dist as independent siblings above it; powerio-prob over both networks; powerio-matrix over the networks and prob; the powerio facade over the component crates with an optional matrix feature edge; powerio-cli, powerio-capi, and powerio-py over the facade; solvers consume instances from powerio-prob and the GridFM pipeline reads Parquet datasets from powerio-matrix.](assets/architecture.svg)

What the picture is drawn to show: the facade does not force a consumer to depend on every component crate (a distribution only consumer uses `powerio-dist` over `powerio-core` and nothing else), and the value families are not one universal format (the two network crates are siblings with no edge between them). Solvers sit outside: they consume instances, while their workspaces and numerical caches stay outside PowerIO.

## Data flow

![The PowerIO data flow: a Source of named immutable bytes enters parse, which produces a PioModule holding the typed value, retained source, diagnostics, and history; time series and scenario sets expose owner rooted typed entries through indexing and iteration; to_balanced derives a balanced module from a multiconductor module with reported assumptions and losses; calc operations return matrices and vectors with element mappings; emit produces grid exchange formats; serialize and deserialize connect the module to PowerIO IR version 1.](assets/dataflow.svg)

Every arrow out of the module is an explicit operation returning diagnostics; nothing transforms as a side effect of something else.
