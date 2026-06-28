# PowerIO

PowerIO parses power system case files into a typed `Network`, converts between
formats, and builds sparse matrices and graph views for solver and analysis
code. This book is the reviewable guide layer for the project. Rustdoc remains
the API reference, and the existing guides under `docs/` remain the detailed
source notes.

The trust model is evidence based:

- writing back to the original file type preserves the original text when the
  reader kept it;
- converting to another file type keeps the electrical core and reports losses
  in warnings;
- matrix builders state their sign, tap, shift, shunt, and reference bus
  conventions;
- benchmarks separate local wall time from correctness gates;
- C, Python, and Julia bindings share the same Rust parser and converter.

Start with [Architecture](architecture.md) for crate boundaries,
[Formats and Fidelity](formats-and-fidelity.md) for interchange behavior, then
[Numerical Conventions](numerical-conventions.md) and
[Reliability Evidence](reliability.md) before relying on benchmark numbers.

Existing detailed references:

- [format fidelity](../guides/format-fidelity.html)
- [matrix conventions](../guides/matrices.html)
- [Python API](../guides/python.html)
- [language API map](../guides/languages.html)
- [PowerWorld evidence](../guides/powerworld.html)
- [benchmark results](https://github.com/eigenergy/powerio/blob/main/benchmarks/RESULTS.md)
