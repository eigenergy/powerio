# Testing and release checks

Keep changes reviewable. A numerical semantics change needs tests and a short
reason in code or docs. A performance change needs before and after
measurements. A documentation change should link to evidence instead of
expanding the README into a second manual.

## Baseline checks

These commands cover the Rust workspace, the Python extension build, the Python
binding tests, and the book:

```sh
cargo fmt --all --check
bash scripts/ci-clippy.sh
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

## Route changes

Use the smallest gate set that covers the changed surface, then run the
[release gates](#release-gates) before a release claim.

| changed surface | extra gates |
| --- | --- |
| parser or writer semantics | `bash benchmarks/run_validation.sh`; format round trip tests; affected `cargo +nightly fuzz run <target> -- -runs=1` harnesses |
| rich model fields | `bash benchmarks/run_rich_validation.sh` |
| matrix builders | `cargo test -p powerio-matrix`; `cargo bench -p powerio-matrix --bench matrix` |
| problem instances or DC OPF bundles | `cargo test -p powerio-prob --no-default-features`; `cargo test -p powerio-prob --features matrix` |
| PowerWorld binary reader | PowerWorld parser tests plus `cargo bench -p powerio --bench parse -- "parse_aux_|parse_pwb_"` |
| C ABI | `scripts/capi-header-parity.sh`; `scripts/capi-smoke.sh`; `cargo test -p powerio-capi --no-default-features`; `cargo test -p powerio-capi --features arrow,matrix,gridfm,dist,pkg,prob`; `bash scripts/ci-clippy.sh capi-no-default`; `bash scripts/ci-clippy.sh capi-release` |
| Python package metadata or extras | `maturin build --release --out /tmp/powerio-wheel-check`; inspect wheel `METADATA` |
| Julia binding compatibility | build `powerio-capi --features arrow,matrix,gridfm,dist,pkg,prob`, then run `PowerIO.jl` tests with `POWERIO_CAPI` |
| shared surface with PowerIO.jl | push a same-named PowerIO.jl companion branch; the tandem CI job tests against it |
| CLI behavior | `cargo test -p powerio-cli --test cli` |
| documentation or website | `mdbook build docs`; `mdbook test docs`; `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps`; regenerate schemas and the C header when their source rustdoc changes; run `scripts/capi-header-parity.sh`; check links to retired guide outputs |

`benchmarks/run_validation.sh` requires the Python oracle stack in the same
Python 3.11+ venv as the local wheel. Missing PyPSA, pandapower, or egret is a
setup failure. `benchmarks/run_rich_validation.sh` treats the committed
PowerModels rich oracle as strict; missing Julia is a setup failure.

## Release gates

Run the full set below, in addition to the baseline checks, before publishing
a release claim:

```sh
cargo test -p powerio-capi --no-default-features
cargo test -p powerio-capi --features arrow,matrix,gridfm,dist,pkg,prob
bash scripts/ci-clippy.sh capi-no-default
bash scripts/ci-clippy.sh capi-release
cargo build -p powerio-capi --release --features arrow,matrix,gridfm,dist,pkg,prob
scripts/capi-header-parity.sh
scripts/capi-smoke.sh
POWERIO_CAPI=$PWD/target/release/libpowerio_capi.dylib \
  julia --project=../PowerIO.jl -e 'using Pkg; Pkg.test()'
cargo bench -p powerio-matrix --bench matrix -- 'matrix_bprime|matrix_ybus|dcopf_'
(cd benchmarks/asv && ../../.venv/bin/asv check -E existing:../../.venv/bin/python)
(cd benchmarks/asv && ../../.venv/bin/asv run --quick --show-stderr -E existing:../../.venv/bin/python --dry-run)
for target in matpower psse pslf powerio_json powerworld_aux pwb pwd; do
  cargo +nightly fuzz run "$target" -- -runs=1
done
bash benchmarks/run_validation.sh
bash benchmarks/run_rich_validation.sh
```

`run_validation.sh` checks the classic transmission paths against
PowerModels.jl, ExaPowerIO.jl, egret, pandapower, and the full legacy reader to
writer matrix; `run_rich_validation.sh` covers fields outside the MATPOWER row
shape (branch terminal admittance, switches, current ratings, solution values,
HVDC costs, load voltage models). GOC3 and Surge have no external oracle in
this harness; the Rust parser, writer, routing, package, and round trip tests
cover them. What the oracle legs prove, per format, is in the
[format fidelity chapter](https://eigenergy.github.io/powerio/guide/format-fidelity.html).

The gates do not prove every source format field is lossless. Known losses are
part of the public behavior and surface as warnings.

## Benchmark updates

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
environment described in the
[performance guide](https://eigenergy.github.io/powerio/guide/performance.html)
when publishing new numbers: commit, tree cleanliness, machine, OS, toolchain,
Python stack, Julia stack, commands, fixtures, and optional local data.

Broad local corpora stay local. Pass them through documented environment
variables or `--root` flags, review the reports under `benchmarks/results/`, and
do not commit corpus paths or generated outputs.
