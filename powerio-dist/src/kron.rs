//! Explicit-neutral Kron projection for [`MulticonductorNetwork`].
//!
//! This is a same-family network transformation, not an OPF formulation. It
//! removes one identified neutral terminal per bus, applies the exact Schur
//! complement to each referenced primitive series-impedance matrix, and
//! rewrites terminal maps and conductor-indexed data onto the retained
//! conductors. The input handle is never mutated.

use std::collections::{BTreeMap, BTreeSet};

use num_complex::Complex64;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::diagnostics::codes;
use crate::{ConductorMatrix, Diagnostic, Error, MulticonductorNetwork, Result};

const SINGULAR_TOLERANCE: f64 = 1e-12;

/// Options controlling explicit-neutral identification and grounding.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct NeutralKronOptions {
    /// Per-bus neutral terminal overrides. Bus identity is case insensitive;
    /// terminal identity remains case sensitive.
    pub neutral_terminals: BTreeMap<String, String>,
    /// Permit an identified neutral that is not listed in the bus's explicit
    /// grounded set to be treated as ideal ground. Off by default because this
    /// changes the physical network.
    pub allow_forced_ideal_ground: bool,
}

impl NeutralKronOptions {
    /// Override the neutral label for one bus.
    #[must_use]
    pub fn with_neutral_terminal(
        mut self,
        bus: impl Into<String>,
        terminal: impl Into<String>,
    ) -> Self {
        self.neutral_terminals.insert(bus.into(), terminal.into());
        self
    }

    /// Opt into treating an identified floating neutral as ideal ground.
    #[must_use]
    pub const fn with_forced_ideal_ground(mut self, allow: bool) -> Self {
        self.allow_forced_ideal_ground = allow;
        self
    }
}

/// How the eliminated neutral was grounded in the source network.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum NeutralKronGrounding {
    Perfect,
    ForcedIdeal,
}

/// One eliminated bus terminal.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct NeutralKronBus {
    pub bus: String,
    pub neutral_terminal: String,
    /// Position in the bus's terminal order before reduction.
    pub source_position: usize,
    pub grounding: NeutralKronGrounding,
}

/// Neutral-current recovery for one reduced linecode.
///
/// Zero neutral voltage drop gives `i_n = K i_p` for retained conductor
/// currents `i_p`, where `K = -Z_nn^-1 Z_np`. Both neutral endpoints remain
/// at ideal ground; this matrix does not recover a neutral voltage.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct NeutralKronRecovery {
    pub linecode: String,
    pub source_linecode: String,
    pub eliminated_position: usize,
    pub retained_positions: Vec<usize>,
    pub k_re: Vec<f64>,
    pub k_im: Vec<f64>,
}

/// One auditable non-matrix rewrite performed by the projection.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct NeutralKronAction {
    pub component: String,
    pub action: String,
    pub details: BTreeMap<String, Value>,
}

/// Complete audit of one neutral-Kron projection.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct NeutralKronReport {
    pub buses: Vec<NeutralKronBus>,
    pub recoveries: Vec<NeutralKronRecovery>,
    pub actions: Vec<NeutralKronAction>,
    pub diagnostics: Vec<Diagnostic>,
}

/// The independently owned reduced network and its projection audit.
#[derive(Clone, Debug)]
pub struct NeutralKronReduction {
    network: MulticonductorNetwork,
    report: NeutralKronReport,
}

impl NeutralKronReduction {
    #[must_use]
    pub fn network(&self) -> &MulticonductorNetwork {
        &self.network
    }

    #[must_use]
    pub const fn report(&self) -> &NeutralKronReport {
        &self.report
    }

    #[must_use]
    pub fn into_parts(self) -> (MulticonductorNetwork, NeutralKronReport) {
        (self.network, self.report)
    }
}

fn fail(message: impl Into<String>) -> Error {
    Error::KronReduction {
        message: message.into(),
    }
}

fn action(component: impl Into<String>, action: impl Into<String>) -> NeutralKronAction {
    NeutralKronAction {
        component: component.into(),
        action: action.into(),
        details: BTreeMap::new(),
    }
}

