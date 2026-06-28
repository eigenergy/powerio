# Contributor Workflow

Keep changes reviewable. A numerical semantics change needs tests and a short
reason in code or docs. A performance change needs before and after measurements.
A documentation change should link to evidence instead of expanding the README
into a second manual.

## Baseline Checks

These commands cover the Rust workspace, the Python extension build, the Python
binding tests, and the book:

```sh
cargo fmt --all --check
cargo clippy --all-targets
cargo test
cargo test -p powerio-cli --test cli
cargo test -p powerio-capi
cargo build -p powerio-py
python3.12 -m venv .venv
.venv/bin/python -m pip install --upgrade pip maturin -r benchmarks/requirements.txt
env VIRTUAL_ENV=$PWD/.venv .venv/bin/maturin develop --release
.venv/bin/pytest python/tests
mdbook build docs
mdbook test docs
```

## Route Changes

Use the smallest gate set that covers the changed surface, then run the full
[Reliability Evidence](reliability.md) gates before a release claim.

| changed surface | extra gates |
| --- | --- |
| parser or writer semantics | `bash benchmarks/run_validation.sh`; format round trip tests; affected `cargo +nightly fuzz run <target> -- -runs=1` harnesses |
| rich model fields | `bash benchmarks/run_rich_validation.sh` |
| matrix builders or DC OPF bundles | `cargo test -p powerio-matrix`; `cargo bench -p powerio-matrix --bench matrix` |
| PowerWorld binary reader | PowerWorld parser tests plus `cargo bench -p powerio --bench parse -- "parse_aux_|parse_pwb_"` |
| C ABI | `scripts/capi-header-parity.sh`; `scripts/capi-smoke.sh`; `cargo test -p powerio-capi --no-default-features`; `cargo test -p powerio-capi --features arrow,gridfm,dist`; matching clippy runs |
| Python package metadata or extras | `maturin build --release --out /tmp/powerio-wheel-check`; inspect wheel `METADATA` |
| Julia binding compatibility | build `powerio-capi --features arrow,gridfm,dist`, then run `PowerIO.jl` tests with `POWERIO_CAPI` |
| CLI behavior | `cargo test -p powerio-cli --test cli` |
| documentation or website | `mdbook build docs`; `mdbook test docs`; check stale links to retired guide outputs |

`benchmarks/run_validation.sh` requires the Python oracle stack in the same
Python 3.11+ venv as the local wheel. Missing PyPSA, pandapower, or egret is a
setup failure. `benchmarks/run_rich_validation.sh` treats the committed
PowerModels rich oracle as strict; missing Julia is a setup failure.

## Benchmark Updates

Regenerate benchmark JSON before changing published tables:

```sh
julia --project=benchmarks benchmarks/bench_julia.jl --json
.venv/bin/python benchmarks/bench_parse.py --json <cases>
cargo bench -p powerio --bench parse -- "parse_aux_|parse_pwb_"
python3 benchmarks/extract_powerworld_bench.py
cargo bench -p powerio-matrix --bench matrix
python3 benchmarks/extract_matrix_bench.py
python3 benchmarks/render_tables.py
python3 benchmarks/render_tables.py --check
```

The ASV suite tracks Python wheel parse and matrix timing across git history.
For an uncommitted worktree, smoke test it against the local venv:

```sh
cd benchmarks/asv
../../.venv/bin/asv check -E existing:../../.venv/bin/python
../../.venv/bin/asv run --quick --show-stderr -E existing:../../.venv/bin/python --dry-run
```

Do not update generated benchmark tables by hand. Update the snapshot
environment in [benchmarks/RESULTS.md](https://github.com/eigenergy/powerio/blob/main/benchmarks/RESULTS.md)
when publishing new numbers: commit, tree cleanliness, machine, OS, toolchain,
Python stack, Julia stack, commands, fixtures, and optional local data.

Broad local corpora stay local. Pass them through documented environment
variables or `--root` flags, review the reports under `benchmarks/results/`, and
do not commit corpus paths or generated outputs.
