//! LoadShape data needed by static UseActual assignment, with bounded storage.
//! No time-series interpolation or simulation is performed.
use super::lex::{Scanner, Value, VarMap};
use std::sync::Arc;

const MAX_POINTS: usize = 1 << 20;
type Result<T> = std::result::Result<T, String>;

#[derive(Clone, Debug)]
pub(super) struct Shape {
    pub use_actual: bool,
    n: usize,
    interval: f64,
    // Direct SngFile can keep native single-precision P storage. Q arrays
    // assigned by DSS scripts are double precision; after MakeLike they may
    // remain allocated but are ignored until P returns to double storage.
    single: bool,
    p: Option<Arc<Vec<f64>>>,
    q: Option<Arc<Vec<f64>>>,
    max_p: f64,
    max_q: f64,
    q_specified: bool,
    error: Option<String>,
}

impl Default for Shape {
    fn default() -> Self {
        Self {
            use_actual: false,
            n: 0,
            interval: 1.0,
            single: false,
            p: None,
            q: None,
            max_p: 1.0,
            max_q: 0.0,
            q_specified: false,
            error: None,
        }
    }
}

impl Shape {
    pub fn property(
        &mut self,
        key: &str,
        value: &Value,
        vars: Option<&VarMap>,
        mut read: impl FnMut(&str) -> Result<Vec<u8>>,
    ) {
        if let Err(error) = self.assign(key, value, vars, &mut read) {
            self.error = Some(format!("{key}: {error}"));
        }
    }

    fn assign(
        &mut self,
        key: &str,
        value: &Value,
        vars: Option<&VarMap>,
        read: &mut impl FnMut(&str) -> Result<Vec<u8>>,
    ) -> Result<()> {
        let number = || value.to_f64(vars).map_err(|_| "invalid number".to_owned());
        match key {
            "npts" => {
                let count = number()?;
                if !(0.0..=MAX_POINTS as f64).contains(&count) {
                    return Err(format!("point count exceeds {MAX_POINTS}"));
                }
                self.n = count.round() as usize;
            }
            "interval" | "minterval" | "sinterval" => self.interval = number()?,
            "hour" => {
                self.interval = 0.0;
                self.single = false;
            }
            "useactual" => self.use_actual = value.to_bool(),
            "pmax" => self.max_p = number()?,
            "qmax" => {
                self.max_q = number()?;
                self.q_specified = true;
            }
            "mult" | "pmult" | "qmult" => {
                self.single = false;
                let mut values = array(value, vars, self.n, read)?;
                if key == "qmult" {
                    if values.len() < self.n {
                        return Err("short Q data leaves unspecified native array entries".into());
                    }
                    self.q = Some(Arc::new(values));
                } else {
                    self.n = self.n.min(values.len());
                    values.truncate(self.n);
                    self.p = Some(Arc::new(values));
                }
            }
            "dblfile" | "sngfile" | "csvfile" | "pqcsvfile" => {
                self.single = key == "sngfile" && self.q.is_none();
                let bytes = read(&value.text)?;
                let time = usize::from(self.interval == 0.0);
                if key == "csvfile" || key == "pqcsvfile" {
                    let text = std::str::from_utf8(&bytes).map_err(|_| "CSV is not UTF-8")?;
                    let p = csv(text, self.n, 1 + time, false, vars)?;
                    if key == "pqcsvfile" {
                        self.q = Some(Arc::new(csv(text, self.n, 2 + time, false, vars)?));
                    }
                    self.n = p.len();
                    self.p = Some(Arc::new(p));
                } else {
                    let all = binary(&bytes, key, self.n * (1 + time))?;
                    if time == 1 && all.len() % 2 != 0 {
                        return Err("incomplete time/value pair".into());
                    }
                    let p = all
                        .into_iter()
                        .skip(time)
                        .step_by(1 + time)
                        .collect::<Vec<_>>();
                    self.n = p.len();
                    self.p = Some(Arc::new(p));
                }
            }
            "mean" | "stddev" | "pbase" | "qbase" => {}
            "memorymapping" if !value.to_bool() => {}
            _ => return Err(format!("static profile property {key} is not qualified")),
        }
        Ok(())
    }

