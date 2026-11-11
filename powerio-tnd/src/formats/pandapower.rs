//! pandapower `pandapowerNet` JSON across both electrical models.
//!
//! One token, `pandapower-json`, names the format. A document whose
//! `asymmetric_load`, `asymmetric_sgen` or `res_bus_3ph` table has rows is a
//! three-phase network for pandapower's `runpp_3ph`
//! ([`powerio_tx::format::is_three_phase_pandapower`]). [`parse`] reads such a
//! document into a [`MulticonductorNetwork`] here and hands any other one to
//! the balanced reader in `powerio-tx`. The `powerio` facade routes the token
//! here.
//!
//! The multiconductor reading follows the `runpp_3ph` model of pandapower 3.5.
//! Every bus has the phase terminals 1, 2 and 3 and an ideally earthed
//! neutral, the grounded terminal 4. Lines turn their sequence data into
//! phase matrices. The reader reports every table and row it does not map.
//! Some of them act in `runpp_3ph`, such as `ward` and `impedance`.

use std::collections::{HashMap, HashSet};
use std::f64::consts::PI;

use powerio_core::{Diagnostic, DiagnosticInfo, Error, FormatId, PioModule, Source};
use powerio_dist::diagnostics::codes;
use powerio_dist::{
    ConductorMatrix, Configuration, CoordinateSpace, DistBus, DistCoordsKind, DistGenerator,
    DistGeoMeta, DistLine, DistLineCode, DistLoad, DistLoadVoltageModel, DistLocation,
    DistSourceFormat, Extras, MulticonductorNetwork, VoltageSource,
};
use powerio_tx::__internal::{DataFrame, MAX_I_KA, Row, network_object, read_bus_geo, read_frame};
use powerio_tx::BalancedNetwork;
use serde_json::{Map, Value};

/// The format token both profiles share.
pub const TOKEN: &str = "pandapower-json";

const PHASES: [&str; 3] = ["1", "2", "3"];
const WYE: [&str; 4] = ["1", "2", "3", "4"];
const EARTH: &str = "4";
/// The element tables the reader maps. Every other non-empty table is
/// reported.
const MAPPED: [&str; 7] = [
    "bus",
    "line",
    "ext_grid",
    "load",
    "asymmetric_load",
    "sgen",
    "asymmetric_sgen",
];

type Result<T> = std::result::Result<T, powerio_tx::Error>;

/// A parsed pandapower document: the module of whichever network kind its
/// tables select.
#[derive(Debug)]
pub enum PandapowerModule {
    /// A balanced network, read by `powerio-tx`.
    Balanced(PioModule<BalancedNetwork>),
    /// A three-phase network, read here.
    Multiconductor(PioModule<MulticonductorNetwork>),
}

/// Parse a pandapower document into the module its tables select. The
/// source is declared `pandapower-json` on the way in. The module retains it
/// for the byte exact echo on a same format write.
///
/// # Errors
/// The source cannot be read, or the selected reader refuses it, with that
/// reader's findings and the source attached.
pub fn parse(source: Source) -> std::result::Result<PandapowerModule, Error> {
    let source = source.with_format(FormatId::new(TOKEN)?);
    let three_phase = source.primary_buffer().is_ok_and(|buffer| {
        std::str::from_utf8(buffer.content_bytes())
            .is_ok_and(powerio_tx::format::is_three_phase_pandapower)
    });
    if three_phase {
        parse_multiconductor(source).map(PandapowerModule::Multiconductor)
    } else {
        powerio_tx::parse(source).map(PandapowerModule::Balanced)
    }
}

fn parse_multiconductor(
    source: Source,
) -> std::result::Result<PioModule<MulticonductorNetwork>, Error> {
    let buffer = source.primary_buffer()?;
    let name_hint = std::path::Path::new(source.name())
        .file_stem()
        .and_then(|stem| stem.to_str());
    let mut reader = Reader::default();
    let read = std::str::from_utf8(buffer.content_bytes())
        .map_err(|e| bad(format!("not valid UTF-8: {e}")))
        .and_then(|text| reader.read(text, name_hint));
    match read {
        Ok(()) => PioModule::parsed(reader.net, source, reader.diagnostics),
        Err(error) => Err(
            Error::new(&codes::PARSE_DIST_SOURCE_MALFORMED, error.to_string())
                .with_diagnostics(reader.diagnostics)
                .with_cause(error)
                .with_source(source),
        ),
    }
}

#[derive(Default)]
struct Reader {
    net: MulticonductorNetwork,
    diagnostics: Vec<Diagnostic>,
    /// The bus id and `vn_kv` of each pandapower bus index.
    buses: HashMap<usize, (String, f64)>,
}

