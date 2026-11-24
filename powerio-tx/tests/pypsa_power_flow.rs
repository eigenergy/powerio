//! The PyPSA CSV writer states the source case's power flow (#920): PV and
//! slack setpoints on the bus, a usable `v_nom`, pi model transformers, and
//! the bus voltage state of the one snapshot.
mod helpers;
#[allow(unused_imports)]
use helpers::*;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use powerio_tx::BalancedNetwork;

fn data(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/data")
        .join(name)
}

/// The written folder as `file name -> text`, plus the rendered findings.
fn write(
    case: &str,
) -> (
    BalancedNetwork,
    PathBuf,
    BTreeMap<String, String>,
    Vec<String>,
) {
    let net = parse_matpower_file(data(case)).unwrap();
    let temp = tempfile::tempdir().unwrap().keep();
    let out = temp.join("pypsa");
    let written = emit_pypsa_csv_folder(&net, &out).unwrap();
    let files = written
        .files
        .iter()
        .map(|path| {
            (
                path.file_name().unwrap().to_str().unwrap().to_owned(),
                std::fs::read_to_string(path).unwrap(),
            )
        })
        .collect();
    let diagnostics = written.render_diagnostics();
    (net, out, files, diagnostics)
}

fn row(text: &str, index: usize) -> Vec<&str> {
    text.lines().nth(index).unwrap().split(',').collect()
}

fn column(text: &str, name: &str) -> usize {
    row(text, 0).iter().position(|h| *h == name).unwrap()
}

#[test]
fn pv_and_slack_setpoints_are_written_on_the_bus() {
    let (net, out, files, _) = write("case9.m");
    let buses = &files["buses.csv"];
    let v_set = column(buses, "v_mag_pu_set");
    // case9 bus 1 is the slack, held at Vg = 1.04; bus 2 is PV at 1.025.
    assert_eq!(row(buses, 1)[v_set], "1.04");
    assert_eq!(row(buses, 2)[v_set], "1.025");
    // A PQ bus keeps its stated magnitude.
    assert_eq!(row(buses, 4)[v_set], "1");
    // The generator column stays for the round trip, and the bus's stated
    // magnitude comes back from the state file rather than as Vg.
    let back = read_pypsa_csv_folder(&out).unwrap().network;
    assert!((back.generators()[0].vg - 1.04).abs() < 1e-12);
    assert!((back.buses()[0].vm - net.buses()[0].vm).abs() < 1e-12);
}

#[test]
fn a_bus_without_the_generator_column_takes_vg_from_the_bus() {
    // A PyPSA written folder has no generator v_mag_pu_set: the PV bus's
    // setpoint is the generator's voltage target.
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    std::fs::write(
        dir.join("buses.csv"),
        "name,v_nom,v_mag_pu_set\nB1,138,1.04\nB2,138,1\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("generators.csv"),
        "name,bus,control,p_nom,p_set\nG1,B1,Slack,100,10\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("lines.csv"),
        "name,bus0,bus1,r,x,s_nom\nL1,B1,B2,1,10,100\n",
    )
    .unwrap();
    let back = read_pypsa_csv_folder(dir).unwrap().network;
    assert!((back.generators()[0].vg - 1.04).abs() < 1e-12);
}

#[test]
fn a_zero_base_voltage_is_written_as_a_substitute_v_nom() {
    let (net, out, files, diagnostics) = write("case14.m");
    let buses = &files["buses.csv"];
    assert_eq!(row(buses, 1)[column(buses, "v_nom")], "1");
    // Line 1 is r = 0.01938 pu; at v_nom = 1 kV and 100 MVA, z_base is 0.01.
    let lines = &files["lines.csv"];
    let r: f64 = row(lines, 1)[column(lines, "r")].parse().unwrap();
    assert!((r - 0.01938 * 0.01).abs() < 1e-15, "{r}");
    assert!(
        diagnostics
            .iter()
            .any(|d| d.contains("EMIT.PYPSA.VALUE_SUBSTITUTED") && d.contains("v_nom = 1 kV")),
        "{diagnostics:?}"
    );
    // Per unit impedances survive the round trip on the substitute base.
    let back = read_pypsa_csv_folder(&out).unwrap().network;
    // The reader appends transformers after the lines, so compare the lines.
    let lines_of = |n: &BalancedNetwork| -> Vec<(f64, f64)> {
        n.branches()
            .iter()
            .filter(|b| !b.is_transformer())
            .map(|b| (b.r, b.x))
            .collect()
    };
    let (back_lines, source_lines) = (lines_of(&back), lines_of(&net));
    assert_eq!(back_lines.len(), source_lines.len());
    for (a, b) in back_lines.iter().zip(&source_lines) {
        assert!((a.0 - b.0).abs() < 1e-12 && (a.1 - b.1).abs() < 1e-12);
    }
    for (a, b) in back.shunts().iter().zip(net.shunts()) {
        assert!((a.b - b.b).abs() < 1e-9);
    }
}

