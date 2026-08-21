//! Regenerates the checked-in BMOPF example outputs in `examples/bmopf/`
//! (`cargo run -p powerio-dist --example regen_bmopf_examples`). Pass
//! `--check` to fail without writing when an example is stale. The IEEE
//! feeders re-convert from their vendored OpenDSS masters; 4bus_dy has no
//! vendored dss source, so it canonicalizes through parse + write (which,
//! unlike the CLI, does not take the same-format echo).

use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn check_or_write(rel: &str, conv: &powerio_dist::Conversion, check: bool) -> bool {
    let path = root().join("examples/bmopf").join(rel);
    if check {
        let actual = std::fs::read_to_string(&path).unwrap();
        if actual != conv.text {
            eprintln!("stale {}", path.display());
            return false;
        }
        println!(
            "checked {} ({} bytes, {} warnings)",
            path.display(),
            conv.text.len(),
            conv.warnings.len()
        );
    } else {
        std::fs::write(&path, &conv.text).unwrap();
        println!(
            "wrote {} ({} bytes, {} warnings)",
            path.display(),
            conv.text.len(),
            conv.warnings.len()
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
        let net =
            powerio_dist::parse_dss_file(root().join("../tests/data/dist").join(dss)).unwrap();
        current &= check_or_write(out, &powerio_dist::write_bmopf_json(&net), check);
    }
    let net = powerio_dist::parse_bmopf_file(root().join("examples/bmopf/4bus_dy.json")).unwrap();
    current &= check_or_write("4bus_dy.json", &powerio_dist::write_bmopf_json(&net), check);
    if !current {
        std::process::exit(1);
    }
}
