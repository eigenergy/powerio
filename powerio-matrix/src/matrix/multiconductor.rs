//! Native multiconductor nodal admittance over [`MulticonductorNetwork`]
//! (#232): terminal and conductor indexed, in the network's actual units
//! (volts, amperes, siemens), with no implicit positive sequence
//! transformation anywhere.
//!
//! The voltage unknown rows are the ungrounded bus terminals: terminal `0`
//! and every explicitly grounded terminal are excluded, and buses joined by
//! an exact unity connection (a closed switch) merge into one electrical
//! node rather than receiving an arbitrary small impedance. Every axis
//! carries the stable [`DistNode`] mapping — bus identity plus terminal — so
//! mappings remain valid after source row reordering.
//!
//! The passive admittance carries lines (series and both shunt halves from
//! the linecode), shunts, and capacitor banks. Ideal equipment — two winding
//! transformer coupling and voltage sources — enters the augmented system as
//! exact constraint rows over the node voltages with their coupled ideal
//! currents. Ideal WYE windings retain explicit floating or grounded neutral
//! terminals. Transformer leakage, non-WYE connections, implicit neutral
//! impedances, core shunts and tap decisions return an unsupported-physics
//! error before assembly.

mod transformer;

use std::collections::{BTreeMap, BTreeSet};

use crate::diagnostics::Diagnostic;
use num_complex::Complex64;
use powerio_dist::{
    Configuration, DistCapacitor, DistLine, DistLineCode, DistShunt, MulticonductorNetwork,
};
use sprs::CsMat;

use crate::diagnostics::codes;
use crate::matrix::triplet::CooBuilder;
use crate::{Error, Result};

/// One voltage unknown: a bus terminal, by stable identity.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct DistNode {
    pub bus: String,
    pub terminal: String,
}

/// Where a bus terminal lands in the nodal system.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeRef {
    /// A voltage unknown at this dense row.
    Node(usize),
    /// Ground: terminal `0` or an explicitly grounded terminal.
    Ground,
}

/// The dense node indexing of one multiconductor network.
#[derive(Clone, Debug)]
pub struct MulticonductorNodeIndex {
    nodes: Vec<DistNode>,
    position: BTreeMap<(String, String), usize>,
    grounded: BTreeSet<(String, String)>,
}

impl MulticonductorNodeIndex {
    /// Build the index: bus table order, each bus's stated terminal order,
    /// with terminal `0` and explicitly grounded terminals excluded from the
    /// unknowns. Closed switches merge their paired terminals into one
    /// node — the first spelling encountered names the merged node.
    pub fn build(network: &MulticonductorNetwork) -> Result<Self> {
        // Components over provisional slots, one per ungrounded terminal.
        let mut provisional: Vec<(String, String)> = Vec::new();
        let mut slot: BTreeMap<(String, String), usize> = BTreeMap::new();
        let mut grounded: BTreeSet<(String, String)> = BTreeSet::new();
        for bus in network.buses() {
            for terminal in &bus.terminals {
                let key = (bus.id.clone(), terminal.clone());
                if terminal == "0" || bus.grounded.contains(terminal) {
                    grounded.insert(key);
                    continue;
                }
                slot.insert(key.clone(), provisional.len());
                provisional.push(key);
            }
        }

        // Every union runs before any group is marked grounded, so a
        // grounding is a property of the finished group and the index is
        // identical under any declaration order of closed switches.
        let mut joined_from: Vec<usize> = Vec::new();
        let mut joined_to: Vec<usize> = Vec::new();
        let mut ground_touched: Vec<usize> = Vec::new();
        for switch in network.switches().iter().filter(|switch| !switch.open) {
            for (from, to) in switch
                .terminal_map_from
                .iter()
                .zip(switch.terminal_map_to.iter())
            {
                let from_key = (switch.bus_from.clone(), from.clone());
                let to_key = (switch.bus_to.clone(), to.clone());
                match (slot.get(&from_key), slot.get(&to_key)) {
                    (Some(&a), Some(&b)) => {
                        joined_from.push(a);
                        joined_to.push(b);
                    }
                    // A closed switch onto ground grounds the other side's
                    // whole finished group.
                    (Some(&a), None) if grounded.contains(&to_key) => {
                        ground_touched.push(a);
                    }
                    (None, Some(&b)) if grounded.contains(&from_key) => {
                        ground_touched.push(b);
                    }
                    _ => {
                        return Err(Error::Mtx(format!(
                            "switch `{}` names a terminal its buses do not declare",
                            switch.name
                        )));
                    }
                }
            }
        }
        // Groups are labeled in order of their first slot, so the first
        // spelling in table order names a merged node; the one component
        // labeling in powerio-tx joins them.
        let labels =
            powerio_tx::calc_dense_island_labels(provisional.len(), &joined_from, &joined_to);
        let grounded_labels: BTreeSet<usize> = ground_touched
            .into_iter()
            .map(|slot| labels[slot])
            .collect();

        // Dense rows: one per surviving group, in provisional (table) order.
        let mut dense_of_label: BTreeMap<usize, usize> = BTreeMap::new();
        let mut nodes = Vec::new();
        let mut position = BTreeMap::new();
        for (index, &label) in labels.iter().enumerate() {
            // A group that touched ground through a switch is grounded whole.
            if grounded_labels.contains(&label) {
                grounded.insert(provisional[index].clone());
                continue;
            }
            let dense = *dense_of_label.entry(label).or_insert_with(|| {
                let (bus, terminal) = provisional[index].clone();
                nodes.push(DistNode { bus, terminal });
                nodes.len() - 1
            });
            position.insert(provisional[index].clone(), dense);
        }
        Ok(Self {
            nodes,
            position,
            grounded,
        })
    }

