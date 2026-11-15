//! Stateful static Vsource interpretation, calibrated before reader integration.
//!
//! Raw assignments and edit boundaries matter: sequence defaults and computed
//! short-circuit quantities feed later assignments. No engine is executed here.

use num_complex::Complex64;

// Pinned DSS C-API 87d85c26, DSSUcomplex.PDEGtoComplex. VSource adds
// 360 degrees before this conversion. Reducing modulo 360 or using PI/180
// changes source phasors because this native constant is rounded.
pub(super) const NATIVE_DEGREES_PER_RADIAN: f64 = 57.295_779_51;

use super::lex::{Value, VarMap};
use super::raw::{RawDss, RawObject};

type Result<T> = std::result::Result<T, String>;

#[derive(Debug)]
pub(super) struct ResolvedSource {
    pub bus1: String,
    pub bus2: String,
    pub emf: Vec<Complex64>,
    pub impedance: Vec<Vec<Complex64>>,
    pub nominal_phase_kv: f64,
    pub base_mva: f64,
}

/// The native `VSource` takes its sequence third by division
/// (`RecalcElementData`: `(2.0 * X1 + X0) / 3.0`).
const NATIVE_THIRD: crate::sequence::SequenceThird = crate::sequence::SequenceThird::Divide;

use crate::electrical::{SQRT_3, calc_line_current};

/// A short circuit level in MVA from its current in amperes at `kv`
/// ([`crate::electrical::calc_line_apparent_power`]), `factor · kV · I / 1000`.
fn apparent_power(kv: f64, current: f64, factor: f64) -> f64 {
    crate::electrical::calc_line_apparent_power(
        kv,
        current,
        factor,
        crate::electrical::ApparentPowerOrder::FactorFirst,
    ) / 1000.0
}

#[derive(Clone, Copy)]
enum ImpedanceSpec {
    Mva,
    Current,
    Sequence,
}

/// The impedance base `kV² / MVA` ([`crate::electrical::calc_impedance_base`]).
/// The parser holds every base positive, so an unusable one only arises by
/// overflow, and is refused.
fn impedance_base(kv: f64, mva: f64) -> Result<f64> {
    crate::electrical::calc_impedance_base(kv, mva)
        .ok_or_else(|| "native source impedance base kV² / MVA is not positive and finite".into())
}

// Shared with the canonical writer's exact inverse of the native MVA form.
pub(super) fn short_circuit_positive(kv: f64, mva3: f64, xr1: f64) -> Result<Complex64> {
    let (r, x) = crate::electrical::calc_impedance_from_ratio(
        impedance_base(kv, mva3)?,
        xr1,
        crate::electrical::ImpedanceRatioForm::OpenDssReactanceOverResistance,
    );
    Ok(Complex64::new(r, x))
}

pub(super) fn short_circuit_zero(
    kv: f64,
    isc1: f64,
    factor: f64,
    xr0: f64,
    positive: Complex64,
) -> Result<Complex64> {
    let (r, x) = (positive.re, positive.im);
    let quadratic = 1.0 + xr0.powi(2);
    let linear = 4.0 * (r + x * xr0);
    let constant = 4.0 * (r * r + x * x) - (3.0 * kv * 1000.0 / factor / isc1).powi(2);
    let discriminant = linear * linear - 4.0 * quadratic * constant;
    if discriminant < 0.0 {
        return Err("source short-circuit profile has no real zero-sequence resistance".into());
    }
    let r0 = (-linear + discriminant.sqrt()) / (2.0 * quadratic);
    Ok(Complex64::new(r0, r0 * xr0))
}

struct State {
    phases: usize,
    kv: f64,
    pu: f64,
    angle: f64,
    sequence: f64,
    base_frequency: f64,
    frequency: f64,
    base_mva: f64,
    mva3: f64,
    mva1: f64,
    isc3: f64,
    isc1: f64,
    xr1: f64,
    xr0: f64,
    z: [Complex64; 3], // zero, positive, negative sequence
    pu_z: [Complex64; 3],
    z_written: [bool; 3],
    pu_written: [bool; 3],
    spec: ImpedanceSpec,
    quasi_ideal: bool,
    ideal_z: Complex64,
    bus1: String,
    bus2: String,
    bus2_defined: bool,
}

