# Performance

PowerIO has five benchmark tiers, listed below. They answer different
questions, so keep their numbers separate when you publish them.

| tier | command | what it answers |
| --- | --- | --- |
| Rust microbenchmarks | `cargo bench -p powerio-tx --bench parse` | parser, writer, and PowerWorld reader timing inside one process |
| Matrix microbenchmarks | `cargo bench -p powerio-matrix --bench matrix` | sparse matrix, DC OPF component, and dense sensitivity builder timing after parse/indexing |
| Cross tool parser and matrix comparison | `julia --project=evals/validation evals/performance/bench_julia.jl --json` | powerio through the C ABI against ExaPowerIO.jl and PowerModels.jl, including parse plus Y bus construction |
| Python parser comparison | `.venv/bin/python evals/performance/bench_parse.py --json <cases>` | Python package parse and matrix path against pandapower reader paths |
| C ABI release size | three `cargo build -p powerio-capi --release` feature sets plus `stat` | binary size for core, `arrow,matrix`, and all release features |

The published tables come from `evals/performance/render_tables.py`, which
renders the JSON the harnesses write, and this page is the reference for how
those numbers are made. Each refresh also records the snapshot environment:
machine model, chip, core count, memory, OS, Rust, C compiler, Julia, Python,
and the package versions of the comparison harnesses. Regenerate the JSON
inputs first, then render:

```sh
bash evals/validation/fetch_cases.sh
cargo build --release -p powerio-capi --features arrow,matrix
python3.11 -m venv .venv
.venv/bin/python -m pip install --upgrade pip maturin -r evals/validation/requirements.txt
env VIRTUAL_ENV=$PWD/.venv .venv/bin/maturin develop --release
julia --project=evals/validation evals/performance/bench_julia.jl --json
.venv/bin/python evals/performance/bench_parse.py --json \
  tests/data/case2869pegase.m \
  tests/data/large/case9241pegase.m \
  tests/data/large/case13659pegase.m \
  tests/data/large/case193k.m
# tests/data/large/ is not in the repository; evals/validation/fetch_cases.sh fills it.
python3 evals/performance/render_tables.py
python3 evals/performance/render_tables.py --check
```

The Julia benchmark writes `rows` for parse only and `matrix_rows` for parse
plus Y bus construction. Each tool is timed on its own path: PowerIO on ABI 7
parsing and typed network access, PowerModels on `parse_file`,
`make_per_unit!`, and `calc_admittance_matrix`, and ExaPowerIO on
`parse_matpower` plus a sparse Y bus assembled from its parsed branch
admittance rows.

The Rust Criterion benchmarks measure PowerWorld `.pwb` and `.aux` parse
timings. Fetch the public fixtures, run
`cargo bench -p powerio-tx --bench parse -- "parse_aux_|parse_pwb_"`, then run
`python3 evals/performance/extract_powerworld_bench.py` before you render the
tables. If you are publishing the Texas7k local row, pass its aux and pwb
paths through `POWERIO_BENCH_AUX` and `POWERIO_BENCH_PWB` during the Criterion
run.

Matrix builder timings do not include parsing. The matrix benchmark parses
each fixture once, builds `IndexedNetwork` once, and times only the derived
matrix construction. Its pipeline row measures `Pipeline::run` for the paired
\\(Y_{\mathrm{bus}}\\) export, including MTX, shunt, and metadata writes:

```sh
cargo bench -p powerio-matrix --bench matrix
python3 evals/performance/extract_matrix_bench.py
python3 evals/performance/render_tables.py
```

## Parse and DC OPF preparation

`cargo bench -p powerio-matrix --bench dcopf` times the path a consumer pays
before a solver sees anything: the MATPOWER reader, `DcOpfInstance::from_network`,
and `build_dc_opf_preparation`. The vendored `case2869pegase` always runs;
point `POWERIO_BENCH_PGLIB` at a pglib-opf checkout to add
`pglib_opf_case13659_pegase.m`.

```sh
POWERIO_BENCH_PGLIB=~/Datasets/pglib-opf cargo bench -p powerio-matrix --bench dcopf
```

On `pglib_opf_case13659_pegase.m` (13659 buses, 20467 branches), 0.11.3
measures:

| Stage | 0.11.2 | 0.11.3 |
| --- | --- | --- |
| MATPOWER parse | 92.0 ms | 92.5 ms |
| `DcOpfInstance::from_network` | 22.1 ms | 0.47 ms |
| `build_dc_opf_preparation` | 27.9 ms | 19.6 ms |
| instance plus preparation | 50.0 ms | 20.1 ms |

The instance step spent nearly all of its time collecting every stated
identity into a set that a case with no missing identity never reads;
`assign_missing_component_ids` now fills that set only when a table is short
one. The preparation step spent its extra time copying every source
identity into an owned string for the four constraint masks, which now borrow
from the source tables. The prepared arrays are unchanged: the 27 file DC OPF
bundle for this case is identical before and after.

While you work on one builder, filter the run down to the benchmarks you care
about, for example:

```sh
cargo bench -p powerio-matrix --bench matrix -- 'matrix_bprime|matrix_ybus|dcopf_'
```

Criterion compares each run against whatever baseline is in your local
`target/criterion`, so treat a `Performance has regressed` line as a reason
to investigate rather than as a publishable claim by itself. A number that
goes into a release note or benchmark page needs the commit, tree
cleanliness, machine, toolchain, command, fixtures, and whether the optional
large cases were present.

Before you publish a C ABI change, measure the release binary size. ABI 7
exports one symbol set and only the `gridfm` feature changes the binary, so
two builds cover the range (the library suffix is `.so` on Linux, `.dylib` on
macOS, and `.dll` on Windows):

```sh
cargo build -p powerio-capi --release --no-default-features
cp target/release/libpowerio_capi.so /tmp/libpowerio_capi-core.so
cargo build -p powerio-capi --release --no-default-features --features gridfm
cp target/release/libpowerio_capi.so /tmp/libpowerio_capi-gridfm.so
stat -c '%s %n' /tmp/libpowerio_capi-core.so /tmp/libpowerio_capi-gridfm.so
```
