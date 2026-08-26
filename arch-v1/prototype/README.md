# PowerIO 1.0 API prototype

This crate is disposable design evidence. It is not an implementation crate and
is not part of the repository workspace.

It compiles the ownership claims that the 1.0 API depends on:

- unconstrained `PioModule<T>` can hold an application value without pretending
  that the value is registered for automatic parsing or stored JSON;
- `PioModule<PioValue>` narrows by move into `PioModule<T>` without cloning the
  value, diagnostics, or retained source, and a mismatch returns the original
  module;
- the flat `PioValue` enum contains only concrete combinations supported at a
  dynamic boundary, while `PioValueKind` supplies the stable stored tag;
- the stored module uses one integer schema version and a real tagged value DTO,
  with header dispatch before exact current version field checking;
- generic `TimeSeries<T>` and `ScenarioSet<T>` need no public traits for memory
  representation;
  operating point handles share private columns and remain valid after the
  parent series is dropped;
- both balanced and multiconductor operating point series compile, and scenario
  lookup uses a stable `ScenarioId` rather than a semantic row number;
- sibling calculation instances share one network owner and solutions share one
  immutable instance;
- an owned `Destination` covers exact file, directory, and named memory output
  without placing a sink lifetime on `write`; `WriteResult` returns either a
  complete path inventory or owned `Vec<u8>` artifacts;
- public shape errors return `Result` rather than panicking.

The prototype deliberately does not settle private `Arc` granularity, every
electrical field in the stored value DTOs, or the C ABI. Production gates in
the implementation document cover those decisions.

The prototype uses the private name `OperatingPointData` only because its one
generic handle currently needs an umbrella enum. Production should prefer the
concrete private balanced and multiconductor types if the complete fields make
that enum unnecessary. This name is not part of the public API and can change
without a 1.0 compatibility concern.

The nested `crate-layout/` workspace checks constraints that a one crate model
cannot expose: dependency direction, Rust's orphan rules, facade owned dynamic
dispatch, and placement of operating point builders above both network crates.

Run:

```bash
cargo test --manifest-path arch-v1/prototype/Cargo.toml
cargo test --manifest-path arch-v1/prototype/crate-layout/Cargo.toml --all-targets
```
