# powerio-pkg

The `.pio.json` compiler package: a versioned envelope around one PowerIO IR
payload.

PowerIO has no single flattened "universal network" struct. It keeps two
concrete static-grid IR families distinct:

- `powerio::BalancedNetwork` — the scalar positive-sequence transmission model
  (historically `powerio::Network`);
- `powerio_dist::MulticonductorNetwork` — the wire-coordinate distribution model
  (historically `powerio_dist::DistNetwork`).

A `NetworkPackage` wraps exactly one of those payloads at a time and carries the
metadata a compiler artifact needs to be trustworthy:

- an explicit `model_kind` (never inferred from which field is present);
- `producer` and `origin` metadata;
- `sources` and `source_maps` (which canonical field came from which source
  record, by what `mapping_kind`);
- structured `diagnostics` with stable codes;
- a `validation` summary;
- `lowering_history`;
- optional `operating_points` for replayable states over the static payload;
- optional `derived` metadata for matrix stats, normalized solver table
  identities, and cache keys.

It serializes to `.pio.json`. Binary `.pio` is out of scope until the JSON
package stabilizes.

Operating points are overlays, not separate payloads. Each point names table
rows and fields to update on the one static payload. GOC3 package construction
extracts the time series into this block while the balanced payload holds the
first interval.

See `docs/src/compiler-ir.md` and `docs/src/pio-json-schema.md` in the
repository root.

```rust
use powerio_pkg::{NetworkPackage, ModelKind};

let net = powerio::BalancedNetwork::in_memory("demo", 100.0, vec![], vec![]);
let pkg = NetworkPackage::from_balanced(net);
assert_eq!(pkg.model_kind(), ModelKind::Balanced);
assert!(pkg.kind_is_consistent());

let json = pkg.to_json_pretty().unwrap();
let back = NetworkPackage::from_json(&json).unwrap();
assert_eq!(back.model_kind(), ModelKind::Balanced);
```

Balanced packages can record the dense normalized solver table contract without
embedding every table row:

```rust
let net = powerio::parse_str("...", "matpower").unwrap().network;
let mut pkg = NetworkPackage::from_balanced(net);
pkg.attach_normalized_solver_table_metadata().unwrap();
```

Operating points can be inspected or materialized:

```rust
let text = std::fs::read_to_string("goc3_case.json").unwrap();
let parsed = powerio::parse_str(&text, "goc3-json").unwrap();
let pkg = NetworkPackage::from_balanced(parsed.network);
if let Some(series) = pkg.operating_points() {
    println!("{} periods", series.time_axis.periods);
}
let static_pkg = pkg.materialize_operating_point(0).unwrap();
assert!(static_pkg.operating_points().is_none());
```
