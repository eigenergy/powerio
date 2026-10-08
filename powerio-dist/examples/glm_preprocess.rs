//! Validation driver for the bounded GLM frontend, not a network converter.
use std::io::Write;

use powerio_core::Source;
use powerio_dist::glm::{GlmDialect, PreprocessOptions, parse_raw_source, preprocess_source};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let input = args
        .next()
        .ok_or("usage: glm_preprocess FILE [pnnl|arras] [ACQUISITION_ROOT]")?;
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
    let prepared = preprocess_source(
        source,
        PreprocessOptions {
            dialect,
            ..Default::default()
        },
    )?;
    parse_raw_source(Source::from_memory(
        "expanded.glm",
        prepared.text().as_bytes(),
    )?)?;
    std::io::stdout()
        .lock()
        .write_all(prepared.text().as_bytes())?;
    Ok(())
}
