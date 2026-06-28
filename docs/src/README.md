# PowerIO Guide

PowerIO parses power system case files into a typed `Network`, converts between
formats, and builds sparse matrices and graph views for solver and analysis code.
This guide is the reader facing documentation. Rustdoc remains the API reference.

The trust model is evidence based:

- same format write back preserves retained source text;
- cross format conversion keeps the electrical core and reports losses as
  warnings;
- matrix builders state sign, tap, shift, shunt, and reference bus conventions;
- benchmarks separate local wall time from correctness gates;
- C, Python, and Julia bindings share the same Rust parser and converter.

Reference pages:

- [format-fidelity.md](format-fidelity.md): numeric conventions every reader
  and writer follows, how they're validated against four independent tools, and the
  per-format limits reported in `Conversion::warnings`.
- [matrices.md](matrices.md): the matrix family `powerio-matrix` builds and the
  sign, tap, per unit, DC, and GridFM conventions across them.
- [dcopf-bundle.md](dcopf-bundle.md): the Matrix Market + manifest schema the
  `dcopf` subcommand writes for a downstream solver.
- [generator-cost-policy.md](generator-cost-policy.md): how missing generator
  costs are handled across PSS/E, MATPOWER, DC OPF, GridFM, and future adapters.
- [languages.md](languages.md): canonical Rust, Python, Julia, and C ABI names.
- [python.md](python.md): Python install extras and API examples.
- [powerworld.md](powerworld.md): PowerWorld AUX, PWB, and PWD evidence.
- [architecture.md](architecture.md): the compiler-IR architecture and the
  `.pio.json` compiler package and its schema.
- [performance.md](performance.md): benchmark tiers and refresh commands.
- [reliability.md](reliability.md): local gates and what each gate proves.
- [contributor-workflow.md](contributor-workflow.md): review, test, validation,
  and benchmark update workflow.
- [releasing.md](releasing.md): release checklist.
- Julia bindings: <https://github.com/eigenergy/PowerIO.jl>.

Rendered API docs (rustdoc) for all crates:
<https://eigenergy.github.io/powerio/>.

## Architecture

`Network` is the canonical model: format neutral, with loads, shunts, branches,
and generators as first-class records. Every reader produces a `Network` and every
writer consumes one, so a format is one reader/writer at the hub rather than a
pairwise converter, and adding one touches a single module. `IndexedNetwork` is the
dense `[0, n)` analysis view derived from a `Network`; the matrix builders work from
it. The parser, the hub, the source retaining writer, and the converters live in the
`powerio` crate (light dependencies); the matrix builders and graph outputs live in
`powerio-matrix`, which re-exports `powerio` so one import pulls in both layers.

| crate | responsibility |
| --- | --- |
| `powerio` | parsers, writers, `Network`, `IndexedNetwork`, normalization, format routing |
| `powerio-matrix` | sparse matrices, graph views, DC OPF bundle, GridFM datasets |
| `powerio-cli` | command line interface and TUI |
| `powerio-py` | PyO3 extension for the Python package |
| `powerio-capi` | C ABI used by C, C++, Julia, and other foreign function interfaces |
| `powerio-dist` | multiconductor distribution model and converters |
| `powerio-pkg` | `.pio.json` package envelope |

Code that maps source bus ids to dense rows must use
`IndexedNetwork::bus_index`; it must not clamp ids or assume 1 based contiguous
ids.
