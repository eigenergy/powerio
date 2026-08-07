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

A JSON Schema `$id` is a stable identifier, and an archive that validates each
file against the URL that file declares keeps resolving those paths for as long
as the files exist — whether or not anyone upgrades the library. Retiring a
lineage from the *reader* (v0.8.0 accepts only its own `major.minor`) is a
separate act from unpublishing the document already-written files point at, and
only the first was intended.

The generator never removes files, so these stay put and the regenerate-and-diff
job stays clean. They are historical artifacts: leave them byte-for-byte as they
were at v0.7.3 rather than regenerating or reformatting them.