fn conventions(network: &MulticonductorNetwork) -> Option<&Value> {
    network.extras().get("bmopf_terminal_conventions")
}

fn override_for_bus<'a>(options: &'a NeutralKronOptions, bus: &str) -> Option<&'a str> {
    options
        .neutral_terminals
        .iter()
        .find(|(id, _)| id.eq_ignore_ascii_case(bus))
        .map(|(_, terminal)| terminal.as_str())
}

fn identify_neutrals(
    network: &MulticonductorNetwork,
    options: &NeutralKronOptions,
) -> Result<Vec<NeutralKronBus>> {
    let roles = conventions(network);
    let mut buses = Vec::new();
    let mut matched_overrides = BTreeSet::new();
    for bus in network.buses() {
        let explicit = override_for_bus(options, &bus.id);
        if explicit.is_some() {
            matched_overrides.insert(bus.id.to_ascii_lowercase());
        }
        let candidates = if let Some(terminal) = explicit {
            vec![terminal.to_owned()]
        } else {
            let phase_positions: BTreeSet<usize> = bus.phase_indices(roles).into_iter().collect();
            bus.terminals
                .iter()
                .enumerate()
                .filter_map(|(position, terminal)| {
                    (!phase_positions.contains(&position)).then_some(terminal.clone())
                })
                .collect::<Vec<_>>()
        };
        if candidates.is_empty() {
            continue;
        }
        if candidates.len() != 1 {
            return Err(fail(format!(
                "bus `{}` has multiple candidate neutral terminals {:?}",
                bus.id, candidates
            )));
        }
        let terminal = &candidates[0];
        let positions = bus
            .terminals
            .iter()
            .enumerate()
            .filter_map(|(position, value)| (value == terminal).then_some(position))
            .collect::<Vec<_>>();
        if positions.len() != 1 {
            return Err(fail(format!(
                "bus `{}` neutral override `{terminal}` does not name exactly one declared terminal",
                bus.id
            )));
        }
        let grounded = bus.grounded.iter().any(|value| value == terminal);
        let grounding = if grounded {
            NeutralKronGrounding::Perfect
        } else if options.allow_forced_ideal_ground {
            NeutralKronGrounding::ForcedIdeal
        } else {
            return Err(fail(format!(
                "bus `{}` neutral terminal `{terminal}` is not perfectly grounded; set allow_forced_ideal_ground only when idealizing it is intended",
                bus.id
            )));
        };
        buses.push(NeutralKronBus {
            bus: bus.id.clone(),
            neutral_terminal: terminal.clone(),
            source_position: positions[0],
            grounding,
        });
    }

    for key in options.neutral_terminals.keys() {
        if !matched_overrides.contains(&key.to_ascii_lowercase()) {
            return Err(fail(format!("neutral override names unknown bus `{key}`")));
        }
    }
    Ok(buses)
}

fn neutral_for_bus<'a>(buses: &'a [NeutralKronBus], bus: &str) -> Option<&'a NeutralKronBus> {
    buses
        .iter()
        .find(|entry| entry.bus.eq_ignore_ascii_case(bus))
}

fn terminal_position(map: &[String], terminal: &str, label: &str) -> Result<Option<usize>> {
    let positions = map
        .iter()
        .enumerate()
        .filter_map(|(position, value)| (value == terminal).then_some(position))
        .collect::<Vec<_>>();
    if positions.len() > 1 {
        return Err(fail(format!(
            "{label} repeats neutral terminal `{terminal}`"
        )));
    }
    Ok(positions.first().copied())
}

fn validate_square(matrix: &ConductorMatrix, n: usize, label: &str) -> Result<()> {
    if matrix.len() != n || matrix.iter().any(|row| row.len() != n) {
        return Err(fail(format!(
            "{label} is not a {n} by {n} conductor matrix"
        )));
    }
    if matrix.iter().flatten().any(|value| !value.is_finite()) {
        return Err(fail(format!("{label} contains a non-finite value")));
    }
    Ok(())
}

fn retained_positions(n: usize, neutral: usize) -> Vec<usize> {
    (0..n).filter(|position| *position != neutral).collect()
}

