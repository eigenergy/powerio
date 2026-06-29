# Performance

PowerIO has four benchmark tiers. Keep them separate when publishing numbers.

| tier | command | what it answers |
| --- | --- | --- |
| Rust microbenchmarks | `cargo bench -p powerio --bench parse` | parser, writer, and PowerWorld reader timing inside one process |
| Matrix microbenchmarks | `cargo bench -p powerio-matrix --bench matrix` | sparse matrix, DC OPF component, and dense sensitivity builder timing after parse/indexing |
| Cross tool parser comparison | `julia --project=benchmarks benchmarks/bench_julia.jl --json` | powerio through the C ABI against ExaPowerIO.jl and PowerModels.jl |
| Python parser comparison | `.venv/bin/python benchmarks/bench_parse.py --json <cases>` | Python package parse and matrix path against pandapower reader paths |

The published table lives in the repository benchmark results, and this guide is
the public reference for how those numbers are produced. Each refresh should
update the snapshot environment there: machine model, chip,
core count, memory, OS, Rust, C compiler, Julia, Python, and the package
versions used by the comparison harnesses. Regenerate the JSON inputs first,
then splice only the marked regions:

```sh
bash benchmarks/fetch_cases.sh
cargo build --release -p powerio-capi
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

PowerWorld `.pwb` and `.aux` numbers come from Criterion. Fetch the public
fixtures, run `cargo bench -p powerio --bench parse -- "parse_aux_|parse_pwb_"`,
then run `python3 benchmarks/extract_powerworld_bench.py` before rendering the
tables. If the Texas7k local row is published, pass its aux and pwb paths through
`POWERIO_BENCH_AUX` and `POWERIO_BENCH_PWB` during the Criterion run.

Matrix builder timings are separate from parse timings. The matrix benchmark
parses each fixture once, builds `IndexedNetwork` once, and times only derived
matrix construction. Its pipeline row measures `Pipeline::run` for the paired
\(Y_{\mathrm{bus}}\) export, including MTX, shunt, and metadata writes:

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
`Performance has regressed` line as a signal to investigate, not as a publishable
claim by itself. A release note or benchmark page needs the commit, tree
cleanliness, machine, toolchain, command, fixtures, and whether optional large
cases were present.

Optimization work should start from measurement. The first audit targets are
allocation count, clone count, string churn, repeated dense work on sparse data,
quadratic scans, and cache behavior in parser and matrix hot paths.
