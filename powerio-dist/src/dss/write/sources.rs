//! Exact ideal-source circuits for canonical OpenDSS output.

use std::collections::BTreeSet;

use num_complex::Complex64;

use super::*;

pub(super) fn validate(net: &MulticonductorNetwork) -> Result<(), String> {
    crate::readiness::require_electrical_readiness(net).map_err(|e| e.to_string())?;
    if !net.base_frequency().is_finite() || net.base_frequency() <= 0.0 {
        return Err("source output needs a positive finite snapshot frequency".into());
    }
    let mut buses = BTreeMap::new();
    for bus in net.buses() {
        if name_breaks_dss(&bus.id, true)
            || buses.insert(bus.id.to_ascii_lowercase(), bus).is_some()
        {
            return Err(format!(
                "bus {} is ambiguous or not representable in DSS",
                bus.id
            ));
        }
        let mut numbers = BTreeSet::new();
        for (i, terminal) in bus.terminals.iter().enumerate() {
            if bus.grounded.contains(terminal) {
                continue;
            }
            let number = terminal.parse::<u32>().unwrap_or((i + 1) as u32);
            if number == 0 || number > i32::MAX as u32 || !numbers.insert(number) {
                return Err(format!("bus {} terminal mapping is not injective", bus.id));
            }
        }
    }
    for source in net.sources() {
        let bus = buses
            .get(&source.bus.to_ascii_lowercase())
            .ok_or_else(|| format!("source {} has no declared bus", source.name))?;
        let n = source.terminal_map.len();
        if n == 0
            || source.v_magnitude.len() != n
            || source.v_angle.len() != n
            || source
                .v_magnitude
                .iter()
                .any(|v| !v.is_finite() || *v < 0.0)
            || source.v_angle.iter().any(|v| !v.is_finite())
            || source
                .terminal_map
                .iter()
                .any(|t| !bus.terminals.contains(t))
            || source
                .reference_terminal
                .as_ref()
                .is_some_and(|t| !bus.terminals.contains(t))
        {
            return Err(format!(
                "source {} has incomplete voltage constraints",
                source.name
            ));
        }
        for (t, &magnitude) in source.terminal_map.iter().zip(&source.v_magnitude) {
            let same = source.reference_terminal.as_ref() == Some(t)
                || (bus.grounded.contains(t)
                    && source
                        .reference_terminal
                        .as_ref()
                        .is_none_or(|r| bus.grounded.contains(r)));
            if same && magnitude != 0.0 {
                return Err(format!(
                    "source {} fixes a nonzero voltage across identical nodes",
                    source.name
                ));
            }
        }
    }
    loads::validate(net)?;
    super::admittance_shunts::validate(net)?;
    ideal_transformers::validate(net)?;
    for code in net.line_codes().iter().filter(|c| asymmetric(c)) {
        if sequence_impedance(code, 1.0).is_none()
            && super::asymmetric_pair::equivalent(code, 1.0).is_none()
        {
            return Err(format!(
                "linecode {}: asymmetric matrix has no qualified DSS equivalent",
                code.name
            ));
        }
    }
    for line in net.lines() {
        if let Some(code) = net
            .line_codes()
            .iter()
            .find(|c| c.name.eq_ignore_ascii_case(&line.linecode))
            && asymmetric(code)
            && sequence_impedance(code, line.length).is_none()
            && super::asymmetric_pair::equivalent(code, line.length).is_none()
        {
            return Err(format!(
                "line {}: scaled asymmetric impedance has no finite DSS equivalent",
                line.name
            ));
        }
    }
    Ok(())
}

pub(super) fn asymmetric(code: &DistLineCode) -> bool {
    [&code.r_series, &code.x_series].into_iter().any(|matrix| {
        matrix.iter().enumerate().any(|(i, row)| {
            row.iter()
                .enumerate()
                .any(|(j, v)| matrix.get(j).and_then(|r| r.get(i)) != Some(v))
        })
    })
}

