//! Median parse time for one `.m` file. `cargo run --release --example timeparse -- <path>`.
fn main() {
    let path = std::env::args().nth(1).expect("usage: timeparse <file.m>");
    let src = std::fs::read_to_string(&path).unwrap();
    for _ in 0..5 {
        std::hint::black_box(caseio::parse_matpower(&src).unwrap());
    }
    let mut times: Vec<f64> = (0..50)
        .map(|_| {
            let t = std::time::Instant::now();
            std::hint::black_box(caseio::parse_matpower(&src).unwrap());
            t.elapsed().as_secs_f64() * 1e3
        })
        .collect();
    times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median = times[times.len() / 2];
    let case = caseio::parse_matpower(&src).unwrap();
    println!(
        "{median:.3} ms  buses={} branches={}",
        case.n(),
        case.branches.len()
    );
}