impl State {
    fn new(default_frequency: f64) -> Result<Self> {
        if !default_frequency.is_finite() || default_frequency <= 0.0 {
            return Err("source constructor frequency must be positive finite".into());
        }
        let mut state = Self {
            phases: 3,
            kv: 115.0,
            pu: 1.0,
            angle: 0.0,
            sequence: 1.0,
            base_frequency: default_frequency,
            frequency: default_frequency,
            base_mva: 100.0,
            mva3: 2000.0,
            mva1: 2100.0,
            isc3: 10_000.0,
            isc1: 10_540.0,
            xr1: 4.0,
            xr0: 3.0,
            z: [
                Complex64::new(1.9, 5.7),
                Complex64::new(1.65, 6.6),
                Complex64::new(1.65, 6.6),
            ],
            pu_z: [Complex64::default(); 3],
            z_written: [false; 3],
            pu_written: [false; 3],
            spec: ImpedanceSpec::Mva,
            quasi_ideal: false,
            ideal_z: Complex64::new(1e-6, 0.001),
            bus1: "sourcebus".into(),
            bus2: "sourcebus.0.0.0".into(),
            bus2_defined: false,
        };
        state.recalculate()?;
        Ok(state)
    }

    // Native PropertySideEffects, kept together to show assignment ordering.
    #[expect(clippy::too_many_lines)]
    fn assign(&mut self, key: &str, value: &Value, vars: &VarMap) -> Result<()> {
        let scalar = || -> Result<f64> {
            let v = value
                .to_f64(Some(vars))
                .map_err(|e| format!("{key}: {e}"))?;
            if !v.is_finite() {
                return Err(format!("{key} must be finite"));
            }
            Ok(v)
        };
        let positive = || -> Result<f64> {
            let v = scalar()?;
            if v <= 0.0 {
                return Err(format!("{key} must be positive"));
            }
            Ok(v)
        };
        match key {
            "phases" => {
                let n = scalar()?.round();
                if !(1.0..=64.0).contains(&n) {
                    return Err("source phases outside 1..=64".into());
                }
                self.phases = n as usize;
            }
            "basekv" => self.kv = positive()?,
            "pu" => self.pu = scalar()?,
            "angle" => self.angle = scalar()?,
            "frequency" => self.frequency = positive()?,
            "basefreq" => self.base_frequency = positive()?,
            "basemva" => self.base_mva = positive()?,
            "mvasc3" => {
                self.mva3 = positive()?;
                self.spec = ImpedanceSpec::Mva;
            }
            "mvasc1" => {
                self.mva1 = positive()?;
                self.spec = ImpedanceSpec::Mva;
            }
            "isc3" => {
                self.isc3 = positive()?;
                self.spec = ImpedanceSpec::Current;
            }
            "isc1" => {
                self.isc1 = positive()?;
                self.spec = ImpedanceSpec::Current;
            }
            "x1r1" => self.xr1 = positive()?,
            "x0r0" => self.xr0 = positive()?,
            "r1" => {
                self.z[1].re = scalar()?;
                self.z[2].re = self.z[1].re;
                self.spec = ImpedanceSpec::Sequence;
            }
            "x1" => {
                self.z[1].im = scalar()?;
                self.z[2].im = self.z[1].im;
                self.spec = ImpedanceSpec::Sequence;
            }
            "r0" => {
                self.z[0].re = scalar()?;
                self.spec = ImpedanceSpec::Sequence;
            }
            "x0" => {
                self.z[0].im = scalar()?;
                self.spec = ImpedanceSpec::Sequence;
            }
            "z0" | "z1" | "z2" | "puz0" | "puz1" | "puz2" => {
                let pu = key.starts_with("pu");
                let i = usize::from(key.as_bytes()[key.len() - 1] - b'0');
                let z = complex(value, vars)?;
                let (values, written) = if pu {
                    (&mut self.pu_z, &mut self.pu_written)
                } else {
                    (&mut self.z, &mut self.z_written)
                };
                values[i] = z;
                written[i] = true;
                if i == 1 {
                    for j in [0, 2] {
                        if !written[j] {
                            values[j] = z;
                        }
                    }
                }
                self.spec = ImpedanceSpec::Sequence;
            }
            "puzideal" => self.ideal_z = complex(value, vars)?,
            "sequence" => {
                self.sequence = source_enum(
                    &value.text,
                    &[("negative", -1.0), ("zero", 0.0), ("positive", 1.0)],
                )?;
            }
            "model" => {
                self.quasi_ideal =
                    source_enum(&value.text, &[("thevenin", 0.0), ("ideal", 1.0)])? > 0.0;
            }
            "bus1" => {
                self.bus1.clone_from(&value.text);
                if !self.bus2_defined {
                    self.bus2 = value.to_bus_spec().name;
                    for _ in 0..self.phases {
                        self.bus2.push_str(".0");
                    }
                }
            }
            "bus2" => {
                self.bus2.clone_from(&value.text);
                self.bus2_defined = true;
            }
            "scantype" => {
                source_enum(
                    &value.text,
                    &[("none", -1.0), ("zero", 0.0), ("positive", 1.0)],
                )?;
            }
            // References were validated at assignment in the raw layer.
            // The reader separately checks the selected solution profile.
            "spectrum" | "yearly" | "daily" | "duty" | "enabled" => {}
            _ => return Err(format!("unqualified source property {key}")),
        }
        Ok(())
    }

