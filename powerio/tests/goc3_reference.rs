use std::path::Path;

use powerio::{PioValue, Source};

#[test]
fn parses_the_pinned_goc3_benchmark_cases() {
    let Ok(root) = std::env::var("POWERIO_GOC3_REFERENCE_DATA") else {
        eprintln!(
            "POWERIO_GOC3_REFERENCE_DATA is unset; the pinned reference-data CI job runs this test"
        );
        return;
    };
    let paths = reference_paths(Path::new(&root));
    assert_eq!(
        paths.len(),
        3,
        "the pinned GOC3Benchmark revision has D1, D2, and D3"
    );

    for path in paths {
        assert_benchmark_case(&path);
    }
}

fn reference_paths(root: &Path) -> Vec<std::path::PathBuf> {
    let mut paths: Vec<_> = std::fs::read_dir(root)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", root.display()))
        .map(|entry| entry.expect("reference-data directory entry").path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .collect();
    paths.sort();
    paths
}

fn assert_benchmark_case(path: &Path) {
    let module = powerio::parse(Source::open(path).unwrap(), Some("goc3"))
        .unwrap_or_else(|error| panic!("failed to parse {}: {error}", path.display()));
    let PioValue::AcScucInstance(instance) = &module.value() else {
        panic!("{} did not produce powerio.AcScucInstance", path.display());
    };
    let name = path.file_name().unwrap().to_string_lossy();
    let expected = expected_case(&name);
    assert_dimensions_and_time_axis(instance, &expected, path);
    assert_bus_and_branch_values(instance, path);
    assert_device_values(instance, &expected, path);

    let retained_only: Vec<_> = module
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code() == "READ.GOC3.RETAINED_SOURCE_ONLY")
        .collect();
    assert_eq!(
        retained_only.len(),
        1,
        "{}: {retained_only:?}",
        path.display()
    );
    assert!(
        retained_only[0]
            .message()
            .contains("con_loss_factor retained in source only for 73 buses"),
        "{}: {retained_only:?}",
        path.display()
    );
}

struct ExpectedCase {
    interval_durations: Vec<f64>,
    startup_cost_window_end: f64,
    energy_upper_bound: Option<f64>,
}

fn expected_case(name: &str) -> ExpectedCase {
    if name.contains("D1_") {
        ExpectedCase {
            interval_durations: [vec![0.25; 8], vec![0.5; 8], vec![1.0; 2]].concat(),
            startup_cost_window_end: 8.0,
            energy_upper_bound: None,
        }
    } else if name.contains("D2_") {
        ExpectedCase {
            interval_durations: vec![1.0; 48],
            startup_cost_window_end: 24.0,
            energy_upper_bound: Some(13.2),
        }
    } else if name.contains("D3_") {
        ExpectedCase {
            interval_durations: vec![4.0; 42],
            startup_cost_window_end: 24.0,
            energy_upper_bound: Some(13.2),
        }
    } else {
        panic!("unexpected pinned benchmark file {name}");
    }
}

fn assert_dimensions_and_time_axis(
    instance: &powerio::AcScucInstance,
    expected: &ExpectedCase,
    path: &Path,
) {
    let network = instance.network();
    let inputs = instance.inputs();
    let expected_periods = expected.interval_durations.len();
    assert_close(network.base_mva(), 100.0, "system base MVA", path);
    assert_eq!(network.buses().len(), 73, "{}", path.display());
    assert_eq!(network.branches().len(), 120, "{}", path.display());
    assert_eq!(network.hvdc().len(), 1, "{}", path.display());
    assert_eq!(inputs.devices.len(), 205, "{}", path.display());
    assert_eq!(inputs.shunts.len(), 73, "{}", path.display());
    assert_eq!(
        inputs.branch_switching_costs.len(),
        120,
        "{}",
        path.display()
    );
    assert_eq!(inputs.transformer_controls.len(), 15, "{}", path.display());
    assert_eq!(inputs.active_reserve_zones.len(), 1, "{}", path.display());
    assert_eq!(inputs.reactive_reserve_zones.len(), 1, "{}", path.display());
    assert_eq!(inputs.contingencies.len(), 2, "{}", path.display());
    assert_eq!(
        inputs.interval_durations.len(),
        expected_periods,
        "{}",
        path.display()
    );
    assert!(
        inputs
            .devices
            .iter()
            .all(|device| device.periods.len() == expected_periods),
        "{}",
        path.display()
    );
    assert_eq!(
        inputs.interval_durations,
        expected.interval_durations,
        "{}: interval durations in hours",
        path.display()
    );
}

