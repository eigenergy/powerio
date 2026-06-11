fn main() {
    let bytes = std::fs::read("tests/data/powerworld/ACTIVSg200.pwb").unwrap();
    let mut best = std::time::Duration::MAX;
    for _ in 0..300 {
        let t = std::time::Instant::now();
        let n = powerio::format::powerworld::parse_pwb(&bytes, None).unwrap();
        assert_eq!(n.buses.len(), 200);
        best = best.min(t.elapsed());
    }
    println!("activsg200 min: {best:?}");
    if let Ok(p) = std::env::var("BISECT_V21") {
        let bytes = std::fs::read(p).unwrap();
        let mut best = std::time::Duration::MAX;
        for _ in 0..5 {
            let t = std::time::Instant::now();
            let n = powerio::format::powerworld::parse_pwb(&bytes, None).unwrap();
            assert_eq!(n.buses.len(), 6717);
            best = best.min(t.elapsed());
        }
        println!("v21 min: {best:?}");
    }
}
