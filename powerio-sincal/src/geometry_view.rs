//! Native view metadata retained independently of either electrical network type.
use crate::{DatabaseSnapshot, DrawingGeometry, Result};
use powerio_core::{Error, PioModule};
use rusqlite::types::Value;
use serde_json::{Map, json};

const FIELDS: &[&str] = &[
    "GraphicArea_ID",
    "Flag",
    "Name",
    "VectorX",
    "VectorY",
    "AreaWidth",
    "AreaHeight",
    "Scale1",
    "Scale2",
    "ScalePaper",
    "ScaleReal",
    "CoordSys",
    "RefLat",
    "RefLon",
    "RefPosX",
    "RefPosY",
];

impl DrawingGeometry {
    /// Preserve view metadata once on the module, including across IR.
    pub fn retain_view<T>(&self, module: &mut PioModule<T>) -> std::result::Result<(), Error> {
        if let Some(view) = &self.view {
            module.insert_extension("powerio.sincal.graphic_view", view.clone())?;
        }
        Ok(())
    }
}

impl DatabaseSnapshot {
    pub(super) fn drawing_view(&self, geometry: &mut DrawingGeometry) -> Result<()> {
        let Some(area) = geometry.area else {
            return Ok(());
        };
        let Some(rows) = self.geometry_rows("GraphicAreaTile", FIELDS, geometry)? else {
            return Ok(());
        };
        let mut selected = rows.into_iter().filter(|r| r[0] == Value::Integer(area));
        let Some(row) = selected.next() else {
            geometry.note("GraphicAreaTile.missing_area", area);
            return Ok(());
        };
        if selected.next().is_some() {
            geometry.note("GraphicAreaTile.duplicate_area", area);
            return Ok(());
        }
        let mut fields = Map::new();
        for (name, value) in FIELDS.iter().zip(&row) {
            let value = match value {
                Value::Null => continue,
                Value::Integer(v) => json!(v),
                Value::Real(v) if v.is_finite() => json!(v),
                Value::Text(v) if v.len() <= 1024 => json!(v),
                _ => {
                    geometry.note("GraphicAreaTile.invalid_field", area);
                    continue;
                }
            };
            fields.insert((*name).to_owned(), value);
        }
        geometry.schematic = row[1] == Value::Integer(2);
        match row[1] {
            Value::Integer(1) => geometry.note("GraphicAreaTile.geographic_crs_unverified", area),
            Value::Integer(2) => {}
            _ => geometry.note("GraphicAreaTile.unsupported_mode", area),
        }
        // Preserve source values without applying an undocumented scale/origin
        // transform. AreaWidth/Height are paper cm, while positions are metres.
        // CoordSys and reference coordinates do not authorize newer electrical
        // schemas or a guessed interpretation of an arbitrary projection string.
        geometry.view = Some(json!({"schema": self.version, "variant": self.variant,
            "native_fields": fields, "coordinates_transformed": false}));
        Ok(())
    }
}
