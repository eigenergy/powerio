//! Read DeepMind OPFData JSON examples into the transmission [`Network`].
//!
//! OPFData stores one solved AC-OPF problem per JSON file. The `grid` section
//! carries the problem inputs, `solution` carries the solved operating point,
//! and `metadata.objective` carries the solved quadratic objective. This reader
//! maps the solved snapshot into the neutral MW/degree model and retains the
//! original JSON for byte-exact same-format echo.
//!
//! The published FullTop and N-1 datasets use the same schema at every grid
//! size. Element counts and topology come exclusively from each document; N-1
//! outages therefore need no case-specific handling.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde::Deserialize;

use crate::network::{
    Branch, BranchCharging, BranchSolution, Bus, BusId, BusType, GenCost, Generator, Load, Network,
    Shunt, SourceFormat,
};
use crate::normalize::{RAD_TO_DEG, cost_from_pu};
use crate::{Error, Result};

use super::Parsed;

const FMT: &str = "OPFData JSON";
type ExtraFields = BTreeMap<String, serde_json::Value>;

#[derive(Debug, Deserialize)]
struct Document {
    grid: Grid,
    solution: Solution,
    metadata: Metadata,
    #[serde(flatten)]
    extra: ExtraFields,
}

#[derive(Debug, Deserialize)]
struct Grid {
    nodes: GridNodes,
    edges: GridEdges,
    context: Vec<Vec<Vec<f64>>>,
    #[serde(flatten)]
    extra: ExtraFields,
}

#[derive(Debug, Deserialize)]
struct GridNodes {
    bus: Vec<BusRow>,
    generator: Vec<GeneratorRow>,
    load: Vec<LoadRow>,
    shunt: Vec<ShuntRow>,
    #[serde(flatten)]
    extra: ExtraFields,
}

#[derive(Debug, Deserialize)]
struct GridEdges {
    ac_line: AcLineEdges,
    transformer: TransformerEdges,
    generator_link: LinkEdges,
    load_link: LinkEdges,
    shunt_link: LinkEdges,
    #[serde(flatten)]
    extra: ExtraFields,
}

#[derive(Debug, Deserialize)]
struct AcLineEdges {
    senders: Vec<usize>,
    receivers: Vec<usize>,
    features: Vec<AcLineRow>,
    #[serde(flatten)]
    extra: ExtraFields,
}

#[derive(Debug, Deserialize)]
struct TransformerEdges {
    senders: Vec<usize>,
    receivers: Vec<usize>,
    features: Vec<TransformerRow>,
    #[serde(flatten)]
    extra: ExtraFields,
}

#[derive(Debug, Deserialize)]
struct LinkEdges {
    senders: Vec<usize>,
    receivers: Vec<usize>,
    #[serde(flatten)]
    extra: ExtraFields,
}

#[derive(Debug, Deserialize)]
struct Solution {
    nodes: SolutionNodes,
    edges: SolutionEdges,
    #[serde(flatten)]
    extra: ExtraFields,
}

#[derive(Debug, Deserialize)]
struct SolutionNodes {
    bus: Vec<BusSolutionRow>,
    generator: Vec<GeneratorSolutionRow>,
    #[serde(flatten)]
    extra: ExtraFields,
}

#[derive(Debug, Deserialize)]
struct SolutionEdges {
    ac_line: SolutionBranchEdges,
    transformer: SolutionBranchEdges,
    #[serde(flatten)]
    extra: ExtraFields,
}

#[derive(Debug, Deserialize)]
struct SolutionBranchEdges {
    senders: Vec<usize>,
    receivers: Vec<usize>,
    features: Vec<BranchSolutionRow>,
    #[serde(flatten)]
    extra: ExtraFields,
}

#[derive(Debug, Deserialize)]
struct Metadata {
    objective: f64,
    #[serde(flatten)]
    extra: ExtraFields,
}

