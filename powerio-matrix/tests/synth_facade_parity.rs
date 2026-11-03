//! Compatibility regression for the historical matrix synth API.

use powerio_matrix::synth as legacy;
use powerio_synth as standalone;
use powerio_tx::BalancedNetwork;

fn standalone_spec(spec: &legacy::SynthSpec) -> standalone::SynthSpec {
    standalone::SynthSpec {
        topology: match spec.topology {
            legacy::Topology::Tree => standalone::Topology::Tree,
            legacy::Topology::Lattice2D => standalone::Topology::Lattice2D,
            legacy::Topology::PegaseLike => standalone::Topology::PegaseLike,
        },
        n: spec.n,
        r_over_x: spec.r_over_x,
        mean_x: spec.mean_x,
        seed: spec.seed,
    }
}

fn same_network(a: &BalancedNetwork, b: &BalancedNetwork) {
    // Comparing their canonical public fields avoids requiring private model
    // identity to be shared between the legacy and standalone API types.
    assert_eq!(a.name(), b.name());
    assert_eq!(a.base_mva(), b.base_mva());
    assert_eq!(a.buses(), b.buses());
    assert_eq!(a.branches(), b.branches());
}

#[test]
fn facade_defaults_serde_and_all_generators_match_standalone() {
    let legacy_default = legacy::SynthSpec::default();
    let standalone_default = standalone::SynthSpec::default();
    assert_eq!(
        serde_json::to_value(&legacy_default).unwrap(),
        serde_json::to_value(&standalone_default).unwrap()
    );
    assert_eq!(legacy_default.n, standalone_default.n);
    assert_eq!(legacy_default.r_over_x, standalone_default.r_over_x);
    assert_eq!(legacy_default.mean_x, standalone_default.mean_x);
    assert_eq!(legacy_default.seed, standalone_default.seed);

    for topology in [
        legacy::Topology::Tree,
        legacy::Topology::Lattice2D,
        legacy::Topology::PegaseLike,
    ] {
        let legacy_spec = legacy::SynthSpec {
            topology,
            n: 17,
            r_over_x: 0.13,
            mean_x: 0.06,
            seed: 0x00C0_FFEE,
        };
        let impl_spec = standalone_spec(&legacy_spec);
        same_network(
            &legacy::generate(&legacy_spec),
            &standalone::generate(&impl_spec),
        );
        same_network(
            &legacy::generate_tree(&legacy_spec),
            &standalone::generate_tree(&standalone::SynthSpec {
                topology: standalone::Topology::Tree,
                ..impl_spec.clone()
            }),
        );
        same_network(
            &legacy::generate_lattice(&legacy_spec),
            &standalone::generate_lattice(&standalone::SynthSpec {
                topology: standalone::Topology::Lattice2D,
                ..impl_spec.clone()
            }),
        );
        same_network(
            &legacy::generate_pegase_like(&legacy_spec),
            &standalone::generate_pegase_like(&standalone::SynthSpec {
                topology: standalone::Topology::PegaseLike,
                ..impl_spec
            }),
        );
    }
}