fn submatrix(matrix: &ConductorMatrix, keep: &[usize]) -> ConductorMatrix {
    keep.iter()
        .map(|&row| keep.iter().map(|&column| matrix[row][column]).collect())
        .collect()
}

fn reduce_series(
    r: &ConductorMatrix,
    x: &ConductorMatrix,
    neutral: usize,
    label: &str,
) -> Result<(ConductorMatrix, ConductorMatrix, Vec<usize>, Vec<Complex64>)> {
    let n = r.len();
    validate_square(r, n, &format!("{label} resistance"))?;
    validate_square(x, n, &format!("{label} reactance"))?;
    if neutral >= n {
        return Err(fail(format!(
            "{label} neutral position {neutral} lies outside its {n} conductors"
        )));
    }
    let z = |row: usize, column: usize| Complex64::new(r[row][column], x[row][column]);
    let scale = r
        .iter()
        .flatten()
        .zip(x.iter().flatten())
        .map(|(&re, &im)| Complex64::new(re, im).norm())
        .fold(1.0_f64, f64::max);
    let z_nn = z(neutral, neutral);
    if !z_nn.norm().is_finite() || z_nn.norm() <= SINGULAR_TOLERANCE * scale {
        return Err(fail(format!(
            "{label} neutral self impedance is singular or near-singular"
        )));
    }
    let keep = retained_positions(n, neutral);
    if keep.is_empty() {
        return Err(fail(format!(
            "{label} has no retained conductor after eliminating its neutral"
        )));
    }
    let recovery = keep
        .iter()
        .map(|&column| -z(neutral, column) / z_nn)
        .collect::<Vec<_>>();
    let mut r_reduced = vec![vec![0.0; keep.len()]; keep.len()];
    let mut x_reduced = vec![vec![0.0; keep.len()]; keep.len()];
    for (out_row, &row) in keep.iter().enumerate() {
        for (out_column, &column) in keep.iter().enumerate() {
            let value = z(row, column) - z(row, neutral) * z(neutral, column) / z_nn;
            if !value.re.is_finite() || !value.im.is_finite() {
                return Err(fail(format!(
                    "{label} produced a non-finite reduced impedance"
                )));
            }
            r_reduced[out_row][out_column] = value.re;
            x_reduced[out_row][out_column] = value.im;
        }
    }
    Ok((r_reduced, x_reduced, keep, recovery))
}

fn slice_if_conductor_aligned(values: &mut Option<Vec<f64>>, old_n: usize, neutral: usize) {
    if values.as_ref().is_some_and(|values| values.len() == old_n) {
        values.as_mut().expect("checked Some").remove(neutral);
    }
}

fn reduce_current_limit(
    values: &mut Option<Vec<f64>>,
    old_n: usize,
    neutral: usize,
    label: &str,
) -> Result<()> {
    if values
        .as_ref()
        .is_some_and(|values| values.len() == old_n && values[neutral] != f64::INFINITY)
    {
        return Err(fail(format!(
            "{label} has a neutral current limit that the reduced network cannot represent"
        )));
    }
    slice_if_conductor_aligned(values, old_n, neutral);
    Ok(())
}

fn reduce_phase_bound(
    values: &mut Option<Vec<f64>>,
    terminal_count: usize,
    neutral: usize,
    label: &str,
) -> Result<()> {
    let Some(values) = values else {
        return Ok(());
    };
    if values.len() == terminal_count {
        values.remove(neutral);
    } else if values.len() != terminal_count.saturating_sub(1) {
        return Err(fail(format!(
            "{label} has {} entries; expected {} phase entries or {terminal_count} terminal entries",
            values.len(),
            terminal_count.saturating_sub(1)
        )));
    }
    Ok(())
}

fn strip_map(map: &mut Vec<String>, neutral: &NeutralKronBus, label: &str) -> Result<bool> {
    let Some(position) = terminal_position(map, &neutral.neutral_terminal, label)? else {
        return Ok(false);
    };
    map.remove(position);
    if map.is_empty() {
        return Err(fail(format!(
            "{label} has no retained terminal after neutral elimination"
        )));
    }
    Ok(true)
}

