//! PowerIO: compiler infrastructure for power system data.
//!
//! The short `powerio` name is the entry facade over the component crates:
//! `powerio-core` (sources, diagnostics, errors, modules), `powerio-tx`
//! (the balanced transmission model and its format parsers and writers),
//! `powerio-dist` (the multiconductor distribution model), and `powerio-prob`
//! (operating points, problem instances, and solutions). The facade owns the
//! dynamic value boundary: [`PioValue`], [`PioValueKind`], the universal
//! [`parse`], and [`try_into_typed`].
//!
//! [`parse`] compiles one source into `PioModule<PioValue>`, routing to
//! whichever built in family claims it. A caller that expects one concrete
//! type narrows the module:
//!
//! ```no_run
//! let module = powerio::parse(powerio::Source::open("case9.m")?)?;
//! let module: powerio::PioModule<powerio::BalancedNetwork> =
//!     powerio::try_into_typed(module)?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! The family entries stay available for callers that already know the
//! family: [`format::parse`] for balanced network formats and
//! [`powerio_dist::parse`] for multiconductor ones.

pub use powerio_tx::*;

/// Core types a consumer needs to name a module: the generic module wrapper,
/// the source it was parsed from, a byte span into a source, the output
/// destination a write commits to, and the two repeated-value containers.
/// `Diagnostic` already arrives through [`powerio_tx`]'s own re-export, since
/// that one is itself `powerio_core::Diagnostic`.
pub use powerio_core::{
    Destination, PioModule, ScenarioSet, Source, SourceSpan, TimePoint, TimeSeries,
};

/// `powerio_tx::*` above already re-exports an `Error`/`Result` pair, but
/// those are powerio-tx's own 0.9 enum and its alias over it, tied to its
/// text format readers, not what [`parse`] and the source layer return. An
/// explicit `use` of a name shadows a glob import of the same name, so these
/// two items are what make `powerio::Error`/`powerio::Result` name the type
/// the facade's own functions actually use.
pub use powerio_core::Error;
pub type Result<T> = std::result::Result<T, powerio_core::Error>;

/// The distribution network type; [`powerio_dist::parse`] routes to it.
pub use powerio_dist::MulticonductorNetwork;

/// A problem instance builder. `powerio-prob` builds problem instances only;
/// it has no solution type to re-export alongside this one.
pub use powerio_prob::AcOpfInstance;

/// Matrix and graph data, re-exported from `powerio-matrix` under the
/// `matrix` feature. Matrix construction is never a parse result, so the
/// facade's automatic parsing and [`PioValue`] do not change with this
/// feature.
#[cfg(feature = "matrix")]
pub use powerio_matrix as matrix;

#[cfg(feature = "gridfm")]
mod collect;
#[cfg(feature = "gridfm")]
pub mod gridfm;

pub mod package;
pub mod stored;
pub mod write;
pub use write::{write_module_as, write_module_str, write_module_str_with_options};

/// The replayable operating state, named at the crate root beside the other
/// module types. From this layer up the 1.0 state type in powerio-prob is the
/// one the module surface stores and selects.
pub use powerio_prob::OperatingPoint;
mod value;
pub use value::{FromPioValue, PioValue, PioValueKind, ValueKindMismatch, try_into_typed};