impl Reader {
    fn read(&mut self, text: &str, name_hint: Option<&str>) -> Result<()> {
        let obj = network_object(text)?;
        let f_hz = positive(&obj, "f_hz", 50.0)?;
        let name = obj
            .get("name")
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty())
            .or(name_hint)
            .unwrap_or("case");
        *self.net.name_mut() = Some(name.to_string());
        *self.net.base_frequency_mut() = f_hz;
        *self.net.source_format_mut() = Some(DistSourceFormat::PandapowerJson);
        self.net
            .extras_mut()
            .insert("sn_mva".into(), positive(&obj, "sn_mva", 1.0)?.into());
        self.read_buses(&obj)?;
        self.read_lines(&obj, f_hz)?;
        self.read_ext_grids(&obj)?;
        for (table, asymmetric, generation) in [
            ("load", false, false),
            ("asymmetric_load", true, false),
            ("sgen", false, true),
            ("asymmetric_sgen", true, true),
        ] {
            self.read_injections(&obj, table, asymmetric, generation)?;
        }
        self.report_other_tables(&obj)
    }

    fn note(&mut self, info: &'static DiagnosticInfo, message: impl Into<String>) {
        self.diagnostics.push(Diagnostic::of(info, message));
    }

    /// Refuses an in-service element that cannot be mapped. An out-of-service
    /// one is reported and the caller skips it.
    fn unusable(&mut self, row: &Row<'_>, name: &str, err: powerio_tx::Error) -> Result<()> {
        if row.bool_or("in_service", true) {
            return Err(err);
        }
        let detail = match err {
            powerio_tx::Error::FormatRead { message, .. } => message,
            other => other.to_string(),
        };
        self.note(
            &codes::READ_PANDAPOWER_ELEMENT_UNSUPPORTED,
            format!(
                "{name} is out of service and is not mapped: {detail}. runpp_3ph leaves it out"
            ),
        );
        Ok(())
    }

    /// The bus id and `vn_kv` a bus reference column points to.
    fn bus(&self, table: &str, row: &Row<'_>, key: &str) -> Result<(String, f64)> {
        let index = index_cell(table, row, key)?;
        self.buses.get(&index).cloned().ok_or_else(|| {
            bad(format!(
                "`{table}` row {}: `{key}` points to unknown bus {index}",
                row.label()
            ))
        })
    }

    /// The row's columns outside `known` as extras. An out-of-service
    /// element keeps `in_service` false there, as the PMD reader keeps a
    /// status.
    fn extras(&mut self, row: &Row<'_>, what: &str, known: &[&str]) -> Extras {
        let mut extras = row.extras_excluding(known);
        extras.remove("in_service");
        if !row.bool_or("in_service", true) {
            extras.insert("in_service".into(), false.into());
            self.note(
                &codes::READ_PANDAPOWER_OUT_OF_SERVICE,
                format!(
                    "{what} is out of service, which runpp_3ph leaves out. It is kept with \
                     `in_service` false in extras. Other formats emit it in service"
                ),
            );
        }
        extras
    }

    fn read_buses(&mut self, obj: &Map<String, Value>) -> Result<()> {
        let frame = read_frame(obj, "bus")?.ok_or_else(|| bad("missing `bus` table"))?;
        let mut geographic = None;
        for row in frame.rows() {
            let index = row.index_usize()?;
            let id = index.to_string();
            let vn_kv = req_positive(&row, "bus", "vn_kv")?;
            let mut bus = DistBus::new(id.clone(), terminals(&WYE));
            bus.grounded = terminals(&[EARTH]);
            // A GeoJSON Point becomes the location. Any other `geo` cell
            // stays in extras.
            let location = read_bus_geo(row.get("geo"));
            let known: &[&str] = if location.is_some() { &["geo"] } else { &[] };
            bus.extras = self.extras(&row, &format!("bus {id}"), known);
            if let Some(location) = location {
                let inside = location.x.abs() <= 180.0 && location.y.abs() <= 90.0;
                geographic = Some(geographic.unwrap_or(true) && inside);
                bus.location = Some(DistLocation {
                    x: location.x,
                    y: location.y,
                    kind: None,
                });
            }
            if self.buses.insert(index, (id, vn_kv)).is_some() {
                return Err(bad(format!("`bus` table: duplicate index {index}")));
            }
            self.net.buses_mut().push(bus);
        }
        if let Some(geographic) = geographic {
            let space = if geographic {
                CoordinateSpace::Geographic { crs: None }
            } else {
                CoordinateSpace::Unknown
            };
            *self.net.geo_mut() = Some(DistGeoMeta {
                space,
                kind: Some(DistCoordsKind::Source),
            });
        }
        Ok(())
    }

    #[allow(clippy::too_many_lines)] // one block per mapped line property
    fn read_lines(&mut self, obj: &Map<String, Value>, f_hz: f64) -> Result<()> {
        const SEQUENCE: [&str; 6] = [
            "r_ohm_per_km",
            "x_ohm_per_km",
            "c_nf_per_km",
            "r0_ohm_per_km",
            "x0_ohm_per_km",
            "c0_nf_per_km",
        ];
        let Some(frame) = read_frame(obj, "line")? else {
            return Ok(());
        };
        unique_indices(&frame, "line")?;
        let mut linecodes = HashMap::new();
        let mut no_zero_sequence = 0;
        for row in frame.rows() {
            let index = row.index_usize()?;
            let name = format!("line_{index}");
            let (from, _) = self.bus("line", &row, "from_bus")?;
            let (to, _) = self.bus("line", &row, "to_bus")?;
            let mut values = [0.0; 6];
            let mut stated = true;
            for (k, key) in SEQUENCE.iter().enumerate() {
                values[k] = match row.f_finite(key) {
                    Some(value) => value,
                    // A network built for runpp alone may lack the zero sequence.
                    None if k >= 3 => {
                        stated = false;
                        values[k - 3]
                    }
                    None => row.req_f(key)?,
                };
            }
            let [r1, x1, c1, r0, x0, c0] = values;
            // pandapower reads no zero-sequence conductance.
            let g1 = row.f_or("g_us_per_km", 0.0);
            let parallel = match parallel(&row, "line") {
                Ok(parallel) => parallel,
                Err(e) => {
                    self.unusable(&row, &name, e)?;
                    continue;
                }
            };
            no_zero_sequence += usize::from(!stated);
            let key = [r1, x1, c1, r0, x0, c0, g1, parallel].map(f64::to_bits);
            let linecode = if let Some(code) = linecodes.get(&key) {
                String::clone(code)
            } else {
                // Ohm, nF and uS per km of one device, to the dist model's
                // per meter matrices with the shunt split into two halves.
                let code_name = format!("linecode_{}", linecodes.len());
                let mut code = DistLineCode::new(
                    code_name.clone(),
                    fortescue(r0, r1, 1e-3 / parallel),
                    fortescue(x0, x1, 1e-3 / parallel),
                );
                code.b_from = fortescue(c0, c1, 2.0 * PI * f_hz * 1e-12 * parallel / 2.0);
                code.b_to.clone_from(&code.b_from);
                code.g_from = fortescue(0.0, g1, 1e-9 * parallel / 2.0);
                code.g_to.clone_from(&code.g_from);
                let all_finite = [&code.r_series, &code.x_series, &code.b_from, &code.g_from]
                    .iter()
                    .all(|matrix| matrix.iter().flatten().all(|v| v.is_finite()));
                if !all_finite {
                    return Err(overflow(&row, "line", "parallel"));
                }
                self.net.line_codes_mut().push(code);
                linecodes.insert(key, code_name.clone());
                code_name
            };
            let length = req_positive(&row, "line", "length_km")? * 1e3;
            let mut line = DistLine::new(
                name.clone(),
                from,
                to,
                terminals(&PHASES),
                terminals(&PHASES),
                linecode,
                finite(&row, "line", "length_km", length)?,
            );
            // pandapower states no limit as 0 or as 99999 kA. The balanced
            // reader reads both as unbounded. It refuses a negative rating.
            line.i_max = match row.f_finite("max_i_ka") {
                Some(i_ka) if i_ka < 0.0 => {
                    return Err(bad(format!(
                        "`line` row {}: `max_i_ka` is negative ({i_ka})",
                        row.label()
                    )));
                }
                Some(i_ka) if i_ka != 0.0 && i_ka < MAX_I_KA => {
                    Some(vec![
                        finite(&row, "line", "max_i_ka", i_ka * 1e3 * parallel)?;
                        3
                    ])
                }
                _ => None,
            };
            let mut known = vec!["from_bus", "to_bus", "length_km", "g_us_per_km"];
            known.extend(SEQUENCE);
            known.extend(["max_i_ka", "parallel"]);
            line.extras = self.extras(&row, &name, &known);
            self.net.lines_mut().push(line);
        }
        if no_zero_sequence > 0 {
            self.note(
                &codes::READ_PANDAPOWER_APPROXIMATED,
                format!(
                    "{no_zero_sequence} line(s) lack zero-sequence data, which runpp_3ph needs. \
                     Their zero sequence is taken equal to the positive sequence"
                ),
            );
        }
        Ok(())
    }

    fn read_ext_grids(&mut self, obj: &Map<String, Value>) -> Result<()> {
        let Some(frame) = read_frame(obj, "ext_grid")? else {
            return Ok(());
        };
        unique_indices(&frame, "ext_grid")?;
        for row in frame.rows() {
            let index = row.index_usize()?;
            let name = format!("ext_grid_{index}");
            let (bus, vn_kv) = self.bus("ext_grid", &row, "bus")?;
            let vm_pu = match positive_or(&row, "ext_grid", "vm_pu", 1.0) {
                Ok(vm_pu) => vm_pu,
                Err(e) => {
                    self.unusable(&row, &name, e)?;
                    continue;
                }
            };
            let v = finite(&row, "ext_grid", "vm_pu", vm_pu * vn_kv * 1e3 / 3f64.sqrt())?;
            let va = row.f_or("va_degree", 0.0);
            let mut source = VoltageSource::new(
                name.clone(),
                bus,
                terminals(&WYE),
                vec![v, v, v, 0.0],
                [va, va - 120.0, va + 120.0, 0.0]
                    .map(f64::to_radians)
                    .to_vec(),
            );
            source.extras = self.extras(&row, &name, &["bus", "vm_pu", "va_degree"]);
            self.note(
                &codes::READ_PANDAPOWER_APPROXIMATED,
                format!(
                    "{name}: an ideal source fixes all three sequence voltages. runpp_3ph fixes \
                     only the positive sequence and puts the short-circuit impedance in the \
                     negative and zero sequence. The short-circuit columns stay in extras"
                ),
            );
            self.net.sources_mut().push(source);
        }
        Ok(())
    }

    /// Loads and static generators, balanced (P/3 and Q/3 per phase) or per
    /// phase, all at constant power as `runpp_3ph` solves them.
    fn read_injections(
        &mut self,
        obj: &Map<String, Value>,
        table: &str,
        asymmetric: bool,
        generation: bool,
    ) -> Result<()> {
        const ZIP: [&str; 6] = [
            "const_z_percent",
            "const_i_percent",
            "const_z_p_percent",
            "const_i_p_percent",
            "const_z_q_percent",
            "const_i_q_percent",
        ];
        let Some(frame) = read_frame(obj, table)? else {
            return Ok(());
        };
        unique_indices(&frame, table)?;
        for row in frame.rows() {
            let index = row.index_usize()?;
            let name = format!("{table}_{index}");
            let (bus, vn_kv) = self.bus(table, &row, "bus")?;
            let kind = match row.get("type") {
                None if !generation => Some("wye"),
                cell => cell.and_then(Value::as_str),
            };
            let (configuration, map, v_nom) = match kind {
                Some("wye") => (
                    Configuration::Wye,
                    terminals(&WYE),
                    vn_kv * 1e3 / 3f64.sqrt(),
                ),
                Some("delta") => (Configuration::Delta, terminals(&PHASES), vn_kv * 1e3),
                _ => {
                    let reason = match row.get("type") {
                        None | Some(Value::Null) => "has no type".to_owned(),
                        Some(Value::String(s)) => {
                            format!("has type `{s}`, which is neither wye nor delta")
                        }
                        Some(v) => format!("has type `{v}`, which is neither wye nor delta"),
                    };
                    self.note(
                        &codes::READ_PANDAPOWER_ELEMENT_UNSUPPORTED,
                        format!("{name}: {reason}. runpp_3ph leaves it out. It is not mapped"),
                    );
                    continue;
                }
            };
            // MW and Mvar to W and var per phase. A delta row puts p_a on
            // the 1-2 branch, p_b on 2-3 and p_c on 3-1.
            let scaling = finite_or(&row, table, "scaling", 1.0)? * 1e6;
            let (p, q) = if asymmetric {
                let mut p = vec![0.0; 3];
                let mut q = vec![0.0; 3];
                for (k, phase) in ["a", "b", "c"].iter().enumerate() {
                    let p_key = format!("p_{phase}_mw");
                    let q_key = format!("q_{phase}_mvar");
                    p[k] = finite(&row, table, &p_key, row.req_f(&p_key)? * scaling)?;
                    q[k] = finite(&row, table, &q_key, row.f_or(&q_key, 0.0) * scaling)?;
                }
                (p, q)
            } else {
                let p = row.req_f("p_mw")? * scaling / 3.0;
                let q = row.f_or("q_mvar", 0.0) * scaling / 3.0;
                (
                    vec![finite(&row, table, "p_mw", p)?; 3],
                    vec![finite(&row, table, "q_mvar", q)?; 3],
                )
            };
            let extras = self.extras(
                &row,
                &name,
                &[
                    "bus", "type", "scaling", "p_mw", "q_mvar", "p_a_mw", "q_a_mvar", "p_b_mw",
                    "q_b_mvar", "p_c_mw", "q_c_mvar",
                ],
            );
            if ZIP.iter().any(|key| row.f_or(key, 0.0) != 0.0) {
                self.note(
                    &codes::READ_PANDAPOWER_RETAINED_SOURCE_ONLY,
                    format!(
                        "{name}: runpp_3ph solves every load or static generator at constant \
                         power. The \
                         constant impedance and current shares stay in extras"
                    ),
                );
            }
            if generation {
                let mut generator = DistGenerator::new(name, bus, map, configuration, p, q);
                generator.extras = extras;
                self.net.generators_mut().push(generator);
            } else {
                let mut load = DistLoad::new(name, bus, map, configuration, p, q);
                load.voltage_model = DistLoadVoltageModel::ConstantPower {
                    v_nom: vec![v_nom; 3],
                };
                load.extras = extras;
                self.net.loads_mut().push(load);
            }
        }
        Ok(())
    }

    /// Every non-empty table the reader does not map, with one finding per
    /// element table. All result tables share one remark.
    fn report_other_tables(&mut self, obj: &Map<String, Value>) -> Result<()> {
        let mut results = Vec::new();
        for (key, value) in obj {
            if MAPPED.contains(&key.as_str())
                || value.get("_class").and_then(Value::as_str) != Some("DataFrame")
            {
                continue;
            }
            // pandapower writes a result table without columns with an index
            // but no data rows, which the codec refuses. It holds nothing.
            if key.starts_with("res_") {
                if let Ok(Some(frame)) = read_frame(obj, key)
                    && frame.rows().next().is_some()
                {
                    results.push(key.as_str());
                }
                continue;
            }
            let rows = read_frame(obj, key)?.map_or(0, |frame| frame.rows().count());
            if rows > 0 {
                self.note(
                    &codes::READ_PANDAPOWER_TABLE_UNSUPPORTED,
                    format!("`{key}` table ignored ({rows} rows): it has no three-phase mapping"),
                );
            }
        }
        if !results.is_empty() {
            self.note(
                &codes::READ_PANDAPOWER_RETAINED_SOURCE_ONLY,
                format!(
                    "result tables {} remain only in the retained source",
                    results.join(", ")
                ),
            );
        }
        Ok(())
    }
}