    /// The voltage unknowns, in dense row order.
    #[must_use]
    pub fn nodes(&self) -> &[DistNode] {
        &self.nodes
    }

    /// The number of voltage unknowns.
    #[must_use]
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Where one bus terminal lands: a dense unknown (closed switch groups
    /// share one), ground, or `None` for a terminal the network does not
    /// declare.
    #[must_use]
    pub fn resolve(&self, bus: &str, terminal: &str) -> Option<NodeRef> {
        let key = (bus.to_owned(), terminal.to_owned());
        if let Some(&dense) = self.position.get(&key) {
            return Some(NodeRef::Node(dense));
        }
        if self.grounded.contains(&key) {
            return Some(NodeRef::Ground);
        }
        None
    }
}

/// One ideal constraint row of the augmented system: exact relations over
/// the node voltages, with the row's coupled ideal current entering the
/// nodal balance through the transpose.
#[derive(Clone, Debug)]
pub struct AugmentedSystem {
    /// Real and imaginary parts of the constraint matrix `A`
    /// (`rows × nodes`): `A v = rhs`.
    pub constraint_re: CsMat<f64>,
    pub constraint_im: CsMat<f64>,
    pub rhs_re: Vec<f64>,
    pub rhs_im: Vec<f64>,
    /// The element behind each constraint row, by stable identity.
    pub labels: Vec<String>,
}

/// The assembled multiconductor nodal system.
#[derive(Clone, Debug)]
pub struct MulticonductorAdmittance {
    index: MulticonductorNodeIndex,
    conductance: CsMat<f64>,
    susceptance: CsMat<f64>,
    augmented: AugmentedSystem,
    diagnostics: Vec<Diagnostic>,
}

impl MulticonductorAdmittance {
    /// The node indexing behind every axis.
    #[must_use]
    pub const fn index(&self) -> &MulticonductorNodeIndex {
        &self.index
    }

    /// The passive nodal conductance `G`, siemens, over the unknowns.
    #[must_use]
    pub const fn conductance(&self) -> &CsMat<f64> {
        &self.conductance
    }

    /// The passive nodal susceptance `B`, siemens: `Y = G + jB`.
    #[must_use]
    pub const fn susceptance(&self) -> &CsMat<f64> {
        &self.susceptance
    }

    /// The ideal equipment constraint rows.
    #[must_use]
    pub const fn augmented(&self) -> &AugmentedSystem {
        &self.augmented
    }

    /// The builder's findings: every stamp it does not support, by element.
    #[must_use]
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }
}

/// The largest elimination pivot, in ohms, a series impedance inverse refuses
/// as singular.
const SINGULAR_PIVOT_OHMS: f64 = 1e-14;

