use std::f64::consts::TAU;

use crate::dss::geometry::{EarthModel, cache::Context, definition::Definition};
use crate::dss::{linecode, raw::RawDss, raw::RawProp};
use linecode::LineMatrices;

#[derive(Clone, Debug)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent native enable, switch, invalidation and explicit-property flags"
)]
pub(in crate::dss) struct State {
    pub n: usize,
    pub frequency: f64,
    pub bus: [String; 2],
    pub enabled: bool,
    pub closed: [Vec<bool>; 2],
    pub is_switch: bool,
    pub normamps: f64,
    pub emergamps: f64,
    pub length: f64,
    pub units: usize,
    sequence: SequenceState,
    matrices: LineMatrices,
    units_convert: f64,
    code_units: Option<usize>,
    rg: f64,
    xg: f64,
    rho: f64,
    kxg: f64,
    dirty: bool,
    cached: Option<(f64, LineMatrices)>,
    geometry: Option<usize>,
    geometry_frequency: Option<f64>,
    // Native Zinv survives MakeLike and LineCode assignment while geometry
    // remains bound; those operations copy Z/Yc without copying Zinv.
    geometry_impedance: Option<LineMatrices>,
    pub earth_model: EarthModel,
    rho_specified: bool,
}

#[derive(Clone, Debug)]
struct SequenceState {
    values: [f64; 6],
    model: bool,
    changed: bool,
    cap_specified: bool,
}

impl State {
    pub fn new(frequency: f64, name: &str) -> Self {
        let code = linecode::State::new(frequency);
        let mut state = Self {
            n: 3,
            frequency,
            bus: [format!("{name}_1"), format!("{name}_2")],
            enabled: true,
            closed: [vec![true; 3], vec![true; 3]],
            is_switch: false,
            normamps: 400.0,
            emergamps: 600.0,
            length: 1.0,
            units: 0,
            sequence: SequenceState {
                values: code.seq,
                model: true,
                changed: false,
                cap_specified: false,
            },
            matrices: LineMatrices {
                r: code.r,
                x: code.x,
                b: code.b,
            },
            units_convert: 1.0,
            code_units: None,
            rg: code.rg,
            xg: code.xg,
            rho: code.rho,
            kxg: 0.0,
            dirty: true,
            cached: None,
            geometry: None,
            geometry_frequency: None,
            geometry_impedance: None,
            earth_model: EarthModel::Deri,
            rho_specified: false,
        };
        state.update_kxg();
        state.recalc();
        state
    }

    fn resize_terminals(&mut self, n: usize) {
        if self.n != n {
            self.closed = [vec![true; n], vec![true; n]];
        }
        self.n = n;
    }

    pub fn set_closed(
        &mut self,
        terminal: usize,
        conductor: usize,
        closed: bool,
    ) -> Result<(), String> {
        let end = self
            .closed
            .get_mut(terminal.wrapping_sub(1))
            .ok_or("invalid Line terminal")?;
        if conductor > self.n {
            return Err("Line terminal command conductor exceeds its current count".into());
        }
        if conductor == 0 {
            end.fill(closed);
        } else {
            end[conductor - 1] = closed;
        }
        self.dirty = true;
        Ok(())
    }

    fn update_kxg(&mut self) {
        self.kxg = self.xg / (658.5 * (self.rho / self.frequency).sqrt()).ln();
    }

    fn recalc(&mut self) {
        if self.n == 1 {
            self.sequence.values[2] = self.sequence.values[0];
            self.sequence.values[3] = self.sequence.values[1];
            self.sequence.values[5] = self.sequence.values[4];
        }
        let sequence = |positive: f64, zero: f64| {
            let mut rows = vec![vec![(zero - positive) / 3.0; self.n]; self.n];
            for (i, row) in rows.iter_mut().enumerate() {
                row[i] = (2.0 * positive + zero) / 3.0;
            }
            rows
        };
        self.matrices = LineMatrices {
            r: sequence(self.sequence.values[0], self.sequence.values[2]),
            x: sequence(self.sequence.values[1], self.sequence.values[3]),
            b: sequence(
                TAU * self.frequency * self.sequence.values[4],
                TAU * self.frequency * self.sequence.values[5],
            ),
        };
        self.sequence.changed = false;
    }

