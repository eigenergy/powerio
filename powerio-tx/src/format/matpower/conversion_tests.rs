use crate::collect::Diagnostics;
use crate::network::BalancedNetwork;

const LITERAL: &str = "mpc.baseMVA = 10;\n\
mpc.bus = [7 3 100 60 3 4 1 1 0 12.66 1 1.1 0.9; 1 1 200 40 0 0 1 1 0 20 1 1.1 0.9];\n\
mpc.branch = [7 1 0.0922 0.047 0.02 9 8 7 0 0 1 -360 360; 1 7 0.2 0.3 0.04 6 5 4 0 0 0 -360 360];\n";
const SCALE: &str = "mpc.bus(:, [3 4]) = mpc.bus(:, [3 4]) / 1e3;";

fn parse(source: &str) -> (BalancedNetwork, Diagnostics) {
    let mut warnings = Diagnostics::new();
    warnings.locate_in(powerio_core::SourceId::new("/input").unwrap(), 0);
    let net = super::parse_matpower_source(source, None, &mut warnings).unwrap();
    (net, warnings)
}

fn close(actual: f64, expected: f64) {
    assert!((actual - expected).abs() < 1e-13, "{actual} != {expected}");
}

#[test]
fn scaling_variants_preserve_nonselected_columns() {
    for conversion in [
        SCALE,
        "mpc.bus(:, [4,3]) = mpc.bus(:, [4 3]) .* (1 / 1000);",
        "scale = 1000; mpc.bus(:, [3,4]) = mpc.bus(:, [3 4]) ./ scale;",
        "mpc.bus(:, 3) = mpc.bus(:, 3) / 1000; mpc.bus(:, 4) = mpc.bus(:, 4) / 1E+3;",
        "mpc.bus(:, ... ignored ' ] )\r\n [3 4]) ... ignored ' ] )\r\n = mpc.bus(:, [3 4]) ./ 1e3;",
        "[~,~,~,~,~,~,active,reactive] = idx_bus(); mpc.bus(:, [active reactive]) = mpc.bus(:, [active reactive]) / 1000;",
        "%{\n%{\nignored = evil();\n%}\nmpc.bus(:, 3) = 0;\n%}\nmpc.bus(:,[3 4]) = mpc.bus(:,[3 4]) / 1e3;",
    ] {
        let (net, warnings) = parse(&format!("{LITERAL}{conversion}\n"));
        assert!(warnings.is_empty(), "{conversion}: {:?}", warnings.lines());
        close(net.loads().iter().map(|load| load.p).sum(), 0.3);
        close(net.loads().iter().map(|load| load.q).sum(), 0.1);
        let original = parse(LITERAL).0;
        assert_eq!(net.buses(), original.buses());
        assert_eq!(net.shunts(), original.shunts());
        assert_eq!(net.branches(), original.branches());
    }
}

#[test]
fn voltage_base_is_row_position_and_impedance_includes_open_branches() {
    for power in ["^", ".^"] {
        let conversion = format!(
            "[~, ~, r, x] = idx_brch;\n\
            voltage = mpc.bus(1, 10) * 1e3; apparent = mpc.baseMVA * 1e6;\n\
            z = voltage{power}2 / apparent;\n\
            mpc.branch(:, [r x]) = mpc.branch(:, [r x]) ./ z;"
        );
        let (net, warnings) = parse(&format!("{LITERAL}{conversion}"));
        assert!(warnings.is_empty(), "{:?}", warnings.lines());
        let z = 12.66_f64.powi(2) / 10.0;
        close(net.branches()[0].r, 0.0922 / z);
        close(net.branches()[1].x, 0.3 / z);
        close(net.branches()[1].b, 0.04);
        close(net.branches()[0].rate_a, 9.0);
        assert!(!net.branches()[1].in_service);
    }
}

#[test]
fn scalar_redefinitions_and_repeated_conversions_follow_source_order() {
    let conversion = "factor = 1000;\n\
        mpc.bus(:, 3) = mpc.bus(:, 3) / factor;\n\
        factor = 500;\n\
        mpc.bus(:, 4) = mpc.bus(:, 4) / factor;\n\
        mpc.bus(:, 3) = mpc.bus(:, 3) * 2;";
    let (net, warnings) = parse(&format!("{LITERAL}{conversion}"));
    assert!(warnings.is_empty());
    close(net.loads().iter().map(|load| load.p).sum(), 0.6);
    close(net.loads().iter().map(|load| load.q).sum(), 0.2);
    let (twice, warnings) = parse(&format!("{LITERAL}{SCALE}\n{SCALE}"));
    assert!(warnings.is_empty());
    close(twice.loads().iter().map(|load| load.p).sum(), 0.0003);
}

