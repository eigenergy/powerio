//! MATPOWER `.m` case file parser. Standard MATPOWER 7.x format.

mod locate;
mod matlab;
mod rows;
mod tokens;
mod writer;

#[cfg(test)]
mod tests;

use std::path::Path;
use std::sync::Arc;

pub use writer::write_matpower;

use crate::network::{Generator, Network, SourceFormat};
use crate::{Error, Result};

/// Parse the MATPOWER case in `content` into a [`Network`].
pub fn parse_matpower(content: &str) -> Result<Network> {
    // The caller owns `content` as a borrow, so retention needs one copy.
    parse_matpower_named(Arc::new(content.to_owned()), "case")
}

/// Parse the MATPOWER case at `path`, using the file stem as the network name.
pub fn parse_matpower_file(path: impl AsRef<Path>) -> Result<Network> {
    let path = path.as_ref();
    let content = std::fs::read_to_string(path)?;
    let name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("case")
        .to_string();
    // We own the file buffer; move it straight into the retained source — no
    // second copy of the whole file.
    parse_matpower_named(Arc::new(content), &name)
}

/// Owned-source entry used by the format hub: move the buffer straight into the
/// retained source (no copy) and take `name_hint` (e.g. the file stem) as the
/// network name.
pub(crate) fn parse_matpower_source(
    source: Arc<String>,
    name_hint: Option<&str>,
) -> Result<Network> {
    parse_matpower_named(source, name_hint.unwrap_or("case"))
}

fn parse_matpower_named(source: Arc<String>, name: &str) -> Result<Network> {
    // Locate each assignment's text directly in `source` and build the network
    // from those borrowed slices in one pass; the typed model owns its data, so
    // the borrows end with `located` and the source Arc moves into the network.
    let mut net = {
        let located = locate::locate_assignments(&source);
        build_case(name, |field| {
            located
                .iter()
                .find(|(f, _)| *f == field)
                .map(|(_, full)| *full)
        })?
    };
    net.source = Some(source);
    // The other format readers validate references; the MATPOWER path must too,
    // or a duplicate or dangling bus id reaches `IndexedNetwork` as silently
    // collapsed aggregates (the dense bus-id map only debug-asserts uniqueness).
    net.check_references("MATPOWER")?;
    Ok(net)
}

/// Build a [`Network`] from a per-field assignment-text accessor `get`, which
/// returns the raw `mpc.<field> = …;` text for a field name. MATPOWER folds
/// demand and shunts onto the bus row; [`rows::bus_row`] splits them back out
/// into the hub's first-class [`Load`](crate::network::Load) /
/// [`Shunt`](crate::network::Shunt). The caller attaches the source afterward.
fn build_case<'a>(name: &str, get: impl Fn(&str) -> Option<&'a str>) -> Result<Network> {
    let base_mva = get("baseMVA")
        .and_then(|raw| matlab::scalar_from_assignment(raw, "baseMVA").transpose())
        .transpose()?
        .ok_or(Error::MissingField("baseMVA"))?;

    let bus_raw = get("bus").ok_or(Error::MissingField("bus"))?;
    let n_bus = estimate_rows(bus_raw);
    let mut buses = Vec::with_capacity(n_bus);
    let mut loads = Vec::with_capacity(n_bus);
    let mut shunts = Vec::with_capacity(n_bus);
    matlab::for_each_matrix_row(bus_raw, "bus", |row, i| {
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
    )?;

    let generators = parse_gens(&get)?;
    let storage = parse_optional(&get, "storage", rows::storage_row)?;
    let hvdc = parse_optional(&get, "dcline", rows::hvdc_row)?;

    // Bus names live in a `{...}` cell array; pull them (quotes kept) and attach
    // by position when the count matches.
    if let Some(raw) = get("bus_name") {
        let names = locate::parse_string_cell(raw);
        if names.len() == buses.len() {
            for (bus, label) in buses.iter_mut().zip(names) {
                bus.name = Some(label);
            }
        }
    }

    Ok(Network {
        name: name.to_string(),
        base_mva,
        buses,
        loads,
        shunts,
        branches,
        generators,
        storage,
        hvdc,
        source_format: SourceFormat::Matpower,
        source: None,
    })
}

/// A cheap upper-bound row count for an assignment (one `;` per row), used to
/// pre-size the typed vectors so parsing doesn't reallocate as it streams.
fn estimate_rows(assignment: &str) -> usize {
    assignment.bytes().filter(|&b| b == b';').count()
}

/// Stream the rows of one assignment, building a typed `T` per row via `ctor`.
fn parse_rows<T>(
    assignment: &str,
    field: &str,
    ctor: impl Fn(&[f64], usize) -> Result<T>,
) -> Result<Vec<T>> {
    let mut out = Vec::with_capacity(estimate_rows(assignment));
    matlab::for_each_matrix_row(assignment, field, |row, i| {
        out.push(ctor(row, i)?);
        Ok(())
    })?;
    Ok(out)
}

/// Like [`parse_rows`] but for an optional `mpc.<field>` block (empty if absent).
fn parse_optional<'a, T>(
    get: &impl Fn(&str) -> Option<&'a str>,
    field: &str,
    ctor: impl Fn(&[f64], usize) -> Result<T>,
) -> Result<Vec<T>> {
    match get(field) {
        Some(raw) => parse_rows(raw, field, ctor),
        None => Ok(Vec::new()),
    }
}

/// Parse `mpc.gen` and fold in the active-power block of `mpc.gencost`.
/// Both are optional: a case with only power flow data has neither and gets no gens.
fn parse_gens<'a>(get: &impl Fn(&str) -> Option<&'a str>) -> Result<Vec<Generator>> {
    let Some(raw) = get("gen") else {
        return Ok(Vec::new());
    };
    let mut gens = parse_rows(raw, "gen", rows::gen_row)?;

    // MATPOWER lays the active-power costs first, one row per generator and in
    // the same order; reactive-power costs (if any) follow in a second block.
    if let Some(craw) = get("gencost") {
        let costs = parse_rows(craw, "gencost", rows::gencost_row)?;
        // Reject a count that is neither `n_gen` (active only) nor `2·n_gen`
        // (active + reactive). A per-row defect surfaces as `ShortRow` first.
        let n = gens.len();
        if costs.len() != n && costs.len() != 2 * n {
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