pub(super) fn sequence_impedance(code: &DistLineCode, length: f64) -> Option<[Complex64; 3]> {
    if code.n_conductors != 3
        || !length.is_finite()
        || length <= 0.0
        || [&code.g_from, &code.g_to, &code.b_from, &code.b_to]
            .into_iter()
            .any(|m| m.iter().flatten().any(|&v| v != 0.0))
    {
        return None;
    }
    let mut z = [[Complex64::default(); 3]; 3];
    for (i, row) in z.iter_mut().enumerate() {
        for (j, value) in row.iter_mut().enumerate() {
            *value = Complex64::new(
                *code.r_series.get(i)?.get(j)?,
                *code.x_series.get(i)?.get(j)?,
            ) * length;
        }
    }
    // Match the full circulant matrix, not an averaged/symmetrized one.
    for i in 0..3 {
        for j in 0..3 {
            if z[i][j] != z[0][(j + 3 - i) % 3] {
                return None;
            }
        }
    }
    let result = if z[0][1] == z[0][2] {
        // DSS's Z1=Z2 branch uses exact (Z0-Z1)/3 mutual impedance.
        // The truncated rotation constants below apply only to Z1!=Z2.
        let sequence = z[0][0] - z[0][1];
        [z[0][0] + 2.0 * z[0][1], sequence, sequence]
    } else {
        let alpha = Complex64::new(-0.5, 0.866_025);
        let first_rotation = alpha - 1.0;
        let second_rotation = alpha * alpha - 1.0;
        let upper_delta = 3.0 * (z[0][1] - z[0][0]);
        let lower_delta = 3.0 * (z[0][2] - z[0][0]);
        let denominator = first_rotation * first_rotation - second_rotation * second_rotation;
        let positive = (upper_delta * first_rotation - lower_delta * second_rotation) / denominator;
        let negative = (lower_delta * first_rotation - upper_delta * second_rotation) / denominator;
        let zero = 3.0 * z[0][0] - positive - negative;
        [zero, positive, negative]
    };
    if result[1] == Complex64::default()
        || result
            .iter()
            .any(|v| !v.re.is_finite() || !v.im.is_finite())
    {
        return None;
    }
    Some(result)
}

impl DssWriter {
    pub(super) fn sequence_impedance_branch(
        &mut self,
        net: &MulticonductorNetwork,
        line: &crate::model::DistLine,
    ) -> bool {
        let Some(code) = net
            .line_codes()
            .iter()
            .find(|c| c.name.eq_ignore_ascii_case(&line.linecode))
        else {
            return false;
        };
        if !asymmetric(code) {
            return false;
        }
        if code.n_conductors == 2 {
            self.asymmetric_pair_branch(net, line, code);
        } else {
            let z =
                sequence_impedance(code, line.length).expect("validated exact impedance profile");
            let mut text = format!(
                "New Vsource._pio_coupled_{} phases=3 bus1={} bus2={} basekv=1 pu=0 frequency={} basefreq={}",
                line.name,
                self.bus_ref(&line.bus_from, &line.terminal_map_from),
                self.bus_ref(&line.bus_to, &line.terminal_map_to),
                num(net.base_frequency()),
                num(net.base_frequency()),
            );
            for (i, value) in z.iter().enumerate() {
                let _ = write!(text, " z{i}=[{} {}]", num(value.re), num(value.im));
            }
            self.line_out(&text);
            self.warn(&C::EMIT_DSS_IMPEDANCE_EQUIVALENT, format!(
            "line {}: complete asymmetric sequence impedance emitted as zero-EMF Vsource._pio_coupled_{}", line.name, line.name,
        ));
        }
        if code.i_max.is_some()
            || code.s_max.is_some()
            || line.i_max.is_some()
            || line.s_max.is_some()
        {
            self.warn(
                &C::EMIT_DSS_FIELD_DROPPED,
                format!(
                    "line {}: impedance equivalent cannot carry line ratings",
                    line.name
                ),
            );
        }
        for key in line.extras.keys().chain(code.extras.keys()) {
            self.warn(
                &C::EMIT_DSS_EXTRAS_DROPPED,
                format!(
                    "line {}: extra {key} not emitted on impedance equivalent",
                    line.name
                ),
            );
        }
        true
    }

