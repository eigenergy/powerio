# powerio-synth

Deterministic, topology-only synthetic power-system network generators for tests,
benchmarks, and CLI tooling. The crate depends on `powerio-tx`, not on matrix
calculation, so cases can be generated without pulling in the matrix stack.

Supported topologies are `Tree`, `Lattice2D`, and `PegaseLike`. Identical
specifications and seeds produce identical networks.

## Example

```rust
use powerio_synth::{generate, SynthSpec, Topology};

let spec = SynthSpec {
    topology: Topology::Tree,
    n: 32,
    r_over_x: 0.1,
    mean_x: 0.05,
    seed: 0x00C0_FFEE,
};
let network = generate(&spec);
assert_eq!(network.buses().len(), 32);
```

The former `powerio_matrix::synth` API remains available as a compatibility
facade. New code can use `powerio-synth` directly.