    pub fn fetch_code(&mut self, code: &linecode::State) {
        self.geometry = None;
        self.frequency = code.frequency;
        self.rg = code.rg;
        self.xg = code.xg;
        self.rho = code.rho;
        self.update_kxg();
        self.code_units = Some(code.units);
        self.units_convert = conversion(code.units, self.units);
        self.normamps = code.normamps;
        self.emergamps = code.emergamps;
        self.resize_terminals(code.n);
        self.sequence.model = code.sequence;
        if code.sequence {
            self.sequence.values = code.seq;
            self.recalc();
        } else {
            self.matrices = LineMatrices {
                r: code.r.clone(),
                x: code.x.clone(),
                b: code.b.clone(),
            };
        }
        self.dirty = true;
    }

    pub fn make_like(&mut self, source: &Self) {
        // Native Line.MakeLike copies selected fields. Buses, units, earth
        // parameters, linecode binding, switch flag and pending recalculation
        // belong to the destination; the cached primitive is not copied.
        if self.n != source.n {
            self.dirty = true;
        }
        self.resize_terminals(source.n);
        self.frequency = source.frequency;
        self.enabled = true;
        self.normamps = source.normamps;
        self.emergamps = source.emergamps;
        self.sequence.values = source.sequence.values;
        self.matrices = source.matrices.clone();
        self.length = source.length;
        self.sequence.model = source.sequence.model;
        self.sequence.cap_specified = source.sequence.cap_specified;
    }

    fn reset_units(&mut self) {
        self.units_convert = 1.0;
        self.units = 0;
        self.code_units = None;
    }

    pub fn apply(&mut self, prop: &RawProp, raw: &RawDss) -> Result<(), String> {
        let key = prop.name.as_deref().ok_or("unnamed Line property")?;
        let number = || -> Result<f64, String> {
            let value = prop
                .value
                .to_f64(Some(&raw.vars))
                .map_err(|e| e.to_string())?;
            if value.is_finite() {
                Ok(value)
            } else {
                Err(format!("nonfinite Line {key}"))
            }
        };
        match key {
            "bus1" => self.bus[0].clone_from(&prop.value.text),
            "bus2" => self.bus[1].clone_from(&prop.value.text),
            "basefreq" => {
                let v = number()?;
                if v <= 0.0 {
                    return Err("Line basefreq must be positive".into());
                }
                self.frequency = v;
            }
            "length" => {
                self.length = number()?;
                if self.length <= 0.0 {
                    return Err("Line length must be positive".into());
                }
                self.dirty = true;
            }
            "phases" => self.set_phases(number()?)?,
            "units" => {
                let units = linecode::units(&prop.value.text);
                self.units_convert = self
                    .code_units
                    .map_or(self.units_convert * conversion(self.units, units), |code| {
                        conversion(code, units)
                    });
                self.units = units;
                self.dirty = true;
            }
            "r1" | "x1" | "r0" | "x0" | "c1" | "c0" | "b1" | "b0" => {
                self.set_sequence(key, number()?);
            }
            "rmatrix" | "xmatrix" | "cmatrix" => {
                let rows = prop
                    .value
                    .to_rows(Some(&raw.vars))
                    .map_err(|e| e.to_string())?;
                let mut matrix = linecode::symmetric_matrix(&rows, self.n)?;
                if key == "cmatrix" {
                    for value in matrix.iter_mut().flatten() {
                        *value *= TAU * self.frequency * 1e-9;
                    }
                    self.matrices.b = matrix;
                    self.sequence.cap_specified = true;
                } else if key == "rmatrix" {
                    self.matrices.r = matrix;
                } else {
                    self.matrices.x = matrix;
                }
                self.sequence.model = false;
                self.geometry = None;
                self.reset_units();
                self.dirty = true;
            }
            "rg" => {
                self.rg = number()?;
                self.dirty = true;
            }
            "xg" => {
                self.xg = number()?;
                self.update_kxg();
                self.dirty = true;
            }
            "rho" => {
                self.rho = number()?;
                self.rho_specified = true;
                self.update_kxg();
            }
            "earthmodel" => self.earth_model = EarthModel::parse(&prop.value.text)?,
            "enabled" => self.enabled = yes(&prop.value.text),
            "switch" => {
                self.is_switch = yes(&prop.value.text);
                if self.is_switch {
                    self.sequence.values = [1.0, 1.0, 1.0, 1.0, 1.1e-9, 1e-9];
                    self.length = 0.001;
                    self.reset_units();
                    self.geometry = None;
                    self.sequence.changed = true;
                    self.dirty = true;
                }
            }
            "normamps" => self.normamps = number()?,
            "emergamps" => self.emergamps = number()?,
            // These fields do not alter an ordinary matrix/sequence primitive.
            "faultrate" | "pctperm" | "repair" | "seasons" | "ratings" | "linetype" => {}
            _ => return Err(format!("unqualified Line property {key}")),
        }
        Ok(())
    }

