# Architecture

`Network` is the canonical transmission model. Every reader produces it, every
writer consumes it, and every binding exposes it directly or through a handle.
That keeps a new format as one adapter at the hub rather than a grid of
pairwise converters.

The workspace is intentionally split:

| crate | responsibility |
| --- | --- |
| `powerio` | parsers, writers, `Network`, `IndexedNetwork`, normalization, format routing |
| `powerio-matrix` | sparse matrices, graph views, DC OPF bundle, GridFM datasets |
| `powerio-cli` | command line interface and TUI |
| `powerio-py` | PyO3 extension for the Python package |
| `powerio-capi` | C ABI used by C, C++, Julia, and other foreign function interfaces |
| `powerio-dist` | multiconductor distribution model and converters |
| `powerio-pkg` | `.pio.json` package envelope |

The parser crate stays free of matrix, TUI, data frame, and Python dependencies.
The matrix crate depends on `powerio` and re-exports it so solver code can
import one crate when it needs both layers.

`IndexedNetwork` is the dense `[0, n)` analysis view derived from `Network`.
Code that maps source bus ids to dense rows must go through `bus_index`; it must
not clamp or assume 1 based contiguous ids.

More detailed design notes live in the source tree:

- [compiler IR](https://github.com/eigenergy/powerio/blob/main/docs/architecture/compiler-ir.md)
- [PIO JSON schema](https://github.com/eigenergy/powerio/blob/main/docs/architecture/pio-json-schema.md)
- [v0.4 release direction](https://github.com/eigenergy/powerio/blob/main/docs/architecture/v0.4-release-direction.md)
