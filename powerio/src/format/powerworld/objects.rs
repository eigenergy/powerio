//! Typed views over aux object types the transmission core does not model.
//!
//! Contingencies, limit sets, and rating set names matter to transmission
//! studies even though [`crate::network::Network`] does not model them. They
//! are retained losslessly by the generic layer ([`super::AuxFile`]); the
//! views here give them names and structure. Everything stays read only: the
//! data round trips through the retained source, untouched.

use super::aux::AuxFile;

/// One contingency from a `Contingency` DATA section, with the actions of its
/// `CTGElement` SUBDATA.
#[derive(Debug, Clone, PartialEq)]
pub struct Contingency {
    /// `CTGLabel`, the contingency's unique name.
    pub label: String,
    /// One entry per CTGElement line: the action string PowerWorld calls
    /// "WhoAmI + action", e.g. `BRANCH 2 1 1 OPEN`.
    pub actions: Vec<String>,
}

/// The contingencies of a parsed aux file, in file order.
///
/// Empty when the file carries no `Contingency` sections. Rows with no
/// `CTGLabel` field are skipped (a contingency without a name is not
/// addressable).
#[must_use]
pub fn contingencies(aux: &AuxFile) -> Vec<Contingency> {
    let mut out = Vec::new();
    for blk in aux.data_of("Contingency") {
        let Some(label_at) = blk.field_index("CTGLabel") else {
            continue;
        };
        for row in &blk.rows {
            let Some(label) = row.values.get(label_at) else {
                continue;
            };
            let actions = row
                .subdata
                .iter()
                .filter(|s| s.name.eq_ignore_ascii_case("CTGElement"))
                .flat_map(|s| s.lines.iter())
                .filter_map(|line| first_quoted(line))
                .map(str::to_string)
                .collect();
            out.push(Contingency {
                label: label.clone(),
                actions,
            });
        }
    }
    out
}

/// Name → row lookup for the per object-type rating set names
/// (`RatingSetNameBus`, `RatingSetNameBranch`, `RatingSetNameInterface`).
/// Returns `(set_number, name)` pairs in file order.
#[must_use]
pub fn rating_set_names(aux: &AuxFile, object_type: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    for blk in aux.data_of(object_type) {
        let (Some(num_at), Some(name_at)) = (
            blk.field_index("RatingSetNum"),
            blk.field_index("RatingSetName"),
        ) else {
            continue;
        };
        for row in &blk.rows {
            if let (Some(num), Some(name)) = (row.values.get(num_at), row.values.get(name_at)) {
                if let Ok(n) = num.trim().parse() {
                    out.push((n, name.clone()));
                }
            }
        }
    }
    out
}

/// The interior of the first `"..."` on a CTGElement line: its action string.
fn first_quoted(line: &str) -> Option<&str> {
    let start = line.find('"')? + 1;
    let end = start + line[start..].find('"')?;
    Some(line[start..end].trim())
}
