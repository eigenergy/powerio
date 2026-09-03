//! Wave 0 allocation, peak memory, and wall time baseline for PowerIO 0.11.
//!
//! Not part of the workspace. It links the current crates by path and reports
//! deterministic allocation counts so later branches have a real "before".

use std::alloc::{GlobalAlloc, Layout, System};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

static ALLOCS: AtomicUsize = AtomicUsize::new(0);
static ALLOC_BYTES: AtomicUsize = AtomicUsize::new(0);
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            record_alloc(layout.size());
        }
        p
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let p = unsafe { System.realloc(ptr, layout, new_size) };
        if !p.is_null() {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
            if new_size > layout.size() {
                let grew = new_size - layout.size();
                ALLOC_BYTES.fetch_add(grew, Ordering::Relaxed);
                bump_live(grew);
            } else {
                LIVE.fetch_sub(layout.size() - new_size, Ordering::Relaxed);
            }
        }
        p
    }
}

fn record_alloc(size: usize) {
    ALLOCS.fetch_add(1, Ordering::Relaxed);
    ALLOC_BYTES.fetch_add(size, Ordering::Relaxed);
    bump_live(size);
}

fn bump_live(size: usize) {
    let live = LIVE.fetch_add(size, Ordering::Relaxed) + size;
    PEAK.fetch_max(live, Ordering::Relaxed);
}

#[global_allocator]
static A: Counting = Counting;

#[derive(Clone, Copy)]
struct Sample {
    allocs: usize,
    bytes: usize,
    peak: usize,
    micros: u128,
}

fn measure<T>(f: impl FnOnce() -> T) -> (T, Sample) {
    // Settle whatever the caller left in flight before taking a peak reading.
    let base_live = LIVE.load(Ordering::Relaxed);
    ALLOCS.store(0, Ordering::Relaxed);
    ALLOC_BYTES.store(0, Ordering::Relaxed);
    PEAK.store(base_live, Ordering::Relaxed);
    let start = Instant::now();
    let value = f();
    let micros = start.elapsed().as_micros();
    let sample = Sample {
        allocs: ALLOCS.load(Ordering::Relaxed),
        bytes: ALLOC_BYTES.load(Ordering::Relaxed),
        peak: PEAK.load(Ordering::Relaxed).saturating_sub(base_live),
        micros,
    };
    (value, sample)
}

fn row(case: &str, op: &str, input_bytes: u64, s: Sample) {
    println!(
        "{case}\t{op}\t{input_bytes}\t{}\t{}\t{}\t{}",
        s.allocs, s.bytes, s.peak, s.micros
    );
}

fn file_len(p: &Path) -> u64 {
    std::fs::metadata(p).map(|m| m.len()).unwrap_or(0)
}

/// Repository root, so a run is reproducible from any working directory.
fn root() -> std::path::PathBuf {
    std::env::var_os("POWERIO_ROOT").map_or_else(
        || {
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .and_then(std::path::Path::parent)
                .expect("evals/allocation sits two levels below the repository root")
                .to_path_buf()
        },
        std::path::PathBuf::from,
    )
}

fn balanced_cases() -> Vec<(&'static str, String)> {
    let small = [
        "tests/data/case9.m",
        "tests/data/case118.m",
        "tests/data/case2869pegase.m",
    ];
    let large = [
        "tests/data/large/case_ACTIVSg2000.m",
        "tests/data/large/case9241pegase.m",
        "tests/data/large/case_ACTIVSg10k.m",
        "tests/data/large/case_ACTIVSg25k.m",
        "tests/data/large/case99k.m",
    ];
    small
        .iter()
        .chain(large.iter())
        .map(|rel| {
            let name: &'static str = Box::leak(
                Path::new(rel)
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
                    .into_boxed_str(),
            );
            (name, root().join(rel).to_string_lossy().into_owned())
        })
        .collect()
}

