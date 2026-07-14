//! The standalone geographic document.
//!
//! Coordinates arrive and leave as files of their own: a `Buscoords` CSV next
//! to a DSS master, a GeoJSON export from a GIS tool, a layout computed by a
//! renderer. [`GeoLayer`] is the container. Reading is tolerant (headerless
//! buscoords CSV, aliased CSV/JSON records, GeoJSON Point/LineString); writing
//! is canonical (a GeoJSON FeatureCollection with the `powerio_geo` foreign
//! member). The reader takes bytes plus a name hint and touches no filesystem,
//! so wasm consumers parse untrusted browser input through it directly.

use std::collections::HashMap;

use serde_json::{Map, Value, json};

use super::{Canvas, CoordinateSpace, CoordsKind, GeoMeta, Location};
use crate::network::{BusId, Network};
use crate::{Error, Result};

/// Version of the `powerio_geo` foreign member this crate writes.
pub const GEO_LAYER_VERSION: &str = "0.1.0";

/// Suggested extension for the canonical document.
pub const GEO_LAYER_EXTENSION: &str = "geo.json";

const FMT: &str = "geo layer";

/// A standalone geographic document: element points and routes in one
/// coordinate space, keyed by element identity rather than embedded in a case.
#[derive(Debug, Clone, PartialEq)]
pub struct GeoLayer {
    /// Coordinate space of every feature.
    pub space: CoordinateSpace,
    /// Default provenance, stamped into the `powerio_geo` member on write.
    pub kind: Option<CoordsKind>,
    pub features: Vec<GeoFeature>,
}

/// One point or route in a [`GeoLayer`].
#[derive(Debug, Clone, PartialEq)]
pub struct GeoFeature {
    pub target: GeoTarget,
    pub key: ElementKey,
    pub geometry: GeoGeometry,
    /// Endpoint bus references for a branch, the unordered fallback identity.
    pub from: Option<String>,
    pub to: Option<String>,
    /// Per feature provenance when it differs from the layer default.
    pub kind: Option<CoordsKind>,
}

/// The element family a feature places.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum GeoTarget {
    Bus,
    Branch,
    /// PowerWorld substations, joined onto buses through the `SubNum` extras
    /// key by [`super::apply_substation_points`].
    Substation,
}

impl GeoTarget {
    fn token(self) -> &'static str {
        match self {
            GeoTarget::Bus => "bus",
            GeoTarget::Branch => "branch",
            GeoTarget::Substation => "substation",
        }
    }
}

/// Feature geometry: a point or a polyline route.
#[derive(Debug, Clone, PartialEq)]
pub enum GeoGeometry {
    Point([f64; 2]),
    LineString(Vec<[f64; 2]>),
}

/// Element identity for one feature. Matching tries `uid`, then `id`, then
/// case insensitive `name`; branches additionally fall back to the unordered
/// `(from, to)` bus pair. `index` is a positional row alias (1-based, the
/// MATPOWER row convention) accepted on read and never written; the durable
/// identity is the payload `uid` (`buses:3`, `branches:7`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ElementKey {
    pub uid: Option<String>,
    pub id: Option<String>,
    pub name: Option<String>,
    pub index: Option<usize>,
}

/// Output of a tolerant geo read: the layer plus the reader's notes on
/// records it could not use.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct GeoParsed {
    pub layer: GeoLayer,
    pub warnings: Vec<String>,
}

/// Result of applying a [`GeoLayer`] to a network.
#[derive(Debug, Clone, Default, PartialEq)]
#[non_exhaustive]
pub struct GeoApplyReport {
    pub matched_buses: usize,
    pub matched_branches: usize,
    pub unmatched_features: usize,
    pub notes: Vec<String>,
}

