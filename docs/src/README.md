# PowerIO Guide

PowerIO parses power system case files into a typed `Network`, converts between
formats, and builds sparse matrices and graph views for solver and analysis code.
This guide records behavior, conventions, and release checks. Rustdoc covers API
details.

The rules these pages document are:

- same format write back preserves retained source text;
- cross format conversion keeps the electrical core and reports losses as
  warnings;
- matrix builders state sign, tap, shift, shunt, and reference bus conventions;
- benchmarks keep local wall time separate from correctness gates;
- C, Python, and Julia bindings share the same Rust parser and converter.

Transmission readers cover MATPOWER, PSS/E revisions 33 through 35,
PowerWorld AUX and PWB, PSLF EPC, PowerModels JSON, egret JSON, pandapower JSON,
PyPSA CSV folders, GO Challenge 3 JSON, Surge JSON, GridFM Parquet datasets, and
PowerIO JSON snapshots. PowerWorld PWD is a display artifact and uses the
display API. Distribution readers and writers live in `powerio-dist` for
OpenDSS, PowerModelsDistribution ENGINEERING JSON, and BMOPF JSON.

Reference pages:

- [Format fidelity](https://eigenergy.github.io/powerio/guide/format-fidelity.html): numeric conventions every reader
  and writer follows, how they're validated against four independent tools, and the
  per-format limits reported in `Conversion::warnings`.
- [Matrix outputs and conventions](https://eigenergy.github.io/powerio/guide/matrices.html): the matrix family `powerio-matrix` builds and the
  sign, tap, per unit, DC, and GridFM conventions across them.
- [DC OPF bundle](https://eigenergy.github.io/powerio/guide/dcopf-bundle.html): the Matrix Market + manifest schema the
  `dcopf` subcommand writes for a downstream solver.
- [Generator cost policy](https://eigenergy.github.io/powerio/guide/generator-cost-policy.html): how missing generator
  costs are handled across PSS/E, MATPOWER, DC OPF, GridFM, and adapters.
- [Language APIs](https://eigenergy.github.io/powerio/guide/languages.html): shared Rust, Python, Julia, and C ABI names.
- [Python](https://eigenergy.github.io/powerio/guide/python.html): Python install extras and API examples.
- [PowerWorld](https://eigenergy.github.io/powerio/guide/powerworld.html): PowerWorld AUX, PWB, and PWD evidence.
- [Architecture](https://eigenergy.github.io/powerio/guide/architecture.html): the compiler-IR architecture and the
  `.pio.json` package, operating points, and schema.
- [Performance](https://eigenergy.github.io/powerio/guide/performance.html): benchmark tiers and refresh commands.
- [Reliability evidence](https://eigenergy.github.io/powerio/guide/reliability.html): local gates and what each gate proves.
- [Contributor workflow](https://eigenergy.github.io/powerio/guide/contributor-workflow.html): review, test, validation,
  and benchmark update workflow.
- Julia bindings: <https://github.com/eigenergy/PowerIO.jl>.

Rendered API docs (rustdoc) for all crates:
<https://eigenergy.github.io/powerio/>.

## Architecture

`powerio::Network` is the current balanced transmission model; v0.4 also exports
`powerio::BalancedNetwork` as the v1 family name for that same type. Loads,
shunts, branches, and generators are first class records. `powerio_dist` keeps
the separate `MulticonductorNetwork` distribution model. `.pio.json` packages
one of those model families with provenance, source maps, diagnostics,
validation, summaries, and lowering history.

Adding a format means adding one reader or writer at the hub, not pairwise
converters. `IndexedNetwork` is the dense \\([0,n)\\) analysis view derived from a
balanced `Network`; matrix builders work from that view. The parser, source
retaining writer, and converters live in `powerio`; matrix builders and graph
outputs live in `powerio-matrix`, which re-exports `powerio`.

| crate | responsibility |
| --- | --- |
| `powerio` | parsers, writers, `Network`, `IndexedNetwork`, normalization, format routing |
| `powerio-matrix` | sparse matrices, graph views, DC OPF bundle, GridFM datasets |
| `powerio-cli` | command line interface and TUI |
| `powerio-py` | PyO3 extension for the Python package |
| `powerio-capi` | C ABI for C, C++, Julia, and other foreign function interfaces |
| `powerio-dist` | multiconductor distribution model and converters |
| `powerio-pkg` | `.pio.json` package envelope |

Code that maps source bus ids to dense rows must use
`IndexedNetwork::bus_index`; it must not clamp ids or assume 1 based contiguous
ids.
