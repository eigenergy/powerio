//! Read the IEEE Common Data Format for solved load flow cases.
//!
//! A CDF file is one title card followed by fixed column sections: bus data,
//! branch data, loss zones, interchange data, and tie lines, each closed by a
//! numeric terminator card. The column ranges match the PowSybl
//! `ieee-cdf-model` readers, which read the public IEEE test cases; the 1973
//! format table disagrees with those files by one column in a few places.
//! The bus, branch, and interchange sections map into [`BalancedNetwork`].
//! Loss zone names and tie lines survive in the retained source only. The
//! format is read only: there is no writer.

use std::collections::HashMap;

use powerio_core::{SourceId, SourceSpan};
use serde_json::Value;

use crate::diagnostics::{Diagnostic, DiagnosticInfo, Diagnostics, codes};
use crate::network::{
    Area, BalancedNetwork, BalancedNetworkTables, Branch, Bus, BusId, BusType, CaseMetadata,
    Generator, Load, Shunt, SourceFormat, TransformerControl, TransformerControlMode,
};
use crate::{Error, Result};

/// The format label in errors and diagnostics.
pub(crate) const FMT: &str = "IEEE CDF";

/// Active power limits the format does not carry: a generator absorbs no
/// active power, and its ceiling is the PSS/E unbounded convention.
const DEFAULT_PMIN_MW: f64 = 0.0;
const DEFAULT_PMAX_MW: f64 = 9999.0;

/// Phase angle window for a phase shifter whose record states no tap limits
/// (the PSS/E default for active power control).
const DEFAULT_ANGLE_LIMIT_DEG: f64 = 180.0;

/// A 1-based inclusive column range, as the format description numbers them.
type Columns = (usize, usize);

mod title_col {
    use super::Columns;
    pub const DATE: Columns = (2, 9);
    pub const ORIGINATOR: Columns = (11, 30);
    pub const BASE_MVA: Columns = (32, 37);
    pub const YEAR: Columns = (39, 42);
    pub const SEASON: Columns = (44, 44);
    pub const CASE_ID: Columns = (46, 73);
}

mod bus_col {
    use super::Columns;
    pub const NUMBER: Columns = (1, 4);
    pub const NAME: Columns = (6, 17);
    pub const AREA: Columns = (19, 20);
    pub const ZONE: Columns = (21, 23);
    pub const TYPE: Columns = (25, 26);
    pub const VM: Columns = (28, 33);
    pub const VA: Columns = (34, 40);
    pub const PD: Columns = (41, 49);
    pub const QD: Columns = (50, 58);
    pub const PG: Columns = (59, 67);
    pub const QG: Columns = (68, 75);
    pub const BASE_KV: Columns = (77, 83);
    pub const V_DESIRED: Columns = (85, 90);
    pub const LIMIT_MAX: Columns = (91, 98);
    pub const LIMIT_MIN: Columns = (99, 106);
    pub const G: Columns = (107, 114);
    pub const B: Columns = (115, 122);
    pub const REMOTE_BUS: Columns = (124, 127);
}

mod branch_col {
    use super::Columns;
    pub const FROM: Columns = (1, 4);
    pub const TO: Columns = (6, 9);
    /// Columns 11-12 and 14-15 hold the branch area and loss zone, which
    /// repeat the bus columns and have no branch field in the balanced model.
    pub const CIRCUIT: Columns = (17, 17);
    pub const TYPE: Columns = (19, 19);
    pub const R: Columns = (20, 29);
    pub const X: Columns = (30, 39);
    pub const B: Columns = (41, 49);
    pub const RATE_A: Columns = (51, 55);
    pub const RATE_B: Columns = (57, 61);
    pub const RATE_C: Columns = (63, 67);
    pub const CONTROL_BUS: Columns = (69, 72);
    pub const SIDE: Columns = (74, 74);
    pub const RATIO: Columns = (77, 82);
    pub const ANGLE: Columns = (84, 90);
    pub const TAP_MIN: Columns = (91, 97);
    pub const TAP_MAX: Columns = (98, 104);
    pub const STEP: Columns = (105, 111);
    pub const LIMIT_MIN: Columns = (113, 119);
    pub const LIMIT_MAX: Columns = (120, 126);
    /// The public IEEE archive files place the last two limits one column
    /// to the left, so a value at column 119 belongs to the maximum.
    pub const LIMIT_MIN_NARROW: Columns = (113, 118);
    pub const LIMIT_MAX_SHIFTED: Columns = (119, 125);
}

mod zone_col {
    use super::Columns;
    pub const NUMBER: Columns = (1, 3);
    pub const NAME: Columns = (5, 16);
}

mod interchange_col {
    use super::Columns;
    pub const AREA: Columns = (1, 2);
    pub const SLACK_BUS: Columns = (4, 7);
    pub const SWING_NAME: Columns = (9, 20);
    pub const EXPORT: Columns = (21, 28);
    pub const TOLERANCE: Columns = (29, 35);
    pub const CODE: Columns = (38, 43);
    pub const NAME: Columns = (46, 75);
}

mod tie_col {
    use super::Columns;
    pub const METERED_BUS: Columns = (1, 4);
    pub const OTHER_BUS: Columns = (11, 14);
}

/// Whether `bytes` open with an IEEE CDF title card: a `MM/DD/YY` date in
/// columns 2 through 9 beside a numeric MVA base in columns 32 through 37, or
/// a `BUS DATA` section header as the first card after the title.
pub(crate) fn looks_like_ieee_cdf(bytes: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return false;
    };
    let mut lines = text.lines();
    let Some(title) = lines.next() else {
        return false;
    };
    let dated = title.len() >= 9 && title.as_bytes()[3] == b'/' && title.as_bytes()[6] == b'/';
    let based = field(title, title_col::BASE_MVA).is_some_and(|raw| parse_float(raw).is_some());
    let bus_header = lines
        .find(|line| !line.trim().is_empty())
        .is_some_and(|line| {
            line.trim_start()
                .get(..8)
                .is_some_and(|head| head.eq_ignore_ascii_case("BUS DATA"))
        });
    (dated && based) || bus_header
}

/// Where the decoded text begins inside the retained buffer, so a record's
/// byte range maps onto that buffer's coordinates for a span.
#[derive(Clone, Debug)]
pub(crate) struct TextOrigin {
    source: SourceId,
    offset: u64,
}

impl TextOrigin {
    pub(crate) fn new(source: SourceId, offset: u64) -> Self {
        Self { source, offset }
    }
}

/// One physical line: its 1-based number and byte range in the decoded text.
#[derive(Clone, Copy, Debug)]
struct Line<'a> {
    number: usize,
    text: &'a str,
    start: usize,
    end: usize,
}

fn lines(text: &str) -> impl Iterator<Item = Line<'_>> {
    let mut offset = 0;
    text.split_inclusive('\n')
        .enumerate()
        .map(move |(index, raw)| {
            let start = offset;
            offset += raw.len();
            let text = raw.trim_end_matches(['\n', '\r']);
            Line {
                number: index + 1,
                text,
                start,
                end: start + text.len(),
            }
        })
}

/// The trimmed text in `columns`, or `None` when the line ends before the
/// first column or the columns hold only blanks.
fn field(line: &str, columns: Columns) -> Option<&str> {
    let (start, end) = columns;
    if line.len() < start {
        return None;
    }
    let slice = line.get(start - 1..end.min(line.len()))?;
    let trimmed = slice.trim();
    (!trimmed.is_empty()).then_some(trimmed)
}

fn parse_float(raw: &str) -> Option<f64> {
    raw.parse::<f64>().ok().filter(|value| value.is_finite())
}

/// An integer field. Fortran writers emit an integral value with a trailing
/// decimal point (`1.`), which counts as the integer it names.
fn parse_int(raw: &str) -> Option<i64> {
    if let Ok(value) = raw.parse::<i64>() {
        return Some(value);
    }
    let value = parse_float(raw)?;
    if value.fract() != 0.0 || value.abs() >= 9.0e15 {
        return None;
    }
    // Integral and bounded by 9e15, so the cast is exact.
    #[allow(clippy::cast_possible_truncation)]
    let integer = value as i64;
    Some(integer)
}

#[derive(Clone, Copy, Debug)]
enum Section {
    Bus,
    Branch,
    LossZone,
    Interchange,
    TieLine,
}

impl Section {
    /// The section a header card opens, keyed on its first word: the format
    /// description states that only that word is significant.
    fn from_header(first_word: &str) -> Option<Self> {
        Some(match first_word.to_ascii_uppercase().as_str() {
            "BUS" => Self::Bus,
            "BRANCH" => Self::Branch,
            "LOSS" => Self::LossZone,
            "INTERCHANGE" => Self::Interchange,
            "TIE" => Self::TieLine,
            _ => return None,
        })
    }

