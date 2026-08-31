//! Regenerates the checked-in BMOPF example outputs in `examples/bmopf/`
//! (`cargo run -p powerio-dist --example regen_bmopf_examples`). Pass
//! `--check` to fail without writing when an example is stale. The IEEE
//! feeders re-convert from their vendored OpenDSS masters; 4bus_dy has no
//! vendored dss source, so it canonicalizes through parse + emit (which,
//! unlike the CLI, does not take the same-format echo).

use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

struct ExampleEmission {
    text: String,
    diagnostic_count: usize,
}

fn emit_bmopf(
    module: powerio_core::PioModule<powerio_dist::MulticonductorNetwork>,
) -> ExampleEmission {
    let result = powerio_dist::emit(
        &module.sever_source(),
        powerio_dist::DistTargetFormat::BmopfJson,
        powerio_core::Destination::memory("example.json").unwrap(),
    )
    .unwrap();
    let diagnostic_count = result.diagnostics().len();
    let powerio_core::EmittedOutput::Memory { mut artifacts } = result.into_output() else {
        unreachable!("memory destination returns memory output");
    };
    let artifact = artifacts.pop().expect("BMOPF emission has one artifact");
    ExampleEmission {
        text: String::from_utf8(artifact.into_bytes()).expect("BMOPF JSON is UTF-8"),
        diagnostic_count,
    }
}

fn check_or_update(rel: &str, emitted: &ExampleEmission, check: bool) -> bool {
    let path = root().join("examples/bmopf").join(rel);
    if check {
        let actual = std::fs::read_to_string(&path).unwrap();
        if actual != emitted.text {
            eprintln!("stale {}", path.display());
            return false;
        }
        println!(
            "checked {} ({} bytes, {} warnings)",
            path.display(),
            emitted.text.len(),
            emitted.diagnostic_count
        );
    } else {
        std::fs::write(&path, &emitted.text).unwrap();
        println!(
            "wrote {} ({} bytes, {} warnings)",
            path.display(),
            emitted.text.len(),
            emitted.diagnostic_count
        );
    }
    true
}

fn main() {
    let mut args = std::env::args().skip(1);
    let check = match args.next().as_deref() {
        None => false,
        Some("--check") => true,
        Some(other) => panic!("unknown argument `{other}`; expected --check"),
    };
    assert!(args.next().is_none(), "expected at most one argument");
    let mut current = true;
    for (dss, out) in [
        ("opendss/ieee34/ieee34Mod1.dss", "ieee34.json"),
        ("opendss/ieee123/IEEE123Master.dss", "ieee123.json"),
    ] {
        let source =
            powerio_core::Source::open(root().join("../tests/data/dist").join(dss)).unwrap();
        let module = powerio_dist::parse(source).unwrap();
        current &= check_or_update(out, &emit_bmopf(module), check);
    }
    let source = powerio_core::Source::open(root().join("examples/bmopf/4bus_dy.json")).unwrap();
    let module = powerio_dist::parse(source).unwrap();
    current &= check_or_update("4bus_dy.json", &emit_bmopf(module), check);
    if !current {
        std::process::exit(1);
    }
}
