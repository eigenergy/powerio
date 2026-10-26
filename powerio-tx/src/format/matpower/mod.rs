//! MATPOWER `.m` case file parser. Standard MATPOWER 7.x format.

mod conversions;
mod locate;
mod matlab;
mod rows;
mod tokens;
mod writer;

#[cfg(test)]
mod conversion_tests;
#[cfg(test)]
mod tests;

#[cfg(test)]
pub(crate) use writer::write_matpower;
pub(crate) use writer::write_matpower_conversion;

use self::locate::Assignment;
use crate::collect::Diagnostics;
use crate::diagnostics::codes::READ_MATPOWER_INDEXED_ASSIGNMENT;
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
    let located = locate::locate_assignments(source);
    let mut net = build_case(
        name,
        |field| {
            located
                .iter()
                .find(|assignment| assignment.field == field)
                .copied()
        },
        warnings,
    )?;
    if !indexed_assignment_spans(source).is_empty()
        && !conversions::apply(source, &located, &mut net)
    {
        warn_indexed_assignments(source, warnings);
    }
    // The other format readers validate references; the MATPOWER path must too,
    // or a duplicate or dangling bus id reaches `IndexedNetwork` as silently
    // collapsed aggregates (the dense bus-id map only debug-asserts uniqueness).
    net.check_references("MATPOWER")?;
    Ok(net)
}

/// Warn when the bounded conversion profile cannot interpret every statement.
/// No conversions are applied to an ambiguous case; the original literal values
/// remain available, accompanied by one diagnostic per indexed assignment.
fn warn_indexed_assignments(source: &str, warnings: &mut Diagnostics) {
    // One diagnostic per assignment, up to a bound; a crafted file of millions
    // of assignments must not exceed the module's diagnostic cap.
    const LISTED: usize = 1_000;
    let spans = indexed_assignment_spans(source);
    if spans.len() > LISTED {
        warnings.push(
            &READ_MATPOWER_INDEXED_ASSIGNMENT,
            format!(
                "{} indexed assignments to mpc fields are not evaluated; the first {LISTED} are listed; no post-literal conversions were applied",
                spans.len()
            ),
        );
    }
    for (start, end, field) in spans.into_iter().take(LISTED) {
        warnings.enter_record(start, end);
        warnings.push(
            &READ_MATPOWER_INDEXED_ASSIGNMENT,
            format!(
                "indexed assignment to mpc.{field} is not evaluated; no post-literal conversions were applied; parsed values may differ from MATLAB execution"
            ),
        );
        warnings.leave_record();
    }
}

/// Lexically find indexed assignments while retaining offsets into the original source.
fn indexed_assignment_spans(source: &str) -> Vec<(usize, usize, &str)> {
    let b = source.as_bytes();
    let mut out = Vec::new();
    let (mut i, mut line_start) = (0, 0);
    // Whether only whitespace, comments, or continuations follow the current
    // statement's start; re-trimming the statement text at every `mpc` token
    // would be quadratic.
    let mut fresh = true;
    let mut block_depth = 0usize;
    let mut quote = None;
    let (mut p, mut square, mut brace) = (0usize, 0usize, 0usize);

    while i < b.len() {
        if i == line_start {
            let mut first = i;
            while first < b.len() && matches!(b[first], b' ' | b'\t' | b'\r') {
                first += 1;
            }
            let end = b[first..]
                .iter()
                .position(|x| *x == b'\n')
                .map_or(b.len(), |n| first + n);

            let delimiter = source[first..end].trim();
            if block_depth > 0 {
                match delimiter {
                    "%{" => block_depth += 1,
                    "%}" => block_depth -= 1,
                    _ => {}
                }
                i = end;
                if i < b.len() {
                    i += 1;
                    line_start = i;
                    if p == 0 && square == 0 && brace == 0 {
                        fresh = true;
                    }
                }
                continue;
            }
            if delimiter == "%{" {
                block_depth = 1;
                i = end;
                if i < b.len() {
                    i += 1;
                    line_start = i;
                }
                continue;
            }
        }

        let c = b[i];
        if let Some(q) = quote {
            if c == q {
                if b.get(i + 1) == Some(&q) {
                    i += 2;
                    continue;
                }
                quote = None;
            }
            i += 1;
            continue;
        }
        if c == b'"' || (c == b'\'' && !is_transpose_quote(b, i, square > 0 || brace > 0)) {
            quote = Some(c);
            fresh = false;
            i += 1;
            continue;
        }
        if let Some(end) = matlab_continuation_end(b, i) {
            // MATLAB joins the next line; a statement that began before the
            // ellipsis is still at its start.
            i = end;
            line_start = i;
            continue;
        }
        if c == b'%' {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }

        if fresh
            && mpc_field_start(b, i).is_some()
            && let Some((lhs_end, field)) = indexed_assignment_at(source, i)
        {
            out.push((i, matlab_statement_end(source, i).max(lhs_end), field));
        }

        match c {
            b'(' => p += 1,
            b')' => p = p.saturating_sub(1),
            b'[' => square += 1,
            b']' => square = square.saturating_sub(1),
            b'{' => brace += 1,
            b'}' => brace = brace.saturating_sub(1),
            _ => {}
        }
        if matches!(c, b',' | b';' | b'\n') && p == 0 && square == 0 && brace == 0 {
            fresh = true;
        } else if !c.is_ascii_whitespace() {
            fresh = false;
        }
        if c == b'\n' {
            line_start = i + 1;
        }
        i += 1;
    }
    out
}

