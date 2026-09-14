# Developer guides

These pages are for contributors and for anyone whose code depends on PowerIO
internals. They cover the change from 0.10, the PowerIO IR document and its
field reference, the BMOPF mapping, the crate graph, the design decisions
borrowed from LLVM and MLIR, the DC OPF bundle, the private corpus harness,
benchmarks, and the checks a release runs.

Everything here describes the current implementation; earlier releases are in
the changelog. The dated design record behind the 0.11 API lives outside the
guide at [`docs/design/`](https://github.com/eigenergy/powerio/tree/main/docs/design)
and is not API authority.

## Panic sites in library code

`unwrap`, `expect`, `panic!`, `unreachable!`, `todo!`, and `unimplemented!`
across the workspace, counted as of 0.11.3. The library column excludes
`#[cfg(test)]` modules, `tests/` trees, and `benches/`; a panic there ends a
test, not a caller's program.

| Crate | Library | `#[cfg(test)]` | `tests/` | `benches/` |
| --- | --- | --- | --- | --- |
| powerio | 67 | 172 | 921 | 0 |
| powerio-core | 5 | 343 | 0 | 0 |
| powerio-tx | 220 | 1497 | 809 | 15 |
| powerio-prob | 99 | 59 | 129 | 0 |
| powerio-matrix | 16 | 182 | 441 | 20 |
| powerio-dist | 46 | 181 | 560 | 0 |
| powerio-cli | 14 | 101 | 247 | 0 |
| powerio-py | 1 | 17 | 0 | 0 |
| powerio-capi | 20 | 108 | 0 | 0 |
| **Total** | **488** | **2660** | **3107** | **35** |

Of the 488 library sites, 366 are `expect` with a stated reason, 46 are
`unreachable!` on an exhaustive match, 4 are `panic!`, and 74 are a bare
`unwrap`. Of those 74, 35 are in `examples/`, which are programs rather than
library code. The 39 that ship are: a fixed-size `try_into` on a slice the
reader has already sized (PowerWorld `.pwb`), an `Option` a check three lines
above has already refused to be `None` (the CGMES SV writer, the BMOPF extras
writer), and the arm of a `match` on a length.

Adding a bare `unwrap` to library code needs a reason a reader can check
locally. A value that comes from a source file is not one: raise a coded
`Diagnostic` instead.
