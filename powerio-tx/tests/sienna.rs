//! Versioned Sienna electrical exchange through the public module API.
mod helpers;
use helpers::{emit_module, emit_value};
use powerio_core::{FormatId, Source};
use powerio_tx::TargetFormat;

#[test]
fn export_reference_cases() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/data");
    for case in ["case9", "case14", "case30", "case57", "case118"] {
        let source = Source::open(root.join(format!("{case}.m"))).unwrap();
        let module = powerio_tx::parse(source).unwrap();
        for version in 1..=4 {
            let format = TargetFormat::SiennaJson { version };
            let written = emit_value(module.value(), format).unwrap();
            let source =
                Source::from_memory(format!("{case}.json"), written.text.as_bytes().to_vec())
                    .unwrap()
                    .with_format(FormatId::new(format.token()).unwrap());
            let recovered = powerio_tx::parse(source).unwrap();
            assert_eq!(
                module.value().buses().len(),
                recovered.value().buses().len()
            );
            assert_eq!(
                module.value().branches().len(),
                recovered.value().branches().len()
            );
            assert_eq!(
                module.value().generators().len(),
                recovered.value().generators().len()
            );
            assert_eq!(emit_module(&recovered, format).unwrap().text, written.text);
            if let Some(dir) = std::env::var_os("POWERIO_SIENNA_OUTPUT_DIR") {
                let dir = std::path::PathBuf::from(dir).join(format!("0.{version}.0"));
                std::fs::create_dir_all(&dir).unwrap();
                std::fs::write(dir.join(format!("{case}.json")), written.text).unwrap();
            }
        }
    }
}

#[test]
fn three_winding_connections_and_pair_bases_survive() {
    use powerio_tx::network::{BusId, Impedance, MagnetizingConnection, Transformer3W, Winding};
    let source = Source::open(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/data/case9.m"),
    )
    .unwrap();
    let module = powerio_tx::parse(source).unwrap();
    for placement in [
        MagnetizingConnection::StarPoint,
        MagnetizingConnection::Winding1Terminal,
    ] {
        let mut net = module.value().clone();
        let mut t = Transformer3W::new(
            [
                Winding::new(BusId(1)),
                Winding::new(BusId(2)),
                Winding::new(BusId(3)),
            ],
            [
                Impedance::new(0.01, 0.1, 50.0),
                Impedance::new(0.02, 0.2, 75.0),
                Impedance::new(0.03, 0.3, 80.0),
            ],
        );
        t.mag_g = 0.001;
        t.mag_b = -0.02;
        t.magnetizing_connection = placement;
        t.windings[0].tap = 1.08;
        t.windings[1].shift = 12.0;
        t.star_vm = 0.98;
        t.star_va = -3.0;
        net.transformers_3w_mut().push(t.clone());
        for version in 1..=4 {
            let out = emit_value(&net, TargetFormat::SiennaJson { version }).unwrap();
            let recovered =
                powerio_tx::parse(Source::from_memory("case.json", out.text.into_bytes()).unwrap())
                    .unwrap();
            let actual = &recovered.value().transformers_3w()[0];
            assert_eq!(actual.magnetizing_connection, placement);
            for (a, b) in actual.windings.iter().zip(&t.windings) {
                assert_eq!(a.bus, b.bus);
                assert!((a.tap-b.tap).abs() < 1e-12);
                assert!((a.shift-b.shift).abs() < 1e-12);
                assert_eq!(a.control, b.control);
            }
            assert!((actual.mag_g - t.mag_g).abs() < 1e-12);
            assert!((actual.mag_b - t.mag_b).abs() < 1e-12);
            assert_eq!(recovered.value().buses().len(), net.buses().len());
            for (a, b) in actual.z.iter().zip(t.z) {
                assert!((a.r - b.r).abs() < 1e-12);
                assert!((a.x - b.x).abs() < 1e-12);
                assert!((a.base_mva - b.base_mva).abs() < 1e-12);
            }
        }
    }
}
