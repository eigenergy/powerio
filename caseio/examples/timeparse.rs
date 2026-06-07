//! Median parse time for one `.m` file. `cargo run --release --example timeparse -- <path>`.
//! `parse_matpower` builds the typed `Network` and retains the source text for a
//! byte-exact round-trip; this is the single parse path benchmarked against
//! other parsers.
fn median(f: impl Fn()) -> f64 {
    for _ in 0..5 {
        f();
    }
    let mut times: Vec<f64> = (0..50)
        .map(|_| {
            let t = std::time::Instant::now();
            f(); // f black-boxes its own result, so the call can't be elided
            t.elapsed().as_secs_f64() * 1e3
        })
        .collect();
    times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    times[times.len() / 2]
}

fn main() {
    let path = std::env::args().nth(1).expect("usage: timeparse <file.m>");
    let src = std::fs::read_to_string(&path).unwrap();

    let parse = median(|| {
        std::hint::black_box(caseio::parse_matpower(std::hint::black_box(&src)).unwrap());
    });

    let case = caseio::parse_matpower(&src).unwrap();
    println!(
        "{parse:.3} ms  buses={} branches={}",
        case.buses.len(),
        case.branches.len()
    );
}
