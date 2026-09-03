# PowerIO 1.0 architecture record

Status: historical design record for the work that led to PowerIO 0.10. It is
not current API authority. Use the released API, current source, tests,
migration guide, and changelog for present behavior.

Read the documents in this order:

1. [V1_TERMINOLOGY.md](V1_TERMINOLOGY.md) fixes public words and names.
2. [V1_RATIONALE.md](V1_RATIONALE.md) explains the alternatives and why the
   selected API won. It is explanatory rather than a second specification.
3. [V1_ONTOLOGY.md](V1_ONTOLOGY.md) fixes public value kinds, source profiles,
   and allowed transformations.
4. [V1_ARCHITECTURE.md](V1_ARCHITECTURE.md) fixes ownership, schema, instance,
   solution, matrix, writer, crate, ABI, and binding semantics.
5. [V1_ISSUE_AUDIT.md](V1_ISSUE_AUDIT.md) maps the design to the issues and PRs
   open when the record was written.
6. [V1_IMPLEMENTATION.md](V1_IMPLEMENTATION.md) records the historical
   implementation order and validation gates.

The disposable [prototype](prototype/) compiles the public Rust choices that
could not be settled from source definitions alone:

- `PioModule<T>` has no marker bound and can hold application defined typed
  values;
- `PioValue` is the finite built in dynamic boundary, with a flat
  nonexhaustive enum and stable `PioValueKind` strings;
- `powerio::try_into_typed::<T>` moves value and module records, and failed
  narrowing returns the original module;
- opaque immutable network snapshots, sibling instances, and solutions share
  owners without a public `BalancedNetworkData` type;
- ordinary generic time series and scenario containers work without public
  traits for memory representation, and balanced and multiconductor operating point series
  share numerical columns;
- the stored module uses one schema version and typed value DTOs;
- named memory and path destinations return complete owned artifact
  inventories.

Run it with:

```bash
cargo test --manifest-path arch-v1/prototype/Cargo.toml
cargo test --manifest-path arch-v1/prototype/crate-layout/Cargo.toml --all-targets
```

## Audit conclusions

`PioModule<T>` is the compiler unit. It contains `T` directly plus common
records and optional runtime source ownership. `PioModule<PioValue>` is used
only when parsing, stored JSON, or a language binding must discover the concrete
type at run time. A typed module can contain an application value absent from
`PioValue`; it cannot cross a built in dynamic boundary until PowerIO adds and
tests a `PioValue` variant, `PioValueKind`, stored DTO, and binding wrapper.

`powerio-pkg` and `powerio-diag` retire at 1.0. The current balanced
implementation crate becomes `powerio-tx`, and the short `powerio` name becomes
the complete entry facade. One `powerio-core` crate holds `Source`, diagnostics,
errors, `PioModule<T>`, common records, generic repeated value containers, and
output destination types. Lower parsers can return typed modules without
depending back on the facade. `PioValue`, universal parser dispatch, and
`.pio.json` stay in the facade because they depend on transmission,
distribution, and problem types.

The required source profiles determine the initial dynamic values. The exact
value kind tags and `.pio.json` top level object are in the architecture
document.
The 1.0 reader upgrades released 0.9.x package shapes that contain a static
value or operating point series. A nonempty legacy `study` must first be
materialized to an explicitly selected revision with the 0.9 migration command;
the 1.0 reader refuses to guess. The already unsupported pre 0.9 lineage stays
rejected.

The PyPSA CSV electrical profile maps one snapshot to `BalancedNetwork`,
supported snapshot-local input series to `TimeSeries<BalancedNetwork>`, and a
complete state-only series to
`TimeSeries<OperatingPoint<BalancedNetwork>>`. Complete PyPSA and NetCDF
support waits for source neutral multi-carrier, multi-period, capacity
expansion, stochastic calculation, and result types. OpenDSS circuit data,
schedules and calculation instructions, and solved QSTS samples remain
distinct; complete sampled states can use
`TimeSeries<OperatingPoint<MulticonductorNetwork>>`.

The issue audit records the then open PRs and their planned disposition. That
snapshot is preserved as history rather than current integration guidance.

The prototype and implementation record explain how the 0.10 work was checked.
They do not prescribe current changes.
