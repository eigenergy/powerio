//! Two-winding impedance bases are checked against the original two-port circuit.
//! PSS/E states R1-2 and X1-2 on the winding voltage base, the bus base, under
//! every CZ: CZ 2 and 3 change only the MVA base. NOMV1 and NOMV2 enter the
//! circuit through CW 2 and 3 ratios, never through the impedance.
mod helpers;
use helpers::*;
use powerio_tx::{BalancedNetwork, Branch, TargetFormat};

fn raw(base: f64, cw: u8, cz: u8, nominal: bool) -> String {
    let (w1, w2) = if cw == 2 {
        (242.0, 106.7)
    } else {
        (1.05, 0.98)
    };
    let (n1, n2) = if nominal { (230.0, 115.0) } else { (0.0, 0.0) };
    let r = if cz == 3 { 50_000.0 } else { 0.01 };
    format!(
        "0,{base},33,0,0,50\nCASE\nCOMMENT\n\
1,'B1',220,3,1,1,1,1.02,0,1.1,.9,1.1,.9\n\
2,'B2',110,1,1,1,1,.99,-3,1.1,.9,1.1,.9\n\
0 / END OF BUS DATA, BEGIN LOAD DATA\n\
0 / END OF LOAD DATA, BEGIN FIXED SHUNT DATA\n\
0 / END OF FIXED SHUNT DATA, BEGIN GENERATOR DATA\n\
0 / END OF GENERATOR DATA, BEGIN BRANCH DATA\n\
0 / END OF BRANCH DATA, BEGIN TRANSFORMER DATA\n\
1,2,0,'1',{cw},{cz},1,0,0,2,'XF',1\n\
{r},.2,50\n\
{w1},{n1},5,100,110,120,0,0,1.1,.9,1.1,.9,33,0,0,0,0\n\
{w2},{n2}\n\
0 / END OF TRANSFORMER DATA, BEGIN AREA DATA\nQ\n"
    )
}

fn inverse(r: f64, x: f64) -> (f64, f64) {
    let d = r * r + x * x;
    (r / d, -x / d)
}

fn scale_rotate((g, b): (f64, f64), scale: f64, angle: f64) -> (f64, f64) {
    let (sin, cos) = angle.sin_cos();
    (scale * (g * cos - b * sin), scale * (g * sin + b * cos))
}

fn terminal_admittances(br: &Branch) -> [(f64, f64); 4] {
    let y = inverse(br.r, br.x);
    let a = br.calc_effective_tap();
    let phi = br.shift.to_radians();
    [
        scale_rotate(y, 1.0 / (a * a), 0.0),
        scale_rotate(y, -1.0 / a, phi),
        scale_rotate(y, -1.0 / a, -phi),
        y,
    ]
}

fn assert_ports(actual: [(f64, f64); 4], expected: [(f64, f64); 4]) {
    for (a, e) in actual.into_iter().zip(expected) {
        assert!(
            (a.0 - e.0).abs() < 1e-10 && (a.1 - e.1).abs() < 1e-10,
            "actual {actual:?}, expected {expected:?}"
        );
    }
}

#[test]
fn all_winding_and_impedance_codes_match_the_original_circuit() {
    for base in [100.0, 200.0] {
        for nominal in [false, true] {
            for cw in 1..=3 {
                for cz in 1..=3 {
                    let net = parse_psse(&raw(base, cw, cz, nominal)).unwrap();
                    let br = &net.branches()[0];
                    let t1 = match cw {
                        2 => 242.0 / 220.0,
                        3 if nominal => 1.05 * 230.0 / 220.0,
                        _ => 1.05,
                    };
                    let t2 = match cw {
                        2 => 106.7 / 110.0,
                        3 if nominal => 0.98 * 115.0 / 110.0,
                        _ => 0.98,
                    };
                    let r = if cz == 3 { 0.001 } else { 0.01 };
                    let x = if cz == 3 {
                        (0.2_f64.powi(2) - r * r).sqrt()
                    } else {
                        0.2
                    };
                    let zscale = if cz == 1 { 1.0 } else { base / 50.0 };
                    // Original circuit: the impedance lies between t1∠5° and t2.
                    // This oracle does not use the reader's base/tap helpers.
                    let y = inverse(r * zscale, x * zscale);
                    let phi = 5.0_f64.to_radians();
                    let expected = [
                        scale_rotate(y, 1.0 / (t1 * t1), 0.0),
                        scale_rotate(y, -1.0 / (t1 * t2), phi),
                        scale_rotate(y, -1.0 / (t1 * t2), -phi),
                        scale_rotate(y, 1.0 / (t2 * t2), 0.0),
                    ];
                    assert_ports(terminal_admittances(br), expected);
                    // Emit the value in a new module, not a same-format source echo.
                    let mut canonical: BalancedNetwork = net;
                    for _ in 0..2 {
                        canonical = parse_psse(
                            &emit_value(&canonical, TargetFormat::Psse { rev: 33 })
                                .unwrap()
                                .text,
                        )
                        .unwrap();
                        assert_ports(terminal_admittances(&canonical.branches()[0]), expected);
                    }
                }
            }
        }
    }
}

#[test]
fn a_nominal_voltage_of_the_other_winding_leaves_the_impedance_as_stated() {
    // NOMV1 states bus J's voltage and NOMV2 bus I's: the impedance is still
    // the MVA conversion of what the record states, not (110 / 220)² of it.
    let text = raw(100.0, 1, 2, true)
        .replace("1.05,230,5", "1.05,110,5")
        .replace("0.98,115", "0.98,220");
    let net = parse_psse(&text).unwrap();
    let br = &net.branches()[0];
    let factor = 2.0 * 0.98_f64.powi(2);
    assert!((br.r - 0.01 * factor).abs() < 1e-12, "{}", br.r);
    assert!((br.x - 0.2 * factor).abs() < 1e-12, "{}", br.x);
}

#[test]
fn rawx_reads_the_impedance_on_the_same_base() {
    let document = serde_json::json!({"network": {
        "caseid": {"fields": ["ic", "sbase", "rev", "basfrq"], "data": [0, 100, 35, 50]},
        "bus": {"fields": ["ibus", "name", "baskv", "ide"], "data": [[1,"B1",220,3],[2,"B2",110,1]]},
        "transformer": {"fields": ["ibus","jbus","kbus","ckt","cw","cz","cm","r1_2","x1_2","sbase1_2","windv1","nomv1","ang1","windv2","nomv2"],
            "data": [[1,2,0,"1",1,2,1,0.01,0.2,50,1.05,230,5,0.98,115]]}
    }});
    let net = parse_str(&document.to_string(), "rawx").unwrap().network;
    let br = &net.branches()[0];
    let factor = 2.0 * 0.98_f64.powi(2);
    assert!((br.r - 0.01 * factor).abs() < 1e-12);
    assert!((br.x - 0.2 * factor).abs() < 1e-12);
}