// These transparent row types make the documented feature widths part of the
// serde contract while keeping column meaning out of the mapping code's magic
// indices. A row with too few or too many values fails deserialization.
#[derive(Debug, Deserialize)]
#[serde(transparent)]
struct BusRow([f64; 4]);

impl BusRow {
    fn base_kv(&self) -> f64 {
        self.0[0]
    }

    fn bus_type(&self) -> f64 {
        self.0[1]
    }

    fn vmin(&self) -> f64 {
        self.0[2]
    }

    fn vmax(&self) -> f64 {
        self.0[3]
    }
}

#[derive(Debug, Deserialize)]
#[serde(transparent)]
struct GeneratorRow([f64; 11]);

impl GeneratorRow {
    fn mbase(&self) -> f64 {
        self.0[0]
    }

    fn pmin(&self) -> f64 {
        self.0[2]
    }

    fn pmax(&self) -> f64 {
        self.0[3]
    }

    fn qmin(&self) -> f64 {
        self.0[5]
    }

    fn qmax(&self) -> f64 {
        self.0[6]
    }

    fn cost_coefficients(&self) -> &[f64] {
        &self.0[8..11]
    }

    fn objective_at(&self, pg: f64) -> f64 {
        self.0[8] * pg * pg + self.0[9] * pg + self.0[10]
    }
}

#[derive(Debug, Deserialize)]
#[serde(transparent)]
struct LoadRow([f64; 2]);

impl LoadRow {
    fn pd(&self) -> f64 {
        self.0[0]
    }

    fn qd(&self) -> f64 {
        self.0[1]
    }
}

#[derive(Debug, Deserialize)]
#[serde(transparent)]
struct ShuntRow([f64; 2]);

impl ShuntRow {
    fn bs(&self) -> f64 {
        self.0[0]
    }

    fn gs(&self) -> f64 {
        self.0[1]
    }
}

#[derive(Debug, Deserialize)]
#[serde(transparent)]
struct AcLineRow([f64; 9]);

impl AcLineRow {
    fn angmin(&self) -> f64 {
        self.0[0]
    }

    fn angmax(&self) -> f64 {
        self.0[1]
    }

    fn b_fr(&self) -> f64 {
        self.0[2]
    }

    fn b_to(&self) -> f64 {
        self.0[3]
    }

    fn r(&self) -> f64 {
        self.0[4]
    }

    fn x(&self) -> f64 {
        self.0[5]
    }

    fn rate_a(&self) -> f64 {
        self.0[6]
    }

    fn rate_b(&self) -> f64 {
        self.0[7]
    }

    fn rate_c(&self) -> f64 {
        self.0[8]
    }
}

#[derive(Debug, Deserialize)]
#[serde(transparent)]
struct TransformerRow([f64; 11]);

impl TransformerRow {
    fn angmin(&self) -> f64 {
        self.0[0]
    }

    fn angmax(&self) -> f64 {
        self.0[1]
    }

    fn r(&self) -> f64 {
        self.0[2]
    }

    fn x(&self) -> f64 {
        self.0[3]
    }

    fn rate_a(&self) -> f64 {
        self.0[4]
    }

    fn rate_b(&self) -> f64 {
        self.0[5]
    }

    fn rate_c(&self) -> f64 {
        self.0[6]
    }

    fn tap(&self) -> f64 {
        self.0[7]
    }

    fn shift(&self) -> f64 {
        self.0[8]
    }

    fn b_fr(&self) -> f64 {
        self.0[9]
    }

    fn b_to(&self) -> f64 {
        self.0[10]
    }
}

#[derive(Debug, Deserialize)]
#[serde(transparent)]
struct BusSolutionRow([f64; 2]);

impl BusSolutionRow {
    fn va(&self) -> f64 {
        self.0[0]
    }

    fn vm(&self) -> f64 {
        self.0[1]
    }
}

#[derive(Debug, Deserialize)]
#[serde(transparent)]
struct GeneratorSolutionRow([f64; 2]);

