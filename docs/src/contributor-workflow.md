# Testing and release checks

If your change alters numerical semantics, it needs tests and a short note in
code or docs saying why. If it is a performance change, it needs before and
after measurements.

## Baseline checks

`scripts/ci-mirror.sh` runs everything `rust.yml` runs, in the same feature combinations: format, clippy, the terminology and symbol gates, header parity, every crate's tests in each gated feature set, the C smoke and header programs, schema generation, packaging checks, and the docs build. Run it before pushing rather than assembling your own subset, because a hand assembled subset misses the feature gated suites. `cargo test --workspace`, for example, builds powerio-capi with default features only and skips every test behind `arrow`, `gridfm`, `matrix`, or `prob`.

Point `POWERIO_JL` at a PowerIO.jl checkout to include the Julia binding suite against the freshly built library, or set `POWERIO_JL_OPTIONAL=1` to run without one.

The Python binding tests need a wheel built into a virtual environment. Build
it from the repository root, where `pyproject.toml` lives:

```sh
python3 -m venv .venv && source .venv/bin/activate
pip install maturin pytest
maturin build --release -o dist
pip install dist/*.whl
python -m pytest python/tests
```

Install the built wheel rather than an editable one. When pytest runs from the
repository root, the `powerio/` crate directory there shadows a
`maturin develop` install.

## Route changes

Pick the smallest set of gates that covers what you changed, then run the
[release gates](#release-gates) before you claim a release.

| changed surface | extra gates |
| --- | --- |
| parser or writer semantics | `bash evals/validation/run_validation.sh`; format round trip tests; affected `cargo +nightly fuzz run <target> -- -runs=1` harnesses |
| rich model fields | `bash evals/validation/run_rich_validation.sh` |
| matrix calculations | `cargo test -p powerio-matrix`; `cargo bench -p powerio-matrix --bench matrix` |
| problem instances or DC OPF bundles | `cargo test -p powerio-prob --no-default-features`; `cargo test -p powerio --features matrix` |
| PowerWorld binary reader | PowerWorld parser tests plus `cargo bench -p powerio-tx --bench parse -- "parse_aux_|parse_pwb_"` |
| C ABI | `scripts/capi-header-parity.sh`; `scripts/capi-smoke.sh`; `cargo test -p powerio-capi --no-default-features`; `cargo test -p powerio-capi --features arrow,matrix,gridfm,dist,prob`; `bash scripts/ci-clippy.sh capi-no-default`; `bash scripts/ci-clippy.sh capi-release` |
| Python package metadata or extras | `maturin build --release --out /tmp/powerio-wheel-check`; inspect wheel `METADATA` |
| Julia binding compatibility | build `powerio-capi --features arrow,matrix,gridfm,dist,prob`, then run `PowerIO.jl` tests with `POWERIO_CAPI` |
| shared surface with PowerIO.jl | push a same-named PowerIO.jl companion branch; the tandem CI job tests against it |
| CLI behavior | `cargo test -p powerio-cli --test cli` |
| documentation or website | `mdbook build docs`; `mdbook test docs`; `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps`; regenerate schemas and the C header when their source rustdoc changes; run `scripts/capi-header-parity.sh`; check links to retired guide outputs |

`evals/validation/run_validation.sh` needs the Python oracle stack in the same
Python 3.11+ venv as the local wheel, and treats a missing PyPSA, pandapower,
or egret as a setup failure. `evals/validation/run_rich_validation.sh` treats
the committed PowerModels rich oracle as strict, so missing Julia is a setup
failure there too.

## Release gates

Before you publish a release claim, run the full set below on top of the
baseline checks:

```sh
cargo test -p powerio-capi --no-default-features
cargo test -p powerio-capi --features arrow,matrix,gridfm,dist,prob
bash scripts/ci-clippy.sh capi-no-default
bash scripts/ci-clippy.sh capi-release
cargo build -p powerio-capi --release --features arrow,matrix,gridfm,dist,prob
scripts/capi-header-parity.sh
scripts/capi-smoke.sh
POWERIO_CAPI=$PWD/target/release/libpowerio_capi.so \
  julia --project=../PowerIO.jl -e 'using Pkg; Pkg.test()'   # .dylib on macOS
cargo bench -p powerio-matrix --bench matrix -- 'matrix_bprime|matrix_ybus|dcopf_'
(cd evals/performance/asv && ../../../.venv/bin/asv check -E existing:../../../.venv/bin/python)
(cd evals/performance/asv && ../../../.venv/bin/asv run --quick --show-stderr -E existing:../../../.venv/bin/python --dry-run)
for target in $(cd fuzz && ls fuzz_targets | sed 's/\.rs$//'); do
  cargo +nightly fuzz run "$target" -- -runs=1
done
bash evals/validation/run_validation.sh
bash evals/validation/run_rich_validation.sh
```

`run_validation.sh` checks the classic transmission paths against
PowerModels.jl, ExaPowerIO.jl, egret, and pandapower to
writer matrix; `run_rich_validation.sh` covers fields outside the MATPOWER row
shape (branch terminal admittance, switches, current ratings, solution values,
HVDC costs, load voltage models). GOC3 has its own `goc3-reference` CI job,
which pins the GO-3 data model, C3DataUtilities, and the D1, D2, and D3 files
from GOC3Benchmark.jl. That job validates PowerIO's problem and solution
documents with the GO-3 data model, parses all three benchmark problems as
`AcScucInstance`, runs the Challenge 3 data checks on them, and runs the same
checks on a PowerIO `AcScucSolution` output document. Surge has no external
validator in this harness, so its current evidence is its Rust parser, writer,
routing, stored module, and round trip tests. The
[format chapter](format-fidelity.md) says what the independent checks prove
for each format.

The gates do not prove that every field of every source format survives.
Known losses are part of the public behavior and show up as warnings.

## Benchmark updates

Regenerate the benchmark JSON before you change a published table:

```sh
julia --project=evals/validation evals/performance/bench_julia.jl --json
.venv/bin/python evals/performance/bench_parse.py --json <cases>
cargo bench -p powerio-tx --bench parse -- "parse_aux_|parse_pwb_"
python3 evals/performance/extract_powerworld_bench.py
cargo bench -p powerio-matrix --bench matrix
python3 evals/performance/extract_matrix_bench.py
python3 evals/performance/render_tables.py
python3 evals/performance/render_tables.py --check
```

The ASV suite tracks Python wheel parse and matrix timing across git history.
To smoke test it on an uncommitted worktree, point it at the local venv:

```sh
cd evals/performance/asv
../../../.venv/bin/asv check -E existing:../../../.venv/bin/python
../../../.venv/bin/asv run --quick --show-stderr -E existing:../../../.venv/bin/python --dry-run
```

Do not edit generated benchmark tables by hand. When you publish new numbers,
update the snapshot environment described on the
[performance page](performance.md)
as well: commit, tree cleanliness, machine, OS, toolchain, Python stack, Julia
stack, commands, fixtures, and optional local data.

Broad local corpora stay local. Pass them in through the documented
environment variables or `--root` flags, review the reports the run writes
under `evals/validation/results/`, and do not commit corpus paths or generated
outputs.