    fn label(self) -> &'static str {
        match self {
            Self::Bus => "bus data",
            Self::Branch => "branch data",
            Self::LossZone => "loss zone",
            Self::Interchange => "interchange data",
            Self::TieLine => "tie line",
        }
    }

    fn terminator(self) -> &'static str {
        match self {
            Self::Bus | Self::Branch | Self::TieLine => "-999",
            Self::LossZone => "-99",
            Self::Interchange => "-9",
        }
    }
}

/// A section header and the records read under it so far.
struct OpenSection<'a> {
    section: Section,
    header: Line<'a>,
    declared: Option<usize>,
    records: usize,
}

/// `-999`, `-99`, or `-9`: a minus sign followed by digits.
fn is_terminator(word: &str) -> bool {
    word.strip_prefix('-')
        .is_some_and(|digits| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
}

/// The `N ITEMS` count on a section header, when the header states one.
fn declared_items(header: &str) -> Option<usize> {
    let words: Vec<&str> = header.split_whitespace().collect();
    let position = words
        .iter()
        .position(|word| word.to_ascii_uppercase().starts_with("ITEM"))?;
    words[..position].last()?.parse().ok()
}

/// A generator, control, or area reference to resolve once every bus is read.
struct PendingReference<'a> {
    line: Line<'a>,
    bus: BusId,
    target: ReferenceTarget,
}

/// Which table row a pending bus reference belongs to.
#[derive(Clone, Copy)]
enum ReferenceTarget {
    GeneratorRemote(usize),
    BranchControl(usize),
    AreaSlack(usize),
}

/// The typed tables under construction plus what the aggregate findings need.
#[derive(Default)]
struct Tables<'a> {
    buses: Vec<Bus>,
    bus_lines: HashMap<BusId, Line<'a>>,
    loads: Vec<Load>,
    shunts: Vec<Shunt>,
    generators: Vec<Generator>,
    branches: Vec<Branch>,
    branch_lines: Vec<Line<'a>>,
    areas: Vec<Area>,
    pending: Vec<PendingReference<'a>>,
    loss_zones: usize,
    tie_lines: usize,
    defaulted_voltage_limits: usize,
    unregulated_limits: usize,
    unity_taps: usize,
    defaulted_tap_limits: usize,
    defaulted_bands: usize,
    defaulted_steps: usize,
    retained_area_fields: usize,
}

struct Reader<'w> {
    origin: Option<TextOrigin>,
    warnings: &'w mut Diagnostics,
}

impl Reader<'_> {
    fn span(&self, line: &Line<'_>) -> Option<SourceSpan> {
        let origin = self.origin.as_ref()?;
        SourceSpan::new(
            origin.source.clone(),
            origin.offset + line.start as u64,
            origin.offset + line.end as u64,
        )
        .ok()
    }

    /// Record a finding located at `line`.
    fn report(&mut self, info: &'static DiagnosticInfo, line: &Line<'_>, message: impl AsRef<str>) {
        let message = format!("line {}: {}", line.number, message.as_ref());
        let diagnostic = match self.span(line) {
            Some(span) => Diagnostic::of(info, message)
                .with_span(span)
                .expect("a fresh record accepts one span"),
            None => Diagnostic::of(info, message),
        };
        self.warnings.record(diagnostic);
    }

    /// Record a failure located at `line` and return the error that ends the
    /// read. The diagnostic carries the span; the error carries the message.
    fn fail(&mut self, line: &Line<'_>, message: impl AsRef<str>) -> Error {
        self.report(&codes::PARSE_IEEE_CDF_MALFORMED, line, &message);
        Error::FormatRead {
            format: FMT,
            message: format!("line {}: {}", line.number, message.as_ref()),
        }
    }

    fn float_at(&mut self, line: &Line<'_>, columns: Columns, what: &str) -> Result<Option<f64>> {
        match field(line.text, columns) {
            None => Ok(None),
            Some(raw) => parse_float(raw).map(Some).ok_or_else(|| {
                self.fail(
                    line,
                    format!(
                        "{what} `{raw}` in columns {}-{} is not a number",
                        columns.0, columns.1
                    ),
                )
            }),
        }
    }

    fn int_at(&mut self, line: &Line<'_>, columns: Columns, what: &str) -> Result<Option<i64>> {
        match field(line.text, columns) {
            None => Ok(None),
            Some(raw) => parse_int(raw).map(Some).ok_or_else(|| {
                self.fail(
                    line,
                    format!(
                        "{what} `{raw}` in columns {}-{} is not an integer",
                        columns.0, columns.1
                    ),
                )
            }),
        }
    }

    /// A mandatory numeric field. An absent one is reported and read as zero,
    /// which is what the format tells a writer to put in a blank item.
    fn required_float(&mut self, line: &Line<'_>, columns: Columns, what: &str) -> Result<f64> {
        if let Some(value) = self.float_at(line, columns, what)? {
            return Ok(value);
        }
        self.truncated(line, columns, what, "zero");
        Ok(0.0)
    }

    fn required_int(
        &mut self,
        line: &Line<'_>,
        columns: Columns,
        what: &str,
        default: i64,
    ) -> Result<i64> {
        if let Some(value) = self.int_at(line, columns, what)? {
            return Ok(value);
        }
        self.truncated(line, columns, what, &default.to_string());
        Ok(default)
    }

    fn truncated(&mut self, line: &Line<'_>, columns: Columns, what: &str, read_as: &str) {
        self.report(
            &codes::READ_IEEE_CDF_RECORD_TRUNCATED,
            line,
            format!(
                "record has no {what} in columns {}-{}; read as {read_as}",
                columns.0, columns.1
            ),
        );
    }

    /// A bus number that identifies the record; a record without one cannot
    /// be placed in the network.
    fn required_id(&mut self, line: &Line<'_>, columns: Columns, what: &str) -> Result<BusId> {
        let Some(number) = self.int_at(line, columns, what)? else {
            return Err(self.fail(
                line,
                format!(
                    "record has no {what} in columns {}-{}",
                    columns.0, columns.1
                ),
            ));
        };
        usize::try_from(number)
            .map(BusId)
            .map_err(|_| self.fail(line, format!("{what} {number} is negative")))
    }

    /// The bus number in `columns` when the record states a positive one.
    fn bus_at(&mut self, line: &Line<'_>, columns: Columns, what: &str) -> Result<Option<BusId>> {
        Ok(self
            .int_at(line, columns, what)?
            .and_then(|number| usize::try_from(number).ok())
            .filter(|number| *number > 0)
            .map(BusId))
    }
}

/// The title card: the MVA base every per unit quantity refers to, the case
/// identification, and the date the case describes.
struct Title {
    base_mva: f64,
    case_id: Option<String>,
    case_date: Option<String>,
}

impl Reader<'_> {
    fn title(&mut self, line: &Line<'_>) -> Result<Title> {
        let Some(raw) = field(line.text, title_col::BASE_MVA) else {
            return Err(self.fail(
                line,
                format!(
                    "title card has no MVA base in columns {}-{}",
                    title_col::BASE_MVA.0,
                    title_col::BASE_MVA.1
                ),
            ));
        };
        let base_mva = match parse_float(raw) {
            Some(value) if value > 0.0 => value,
            _ => {
                return Err(self.fail(
                    line,
                    format!("title card MVA base `{raw}` is not a positive number"),
                ));
            }
        };
        let case_date = field(line.text, title_col::DATE).and_then(|raw| {
            let date = iso_date(raw);
            if date.is_none() && raw.bytes().any(|b| b.is_ascii_digit() && b != b'0') {
                self.report(
                    &codes::READ_IEEE_CDF_SOURCE_MALFORMED,
                    line,
                    format!("title card date `{raw}` is not MM/DD/YY; no case date recorded"),
                );
            }
            date
        });
        let retained: Vec<String> = [
            (title_col::ORIGINATOR, "originator"),
            (title_col::YEAR, "year"),
            (title_col::SEASON, "season"),
        ]
        .into_iter()
        .filter_map(|(columns, what)| {
            field(line.text, columns).map(|raw| format!("{what} `{raw}`"))
        })
        .collect();
        if !retained.is_empty() {
            self.report(
                &codes::READ_IEEE_CDF_RETAINED_SOURCE_ONLY,
                line,
                format!(
                    "title card {} survive in the retained source only",
                    retained.join(", ")
                ),
            );
        }
        Ok(Title {
            base_mva,
            case_id: field(line.text, title_col::CASE_ID).map(str::to_owned),
            case_date,
        })
    }
}

