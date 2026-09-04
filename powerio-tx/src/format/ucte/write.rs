//! The UCTE-DEF writer: fresh revision 2007.05.01 text from the balanced
//! network.

use std::fmt::Write as _;

use super::{
    AngleRegulation, BASE_MVA, BTreeSet, BUSBARS, BalancedNetwork, Branch, Bus, BusId, BusType,
    COUNTRIES, Diagnostics, Error, Extras, F, FMT, Generator, GeneratorEnergySource, HashMap,
    NodeCode, ORDER_CODES, PLANT_TYPES, PhaseRegulation, REVISION, Result, SQRT_3, TextEmission,
    TransformerControlMode, VOLTAGE_LEVELS_KV, Value, country_iso, country_letter, json,
    voltage_level_code, warn_extra_branch_rating_sets,
};

/// One fixed column output record.
struct Field {
    chars: Vec<char>,
}

impl Field {
    fn new() -> Self {
        Self { chars: Vec::new() }
    }

    fn put(&mut self, begin: usize, text: &str) {
        let needed = begin + text.chars().count();
        if self.chars.len() < needed {
            self.chars.resize(needed, ' ');
        }
        for (offset, c) in text.chars().enumerate() {
            self.chars[begin + offset] = c;
        }
    }

    fn finish(self) -> String {
        let text: String = self.chars.into_iter().collect();
        text.trim_end().to_owned()
    }
}

/// A number in a left aligned field of `width` characters with as many
/// decimals as fit, zero padded on the right as PowSybl writes them; `None`
/// when the integer part alone does not fit.
fn fixed(value: f64, width: usize) -> Option<String> {
    if !value.is_finite() {
        return None;
    }
    let integer_digits = {
        let magnitude = value.abs().trunc();
        let digits = if magnitude < 1.0 {
            1
        } else {
            magnitude.log10().floor() as usize + 1
        };
        digits + usize::from(value < 0.0)
    };
    if integer_digits > width {
        return None;
    }
    let mut decimals = (width - integer_digits).saturating_sub(1).min(5);
    loop {
        let text = format!("{value:.decimals$}");
        if text.chars().count() <= width {
            let mut text = text;
            if !text.contains('.') && text.chars().count() < width {
                text.push('.');
            }
            while text.chars().count() < width {
                text.push('0');
            }
            return Some(text);
        }
        if decimals == 0 {
            return None;
        }
        decimals -= 1;
    }
}

/// An integer right aligned in `width` characters, or `None` when it does not
/// fit.
fn integer(value: i64, width: usize) -> Option<String> {
    let text = value.to_string();
    (text.len() <= width).then(|| format!("{text:>width$}"))
}

/// The writer's running fidelity accounting, summarized into one warning per
/// finding kind at the end.
#[derive(Default)]
struct Losses {
    derived_codes: Vec<String>,
    level_substitutions: Vec<String>,
    unstated_kv: Vec<String>,
    truncated_names: usize,
    sanitized_names: usize,
    out_of_range: Vec<String>,
    non_finite: usize,
    multiple_generators: usize,
    out_of_service_generators: usize,
    out_of_service_loads: usize,
    isolated_buses: usize,
    charging_conductance: usize,
    asymmetric_charging: usize,
    transformer_to_side_admittance: usize,
    remote_regulation: usize,
    quantized_taps: usize,
    relabeled_lines: usize,
    rate_b_c: usize,
    dropped_switches: usize,
    self_loops: usize,
    dispatch_limit_repairs: Vec<String>,
}

/// The UCTE node code of each bus, in bus table order.
///
/// A bus whose name is a UCTE node code keeps it. Any other bus gets
/// `<country><spot><level><busbar>`: the country letter of its area's ISO
/// code when the area name is one, else the area number's entry in the UCTE
/// country table in ISO order; the bus id in base 36 as the five character
/// spot; the voltage level digit nearest its base kV (level 1, 380 kV, for a
/// bus whose base kV is not stated); and busbar `1`, bumped on a collision.
fn assign_node_codes(net: &BalancedNetwork, losses: &mut Losses) -> Result<Vec<NodeCode>> {
    let area_iso: HashMap<usize, &str> = net
        .areas()
        .iter()
        .filter_map(|area| area.name.as_deref().map(|name| (area.number, name)))
        .collect();
    let mut fallback: Vec<char> = COUNTRIES
        .iter()
        .filter(|(letter, _)| *letter != 'X')
        .map(|(letter, _)| *letter)
        .collect();
    fallback.sort_by_key(|letter| country_iso(*letter));
    let mut used: BTreeSet<NodeCode> = BTreeSet::new();
    let mut codes = Vec::with_capacity(net.buses().len());
    let named: Vec<Option<NodeCode>> = net
        .buses()
        .iter()
        .map(|bus| bus.name.as_deref().and_then(NodeCode::parse))
        .collect();
    for (bus, code) in net.buses().iter().zip(&named) {
        if let Some(code) = code
            && used.insert(*code)
        {
            if bus.base_kv <= 0.0 {
                losses.unstated_kv.push(format!(
                    "bus {} (level {} = {} kV)",
                    bus.id,
                    code.level(),
                    code.base_kv()
                ));
            } else if (code.base_kv() - bus.base_kv).abs() > 1e-9 {
                losses.level_substitutions.push(format!(
                    "bus {} ({} kV under level {} = {} kV)",
                    bus.id,
                    bus.base_kv,
                    code.level(),
                    code.base_kv()
                ));
            }
            codes.push(Some(*code));
        } else {
            codes.push(None);
        }
    }
    let mut assigned = Vec::with_capacity(net.buses().len());
    for (bus, code) in net.buses().iter().zip(codes) {
        if let Some(code) = code {
            assigned.push(code);
            continue;
        }
        let letter = area_iso
            .get(&bus.area)
            .and_then(|iso| country_letter(iso))
            .unwrap_or_else(|| fallback[(bus.area.max(1) - 1) % fallback.len()]);
        assigned.push(derive_node_code(bus, letter, &mut used, losses)?);
    }
    Ok(assigned)
}

