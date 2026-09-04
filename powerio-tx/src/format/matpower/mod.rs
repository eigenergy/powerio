//! MATPOWER `.m` case file parser. Standard MATPOWER 7.x format.

mod locate;
mod matlab;
mod rows;
mod tokens;
mod writer;

#[cfg(test)]
mod tests;

#[cfg(test)]
pub(crate) use writer::write_matpower;
pub(crate) use writer::write_matpower_conversion;

use self::locate::Assignment;
use crate::collect::Diagnostics;
use crate::network::{BalancedNetwork, BalancedNetworkTables, Generator, Hvdc, SourceFormat};
use crate::{Error, Result};

/// Owned-source entry used by the format hub: move the buffer straight into the
/// retained source (no copy) and take `name_hint` (e.g. the file stem) as the
/// network name.
///
/// A failure at a known record leaves that record's byte range on
/// `warnings` (see [`Diagnostics::record_span`]): a malformed or short matrix
/// row, a truncated matrix, a malformed scalar, or a cost table whose row
/// count disagrees with its element table. A missing field and a dangling
/// reference name no record.
pub(crate) fn parse_matpower_source(
    source: &str,
    name_hint: Option<&str>,
    warnings: &mut Diagnostics,
) -> Result<BalancedNetwork> {
    let name = name_hint
        .map(str::to_owned)
        .or_else(|| matpower_function_name(source).map(str::to_owned))
        .unwrap_or_else(|| "case".to_string());
    parse_matpower_named(source, &name, warnings)
}

fn matpower_function_name(source: &str) -> Option<&str> {
    for line in source.lines() {
        let line = line.trim_start();
        if !line.starts_with("function") {
            continue;
        }
        let Some((_, rhs)) = line.split_once('=') else {
            continue;
        };
        let rhs = rhs.trim_start();
        let end = rhs
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .unwrap_or(rhs.len());
        let starts_ident = rhs
            .as_bytes()
            .first()
            .is_some_and(|b| b.is_ascii_alphabetic() || *b == b'_');
        if end > 0 && starts_ident {
            return Some(&rhs[..end]);
        }
    }
    None
}

fn parse_matpower_named(
    source: &str,
    name: &str,
    warnings: &mut Diagnostics,
) -> Result<BalancedNetwork> {
    // Locate each assignment's text directly in `source` and build the network
    // from those borrowed slices in one pass; the typed model owns its data, so
    // the borrows end with `located` and the source Arc moves into the network.
    let net = {
        let located = locate::locate_assignments(source);
        build_case(
            name,
            |field| {
                located
                    .iter()
                    .find(|assignment| assignment.field == field)
                    .copied()
            },
            warnings,
        )?
    };
    // The other format readers validate references; the MATPOWER path must too,
    // or a duplicate or dangling bus id reaches `IndexedNetwork` as silently
    // collapsed aggregates (the dense bus-id map only debug-asserts uniqueness).
    net.check_references("MATPOWER")?;
    Ok(net)
}

/// Build a [`BalancedNetwork`] from a per-field assignment accessor `get`,
/// which returns the located `mpc.<field> = ...;` assignment for a field name.
/// MATPOWER folds demand and shunts onto the bus row; [`rows::bus_row`] splits
/// them back out into the hub's first-class [`Load`](crate::network::Load) /
/// [`Shunt`](crate::network::Shunt). The caller attaches the source afterward.
fn build_case<'a>(
    name: &str,
    get: impl Fn(&str) -> Option<Assignment<'a>>,
    warnings: &mut Diagnostics,
) -> Result<BalancedNetwork> {
    let base_mva = match get("baseMVA") {
        Some(assignment) => {
            // A malformed scalar is reported against its whole assignment.
            let (start, end) = assignment.range();
            warnings.enter_record(start, end);
            let value = matlab::scalar_from_assignment(assignment.text, "baseMVA")?;
            warnings.leave_record();
            value
        }
        None => None,
    }
    .ok_or(Error::MissingField("baseMVA"))?;

    let bus = get("bus").ok_or(Error::MissingField("bus"))?;
    let n_bus = estimate_rows(bus.text);
    let mut buses = Vec::with_capacity(n_bus);
    let mut loads = Vec::with_capacity(n_bus);
    let mut shunts = Vec::with_capacity(n_bus);
    matlab::for_each_matrix_row(bus.text, bus.start, "bus", warnings, |row, i| {
        let (bus, load, shunt) = rows::bus_row(row, i)?;
        buses.push(bus);
        if let Some(l) = load {
            loads.push(l);
        }
        if let Some(s) = shunt {
            shunts.push(s);
        }
        Ok(())
    })?;

    let branches = parse_rows(
        get("branch").ok_or(Error::MissingField("branch"))?,
        "branch",
        rows::branch_row,
        warnings,
    )?;

    let generators = parse_gens(&get, warnings)?;
    let storage = parse_optional(&get, "storage", rows::storage_row, warnings)?;
    let areas = parse_optional(&get, "areas", rows::area_row, warnings)?;
    let mut hvdc = parse_optional(&get, "dcline", rows::hvdc_row, warnings)?;
    attach_dcline_costs(&get, &mut hvdc, warnings)?;

    // Bus names live in a `{...}` cell array; pull them (quotes kept) and attach
    // by position when the count matches.
    if let Some(assignment) = get("bus_name") {
        let names = locate::parse_string_cell(assignment.text);
        if names.len() == buses.len() {
            for (bus, label) in buses.iter_mut().zip(names) {
                // An empty cell is an unnamed bus, not a bus named "".
                bus.name = (!label.is_empty()).then_some(label);
            }
        }
    }

    Ok(BalancedNetwork::from_tables(BalancedNetworkTables {
        name: name.to_string(),
        base_mva,
        base_frequency: crate::network::DEFAULT_BASE_FREQUENCY,
        geo: None,
        case_metadata: crate::network::CaseMetadata::default(),
        detailed_connectivity: None,
        generated_uids: std::collections::BTreeSet::default(),
        buses: buses.into(),
        loads: loads.into(),
        shunts: shunts.into(),
        static_var_compensators: Vec::new().into(),
        branches: branches.into(),
        switches: Vec::new().into(),
        generators: generators.into(),
        storage: storage.into(),
        hvdc: hvdc.into(),
        transformers_3w: Vec::new().into(),
        areas: areas.into(),
        solver: None,
        source_format: SourceFormat::Matpower,
    }))
}

