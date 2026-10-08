//! Validation report for explicit GLM objects; not electrical conversion.
use powerio_core::Source;
use powerio_dist::glm::{GlmDialect, ObjectKey, PreprocessOptions, parse_project};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let input = args
        .next()
        .ok_or("usage: glm_objects FILE [pnnl|arras] [ACQUISITION_ROOT]")?;
    let dialect = match args.next().as_deref() {
        None | Some("pnnl") => GlmDialect::Pnnl5_3,
        Some("arras") => GlmDialect::Arras4_3,
        _ => return Err("expected pnnl or arras dialect".into()),
    };
    let mut source = Source::open(input)?;
    if let Some(root) = args.next() {
        source = source.with_acquisition_root(root)?;
    }
    if args.next().is_some() {
        return Err("too many arguments".into());
    }
    let project = parse_project(
        source,
        PreprocessOptions {
            dialect,
            ..Default::default()
        },
    )?;
    let index = project.index_objects()?;
    let mut objects = Vec::new();
    for object in index.objects() {
        let mut references = serde_json::Map::new();
        for property in ["from", "to", "configuration"] {
            if object.properties().iter().any(|p| p.name == property) {
                references.insert(
                    property.to_owned(),
                    serde_json::json!(
                        index
                            .resolve_property(object.key(), property)?
                            .map(ObjectKey::ordinal)
                    ),
                );
            }
        }
        objects.push(serde_json::json!({
            "key": object.key().ordinal(), "class": object.class_name(), "name": object.name(),
            "explicit_id": object.explicit_id(), "parent": index.parent(object.key()).map(ObjectKey::ordinal),
            "references": references,
        }));
    }
    println!("{}", serde_json::to_string_pretty(&objects)?);
    Ok(())
}