    fn recalculate(&mut self) -> Result<()> {
        let factor = if self.phases == 1 { 1.0 } else { SQRT_3 };
        if matches!(self.spec, ImpedanceSpec::Current) {
            self.mva3 = apparent_power(self.kv, self.isc3, SQRT_3);
            self.mva1 = apparent_power(self.kv, self.isc1, factor);
        }
        if matches!(self.spec, ImpedanceSpec::Mva | ImpedanceSpec::Current) {
            self.z[1] = short_circuit_positive(self.kv, self.mva3, self.xr1)?;
            self.z[2] = self.z[1];
            if matches!(self.spec, ImpedanceSpec::Mva) {
                self.isc3 = calc_line_current(self.mva3 * 1000.0, self.kv, SQRT_3);
                self.isc1 = calc_line_current(self.mva1 * 1000.0, self.kv, factor);
            }
            self.z[0] = short_circuit_zero(self.kv, self.isc1, factor, self.xr0, self.z[1])?;
        } else {
            if self.pu_written[1] {
                let base = impedance_base(self.kv, self.base_mva)?;
                self.z = self.pu_z.map(|v| v * base);
            }
            if self.z[1] == Complex64::default() {
                return Err("native source Z1 cannot be zero".into());
            }
            self.isc3 =
                crate::electrical::calc_wye_phase_voltage(self.kv * 1000.0) / self.z[1].norm();
            if self.phases == 1 {
                self.z[0] = self.z[1];
                self.z[2] = self.z[1];
            }
            let zs = crate::sequence::calc_sequence_self(self.z[1], self.z[0], NATIVE_THIRD);
            self.isc1 = self.kv * 1000.0 / factor / zs.norm();
            self.mva3 = apparent_power(self.kv, self.isc3, SQRT_3);
            self.mva1 = apparent_power(self.kv, self.isc1, factor);
        }
        if !self.pu_written.iter().any(|&v| v) {
            let base = impedance_base(self.kv, self.base_mva)?;
            self.pu_z = self.z.map(|z| z / base);
        }
        if self
            .z
            .iter()
            .chain(self.pu_z.iter())
            .any(|z| !z.re.is_finite() || !z.im.is_finite())
            || [self.isc3, self.isc1, self.mva3, self.mva1]
                .iter()
                .any(|v| !v.is_finite())
        {
            return Err("source recalculation became nonfinite".into());
        }
        Ok(())
    }

