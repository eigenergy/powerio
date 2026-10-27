//! Selected absolute daily load powers in the schema-11.5 profile.
//! Input Data (April 2014), pp.292–295, Database Description pp.84–85:
//! OpSerVal P/Q are kW/kvar; Load fP/fQ apply once. No result tables enter here.
use std::cell::OnceCell;
use std::collections::BTreeMap;

use super::{DatabaseSnapshot, NativeRow, Result, error, get, table};

/// `OpSer` and `OpSerVal`, each read on first use and kept for the parse.
/// Before, every profiled load read both tables again. The two stay separate
/// so a missing `OpSerVal` still reports after the series checks.
#[derive(Default)]
pub(super) struct Tables {
    series: OnceCell<BTreeMap<i64, NativeRow>>,
    values: OnceCell<Samples>,
}

/// `OpSerVal` rows with their keys grouped by `OpSer_ID`.
struct Samples {
    rows: BTreeMap<i64, NativeRow>,
    by_series: BTreeMap<i64, Vec<i64>>,
    /// The first row, in key order, whose `OpSer_ID` is not an integer.
    first_invalid: Option<i64>,
}

impl Tables {
    fn series(&self, db: &DatabaseSnapshot) -> Result<&BTreeMap<i64, NativeRow>> {
        if let Some(rows) = self.series.get() {
            return Ok(rows);
        }
        let rows = table(db, "OpSer", "OpSer_ID")?;
        Ok(self.series.get_or_init(|| rows))
    }

    fn values(&self, db: &DatabaseSnapshot) -> Result<&Samples> {
        if let Some(samples) = self.values.get() {
            return Ok(samples);
        }
        let rows = table(db, "OpSerVal", "OpSerVal_ID")?;
        let mut by_series = BTreeMap::<i64, Vec<i64>>::new();
        let mut first_invalid = None;
        for (&key, row) in &rows {
            match row.integer("OpSer_ID") {
                Ok(series) => by_series.entry(series).or_default().push(key),
                Err(_) => {
                    first_invalid.get_or_insert(key);
                }
            }
        }
        Ok(self.values.get_or_init(|| Samples {
            rows,
            by_series,
            first_invalid,
        }))
    }
}

impl Samples {
    /// The samples of one series in key order. A row with an invalid
    /// `OpSer_ID` fails every lookup, at the place a scan of the whole table
    /// in key order would have reached it.
    fn of(&self, id: i64) -> impl Iterator<Item = Result<(i64, &NativeRow)>> {
        let keys = self.by_series.get(&id).map_or(&[][..], Vec::as_slice);
        let invalid = self.first_invalid;
        let before = invalid.map_or(keys.len(), |bad| keys.partition_point(|&k| k < bad));
        keys[..before]
            .iter()
            .map(|&key| Ok((key, &self.rows[&key])))
            .chain(invalid.map(|bad| Err(self.rows[&bad].integer("OpSer_ID").unwrap_err())))
    }
}

#[derive(serde::Serialize)]
pub(super) struct Selection {
    pub profile: i64,
    pub requested_hours: f64,
    pub cyclic_hours: f64,
    pub period_hours: f64,
    pub power_factors: [f64; 2],
    /// Balanced-network power units, MW/Mvar.
    pub p: f64,
    pub q: f64,
}

pub(super) fn validate_time(hours: Option<f64>) -> Result<()> {
    if hours.is_some_and(|t| !t.is_finite() || t < 0.0) {
        return Err(error("snapshot time must be finite nonnegative hours"));
    }
    Ok(())
}

#[allow(clippy::float_cmp)] // Exact stored timestamps/coefficients, not approximate numerical tests.
pub(super) fn absolute_daily(
    db: &DatabaseSnapshot,
    load: &NativeRow,
    hours: Option<f64>,
    tables: &Tables,
) -> Result<Option<Selection>> {
    let Some(id) = load.reference("DayOpSer_ID")? else {
        return Ok(None);
    };
    let requested = hours.ok_or_else(|| load.bad("DayOpSer_ID", "explicit snapshot required"))?;
    let series = get(tables.series(db)?, id, "OpSer")?;
    series.equals("Flag_Ser", 1)?;
    series.equals("Flag_Typ", 3)?;
    // Variant selection is explicit; the stored active-row cache is not selection.
    for field in ["Power_a1", "Power_b1", "Reduce_a2", "Reduce_b2"] {
        if series.number(field)? != 0.0 {
            return Err(series.bad(field, "unresolved profile coefficient"));
        }
    }
    let period = match series.nonnegative("BaseT")? {
        0.0 => 24.0,
        v => v,
    };
    let mut points = Vec::new();
    for sample in tables.values(db)?.of(id) {
        let (key, row) = sample?;
        let time = row.nonnegative("OpTime")?;
        let curve = row.integer("Flag_Curve")?;
        row.inactive(&["Op_ID"])?;
        if key <= 0 || time > period || !matches!(curve, 1 | 2) {
            return Err(row.bad("OpTime/Flag_Curve", "unsupported daily sample"));
        }
        points.push((time, [row.number("P")?, row.number("Q")?], curve));
    }
    points.sort_by(|a, b| a.0.total_cmp(&b.0));
    if points.first().is_none_or(|p| p.0 != 0.0) || points.windows(2).any(|w| w[0].0 >= w[1].0) {
        return Err(series.bad(
            "OpTime",
            "daily series must start at zero with unique timestamps",
        ));
    }
    if points.last().unwrap().0 == period {
        if points.last().unwrap().1 != points[0].1 {
            return Err(series.bad("OpTime", "conflicting cyclic endpoint"));
        }
        points.pop();
    }
    let time = requested.rem_euclid(period);
    let left = points.partition_point(|p| p.0 <= time) - 1;
    let a = points[left];
    let b = points
        .get(left + 1)
        .copied()
        .unwrap_or((period, points[0].1, points[0].2));
    let fraction = if a.2 == 2 {
        0.0
    } else {
        (time - a.0) / (b.0 - a.0)
    };
    let factors = [load.nonnegative("fP")?, load.nonnegative("fQ")?];
    let mut powers = [0.0; 2];
    for i in 0..2 {
        let sample = a.1[i] * (1.0 - fraction) + b.1[i] * fraction;
        powers[i] = sample * 0.001 * factors[i];
        if !powers[i].is_finite() || (sample != 0.0 && factors[i] != 0.0 && powers[i] == 0.0) {
            return Err(series.bad("P/Q", "scaled profile power overflows or underflows"));
        }
    }
    Ok(Some(Selection {
        profile: id,
        requested_hours: requested,
        cyclic_hours: time,
        period_hours: period,
        power_factors: factors,
        p: powers[0],
        q: powers[1],
    }))
}