impl GeoLayer {
    /// Tolerant read of a geographic sidecar from bytes. `name_hint` (a file
    /// name) picks CSV against JSON when present; otherwise the content is
    /// sniffed. Accepts headerless buscoords CSV (`bus,x,y`), CSV and JSON
    /// records with aliased field names, and GeoJSON Point/LineString
    /// features. Rejects input carrying no usable coordinates.
    pub fn parse_bytes(bytes: &[u8], name_hint: Option<&str>) -> Result<GeoParsed> {
        // Windows exports lead with a UTF-8 BOM; serde_json rejects it.
        let bytes = bytes
            .strip_prefix(b"\xef\xbb\xbf".as_slice())
            .unwrap_or(bytes);
        let mut parsed = GeoParsed {
            layer: GeoLayer {
                space: CoordinateSpace::Unknown,
                kind: None,
                features: Vec::new(),
            },
            warnings: Vec::new(),
        };
        let mut declared_space = false;
        let hint_ext = name_hint
            .and_then(|name| name.rsplit('.').next())
            .map(str::to_ascii_lowercase);
        let looks_json = match hint_ext.as_deref() {
            Some("csv") => false,
            Some("json" | "geojson") => true,
            _ => sniff_json(bytes),
        };
        if looks_json {
            let value: Value = serde_json::from_slice(bytes)
                .map_err(|error| bad(format!("invalid JSON: {error}")))?;
            if let Some(features) = feature_collection(&value) {
                declared_space = read_powerio_geo_member(&value, &mut parsed.layer);
                for feature in features {
                    read_geojson_feature(feature, &mut parsed);
                }
            } else {
                let mut records = Vec::new();
                collect_records(&value, &mut records);
                for record in records {
                    read_record(&record, &mut parsed);
                }
            }
        } else {
            let text = String::from_utf8_lossy(bytes);
            read_csv(&text, &mut parsed);
        }
        if parsed.layer.features.is_empty() {
            return Err(bad("no bus coordinates or branch routes found"));
        }
        if !declared_space {
            parsed.layer.space = inferred_space(&parsed.layer);
        }
        Ok(parsed)
    }

    /// Serialize the canonical wire form: a GeoJSON FeatureCollection with the
    /// `powerio_geo` foreign member. Valid RFC 7946 GeoJSON when the space is
    /// geographic, so GIS tools open it directly.
    #[must_use]
    pub fn to_geojson(&self) -> String {
        let mut member = Map::new();
        member.insert("version".to_owned(), json!(GEO_LAYER_VERSION));
        let detail = match &self.space {
            CoordinateSpace::Geographic { crs } | CoordinateSpace::Projected { crs } => {
                crs.as_ref().map(crs_entry)
            }
            CoordinateSpace::Diagram { canvas } => canvas.as_ref().map(canvas_entry),
            _ => None,
        };
        member.insert("space".to_owned(), json!(self.space.token()));
        if let Some((key, value)) = detail {
            member.insert(key.to_owned(), value);
        }
        if let Some(kind) = self.kind {
            member.insert("kind".to_owned(), kind_value(kind));
        }
        let features: Vec<Value> = self.features.iter().map(feature_value).collect();
        let document = json!({
            "type": "FeatureCollection",
            "powerio_geo": Value::Object(member),
            "features": features,
        });
        // Serializing an in-memory `Value` does not fail; `Display` is the
        // infallible (compact) fallback.
        serde_json::to_string_pretty(&document).unwrap_or_else(|_| document.to_string())
    }
}

fn crs_entry(crs: &String) -> (&'static str, Value) {
    ("crs", json!(crs))
}

fn canvas_entry(canvas: &Canvas) -> (&'static str, Value) {
    (
        "canvas",
        serde_json::to_value(canvas).unwrap_or(Value::Null),
    )
}

fn kind_value(kind: CoordsKind) -> Value {
    serde_json::to_value(kind).unwrap_or(Value::Null)
}

fn feature_value(feature: &GeoFeature) -> Value {
    let mut properties = Map::new();
    properties.insert("target".to_owned(), json!(feature.target.token()));
    if let Some(uid) = &feature.key.uid {
        properties.insert("uid".to_owned(), json!(uid));
    }
    if let Some(id) = &feature.key.id {
        properties.insert("id".to_owned(), json!(id));
    }
    if let Some(name) = &feature.key.name {
        properties.insert("name".to_owned(), json!(name));
    }
    if let Some(from) = &feature.from {
        properties.insert("from".to_owned(), json!(from));
    }
    if let Some(to) = &feature.to {
        properties.insert("to".to_owned(), json!(to));
    }
    if let Some(kind) = feature.kind {
        properties.insert("kind".to_owned(), kind_value(kind));
    }
    let geometry = match &feature.geometry {
        GeoGeometry::Point(point) => json!({"type": "Point", "coordinates": point}),
        GeoGeometry::LineString(path) => json!({"type": "LineString", "coordinates": path}),
    };
    json!({"type": "Feature", "geometry": geometry, "properties": Value::Object(properties)})
}