/// The derived node code of a bus whose name is not one (see
/// [`assign_node_codes`]).
fn derive_node_code(
    bus: &Bus,
    letter: char,
    used: &mut BTreeSet<NodeCode>,
    losses: &mut Losses,
) -> Result<NodeCode> {
    let spot = base36(bus.id.0 as u64, 5).ok_or_else(|| Error::Emit {
        format: FMT,
        message: format!(
            "bus id {} exceeds the 36^5 ids a five character geographical spot can name",
            bus.id
        ),
    })?;
    let level = if bus.base_kv > 0.0 {
        voltage_level_code(bus.base_kv)
    } else {
        1
    };
    if bus.base_kv <= 0.0 {
        losses.unstated_kv.push(format!(
            "bus {} (level {level} = {} kV)",
            bus.id, VOLTAGE_LEVELS_KV[level]
        ));
    } else if (VOLTAGE_LEVELS_KV[level] - bus.base_kv).abs() > 1e-9 {
        losses.level_substitutions.push(format!(
            "bus {} ({} kV under level {level} = {} kV)",
            bus.id, bus.base_kv, VOLTAGE_LEVELS_KV[level]
        ));
    }
    let level_digit = char::from(b'0' + u8::try_from(level).expect("ten voltage levels"));
    let code = BUSBARS
        .chars()
        .map(|busbar| {
            let mut chars = [letter, ' ', ' ', ' ', ' ', ' ', level_digit, busbar];
            for (slot, c) in spot.chars().enumerate() {
                chars[1 + slot] = c;
            }
            NodeCode(chars)
        })
        .find(|candidate| used.insert(*candidate))
        .ok_or_else(|| Error::Emit {
            format: FMT,
            message: format!("bus {} has no free UCTE node code", bus.id),
        })?;
    losses
        .derived_codes
        .push(format!("bus {} -> {:?}", bus.id, code.text()));
    Ok(code)
}

fn base36(mut value: u64, width: usize) -> Option<String> {
    const DIGITS: &[u8; 36] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    let mut out = vec![b'0'; width];
    for slot in (0..width).rev() {
        out[slot] = DIGITS[(value % 36) as usize];
        value /= 36;
    }
    (value == 0).then(|| String::from_utf8(out).expect("base 36 digits are ASCII"))
}

fn extra_f64(extras: &Extras, key: &str) -> Option<f64> {
    extras.get(key).and_then(Value::as_f64)
}

fn extra_i64(extras: &Extras, key: &str) -> Option<i64> {
    extras.get(key).and_then(Value::as_i64)
}

fn extra_text<'a>(extras: &'a Extras, key: &str) -> Option<&'a str> {
    extras.get(key).and_then(Value::as_str)
}

/// The order code of one element between two node codes: the retained one
/// when free, else the first free code.
fn order_code(
    taken: &mut BTreeSet<(NodeCode, NodeCode, char)>,
    node1: NodeCode,
    node2: NodeCode,
    preferred: Option<char>,
) -> Option<char> {
    preferred
        .into_iter()
        .chain(ORDER_CODES.chars())
        .find(|order| taken.insert((node1, node2, *order)))
}

struct NumberWriter<'a> {
    losses: &'a mut Losses,
}

impl NumberWriter<'_> {
    /// A number into `width` characters, clamping a value that does not fit
    /// to the widest representable magnitude and leaving a non finite value
    /// blank; both are reported.
    fn fixed(&mut self, value: f64, width: usize, what: &str) -> Option<String> {
        if !value.is_finite() {
            self.losses.non_finite += 1;
            return None;
        }
        if let Some(text) = fixed(value, width) {
            return Some(text);
        }
        let digits =
            i32::try_from(width.saturating_sub(usize::from(value < 0.0))).unwrap_or(i32::MAX);
        let limit = 10f64.powi(digits) - 1.0;
        let clamped = limit.copysign(value);
        self.losses
            .out_of_range
            .push(format!("{what} {value} clamped to {clamped}"));
        fixed(clamped, width)
    }

    fn integer(&mut self, value: i64, width: usize, what: &str) -> Option<String> {
        if let Some(text) = integer(value, width) {
            return Some(text);
        }
        // The widest magnitude the field holds: `width` digits, one of them
        // spent on the minus sign of a negative value. A zero width leaves no
        // digit, and a width past the 19 digits of an `i64` cannot be reached
        // by a value that did not already fit.
        let digits =
            u32::try_from(width.saturating_sub(usize::from(value < 0))).unwrap_or(u32::MAX);
        let limit = 10i64
            .checked_pow(digits)
            .map_or(i64::MAX, |power| power - 1);
        let clamped = if value < 0 { -limit } else { limit };
        self.losses
            .out_of_range
            .push(format!("{what} {value} clamped to {clamped}"));
        integer(clamped, width)
    }
}

fn put_number(
    field: &mut Field,
    numbers: &mut NumberWriter<'_>,
    begin: usize,
    end: usize,
    value: f64,
    what: &str,
) {
    if let Some(text) = numbers.fixed(value, end - begin, what) {
        field.put(begin, &text);
    }
}

fn put_integer(
    field: &mut Field,
    numbers: &mut NumberWriter<'_>,
    begin: usize,
    end: usize,
    value: i64,
    what: &str,
) {
    if let Some(text) = numbers.integer(value, end - begin, what) {
        field.put(begin, &text);
    }
}

fn put_name(field: &mut Field, losses: &mut Losses, begin: usize, width: usize, name: &str) {
    let name = crate::format::sanitize_quoted(name.trim_end(), &[], ' ');
    losses.sanitized_names += usize::from(matches!(&name, std::borrow::Cow::Owned(_)));
    let text: String = name.chars().take(width).collect();
    if text.chars().count() < name.chars().count() {
        losses.truncated_names += 1;
    }
    field.put(begin, &text);
}