/// Parse one source into a compiled module of whichever built in family
/// claims it. Balanced network formats produce
/// [`PioValue::BalancedNetwork`]; network only distribution formats (OpenDSS
/// `.dss`, PMD ENGINEERING JSON) produce
/// [`PioValue::MulticonductorNetwork`]. A source that defines a particular
/// calculation produces that calculation's value: DOE GO Challenge 3 JSON
/// produces [`PioValue::AcScucInstance`], BMOPF JSON produces
/// [`PioValue::McAcOpfInstance`], and DeepMind OPFData JSON, which explicitly
/// represents a solved AC OPF, produces [`PioValue::AcOpfSolution`]. The
/// reader's findings are the module's diagnostics and the source is retained
/// for the byte exact echo tier of the family's `write_as`.
///
/// The family comes from the source's declared format when one was selected,
/// and otherwise from the name and content: a `.dss` extension routes to the
/// distribution reader, a `.json` document routes by its top level markers
/// ([`format::routing::classify_json_text`]), a name with no recognized
/// extension whose content opens a JSON document (an in-memory source has no
/// extension to state) routes the same way, and every other name routes to
/// the balanced network hub, whose own detection and refusals apply.
///
/// Bare model JSON, the network serialization, decodes to
/// [`PioValue::BalancedNetwork`] like any other balanced source, and a
/// `.pio.json` document loads through the stored reader, including its one
/// way 0.9 upgrade, retaining the file as the module's runtime source.
///
/// # Errors
/// The routed family's failure, carrying its findings and the retained
/// source.
pub fn parse(
    source: powerio_core::Source,
) -> std::result::Result<powerio_core::PioModule<PioValue>, powerio_core::Error> {
    match routed_family(&source)? {
        RoutedFamily::Goc3 => {
            powerio_prob::parse_goc3_instance(source).map(|module| module.map_value(PioValue::from))
        }
        RoutedFamily::Bmopf => powerio_prob::parse_bmopf_instance(source)
            .map(|module| module.map_value(PioValue::from)),
        RoutedFamily::OpfData => powerio_prob::parse_opfdata_solution(source)
            .map(|module| module.map_value(PioValue::from)),
        RoutedFamily::Distribution => {
            powerio_dist::parse(source).map(|module| module.map_value(PioValue::from))
        }
        RoutedFamily::PypsaDirectory => parse_pypsa(source),
        #[cfg(feature = "gridfm")]
        RoutedFamily::Gridfm => parse_gridfm(source),
        RoutedFamily::Stored => parse_stored(source),
        RoutedFamily::Egret => parse_egret(source),
        RoutedFamily::Balanced(json_class) => format::parse_with_json_class(source, json_class)
            .map(|module| module.map_value(PioValue::from)),
    }
}

/// PyPSA CSV dispatch: one snapshot with no series siblings is the scalar
/// profile through the balanced hub; a declared axis routes to the sequence
/// reader, producing a network series or, when only complete solved state
/// varies, an operating point series over one shared network.
fn parse_pypsa(
    source: powerio_core::Source,
) -> std::result::Result<powerio_core::PioModule<PioValue>, powerio_core::Error> {
    if !source.is_directory() {
        // A file claiming the PyPSA token gets the hub's own refusal wording.
        return format::parse(source).map(|module| module.map_value(PioValue::from));
    }
    let axis = match format::pypsa_axis(&source) {
        Ok(axis) => axis,
        Err(error) => {
            let core = powerio_core::Error::new(error.code(), error.to_string());
            return Err(core.with_source(source));
        }
    };
    match axis {
        format::PypsaAxis::SingleSnapshot => {
            format::parse(source).map(|module| module.map_value(PioValue::from))
        }
        format::PypsaAxis::Series => match powerio_prob::parse_pypsa_sequence(&source) {
            Ok((sequence, diagnostics)) => {
                let value = match sequence {
                    powerio_prob::PypsaSequence::Networks(series) => {
                        PioValue::BalancedNetworkTimeSeries(series)
                    }
                    powerio_prob::PypsaSequence::OperatingPoints(states) => {
                        PioValue::BalancedOperatingPointTimeSeries(states)
                    }
                };
                powerio_core::PioModule::parsed(value, source, diagnostics)
            }
            Err(error) => Err(error.with_source(source)),
        },
    }
}

/// gridfm dispatch: every scenario of the Parquet dataset as one scenario
/// set over shared element identities.
#[cfg(feature = "gridfm")]
fn parse_gridfm(
    source: powerio_core::Source,
) -> std::result::Result<powerio_core::PioModule<PioValue>, powerio_core::Error> {
    if !source.is_directory() {
        // A file claiming the gridfm token gets the hub's own refusal wording.
        return format::parse(source).map(|module| module.map_value(PioValue::from));
    }
    match gridfm::parse_gridfm_source(&source) {
        Ok((set, diagnostics)) => powerio_core::PioModule::parsed(
            PioValue::BalancedNetworkScenarioSet(set),
            source,
            diagnostics,
        ),
        Err(error) => Err(error.with_source(source)),
    }
}

