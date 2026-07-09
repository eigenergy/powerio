# powerio-dist

`powerio-dist` parses multiconductor distribution networks into a typed model
in wire coordinates. It converts between OpenDSS `.dss`,
PowerModelsDistribution ENGINEERING JSON, and the draft BMOPF schema from the
IEEE PES Task Force on Benchmarking Multiconductor OPF
(<https://github.com/frederikgeth/bmopf-report>).

Writing back to the source format reproduces the retained bytes. Cross format
conversion reports fields the target cannot represent. The DSS reader expands
OpenDSS class defaults into explicit model values and records which values came
from defaults. BMOPF output therefore contains explicit values for those
fields. The generated conversion matrix records behavior for each fixture in
`powerio-dist/docs/conversion-matrix.md`.

```rust
let net = powerio_dist::parse_file("feeder.dss", None)?;
let pmd = net.to_format(powerio_dist::DistTargetFormat::PmdJson);
for w in &pmd.warnings {
    eprintln!("fidelity: {w}");
}
```

The same surface is available from the `powerio` CLI
(`powerio convert feeder.dss --to pmd-json`), the Python package
(`powerio.dist`), and the C ABI (`pio_dist_*`, behind the `dist` cargo
feature of `powerio-capi`).

Fixtures live in `tests/data/dist/` at the workspace root with provenance
recorded in its README. The oracle harnesses under `tools/` re-solve emitted
`.dss` in OpenDSS and validate emitted PMD JSON against
PowerModelsDistribution; CI runs the schema validation and round trip suites.
For a local OpenDSS corpus, set `POWERIO_DIST_LOCAL_DSS_CORPUS` to a directory
tree and run `cargo test -p powerio-dist --test local_dss_corpus -- --nocapture`;
the test parses every `.dss`, writes BMOPF JSON, validates it against the
vendored schema, reparses it, writes DSS, reparses that DSS, and checks that
the second BMOPF JSON remains schema valid and stable up to JSON numeric
rounding.

The workspace README covers the CLI, Python package, C ABI, and the
transmission crates: <https://github.com/eigenergy/powerio>.
