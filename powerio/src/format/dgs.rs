//! DIgSILENT PowerFactory DGS plaintext interchange reader.
//!
//! The `.pfd` project export is an encrypted binary container with no public
//! decoder (see [`super::powerfactory`]). DGS is the format PowerFactory writes
//! for data exchange, and the one every open tool (powsybl, GridCal, roseau)
//! reads to interoperate with PowerFactory without the GUI. It is a flat
//! table-per-class text dump: a `$$<Class>;name(type:width);...` header followed
//! by semicolon-delimited rows, with type codes `a` string, `i` integer, `r`
//! real, `p` object pointer. Connectivity runs element → `StaCubic` cubicle →
//! `ElmTerm` bus, so a reader resolves endpoints in a second pass over the
//! cubicles.
//!
//! Two schema generations appear in the wild and both parse here: V5 uses an
//! integer `ID` column and dot decimals; V7 uses a string `FID` column, an extra
//! `OP` column, and comma decimals. The reader keys columns by descriptor name
//! (not by position) and object pointers by their raw string, so column-order and
//! id-scheme differences resolve uniformly.
//!
//! Electrical parameters live on the type object (`TypLne`/`TypTr2`), not the
//! element: a line's series impedance is `rline·dline`, converted to per unit on
//! the bus voltage base. DGS carries no system MVA base, so `base_mva` defaults
//! to 100, matching the PowerWorld readers.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use crate::network::{
    Branch, Bus, BusId, BusType, Extras, GenCaps, Generator, Load, Network, Shunt, SourceFormat,
};
use crate::{Error, Result};

pub(crate) const FMT: &str = "PowerFactory DGS";
const DEFAULT_BASE_MVA: f64 = 100.0;
const DEFAULT_FREQ_HZ: f64 = 50.0;

/// Parse the DGS case in `content` into a [`Network`].
///
/// # Errors
/// [`Error::FormatRead`] when the text carries no `$$ElmTerm` bus table or a
/// required field is present but unparseable.
pub fn parse_dgs(content: &str) -> Result<Network> {
    parse_dgs_source(Arc::new(content.to_owned()), None).map(|(net, _)| net)
}

/// Parse the DGS case at `path`, using the file stem as a fallback network name.
///
/// # Errors
/// [`Error::Io`] if the file cannot be read; otherwise as [`parse_dgs`].
pub fn parse_dgs_file(path: impl AsRef<Path>) -> Result<Network> {
    let path = path.as_ref();
    let content = std::fs::read_to_string(path)?;
    let stem = path.file_stem().and_then(|s| s.to_str());
    parse_dgs_source(Arc::new(content), stem).map(|(net, _)| net)
}

/// Owned-source entry used by the format hub: parse `source`, retain it for a
/// same-format echo, and return the network plus fidelity warnings.
pub(crate) fn parse_dgs_source(
    source: Arc<String>,
    name_hint: Option<&str>,
) -> Result<(Network, Vec<String>)> {
    let mut warnings = Vec::new();
    let mut net = build_network(&source, name_hint, &mut warnings)?;
    net.source = Some(source);
    super::reject_empty_case(&net, FMT)?;
    net.check_references(FMT)?;
    Ok((net, warnings))
}

// ---------------------------------------------------------------------------
// Lexing: source text -> typed tables
// ---------------------------------------------------------------------------

/// One logical attribute in a table header. Vector and matrix attributes occupy
/// a variable number of data fields per row (a leading size, then the cells), so
/// they are tracked explicitly to keep row decoding aligned.
struct Attr {
    name: String,
    kind: AttrKind,
}

enum AttrKind {
    Simple,
    Vector,
    Matrix,
}

struct Table {
    cols: Vec<Attr>,
    rows: Vec<Vec<String>>,
}

