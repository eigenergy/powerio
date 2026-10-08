//! Optional native drawing geometry, independent of either electrical family.
//!
//! Node position selectors and CRS/datum metadata are not yet verified. Drawing
//! coordinates therefore have unknown space, never inferred geographic space.
use std::collections::{BTreeMap, BTreeSet};

use powerio_core::{Diagnostic, DiagnosticInfo};
use rusqlite::types::Value;
use serde::Serialize;
use serde_json::json;

use crate::{DatabaseSnapshot, Result, format_error};

/// Tables requested by the optional Access graphics acquisition profile.
pub const GRAPHICS_TABLES: &[&str] = &[
    "GraphicNode",
    "GraphicElement",
    "GraphicTerminal",
    "GraphicBucklePoint",
];

/// A native graphic node; extended busbars use a derived midpoint.
#[derive(Clone, Debug, PartialEq)]
pub struct DrawingPoint {
    pub start: [f64; 2],
    pub end: [f64; 2],
}
impl DrawingPoint {
    pub fn point(&self) -> [f64; 2] {
        if !self.derived() {
            return self.start;
        }
        [
            self.start[0] / 2.0 + self.end[0] / 2.0,
            self.start[1] / 2.0 + self.end[1] / 2.0,
        ]
    }
    #[allow(clippy::float_cmp)] // Native endpoint equality distinguishes a point from a busbar.
    pub fn derived(&self) -> bool {
        self.start != self.end
    }
    pub fn provenance(&self) -> serde_json::Value {
        json!({"profile":"native_drawing_unknown_crs", "start":self.start,"end":self.end,
            "representative":if self.derived(){"midpoint"}else{"source_point"}})
    }
}

#[derive(Default, Debug, Serialize)]
pub struct GeometryFinding {
    pub count: usize,
    pub sample_ids: Vec<i64>,
}

/// Variant-local coordinates keyed by native identity, not name or row order.
#[derive(Default, Debug)]
pub struct DrawingGeometry {
    pub points: BTreeMap<i64, DrawingPoint>,
    pub routes: BTreeMap<i64, Vec<[f64; 2]>>,
    pub findings: BTreeMap<String, GeometryFinding>,
    pub area: Option<i64>,
}

impl DrawingGeometry {
    fn note(&mut self, reason: &str, id: i64) {
        let finding = self.findings.entry(reason.to_owned()).or_default();
        finding.count += 1;
        if finding.sample_ids.len() < 8 {
            finding.sample_ids.push(id);
        }
    }

    /// One bounded structured report; no per-row diagnostic explosion.
    pub fn diagnostic(
        &self,
        code: &'static DiagnosticInfo,
    ) -> std::result::Result<Diagnostic, powerio_core::Error> {
        let mut diagnostic = Diagnostic::of(
            code,
            "Optional SINCAL drawing coordinates use unknown CRS; busbar midpoints are derived. Geographic node fields, styles and unsupported geometry remain in source.",
        );
        diagnostic.insert_detail("profile", json!("native_drawing_unknown_crs"))?;
        diagnostic.insert_detail("area", json!(self.area))?;
        diagnostic.insert_detail("native_points", json!(self.points.len()))?;
        diagnostic.insert_detail("native_line_routes", json!(self.routes.len()))?;
        diagnostic.insert_detail("findings", json!(self.findings))?;
        Ok(diagnostic)
    }

    /// Correct table-only inventory wording when selected geometry is promoted.
    pub fn annotate_retention(
        &self,
        diagnostics: &mut [Diagnostic],
        mapped_points: usize,
        mapped_routes: usize,
    ) -> std::result::Result<(), powerio_core::Error> {
        if mapped_points == 0 && mapped_routes == 0 {
            return Ok(());
        }
        for diagnostic in diagnostics {
            let Some(table) = diagnostic.details().get("table").and_then(|v| v.as_str()) else {
                continue;
            };
            if !GRAPHICS_TABLES.contains(&table)
                || (table == "GraphicNode" && mapped_points == 0)
                || (table != "GraphicNode" && mapped_routes == 0)
            {
                continue;
            }
            let Some(code) = diagnostic.registered_info() else {
                continue;
            };
            let mut details = diagnostic.details().clone();
            details.insert("whole_table".into(), json!(false));
            details.insert(
                "retention_scope".into(),
                json!("unmapped_geometry_and_untyped_graphic_fields"),
            );
            *diagnostic = Diagnostic::of(code, format!("{table}: selected drawing geometry is typed; unmapped geometry, styles and other graphic fields remain only in retained source"))
                .with_details(details)?;
        }
        Ok(())
    }
}

fn integer(value: &Value) -> Option<i64> {
    if let Value::Integer(v) = value {
        Some(*v)
    } else {
        None
    }
}
fn number(value: &Value) -> Option<f64> {
    let value = match value {
        Value::Real(v) => *v,
        Value::Integer(v) => *v as f64,
        _ => return None,
    };
    value.is_finite().then_some(value)
}
fn point(values: &[Value]) -> Option<[f64; 2]> {
    Some([number(&values[0])?, number(&values[1])?])
}

