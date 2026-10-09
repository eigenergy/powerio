//! Ordered linecode electrical state, calibrated against the pinned DSS engine.
//!
//! Matrices are ohms and total shunt siemens per declared length unit. Charging
//! is retained at its construction frequency; changing BaseFrequency in matrix
//! mode does not rebuild it. This module is staged until line lowering uses it.

use std::collections::{HashMap, HashSet};
use std::f64::consts::TAU;

use super::line_history::Snapshot;
use super::raw::{RawDss, RawProp};

#[derive(Clone, Debug)]
pub(super) struct State {
    pub n: usize,
    pub frequency: f64,
    pub units: usize,
    pub sequence: bool,
    pub r: Vec<Vec<f64>>,
    pub x: Vec<Vec<f64>>,
    pub b: Vec<Vec<f64>>,
    pub rg: f64,
    pub xg: f64,
    pub rho: f64,
    pub neutral: usize,
    pub normamps: f64,
    pub emergamps: f64,
    seq: [f64; 6], // R1, X1, R0, X0, C1, C0 (farads)
}

#[derive(Debug)]
pub(super) struct LineMatrices {
    pub r: Vec<Vec<f64>>,
    pub x: Vec<Vec<f64>>,
    pub b: Vec<Vec<f64>>,
}

impl State {
    /// Total series impedance and whole-line charging for a direct linecode
    /// assignment, before further Line setters. Native FetchLineCode copies
    /// these fields, recalculates sequence matrices, and captures Kxg here.
    /// Later edits require ordered Line interpretation, not another call using
    /// the final line properties. Positive-sequence and long-line modes are
    /// outside this helper's ordinary multiconductor pi-section contract.
    pub fn line_matrices(
        &self,
        operating: f64,
        length: f64,
        line_units: usize,
    ) -> Result<LineMatrices, String> {
        if !operating.is_finite() || operating < 0.51 || !length.is_finite() || length <= 0.0 {
            return Err(
                "line requires a positive length and AC operating frequency >= 0.51 Hz".into(),
            );
        }
        let meters = [
            1.0, 1609.344, 304.8, 1000.0, 1.0, 0.3048, 0.0254, 0.01, 0.001,
        ];
        let code_meters = *meters.get(self.units).ok_or("invalid linecode units")?;
        let line_meters = *meters.get(line_units).ok_or("invalid line units")?;
        let length = if self.units == 0 || line_units == 0 {
            length
        } else {
            length * line_meters / code_meters
        };
        let ratio = operating / self.frequency;
        let xgmod = if self.xg == 0.0 {
            0.0
        } else {
            let kxg = self.xg / (658.5 * (self.rho / self.frequency).sqrt()).ln();
            0.5 * kxg * ratio.ln()
        };
        let (mut r, mut x, mut b) = if self.sequence && self.n == 1 {
            (
                vec![vec![self.seq[0]]],
                vec![vec![self.seq[1]]],
                vec![vec![TAU * self.frequency * self.seq[4]]],
            )
        } else {
            (self.r.clone(), self.x.clone(), self.b.clone())
        };
        for i in 0..self.n {
            for j in 0..self.n {
                r[i][j] = (r[i][j] + self.rg * (ratio - 1.0)) * length;
                x[i][j] = (x[i][j] - xgmod) * length * ratio;
                b[i][j] *= length * ratio;
                if !r[i][j].is_finite() || !x[i][j].is_finite() || !b[i][j].is_finite() {
                    return Err("line primitive became nonfinite".into());
                }
            }
        }
        Ok(LineMatrices { r, x, b })
    }

    fn new(frequency: f64) -> Self {
        let mut state = Self {
            n: 3,
            frequency,
            units: 0,
            sequence: true,
            r: Vec::new(),
            x: Vec::new(),
            b: Vec::new(),
            rg: 0.01805,
            xg: 0.155_081,
            rho: 100.0,
            neutral: 3,
            normamps: 400.0,
            emergamps: 600.0,
            seq: [0.058, 0.1206, 0.1784, 0.4047, 3.4e-9, 1.6e-9],
        };
        state.recalc();
        state
    }

    fn recalc(&mut self) {
        // LineCode uses this formula even for one conductor. Line has a
        // different one-phase special case; do not share that shortcut.
        self.r = sequence_matrix(self.n, self.seq[0], self.seq[2]);
        self.x = sequence_matrix(self.n, self.seq[1], self.seq[3]);
        self.b = sequence_matrix(
            self.n,
            TAU * self.frequency * self.seq[4],
            TAU * self.frequency * self.seq[5],
        );
    }