/// The permanent current limit in ampere from the exact current rating when
/// present, else from `rate_a` at `kv`; `None` for an unrated branch.
fn current_limit_amps(branch: &Branch, kv: f64) -> Option<i64> {
    let amps = branch
        .current_ratings
        .map(|ratings| ratings.c_rating_a)
        .filter(|amps| *amps > 0.0)
        .or_else(|| (branch.rate_a > 0.0).then(|| branch.rate_a * 1000.0 / (SQRT_3 * kv)))?;
    Some(amps.round() as i64)
}

/// Serialize `net` as UCTE-DEF revision 2007.05.01.
///
/// # Errors
/// [`Error::Emit`] when a bus id cannot be given a node code or a branch has
/// no free order code.
#[expect(clippy::too_many_lines)]
pub(crate) fn write_ucte(net: &BalancedNetwork) -> Result<TextEmission> {
    let mut warnings = Diagnostics::new();
    let mut losses = Losses::default();
    let codes = assign_node_codes(net, &mut losses)?;
    let code_of: HashMap<BusId, NodeCode> = net
        .buses()
        .iter()
        .zip(&codes)
        .map(|(bus, code)| (bus.id, *code))
        .collect();
    // Physical conversions use the stated base kV; a bus with none stated is
    // taken at its level's nominal voltage, which is what a re-read assumes.
    let kv_of: HashMap<BusId, f64> = net
        .buses()
        .iter()
        .zip(&codes)
        .map(|(bus, code)| {
            let kv = if bus.base_kv > 0.0 {
                bus.base_kv
            } else {
                code.base_kv()
            };
            (bus.id, kv)
        })
        .collect();
    let mut out = String::new();
    out.push_str("##C ");
    out.push_str(REVISION);
    out.push('\n');
    let _ = writeln!(
        out,
        "powerio export: {}",
        net.name().replace(['\r', '\n'], " ")
    );

    // Loads and generators by bus, in service only.
    let mut loads_by_bus: HashMap<BusId, (f64, f64)> = HashMap::new();
    for load in net.loads() {
        if !load.in_service {
            losses.out_of_service_loads += 1;
            continue;
        }
        let entry = loads_by_bus.entry(load.bus).or_insert((0.0, 0.0));
        entry.0 += load.p;
        entry.1 += load.q;
    }
    let mut generators_by_bus: HashMap<BusId, Vec<&Generator>> = HashMap::new();
    for generator in net.generators() {
        if !generator.in_service {
            losses.out_of_service_generators += 1;
            continue;
        }
        generators_by_bus
            .entry(generator.bus)
            .or_default()
            .push(generator);
    }

    out.push_str("##N\n");
    let mut numbers = NumberWriter {
        losses: &mut losses,
    };
    let mut countries: Vec<char> = Vec::new();
    for code in &codes {
        if !countries.contains(&code.country()) {
            countries.push(code.country());
        }
    }
    for letter in countries {
        out.push_str("##Z");
        out.push_str(country_iso(letter).unwrap_or("XX"));
        out.push('\n');
        for (bus, code) in net.buses().iter().zip(&codes) {
            if code.country() != letter {
                continue;
            }
            let mut field = Field::new();
            field.put(0, &code.text());
            let geographical_name = extra_text(&bus.extras, "ucte_geographical_name")
                .map(str::to_owned)
                .or_else(|| {
                    bus.name
                        .as_deref()
                        .filter(|name| NodeCode::parse(name) != Some(*code))
                        .map(str::to_owned)
                });
            if let Some(name) = geographical_name {
                put_name(&mut field, numbers.losses, 9, 12, &name);
            }
            let status = if extra_i64(&bus.extras, "ucte_node_status") == Some(1) {
                "1"
            } else {
                "0"
            };
            field.put(22, status);
            let generators = generators_by_bus.get(&bus.id);
            let regulating = generators
                .is_some_and(|generators| generators.iter().any(|g| g.voltage_regulation_on));
            let type_code = match bus.kind {
                BusType::Ref => "3",
                BusType::Pv => "2",
                BusType::Pq => "0",
                BusType::Isolated => {
                    numbers.losses.isolated_buses += 1;
                    "0"
                }
            };
            field.put(24, type_code);
            let bus_kv = kv_of[&bus.id];
            let reference_kv = if matches!(bus.kind, BusType::Ref | BusType::Pv) && regulating {
                generators
                    .and_then(|generators| generators.iter().find(|g| g.voltage_regulation_on))
                    .map(|g| g.vg * bus_kv)
            } else {
                (bus.vm.is_finite() && bus.vm > 0.0).then_some(bus.vm * bus_kv)
            };
            if let Some(reference) = reference_kv {
                put_number(
                    &mut field,
                    &mut numbers,
                    26,
                    32,
                    reference,
                    "voltage reference",
                );
            }
            let (p_load, q_load) = loads_by_bus.get(&bus.id).copied().unwrap_or((0.0, 0.0));
            put_number(&mut field, &mut numbers, 33, 40, p_load, "active load");
            put_number(&mut field, &mut numbers, 41, 48, q_load, "reactive load");
            let generators = generators.map_or(&[][..], Vec::as_slice);
            if generators.len() > 1 {
                numbers.losses.multiple_generators += 1;
            }
            let sum =
                |value: fn(&Generator) -> f64| generators.iter().map(|g| value(g)).sum::<f64>();
            let p_gen = -sum(|g| g.pg);
            let q_gen = -sum(|g| g.qg);
            put_number(
                &mut field,
                &mut numbers,
                49,
                56,
                p_gen,
                "active power generation",
            );
            put_number(
                &mut field,
                &mut numbers,
                57,
                64,
                q_gen,
                "reactive power generation",
            );
            if !generators.is_empty() {
                let mut min_p = -sum(|g| g.pmin);
                let mut max_p = -sum(|g| g.pmax);
                let mut min_q = -sum(|g| g.qmin);
                let mut max_q = -sum(|g| g.qmax);
                let stated = (min_p, max_p, min_q, max_q);
                if p_gen < max_p {
                    max_p = p_gen;
                }
                if p_gen != 0.0 && p_gen > min_p {
                    min_p = p_gen;
                }
                if q_gen < max_q {
                    max_q = q_gen;
                }
                if q_gen > min_q {
                    min_q = q_gen;
                }
                if stated != (min_p, max_p, min_q, max_q) {
                    numbers.losses.dispatch_limit_repairs.push(format!(
                        "bus {} limits P [{}, {}] and Q [{}, {}] became P [{min_p}, {max_p}] and Q [{min_q}, {max_q}] in the UCTE generation sign convention",
                        bus.id, stated.0, stated.1, stated.2, stated.3
                    ));
                }
                put_number(
                    &mut field,
                    &mut numbers,
                    65,
                    72,
                    min_p,
                    "minimum permissible active power generation",
                );
                put_number(
                    &mut field,
                    &mut numbers,
                    73,
                    80,
                    max_p,
                    "maximum permissible active power generation",
                );
                put_number(
                    &mut field,
                    &mut numbers,
                    81,
                    88,
                    min_q,
                    "minimum permissible reactive power generation",
                );
                put_number(
                    &mut field,
                    &mut numbers,
                    89,
                    96,
                    max_q,
                    "maximum permissible reactive power generation",
                );
            }
            if let Some(value) = extra_f64(&bus.extras, "ucte_primary_control_static") {
                put_number(
                    &mut field,
                    &mut numbers,
                    97,
                    102,
                    value,
                    "static of primary control",
                );
            }
            if let Some(value) = extra_f64(&bus.extras, "ucte_primary_control_power") {
                put_number(
                    &mut field,
                    &mut numbers,
                    103,
                    110,
                    value,
                    "nominal power for primary control",
                );
            }
            if let Some(value) = extra_f64(&bus.extras, "ucte_short_circuit_power") {
                put_number(
                    &mut field,
                    &mut numbers,
                    111,
                    118,
                    value,
                    "three phase short circuit power",
                );
            }
            if let Some(value) = extra_f64(&bus.extras, "ucte_xr_ratio") {
                put_number(&mut field, &mut numbers, 119, 126, value, "X/R ratio");
            }
            let plant_type = extra_text(&bus.extras, "ucte_power_plant_type")
                .and_then(|text| text.chars().next())
                .filter(|letter| PLANT_TYPES.contains(*letter))
                .or_else(|| {
                    net.generators()
                        .iter()
                        .find(|generator| generator.bus == bus.id)
                        .map(|generator| match generator.energy_source {
                            GeneratorEnergySource::Hydro => 'H',
                            GeneratorEnergySource::Nuclear => 'N',
                            GeneratorEnergySource::Thermal => 'C',
                            GeneratorEnergySource::Wind => 'W',
                            GeneratorEnergySource::Solar | GeneratorEnergySource::Other => 'F',
                        })
                });
            if let Some(letter) = plant_type {
                field.put(127, &letter.to_string());
            }
            out.push_str(&field.finish());
            out.push('\n');
        }
    }

    // Branches: a line joins two nodes of one voltage level with no ratio and
    // no shift; anything else is a transformer from the regulated winding.
    let mut lines = String::new();
    let mut transformers = String::new();
    let mut regulations = String::new();
    let mut special_descriptions = String::new();
    let mut taken: BTreeSet<(NodeCode, NodeCode, char)> = BTreeSet::new();
    for branch in net.branches() {
        let (Some(from_code), Some(to_code)) = (code_of.get(&branch.from), code_of.get(&branch.to))
        else {
            continue;
        };
        if branch.from == branch.to {
            numbers.losses.self_loops += 1;
            continue;
        }
        let from_kv = kv_of[&branch.from];
        let to_kv = kv_of[&branch.to];
        let preferred =
            extra_text(&branch.extras, "ucte_order_code").and_then(|text| text.chars().next());
        let is_transformer = branch.is_transformer()
            || from_code.level() != to_code.level()
            || (from_kv - to_kv).abs() > 1e-9
            || branch.control.is_some()
            || branch.extras.contains_key("ucte_rated_voltage_1");
        let status = match (
            branch.in_service,
            branch.extras.get("ucte_equivalent") == Some(&json!(true)),
        ) {
            (true, false) => "0",
            (false, false) => "8",
            (true, true) => "1",
            (false, true) => "9",
        };
        let charging = branch.calc_terminal_charging();
        if charging.g_fr.abs() > f64::EPSILON || charging.g_to.abs() > f64::EPSILON {
            if is_transformer {
                if charging.g_to.abs() > f64::EPSILON {
                    numbers.losses.transformer_to_side_admittance += 1;
                }
            } else {
                numbers.losses.charging_conductance += 1;
            }
        }
        if crate::format::nonzero_differs(branch.rate_b, branch.rate_a)
            || crate::format::nonzero_differs(branch.rate_c, branch.rate_a)
        {
            numbers.losses.rate_b_c += 1;
        }
        if !is_transformer {
            let Some(order) = order_code(&mut taken, *from_code, *to_code, preferred) else {
                return Err(Error::Emit {
                    format: FMT,
                    message: format!(
                        "no free order code for a line from bus {} to bus {}",
                        branch.from, branch.to
                    ),
                });
            };
            let zbase = to_kv * to_kv / BASE_MVA;
            let mut field = Field::new();
            field.put(0, &from_code.text());
            field.put(9, &to_code.text());
            field.put(18, &order.to_string());
            field.put(20, status);
            put_number(
                &mut field,
                &mut numbers,
                22,
                28,
                branch.r * zbase,
                "resistance",
            );
            put_number(
                &mut field,
                &mut numbers,
                29,
                35,
                branch.x * zbase,
                "reactance",
            );
            if (charging.b_fr - charging.b_to).abs() > f64::EPSILON {
                numbers.losses.asymmetric_charging += 1;
            }
            put_number(
                &mut field,
                &mut numbers,
                36,
                44,
                charging.calc_total_b() / zbase * 1e6,
                "susceptance",
            );
            if let Some(amps) = current_limit_amps(branch, to_kv) {
                put_integer(&mut field, &mut numbers, 45, 51, amps, "current limit");
            }
            if let Some(name) = branch.name.as_deref() {
                put_name(&mut field, numbers.losses, 52, 12, name);
            }
            lines.push_str(&field.finish());
            lines.push('\n');
            continue;
        }
        if !branch.is_transformer() && branch.control.is_none() {
            numbers.losses.relabeled_lines += 1;
        }
        // Node 1 is the non regulated winding, the to end; node 2 the
        // regulated winding, the from end.
        let (node1, node2) = (*to_code, *from_code);
        let Some(order) = order_code(&mut taken, node1, node2, preferred) else {
            return Err(Error::Emit {
                format: FMT,
                message: format!(
                    "no free order code for a transformer from bus {} to bus {}",
                    branch.from, branch.to
                ),
            });
        };
        let zbase = to_kv * to_kv / BASE_MVA;
        let tap = branch.calc_effective_tap();
        let rated_u1 = extra_f64(&branch.extras, "ucte_rated_voltage_1")
            .filter(|u| *u > 0.0)
            .unwrap_or(to_kv);
        let regulation = transformer_regulation(branch, from_kv, numbers.losses);
        let mut ratio_factor = 1.0;
        if let Some(phase) = &regulation.phase {
            ratio_factor *= 1.0 + phase.np as f64 * phase.du / 100.0;
        }
        if let Some(angle) = &regulation.angle {
            ratio_factor /= angle.rho_alpha(angle.np).0;
        }
        let rated_u2 = tap * from_kv * (rated_u1 / to_kv) / ratio_factor;
        let mut field = Field::new();
        field.put(0, &node1.text());
        field.put(9, &node2.text());
        field.put(18, &order.to_string());
        field.put(20, status);
        put_number(
            &mut field,
            &mut numbers,
            22,
            27,
            rated_u1,
            "rated voltage 1",
        );
        put_number(
            &mut field,
            &mut numbers,
            28,
            33,
            rated_u2,
            "rated voltage 2",
        );
        let nominal_power = extra_f64(&branch.extras, "ucte_nominal_power").or_else(|| {
            branch
                .control
                .as_ref()
                .map(|control| control.mva_base)
                .filter(|mva| *mva > 0.0)
        });
        if let Some(power) = nominal_power {
            put_number(&mut field, &mut numbers, 34, 39, power, "nominal power");
        }
        put_number(
            &mut field,
            &mut numbers,
            40,
            46,
            branch.r * zbase,
            "resistance",
        );
        put_number(
            &mut field,
            &mut numbers,
            47,
            53,
            branch.x * zbase,
            "reactance",
        );
        if charging.b_to.abs() > f64::EPSILON {
            numbers.losses.transformer_to_side_admittance += 1;
        }
        put_number(
            &mut field,
            &mut numbers,
            54,
            62,
            charging.calc_total_b() / zbase * 1e6,
            "susceptance",
        );
        put_number(
            &mut field,
            &mut numbers,
            63,
            69,
            charging.calc_total_g() / zbase * 1e6,
            "conductance",
        );
        if let Some(amps) = current_limit_amps(branch, to_kv) {
            put_integer(&mut field, &mut numbers, 70, 76, amps, "current limit");
        }
        if let Some(name) = branch.name.as_deref() {
            put_name(&mut field, numbers.losses, 77, 12, name);
        }
        transformers.push_str(&field.finish());
        transformers.push('\n');
        if regulation.phase.is_some() || regulation.angle.is_some() {
            let mut field = Field::new();
            field.put(0, &node1.text());
            field.put(9, &node2.text());
            field.put(18, &order.to_string());
            if let Some(phase) = &regulation.phase {
                put_number(
                    &mut field,
                    &mut numbers,
                    20,
                    25,
                    phase.du,
                    "phase regulation voltage step",
                );
                put_integer(
                    &mut field,
                    &mut numbers,
                    26,
                    28,
                    phase.n,
                    "phase regulation tap count",
                );
                put_integer(
                    &mut field,
                    &mut numbers,
                    29,
                    32,
                    phase.np,
                    "phase regulation tap position",
                );
                if let Some(u) = phase.u {
                    put_number(
                        &mut field,
                        &mut numbers,
                        33,
                        38,
                        u,
                        "phase regulation voltage target",
                    );
                }
            }
            if let Some(angle) = &regulation.angle {
                put_number(
                    &mut field,
                    &mut numbers,
                    39,
                    44,
                    angle.du,
                    "angle regulation voltage step",
                );
                put_number(
                    &mut field,
                    &mut numbers,
                    45,
                    50,
                    angle.theta,
                    "angle regulation angle",
                );
                put_integer(
                    &mut field,
                    &mut numbers,
                    51,
                    53,
                    angle.n,
                    "angle regulation tap count",
                );
                put_integer(
                    &mut field,
                    &mut numbers,
                    54,
                    57,
                    angle.np,
                    "angle regulation tap position",
                );
                if let Some(p) = angle.p {
                    put_number(
                        &mut field,
                        &mut numbers,
                        58,
                        63,
                        p,
                        "angle regulation active power target",
                    );
                }
                field.put(64, if angle.symmetrical { "SYMM" } else { "ASYM" });
            }
            regulations.push_str(&field.finish());
            regulations.push('\n');
        }
        if let Some(Value::Array(rows)) = branch.extras.get("ucte_special_description") {
            let id = format!("{} {} {order}", node1.text(), node2.text());
            for row in rows.iter().filter_map(Value::as_str) {
                let mut field = Field::new();
                field.put(0, &id);
                let tail: String = row.chars().skip(19).collect();
                field.put(19, &tail);
                special_descriptions.push_str(&field.finish());
                special_descriptions.push('\n');
            }
        }
    }
    for switch in net.switches() {
        let (Some(from_code), Some(to_code)) = (code_of.get(&switch.from), code_of.get(&switch.to))
        else {
            continue;
        };
        if from_code.level() != to_code.level() || switch.from == switch.to {
            numbers.losses.dropped_switches += 1;
            continue;
        }
        let preferred =
            extra_text(&switch.extras, "ucte_order_code").and_then(|text| text.chars().next());
        let Some(order) = order_code(&mut taken, *from_code, *to_code, preferred) else {
            return Err(Error::Emit {
                format: FMT,
                message: format!(
                    "no free order code for a busbar coupler from bus {} to bus {}",
                    switch.from, switch.to
                ),
            });
        };
        let mut field = Field::new();
        field.put(0, &from_code.text());
        field.put(9, &to_code.text());
        field.put(18, &order.to_string());
        field.put(20, if switch.closed { "2" } else { "7" });
        field.put(22, "0.0000 0.0000 0.000000");
        let amps = switch
            .current_rating
            .filter(|amps| *amps > 0.0)
            .or_else(|| {
                switch
                    .thermal_rating
                    .filter(|mva| *mva > 0.0)
                    .map(|mva| mva * 1000.0 / (SQRT_3 * kv_of[&switch.to]))
            });
        if let Some(amps) = amps {
            put_integer(
                &mut field,
                &mut numbers,
                45,
                51,
                amps.round() as i64,
                "current limit",
            );
        }
        if let Some(name) = extra_text(&switch.extras, "ucte_element_name") {
            put_name(&mut field, numbers.losses, 52, 12, name);
        }
        lines.push_str(&field.finish());
        lines.push('\n');
    }
    out.push_str("##L\n");
    out.push_str(&lines);
    out.push_str("##T\n");
    out.push_str(&transformers);
    out.push_str("##R\n");
    out.push_str(&regulations);
    if !special_descriptions.is_empty() {
        out.push_str("##TT\n");
        out.push_str(&special_descriptions);
    }

    report_losses(net, &losses, &mut warnings);
    Ok(TextEmission::new(out, warnings))
}

