//! Regenerates the checked-in BMOPF example outputs in `examples/bmopf/`
//! (`cargo run -p powerio-dist --example regen_bmopf_examples`). The IEEE
//! feeders re-convert from their vendored OpenDSS masters; 4bus_dy has no
//! vendored dss source, so it canonicalizes through parse + write (which,
//! unlike the CLI, does not take the same-format echo).

use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn write(rel: &str, conv: &powerio_dist::Conversion) {
    let path = root().join("examples/bmopf").join(rel);
    std::fs::write(&path, &conv.text).unwrap();
    println!(
        "wrote {} ({} warnings)",
        path.display(),
        conv.warnings.len()
    );
}

fn main() {
    for (dss, out) in [
        ("opendss/ieee34/ieee34Mod1.dss", "ieee34.json"),
        ("opendss/ieee123/IEEE123Master.dss", "ieee123.json"),
    ] {
        let net =
            powerio_dist::parse_dss_file(root().join("../tests/data/dist").join(dss)).unwrap();
        write(out, &powerio_dist::write_bmopf_json(&net));
    }
    let net = powerio_dist::parse_bmopf_file(root().join("examples/bmopf/4bus_dy.json")).unwrap();
    write("4bus_dy.json", &powerio_dist::write_bmopf_json(&net));
}