#[test]
fn unsupported_or_ambiguous_blocks_keep_all_literal_values_and_diagnostics() {
    let original = parse(LITERAL).0;
    for extra in [
        "mpc.bus(:, 3) = 0;",
        "mpc.bus(1, 3) = mpc.bus(1, 3) / 1000;",
        "mpc.bus(:, [3 3]) = mpc.bus(:, [3 3]) / 1000;",
        "mpc.bus(:, [3 4]) = mpc.bus(:, [4 3]) / 1000;",
        "mpc.bus(:, [PD QD]) = mpc.bus(:, [PD QD]) / 1000;",
        "mpc.bus(:, 3:4) = mpc.bus(:, 3:4) / 1000;",
        "mpc.bus(:, 10) = mpc.bus(:, 10) * 1000;",
        "mpc.bus(:, 3) = mpc.bus(:, 3) / missing; missing = 1000;",
        "mpc.bus(:, 3) = mpc.bus(:, 3) / 1000 / 2;",
        "mpc.bus(:, 3) = mpc.bus(:, 3) / 1000 * 2;",
        "factor = unknown();",
        "factor = 2 ^ 3;",
        "factor = -2 ^ 2;",
        "idx_bus = 42;",
        "mpc.userdata = unknown();",
        "mpc.baseMVA = 100;",
        "if true\nmpc.bus(:, 3) = mpc.bus(:, 3) / 10;\nend",
        "for i = 1:2\nmpc.bus(:, 3) = mpc.bus(:, 3) / 10;\nend",
        "return;",
        "function other = helper\nother = 1;\nend",
        "text = 'mpc.bus(:, 3) = 0;';",
    ] {
        for block in [format!("{extra}\n{SCALE}"), format!("{SCALE}\n{extra}")] {
            let source = format!("{LITERAL}{block}\n");
            let (net, warnings) = parse(&source);
            assert_eq!(net.loads(), original.loads(), "{block}");
            assert_eq!(net.branches(), original.branches(), "{block}");
            assert!(!warnings.is_empty(), "{block}");
            assert!(warnings.records().iter().all(|record| {
                record.code() == "READ.MATPOWER.INDEXED_ASSIGNMENT"
                    && record.severity() == powerio_core::DiagnosticSeverity::Warning
                    && !record.spans().is_empty()
            }));
        }
    }
}

#[test]
fn mutations_after_a_continuation_or_as_octave_operators_are_not_missed() {
    // MATLAB joins the line after `...`, so the assignment still starts a
    // statement; Octave's `/=` mutates the same element. Each must be either
    // converted or reported, never read as literal values in silence.
    for mutation in [
        "x = 1; ...\nmpc.bus(:, [3 4]) = mpc.bus(:, [3 4]) / 1e3;",
        "mpc.bus(:, [3 4]) /= 1e3;",
    ] {
        let (net, warnings) = parse(&format!("{LITERAL}{mutation}\n"));
        let converted = (net.loads().iter().map(|load| load.p).sum::<f64>() - 0.3).abs() < 1e-13;
        let reported = warnings
            .records()
            .iter()
            .any(|record| record.code() == "READ.MATPOWER.INDEXED_ASSIGNMENT");
        assert!(converted || reported, "{mutation}");
    }
}

#[test]
fn indexed_assignment_warnings_stay_within_the_diagnostic_cap() {
    let (_, warnings) = parse(&format!("{LITERAL}{}", "mpc.bus(:,3)=0;\n".repeat(1_005)));
    let reported = warnings
        .records()
        .iter()
        .filter(|record| record.code() == "READ.MATPOWER.INDEXED_ASSIGNMENT")
        .count();
    assert_eq!(reported, 1_001);
}