// ---------------------------------------------------------------------------
// Tolerant reading
// ---------------------------------------------------------------------------

/// Alias tables, matched on keys normalized to lowercase alphanumeric. These
/// port the sidecar vocabulary tellegen's renderer accepted, so a file that
/// loaded there loads here.
const BUS_ID_ALIASES: &[&str] = &["busi", "bus", "busid", "busnumber", "number", "id"];
const LAT_ALIASES: &[&str] = &["lat", "latitude", "y"];
const LON_ALIASES: &[&str] = &["lon", "lng", "longitude", "x"];
const FROM_ALIASES: &[&str] = &["fbus", "from", "frombus"];
const TO_ALIASES: &[&str] = &["tbus", "to", "tobus"];
const BRANCH_ID_ALIASES: &[&str] = &["branch", "branchid", "branchnumber", "catsid", "id"];
const PATH_ALIASES: &[&str] = &["path", "geometry", "coordinates"];
const FROM_LAT_ALIASES: &[&str] = &["lat1", "fromlat"];
const FROM_LON_ALIASES: &[&str] = &["lon1", "lng1", "fromlon", "fromlng"];
const TO_LAT_ALIASES: &[&str] = &["lat2", "tolat"];
const TO_LON_ALIASES: &[&str] = &["lon2", "lng2", "tolon", "tolng"];
const NAME_ALIASES: &[&str] = &["name", "busname"];

fn bad(message: impl Into<String>) -> Error {
    Error::FormatRead {
        format: FMT,
        message: message.into(),
    }
}

fn sniff_json(bytes: &[u8]) -> bool {
    bytes
        .iter()
        .copied()
        .find(|byte| !byte.is_ascii_whitespace() && *byte != 0xEF && *byte != 0xBB && *byte != 0xBF)
        .is_some_and(|byte| byte == b'{' || byte == b'[')
}

