use powerio_dist::MulticonductorNetwork;
pub struct Parsed {
    pub network: MulticonductorNetwork,
}
pub fn parse_str(text: &str, from: &str) -> Result<Parsed, powerio_core::Error> {
    let source = powerio_core::Source::from_memory("test", text.as_bytes().to_vec())?
        .with_format(powerio_core::FormatId::new(from)?);
    powerio_dist::parse(source).map(|m| Parsed {
        network: m.value().clone(),
    })
}

impl std::ops::Deref for Parsed {
    type Target = MulticonductorNetwork;
    fn deref(&self) -> &Self::Target {
        &self.network
    }
}
pub struct TextEmission {
    pub text: String,
    pub diagnostics: Vec<powerio_core::Diagnostic>,
}
pub fn emit_value_text(
    net: &MulticonductorNetwork,
    format: powerio_dist::DistTargetFormat,
) -> TextEmission {
    let result = powerio_dist::emit(
        &powerio_core::PioModule::new(net.clone()),
        format,
        powerio_core::Destination::memory("output").unwrap(),
    )
    .unwrap();
    let diagnostics = result.diagnostics().to_vec();
    let powerio_core::EmittedOutput::Memory { artifacts } = result.into_output() else {
        panic!("expected memory output")
    };
    TextEmission {
        text: String::from_utf8(artifacts.into_iter().next().unwrap().into_bytes()).unwrap(),
        diagnostics,
    }
}