/// `.pio.json` dispatch: the versioned stored serialization of
/// `PioModule<PioValue>` loads through the stored reader, including the one
/// way 0.9 upgrade. The loaded module retains the `.pio.json` file as its
/// runtime source, so a same format write echoes it and diagnostics can
/// reference it.
fn parse_stored(
    source: powerio_core::Source,
) -> std::result::Result<powerio_core::PioModule<PioValue>, powerio_core::Error> {
    let loaded = {
        let buffer = source.primary_buffer()?;
        match std::str::from_utf8(buffer.content_bytes()) {
            Ok(text) => stored::read_module(text),
            Err(error) => {
                let cause = powerio_tx::Error::FormatRead {
                    format: "stored module",
                    message: format!("not valid UTF-8: {error}"),
                };
                Err(powerio_core::Error::new(cause.code(), cause.to_string()))
            }
        }
    };
    match loaded {
        Ok(module) => Ok(module.with_source(source)),
        Err(error) => Err(error.with_source(source)),
    }
}

/// Egret dispatch: a document declaring `system.time_keys` routes to the
/// sequence reader and produces a balanced network time series; a scalar
/// document routes through the balanced hub.
fn parse_egret(
    source: powerio_core::Source,
) -> std::result::Result<powerio_core::PioModule<PioValue>, powerio_core::Error> {
    let declares_series = {
        let buffer = source.primary_buffer()?;
        std::str::from_utf8(buffer.content_bytes()).is_ok_and(format::egret_declares_time_series)
    };
    if !declares_series {
        return format::parse(source).map(|module| module.map_value(PioValue::from));
    }
    let parsed = {
        let buffer = source.primary_buffer()?;
        let stem = std::path::Path::new(source.name())
            .file_stem()
            .and_then(|stem| stem.to_str())
            .map(str::to_owned);
        match std::str::from_utf8(buffer.content_bytes()) {
            Ok(text) => format::parse_egret_time_series(text, stem.as_deref())
                .map_err(|error| powerio_core::Error::new(error.code(), error.to_string())),
            Err(error) => {
                let cause = powerio_tx::Error::FormatRead {
                    format: "case text",
                    message: format!("not valid UTF-8: {error}"),
                };
                Err(powerio_core::Error::new(cause.code(), cause.to_string()))
            }
        }
    };
    match parsed {
        Ok(series) => powerio_core::PioModule::parsed(
            PioValue::BalancedNetworkTimeSeries(series),
            source,
            Vec::new(),
        ),
        Err(error) => Err(error.with_source(source)),
    }
}

/// The family a source routes to. The balanced hub is the default: it owns
/// the guidance for unknown names and refused shapes. `Balanced` carries the
/// JSON classification when routing here was itself the result of one (so
/// the balanced hub does not classify the same text a second time); `None`
/// when the source routed here by extension, by a declared token, or by a
/// directory shape, none of which run a JSON classification at all.
enum RoutedFamily {
    Balanced(Option<format::routing::JsonClass>),
    Distribution,
    Goc3,
    Bmopf,
    OpfData,
    PypsaDirectory,
    Egret,
    #[cfg(feature = "gridfm")]
    Gridfm,
    Stored,
}

