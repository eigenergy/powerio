//! Collapse an isolated, imported source equivalent using current typed values.
//! Unrepresentable phasors, impedance, returns or topology keep the general path.
use num_complex::Complex64;

use super::{BTreeSet, C, DistBus, DssWriter, MulticonductorNetwork, num};
use crate::{DistLine, VoltageSource};

struct Native<'a> {
    line: &'a DistLine,
    kv: f64,
    angle: f64,
    sequence: &'static str,
    zero: Complex64,
    positive: Complex64,
    mva: Option<ShortCircuit>,
}

struct ShortCircuit {
    mva3: f64,
    mva1: f64,
    xr1: f64,
    xr0: f64,
}

// An MVA-defined source uses a different mutual-reactance arithmetic path.
// Derive candidates from typed sequence values and accept only exact forward
// reconstruction; no source property or feeder identity participates.
fn short_circuit(kv: f64, zero: Complex64, positive: Complex64) -> Option<ShortCircuit> {
    use crate::dss::source::{short_circuit_positive, short_circuit_zero};
    let valid = |v: f64| v.is_finite() && v > 0.0;
    if ![zero.re, zero.im, positive.re, positive.im]
        .into_iter()
        .all(valid)
    {
        return None;
    }
    let (mut positive_spec, mut zero_spec) = (None, None);
    'positive: for xr1 in neighbors(positive.im / positive.re)
        .into_iter()
        .filter(|&v| valid(v))
    {
        let mva3 = kv.powi(2) / positive.im / (1.0 + 1.0 / xr1.powi(2)).sqrt();
        for mva3 in neighbors(mva3).into_iter().filter(|&v| valid(v)) {
            if short_circuit_positive(kv, mva3, xr1) == positive {
                positive_spec = Some((mva3, xr1));
                break 'positive;
            }
        }
    }
    let (mva3, xr1) = positive_spec?;
    'zero: for xr0 in neighbors(zero.im / zero.re)
        .into_iter()
        .filter(|&v| valid(v))
    {
        let mva1 = 3.0 * kv.powi(2) / (2.0 * positive + zero).norm();
        for mva1 in neighbors(mva1).into_iter().filter(|&v| valid(v)) {
            let factor = 3.0_f64.sqrt();
            let isc1 = mva1 * 1000.0 / (factor * kv);
            if short_circuit_zero(kv, isc1, factor, xr0, positive).ok() == Some(zero) {
                zero_spec = Some((mva1, xr0));
                break 'zero;
            }
        }
    }
    let (mva1, xr0) = zero_spec?;
    Some(ShortCircuit {
        mva3,
        mva1,
        xr1,
        xr0,
    })
}

fn neighbors(value: f64) -> [f64; 17] {
    let mut result = [value; 17];
    let (mut up, mut down) = (value, value);
    for pair in result[1..].as_chunks_mut::<2>().0 {
        up = up.next_up();
        down = down.next_down();
        pair.copy_from_slice(&[up, down]);
    }
    result
}

#[allow(
    clippy::float_cmp,
    reason = "native matrix reconstruction must be exact"
)]
fn sequence(diagonal: f64, mutual: f64, reactance: bool) -> Option<(f64, f64)> {
    for zero in neighbors(diagonal + 2.0 * mutual) {
        for positive in neighbors(diagonal - mutual) {
            if zero.is_finite()
                && positive.is_finite()
                && (2.0 * positive + zero) / 3.0 == diagonal
                && (if reactance {
                    diagonal - positive
                } else {
                    (zero - positive) / 3.0
                }) == mutual
            {
                return Some((zero, positive));
            }
        }
    }
    None
}

