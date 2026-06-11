//! Corpus harness: every available PowerWorld binary classifies into exactly
//! one support tier, and the committed coverage matrix in `docs/powerworld.md`
//! mirrors this table. Vendored and fetched entries carry committed
//! expectations and skip when the fetch has not run. A gitignored local
//! manifest (`tests/data/local_pwb_corpus.tsv`; tab separated `label`,
//! `path`, `expectation` per line, `#` comments) adds machine specific files
//! under the same tiers; the matrix lists those under generic labels with no
//! paths. Expectations: `decoded:<buses>:<branches>` or
//! `rejected:<error substring>`.

use std::path::Path;

use powerio::format::powerworld::parse_pwb;

mod common;
use common::{activsg2000_fetched, powerworld_vendored, rts_gmlc_fetched};

enum Expect {
    Decoded { buses: usize, branches: usize },
    Rejected { evidence: String },
}

fn check(label: &str, path: &Path, expect: &Expect) {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("{label}: {e}"));
    let outcome = parse_pwb(&bytes, None);
    match expect {
        Expect::Decoded { buses, branches } => {
            let net = outcome.unwrap_or_else(|e| panic!("{label}: expected a decode, got: {e}"));
            assert_eq!(net.buses.len(), *buses, "{label} buses");
            assert_eq!(net.branches.len(), *branches, "{label} branches");
        }
        Expect::Rejected { evidence } => {
            let err = match outcome {
                Err(e) => e.to_string(),
                Ok(net) => panic!(
                    "{label}: expected a rejection, decoded {} buses / {} branches; \
                     promote it in the coverage matrix",
                    net.buses.len(),
                    net.branches.len()
                ),
            };
            assert!(
                err.contains(evidence.as_str()),
                "{label}: rejection evidence changed: {err}"
            );
        }
    }
}

#[test]
fn every_available_pwb_lands_in_its_tier() {
    let committed = [
        (
            "ACTIVSg200 June 2018 export (vendored)",
            Some(powerworld_vendored("ACTIVSg200.pwb")),
            Expect::Decoded {
                buses: 200,
                branches: 246,
            },
        ),
        (
            "ACTIVSg2000 June 2016 export (fetched)",
            activsg2000_fetched("Texas2000_June2016.pwb"),
            Expect::Decoded {
                buses: 2007,
                branches: 3043,
            },
        ),
        (
            "ACTIVSg2000 v19 2017 export (fetched)",
            activsg2000_fetched("ACTIV_SG_2000_v19.pwb"),
            Expect::Decoded {
                buses: 2000,
                branches: 3202,
            },
        ),
        (
            "RTS-GMLC (fetched)",
            rts_gmlc_fetched("RTS-GMLC.PWB"),
            Expect::Decoded {
                buses: 73,
                branches: 120,
            },
        ),
    ];
    for (label, path, expect) in &committed {
        match path {
            Some(p) => check(label, p, expect),
            None => eprintln!("skipped {label}: run benchmarks/fetch_powerworld.sh"),
        }
    }

    // Machine specific corpus: real identities live only in the gitignored
    // manifest; the committed matrix carries the generic labels.
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/data/local_pwb_corpus.tsv");
    let Ok(text) = std::fs::read_to_string(&manifest) else {
        eprintln!("skipped local corpus: no {}", manifest.display());
        return;
    };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut f = line.split('\t');
        let (Some(label), Some(path), Some(exp)) = (f.next(), f.next(), f.next()) else {
            panic!("malformed manifest line: {line}");
        };
        let path = Path::new(path);
        if !path.exists() {
            eprintln!("skipped {label}: {} absent", path.display());
            continue;
        }
        let expect = if let Some(rest) = exp.strip_prefix("decoded:") {
            let (b, br) = rest.split_once(':').expect("decoded:<buses>:<branches>");
            Expect::Decoded {
                buses: b.parse().unwrap(),
                branches: br.parse().unwrap(),
            }
        } else if let Some(evidence) = exp.strip_prefix("rejected:") {
            Expect::Rejected {
                evidence: evidence.to_string(),
            }
        } else {
            panic!("unknown expectation {exp:?} for {label}");
        };
        check(label, path, &expect);
    }
}