    fn set_phases(&mut self, n: f64) -> Result<(), String> {
        if !(1.0..=32.0).contains(&n) || n.fract() != 0.0 {
            return Err("invalid Line conductor count".into());
        }
        let n = n as usize;
        if n != self.n {
            if !self.sequence.model {
                return Err("illegal matrix-mode Line phase change (native error 18101)".into());
            }
            self.resize_terminals(n);
            self.recalc();
        }
        self.dirty = true;
        Ok(())
    }

    fn set_sequence(&mut self, key: &str, value: f64) {
        self.geometry = None;
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
        self.sequence.values[index] = value * scale;
        if matches!(key, "c1" | "c0" | "b1" | "b0") {
            self.sequence.cap_specified = true;
        }
        self.reset_units();
        self.sequence.model = true;
        self.sequence.changed = true;
        // The pinned Line side effects do not invalidate YPrim for B aliases.
        if !matches!(key, "b1" | "b0") {
            self.dirty = true;
        }
    }

    pub fn fetch_geometry(
        &mut self,
        object: usize,
        definition: &Definition,
        context: &mut Context<'_>,
    ) -> Result<(), String> {
        let n = if definition.reduce {
            definition.phases
        } else {
            definition.conductors.len()
        };
        if n == 0 || n > 32 {
            return Err("invalid geometry Line conductor count".into());
        }
        self.geometry = Some(object);
        self.geometry_frequency = None;
        self.code_units = None;
        self.resize_terminals(n);
        self.normamps = definition.normamps;
        self.emergamps = definition.emergamps;
        self.sequence.model = false;
        self.sequence.changed = false;
        self.dirty = true;
        if self.rho_specified {
            context.set_rho(object, self.rho)?;
        }
        Ok(())
    }

    pub fn update_geometry_rho(&mut self, context: &mut Context<'_>) -> Result<(), String> {
        if let Some(object) = self.geometry {
            context.set_rho(object, self.rho)?;
            self.dirty = true;
        }
        Ok(())
    }

