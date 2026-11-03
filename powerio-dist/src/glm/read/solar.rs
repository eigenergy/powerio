//! Static BASEEFFICIENT solar inputs, without weather files or time series.
use super::{Reader, class};
use crate::glm::{GlmQuantityKind, ObjectKey};
use powerio_core::Error;

pub(super) fn quantity_spec(
    class: &str,
    name: &str,
) -> Option<(GlmQuantityKind, Option<&'static str>)> {
    let unit = match (class, name) {
        ("climate", "temperature") | ("solar", "NOCT") => Some("degF"),
        ("climate", "solar_direct") | ("solar", "Rated_Insolation") => Some("W/sf"),
        ("solar", "rated_power") => Some("W"),
        ("solar", "area") => Some("sf"),
        ("solar", "efficiency") => Some("unit"),
        ("solar", "shading_factor" | "soiling" | "derating") => Some("pu"),
        ("solar", "Pmax_temp_coeff") => None,
        _ => return None,
    };
    Some((GlmQuantityKind::Real, unit))
}
pub(super) fn allowed(class: &str, name: &str) -> bool {
    quantity_spec(class, name).is_some()
        || (class == "solar"
            && matches!(
                name,
                "panel_type" | "SOLAR_POWER_MODEL" | "orientation" | "weather"
            ))
}
impl Reader<'_> {
    pub(super) fn solar_dc_power(&self, key: ObjectKey) -> Result<f64, Error> {
        if self
            .text(key, "SOLAR_POWER_MODEL")?
            .is_some_and(|v| v != "BASEEFFICIENT")
            || self
                .text(key, "orientation")?
                .is_some_and(|v| v != "DEFAULT")
        {
            return Err(self.error(key,"solar snapshot requires BASEEFFICIENT with default ideal orientation; other irradiance/thermal models need a verified mapping"));
        }
        let (eff_default, coeff_default) = match self
            .text(key, "panel_type")?
            .as_deref()
            .unwrap_or("SINGLE_CRYSTAL_SILICON")
        {
            "SINGLE_CRYSTAL_SILICON" => (0.2, -0.00437 / 33.8),
            "MULTI_CRYSTAL_SILICON" => (0.15, -0.00416 / 33.8),
            "AMORPHOUS_SILICON" => (0.07, 0.1745 / 33.8),
            _ => {
                return Err(self.error(
                    key,
                    "solar panel type needs an additional verified default profile",
                ));
            }
        };
        let efficiency = self
            .real(key, "efficiency")
            .filter(|v| *v != 0.0)
            .unwrap_or(eff_default);
        let coefficient = self
            .real(key, "Pmax_temp_coeff")
            .filter(|v| *v != 0.0)
            .unwrap_or(coeff_default);
        let rated = self.real(key, "Rated_Insolation").unwrap_or(92.902);
        let area = self.real(key, "area").unwrap_or(0.0);
        let mut maximum = self.real(key, "rated_power").unwrap_or(0.0);
        if efficiency <= 0.0 || efficiency > 1.0 || rated <= 0.0 || area < 0.0 || maximum < 0.0 {
            return Err(self.error(
                key,
                "solar efficiency/rating/area requires an additional native initialization profile",
            ));
        }
        if maximum == 0.0 {
            maximum = rated * efficiency * area;
        } else if area != 0.0 {
            let derived = rated * efficiency * area;
            if derived < maximum * 0.99 || derived > maximum * 1.01 {
                maximum = derived;
            }
        }
        let (irradiance, temperature) = self.solar_weather(key, rated)?;
        let shade = self.real(key, "shading_factor").unwrap_or(1.0);
        if !(0.0..=1.0).contains(&shade) {
            return Err(self.error(key, "solar shading_factor must lie in 0..1"));
        }
        let insolation = (irradiance * shade).max(0.0);
        let noct = self.real(key, "NOCT").unwrap_or(118.4);
        let module_temperature = temperature + (noct - 68.0) / 74.32 * insolation;
        let power = maximum * insolation / rated
            * (1.0 + coefficient * (module_temperature - 77.0))
            * self.real(key, "derating").unwrap_or(0.95)
            * self.real(key, "soiling").unwrap_or(0.95);
        if !power.is_finite() || power < 0.0 {
            return Err(self.error(key,"solar initialized power is nonfinite or negative; this thermal state is not qualified"));
        }
        Ok(power)
    }
    fn solar_weather(&self, key: ObjectKey, rated: f64) -> Result<(f64, f64), Error> {
        let explicit = self.index.resolve_property(key, "weather")?;
        let climates = self
            .index
            .objects()
            .iter()
            .filter(|o| class(o) == "climate")
            .map(crate::glm::GlmObject::key)
            .collect::<Vec<_>>();
        let weather = if let Some(weather) = explicit {
            Some(weather)
        } else if climates.len() <= 1 {
            climates.first().copied()
        } else {
            return Err(self.error(
                key,
                "multiple climates require an explicit solar weather reference",
            ));
        };
        let Some(weather) = weather else {
            return Ok((rated, 59.0));
        };
        if class(&self.index.objects()[weather.ordinal()]) != "climate" {
            return Err(self.error(
                key,
                "solar weather reference must identify a climate object",
            ));
        }
        Ok((
            self.real(weather, "solar_direct").unwrap_or(0.0),
            self.real(weather, "temperature").unwrap_or(59.0),
        ))
    }
}
