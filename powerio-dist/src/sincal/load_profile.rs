//! Explicit daily absolute-power and common-factor snapshots. General Input Data (April 2014)
//! pp. 292–295 and Database Description pp. 84–85 define units and selectors;
//! Load Flow pp. 48–50 defines cyclic repetition. No stored result is used.

use std::collections::BTreeSet;

use super::{
    format_error,
    load::{LoadInput, PowerInput},
    schema::{NativeDatabase, require_table},
    transformer::{integer, number, reference},
};
use crate::Result;

#[derive(Clone, Debug, serde::Serialize)]
pub(super) struct LoadProfileSelection {
    pub profile: i64,
    pub requested_hours: f64,
    pub cyclic_hours: f64,
    pub period_hours: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub power_factors: Option<[f64; 2]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub relative_factor: Option<f64>,
}

#[derive(Clone, Copy)]
enum ProfileFunction {
    CommonFactor,
    AbsolutePower,
}

pub(super) fn validate_time(hours: f64) -> Result<()> {
    if !hours.is_finite() || hours < 0.0 {
        return Err(format_error(
            "snapshot time must be finite nonnegative hours",
        ));
    }
    Ok(())
}

impl NativeDatabase {
    pub fn load_input_at(&self, element: i64, hours: f64) -> Result<LoadInput> {
        validate_time(hours)?;
        let mut input = self.load_input(element)?;
        if input.operating_series == [None; 3] {
            return Ok(input);
        }
        if self.version.to_bits() != 11.5_f64.to_bits() {
            return Err(format_error(
                "daily load profile semantics require verified schema 11.5",
            ));
        }
        let [Some(profile), None, None] = input.operating_series else {
            return Err(format_error(
                "load snapshot requires a single daily profile; weekly/yearly composition is unresolved",
            ));
        };
        let (period, function) = self.daily_power_period(profile)?;
        let points = self.daily_power_points(profile, period, function)?;
        let time = hours.rem_euclid(period);
        let sampled = sample(&points, period, time)?;
        let (power_factors, relative_factor) = match function {
            ProfileFunction::CommonFactor => {
                // The base powers already include the input mode's fP/fQ or
                // fS. Scale each existing branch once, without redistributing
                // total power or replacing its Wye/delta connection.
                scale_power(&mut input.power, sampled[0])?;
                (None, Some(sampled[0]))
            }
            ProfileFunction::AbsolutePower => {
                let factors = self.absolute_profile_power(element, sampled, &mut input.power)?;
                (Some(factors), None)
            }
        };
        input.operating_series = [None; 3];
        input.profile_selection = Some(LoadProfileSelection {
            profile,
            requested_hours: hours,
            cyclic_hours: time,
            period_hours: period,
            power_factors,
            relative_factor,
        });
        Ok(input)
    }

    fn absolute_profile_power(
        &self,
        element: i64,
        sampled: [f64; 2],
        power: &mut PowerInput,
    ) -> Result<[f64; 2]> {
        if !matches!(
            power,
            PowerInput::Total { .. } | PowerInput::DeltaTotal { .. }
        ) {
            return Err(format_error(
                "absolute profile allocation over per-phase input powers is unresolved",
            ));
        }
        // Input Data pp.94 and 292–295: an absolute profile supplies P/Q;
        // the load's respective P/Q input factors still multiply those values.
        // fS belongs to the replaced apparent-power input, not to this profile.
        let mut stmt = self
            .connection
            .prepare("SELECT fP,fQ FROM Load WHERE Element_ID=?1 AND Variant_ID=?2")
            .map_err(format_error)?;
        let factors = stmt
            .query_row([element, self.variant], |row| {
                Ok([row.get::<_, f64>(0)?, row.get::<_, f64>(1)?])
            })
            .map_err(format_error)?;
        if factors.iter().any(|f| !f.is_finite() || *f < 0.0) {
            return Err(format_error(
                "absolute profile requires finite nonnegative P/Q factors",
            ));
        }
        let values = [sampled[0] * factors[0], sampled[1] * factors[1]];
        if values.iter().any(|v| !v.is_finite())
            || (0..2).any(|i| sampled[i] != 0.0 && factors[i] != 0.0 && values[i] == 0.0)
        {
            return Err(format_error("scaled profile power overflows or underflows"));
        }
        *power = match power {
            PowerInput::DeltaTotal { .. } => PowerInput::DeltaTotal {
                p: values[0],
                q: values[1],
            },
            _ => PowerInput::Total {
                p: values[0],
                q: values[1],
            },
        };
        Ok(factors)
    }

