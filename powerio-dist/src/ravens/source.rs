//! Preserve a split source's coupled Thevenin impedance as one physical line.
use std::borrow::Cow;
use std::collections::BTreeSet;

use crate::diagnostics::codes as C;
use crate::model::{
    ConductorMatrix, DistBus, DistLine, DistLineCode, MulticonductorNetwork, VoltageSource,
};

use super::terminal_phase;

type Impedance = (Vec<String>, ConductorMatrix, ConductorMatrix);

/// The EnergySource profile regenerates a positive-sequence regular polygon.
pub(super) fn is_unbalanced(source: &VoltageSource) -> bool {
    let hot: Vec<_> = source
        .terminal_map
        .iter()
        .enumerate()
        .filter(|(_, t)| terminal_phase(t).is_some_and(|p| p != "N"))
        .map(|(k, _)| k)
        .collect();
    if hot.len() <= 1 {
        return false;
    }
    let first = hot[0];
    let magnitude = source.v_magnitude.get(first).copied().unwrap_or(0.0);
    let angle = source.v_angle.get(first).copied().unwrap_or(0.0);
    hot.windows(2).any(|pair| {
        terminal_phase(&source.terminal_map[pair[0]])
            >= terminal_phase(&source.terminal_map[pair[1]])
    }) || hot.iter().enumerate().any(|(i, &k)| {
        let expected = angle - std::f64::consts::TAU * i as f64 / hot.len() as f64;
        let deviation = source.v_angle.get(k).map_or(0.0, |a| {
            (a - expected + std::f64::consts::PI).rem_euclid(std::f64::consts::TAU)
                - std::f64::consts::PI
        });
        source
            .v_magnitude
            .get(k)
            .is_some_and(|v| (v - magnitude).abs() > 1e-9 * magnitude.abs().max(1.0))
            || deviation.abs() > 1e-9
    })
}

fn invalid(source: &VoltageSource, message: &str) -> powerio_core::Error {
    powerio_core::Error::new(
        &C::BUILD_DIST_ELECTRICAL_INCOMPLETE,
        format!(
            "vsource {}: cannot split the source while preserving its Thevenin impedance: {message}",
            source.name
        ),
    )
}

#[allow(clippy::float_cmp)] // The symmetric RAVENS matrix must preserve both stated triangles exactly.
fn impedance(source: &VoltageSource) -> Result<Option<Impedance>, powerio_core::Error> {
    let hot: Vec<_> = source
        .terminal_map
        .iter()
        .filter(|t| terminal_phase(t).is_some_and(|p| p != "N"))
        .cloned()
        .collect();
    let read = |key: &str| -> Result<Option<ConductorMatrix>, powerio_core::Error> {
        let Some(value) = source.extras.get(key) else {
            return Ok(None);
        };
        let Some(rows) = value.as_array() else {
            return Err(invalid(source, "rs/xs must be square numeric matrices"));
        };
        let n = rows.len();
        if n == 0 || (n != hot.len() && n != source.terminal_map.len()) {
            return Err(invalid(
                source,
                "rs/xs dimensions must match the source conductors",
            ));
        }
        let matrix: Option<ConductorMatrix> = rows
            .iter()
            .map(|row| {
                let row = row.as_array()?;
                if row.len() != n {
                    return None;
                }
                row.iter()
                    .map(|v| v.as_f64().filter(|v| v.is_finite()))
                    .collect()
            })
            .collect();
        matrix
            .map(Some)
            .ok_or_else(|| invalid(source, "rs/xs must be square finite numeric matrices"))
    };
    let (terminals, rs, xs) =
        if source.extras.contains_key("rs") || source.extras.contains_key("xs") {
            let (rs, xs) = (read("rs")?, read("xs")?);
            let n = rs.as_ref().or(xs.as_ref()).map_or(0, Vec::len);
            let zero = || vec![vec![0.0; n]; n];
            let (rs, xs) = (rs.unwrap_or_else(zero), xs.unwrap_or_else(zero));
            if rs.len() != xs.len() {
                return Err(invalid(source, "rs and xs dimensions differ"));
            }
            let terminals = if n == source.terminal_map.len() {
                source.terminal_map.clone()
            } else {
                hot
            };
            (terminals, rs, xs)
        } else {
            let number = |key: &str, default: f64| -> Result<f64, powerio_core::Error> {
                source.extras.get(key).map_or(Ok(default), |v| {
                    v.as_f64().filter(|v| v.is_finite()).ok_or_else(|| {
                        invalid(source, "sequence impedances must be finite numbers")
                    })
                })
            };
            let (r1, x1) = (number("r1", 0.0)?, number("x1", 0.0)?);
            let (r0, x0) = (number("r0", r1)?, number("x0", x1)?);
            let matrix = |positive: f64, zero: f64| -> ConductorMatrix {
                (0..hot.len())
                    .map(|i| {
                        (0..hot.len())
                            .map(|j| {
                                if i == j {
                                    (zero + 2.0 * positive) / 3.0
                                } else {
                                    (zero - positive) / 3.0
                                }
                            })
                            .collect()
                    })
                    .collect()
            };
            let (rs, xs) = (matrix(r1, r0), matrix(x1, x0));
            (hot, rs, xs)
        };
    for matrix in [&rs, &xs] {
        for (i, row) in matrix.iter().enumerate() {
            for (j, &value) in row.iter().enumerate() {
                if !value.is_finite() || value != matrix[j][i] {
                    return Err(invalid(
                        source,
                        "the RAVENS line matrix requires finite symmetric impedance",
                    ));
                }
            }
        }
    }
    Ok(rs
        .iter()
        .chain(&xs)
        .flatten()
        .any(|v| *v != 0.0)
        .then_some((terminals, rs, xs)))
}

