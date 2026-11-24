//! PowerIO IR serialization and deserialization.

use powerio_core::__implementation::ElementMapBuilder;
use powerio_core::{
    ArtifactPath, Diagnostic, ElementMap, EmitResult, Error, Fidelity, MemoryArtifact, PioModule,
    RowRef,
};

use crate::PioValue;

/// Serialize a diagnostics list as the JSON array of PowerIO IR diagnostic
/// records: the encoding a module's `diagnostics` field carries, with an
/// identity minted for every record that has none.
///
/// # Errors
/// The records cannot be encoded as JSON.
pub fn serialize_diagnostics(diagnostics: &[Diagnostic]) -> Result<String, Error> {
    let records = crate::stored::encode_diagnostics(diagnostics);
    serde_json::to_string(&records).map_err(|cause| {
        Error::new(
            &crate::codes::READ_MODULE_INVALID,
            format!("diagnostics could not be encoded as JSON: {cause}"),
        )
        .with_cause(cause)
    })
}

/// Generate the JSON Schema for this build's PowerIO IR document.
#[cfg(feature = "schema")]
#[must_use]
pub fn generate_ir_schema() -> schemars::Schema {
    let mut schema = schemars::schema_for!(crate::stored::StoredModule);
    crate::schema_units::annotate(&mut schema);
    schema
}

/// Serialize a dynamic module as PowerIO IR.
///
/// PowerIO IR is the durable representation of a complete [`PioModule`]. It
/// is separate from the grid exchange formats handled by [`crate::emit`].
///
/// For a balanced network the result carries the identity element map
/// ([`EmitResult::element_map`]): each bus, branch, and generator maps to its
/// own position in the IR network's `buses`, `branches`, and `generators`
/// tables, so a caller reads
/// IR and grid exchange output through one code path.
///
/// # Errors
/// The module cannot be represented by the current IR schema, or the
/// destination refuses the artifact.
pub fn serialize<T>(
    module: &PioModule<T>,
    output: impl powerio_core::IntoDestination,
) -> Result<EmitResult, Error>
where
    T: Clone + Into<PioValue>,
{
    let module = module.clone().map_value(Into::into);
    let text = crate::stored::emit_module(&module)?;
    let (map, diagnostics) = match module.value() {
        PioValue::BalancedNetwork(network) => identity_element_map(network)?,
        _ => (None, Vec::new()),
    };
    let artifact = MemoryArtifact::new(ArtifactPath::new("module.pio.json")?, text.into_bytes());
    let result = output.into_destination()?.__commit_artifacts(
        false,
        Fidelity::Canonical,
        vec![artifact],
        diagnostics,
    )?;
    Ok(match map {
        Some(map) => result.__with_element_map(map),
        None => result,
    })
}

/// The element map of a balanced network written as IR: every element at its
/// own position, keyed on the uid the stored form carries.
fn identity_element_map(
    network: &crate::BalancedNetwork,
) -> Result<(Option<ElementMap>, Vec<Diagnostic>), Error> {
    let mut network = network.clone();
    network.assign_missing_component_ids();
    let uid = |uid: &Option<String>| uid.clone().unwrap_or_default();
    let mut map = ElementMapBuilder::new(
        crate::IR_SCHEMA_NAME,
        network.buses().iter().map(|bus| uid(&bus.uid)),
        network.branches().iter().map(|branch| uid(&branch.uid)),
        network
            .generators()
            .iter()
            .map(|generator| uid(&generator.uid)),
    );
    for index in 0..network.buses().len() {
        map.bus(index, RowRef::new("buses", index));
    }
    for index in 0..network.branches().len() {
        map.branch(index, RowRef::new("branches", index), false);
    }
    for index in 0..network.generators().len() {
        map.generator(index, RowRef::new("generators", index));
    }
    map.finish()
}

/// Deserialize one PowerIO IR input: a file name, content in memory, or a
/// [`powerio_core::Source`].
///
/// # Errors
/// The source is not one UTF-8 IR document, or its schema is unsupported or
/// invalid.
pub fn deserialize(input: impl powerio_core::IntoSource) -> Result<PioModule<PioValue>, Error> {
    let source = input.into_source()?;
    let buffer = source.primary_buffer()?;
    let text = std::str::from_utf8(buffer.content_bytes()).map_err(|cause| {
        Error::new(
            &crate::codes::READ_MODULE_INVALID,
            format!("PowerIO IR is not valid UTF-8: {cause}"),
        )
        .with_cause(cause)
        .with_source(source.clone())
    })?;
    crate::stored::read_module(text)
        .map(|module| module.with_source(source.clone()))
        .map_err(|error| error.with_source(source))
}