struct Regulation {
    phase: Option<PhaseRegulation>,
    angle: Option<AngleRegulation>,
}

/// The `##R` record of a transformer: the retained UCTE regulation when the
/// branch carries one, else a symmetrical one step angle regulation for a
/// phase shift and a phase regulation for a voltage control with a tap range.
fn transformer_regulation(branch: &Branch, from_kv: f64, losses: &mut Losses) -> Regulation {
    let retained_phase = branch
        .extras
        .get("ucte_phase_regulation")
        .and_then(|value| {
            Some(PhaseRegulation {
                du: value.get("du")?.as_f64()?,
                n: value.get("n")?.as_i64()?,
                np: value.get("np")?.as_i64()?,
                u: value.get("u").and_then(Value::as_f64),
            })
        });
    let retained_angle = branch
        .extras
        .get("ucte_angle_regulation")
        .and_then(|value| {
            Some(AngleRegulation {
                du: value.get("du")?.as_f64()?,
                theta: value.get("theta")?.as_f64()?,
                n: value.get("n")?.as_i64()?,
                np: value.get("np")?.as_i64()?,
                p: value.get("p").and_then(Value::as_f64),
                symmetrical: value.get("type").and_then(Value::as_str) == Some("SYMM"),
                type_stated: true,
            })
        });
    if retained_phase.is_some() || retained_angle.is_some() {
        return Regulation {
            phase: retained_phase,
            angle: retained_angle,
        };
    }
    let angle = (branch.shift != 0.0 && branch.shift.is_finite()).then(|| AngleRegulation {
        du: 200.0 * (branch.shift.abs().to_radians() / 2.0).tan(),
        theta: 90.0,
        n: 1,
        np: if branch.shift > 0.0 { 1 } else { -1 },
        p: None,
        symmetrical: true,
        type_stated: true,
    });
    let phase = branch.control.as_ref().and_then(|control| {
        if control.mode != TransformerControlMode::Voltage
            || control.ntp < 3
            || control.tap_max <= control.tap_min
        {
            return None;
        }
        let tap = branch.calc_effective_tap();
        let n = i64::from((control.ntp - 1) / 2);
        let nominal = f64::midpoint(control.tap_max, control.tap_min);
        let step = (control.tap_max - control.tap_min) / f64::from(control.ntp - 1);
        if nominal <= 0.0 || step <= 0.0 {
            return None;
        }
        let du = step / nominal * 100.0;
        let position = ((tap - nominal) / step).round();
        let np = position as i64;
        if (tap - (nominal + position * step)).abs() > 1e-9 || np.abs() > n {
            losses.quantized_taps += 1;
        }
        let np = np.clamp(-n, n);
        let u = if control.enabled {
            if control.controlled_bus.is_none_or(|bus| bus == branch.from) {
                Some(f64::midpoint(control.band_min, control.band_max) * from_kv)
            } else {
                losses.remote_regulation += 1;
                None
            }
        } else {
            None
        };
        Some(PhaseRegulation { du, n, np, u })
    });
    Regulation { phase, angle }
}