/// Small dense complex inverse by Gaussian elimination with partial
/// pivoting, for conductor count sized matrices. `None` when a pivot is
/// [`SINGULAR_PIVOT_OHMS`] or smaller (or NaN).
fn invert(matrix: &[Vec<Complex64>]) -> Option<Vec<Vec<Complex64>>> {
    let n = matrix.len();
    let mut work: Vec<Vec<Complex64>> = matrix.to_vec();
    let mut inverse: Vec<Vec<Complex64>> = (0..n)
        .map(|row| {
            (0..n)
                .map(|column| {
                    if row == column {
                        Complex64::new(1.0, 0.0)
                    } else {
                        Complex64::new(0.0, 0.0)
                    }
                })
                .collect()
        })
        .collect();
    for pivot in 0..n {
        // The first of equal largest magnitudes, and a pivot no larger than
        // `SINGULAR_PIVOT_OHMS` is singular: a near zero jumper's impedance
        // would otherwise stamp an admittance of order 1e14 and beyond.
        let mut best = pivot;
        for row in pivot + 1..n {
            if work[row][pivot].norm() > work[best][pivot].norm() {
                best = row;
            }
        }
        if work[best][pivot].norm().is_nan() || work[best][pivot].norm() <= SINGULAR_PIVOT_OHMS {
            return None;
        }
        work.swap(pivot, best);
        inverse.swap(pivot, best);
        let lead = work[pivot][pivot];
        for column in 0..n {
            work[pivot][column] /= lead;
            inverse[pivot][column] /= lead;
        }
        for row in 0..n {
            if row == pivot {
                continue;
            }
            let factor = work[row][pivot];
            if factor.norm() == 0.0 {
                continue;
            }
            for column in 0..n {
                let w = work[pivot][column];
                let i = inverse[pivot][column];
                work[row][column] -= factor * w;
                inverse[row][column] -= factor * i;
            }
        }
    }
    Some(inverse)
}

struct Stamper {
    conductance: CooBuilder,
    susceptance: CooBuilder,
}

impl Stamper {
    fn new(n: usize) -> Self {
        Self {
            conductance: CooBuilder::new(n),
            susceptance: CooBuilder::new(n),
        }
    }

    /// Accumulate one admittance between two node references: ground rows
    /// and columns vanish, which is exactly the grounded reduction.
    fn add(&mut self, from: NodeRef, to: NodeRef, value: Complex64) {
        if let (NodeRef::Node(i), NodeRef::Node(j)) = (from, to) {
            self.conductance.add(i, j, value.re);
            self.susceptance.add(i, j, value.im);
        }
    }

    /// Accumulate an element's primitive admittance over the node references
    /// of its terminals, in the primitive's terminal order.
    fn add_primitive(&mut self, terminals: &[NodeRef], primitive: &[Vec<Complex64>]) {
        for (row, &from) in terminals.iter().enumerate() {
            for (column, &to) in terminals.iter().enumerate() {
                self.add(from, to, primitive[row][column]);
            }
        }
    }
}

