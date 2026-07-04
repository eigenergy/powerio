# PowerIO Guide

PowerIO is compiler infrastructure for power system data. Source formats parse
into typed models. Explicit, recorded passes normalize, validate, and lower
them, and writers emit any supported target format. The `.pio.json` document
records how a source was interpreted: model kind, provenance, source maps,
structured diagnostics, validation, and lowering history. Sparse matrices and
graph views are built from the same models for solver and analysis code. This
guide records behavior, conventions, and release checks. Rustdoc covers API
detail.

The rules these pages document:

- same format write back preserves retained source text;
- cross format conversion keeps the electrical core and reports losses as
  warnings;
- lowering between model families is an explicit, recorded pass, never an
  implicit side effect;
- matrix builders state sign, tap, shift, shunt, and reference bus conventions;
- C, Python, and Julia bindings share the same Rust core.

Transmission readers cover MATPOWER, PSS/E revisions 33 through 35,
PowerWorld AUX and PWB, PSLF EPC, PowerModels JSON, egret JSON, pandapower JSON,
PyPSA CSV folders, GO Challenge 3 JSON, Surge JSON, GridFM Parquet datasets, and
PowerIO JSON snapshots. PowerWorld PWD is a display artifact and uses the
display API. Distribution readers and writers live in `powerio-dist` for
OpenDSS, PowerModelsDistribution ENGINEERING JSON, and BMOPF JSON.

Where to look:

- [Compiler IR](https://powerio.dev/guide/compiler-ir.html): the
  `BalancedNetwork` and `MulticonductorNetwork` model families and the
  `.pio.json` document.
- [PIO JSON schema](https://powerio.dev/guide/pio-json-schema.html): the
  `.pio.json` field reference, metadata and model JSON versioning, and row
  identity.
- [Format fidelity](https://powerio.dev/guide/format-fidelity.html): numeric
  conventions, the validation oracles, known limits per format, and the missing
  generator cost policy.
- [Matrix outputs](https://powerio.dev/guide/matrices.html) and the
  [DC OPF bundle](https://powerio.dev/guide/dcopf-bundle.html).
- [Language APIs](https://powerio.dev/guide/languages.html) and
  [Python](https://powerio.dev/guide/python.html).
- [Performance](https://powerio.dev/guide/performance.html) and
  [testing and release checks](https://powerio.dev/guide/contributor-workflow.html).
- Julia bindings: <https://github.com/eigenergy/PowerIO.jl>.

Rendered API docs (rustdoc) for all crates: <https://powerio.dev>.

## Crates

| crate | responsibility |
| --- | --- |
| `powerio` | parsers, writers, `Network`, `IndexedNetwork`, normalization, format routing |
| `powerio-matrix` | sparse matrices, graph views, DC OPF bundle, GridFM datasets |
| `powerio-dist` | multiconductor distribution model and converters |
| `powerio-pkg` | `.pio.json` document metadata and model JSON |
| `powerio-cli` | command line interface and TUI |
| `powerio-py` | PyO3 extension for the Python package |
| `powerio-capi` | C ABI for C, C++, Julia, and other foreign function interfaces |

Adding a format means adding one reader or writer at the hub, not pairwise
converters. `IndexedNetwork` is the dense \\([0,n)\\) analysis view derived from
a balanced `Network`; matrix builders work from that view. Code that maps
source bus ids to dense rows must use `IndexedNetwork::bus_index`; it must not
clamp ids or assume 1 based contiguous ids.