fn normalize_key(key: &str) -> String {
    key.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// A record read from CSV or JSON, with normalized keys.
struct Record {
    fields: HashMap<String, Value>,
}

impl Record {
    fn value(&self, aliases: &[&str]) -> Option<&Value> {
        aliases.iter().find_map(|alias| self.fields.get(*alias))
    }

    fn number(&self, aliases: &[&str]) -> Option<f64> {
        value_number(self.value(aliases)?)
    }

    fn string(&self, aliases: &[&str]) -> Option<String> {
        match self.value(aliases)? {
            Value::String(text) => {
                let trimmed = text.trim();
                (!trimmed.is_empty()).then(|| trimmed.to_owned())
            }
            Value::Number(number) => Some(number.to_string()),
            _ => None,
        }
    }
}

fn value_number(value: &Value) -> Option<f64> {
    match value {
        Value::Number(number) => number.as_f64().filter(|v| v.is_finite()),
        Value::String(text) => {
            let trimmed = text.trim().trim_matches(|c| c == '\'' || c == '"');
            trimmed.parse::<f64>().ok().filter(|v| v.is_finite())
        }
        _ => None,
    }
}

fn feature_collection(value: &Value) -> Option<&Vec<Value>> {
    value.get("features")?.as_array()
}

/// Read the `powerio_geo` foreign member into the layer; `true` when a space
/// was declared.
fn read_powerio_geo_member(value: &Value, layer: &mut GeoLayer) -> bool {
    let Some(member) = value.get("powerio_geo").and_then(Value::as_object) else {
        return false;
    };
    layer.kind = member.get("kind").and_then(read_kind);
    let crs = member.get("crs").and_then(Value::as_str).map(str::to_owned);
    let canvas = member
        .get("canvas")
        .and_then(|canvas| serde_json::from_value(canvas.clone()).ok());
    match member.get("space").and_then(Value::as_str) {
        Some("geographic") => layer.space = CoordinateSpace::Geographic { crs },
        Some("projected") => layer.space = CoordinateSpace::Projected { crs },
        Some("diagram") => layer.space = CoordinateSpace::Diagram { canvas },
        Some(_) => layer.space = CoordinateSpace::Unknown,
        None => return false,
    }
    true
}

fn read_kind(value: &Value) -> Option<CoordsKind> {
    serde_json::from_value(value.clone()).ok()
}

fn read_geojson_feature(feature: &Value, parsed: &mut GeoParsed) {
    let Some(geometry) = feature.get("geometry").and_then(Value::as_object) else {
        return;
    };
    let properties = feature
        .get("properties")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let record = Record {
        fields: properties
            .into_iter()
            .map(|(key, value)| (normalize_key(&key), value))
            .collect(),
    };
    let target = record.string(&["target"]);
    let kind = record.value(&["kind"]).and_then(read_kind);
    match geometry.get("type").and_then(Value::as_str) {
        Some("Point") => {
            let Some(point) = geometry.get("coordinates").and_then(coordinate) else {
                parsed
                    .warnings
                    .push("skipped a Point feature with unusable coordinates".to_owned());
                return;
            };
            let target = match target.as_deref() {
                Some("substation") => GeoTarget::Substation,
                _ => GeoTarget::Bus,
            };
            parsed.layer.features.push(GeoFeature {
                target,
                key: point_key(&record),
                geometry: GeoGeometry::Point(point),
                from: None,
                to: None,
                kind,
            });
        }
        Some("LineString") => {
            let path = geometry
                .get("coordinates")
                .and_then(Value::as_array)
                .map(|raw| coordinate_path(raw))
                .unwrap_or_default();
            if path.len() < 2 {
                parsed
                    .warnings
                    .push("skipped a LineString feature with fewer than 2 points".to_owned());
                return;
            }
            push_branch_feature(&record, path, parsed);
        }
        Some(other) => {
            // Truncate the echoed type name: it is attacker controlled, and
            // unbounded distinct warnings would defeat the dedup below.
            let shown: String = other.chars().take(32).collect();
            push_once(
                &mut parsed.warnings,
                format!("skipped unsupported GeoJSON geometry `{shown}`"),
            );
        }
        None => {}
    }
}

/// Key for a point record: the payload `uid`, an aliased id, and a name.
fn point_key(record: &Record) -> ElementKey {
    ElementKey {
        uid: record.string(&["uid"]),
        id: record.string(BUS_ID_ALIASES),
        name: record.string(NAME_ALIASES),
        index: None,
    }
}

/// Key for a branch record. A bare unsigned integer id is a positional row
/// alias (read only); everything else matches by string id or name.
fn branch_key(record: &Record) -> ElementKey {
    let id = record.string(BRANCH_ID_ALIASES);
    let index = id
        .as_deref()
        .and_then(|raw| raw.parse::<usize>().ok())
        .filter(|_| record.string(&["uid"]).is_none());
    ElementKey {
        uid: record.string(&["uid"]),
        id,
        name: record.string(NAME_ALIASES),
        index,
    }
}

fn push_branch_feature(record: &Record, path: Vec<[f64; 2]>, parsed: &mut GeoParsed) {
    let from = record.string(FROM_ALIASES);
    let to = record.string(TO_ALIASES);
    let key = branch_key(record);
    if key.uid.is_none()
        && key.id.is_none()
        && key.name.is_none()
        && (from.is_none() || to.is_none())
    {
        push_once(
            &mut parsed.warnings,
            "skipped a branch route with no id, uid, name, or endpoint pair".to_owned(),
        );
        return;
    }
    parsed.layer.features.push(GeoFeature {
        target: GeoTarget::Branch,
        key,
        geometry: GeoGeometry::LineString(path),
        from,
        to,
        kind: record.value(&["kind"]).and_then(read_kind),
    });
}

fn coordinate(raw: &Value) -> Option<[f64; 2]> {
    let items = raw.as_array()?;
    let x = value_number(items.first()?)?;
    let y = value_number(items.get(1)?)?;
    Some([x, y])
}

fn coordinate_path(raw: &[Value]) -> Vec<[f64; 2]> {
    raw.iter().filter_map(coordinate).collect()
}

/// Flatten arbitrary JSON into candidate records: arrays recurse, an object
/// whose values contain arrays of objects yields those, and a plain object is
/// itself one record. Depth is bounded by the parsed document.
fn collect_records(value: &Value, out: &mut Vec<Record>) {
    match value {
        Value::Array(items) => {
            for item in items {
                collect_records(item, out);
            }
        }
        Value::Object(object) => {
            let before = out.len();
            for nested in object.values() {
                if let Value::Array(items) = nested {
                    for item in items {
                        if item.is_object() {
                            collect_records(item, out);
                        }
                    }
                }
            }
            if out.len() == before {
                out.push(Record {
                    fields: object
                        .iter()
                        .map(|(key, value)| (normalize_key(key), value.clone()))
                        .collect(),
                });
            }
        }
        _ => {}
    }
}

/// One aliased record can carry a bus point, a branch route, or both.
fn read_record(record: &Record, parsed: &mut GeoParsed) {
    read_point_record(record, parsed);
    read_branch_record(record, parsed);
}

fn read_point_record(record: &Record, parsed: &mut GeoParsed) {
    let key = point_key(record);
    if key.uid.is_none() && key.id.is_none() && key.name.is_none() {
        return;
    }
    let (Some(lon), Some(lat)) = (record.number(LON_ALIASES), record.number(LAT_ALIASES)) else {
        return;
    };
    parsed.layer.features.push(GeoFeature {
        target: GeoTarget::Bus,
        key,
        geometry: GeoGeometry::Point([lon, lat]),
        from: None,
        to: None,
        kind: None,
    });
}

fn read_branch_record(record: &Record, parsed: &mut GeoParsed) {
    let path = record_path(record);
    if path.len() < 2 {
        return;
    }
    push_branch_feature(record, path, parsed);
}

fn record_path(record: &Record) -> Vec<[f64; 2]> {
    if let Some(Value::Array(raw)) = record.value(PATH_ALIASES) {
        return coordinate_path(raw);
    }
    let endpoints = (
        record.number(FROM_LON_ALIASES),
        record.number(FROM_LAT_ALIASES),
        record.number(TO_LON_ALIASES),
        record.number(TO_LAT_ALIASES),
    );
    if let (Some(lon1), Some(lat1), Some(lon2), Some(lat2)) = endpoints {
        return vec![[lon1, lat1], [lon2, lat2]];
    }
    Vec::new()
}

// ---------------------------------------------------------------------------
// CSV
// ---------------------------------------------------------------------------

fn read_csv(text: &str, parsed: &mut GeoParsed) {
    let rows = csv_rows(text);
    let Some(first) = rows.first() else { return };
    let has_header = first
        .iter()
        .any(|cell| is_known_alias(&normalize_key(cell)));
    if has_header {
        let headers: Vec<String> = first.iter().map(|cell| normalize_key(cell)).collect();
        for cells in &rows[1..] {
            let record = Record {
                fields: headers
                    .iter()
                    .zip(cells)
                    .map(|(header, cell)| (header.clone(), Value::String(cell.clone())))
                    .collect(),
            };
            read_record(&record, parsed);
        }
    } else {
        // Headerless buscoords: `bus, x, y` (the OpenDSS sidecar layout).
        for cells in &rows {
            read_buscoords_row(cells, parsed);
        }
    }
}

fn is_known_alias(normalized: &str) -> bool {
    [
        BUS_ID_ALIASES,
        LAT_ALIASES,
        LON_ALIASES,
        FROM_ALIASES,
        TO_ALIASES,
        BRANCH_ID_ALIASES,
        PATH_ALIASES,
        NAME_ALIASES,
        FROM_LAT_ALIASES,
        FROM_LON_ALIASES,
        TO_LAT_ALIASES,
        TO_LON_ALIASES,
        &["uid", "target", "kind"],
    ]
    .iter()
    .any(|aliases| aliases.contains(&normalized))
}

fn read_buscoords_row(cells: &[String], parsed: &mut GeoParsed) {
    // Buscoords in the wild are comma or whitespace separated; a row that
    // arrived as one comma-free cell splits on whitespace.
    let split: Vec<String>;
    let cells = if cells.len() == 1 && cells[0].split_whitespace().count() >= 3 {
        split = cells[0].split_whitespace().map(str::to_owned).collect();
        &split
    } else {
        cells
    };
    if cells.len() < 3 {
        push_once(
            &mut parsed.warnings,
            "skipped a buscoords row with fewer than 3 columns".to_owned(),
        );
        return;
    }
    let bus = cells[0].trim();
    let x = cells[1]
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite());
    let y = cells[2]
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite());
    let (Some(x), Some(y)) = (x, y) else {
        push_once(
            &mut parsed.warnings,
            "skipped a buscoords row with unparseable coordinates".to_owned(),
        );
        return;
    };
    if bus.is_empty() {
        return;
    }
    parsed.layer.features.push(GeoFeature {
        target: GeoTarget::Bus,
        key: ElementKey {
            uid: None,
            id: Some(bus.to_owned()),
            name: Some(bus.to_owned()),
            index: None,
        },
        geometry: GeoGeometry::Point([x, y]),
        from: None,
        to: None,
        kind: None,
    });
}