/// The primitive admittance of a line, siemens, over its terminals in the
/// order `terminal_map_from` then `terminal_map_to`: the inverse of the
/// linecode's series impedance over the length couples the two ends, and
/// each end carries its shunt half,
///
/// ```text
/// [ Y + Y_from     -Y      ]      Y = (Z_series · length)⁻¹
/// [    -Y       Y + Y_to   ]      Y_from = (G_from + j B_from) · length
/// ```
///
/// The one line stamp: [`calc_multiconductor_admittance_matrix`] scatters
/// it into the nodal system.
///
/// # Errors
/// A line whose terminal maps disagree with the linecode's conductor count,
/// a linecode matrix of another shape, a length that is not finite and
/// positive, or a singular series impedance.
pub fn calc_line_primitive_admittance(
    line: &DistLine,
    code: &DistLineCode,
) -> Result<Vec<Vec<Complex64>>> {
    let conductors = code.n_conductors;
    if line.terminal_map_from.len() != conductors || line.terminal_map_to.len() != conductors {
        return Err(Error::Mtx(format!(
            "line `{}` maps {}/{} terminals over a {conductors} conductor linecode",
            line.name,
            line.terminal_map_from.len(),
            line.terminal_map_to.len()
        )));
    }
    if !line.length.is_finite() || line.length <= 0.0 {
        return Err(Error::Mtx(format!(
            "line `{}` states no usable length",
            line.name
        )));
    }
    let square = |matrix: &[Vec<f64>]| {
        matrix.len() == conductors && matrix.iter().all(|row| row.len() == conductors)
    };
    if ![
        &code.r_series,
        &code.x_series,
        &code.g_from,
        &code.b_from,
        &code.g_to,
        &code.b_to,
    ]
    .into_iter()
    .all(|matrix| square(matrix))
    {
        return Err(Error::Mtx(format!(
            "linecode `{}` states a matrix that is not {conductors} by {conductors}",
            code.name
        )));
    }
    let z: Vec<Vec<Complex64>> = (0..conductors)
        .map(|row| {
            (0..conductors)
                .map(|column| {
                    Complex64::new(code.r_series[row][column], code.x_series[row][column])
                        * line.length
                })
                .collect()
        })
        .collect();
    let Some(y_series) = invert(&z) else {
        return Err(Error::Mtx(format!(
            "line `{}` has a singular series impedance matrix",
            line.name
        )));
    };
    let mut primitive = vec![vec![Complex64::new(0.0, 0.0); 2 * conductors]; 2 * conductors];
    for row in 0..conductors {
        for column in 0..conductors {
            let y = y_series[row][column];
            let shunt_from =
                Complex64::new(code.g_from[row][column], code.b_from[row][column]) * line.length;
            let shunt_to =
                Complex64::new(code.g_to[row][column], code.b_to[row][column]) * line.length;
            primitive[row][column] = y + shunt_from;
            primitive[conductors + row][conductors + column] = y + shunt_to;
            primitive[row][conductors + column] = -y;
            primitive[conductors + row][column] = -y;
        }
    }
    Ok(primitive)
}

/// The primitive admittance of a shunt element, siemens, over its
/// `terminal_map`: its stated `G + j B`.
#[must_use]
pub fn calc_shunt_primitive_admittance(shunt: &DistShunt) -> Vec<Vec<Complex64>> {
    shunt
        .g
        .iter()
        .zip(&shunt.b)
        .map(|(g_row, b_row)| {
            g_row
                .iter()
                .zip(b_row)
                .map(|(&g, &b)| Complex64::new(g, b))
                .collect()
        })
        .collect()
}

/// The primitive admittance of a capacitor bank, siemens, over its
/// `terminal_map`, from its nameplate reactive power at nameplate voltage:
///
/// - single phase: one coil of `q / v²` across its two terminals;
/// - wye: one coil per phase terminal to the last terminal (the neutral),
///   `q / phases / v_ln²` with the line to line nameplate `v_ln = v / √3`;
/// - delta: a two terminal open delta bank is one coil, an `N` terminal bank
///   `N` coils around the ring, each `q / coils / v²`.
///
/// `None` for a configuration the builders do not stamp. The one capacitor
/// stamp: [`calc_multiconductor_admittance_matrix`] scatters it into the
/// nodal system.
///
/// # Errors
/// A single phase bank without exactly two terminals, a wye bank with no
/// phase terminal, or a delta bank with fewer than two terminals.
pub fn calc_capacitor_primitive_admittance(
    capacitor: &DistCapacitor,
) -> Result<Option<Vec<Vec<Complex64>>>> {
    let terminals = capacitor.terminal_map.len();
    let mut primitive = vec![vec![Complex64::new(0.0, 0.0); terminals]; terminals];
    let mut couple = |a: usize, c: usize, b: f64| {
        let y = Complex64::new(0.0, b);
        primitive[a][a] += y;
        primitive[c][c] += y;
        primitive[a][c] -= y;
        primitive[c][a] -= y;
    };
    match capacitor.configuration {
        Configuration::SinglePhase => {
            if terminals != 2 {
                return Err(Error::Mtx(format!(
                    "single phase capacitor `{}` maps {terminals} terminals",
                    capacitor.name
                )));
            }
            couple(
                0,
                1,
                capacitor.q_rated / (capacitor.v_nom * capacitor.v_nom),
            );
        }
        Configuration::Wye => {
            let phases = terminals.saturating_sub(1);
            if phases == 0 {
                return Err(Error::Mtx(format!(
                    "wye capacitor `{}` maps no phase terminal",
                    capacitor.name
                )));
            }
            let v_ln = capacitor.v_nom / 3f64.sqrt();
            let b = capacitor.q_rated / (phases as f64) / (v_ln * v_ln);
            for phase in 0..phases {
                couple(phase, phases, b);
            }
        }
        Configuration::Delta => {
            if terminals < 2 {
                return Err(Error::Mtx(format!(
                    "delta capacitor `{}` maps {terminals} terminals",
                    capacitor.name
                )));
            }
            let loops = if terminals == 2 { 1 } else { terminals };
            let b = capacitor.q_rated / (loops as f64) / (capacitor.v_nom * capacitor.v_nom);
            for pair in 0..loops {
                couple(pair, (pair + 1) % terminals, b);
            }
        }
        _ => return Ok(None),
    }
    Ok(Some(primitive))
}

