# powerio-pkg

The `.pio.json` compiler package: a versioned envelope around one PowerIO IR
payload.

PowerIO has no single flattened "universal network" struct. It keeps two
concrete static-grid IR families distinct:

- `powerio::BalancedNetwork` — the scalar positive-sequence transmission model
  (historically `powerio::Network`);
- `powerio_dist::MulticonductorNetwork` — the wire-coordinate distribution model
  (historically `powerio_dist::DistNetwork`).

A `CompilerPackage` wraps exactly one of those payloads at a time and carries the
metadata a compiler artifact needs to be trustworthy:

- an explicit `model_kind` (never inferred from which field is present);
- `producer` and `origin` metadata;
- `sources` and `source_maps` (which canonical field came from which source
  record, by what `mapping_kind`);
- structured `diagnostics` with stable codes;
- a `validation` summary;
- `lowering_history`.

It serializes to `.pio.json`. Binary `.pio` is out of scope until the JSON
package stabilizes.

See `docs/architecture/compiler-ir.md` and
`docs/architecture/pio-json-schema.md` in the repository root.

```rust
use powerio_pkg::{CompilerPackage, ModelKind};

let net = powerio::BalancedNetwork::in_memory("demo", 100.0, vec![], vec![]);
let pkg = CompilerPackage::from_balanced(net);
assert_eq!(pkg.model_kind(), ModelKind::Balanced);
assert!(pkg.kind_is_consistent());

let json = pkg.to_json_pretty().unwrap();
let back = CompilerPackage::from_json(&json).unwrap();
assert_eq!(back.model_kind(), ModelKind::Balanced);
```
