use std::collections::BTreeMap;

use powerio_core::Error;

use super::{GlmPropertyValue, ObjectIndex, ObjectKey, Statement, TokenKind};

/// Source-declared object coordinates in decimal degrees. An absent axis is
/// distinct from zero. These common GLM fields do not declare a datum or CRS.
/// Powerflow initialization may subsequently replace a link's location.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GlmCoordinates {
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
}

impl ObjectIndex<'_> {
    /// Evaluate coordinate assignments in expanded source order. Native
    /// `(name.latitude)` / `(name.longitude)` references copy the value at the
    /// assignment, not the referenced object's eventual value. Only already
    /// named objects are visible; an absent referenced axis stays absent.
    ///
    /// The returned vector uses [`ObjectKey::ordinal`]. This is source state,
    /// not initialized engine geometry. Malformed/nonfinite/out-of-range
    /// coordinates fail explicitly instead of relying on native prefix parsing.
    pub fn declared_coordinates(&self) -> Result<Vec<GlmCoordinates>, Error> {
        let mut result = vec![GlmCoordinates::default(); self.objects().len()];
        let mut events = Vec::new();
        for object in self.objects() {
            for statement in object.other_statements() {
                if let Statement::Simple { tokens, span } = statement
                    && tokens
                        .first()
                        .is_some_and(|token| matches!(token.text(), "latitude" | "longitude"))
                {
                    return Err(self.fail(span, "coordinate assignment is missing its value"));
                }
            }
            for property in object.properties() {
                if matches!(property.name, "name" | "latitude" | "longitude") {
                    events.push((object, property));
                }
            }
        }
        // Expanded spans provide one ordering across included source files.
        // This also handles parent properties written after nested objects.
        events.sort_by_key(|(_, property)| property.span.byte_start());
        let mut named: BTreeMap<&str, ObjectKey> = BTreeMap::new();
        for (object, property) in events {
            if property.name == "name" {
                if let Some(name) = object.name() {
                    named.insert(name, object.key());
                }
                continue;
            }
            let fail = |message| self.fail(property.span, message);
            let GlmPropertyValue::Tokens(tokens) = &property.value else {
                return Err(fail("coordinate cannot be an inline object"));
            };
            let [token] = *tokens else {
                return Err(fail(
                    "coordinate requires one literal or object-axis reference",
                ));
            };
            let text = if token.kind() == TokenKind::Quoted {
                token
                    .text()
                    .strip_prefix('"')
                    .and_then(|s| s.strip_suffix('"'))
                    .ok_or_else(|| fail("coordinate strings require double quotes"))?
            } else {
                token.text()
            };
            let latitude = property.name == "latitude";
            let value = if let Some(reference) = text.strip_prefix('(') {
                let (name, axis) = reference
                    .strip_suffix(')')
                    .and_then(|s| s.split_once('.'))
                    .ok_or_else(|| fail("expected (object.latitude) or (object.longitude)"))?;
                if axis != property.name {
                    return Err(fail("coordinate reference must use the same axis"));
                }
                let key = named.get(name).ok_or_else(|| {
                    fail("coordinate reference requires an object named before this assignment")
                })?;
                let point = result[key.ordinal()];
                if latitude {
                    point.latitude
                } else {
                    point.longitude
                }
            } else {
                Some(parse_coordinate(text, latitude).map_err(fail)?)
            };
            let point = &mut result[object.key().ordinal()];
            if latitude {
                point.latitude = value;
            } else {
                point.longitude = value;
            }
        }
        Ok(result)
    }
}

fn parse_coordinate(text: &str, latitude: bool) -> Result<f64, &'static str> {
    let hemispheres = if latitude { ['N', 'S'] } else { ['E', 'W'] };
    let value = if let Some(position) = text.find(hemispheres) {
        let (degrees, tail) = text.split_at(position);
        if degrees.is_empty() || !degrees.bytes().all(|c| c.is_ascii_digit()) {
            return Err("hemisphere coordinates require unsigned integer degrees");
        }
        let degrees = number(degrees)?;
        let (minutes, seconds) = if let Some((minutes, seconds)) = tail[1..].split_once(':') {
            if minutes.is_empty() || !minutes.bytes().all(|c| c.is_ascii_digit()) {
                return Err("DMS coordinates require unsigned integer minutes");
            }
            (number(minutes)?, number(seconds)?)
        } else {
            (number(&tail[1..])?, 0.0)
        };
        if !(0.0..60.0).contains(&minutes) || !(0.0..60.0).contains(&seconds) {
            return Err("coordinate minutes and seconds must be in [0, 60)");
        }
        let magnitude = degrees + minutes / 60.0 + seconds / 3600.0;
        if tail.starts_with(hemispheres[1]) {
            -magnitude
        } else {
            magnitude
        }
    } else {
        number(text)?
    };
    let limit = if latitude { 90.0 } else { 180.0 };
    if !(-limit..=limit).contains(&value) {
        return Err("coordinate is outside the geographic axis range");
    }
    Ok(value)
}

fn number(text: &str) -> Result<f64, &'static str> {
    let value = text
        .parse::<f64>()
        .map_err(|_| "invalid coordinate number")?;
    if !value.is_finite() {
        return Err("coordinate must be finite");
    }
    Ok(value)
}
