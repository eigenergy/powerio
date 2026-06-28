# Reliability Evidence

The reliability story is a set of gates, not a single test command.

## Local Gates

Run these before publishing a release claim:

```sh
cargo fmt --all --check
cargo clippy --all-targets
cargo test
cargo test -p powerio-cli --test cli
cargo test -p powerio-capi
cargo test -p powerio-capi --no-default-features
cargo test -p powerio-capi --features arrow,gridfm,dist
cargo clippy -p powerio-capi --all-targets --no-default-features -- -D warnings
cargo clippy -p powerio-capi --all-targets --features arrow,gridfm,dist -- -D warnings
cargo build -p powerio-py
python3.12 -m venv .venv
.venv/bin/python -m pip install --upgrade pip maturin -r benchmarks/requirements.txt
env VIRTUAL_ENV=$PWD/.venv .venv/bin/maturin develop --release
.venv/bin/pytest python/tests
cargo build -p powerio-capi --release --features arrow,gridfm,dist
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

`benchmarks/run_validation.sh` checks the classic transmission paths against
PowerModels.jl, ExaPowerIO.jl, egret, pandapower, and the full legacy reader to
writer matrix. PyPSA, pandapower, and egret are required Python oracles for the
validation run; a missing import is a setup failure.

`benchmarks/run_rich_validation.sh` covers fields outside the MATPOWER row
shape: branch terminal admittance, switches, branch current ratings and solution
values, storage current ratings, HVDC costs, and load voltage models.
The committed rich oracle is a strict PowerModels.jl check, so missing Julia is
a setup failure.

## What Is Proved

The gates prove these properties when all required legs pass:

- writing back to the original file type preserves retained source text for
  formats whose readers keep it;
- conversions to another file type preserve the electrical core checked by the
  oracle suite;
- Y_bus agrees with pandapower/PYPOWER on the MATPOWER corpus;
- PSS/E read and write paths agree with PowerModels.jl on counts and aggregate
  power quantities;
- PowerModels JSON writer and reader paths are checked separately;
- C ABI handle, string buffer, null, warning, and header/export behavior passes
  their crate tests;
- the checked in C header declares the same `pio_*` symbols exported from the
  Rust C ABI source;
- the checked in C and C++ examples compile and run against the release C ABI
  with Arrow, GridFM, and distribution features enabled;
- Python parse, conversion, matrix, graph, package, and display paths pass their
  binding tests when extras are installed.
- ASV discovers and runs the Python parse, Y_bus, and B' benchmark definitions
  against the installed wheel.
- the parser fuzz targets build and enter libFuzzer, covering the hand written
  text and binary readers.
- Julia's `PowerIO.jl` passes against the local release C ABI with Arrow,
  GridFM, and distribution features enabled.
- CLI help, stdout/stderr, JSON summary, batch matrix export metadata,
  directory target errors, and family mismatch exits pass through the binary
  integration tests.

The gates do not prove every source format field is lossless. Known losses are
part of the public behavior and must surface as warnings.