impl DatabaseSnapshot {
    /// Decode a conservative drawing profile. Invalid optional records are
    /// diagnosed and omitted; bounded-query/transport failures remain errors.
    pub fn drawing_geometry(&self) -> Result<DrawingGeometry> {
        let mut result = DrawingGeometry::default();
        let Some(nodes) = self.geometry_rows(
            "GraphicNode",
            &[
                "GraphicNode_ID",
                "Node_ID",
                "GraphicArea_ID",
                "NodeStartX",
                "NodeStartY",
                "NodeEndX",
                "NodeEndY",
            ],
            &mut result,
        )?
        else {
            return Ok(result);
        };
        let mut areas = BTreeSet::new();
        let mut conflicting = BTreeSet::new();
        let mut identities = BTreeMap::new();
        for row in nodes {
            let id = integer(&row[1]).unwrap_or(0);
            if self.nodes.contains(&id)
                && let Some(area) = integer(&row[2])
            {
                areas.insert(area);
            }
            let decoded = (|| {
                Some((
                    integer(&row[0])?,
                    integer(&row[1])?,
                    integer(&row[2])?,
                    DrawingPoint {
                        start: point(&row[3..5])?,
                        end: point(&row[5..7])?,
                    },
                ))
            })();
            let Some((graphic, node, area, location)) = decoded else {
                conflicting.insert(id);
                result.note("GraphicNode.invalid_fields", id);
                continue;
            };
            if !self.nodes.contains(&node) {
                result.note("GraphicNode.unknown_node", node);
                continue;
            }
            areas.insert(area);
            if let Some(previous) = identities.insert(graphic, node)
                && previous != node
            {
                conflicting.insert(previous);
                conflicting.insert(node);
                result.note("GraphicNode.duplicate_identity", graphic);
            }
            if let Some(previous) = result.points.insert(node, location.clone())
                && previous != location
            {
                conflicting.insert(node);
            }
        }
        if areas.len() > 1 {
            result.points.clear();
            result.note("GraphicNode.ambiguous_areas", 0);
            return Ok(result);
        }
        result.area = areas.first().copied();
        for id in conflicting {
            result.points.remove(&id);
            result.note("GraphicNode.conflicting_positions", id);
        }
        self.drawing_routes(&mut result)?;
        Ok(result)
    }

    fn geometry_rows(
        &self,
        table: &str,
        columns: &[&str],
        result: &mut DrawingGeometry,
    ) -> Result<Option<Vec<Vec<Value>>>> {
        let mut statement = self
            .connection
            .prepare("SELECT name FROM pragma_table_info(?1)")
            .map_err(format_error)?;
        let available: BTreeSet<String> = statement
            .query_map([table], |r| r.get(0))
            .map_err(format_error)?
            .collect::<rusqlite::Result<_>>()
            .map_err(format_error)?;
        if available.is_empty() {
            if self.excluded_tables.iter().any(|t| t == table) {
                result.note(&format!("{table}.not_acquired"), 0);
            }
            return Ok(None);
        }
        if columns
            .iter()
            .chain(["Variant_ID", "Flag_Variant"].iter())
            .any(|c| !available.contains(*c))
        {
            result.note(&format!("{table}.unsupported_columns"), 0);
            return Ok(None);
        }
        // Table and column identifiers are internal constants, never input SQL.
        let sql = format!(
            "SELECT {} FROM {table} WHERE Variant_ID=?1 AND Flag_Variant=1 LIMIT 100001",
            columns.join(",")
        );
        let mut statement = self.connection.prepare(&sql).map_err(format_error)?;
        let rows: Vec<Vec<Value>> = statement
            .query_map([self.variant], |r| {
                (0..columns.len()).map(|i| r.get(i)).collect()
            })
            .map_err(format_error)?
            .collect::<rusqlite::Result<_>>()
            .map_err(format_error)?;
        if rows.len() > 100_000 {
            return Err(format_error("optional geometry exceeds row budget"));
        }
        Ok(Some(rows))
    }

    fn drawing_routes(&self, result: &mut DrawingGeometry) -> Result<()> {
        let Some(area) = result.area else {
            return Ok(());
        };
        let Some(elements) = self.geometry_rows(
            "GraphicElement",
            &["GraphicElement_ID", "Element_ID", "GraphicArea_ID"],
            result,
        )?
        else {
            return Ok(());
        };
        let Some(terminals) = self.geometry_rows(
            "GraphicTerminal",
            &[
                "GraphicTerminal_ID",
                "GraphicElement_ID",
                "Terminal_ID",
                "GraphicArea_ID",
                "PosX",
                "PosY",
            ],
            result,
        )?
        else {
            return Ok(());
        };
        let Some(bends) = self.geometry_rows(
            "GraphicBucklePoint",
            &[
                "GraphicPoint_ID",
                "GraphicTerminal_ID",
                "NoPoint",
                "PosX",
                "PosY",
            ],
            result,
        )?
        else {
            return Ok(());
        };
        self.join_drawing_routes(result, area, elements, terminals, bends);
        Ok(())
    }

