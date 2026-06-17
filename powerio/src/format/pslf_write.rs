//! Write GE PSLF `.epc` power flow cases.
//!
//! The inverse of the [`pslf`](super::pslf) reader: it emits the same colon
//! separated `lhs : rhs` records the reader parses, so a `.epc` → [`Network`] →
//! `.epc` round trip preserves the power flow core. Column positions mirror the
//! reader's field indices exactly (it is the source of truth for `.epc` syntax).
//! Where a PSLF read stashed a field the neutral model does not name under a
//! `pslf_*` extras key (the ZIP load split, the per-unit shunt G/B, the branch
//! circuit id, the transformer winding base), the writer replays it; otherwise it
//! synthesizes the column from the neutral model.
//!
//! Same-format byte-exact echo still rides the retained source text (see
//! [`crate::write_as`]); this serializer is the cross-format path and the
//! fallback for a PSLF network whose source text was dropped (for example after a
//! JSON round trip).

use std::collections::HashMap;
use std::fmt::Write as _;

use serde_json::Value;

use super::{Conversion, sanitize_quoted};
use crate::network::{Branch, BusId, BusType, Extras, Network};

/// The double quote delimits an EPC name token, and the reader's tokenizer
/// toggles on it with no un-escaping, so an embedded quote would shift the record.
const NAME_FORBIDDEN: &[char] = &['"'];

/// Per-bus identity the EPC `lhs` carries on every element record.
#[derive(Clone, Copy)]
struct BusRef<'a> {
    name: &'a str,
    base_kv: f64,
    area: usize,
    zone: usize,
}