/// RFC-style quoted CSV split into trimmed cells; blank rows dropped.
/// Deliberately separate from the strict case-file CSV reader in
/// `format::pypsa`: this one parses untrusted sidecars, so malformed quoting
/// degrades instead of erroring.
fn csv_rows(text: &str) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut cell = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            if c == '"' && chars.peek() == Some(&'"') {
                cell.push('"');
                chars.next();
            } else if c == '"' {
                quoted = false;
            } else {
                cell.push(c);
            }
            continue;
        }
        match c {
            '"' => quoted = true,
            ',' => {
                row.push(std::mem::take(&mut cell));
                cell.clear();
            }
            '\n' => {
                row.push(std::mem::take(&mut cell));
                rows.push(std::mem::take(&mut row));
            }
            '\r' => {}
            _ => cell.push(c),
        }
    }
    if !cell.is_empty() || !row.is_empty() {
        row.push(cell);
        rows.push(row);
    }
    rows.retain(|row| row.iter().any(|cell| !cell.trim().is_empty()));
    for row in &mut rows {
        for cell in row.iter_mut() {
            // Reallocate only when there is whitespace to strip.
            let trimmed = cell.trim();
            if trimmed.len() != cell.len() {
                *cell = trimmed.to_owned();
            }
        }
    }
    rows
}

