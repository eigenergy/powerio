//! PyPSA CSV folders with snapshot-local time series siblings read as a
//! balanced network time series with shared static tables.

use powerio_tx::__parse_pypsa_csv_time_series;

fn write_folder(dir: &std::path::Path, files: &[(&str, &str)]) {
    std::fs::create_dir_all(dir).unwrap();
    for (name, content) in files {
        std::fs::write(dir.join(name), content).unwrap();
    }
}

const STATIC_TABLES: [(&str, &str); 4] = [
    ("network.csv", "name\nseq\n"),
    ("buses.csv", "name,v_nom\nB1,138.0\nB2,138.0\n"),
    ("loads.csv", "name,bus,p_set,q_set\nL1,B2,5.0,1.0\n"),
    (
        "generators.csv",
        "name,bus,control,p_nom,p_set\nG1,B1,Slack,100.0,12.0\n",
    ),
];

fn parse(
    dir: &std::path::Path,
) -> Result<
    (
        powerio_core::TimeSeries<powerio_tx::BalancedNetwork>,
        Vec<powerio_tx::Diagnostic>,
    ),
    powerio_tx::Error,
> {
    let source = powerio_core::Source::open(dir).unwrap();
    let sequence = __parse_pypsa_csv_time_series(&source)?;
    Ok((sequence.series, sequence.diagnostics))
}

#[test]
fn input_series_produce_a_shared_table_sequence() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("seq");
    write_folder(&dir, &STATIC_TABLES);
    write_folder(
        &dir,
        &[
            ("snapshots.csv", ",snapshot\n0,now\n1,later\n"),
            ("loads-p_set.csv", "snapshot,L1\nnow,10.0\nlater,20.0\n"),
        ],
    );
    let (series, _diagnostics) = parse(&dir).unwrap();
    assert_eq!(series.len(), 2);
    let first = &series.values()[0];
    let second = &series.values()[1];
    assert!((first.loads()[0].p - 10.0).abs() < f64::EPSILON);
    assert!((second.loads()[0].p - 20.0).abs() < f64::EPSILON);
    // Static tables are one allocation across the series; only the varied
    // load table was copied per point.
    assert!(std::ptr::eq(
        first.buses().as_ptr(),
        second.buses().as_ptr()
    ));
    assert!(std::ptr::eq(
        first.generators().as_ptr(),
        second.generators().as_ptr()
    ));
    assert!(!std::ptr::eq(
        first.loads().as_ptr(),
        second.loads().as_ptr()
    ));
    assert_eq!(series.time_points()[1].label(), "later");
}

#[test]
fn state_series_patch_the_solved_voltages() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("state");
    write_folder(&dir, &STATIC_TABLES);
    write_folder(
        &dir,
        &[
            ("snapshots.csv", ",snapshot\n0,now\n1,later\n"),
            (
                "buses-v_mag_pu.csv",
                "snapshot,B1,B2\nnow,1.0,0.99\nlater,1.0,0.97\n",
            ),
            (
                "buses-v_ang.csv",
                "snapshot,B1,B2\nnow,0.0,-0.017453292519943295\nlater,0.0,-0.03490658503988659\n",
            ),
        ],
    );
    let (series, _diagnostics) = parse(&dir).unwrap();
    let later = &series.values()[1];
    assert!((later.buses()[1].vm - 0.97).abs() < f64::EPSILON);
    // v_ang is radians in PyPSA and degrees on the bus.
    assert!((later.buses()[1].va + 2.0).abs() < 1e-9);
    assert!(std::ptr::eq(
        series.values()[0].loads().as_ptr(),
        later.loads().as_ptr()
    ));
}