fn report_losses(net: &BalancedNetwork, losses: &Losses, warnings: &mut Diagnostics) {
    let sample = |items: &[String]| {
        let shown: Vec<&str> = items.iter().take(5).map(String::as_str).collect();
        if items.len() > shown.len() {
            format!("{}, ...", shown.join(", "))
        } else {
            shown.join(", ")
        }
    };
    if !losses.dispatch_limit_repairs.is_empty() {
        warnings.push(
            &F.value_substituted,
            format!(
                "{} bus generation limit set(s) did not contain the dispatch and were widened before UCTE emission, matching UCTE consistency rules: {}",
                losses.dispatch_limit_repairs.len(),
                sample(&losses.dispatch_limit_repairs)
            ),
        );
    }
    if !losses.derived_codes.is_empty() {
        warnings.push(
            &F.value_substituted,
            format!(
                "{} bus name(s) are not UCTE node codes; each was given <country letter of the area's ISO name, else the area number's UCTE country in ISO order><bus id in base 36, five characters><voltage level digit><busbar 1, bumped on collision>: {}",
                losses.derived_codes.len(),
                sample(&losses.derived_codes)
            ),
        );
    }
    if !losses.level_substitutions.is_empty() {
        warnings.push(
            &F.value_substituted,
            format!(
                "{} bus(es) have a base kV that is not one of the ten UCTE voltage levels and were written under the nearest level; ohm, kV, and MW values stay physical, so a re-read expresses them per unit on the level's nominal voltage: {}",
                losses.level_substitutions.len(),
                sample(&losses.level_substitutions)
            ),
        );
    }
    if !losses.unstated_kv.is_empty() {
        warnings.push(
            &F.value_defaulted,
            format!(
                "{} bus(es) state no positive base kV; each was written at its voltage level's nominal voltage, which the physical ohm and kV values are computed on: {}",
                losses.unstated_kv.len(),
                sample(&losses.unstated_kv)
            ),
        );
    }
    if losses.truncated_names > 0 {
        warnings.push(
            &F.value_truncated,
            format!(
                "{} name(s) shortened to the 12 characters a UCTE name field holds",
                losses.truncated_names
            ),
        );
    }
    if losses.sanitized_names > 0 {
        warnings.push(
            &F.value_substituted,
            format!(
                "{} name(s) contained a line break that was replaced with a space",
                losses.sanitized_names
            ),
        );
    }
    if !losses.out_of_range.is_empty() {
        warnings.push(
            &F.value_substituted,
            format!(
                "{} value(s) do not fit their fixed width field and were clamped: {}",
                losses.out_of_range.len(),
                sample(&losses.out_of_range)
            ),
        );
    }
    if losses.non_finite > 0 {
        warnings.push(
            &F.not_a_number,
            format!(
                "{} non-finite value(s) left blank: UCTE-DEF has no Inf or NaN",
                losses.non_finite
            ),
        );
    }
    report_element_losses(losses, warnings);
    report_model_losses(net, warnings);
}

