//! Pins the values the seeded synthesis generators produce.
//!
//! `SynthSpec::seed` is documented as fully determining the generated case, and
//! the pipeline reports a case digest that depends on it. Every generator draws
//! from `ChaCha8Rng::seed_from_u64`, so a change in how the random number
//! generator turns that stream into integers and floats shifts every value
//! below while still producing a structurally valid network. Without these
//! assertions such a shift passes unnoticed.

use powerio_matrix::synth::{SynthSpec, Topology, generate};

const SEED: u64 = 0x00C0_FFEE;

/// Relative tolerance on the reactance draws. Wide enough to absorb a
/// last-place difference between `exp` implementations, far tighter than the
/// distance between two draws from different streams.
const TOL: f64 = 1e-12;

/// Reactances of the three branches `generate_tree` produces at `n = 4`.
const TREE_X: [f64; 3] = [
    0.030_214_074_977_891_49,
    0.033_996_552_571_692_75,
    0.055_739_263_889_054_4,
];

/// Reactances of the seven branches `generate_pegase_like` produces at `n = 6`.
/// The first three repeat the tree values because both generators start from
/// the same seed and draw in the same order.
const PEGASE_X: [f64; 7] = [
    0.030_214_074_977_891_49,
    0.033_996_552_571_692_75,
    0.055_739_263_889_054_4,
    0.055_434_536_191_711_2,
    0.030_042_785_606_398_295,
    0.042_636_359_191_034_65,
    0.027_564_813_288_522_068,
];

fn spec(topology: Topology, n: usize) -> SynthSpec {
    SynthSpec {
        topology,
        n,
        r_over_x: 0.1,
        mean_x: 0.05,
        seed: SEED,
    }
}

#[track_caller]
fn assert_close(actual: f64, expected: f64, what: &str) {
    assert!(
        (actual - expected).abs() <= TOL * expected.abs(),
        "{what}: expected {expected:.17e}, got {actual:.17e}"
    );
}

/// `generate_tree` draws one integer per node to pick a parent and one float
/// per branch for the reactance, so the edge list and the reactances together
/// cover both draw kinds.
#[test]
fn tree_generator_output_is_pinned() {
    let case = generate(&spec(Topology::Tree, 4));
    let branches = case.branches();

    let edges: Vec<(usize, usize)> = branches.iter().map(|b| (b.from.0, b.to.0)).collect();
    assert_eq!(edges, vec![(1, 2), (2, 3), (3, 4)]);

    for (i, (branch, expected)) in branches.iter().zip(TREE_X).enumerate() {
        assert_close(branch.x, expected, &format!("tree branch {i} reactance"));
        assert_close(
            branch.r,
            0.1 * expected,
            &format!("tree branch {i} resistance"),
        );
    }
}

/// `generate_pegase_like` adds two more integer draws per extra edge on top of
/// the spanning tree, so it pins a longer stretch of the stream. At `n = 6`
/// that is a five branch backbone plus two cross edges.
#[test]
fn pegase_like_generator_output_is_pinned() {
    let case = generate(&spec(Topology::PegaseLike, 6));
    let branches = case.branches();

    let edges: Vec<(usize, usize)> = branches.iter().map(|b| (b.from.0, b.to.0)).collect();
    assert_eq!(
        edges,
        vec![(1, 2), (2, 3), (3, 4), (3, 5), (4, 6), (2, 4), (6, 5)]
    );

    for (i, (branch, expected)) in branches.iter().zip(PEGASE_X).enumerate() {
        assert_close(branch.x, expected, &format!("pegase branch {i} reactance"));
    }
}