/// `MM/DD/YY` as an ISO 8601 date. Two digit years pivot at 1970, as the
/// PowSybl reader pivots them. A blank date (`0 /0 /0 `) is no date.
fn iso_date(raw: &str) -> Option<String> {
    let mut parts = raw.split('/');
    let month: u32 = parts.next()?.trim().parse().ok()?;
    let day: u32 = parts.next()?.trim().parse().ok()?;
    let year: u32 = parts.next()?.trim().parse().ok()?;
    if parts.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let year = match year {
        0..=69 => 2000 + year,
        70..=99 => 1900 + year,
        _ => year,
    };
    Some(format!("{year:04}-{month:02}-{day:02}"))
}

impl Reader<'_> {
    #[allow(clippy::too_many_lines)]
    fn bus<'a>(&mut self, line: &Line<'a>, base_mva: f64, tables: &mut Tables<'a>) -> Result<()> {
        let id = self.required_id(line, bus_col::NUMBER, "bus number")?;
        if let Some(previous) = tables.bus_lines.get(&id) {
            return Err(self.fail(
                line,
                format!(
                    "bus {id} is declared twice; the first record is at line {}",
                    previous.number
                ),
            ));
        }
        tables.bus_lines.insert(id, *line);
        let name = field(line.text, bus_col::NAME).map(str::to_owned);
        let area = self.required_int(line, bus_col::AREA, "area number", 1)?;
        let zone = self
            .int_at(line, bus_col::ZONE, "loss zone number")?
            .unwrap_or(1);
        let kind_code = self.required_int(line, bus_col::TYPE, "bus type", 0)?;
        let kind_code = match kind_code {
            0..=3 => kind_code,
            other => {
                self.report(
                    &codes::READ_IEEE_CDF_VALUE_SUBSTITUTED,
                    line,
                    format!("bus {id} type {other} is outside 0 through 3; read as type 0 (PQ)"),
                );
                0
            }
        };
        let vm = self.required_float(line, bus_col::VM, "final voltage")?;
        let va = self.required_float(line, bus_col::VA, "final angle")?;
        let pd = self.required_float(line, bus_col::PD, "load MW")?;
        let qd = self.required_float(line, bus_col::QD, "load MVAr")?;
        let pg = self.required_float(line, bus_col::PG, "generation MW")?;
        let qg = self.required_float(line, bus_col::QG, "generation MVAr")?;
        let base_kv = self
            .float_at(line, bus_col::BASE_KV, "base kV")?
            .unwrap_or(0.0);
        let v_desired = self.float_at(line, bus_col::V_DESIRED, "desired voltage")?;
        let limit_max = self
            .float_at(line, bus_col::LIMIT_MAX, "maximum limit")?
            .unwrap_or(0.0);
        let limit_min = self
            .float_at(line, bus_col::LIMIT_MIN, "minimum limit")?
            .unwrap_or(0.0);
        let g = self.required_float(line, bus_col::G, "shunt conductance")?;
        let b = self.required_float(line, bus_col::B, "shunt susceptance")?;
        let remote = self.bus_at(line, bus_col::REMOTE_BUS, "remote controlled bus")?;

        let kind = match kind_code {
            2 => BusType::Pv,
            3 => BusType::Ref,
            _ => BusType::Pq,
        };
        let mut bus = Bus::new(id, kind, base_kv);
        bus.vm = vm;
        bus.va = va;
        bus.area = usize::try_from(area).unwrap_or(0);
        bus.zone = usize::try_from(zone).unwrap_or(0);
        bus.name = name;
        match kind_code {
            // The limit columns bound the voltage of a bus that holds its
            // reactive generation.
            1 if limit_max > limit_min => {
                bus.vmax = limit_max;
                bus.vmin = limit_min;
            }
            0 if limit_max != 0.0 || limit_min != 0.0 => {
                tables.unregulated_limits += 1;
                tables.defaulted_voltage_limits += 1;
            }
            _ => tables.defaulted_voltage_limits += 1,
        }
        tables.buses.push(bus);

        if pd != 0.0 || qd != 0.0 {
            tables.loads.push(Load::new(id, pd, qd));
        }
        if g != 0.0 || b != 0.0 {
            tables
                .shunts
                .push(Shunt::new(id, g * base_mva, b * base_mva));
        }
        // Types 1 through 3 declare a machine at the bus; an unregulated bus
        // with a stated output carries one too, so the injection survives.
        let regulates = matches!(kind_code, 2 | 3);
        if kind_code != 0 || pg != 0.0 || qg != 0.0 {
            let mut generator = Generator::new(id);
            generator.pg = pg;
            generator.qg = qg;
            generator.vg = v_desired.filter(|v| *v > 0.0).unwrap_or(vm);
            (generator.qmax, generator.qmin) = if regulates {
                (limit_max, limit_min)
            } else {
                (qg, qg)
            };
            generator.pmax = DEFAULT_PMAX_MW;
            generator.pmin = DEFAULT_PMIN_MW;
            generator.mbase = base_mva;
            generator.voltage_regulation_on = regulates;
            if let Some(remote) = remote.filter(|remote| *remote != id) {
                tables.pending.push(PendingReference {
                    line: *line,
                    bus: remote,
                    target: ReferenceTarget::GeneratorRemote(tables.generators.len()),
                });
            }
            tables.generators.push(generator);
        }
        Ok(())
    }

    #[allow(clippy::too_many_lines)]
    fn branch<'a>(
        &mut self,
        line: &Line<'a>,
        base_mva: f64,
        tables: &mut Tables<'a>,
    ) -> Result<()> {
        let from = self.required_id(line, branch_col::FROM, "tap bus number")?;
        let to = self.required_id(line, branch_col::TO, "Z bus number")?;
        // The branch area and loss zone repeat the bus columns and have no
        // branch field in the balanced model; the sequence column is a row
        // index. One file level remark covers them.
        let circuit = self.int_at(line, branch_col::CIRCUIT, "circuit")?;
        // The public 9 bus case leaves the type blank on its lines, which the
        // PowSybl reader reads as a transmission line; so does this one.
        let kind = self
            .int_at(line, branch_col::TYPE, "branch type")?
            .unwrap_or(0);
        let kind = match kind {
            0..=4 => kind,
            other => {
                self.report(
                    &codes::READ_IEEE_CDF_VALUE_SUBSTITUTED,
                    line,
                    format!(
                        "branch {from}-{to} type {other} is outside 0 through 4; read as a \
                         line when the turns ratio is zero and as a fixed tap transformer otherwise"
                    ),
                );
                0
            }
        };
        let r = self.required_float(line, branch_col::R, "resistance")?;
        let x = self.required_float(line, branch_col::X, "reactance")?;
        let b = self.required_float(line, branch_col::B, "line charging")?;
        if r == 0.0 && x == 0.0 {
            self.report(
                &codes::READ_IEEE_CDF_SOURCE_MALFORMED,
                line,
                format!(
                    "branch {from}-{to} states zero series impedance, which the format forbids; \
                     the matrix builders reject the branch"
                ),
            );
        }
        let rate_a = self
            .float_at(line, branch_col::RATE_A, "MVA rating 1")?
            .unwrap_or(0.0);
        let rate_b = self
            .float_at(line, branch_col::RATE_B, "MVA rating 2")?
            .unwrap_or(0.0);
        let rate_c = self
            .float_at(line, branch_col::RATE_C, "MVA rating 3")?
            .unwrap_or(0.0);
        let control_bus = self.bus_at(line, branch_col::CONTROL_BUS, "control bus number")?;
        let side = self.int_at(line, branch_col::SIDE, "side")?;
        let ratio = self
            .float_at(line, branch_col::RATIO, "turns ratio")?
            .unwrap_or(0.0);
        let angle = self
            .float_at(line, branch_col::ANGLE, "phase shift angle")?
            .unwrap_or(0.0);
        let tap_min = self
            .float_at(line, branch_col::TAP_MIN, "minimum tap")?
            .unwrap_or(0.0);
        let tap_max = self
            .float_at(line, branch_col::TAP_MAX, "maximum tap")?
            .unwrap_or(0.0);
        let step = self
            .float_at(line, branch_col::STEP, "step size")?
            .unwrap_or(0.0);
        let (limit_min, limit_max) = self.branch_limits(line)?;

        let mut branch = Branch::new(from, to, r, x);
        branch.b = b;
        branch.rate_a = rate_a;
        branch.rate_b = rate_b;
        branch.rate_c = rate_c;
        branch.shift = angle;
        branch.tap = if kind == 0 || ratio != 0.0 {
            ratio
        } else {
            tables.unity_taps += 1;
            1.0
        };
        if let Some(circuit) = circuit.filter(|circuit| *circuit != 1) {
            branch
                .extras
                .insert("ieee_cdf_circuit".into(), Value::from(circuit));
        }
        let side = match side {
            None | Some(0..=2) => side.unwrap_or(0),
            Some(other) => {
                self.report(
                    &codes::READ_IEEE_CDF_VALUE_SUBSTITUTED,
                    line,
                    format!(
                        "branch {from}-{to} side {other} is outside 0 through 2; read as 0 \
                         (the controlled bus is one of the terminals)"
                    ),
                );
                0
            }
        };
        if matches!(kind, 2..=4) {
            let mode = match kind {
                2 => TransformerControlMode::Voltage,
                3 => TransformerControlMode::ReactiveFlow,
                _ => TransformerControlMode::ActiveFlow,
            };
            let mut control = TransformerControl::new(mode);
            control.mva_base = base_mva;
            control.controlled_bus = control_bus.or(match side {
                1 => Some(from),
                2 => Some(to),
                _ => None,
            });
            if tap_max > tap_min {
                control.tap_min = tap_min;
                control.tap_max = tap_max;
            } else {
                tables.defaulted_tap_limits += 1;
                if mode == TransformerControlMode::ActiveFlow {
                    control.tap_min = -DEFAULT_ANGLE_LIMIT_DEG;
                    control.tap_max = DEFAULT_ANGLE_LIMIT_DEG;
                }
            }
            if limit_max > limit_min {
                control.band_min = limit_min;
                control.band_max = limit_max;
            } else {
                tables.defaulted_bands += 1;
            }
            if step > 0.0 && tap_max > tap_min {
                let positions = ((tap_max - tap_min) / step).round();
                // A bounded, rounded, nonnegative count; the cast is exact.
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                {
                    control.ntp = (positions.min(f64::from(u32::MAX - 1)) as u32).max(1) + 1;
                }
            } else {
                tables.defaulted_steps += 1;
            }
            if let Some(controlled) = control_bus.filter(|bus| *bus != from && *bus != to) {
                tables.pending.push(PendingReference {
                    line: *line,
                    bus: controlled,
                    target: ReferenceTarget::BranchControl(tables.branches.len()),
                });
            }
            branch.control = Some(control);
        }
        tables.branches.push(branch);
        tables.branch_lines.push(*line);
        Ok(())
    }

    /// The minimum and maximum voltage, MVAr, or MW limits. When the wide
    /// minimum column does not hold one number, the record uses the narrower
    /// layout of the public IEEE archive files and the maximum starts one
    /// column earlier.
    fn branch_limits(&mut self, line: &Line<'_>) -> Result<(f64, f64)> {
        let wide = field(line.text, branch_col::LIMIT_MIN);
        if wide.is_none_or(|raw| parse_float(raw).is_some()) {
            let min = self
                .float_at(line, branch_col::LIMIT_MIN, "minimum limit")?
                .unwrap_or(0.0);
            let max = self
                .float_at(line, branch_col::LIMIT_MAX, "maximum limit")?
                .unwrap_or(0.0);
            return Ok((min, max));
        }
        let min = self
            .float_at(line, branch_col::LIMIT_MIN_NARROW, "minimum limit")?
            .unwrap_or(0.0);
        let max = self
            .float_at(line, branch_col::LIMIT_MAX_SHIFTED, "maximum limit")?
            .unwrap_or(0.0);
        Ok((min, max))
    }

    fn loss_zone<'a>(&mut self, line: &Line<'a>, tables: &mut Tables<'a>) -> Result<()> {
        if self
            .int_at(line, zone_col::NUMBER, "loss zone number")?
            .is_none()
        {
            self.truncated(line, zone_col::NUMBER, "loss zone number", "no zone");
            return Ok(());
        }
        if field(line.text, zone_col::NAME).is_some() {
            tables.loss_zones += 1;
        }
        Ok(())
    }

    fn interchange<'a>(&mut self, line: &Line<'a>, tables: &mut Tables<'a>) -> Result<()> {
        let number = self.required_id(line, interchange_col::AREA, "area number")?;
        let slack = self.bus_at(line, interchange_col::SLACK_BUS, "interchange slack bus")?;
        let export = self.required_float(line, interchange_col::EXPORT, "interchange export")?;
        let tolerance =
            self.required_float(line, interchange_col::TOLERANCE, "interchange tolerance")?;
        let code = field(line.text, interchange_col::CODE);
        let name = field(line.text, interchange_col::NAME);
        let swing_name = field(line.text, interchange_col::SWING_NAME);
        let mut area = Area::new(number.0);
        area.net_interchange = export;
        area.tolerance = tolerance;
        area.name = name.or(code).map(str::to_owned);
        if swing_name.is_some() || (name.is_some() && code.is_some()) {
            tables.retained_area_fields += 1;
        }
        if let Some(slack) = slack {
            tables.pending.push(PendingReference {
                line: *line,
                bus: slack,
                target: ReferenceTarget::AreaSlack(tables.areas.len()),
            });
        }
        tables.areas.push(area);
        Ok(())
    }

    fn tie_line<'a>(&mut self, line: &Line<'a>, tables: &mut Tables<'a>) -> Result<()> {
        let metered = self.int_at(line, tie_col::METERED_BUS, "metered bus number")?;
        let other = self.int_at(line, tie_col::OTHER_BUS, "non-metered bus number")?;
        if metered.is_none() || other.is_none() {
            self.truncated(
                line,
                tie_col::OTHER_BUS,
                "tie line bus numbers",
                "no tie line",
            );
            return Ok(());
        }
        tables.tie_lines += 1;
        Ok(())
    }

    /// Close a section: the declared item count must match the records read.
    fn close(&mut self, open: &OpenSection<'_>) {
        if let Some(declared) = open.declared
            && declared != open.records
        {
            self.report(
                &codes::READ_IEEE_CDF_SOURCE_MALFORMED,
                &open.header,
                format!(
                    "{} header declares {declared} item(s); {} record(s) follow",
                    open.section.label(),
                    open.records
                ),
            );
        }
    }

    /// Point every stored bus reference at a declared bus, dropping the
    /// references the case cannot satisfy.
    fn resolve_references(&mut self, tables: &mut Tables<'_>) {
        let pending = std::mem::take(&mut tables.pending);
        for reference in pending {
            let declared = tables.bus_lines.contains_key(&reference.bus);
            match reference.target {
                ReferenceTarget::GeneratorRemote(index) => {
                    if declared {
                        tables.generators[index].regulated_bus = Some(reference.bus);
                    } else {
                        self.report(
                            &codes::READ_IEEE_CDF_SOURCE_MALFORMED,
                            &reference.line,
                            format!(
                                "remote controlled bus {} is not declared; the generator at bus {} \
                                 regulates its own bus",
                                reference.bus, tables.generators[index].bus
                            ),
                        );
                    }
                }
                ReferenceTarget::BranchControl(index) => {
                    if !declared {
                        let branch = &mut tables.branches[index];
                        self.report(
                            &codes::READ_IEEE_CDF_SOURCE_MALFORMED,
                            &reference.line,
                            format!(
                                "control bus {} is not declared; transformer {}-{} regulates \
                                 its own terminal",
                                reference.bus, branch.from, branch.to
                            ),
                        );
                        if let Some(control) = branch.control.as_mut() {
                            control.controlled_bus = None;
                        }
                    }
                }
                ReferenceTarget::AreaSlack(index) => {
                    if declared {
                        tables.areas[index].slack_bus = Some(reference.bus);
                    } else {
                        self.report(
                            &codes::READ_IEEE_CDF_SOURCE_MALFORMED,
                            &reference.line,
                            format!(
                                "interchange slack bus {} of area {} is not declared; \
                                 the area has no slack bus",
                                reference.bus, tables.areas[index].number
                            ),
                        );
                    }
                }
            }
        }
    }

    /// The findings that summarize the whole file rather than one record.
    fn summarize(&mut self, tables: &Tables<'_>, base_mva: f64) {
        if tables.defaulted_voltage_limits > 0 {
            self.warnings.push(
                &codes::READ_IEEE_CDF_VALUE_DEFAULTED,
                format!(
                    "{} bus(es) state no voltage limits; vmax 1.1 and vmin 0.9 p.u. assumed",
                    tables.defaulted_voltage_limits
                ),
            );
        }
        if !tables.generators.is_empty() {
            self.warnings.push(
                &codes::READ_IEEE_CDF_VALUE_DEFAULTED,
                format!(
                    "{} generator(s): the format states no active power limits or machine base; \
                     pmin {DEFAULT_PMIN_MW} MW, pmax {DEFAULT_PMAX_MW} MW, and mbase equal to \
                     the {base_mva} MVA system base assumed",
                    tables.generators.len()
                ),
            );
        }
        if tables.unity_taps > 0 {
            self.warnings.push(
                &codes::READ_IEEE_CDF_VALUE_DEFAULTED,
                format!(
                    "{} transformer(s) of type 1 through 4 state no turns ratio; unity ratio \
                     assumed",
                    tables.unity_taps
                ),
            );
        }
        let controls = [
            (tables.defaulted_tap_limits, "tap limits"),
            (tables.defaulted_bands, "controlled quantity band"),
            (tables.defaulted_steps, "step size"),
        ];
        for (count, what) in controls {
            if count > 0 {
                self.warnings.push(
                    &codes::READ_IEEE_CDF_VALUE_DEFAULTED,
                    format!(
                        "{count} regulating transformer(s) state no {what}; the PSS/E default \
                         control block values assumed"
                    ),
                );
            }
        }
        if tables.unregulated_limits > 0 {
            self.warnings.push(
                &codes::READ_IEEE_CDF_RETAINED_SOURCE_ONLY,
                format!(
                    "{} unregulated (type 0) bus(es) state MVAr or voltage limits, which \
                     survive in the retained source only",
                    tables.unregulated_limits
                ),
            );
        }
        if !tables.branches.is_empty() {
            self.warnings.push(
                &codes::READ_IEEE_CDF_RETAINED_SOURCE_ONLY,
                format!(
                    "branch area, loss zone, and sequence columns of {} branch(es) survive in \
                     the retained source only",
                    tables.branches.len()
                ),
            );
        }
        if tables.loss_zones > 0 {
            self.warnings.push(
                &codes::READ_IEEE_CDF_RETAINED_SOURCE_ONLY,
                format!(
                    "{} loss zone name(s) survive in the retained source only; each bus keeps \
                     its zone number",
                    tables.loss_zones
                ),
            );
        }
        if tables.retained_area_fields > 0 {
            self.warnings.push(
                &codes::READ_IEEE_CDF_RETAINED_SOURCE_ONLY,
                format!(
                    "alternate swing bus names and area codes of {} area(s) survive in the \
                     retained source only",
                    tables.retained_area_fields
                ),
            );
        }
        if tables.tie_lines > 0 {
            self.warnings.push(
                &codes::READ_IEEE_CDF_RETAINED_SOURCE_ONLY,
                format!(
                    "{} tie line record(s) survive in the retained source only; the balanced \
                     model has no metered tie line table",
                    tables.tie_lines
                ),
            );
        }
    }
}