fn isolated(net: &MulticonductorNetwork, source: &VoltageSource) -> bool {
    let at = |bus: &str| bus.eq_ignore_ascii_case(&source.bus);
    net.sources().iter().filter(|s| at(&s.bus)).count() == 1
        && net
            .lines()
            .iter()
            .filter(|l| at(&l.bus_from) || at(&l.bus_to))
            .count()
            == 1
        && !net
            .switches()
            .iter()
            .any(|s| at(&s.bus_from) || at(&s.bus_to))
        && !net
            .transformers()
            .iter()
            .flat_map(|t| &t.windings)
            .any(|w| at(&w.bus))
        && !net.loads().iter().any(|l| at(&l.bus))
        && !net.generators().iter().any(|g| at(&g.bus))
        && !net.ibrs().iter().any(|g| at(&g.bus))
        && !net.shunts().iter().any(|s| at(&s.bus))
        && !net.capacitors().iter().any(|c| at(&c.bus))
}

fn voltage(source: &VoltageSource) -> Option<(f64, f64, &'static str)> {
    let magnitude = *source.v_magnitude.first()?;
    if magnitude <= 0.0 || source.v_magnitude.len() != 3 || source.v_angle.len() != 3 {
        return None;
    }
    let factor = 2.0 * (std::f64::consts::PI / 3.0).sin();
    let kv = neighbors(magnitude * factor / 1000.0)
        .into_iter()
        .find(|kv| (kv * 1000.0 / factor).to_bits() == magnitude.to_bits())?;
    let degrees = crate::dss::source::NATIVE_DEGREES_PER_RADIAN;
    let angle = (source.v_angle[0] + std::f64::consts::TAU) * degrees - 360.0;
    for (name, direction) in [("positive", 1.0), ("negative", -1.0), ("zero", 0.0)] {
        // atan2/norm and polar reconstruction incur a few floating-point ulps.
        // This is not a balance tolerance: material unbalance uses the general
        // per-conductor source circuit, including after typed edits.
        if source
            .v_magnitude
            .iter()
            .zip(&source.v_angle)
            .enumerate()
            .all(|(i, (&m, &a))| {
                let native = Complex64::from_polar(
                    magnitude,
                    (360.0 + angle - direction * i as f64 * 120.0) / degrees,
                );
                let typed = Complex64::from_polar(m, a);
                (native - typed).norm() <= 8.0 * f64::EPSILON * magnitude
            })
        {
            return Some((kv, angle, name));
        }
    }
    None
}

fn equivalent<'a>(net: &'a MulticonductorNetwork, source: &VoltageSource) -> Option<Native<'a>> {
    if !source.name.eq_ignore_ascii_case("source") || source.terminal_map.len() != 3 {
        return None;
    }
    let bus = net
        .buses()
        .iter()
        .find(|b| b.id.eq_ignore_ascii_case(&source.bus))?;
    if source
        .reference_terminal
        .as_ref()
        .is_some_and(|r| !bus.grounded.contains(r))
        || source.terminal_map.iter().any(|t| bus.grounded.contains(t))
        || source.terminal_map.iter().collect::<BTreeSet<_>>().len() != 3
        || !net.component_locations().is_empty()
    {
        return None;
    }
    // Preserve any user-added metadata, bounds or location on the EMF bus.
    let mut plain = DistBus::new(&bus.id, bus.terminals.clone());
    plain.grounded.clone_from(&bus.grounded);
    if *bus != plain || !isolated(net, source) {
        return None;
    }
    let line = net
        .lines()
        .iter()
        .find(|l| l.bus_from.eq_ignore_ascii_case(&source.bus))?;
    if line.name != "_pio_vsource_source_impedance"
        || line.terminal_map_from != source.terminal_map
        || line.terminal_map_to.len() != 3
        || line.route.is_some()
        || line.i_max.is_some()
        || line.s_max.is_some()
        || !line.extras.is_empty()
        || !line.length.is_finite()
        || line.length <= 0.0
    {
        return None;
    }
    let code = net
        .line_codes()
        .iter()
        .find(|c| c.name.eq_ignore_ascii_case(&line.linecode))?;
    // Provenance only identifies a compiler-created internal branch. It never
    // supplies electrical parameters or overrides the edited typed matrices.
    if code.source.as_deref() != Some("OpenDSS Vsource.source Thevenin impedance")
        || code.n_conductors != 3
        || code.i_max.is_some()
        || code.s_max.is_some()
        || !code.extras.is_empty()
        || [&code.g_from, &code.g_to, &code.b_from, &code.b_to]
            .into_iter()
            .any(|m| m.iter().flatten().any(|&v| v != 0.0))
    {
        return None;
    }
    let mut values = Vec::new();
    for matrix in [&code.r_series, &code.x_series] {
        if matrix.len() != 3 || matrix.iter().any(|row| row.len() != 3) {
            return None;
        }
        let diagonal = matrix[0][0] * line.length;
        let mutual = matrix[0][1] * line.length;
        if matrix.iter().enumerate().any(|(i, row)| {
            row.iter().enumerate().any(|(j, &v)| {
                (v * line.length).to_bits() != (if i == j { diagonal } else { mutual }).to_bits()
            })
        }) {
            return None;
        }
        values.push((diagonal, mutual));
    }
    let r = sequence(values[0].0, values[0].1, false)?;
    let explicit_x = sequence(values[1].0, values[1].1, true);
    let x = explicit_x.or_else(|| sequence(values[1].0, values[1].1, false))?;
    let zero = Complex64::new(r.0, x.0);
    let positive = Complex64::new(r.1, x.1);
    if positive == Complex64::default() {
        return None;
    }
    let (kv, angle, sequence) = voltage(source)?;
    let mva = if explicit_x.is_some() {
        None
    } else {
        Some(short_circuit(kv, zero, positive)?)
    };

    Some(Native {
        line,
        kv,
        angle,
        sequence,
        zero,
        positive,
        mva,
    })
}