impl GeneratorSolutionRow {
    fn pg(&self) -> f64 {
        self.0[0]
    }

    fn qg(&self) -> f64 {
        self.0[1]
    }
}

#[derive(Debug, Deserialize)]
#[serde(transparent)]
struct BranchSolutionRow([f64; 4]);

impl BranchSolutionRow {
    fn to_network(&self, base_mva: f64) -> BranchSolution {
        // OPFData orders solved flows [pt, qt, pf, qf].
        BranchSolution::new(
            self.0[2] * base_mva,
            self.0[3] * base_mva,
            self.0[0] * base_mva,
            self.0[1] * base_mva,
        )
    }
}

fn bad(message: impl Into<String>) -> Error {
    Error::FormatRead {
        format: FMT,
        message: message.into(),
    }
}

fn base_mva(context: &[Vec<Vec<f64>>]) -> Result<f64> {
    if context.len() != 1 || context[0].len() != 1 || context[0][0].len() != 1 {
        return Err(bad(format!(
            "`grid.context` must have shape [1, 1, 1], got outer lengths [{}, {}, {}]",
            context.len(),
            context.first().map_or(0, Vec::len),
            context
                .first()
                .and_then(|row| row.first())
                .map_or(0, Vec::len)
        )));
    }
    let base = context[0][0][0];
    if !base.is_finite() || base <= 0.0 {
        return Err(bad(format!(
            "`grid.context` baseMVA must be positive and finite, got {base}"
        )));
    }
    Ok(base)
}

fn equal_len(
    what: &str,
    left_name: &str,
    left: usize,
    right_name: &str,
    right: usize,
) -> Result<()> {
    if left != right {
        return Err(bad(format!(
            "`{what}` length mismatch: `{left_name}` has {left} rows but `{right_name}` has {right}"
        )));
    }
    Ok(())
}

fn validate_edge_arrays(
    what: &str,
    senders: &[usize],
    receivers: &[usize],
    features: usize,
    buses: usize,
) -> Result<()> {
    equal_len(what, "senders", senders.len(), "receivers", receivers.len())?;
    equal_len(what, "senders", senders.len(), "features", features)?;
    for (index, (&from, &to)) in senders.iter().zip(receivers).enumerate() {
        if from >= buses || to >= buses {
            return Err(bad(format!(
                "`{what}` row {index} references bus indices ({from}, {to}) but there are {buses} buses"
            )));
        }
    }
    Ok(())
}

fn validate_solution_edges(
    what: &str,
    grid_senders: &[usize],
    grid_receivers: &[usize],
    solution: &SolutionBranchEdges,
    buses: usize,
) -> Result<()> {
    validate_edge_arrays(
        what,
        &solution.senders,
        &solution.receivers,
        solution.features.len(),
        buses,
    )?;
    equal_len(
        what,
        "grid edges",
        grid_senders.len(),
        "solution edges",
        solution.senders.len(),
    )?;
    for (index, ((&grid_from, &grid_to), (&sol_from, &sol_to))) in grid_senders
        .iter()
        .zip(grid_receivers)
        .zip(solution.senders.iter().zip(&solution.receivers))
        .enumerate()
    {
        if (grid_from, grid_to) != (sol_from, sol_to) {
            return Err(bad(format!(
                "`{what}` row {index} topology differs between grid ({grid_from}, {grid_to}) and solution ({sol_from}, {sol_to})"
            )));
        }
    }
    Ok(())
}

