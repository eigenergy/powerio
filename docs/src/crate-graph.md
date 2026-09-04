# Crate graph and architecture map

The workspace layers upward: shared infrastructure below every parser, one
thin facade on top. No lower crate depends on the facade, and the two network
crates never depend on each other.

```text
powerio-core          Source, FormatId, Diagnostic, Error, PioModule<T>,
                      TimePoint, TimeSeries<T>, ScenarioSet<T>, Destination
├── powerio-tx        BalancedNetwork and the balanced readers and writers
└── powerio-dist      MulticonductorNetwork and the OpenDSS, PMD, and BMOPF converters

powerio-prob          operating points, updates, the seven instances, the eight
                      solutions, and GO Challenge 3, OPFData, and BMOPF assembly;
                      depends on core, tx, and dist; matrix free
powerio-matrix        sparse matrices and graph data for both network families;
                      depends on core, tx, dist, and prob
powerio               PioValue, parse, emit, serialize, deserialize, and the
                      re-exports; depends on core, tx, dist, and prob, and on
                      matrix behind the `matrix` feature
powerio-cli, powerio-capi, powerio-py    over the facade
```

The rules a change holds to:

- `powerio-core` owns no electrical type, parser, matrix, instance, or IR
  record layout;
- `powerio-tx` and `powerio-dist` stay independently usable: a distribution
  consumer pulls neither the balanced model nor the matrix stack;
- `powerio-prob` never reaches `powerio-matrix`;
- the facade re-exports the component crates, so `cargo add powerio` is the
  complete surface, and the `matrix` feature adds matrices without a cycle.

CI asserts these edges from `cargo metadata`, so the drawn map below and the
manifests cannot drift apart.

## Components

![The PowerIO component map: powerio-core at the bottom; powerio-tx and powerio-dist as independent siblings above it; powerio-prob over both networks; powerio-matrix over the networks and prob; the powerio facade over the component crates with an optional matrix feature edge; powerio-cli, powerio-capi, and powerio-py over the facade; solvers consume instances from powerio-prob and the GridFM pipeline reads Parquet datasets from powerio-matrix.](assets/architecture.svg)

The facade does not force a consumer to depend on every component crate, and
the two network crates are siblings with no edge between them. Solvers sit
outside: they consume instances, and their workspaces and numerical caches
stay outside PowerIO.

## Data flow

![The PowerIO data flow: a Source of named immutable bytes enters parse, which produces a PioModule holding the typed value, retained source, diagnostics, and history; time series and scenario sets expose owner rooted typed entries through indexing and iteration; to_balanced derives a balanced module from a multiconductor module with reported assumptions and losses; calc operations return matrices and vectors with element mappings; emit produces grid exchange formats; serialize and deserialize connect the module to PowerIO IR generation 2.](assets/dataflow.svg)

Every arrow out of the module is an explicit operation that returns
diagnostics. Nothing transforms as a side effect of something else.

The diagram sources are `docs/diagrams/*.dot`.
`scripts/check-architecture-map.py` checks the drawn crate edges against
`cargo metadata` and, with `--render`, regenerates the images.