#[test]
fn transformers_are_written_as_pi_models() {
    let (_, _, files, _) = write("case118.m");
    let transformers = &files["transformers.csv"];
    let model = column(transformers, "model");
    for i in 1..transformers.lines().count() {
        assert_eq!(row(transformers, i)[model], "pi");
    }
}

#[test]
fn the_bus_voltage_state_is_written_and_read_back() {
    let (net, out, files, diagnostics) = write("case118.m");
    let v_mag = &files["buses-v_mag_pu.csv"];
    let v_ang = &files["buses-v_ang.csv"];
    assert_eq!(v_mag.lines().count(), 2);
    assert_eq!(row(v_mag, 1)[0], "now");
    // v_ang is radians in PyPSA: case118's reference bus 69 is at 30 degrees.
    // Columns follow buses.csv after the snapshot label.
    let ref_col = 1 + net.buses().iter().position(|b| b.id.0 == 69).unwrap();
    let angle: f64 = row(v_ang, 1)[ref_col].parse().unwrap();
    assert!((angle - 30f64.to_radians()).abs() < 1e-12, "{angle}");
    // The state files are read, not reported as ignored.
    let parsed = read_pypsa_csv_folder(&out).unwrap();
    let rendered = parsed.render_diagnostics();
    assert!(
        !rendered.iter().any(|d| d.contains("buses-v_")),
        "{rendered:?}"
    );
    assert!(!diagnostics.iter().any(|d| d.contains("buses-v_")));
    for (a, b) in parsed.network.buses().iter().zip(net.buses()) {
        assert!((a.vm - b.vm).abs() < 1e-12, "{} vm", b.id.0);
        assert!((a.va - b.va).abs() < 1e-9, "{} va", b.id.0);
    }
}

#[test]
fn a_single_snapshot_with_bus_state_stays_one_network() {
    let (_, out, _, _) = write("case9.m");
    let source = powerio_core::Source::open(&out).unwrap();
    assert_eq!(
        powerio_tx::format::__pypsa_axis(&source).unwrap(),
        powerio_tx::format::PypsaAxis::SingleSnapshot
    );
}

/// Drops one column from a CSV text, as PyPSA's exporter does for a static
/// attribute whose every value equals its default.
fn without_column(text: &str, name: &str) -> String {
    let drop = column(text, name);
    text.lines()
        .map(|line| {
            let mut fields: Vec<&str> = line.split(',').collect();
            fields.remove(drop);
            fields.join(",") + "\n"
        })
        .collect()
}

#[test]
fn a_missing_v_nom_reads_as_pypsas_default() {
    // case14 has BASE_KV = 0 on every bus, so the writer states v_nom = 1 kV
    // everywhere; PyPSA then omits the all default column when it re-exports
    // the folder. The reader takes PyPSA's default back instead of refusing.
    let (net, out, files, _) = write("case14.m");
    let stripped = without_column(&files["buses.csv"], "v_nom");
    assert!(!stripped.lines().next().unwrap().contains("v_nom"));
    std::fs::write(out.join("buses.csv"), stripped).unwrap();

    let parsed = read_pypsa_csv_folder(&out).unwrap();
    let rendered = parsed.render_diagnostics();
    assert!(
        rendered
            .iter()
            .any(|d| d.contains("READ.PYPSA.VALUE_DEFAULTED") && d.contains("v_nom")),
        "{rendered:?}"
    );
    let back = parsed.network;
    assert!(
        back.buses()
            .iter()
            .all(|b| (b.base_kv - 1.0).abs() < f64::EPSILON)
    );
    let lines_of = |n: &BalancedNetwork| -> Vec<(f64, f64)> {
        n.branches()
            .iter()
            .filter(|b| !b.is_transformer())
            .map(|b| (b.r, b.x))
            .collect()
    };
    let (back_lines, source_lines) = (lines_of(&back), lines_of(&net));
    assert_eq!(back_lines.len(), source_lines.len());
    for (a, b) in back_lines.iter().zip(&source_lines) {
        assert!((a.0 - b.0).abs() < 1e-12 && (a.1 - b.1).abs() < 1e-12);
    }
}

#[test]
fn a_stated_v_nom_that_is_not_a_number_is_refused() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    std::fs::write(dir.join("buses.csv"), "name,v_nom\nB1,abc\n").unwrap();
    let err = read_pypsa_csv_folder(dir).unwrap_err().to_string();
    assert!(err.contains("v_nom"), "{err}");
}
