# Performance

PowerIO has five benchmark tiers. Keep them separate when publishing numbers.

| tier | command | what it answers |
| --- | --- | --- |
| Rust microbenchmarks | `cargo bench -p powerio --bench parse` | parser, writer, and PowerWorld reader timing inside one process |
| Matrix microbenchmarks | `cargo bench -p powerio-matrix --bench matrix` | sparse matrix, DC OPF component, and dense sensitivity builder timing after parse/indexing |
| Cross tool parser and matrix comparison | `julia --project=benchmarks benchmarks/bench_julia.jl --json` | powerio through the C ABI against ExaPowerIO.jl and PowerModels.jl, including parse plus Y bus construction |
| Python parser comparison | `.venv/bin/python benchmarks/bench_parse.py --json <cases>` | Python package parse and matrix path against pandapower reader paths |
| C ABI release size | three `cargo build -p powerio-capi --release` feature sets plus `stat` | binary size for core, `arrow,matrix`, and all release features |

The published table lives in the repository benchmark results, and this guide is
the public reference for how those numbers are produced. Each refresh should
update the snapshot environment there: machine model, chip,
core count, memory, OS, Rust, C compiler, Julia, Python, and the package
versions used by the comparison harnesses. Regenerate the JSON inputs first,
then splice only the marked regions:

```sh
bash benchmarks/fetch_cases.sh
cargo build --release -p powerio-capi --features arrow,matrix
python3.12 -m venv .venv
.venv/bin/python -m pip install --upgrade pip maturin -r benchmarks/requirements.txt
env VIRTUAL_ENV=$PWD/.venv .venv/bin/maturin develop --release
julia --project=benchmarks benchmarks/bench_julia.jl --json
.venv/bin/python benchmarks/bench_parse.py --json \
  tests/data/case2869pegase.m \
  tests/data/large/case9241pegase.m \
  tests/data/large/case13659pegase.m \
  tests/data/large/case193k.m
python3 benchmarks/render_tables.py
python3 benchmarks/render_tables.py --check
```

The Julia benchmark writes `rows` for parse only and `matrix_rows` for parse
plus Y bus construction. PowerIO measures `pio_parse_file` plus `pio_to_arrow`
for table `ybus`; PowerModels measures `parse_file`, `make_per_unit!`, and
`calc_admittance_matrix`; ExaPowerIO measures `parse_matpower` plus a sparse
Y bus assembled from its parsed branch admittance rows.

PowerWorld `.pwb` and `.aux` parse timings are measured by the Rust Criterion
benchmarks. Fetch the public fixtures, run
`cargo bench -p powerio --bench parse -- "parse_aux_|parse_pwb_"`, then run
`python3 benchmarks/extract_powerworld_bench.py` before rendering the tables. If
the Texas7k local row is published, pass its aux and pwb paths through
`POWERIO_BENCH_AUX` and `POWERIO_BENCH_PWB` during the Criterion run.

Matrix builder timings are separate from parse timings. The matrix benchmark
parses each fixture once, builds `IndexedNetwork` once, and times only derived
matrix construction. Its pipeline row measures `Pipeline::run` for the paired
\\(Y_{\mathrm{bus}}\\) export, including MTX, shunt, and metadata writes:

```sh
cargo bench -p powerio-matrix --bench matrix
python3 benchmarks/extract_matrix_bench.py
python3 benchmarks/render_tables.py
```

Use filtered runs while developing a focused change, for example:

```sh
cargo bench -p powerio-matrix --bench matrix -- 'matrix_bprime|matrix_ybus|dcopf_'
```

Criterion compares against the local `target/criterion` baseline. Treat a
`Performance has regressed` line as a signal to investigate rather than a
publishable claim by itself. A release note or benchmark page needs the commit, tree
cleanliness, machine, toolchain, command, fixtures, and whether optional large
cases were present.

Measure C ABI release size before publishing a C ABI change:

```sh
cargo build -p powerio-capi --release --no-default-features
cp target/release/libpowerio_capi.dylib /tmp/libpowerio_capi-core.dylib
cargo build -p powerio-capi --release --no-default-features --features arrow,matrix
cp target/release/libpowerio_capi.dylib /tmp/libpowerio_capi-arrow-matrix.dylib
cargo build -p powerio-capi --release --no-default-features --features arrow,matrix,gridfm,dist,pkg,prob
cp target/release/libpowerio_capi.dylib /tmp/libpowerio_capi-all.dylib
stat -f '%z %N' /tmp/libpowerio_capi-core.dylib \
  /tmp/libpowerio_capi-arrow-matrix.dylib \
  /tmp/libpowerio_capi-all.dylib
```
