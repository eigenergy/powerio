# Fuzzing the parser surface

libFuzzer harnesses for the readers that take untrusted input: `matpower`,
`psse`, and `powerio_json` feed `parse_str`; `pwb` and `pwd` feed the
PowerWorld binary decoders raw bytes. The invariant under test is the parser
trust model: any input returns `Ok` or a structured `Err`, never a panic and
never undefined behavior.

Needs nightly and [cargo-fuzz](https://github.com/rust-fuzz/cargo-fuzz):

```sh
cargo install cargo-fuzz
cargo +nightly fuzz run matpower -- -max_total_time=60
```

Seed a corpus from the test fixtures for much better coverage:

```sh
mkdir -p corpus/matpower && cp ../tests/data/*.m corpus/matpower/
mkdir -p corpus/pwb && cp ../tests/data/powerworld/*.pwb corpus/pwb/ 2>/dev/null || true
```

The crate is excluded from the workspace and from CI; run it when touching a
reader. A crash reproducer lands in `artifacts/<target>/` — turn it into a
regression test next to the reader before fixing.
