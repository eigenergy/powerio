//! The PowerIO IR reference page and the generated schema name the same
//! fields.
//!
//! Each field table on the page follows a line that names the schema
//! definitions it documents. Both directions are checked: every field the
//! page names exists in each named definition, and every property of each
//! named definition appears in the table. Every structural type name in the
//! schema's value discriminator must appear on the page, and the definition
//! each value arm refers to must have a table.

use std::collections::BTreeSet;

const PAGE: &str = include_str!("../../docs/src/ir-reference.md");
const SCHEMA_PATH: &str = concat!(
    "docs/schema/pio-module/",
    env!("CARGO_PKG_VERSION"),
    "/schema.json"
);
const SCHEMA: &str = include_str!(concat!(
    "../../docs/schema/pio-module/",
    env!("CARGO_PKG_VERSION"),
    "/schema.json"
));

/// A line beginning with this text names the definitions of the next table.
const MARKER: &str = "Schema definition";

struct Table {
    definitions: Vec<String>,
    fields: Vec<String>,
    /// The page line of the marker, for reporting.
    line: usize,
}

fn backticked(text: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find('`') {
        let after = &rest[open + 1..];
        let Some(close) = after.find('`') else {
            break;
        };
        names.push(after[..close].to_owned());
        rest = &after[close + 1..];
    }
    names
}

fn first_cell(row: &str) -> &str {
    row.trim_start_matches('|')
        .split('|')
        .next()
        .unwrap_or("")
        .trim()
}

fn tables() -> Vec<Table> {
    let mut tables = Vec::new();
    let mut pending: Option<(Vec<String>, usize)> = None;
    let mut current: Option<Table> = None;
    for (index, line) in PAGE.lines().enumerate() {
        let number = index + 1;
        if let Some(rest) = line.strip_prefix(MARKER) {
            assert!(
                pending.is_none(),
                "line {number}: the previous marker has no table"
            );
            let definitions = backticked(rest);
            assert!(
                !definitions.is_empty(),
                "line {number}: the marker names no definition"
            );
            pending = Some((definitions, number));
            continue;
        }
        if line.starts_with('|') {
            let cell = first_cell(line);
            if let Some(table) = &mut current {
                if cell.starts_with("---") {
                    continue;
                }
                let names = backticked(cell);
                assert!(!names.is_empty(), "line {number}: the row names no field");
                table.fields.extend(names);
            } else if let Some((definitions, marker_line)) = pending.take() {
                assert_eq!(
                    cell, "field",
                    "line {number}: a field table starts with a `field` column"
                );
                current = Some(Table {
                    definitions,
                    fields: Vec::new(),
                    line: marker_line,
                });
            }
            continue;
        }
        if let Some(table) = current.take() {
            tables.push(table);
        }
    }
    assert!(pending.is_none(), "the last marker has no table");
    if let Some(table) = current.take() {
        tables.push(table);
    }
    tables
}

#[test]
fn every_documented_field_exists_and_every_schema_field_is_documented() {
    let schema: serde_json::Value = serde_json::from_str(SCHEMA).unwrap();
    let definitions = schema["$defs"].as_object().unwrap();
    let mut documented = BTreeSet::new();
    let mut problems = Vec::new();

    let tables = tables();
    assert!(
        tables.len() > 50,
        "only {} field tables found",
        tables.len()
    );
    for table in &tables {
        let page_fields: BTreeSet<&str> = table.fields.iter().map(String::as_str).collect();
        if page_fields.len() != table.fields.len() {
            problems.push(format!("line {}: a field is listed twice", table.line));
        }
        for definition in &table.definitions {
            let Some(properties) = definitions
                .get(definition)
                .and_then(|entry| entry["properties"].as_object())
            else {
                problems.push(format!(
                    "line {}: `{definition}` is not an object definition of the schema",
                    table.line
                ));
                continue;
            };
            if !documented.insert(definition.clone()) {
                problems.push(format!(
                    "line {}: `{definition}` has more than one table",
                    table.line
                ));
            }
            let schema_fields: BTreeSet<&str> = properties.keys().map(String::as_str).collect();
            for field in page_fields.difference(&schema_fields) {
                problems.push(format!(
                    "line {}: `{definition}` has no field `{field}`",
                    table.line
                ));
            }
            for field in schema_fields.difference(&page_fields) {
                problems.push(format!(
                    "line {}: `{definition}.{field}` is not documented",
                    table.line
                ));
            }
        }
    }

    for arm in definitions["StoredValue"]["oneOf"].as_array().unwrap() {
        let type_name = arm["properties"]["type"]["const"].as_str().unwrap();
        let reference = arm["properties"]["data"]["$ref"].as_str().unwrap();
        let definition = reference.rsplit('/').next().unwrap();
        if !documented.contains(definition) {
            problems.push(format!(
                "`{definition}`, the data of `{type_name}`, has no field table"
            ));
        }
        if !PAGE.contains(&format!("`{type_name}`")) {
            problems.push(format!("`{type_name}` is not named on the page"));
        }
    }

    assert!(
        problems.is_empty(),
        "docs/src/ir-reference.md disagrees with {SCHEMA_PATH}:\n{}",
        problems.join("\n")
    );
}