/// Parse a CDF case into a [`BalancedNetwork`]. `origin` locates the decoded
/// text inside the retained buffer so record findings carry spans.
///
/// # Errors
/// A title card without a positive MVA base, a record without its bus
/// numbers or with a field that is not a number, a bus declared twice, and a
/// branch on an undeclared bus end the read; every such failure also leaves a
/// spanned `PARSE.IEEE_CDF.MALFORMED` finding.
#[allow(clippy::too_many_lines)]
pub(crate) fn parse_ieee_cdf_source(
    text: &str,
    name_hint: Option<&str>,
    origin: Option<TextOrigin>,
    warnings: &mut Diagnostics,
) -> Result<BalancedNetwork> {
    let mut reader = Reader { origin, warnings };
    let mut lines = lines(text);
    let Some(title_line) = lines.next() else {
        return Err(Error::FormatRead {
            format: FMT,
            message: "the source is empty; a CDF case opens with a title card".into(),
        });
    };
    let title = reader.title(&title_line)?;
    let base_mva = title.base_mva;

    let mut tables = Tables::default();
    let mut open: Option<OpenSection<'_>> = None;
    let mut ended = false;
    for line in lines {
        let trimmed = line.text.trim();
        let Some(first) = trimmed.split_whitespace().next() else {
            continue;
        };
        if ended {
            reader.report(
                &codes::READ_IEEE_CDF_SOURCE_MALFORMED,
                &line,
                "text after END OF DATA is ignored",
            );
            break;
        }
        if is_terminator(first) {
            match open.take() {
                Some(section) => {
                    if first != section.section.terminator() {
                        reader.report(
                            &codes::READ_IEEE_CDF_SOURCE_MALFORMED,
                            &line,
                            format!(
                                "{} section ends with `{first}` rather than `{}`",
                                section.section.label(),
                                section.section.terminator()
                            ),
                        );
                    }
                    reader.close(&section);
                }
                None => reader.report(
                    &codes::READ_IEEE_CDF_SOURCE_MALFORMED,
                    &line,
                    format!("terminator `{first}` outside a section is ignored"),
                ),
            }
            continue;
        }
        // Header and end cards start with a letter in column 1; a record
        // starts with its right justified bus number, so one that lost the
        // number opens with blanks and is not mistaken for a header.
        let card = line.text.starts_with(|c: char| c.is_ascii_alphabetic());
        if card && first.eq_ignore_ascii_case("END") {
            if let Some(section) = open.take() {
                reader.unterminated(&section, &line);
            }
            ended = true;
            continue;
        }
        if card && let Some(next) = Section::from_header(first) {
            if let Some(section) = open.take() {
                reader.unterminated(&section, &line);
            }
            open = Some(OpenSection {
                section: next,
                header: line,
                declared: declared_items(trimmed),
                records: 0,
            });
            continue;
        }
        let Some(section) = open.as_mut() else {
            reader.report(
                &codes::READ_IEEE_CDF_SOURCE_MALFORMED,
                &line,
                "record outside a section is ignored",
            );
            continue;
        };
        section.records += 1;
        match section.section {
            Section::Bus => reader.bus(&line, base_mva, &mut tables)?,
            Section::Branch => reader.branch(&line, base_mva, &mut tables)?,
            Section::LossZone => reader.loss_zone(&line, &mut tables)?,
            Section::Interchange => reader.interchange(&line, &mut tables)?,
            Section::TieLine => reader.tie_line(&line, &mut tables)?,
        }
    }
    if let Some(section) = open.take() {
        let last = Line {
            number: text.lines().count().max(1),
            text: "",
            start: text.len(),
            end: text.len(),
        };
        reader.unterminated(&section, &last);
    }

    for (branch, line) in tables.branches.iter().zip(&tables.branch_lines) {
        for bus in [branch.from, branch.to] {
            if !tables.bus_lines.contains_key(&bus) {
                return Err(reader.fail(
                    line,
                    format!(
                        "branch {}-{} references undeclared bus {bus}",
                        branch.from, branch.to
                    ),
                ));
            }
        }
    }
    reader.resolve_references(&mut tables);
    reader.summarize(&tables, base_mva);

    let name = name_hint
        .map(str::to_owned)
        .or(title.case_id)
        .unwrap_or_else(|| "case".to_owned());
    let net = BalancedNetwork::from_tables(BalancedNetworkTables {
        name,
        base_mva,
        base_frequency: crate::network::DEFAULT_BASE_FREQUENCY,
        geo: None,
        case_metadata: CaseMetadata {
            case_date: title.case_date,
            ..CaseMetadata::default()
        },
        detailed_connectivity: None,
        buses: tables.buses.into(),
        loads: tables.loads.into(),
        shunts: tables.shunts.into(),
        static_var_compensators: Vec::new().into(),
        branches: tables.branches.into(),
        switches: Vec::new().into(),
        generators: tables.generators.into(),
        storage: Vec::new().into(),
        hvdc: Vec::new().into(),
        transformers_3w: Vec::new().into(),
        areas: tables.areas.into(),
        solver: None,
        source_format: SourceFormat::IeeeCdf,
    });
    net.check_references(FMT)?;
    Ok(net)
}

