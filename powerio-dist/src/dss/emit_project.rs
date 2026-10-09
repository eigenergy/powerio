//! Byte-exact replay of an acquired DSS project, including binary profiles.
use crate::diagnostics::codes::EMIT_DSS_PROJECT_UNREPRESENTABLE;
use crate::{DistSourceFormat, MulticonductorNetwork};
use powerio_core::{ArtifactPath, Error, MemoryArtifact, PioModule};

pub(crate) fn retained_project(
    module: &PioModule<MulticonductorNetwork>,
) -> Result<Option<Vec<MemoryArtifact>>, Error> {
    let Some(source) = module.source() else {
        return Ok(None);
    };
    if source.format().is_none_or(|f| f.as_str() != "dss")
        || *module.value().source_format() != Some(DistSourceFormat::Dss)
    {
        return Ok(None);
    }
    let buffers = source.acquired_buffers();
    // Preserve the established single-document case.dss destination contract.
    if buffers.len() <= 1 {
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
    let mut names = std::collections::BTreeSet::new();
    let mut artifacts = Vec::new();
    for buffer in buffers {
        let path = ArtifactPath::new(if buffer.id() == primary.id() {
            &entrypoint
        } else {
            buffer.name()
        })?;
        if !names.insert(path.as_str().to_ascii_lowercase()) {
            return Err(invalid(
                "retained DSS file names collide in a portable output directory",
            ));
        }
        artifacts.push(MemoryArtifact::new(path, buffer.bytes().to_vec()));
    }
    // Keep the established case.dss entrypoint for root-level projects while
    // also retaining the original filename for internal source references.
    if primary.directory_segments().next().is_none() && entrypoint != "case.dss" {
        if !names.insert("case.dss".into()) {
            return Err(invalid(
                "retained dependency collides with the case.dss entrypoint",
            ));
        }
        artifacts.push(MemoryArtifact::new(
            ArtifactPath::new("case.dss")?,
            primary.bytes().to_vec(),
        ));
    }
    Ok(Some(artifacts))
}