fn routed_family(
    source: &powerio_core::Source,
) -> std::result::Result<RoutedFamily, powerio_core::Error> {
    if let Some(declared) = source.format() {
        return Ok(family_of_token(declared.as_str()));
    }
    if source.is_directory() {
        // Two directory case formats exist: a PyPSA CSV folder (one holding
        // a network.csv) and a gridfm Parquet dataset (bus_data.parquet at
        // the root, under raw/, or under one <case>/raw/). Anything else
        // falls to the balanced hub's refusal.
        let marker = powerio_core::ArtifactPath::new("network.csv")
            .expect("static name is a valid artifact path");
        if source.buffer(&marker).is_ok() {
            return Ok(RoutedFamily::PypsaDirectory);
        }
        #[cfg(feature = "gridfm")]
        if let Ok(entries) = source.entry_names()
            && entries.iter().any(|entry| {
                entry.as_str().ends_with("bus_data.parquet")
                    && matches!(entry.as_str().matches('/').count(), 0..=2)
            })
        {
            return Ok(RoutedFamily::Gridfm);
        }
        return Ok(RoutedFamily::Balanced(None));
    }
    let extension = std::path::Path::new(source.name())
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match extension.as_str() {
        "dss" => Ok(RoutedFamily::Distribution),
        "json" => json_family(source),
        // Extensions with dedicated non-JSON readers keep them; anything
        // else (a nameless in-memory source above all) can still carry a
        // JSON document, so content that opens one routes by classification,
        // mirroring the balanced hub's own sniff.
        "m" | "raw" | "aux" | "epc" | "pwb" | "pwd" => Ok(RoutedFamily::Balanced(None)),
        _ => {
            let jsonish = source.primary_buffer().is_ok_and(|buffer| {
                std::str::from_utf8(buffer.content_bytes()).is_ok_and(|text| {
                    // Strip a UTF-8 BOM the way the JSON classifier does, so
                    // a BOM-prefixed nameless document routes the same as
                    // the identical content saved with a .json name.
                    text.trim_start_matches('\u{feff}')
                        .trim_start()
                        .starts_with(['{', '['])
                })
            });
            if jsonish {
                json_family(source)
            } else {
                Ok(RoutedFamily::Balanced(None))
            }
        }
    }
}

/// The family a JSON document's content markers select.
fn json_family(
    source: &powerio_core::Source,
) -> std::result::Result<RoutedFamily, powerio_core::Error> {
    use format::routing::{Detection, JsonClass, SourceFormat, TransmissionFormat};

    let buffer = source.primary_buffer()?;
    // Family routing needs decoded text; a non-UTF-8 `.json` fails in
    // the balanced hub with its own wording. Classification never ran, so
    // the balanced hub gets no hint and classifies it itself.
    let Ok(text) = std::str::from_utf8(buffer.content_bytes()) else {
        return Ok(RoutedFamily::Balanced(None));
    };
    let class = format::routing::classify_json_text(text);
    match class {
        JsonClass::Case(Detection::Known(SourceFormat::Transmission(
            TransmissionFormat::Goc3Json,
        ))) => Ok(RoutedFamily::Goc3),
        JsonClass::Case(Detection::Known(SourceFormat::Transmission(
            TransmissionFormat::DeepMindOpfDataJson,
        ))) => Ok(RoutedFamily::OpfData),
        JsonClass::Case(Detection::Known(SourceFormat::Transmission(
            TransmissionFormat::EgretJson,
        ))) => Ok(RoutedFamily::Egret),
        JsonClass::Case(Detection::Known(SourceFormat::Distribution(dist_format))) => {
            Ok(match dist_format {
                format::routing::DistributionFormat::BmopfJson => RoutedFamily::Bmopf,
                _ => RoutedFamily::Distribution,
            })
        }
        JsonClass::Module => Ok(RoutedFamily::Stored),
        // The balanced hub's own JSON detection carries the refusal
        // wording for unrecognized or ambiguous documents, and decodes bare
        // model JSON itself; it gets this classification so it never
        // re-derives it from the same bytes.
        JsonClass::Case(Detection::Known(_) | Detection::Ambiguous | Detection::Unknown)
        | JsonClass::ModelJson => Ok(RoutedFamily::Balanced(Some(class))),
    }
}