/// Serialize `net` to PSLF `.epc` text.
#[must_use]
// A flat serializer: one stanza per EPC section; splitting it would add
// indirection without clarity.
#[expect(clippy::too_many_lines)]
pub fn write_pslf(net: &Network) -> Conversion {
    let mut warnings = Vec::new();
    let mut nonfinite = false;
    let mut sanitized_names = 0usize;
    let mut s = String::new();

    let mut num = |x: f64| -> String {
        if x.is_finite() {
            format!("{x}")
        } else {
            nonfinite = true;
            let sentinel = if x > 0.0 {
                1.0e10
            } else if x < 0.0 {
                -1.0e10
            } else {
                0.0
            };
            format!("{sentinel}")
        }
    };

    // Bus identity for the lhs of every downstream record, keyed by source id.
    let bus_refs: HashMap<BusId, BusRef> = net
        .buses
        .iter()
        .map(|b| {
            (
                b.id,
                BusRef {
                    name: b.name.as_deref().unwrap_or(""),
                    base_kv: b.base_kv,
                    area: b.area,
                    zone: b.zone,
                },
            )
        })
        .collect();
    let bus_ref = |id: BusId| -> BusRef {
        bus_refs.get(&id).copied().unwrap_or(BusRef {
            name: "",
            base_kv: 0.0,
            area: 1,
            zone: 1,
        })
    };
    // A quoted, sanitized name token; counts substitutions for the warning.
    let mut name_tok = |name: &str| -> String {
        let clean = sanitize_quoted(name, NAME_FORBIDDEN, ' ');
        if matches!(clean, std::borrow::Cow::Owned(_)) {
            sanitized_names += 1;
        }
        format!("\"{clean}\"")
    };

    // ---- header blocks ----
    let _ = writeln!(s, "title");
    let _ = writeln!(s, "{}", net.name);
    let _ = writeln!(s, "!");
    let _ = writeln!(s, "comments");
    let _ = writeln!(s, "powerio export");
    let _ = writeln!(s, "!");
    let _ = writeln!(s, "solution parameters");
    let _ = writeln!(s, "sbase {}", num(net.base_mva));
    let _ = writeln!(s, "!");

    // ---- bus data ----
    let _ = writeln!(
        s,
        "bus data [{}] ty vsched volt angle ar zone vmax vmin",
        net.buses.len()
    );
    for b in &net.buses {
        let _ = writeln!(
            s,
            "{} {} {} : {} {} {} {} {} {} {} {}",
            b.id,
            name_tok(b.name.as_deref().unwrap_or("")),
            num(b.base_kv),
            pslf_type(b.kind),
            num(b.vm),
            num(b.vm),
            num(b.va),
            b.area,
            b.zone,
            num(b.vmax),
            num(b.vmin),
        );
    }

    // ---- load data ----
    if !net.loads.is_empty() {
        let _ = writeln!(
            s,
            "load data [{}] id long_id st mw mvar mw_i mvar_i mw_z mvar_z ar zone",
            net.loads.len()
        );
        for l in &net.loads {
            let r = bus_ref(l.bus);
            // Replay the ZIP split a PSLF read preserved; otherwise put the whole
            // demand in the constant-power column.
            let mw = extra_f64(&l.extras, "pslf_mw").unwrap_or(l.p);
            let mvar = extra_f64(&l.extras, "pslf_mvar").unwrap_or(l.q);
            let mw_i = extra_f64(&l.extras, "pslf_mw_i").unwrap_or(0.0);
            let mvar_i = extra_f64(&l.extras, "pslf_mvar_i").unwrap_or(0.0);
            let mw_z = extra_f64(&l.extras, "pslf_mw_z").unwrap_or(0.0);
            let mvar_z = extra_f64(&l.extras, "pslf_mvar_z").unwrap_or(0.0);
            let _ = writeln!(
                s,
                "{} {} {} \"1\" \"load\" : {} {} {} {} {} {} {} {} {}",
                l.bus,
                name_tok(r.name),
                num(r.base_kv),
                i32::from(l.in_service),
                num(mw),
                num(mvar),
                num(mw_i),
                num(mvar_i),
                num(mw_z),
                num(mvar_z),
                r.area,
                r.zone,
            );
        }
    }

    // ---- shunt data ----
    if !net.shunts.is_empty() {
        let _ = writeln!(
            s,
            "shunt data [{}] id ck se long_id st ar zone pu_mw pu_mvar",
            net.shunts.len()
        );
        for sh in &net.shunts {
            let r = bus_ref(sh.bus);
            // PSLF stores shunt G/B per unit on the system base; replay the read
            // values when present, else divide the MW/MVAr-at-1pu back out.
            let pu_mw = extra_f64(&sh.extras, "pslf_pu_mw")
                .or_else(|| extra_f64(&sh.extras, "pslf_pu_g"))
                .unwrap_or_else(|| safe_div(sh.g, net.base_mva));
            let pu_mvar = extra_f64(&sh.extras, "pslf_pu_mvar")
                .or_else(|| extra_f64(&sh.extras, "pslf_pu_b"))
                .unwrap_or_else(|| safe_div(sh.b, net.base_mva));
            let _ = writeln!(
                s,
                "{} {} {} \"1\" : {} {} {} {} {}",
                sh.bus,
                name_tok(r.name),
                num(r.base_kv),
                i32::from(sh.in_service),
                r.area,
                r.zone,
                num(pu_mw),
                num(pu_mvar),
            );
        }
    }

    // ---- branch data (non-transformer) ----
    let lines: Vec<&Branch> = net
        .branches
        .iter()
        .filter(|b| !b.is_transformer())
        .collect();
    if !lines.is_empty() {
        let _ = writeln!(
            s,
            "branch data [{}] ck se long_id st resist react charge rate1 rate2 rate3",
            lines.len()
        );
        for br in lines {
            let f = bus_ref(br.from);
            let t = bus_ref(br.to);
            let _ = writeln!(
                s,
                "{} {} {} {} {} {} {} 1 \"line\" : {} {} {} {} {} {} {}",
                br.from,
                name_tok(f.name),
                num(f.base_kv),
                br.to,
                name_tok(t.name),
                num(t.base_kv),
                circuit_tok(&br.extras),
                i32::from(br.in_service),
                num(br.r),
                num(br.x),
                num(br.b),
                num(br.rate_a),
                num(br.rate_b),
                num(br.rate_c),
            );
        }
    }

    // ---- transformer data (2-winding) ----
    let xfmrs: Vec<&Branch> = net.branches.iter().filter(|b| b.is_transformer()).collect();
    if !xfmrs.is_empty() {
        let _ = writeln!(s, "transformer data [{}]", xfmrs.len());
        for br in xfmrs {
            let f = bus_ref(br.from);
            let t = bus_ref(br.to);
            let tbase = extra_f64(&br.extras, "pslf_tbase").unwrap_or(net.base_mva);
            // First physical line: identity lhs, then the 21-field rhs the reader
            // indexes (status 0, tertiary 9 = 0, base 14, R 15, X 16, and the
            // pt/ts tertiary impedances 17-20 = 0 to mark a 2-winding unit). The
            // trailing `/` continues the record onto the second line.
            let mut rhs1 = vec!["0".to_string(); 21];
            rhs1[0] = i32::from(br.in_service).to_string();
            rhs1[14] = num(tbase);
            rhs1[15] = num(br.r);
            rhs1[16] = num(br.x);
            let _ = writeln!(
                s,
                "{} {} {} {} {} {} {} 1 \"xfmr\" : {} /",
                br.from,
                name_tok(f.name),
                num(f.base_kv),
                br.to,
                name_tok(t.name),
                num(t.base_kv),
                circuit_tok(&br.extras),
                rhs1.join(" "),
            );
            // Second physical line: ratings at 6-8, phase shift at 10, tap at 16.
            let mut line2 = vec!["0".to_string(); 17];
            line2[6] = num(br.rate_a);
            line2[7] = num(br.rate_b);
            line2[8] = num(br.rate_c);
            line2[10] = num(br.shift);
            line2[16] = num(br.effective_tap());
            let _ = writeln!(s, "{}", line2.join(" "));
        }
    }

    // ---- generator data ----
    if !net.generators.is_empty() {
        let _ = writeln!(
            s,
            "generator data [{}] id long_id st no reg_name reg_kv prf qrf ar zone \
             pgen pmax pmin qgen qmax qmin mbase",
            net.generators.len()
        );
        for g in &net.generators {
            let r = bus_ref(g.bus);
            // rhs indices the reader reads: status 0, pgen 8, pmax 9, pmin 10,
            // qgen 11, qmax 12, qmin 13, mbase 14. The reader takes vg from the bus
            // voltage, so it is not carried here.
            let _ = writeln!(
                s,
                "{} {} \"1\" \"gen\" : {} 1 0 0 1 1 {} {} {} {} {} {} {} {} {}",
                g.bus,
                name_tok(r.name),
                i32::from(g.in_service),
                r.area,
                r.zone,
                num(g.pg),
                num(g.pmax),
                num(g.pmin),
                num(g.qg),
                num(g.qmax),
                num(g.qmin),
                num(g.mbase),
            );
        }
    }

    let _ = writeln!(s, "end");

    // ---- fidelity warnings ----
    if !net.hvdc.is_empty() {
        warnings.push(format!(
            "{} dcline(s) dropped: PSLF DC converter/line export not yet modeled",
            net.hvdc.len()
        ));
    }
    if !net.storage.is_empty() {
        warnings.push(format!(
            "{} storage unit(s) dropped: PSLF .epc has no storage record",
            net.storage.len()
        ));
    }
    if net.generators.iter().any(|g| g.cost.is_some()) {
        warnings.push("generator cost curves dropped: PSLF .epc carries no cost data".into());
    }
    if sanitized_names > 0 {
        warnings.push(format!(
            "{sanitized_names} name(s) contained a double quote that would corrupt an EPC \
             record; replaced with spaces"
        ));
    }
    if nonfinite {
        warnings.push("non-finite values written as ±1e10 sentinels (PSLF has no Inf/NaN)".into());
    }

    Conversion { text: s, warnings }
}

/// Neutral bus kind → PSLF bus type code (inverse of the reader's `pslf_bus_type`).
fn pslf_type(kind: BusType) -> u8 {
    match kind {
        BusType::Ref => 0,
        BusType::Pv => 2,
        BusType::Isolated => 4,
        BusType::Pq => 1,
    }
}

/// The branch/transformer circuit id token, replayed from `pslf_circuit` when a
/// PSLF read kept it, else `"1"`.
fn circuit_tok(extras: &Extras) -> String {
    let ck = extras
        .get("pslf_circuit")
        .and_then(Value::as_str)
        .unwrap_or("1");
    format!("\"{ck}\"")
}

/// A numeric `pslf_*` extra, if present and finite.
fn extra_f64(extras: &Extras, key: &str) -> Option<f64> {
    extras.get(key).and_then(Value::as_f64)
}

/// `a / b`, or 0 when `b` is not a usable divisor (the identity for an absent base).
fn safe_div(a: f64, b: f64) -> f64 {
    if b.is_finite() && b != 0.0 {
        a / b
    } else {
        0.0
    }
}
