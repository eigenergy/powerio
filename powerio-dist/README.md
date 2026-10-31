# powerio-dist

`powerio-dist` parses multiconductor distribution networks into a typed model
in wire coordinates. It converts between OpenDSS `.dss`,
PowerModelsDistribution ENGINEERING JSON, BMOPF JSON, the schema of the
IEEE PES Task Force on Benchmarking Multiconductor OPF published at
<https://github.com/distribution-system-opt/dsopt-schema>, and distribution
CIM. It reads BMOPF schema 0.1.0 and 0.2.0 and writes 0.2.0. Distribution CIM
(`cim-dist`) is CIM100 RDF/XML, the iec61970cim17v40 and iec61968cim13v13
release that GridAPPS-D and CIMHub exchange, read from one document or a
directory and written as one document. The reader also reads the CIM16 LV
networks of ADMS exports with the Schneider Electric extension.

SINCAL profiles are provided by `powerio-tnd` and the `powerio` facade.
This crate owns the multiconductor model and its distribution formats.

```rust
let source = powerio_core::Source::open("feeder.dss")?;
let module = powerio_dist::parse(source)?;
let emitted = powerio_dist::emit(
    &module,
    powerio_dist::DistTargetFormat::PmdJson,
    powerio_core::Destination::memory("feeder.pmd.json")?,
)?;
for line in powerio_dist::diagnostics::render_diagnostics(emitted.diagnostics()) {
    eprintln!("fidelity: {line}");
}
```

The same parse and emit are available from the `powerio` CLI
(`powerio convert feeder.dss --to pmd-json`), the Python package
(`powerio.parse` and `powerio.emit`), and the C ABI (`pio_source_open`,
`pio_parse`, typed value access, and `pio_emit`).

Fixtures live in `tests/data/dist/` at the workspace root, with their
provenance recorded in the README there. The oracle harnesses under `tools/`
solve emitted `.dss` again in OpenDSS and validate emitted PMD JSON against
PowerModelsDistribution; CI runs the schema validation and round trip suites.
If you have a local OpenDSS corpus, set `POWERIO_DIST_LOCAL_DSS_CORPUS` to the
directory tree and run
`cargo test -p powerio-dist --test local_dss_corpus -- --nocapture`. That
test parses every `.dss`, emits BMOPF JSON, validates it against the vendored
schema, reparses it, emits DSS, reparses that DSS, and checks that the second
BMOPF JSON is still schema valid and stable up to JSON numeric rounding.

The workspace README covers the CLI, Python package, C ABI, and the
transmission crates: <https://github.com/eigenergy/powerio>.
