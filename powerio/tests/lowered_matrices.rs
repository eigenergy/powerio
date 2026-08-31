use powerio::transform::{MulticonductorToBalancedOptions, to_balanced_network};
use powerio_matrix::{BuildOptions, IndexedNetwork, calc_admittance_matrix, calc_bprime_matrix};

const FOUR_WIRE_DSS: &str = r"! Four wire line with an explicit neutral conductor (no Kron reduction).
Clear
Set DefaultBaseFrequency=60

New Circuit.fourwire basekv=0.416 pu=1.0 phases=3 bus1=sourcebus MVAsc3=2000 MVAsc1=2100

New Linecode.lc4 nphases=4 basefreq=60 units=km
~ rmatrix = (0.211 | 0.049 0.211 | 0.049 0.049 0.211 | 0.049 0.049 0.049 0.211)
~ xmatrix = (0.747 | 0.673 0.747 | 0.651 0.673 0.747 | 0.673 0.651 0.673 0.747)
~ cmatrix = (10.0 | 0.0 10.0 | 0.0 0.0 10.0 | 0.0 0.0 0.0 10.0)
~ normamps=185 emergamps=240

New Line.l1 bus1=sourcebus.1.2.3.0 bus2=loadbus.1.2.3.4 phases=4 linecode=lc4 length=0.4 units=km

New Load.la bus1=loadbus.1.4 phases=1 conn=wye kv=0.24 kw=8 pf=0.95 model=1 vminpu=0.8 vmaxpu=1.2
New Load.lb bus1=loadbus.2.4 phases=1 conn=wye kv=0.24 kw=6 pf=0.95 model=1 vminpu=0.8 vmaxpu=1.2
New Load.lc bus1=loadbus.3.4 phases=1 conn=wye kv=0.24 kw=10 pf=0.95 model=1 vminpu=0.8 vmaxpu=1.2
";

#[test]
fn lowered_multiconductor_balanced_model_builds_matrices() {
    let source = powerio_core::Source::from_bytes("<memory>", FOUR_WIRE_DSS.as_bytes().to_vec())
        .expect("memory source")
        .with_format(powerio_core::FormatId::new("dss").expect("format id"));
    let net = powerio_dist::parse(source)
        .expect("distribution text parses")
        .into_value();
    let lowered = to_balanced_network(&net, MulticonductorToBalancedOptions::default())
        .expect("lower to balanced");

    let view = IndexedNetwork::new(&lowered.network);
    let bprime = calc_bprime_matrix(&view, &BuildOptions::default()).expect("calculate Bp");
    let ybus = calc_admittance_matrix(&view, &BuildOptions::default()).expect("calculate Y bus");

    assert_eq!(bprime.rows(), view.n());
    assert_eq!(bprime.cols(), view.n());
    assert_eq!(ybus.g.rows(), view.n());
    assert_eq!(ybus.b.cols(), view.n());
    assert!(bprime.nnz() > 0);
}
