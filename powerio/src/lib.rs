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
//! let module = powerio::parse(powerio_core::Source::open("case9.m")?)?;
//! let module: powerio_core::PioModule<powerio::BalancedNetwork> =
//!     powerio::try_into_typed(module)?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! The family entries stay available for callers that already know the
//! family: [`format::parse`] for balanced network formats and
//! [`powerio_dist::parse`] for multiconductor ones.

pub use powerio_tx::*;

pub mod package;
mod value;
pub use value::{FromPioValue, PioValue, PioValueKind, ValueKindMismatch, try_into_typed};

/// Parse one source into a compiled module of whichever built in family
/// claims it: balanced network formats produce
/// [`PioValue::BalancedNetwork`], and distribution formats (OpenDSS `.dss`,
/// PMD ENGINEERING JSON, BMOPF JSON) produce
/// [`PioValue::MulticonductorNetwork`]. The reader's findings are the
/// module's diagnostics and the source is retained for the byte exact echo
/// tier of the family's `write_as`.
///
/// The family comes from the source's declared format when one was selected,
/// and otherwise from the name and content: a `.dss` extension routes to the
/// distribution reader, a `.json` document routes by its top level markers
/// ([`format::routing::classify_json_text`]), and every other name routes to
/// the balanced network hub, whose own detection and refusals apply.
///
/// Bare model JSON, the network serialization, decodes to
/// [`PioValue::BalancedNetwork`] like any other balanced source.
///
/// # Errors
/// The routed family's failure, carrying its findings and the retained
/// source; a `.pio.json` package is refused with the surface that reads it
/// named.
pub fn parse(
    source: powerio_core::Source,
) -> std::result::Result<powerio_core::PioModule<PioValue>, powerio_core::Error> {
    if family_is_distribution(&source)? {
        return powerio_dist::parse(source).map(|module| module.map_value(PioValue::from));
    }
    format::parse(source).map(|module| module.map_value(PioValue::from))
}

/// Whether the source routes to the distribution family. The balanced hub is
/// the default: it owns the guidance for unknown names and refused shapes.
fn family_is_distribution(
    source: &powerio_core::Source,
) -> std::result::Result<bool, powerio_core::Error> {
    use format::routing::{Detection, Domain, JsonClass};

    if let Some(declared) = source.format() {
        return Ok(powerio_dist::dist_target_from_name(declared.as_str()).is_some());
    }
    if source.is_directory() {
        // The only directory case format is a PyPSA CSV folder; the balanced
        // hub owns that dispatch and the refusal for anything else.
        return Ok(false);
    }
    let extension = std::path::Path::new(source.name())
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match extension.as_str() {
        "dss" => Ok(true),
        "json" => {
            let buffer = source.primary_buffer()?;
            // Family routing needs decoded text; a non-UTF-8 `.json` fails in
            // the balanced hub with its own wording.
            let Ok(text) = std::str::from_utf8(buffer.content_bytes()) else {
                return Ok(false);
            };
            match format::routing::classify_json_text(text) {
                JsonClass::Case(Detection::Known(json_format)) => {
                    Ok(json_format.domain() == Domain::Distribution)
                }
                // The balanced hub's own JSON detection carries the refusal
                // wording for packages, model JSON, and unrecognized or
                // ambiguous documents.
                JsonClass::Package
                | JsonClass::ModelJson
                | JsonClass::Case(Detection::Ambiguous | Detection::Unknown) => Ok(false),
            }
        }
        _ => Ok(false),
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
}