fn merge_phase_neutral_bounds(bus: &mut crate::DistBus) -> Result<()> {
    let phase_count = bus.terminals.len().saturating_sub(1);
    if let Some(vpn_min) = bus.vpn_min.take() {
        if vpn_min.len() != phase_count {
            return Err(fail(format!(
                "bus `{}` vpn_min has {} entries; expected {phase_count}",
                bus.id,
                vpn_min.len()
            )));
        }
        let existing = bus
            .v_min_phase
            .take()
            .unwrap_or_else(|| vec![bus.v_min.unwrap_or(f64::NEG_INFINITY); phase_count]);
        if existing.len() != phase_count {
            return Err(fail(format!(
                "bus `{}` phase minimum bound has {} entries; expected {phase_count}",
                bus.id,
                existing.len()
            )));
        }
        bus.v_min = None;
        bus.v_min_phase = Some(
            existing
                .into_iter()
                .zip(vpn_min)
                .map(|(ground, neutral)| ground.max(neutral))
                .collect(),
        );
    }
    if let Some(vpn_max) = bus.vpn_max.take() {
        if vpn_max.len() != phase_count {
            return Err(fail(format!(
                "bus `{}` vpn_max has {} entries; expected {phase_count}",
                bus.id,
                vpn_max.len()
            )));
        }
        let existing = bus
            .v_max_phase
            .take()
            .unwrap_or_else(|| vec![bus.v_max.unwrap_or(f64::INFINITY); phase_count]);
        if existing.len() != phase_count {
            return Err(fail(format!(
                "bus `{}` phase maximum bound has {} entries; expected {phase_count}",
                bus.id,
                existing.len()
            )));
        }
        bus.v_max = None;
        bus.v_max_phase = Some(
            existing
                .into_iter()
                .zip(vpn_max)
                .map(|(ground, neutral)| ground.min(neutral))
                .collect(),
        );
    }
    if let (Some(minimum), Some(maximum)) = (&bus.v_min_phase, &bus.v_max_phase)
        && minimum
            .iter()
            .zip(maximum)
            .any(|(minimum, maximum)| minimum > maximum)
    {
        return Err(fail(format!(
            "bus `{}` has contradictory voltage bounds after neutral elimination",
            bus.id
        )));
    }
    bus.vn_max = None;
    Ok(())
}

fn unique_linecode_name(network: &MulticonductorNetwork, base: &str) -> String {
    let mut candidate = base.to_owned();
    let mut suffix = 2usize;
    while network
        .line_codes()
        .iter()
        .any(|code| code.name.eq_ignore_ascii_case(&candidate))
    {
        candidate = format!("{base}_{suffix}");
        suffix += 1;
    }
    candidate
}

#[derive(Clone, Copy)]
struct LineUse {
    line: usize,
    code: usize,
    neutral: Option<usize>,
}

fn collect_line_uses(
    network: &MulticonductorNetwork,
    buses: &[NeutralKronBus],
) -> Result<Vec<LineUse>> {
    let mut uses = Vec::with_capacity(network.lines().len());
    for (line_index, line) in network.lines().iter().enumerate() {
        let code = network
            .line_codes()
            .iter()
            .position(|code| code.name.eq_ignore_ascii_case(&line.linecode))
            .ok_or_else(|| {
                fail(format!(
                    "line `{}` references unknown linecode `{}`",
                    line.name, line.linecode
                ))
            })?;
        let from = neutral_for_bus(buses, &line.bus_from)
            .map(|entry| {
                terminal_position(
                    &line.terminal_map_from,
                    &entry.neutral_terminal,
                    &format!("line `{}` from map", line.name),
                )
            })
            .transpose()?
            .flatten();
        let to = neutral_for_bus(buses, &line.bus_to)
            .map(|entry| {
                terminal_position(
                    &line.terminal_map_to,
                    &entry.neutral_terminal,
                    &format!("line `{}` to map", line.name),
                )
            })
            .transpose()?
            .flatten();
        let neutral = match (from, to) {
            (None, None) => None,
            (Some(from), Some(to)) if from == to => Some(from),
            (Some(from), Some(to)) => {
                return Err(fail(format!(
                    "line `{}` maps its neutral at different conductor positions {from} and {to}",
                    line.name
                )));
            }
            _ => {
                return Err(fail(format!(
                    "line `{}` carries a neutral at only one endpoint",
                    line.name
                )));
            }
        };
        uses.push(LineUse {
            line: line_index,
            code,
            neutral,
        });
    }
    Ok(uses)
}

