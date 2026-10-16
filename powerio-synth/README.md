# powerio-synth

Deterministic synthetic power-system network generators used for testing and benchmarking.

The generators produce topology-only `powerio::BalancedNetwork` values. Identical specifications and seeds produce identical cases.

Current topologies:
- `Tree`
- `Lattice2D`
- `PegaseLike`

The crate is separate from `powerio-matrix` so synthetic-case generation can be reused by benchmarks, tests, CLI tooling, and future training-data workflows without coupling it to matrix calculations.
