//! MATPOWER `.m` case file parser. Standard MATPOWER 7.x format.

mod locate;
mod matlab;
mod tokens;
mod writer;

#[cfg(test)]
mod tests;

use std::path::Path;

pub use writer::write_matpower;

use crate::case::{Branch, Bus, DcLine, GenCost, Generator, MpcCase, Storage};
use crate::{Error, Result};

/// Parse the MATPOWER case in `content` and return a domain `MpcCase`.
pub fn parse_matpower(content: &str) -> Result<MpcCase> {
    parse_matpower_named(content, "case")
}

/// Parse the MATPOWER case at `path`, using the file stem as `MpcCase::name`.
pub fn parse_matpower_file(path: impl AsRef<Path>) -> Result<MpcCase> {
    let path = path.as_ref();
    let content = std::fs::read_to_string(path)?;
    let name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("case")
        .to_string();
    parse_matpower_named(&content, &name)
}

fn parse_matpower_named(content: &str, name: &str) -> Result<MpcCase> {
    // Locate each assignment's text directly in `content` and build the typed
    // case from those borrowed slices in one pass. The case keeps the original
    // source text so the writer can echo it for a byte-exact round-trip.
    let located = locate::locate_assignments(content);
    let case = build_case(name, |field| {
        located
            .iter()
            .find(|(f, _)| *f == field)
            .map(|(_, full)| *full)
    })?;
    Ok(case.with_source(content))
}

/// Build the typed [`MpcCase`] from a per-field assignment-text accessor `get`,
/// which returns the raw `mpc.<field> = …;` text for a field name. The caller
/// attaches the source text afterward so the case can round-trip.
fn build_case<'a>(name: &str, get: impl Fn(&str) -> Option<&'a str>) -> Result<MpcCase> {
    let base_mva = get("baseMVA")
        .and_then(|raw| matlab::scalar_from_assignment(raw, "baseMVA").transpose())
        .transpose()?
        .ok_or(Error::MissingField("baseMVA"))?;

    let mut buses = parse_rows(
        get("bus").ok_or(Error::MissingField("bus"))?,
        "bus",
        Bus::from_row,
    )?;
    let branches = parse_rows(
        get("branch").ok_or(Error::MissingField("branch"))?,
        "branch",
        Branch::from_row,
    )?;

    let gens = parse_gens(&get)?;
    let storage = parse_optional(&get, "storage", Storage::from_row)?;
    let dclines = parse_optional(&get, "dcline", DcLine::from_row)?;

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

    Ok(MpcCase::new(name, base_mva, buses, branches)
        .with_gens(gens)
        .with_storage(storage)
        .with_dclines(dclines))
}

/// Stream the rows of one assignment, building a typed `T` per row via `ctor`.
fn parse_rows<T>(
    assignment: &str,
    field: &str,
    ctor: impl Fn(&[f64], usize) -> Result<T>,
) -> Result<Vec<T>> {
    let mut out = Vec::new();
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
/// Both are optional: a power-flow-only case has neither and gets no gens.
fn parse_gens<'a>(get: &impl Fn(&str) -> Option<&'a str>) -> Result<Vec<Generator>> {
    let Some(raw) = get("gen") else {
        return Ok(Vec::new());
    };
    let mut gens = parse_rows(raw, "gen", Generator::from_row)?;

    // MATPOWER lays the active-power costs first, one row per generator and in
    // the same order; reactive-power costs (if any) follow in a second block.
    if let Some(craw) = get("gencost") {
        let costs = parse_rows(craw, "gencost", GenCost::from_row)?;
        // Reject a count that is neither `n_gen` (active only) nor `2·n_gen`
        // (active + reactive). A per-row defect (e.g. a short row) surfaces as
        // `ShortRow` from the parse above before this count check runs.
        let n = gens.len();
        if costs.len() != n && costs.len() != 2 * n {
            return Err(Error::GenCostCountMismatch {
                gens: n,
                gencost: costs.len(),
            });
        }
        // `costs` is consumed here, so move each row into its generator rather
        // than cloning the `coeffs` Vec. The first `n` rows are the active-power
        // costs in gen order; any reactive-power second block is accepted by the
        // count check above but not retained (nothing downstream consumes it).
        for (gen, cost) in gens.iter_mut().zip(costs) {
            gen.cost = Some(cost);
        }
    }

    Ok(gens)
}