/// Split `source` into one [`Table`] per `$$Class` section. Comment lines (`*`)
/// and blanks are skipped; a `$$General` or `$$<Class>` line opens a table whose
/// subsequent rows are appended until the next header.
fn lex_tables(source: &str) -> HashMap<String, Table> {
    let mut tables: HashMap<String, Table> = HashMap::new();
    let mut current: Option<String> = None;
    for (line_no, raw) in source.lines().enumerate() {
        let line = if line_no == 0 { strip_bom(raw) } else { raw };
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('*') {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("$$") {
            let class = rest.split(';').next().unwrap_or("").trim().to_string();
            let descriptors: Vec<&str> = trimmed.split(';').skip(1).collect();
            let cols = parse_descriptors(&descriptors);
            current = Some(class.clone());
            tables.entry(class).or_insert(Table {
                cols,
                rows: Vec::new(),
            });
            continue;
        }
        if let Some(class) = &current {
            if let Some(table) = tables.get_mut(class) {
                table.rows.push(split_quote_aware(trimmed));
            }
        }
    }
    tables
}

/// Group raw header descriptors into logical [`Attr`]s. A `NAME:SIZEROW`
/// descriptor begins a vector, or a matrix when the next descriptor is
/// `NAME:SIZECOL`; the following `NAME:*` cell descriptors are folded into it.
fn parse_descriptors(raw: &[&str]) -> Vec<Attr> {
    let names: Vec<String> = raw.iter().map(|d| descriptor_name(d)).collect();
    let mut attrs = Vec::new();
    let mut i = 0;
    while i < names.len() {
        let name = &names[i];
        if let Some(base) = name.strip_suffix(":SIZEROW") {
            let base = base.to_string();
            let is_matrix = names
                .get(i + 1)
                .is_some_and(|next| *next == format!("{base}:SIZECOL"));
            attrs.push(Attr {
                name: base.clone(),
                kind: if is_matrix {
                    AttrKind::Matrix
                } else {
                    AttrKind::Vector
                },
            });
            i += 1;
            let cell_prefix = format!("{base}:");
            while i < names.len() && names[i].starts_with(&cell_prefix) {
                i += 1;
            }
        } else {
            attrs.push(Attr {
                name: name.clone(),
                kind: AttrKind::Simple,
            });
            i += 1;
        }
    }
    attrs
}

/// The attribute name in a descriptor: everything before the `(type:width)`.
fn descriptor_name(descriptor: &str) -> String {
    descriptor
        .split_once('(')
        .map_or(descriptor, |(name, _)| name)
        .trim()
        .to_string()
}

/// Split a data row on `;`, treating a double-quoted span as one field that may
/// contain semicolons. Quotes are not escaped: a `"` opens a span and the next
/// `"` closes it.
fn split_quote_aware(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut field = String::new();
    let mut in_quotes = false;
    for ch in line.chars() {
        match ch {
            '"' => in_quotes = !in_quotes,
            ';' if !in_quotes => out.push(std::mem::take(&mut field)),
            _ => field.push(ch),
        }
    }
    out.push(field);
    out
}

fn strip_bom(line: &str) -> &str {
    line.strip_prefix('\u{feff}').unwrap_or(line)
}

// ---------------------------------------------------------------------------
// Row access
// ---------------------------------------------------------------------------

/// Decode a row into a name → value map of its simple (scalar) attributes,
/// consuming vector and matrix fields by their row-declared sizes to stay
/// aligned. The id column (`ID` in V5, `FID` in V7) is included like any other.
fn row_map<'a>(cols: &'a [Attr], row: &'a [String]) -> HashMap<&'a str, &'a str> {
    let mut map = HashMap::new();
    let mut field = 0usize;
    for attr in cols {
        match attr.kind {
            AttrKind::Simple => {
                if let Some(value) = row.get(field) {
                    map.insert(attr.name.as_str(), value.as_str());
                }
                field += 1;
            }
            AttrKind::Vector => {
                let count = row.get(field).and_then(|s| parse_usize(s)).unwrap_or(0);
                field += 1 + count;
            }
            AttrKind::Matrix => {
                let rows = row.get(field).and_then(|s| parse_usize(s)).unwrap_or(0);
                let cols = row.get(field + 1).and_then(|s| parse_usize(s)).unwrap_or(0);
                field += 2 + rows * cols;
            }
        }
    }
    map
}