/// The element level collapses and drops of one emission.
fn report_element_losses(losses: &Losses, warnings: &mut Diagnostics) {
    if losses.multiple_generators > 0 {
        warnings.push(
            &F.value_collapsed,
            format!(
                "{} bus(es) host several in service generators; a UCTE node carries one generation record, so their set points and limits were summed",
                losses.multiple_generators
            ),
        );
    }
    if losses.out_of_service_generators > 0 {
        warnings.push(
            &F.record_dropped,
            format!(
                "{} out of service generator(s) dropped: a UCTE node record has no generator status",
                losses.out_of_service_generators
            ),
        );
    }
    if losses.out_of_service_loads > 0 {
        warnings.push(
            &F.record_dropped,
            format!(
                "{} out of service load(s) dropped: a UCTE node record has no load status",
                losses.out_of_service_loads
            ),
        );
    }
    if losses.isolated_buses > 0 {
        warnings.push(
            &F.value_collapsed,
            format!(
                "{} isolated bus(es) written as PQ nodes: UCTE-DEF has no isolated node type",
                losses.isolated_buses
            ),
        );
    }
    report_branch_losses(losses, warnings);
}

/// The branch and switch level collapses and drops of one emission.
fn report_branch_losses(losses: &Losses, warnings: &mut Diagnostics) {
    if losses.charging_conductance > 0 {
        warnings.push(
            &F.field_dropped,
            format!(
                "{} line(s) carry terminal conductance, which a UCTE line record has no field for; dropped",
                losses.charging_conductance
            ),
        );
    }
    if losses.asymmetric_charging > 0 {
        warnings.push(
            &F.value_collapsed,
            format!(
                "{} line(s) carry unequal terminal susceptance; a UCTE line states one total that reads back split evenly",
                losses.asymmetric_charging
            ),
        );
    }
    if losses.transformer_to_side_admittance > 0 {
        warnings.push(
            &F.value_collapsed,
            format!(
                "{} transformer(s) carry admittance on the non regulated winding side; a UCTE transformer states one magnetizing admittance that reads back on the regulated side",
                losses.transformer_to_side_admittance
            ),
        );
    }
    if losses.remote_regulation > 0 {
        warnings.push(
            &F.field_dropped,
            format!(
                "{} transformer(s) regulate the voltage of a bus other than their regulated winding; a UCTE phase regulation targets that winding, so the target was dropped",
                losses.remote_regulation
            ),
        );
    }
    if losses.quantized_taps > 0 {
        warnings.push(
            &F.value_collapsed,
            format!(
                "{} transformer tap ratio(s) sit between the positions of their tap range and were written at the nearest position",
                losses.quantized_taps
            ),
        );
    }
    if losses.relabeled_lines > 0 {
        warnings.push(
            &F.element_relabeled,
            format!(
                "{} line(s) join two voltage levels or bases and were written as transformers with nominal rated voltages",
                losses.relabeled_lines
            ),
        );
    }
    if losses.rate_b_c > 0 {
        warnings.push(
            &F.field_dropped,
            format!(
                "{} branch(es) carry rate_b or rate_c; a UCTE element has one permanent current limit, so they were dropped",
                losses.rate_b_c
            ),
        );
    }
    if losses.dropped_switches > 0 {
        warnings.push(
            &F.record_dropped,
            format!(
                "{} switch(es) join two voltage levels or one bus to itself; a UCTE busbar coupler joins two nodes of one level, so they were dropped",
                losses.dropped_switches
            ),
        );
    }
    if losses.self_loops > 0 {
        warnings.push(
            &F.record_dropped,
            format!(
                "{} branch(es) join a bus to itself; dropped",
                losses.self_loops
            ),
        );
    }
}