/// The phase matrix of a symmetric element from its zero and positive
/// sequence values, times `scale`: self `(z0 + 2 z1)/3`, mutual `(z0 - z1)/3`.
fn fortescue(zero: f64, positive: f64, scale: f64) -> ConductorMatrix {
    let mutual = (zero - positive) / 3.0 * scale;
    let mut matrix = vec![vec![mutual; 3]; 3];
    for (i, row) in matrix.iter_mut().enumerate() {
        row[i] = (zero + 2.0 * positive) / 3.0 * scale;
    }
    matrix
}

fn terminals(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).to_string()).collect()
}

/// The `parallel` column. A missing column means one device. A null or
/// nonpositive value is an error.
fn parallel(row: &Row<'_>, table: &str) -> Result<f64> {
    positive_or(row, table, "parallel", 1.0)
}

/// An element index cell: a non-negative integer, possibly float encoded.
fn index_cell(table: &str, row: &Row<'_>, key: &str) -> Result<usize> {
    row.get(key)
        .and_then(Value::as_u64)
        .and_then(|u| usize::try_from(u).ok())
        .or_else(|| {
            row.f_finite(key)
                .filter(|v| *v >= 0.0 && v.fract() == 0.0)
                .map(|v| v as usize)
        })
        .ok_or_else(|| {
            bad(format!(
                "`{table}` row {}: `{key}` is not a non-negative integer",
                row.label()
            ))
        })
}