/// A cheap upper-bound row count for an assignment (one `;` per row), used to
/// pre-size the typed vectors so parsing doesn't reallocate as it streams.
/// Capped: each `;` byte would otherwise pre-allocate a full element (~100
/// bytes), letting a small crafted file demand ~100x its size in memory up
/// front. Real cases sit far below the cap (largest vendored case: 13659
/// buses); beyond it the vectors just grow as rows actually parse.
fn estimate_rows(assignment: &str) -> usize {
    const MAX_ROW_HINT: usize = 1 << 20;
    assignment
        .bytes()
        .filter(|&b| b == b';')
        .count()
        .min(MAX_ROW_HINT)
}

/// Stream the rows of one assignment, building a typed `T` per row via `ctor`.
fn parse_rows<T>(
    assignment: Assignment<'_>,
    field: &str,
    ctor: impl Fn(&[f64], usize) -> Result<T>,
    warnings: &mut Diagnostics,
) -> Result<Vec<T>> {
    let mut out = Vec::with_capacity(estimate_rows(assignment.text));
    matlab::for_each_matrix_row(
        assignment.text,
        assignment.start,
        field,
        warnings,
        |row, i| {
            out.push(ctor(row, i)?);
            Ok(())
        },
    )?;
    Ok(out)
}

/// Like [`parse_rows`] but for an optional `mpc.<field>` block (empty if absent).
fn parse_optional<'a, T>(
    get: &impl Fn(&str) -> Option<Assignment<'a>>,
    field: &str,
    ctor: impl Fn(&[f64], usize) -> Result<T>,
    warnings: &mut Diagnostics,
) -> Result<Vec<T>> {
    match get(field) {
        Some(assignment) => parse_rows(assignment, field, ctor, warnings),
        None => Ok(Vec::new()),
    }
}

/// Fold `mpc.dclinecost` into the dcline rows. Same row layout as `mpc.gencost`,
/// one row per dcline in order (MATPOWER's `toggle_dcline` requires full
/// coverage, so unlike `gencost` there is no reactive second block). A line
/// with no usage cost is padded with an all-zero polynomial row, and a zero
/// cost prices the line exactly as no cost term at all, so a zero row reads
/// back as no cost and write-then-read stays stable for networks whose lines
/// carry none.
fn attach_dcline_costs<'a>(
    get: &impl Fn(&str) -> Option<Assignment<'a>>,
    hvdc: &mut [Hvdc],
    warnings: &mut Diagnostics,
) -> Result<()> {
    let Some(assignment) = get("dclinecost") else {
        return Ok(());
    };
    let costs = parse_rows(assignment, "dclinecost", rows::gencost_row, warnings)?;
    if costs.len() != hvdc.len() {
        let (start, end) = assignment.range();
        warnings.enter_record(start, end);
        return Err(Error::DcLineCostCountMismatch {
            dclines: hvdc.len(),
            dclinecost: costs.len(),
        });
    }
    for (line, cost) in hvdc.iter_mut().zip(costs) {
        let zero =
            cost.startup == 0.0 && cost.shutdown == 0.0 && cost.coeffs.iter().all(|c| *c == 0.0);
        if !zero {
            line.cost = Some(cost);
        }
    }
    Ok(())
}

/// Parse `mpc.gen` and fold in the active-power block of `mpc.gencost`.
/// Both are optional: a case with only power flow data has neither and gets no gens.
fn parse_gens<'a>(
    get: &impl Fn(&str) -> Option<Assignment<'a>>,
    warnings: &mut Diagnostics,
) -> Result<Vec<Generator>> {
    let Some(assignment) = get("gen") else {
        return Ok(Vec::new());
    };
    let mut gens = parse_rows(assignment, "gen", rows::gen_row, warnings)?;

    // MATPOWER lays the active-power costs first, one row per generator and in
    // the same order; reactive-power costs (if any) follow in a second block.
    if let Some(costs_assignment) = get("gencost") {
        let costs = parse_rows(costs_assignment, "gencost", rows::gencost_row, warnings)?;
        // Reject a count that is neither `n_gen` (active only) nor `2·n_gen`
        // (active + reactive). A per-row defect is reported as `ShortRow` first.
        let n = gens.len();
        if costs.len() != n && costs.len() != 2 * n {
            let (start, end) = costs_assignment.range();
            warnings.enter_record(start, end);
            return Err(Error::GenCostCountMismatch {
                gens: n,
                gencost: costs.len(),
            });
        }
        // The first `n` rows are the active-power costs in gen order; any
        // reactive-power second block is accepted but not retained.
        for (generator, cost) in gens.iter_mut().zip(costs) {
            generator.cost = Some(cost);
        }
    }

    Ok(gens)
}
