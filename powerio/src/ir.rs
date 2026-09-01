//! PowerIO IR serialization and deserialization.

use powerio_core::{
    ArtifactPath, Destination, EmitResult, Error, Fidelity, MemoryArtifact, PioModule, Source,
};

use crate::PioValue;

/// Generate the JSON Schema for the PowerIO 1.0 IR document.
#[cfg(feature = "schema")]
#[must_use]
pub fn generate_ir_schema() -> schemars::Schema {
    schemars::schema_for!(crate::stored::StoredModule)
}

/// Serialize a dynamic module as PowerIO IR.
///
/// PowerIO IR is the durable representation of a complete [`PioModule`]. It
/// is separate from the grid exchange formats handled by [`crate::emit`].
///
/// # Errors
/// The module cannot be represented by the current IR schema, or the
/// destination refuses the artifact.
pub fn serialize<T>(module: &PioModule<T>, destination: Destination) -> Result<EmitResult, Error>
where
    T: Clone + Into<PioValue>,
{
    let module = module.clone().map_value(Into::into);
    let text = crate::stored::emit_module(&module)?;
    let artifact = MemoryArtifact::new(ArtifactPath::new("module.pio.json")?, text.into_bytes());
    destination.__commit_artifacts(false, Fidelity::Canonical, vec![artifact], Vec::new())
}

/// Deserialize one PowerIO IR source.
///
/// # Errors
/// The source is not one UTF-8 IR document, or its schema is unsupported or
/// invalid.
pub fn deserialize(source: Source) -> Result<PioModule<PioValue>, Error> {
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
