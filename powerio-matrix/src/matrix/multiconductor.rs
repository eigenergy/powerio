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
//! the linecode), shunts, capacitor banks, and every transformer that states
//! leakage: its terminal primitive
//! ([`calc_transformer_primitive_admittance`]) carries WYE and DELTA
//! windings, fixed taps, the magnetizing branch and finite neutral
//! impedances, for two winding units, the BMOPF centre tap and the BMOPF
//! single phase autotransformer. A solidly grounded transformer neutral
//! enters the augmented system as an exact zero voltage row. Ideal equipment
//! — a two winding transformer stating no leakage and voltage sources —
//! enters the augmented system as exact constraint rows over the node
//! voltages with their coupled ideal currents. Ideal WYE windings retain
//! explicit floating or grounded neutral terminals; an ideal unit with a
//! non-WYE connection, neutral impedances, a core shunt or tap bounds, and a
//! leakage unit outside the primitive's shapes, return an unsupported
//! physics error before assembly. Tap bounds on a leakage unit are decision
//! data the nodal system does not read: it stamps the stated fixed tap.

mod transformer;

pub use powerio_dist::transformer::{
    TransformerLeakageBase, TransformerPrimitive, calc_transformer_leakage_impedances,
    calc_transformer_primitive_admittance, calc_transformer_winding_admittance,
};

use std::collections::{BTreeMap, BTreeSet};