    fn finish(self, frequency: f64) -> Result<ResolvedSource> {
        if !frequency.is_finite() || frequency <= 0.0 {
            return Err("snapshot frequency must be positive finite".into());
        }
        let ratio = frequency / self.base_frequency;
        let quasi = self.quasi_ideal && ratio - 1.0 < 1e-12;
        let mut impedance = vec![vec![Complex64::default(); self.phases]; self.phases];
        let diagonal = if quasi {
            self.ideal_z * impedance_base(self.kv, self.base_mva)?
        } else if self.z[1] == self.z[2] {
            crate::sequence::calc_sequence_self(self.z[1], self.z[0], NATIVE_THIRD)
        } else {
            crate::sequence::calc_circulant_phase_entry(self.z, 0, 0)
        };
        for (i, row) in impedance.iter_mut().enumerate() {
            row[i] = diagonal;
        }
        if !quasi {
            for (i, row) in impedance.iter_mut().enumerate() {
                for (j, value) in row.iter_mut().enumerate() {
                    if i != j {
                        *value = if self.z[1] == self.z[2] {
                            // Native sequence setters compute Xm as Xs-X1;
                            // short-circuit setters use (X0-X1)/3. Their
                            // rounding differs in ill-conditioned networks.
                            let reactance_third = if matches!(self.spec, ImpedanceSpec::Sequence) {
                                crate::sequence::SequenceThird::SelfLessPositive
                            } else {
                                NATIVE_THIRD
                            };
                            Complex64::new(
                                crate::sequence::calc_sequence_mutual(
                                    self.z[1].re,
                                    self.z[0].re,
                                    NATIVE_THIRD,
                                ),
                                crate::sequence::calc_sequence_mutual(
                                    self.z[1].im,
                                    self.z[0].im,
                                    reactance_third,
                                ),
                            )
                        } else if self.phases == 3 {
                            // OpenDSS's rounded rotation (`OPENDSS_ALPHA`).
                            crate::sequence::calc_circulant_phase_entry(self.z, i, j)
                        } else {
                            Complex64::default()
                        };
                    }
                    value.im *= ratio;
                }
            }
        }
        let nominal_phase_kv = if self.phases > 1 {
            self.kv / crate::sequence::calc_polygon_chord(self.phases)
        } else {
            self.kv
        };
        let mut magnitude = self.kv * self.pu * 1000.0;
        if self.phases > 1 {
            magnitude /= crate::sequence::calc_polygon_chord(self.phases);
        }
        if (frequency - self.frequency).abs() > 1e-3 {
            magnitude = 0.0;
        }
        let emf: Vec<_> = (0..self.phases)
            .map(|i| {
                Complex64::from_polar(
                    magnitude,
                    (360.0 + self.angle - self.sequence * i as f64 * 360.0 / self.phases as f64)
                        / NATIVE_DEGREES_PER_RADIAN,
                )
            })
            .collect();
        if impedance
            .iter()
            .flatten()
            .chain(emf.iter())
            .any(|z| !z.re.is_finite() || !z.im.is_finite())
        {
            return Err("source snapshot became nonfinite".into());
        }
        super::primitive::validate(&impedance, "source", 325)?;
        Ok(ResolvedSource {
            bus1: self.bus1,
            bus2: self.bus2,
            emf,
            impedance,
            nominal_phase_kv,
            base_mva: self.base_mva,
        })
    }
}

fn source_enum(value: &str, choices: &[(&str, f64)]) -> Result<f64> {
    // Native source enums compare one leading character but reject values
    // longer than the corresponding canonical name (MinChars=MaxChars=1).
    choices
        .iter()
        .find(|(name, _)| {
            !value.is_empty()
                && value.len() <= name.len()
                && value.as_bytes()[0].eq_ignore_ascii_case(&name.as_bytes()[0])
        })
        .map(|(_, ordinal)| *ordinal)
        .ok_or_else(|| format!("invalid source enum value {value:?}"))
}

fn complex(value: &Value, vars: &VarMap) -> Result<Complex64> {
    let v = value.to_vector(Some(vars)).map_err(|e| e.to_string())?;
    let [r, x] = v.as_slice() else {
        return Err("source sequence impedance requires two components".into());
    };
    if !r.is_finite() || !x.is_finite() {
        return Err("source impedance must be finite".into());
    }
    Ok(Complex64::new(*r, *x))
}