/// Whether a bus states a voltage band other than the model default; exact,
/// because the default is the literal `Bus::new` writes.
#[expect(clippy::float_cmp)]
fn has_voltage_band(bus: &Bus) -> bool {
    bus.vmax != 1.1 || bus.vmin != 0.9 || bus.evhi.is_some() || bus.evlo.is_some()
}

/// The records and fields of the network itself that UCTE-DEF has no place
/// for.
fn report_model_losses(net: &BalancedNetwork, warnings: &mut Diagnostics) {
    warn_extra_branch_rating_sets(&F, "UCTE-DEF", net, warnings);
    let count =
        |n: usize, what: &str, why: &str| (n > 0).then(|| format!("{n} {what} dropped: {why}"));
    for message in [
        count(
            net.shunts().len(),
            "shunt(s)",
            "UCTE-DEF has no shunt record",
        ),
        count(
            net.static_var_compensators().len(),
            "static VAR compensator(s)",
            "UCTE-DEF has no such record",
        ),
        count(
            net.hvdc().len(),
            "HVDC link(s)",
            "UCTE-DEF has no DC record",
        ),
        count(
            net.storage().len(),
            "storage unit(s)",
            "UCTE-DEF has no storage record",
        ),
        count(
            net.transformers_3w().len(),
            "three winding transformer(s)",
            "UCTE-DEF has two winding transformers only",
        ),
    ]
    .into_iter()
    .flatten()
    {
        warnings.push(&F.record_dropped, message);
    }
    if net.generators().iter().any(|g| g.cost.is_some()) {
        warnings.push(
            &F.field_dropped,
            "generator cost curves dropped: UCTE-DEF has no cost data",
        );
    }
    if net.generators().iter().any(Generator::has_caps) {
        warnings.push(
            &F.field_dropped,
            "generator ramp/capability columns dropped: UCTE-DEF has no equivalent fields",
        );
    }
    let remote = net
        .generators()
        .iter()
        .filter(|g| g.in_service && g.regulated_bus.is_some_and(|bus| bus != g.bus))
        .count();
    if remote > 0 {
        warnings.push(
            &F.field_dropped,
            format!("{remote} generator(s) regulate a remote bus; a UCTE node regulates its own voltage, so the remote target was dropped"),
        );
    }
    report_field_losses(net, warnings);
}