#[test]
fn invalid_scalars_and_overflow_do_not_partially_convert() {
    for divisor in [
        "0",
        "-1",
        "NaN",
        "Inf",
        "1e999",
        "(1 / 0)",
        "(1e-300 * 1e-300)",
        "1e-308",
    ] {
        let conversion = format!(
            "mpc.branch(:, [3 4]) = mpc.branch(:, [3 4]) / 2;\n\
            mpc.bus(:, [3 4]) = mpc.bus(:, [3 4]) / {divisor};"
        );
        let (net, warnings) = parse(&format!("{LITERAL}{conversion}"));
        assert!(!warnings.is_empty(), "{divisor}");
        close(net.loads()[0].p, 100.0);
        close(net.branches()[0].r, 0.0922);
    }
    for literal in [
        LITERAL.replace("12.66", "0"),
        LITERAL.replace("12.66", "-12.66"),
        LITERAL.replace("baseMVA = 10", "baseMVA = -10"),
    ] {
        let (net, warnings) = parse(&format!(
            "{literal}mpc.branch(:, [3 4]) = mpc.branch(:, [3 4]) / (mpc.bus(1,10)^2 / mpc.baseMVA);"
        ));
        assert!(!warnings.is_empty());
        close(net.branches()[0].r, 0.0922);
    }
}

#[test]
fn conversion_before_literals_is_not_reordered() {
    let (net, warnings) = parse(&format!("{SCALE}\n{LITERAL}"));
    assert!(!warnings.is_empty());
    close(net.loads()[0].p, 100.0);
}

#[test]
fn bounded_expressions_fail_without_recursing_indefinitely() {
    for expression in [
        format!("{}1{}", "(".repeat(80), ")".repeat(80)),
        "1*".repeat(200),
    ] {
        let (net, warnings) = parse(&format!(
            "{LITERAL}mpc.bus(:, 3) = mpc.bus(:, 3) / ({expression});"
        ));
        assert!(!warnings.is_empty());
        close(net.loads()[0].p, 100.0);
    }
}

#[test]
fn literal_expressions_and_code_inside_cells_do_not_authorize_conversion() {
    for source in [
        LITERAL.replace("0 -360 360];", "0 -360 360] / 100;"),
        LITERAL.replace("baseMVA = 10", "baseMVA = '10'"),
        format!("{LITERAL}mpc.version = unknown();\n"),
        format!("{LITERAL}mpc.bus_name = {{unknown()}};\n"),
        format!("{LITERAL}mpc.gencost = [unknown()];\n"),
        format!("%{{\nmpc.baseMVA = 20;\n%}}\n{LITERAL}"),
    ] {
        let (net, warnings) = parse(&format!("{source}{SCALE}"));
        assert!(!warnings.is_empty(), "{source}");
        close(net.loads()[0].p, 100.0);
    }
}

#[test]
fn spaced_dot_targets_are_converted_or_diagnosed() {
    let (net, warnings) = parse(&format!(
        "{LITERAL}{}",
        SCALE.replace("mpc.bus", "mpc . bus")
    ));
    assert!(warnings.is_empty());
    close(net.loads()[0].p, 0.1);
    let mutation = "mpc . bus(:, 3) = unknown();";
    let (net, warnings) = parse(&format!("{LITERAL}{mutation}"));
    close(net.loads()[0].p, 100.0);
    assert_eq!(warnings.records().len(), 1);
    let span = &warnings.records()[0].spans()[0];
    assert_eq!(span.byte_start(), LITERAL.len() as u64);
    assert_eq!(span.byte_end(), (LITERAL.len() + mutation.len()) as u64);
}

#[test]
fn block_delimiters_must_occupy_the_entire_physical_line() {
    for block in [
        "factor=1000; %{\nmpc.bus(:,3)=0;\n%}\n",
        "%{ not a block opener\nmpc.bus(:,3)=0;\n%}\n",
        "%{\n%} not a block closer\nmpc.bus(:,3)=0;\n%}\nmpc.bus(:,3)=unknown();\n",
    ] {
        let (net, warnings) = parse(&format!("{LITERAL}{block}{SCALE}"));
        close(net.loads()[0].p, 100.0);
        assert!(!warnings.is_empty(), "{block}");
    }
}

#[test]
fn whitespace_separated_cell_strings_do_not_hide_conversions() {
    let source = format!("{LITERAL}mpc.bus_name = {{'a' '['}};\n{SCALE}");
    let (net, warnings) = parse(&source);
    assert!(warnings.is_empty(), "{:?}", warnings.lines());
    close(net.loads()[0].p, 0.1);
    assert_eq!(net.buses()[1].name.as_deref(), Some("["));
    let source = source.replace(SCALE, "mpc.bus(:, 3) = unknown();");
    let (net, warnings) = parse(&source);
    close(net.loads()[0].p, 100.0);
    assert_eq!(warnings.records().len(), 1);
}
