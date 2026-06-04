//! MATPOWER `.m` case file parser. Standard MATPOWER 7.x format.

mod matlab;
mod tokens;

#[cfg(test)]
mod tests;

use std::path::Path;

use crate::case::{Branch, Bus, GenCost, Generator, MpcCase, Storage};
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
    let stripped = tokens::strip_comments(content);

    let base_mva = matlab::find_scalar(&stripped, "baseMVA")?
        .ok_or(Error::MissingField("baseMVA"))?;

    let bus_rows = matlab::find_matrix(&stripped, "bus")?
        .ok_or(Error::MissingField("bus"))?;
    let branch_rows = matlab::find_matrix(&stripped, "branch")?
        .ok_or(Error::MissingField("branch"))?;

    let buses: Vec<Bus> = bus_rows
        .iter()
        .enumerate()
        .map(|(i, row)| Bus::from_row(row, i))
        .collect::<Result<_>>()?;

    let branches: Vec<Branch> = branch_rows
        .iter()
        .enumerate()
        .map(|(i, row)| Branch::from_row(row, i))
        .collect::<Result<_>>()?;

    let gens = parse_gens(&stripped)?;
    let storage = parse_storage(&stripped)?;

    Ok(MpcCase::new(name, base_mva, buses, branches)
        .with_gens(gens)
        .with_storage(storage))
}

/// Parse the optional `mpc.storage` block. A case without storage gets none.
fn parse_storage(stripped: &str) -> Result<Vec<Storage>> {
    let Some(rows) = matlab::find_matrix(stripped, "storage")? else {
        return Ok(Vec::new());
    };
    rows.iter()
        .enumerate()
        .map(|(i, row)| Storage::from_row(row, i))
        .collect()
}

/// Parse `mpc.gen` and fold in the active-power block of `mpc.gencost`.
/// Both are optional: a power-flow-only case has neither and gets no gens.
fn parse_gens(stripped: &str) -> Result<Vec<Generator>> {
    let gen_rows = match matlab::find_matrix(stripped, "gen")? {
        Some(rows) => rows,
        None => return Ok(Vec::new()),
    };

    let mut gens: Vec<Generator> = gen_rows
        .iter()
        .enumerate()
        .map(|(i, row)| Generator::from_row(row, i))
        .collect::<Result<_>>()?;

    // MATPOWER lays the active-power costs first, one row per generator and in
    // the same order; reactive-power costs (if any) follow and are ignored.
    if let Some(cost_rows) = matlab::find_matrix(stripped, "gencost")? {
        // Reject a count that is neither `n_gen` (active only) nor `2·n_gen`
        // (active + reactive) so a truncated/garbled block is caught here
        // instead of silently truncating via `zip` and surfacing later as a
        // misleading per-generator `MissingGenCost`.
        if cost_rows.len() != gens.len() && cost_rows.len() != 2 * gens.len() {
            return Err(Error::GenCostCountMismatch {
                gens: gens.len(),
                gencost: cost_rows.len(),
            });
        }
        for (i, (gen, row)) in gens.iter_mut().zip(cost_rows.iter()).enumerate() {
            gen.cost = Some(GenCost::from_row(row, i)?);
        }
    }

    Ok(gens)
}
