# Published JSON Schema documents

Each directory is a schema identifier path. `pio-package/0.2/schema.json` is the
current `.pio.json` document schema and the only one
`cargo run -p powerio-pkg --example generate_schemas --features schema -- docs/schema`
emits; `rust.yml` regenerates it on every pull request and fails on a diff.

## Retired lineages are still served

`pio-package/0.1/`, `pio-payload-balanced/1/`, and `pio-payload-multiconductor/1/`
are frozen copies of the v0.7.x documents. **Do not delete them.**

A `.pio.json` written before v0.8.0 carries these identifiers as literal field
values:

```json
"schema": "https://powerio.dev/schema/pio-package/0.1",
"payload_schema": "https://powerio.dev/schema/pio-payload-multiconductor/1",
```

A JSON Schema `$id` is a stable identifier. A tool that validates a file
against the URL the file declares depends on that URL staying served. The
reader can retire a lineage; the published document stays.

The generator never removes files. Keep these documents byte for byte; do
not regenerate or reformat them. The `frozen_schemas` test in powerio-pkg
pins them.
