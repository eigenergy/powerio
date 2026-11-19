//! A pass over the transformers of a large feeder looks each winding's bus
//! up once per coil. Ignored by default: it only reports a time.
//!
//! `cargo test -p powerio-dist --release --test bus_lookup_scaling -- --ignored --nocapture`

mod helpers;
use std::fmt::Write as _;
use std::time::Instant;

use helpers::parse_dss_str;
use powerio_dist::transformer::calc_transformer_primitive_admittance;

const TRANSFORMERS: usize = 10_000;

/// 10,000 wye-wye transformers, each feeding four line sections: 50,001 buses.
fn feeder() -> String {
    let mut dss = String::from("New Circuit.c basekv=12.47 bus1=sourcebus\n");
    for k in 0..TRANSFORMERS {
        writeln!(
            dss,
            "New Line.feed{k} bus1=sourcebus.1.2.3 bus2=H{k}.1.2.3 phases=3 length=0.1\n\
             New Transformer.t{k} phases=3 windings=2 buses=(h{k}.1.2.3.0, l{k}.1.2.3.0) \
             conns=(wye, wye) kvs=(12.47, 0.48) kvas=(500, 500) xhl=5"
        )
        .unwrap();
        for section in 0..3 {
            let from = if section == 0 {
                format!("l{k}")
            } else {
                format!("m{k}_{}", section - 1)
            };
            writeln!(
                dss,
                "New Line.s{k}_{section} bus1={from}.1.2.3 bus2=m{k}_{section}.1.2.3 phases=3 length=0.05"
            )
            .unwrap();
        }
    }
    dss
}

#[test]
#[ignore = "timing report"]
fn a_pass_over_the_transformers_of_a_large_feeder() {
    let net = parse_dss_str(&feeder());
    assert_eq!(net.transformers().len(), TRANSFORMERS);
    let start = Instant::now();
    let mut entries = 0usize;
    for transformer in net.transformers() {
        let primitive = calc_transformer_primitive_admittance(&net, transformer).unwrap();
        entries += primitive.terminals.len();
    }
    eprintln!(
        "{} buses, {} transformers: one pass in {:.3?} ({entries} terminals)",
        net.buses().len(),
        net.transformers().len(),
        start.elapsed()
    );
}
