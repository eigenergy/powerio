# Fuzzing the parser surface

libFuzzer harnesses for the readers that take untrusted input: `matpower`,
`psse`, `pslf`, and `powerworld_aux` feed memory sources to the corresponding
readers; `pwb` and `pwd` feed the PowerWorld binary decoders raw bytes; `dss`
feeds the distribution
family's tokenizer. The remaining JSON formats
(PowerModels, egret, pandapower) ride serde_json rather than a hand-written
tokenizer, so the harnesses cover every hand-rolled reader. The invariant
under test is the parser trust model: any input returns `Ok` or a structured
`Err`, never a panic and never undefined behavior.

`json_classify` drives the `.json` classifier and its C entry point over
arbitrary bytes, asserting that every answer is one of the documented families.
A file picker dispatches on that answer, so an undocumented token is a defect
even when nothing panics.

`dss` and `pmd_json` also write the parsed network back and project its graph.
A reader that accepts an unbounded count is only half the hazard; the other
half is a consumer that sizes an allocation from it, which is where a small
input turns into gigabytes. Fuzzing the pair catches a cap that does not hold.

Needs nightly and [cargo-fuzz](https://github.com/rust-fuzz/cargo-fuzz):

```sh
cargo install cargo-fuzz
cargo +nightly fuzz run matpower -- -max_total_time=60
```

Seed a corpus from the test fixtures for much better coverage:

```sh
mkdir -p corpus/matpower && cp ../tests/data/*.m corpus/matpower/
mkdir -p corpus/powerworld_aux && cp ../tests/data/powerworld/*.aux corpus/powerworld_aux/ 2>/dev/null || true
mkdir -p corpus/pwb && cp ../tests/data/powerworld/*.pwb corpus/pwb/ 2>/dev/null || true
```

The crate is excluded from the main workspace. CI runs a short smoke pass
(`.github/workflows/fuzz.yml`): seconds per target on pull requests that touch
a reader, minutes per target weekly. Long campaigns stay manual; run one when
touching a reader. A crash reproducer lands in `artifacts/<target>/`; turn it
into a regression test next to the reader before fixing.