fn reduce_linecodes(
    network: &mut MulticonductorNetwork,
    uses: &[LineUse],
    report: &mut NeutralKronReport,
) -> Result<()> {
    let mut by_code: BTreeMap<usize, BTreeMap<Option<usize>, Vec<usize>>> = BTreeMap::new();
    for usage in uses {
        by_code
            .entry(usage.code)
            .or_default()
            .entry(usage.neutral)
            .or_default()
            .push(usage.line);
    }

    for (code_index, groups) in by_code {
        let source = network.line_codes()[code_index].clone();
        let reduced_groups = groups
            .iter()
            .filter_map(|(position, lines)| position.map(|position| (position, lines)))
            .collect::<Vec<_>>();
        if reduced_groups.is_empty() {
            continue;
        }
        let simple = reduced_groups.len() == 1 && !groups.contains_key(&None);
        for (position, lines) in reduced_groups {
            if position >= source.n_conductors {
                return Err(fail(format!(
                    "linecode `{}` has {} conductors but a line maps its neutral at position {position}",
                    source.name, source.n_conductors
                )));
            }
            let target_name = if simple {
                source.name.clone()
            } else {
                unique_linecode_name(network, &format!("{}__kron_{}", source.name, position + 1))
            };
            let (r_series, x_series, keep, recovery) = reduce_series(
                &source.r_series,
                &source.x_series,
                position,
                &format!("linecode `{}`", source.name),
            )?;
            for (matrix, label) in [
                (&source.g_from, "from conductance"),
                (&source.b_from, "from susceptance"),
                (&source.g_to, "to conductance"),
                (&source.b_to, "to susceptance"),
            ] {
                validate_square(
                    matrix,
                    source.n_conductors,
                    &format!("linecode `{}` {label}", source.name),
                )?;
            }
            let mut reduced = source.clone();
            reduced.name.clone_from(&target_name);
            reduced.n_conductors = keep.len();
            reduced.r_series = r_series;
            reduced.x_series = x_series;
            reduced.g_from = submatrix(&source.g_from, &keep);
            reduced.b_from = submatrix(&source.b_from, &keep);
            reduced.g_to = submatrix(&source.g_to, &keep);
            reduced.b_to = submatrix(&source.b_to, &keep);
            reduce_current_limit(
                &mut reduced.i_max,
                source.n_conductors,
                position,
                &format!("linecode `{}`", source.name),
            )?;
            slice_if_conductor_aligned(&mut reduced.s_max, source.n_conductors, position);
            reduced.source = Some("kron_reduction".to_owned());

            if simple {
                network.line_codes_mut()[code_index] = reduced;
            } else {
                network.line_codes_mut().push(reduced);
                for &line in lines {
                    network.lines_mut()[line].linecode.clone_from(&target_name);
                }
            }
            report.recoveries.push(NeutralKronRecovery {
                linecode: target_name,
                source_linecode: source.name.clone(),
                eliminated_position: position,
                retained_positions: keep,
                k_re: recovery.iter().map(|value| value.re).collect(),
                k_im: recovery.iter().map(|value| value.im).collect(),
            });
        }
    }
    Ok(())
}

