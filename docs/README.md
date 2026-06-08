# powerio docs

Reference material that doesn't fit in the top-level [README](../README.md) or a
crate doc comment.

- [format-fidelity.md](format-fidelity.md) — the numeric conventions every reader
  and writer follows, how they're validated against four independent tools, and the
  per-format limits reported in `Conversion::warnings`.
- [matrices.md](matrices.md) — the matrix family `powerio-matrix` builds and the
  sign, tap, per unit, and DC conventions across them.
- [dcopf-bundle.md](dcopf-bundle.md) — the Matrix Market + manifest schema the
  `dcopf` subcommand writes for a downstream solver.

Rendered API docs (rustdoc) for all crates:
<https://eigenergy.github.io/powerio/>.

## Architecture

`Network` is the one canonical model: format neutral, with loads, shunts, branches,
and generators as first-class records. Every reader produces a `Network` and every
writer consumes one, so a format is one reader/writer at the hub rather than a
pairwise converter, and adding one touches a single module. `IndexedNetwork` is the
dense `[0, n)` analysis view derived from a `Network`; the matrix builders work from
it. The parser, the hub, the lossless writer, and the converters live in the
`powerio` crate (light dependencies); the matrices and graph views live in
`powerio-matrix`, which re-exports `powerio` so one import pulls in both layers.