impl DssWriter {
    pub(super) fn native_thevenin_source(
        &mut self,
        net: &MulticonductorNetwork,
        source: &VoltageSource,
    ) -> bool {
        let Some(native) = equivalent(net, source) else {
            return false;
        };
        let circuit = net
            .name()
            .as_deref()
            .filter(|n| !n.trim().is_empty())
            .unwrap_or("converted");
        self.check_name("circuit", circuit);
        let port = self.bus_ref(&native.line.bus_to, &native.line.terminal_map_to);
        let impedance = if let Some(mva) = native.mva {
            format!(
                "mvasc3={} mvasc1={} x1r1={} x0r0={}",
                num(mva.mva3),
                num(mva.mva1),
                num(mva.xr1),
                num(mva.xr0)
            )
        } else {
            format!(
                "z0=[{} {}] z1=[{} {}] z2=[{} {}]",
                num(native.zero.re),
                num(native.zero.im),
                num(native.positive.re),
                num(native.positive.im),
                num(native.positive.re),
                num(native.positive.im)
            )
        };
        self.line_out(&format!(
            "New Circuit.{circuit} phases=3 bus1={} basekv={} pu=1 angle={} sequence={} frequency={} basefreq={} {impedance}",
            port,
            num(native.kv), num(native.angle), native.sequence,
            num(net.base_frequency()), num(net.base_frequency()),
        ));
        self.native_source_line = Some(native.line.name.clone());
        if net
            .lines()
            .iter()
            .filter(|line| line.linecode.eq_ignore_ascii_case(&native.line.linecode))
            .count()
            == 1
        {
            self.native_source_code = Some(native.line.linecode.clone());
        }
        self.warn(&C::EMIT_DSS_SOURCE_EQUIVALENT, format!(
            "source {} and internal series branch {} emitted as native Vsource.source from their current typed voltages and impedance",
            source.name, native.line.name,
        ));
        for key in source.extras.keys() {
            self.warn(
                &C::EMIT_DSS_EXTRAS_DROPPED,
                format!(
                    "source {}: extra {key} is not part of the typed voltage constraint",
                    source.name
                ),
            );
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::super::emit_staged_sources;
    use super::*;

    fn model() -> MulticonductorNetwork {
        crate::parse(powerio_core::Source::from_memory("case.dss", b"new circuit.c bus1=feed basekv=12.47 r1=1 x1=2 r0=3 x0=4\nnew line.line1 bus1=feed bus2=end r1=.1 x1=.2 r0=.1 x0=.2 c1=0 c0=0\nnew load.l bus1=end kv=12.47 kw=3 kvar=1\n".as_slice()).unwrap()).unwrap().value().clone()
    }

    #[test]
    fn isolated_source_uses_current_typed_voltage_and_impedance() {
        let mut net = model();
        assert!(equivalent(&net, &net.sources()[0]).is_some());
        for magnitude in &mut net.sources_mut()[0].v_magnitude {
            *magnitude *= 1.125;
        }
        let code = net
            .line_codes_mut()
            .iter_mut()
            .find(|c| c.name == "_pio_vsource_source_impedance")
            .unwrap();
        for value in code.r_series.iter_mut().flatten() {
            *value *= 2.0;
        }
        let folded = equivalent(&net, &net.sources()[0]).unwrap();
        assert!((folded.kv - 12.47 * 1.125).abs() < 1e-12);
        assert!((folded.positive.re - 2.0).abs() < 1e-14);
        let emitted = emit_staged_sources(&net).unwrap();
        assert!(
            !emitted
                .text
                .contains("New Line._pio_vsource_source_impedance")
        );
        assert!(!emitted.text.contains("_pio_ideal_"));
        let restored = crate::parse(
            powerio_core::Source::from_memory("case.dss", emitted.text.as_bytes()).unwrap(),
        )
        .unwrap();
        let after = restored.value();
        assert!((after.sources()[0].v_magnitude[0] - net.sources()[0].v_magnitude[0]).abs() < 1e-9);
        let after_code = after
            .line_codes()
            .iter()
            .find(|c| c.name == "_pio_vsource_source_impedance")
            .unwrap();
        let before_code = net
            .line_codes()
            .iter()
            .find(|c| c.name == "_pio_vsource_source_impedance")
            .unwrap();
        assert_eq!(after_code.r_series, before_code.r_series);
        assert_eq!(after_code.x_series, before_code.x_series);
    }

    #[test]
    fn unbalance_returns_attachments_and_typed_metadata_keep_general_circuit() {
        let original = model();
        for change in 0..8 {
            let mut net = original.clone();
            let bus = net.sources()[0].bus.clone();
            match change {
                0 => net.sources_mut()[0].v_magnitude[1] *= 1.001,
                1 => {
                    net.buses_mut()
                        .iter_mut()
                        .find(|b| b.id == bus)
                        .unwrap()
                        .terminals
                        .push("5".into());
                    net.sources_mut()[0].reference_terminal = Some("5".into());
                }
                2 => net.loads_mut()[0].bus = bus,
                3 => {
                    net.buses_mut()
                        .iter_mut()
                        .find(|b| b.id == bus)
                        .unwrap()
                        .v_max = Some(15000.0);
                }
                4 => {
                    net.line_codes_mut()
                        .iter_mut()
                        .find(|c| c.name == "_pio_vsource_source_impedance")
                        .unwrap()
                        .b_from[0][0] = 1e-6;
                }
                5 => {
                    net.line_codes_mut()
                        .iter_mut()
                        .find(|c| c.name == "_pio_vsource_source_impedance")
                        .unwrap()
                        .x_series[0][1] += 0.01;
                }
                6 => {
                    net.lines_mut()
                        .iter_mut()
                        .find(|l| l.name == "_pio_vsource_source_impedance")
                        .unwrap()
                        .i_max = Some(vec![100.0; 3]);
                }
                _ => {
                    net.lines_mut()
                        .iter_mut()
                        .find(|l| l.name == "_pio_vsource_source_impedance")
                        .unwrap()
                        .route = Some(Vec::new());
                }
            }
            assert!(
                equivalent(&net, &net.sources()[0]).is_none(),
                "change {change}"
            );
        }
    }
}
