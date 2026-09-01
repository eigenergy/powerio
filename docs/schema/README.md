# PowerIO JSON Schemas

`pio-module/1/schema.json` is the PowerIO 1.0 IR schema. Its document header is:

```json
{
  "schema": "powerio.module",
  "version": 1
}
```

PowerIO 1.0 reads and writes that schema only. Run:

```text
cargo run -p powerio --example generate_schemas --features schema -- docs/schema
```

The schema generation check in CI fails when the checked in file differs from
the Rust types.