    pub fn primitive_with_geometry(
        &mut self,
        frequency: f64,
        context: &mut Context<'_>,
    ) -> Result<LineMatrices, String> {
        let Some(object) = self.geometry else {
            return self.primitive(frequency);
        };
        if let Some((f, matrix)) = &self.cached
            && !self.dirty
            && f.to_bits() == frequency.to_bits()
        {
            return Ok(matrix.clone());
        }
        if !frequency.is_finite() || frequency <= 0.51 {
            return Err("Line AC frequency must exceed 0.51 Hz".into());
        }
        if self
            .geometry_frequency
            .is_none_or(|f| f.to_bits() != frequency.to_bits())
        {
            context.active_earth = Ok(self.earth_model);
            let mut matrix = context.calculate(object, frequency)?;
            Context::scale(&mut matrix, self.length, self.units);
            self.geometry_impedance = Some(matrix.clone());
            self.matrices = matrix;
            self.geometry_frequency = Some(frequency);
        }
        let mut effective = self.matrices.clone();
        if let Some(impedance) = &self.geometry_impedance {
            effective.r.clone_from(&impedance.r);
            effective.x.clone_from(&impedance.x);
        }
        if [&effective.r, &effective.x, &effective.b]
            .iter()
            .any(|m| m.len() != self.n || m.iter().any(|row| row.len() != self.n))
        {
            return Err("geometry calculation dimension differs from the Line terminals".into());
        }
        let impedance: Vec<Vec<_>> = self
            .matrices
            .r
            .iter()
            .zip(&effective.x)
            .map(|(r, x)| {
                r.iter()
                    .zip(x)
                    .map(|(&r, &x)| num_complex::Complex64::new(r, x))
                    .collect()
            })
            .collect();
        crate::dss::primitive::validate(&impedance, "Line", 183)?;
        self.cached = Some((frequency, effective.clone()));
        self.dirty = false;
        Ok(effective.clone())
    }

    pub fn physical_length(&self) -> Option<f64> {
        let units = if self.units == 0 {
            self.code_units?
        } else {
            self.units
        };
        if units == 0 {
            None
        } else {
            Some(self.length * conversion(units, 4))
        }
    }

    pub fn solve(&mut self, frequency: f64) -> Result<(), String> {
        if self.enabled {
            self.primitive(frequency)?;
        }
        Ok(())
    }

    pub fn primitive(&mut self, frequency: f64) -> Result<LineMatrices, String> {
        if let Some((cached_frequency, matrix)) = &self.cached
            && !self.dirty
            && cached_frequency.to_bits() == frequency.to_bits()
        {
            return Ok(matrix.clone());
        }
        if self.geometry.is_some() {
            return Err("geometry Line requires a resolved shared calculation context".into());
        }
        if !frequency.is_finite() || frequency <= 0.51 {
            return Err("Line AC frequency must exceed 0.51 Hz".into());
        }
        if self.sequence.changed {
            if !self.sequence.cap_specified {
                self.sequence.values[4] /= conversion(2, self.units);
                self.sequence.values[5] /= conversion(2, self.units);
                self.sequence.cap_specified = true;
            }
            self.recalc();
        }
        let ratio = frequency / self.frequency;
        let length = self.length / self.units_convert;
        let xgmod = if self.xg == 0.0 {
            0.0
        } else {
            0.5 * self.kxg * ratio.ln()
        };
        let mut matrix = self.matrices.clone();
        for i in 0..self.n {
            for j in 0..self.n {
                matrix.r[i][j] = (matrix.r[i][j] + self.rg * (ratio - 1.0)) * length;
                matrix.x[i][j] = (matrix.x[i][j] - xgmod) * length * ratio;
                matrix.b[i][j] *= length * ratio;
                if !matrix.r[i][j].is_finite()
                    || !matrix.x[i][j].is_finite()
                    || !matrix.b[i][j].is_finite()
                {
                    return Err("Line primitive became nonfinite".into());
                }
            }
        }
        let impedance: Vec<Vec<_>> = matrix
            .r
            .iter()
            .zip(&matrix.x)
            .map(|(r, x)| {
                r.iter()
                    .zip(x)
                    .map(|(&r, &x)| num_complex::Complex64::new(r, x))
                    .collect()
            })
            .collect();
        crate::dss::primitive::validate(&impedance, "Line", 183)?;
        self.cached = Some((frequency, matrix.clone()));
        self.dirty = false;
        Ok(matrix)
    }
}

fn conversion(from: usize, to: usize) -> f64 {
    const METERS: [f64; 9] = [
        1.0, 1609.344, 304.8, 1000.0, 1.0, 0.3048, 0.0254, 0.01, 0.001,
    ];
    if from == 0 || to == 0 {
        1.0
    } else {
        METERS[from] / METERS[to]
    }
}

fn yes(text: &str) -> bool {
    text.starts_with(['y', 'Y', 't', 'T'])
}
