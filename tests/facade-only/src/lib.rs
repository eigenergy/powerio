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
    /// A file name and content in memory reach `parse`, and a file name
    /// reaches `emit`, without naming `Source` or `Destination`.
    #[test]
    fn the_common_operations_need_no_input_or_output_type() {
        let data = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../data");
        let path = data.join("case9.m");
        let from_name = powerio::parse(&path).unwrap();

        // A document that identifies itself parses from content alone.
        let self_identifying = std::fs::read(data.join("egret/case9.json")).unwrap();
        assert_eq!(
            powerio::parse(self_identifying).unwrap().value.type_name(),
            from_name.value.type_name()
        );

        // A format read from a file extension needs the format declared,
        // since content in memory carries no extension.
        let content = std::fs::read(&path).unwrap();
        let declared = powerio::parse_with_options(
            content,
            &powerio::ParseOptions::default().format("matpower").unwrap(),
        )
        .unwrap();
        assert_eq!(declared.value.type_name(), from_name.value.type_name());

        let out = std::env::temp_dir().join(format!(
            "powerio-facade-{}.m",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        powerio::emit(&from_name, "matpower", &out).unwrap();
        assert!(out.is_file());
        let _ = std::fs::remove_file(&out);
    }

    #[test]
    fn the_readme_example_resolves_through_the_facade_alone() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../data/case9.m");
        let module = powerio::parse(&path).unwrap();
        let powerio::PioValue::BalancedNetwork(network) = &module.value else {
            panic!(
                "expected a balanced network, found {}",
                module.value().type_name()
            );
        };
        assert!(!network.buses().is_empty());

        let serialized =
            powerio::serialize(&module, powerio::Destination::memory("module").unwrap()).unwrap();
        let powerio::EmittedOutput::Memory { artifacts } = serialized.into_output() else {
            panic!("a memory destination returned path output");
        };
        let bytes = artifacts.into_iter().next().unwrap().into_bytes();
        let decoded = powerio::deserialize(bytes).unwrap();
        assert!(matches!(
            &decoded.value(),
            powerio::PioValue::BalancedNetwork(_)
        ));
    }

    /// `powerio::Error` must name the type `powerio::parse` actually
    /// returns, not powerio-tx's own error enum that `powerio_tx::*` also
    /// glob re-exports under the same name.
    #[test]
    fn powerio_error_is_the_type_parse_returns() {
        let _: powerio::Error = powerio::parse(b"not a case file").unwrap_err();
        let _: powerio::Error = powerio::parse("no-such-case.m").unwrap_err();
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
