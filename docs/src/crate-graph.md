# Crate graph and dependency rules

The workspace layers upward, with shared infrastructure below every parser and one thin facade on top. No lower crate depends on the facade, and the two network crates never depend on each other.

```text
powerio-core          Source, FormatId, Diagnostic, Error, PioModule<T>,
                      TimePoint, TimeSeries<T>, ScenarioSet<T>, Destination
├── powerio-tx        BalancedNetwork + the balanced format parsers and writers
└── powerio-dist      MulticonductorNetwork + OpenDSS, PMD, BMOPF decoding

powerio-prob          operating points, the seven instances, the eight
                      solutions, DOE GO Challenge 3 / OPFData / BMOPF assembly
                      → depends on core, tx, dist; matrix free
powerio-matrix        sparse matrices and graph data for both network families
                      → depends on core, tx, dist, prob
powerio (facade)      PioValue, parse, emit, serialize, deserialize, re-exports
                      → core, tx, dist, prob (+ matrix behind the matrix feature)
powerio-cli, powerio-capi, powerio-py → the facade
```

The rules a change must hold to:

- `powerio-core` owns no electrical type, parser, matrix, instance, or stored DTO;
- `powerio-tx` and `powerio-dist` stay independently usable: a distribution only consumer pulls neither the balanced model nor the matrix stack;
- `powerio-prob` never reaches `powerio-matrix` (a `cargo tree` gate enforces it);
- the facade re-exports the component APIs so `cargo add powerio` is the complete surface, and the matrix feature adds matrices without a dependency cycle.

CI asserts the edges from `cargo metadata`, so the drawn [architecture map](architecture-map.md) and the manifest cannot drift apart silently.