#[test]
fn an_out_of_profile_series_is_reported_and_retained() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("outside");
    write_folder(&dir, &STATIC_TABLES);
    write_folder(
        &dir,
        &[
            ("snapshots.csv", ",snapshot\n0,now\n"),
            ("stores-e.csv", "snapshot,S1\nnow,4.0\n"),
        ],
    );
    let (series, diagnostics) = parse(&dir).unwrap();
    assert_eq!(series.len(), 1);
    assert!(
        diagnostics
            .iter()
            .any(|d| d.message().contains("stores-e.csv")
                && d.message()
                    .contains("outside the snapshot-local series profile")),
        "{diagnostics:?}"
    );
}

#[test]
fn a_row_axis_disagreement_is_refused() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("axis");
    write_folder(&dir, &STATIC_TABLES);
    write_folder(
        &dir,
        &[
            ("snapshots.csv", ",snapshot\n0,now\n1,later\n"),
            ("loads-p_set.csv", "snapshot,L1\nnow,10.0\n"),
        ],
    );
    let error = parse(&dir).unwrap_err().to_string();
    assert!(error.contains("states 1 rows for 2 snapshots"), "{error}");
}

#[test]
fn an_unknown_element_column_is_refused() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("unknown");
    write_folder(&dir, &STATIC_TABLES);
    write_folder(
        &dir,
        &[
            ("snapshots.csv", ",snapshot\n0,now\n"),
            ("loads-p_set.csv", "snapshot,L9\nnow,10.0\n"),
        ],
    );
    let error = parse(&dir).unwrap_err().to_string();
    assert!(error.contains("names no element"), "{error}");
}

#[test]
fn branch_limits_use_each_components_own_nominal_rating() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    write_folder(dir, &STATIC_TABLES);
    write_folder(
        dir,
        &[
            ("snapshots.csv", "snapshot\nzero\nunbounded\nfinite\n"),
            (
                "lines.csv",
                "name,bus0,bus1,x,s_nom,s_max_pu\nshared,B1,B2,0.1,100,1\nfixed,B1,B2,0.2,20,0.5\n",
            ),
            (
                "transformers.csv",
                "name,bus0,bus1,x,s_nom,tap_ratio\nshared,B1,B2,0.1,50,1.1\n",
            ),
            (
                "lines-s_max_pu.csv",
                "snapshot,shared\nzero,0\nunbounded,inf\nfinite,0.75\n",
            ),
            (
                "transformers-s_max_pu.csv",
                "snapshot,shared\nzero,inf\nunbounded,0\nfinite,0.5\n",
            ),
        ],
    );
    let source = powerio_core::Source::open(dir).unwrap();
    let sequence = __parse_pypsa_csv_time_series(&source).unwrap();
    assert!(sequence.inputs_vary);
    assert!(sequence.has_varying_columns);
    assert!(
        !sequence
            .diagnostics
            .iter()
            .any(|d| d.message().contains("s_max_pu.csv"))
    );
    for (net, ratings) in sequence.series.values().iter().zip([
        [0.0, 10.0, f64::INFINITY],
        [f64::INFINITY, 10.0, 0.0],
        [75.0, 10.0, 25.0],
    ]) {
        assert_eq!(
            net.branches().iter().map(|b| b.rate_a).collect::<Vec<_>>(),
            ratings
        );
        assert!(net.branches().iter().all(|b| b.in_service));
        assert!((net.branches()[2].x - 0.002).abs() < 1e-12);
        assert!((net.branches()[2].tap - 1.1).abs() < 1e-12);
    }
    for invalid in ["-1", "NaN", "-inf", "1e308", "4.9406564584124654e-324"] {
        let nominal = if invalid == "4.9406564584124654e-324" {
            "0.25"
        } else {
            "100"
        };
        write_folder(
            dir,
            &[
                (
                    "lines.csv",
                    &format!("name,bus0,bus1,x,s_nom\nshared,B1,B2,0.1,{nominal}\n"),
                ),
                (
                    "lines-s_max_pu.csv",
                    &format!("snapshot,shared\nzero,{invalid}\nunbounded,1\nfinite,1\n"),
                ),
            ],
        );
        assert!(parse(dir).is_err(), "accepted {invalid}");
    }
}
