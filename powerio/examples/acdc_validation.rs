//! Generate native declarations and a source-independent export for the Julia oracle.
use powerio::{Destination, PioValue};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let input = args
        .next()
        .ok_or("usage: acdc_validation CASE OUTPUT_DIRECTORY")?;
    let out = std::path::PathBuf::from(args.next().ok_or("missing output directory")?);
    std::fs::create_dir_all(&out)?;
    let module = powerio::parse(input.as_str())?;
    let PioValue::BalancedNetwork(network) = module.value() else {
        return Err("not a balanced network".into());
    };
    if network.ac_dc_equivalent().is_none() {
        return Err("no AC/DC declarations".into());
    }
    powerio::serialize(&module, Destination::path(out.join("source.pio.json")))?;
    let restored = powerio::deserialize(out.join("source.pio.json"))?;
    powerio::emit(
        &restored,
        "matpower-acdc",
        Destination::path(out.join("fresh.m")),
    )?;
    Ok(())
}