/// Without a declared space, coordinates that all fit longitude and latitude
/// bounds read as geographic; anything else stays unknown.
fn inferred_space(layer: &GeoLayer) -> CoordinateSpace {
    let mut points = layer.features.iter().flat_map(|feature| {
        let slice: &[[f64; 2]] = match &feature.geometry {
            GeoGeometry::Point(point) => std::slice::from_ref(point),
            GeoGeometry::LineString(path) => path,
        };
        slice.iter()
    });
    if points.all(|[x, y]| x.abs() <= 180.0 && y.abs() <= 90.0) {
        CoordinateSpace::Geographic { crs: None }
    } else {
        CoordinateSpace::Unknown
    }
}

/// Reader notes are bounded: the dedup scan is linear, so an unbounded
/// number of distinct notes from adversarial input would go quadratic.
const MAX_READER_NOTES: usize = 16;

fn push_once(warnings: &mut Vec<String>, warning: String) {
    if warnings.len() >= MAX_READER_NOTES {
        return;
    }
    if !warnings.contains(&warning) {
        warnings.push(warning);
        if warnings.len() == MAX_READER_NOTES {
            warnings.push("further reader notes suppressed".to_owned());
        }
    }
}

// ---------------------------------------------------------------------------
// Extract and apply on the balanced network
// ---------------------------------------------------------------------------

impl Network {
    /// Extract this network's coordinates as a standalone [`GeoLayer`]:
    /// one point per located bus, one route per routed branch. The layer
    /// carries the network's coordinate space and default provenance.
    #[must_use]
    pub fn geo_layer(&self) -> GeoLayer {
        let mut features = Vec::new();
        for (row, bus) in self.buses.iter().enumerate() {
            let Some(location) = bus.location else {
                continue;
            };
            features.push(GeoFeature {
                target: GeoTarget::Bus,
                key: ElementKey {
                    uid: Some(payload_uid("buses", row, bus.uid.as_deref())),
                    id: Some(bus.id.to_string()),
                    name: bus.name.clone(),
                    index: None,
                },
                geometry: GeoGeometry::Point([location.x, location.y]),
                from: None,
                to: None,
                kind: location.kind,
            });
        }
        for (row, branch) in self.branches.iter().enumerate() {
            let Some(route) = &branch.route else {
                continue;
            };
            features.push(GeoFeature {
                target: GeoTarget::Branch,
                key: ElementKey {
                    uid: Some(payload_uid("branches", row, branch.uid.as_deref())),
                    id: None,
                    name: None,
                    index: None,
                },
                geometry: GeoGeometry::LineString(
                    route.iter().map(|point| [point.x, point.y]).collect(),
                ),
                from: Some(branch.from.to_string()),
                to: Some(branch.to.to_string()),
                kind: None,
            });
        }
        GeoLayer {
            space: self
                .geo
                .as_ref()
                .map_or(CoordinateSpace::Unknown, |geo| geo.space.clone()),
            kind: self.geo.as_ref().and_then(|geo| geo.kind),
            features,
        }
    }