/// The family a declared format token selects. Unknown tokens fall to the
/// balanced hub, which owns the refusal wording and the accepted name list.
fn family_of_token(token: &str) -> RoutedFamily {
    use format::TargetFormat;
    use powerio_dist::DistTargetFormat;

    if let Some(dist_format) = powerio_dist::dist_target_from_name(token) {
        return match dist_format {
            DistTargetFormat::BmopfJson => RoutedFamily::Bmopf,
            _ => RoutedFamily::Distribution,
        };
    }
    if format::is_pypsa_csv_name(token) {
        return RoutedFamily::PypsaDirectory;
    }
    #[cfg(feature = "gridfm")]
    if token.eq_ignore_ascii_case("gridfm") {
        return RoutedFamily::Gridfm;
    }
    match format::target_format_from_name(token) {
        Some(TargetFormat::Goc3Json) => RoutedFamily::Goc3,
        Some(TargetFormat::DeepMindOpfDataJson) => RoutedFamily::OpfData,
        Some(TargetFormat::EgretJson) => RoutedFamily::Egret,
        _ => RoutedFamily::Balanced(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn memory(name: &str, text: &str) -> powerio_core::Source {
        powerio_core::Source::from_bytes(name, text.as_bytes().to_vec()).expect("memory source")
    }

    #[test]
    fn a_matpower_source_parses_to_the_balanced_kind() {
        let case = "function mpc = case\n\
                    mpc.version = '2';\n\
                    mpc.baseMVA = 100;\n\
                    mpc.bus = [1 3 0 0 0 0 1 1 0 230 1 1.1 0.9;];\n\
                    mpc.gen = [1 0 0 10 -10 1 100 1 10 0;];\n\
                    mpc.branch = [];\n";
        let module = parse(
            memory("case.m", case).with_format(powerio_core::FormatId::new("matpower").unwrap()),
        )
        .expect("matpower parses");
        assert_eq!(module.value().kind(), PioValueKind::BalancedNetwork);
    }

    #[test]
    fn a_dss_source_parses_to_the_multiconductor_kind() {
        let module = parse(memory(
            "feeder.dss",
            "New Circuit.c basekv=12.47 bus1=src\n",
        ))
        .expect("dss parses");
        assert_eq!(module.value().kind(), PioValueKind::MulticonductorNetwork);
        let typed: powerio_core::PioModule<powerio_dist::MulticonductorNetwork> =
            try_into_typed(module).expect("narrows to the parsed kind");
        assert_eq!(typed.value().name().as_deref(), Some("c"));
    }

    #[test]
    fn a_declared_distribution_format_routes_without_an_extension() {
        let module = parse(
            memory("<memory>", "New Circuit.c basekv=12.47 bus1=src\n")
                .with_format(powerio_core::FormatId::new("dss").unwrap()),
        )
        .expect("declared dss parses");
        assert_eq!(module.value().kind(), PioValueKind::MulticonductorNetwork);
    }

    #[test]
    fn json_routes_by_top_level_markers() {
        // A PMD document carries `data_model`, which no balanced format does.
        let module = parse(memory(
            "feeder.json",
            r#"{"data_model": "ENGINEERING", "bus": {}}"#,
        ))
        .expect("pmd parses");
        assert_eq!(module.value().kind(), PioValueKind::MulticonductorNetwork);
    }

    #[test]
    fn bare_model_json_parses_to_the_balanced_kind() {
        use powerio_tx::{Bus, BusId, BusType};
        let network = powerio_tx::BalancedNetwork::in_memory(
            "transport",
            100.0,
            vec![Bus::new(BusId(1), BusType::Ref, 230.0)],
            vec![],
        );
        let json = network.to_json().expect("network serializes");
        let module = parse(memory("net.json", &json)).expect("model json parses");
        assert_eq!(module.value().kind(), PioValueKind::BalancedNetwork);
    }

    #[test]
    fn the_error_path_retains_the_source() {
        let error = parse(memory("case.m", "not matpower at all")).expect_err("malformed");
        assert!(error.retained_source().is_some());
    }

    fn fixture(path: &str) -> String {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        std::fs::read_to_string(root.join(path)).unwrap()
    }

    #[test]
    fn goc3_parses_to_the_scuc_instance_kind() {
        // A calculation defining source produces its calculation's value,
        // never the bare network its data could also build.
        let text = fixture("../powerio-prob/tests/data/goc3_small.json");
        let module = parse(memory("goc3_small.json", &text)).expect("goc3 parses");
        assert_eq!(module.value().kind(), PioValueKind::AcScucInstance);
        assert!(module.source().is_some());
        let typed: powerio_core::PioModule<powerio_prob::AcScucInstance> =
            try_into_typed(module).expect("narrows to the parsed kind");
        assert_eq!(typed.value().network().buses().len(), 2);
    }

    #[test]
    fn opfdata_parses_to_the_solved_ac_opf_kind() {
        let text = fixture("../tests/data/opfdataset/example_0.json");
        let module = parse(memory("example_0.json", &text)).expect("opfdata parses");
        assert_eq!(module.value().kind(), PioValueKind::AcOpfSolution);
        let typed: powerio_core::PioModule<powerio_prob::AcOpfSolution> =
            try_into_typed(module).expect("narrows to the parsed kind");
        assert_eq!(typed.value().instance().network().buses().len(), 14);
    }

    const BMOPF_TINY: &str = r#"{
      "bus": {"a": {"terminal_names": ["1", "2", "3", "n"],
        "perfectly_grounded_terminals": ["n"]}},
      "voltage_source": {"s": {"bus": "a", "terminal_map": ["1", "2", "3"],
        "v_magnitude": [240.0, 240.0, 240.0], "v_angle": [0.0, -2.0944, 2.0944]}}
    }"#;

    #[test]
    fn bmopf_parses_to_the_multiconductor_opf_instance_kind() {
        // BMOPF defines an optimization calculation, so it routes to the
        // instance rather than to the bare multiconductor network, both
        // sniffed and declared.
        let module = parse(memory("feeder.json", BMOPF_TINY)).expect("sniffed bmopf parses");
        assert_eq!(module.value().kind(), PioValueKind::McAcOpfInstance);

        let module = parse(
            memory("<memory>", BMOPF_TINY)
                .with_format(powerio_core::FormatId::new("bmopf-json").unwrap()),
        )
        .expect("declared bmopf parses");
        assert_eq!(module.value().kind(), PioValueKind::McAcOpfInstance);
    }

    #[test]
    fn nameless_json_text_routes_by_content() {
        use powerio_tx::{Bus, BusId, BusType};
        // An in-memory source has no extension to state, so the family comes
        // from the document's own markers: a calculation defining format, a
        // distribution format, and bare model JSON all dispatch the same way
        // they would from a `.json` file.
        let goc3 = fixture("../powerio-prob/tests/data/goc3_small.json");
        let module = parse(memory("<memory>", &goc3)).expect("nameless goc3 parses");
        assert_eq!(module.value().kind(), PioValueKind::AcScucInstance);

        let module = parse(memory("<memory>", BMOPF_TINY)).expect("nameless bmopf parses");
        assert_eq!(module.value().kind(), PioValueKind::McAcOpfInstance);

        let network = powerio_tx::BalancedNetwork::in_memory(
            "transport",
            100.0,
            vec![Bus::new(BusId(1), BusType::Ref, 230.0)],
            vec![],
        );
        let json = network.to_json().expect("network serializes");
        let module = parse(memory("<memory>", &json)).expect("nameless model json parses");
        assert_eq!(module.value().kind(), PioValueKind::BalancedNetwork);
    }

    #[test]
    fn a_declared_problem_format_that_fails_retains_the_source() {
        let error = parse(
            memory("broken.json", "{\"network\": {}}")
                .with_format(powerio_core::FormatId::new("goc3-json").unwrap()),
        )
        .expect_err("malformed goc3");
        assert!(error.retained_source().is_some());
    }

    const PYPSA_STATIC: [(&str, &str); 4] = [
        ("network.csv", "name\nseq\n"),
        ("buses.csv", "name,v_nom\nB1,138.0\nB2,138.0\n"),
        ("loads.csv", "name,bus,p_set,q_set\nL1,B2,5.0,1.0\n"),
        (
            "generators.csv",
            "name,bus,control,p_nom,p_set\nG1,B1,Slack,100.0,12.0\n",
        ),
    ];

    fn pypsa_folder(extra: &[(&str, &str)]) -> tempfile::TempDir {
        let temp = tempfile::tempdir().unwrap();
        for (name, content) in PYPSA_STATIC.iter().chain(extra) {
            std::fs::write(temp.path().join(name), content).unwrap();
        }
        temp
    }

    #[test]
    fn a_pypsa_snapshot_parses_to_the_balanced_kind() {
        let dir = pypsa_folder(&[("snapshots.csv", ",snapshot\n0,now\n")]);
        let module =
            parse(powerio_core::Source::open(dir.path()).unwrap()).expect("snapshot parses");
        assert_eq!(module.value().kind(), PioValueKind::BalancedNetwork);
    }

    #[test]
    fn a_pypsa_input_series_parses_to_the_network_time_series_kind() {
        let dir = pypsa_folder(&[
            ("snapshots.csv", ",snapshot\n0,now\n1,later\n"),
            ("loads-p_set.csv", "snapshot,L1\nnow,10.0\nlater,20.0\n"),
        ]);
        let module = parse(powerio_core::Source::open(dir.path()).unwrap()).expect("series parses");
        assert_eq!(
            module.value().kind(),
            PioValueKind::BalancedNetworkTimeSeries
        );
        assert!(module.source().is_some());
    }

    #[test]
    fn a_pypsa_state_only_series_parses_to_the_operating_point_kind() {
        let dir = pypsa_folder(&[
            ("snapshots.csv", ",snapshot\n0,now\n1,later\n"),
            (
                "buses-v_mag_pu.csv",
                "snapshot,B1,B2\nnow,1.0,0.99\nlater,1.0,0.97\n",
            ),
            (
                "buses-v_ang.csv",
                "snapshot,B1,B2\nnow,0.0,-0.017453292519943295\nlater,0.0,-0.03490658503988659\n",
            ),
        ]);
        let module =
            parse(powerio_core::Source::open(dir.path()).unwrap()).expect("state series parses");
        assert_eq!(
            module.value().kind(),
            PioValueKind::BalancedOperatingPointTimeSeries
        );
    }

    #[test]
    fn a_pypsa_axis_with_no_series_stays_a_network_time_series() {
        // Several declared snapshots and nothing varying preserve the axis
        // as networks sharing every table; nothing here is an operating
        // point series.
        let dir = pypsa_folder(&[("snapshots.csv", ",snapshot\n0,now\n1,later\n")]);
        let module = parse(powerio_core::Source::open(dir.path()).unwrap()).expect("axis parses");
        assert_eq!(
            module.value().kind(),
            PioValueKind::BalancedNetworkTimeSeries
        );
    }

    const EGRET_SERIES: &str = r#"{
        "model_name": "uc2",
        "elements": {
            "bus": {"1": {"matpower_bustype": "ref", "base_kv": 138.0},
                    "2": {"matpower_bustype": "PQ", "base_kv": 138.0}},
            "load": {"load_1": {"bus": "2",
                "p_load": {"data_type": "time_series", "values": [10.0, 20.0]},
                "q_load": 3.0}},
            "generator": {"1": {"bus": "1", "pg": 12.0, "qg": 0.0,
                "p_min": 0.0, "p_max": 50.0, "q_min": -10.0, "q_max": 10.0}},
            "branch": {"1": {"from_bus": "1", "to_bus": "2",
                "resistance": 0.01, "reactance": 0.1, "charging_susceptance": 0.0,
                "rating_long_term": 100.0, "rating_short_term": 100.0,
                "rating_emergency": 100.0, "transformer_phase_shift": 0.0}}
        },
        "system": {"baseMVA": 100.0, "time_keys": ["t1", "t2"]}
    }"#;

    #[test]
    fn egret_time_keys_parse_to_the_network_time_series_kind() {
        let module = parse(
            memory("uc2.json", EGRET_SERIES)
                .with_format(powerio_core::FormatId::new("egret-json").unwrap()),
        )
        .expect("egret series parses");
        assert_eq!(
            module.value().kind(),
            PioValueKind::BalancedNetworkTimeSeries
        );
        assert!(module.source().is_some());

        // The sniffed route agrees with the declared one.
        let module = parse(memory("uc2.json", EGRET_SERIES)).expect("sniffed egret parses");
        assert_eq!(
            module.value().kind(),
            PioValueKind::BalancedNetworkTimeSeries
        );
    }

    #[cfg(feature = "gridfm")]
    #[test]
    fn a_stored_module_parses_back_through_the_universal_parse() {
        use powerio_tx::{Bus, BusId, BusType};
        let network = powerio_tx::BalancedNetwork::in_memory(
            "stored",
            100.0,
            vec![Bus::new(BusId(1), BusType::Ref, 230.0)],
            vec![],
        );
        let text = stored::write_module(&powerio_core::PioModule::new(PioValue::BalancedNetwork(
            network,
        )))
        .expect("module writes");
        let module = parse(memory("case.pio.json", &text)).expect("stored module parses");
        assert_eq!(module.value().kind(), PioValueKind::BalancedNetwork);
        // The `.pio.json` file is the loaded module's runtime source.
        assert!(module.source().is_some());
    }

    #[cfg(feature = "gridfm")]
    #[test]
    fn a_gridfm_dataset_parses_to_the_scenario_set_kind() {
        // Write a two scenario dataset with the matrix writer, then parse the
        // directory through the universal parse.
        let case = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/data/case9.m");
        let base = powerio_tx::parse(powerio_core::Source::open(case).unwrap())
            .expect("case9 parses")
            .into_value();
        let mut varied = base.clone();
        varied.loads_mut()[0].p += 5.0;
        let out = tempfile::tempdir().unwrap();
        let snapshots = [
            powerio_matrix::GridfmSnapshot::new(&base, 0),
            powerio_matrix::GridfmSnapshot::new(&varied, 1),
        ];
        powerio_matrix::write_gridfm_batch(
            &snapshots,
            out.path(),
            &powerio_matrix::GridfmOptions::default(),
        )
        .expect("dataset writes");

        let module =
            parse(powerio_core::Source::open(out.path()).unwrap()).expect("dataset parses");
        assert_eq!(
            module.value().kind(),
            PioValueKind::BalancedNetworkScenarioSet
        );
        assert!(module.source().is_some());
        let typed: powerio_core::PioModule<powerio_core::ScenarioSet<powerio_tx::BalancedNetwork>> =
            try_into_typed(module).expect("narrows to the parsed kind");
        let set = typed.value();
        assert_eq!(set.len(), 2);
        assert!(set.get("0").is_some());
        assert!(set.get("1").is_some());
    }

    #[test]
    fn an_unrecognized_directory_is_refused_with_the_hub_wording() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("notes.txt"), "not a case").unwrap();
        let error =
            parse(powerio_core::Source::open(dir.path()).unwrap()).expect_err("refused directory");
        assert!(error.to_string().contains("directory"), "{error}");
    }

    #[test]
    fn a_scalar_egret_document_stays_a_balanced_network() {
        let scalar = EGRET_SERIES
            .replace(r#", "time_keys": ["t1", "t2"]"#, "")
            .replace(
                r#"{"data_type": "time_series", "values": [10.0, 20.0]}"#,
                "10.0",
            );
        let module = parse(
            memory("uc2.json", &scalar)
                .with_format(powerio_core::FormatId::new("egret-json").unwrap()),
        )
        .expect("scalar egret parses");
        assert_eq!(module.value().kind(), PioValueKind::BalancedNetwork);
    }
}