    fn make_like(&mut self, source: &Self) {
        // TLineCodeObj.MakeLike leaves the destination mode and units alone.
        // TDSSObject.MakeLike only copies property tracking, not field values.
        let (units, sequence) = (self.units, self.sequence);
        *self = source.clone();
        self.units = units;
        self.sequence = sequence;
    }

    fn apply(&mut self, prop: &RawProp, raw: &RawDss) -> Result<(), String> {
        let key = prop.name.as_deref().ok_or("unnamed linecode property")?;
        let number = || -> Result<f64, String> {
            let value = prop
                .value
                .to_f64(Some(&raw.vars))
                .map_err(|e| e.to_string())?;
            if value.is_finite() {
                Ok(value)
            } else {
                Err(format!("nonfinite linecode {key}"))
            }
        };
        match key {
            "nphases" => {
                let n = number()?;
                if !(1.0..=32.0).contains(&n) || n.fract() != 0.0 {
                    return Err("invalid linecode conductor count".into());
                }
                let n = n as usize;
                if n != self.n {
                    self.n = n;
                    self.neutral = n;
                    self.recalc();
                }
                self.sequence = true;
            }
            "basefreq" => {
                let frequency = number()?;
                if frequency <= 0.0 {
                    return Err("linecode basefreq must be positive".into());
                }
                self.frequency = frequency;
            }
            "r1" | "x1" | "r0" | "x0" | "c1" | "c0" | "b1" | "b0" => {
                let (index, scale) = match key {
                    "r1" => (0, 1.0),
                    "x1" => (1, 1.0),
                    "r0" => (2, 1.0),
                    "x0" => (3, 1.0),
                    "c1" => (4, 1e-9),
                    "c0" => (5, 1e-9),
                    "b1" => (4, 1e-6 / (TAU * self.frequency)),
                    _ => (5, 1e-6 / (TAU * self.frequency)),
                };
                self.seq[index] = number()? * scale;
                // C0 is absent from the pinned engine's mode-switch side
                // effects. Its ConditionalValue flag governs reads, not writes.
                if key != "c0" {
                    self.sequence = true;
                }
            }
            "rmatrix" | "xmatrix" | "cmatrix" => {
                let rows = prop
                    .value
                    .to_rows(Some(&raw.vars))
                    .map_err(|e| e.to_string())?;
                let mut matrix = symmetric_matrix(&rows, self.n)?;
                if key == "cmatrix" {
                    for value in matrix.iter_mut().flatten() {
                        *value *= TAU * self.frequency * 1e-9;
                    }
                    self.b = matrix;
                } else if key == "rmatrix" {
                    self.r = matrix;
                } else {
                    self.x = matrix;
                }
                self.sequence = false;
            }
            "rg" => self.rg = number()?,
            "xg" => self.xg = number()?,
            "rho" => self.rho = number()?,
            "normamps" => self.normamps = number()?,
            "emergamps" => self.emergamps = number()?,
            "neutral" => {
                let neutral = number()?;
                if neutral < 0.0 || neutral > self.n as f64 || neutral.fract() != 0.0 {
                    return Err("invalid linecode neutral index".into());
                }
                self.neutral = neutral as usize;
            }
            "units" => self.units = units(&prop.value.text)?,
            "kron" if prop.value.text.starts_with(['y', 'Y', 't', 'T']) => {
                return Err("ordered linecode Kron reduction is not implemented yet".into());
            }
            // Reliability, seasonal ratings and line-type metadata do not
            // change this static electrical primitive. Raw source retains them.
            "kron" | "faultrate" | "pctperm" | "repair" | "seasons" | "ratings" | "linetype" => {}
            _ => return Err(format!("unqualified linecode property {key}")),
        }
        Ok(())
    }
}

fn sequence_matrix(n: usize, positive: f64, zero: f64) -> Vec<Vec<f64>> {
    let mut matrix = vec![vec![(zero - positive) * (1.0 / 3.0); n]; n];
    for (i, row) in matrix.iter_mut().enumerate() {
        row[i] = (2.0 * positive + zero) * (1.0 / 3.0);
    }
    matrix
}

// This staged profile requires exactly symmetric input, without averaging.
#[allow(clippy::float_cmp)]
fn symmetric_matrix(rows: &[Vec<f64>], n: usize) -> Result<Vec<Vec<f64>>, String> {
    if rows.len() != n || rows.iter().flatten().any(|v| !v.is_finite()) {
        return Err("invalid linecode matrix".into());
    }
    if rows.iter().all(|row| row.len() == n) {
        // Asymmetric full-matrix parsing remains a separate qualification;
        // do not silently average its two triangles.
        if (0..n).any(|i| (0..n).any(|j| rows[i][j] != rows[j][i])) {
            return Err("asymmetric linecode matrix".into());
        }
        return Ok(rows.to_vec());
    }
    if !rows.iter().enumerate().all(|(i, row)| row.len() == i + 1) {
        return Err("invalid linecode triangular matrix".into());
    }
    let mut matrix = vec![vec![0.0; n]; n];
    for (i, row) in rows.iter().enumerate() {
        for (j, value) in row.iter().enumerate() {
            matrix[i][j] = *value;
            matrix[j][i] = *value;
        }
    }
    Ok(matrix)
}

