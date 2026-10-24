use powerio::{DynamicStudy, PioValue, parse, parse_dyr};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let raw = parse("case.raw")?;
    let PioValue::BalancedNetwork(network) = raw.value() else {
        return Err("case.raw did not produce a balanced network".into());
    };

    let dyr_text = std::fs::read_to_string("case.dyr")?;
    let dynamics = parse_dyr(&dyr_text)?;
    let study = DynamicStudy::new(network.clone(), dynamics);

    for unresolved in study.unresolved_buses() {
        eprintln!(
            "line {}: {} references missing bus {}",
            unresolved.line, unresolved.model_name, unresolved.bus_id
        );
    }
    Ok(())
}