/// Build the passive nodal admittance and the ideal equipment constraints.
///
/// # Errors
/// A structurally broken network: an element naming an undeclared bus or
/// terminal, or a line whose terminal maps disagree with its linecode shape.
/// Everything the builder cannot stamp exactly is a structured diagnostic,
/// never a silent omission or a fabricated impedance.
#[allow(clippy::too_many_lines)] // one stamp block per element family
#[allow(clippy::many_single_char_names)] // n and per conductor y/z/b follow the textbook stamps
pub fn calc_multiconductor_admittance_matrix(
    network: &MulticonductorNetwork,
) -> Result<MulticonductorAdmittance> {
    powerio_dist::require_electrical_readiness(network)?;
    let index = MulticonductorNodeIndex::build(network)?;
    let transformer_constraints = transformer::ideal_wye_constraints(network, &index)?;
    let n = index.len();
    let mut stamper = Stamper::new(n);
    let mut diagnostics = Vec::new();

    let resolve = |bus: &str, terminal: &str, element: &str| -> Result<NodeRef> {
        index.resolve(bus, terminal).ok_or_else(|| {
            Error::Mtx(format!(
                "{element} names terminal `{terminal}` bus `{bus}` does not declare"
            ))
        })
    };

    let linecode_of = |name: &str| {
        network
            .line_codes()
            .iter()
            .find(|linecode| linecode.name == name)
    };

    // Lines: series inverse of the linecode impedance over the length, and
    // both shunt halves.
    for line in network.lines() {
        let Some(code) = linecode_of(&line.linecode) else {
            return Err(Error::Mtx(format!(
                "line `{}` names linecode `{}` the network does not declare",
                line.name, line.linecode
            )));
        };
        let conductors = code.n_conductors;
        if line.terminal_map_from.len() != conductors || line.terminal_map_to.len() != conductors {
            return Err(Error::Mtx(format!(
                "line `{}` maps {}/{} terminals over a {conductors} conductor linecode",
                line.name,
                line.terminal_map_from.len(),
                line.terminal_map_to.len()
            )));
        }
        if !line.length.is_finite() || line.length <= 0.0 {
            diagnostics.push(Diagnostic::of(
                &codes::BUILD_MULTI_UNSUPPORTED_STAMP,
                format!(
                    "line `{}` states no usable length; its stamp is omitted",
                    line.name
                ),
            ));
            continue;
        }
        let primitive = calc_line_primitive_admittance(line, code)?;
        let terminals: Vec<NodeRef> =
            line.terminal_map_from
                .iter()
                .map(|terminal| resolve(&line.bus_from, terminal, &format!("line `{}`", line.name)))
                .chain(line.terminal_map_to.iter().map(|terminal| {
                    resolve(&line.bus_to, terminal, &format!("line `{}`", line.name))
                }))
                .collect::<Result<_>>()?;
        stamper.add_primitive(&terminals, &primitive);
    }

    // Shunt elements: their stated admittance matrix across their terminals.
    for shunt in network.shunts() {
        let terminals: Vec<NodeRef> = shunt
            .terminal_map
            .iter()
            .map(|terminal| resolve(&shunt.bus, terminal, &format!("shunt `{}`", shunt.name)))
            .collect::<Result<_>>()?;
        stamper.add_primitive(&terminals, &calc_shunt_primitive_admittance(shunt));
    }

    // Capacitor banks: nameplate reactive power at nameplate voltage.
    for capacitor in network.capacitors() {
        let terminals: Vec<NodeRef> = capacitor
            .terminal_map
            .iter()
            .map(|terminal| {
                resolve(
                    &capacitor.bus,
                    terminal,
                    &format!("capacitor `{}`", capacitor.name),
                )
            })
            .collect::<Result<_>>()?;
        match calc_capacitor_primitive_admittance(capacitor)? {
            Some(primitive) => stamper.add_primitive(&terminals, &primitive),
            None => diagnostics.push(Diagnostic::of(
                &codes::BUILD_MULTI_UNSUPPORTED_STAMP,
                format!(
                    "capacitor `{}` states a configuration this builder does not stamp",
                    capacitor.name
                ),
            )),
        }
    }

    // Ideal equipment: constraint rows over the node voltages, collected as
    // triplets until the row count is known.
    let mut constraint_re: Vec<(usize, usize, f64)> = Vec::new();
    let constraint_im: Vec<(usize, usize, f64)> = Vec::new();
    let mut rhs_re = Vec::new();
    let mut rhs_im = Vec::new();
    let mut labels = Vec::new();
    let mut rows = 0usize;

    // Each source specifies a voltage difference. The transpose of the
    // incidence rows supplies equal opposite reference-terminal currents.
    for source in network.sources() {
        let reference =
            source
                .reference_terminal
                .as_ref()
                .map_or(Ok(NodeRef::Ground), |terminal| {
                    resolve(
                        &source.bus,
                        terminal,
                        &format!("source `{}` reference", source.name),
                    )
                })?;
        for (position, terminal) in source.terminal_map.iter().enumerate() {
            let node = resolve(&source.bus, terminal, &format!("source `{}`", source.name))?;
            let magnitude = source.v_magnitude.get(position).copied().unwrap_or(0.0);
            let angle = source.v_angle.get(position).copied().unwrap_or(0.0);
            if node == reference {
                if source.reference_terminal.is_some() && magnitude != 0.0 {
                    return Err(Error::Mtx(format!(
                        "source `{}` has a nonzero voltage across electrically identical terminals",
                        source.name
                    )));
                }
                continue;
            }
            if let NodeRef::Node(dense) = node {
                constraint_re.push((rows, dense, 1.0));
            }
            if let NodeRef::Node(dense) = reference {
                constraint_re.push((rows, dense, -1.0));
            }
            rhs_re.push(magnitude * angle.cos());
            rhs_im.push(magnitude * angle.sin());
            labels.push(format!("source:{}:{terminal}", source.name));
            rows += 1;
        }
    }

    // WYE constraints use phase-to-neutral voltages on each side. The
    // transpose carries each winding's neutral current into nodal balance.
    for constraint in transformer_constraints {
        for (column, coefficient) in constraint.coefficients {
            constraint_re.push((rows, column, coefficient));
        }
        rhs_re.push(0.0);
        rhs_im.push(0.0);
        labels.push(constraint.label);
        rows += 1;
    }

    // Injections (loads, generators) are boundary data, never admittance;
    // families with no exact stamp are reported.
    for ibr in network.ibrs() {
        diagnostics.push(
            Diagnostic::of(
                &codes::BUILD_MULTI_UNSUPPORTED_STAMP,
                format!(
                    "inverter based resource `{}` has no passive admittance stamp; it is an injection, not an admittance",
                    ibr.name
                ),
            )
            ,
        );
    }

    let build_constraints = |triplets: &[(usize, usize, f64)]| {
        let mut builder = CooBuilder::new_rect(rows.max(1), n.max(1));
        for &(row, column, value) in triplets {
            builder.add(row, column, value);
        }
        let mut matrix = builder.finish_csr();
        if rows == 0 || n == 0 {
            matrix = CooBuilder::new_rect(rows.max(1), n.max(1)).finish_csr();
        }
        matrix
    };
    let augmented = AugmentedSystem {
        constraint_re: build_constraints(&constraint_re),
        constraint_im: build_constraints(&constraint_im),
        rhs_re,
        rhs_im,
        labels,
    };
    Ok(MulticonductorAdmittance {
        index,
        conductance: stamper.conductance.finish_csr(),
        susceptance: stamper.susceptance.finish_csr(),
        augmented,
        diagnostics,
    })
}
