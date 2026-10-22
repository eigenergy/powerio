fn main() {
    let bytes = std::fs::read(std::env::args().nth(1).unwrap()).unwrap();
    let parsed = powerio_tx::format::powerworld::__parse_pwd_layer(&bytes).unwrap();
    let buses = parsed
        .layer
        .features
        .iter()
        .filter(|f| f.target == powerio_tx::geo::GeoTarget::Bus)
        .count();
    let branches = parsed
        .layer
        .features
        .iter()
        .filter(|f| f.target == powerio_tx::geo::GeoTarget::Branch)
        .count();
    println!(
        "{buses} bus positions, {branches} branch routes; {:?}",
        parsed.diagnostics
    );
    assert_eq!(buses, 250);
    assert_eq!(branches, 339);
    assert_eq!(parsed.diagnostics, [] as [powerio_tx::Diagnostic; 0]);
}