use crate::diagnostics::Diagnostic;
use num_complex::Complex64;
use powerio_dist::{
    Configuration, DistCapacitor, DistLine, DistLineCode, DistShunt, DistTransformer,
    MulticonductorNetwork,
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

/// The dense node indexing of one multiconductor network, over powerio-dist's
/// flat terminal index (so bus names compare ASCII case insensitively, as
/// every multiconductor calculation reads them).
#[derive(Clone, Debug)]
pub struct MulticonductorNodeIndex {
    nodes: Vec<DistNode>,
    terminals: powerio_dist::MulticonductorTerminalIndex,
    /// Flat terminal position to dense row; `None` for a grounded terminal.
    dense_of_terminal: Vec<Option<usize>>,
}

impl MulticonductorNodeIndex {
    /// Build the index: bus table order, each bus's stated terminal order,
    /// with terminal `0` and explicitly grounded terminals excluded from the
    /// unknowns. Closed switches merge their paired terminals into one
    /// node — the first spelling encountered names the merged node.
    ///
    /// # Errors
    /// A duplicated bus or terminal identity, or a closed switch naming a
    /// terminal its buses do not declare.
    pub fn build(network: &MulticonductorNetwork) -> Result<Self> {
        let terminals = powerio_dist::MulticonductorTerminalIndex::build(network)
            .map_err(|error| Error::Mtx(error.to_string()))?;
        // Components over provisional slots, one per ungrounded terminal.
        let mut grounded = vec![false; terminals.len()];
        let mut slot: Vec<Option<usize>> = vec![None; terminals.len()];
        let mut provisional: Vec<usize> = Vec::new();
        for bus in network.buses() {
            for terminal in &bus.terminals {
                let Some(position) = terminals.resolve(&bus.id, terminal) else {
                    continue;
                };
                if terminal == "0" || bus.grounded.contains(terminal) {
                    grounded[position] = true;
                    continue;
                }
                slot[position] = Some(provisional.len());
                provisional.push(position);
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
                let from = terminals.resolve(&switch.bus_from, from);
                let to = terminals.resolve(&switch.bus_to, to);
                let slot_of = |position: Option<usize>| position.and_then(|p| slot[p]);
                let grounded_at = |position: Option<usize>| position.is_some_and(|p| grounded[p]);
                match (slot_of(from), slot_of(to)) {
                    (Some(a), Some(b)) => {
                        joined_from.push(a);
                        joined_to.push(b);
                    }
                    // A closed switch onto ground grounds the other side's
                    // whole finished group.
                    (Some(a), None) if grounded_at(to) => ground_touched.push(a),
                    (None, Some(b)) if grounded_at(from) => ground_touched.push(b),
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
        let mut dense_of_terminal = vec![None; terminals.len()];
        for (index, &label) in labels.iter().enumerate() {
            // A group that touched ground through a switch is grounded whole.
            if grounded_labels.contains(&label) {
                continue;
            }
            let position = provisional[index];
            let dense = *dense_of_label.entry(label).or_insert_with(|| {
                let (bus, terminal) = terminals.terminals()[position].clone();
                nodes.push(DistNode { bus, terminal });
                nodes.len() - 1
            });
            dense_of_terminal[position] = Some(dense);
        }
        Ok(Self {
            nodes,
            terminals,
            dense_of_terminal,
        })
    }

    /// The flat terminal index underneath: every declared terminal, grounded
    /// or not, at its stable position.
    #[must_use]
    pub const fn terminals(&self) -> &powerio_dist::MulticonductorTerminalIndex {
        &self.terminals
    }

    /// Where the terminal at a flat position lands: a dense unknown or
    /// ground. `None` for a position past the terminal index.
    #[must_use]
    pub fn resolve_position(&self, position: usize) -> Option<NodeRef> {
        self.dense_of_terminal
            .get(position)
            .map(|dense| dense.map_or(NodeRef::Ground, NodeRef::Node))
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
        let position = self.terminals.resolve(bus, terminal)?;
        Some(self.dense_of_terminal[position].map_or(NodeRef::Ground, NodeRef::Node))
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

/// The dense inverse of a series impedance: `None` when a pivot is
/// [`SINGULAR_PIVOT_OHMS`] or smaller (or NaN).
fn invert(matrix: &[Vec<Complex64>]) -> Option<Vec<Vec<Complex64>>> {
    powerio_dist::dense::invert_dense_matrix(
        matrix,
        powerio_dist::dense::DensePivot::Partial,
        SINGULAR_PIVOT_OHMS,
        0.0,
    )
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
    fn add_primitive(&mut self, terminals: &[NodeRef], primitive: &[Vec<Complex64>]) -> Result<()> {
        scatter_primitive_admittance(terminals, primitive, |from, to, value| {
            self.add(from, to, value);
        })
    }
}

/// Scatter an element's primitive admittance onto the system positions of
/// its terminals: `add(terminals[r], terminals[c], primitive[r][c])` for every
/// entry, in row major order. The one scatter of a primitive into a nodal
/// system: [`calc_multiconductor_admittance_matrix`] reads it over its node
/// references, and a caller indexing terminals its own way reads it over
/// its positions.
///
/// # Errors
/// A primitive that is not square over `terminals`.
pub fn scatter_primitive_admittance<T: Copy>(
    terminals: &[T],
    primitive: &[Vec<Complex64>],
    mut add: impl FnMut(T, T, Complex64),
) -> Result<()> {
    if primitive.len() != terminals.len()
        || primitive.iter().any(|row| row.len() != terminals.len())
    {
        return Err(Error::Mtx(format!(
            "a primitive admittance of {} rows does not match its {} terminals",
            primitive.len(),
            terminals.len()
        )));
    }
    for (row, &from) in terminals.iter().enumerate() {
        for (column, &to) in terminals.iter().enumerate() {
            add(from, to, primitive[row][column]);
        }
    }
    Ok(())
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

/// A capacitor bank's primitive admittance, siemens, over its terminal map.
///
/// Each coil of the bank's connection ([`powerio_dist::calc_element_connection`]:
/// a WYE coil from each phase to its neutral or to ground, a DELTA coil
/// between consecutive terminals or across a two terminal bank, a single
/// phase coil across its two terminals) carries an equal share of the rated
/// reactive power at its coil voltage:
///
/// ```text
/// b = q_rated / coils / V²,   V = v_nom / √3 (WYE with more than one terminal), v_nom otherwise
/// ```
///
/// The WYE nameplate is line to line when the bank has an explicit return
/// or several phases; a one terminal WYE bank states its coil voltage.
/// `None` for a configuration with no capacitor connection. The one
/// capacitor stamp: [`calc_multiconductor_admittance_matrix`] scatters it
/// into the nodal system and the multiconductor OPF writes it per unit.
///
/// # Errors
/// A nameplate that is not finite with a positive voltage, or a terminal map
/// its configuration cannot carry (an undeclared bus, an ambiguous WYE
/// neutral, a single phase bank without two terminals).
pub fn calc_capacitor_primitive_admittance(
    network: &MulticonductorNetwork,
    capacitor: &DistCapacitor,
) -> Result<Option<Vec<Vec<Complex64>>>> {
    if !matches!(
        capacitor.configuration,
        Configuration::Wye | Configuration::Delta | Configuration::SinglePhase
    ) {
        return Ok(None);
    }
    if !capacitor.q_rated.is_finite() || !capacitor.v_nom.is_finite() || capacitor.v_nom <= 0.0 {
        return Err(Error::Mtx(format!(
            "capacitor `{}` states no finite nameplate reactive power at a positive voltage",
            capacitor.name
        )));
    }
    let terminals = capacitor.terminal_map.len();
    if capacitor.configuration == Configuration::SinglePhase && terminals != 2 {
        return Err(Error::Mtx(format!(
            "single phase capacitor `{}` maps {terminals} terminals",
            capacitor.name
        )));
    }
    let coils = powerio_dist::calc_element_connection(
        network,
        &capacitor.bus,
        &capacitor.terminal_map,
        capacitor.configuration,
        None,
    )
    .map_err(|reason| Error::Mtx(format!("capacitor `{}`: {reason}", capacitor.name)))?;
    let coil_voltage = if capacitor.configuration == Configuration::Wye && terminals > 1 {
        capacitor.v_nom / 3f64.sqrt()
    } else {
        capacitor.v_nom
    };
    let y = Complex64::new(
        0.0,
        capacitor.q_rated / (coils.len() as f64) / (coil_voltage * coil_voltage),
    );
    let mut primitive = vec![vec![Complex64::new(0.0, 0.0); terminals]; terminals];
    for coil in coils {
        primitive[coil.from][coil.from] += y;
        if let Some(to) = coil.to {
            primitive[to][to] += y;
            primitive[coil.from][to] -= y;
            primitive[to][coil.from] -= y;
        }
    }
    Ok(Some(primitive))
}

/// The passive element family behind a [`PassivePrimitive`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum PassiveFamily {
    Line,
    Shunt,
    Capacitor,
    Transformer,
}

impl PassiveFamily {
    /// The family's lowercase name, as messages and element listings print it.
    #[must_use]
    pub const fn noun(self) -> &'static str {
        match self {
            Self::Line => "line",
            Self::Shunt => "shunt",
            Self::Capacitor => "capacitor",
            Self::Transformer => "transformer",
        }
    }
}

/// A transformer neutral the source grounds solidly: an exact zero voltage
/// at `(bus, terminal)`, not an admittance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct GroundedNeutral<'a> {
    /// The transformer that grounds it.
    pub transformer: &'a str,
    pub bus: &'a str,
    pub terminal: &'a str,
}

impl GroundedNeutral<'_> {
    /// Whether this neutral needs its own zero voltage row, given the voltage
    /// an ideal source already prescribes on the same node (`None` when none
    /// does): `true` without a prescription, `false` when the source already
    /// fixes the node at exactly zero (a second row would make the
    /// constraint rows rank deficient).
    ///
    /// The one rule for a solid neutral meeting a source:
    /// [`calc_multiconductor_admittance_matrix`] and every caller assembling
    /// its own prescribed voltages read it.
    ///
    /// # Errors
    /// A nonzero prescribed voltage on the grounded node.
    pub fn needs_zero_voltage_row(&self, prescribed: Option<Complex64>) -> Result<bool> {
        match prescribed {
            None => Ok(true),
            Some(voltage) if voltage == Complex64::new(0.0, 0.0) => Ok(false),
            Some(voltage) => Err(Error::Commit(powerio_core::Error::new(
                &codes::BUILD_MULTI_PHYSICS_UNSUPPORTED,
                format!(
                    "transformer `{}` grounds neutral `{}`:{} solidly, but a source prescribes {} V there",
                    self.transformer,
                    self.bus,
                    self.terminal,
                    voltage.norm()
                ),
            ))),
        }
    }
}

/// Every admittance element of a network with the exact constraints it
/// leaves to the caller: [`calc_passive_primitives`].
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct NetworkPrimitives<'a> {
    /// Lines, shunts, capacitor banks, then transformers stating leakage,
    /// each family in table order.
    pub elements: Vec<PassivePrimitive<'a>>,
    /// Solidly grounded transformer neutrals, once per `(bus, terminal)`
    /// spelling, in transformer order.
    pub grounded: Vec<GroundedNeutral<'a>>,
    /// Transformers that state no leakage: ideal couplings with no
    /// admittance, which need exact voltage constraints.
    pub ideal_transformers: Vec<&'a DistTransformer>,
    /// Elements left out because the builders do not stamp them.
    pub diagnostics: Vec<Diagnostic>,
}

/// One passive element's primitive admittance, siemens, with the bus
/// terminal behind each primitive row.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct PassivePrimitive<'a> {
    pub family: PassiveFamily,
    pub name: &'a str,
    /// `(bus, terminal)` of each primitive row, in primitive order.
    pub terminals: Vec<(&'a str, &'a str)>,
    pub primitive: Vec<Vec<Complex64>>,
}

/// Every admittance element of the network with its primitive admittance:
/// lines (series inverse and both shunt halves,
/// [`calc_line_primitive_admittance`]), then shunts
/// ([`calc_shunt_primitive_admittance`]), then capacitor banks
/// ([`calc_capacitor_primitive_admittance`]), then every transformer that
/// states leakage ([`calc_transformer_primitive_admittance`]), each family
/// in table order, with the transformers' solidly grounded neutrals and the
/// ideal transformers left for exact constraints. The one element list:
/// [`calc_multiconductor_admittance_matrix`] scatters it over its node
/// references, and a caller indexing terminals its own way scatters it over
/// its positions with [`scatter_primitive_admittance`].
///
/// A line's linecode is found under the network's identifier convention
/// ([`MulticonductorNetwork::identifier_key`]), the convention readiness
/// checks. An element the builders do not stamp (a line without a usable
/// length, a capacitor configuration with no primitive) is left out and
/// reported in the diagnostics, never stamped approximately.
///
/// # Errors
/// A line naming an undeclared linecode or mapping terminals that disagree
/// with its linecode, a singular series impedance, a shunt whose matrices
/// are not square over its terminal map or not finite, a capacitor whose
/// nameplate is not finite and positive, a capacitor terminal map its
/// configuration cannot carry, or a leakage transformer the primitive
/// refuses.
#[allow(clippy::too_many_lines)] // one block per element family, in stamp order
pub fn calc_passive_primitives(network: &MulticonductorNetwork) -> Result<NetworkPrimitives<'_>> {
    let mut linecodes: BTreeMap<String, &DistLineCode> = BTreeMap::new();
    for code in network.line_codes() {
        linecodes
            .entry(network.identifier_key(&code.name))
            .or_insert(code);
    }
    let mut primitives = Vec::with_capacity(
        network.lines().len() + network.shunts().len() + network.capacitors().len(),
    );
    let mut diagnostics = Vec::new();

    for line in network.lines() {
        let Some(code) = linecodes.get(&network.identifier_key(&line.linecode)) else {
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
        primitives.push(PassivePrimitive {
            family: PassiveFamily::Line,
            name: &line.name,
            terminals: line
                .terminal_map_from
                .iter()
                .map(|terminal| (line.bus_from.as_str(), terminal.as_str()))
                .chain(
                    line.terminal_map_to
                        .iter()
                        .map(|terminal| (line.bus_to.as_str(), terminal.as_str())),
                )
                .collect(),
            primitive: calc_line_primitive_admittance(line, code)?,
        });
    }

    for shunt in network.shunts() {
        let size = shunt.terminal_map.len();
        let square = |matrix: &[Vec<f64>]| {
            matrix.len() == size
                && matrix
                    .iter()
                    .all(|row| row.len() == size && row.iter().all(|value| value.is_finite()))
        };
        if !square(&shunt.g) || !square(&shunt.b) {
            return Err(Error::Mtx(format!(
                "shunt `{}` states a matrix that is not finite and {size} by {size}",
                shunt.name
            )));
        }
        primitives.push(PassivePrimitive {
            family: PassiveFamily::Shunt,
            name: &shunt.name,
            terminals: shunt
                .terminal_map
                .iter()
                .map(|terminal| (shunt.bus.as_str(), terminal.as_str()))
                .collect(),
            primitive: calc_shunt_primitive_admittance(shunt),
        });
    }

    for capacitor in network.capacitors() {
        match calc_capacitor_primitive_admittance(network, capacitor)? {
            Some(primitive) => primitives.push(PassivePrimitive {
                family: PassiveFamily::Capacitor,
                name: &capacitor.name,
                terminals: capacitor
                    .terminal_map
                    .iter()
                    .map(|terminal| (capacitor.bus.as_str(), terminal.as_str()))
                    .collect(),
                primitive,
            }),
            None => diagnostics.push(Diagnostic::of(
                &codes::BUILD_MULTI_UNSUPPORTED_STAMP,
                format!(
                    "capacitor `{}` states a configuration this builder does not stamp",
                    capacitor.name
                ),
            )),
        }
    }

    let mut grounded: Vec<GroundedNeutral<'_>> = Vec::new();
    let mut ideal_transformers = Vec::new();
    for transformer in network.transformers() {
        if powerio_dist::transformer::transformer_states_no_leakage(transformer) {
            ideal_transformers.push(transformer);
            continue;
        }
        let element = calc_transformer_primitive_admittance(network, transformer)?;
        for &(bus, terminal) in &element.grounded {
            if !grounded
                .iter()
                .any(|g| g.bus.eq_ignore_ascii_case(bus) && g.terminal == terminal)
            {
                grounded.push(GroundedNeutral {
                    transformer: element.name,
                    bus,
                    terminal,
                });
            }
        }
        primitives.push(PassivePrimitive {
            family: PassiveFamily::Transformer,
            name: element.name,
            terminals: element.terminals,
            primitive: element.primitive,
        });
    }
    Ok(NetworkPrimitives {
        elements: primitives,
        grounded,
        ideal_transformers,
        diagnostics,
    })
}

/// Build the passive nodal admittance and the ideal equipment constraints.
///
/// # Errors
/// A structurally broken network: an element naming an undeclared bus or
/// terminal, or a line whose terminal maps disagree with its linecode shape.
/// Everything the builder cannot stamp exactly is a structured diagnostic,
/// never a silent omission or a fabricated impedance.
pub fn calc_multiconductor_admittance_matrix(
    network: &MulticonductorNetwork,
) -> Result<MulticonductorAdmittance> {
    powerio_dist::require_electrical_readiness(network)?;
    let primitives = calc_passive_primitives(network)?;
    assemble_multiconductor_admittance(network, &primitives)
}

/// [`calc_multiconductor_admittance_matrix`] over element primitives the
/// caller already holds from [`calc_passive_primitives`] of the same
/// network: a caller that also reads each element's primitive (for its port
/// currents) lists them once. The one assembly of the multiconductor nodal
/// system and its source, ideal transformer and grounded neutral rows.
///
/// # Errors
/// As [`calc_multiconductor_admittance_matrix`], except that the network's
/// electrical readiness is not checked again.
#[allow(clippy::too_many_lines)] // one stamp block per element family
#[allow(clippy::many_single_char_names)] // n and per conductor y/z/b follow the textbook stamps
pub fn assemble_multiconductor_admittance(
    network: &MulticonductorNetwork,
    primitives: &NetworkPrimitives<'_>,
) -> Result<MulticonductorAdmittance> {
    let index = MulticonductorNodeIndex::build(network)?;
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

    // Lines, shunts, capacitor banks and leakage transformers: the network's
    // one element list, scattered over the node references of its
    // terminals.
    let elements = primitives;
    let transformer_constraints =
        transformer::ideal_wye_constraints(network, &elements.ideal_transformers, &index)?;
    diagnostics.extend(elements.diagnostics.iter().cloned());
    for element in &elements.elements {
        let label = format!("{} `{}`", element.family.noun(), element.name);
        let terminals: Vec<NodeRef> = element
            .terminals
            .iter()
            .map(|&(bus, terminal)| resolve(bus, terminal, &label))
            .collect::<Result<_>>()?;
        stamper.add_primitive(&terminals, &element.primitive)?;
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
            // A terminal that is its own reference (ground, or the stated
            // reference terminal) has no voltage to prescribe: zero is
            // already true there, anything else contradicts it.
            if node == reference {
                if magnitude != 0.0 {
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

    // A solidly grounded transformer neutral: v = 0, its transpose the
    // neutral's ground current, once per node. A source row that already
    // fixes the node alone settles it (`GroundedNeutral::needs_zero_voltage_row`).
    let mut fixed_by_source: BTreeMap<usize, Complex64> = BTreeMap::new();
    for row in 0..rows {
        let entries: Vec<&(usize, usize, f64)> =
            constraint_re.iter().filter(|(r, _, _)| *r == row).collect();
        // A one entry source row is `±v = rhs` (the node, or a reference
        // terminal against ground).
        if let [(_, dense, coefficient)] = entries.as_slice()
            && labels[row].starts_with("source:")
        {
            fixed_by_source.insert(
                *dense,
                Complex64::new(rhs_re[row], rhs_im[row]) / *coefficient,
            );
        }
    }
    let mut grounded_nodes: BTreeSet<usize> = BTreeSet::new();
    for neutral in &elements.grounded {
        let label = format!("transformer `{}`", neutral.transformer);
        let NodeRef::Node(dense) = resolve(neutral.bus, neutral.terminal, &label)? else {
            continue;
        };
        if !grounded_nodes.insert(dense)
            || !neutral.needs_zero_voltage_row(fixed_by_source.get(&dense).copied())?
        {
            continue;
        }
        constraint_re.push((rows, dense, 1.0));
        rhs_re.push(0.0);
        rhs_im.push(0.0);
        labels.push(format!(
            "transformer:{}:neutral:{}.{}",
            neutral.transformer, neutral.bus, neutral.terminal
        ));
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
