//! MATPOWER element map: each mapped row of `mpc.bus`, `mpc.branch`, and
//! `mpc.gen` reads back as its element (eigenergy/powerio#916).

mod common;

use common::element_map::{Kind, ReadBack, Written, check_format, check_retained_echo_has_no_map};
use powerio::RowRef;

/// The numeric rows of one `mpc.<table> = [ ... ];` matrix.
fn matrix(text: &str, table: &str) -> Vec<Vec<f64>> {
    let start = text
        .find(&format!("mpc.{table} = ["))
        .unwrap_or_else(|| panic!("no mpc.{table}"));
    let body = &text[start..];
    let body = &body[body.find('[').unwrap() + 1..body.find("];").unwrap()];
    body.lines()
        .map(|line| line.trim().trim_end_matches(';'))
        .filter(|line| !line.is_empty())
        .map(|line| {
            line.split_whitespace()
                .map(|value| value.parse().unwrap())
                .collect()
        })
        .collect()
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn read(written: &Written, kind: Kind, row: &RowRef) -> ReadBack {
    let text = written.text();
    let cell = |table: &str, column: usize| matrix(text, table)[row.index][column] as usize;
    match kind {
        Kind::Bus => {
            assert_eq!(row.table, "bus");
            let number = cell("bus", 0);
            assert_eq!(row.key.as_deref(), Some(number.to_string().as_str()));
            ReadBack::Bus(number)
        }
        Kind::Branch => {
            assert_eq!(row.table, "branch");
            ReadBack::Branch(cell("branch", 0), cell("branch", 1))
        }
        Kind::Generator => {
            assert_eq!(row.table, "gen");
            ReadBack::Generator(cell("gen", 0))
        }
    }
}

#[test]
fn matpower_rows_read_back_as_their_elements() {
    let written = check_format("matpower", read);
    check_retained_echo_has_no_map(&written, "matpower");
}