/// The row's own object id, reading `FID` (V7) before `ID` (V5).
fn row_id(map: &HashMap<&str, &str>) -> Option<String> {
    map.get("FID")
        .or_else(|| map.get("ID"))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Build an `id -> owned field map` index for a type table (`TypLne`, `TypTr2`,
/// `TypSym`) so elements can resolve their `typ_id` pointer.
fn index_by_id(table: &Table) -> HashMap<String, HashMap<String, String>> {
    let mut out = HashMap::new();
    for row in &table.rows {
        let map = row_map(&table.cols, row);
        if let Some(id) = row_id(&map) {
            let owned = map
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect();
            out.insert(id, owned);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Numeric helpers
// ---------------------------------------------------------------------------

/// Parse a DGS real, accepting either a dot or a comma decimal separator. DGS
/// uses `;` as the field delimiter, so a comma is unambiguously a decimal point.
fn parse_real(s: &str) -> Option<f64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    s.parse::<f64>()
        .ok()
        .or_else(|| s.replace(',', ".").parse::<f64>().ok())
}

fn parse_int(s: &str) -> Option<i64> {
    s.trim().parse().ok()
}

fn parse_usize(s: &str) -> Option<usize> {
    s.trim().parse().ok()
}

/// The first unsigned integer embedded in a name, e.g. `"Bus 01" -> 1`,
/// `"1 RIVERSDE" -> 1`. PowerFactory bus labels carry the case bus number.
fn first_uint(name: &str) -> Option<usize> {
    let bytes = name.as_bytes();
    let start = bytes.iter().position(u8::is_ascii_digit)?;
    let end = bytes[start..]
        .iter()
        .position(|b| !b.is_ascii_digit())
        .map_or(name.len(), |off| start + off);
    name[start..end].parse().ok()
}

// ---------------------------------------------------------------------------
// Build
// ---------------------------------------------------------------------------

fn build_network(
    source: &str,
    name_hint: Option<&str>,
    warnings: &mut Vec<String>,
) -> Result<Network> {
    let tables = lex_tables(source);
    let term_table = tables.get("ElmTerm").ok_or(Error::FormatRead {
        format: FMT,
        message: "no $$ElmTerm bus table; not a PowerFactory DGS export".into(),
    })?;

    let (net_name, freq) = grid_header(&tables, name_hint);
    let base_mva = DEFAULT_BASE_MVA;

    let (mut buses, bus_of_term, base_kv_of_bus) = build_buses(term_table)?;
    let cubicles = collect_cubicles(&tables, &bus_of_term);

    let branches = build_branches(
        &tables,
        &cubicles,
        &base_kv_of_bus,
        base_mva,
        freq,
        warnings,
    );
    let loads = build_loads(&tables, &cubicles, warnings);
    let shunts = build_shunts(&tables, &cubicles, warnings);
    let (generators, ref_buses, pv_buses) =
        build_generators(&tables, &cubicles, base_mva, warnings);

    // Apply derived bus kinds: an explicit slack is Ref; any other in-service
    // generator bus is Pv. DGS exports often omit the slack designation, in
    // which case the network has no reference bus, exactly like a PowerWorld
    // .pwb; to_normalized synthesizes one downstream.
    for bus in &mut buses {
        if ref_buses.contains(&bus.id) {
            bus.kind = BusType::Ref;
        } else if pv_buses.contains(&bus.id) {
            bus.kind = BusType::Pv;
        }
    }
    if ref_buses.is_empty() && !generators.is_empty() {
        warnings.push(
            "no slack designation in the DGS export; the network has no reference bus".into(),
        );
    }

    Ok(Network {
        name: net_name,
        base_mva,
        buses,
        loads,
        shunts,
        branches,
        generators,
        storage: Vec::new(),
        hvdc: Vec::new(),
        source_format: SourceFormat::PowerFactoryDgs,
        source: None,
    })
}

type BusTables = (Vec<Bus>, HashMap<String, BusId>, HashMap<BusId, f64>);

/// Build the bus list plus the lookups other passes need: terminal id → `BusId`
/// and `BusId` → base kV.
fn build_buses(term_table: &Table) -> Result<BusTables> {
    let terms = collect_terms(term_table)?;
    let bus_ids = assign_bus_ids(&terms);
    let mut buses = Vec::with_capacity(terms.len());
    let mut bus_of_term: HashMap<String, BusId> = HashMap::with_capacity(terms.len());
    let mut base_kv_of_bus: HashMap<BusId, f64> = HashMap::with_capacity(terms.len());
    for (term, &num) in terms.iter().zip(&bus_ids) {
        let id = BusId(num);
        bus_of_term.insert(term.id.clone(), id);
        base_kv_of_bus.insert(id, term.base_kv);
        buses.push(Bus {
            id,
            kind: BusType::Pq,
            vm: 1.0,
            va: 0.0,
            base_kv: term.base_kv,
            vmax: 1.1,
            vmin: 0.9,
            area: 0,
            zone: 0,
            name: Some(term.name.clone()),
            extras: Extras::new(),
        });
    }
    Ok((buses, bus_of_term, base_kv_of_bus))
}

type Cubicles = HashMap<String, Vec<(BusId, i64)>>;

/// The single bus an element hangs off (load, shunt, machine): its first
/// cubicle's terminal.
fn bus_of_element(cubicles: &Cubicles, elem: &str) -> Option<BusId> {
    cubicles
        .get(elem)
        .and_then(|ends| ends.first())
        .map(|e| e.0)
}

/// The two endpoint buses of a branch element, ordered by terminal index
/// (`obj_bus` 0 = from / HV, 1 = to / LV).
fn endpoints(cubicles: &Cubicles, elem: &str) -> Option<(BusId, BusId)> {
    let ends = cubicles.get(elem)?;
    if ends.len() < 2 {
        return None;
    }
    let mut sorted = ends.clone();
    sorted.sort_by_key(|e| e.1);
    Some((sorted[0].0, sorted[1].0))
}

fn build_branches(
    tables: &HashMap<String, Table>,
    cubicles: &Cubicles,
    base_kv_of_bus: &HashMap<BusId, f64>,
    base_mva: f64,
    freq: f64,
    warnings: &mut Vec<String>,
) -> Vec<Branch> {
    let typlne = tables.get("TypLne").map(index_by_id).unwrap_or_default();
    let typtr2 = tables.get("TypTr2").map(index_by_id).unwrap_or_default();
    let mut branches = Vec::new();

    if let Some(lines) = tables.get("ElmLne") {
        for row in &lines.rows {
            let m = row_map(&lines.cols, row);
            let Some(id) = row_id(&m) else { continue };
            let Some((from, to)) = endpoints(cubicles, &id) else {
                warnings.push(format!(
                    "ElmLne {id} has no resolvable two-terminal connection; skipped"
                ));
                continue;
            };
            let base_kv = base_kv_of_bus.get(&from).copied().unwrap_or(0.0);
            branches.push(line_branch(&m, &typlne, from, to, base_kv, base_mva, freq));
        }
    }

    if let Some(xfmrs) = tables.get("ElmTr2") {
        for row in &xfmrs.rows {
            let m = row_map(&xfmrs.cols, row);
            let Some(id) = row_id(&m) else { continue };
            let Some((from, to)) = endpoints(cubicles, &id) else {
                warnings.push(format!(
                    "ElmTr2 {id} has no resolvable two-terminal connection; skipped"
                ));
                continue;
            };
            let from_kv = base_kv_of_bus.get(&from).copied().unwrap_or(0.0);
            let to_kv = base_kv_of_bus.get(&to).copied().unwrap_or(0.0);
            branches.push(transformer_branch(
                &m, &typtr2, from, to, from_kv, to_kv, base_mva,
            ));
        }
    }
    if tables.contains_key("ElmTr3") {
        warnings
            .push("three-winding transformers (ElmTr3) are not yet mapped and were skipped".into());
    }
    branches
}

fn build_loads(
    tables: &HashMap<String, Table>,
    cubicles: &Cubicles,
    warnings: &mut Vec<String>,
) -> Vec<Load> {
    let mut loads = Vec::new();
    if let Some(table) = tables.get("ElmLod") {
        for row in &table.rows {
            let m = row_map(&table.cols, row);
            let Some(id) = row_id(&m) else { continue };
            let Some(bus) = bus_of_element(cubicles, &id) else {
                warnings.push(format!("ElmLod {id} has no resolvable bus; skipped"));
                continue;
            };
            loads.push(Load {
                bus,
                p: m.get("plini").and_then(|v| parse_real(v)).unwrap_or(0.0),
                q: m.get("qlini").and_then(|v| parse_real(v)).unwrap_or(0.0),
                in_service: !out_of_service(&m),
                extras: Extras::new(),
            });
        }
    }
    loads
}

fn build_shunts(
    tables: &HashMap<String, Table>,
    cubicles: &Cubicles,
    warnings: &mut Vec<String>,
) -> Vec<Shunt> {
    let mut shunts = Vec::new();
    if let Some(table) = tables.get("ElmShnt") {
        for row in &table.rows {
            let m = row_map(&table.cols, row);
            let Some(id) = row_id(&m) else { continue };
            let Some(bus) = bus_of_element(cubicles, &id) else {
                warnings.push(format!("ElmShnt {id} has no resolvable bus; skipped"));
                continue;
            };
            shunts.push(Shunt {
                bus,
                g: 0.0,
                b: shunt_b(&m),
                in_service: !out_of_service(&m),
                extras: Extras::new(),
            });
        }
    }
    shunts
}

/// Build generators and the buses they make Ref (explicit slack) or Pv
/// (any other in-service machine bus).
fn build_generators(
    tables: &HashMap<String, Table>,
    cubicles: &Cubicles,
    base_mva: f64,
    warnings: &mut Vec<String>,
) -> (Vec<Generator>, HashSet<BusId>, HashSet<BusId>) {
    let typsym = tables.get("TypSym").map(index_by_id).unwrap_or_default();
    let mut generators = Vec::new();
    let mut pv_buses: HashSet<BusId> = HashSet::new();
    let mut ref_buses: HashSet<BusId> = HashSet::new();
    let Some(table) = tables.get("ElmSym") else {
        return (generators, ref_buses, pv_buses);
    };
    for row in &table.rows {
        let m = row_map(&table.cols, row);
        let Some(id) = row_id(&m) else { continue };
        let Some(bus) = bus_of_element(cubicles, &id) else {
            warnings.push(format!("ElmSym {id} has no resolvable bus; skipped"));
            continue;
        };
        let mbase = m
            .get("typ_id")
            .and_then(|t| typsym.get(*t))
            .and_then(|t| t.get("sgn"))
            .and_then(|v| parse_real(v))
            .filter(|s| *s > 0.0)
            .unwrap_or(base_mva);
        let in_service = !out_of_service(&m);
        if in_service {
            if is_slack(&m) {
                ref_buses.insert(bus);
            } else {
                pv_buses.insert(bus);
            }
        }
        generators.push(Generator {
            bus,
            pg: m.get("pgini").and_then(|v| parse_real(v)).unwrap_or(0.0),
            qg: m.get("qgini").and_then(|v| parse_real(v)).unwrap_or(0.0),
            pmax: m
                .get("Pmax_uc")
                .and_then(|v| parse_real(v))
                .unwrap_or(mbase),
            pmin: m.get("Pmin_uc").and_then(|v| parse_real(v)).unwrap_or(0.0),
            qmax: m.get("q_max").and_then(|v| parse_real(v)).unwrap_or(mbase),
            qmin: m.get("q_min").and_then(|v| parse_real(v)).unwrap_or(-mbase),
            vg: m.get("usetp").and_then(|v| parse_real(v)).unwrap_or(1.0),
            mbase,
            in_service,
            cost: None,
            caps: GenCaps::default(),
        });
    }
    (generators, ref_buses, pv_buses)
}

/// Network name and base frequency from `$$ElmNet` (the grid container), falling
/// back to the file stem and 50 Hz.
fn grid_header(tables: &HashMap<String, Table>, name_hint: Option<&str>) -> (String, f64) {
    let mut name = None;
    let mut freq = None;
    if let Some(table) = tables.get("ElmNet") {
        if let Some(row) = table.rows.first() {
            let m = row_map(&table.cols, row);
            name = m
                .get("loc_name")
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            freq = m.get("frnom").and_then(|v| parse_real(v));
        }
    }
    let name = name
        .or_else(|| name_hint.map(str::to_owned))
        .unwrap_or_else(|| "case".to_string());
    (name, freq.unwrap_or(DEFAULT_FREQ_HZ))
}

struct Term {
    id: String,
    name: String,
    base_kv: f64,
}

fn collect_terms(table: &Table) -> Result<Vec<Term>> {
    let mut terms = Vec::with_capacity(table.rows.len());
    for row in &table.rows {
        let m = row_map(&table.cols, row);
        let Some(id) = row_id(&m) else { continue };
        terms.push(Term {
            id,
            name: m.get("loc_name").map_or("", |s| s.trim()).to_string(),
            base_kv: m.get("uknom").and_then(|v| parse_real(v)).unwrap_or(0.0),
        });
    }
    if terms.is_empty() {
        return Err(Error::FormatRead {
            format: FMT,
            message: "the $$ElmTerm table has no bus rows".into(),
        });
    }
    Ok(terms)
}

/// Bus ids from the terminal labels when every label carries a distinct integer
/// (the standard `Bus 01` / `1 RIVERSDE` case), else a dense `1..=n` fallback.
fn assign_bus_ids(terms: &[Term]) -> Vec<usize> {
    let nums: Vec<Option<usize>> = terms.iter().map(|t| first_uint(&t.name)).collect();
    let all_present = nums.iter().all(Option::is_some);
    let distinct = nums.iter().flatten().collect::<HashSet<_>>().len() == nums.len();
    if all_present && distinct {
        nums.into_iter().flatten().collect()
    } else {
        (1..=terms.len()).collect()
    }
}

/// element id -> attached `(bus, side)` cubicles, from `$$StaCubic`. A cubicle's
/// `fold_id` is the owning `ElmTerm` (the bus); `obj_id` is the connected
/// element; `obj_bus` is the terminal index (0 = from / HV, 1 = to / LV).
fn collect_cubicles(
    tables: &HashMap<String, Table>,
    bus_of_term: &HashMap<String, BusId>,
) -> HashMap<String, Vec<(BusId, i64)>> {
    let mut out: HashMap<String, Vec<(BusId, i64)>> = HashMap::new();
    if let Some(table) = tables.get("StaCubic") {
        for row in &table.rows {
            let m = row_map(&table.cols, row);
            let (Some(term), Some(elem)) = (m.get("fold_id"), m.get("obj_id")) else {
                continue;
            };
            let elem = elem.trim();
            if elem.is_empty() {
                continue;
            }
            let Some(&bus) = bus_of_term.get(term.trim()) else {
                continue;
            };
            let side = m.get("obj_bus").and_then(|v| parse_int(v)).unwrap_or(0);
            out.entry(elem.to_string()).or_default().push((bus, side));
        }
    }
    out
}

fn out_of_service(m: &HashMap<&str, &str>) -> bool {
    m.get("outserv").and_then(|v| parse_int(v)) == Some(1)
}

/// A synchronous machine is the system slack when it controls the reference
/// (`ip_ctrl == 1`) or is typed as a slack bus (`bustp == SL`).
fn is_slack(m: &HashMap<&str, &str>) -> bool {
    m.get("ip_ctrl").and_then(|v| parse_int(v)) == Some(1)
        || m.get("bustp")
            .is_some_and(|v| v.trim().eq_ignore_ascii_case("SL"))
}

fn line_branch(
    m: &HashMap<&str, &str>,
    typlne: &HashMap<String, HashMap<String, String>>,
    from: BusId,
    to: BusId,
    base_kv: f64,
    base_mva: f64,
    freq: f64,
) -> Branch {
    let dline = m.get("dline").and_then(|v| parse_real(v)).unwrap_or(1.0);
    let parallels = m
        .get("nlnum")
        .and_then(|v| parse_real(v))
        .filter(|n| *n >= 1.0)
        .unwrap_or(1.0);
    let typ = m.get("typ_id").and_then(|t| typlne.get(*t));
    let real = |key: &str| typ.and_then(|t| t.get(key)).and_then(|v| parse_real(v));

    let zbase = super::zbase(base_kv, base_mva);
    let r_ohm = real("rline").unwrap_or(0.0) * dline / parallels;
    let x_ohm = real("xline").unwrap_or(0.0) * dline / parallels;
    // Charging: prefer an explicit susceptance per length (uS/km), else derive
    // from capacitance per length (uF/km) at the base frequency.
    let b_siemens = if let Some(bline) = real("bline") {
        bline * 1e-6 * dline * parallels
    } else if let Some(cline) = real("cline") {
        2.0 * std::f64::consts::PI * freq * cline * 1e-6 * dline * parallels
    } else {
        0.0
    };

    Branch {
        from,
        to,
        r: r_ohm / zbase,
        x: x_ohm / zbase,
        b: b_siemens * zbase,
        rate_a: 0.0,
        rate_b: 0.0,
        rate_c: 0.0,
        tap: 0.0,
        shift: 0.0,
        in_service: !out_of_service(m),
        angmin: -360.0,
        angmax: 360.0,
        extras: crate::network::Extras::new(),
    }
}

fn transformer_branch(
    m: &HashMap<&str, &str>,
    typtr2: &HashMap<String, HashMap<String, String>>,
    from: BusId,
    to: BusId,
    from_kv: f64,
    to_kv: f64,
    base_mva: f64,
) -> Branch {
    let typ = m.get("typ_id").and_then(|t| typtr2.get(*t));
    let real = |key: &str| typ.and_then(|t| t.get(key)).and_then(|v| parse_real(v));

    let strn = real("strn").filter(|s| *s > 0.0).unwrap_or(base_mva);
    let uktr = real("uktr").unwrap_or(0.0); // short-circuit voltage, percent
    let pcutr = real("pcutr").unwrap_or(0.0); // copper losses, kW
    // Impedance on the transformer's own MVA base, referred to the system base.
    let scale = base_mva / strn;
    let z = (uktr / 100.0) * scale;
    let r = (pcutr / 1000.0 / strn) * scale;
    let x = (z * z - r * r).max(0.0).sqrt();

    // Off-nominal turns ratio: type ratio against the bus base voltages, stepped
    // by the active tap. Held nonzero so the branch reads as a transformer.
    let utrn_h = real("utrn_h").filter(|v| *v > 0.0);
    let utrn_l = real("utrn_l").filter(|v| *v > 0.0);
    let dutap = real("dutap").unwrap_or(0.0); // tap step, percent
    let nntap = m.get("nntap").and_then(|v| parse_real(v)).unwrap_or(0.0);
    let mut tap = match (utrn_h, utrn_l) {
        (Some(uh), Some(ul)) if from_kv > 0.0 && to_kv > 0.0 => (uh / from_kv) / (ul / to_kv),
        _ => 1.0,
    };
    tap *= 1.0 + nntap * dutap / 100.0;
    if !tap.is_finite() || tap <= 0.0 {
        tap = 1.0;
    }

    Branch {
        from,
        to,
        r,
        x,
        b: 0.0,
        rate_a: 0.0,
        rate_b: 0.0,
        rate_c: 0.0,
        tap,
        shift: 0.0,
        in_service: !out_of_service(m),
        angmin: -360.0,
        angmax: 360.0,
        extras: crate::network::Extras::new(),
    }
}

/// Best-effort shunt susceptance (MVAr at 1 p.u.): a capacitor (`shtype == 2`)
/// is positive, a reactor (`shtype == 1`) negative. Exact reactive output is
/// not modeled; the value is for completeness, not parity.
fn shunt_b(m: &HashMap<&str, &str>) -> f64 {
    let q = m
        .get("qcapn")
        .or_else(|| m.get("Qact"))
        .and_then(|v| parse_real(v))
        .unwrap_or(0.0);
    match m.get("shtype").and_then(|v| parse_int(v)) {
        Some(1) => -q,
        _ => q,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::BusType;
    use std::collections::BTreeSet;

    const V5_CASE: &str = "\
$$General;ID(a:40);Descr(a:40);Val(a:40)
1;Version;5.0
$$ElmNet;ID(a:40);loc_name(a:40);fold_id(p);frnom(r)
10;Test Net;;60
$$ElmTerm;ID(a:40);loc_name(a:40);fold_id(p);uknom(r)
101;Bus 1;10;110
102;Bus 2;10;110
103;Bus 3;10;22
$$ElmLne;ID(a:40);loc_name(a:40);fold_id(p);typ_id(p);dline(r)
201;Line 1-2;10;301;10
$$TypLne;ID(a:40);loc_name(a:40);rline(r);xline(r);cline(r)
301;Type A;0.1;0.4;0.01
$$ElmTr2;ID(a:40);loc_name(a:40);typ_id(p);nntap(i)
210;Trf 2-3;310;0
$$TypTr2;ID(a:40);loc_name(a:40);strn(r);utrn_h(r);utrn_l(r);uktr(r);pcutr(r)
310;TrType;100;110;22;10;50
$$ElmSym;ID(a:40);loc_name(a:40);pgini(r);qgini(r);usetp(r);ip_ctrl(i)
401;Gen 1;100;10;1.02;1
$$ElmLod;ID(a:40);loc_name(a:40);plini(r);qlini(r)
501;Load 3;50;20
$$StaCubic;ID(a:40);loc_name(a:40);fold_id(p);obj_bus(i);obj_id(p)
601;c1;101;0;201
602;c2;102;1;201
603;c3;102;0;210
604;c4;103;1;210
605;c5;101;0;401
606;c6;103;0;501
";

    #[test]
    fn v5_integer_ids_resolve_topology_and_slack() {
        let net = parse_dgs(V5_CASE).unwrap();
        assert_eq!(net.buses.len(), 3);
        let ids: BTreeSet<usize> = net.buses.iter().map(|b| b.id.0).collect();
        assert_eq!(ids, BTreeSet::from([1, 2, 3]));

        assert_eq!(net.branches.len(), 2);
        let endpoints: BTreeSet<(usize, usize)> = net
            .branches
            .iter()
            .map(|b| (b.from.0.min(b.to.0), b.from.0.max(b.to.0)))
            .collect();
        assert_eq!(endpoints, BTreeSet::from([(1, 2), (2, 3)]));
        assert_eq!(net.branches.iter().filter(|b| b.tap != 0.0).count(), 1);

        assert_eq!(net.generators.len(), 1);
        assert_eq!(net.generators[0].bus, BusId(1));
        assert_eq!(net.loads.len(), 1);
        assert_eq!(net.loads[0].bus, BusId(3));

        let kind = |id: usize| net.buses.iter().find(|b| b.id.0 == id).unwrap().kind;
        assert_eq!(kind(1), BusType::Ref); // slack via ip_ctrl
        assert_eq!(kind(2), BusType::Pq);
        assert_eq!(kind(3), BusType::Pq);

        // Series impedance is finite and the line per-unit value is positive.
        let line = net.branches.iter().find(|b| b.tap == 0.0).unwrap();
        assert!(line.r > 0.0 && line.x > 0.0 && line.r.is_finite());
    }

    const V7_CASE: &str = "\
$$General;ID(a:40);Descr(a:40);Val(a:40)
1;Version;7.0
$$ElmTerm;FID(a:40);OP(a:40);loc_name(a:40);uknom(r)
ElmTerm_1;C;1 ALPHA;138
ElmTerm_2;C;2 BETA;138
$$ElmLne;FID(a:40);OP(a:40);loc_name(a:40);typ_id(p);dline(r)
275;C;lne_1_2;T1;10,5
$$TypLne;FID(a:40);OP(a:40);loc_name(a:40);rline(r);xline(r)
T1;C;typ;0,1;0,4
$$StaCubic;FID(a:40);OP(a:40);loc_name(a:40);fold_id(p);obj_bus(i);obj_id(p)
480;C;c1;ElmTerm_1;0;275
613;C;c2;ElmTerm_2;1;275
";

    #[test]
    fn v7_string_fids_and_comma_decimals() {
        let net = parse_dgs(V7_CASE).unwrap();
        assert_eq!(net.buses.len(), 2);
        let ids: BTreeSet<usize> = net.buses.iter().map(|b| b.id.0).collect();
        assert_eq!(ids, BTreeSet::from([1, 2]));
        assert_eq!(net.branches.len(), 1);
        let br = &net.branches[0];
        assert_eq!((br.from.0.min(br.to.0), br.from.0.max(br.to.0)), (1, 2));
        // dline 10,5 and rline 0,1 (comma decimals) yield a finite positive r.
        assert!(br.r > 0.0 && br.r.is_finite());
    }

    #[test]
    fn comma_and_dot_decimals_parse() {
        assert_eq!(parse_real("79,67434"), Some(79.674_34));
        assert_eq!(parse_real("12.5"), Some(12.5));
        assert_eq!(parse_real(""), None);
    }

    #[test]
    fn quoted_field_keeps_embedded_semicolon() {
        assert_eq!(
            split_quote_aware("a;\"b;c\";d"),
            vec!["a".to_string(), "b;c".to_string(), "d".to_string()]
        );
    }

    #[test]
    fn vector_columns_keep_following_fields_aligned() {
        // A vector column (count + cells) precedes uknom; reading it by declared
        // capacity instead of the row's actual count would mis-read base_kv.
        let case = "\
$$ElmTerm;ID(a:40);loc_name(a:40);vec:SIZEROW(i);vec:0(r);vec:1(r);uknom(r)
1;Bus 9;2;1.1;2.2;138
";
        let net = parse_dgs(case).unwrap();
        assert_eq!(net.buses.len(), 1);
        assert_eq!(net.buses[0].id, BusId(9));
        assert!((net.buses[0].base_kv - 138.0).abs() < 1e-9);
    }

    #[test]
    fn missing_bus_table_is_rejected() {
        let err = parse_dgs("$$General;ID(a:40);Descr(a:40);Val(a:40)\n1;Version;5.0\n")
            .unwrap_err()
            .to_string();
        assert!(err.contains("PowerFactory DGS"), "{err}");
    }
}