fn unique(stem: &str, occupied: &mut BTreeSet<String>) -> String {
    let mut name = stem.to_owned();
    let mut index = 1;
    while !occupied.insert(name.to_ascii_lowercase()) {
        name = format!("{stem}_{index}");
        index += 1;
    }
    name
}

pub(super) fn prepare(
    net: &MulticonductorNetwork,
) -> Result<Cow<'_, MulticonductorNetwork>, powerio_core::Error> {
    let mut prepared = Cow::Borrowed(net);
    let mut buses: BTreeSet<_> = net
        .buses()
        .iter()
        .map(|b| b.id.to_ascii_lowercase())
        .collect();
    let mut lines: BTreeSet<_> = net
        .lines()
        .iter()
        .map(|l| l.name.to_ascii_lowercase())
        .chain(net.line_codes().iter().map(|c| c.name.to_ascii_lowercase()))
        .collect();
    for (index, original) in net.sources().iter().enumerate() {
        // The writer already diagnoses unsupported/repeated phase maps.
        // Do not allocate an impedance matrix for a record it cannot emit.
        let phases: Option<BTreeSet<_>> = original
            .terminal_map
            .iter()
            .map(|t| terminal_phase(t))
            .collect();
        if phases.is_none_or(|p| p.len() != original.terminal_map.len()) {
            continue;
        }
        if !is_unbalanced(original) {
            continue;
        }
        let Some((terminals, rs, xs)) = impedance(original)? else {
            continue;
        };
        let emf = unique(&format!("_pio_ravens_{}_emf", original.name), &mut buses);
        let name = unique(
            &format!("_pio_ravens_{}_impedance", original.name),
            &mut lines,
        );
        let network = prepared.to_mut();
        network
            .buses_mut()
            .push(DistBus::new(&emf, original.terminal_map.clone()));
        network
            .line_codes_mut()
            .push(DistLineCode::new(&name, rs, xs));
        network.lines_mut().push(DistLine::new(
            &name,
            &emf,
            &original.bus,
            terminals.clone(),
            terminals,
            &name,
            1.0,
        ));
        let source = &mut network.sources_mut()[index];
        source.bus = emf;
        // The explicit line now carries the complete matrix. No split part
        // may apply the impedance again or turn other phases into ideal ports.
        for key in ["rs", "xs", "r1", "x1", "r0", "x0"] {
            source.extras.remove(key);
        }
    }
    Ok(prepared)
}