    /// Apply a [`GeoLayer`] onto this network: matched bus points land in
    /// `Bus.location`, matched branch routes in `Branch.route`, and the
    /// layer's space becomes the network's [`GeoMeta`] when anything matched.
    /// Matching follows [`ElementKey`]. Substation features are not applied
    /// here; join them through [`super::apply_substation_points`].
    pub fn apply_geo_layer(&mut self, layer: &GeoLayer) -> GeoApplyReport {
        let mut target = BalancedApply {
            buses: BalancedBusIndex::new(self),
            branches: BalancedBranchIndex::new(self),
            net: self,
        };
        let mut report = apply_geo_features(layer, &mut target);
        if report.matched_buses > 0 || report.matched_branches > 0 {
            note_space_change(&mut report, self.geo.as_ref(), &layer.space);
            self.geo = Some(GeoMeta {
                space: layer.space.clone(),
                kind: layer.kind,
            });
        }
        report
    }
}

/// Note when an apply moves the network to a different coordinate space, so
/// replacing (say) geographic locations with diagram points is never silent.
pub(super) fn note_space_change(
    report: &mut GeoApplyReport,
    previous: Option<&GeoMeta>,
    space: &CoordinateSpace,
) {
    if let Some(previous) = previous {
        if previous.space != *space {
            report.notes.push(format!(
                "the network's coordinate space changed from {} to {}",
                previous.space.token(),
                space.token()
            ));
        }
    }
}

/// The model half of one [`apply_geo_features`] pass: how a feature key
/// resolves to a row, and how a matched point or route lands on the model.
pub trait GeoApplyTarget {
    fn bus_row(&self, key: &ElementKey) -> Option<usize>;
    fn branch_row(&self, feature: &GeoFeature) -> Option<usize>;
    fn place_bus(&mut self, row: usize, point: [f64; 2], kind: Option<CoordsKind>);
    fn place_branch(&mut self, row: usize, path: &[[f64; 2]], kind: Option<CoordsKind>);
    /// Report note for substation features this target cannot place.
    fn substation_note(&self, count: usize) -> String;
}

/// One apply pass over a layer's features. The model-specific lookups and
/// placements come from the [`GeoApplyTarget`]; the feature dispatch, match
/// counting, and substation bookkeeping live here, so the balanced network
/// and the multiconductor glue in `powerio-pkg` report identically.
pub fn apply_geo_features(layer: &GeoLayer, target: &mut impl GeoApplyTarget) -> GeoApplyReport {
    let mut report = GeoApplyReport::default();
    let mut substations = 0usize;
    for feature in &layer.features {
        match (&feature.target, &feature.geometry) {
            (GeoTarget::Bus, GeoGeometry::Point(point)) => {
                if let Some(row) = target.bus_row(&feature.key) {
                    target.place_bus(row, *point, feature.kind);
                    report.matched_buses += 1;
                } else {
                    report.unmatched_features += 1;
                }
            }
            (GeoTarget::Branch, GeoGeometry::LineString(path)) => {
                if let Some(row) = target.branch_row(feature) {
                    target.place_branch(row, path, feature.kind);
                    report.matched_branches += 1;
                } else {
                    report.unmatched_features += 1;
                }
            }
            (GeoTarget::Substation, _) => substations += 1,
            _ => report.unmatched_features += 1,
        }
    }
    if substations > 0 {
        report.unmatched_features += substations;
        report.notes.push(target.substation_note(substations));
    }
    report
}

/// The balanced network as an apply target.
struct BalancedApply<'a> {
    net: &'a mut Network,
    buses: BalancedBusIndex,
    branches: BalancedBranchIndex,
}

impl GeoApplyTarget for BalancedApply<'_> {
    fn bus_row(&self, key: &ElementKey) -> Option<usize> {
        self.buses.row_for(key)
    }

    fn branch_row(&self, feature: &GeoFeature) -> Option<usize> {
        self.branches.row_for(feature, self.net.branches.len())
    }

    fn place_bus(&mut self, row: usize, point: [f64; 2], kind: Option<CoordsKind>) {
        self.net.buses[row].location = Some(Location {
            x: point[0],
            y: point[1],
            kind,
        });
    }

    fn place_branch(&mut self, row: usize, path: &[[f64; 2]], kind: Option<CoordsKind>) {
        self.net.branches[row].route = Some(
            path.iter()
                .map(|[x, y]| Location { x: *x, y: *y, kind })
                .collect(),
        );
    }

    fn substation_note(&self, count: usize) -> String {
        format!("{count} substation feature(s) not applied; join them with apply_substation_points")
    }
}