/// Refuses a table whose pandas index repeats, as two elements would share
/// one name.
fn unique_indices(frame: &DataFrame, table: &str) -> Result<()> {
    let mut seen = HashSet::new();
    for row in frame.rows() {
        let index = row.index_usize()?;
        if !seen.insert(index) {
            return Err(bad(format!("`{table}` table: duplicate index {index}")));
        }
    }
    Ok(())
}

/// An optional column that must hold a positive number when present. A
/// missing column takes the pandapower default. A null or nonpositive cell
/// is an error.
fn positive_or(row: &Row<'_>, table: &str, key: &str, default: f64) -> Result<f64> {
    match row.get(key) {
        None => Ok(default),
        Some(_) => req_positive(row, table, key),
    }
}

/// An optional numeric column. A missing column takes the pandapower
/// default. A null or non-numeric cell is an error.
fn finite_or(row: &Row<'_>, table: &str, key: &str, default: f64) -> Result<f64> {
    match row.get(key) {
        None => Ok(default),
        Some(_) => row.req_f(key).map_err(|_| {
            bad(format!(
                "`{table}` row {}: `{key}` is not a number",
                row.label()
            ))
        }),
    }
}

/// A value scaled from `key` that must stay finite.
fn finite(row: &Row<'_>, table: &str, key: &str, value: f64) -> Result<f64> {
    if value.is_finite() {
        Ok(value)
    } else {
        Err(overflow(row, table, key))
    }
}

fn overflow(row: &Row<'_>, table: &str, key: &str) -> powerio_tx::Error {
    bad(format!(
        "`{table}` row {}: `{key}` overflows when scaled to SI units",
        row.label()
    ))
}

/// A required column that must hold a positive number.
fn req_positive(row: &Row<'_>, table: &str, key: &str) -> Result<f64> {
    let value = row.req_f(key)?;
    if value > 0.0 {
        Ok(value)
    } else {
        Err(bad(format!(
            "`{table}` row {}: `{key}` is not a positive number ({value})",
            row.label()
        )))
    }
}

/// A network level number: absent takes the pandapower default, anything
/// but a positive number is an error.
fn positive(obj: &Map<String, Value>, key: &str, default: f64) -> Result<f64> {
    obj.get(key).map_or(Ok(default), |value| {
        value
            .as_f64()
            .filter(|v| v.is_finite() && *v > 0.0)
            .ok_or_else(|| bad(format!("`{key}` is not a positive number (`{value}`)")))
    })
}

fn bad(message: impl Into<String>) -> powerio_tx::Error {
    powerio_tx::Error::FormatRead {
        format: "pandapower JSON",
        message: message.into(),
    }
}

#[cfg(test)]
// Exact compares where no arithmetic intervenes. Helpers take `Value` by value
// for `json!` call site ergonomics.
#[allow(clippy::float_cmp, clippy::needless_pass_by_value)]
mod tests {
    use super::*;
    use serde_json::json;

    fn frame(columns: &[&str], data: Value) -> Value {
        let rows = data.as_array().map_or(0, Vec::len);
        let inner =
            json!({"columns": columns, "index": (0..rows).collect::<Vec<_>>(), "data": data});
        json!({
            "_module": "pandas.core.frame", "_class": "DataFrame", "orient": "split",
            "_object": inner.to_string(), "is_multiindex": false, "is_multicolumn": false
        })
    }

    /// A frame with an explicit pandas index.
    fn frame_with_index(columns: &[&str], index: Value, data: Value) -> Value {
        let inner = json!({"columns": columns, "index": index, "data": data});
        json!({
            "_module": "pandas.core.frame", "_class": "DataFrame", "orient": "split",
            "_object": inner.to_string(), "is_multiindex": false, "is_multicolumn": false
        })
    }

    /// A frame from records that share their keys.
    fn table(records: Value) -> Value {
        let records = records.as_array().unwrap();
        let columns: Vec<&str> = records[0]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        let data: Vec<Vec<Value>> = records
            .iter()
            .map(|record| columns.iter().map(|c| record[*c].clone()).collect())
            .collect();
        frame(&columns, json!(data))
    }

    /// Buses 0 to 2 at 0.4 kV and bus 3 at 20 kV, then `tables`.
    fn net(tables: Vec<(&str, Value)>) -> String {
        let mut object = Map::new();
        object.insert("f_hz".into(), json!(50.0));
        object.insert("sn_mva".into(), json!(1.0));
        object.insert(
            "bus".into(),
            frame(
                &["name", "vn_kv", "in_service"],
                json!([
                    ["a", 0.4, true],
                    ["b", 0.4, true],
                    ["c", 0.4, true],
                    ["mv", 20.0, true]
                ]),
            ),
        );
        for (name, value) in tables {
            object.insert(name.into(), value);
        }
        json!({"_module": "pandapower.auxiliary", "_class": "pandapowerNet", "_object": object})
            .to_string()
    }

    fn read(text: &str) -> PioModule<MulticonductorNetwork> {
        parse_multiconductor(Source::from_memory("case.json", text.as_bytes().to_vec()).unwrap())
            .unwrap()
    }

    fn codes(module: &PioModule<MulticonductorNetwork>) -> Vec<&str> {
        module.diagnostics().iter().map(Diagnostic::code).collect()
    }

    fn close(a: f64, b: f64) {
        assert!((a - b).abs() <= 1e-9 * a.abs().max(b.abs()), "{a} != {b}");
    }