pub(super) fn resolve(
    raw: &RawDss,
    object: &RawObject,
    snapshot_frequency: f64,
) -> Result<ResolvedSource> {
    let replay = raw.source_replay(object);
    if let Some(error) = &replay.invalid_reference {
        return Err(error.clone());
    }
    let mut resets = replay.resets.iter().peekable();
    let mut state = State::new(replay.constructor_frequency)?;
    let mut start = 0;
    for end in object.edit_bounds() {
        // Empty source histories can still transfer constructor state.
        if start == end {
            while let Some(&&(offset, frequency)) = resets.peek() {
                if offset != start {
                    break;
                }
                state = State::new(frequency)?;
                resets.next();
            }
        }
        for (offset, p) in object.props.iter().enumerate().take(end).skip(start) {
            while let Some(&&(reset_at, frequency)) = resets.peek() {
                if reset_at != offset {
                    break;
                }
                state = State::new(frequency)?;
                resets.next();
            }
            let key = p.name.as_deref().ok_or("unresolved source property")?;
            state.assign(key, &p.value, &raw.vars)?;
        }
        state.recalculate()?;
        start = end;
    }
    if resets.next().is_some() {
        return Err("source state reset lies outside its history".into());
    }
    state.finish(snapshot_frequency)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dss::raw::__parse_raw_with;

    fn read(text: &str) -> Result<ResolvedSource> {
        let raw = __parse_raw_with(text, "source.dss", &mut |_: &std::path::Path| {
            Err(std::io::Error::other("no includes"))
        });
        resolve(&raw, raw.find("vsource", "source").unwrap(), 60.0)
    }

    #[test]
    fn primitive_validity_respects_edit_boundaries_without_regularizing_impedance() {
        for enabled in ["yes", "no"] {
            for properties in [
                "phases=1 model=ideal puzideal=[0 0]",
                "phases=2 z1=[1 0] z0=[-0.5 0]",
                "phases=3 z1=[1 0] z0=[0 0]",
                "phases=4 z1=[1 0] z0=[0.25 0]",
            ] {
                let script = format!("New Circuit.c enabled={enabled} {properties}");
                assert!(read(&script).unwrap_err().contains("error 325"), "{script}");
            }
        }
        assert!(read("New Circuit.c phases=1 z1=[0 0]\nEdit Vsource.source z1=[1 0]").is_err());
        assert!(read("New Circuit.c phases=1 z1=[0 0] z1=[1 0]").is_ok());
        assert!(
            read("New Circuit.c phases=3 z1=[1 0] z0=[0 0]\nEdit Vsource.source z0=[1 0]").is_ok()
        );
        let tiny = read("New Circuit.c phases=1 z1=[1e-12 2e-12]").unwrap();
        assert_eq!(tiny.impedance, vec![vec![Complex64::new(1e-12, 2e-12)]]);
    }

    #[test]
    fn sequence_impedance_and_voltage_phase_order_are_independent() {
        let source = read(
            "New Circuit.c phases=3 basekv=0.4156921938165305 sequence=negative z1=[1 2] z0=[4 5]",
        )
        .unwrap();
        assert!(
            (source.emf[1] / source.emf[1].norm()
                - Complex64::from_polar(1.0, 480.0 / NATIVE_DEGREES_PER_RADIAN))
            .norm()
                < 1e-12
        );
        assert!((source.impedance[0][0] - Complex64::new(2.0, 3.0)).norm() < 1e-12);
        assert!((source.impedance[0][1] - Complex64::new(1.0, 1.0)).norm() < 1e-12);
    }

    #[test]
    fn explicit_return_survives_later_bus1_and_phase_assignments() {
        let source = read("New Circuit.c bus2=return.3 phases=1 bus1=feed.1 r1=1 x1=0").unwrap();
        assert_eq!(source.bus1, "feed.1");
        assert_eq!(source.bus2, "return.3");
        assert_eq!(source.impedance, vec![vec![Complex64::new(1.0, 0.0)]]);
    }

    #[test]
    fn scalar_assignment_preserves_native_per_unit_flag_and_recalc_order() {
        let source =
            read("New Circuit.c basekv=1 basemva=1 puz1=[2 3]\nEdit Vsource.source r1=9").unwrap();
        assert_eq!(source.impedance[0][0], Complex64::new(2.0, 3.0));
        assert!(read("New Circuit.c r1=0 x1=0").is_err());
        assert!(read("New Circuit.c phases=1000000000").is_err());
        assert!(read("New Circuit.c basekv=NaN").is_err());
    }

    #[test]
    fn state_copy_resets_target_flags_and_keeps_source_constructor_frequency() {
        let text = "Set DefaultBaseFrequency=60\nNew Circuit.c z1=[1 2]\nSet DefaultBaseFrequency=50\nNew Vsource.mid puz1=[0.1 0.2] like=source\nNew Vsource.copy z2=[9 9] like=mid bus1=copyfeed";
        let raw = __parse_raw_with(text, "copy.dss", &mut |_: &std::path::Path| {
            Err(std::io::Error::other("no includes"))
        });
        let result = resolve(&raw, raw.find("vsource", "copy").unwrap(), 60.0).unwrap();
        assert!((result.emf[0].norm() - 115_000.0 / 3.0_f64.sqrt()).abs() < 1e-9);
        assert_eq!(result.impedance[0][0], Complex64::new(1.0, 2.0));
        assert_eq!(result.impedance[0][1], Complex64::default());
    }

    #[test]
    fn self_copy_does_not_recalculate_an_unfinished_edit() {
        let result =
            read("New Circuit.c z1=[1 2]\nEdit Vsource.source r1=0 x1=0 like=source z1=[1 2]")
                .unwrap();
        assert_eq!(result.impedance[0][0], Complex64::new(1.0, 2.0));
    }

    #[test]
    fn sequence_source_preserves_native_mutual_reactance_rounding() {
        // IEEE modified secondary source; pinned VSource.pas sequence branch.
        let source =
            read("New Circuit.c basekv=46 r1=1.036 x1=10.1433 r0=5.462 x0=34.1897").unwrap();
        assert_eq!(
            source.impedance[0][0].re.to_bits(),
            2.511_333_333_333_333_4_f64.to_bits()
        );
        assert_eq!(
            source.impedance[0][0].im.to_bits(),
            18.158_766_666_666_67_f64.to_bits()
        );
        assert_eq!(
            source.impedance[0][1].im.to_bits(),
            (18.158_766_666_666_67_f64 - 10.1433).to_bits()
        );
        assert_ne!(
            source.impedance[0][1].im.to_bits(),
            ((34.1897_f64 - 10.1433) / 3.0).to_bits()
        );
    }

    #[test]
    #[ignore = "requires observations from the pinned local OpenDSS source-state oracle"]
    fn pinned_source_states() {
        #[derive(serde::Deserialize)]
        struct Case {
            script: String,
            constructor_frequency: f64,
            snapshot_frequency: f64,
            #[serde(default)]
            ordered_frequency: bool,
            target: String,
            #[serde(default)]
            error: Option<String>,
            #[serde(default)]
            bus1: String,
            #[serde(default)]
            bus2: String,
            #[serde(default)]
            emf: Vec<[f64; 2]>,
            #[serde(default)]
            impedance: Vec<Vec<[f64; 2]>>,
        }
        #[derive(serde::Deserialize)]
        struct Report {
            cases: std::collections::BTreeMap<String, Case>,
        }
        let path = std::env::var("POWERIO_SOURCE_STATE_ORACLE").expect("oracle report path");
        let report: Report = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert!(!report.cases.is_empty());
        let mut findings = std::collections::BTreeMap::new();
        for (name, case) in report.cases {
            let script = format!(
                "Set DefaultBaseFrequency={}\n{}",
                case.constructor_frequency, case.script
            );
            let raw = __parse_raw_with(&script, "oracle.dss", &mut |_: &std::path::Path| {
                Err(std::io::Error::other("no includes"))
            });
            let result = resolve(
                &raw,
                raw.find("vsource", &case.target).unwrap(),
                if case.ordered_frequency {
                    // These literal option assignments must survive exactly.
                    assert_eq!(
                        raw.snapshot_frequency().to_bits(),
                        case.snapshot_frequency.to_bits(),
                        "{name}"
                    );
                    raw.snapshot_frequency()
                } else {
                    case.snapshot_frequency
                },
            );
            let mut failures = Vec::new();
            if let Some(error) = case.error {
                assert!(
                    error.contains("real-valued R0"),
                    "unqualified native failure: {error}"
                );
                if result.is_ok() {
                    failures.push("native invalid short-circuit state was accepted".into());
                }
            } else if let Ok(source) = result {
                if !source.bus1.eq_ignore_ascii_case(&case.bus1)
                    || !source.bus2.eq_ignore_ascii_case(&case.bus2)
                {
                    failures.push(format!(
                        "bus endpoints {:?}/{:?} != {:?}/{:?}",
                        source.bus1, source.bus2, case.bus1, case.bus2
                    ));
                }
                assert_eq!(source.emf.len(), case.emf.len());
                for (i, (actual, expected)) in source.emf.iter().zip(&case.emf).enumerate() {
                    let expected = Complex64::new(expected[0], expected[1]);
                    if (*actual - expected).norm() > 1e-6 + 1e-9 * expected.norm() {
                        failures.push(format!("emf[{i}] {actual} != {expected}"));
                    }
                }
                for (i, row) in source.impedance.iter().enumerate() {
                    for (j, &actual) in row.iter().enumerate() {
                        let expected =
                            Complex64::new(case.impedance[i][j][0], case.impedance[i][j][1]);
                        if (actual - expected).norm() > 1e-10 + 1e-10 * expected.norm() {
                            failures.push(format!("Z[{i},{j}] {actual} != {expected}"));
                        }
                    }
                }
            } else {
                failures.push(result.unwrap_err());
            }
            findings.insert(name, failures);
        }
        let output = std::env::var("POWERIO_SOURCE_STATE_RESULT").expect("result report path");
        std::fs::write(output, serde_json::to_vec_pretty(&findings).unwrap()).unwrap();
        let failed: Vec<_> = findings
            .iter()
            .filter(|(_, failures)| !failures.is_empty())
            .collect();
        assert!(failed.is_empty(), "{failed:?}");
    }
}