impl Reader<'_> {
    fn unterminated(&mut self, section: &OpenSection<'_>, at: &Line<'_>) {
        self.report(
            &codes::READ_IEEE_CDF_SOURCE_MALFORMED,
            at,
            format!(
                "{} section is not closed by `{}`",
                section.section.label(),
                section.section.terminator()
            ),
        );
        self.close(section);
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::float_cmp)]

    use super::*;
    use crate::format::test_parse::{TestParsed, parse_str};

    // Records copied from the public IEEE 14 and 300 bus archive files, so
    // the column layout under test is the one those files use.
    const TITLE: &str = " 08/19/93 UW ARCHIVE           100.0  1962 W IEEE 14 Bus Test Case";
    const BUS_1: &str = "   1 Bus 1     HV  1  1  3 1.060    0.0      0.0      0.0    232.4   -16.9     0.0  1.060     0.0     0.0   0.0    0.0        0";
    const BUS_2: &str = "   2 Bus 2     HV  1  1  2 1.045  -4.98     21.7     12.7     40.0    42.4     0.0  1.045    50.0   -40.0   0.0    0.0        0";
    const BUS_9: &str = "   9 Bus 9     LV  1  1  0 1.056 -14.94     29.5     16.6      0.0     0.0     0.0  0.0       0.0     0.0   0.0    0.19       0";
    const LINE_1_2: &str = "   1    2  1  1 1 0  0.01938   0.05917     0.0528     0     0     0    0 0  0.0       0.0 0.0    0.0     0.0    0.0   0.0";
    const TRANSFORMER_4_7: &str = "   4    7  1  1 1 0  0.0       0.20912     0.0        0     0     0    0 0  0.978     0.0 0.0    0.0     0.0    0.0   0.0";
    const TRANSFORMER_5_6: &str = "   5    6  1  1 1 0  0.0       0.25202     0.0        0     0     0    0 0  0.932     0.0 0.0    0.0     0.0    0.0   0.0";
    const LTC_9001_9006: &str = "9001 9006  1  9 1 2  0.024390  0.436820   0.00000     0     0     0 9006 0  0.9668    0.00 0.9391 1.1478 .00417  0.9900 1.0100     3";
    const INTERCHANGE: &str = " 1    2 Bus 2     HV    0.0  999.99  IEEE14  IEEE 14 Bus Test Case";

    fn two_bus() -> String {
        let bus_2 = BUS_9.replace("   9 Bus 9     LV", "   2 Bus 2     HV");
        format!(
            "{TITLE}\n\
             BUS DATA FOLLOWS                             2 ITEMS\n\
             {BUS_1}\n{bus_2}\n-999\n\
             BRANCH DATA FOLLOWS                          1 ITEMS\n\
             {LINE_1_2}\n-999\n\
             LOSS ZONES FOLLOWS                     1 ITEMS\n  1 IEEE 14 BUS\n-99\n\
             INTERCHANGE DATA FOLLOWS                 1 ITEMS\n{INTERCHANGE}\n-9\n\
             TIE LINES FOLLOWS                     0 ITEMS\n-999\n\
             END OF DATA\n"
        )
    }

    fn codes_of(parsed: &TestParsed) -> Vec<&str> {
        parsed.diagnostics.iter().map(Diagnostic::code).collect()
    }

    fn messages(parsed: &TestParsed, code: &str) -> Vec<String> {
        parsed
            .diagnostics
            .iter()
            .filter(|d| d.code() == code)
            .map(|d| d.message().to_owned())
            .collect()
    }

    #[test]
    fn the_title_card_and_the_bus_header_are_both_signatures() {
        assert!(looks_like_ieee_cdf(two_bus().as_bytes()));
        assert!(looks_like_ieee_cdf(
            b"                               100.0\nBUS DATA FOLLOWS\n"
        ));
        assert!(!looks_like_ieee_cdf(b"function mpc = case9\n"));
        assert!(!looks_like_ieee_cdf(b"0, 100.0, 33, 0, 0, 60.0\n"));
        assert!(!looks_like_ieee_cdf(b""));
    }

    #[test]
    fn reads_the_two_bus_case() {
        let parsed = parse_str(&two_bus(), "ieee-cdf").unwrap();
        let net = &parsed.network;
        assert_eq!(net.source_format(), SourceFormat::IeeeCdf);
        assert_eq!(net.base_mva(), 100.0);
        assert_eq!(net.case_metadata().case_date.as_deref(), Some("1993-08-19"));
        assert_eq!(net.buses().len(), 2);
        assert_eq!(net.buses()[0].kind, BusType::Ref);
        assert_eq!(net.buses()[0].name.as_deref(), Some("Bus 1     HV"));
        assert_eq!((net.buses()[0].vm, net.buses()[0].va), (1.06, 0.0));
        assert_eq!(net.buses()[1].kind, BusType::Pq);
        assert_eq!((net.buses()[1].vm, net.buses()[1].va), (1.056, -14.94));
        assert_eq!((net.buses()[1].area, net.buses()[1].zone), (1, 1));
        assert_eq!(net.loads().len(), 1);
        assert_eq!(net.loads()[0].bus, BusId(2));
        assert_eq!((net.loads()[0].p, net.loads()[0].q), (29.5, 16.6));
        assert_eq!(net.shunts().len(), 1);
        assert_eq!(net.shunts()[0].bus, BusId(2));
        assert_eq!((net.shunts()[0].g, net.shunts()[0].b), (0.0, 19.0));
        assert_eq!(net.generators().len(), 1);
        let generator = &net.generators()[0];
        assert_eq!(generator.bus, BusId(1));
        assert_eq!((generator.pg, generator.qg), (232.4, -16.9));
        assert_eq!(generator.vg, 1.06);
        assert_eq!((generator.qmax, generator.qmin), (0.0, 0.0));
        assert_eq!((generator.pmin, generator.pmax), (0.0, 9999.0));
        assert_eq!(generator.mbase, 100.0);
        assert!(generator.voltage_regulation_on);
        assert_eq!(generator.regulated_bus, None);
        assert_eq!(net.branches().len(), 1);
        let branch = &net.branches()[0];
        assert_eq!((branch.from, branch.to), (BusId(1), BusId(2)));
        assert_eq!((branch.r, branch.x, branch.b), (0.01938, 0.05917, 0.0528));
        assert_eq!((branch.tap, branch.shift), (0.0, 0.0));
        assert!(!branch.is_transformer());
        assert!(branch.control.is_none());
        assert!(branch.extras.is_empty());
        assert_eq!(net.areas().len(), 1);
        let area = &net.areas()[0];
        assert_eq!(area.number, 1);
        assert_eq!(area.slack_bus, Some(BusId(2)));
        assert_eq!((area.net_interchange, area.tolerance), (0.0, 999.99));
        assert_eq!(area.name.as_deref(), Some("IEEE 14 Bus Test Case"));

        let codes = codes_of(&parsed);
        assert!(
            codes.contains(&"READ.IEEE_CDF.VALUE_DEFAULTED"),
            "{codes:?}"
        );
        assert!(
            codes.contains(&"READ.IEEE_CDF.RETAINED_SOURCE_ONLY"),
            "{codes:?}"
        );
        assert!(
            !codes.contains(&"READ.IEEE_CDF.SOURCE_MALFORMED"),
            "{:?}",
            parsed.render_diagnostics()
        );
        assert!(
            !codes.contains(&"READ.IEEE_CDF.RECORD_TRUNCATED"),
            "{:?}",
            parsed.render_diagnostics()
        );
    }

    #[test]
    fn the_name_hint_wins_over_the_case_identification() {
        let parsed = parse_str(&two_bus(), "ieee-cdf").unwrap();
        assert_eq!(parsed.network.name(), "IEEE 14 Bus Test Case");
        let mut warnings = Diagnostics::new();
        let net =
            parse_ieee_cdf_source(&two_bus(), Some("ieee14cdf"), None, &mut warnings).unwrap();
        assert_eq!(net.name(), "ieee14cdf");
    }

    #[test]
    fn a_truncated_bus_record_is_reported_with_its_span() {
        let case = two_bus();
        let full = BUS_9.replace("   9 Bus 9     LV", "   2 Bus 2     HV");
        let short = &full[..48];
        let cut = case.replace(&full, short);
        let parsed = parse_str(&cut, "ieee-cdf").unwrap();
        let truncated: Vec<&Diagnostic> = parsed
            .diagnostics
            .iter()
            .filter(|d| d.code() == "READ.IEEE_CDF.RECORD_TRUNCATED")
            .collect();
        // Load MVAr, generation MW and MVAr, and both shunt columns are gone;
        // the optional base kV, desired voltage, and limit columns are not
        // reported.
        assert_eq!(truncated.len(), 5, "{:?}", parsed.render_diagnostics());
        let line_start = cut.find(short).unwrap() as u64;
        let line_end = line_start + short.len() as u64;
        for diagnostic in &truncated {
            assert!(
                diagnostic.message().starts_with("line 4: "),
                "{diagnostic:?}"
            );
            assert_eq!(diagnostic.spans().len(), 1);
            let span = &diagnostic.spans()[0];
            assert_eq!((span.byte_start(), span.byte_end()), (line_start, line_end));
        }
        assert_eq!(
            (parsed.network.loads()[0].p, parsed.network.loads()[0].q),
            (29.5, 0.0)
        );
        assert!(parsed.network.shunts().is_empty());
    }

    #[test]
    fn a_record_without_a_bus_number_fails_with_a_spanned_finding() {
        let broken = two_bus().replace("   2 Bus 2", "     Bus 2");
        let error = parse_str(&broken, "ieee-cdf").unwrap_err();
        assert!(error.to_string().contains("line 4"), "{error}");
        let malformed: Vec<_> = error
            .diagnostics()
            .iter()
            .filter(|d| d.code() == "PARSE.IEEE_CDF.MALFORMED")
            .collect();
        assert_eq!(malformed.len(), 1, "{:?}", error.diagnostics());
        assert_eq!(malformed[0].spans().len(), 1);
        assert_eq!(
            malformed[0].severity(),
            powerio_core::DiagnosticSeverity::Error
        );
    }

    #[test]
    fn a_non_numeric_field_fails() {
        let broken = two_bus().replace("0.01938", "0.0193x");
        let error = parse_str(&broken, "ieee-cdf").unwrap_err();
        assert!(error.to_string().contains("resistance"), "{error}");
        assert!(error.to_string().contains("line 7"), "{error}");
    }

    #[test]
    fn a_duplicate_bus_and_an_undeclared_endpoint_fail() {
        let duplicate = two_bus().replace("   2 Bus 2", "   1 Bus 2");
        let error = parse_str(&duplicate, "ieee-cdf").unwrap_err();
        assert!(error.to_string().contains("declared twice"), "{error}");

        let dangling = two_bus().replace("   1    2  1  1 1 0", "   1    3  1  1 1 0");
        let error = parse_str(&dangling, "ieee-cdf").unwrap_err();
        assert!(error.to_string().contains("undeclared bus 3"), "{error}");
        assert_eq!(error.diagnostics().last().unwrap().spans().len(), 1);
    }

    #[test]
    fn a_title_card_without_a_base_fails() {
        let no_base = two_bus().replacen("100.0", "     ", 1);
        let error = parse_str(&no_base, "ieee-cdf").unwrap_err();
        assert!(error.to_string().contains("MVA base"), "{error}");
        assert!(
            error
                .diagnostics()
                .iter()
                .any(|d| d.code() == "PARSE.IEEE_CDF.MALFORMED" && d.spans().len() == 1),
            "{:?}",
            error.diagnostics()
        );

        let error = parse_str("", "ieee-cdf").unwrap_err();
        assert!(error.to_string().contains("title card"), "{error}");
    }

    #[test]
    fn a_type_one_bus_holds_its_reactive_generation_within_voltage_limits() {
        // Type 1, no desired voltage, and the limit columns as a voltage band.
        let held = BUS_2
            .replace("  1  1  2 1.045", "  1  1  1 1.045")
            .replace("  1.045    50.0   -40.0", "  0.0      1.05    0.95");
        let case = two_bus().replace(
            &BUS_9.replace("   9 Bus 9     LV", "   2 Bus 2     HV"),
            &held,
        );
        let parsed = parse_str(&case, "ieee-cdf").unwrap();
        let bus = &parsed.network.buses()[1];
        assert_eq!(bus.kind, BusType::Pq);
        assert_eq!((bus.vmax, bus.vmin), (1.05, 0.95));
        let generator = &parsed.network.generators()[1];
        assert_eq!(generator.bus, BusId(2));
        assert_eq!((generator.pg, generator.qg), (40.0, 42.4));
        assert_eq!((generator.qmax, generator.qmin), (42.4, 42.4));
        assert!(!generator.voltage_regulation_on);
        assert_eq!(generator.vg, 1.045);
        // One bus took the model's voltage limits, the other stated its own.
        let defaulted = messages(&parsed, "READ.IEEE_CDF.VALUE_DEFAULTED");
        assert!(
            defaulted
                .iter()
                .any(|m| m.starts_with("1 bus(es) state no voltage limits")),
            "{defaulted:?}"
        );
    }

    #[test]
    fn an_unregulated_bus_with_output_keeps_a_fixed_generator() {
        let injecting = BUS_2.replace("  1  1  2 1.045", "  1  1  0 1.045");
        let case = two_bus().replace(
            &BUS_9.replace("   9 Bus 9     LV", "   2 Bus 2     HV"),
            &injecting,
        );
        let parsed = parse_str(&case, "ieee-cdf").unwrap();
        assert_eq!(parsed.network.buses()[1].kind, BusType::Pq);
        assert_eq!(
            (
                parsed.network.buses()[1].vmax,
                parsed.network.buses()[1].vmin
            ),
            (1.1, 0.9)
        );
        let generator = &parsed.network.generators()[1];
        assert_eq!((generator.pg, generator.qg), (40.0, 42.4));
        assert!(!generator.voltage_regulation_on);
        let retained = messages(&parsed, "READ.IEEE_CDF.RETAINED_SOURCE_ONLY");
        assert!(
            retained
                .iter()
                .any(|m| m.starts_with("1 unregulated (type 0) bus(es)")),
            "{retained:?}"
        );
    }

    #[test]
    fn typed_transformers_carry_taps_and_control_blocks() {
        let bus_3 = BUS_9.replace("   9 Bus 9     LV", "   3 Bus 3     HV");
        let bus_2 = BUS_9.replace("   9 Bus 9     LV", "   2 Bus 2     HV");
        let fixed = TRANSFORMER_4_7
            .replace("   4    7", "   1    2")
            .replace("1 0  0.0       0.20912", "1 1  0.0       0.20912");
        let voltage = LTC_9001_9006
            .replace("9001 9006", "   1    3")
            .replace(" 9006 0 ", "    3 0 ")
            .replace("  1  9 1 2", "  1  9 2 2");
        let shifter = TRANSFORMER_5_6
            .replace("   5    6", "   2    3")
            .replace("1 0  0.0       0.25202", "1 4  0.0       0.25202")
            .replace("    0 0  0.932     0.0 0.0", "    0 2  1.0      -3.5 0.0");
        let reactive = TRANSFORMER_5_6
            .replace("   5    6", "   2    3")
            .replace("1 0  0.0       0.25202", "1 3  0.0       0.25202")
            .replace("    0 0  0.932", "    0 1  0.0  ");
        let case = format!(
            "{TITLE}\nBUS DATA FOLLOWS 3 ITEMS\n{BUS_1}\n{bus_2}\n{bus_3}\n-999\n\
             BRANCH DATA FOLLOWS 4 ITEMS\n{fixed}\n{voltage}\n{shifter}\n{reactive}\n-999\n\
             END OF DATA\n"
        );
        let parsed = parse_str(&case, "ieee-cdf").unwrap();
        let branches = parsed.network.branches();
        assert_eq!(branches.len(), 4);

        let fixed = &branches[0];
        assert_eq!(fixed.tap, 0.978);
        assert!(fixed.is_transformer());
        assert!(fixed.control.is_none());
        assert!(fixed.extras.is_empty());

        let voltage = &branches[1];
        assert_eq!((voltage.r, voltage.x), (0.02439, 0.43682));
        assert_eq!(voltage.tap, 0.9668);
        assert_eq!(voltage.extras["ieee_cdf_circuit"], Value::from(2));
        let control = voltage.control.as_ref().unwrap();
        assert_eq!(control.mode, TransformerControlMode::Voltage);
        assert!(control.enabled);
        assert_eq!(control.controlled_bus, Some(BusId(3)));
        assert_eq!((control.tap_min, control.tap_max), (0.9391, 1.1478));
        assert_eq!((control.band_min, control.band_max), (0.99, 1.01));
        assert_eq!(control.ntp, 51);
        assert_eq!(control.mva_base, 100.0);

        let shifter = &branches[2];
        assert_eq!((shifter.tap, shifter.shift), (1.0, -3.5));
        let control = shifter.control.as_ref().unwrap();
        assert_eq!(control.mode, TransformerControlMode::ActiveFlow);
        assert_eq!(control.controlled_bus, Some(BusId(3)));
        assert_eq!((control.tap_min, control.tap_max), (-180.0, 180.0));
        assert_eq!((control.band_min, control.band_max), (0.9, 1.1));
        assert_eq!(control.ntp, 33);

        let reactive = &branches[3];
        assert_eq!(reactive.tap, 1.0);
        let control = reactive.control.as_ref().unwrap();
        assert_eq!(control.mode, TransformerControlMode::ReactiveFlow);
        assert_eq!(control.controlled_bus, Some(BusId(2)));

        let defaulted = messages(&parsed, "READ.IEEE_CDF.VALUE_DEFAULTED");
        assert!(
            defaulted
                .iter()
                .any(|m| m.starts_with("1 transformer(s) of type 1 through 4 state no turns ratio")),
            "{defaulted:?}"
        );
        assert!(
            defaulted
                .iter()
                .any(|m| m.starts_with("2 regulating transformer(s) state no tap limits")),
            "{defaulted:?}"
        );
    }

    #[test]
    fn the_narrow_limit_layout_reads_like_the_wide_one() {
        // The archive files end a branch record with the two limits one
        // column to the left of the documented layout.
        let narrow = LINE_1_2
            .replace("1 0  0.01938", "1 2  0.01938")
            .replace("    0.0   0.0", "    0.9   1.1");
        let wide = format!("{}{}", &narrow[..112], "    0.9    1.1");
        for (label, record) in [("narrow", narrow), ("wide", wide)] {
            let case = two_bus().replace(LINE_1_2, &record);
            let parsed = parse_str(&case, "ieee-cdf").unwrap();
            let control = parsed.network.branches()[0].control.as_ref().unwrap();
            assert_eq!((control.band_min, control.band_max), (0.9, 1.1), "{label}");
        }
    }

    #[test]
    fn a_misplaced_record_and_a_wrong_terminator_are_reported_and_skipped() {
        let quirk = two_bus().replace(
            &format!("{INTERCHANGE}\n-9\n"),
            &format!("-9\n{INTERCHANGE}\n"),
        );
        let parsed = parse_str(&quirk, "ieee-cdf").unwrap();
        assert!(parsed.network.areas().is_empty());
        let malformed = messages(&parsed, "READ.IEEE_CDF.SOURCE_MALFORMED");
        assert_eq!(malformed.len(), 2, "{malformed:?}");
        assert!(
            malformed[0].contains("interchange data header declares 1 item(s); 0 record(s)"),
            "{malformed:?}"
        );
        assert!(
            malformed[1].contains("record outside a section"),
            "{malformed:?}"
        );

        let wrong = two_bus().replacen("-99\n", "-999\n", 1);
        let parsed = parse_str(&wrong, "ieee-cdf").unwrap();
        let malformed = messages(&parsed, "READ.IEEE_CDF.SOURCE_MALFORMED");
        assert!(
            malformed
                .iter()
                .any(|m| m.contains("loss zone section ends with `-999` rather than `-99`")),
            "{malformed:?}"
        );
    }

    #[test]
    fn an_unterminated_section_and_trailing_text_are_reported() {
        let case = two_bus();
        let cut = case.split("-999\nLOSS ZONES").next().unwrap();
        let parsed = parse_str(cut, "ieee-cdf").unwrap();
        assert_eq!(parsed.network.branches().len(), 1);
        let malformed = messages(&parsed, "READ.IEEE_CDF.SOURCE_MALFORMED");
        assert!(
            malformed
                .iter()
                .any(|m| m.contains("branch data section is not closed by `-999`")),
            "{malformed:?}"
        );

        let trailing = format!("{case}stray text\n");
        let parsed = parse_str(&trailing, "ieee-cdf").unwrap();
        let malformed = messages(&parsed, "READ.IEEE_CDF.SOURCE_MALFORMED");
        assert!(
            malformed
                .iter()
                .any(|m| m.contains("text after END OF DATA")),
            "{malformed:?}"
        );
    }

    #[test]
    fn undeclared_references_are_dropped_with_a_finding() {
        let remote = two_bus()
            .replace(BUS_1, &format!("{}9", &BUS_1[..BUS_1.len() - 1]))
            .replace(
                INTERCHANGE,
                &INTERCHANGE.replace(" 1    2 Bus 2", " 1    7 Bus 2"),
            );
        let parsed = parse_str(&remote, "ieee-cdf").unwrap();
        assert_eq!(parsed.network.generators()[0].regulated_bus, None);
        assert_eq!(parsed.network.areas()[0].slack_bus, None);
        let malformed = messages(&parsed, "READ.IEEE_CDF.SOURCE_MALFORMED");
        assert!(
            malformed
                .iter()
                .any(|m| m.contains("remote controlled bus 9 is not declared")),
            "{malformed:?}"
        );
        assert!(
            malformed
                .iter()
                .any(|m| m.contains("interchange slack bus 7 of area 1 is not declared")),
            "{malformed:?}"
        );

        let declared = two_bus().replace(BUS_1, &format!("{}2", &BUS_1[..BUS_1.len() - 1]));
        let parsed = parse_str(&declared, "ieee-cdf").unwrap();
        assert_eq!(parsed.network.generators()[0].regulated_bus, Some(BusId(2)));
    }

    #[test]
    fn tie_lines_and_loss_zone_names_are_counted() {
        let tie = two_bus().replace(
            "TIE LINES FOLLOWS                     0 ITEMS\n",
            "TIE LINES FOLLOWS                     1 ITEMS\n   1  1     2  1   1\n",
        );
        let parsed = parse_str(&tie, "ieee-cdf").unwrap();
        let retained = messages(&parsed, "READ.IEEE_CDF.RETAINED_SOURCE_ONLY");
        assert!(
            retained
                .iter()
                .any(|m| m.starts_with("1 tie line record(s)")),
            "{retained:?}"
        );
        assert!(
            retained
                .iter()
                .any(|m| m.starts_with("1 loss zone name(s)")),
            "{retained:?}"
        );
        assert!(
            retained
                .iter()
                .any(|m| m.contains("title card originator `UW ARCHIVE`, year `1962`, season `W`")),
            "{retained:?}"
        );
    }

    #[test]
    fn a_zero_impedance_branch_is_kept_and_reported() {
        let zero = two_bus().replace("0.01938   0.05917", "0.0       0.0    ");
        let parsed = parse_str(&zero, "ieee-cdf").unwrap();
        assert_eq!(parsed.network.branches().len(), 1);
        let malformed = messages(&parsed, "READ.IEEE_CDF.SOURCE_MALFORMED");
        assert!(
            malformed
                .iter()
                .any(|m| m.contains("zero series impedance")),
            "{malformed:?}"
        );
    }

    #[test]
    fn codes_outside_the_documented_sets_are_substituted() {
        let odd = two_bus()
            .replace("  1  1  3 1.060", "  1  1  7 1.060")
            .replace("1 0  0.01938", "1 9  0.01938");
        let parsed = parse_str(&odd, "ieee-cdf").unwrap();
        assert_eq!(parsed.network.buses()[0].kind, BusType::Pq);
        assert!(!parsed.network.branches()[0].is_transformer());
        let substituted = messages(&parsed, "READ.IEEE_CDF.VALUE_SUBSTITUTED");
        assert_eq!(substituted.len(), 2, "{substituted:?}");
        assert!(substituted[0].contains("bus 1 type 7"), "{substituted:?}");
        assert!(
            substituted[1].contains("branch 1-2 type 9"),
            "{substituted:?}"
        );
    }

    #[test]
    fn dates_pivot_at_1970_and_blank_dates_are_none() {
        assert_eq!(iso_date("08/19/93").as_deref(), Some("1993-08-19"));
        assert_eq!(iso_date("04/26/09").as_deref(), Some("2009-04-26"));
        assert_eq!(iso_date("0 /0 /0 "), None);
        assert_eq!(iso_date("13/01/93"), None);
        let undated = two_bus().replacen("08/19/93", "0 /0 /0 ", 1);
        let parsed = parse_str(&undated, "ieee-cdf").unwrap();
        assert_eq!(parsed.network.case_metadata().case_date, None);
        assert!(messages(&parsed, "READ.IEEE_CDF.SOURCE_MALFORMED").is_empty());
    }

    #[test]
    fn card_helpers_read_fields_and_terminators() {
        assert!(is_terminator("-999"));
        assert!(is_terminator("-9"));
        assert!(!is_terminator("-"));
        assert!(!is_terminator("1"));
        assert_eq!(declared_items("BUS DATA FOLLOWS   14 ITEMS"), Some(14));
        assert_eq!(declared_items("BUS DATA FOLLOWS"), None);
        assert_eq!(parse_int("1."), Some(1));
        assert_eq!(parse_int("+3"), Some(3));
        assert_eq!(parse_int("1.5"), None);
        assert_eq!(parse_float(" .004"), None);
        assert_eq!(parse_float(".004"), Some(0.004));
        assert_eq!(field("ab", (3, 4)), None);
        assert_eq!(field("abcd", (2, 3)), Some("bc"));
        assert_eq!(field("a   ", (2, 4)), None);
    }
}
