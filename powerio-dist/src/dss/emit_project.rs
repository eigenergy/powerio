//! Byte-exact replay of an acquired DSS project, including binary profiles.
use crate::diagnostics::codes::{
    EMIT_DSS_PROJECT_COMPANION_KEPT, EMIT_DSS_PROJECT_UNREPRESENTABLE,
};
use crate::{DistSourceFormat, MulticonductorNetwork};
use powerio_core::{ArtifactPath, Diagnostic, Error, MemoryArtifact, PioModule};

/// The replayed files plus a remark naming the companions it kept.
pub(crate) struct RetainedProject {
    pub artifacts: Vec<MemoryArtifact>,
    pub diagnostics: Vec<Diagnostic>,
}

pub(crate) fn retained_project(
    module: &PioModule<MulticonductorNetwork>,
) -> Result<Option<RetainedProject>, Error> {
    let Some(source) = module.source() else {
        return Ok(None);
    };
    if source.format().is_none_or(|f| f.as_str() != "dss")
        || *module.value().source_format() != Some(DistSourceFormat::Dss)
    {
        return Ok(None);
    }
    let buffers = source.acquired_buffers();
    // Preserve the established single-document case.dss destination layout.
    let primary = source.primary_buffer()?;
    if buffers.len() <= 1 && primary.directory_segments().next().is_none() {
        return Ok(None);
    }
    let invalid = |message| Error::new(&EMIT_DSS_PROJECT_UNREPRESENTABLE, message);
    let context = module.extensions().get("org.powerio.opendss")
        .ok_or_else(|| invalid("retained DSS project lacks validated dependency context; emit the typed value canonically"))?;
    if context.get("version").and_then(serde_json::Value::as_u64) != Some(1)
        || context
            .get("absolute_dependency")
            .and_then(serde_json::Value::as_bool)
            != Some(false)
    {
        return Err(invalid(
            "absolute or unqualified DSS dependencies cannot be relocated byte exactly; emit the typed value canonically",
        ));
    }
    let primary = source.primary_buffer()?;
    let file = std::path::Path::new(primary.name())
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| invalid("DSS entrypoint has no portable file name"))?;
    let mut parts = primary.directory_segments().collect::<Vec<_>>();
    parts.push(file);
    let entrypoint = parts.join("/");
    let dependencies = context
        .get("dependencies")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| invalid("retained DSS project lacks its dependency inventory"))?;
    let dependencies = dependencies
        .iter()
        .map(|value| {
            value
                .as_str()
                .ok_or_else(|| invalid("retained DSS dependency inventory is malformed"))
        })
        .collect::<Result<std::collections::BTreeSet<_>, _>>()?;
    // Every acquired buffer replays: the dependency files the reader followed
    // and the companion files beside them (a `Redirect` that a later edit
    // retargets, a data file the deck names by a path the reader never opened).
    // The companions are kept on purpose and named in one remark, so the copy
    // is never silent.
    let mut names = std::collections::BTreeSet::new();
    let mut artifacts = Vec::new();
    let mut companions = Vec::new();
    for buffer in buffers {
        if buffer.id() != primary.id() && !dependencies.contains(buffer.id().as_str()) {
            companions.push(buffer.name().to_owned());
        }
        let name = if buffer.id() == primary.id() {
            entrypoint.as_str()
        } else {
            buffer.name()
        };
        let path = ArtifactPath::new(format!("source/{name}"))?;
        if !names.insert(path.as_str().to_ascii_lowercase()) {
            return Err(invalid(
                "retained DSS file names collide in a portable output directory",
            ));
        }
        artifacts.push(MemoryArtifact::new(path, buffer.bytes().to_vec()));
    }
    let entrypoint = format!("source/{entrypoint}");
    let (token, representable) = if entrypoint.contains('"') {
        super::write::dss_value_out(&entrypoint)
    } else {
        (format!("\"{entrypoint}\""), true)
    };
    if !representable {
        return Err(invalid(
            "DSS entrypoint cannot be represented by a redirect",
        ));
    }
    artifacts.push(MemoryArtifact::new(
        ArtifactPath::new("case.dss")?,
        format!("Redirect {token}\n").into_bytes(),
    ));
    let mut diagnostics = Vec::new();
    if !companions.is_empty() {
        let count = companions.len();
        let noun = if count == 1 { "file" } else { "files" };
        diagnostics.push(Diagnostic::of(
            &EMIT_DSS_PROJECT_COMPANION_KEPT,
            format!(
                "copied {count} supplied {noun} the deck never read under source/: {}",
                companions.join(", ")
            ),
        ));
    }
    Ok(Some(RetainedProject {
        artifacts,
        diagnostics,
    }))
}