    #[allow(clippy::float_cmp)] // Exact native coefficient profiles, not approximate numerical tests.
    fn daily_power_period(&self, profile: i64) -> Result<(f64, ProfileFunction)> {
        require_table(&self.connection, "OpSer", &["OpSer_ID", "Variant_ID"])?;
        let mut stmt = self
            .connection
            .prepare("SELECT * FROM OpSer WHERE OpSer_ID=?1 AND Variant_ID=?2")
            .map_err(format_error)?;
        let mut rows = stmt.query([profile, self.variant]).map_err(format_error)?;
        let row = rows
            .next()
            .map_err(format_error)?
            .ok_or_else(|| format_error(format!("missing OpSer {profile}")))?;
        let function = match integer(row, "Flag_Typ")? {
            1 => ProfileFunction::CommonFactor,
            3 => ProfileFunction::AbsolutePower,
            _ => return Err(format_error("unsupported daily profile function")),
        };
        if integer(row, "Flag_Ser")? != 1 || integer(row, "Flag_Variant")? != 1 {
            return Err(format_error("snapshot requires a local daily profile"));
        }
        for field in ["Power_a1", "Power_b1", "Reduce_a2"] {
            if number(row, field)? != 0.0 {
                return Err(format_error(format!(
                    "profile requires resolution of {field}"
                )));
            }
        }
        // Input Data p.294 explicitly gives direct scaling for a2=0, b2=1.
        // Do not infer topology-dependent coincidence for relative profiles.
        // Absolute-power profiles retain the separately verified zero/zero
        // coefficient profile used by the native CSIRO corpus.
        let reduction_b = match function {
            ProfileFunction::CommonFactor => 1.0,
            ProfileFunction::AbsolutePower => 0.0,
        };
        if number(row, "Reduce_b2")? != reduction_b {
            return Err(format_error("profile requires resolution of Reduce_b2"));
        }
        let period = match number(row, "BaseT")? {
            0.0 => 24.0,
            value => value,
        };
        if period <= 0.0 || rows.next().map_err(format_error)?.is_some() {
            return Err(format_error("invalid or duplicate daily profile"));
        }
        Ok((period, function))
    }

    #[allow(clippy::float_cmp)] // Exact timestamps/endpoints; never merge nearby native samples.
    fn daily_power_points(
        &self,
        profile: i64,
        period: f64,
        function: ProfileFunction,
    ) -> Result<Vec<Point>> {
        require_table(&self.connection, "OpSerVal", &["OpSer_ID", "Variant_ID"])?;
        let mut stmt = self
            .connection
            .prepare("SELECT * FROM OpSerVal WHERE OpSer_ID=?1 AND Variant_ID=?2 ORDER BY OpTime")
            .map_err(format_error)?;
        let mut rows = stmt.query([profile, self.variant]).map_err(format_error)?;
        let mut points: Vec<Point> = Vec::new();
        let mut ids = BTreeSet::new();
        while let Some(row) = rows.next().map_err(format_error)? {
            if points.len() == 100_000 {
                return Err(format_error("daily profile exceeds sample budget"));
            }
            let id = integer(row, "OpSerVal_ID")?;
            let time = number(row, "OpTime")?;
            let curve = integer(row, "Flag_Curve")?;
            if points.last().is_some_and(|p| time <= p.time) {
                return Err(format_error(format!(
                    "OpSer {profile}: duplicate or unordered OpTime {time}"
                )));
            }
            if id <= 0
                || !ids.insert(id)
                || time < 0.0
                || time > period
                || !matches!(curve, 1 | 2)
                || integer(row, "Flag_Variant")? != 1
                || reference(row, "Op_ID")?.is_some()
            {
                return Err(format_error(
                    "invalid, duplicate or unsupported daily profile sample",
                ));
            }
            let values = match function {
                ProfileFunction::CommonFactor => {
                    let factor = number(row, "Factor")?;
                    if factor < 0.0 {
                        return Err(format_error("negative daily profile factor"));
                    }
                    [factor; 2]
                }
                ProfileFunction::AbsolutePower => {
                    [number(row, "P")? * 1000.0, number(row, "Q")? * 1000.0]
                }
            };
            if !values.iter().all(|v| v.is_finite()) {
                return Err(format_error("daily profile power overflows SI units"));
            }
            points.push(Point {
                time,
                values,
                curve,
            });
        }
        if points.first().is_none_or(|p| p.time != 0.0) {
            return Err(format_error(
                "daily snapshot requires a sample at time zero",
            ));
        }
        if points.last().is_some_and(|p| p.time == period) {
            if points.last().unwrap().values != points[0].values {
                return Err(format_error("ambiguous daily profile cyclic endpoint"));
            }
            points.pop();
        }
        Ok(points)
    }
}

struct Point {
    time: f64,
    values: [f64; 2],
    curve: i64,
}

fn scale_power(power: &mut PowerInput, factor: f64) -> Result<()> {
    let scale = |value: &mut f64| -> Result<()> {
        let scaled = *value * factor;
        if !scaled.is_finite() || (*value != 0.0 && factor != 0.0 && scaled == 0.0) {
            return Err(format_error(
                "relative profile power overflows or underflows",
            ));
        }
        *value = scaled;
        Ok(())
    };
    match power {
        PowerInput::Total { p, q } | PowerInput::DeltaTotal { p, q } => {
            scale(p)?;
            scale(q)?;
        }
        PowerInput::Wye { p, q } | PowerInput::Delta { p, q } => {
            for value in p.iter_mut().chain(q) {
                scale(value)?;
            }
        }
    }
    Ok(())
}

#[allow(clippy::float_cmp)] // Exact knots select their declared value; all other times interpolate.
fn sample(points: &[Point], period: f64, time: f64) -> Result<[f64; 2]> {
    let index = points.partition_point(|p| p.time <= time) - 1;
    let left = &points[index];
    if time == left.time || left.curve == 2 {
        return Ok(left.values);
    }
    let (right_time, right) = points
        .get(index + 1)
        .map_or((period, &points[0]), |p| (p.time, p));
    let fraction = (time - left.time) / (right_time - left.time);
    let values =
        std::array::from_fn(|i| (1.0 - fraction) * left.values[i] + fraction * right.values[i]);
    if !values.iter().all(|v| v.is_finite()) {
        return Err(format_error("interpolated daily load power overflows"));
    }
    Ok(values)
}
