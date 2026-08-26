//! Include resolution fuzzing for the OpenDSS reader over the in-memory
//! acquisition path (#339). The input splits on 0xFF into a main script and
//! up to eight named buffers (`inc0.dss` .. `inc7.dss`), so a script's
//! `Redirect`/`Compile` lines can reach real content: lexical containment,
//! the include count and total byte budgets, nesting, and `Clear` all
//! execute, where the plain dss target's loader refused every include.
//! Canonical containment (symlinks, `..` through the real filesystem) is the
//! sibling `dss_includes_fs` target's half.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let mut parts = data.split(|b| *b == 0xFF);
    let Some(main) = parts.next() else {
        return;
    };
    if std::str::from_utf8(main).is_err() {
        return;
    }
    let Ok(mut source) = powerio_core::Source::from_bytes("main.dss", main.to_vec()) else {
        return;
    };
    for (i, chunk) in parts.take(8).enumerate() {
        let Ok(with) = source.with_named_buffer(format!("inc{i}.dss"), chunk.to_vec()) else {
            return;
        };
        source = with;
    }
    let Ok(id) = powerio_core::FormatId::new("dss") else {
        return;
    };
    let Ok(module) = powerio_dist::parse(source.with_format(id)) else {
        return;
    };
    let net = module.value();
    let _ = powerio_dist::write_dss(net);
});