    pub fn finish(&mut self) {
        if let Some(p) = &self.p {
            if p.len() < self.n
                || (!self.single && self.q.as_ref().is_some_and(|q| q.len() < self.n))
            {
                self.error = Some("point count exceeds allocated data".into());
                return;
            }
            let mut peak = None;
            for (i, &value) in p.iter().take(self.n).enumerate() {
                // Native iMaxAbsArrayValue updates only for a strictly larger
                // magnitude, retaining the first equal-magnitude sample.
                if peak.is_none_or(|j: usize| value.abs() > p[j].abs()) {
                    peak = Some(i);
                }
            }
            if let Some(i) = peak {
                self.max_p = p[i];
                if !self.q_specified {
                    self.max_q = if self.single {
                        0.0
                    } else {
                        self.q.as_ref().map_or(0.0, |q| q[i])
                    };
                }
            }
        }
    }

    pub fn make_like(&mut self, source: &Self) {
        // Native copies the arrays/UseActual but preserves the target's Qmax
        // override. An absent source Q array also leaves the target array.
        self.n = source.n;
        self.interval = source.interval;
        self.single = source.single;
        self.p.clone_from(&source.p);
        if source.q.is_some() {
            self.q.clone_from(&source.q);
        }
        self.use_actual = source.use_actual;
        self.error.clone_from(&source.error);
        self.finish();
    }

    pub fn actual_power(&self) -> Result<(f64, f64)> {
        if let Some(error) = &self.error {
            return Err(error.clone());
        }
        if !self.max_p.is_finite() || !self.max_q.is_finite() {
            return Err("non-finite peak power".into());
        }
        Ok((self.max_p, self.max_q))
    }
}

fn array(
    value: &Value,
    vars: Option<&VarMap>,
    count: usize,
    read: &mut impl FnMut(&str) -> Result<Vec<u8>>,
) -> Result<Vec<f64>> {
    let mut scan = Scanner::new(&value.text, vars);
    if let Some(first) = scan.next_param()
        && let Some(key) = first.name
    {
        let bytes = read(&first.value.text)?;
        match key.to_ascii_lowercase().as_str() {
            "dblfile" | "sngfile" => return binary(&bytes, &key.to_ascii_lowercase(), count),
            "file" => {
                let mut column = 1;
                let mut header = false;
                while let Some(p) = scan.next_param() {
                    match p.name.as_deref().map(str::to_ascii_lowercase).as_deref() {
                        Some("column") => {
                            column = usize::try_from(
                                p.value.to_i64(vars).map_err(|_| "invalid CSV column")?,
                            )
                            .map_err(|_| "negative CSV column")?;
                        }
                        Some("header") => header = p.value.to_bool(),
                        _ => return Err("unknown CSV array option".into()),
                    }
                }
                let text = std::str::from_utf8(&bytes).map_err(|_| "CSV is not UTF-8")?;
                return csv(text, count, column, header, vars);
            }
            _ => return Err("unsupported array source".into()),
        }
    }
    let mut values = value.to_vector(vars).map_err(|_| "invalid inline array")?;
    values.resize(count, 0.0);
    if values.iter().any(|v| !v.is_finite()) {
        return Err("non-finite array data".into());
    }
    Ok(values)
}

fn binary(bytes: &[u8], kind: &str, count: usize) -> Result<Vec<f64>> {
    let size = if kind == "dblfile" { 8 } else { 4 };
    let values = bytes
        .chunks_exact(size)
        .take(count)
        .map(|part| {
            if size == 8 {
                f64::from_le_bytes(part.try_into().unwrap())
            } else {
                f64::from(f32::from_le_bytes(part.try_into().unwrap()))
            }
        })
        .collect::<Vec<_>>();
    if values.iter().any(|v| !v.is_finite()) {
        return Err("non-finite binary profile data".into());
    }
    Ok(values)
}

fn csv(
    text: &str,
    count: usize,
    column: usize,
    header: bool,
    vars: Option<&VarMap>,
) -> Result<Vec<f64>> {
    if column == 0 || column > 4096 {
        return Err("CSV column outside supported range".into());
    }
    text.lines()
        .skip(usize::from(header))
        .take(count)
        .map(|line| {
            let mut scan = Scanner::new(line, vars);
            let mut value = Value::default();
            for _ in 0..column {
                value = scan
                    .next_param()
                    .map(|p| p.value)
                    .ok_or("missing CSV column")?;
            }
            let value = value.to_f64(vars).map_err(|_| "invalid CSV number")?;
            if !value.is_finite() {
                return Err("non-finite CSV data".into());
            }
            Ok(value)
        })
        .collect()
}