/// Bus row lookups for one apply pass: by uid (element uid and payload row
/// uid), external id, and case insensitive name.
struct BalancedBusIndex {
    ids: HashMap<BusId, usize>,
    uids: HashMap<String, usize>,
    names: HashMap<String, usize>,
}

impl BalancedBusIndex {
    fn new(net: &Network) -> Self {
        let mut index = Self {
            ids: HashMap::new(),
            uids: HashMap::new(),
            names: HashMap::new(),
        };
        for (row, bus) in net.buses.iter().enumerate() {
            index.ids.insert(bus.id, row);
            index
                .uids
                .insert(payload_uid("buses", row, bus.uid.as_deref()), row);
            if let Some(uid) = &bus.uid {
                index.uids.insert(uid.clone(), row);
            }
            if let Some(name) = &bus.name {
                index.names.entry(name.to_ascii_lowercase()).or_insert(row);
            }
        }
        index
    }

    fn row_for(&self, key: &ElementKey) -> Option<usize> {
        key.uid
            .as_ref()
            .and_then(|uid| self.uids.get(uid))
            .or_else(|| {
                // A numeric id is the external BusId; a string id (one wire
                // form serves the string-keyed multiconductor model too)
                // matches the bus name.
                let id = key.id.as_ref()?;
                match id.parse::<usize>() {
                    Ok(id) => self.ids.get(&BusId(id)),
                    Err(_) => self.names.get(&id.to_ascii_lowercase()),
                }
            })
            .or_else(|| {
                key.name
                    .as_ref()
                    .and_then(|name| self.names.get(&name.to_ascii_lowercase()))
            })
            .copied()
    }
}

/// Branch row lookups for one apply pass: by uid, positional row alias, and
/// the unordered endpoint pair.
struct BalancedBranchIndex {
    uids: HashMap<String, usize>,
    pairs: HashMap<(BusId, BusId), usize>,
}

impl BalancedBranchIndex {
    fn new(net: &Network) -> Self {
        let mut index = Self {
            uids: HashMap::new(),
            pairs: HashMap::new(),
        };
        for (row, branch) in net.branches.iter().enumerate() {
            index
                .uids
                .insert(payload_uid("branches", row, branch.uid.as_deref()), row);
            if let Some(uid) = &branch.uid {
                index.uids.insert(uid.clone(), row);
            }
            index
                .pairs
                .entry(ordered_pair(branch.from, branch.to))
                .or_insert(row);
        }
        index
    }

    fn row_for(&self, feature: &GeoFeature, branches: usize) -> Option<usize> {
        feature
            .key
            .uid
            .as_ref()
            .and_then(|uid| self.uids.get(uid).copied())
            .or_else(|| {
                // Balanced branches have no external id or name of their own;
                // a foreign record's id/name still matches a source uid, the
                // documented uid -> id -> name order.
                feature
                    .key
                    .id
                    .as_ref()
                    .and_then(|id| self.uids.get(id))
                    .or_else(|| {
                        feature
                            .key
                            .name
                            .as_ref()
                            .and_then(|name| self.uids.get(name))
                    })
                    .copied()
            })
            .or_else(|| {
                // Positional row alias, 1-based (MATPOWER rows).
                feature
                    .key
                    .index
                    .and_then(|index| index.checked_sub(1))
                    .filter(|row| *row < branches)
            })
            .or_else(|| {
                let from = feature.from.as_ref()?.parse::<usize>().ok()?;
                let to = feature.to.as_ref()?.parse::<usize>().ok()?;
                self.pairs
                    .get(&ordered_pair(BusId(from), BusId(to)))
                    .copied()
            })
    }
}

/// The payload row uid (`buses:3`), preferring the element's own uid. The same
/// identity `powerio-pkg` stamps on payload rows, so a layer written from a
/// package round-trips.
fn payload_uid(table: &str, row: usize, uid: Option<&str>) -> String {
    uid.map_or_else(|| format!("{table}:{row}"), str::to_owned)
}

fn ordered_pair(a: BusId, b: BusId) -> (BusId, BusId) {
    if b.0 < a.0 { (b, a) } else { (a, b) }
}