fn units(text: &str) -> Result<usize, String> {
    let text = text.to_ascii_lowercase();
    let names = [
        ("none", 0),
        ("mi", 1),
        ("kft", 2),
        ("km", 3),
        ("m", 4),
        ("ft", 5),
        ("in", 6),
        ("cm", 7),
        ("mm", 8),
        ("meter", 4),
        ("miles", 1),
    ];
    names
        .iter()
        .find(|(name, _)| *name == text)
        .map(|(_, unit)| *unit)
        .ok_or_else(|| format!("unqualified linecode units {text}"))
}

/// Evaluate captured prefixes without recursion or replaying copied property
/// text as setters. The cache stores at most the bounded evaluation budget.
pub(super) fn resolve(raw: &RawDss, point: Snapshot) -> Result<State, String> {
    let mut cache = HashMap::new();
    let mut pending = vec![(point, false)];
    let mut visiting = HashSet::new();
    let mut budget = 1_usize << 22;
    while let Some((point, ready)) = pending.pop() {
        if cache.contains_key(&point) {
            continue;
        }
        let object = raw.objects.get(point.object).ok_or("missing linecode")?;
        if object.class != "linecode" {
            return Err("expected linecode history".into());
        }
        let history = raw
            .line_history
            .get(&point.object)
            .ok_or("missing linecode history")?;
        let copies = history
            .copies
            .get(..point.copies)
            .ok_or("invalid linecode copy prefix")?;
        if ready {
            let state = replay(raw, point, &cache, &mut budget)?;
            visiting.remove(&point);
            charge(&mut budget, state.n * state.n * 3)?;
            cache.insert(point, state);
        } else {
            if !visiting.insert(point) {
                return Err("cyclic linecode history".into());
            }
            charge(&mut budget, 1)?;
            pending.push((point, true));
            for copy in copies.iter().rev() {
                pending.push((copy.source, false));
            }
        }
    }
    cache
        .remove(&point)
        .ok_or_else(|| "missing resolved linecode".into())
}

fn charge(budget: &mut usize, amount: usize) -> Result<(), String> {
    *budget = budget
        .checked_sub(amount)
        .ok_or("linecode evaluation work limit exceeded")?;
    Ok(())
}

fn replay(
    raw: &RawDss,
    point: Snapshot,
    cache: &HashMap<Snapshot, State>,
    budget: &mut usize,
) -> Result<State, String> {
    let object = &raw.objects[point.object];
    let history = &raw.line_history[&point.object];
    let props = object
        .props
        .get(..point.properties)
        .ok_or("invalid linecode property prefix")?;
    let edits = object
        .edits
        .get(..point.edits)
        .ok_or("invalid linecode edit prefix")?;
    let copies = &history.copies[..point.copies];
    let mut state = State::new(history.constructor_frequency);
    let (mut property, mut edit, mut copy) = (0, 0, 0);
    loop {
        // Native edit boundaries before a copy (even at the same property
        // offset) must be applied. Copied legacy boundaries must be skipped.
        let next = copies.get(copy);
        if edit < edits.len() && edits[edit] == property && next.is_none_or(|c| edit < c.edit_start)
        {
            charge(budget, state.n * state.n * 3)?;
            if state.sequence {
                state.recalc();
            }
            edit += 1;
        } else if let Some(c) = next.filter(|c| c.start == property && c.edit_start == edit) {
            if c.end < c.start
                || c.end > props.len()
                || c.edit_end < edit
                || c.edit_end > edits.len()
            {
                return Err("invalid linecode copy span".into());
            }
            state.make_like(cache.get(&c.source).ok_or("unresolved linecode copy")?);
            property = c.end;
            edit = c.edit_end;
            copy += 1;
        } else if property < props.len() {
            charge(budget, state.n * state.n * 3 + 1)?;
            state.apply(&props[property], raw)?;
            property += 1;
        } else {
            break;
        }
    }
    if edit != edits.len() || copy != copies.len() {
        return Err("unconsumed linecode history".into());
    }
    Ok(state)
}

#[cfg(test)]
mod tests;
