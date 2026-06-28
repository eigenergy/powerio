# FFI and Language Bindings

The C ABI is the stable boundary for non Rust callers. Handles own parsed
networks. Callers free network handles with `pio_network_free`, free returned
text with `pio_string_free`, size output buffers before filling them, and treat
every format name as a string routed through the same parser and writer hub.

C ABI review points:

- null handles must return documented defaults or errors, not crash;
- optional output buffers must be safe to pass as null; required output structs
  such as Arrow exports must report an error when null;
- returned text and warning buffers must be NUL terminated when capacity permits;
- reported lengths must let callers allocate exact buffers;
- header declarations and exported Rust symbols must match;
- feature gated exports such as Arrow and GridFM must be additive;
- ownership rules must be documented in the header, README, and binding code.

Python uses a mixed maturin layout: `powerio-py` builds the native
`powerio._powerio` module, and `python/powerio` provides the Python API. The base
import has no third party runtime dependency. Matrix and graph helpers assemble
SciPy and NetworkX objects only when the optional extras are installed. The
`bench` extra is for oracle work, not normal use: it follows the same Python
3.11+ pins as `benchmarks/requirements.txt` so PyPSA, pandapower, egret, and
matpowercaseframes resolve to the validated versions.

Julia's `PowerIO.jl` uses the C ABI for handles, dense extractors, Arrow,
GridFM, PyPSA CSV folders, and distribution conversion. Whole-network transport
uses `powerio-json`, so the binding does not stitch together a separate model
from individual table calls. Convenience functions follow the language API
table.

The Julia binding checks `pio_abi_version()` against `PIO_ABI_VERSION` on first
use. Distribution calls also check `pio_dist_abi_version()`. During development,
test the sibling binding against the local C ABI instead of an artifact:

```sh
cargo build -p powerio-capi --release --features arrow,gridfm,dist
POWERIO_CAPI=$PWD/target/release/libpowerio_capi.dylib \
  julia --project=../PowerIO.jl -e 'using Pkg; Pkg.test()'
```

That test exercises parse/convert, `powerio-json`, dense extractors, PyPSA CSV
writing, normalization, Arrow C Data Interface export, GridFM readback, and the
distribution surface when the corresponding C features are present.

Binding contracts checked in this audit:

| surface | contract |
| --- | --- |
| Python base import | `import powerio` does not import NumPy, SciPy, NetworkX, Polars, pandas, pyarrow, or the MCP SDK |
| Python optional paths | matrix, graph, GridFM inspection, pandas, MCP, and benchmark oracles live behind extras |
| C ABI | `pio_abi_version()` is the core compatibility check; optional symbols are additive and feature probed |
| Julia | `PowerIO.jl` checks the C ABI version before first use and checks `pio_dist_abi_version()` before distribution calls |
| Arrow | C returns Arrow C Data Interface structs; Julia's default `to_arrow` copies to owned vectors, while `copy=false` keeps the wrapper alive for zero copy reads |
| GridFM | Julia and C read GridFM through `pio_read_dir` / `"gridfm"` and surface schema losses as warnings |
| Distribution | Python, Julia, Rust, and C use separate distribution handles; transmission and distribution conversion paths do not mix |

See [language APIs](../guides/languages.html) for the canonical naming table.