fn is_transpose_quote(b: &[u8], at: usize, in_array: bool) -> bool {
    // Within matrix/cell construction whitespace separates elements. A quote
    // after that separator opens a character literal, not a transpose of the
    // previous element (which might itself be a quoted literal).
    if in_array && at > 0 && b[at - 1].is_ascii_whitespace() {
        return false;
    }
    let mut j = at;
    while j > 0 && b[j - 1].is_ascii_whitespace() {
        j -= 1;
    }
    // The dot in `A.'` belongs to the nonconjugate transpose operator.
    if j > 0 && b[j - 1] == b'.' {
        j -= 1;
    }
    j > 0
        && matches!(
            b[j - 1],
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_' | b')' | b']' | b'}' | b'\''
        )
}

/// MATLAB ignores everything after an unquoted ellipsis on its physical line.
/// Return the original offset after that line, without inspecting its contents.
fn matlab_continuation_end(b: &[u8], at: usize) -> Option<usize> {
    (b.get(at..at + 3) == Some(b"...")).then(|| {
        b[at..]
            .iter()
            .position(|c| *c == b'\n')
            .map_or(b.len(), |offset| at + offset + 1)
    })
}

fn skip_matlab_assignment_trivia(b: &[u8], mut at: usize) -> usize {
    loop {
        while at < b.len() && b[at].is_ascii_whitespace() {
            at += 1;
        }
        if let Some(end) = matlab_continuation_end(b, at) {
            at = end;
        } else {
            return at;
        }
    }
}

fn mpc_field_start(b: &[u8], start: usize) -> Option<usize> {
    if b.get(start..start + 3) != Some(b"mpc") {
        return None;
    }
    let dot = skip_matlab_assignment_trivia(b, start + 3);
    if b.get(dot) != Some(&b'.') {
        return None;
    }
    Some(skip_matlab_assignment_trivia(b, dot + 1))
}

fn indexed_assignment_at(source: &str, start: usize) -> Option<(usize, &str)> {
    let b = source.as_bytes();
    let mut i = mpc_field_start(b, start)?;
    let field_start = i;
    while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
        i += 1;
    }
    if i == field_start {
        return None;
    }
    let field = &source[field_start..i];
    i = skip_matlab_assignment_trivia(b, i);
    if b.get(i) != Some(&b'(') {
        return None;
    }
    i += 1;
    let mut depth = 0usize;
    let (mut square, mut brace) = (0usize, 0usize);
    let mut quote = None;

    while i < b.len() {
        let c = b[i];
        if let Some(q) = quote {
            if c == q {
                if b.get(i + 1) == Some(&q) {
                    i += 2;
                    continue;
                }
                quote = None;
            }
            i += 1;
            continue;
        }
        if c == b'"' || (c == b'\'' && !is_transpose_quote(b, i, square > 0 || brace > 0)) {
            quote = Some(c);
            i += 1;
            continue;
        }
        if let Some(end) = matlab_continuation_end(b, i) {
            i = end;
            continue;
        }
        if c == b'%' {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }

        match c {
            // MATLAB needs `...` to break a line inside parentheses, so the
            // index ends here; scanning on would be quadratic across a file.
            b'\n' if square == 0 && brace == 0 => return None,
            b'[' => square += 1,
            b']' => square = square.saturating_sub(1),
            b'{' => brace += 1,
            b'}' => brace = brace.saturating_sub(1),
            b'(' => depth += 1,
            b')' if depth == 0 => {
                let end = i + 1;
                i = skip_matlab_assignment_trivia(b, end);
                // Octave's `mpc.bus(:, 3) /= 1e3` mutates the same element.
                if matches!(b.get(i), Some(b'+' | b'-' | b'*' | b'/' | b'\\' | b'^'))
                    && b.get(i + 1) == Some(&b'=')
                    && b.get(i + 2) != Some(&b'=')
                {
                    return Some((end, field));
                }
                if b.get(i) == Some(&b'=')
                    && b.get(i + 1) != Some(&b'=')
                    && !matches!(
                        i.checked_sub(1).and_then(|n| b.get(n)),
                        Some(b'=' | b'<' | b'>' | b'~')
                    )
                {
                    return Some((end, field));
                }
                return None;
            }
            b')' => depth -= 1,
            _ => {}
        }
        i += 1;
    }
    None
}

fn matlab_statement_end(source: &str, start: usize) -> usize {
    let b = source.as_bytes();
    let mut i = start;
    let (mut p, mut square, mut brace) = (0usize, 0usize, 0usize);
    let mut quote = None;

    while i < b.len() {
        let c = b[i];
        if let Some(q) = quote {
            if c == q {
                if b.get(i + 1) == Some(&q) {
                    i += 2;
                    continue;
                }
                quote = None;
            }
            i += 1;
            continue;
        }
        if c == b'"' || (c == b'\'' && !is_transpose_quote(b, i, square > 0 || brace > 0)) {
            quote = Some(c);
            i += 1;
            continue;
        }
        if let Some(end) = matlab_continuation_end(b, i) {
            i = end;
            continue;
        }
        if c == b'%' {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }

        match c {
            b'(' => p += 1,
            b')' => p = p.saturating_sub(1),
            b'[' => square += 1,
            b']' => square = square.saturating_sub(1),
            b'{' => brace += 1,
            b'}' => brace = brace.saturating_sub(1),
            b',' | b';' if p == 0 && square == 0 && brace == 0 => return i + 1,
            b'\n' if p == 0 && square == 0 && brace == 0 => return i,
            _ => {}
        }
        i += 1;
    }
    source.len()
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
        generated_uids: std::sync::Arc::default(),
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