    pub(super) fn exact_sources(&mut self, net: &MulticonductorNetwork) {
        let mut used: BTreeSet<String> = net
            .buses()
            .iter()
            .map(|v| v.id.to_ascii_lowercase())
            .chain(net.sources().iter().map(|v| v.name.to_ascii_lowercase()))
            .chain(net.lines().iter().map(|v| v.name.to_ascii_lowercase()))
            .collect();
        let mut count = 0;
        for source in net.sources() {
            if count == 0 && self.native_thevenin_source(net, source) {
                count += 1;
                continue;
            }
            let reference = source.reference_terminal.as_ref().map_or_else(
                || format!("{}.0", source.bus),
                |r| self.bus_ref(&source.bus, std::slice::from_ref(r)),
            );
            let mut emitted = 0;
            for ((terminal, &magnitude), &angle) in source
                .terminal_map
                .iter()
                .zip(&source.v_magnitude)
                .zip(&source.v_angle)
            {
                let port = self.bus_ref(&source.bus, std::slice::from_ref(terminal));
                if port == reference {
                    continue; // validate proved this is a redundant zero constraint
                }
                let mut name = format!("_pio_ideal_{count}");
                while !used.insert(name.to_ascii_lowercase()) {
                    name.push('_');
                }
                let head = if count == 0 {
                    let circuit = net
                        .name()
                        .as_deref()
                        .filter(|n| !n.trim().is_empty())
                        .unwrap_or("converted");
                    self.check_name("circuit", circuit);
                    format!("New Circuit.{circuit}")
                } else {
                    format!("New Vsource.{name}")
                };
                // R + (-R) is exact: internal-node KCL eliminates both
                // resistances and leaves V(port)-V(reference)=E. Neither is
                // an epsilon, and each conductor retains its own phasor.
                self.line_out(&format!(
                    "{head} phases=1 bus1={name}.1 bus2={reference} basekv={} pu={} angle={} frequency={} basefreq={} r1=1 x1=0 r0=1 x0=0",
                    num(if magnitude == 0.0 { 0.001 } else { magnitude / 1000.0 }),
                    i32::from(magnitude != 0.0), num(angle * crate::dss::source::NATIVE_DEGREES_PER_RADIAN - 360.0),
                    num(net.base_frequency()), num(net.base_frequency()),
                ));
                self.line_out(&format!(
                    "New Line.{name} phases=1 bus1={name}.1 bus2={port} length=1 r1=-1 x1=0 r0=-1 x0=0 c1=0 c0=0 basefreq={} units=m",
                    num(net.base_frequency()),
                ));
                count += 1;
                emitted += 1;
            }
            self.warn(&C::EMIT_DSS_SOURCE_EQUIVALENT, format!(
                "source {}: {emitted} voltage constraint(s) emitted with exact resistance cancellation", source.name,
            ));
            for key in source.extras.keys() {
                self.warn(
                    &C::EMIT_DSS_EXTRAS_DROPPED,
                    format!(
                        "source {}: extra {key} is not part of the typed ideal voltage constraint",
                        source.name,
                    ),
                );
            }
        }
        if count == 0 {
            // A DSS circuit always constructs Vsource.source. Explicitly
            // disable it when the typed network states no voltage constraint.
            let circuit = net
                .name()
                .as_deref()
                .filter(|n| !n.trim().is_empty())
                .unwrap_or("converted");
            self.line_out(&format!("New Circuit.{circuit} enabled=no"));
        }
        self.out.push('\n');
    }
}