/// The fields of retained records that UCTE-DEF has no column for.
fn report_field_losses(net: &BalancedNetwork, warnings: &mut Diagnostics) {
    if net.branches().iter().any(Branch::has_angle_limits) {
        warnings.push(
            &F.field_dropped,
            "branch angle limits (angmin/angmax) dropped: UCTE element records carry none",
        );
    }
    let solved = net
        .branches()
        .iter()
        .filter(|b| b.solution.is_some())
        .count();
    if solved > 0 {
        warnings.push(
            &F.field_dropped,
            format!(
                "{solved} branch solution value set(s) dropped: UCTE-DEF carries no flow results"
            ),
        );
    }
    let angles = net.buses().iter().filter(|b| b.va != 0.0).count();
    if angles > 0 {
        warnings.push(
            &F.field_dropped,
            format!("{angles} bus voltage angle(s) dropped: a UCTE node states a voltage reference magnitude only"),
        );
    }
    let bands = net.buses().iter().filter(|b| has_voltage_band(b)).count();
    if bands > 0 {
        warnings.push(
            &F.field_dropped,
            format!("{bands} bus voltage band(s) dropped: a UCTE node has no voltage limit fields"),
        );
    }
    let load_models = net
        .loads()
        .iter()
        .filter(|l| l.voltage_model.is_some())
        .count();
    if load_models > 0 {
        warnings.push(
            &F.field_dropped,
            format!("{load_models} load voltage model(s) dropped: a UCTE node states constant power only"),
        );
    }
    let interchange = net
        .areas()
        .iter()
        .filter(|a| a.net_interchange != 0.0 || a.tolerance != 0.0 || a.slack_bus.is_some())
        .count();
    if interchange > 0 {
        warnings.push(
            &F.field_dropped,
            format!("{interchange} area interchange record(s) dropped: fresh UCTE output writes no ##E block"),
        );
    }
    if (net.base_mva() - BASE_MVA).abs() > 1e-9 {
        warnings.push(
            &F.value_substituted,
            format!(
                "system base {} MVA not written: UCTE-DEF states physical units, so a re-read uses {BASE_MVA} MVA",
                net.base_mva()
            ),
        );
    }
    crate::format::warn_dropped_extras(
        &F,
        "UCTE-DEF",
        net,
        |key| key.starts_with("ucte_"),
        warnings,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_fill_their_field_and_integers_right_align() {
        assert_eq!(fixed(400.0, 6).unwrap(), "400.00");
        assert_eq!(fixed(3.96708, 7).unwrap(), "3.96708");
        assert_eq!(fixed(-800.0, 7).unwrap(), "-800.00");
        assert_eq!(fixed(1300.0, 5).unwrap(), "1300.");
        assert_eq!(fixed(99999.9, 6).unwrap(), "100000");
        assert_eq!(fixed(9.9999, 5).unwrap(), "10.00");
        assert!(fixed(1_000_000.0, 6).is_none());
        assert_eq!(integer(1519, 6).unwrap(), "  1519");
        assert!(integer(1_000_000, 6).is_none());
        assert_eq!(base36(1, 5).unwrap(), "00001");
        assert_eq!(base36(36, 5).unwrap(), "00010");
        assert!(base36(36u64.pow(5), 5).is_none());
    }

    #[test]
    fn names_cannot_split_a_fixed_width_record() {
        let mut field = Field::new();
        let mut losses = Losses::default();
        put_name(&mut field, &mut losses, 0, 12, "alpha\nbeta");
        assert_eq!(field.finish(), "alpha beta");
        assert_eq!(losses.sanitized_names, 1);
    }
}