fn reduce_buses(network: &mut MulticonductorNetwork, report: &mut NeutralKronReport) -> Result<()> {
    for bus_reduction in &report.buses {
        let bus = network
            .buses_mut()
            .iter_mut()
            .find(|bus| bus.id.eq_ignore_ascii_case(&bus_reduction.bus))
            .expect("identified bus remains present");
        let terminal_count = bus.terminals.len();
        reduce_phase_bound(
            &mut bus.v_min_phase,
            terminal_count,
            bus_reduction.source_position,
            &format!("bus `{}` phase minimum bound", bus.id),
        )?;
        reduce_phase_bound(
            &mut bus.v_max_phase,
            terminal_count,
            bus_reduction.source_position,
            &format!("bus `{}` phase maximum bound", bus.id),
        )?;
        merge_phase_neutral_bounds(bus)?;
        bus.terminals.remove(bus_reduction.source_position);
        bus.grounded
            .retain(|terminal| terminal != &bus_reduction.neutral_terminal);
        let mut record = action(format!("bus/{}", bus.id), "removed_neutral_terminal");
        record
            .details
            .insert("terminal".to_owned(), json!(bus_reduction.neutral_terminal));
        report.actions.push(record);
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
fn reduce_terminal_maps(
    network: &mut MulticonductorNetwork,
    report: &mut NeutralKronReport,
) -> Result<()> {
    let buses = report.buses.clone();
    for line in network.lines_mut() {
        if let Some(neutral) = neutral_for_bus(&buses, &line.bus_from) {
            let old_n = line.terminal_map_from.len();
            if let Some(position) = terminal_position(
                &line.terminal_map_from,
                &neutral.neutral_terminal,
                &format!("line `{}` from map", line.name),
            )? {
                line.terminal_map_from.remove(position);
                reduce_current_limit(
                    &mut line.i_max,
                    old_n,
                    position,
                    &format!("line `{}`", line.name),
                )?;
                slice_if_conductor_aligned(&mut line.s_max, old_n, position);
            }
        }
        if let Some(neutral) = neutral_for_bus(&buses, &line.bus_to) {
            let position = terminal_position(
                &line.terminal_map_to,
                &neutral.neutral_terminal,
                &format!("line `{}` to map", line.name),
            )?;
            if let Some(position) = position {
                line.terminal_map_to.remove(position);
            }
        }
        if line.terminal_map_from.is_empty() || line.terminal_map_to.is_empty() {
            return Err(fail(format!(
                "line `{}` has no retained conductor after neutral elimination",
                line.name
            )));
        }
    }

    for load in network.loads_mut() {
        if let Some(neutral) = neutral_for_bus(&buses, &load.bus) {
            strip_map(
                &mut load.terminal_map,
                neutral,
                &format!("load `{}` terminal map", load.name),
            )?;
        }
    }
    for generator in network.generators_mut() {
        if let Some(neutral) = neutral_for_bus(&buses, &generator.bus) {
            let old_n = generator.terminal_map.len();
            if let Some(position) = terminal_position(
                &generator.terminal_map,
                &neutral.neutral_terminal,
                &format!("generator `{}` terminal map", generator.name),
            )? {
                generator.terminal_map.remove(position);
                reduce_current_limit(
                    &mut generator.i_max,
                    old_n,
                    position,
                    &format!("generator `{}`", generator.name),
                )?;
                slice_if_conductor_aligned(&mut generator.s_max, old_n, position);
            }
            if generator.terminal_map.is_empty() {
                return Err(fail(format!(
                    "generator `{}` has no retained terminal after neutral elimination",
                    generator.name
                )));
            }
        }
    }
    for source in network.sources_mut() {
        if let Some(neutral) = neutral_for_bus(&buses, &source.bus) {
            let old_n = source.terminal_map.len();
            if let Some(position) = terminal_position(
                &source.terminal_map,
                &neutral.neutral_terminal,
                &format!("voltage source `{}` terminal map", source.name),
            )? {
                source.terminal_map.remove(position);
                if source.v_magnitude.len() == old_n {
                    source.v_magnitude.remove(position);
                }
                if source.v_angle.len() == old_n {
                    source.v_angle.remove(position);
                }
                if source
                    .energy_cost_rate
                    .as_ref()
                    .is_some_and(|values| values.len() == old_n)
                {
                    source
                        .energy_cost_rate
                        .as_mut()
                        .expect("checked Some")
                        .remove(position);
                }
            }
            if source.terminal_map.is_empty() {
                return Err(fail(format!(
                    "voltage source `{}` has no retained terminal after neutral elimination",
                    source.name
                )));
            }
        }
    }

    for ibr in network.ibrs() {
        if neutral_for_bus(&buses, &ibr.bus).is_some_and(|neutral| {
            ibr.terminal_map
                .iter()
                .any(|terminal| terminal == &neutral.neutral_terminal)
        }) {
            return Err(fail(format!(
                "IBR `{}` contains an active neutral leg; this projection cannot preserve neutral-current physics",
                ibr.name
            )));
        }
    }
    for capacitor in network.capacitors() {
        if neutral_for_bus(&buses, &capacitor.bus).is_some_and(|neutral| {
            capacitor
                .terminal_map
                .iter()
                .any(|terminal| terminal == &neutral.neutral_terminal)
        }) {
            return Err(fail(format!(
                "capacitor `{}` contains an explicit neutral; convert it to a typed shunt before neutral reduction",
                capacitor.name
            )));
        }
    }

    let mut retained_shunts = Vec::with_capacity(network.shunts().len());
    for mut shunt in std::mem::take(network.shunts_mut()) {
        if let Some(neutral) = neutral_for_bus(&buses, &shunt.bus)
            && let Some(position) = terminal_position(
                &shunt.terminal_map,
                &neutral.neutral_terminal,
                &format!("shunt `{}` terminal map", shunt.name),
            )?
        {
            if shunt.terminal_map.len() == 1 {
                report.actions.push(action(
                    format!("shunt/{}", shunt.name),
                    "removed_neutral_only_shunt",
                ));
                continue;
            }
            let old_n = shunt.terminal_map.len();
            validate_square(
                &shunt.g,
                old_n,
                &format!("shunt `{}` conductance", shunt.name),
            )?;
            validate_square(
                &shunt.b,
                old_n,
                &format!("shunt `{}` susceptance", shunt.name),
            )?;
            let keep = retained_positions(old_n, position);
            shunt.terminal_map.remove(position);
            shunt.g = submatrix(&shunt.g, &keep);
            shunt.b = submatrix(&shunt.b, &keep);
        }
        retained_shunts.push(shunt);
    }
    *network.shunts_mut() = retained_shunts;

    let mut retained_switches = Vec::with_capacity(network.switches().len());
    for mut switch in std::mem::take(network.switches_mut()) {
        let from = neutral_for_bus(&buses, &switch.bus_from)
            .map(|neutral| {
                terminal_position(
                    &switch.terminal_map_from,
                    &neutral.neutral_terminal,
                    &format!("switch `{}` from map", switch.name),
                )
            })
            .transpose()?
            .flatten();
        let to = neutral_for_bus(&buses, &switch.bus_to)
            .map(|neutral| {
                terminal_position(
                    &switch.terminal_map_to,
                    &neutral.neutral_terminal,
                    &format!("switch `{}` to map", switch.name),
                )
            })
            .transpose()?
            .flatten();
        match (from, to) {
            (Some(from), Some(to)) => {
                if switch.terminal_map_from.len() == 1 && switch.terminal_map_to.len() == 1 {
                    report.actions.push(action(
                        format!("switch/{}", switch.name),
                        "removed_neutral_only_switch",
                    ));
                    continue;
                }
                let old_n = switch.terminal_map_from.len();
                switch.terminal_map_from.remove(from);
                switch.terminal_map_to.remove(to);
                reduce_current_limit(
                    &mut switch.i_max,
                    old_n,
                    from,
                    &format!("switch `{}`", switch.name),
                )?;
            }
            (None, None) => {}
            _ => {
                return Err(fail(format!(
                    "switch `{}` carries a neutral at only one endpoint",
                    switch.name
                )));
            }
        }
        retained_switches.push(switch);
    }
    *network.switches_mut() = retained_switches;

    for transformer in network.transformers_mut() {
        for (winding_index, winding) in transformer.windings.iter_mut().enumerate() {
            let removed = if let Some(neutral) = neutral_for_bus(&buses, &winding.bus) {
                strip_map(
                    &mut winding.terminal_map,
                    neutral,
                    &format!(
                        "transformer `{}` winding {} terminal map",
                        transformer.name,
                        winding_index + 1
                    ),
                )?
            } else {
                false
            };
            if removed {
                let had_r = winding.r_neutral.take().is_some();
                let had_x = winding.x_neutral.take().is_some();
                if !(had_r || had_x) {
                    continue;
                }
                report.actions.push(action(
                    format!(
                        "transformer/{}/winding/{}",
                        transformer.name,
                        winding_index + 1
                    ),
                    "removed_neutral_grounding_impedance",
                ));
            }
        }
    }
    Ok(())
}

fn update_terminal_conventions(network: &mut MulticonductorNetwork, buses: &[NeutralKronBus]) {
    let removed = buses
        .iter()
        .map(|entry| entry.neutral_terminal.as_str())
        .collect::<BTreeSet<_>>();
    let Some(Value::Object(roles)) = network.extras_mut().get_mut("bmopf_terminal_conventions")
    else {
        return;
    };
    if let Some(Value::Array(neutrals)) = roles.get_mut("neutral") {
        neutrals.retain(|value| {
            value
                .as_str()
                .is_none_or(|terminal| !removed.contains(terminal))
        });
    }
}

/// Produce a neutral-reduced multiconductor network without mutating the
/// input. Neutral roles come from `options.neutral_terminals`, retained BMOPF
/// terminal conventions, or the model's conservative `n` / four-wire `4`
/// convention. Only perfectly grounded neutrals are accepted unless the
/// explicit forced-ground option is enabled.
///
/// # Errors
/// Ambiguous or floating neutral roles, inconsistent conductor maps,
/// singular primitive impedance, or typed neutral-current physics the
/// projection cannot preserve.
pub fn neutral_kron_reduce(
    network: &MulticonductorNetwork,
    options: &NeutralKronOptions,
) -> Result<NeutralKronReduction> {
    crate::readiness::require_earth_referenced_sources(network)
        .map_err(|error| fail(error.to_string()))?;
    let buses = identify_neutrals(network, options)?;
    let mut report = NeutralKronReport {
        buses,
        ..NeutralKronReport::default()
    };
    if report.buses.is_empty() {
        return Ok(NeutralKronReduction {
            network: network.clone(),
            report,
        });
    }

    for (index, bus) in report.buses.iter().enumerate() {
        let code = match bus.grounding {
            NeutralKronGrounding::Perfect => &codes::TRANSFORM_DIST_NEUTRAL_KRON_REDUCED,
            NeutralKronGrounding::ForcedIdeal => &codes::TRANSFORM_DIST_NEUTRAL_KRON_FORCED_GROUND,
        };
        let mut diagnostic = Diagnostic::of(
            code,
            format!(
                "bus `{}` neutral terminal `{}` was eliminated",
                bus.bus, bus.neutral_terminal
            ),
        );
        crate::diagnostics::attach_target(&mut diagnostic, format!("/buses/{index}/terminals"));
        let _ = diagnostic.insert_detail("neutral_terminal", json!(bus.neutral_terminal));
        report.diagnostics.push(diagnostic);
    }
    if !network.untyped_objects().is_empty() {
        report.diagnostics.push(Diagnostic::of(
            &codes::TRANSFORM_DIST_KRON_UNTYPED_RETAINED,
            format!(
                "{} untyped source object(s) were retained unchanged",
                network.untyped_objects().len()
            ),
        ));
    }

    let uses = collect_line_uses(network, &report.buses)?;
    let mut reduced = network.clone();
    reduce_linecodes(&mut reduced, &uses, &mut report)?;
    reduce_terminal_maps(&mut reduced, &mut report)?;
    reduce_buses(&mut reduced, &mut report)?;
    update_terminal_conventions(&mut reduced, &report.buses);
    reduced.extras_mut().insert(
        "powerio_neutral_kron".to_owned(),
        json!({
            "method": "explicit_neutral_schur_complement",
            "buses": &report.buses,
            "recoveries": &report.recoveries,
            "actions": &report.actions,
        }),
    );
    Ok(NeutralKronReduction {
        network: reduced,
        report,
    })
}
