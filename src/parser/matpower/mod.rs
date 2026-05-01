//! MATPOWER `.m` case file parser. Standard MATPOWER 7.x format.

mod matlab;
mod tokens;

#[cfg(test)]
mod tests;

use std::path::Path;

use crate::case::{Branch, Bus, MpcCase};
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

    Ok(MpcCase::new(name, base_mva, buses, branches))
}
