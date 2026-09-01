//! Regression crate for the facade's completeness: this package depends on
//! `powerio` alone, never `powerio-core`, `powerio-dist`, or `powerio-prob`.
//! If a future edit stops re-exporting a type the facade promises, this
//! fails to compile with an unresolved name here rather than a downstream
//! consumer finding out first.

/// Every ontology type the facade re-exports must be nameable through a
/// `powerio::` path alone. A type alias is enough: it only needs the name to
/// resolve, not a value to construct.
///
#[allow(dead_code)]
mod names_resolve_through_the_facade {
    type _Source = powerio::Source;
    type _PioModule = powerio::PioModule<powerio::BalancedNetwork>;
    type _Error = powerio::Error;
    type _Destination = powerio::Destination;
    type _FormatInfo = powerio::FormatInfo;
    type _TimeSeries = powerio::TimeSeries<f64>;
    type _ScenarioSet = powerio::ScenarioSet<f64>;
    type _TimePoint = powerio::TimePoint;
    type _SourceSpan = powerio::SourceSpan;
    type _HistoryEntry = powerio::HistoryEntry;
    type _HistoryId = powerio::HistoryId;
    type _HistoryKind = powerio::HistoryKind;
    type _OperatingPoint = powerio::OperatingPoint<powerio::BalancedNetwork>;
    type _AcPfInstance = powerio::AcPfInstance;
    type _AcPfSolution = powerio::AcPfSolution;
    type _AcOpfInstance = powerio::AcOpfInstance;
    type _AcOpfSolution = powerio::AcOpfSolution;
    type _DcOpfInstance = powerio::DcOpfInstance;
    type _DcOpfSolution = powerio::DcOpfSolution;
    type _MulticonductorNetwork = powerio::MulticonductorNetwork;
    type _BalancedNetwork = powerio::BalancedNetwork;
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_readme_example_resolves_through_the_facade_alone() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../data/case9.m");
        let module = powerio::parse(powerio::Source::open(&path).unwrap(), None).unwrap();
        let powerio::PioValue::BalancedNetwork(network) = &module.value else {
            panic!(
                "expected a balanced network, found {}",
                module.value.type_name()
            );
        };
        assert!(!network.buses().is_empty());

        let serialized =
            powerio::serialize(&module, powerio::Destination::memory("module").unwrap()).unwrap();
        let powerio::EmittedOutput::Memory { artifacts } = serialized.into_output() else {
            panic!("a memory destination returned path output");
        };
        let bytes = artifacts.into_iter().next().unwrap().into_bytes();
        let decoded =
            powerio::deserialize(powerio::Source::from_memory("module.pio.json", bytes).unwrap())
                .unwrap();
        assert!(matches!(
            &decoded.value,
            powerio::PioValue::BalancedNetwork(_)
        ));
    }

    /// `powerio::Error` must name the type `powerio::parse` actually
    /// returns, not powerio-tx's own error enum that `powerio_tx::*` also
    /// glob re-exports under the same name.
    #[test]
    fn powerio_error_is_the_type_parse_returns() {
        let bad = powerio::Source::from_memory("<memory>", b"not a case file".to_vec()).unwrap();
        let _: powerio::Error = powerio::parse(bad, None).unwrap_err();
    }

    #[test]
    fn facade_metadata_and_display_operations_keep_facade_types() {
        assert!(
            powerio::resolve_format("pio-json").is_none(),
            "PowerIO IR is serialized, not a grid exchange format"
        );

        let source =
            powerio::Source::from_memory("display.pwd", b"not a display".to_vec()).unwrap();
        let _: powerio::Error = powerio::parse_display(source, None).unwrap_err();
        let _: powerio::Error = powerio::to_geo_layer_from_aux_text(
            "DATA (Substation, [SubNum, Latitude, Longitude])\n{\n7 34 -80 99\n}\n",
        )
        .unwrap_err();
    }
}