fn assert_bus_and_branch_values(instance: &powerio::AcScucInstance, path: &Path) {
    let network = instance.network();
    let inputs = instance.inputs();
    let bus = network
        .buses()
        .iter()
        .find(|bus| bus.uid.as_deref() == Some("bus_00"))
        .expect("pinned benchmark bus_00");
    assert_close(bus.base_kv, 138.0, "bus_00 base voltage kV", path);
    assert_close(bus.vm, 1.04844, "bus_00 voltage magnitude p.u.", path);
    assert_close(
        bus.va,
        -0.187_781_021_419_621_1_f64.to_degrees(),
        "bus_00 voltage angle degrees",
        path,
    );
    assert_close(bus.vmin, 0.95, "bus_00 minimum voltage p.u.", path);
    assert_close(bus.vmax, 1.05, "bus_00 maximum voltage p.u.", path);

    let ac_line = network
        .branches()
        .iter()
        .find(|branch| branch.uid.as_deref() == Some("acl_000"))
        .expect("pinned benchmark acl_000");
    assert_close(ac_line.r, 0.003, "acl_000 resistance p.u.", path);
    assert_close(ac_line.x, 0.026, "acl_000 reactance p.u.", path);
    assert_close(ac_line.b, 0.055, "acl_000 line charging p.u.", path);
    assert_close(ac_line.rate_a, 500.0, "acl_000 nominal rating MVA", path);

    let transformer = network
        .branches()
        .iter()
        .find(|branch| branch.uid.as_deref() == Some("xfr_00"))
        .expect("pinned benchmark xfr_00");
    assert_close(transformer.tap, 1.03, "xfr_00 tap ratio p.u.", path);
    assert_close(transformer.shift, 0.0, "xfr_00 phase shift degrees", path);
    assert_close(transformer.rate_a, 400.0, "xfr_00 nominal rating MVA", path);
    assert!(
        transformer.control.is_none(),
        "{}: SCUC phase shift bounds are not automatic transformer control",
        path.display()
    );
    let transformer_input = inputs
        .transformer_controls
        .iter()
        .find(|control| control.id.local_id() == "xfr_00")
        .expect("pinned benchmark xfr_00 SCUC bounds");
    assert_close(
        transformer_input.tap_ratio_min,
        1.03,
        "xfr_00 minimum SCUC tap ratio p.u.",
        path,
    );
    assert_close(
        transformer_input.tap_ratio_max,
        1.03,
        "xfr_00 maximum SCUC tap ratio p.u.",
        path,
    );
    assert_close(
        transformer_input.phase_shift_min,
        0.0,
        "xfr_00 minimum SCUC phase shift radians",
        path,
    );
    assert_close(
        transformer_input.phase_shift_max,
        0.0,
        "xfr_00 maximum SCUC phase shift radians",
        path,
    );

    let dc_line = network
        .hvdc()
        .iter()
        .find(|line| line.uid.as_deref() == Some("dcl_0"))
        .expect("pinned benchmark dcl_0");
    assert_close(dc_line.pmin, -100.0, "dcl_0 minimum active power MW", path);
    assert_close(dc_line.pmax, 100.0, "dcl_0 maximum active power MW", path);
    assert_close(
        dc_line.qmaxf,
        100.0,
        "dcl_0 from side maximum reactive power MVAr",
        path,
    );

    let generator = network
        .generators()
        .iter()
        .find(|generator| generator.uid.as_deref() == Some("sd_000"))
        .expect("pinned benchmark sd_000 generator");
    assert_close(generator.pmin, 22.0, "sd_000 active power minimum MW", path);
    assert_close(generator.pmax, 55.0, "sd_000 active power maximum MW", path);
    assert_close(
        generator.qmin,
        -15.0,
        "sd_000 reactive power minimum MVAr",
        path,
    );
    assert_close(
        generator.qmax,
        19.0,
        "sd_000 reactive power maximum MVAr",
        path,
    );
}

fn assert_device_values(instance: &powerio::AcScucInstance, expected: &ExpectedCase, path: &Path) {
    let inputs = instance.inputs();
    let device = inputs.device("sd_000").expect("pinned benchmark sd_000");
    assert_close(
        device.ramp_limits.startup,
        0.55,
        "sd_000 startup ramp limit p.u./hour",
        path,
    );
    assert_close(
        device.ramp_limits.shutdown,
        0.55,
        "sd_000 shutdown ramp limit p.u./hour",
        path,
    );
    assert_close(
        device.reserve_limits.regulation_up,
        0.185,
        "sd_000 regulation up reserve limit p.u.",
        path,
    );
    assert_close(
        device.reserve_limits.nonsynchronized,
        0.37,
        "sd_000 non-synchronized reserve limit p.u.",
        path,
    );
    assert_close(
        device.periods[0].active_power_min,
        0.22,
        "sd_000 first interval active power minimum p.u.",
        path,
    );
    assert_close(
        device.periods[0].active_power_max,
        0.55,
        "sd_000 first interval active power maximum p.u.",
        path,
    );
    assert_close(
        device.periods[0].energy_cost_blocks[0].marginal_cost,
        2_333.498_166,
        "sd_000 first marginal cost $/(p.u. h)",
        path,
    );
    assert_close(
        device.periods[0].energy_cost_blocks[0].block_size,
        0.22,
        "sd_000 first cost block p.u.",
        path,
    );
    assert_close(
        device.startup_cost_adjustments[0].maximum_down_time,
        expected.startup_cost_window_end,
        "sd_000 startup cost window hours",
        path,
    );
    match expected.energy_upper_bound {
        Some(energy) => {
            assert_eq!(device.energy_upper_bounds.len(), 1, "{}", path.display());
            assert_close(
                device.energy_upper_bounds[0].energy,
                energy,
                "sd_000 energy upper bound p.u.",
                path,
            );
        }
        None => assert!(device.energy_upper_bounds.is_empty(), "{}", path.display()),
    }
}

fn assert_close(actual: f64, expected: f64, quantity: &str, path: &Path) {
    let tolerance = 1e-10 * expected.abs().max(1.0);
    assert!(
        (actual - expected).abs() <= tolerance,
        "{}: {quantity} is {actual}, expected {expected}",
        path.display()
    );
}
