# powerio-pkg

A `.pio.json` document stores one PowerIO model payload with versioned metadata.
The payload belongs to one of two model families:

- `powerio::BalancedNetwork` — the scalar positive sequence transmission model;
- `powerio_dist::MulticonductorNetwork` — the wire coordinate distribution model.

A `NetworkPackage` owns one model payload and records:

- an explicit `model_kind`;
- `producer` and `origin` metadata;
- `sources` and `source_maps` (which canonical field came from which source
  record, by what `mapping_kind`);
- structured `diagnostics` with stable codes;
- a `validation` summary;
- `lowering_history`;
- optional `operating_points` for replayable states over the static model JSON;
- optional `study` commits for cumulative edits;
- optional `derived` metadata for matrix stats, normalized solver table
  identities, and cache keys.

It serializes to `.pio.json`.

Operating points are overlays: each point names table rows and fields to
update on the static model payload.
Model rows carry stable `uid` identities (source uids where the format has
them, synthesized `{table}:{row}` values otherwise); an update's `source_uid`
resolves against them and is authoritative, with the wire `row` as a fallback
and consistency check. GOC3 document construction extracts the time series into
this block while the balanced model JSON holds the first interval.

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

Balanced documents can record the dense normalized solver table layout without
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
