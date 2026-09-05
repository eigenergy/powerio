# Fuzzing the parser surface

These are libFuzzer harnesses for the readers that take untrusted input. The
invariant under test is the parser trust model: any input returns `Ok` or a
structured `Err`, with no panic and no undefined behavior.

| Target | Input |
|---|---|
| `matpower`, `psse`, `pslf`, `powerworld_aux` | memory sources for the corresponding readers |
| `pwb`, `pwd` | raw bytes for the PowerWorld binary decoders |
| `json_classify` | arbitrary bytes for the `.json` classifier and its C entry point; every answer must be a documented family, since a file picker dispatches on it |
| `stored_module` | the PowerIO IR reader; a document that deserializes must serialize and deserialize again |
| `dss`, `pmd_json` | the distribution readers, which also write the parsed network back and project its graph, so a count cap that a consumer sizes an allocation from is exercised on both sides |
| `dist_classify` | the distribution `.json` classifier, which runs before any reader cap applies, and the reader and writers it names |
| `dss_includes`, `dss_includes_fs` | OpenDSS include resolution over in-memory buffers and over a real filesystem tree with an escaping symbolic link |

The JSON formats that ride on serde_json (PowerModels, egret, pandapower)
have no hand written tokenizer, so between them the harnesses cover every
hand rolled reader.

You need nightly and [cargo-fuzz](https://github.com/rust-fuzz/cargo-fuzz):

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
a reader, minutes per target weekly. Long campaigns stay manual, so run one
yourself when you touch a reader. A crash reproducer lands in
`artifacts/<target>/`; turn it into a regression test next to the reader
before you fix the bug.
