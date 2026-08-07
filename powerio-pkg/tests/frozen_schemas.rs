//! The retired schema documents under `docs/schema/` are frozen. Old
//! `.pio.json` files declare their URLs, and the docs promise each URL
//! stays served. The `rust.yml` schemas job regenerates only the current
//! document, so it cannot catch a deletion here. This test pins the frozen
//! documents byte for byte. To retire one, change the pins and
//! `docs/schema/README.md` together.

/// FNV-1a 64. Implemented inline so the pin does not depend on a hasher whose
/// output could change across Rust or dependency versions.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[test]
fn retired_schema_documents_stay_published_byte_for_byte() {
    let frozen: [(&str, usize, u64); 3] = [
        (
            "../docs/schema/pio-package/0.1/schema.json",
            125_750,
            0xe5af_9f64_b26e_edc2,
        ),
        (
            "../docs/schema/pio-payload-balanced/1/schema.json",
            51_415,
            0xe790_d6e1_ba75_a74f,
        ),
        (
            "../docs/schema/pio-payload-multiconductor/1/schema.json",
            35_414,
            0xabfe_0107_29fa_2afa,
        ),
    ];
    for (path, len, hash) in frozen {
        let bytes = std::fs::read(path).unwrap_or_else(|e| {
            panic!(
                "{path} is a frozen schema document that pre-v0.8.0 .pio.json files \
                 reference by URL; it must stay published (see docs/schema/README.md). \
                 Could not read it: {e}"
            )
        });
        assert_eq!(
            (bytes.len(), fnv1a(&bytes)),
            (len, hash),
            "{path} changed, but it is frozen at its v0.7.3 bytes: documents in the wild \
             validate against it by URL, so edits belong in a NEW identifier path, not here \
             (see docs/schema/README.md)"
        );
    }
}
