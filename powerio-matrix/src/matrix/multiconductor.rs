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
//! currents. Transformer leakage, non-WYE connections, floating winding
//! neutrals, core shunts and tap decisions require a different augmented
//! formulation and return an unsupported-physics error before assembly.

use std::collections::{BTreeMap, BTreeSet};

use crate::diagnostics::Diagnostic;
use num_complex::Complex64;
use powerio_dist::{Configuration, MulticonductorNetwork};
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

fn find(parent: &mut [usize], node: usize) -> usize {
    let mut root = node;
    while parent[root] != root {
        root = parent[root];
    }
    let mut walk = node;
    while parent[walk] != root {
        let next = parent[walk];
        parent[walk] = root;
        walk = next;
    }
    root
}

impl MulticonductorNodeIndex {
    /// Build the index: bus table order, each bus's stated terminal order,
    /// with terminal `0` and explicitly grounded terminals excluded from the
    /// unknowns. Closed switches merge their paired terminals into one
    /// node — the first spelling encountered names the merged node.
    pub fn build(network: &MulticonductorNetwork) -> Result<Self> {
        // Union-find over provisional slots, one per ungrounded terminal.
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
        let mut parent: Vec<usize> = (0..provisional.len()).collect();
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
                        let (ra, rb) = (find(&mut parent, a), find(&mut parent, b));
                        if ra != rb {
                            parent[ra.max(rb)] = ra.min(rb);
                        }
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
        let grounded_roots: BTreeSet<usize> = ground_touched
            .into_iter()
            .map(|slot| find(&mut parent, slot))
            .collect();

        // Dense rows: one per surviving root, in provisional (table) order.
        let mut dense_of_root: BTreeMap<usize, usize> = BTreeMap::new();
        let mut nodes = Vec::new();
        let mut position = BTreeMap::new();
        for index in 0..provisional.len() {
            let root = find(&mut parent, index);
            // A root whose group touched ground through a switch grounds the
            // whole group.
            if grounded_roots.contains(&root) {
                grounded.insert(provisional[index].clone());
                continue;
            }
            let dense = *dense_of_root.entry(root).or_insert_with(|| {
                let (bus, terminal) = provisional[root].clone();
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

/// Small dense complex inverse by Gaussian elimination with partial
/// pivoting, for conductor count sized matrices.
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
        let best = (pivot..n).max_by(|&a, &b| {
            work[a][pivot]
                .norm()
                .partial_cmp(&work[b][pivot].norm())
                .unwrap_or(std::cmp::Ordering::Equal)
        })?;
        if work[best][pivot].norm() == 0.0 {
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
    let index = MulticonductorNodeIndex::build(network)?;
    for transformer in network.transformers() {
        let supported = transformer.windings.len() == 2
            && transformer.xsc_pct.iter().all(|&x| x == 0.0)
            && transformer.windings.iter().all(|w| {
                w.conn == powerio_dist::DistWindingConn::Wye
                    && w.r_pct == 0.0
                    && w.r_neutral.is_none_or(|r| r == 0.0)
                    && w.x_neutral.is_none_or(|x| x == 0.0)
                    && (w.terminal_map.len() == 1
                        || w.terminal_map.last().is_some_and(|terminal| {
                            index.resolve(&w.bus, terminal) == Some(NodeRef::Ground)
                        }))
            })
            && transformer.windings[0].terminal_map.len()
                == transformer.windings[1].terminal_map.len()
            && ["g_no_load", "b_no_load", "%noloadloss", "%imag"]
                .iter()
                .all(|key| {
                    transformer
                        .extras
                        .get(*key)
                        .and_then(serde_json::Value::as_f64)
                        .is_none_or(|v| v == 0.0)
                })
            && !["tap_min", "tap_max", "tap_ratio_min", "tap_ratio_max"]
                .iter()
                .any(|key| transformer.extras.contains_key(*key));
        if !supported {
            return Err(powerio_core::Error::new(&codes::BUILD_MULTI_PHYSICS_UNSUPPORTED,
                format!("transformer `{}` requires leakage, winding connection, neutral, core-loss, or tap-control equations outside the ideal grounded-WYE admittance profile", transformer.name)).into());
        }
    }
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
        let from: Vec<NodeRef> = line
            .terminal_map_from
            .iter()
            .map(|terminal| resolve(&line.bus_from, terminal, &format!("line `{}`", line.name)))
            .collect::<Result<_>>()?;
        let to: Vec<NodeRef> = line
            .terminal_map_to
            .iter()
            .map(|terminal| resolve(&line.bus_to, terminal, &format!("line `{}`", line.name)))
            .collect::<Result<_>>()?;
        for row in 0..conductors {
            for column in 0..conductors {
                let y = y_series[row][column];
                stamper.add(from[row], from[column], y);
                stamper.add(to[row], to[column], y);
                stamper.add(from[row], to[column], -y);
                stamper.add(to[row], from[column], -y);
                let shunt_from = Complex64::new(code.g_from[row][column], code.b_from[row][column])
                    * line.length;
                let shunt_to =
                    Complex64::new(code.g_to[row][column], code.b_to[row][column]) * line.length;
                stamper.add(from[row], from[column], shunt_from);
                stamper.add(to[row], to[column], shunt_to);
            }
        }
    }

    // Shunt elements: their stated admittance matrix across their terminals.
    for shunt in network.shunts() {
        let terminals: Vec<NodeRef> = shunt
            .terminal_map
            .iter()
            .map(|terminal| resolve(&shunt.bus, terminal, &format!("shunt `{}`", shunt.name)))
            .collect::<Result<_>>()?;
        for row in 0..terminals.len() {
            for column in 0..terminals.len() {
                let value = Complex64::new(shunt.g[row][column], shunt.b[row][column]);
                stamper.add(terminals[row], terminals[column], value);
            }
        }
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
        match capacitor.configuration {
            Configuration::SinglePhase => {
                if terminals.len() != 2 {
                    return Err(Error::Mtx(format!(
                        "single phase capacitor `{}` maps {} terminals",
                        capacitor.name,
                        terminals.len()
                    )));
                }
                let b = capacitor.q_rated / (capacitor.v_nom * capacitor.v_nom);
                let y = Complex64::new(0.0, b);
                stamper.add(terminals[0], terminals[0], y);
                stamper.add(terminals[1], terminals[1], y);
                stamper.add(terminals[0], terminals[1], -y);
                stamper.add(terminals[1], terminals[0], -y);
            }
            Configuration::Wye => {
                // Phase terminals to the last terminal (neutral); nameplate
                // voltage is line to line.
                let phases = terminals.len().saturating_sub(1);
                if phases == 0 {
                    return Err(Error::Mtx(format!(
                        "wye capacitor `{}` maps no phase terminal",
                        capacitor.name
                    )));
                }
                let v_ln = capacitor.v_nom / 3f64.sqrt();
                let b = capacitor.q_rated / (phases as f64) / (v_ln * v_ln);
                let neutral = terminals[phases];
                for &phase in &terminals[..phases] {
                    let y = Complex64::new(0.0, b);
                    stamper.add(phase, phase, y);
                    stamper.add(neutral, neutral, y);
                    stamper.add(phase, neutral, -y);
                    stamper.add(neutral, phase, -y);
                }
            }
            Configuration::Delta => {
                let phases = terminals.len();
                if phases < 2 {
                    return Err(Error::Mtx(format!(
                        "delta capacitor `{}` maps {} terminals",
                        capacitor.name, phases
                    )));
                }
                let b = capacitor.q_rated / (phases as f64) / (capacitor.v_nom * capacitor.v_nom);
                // Two terminals close one delta loop; more close `phases`.
                let loops = if phases == 2 { 1 } else { phases };
                for pair in 0..loops {
                    let a = terminals[pair];
                    let c = terminals[(pair + 1) % phases];
                    let y = Complex64::new(0.0, b);
                    stamper.add(a, a, y);
                    stamper.add(c, c, y);
                    stamper.add(a, c, -y);
                    stamper.add(c, a, -y);
                }
            }
            _ => {
                diagnostics.push(Diagnostic::of(
                    &codes::BUILD_MULTI_UNSUPPORTED_STAMP,
                    format!(
                        "capacitor `{}` states a configuration this builder does not stamp",
                        capacitor.name
                    ),
                ));
            }
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

    // Voltage sources: v(terminal) = stated complex voltage, one row per
    // ungrounded source terminal.
    for source in network.sources() {
        for (position, terminal) in source.terminal_map.iter().enumerate() {
            let node = resolve(&source.bus, terminal, &format!("source `{}`", source.name))?;
            let NodeRef::Node(dense) = node else {
                continue;
            };
            constraint_re.push((rows, dense, 1.0));
            let magnitude = source.v_magnitude.get(position).copied().unwrap_or(0.0);
            let angle = source.v_angle.get(position).copied().unwrap_or(0.0);
            rhs_re.push(magnitude * angle.cos());
            rhs_im.push(magnitude * angle.sin());
            labels.push(format!("source:{}:{terminal}", source.name));
            rows += 1;
        }
    }

    // Grounded WYE winding pairs use an exact voltage-ratio constraint.
    // Unsupported transformer physics is rejected before assembly.
    for transformer in network.transformers() {
        if transformer.windings.len() != 2 {
            diagnostics.push(
                Diagnostic::of(
                    &codes::BUILD_MULTI_UNSUPPORTED_STAMP,
                    format!(
                        "transformer `{}` has {} windings; only the two winding ideal plus leakage stamp is supported",
                        transformer.name,
                        transformer.windings.len()
                    ),
                )
                ,
            );
            continue;
        }
        let primary = &transformer.windings[0];
        let secondary = &transformer.windings[1];
        let pairs = primary.terminal_map.len().min(secondary.terminal_map.len());
        let ratio = if secondary.v_ref == 0.0 {
            0.0
        } else {
            (primary.v_ref * primary.tap) / (secondary.v_ref * secondary.tap)
        };
        if !ratio.is_finite() || ratio == 0.0 {
            diagnostics.push(Diagnostic::of(
                &codes::BUILD_MULTI_UNSUPPORTED_STAMP,
                format!(
                    "transformer `{}` states no finite winding ratio; its stamp is omitted",
                    transformer.name
                ),
            ));
            continue;
        }
        for pair in 0..pairs {
            let p = resolve(
                &primary.bus,
                &primary.terminal_map[pair],
                &format!("transformer `{}`", transformer.name),
            )?;
            let s = resolve(
                &secondary.bus,
                &secondary.terminal_map[pair],
                &format!("transformer `{}`", transformer.name),
            )?;
            match (p, s) {
                (NodeRef::Node(dense_p), NodeRef::Node(dense_s)) => {
                    constraint_re.push((rows, dense_p, 1.0));
                    constraint_re.push((rows, dense_s, -ratio));
                    rhs_re.push(0.0);
                    rhs_im.push(0.0);
                    labels.push(format!("transformer:{}:{pair}", transformer.name));
                    rows += 1;
                }
                // A grounded terminal fixes that side of the relation.
                (NodeRef::Node(dense_p), NodeRef::Ground) => {
                    constraint_re.push((rows, dense_p, 1.0));
                    rhs_re.push(0.0);
                    rhs_im.push(0.0);
                    labels.push(format!("transformer:{}:{pair}", transformer.name));
                    rows += 1;
                }
                (NodeRef::Ground, NodeRef::Node(dense_s)) => {
                    constraint_re.push((rows, dense_s, 1.0));
                    rhs_re.push(0.0);
                    rhs_im.push(0.0);
                    labels.push(format!("transformer:{}:{pair}", transformer.name));
                    rows += 1;
                }
                (NodeRef::Ground, NodeRef::Ground) => {}
            }
        }
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
