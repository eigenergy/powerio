//! Validation-only quantity schema for the analytic oracle circuit.
//! This reports source constants, not a converted electrical network.
use powerio_core::Source;
use powerio_dist::glm::{GlmDialect, GlmQuantityKind, PreprocessOptions, parse_project};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let input = args
        .next()
        .ok_or("usage: glm_quantities FILE [pnnl|arras]")?;
    let dialect = match args.next().as_deref() {
        None | Some("pnnl") => GlmDialect::Pnnl5_3,
        Some("arras") => GlmDialect::Arras4_3,
        _ => return Err("expected pnnl or arras dialect".into()),
    };
    if args.next().is_some() {
        return Err("too many arguments".into());
    }
    let project = parse_project(
        Source::open(input)?,
        PreprocessOptions {
            dialect,
            ..Default::default()
        },
    )?;
    let index = project.index_objects()?;
    let mut values = serde_json::Map::new();
    for object in index.objects() {
        let Some(name) = object.name() else { continue };
        let class = object.class_name().rsplit('.').next().unwrap_or("");
        for property in object.properties() {
            let spec = match (class, property.name) {
                ("node" | "load", "nominal_voltage") => Some((GlmQuantityKind::Real, "V")),
                ("node" | "load", "voltage_A") => Some((GlmQuantityKind::Complex, "V")),
                ("load", "constant_impedance_A") => Some((GlmQuantityKind::Complex, "Ohm")),
                ("overhead_line", "length") => Some((GlmQuantityKind::Real, "ft")),
                (
                    "line_configuration",
                    "z11" | "z12" | "z13" | "z21" | "z22" | "z23" | "z31" | "z32" | "z33",
                ) => Some((GlmQuantityKind::Complex, "Ohm/mile")),
                ("line_configuration", "c11") => Some((GlmQuantityKind::Real, "nF/mile")),
                ("line_configuration", "rating.summer.continuous") => {
                    Some((GlmQuantityKind::Real, "A"))
                }
                _ => None,
            };
            if let Some((kind, unit)) = spec {
                let quantity = project.quantity(property, kind, Some(unit))?;
                let key = format!("{name}.{}", property.name);
                if values.insert(key, serde_json::json!({
                    "value": [quantity.value.re, quantity.value.im], "unit": unit,
                    "diagnostics": quantity.diagnostics.iter().map(powerio_core::Diagnostic::code).collect::<Vec<_>>(),
                })).is_some() { return Err("duplicate oracle quantity".into()); }
            }
        }
    }
    println!("{}", serde_json::to_string_pretty(&values)?);
    Ok(())
}