fn linked_buses(what: &str, link: &LinkEdges, rows: usize, buses: usize) -> Result<Vec<BusId>> {
    equal_len(
        what,
        "senders",
        link.senders.len(),
        "receivers",
        link.receivers.len(),
    )?;
    equal_len(what, "links", link.senders.len(), "node rows", rows)?;

    let mut mapped = vec![None; rows];
    for (index, (&sender, &receiver)) in link.senders.iter().zip(&link.receivers).enumerate() {
        if sender >= rows {
            return Err(bad(format!(
                "`{what}` row {index} references node index {sender} but there are {rows} node rows"
            )));
        }
        if receiver >= buses {
            return Err(bad(format!(
                "`{what}` row {index} references bus index {receiver} but there are {buses} buses"
            )));
        }
        if mapped[sender].replace(BusId(receiver + 1)).is_some() {
            return Err(bad(format!(
                "`{what}` contains more than one link for node index {sender}"
            )));
        }
    }

    mapped
        .into_iter()
        .enumerate()
        .map(|(index, bus)| {
            bus.ok_or_else(|| bad(format!("`{what}` has no link for node index {index}")))
        })
        .collect()
}

fn bus_type(value: f64, row: usize) -> Result<BusType> {
    match value {
        1.0 => Ok(BusType::Pq),
        2.0 => Ok(BusType::Pv),
        3.0 => Ok(BusType::Ref),
        4.0 => Ok(BusType::Isolated),
        _ => Err(bad(format!(
            "`grid.nodes.bus` row {row} has invalid bus type {value}; expected 1, 2, 3, or 4"
        ))),
    }
}

