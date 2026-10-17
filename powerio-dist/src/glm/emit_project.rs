use std::collections::BTreeSet;
use std::path::Path;

use powerio_core::{ArtifactPath, Diagnostic, Error, MemoryArtifact, PioModule};

use crate::MulticonductorNetwork;
use crate::diagnostics::codes;

pub(crate) struct RetainedProject {
    pub artifacts: Vec<MemoryArtifact>,
    pub diagnostics: Vec<Diagnostic>,
}

/// Replay only retained bytes. Never reacquire a dependency from the filesystem
/// or rerun preprocessing with an implicit environment at emission time.
pub(crate) fn retained_project(
    module: &PioModule<MulticonductorNetwork>,
    format: crate::DistTargetFormat,
) -> Result<Option<RetainedProject>, Error> {
    let Some(source) = module.source() else {
        return Ok(None);
    };
    if source
        .format()
        .and_then(|id| crate::parse_dist_target_format(id.as_str()))
        != Some(format)
    {
        return Ok(None);
    }
    let context = module.extensions().get("org.powerio.gridlabd").ok_or_else(|| invalid("retained GLM bytes have no validated preprocessing context; emit the typed value canonically"))?;
    if context.get("version").and_then(serde_json::Value::as_u64) != Some(1)
        || context.get("dialect").and_then(serde_json::Value::as_str) != Some(format.name())
    {
        return Err(invalid(
            "GLM replay context does not match the retained source profile",
        ));
    }
    match context
        .get("absolute_include")
        .and_then(serde_json::Value::as_bool)
    {
        Some(false) => {}
        Some(true) => {
            return Err(invalid(
                "an absolute include cannot be relocated without changing source bytes; emit the typed value canonically",
            ));
        }
        None => {
            return Err(invalid(
                "GLM replay context lacks its include portability record",
            ));
        }
    }
    let primary = source.primary_buffer()?;
    let file_name = Path::new(primary.name())
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| invalid("the primary source has no portable file name"))?;
    let mut primary_parts: Vec<_> = primary.directory_segments().collect();
    primary_parts.push(file_name);
    let primary_name = primary_parts.join("/");
    let mut artifacts = Vec::new();
    let mut identities = BTreeSet::new();
    for buffer in source.acquired_buffers() {
        let name = if buffer.id() == primary.id() {
            &primary_name
        } else {
            buffer.name()
        };
        let name = ArtifactPath::new(name)?;
        if !identities.insert(name.as_str().to_ascii_lowercase()) {
            return Err(invalid(
                "retained source names collide in a portable output directory",
            ));
        }
        artifacts.push(MemoryArtifact::new(name, buffer.bytes().to_vec()));
    }
    let mut diagnostics = Vec::new();
    let preprocessing = context
        .get("preprocessing")
        .ok_or_else(|| invalid("GLM replay context lacks preprocessing inputs"))?;
    let defines = preprocessing
        .get("defines")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| invalid("GLM replay definitions are malformed"))?;
    let reinclude = preprocessing
        .get("reinclude")
        .and_then(serde_json::Value::as_bool)
        .ok_or_else(|| invalid("GLM replay reinclude flag is malformed"))?;
    if !defines.is_empty() || reinclude {
        diagnostics.push(Diagnostic::of(&codes::EMIT_GLM_REPLAY_CONTEXT_REQUIRED,
            "exact source bytes retain external preprocessing dependencies; use the module's org.powerio.gridlabd preprocessing inputs and declared engine profile"));
    }
    Ok(Some(RetainedProject {
        artifacts,
        diagnostics,
    }))
}

fn invalid(message: impl Into<String>) -> Error {
    Error::new(&codes::EMIT_GLM_PROJECT_UNREPRESENTABLE, message)
}