fn main() {
    println!("case\top\tinput_bytes\tallocs\talloc_bytes\tpeak_live_bytes\twall_micros");

    for (name, path) in balanced_cases() {
        let p = Path::new(&path);
        if !p.exists() {
            eprintln!("skip missing {path}");
            continue;
        }
        let len = file_len(p);

        let (parsed, s) = measure(|| {
            powerio_core::Source::open(&path)
                .map_err(|e| e.to_string())
                .and_then(|source| powerio_tx::format::parse(source).map_err(|e| e.to_string()))
                .map(powerio_core::PioModule::into_value)
        });
        let parsed = match parsed {
            Ok(v) => v,
            Err(e) => {
                eprintln!("parse failed {name}: {e}");
                continue;
            }
        };
        row(name, "parse_matpower", len, s);

        let net = parsed;

        let (indexed, s) = measure(|| powerio_matrix::IndexedNetwork::new(&net));
        row(name, "indexed_build", len, s);

        let (_, s) = measure(|| net.clone());
        row(name, "network_clone", len, s);

        let opts = powerio_matrix::matrix::BuildOptions::default();
        let (_, s) = measure(|| powerio_matrix::matrix::calc_admittance_matrix(&indexed, &opts));
        row(name, "ybus", len, s);

        let dc_instance = powerio::DcPfInstance::from_network(net.clone())
            .expect("parsed case has a DC power flow instance");
        let (_, s) = measure(|| {
            powerio_matrix::DcOperators::build(&dc_instance)
                .map(|operators| operators.calc_incidence_matrix())
        });
        row(name, "dc_incidence", len, s);

        // The dense sensitivity path is quadratic; keep it to cases where that
        // is still measurable in a reasonable time.
        if indexed.n() <= 3000 {
            let (_, s) = measure(|| {
                powerio_matrix::matrix::sensitivity::calc_ptdf(
                    &indexed,
                    powerio_matrix::BranchSusceptanceFormula::default(),
                )
            });
            row(name, "ptdf", len, s);

            let (_, s) = measure(|| {
                powerio_matrix::matrix::sensitivity::calc_ptdf_lodf(
                    &indexed,
                    powerio_matrix::BranchSusceptanceFormula::default(),
                )
            });
            row(name, "ptdf_lodf", len, s);
        }
    }

    // PSS/E, pandapower, PyPSA, Egret: the parsers the architecture calls out
    // as allocating owned strings while scanning.
    for (name, rel, from) in [
        ("case14.raw", "tests/data/psse/case14.raw", Some("psse")),
        (
            "pandapower-example.json",
            "tests/data/pandapower/example.json",
            Some("pandapower"),
        ),
        ("pypsa-example", "tests/data/pypsa/example", Some("pypsa")),
    ] {
        let path = root().join(rel).to_string_lossy().into_owned();
        let p = Path::new(&path);
        if !p.exists() {
            eprintln!("skip missing {path}");
            continue;
        }
        let len = if p.is_dir() {
            std::fs::read_dir(p)
                .map(|d| d.filter_map(|e| e.ok()).map(|e| file_len(&e.path())).sum())
                .unwrap_or(0)
        } else {
            file_len(p)
        };
        let (r, s) = measure(|| {
            let mut source = powerio_core::Source::open(&path).map_err(|e| e.to_string())?;
            if let Some(token) = from {
                source = source
                    .with_format(powerio_core::FormatId::new(token).map_err(|e| e.to_string())?);
            }
            powerio_tx::format::parse(source)
                .map(powerio_core::PioModule::into_value)
                .map_err(|e| e.to_string())
        });
        match r {
            Ok(_) => row(name, "parse", len, s),
            Err(e) => eprintln!("parse failed {name}: {e}"),
        }
    }

    // #293: the big-case JSON and token readers, on the same case2869pegase
    // every number in the issue used, via in-process conversions so no
    // fixture is duplicated. The input buffer is handed to the source inside
    // the measurement, so peak includes exactly what a real parse holds.
    let big = root().join("tests/data/case2869pegase.m");
    if big.exists() {
        let module = powerio_core::Source::open(&big)
            .map_err(|e| e.to_string())
            .and_then(|s| powerio_tx::format::parse(s).map_err(|e| e.to_string()))
            .expect("case2869pegase parses");
        for (op, token) in [
            ("parse_powermodels_2869", "powermodels-json"),
            ("parse_egret_2869", "egret-json"),
            ("parse_pandapower_2869", "pandapower-json"),
            ("parse_psse_2869", "psse"),
            ("parse_aux_2869", "aux"),
        ] {
            let Some(target) = powerio_tx::format::parse_target_format(token) else {
                eprintln!("unknown target {token}");
                continue;
            };
            let destination = match powerio_core::Destination::memory("case") {
                Ok(destination) => destination,
                Err(e) => {
                    eprintln!("emit destination failed {token}: {e}");
                    continue;
                }
            };
            let text = match powerio_tx::emit(&module, target, destination) {
                Ok(result) => {
                    let powerio_core::EmittedOutput::Memory { mut artifacts } =
                        result.into_output()
                    else {
                        unreachable!("memory destination returns memory output");
                    };
                    let bytes = artifacts
                        .pop()
                        .expect("text emission has one artifact")
                        .into_bytes();
                    String::from_utf8(bytes).expect("case text is UTF-8")
                }
                Err(e) => {
                    eprintln!("emit failed {token}: {e}");
                    continue;
                }
            };
            let len = text.len() as u64;
            let bytes = text.into_bytes();
            let (r, s) = measure(move || {
                powerio_core::Source::from_memory("case2869", bytes)
                    .map_err(|e| e.to_string())
                    .and_then(|source| {
                        let source = source.with_format(
                            powerio_core::FormatId::new(token).map_err(|e| e.to_string())?,
                        );
                        powerio_tx::format::parse(source)
                            .map(powerio_core::PioModule::into_value)
                            .map_err(|e| e.to_string())
                    })
            });
            match r {
                Ok(_) => row(op, "parse", len, s),
                Err(e) => eprintln!("parse failed {op}: {e}"),
            }
        }
    }

    // Multiconductor: OpenDSS and BMOPF.
    for (name, rel, from) in [
        (
            "ieee13.dss",
            "tests/data/dist/opendss/ieee13/IEEE13Nodeckt.dss",
            None,
        ),
        (
            "ieee123.dss",
            "tests/data/dist/opendss/ieee123/IEEE123Master.dss",
            None,
        ),
        (
            "bmopf-ieee13.json",
            "tests/data/dist/bmopf/example_ieee13.json",
            Some("bmopf"),
        ),
        (
            "bmopf-enwl.json",
            "tests/data/dist/bmopf/example_enwl_n1_f2.json",
            Some("bmopf"),
        ),
    ] {
        let path = root().join(rel).to_string_lossy().into_owned();
        let p = Path::new(&path);
        if !p.exists() {
            eprintln!("skip missing {path}");
            continue;
        }
        let len = file_len(p);
        let (r, s) = measure(|| {
            let mut source = powerio_core::Source::open(&path).map_err(|e| e.to_string())?;
            if let Some(token) = from {
                let token = if token == "bmopf" {
                    "bmopf-json"
                } else {
                    token
                };
                source = source
                    .with_format(powerio_core::FormatId::new(token).map_err(|e| e.to_string())?);
            }
            powerio_dist::parse(source)
                .map(powerio_core::PioModule::into_value)
                .map_err(|e| e.to_string())
        });
        match r {
            Ok(net) => {
                row(name, "dist_parse", len, s);
                let (_, s) = measure(|| net.clone());
                row(name, "dist_network_clone", len, s);
            }
            Err(e) => eprintln!("dist parse failed {name}: {e}"),
        }
    }
}
