# powerio-dist

`powerio-dist` parses multiconductor distribution network cases into a typed
model in wire coordinates and converts between OpenDSS `.dss`,
PowerModelsDistribution ENGINEERING JSON, and the draft BMOPF schema from the
IEEE PES Task Force on Benchmarking Multiconductor OPF
(<https://github.com/frederikgeth/bmopf-report>).

Writing back to the source format reproduces the file byte for byte; every
cross format conversion reports each field the target cannot represent in its
warnings. The dss reader materializes every OpenDSS class default into an
explicit model value (verified against the OpenDSS source and empirically
against `opendssdirect`) and records which fields were defaulted, so BMOPF
output is always fully explicit. The per fixture conversion matrix is
generated into [docs/conversion-matrix.md](docs/conversion-matrix.md).

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

The workspace README covers the CLI, Python package, C ABI, and the
transmission crates: <https://github.com/eigenergy/powerio>.
