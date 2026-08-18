# Published JSON Schema documents

Each directory is a schema identifier path, named for the powerio lineage that
serves it: `0.9` while the major is 0, `1` afterwards. The current one is the
only document
`cargo run -p powerio-pkg --example generate_schemas --features schema -- docs/schema`
emits; `rust.yml` regenerates it on every pull request and fails on a diff. The
path moves when and only when a document stops loading, which is the same rule
the reader applies to `powerio_version`.

## Retired lineages are still served

`pio-package/0.1/`, `pio-package/0.2/`, `pio-payload-balanced/1/`, and
`pio-payload-multiconductor/1/` are frozen copies of what earlier releases
published. **Do not delete them.**

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

## Recognizing a document

Two rules classify a bare JSON document. A package is a top level `model_kind` of `"balanced"` or `"multiconductor"` beside a `model` key; model JSON is `buses` beside another network key (the case formats spell it `bus`). `powerio::classify_json_text` is the reference implementation, and `powerio::JSON_CLASSES` carries the permanent family spellings; a consumer classifying a dropped file in TypeScript, Python, or Julia restates this rule.