    fn fixture(path: &str) -> Source {
        Source::open(format!(
            "{}/../tests/data/{path}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
    }

    #[test]
    fn the_detector_reads_the_three_phase_tables() {
        let detect = powerio_tx::format::is_three_phase_pandapower;
        let load = table(json!([{"bus": 0, "p_a_mw": 0.01, "p_b_mw": 0.0, "p_c_mw": 0.0}]));
        assert!(detect(&net(vec![("asymmetric_load", load.clone())])));
        assert!(detect(&net(vec![("asymmetric_sgen", load)])));
        assert!(detect(&net(vec![(
            "res_bus_3ph",
            table(json!([{"vm_a_pu": 1.0}]))
        )])));
        let empty = frame(&["bus", "p_a_mw"], json!([]));
        assert!(!detect(&net(vec![
            ("asymmetric_load", empty.clone()),
            ("asymmetric_sgen", empty.clone()),
            ("res_bus_3ph", empty),
        ])));
        assert!(!detect(&net(vec![(
            "res_bus",
            table(json!([{"vm_pu": 1.0}]))
        )])));
        assert!(!detect("{}"));
    }

    #[test]
    fn parse_routes_by_the_tables_the_document_fills() {
        let module = parse(fixture("dist/pandapower/tiny_3ph.json")).unwrap();
        assert!(matches!(module, PandapowerModule::Multiconductor(_)));
        let module = parse(fixture("pandapower/example.json")).unwrap();
        let PandapowerModule::Balanced(module) = module else {
            panic!("a balanced document reads balanced");
        };
        assert_eq!(module.value().buses().len(), 7);
        assert_eq!(
            module
                .source()
                .and_then(Source::format)
                .map(FormatId::as_str),
            Some(TOKEN)
        );
    }

    #[test]
    fn fortescue_gives_the_self_and_mutual_terms() {
        assert_eq!(
            fortescue(4.0, 1.0, 3.0),
            vec![
                vec![6.0, 3.0, 3.0],
                vec![3.0, 6.0, 3.0],
                vec![3.0, 3.0, 6.0]
            ]
        );
    }

    #[test]
    fn buses_get_three_phases_and_a_grounded_neutral() {
        let module = read(&net(vec![]));
        let network = module.value();
        assert_eq!(network.name().as_deref(), Some("case"));
        assert_eq!(network.base_frequency(), 50.0);
        assert_eq!(
            *network.source_format(),
            Some(DistSourceFormat::PandapowerJson)
        );
        assert_eq!(network.extras()["sn_mva"], json!(1.0));
        let bus = &network.buses()[3];
        assert_eq!(bus.id, "3");
        assert_eq!(bus.terminals, terminals(&WYE));
        assert_eq!(bus.grounded, vec!["4"]);
        assert_eq!(bus.extras["vn_kv"], json!(20.0));
        assert_eq!(bus.extras["name"], json!("mv"));
        assert!(!bus.extras.contains_key("in_service"));
        assert!(codes(&module).is_empty(), "{:?}", codes(&module));
    }

    #[test]
    fn a_geojson_point_becomes_the_bus_location() {
        let buses = frame(
            &["vn_kv", "geo"],
            json!([
                [0.4, null],
                [0.4, r#"{"type": "Point", "coordinates": [11.5, 48.1]}"#],
                [
                    0.4,
                    r#"{"type": "LineString", "coordinates": [[0, 0], [1, 1]]}"#
                ]
            ]),
        );
        let module = read(&net(vec![("bus", buses)]));
        let network = module.value();
        let located = &network.buses()[1];
        assert_eq!(
            located.location,
            Some(DistLocation {
                x: 11.5,
                y: 48.1,
                kind: None
            })
        );
        assert!(!located.extras.contains_key("geo"));
        let line = &network.buses()[2];
        assert_eq!(line.location, None);
        assert!(line.extras.contains_key("geo"));
        assert_eq!(
            network.geo().as_ref().map(|geo| &geo.space),
            Some(&CoordinateSpace::Geographic { crs: None })
        );
        assert!(read(&net(vec![])).value().geo().is_none());
    }

    #[test]
    fn the_network_name_and_frequency_are_read() {
        // Absent keys take the pandapower defaults.
        let mut bare: Value = serde_json::from_str(&net(vec![])).unwrap();
        let object = bare["_object"].as_object_mut().unwrap();
        object.remove("f_hz");
        object.remove("sn_mva");
        let module = read(&bare.to_string());
        assert_eq!(module.value().base_frequency(), 50.0);
        assert_eq!(module.value().extras()["sn_mva"], json!(1.0));
        let line = table(json!([{
            "from_bus": 0, "to_bus": 1, "length_km": 0.1, "r_ohm_per_km": 0.2,
            "x_ohm_per_km": 0.08, "c_nf_per_km": 250.0, "r0_ohm_per_km": 0.8,
            "x0_ohm_per_km": 0.32, "c0_nf_per_km": 100.0
        }]));
        let mut text: Value = serde_json::from_str(&net(vec![("line", line)])).unwrap();
        text["_object"]["f_hz"] = json!(60.0);
        let module = parse_multiconductor(
            Source::from_memory("feeder.json", text.to_string().into_bytes()).unwrap(),
        )
        .unwrap();
        let network = module.value();
        assert_eq!(network.name().as_deref(), Some("feeder"));
        assert_eq!(network.base_frequency(), 60.0);
        let omega = 2.0 * PI * 60.0;
        close(
            network.line_codes()[0].b_from[0][0],
            omega * (100.0 + 500.0) / 3.0 * 1e-12 / 2.0,
        );
        text["_object"]["name"] = json!("lv grid");
        assert_eq!(
            read(&text.to_string()).value().name().as_deref(),
            Some("lv grid")
        );
        text["_object"]["f_hz"] = json!(0.0);
        assert!(
            parse_multiconductor(
                Source::from_memory("case.json", text.to_string().into_bytes()).unwrap()
            )
            .is_err()
        );
    }

    #[test]
    fn an_out_of_service_element_keeps_its_status_in_extras() {
        let bus = table(json!([{"vn_kv": 0.4, "in_service": false}]));
        let module = read(&net(vec![("bus", bus)]));
        assert_eq!(module.value().buses()[0].extras["in_service"], json!(false));
        assert_eq!(codes(&module), vec!["READ.DIST.PANDAPOWER_OUT_OF_SERVICE"]);
    }

    fn one_line() -> Value {
        table(json!([{
            "from_bus": 0, "to_bus": 1, "length_km": 0.1, "r_ohm_per_km": 0.2,
            "x_ohm_per_km": 0.08, "c_nf_per_km": 0.0, "r0_ohm_per_km": 0.8,
            "x0_ohm_per_km": 0.32, "c0_nf_per_km": 0.0
        }]))
    }

    #[test]
    fn lines_turn_sequence_data_into_phase_matrices_per_meter() {
        let line = |from: u8, to: u8, r: f64| {
            json!({
                "from_bus": from, "to_bus": to, "length_km": 0.5, "r_ohm_per_km": r,
                "x_ohm_per_km": 0.08, "c_nf_per_km": 250.0, "g_us_per_km": 1.0,
                "r0_ohm_per_km": 0.8, "x0_ohm_per_km": 0.32, "c0_nf_per_km": 100.0,
                "max_i_ka": 0.3, "df": 0.9, "parallel": 2, "in_service": true
            })
        };
        let lines = table(json!([line(0, 1, 0.2), line(1, 2, 0.2), line(0, 2, 0.3)]));
        let module = read(&net(vec![("line", lines)]));
        let network = module.value();
        assert_eq!(network.line_codes().len(), 2);
        let names: Vec<_> = network
            .lines()
            .iter()
            .map(|l| l.linecode.as_str())
            .collect();
        assert_eq!(names, ["linecode_0", "linecode_0", "linecode_1"]);
        let code = &network.line_codes()[0];
        // Two parallel devices halve the impedance and double the shunt.
        close(code.r_series[0][0], (0.8 + 0.4) / 3.0 / 1e3 / 2.0);
        close(code.r_series[0][1], (0.8 - 0.2) / 3.0 / 1e3 / 2.0);
        close(code.x_series[1][1], (0.32 + 0.16) / 3.0 / 1e3 / 2.0);
        close(code.x_series[2][1], (0.32 - 0.08) / 3.0 / 1e3 / 2.0);
        let omega = 2.0 * PI * 50.0;
        close(code.b_from[0][0], omega * (100.0 + 500.0) / 3.0 * 1e-12);
        close(code.b_to[0][2], omega * (100.0 - 250.0) / 3.0 * 1e-12);
        close(code.g_from[1][1], 2.0 / 3.0 * 1e-9);
        close(code.g_to[1][0], -1.0 / 3.0 * 1e-9);
        let line = &network.lines()[0];
        assert_eq!((line.bus_from.as_str(), line.bus_to.as_str()), ("0", "1"));
        assert_eq!(line.terminal_map_from, terminals(&PHASES));
        close(line.length, 500.0);
        assert_eq!(line.i_max, Some(vec![600.0; 3]));
        assert_eq!(line.extras["df"], json!(0.9));
        assert!(!line.extras.contains_key("r_ohm_per_km"));
    }

    #[test]
    fn line_max_i_ka_zero_and_marker_state_no_bound() {
        let line = |max_i_ka: f64| {
            json!({
                "from_bus": 0, "to_bus": 1, "length_km": 0.1, "r_ohm_per_km": 0.2,
                "x_ohm_per_km": 0.08, "c_nf_per_km": 0.0, "r0_ohm_per_km": 0.8,
                "x0_ohm_per_km": 0.32, "c0_nf_per_km": 0.0, "max_i_ka": max_i_ka, "parallel": 1
            })
        };
        let lines = table(json!([line(0.0), line(99_999.0), line(0.3)]));
        let module = read(&net(vec![("line", lines)]));
        let limits: Vec<_> = module
            .value()
            .lines()
            .iter()
            .map(|l| l.i_max.clone())
            .collect();
        assert_eq!(limits, [None, None, Some(vec![300.0; 3])]);
    }

    #[test]
    fn a_line_without_zero_sequence_data_takes_the_positive_sequence() {
        let line = table(json!([{
            "from_bus": 0, "to_bus": 1, "length_km": 0.1, "r_ohm_per_km": 0.2,
            "x_ohm_per_km": 0.08, "c_nf_per_km": 250.0
        }]));
        let module = read(&net(vec![("line", line)]));
        let code = &module.value().line_codes()[0];
        close(code.r_series[0][0], 0.2e-3);
        assert_eq!((code.r_series[0][1], code.x_series[0][1]), (0.0, 0.0));
        assert_eq!(codes(&module), vec!["READ.DIST.PANDAPOWER_APPROXIMATED"]);
    }

    #[test]
    fn an_external_grid_is_an_ideal_earthed_source() {
        let grid = table(json!([{
            "bus": 3, "vm_pu": 1.02, "va_degree": 10.0, "s_sc_max_mva": 1000.0,
            "rx_max": 0.1, "x0x_max": 1.0, "r0x0_max": 0.1, "in_service": true
        }]));
        let module = read(&net(vec![("ext_grid", grid)]));
        let source = &module.value().sources()[0];
        assert_eq!(
            (source.name.as_str(), source.bus.as_str()),
            ("ext_grid_0", "3")
        );
        assert_eq!(source.terminal_map, terminals(&WYE));
        assert_eq!(source.reference_terminal, None);
        let v = 1.02 * 20e3 / 3f64.sqrt();
        assert_eq!(source.v_magnitude, vec![v, v, v, 0.0]);
        let angles = [10.0, -110.0, 130.0, 0.0].map(f64::to_radians);
        assert_eq!(source.v_angle, angles.to_vec());
        assert_eq!(source.extras["s_sc_max_mva"], json!(1000.0));
        assert_eq!(source.extras["x0x_max"], json!(1.0));
        assert_eq!(codes(&module), vec!["READ.DIST.PANDAPOWER_APPROXIMATED"]);
    }

    #[test]
    fn loads_are_constant_power_wye_or_delta() {
        let loads = table(json!([
            {"bus": 0, "p_mw": 0.03, "q_mvar": 0.006, "const_z_percent": 0.0, "scaling": 0.5,
             "type": "wye"},
            {"bus": 1, "p_mw": 0.03, "q_mvar": 0.006, "const_z_percent": 0.0, "scaling": 1.0,
             "type": "delta"},
            {"bus": 2, "p_mw": 0.03, "q_mvar": 0.0, "const_z_percent": 20.0, "scaling": 1.0,
             "type": "wye"}
        ]));
        let module = read(&net(vec![("load", loads)]));
        let loads = module.value().loads();
        let (wye, delta) = (&loads[0], &loads[1]);
        assert_eq!(wye.configuration, Configuration::Wye);
        assert_eq!(wye.terminal_map, terminals(&WYE));
        assert_eq!(wye.p_nom, vec![5e3; 3]);
        assert_eq!(wye.q_nom, vec![1e3; 3]);
        let wye_v = vec![400.0 / 3f64.sqrt(); 3];
        assert_eq!(
            wye.voltage_model,
            DistLoadVoltageModel::ConstantPower { v_nom: wye_v }
        );
        assert_eq!(delta.configuration, Configuration::Delta);
        assert_eq!(delta.terminal_map, terminals(&PHASES));
        assert_eq!(delta.p_nom, vec![1e4; 3]);
        assert_eq!(
            delta.voltage_model,
            DistLoadVoltageModel::ConstantPower {
                v_nom: vec![400.0; 3]
            }
        );
        assert_eq!(loads[2].extras["const_z_percent"], json!(20.0));
        assert_eq!(
            codes(&module),
            vec!["READ.DIST.PANDAPOWER_RETAINED_SOURCE_ONLY"]
        );
        // A file without a type column reads its loads as wye.
        let module = read(&net(vec![(
            "load",
            table(json!([{"bus": 0, "p_mw": 0.003}])),
        )]));
        assert_eq!(module.value().loads()[0].configuration, Configuration::Wye);
    }

    #[test]
    fn asymmetric_loads_keep_their_phase_powers() {
        let loads = table(json!([
            {"bus": 0, "p_a_mw": 0.01, "q_a_mvar": 0.002, "p_b_mw": 0.02, "q_b_mvar": 0.0,
             "p_c_mw": 0.0, "q_c_mvar": 0.0, "scaling": 2.0, "type": "wye"},
            {"bus": 1, "p_a_mw": 0.01, "q_a_mvar": 0.0, "p_b_mw": 0.0, "q_b_mvar": 0.0,
             "p_c_mw": 0.003, "q_c_mvar": 0.001, "scaling": 1.0, "type": "delta"}
        ]));
        let module = read(&net(vec![("asymmetric_load", loads)]));
        let loads = module.value().loads();
        assert_eq!(loads[0].name, "asymmetric_load_0");
        assert_eq!(loads[0].p_nom, vec![2e4, 4e4, 0.0]);
        assert_eq!(loads[0].q_nom, vec![4e3, 0.0, 0.0]);
        // p_a sits on the 1-2 branch and p_c on the 3-1 branch.
        assert_eq!(loads[1].configuration, Configuration::Delta);
        assert_eq!(loads[1].p_nom, vec![1e4, 0.0, 3e3]);
        assert_eq!(loads[1].q_nom, vec![0.0, 0.0, 1e3]);
    }

    #[test]
    fn static_generators_need_a_wye_or_delta_type() {
        let sgens = table(json!([
            {"bus": 0, "p_mw": 0.006, "q_mvar": 0.0015, "scaling": 1.0, "type": "wye"},
            {"bus": 1, "p_mw": 0.003, "q_mvar": 0.0, "scaling": 1.0, "type": null},
            {"bus": 2, "p_mw": 0.003, "q_mvar": 0.0, "scaling": 1.0, "type": "PV"}
        ]));
        let asymmetric = table(json!([{
            "bus": 2, "p_a_mw": 0.0, "q_a_mvar": 0.0, "p_b_mw": 0.004, "q_b_mvar": 0.0,
            "p_c_mw": 0.0, "q_c_mvar": 0.0, "scaling": 1.0, "type": "delta"
        }]));
        let module = read(&net(vec![("sgen", sgens), ("asymmetric_sgen", asymmetric)]));
        let generators = module.value().generators();
        assert_eq!(generators.len(), 2);
        assert_eq!(generators[0].name, "sgen_0");
        assert_eq!(generators[0].terminal_map, terminals(&WYE));
        assert_eq!(generators[0].p_nom, vec![2e3; 3]);
        assert_eq!(generators[0].q_nom, vec![500.0; 3]);
        assert_eq!(generators[1].configuration, Configuration::Delta);
        assert_eq!(generators[1].p_nom, vec![0.0, 4e3, 0.0]);
        assert_eq!(module.value().loads().len(), 0);
        assert_eq!(
            codes(&module),
            vec!["READ.DIST.PANDAPOWER_ELEMENT_UNSUPPORTED"; 2]
        );
    }

    #[test]
    fn a_static_generator_without_a_known_type_says_why() {
        let sgens = table(json!([
            {"bus": 0, "p_mw": 0.003, "type": null},
            {"bus": 1, "p_mw": 0.003, "type": 5}
        ]));
        let module = read(&net(vec![("sgen", sgens)]));
        assert_eq!(module.value().generators().len(), 0);
        let messages: Vec<_> = module
            .diagnostics()
            .iter()
            .filter(|d| d.code() == "READ.DIST.PANDAPOWER_ELEMENT_UNSUPPORTED")
            .map(Diagnostic::message)
            .collect();
        assert_eq!(messages.len(), 2);
        assert!(
            messages[0].contains("sgen_0: has no type"),
            "{}",
            messages[0]
        );
        assert!(!messages[0].contains("`null`"), "{}", messages[0]);
        assert!(messages[1].contains("has type `5`"), "{}", messages[1]);
    }

    #[test]
    fn unmapped_tables_are_reported_once_each() {
        let two = table(json!([{"hv_bus": 3}, {"hv_bus": 3}]));
        let no_columns = json!({
            "_module": "pandas.core.frame", "_class": "DataFrame", "orient": "split",
            "_object": r#"{"columns":[],"index":[0],"data":[]}"#
        });
        let module = read(&net(vec![
            ("trafo3w", two),
            ("ward", table(json!([{"bus": 0}]))),
            ("impedance", frame(&["from_bus"], json!([]))),
            ("res_bus_3ph", table(json!([{"vm_a_pu": 1.0}]))),
            ("res_line_3ph", frame(&["p_a_from_mw"], json!([]))),
            ("res_shunt_3ph", no_columns),
        ]));
        let messages: Vec<_> = module
            .diagnostics()
            .iter()
            .map(|d| (d.code(), d.message()))
            .collect();
        assert_eq!(
            messages,
            [
                (
                    "READ.DIST.PANDAPOWER_TABLE_UNSUPPORTED",
                    "`trafo3w` table ignored (2 rows): it has no three-phase mapping"
                ),
                (
                    "READ.DIST.PANDAPOWER_TABLE_UNSUPPORTED",
                    "`ward` table ignored (1 rows): it has no three-phase mapping"
                ),
                (
                    "READ.DIST.PANDAPOWER_RETAINED_SOURCE_ONLY",
                    "result tables res_bus_3ph remain only in the retained source"
                ),
            ]
        );
    }

    #[test]
    fn tables_without_a_mapping_are_reported_unsupported() {
        let module = read(&net(vec![
            ("trafo", table(json!([{"hv_bus": 3, "lv_bus": 0}]))),
            (
                "trafo_characteristic_table",
                table(json!([{"id_characteristic": 0}])),
            ),
            ("shunt", table(json!([{"bus": 0}]))),
            (
                "shunt_characteristic_table",
                table(json!([{"id_characteristic": 0}])),
            ),
            ("switch", table(json!([{"bus": 0}]))),
        ]));
        assert!(
            module
                .diagnostics()
                .iter()
                .all(|d| d.code() == "READ.DIST.PANDAPOWER_TABLE_UNSUPPORTED")
        );
        let mut messages: Vec<_> = module
            .diagnostics()
            .iter()
            .map(Diagnostic::message)
            .collect();
        messages.sort_unstable();
        let mut expected = [
            "shunt",
            "shunt_characteristic_table",
            "switch",
            "trafo",
            "trafo_characteristic_table",
        ]
        .map(|table| format!("`{table}` table ignored (1 rows): it has no three-phase mapping"));
        expected.sort_unstable();
        assert_eq!(messages, expected);
        assert_eq!(module.value().transformers().len(), 0);
        assert_eq!(module.value().shunts().len(), 0);
        assert_eq!(module.value().switches().len(), 0);
    }

    #[test]
    fn a_nonpositive_voltage_is_an_error() {
        let parse = |text: String| {
            parse_multiconductor(Source::from_memory("case.json", text.into_bytes()).unwrap())
        };
        let bus = table(json!([{"vn_kv": 0.0}]));
        let error = parse(net(vec![("bus", bus)])).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("`vn_kv` is not a positive number"),
            "{error}"
        );
    }

    #[test]
    fn a_duplicate_element_index_is_refused() {
        let line = json!({
            "from_bus": 0, "to_bus": 1, "length_km": 0.1, "r_ohm_per_km": 0.2,
            "x_ohm_per_km": 0.08, "c_nf_per_km": 0.0, "r0_ohm_per_km": 0.8,
            "x0_ohm_per_km": 0.32, "c0_nf_per_km": 0.0, "parallel": 1
        });
        for (name, row) in [
            ("line", line),
            (
                "ext_grid",
                json!({"bus": 3, "vm_pu": 1.0, "va_degree": 0.0}),
            ),
            ("load", json!({"bus": 0, "p_mw": 0.01, "q_mvar": 0.0})),
            (
                "asymmetric_load",
                json!({
                    "bus": 0, "p_a_mw": 0.01, "p_b_mw": 0.0, "p_c_mw": 0.0, "q_a_mvar": 0.0,
                    "q_b_mvar": 0.0, "q_c_mvar": 0.0
                }),
            ),
            (
                "sgen",
                json!({"bus": 0, "p_mw": 0.01, "q_mvar": 0.0, "type": "wye"}),
            ),
        ] {
            let columns: Vec<&str> = row
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect();
            let cells: Vec<Value> = columns.iter().map(|c| row[*c].clone()).collect();
            let data = json!([cells.clone(), cells]);
            let parse = |index: Value| {
                let table = frame_with_index(&columns, index, data.clone());
                parse_multiconductor(
                    Source::from_memory("case.json", net(vec![(name, table)]).into_bytes())
                        .unwrap(),
                )
            };
            let error = parse(json!([0, 0])).unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains(&format!("`{name}` table: duplicate index 0")),
                "{name}: {error}"
            );
            assert!(parse(json!([0, 1])).is_ok(), "{name}");
        }
    }

    #[test]
    fn a_scaled_overflow_is_an_error() {
        let parse = |text: String| {
            parse_multiconductor(Source::from_memory("case.json", text.into_bytes()).unwrap())
        };
        for (name, row, needle) in [
            (
                "asymmetric_load",
                json!({"bus": 0, "p_a_mw": 1e308, "p_b_mw": 0.0, "p_c_mw": 0.0, "type": "wye"}),
                "`p_a_mw` overflows",
            ),
            (
                "line",
                json!({
                    "from_bus": 0, "to_bus": 1, "length_km": 1e306, "r_ohm_per_km": 0.1,
                    "x_ohm_per_km": 0.1, "c_nf_per_km": 0.0
                }),
                "`length_km` overflows",
            ),
            ("load", json!({"bus": 0, "p_mw": 1e303}), "`p_mw` overflows"),
        ] {
            let error = parse(net(vec![(name, table(json!([row])))])).unwrap_err();
            assert!(error.to_string().contains(needle), "{name}: {error}");
        }
    }

    #[test]
    fn a_null_or_nonpositive_set_point_is_an_error() {
        let parse = |text: String| {
            parse_multiconductor(Source::from_memory("case.json", text.into_bytes()).unwrap())
        };
        let refused = |name: &str, row: Value, column: &str| {
            let error = parse(net(vec![(name, table(json!([row])))])).unwrap_err();
            assert!(
                error.to_string().contains(&format!("`{column}`")),
                "{name} {column}: {error}"
            );
        };
        for vm_pu in [json!(null), json!(0.0), json!(-1.0)] {
            refused("ext_grid", json!({"bus": 3, "vm_pu": vm_pu}), "vm_pu");
        }
        let line = |key: &str, value: Value| {
            let mut row = json!({
                "from_bus": 0, "to_bus": 1, "length_km": 0.1, "r_ohm_per_km": 0.2,
                "x_ohm_per_km": 0.08, "c_nf_per_km": 0.0, "r0_ohm_per_km": 0.8,
                "x0_ohm_per_km": 0.32, "c0_nf_per_km": 0.0
            });
            row[key] = value;
            row
        };
        for length in [0.0, -0.1] {
            refused("line", line("length_km", json!(length)), "length_km");
        }
        for parallel in [json!(0), json!(-2), json!(null)] {
            refused("line", line("parallel", parallel), "parallel");
        }
        refused(
            "load",
            json!({"bus": 0, "p_mw": 0.01, "scaling": null}),
            "scaling",
        );
        // A missing column still takes the default.
        let module = read(&net(vec![
            ("ext_grid", table(json!([{"bus": 3}]))),
            ("line", table(json!([line("parallel", json!(1))]))),
        ]));
        let v = 20e3 / 3f64.sqrt();
        assert_eq!(module.value().sources()[0].v_magnitude, vec![v, v, v, 0.0]);
        let module = read(&net(vec![("line", one_line())]));
        close(
            module.value().line_codes()[0].r_series[0][0],
            (0.8 + 0.4) / 3.0 / 1e3,
        );
    }

    const UNSUPPORTED: &str = "READ.DIST.PANDAPOWER_ELEMENT_UNSUPPORTED";

    #[test]
    fn out_of_service_line_with_null_parallel_is_skipped() {
        let mut line = one_line();
        let mut row: Value = serde_json::from_str(line["_object"].as_str().unwrap()).unwrap();
        row["columns"]
            .as_array_mut()
            .unwrap()
            .extend([json!("parallel"), json!("in_service")]);
        row["data"][0]
            .as_array_mut()
            .unwrap()
            .extend([json!(null), json!(false)]);
        line["_object"] = json!(row.to_string());
        let module = read(&net(vec![("line", line)]));
        assert_eq!(module.value().lines().len(), 0);
        assert_eq!(module.value().line_codes().len(), 0);
        assert_eq!(codes(&module), vec![UNSUPPORTED]);
        let message = module.diagnostics()[0].message();
        assert!(
            message.starts_with("line_0 is out of service and is not mapped: `line` row 0:"),
            "{message}"
        );
        assert!(!message.contains("read error"), "{message}");
        assert_eq!(message.matches("not mapped").count(), 1, "{message}");
    }

    #[test]
    fn a_skipped_line_is_not_counted_as_lacking_zero_sequence() {
        let row = |parallel: Value, in_service: bool| {
            json!({
                "from_bus": 0, "to_bus": 1, "length_km": 0.1, "r_ohm_per_km": 0.2,
                "x_ohm_per_km": 0.08, "c_nf_per_km": 0.0, "parallel": parallel,
                "in_service": in_service
            })
        };
        let module = read(&net(vec![(
            "line",
            table(json!([row(json!(null), false)])),
        )]));
        assert_eq!(module.value().lines().len(), 0);
        assert_eq!(codes(&module), vec![UNSUPPORTED]);
        let lines = table(json!([row(json!(null), false), row(json!(1), true)]));
        let module = read(&net(vec![("line", lines)]));
        let counted: Vec<_> = module
            .diagnostics()
            .iter()
            .filter(|d| d.message().contains("1 line(s) lack zero-sequence data"))
            .collect();
        assert_eq!(counted.len(), 1, "{:?}", module.diagnostics());
    }

    #[test]
    fn a_negative_line_max_i_ka_is_refused() {
        let line = json!({
            "from_bus": 0, "to_bus": 1, "length_km": 0.1, "r_ohm_per_km": 0.2,
            "x_ohm_per_km": 0.08, "c_nf_per_km": 0.0, "r0_ohm_per_km": 0.8,
            "x0_ohm_per_km": 0.32, "c0_nf_per_km": 0.0, "max_i_ka": -0.2, "parallel": 1
        });
        let error = parse_multiconductor(
            Source::from_memory(
                "case.json",
                net(vec![("line", table(json!([line])))]).into_bytes(),
            )
            .unwrap(),
        )
        .unwrap_err();
        let text = error.to_string();
        assert!(
            text.contains("`max_i_ka`") && text.contains("negative"),
            "{text}"
        );
    }

    #[test]
    fn out_of_service_ext_grid_with_null_vm_pu_is_skipped() {
        let grid = table(json!([{"bus": 3, "vm_pu": null, "in_service": false}]));
        let module = read(&net(vec![("ext_grid", grid)]));
        assert_eq!(module.value().sources().len(), 0);
        assert_eq!(codes(&module), vec![UNSUPPORTED]);
        assert!(!module.diagnostics()[0].message().contains("read error"));
        // pandapower gives NaN voltages for a null scaling even out of service.
        let load = table(json!([{"bus": 0, "p_mw": 0.01, "scaling": null, "in_service": false}]));
        let text = net(vec![("load", load)]);
        assert!(
            parse_multiconductor(Source::from_memory("case.json", text.into_bytes()).unwrap())
                .is_err()
        );
    }

    #[test]
    fn a_large_bus_index_resolves_exactly() {
        let buses = frame_with_index(
            &["vn_kv"],
            json!([9_007_199_254_740_992_u64, 9_007_199_254_740_993_u64]),
            json!([[0.4], [0.4]]),
        );
        let grid = table(json!([{"bus": 9_007_199_254_740_993_u64}]));
        let module = read(&net(vec![("bus", buses), ("ext_grid", grid)]));
        assert_eq!(module.value().sources()[0].bus, "9007199254740993");
    }

    #[test]
    fn a_bad_reference_is_an_error() {
        let line = table(json!([{"from_bus": 0, "to_bus": 9, "length_km": 0.1}]));
        let error = parse_multiconductor(
            Source::from_memory("case.json", net(vec![("line", line)]).into_bytes()).unwrap(),
        )
        .unwrap_err();
        assert_eq!(
            error.info().map(|info| info.code),
            Some("PARSE.DIST.SOURCE_MALFORMED")
        );
        assert!(error.to_string().contains("unknown bus 9"), "{error}");
        for from in [json!(1.5), json!(-1)] {
            let line = table(json!([{"from_bus": from, "to_bus": 0, "length_km": 0.1}]));
            let error = parse_multiconductor(
                Source::from_memory("case.json", net(vec![("line", line)]).into_bytes()).unwrap(),
            )
            .unwrap_err();
            assert_eq!(
                error.info().map(|info| info.code),
                Some("PARSE.DIST.SOURCE_MALFORMED")
            );
            assert!(
                error
                    .to_string()
                    .contains("`from_bus` is not a non-negative integer"),
                "{error}"
            );
        }
    }
}
