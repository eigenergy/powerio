//! MATPOWER `.m` case file parser. Standard MATPOWER 7.x format.

pub mod document;
mod matlab;
mod tokens;
mod writer;

#[cfg(test)]
mod tests;

use std::path::Path;

pub use document::MatpowerDocument;
pub use writer::{write_matpower, write_matpower_file};

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
    // One pass builds the faithful source document (for lossless round-trip);
    // the typed structs are then derived from its located assignments, so the
    // file is never re-scanned per field and never comment-stripped whole.
    let source = document::build_document(content);

    let base_mva = source
        .assignment("baseMVA")
        .and_then(|raw| matlab::scalar_from_assignment(raw, "baseMVA").transpose())
        .transpose()?
        .ok_or(Error::MissingField("baseMVA"))?;

    let mut buses = parse_rows(
        source.assignment("bus").ok_or(Error::MissingField("bus"))?,
        "bus",
        Bus::from_row,
    )?;
    let branches = parse_rows(
        source
            .assignment("branch")
            .ok_or(Error::MissingField("branch"))?,
        "branch",
        Branch::from_row,
    )?;

    let gens = parse_gens(&source)?;
    let storage = parse_optional(&source, "storage", Storage::from_row)?;
    let dclines = parse_optional(&source, "dcline", DcLine::from_row)?;

    // Bus names live in a `{...}` cell array; pull them from the document
    // (which kept the quotes) and attach by position when the count matches.
    if let Some(raw) = source.assignment("bus_name") {
        let names = document::parse_string_cell(raw);
        if names.len() == buses.len() {
            for (bus, label) in buses.iter_mut().zip(names) {
                bus.name = Some(label);
            }
        }
    }

    Ok(MpcCase::new(name, base_mva, buses, branches)
        .with_gens(gens)
        .with_storage(storage)
        .with_dclines(dclines)
        .with_source(source))
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
fn parse_optional<T>(
    doc: &MatpowerDocument,
    field: &str,
    ctor: impl Fn(&[f64], usize) -> Result<T>,
) -> Result<Vec<T>> {
    match doc.assignment(field) {
        Some(raw) => parse_rows(raw, field, ctor),
        None => Ok(Vec::new()),
    }
}

/// Parse `mpc.gen` and fold in the active-power block of `mpc.gencost`.
/// Both are optional: a power-flow-only case has neither and gets no gens.
fn parse_gens(doc: &MatpowerDocument) -> Result<Vec<Generator>> {
    let Some(raw) = doc.assignment("gen") else {
        return Ok(Vec::new());
    };
    let mut gens = parse_rows(raw, "gen", Generator::from_row)?;

    // MATPOWER lays the active-power costs first, one row per generator and in
    // the same order; reactive-power costs (if any) follow in a second block.
    if let Some(craw) = doc.assignment("gencost") {
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
        // than cloning the `coeffs` Vec. The first `n` are active-power costs;
        // the next `n` (when present) are reactive-power costs, in gen order.
        let mut costs = costs.into_iter();
        for (gen, cost) in gens.iter_mut().zip(costs.by_ref().take(n)) {
            gen.cost = Some(cost);
        }
        for (gen, cost) in gens.iter_mut().zip(costs) {
            gen.reactive_cost = Some(cost);
        }
    }

    Ok(gens)
}