#[test]
fn typed_snapshot_frequency_supersedes_retained_frequency_options() {
    let mut net = MulticonductorNetwork::default();
    net.buses_mut().push(DistBus::new("feed", vec!["1".into()]));
    net.sources_mut().push(crate::VoltageSource::new(
        "s",
        "feed",
        vec!["1".into()],
        vec![240.0],
        vec![0.0],
    ));
    for key in ["f", "frequency", "basef", "defaultb"] {
        net.options_mut().push((key.into(), "50".into()));
    }
    let output = emit_staged_sources(&net).unwrap();
    let raw = crate::dss::raw::__parse_raw_with(
        &output.text,
        "frequency.dss",
        &mut |_: &std::path::Path| Err(std::io::Error::other("no includes")),
    );
    assert_eq!(raw.snapshot_frequency().to_bits(), 60.0_f64.to_bits());
    let source =
        crate::dss::source::resolve(&raw, raw.find("vsource", "source").unwrap(), 60.0).unwrap();
    assert!((source.emf[0] - Complex64::new(240.0, 0.0)).norm() < 1e-12);
    assert!(output.text.contains("Set Mode=Snapshot ControlMode=Off"));
}

#[test]
fn source_validation_refuses_aliasing_and_conflicting_ground_voltage() {
    use crate::model::VoltageSource;
    let mut net = MulticonductorNetwork::default();
    net.buses_mut()
        .push(DistBus::new("bus", vec!["2".into(), "A".into()]));
    assert!(validate(&net).is_err());
    net.buses_mut()[0] = DistBus::new("bus", vec!["1".into(), "4".into()]);
    net.buses_mut()[0].grounded.push("4".into());
    net.sources_mut().push(VoltageSource::new(
        "source",
        "bus",
        vec!["4".into()],
        vec![1.0],
        vec![0.0],
    ));
    assert!(validate(&net).is_err());
    net.sources_mut()[0].v_magnitude[0] = 0.0;
    assert!(validate(&net).is_ok());
}

#[test]
fn asymmetric_matrices_are_never_averaged_into_the_sequence_profile() {
    let mut net = MulticonductorNetwork::default();
    net.line_codes_mut().push(DistLineCode::new(
        "circulant",
        vec![
            vec![2.0, 1.0, 0.0],
            vec![0.0, 2.0, 1.0],
            vec![1.0, 0.0, 2.0],
        ],
        vec![vec![0.0; 3]; 3],
    ));
    assert!(validate(&net).is_ok());
    net.line_codes_mut()[0].r_series[1][0] = 0.2;
    assert!(validate(&net).is_err());
}

#[test]
fn generated_source_bus_does_not_alias_an_original_bus() {
    use crate::model::VoltageSource;
    let mut net = MulticonductorNetwork::default();
    net.buses_mut()
        .push(DistBus::new("_pio_ideal_0", vec!["1".into()]));
    net.sources_mut().push(VoltageSource::new(
        "test",
        "_pio_ideal_0",
        vec!["1".into()],
        vec![240.0],
        vec![0.0],
    ));
    let output = emit_staged_sources(&net).unwrap();
    let raw =
        crate::dss::raw::__parse_raw_with(&output.text, "test.dss", &mut |_: &std::path::Path| {
            Err(std::io::Error::other("no includes"))
        });
    let source = raw.find("vsource", "source").unwrap();
    let virtual_bus = source.get("bus1").unwrap().to_bus_spec().name;
    assert_ne!(virtual_bus, "_pio_ideal_0");
    assert_eq!(source.get("bus2").unwrap().text, "_pio_ideal_0.0");
    let line = raw.of_class("line").next().unwrap();
    assert_eq!(line.get("bus1").unwrap().to_bus_spec().name, virtual_bus);
    assert_eq!(line.get("bus2").unwrap().text, "_pio_ideal_0.1");
    // Sequence setters reset native line units, so the unit assignment must
    // follow them. The electrical equivalent has an explicit one-metre basis.
    let (restored, diagnostics) = crate::dss::to_network_from_raw(&raw);
    assert!(
        !diagnostics
            .iter()
            .any(|d| d.message().contains("no declared length unit"))
    );
    let branch = restored
        .lines()
        .iter()
        .find(|l| l.name == line.name)
        .unwrap();
    assert_eq!(branch.extras["dss_source_units"], serde_json::json!(4));
    assert!((branch.length - 1.0).abs() < 1e-15);
    let code = restored.linecode(&branch.linecode).unwrap();
    assert!((code.r_series[0][0] + 1.0).abs() < 1e-15);
}