fn warn_extra_fields(path: &str, extra: &ExtraFields, warnings: &mut Vec<String>) {
    if extra.is_empty() {
        return;
    }
    let fields = extra
        .keys()
        .map(|field| {
            if path.is_empty() {
                format!("`{field}`")
            } else {
                format!("`{path}.{field}`")
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    warnings.push(format!(
        "OPFData fields {fields} are not part of the published schema; they remain in the retained source but are not represented in the canonical snapshot"
    ));
}

fn warn_document_extras(document: &Document, warnings: &mut Vec<String>) {
    warn_extra_fields("", &document.extra, warnings);
    warn_extra_fields("grid", &document.grid.extra, warnings);
    warn_extra_fields("grid.nodes", &document.grid.nodes.extra, warnings);
    warn_extra_fields("grid.edges", &document.grid.edges.extra, warnings);
    warn_extra_fields(
        "grid.edges.ac_line",
        &document.grid.edges.ac_line.extra,
        warnings,
    );
    warn_extra_fields(
        "grid.edges.transformer",
        &document.grid.edges.transformer.extra,
        warnings,
    );
    warn_extra_fields(
        "grid.edges.generator_link",
        &document.grid.edges.generator_link.extra,
        warnings,
    );
    warn_extra_fields(
        "grid.edges.load_link",
        &document.grid.edges.load_link.extra,
        warnings,
    );
    warn_extra_fields(
        "grid.edges.shunt_link",
        &document.grid.edges.shunt_link.extra,
        warnings,
    );
    warn_extra_fields("solution", &document.solution.extra, warnings);
    warn_extra_fields("solution.nodes", &document.solution.nodes.extra, warnings);
    warn_extra_fields("solution.edges", &document.solution.edges.extra, warnings);
    warn_extra_fields(
        "solution.edges.ac_line",
        &document.solution.edges.ac_line.extra,
        warnings,
    );
    warn_extra_fields(
        "solution.edges.transformer",
        &document.solution.edges.transformer.extra,
        warnings,
    );
    warn_extra_fields("metadata", &document.metadata.extra, warnings);
}

fn objective_warning(document: &Document) -> Option<String> {
    let calculated = document
        .grid
        .nodes
        .generator
        .iter()
        .zip(&document.solution.nodes.generator)
        .map(|(generator, solution)| generator.objective_at(solution.pg()))
        .sum::<f64>();
    let stated = document.metadata.objective;
    let tolerance = 1.0e-8 * stated.abs().max(calculated.abs()).max(1.0);
    (!calculated.is_finite() || !stated.is_finite() || (calculated - stated).abs() > tolerance)
        .then(|| {
            format!(
                "`metadata.objective` is {stated}, but the solved generator dispatch and costs evaluate to {calculated}"
            )
        })
}

struct NodeLinks {
    generators: Vec<BusId>,
    loads: Vec<BusId>,
    shunts: Vec<BusId>,
}

fn validate_document(document: &Document, bus_count: usize) -> Result<NodeLinks> {
    equal_len(
        "nodes.bus",
        "grid rows",
        bus_count,
        "solution rows",
        document.solution.nodes.bus.len(),
    )?;
    equal_len(
        "nodes.generator",
        "grid rows",
        document.grid.nodes.generator.len(),
        "solution rows",
        document.solution.nodes.generator.len(),
    )?;

    validate_edge_arrays(
        "grid.edges.ac_line",
        &document.grid.edges.ac_line.senders,
        &document.grid.edges.ac_line.receivers,
        document.grid.edges.ac_line.features.len(),
        bus_count,
    )?;
    validate_edge_arrays(
        "grid.edges.transformer",
        &document.grid.edges.transformer.senders,
        &document.grid.edges.transformer.receivers,
        document.grid.edges.transformer.features.len(),
        bus_count,
    )?;
    validate_solution_edges(
        "solution.edges.ac_line",
        &document.grid.edges.ac_line.senders,
        &document.grid.edges.ac_line.receivers,
        &document.solution.edges.ac_line,
        bus_count,
    )?;
    validate_solution_edges(
        "solution.edges.transformer",
        &document.grid.edges.transformer.senders,
        &document.grid.edges.transformer.receivers,
        &document.solution.edges.transformer,
        bus_count,
    )?;

    Ok(NodeLinks {
        generators: linked_buses(
            "grid.edges.generator_link",
            &document.grid.edges.generator_link,
            document.grid.nodes.generator.len(),
            bus_count,
        )?,
        loads: linked_buses(
            "grid.edges.load_link",
            &document.grid.edges.load_link,
            document.grid.nodes.load.len(),
            bus_count,
        )?,
        shunts: linked_buses(
            "grid.edges.shunt_link",
            &document.grid.edges.shunt_link,
            document.grid.nodes.shunt.len(),
            bus_count,
        )?,
    })
}

/// Parse one raw FullTop or N-1 OPFData JSON example as a solved network
/// snapshot. The reader does not assume a case name or fixed element counts.
pub fn parse_opfdata_json(content: &str) -> Result<Parsed> {
    let mut warnings = Vec::new();
    let network = parse_opfdata_source(Arc::new(content.to_owned()), None, &mut warnings)?;
    Ok(Parsed { network, warnings })
}

#[allow(clippy::too_many_lines)]
pub(crate) fn parse_opfdata_source(
    source: Arc<String>,
    name_hint: Option<&str>,
    warnings: &mut Vec<String>,
) -> Result<Network> {
    let document: Document = serde_json::from_str(&source)
        .map_err(|error| bad(format!("invalid OPFData schema: {error}")))?;
    let base = base_mva(&document.grid.context)?;
    let bus_count = document.grid.nodes.bus.len();
    let links = validate_document(&document, bus_count)?;
    warn_document_extras(&document, warnings);

    let buses = document
        .grid
        .nodes
        .bus
        .iter()
        .zip(&document.solution.nodes.bus)
        .enumerate()
        .map(|(index, (grid, solution))| {
            let mut bus = Bus::new(
                BusId(index + 1),
                bus_type(grid.bus_type(), index)?,
                grid.base_kv(),
            );
            bus.vmin = grid.vmin();
            bus.vmax = grid.vmax();
            bus.va = solution.va() * RAD_TO_DEG;
            bus.vm = solution.vm();
            Ok(bus)
        })
        .collect::<Result<Vec<_>>>()?;

    let generators = document
        .grid
        .nodes
        .generator
        .iter()
        .zip(&document.solution.nodes.generator)
        .zip(links.generators)
        .map(|((grid, solution), bus)| {
            let mut generator = Generator::new(bus);
            generator.mbase = grid.mbase();
            generator.pg = solution.pg() * base;
            generator.pmin = grid.pmin() * base;
            generator.pmax = grid.pmax() * base;
            generator.qg = solution.qg() * base;
            generator.qmin = grid.qmin() * base;
            generator.qmax = grid.qmax() * base;
            generator.vg = buses[bus.0 - 1].vm;
            generator.cost = Some(GenCost::new(
                2,
                0.0,
                0.0,
                cost_from_pu(grid.cost_coefficients(), 2, base),
            ));
            generator
        })
        .collect();

    let loads = document
        .grid
        .nodes
        .load
        .iter()
        .zip(links.loads)
        .map(|(row, bus)| Load::new(bus, row.pd() * base, row.qd() * base))
        .collect();

    let shunts = document
        .grid
        .nodes
        .shunt
        .iter()
        .zip(links.shunts)
        .map(|(row, bus)| Shunt::new(bus, row.gs() * base, row.bs() * base))
        .collect();

    let mut branches = Vec::with_capacity(
        document.grid.edges.ac_line.features.len() + document.grid.edges.transformer.features.len(),
    );
    for (((&from, &to), grid), solution) in document
        .grid
        .edges
        .ac_line
        .senders
        .iter()
        .zip(&document.grid.edges.ac_line.receivers)
        .zip(&document.grid.edges.ac_line.features)
        .zip(&document.solution.edges.ac_line.features)
    {
        let mut branch = Branch::new(BusId(from + 1), BusId(to + 1), grid.r(), grid.x());
        branch.b = grid.b_fr() + grid.b_to();
        branch.charging = Some(BranchCharging::new(0.0, grid.b_fr(), 0.0, grid.b_to()));
        branch.rate_a = grid.rate_a() * base;
        branch.rate_b = grid.rate_b() * base;
        branch.rate_c = grid.rate_c() * base;
        branch.angmin = grid.angmin() * RAD_TO_DEG;
        branch.angmax = grid.angmax() * RAD_TO_DEG;
        branch.solution = Some(solution.to_network(base));
        branches.push(branch);
    }
    for (((&from, &to), grid), solution) in document
        .grid
        .edges
        .transformer
        .senders
        .iter()
        .zip(&document.grid.edges.transformer.receivers)
        .zip(&document.grid.edges.transformer.features)
        .zip(&document.solution.edges.transformer.features)
    {
        let mut branch = Branch::new(BusId(from + 1), BusId(to + 1), grid.r(), grid.x());
        branch.rate_a = grid.rate_a() * base;
        branch.rate_b = grid.rate_b() * base;
        branch.rate_c = grid.rate_c() * base;
        branch.tap = grid.tap();
        branch.shift = grid.shift() * RAD_TO_DEG;
        branch.b = grid.b_fr() + grid.b_to();
        branch.charging = Some(BranchCharging::new(0.0, grid.b_fr(), 0.0, grid.b_to()));
        branch.angmin = grid.angmin() * RAD_TO_DEG;
        branch.angmax = grid.angmax() * RAD_TO_DEG;
        branch.solution = Some(solution.to_network(base));
        branches.push(branch);
    }

    if !document.grid.nodes.generator.is_empty() {
        warnings.push(
            "OPFData generator pg/qg/vg grid features are solver initial values; the canonical snapshot uses solved pg/qg and terminal-bus voltage, so initial values remain only in the retained source"
                .to_string(),
        );
    }
    warnings.push(format!(
        "OPFData does not carry original bus IDs/names, areas/zones, or base frequency; synthesized IDs 1..{bus_count}, area/zone 1, and {} Hz",
        crate::network::DEFAULT_BASE_FREQUENCY
    ));
    if let Some(warning) = objective_warning(&document) {
        warnings.push(warning);
    }

    let mut network = Network::new(name_hint.unwrap_or("opfdata"), base);
    network.buses = buses;
    network.loads = loads;
    network.shunts = shunts;
    network.branches = branches;
    network.generators = generators;
    network.source_format = SourceFormat::OpfDataJson;
    network.source = Some(source);
    Ok(network)
}