    #[allow(clippy::too_many_lines)] // Keep three related identity joins and omission policy together.
    fn join_drawing_routes(
        &self,
        result: &mut DrawingGeometry,
        area: i64,
        elements: Vec<Vec<Value>>,
        terminals: Vec<Vec<Value>>,
        bends: Vec<Vec<Value>>,
    ) {
        let mut graphic_elements = BTreeMap::new();
        let mut bad_elements = BTreeSet::new();
        let mut seen_elements = BTreeSet::new();
        for row in elements {
            let Some((graphic, element, a)) =
                (|| Some((integer(&row[0])?, integer(&row[1])?, integer(&row[2])?)))()
            else {
                result.note("GraphicElement.invalid_identity", 0);
                continue;
            };
            if a != area {
                result.note("GraphicElement.unselected_area", element);
                continue;
            }
            match self.elements.get(&element) {
                None => {
                    result.note("GraphicElement.unknown_element", element);
                    continue;
                }
                Some(kind) if kind != "Line" => continue,
                Some(_) => {}
            }
            let previous = graphic_elements.insert(graphic, element);
            let duplicate_element = !seen_elements.insert(element);
            if previous.is_some() || duplicate_element {
                if let Some(previous) = previous {
                    bad_elements.insert(previous);
                }
                bad_elements.insert(element);
                result.note("GraphicElement.duplicate", element);
            }
        }
        let mut ports: BTreeMap<i64, BTreeMap<i64, (i64, [f64; 2])>> = BTreeMap::new();
        let mut terminal_owner = BTreeMap::new();
        for row in terminals {
            let Some((id, graphic, terminal, a, position)) = (|| {
                Some((
                    integer(&row[0])?,
                    integer(&row[1])?,
                    integer(&row[2])?,
                    integer(&row[3])?,
                    point(&row[4..6])?,
                ))
            })() else {
                if let Some(element) = integer(&row[1]).and_then(|id| graphic_elements.get(&id)) {
                    bad_elements.insert(*element);
                }
                result.note("GraphicTerminal.invalid_fields", 0);
                continue;
            };
            let Some(&element) = graphic_elements.get(&graphic) else {
                continue;
            };
            if a != area {
                bad_elements.insert(element);
                result.note("GraphicTerminal.mixed_area", element);
                continue;
            }
            let Some(native) = self
                .terminals
                .get(&terminal)
                .filter(|t| t.element == element && matches!(t.position, 1 | 2))
            else {
                bad_elements.insert(element);
                result.note("GraphicTerminal.unresolved_port", terminal);
                continue;
            };
            let duplicate_port = ports
                .entry(element)
                .or_default()
                .insert(native.position, (id, position))
                .is_some();
            let previous = terminal_owner.insert(id, element);
            if duplicate_port || previous.is_some() {
                if let Some(previous) = previous {
                    bad_elements.insert(previous);
                }
                bad_elements.insert(element);
                result.note("GraphicTerminal.duplicate", element);
            }
        }
        let mut vertices: BTreeMap<i64, Vec<[f64; 2]>> = BTreeMap::new();
        for row in bends {
            let Some(id) = integer(&row[1]) else {
                result.note("GraphicBucklePoint.invalid_identity", 0);
                continue;
            };
            let Some(&element) = terminal_owner.get(&id) else {
                result.note("GraphicBucklePoint.unknown_terminal", id);
                continue;
            };
            if integer(&row[2]) != Some(1) || point(&row[3..5]).is_none() {
                bad_elements.insert(element);
                result.note("GraphicBucklePoint.unsupported_order_or_value", element);
                continue;
            }
            vertices
                .entry(id)
                .or_default()
                .push(point(&row[3..5]).unwrap());
        }
        for (element, ports) in ports {
            if bad_elements.contains(&element) {
                continue;
            }
            let (Some(&(a, start)), Some(&(b, end))) = (ports.get(&1), ports.get(&2)) else {
                result.note("GraphicTerminal.incomplete_line", element);
                continue;
            };
            let va = vertices.get(&a).map_or(&[][..], Vec::as_slice);
            let vb = vertices.get(&b).map_or(&[][..], Vec::as_slice);
            // Native multi-bend direction is not yet independently verified.
            // One bend per side needs no ordering guess; symbol centers are not
            // waypoints. Preserve terminal endpoints, not busbar midpoints.
            if va.len() > 1 || vb.len() > 1 {
                result.note("GraphicBucklePoint.multiple_per_port", element);
                continue;
            }
            let mut route = vec![start];
            route.extend_from_slice(va);
            route.extend(vb.iter().rev().copied());
            route.push(end);
            route.dedup();
            if route.len() < 2 {
                result.note("GraphicTerminal.degenerate_route", element);
                continue;
            }
            result.routes.insert(element, route);
        }
    }
}
