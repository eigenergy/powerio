//! Cross format conversion output.

/// Text in the target format plus every fidelity loss the writer took.
/// Nothing drops silently: a field the target cannot represent appears
/// here as a warning naming the element and field.
#[derive(Debug, Clone)]
pub struct Conversion {
    pub text: String,
    pub warnings: Vec<String>,
}
