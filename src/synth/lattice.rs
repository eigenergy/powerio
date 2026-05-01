//! Square 2-D lattice / grid topology. `n` is rounded up to the nearest
//! perfect square.

use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;

use crate::case::MpcCase;

use super::tree::{make_branch, make_bus};
use super::SynthSpec;

pub fn generate_lattice(spec: &SynthSpec) -> MpcCase {
    let side = ((spec.n as f64).sqrt().ceil() as usize).max(2);
    let n = side * side;
    let mut rng = ChaCha8Rng::seed_from_u64(spec.seed);

    let buses = (0..n).map(|i| make_bus(i + 1)).collect();
    let mut branches = Vec::with_capacity(2 * side * (side - 1));
    for r in 0..side {
        for c in 0..side {
            let idx = r * side + c;
            if c + 1 < side {
                branches.push(make_branch(idx + 1, idx + 2, spec, &mut rng));
            }
            if r + 1 < side {
                branches.push(make_branch(idx + 1, idx + side + 1, spec, &mut rng));
            }
        }
    }

    MpcCase::new(
        format!("synth_lattice_{side}x{side}"),
        100.0,
        buses,
        branches,
    )
}
