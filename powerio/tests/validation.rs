//! Input validation and reader behavior guarantees: malformed input must fail
//! loudly, never silently default into a structurally valid but wrong network.

use std::path::Path;

use powerio::network::{Branch, Bus, BusId, BusType, Extras, Network};
use powerio::{Error, parse_powerworld, parse_psse, write_powerworld};

fn bus(id: usize, kind: BusType) -> Bus {
    Bus {
        id: BusId(id),
        kind,
        vm: 1.0,
        va: 0.0,
        base_kv: 1.0,
        vmax: 1.1,
        vmin: 0.9,
        area: 1,
        zone: 1,
        name: None,
        extras: Extras::new(),
    }
}

fn branch(from: usize, to: usize) -> Branch {
    Branch {
        from: BusId(from),
        to: BusId(to),
        r: 0.0,
        x: 0.1,
        b: 0.0,
        rate_a: 0.0,
        rate_b: 0.0,
        rate_c: 0.0,
        tap: 0.0,
        shift: 0.0,
        in_service: true,
        angmin: -360.0,
        angmax: 360.0,
        control: None,
        extras: Extras::new(),
    }
}

#[test]
fn validate_rejects_duplicate_bus_id() {
    // Two buses share id 1: dense indexing would collapse them onto one index
    // and silently corrupt every nodal aggregate, so validate() must reject it.
    let net = Network::in_memory(
        "dup",
        100.0,
        vec![bus(1, BusType::Ref), bus(1, BusType::Pq)],
        Vec::new(),
    );
    assert!(matches!(net.validate(), Err(Error::FormatRead { .. })));
}

#[test]
fn validate_rejects_dangling_branch_endpoint() {
    let net = Network::in_memory(
        "dangling",
        100.0,
        vec![bus(1, BusType::Ref), bus(2, BusType::Pq)],
        vec![branch(1, 99)],
    );
    assert!(matches!(net.validate(), Err(Error::FormatRead { .. })));
}

#[test]
fn from_json_rejects_dangling_reference() {
    // to_json does not validate, so a hand-built (or hand-edited) invalid network
    // serializes fine; from_json must reject it on the way back in, since the
    // C ABI and Julia bridge ride on this transport.
    let bad = Network::in_memory(
        "bad",
        100.0,
        vec![bus(1, BusType::Ref), bus(2, BusType::Pq)],
        vec![branch(1, 99)],
    );
    let json = bad.to_json().unwrap();
    assert!(matches!(
        Network::from_json(&json),
        Err(Error::FormatRead { .. })
    ));
}

#[test]
fn from_json_rejects_zero_bus_network() {
    // A bus-less network is content-free; read_source rejects it for every parse
    // path, and from_json (the JSON transport) must reject it too so the guard is
    // universal rather than skippable through the C ABI / Julia bridge.
    let empty = Network::in_memory("empty", 100.0, vec![], vec![]);
    let json = empty.to_json().unwrap();
    assert!(matches!(
        Network::from_json(&json),
        Err(Error::FormatRead { .. })
    ));
}

#[test]
fn psse_rejects_malformed_numeric_field() {
    // The pristine fixture parses; corrupting one numeric field (a bus voltage
    // magnitude) must error rather than silently default it — a present-but-
    // garbage number that becomes 0.0 would corrupt the matrices downstream.
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/data/psse/case14.raw");
    let good = std::fs::read_to_string(&path).unwrap();
    assert!(parse_psse(&good).is_ok(), "pristine fixture should parse");

    let bad = good.replacen("1.05999994", "1.0xx99994", 1);
    assert_ne!(good, bad, "corruption target not found in fixture");
    assert!(matches!(parse_psse(&bad), Err(Error::FormatRead { .. })));
}

#[test]
fn powerworld_rejects_malformed_numeric_field() {
    // Same contract as PSS/E, for the sibling .aux reader: write a valid file,
    // corrupt one numeric field (a branch reactance), and the reader must error
    // rather than silently default it to 0.0.
    let mut br = branch(1, 2);
    br.x = 0.123_45; // distinctive token to corrupt
    let net = Network::in_memory(
        "pw",
        100.0,
        vec![bus(1, BusType::Ref), bus(2, BusType::Pq)],
        vec![br],
    );
    let good = write_powerworld(&net).text;
    assert!(
        parse_powerworld(&good).is_ok(),
        "pristine .aux should parse"
    );

    let bad = good.replacen("0.12345", "0.1x345", 1);
    assert_ne!(good, bad, "corruption target not found in .aux");
    assert!(matches!(
        parse_powerworld(&bad),
        Err(Error::FormatRead { .. })
    ));
}

#[test]
fn psse_reads_switched_shunt_as_fixed() {
    // case14.raw carries its only shunt in the SWITCHED SHUNT section (BINIT = 19
    // MVAr at bus 9). powerio reads it as a fixed shunt (gs = 0, bs = BINIT), the
    // same reduction PowerModels makes, so the susceptance isn't silently dropped.
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/data/psse/case14.raw");
    let net = parse_psse(&std::fs::read_to_string(path).unwrap()).unwrap();
    let s = net
        .shunts
        .iter()
        .find(|s| s.bus == BusId(9))
        .expect("switched shunt at bus 9 read as a fixed shunt");
    assert!((s.b - 19.0).abs() < 1e-9, "BINIT susceptance, got {}", s.b);
    assert!((s.g).abs() < 1e-12, "switched shunt has no conductance");
}

#[test]
fn powerworld_reads_the_2022_vocabulary() {
    // The third field naming generation (Simulator 21+ concise exports, the
    // Hawaii40 set): committed evidence for the README claim instead of a
    // machine specific parity test only.
    let aux = "\
Bus (Number, Name, NomkV, Vpu, Vangle)\n{\n\
1 \"A\" 138.0 1.02 -3.5\n\
2 \"B\" 13.8 1.01 -4.0\n\
}\n\
Load (BusNum, ID, Status, SMW, SMvar)\n{\n\
1 \"1\" \"Closed\" 12.5 3.25\n\
}\n\
Gen (BusNum, ID, Status, MWSetPoint, MvarSetPoint)\n{\n\
2 \"1\" \"Closed\" 50.0 7.5\n\
}\n\
Branch (BusNumFrom, BusNumTo, Circuit, BranchDeviceType, R, X, B, LimitMVAA)\n{\n\
1 2 \"1\" \"Transformer\" 0.0015 0.0525 0.002 95.0\n\
}\n\
Transformer (BusNumFrom, BusNumTo, Circuit, Rxfbase, Xxfbase, Tapxfbase)\n{\n\
1 2 \"1\" 0.0015 0.0525 1.0375\n\
}\n";
    let net = parse_powerworld(aux).unwrap();
    assert_eq!(net.buses.len(), 2);
    assert!((net.buses[0].base_kv - 138.0).abs() < 1e-9);
    assert!((net.buses[0].vm - 1.02).abs() < 1e-9);
    assert!((net.buses[0].va - -3.5).abs() < 1e-9);
    assert_eq!(net.loads.len(), 1);
    assert!((net.loads[0].p - 12.5).abs() < 1e-9);
    assert!((net.loads[0].q - 3.25).abs() < 1e-9);
    assert_eq!(net.generators.len(), 1);
    assert!((net.generators[0].pg - 50.0).abs() < 1e-9);
    assert_eq!(net.branches.len(), 1);
    let br = &net.branches[0];
    // Real 2022 exports print the same impedance in the Branch and
    // Transformer sections (the Branch row already carries the corrected
    // value); the tap exists only on the Transformer section.
    assert!((br.r - 0.0015).abs() < 1e-9);
    assert!((br.x - 0.0525).abs() < 1e-9);
    assert!((br.tap - 1.0375).abs() < 1e-9, "Tapxfbase read: {}", br.tap);
    assert!((br.rate_a - 95.0).abs() < 1e-9);
}

#[test]
fn powerworld_reads_bare_line_tap() {
    // 2016 era exports declare the tap under the bare name like every other
    // transformer field; an off nominal value must survive (the fetched 2016
    // corpus stores 1.0 everywhere, which masked this path from parity).
    let aux = "\
DATA (Bus, [BusNum, BusName])\n{\n1 \"A\"\n2 \"B\"\n}\n\
DATA (Branch, [BusNum, BusNum:1, LineCircuit, BranchDeviceType, LineR, LineX, LineTap])\n{\n\
1 2 \"1\" \"Transformer\" 0.001 0.05 1.0625\n\
}\n";
    let net = parse_powerworld(aux).unwrap();
    assert_eq!(net.branches.len(), 1);
    assert!(
        (net.branches[0].tap - 1.0625).abs() < 1e-9,
        "bare LineTap read: {}",
        net.branches[0].tap
    );
}

#[test]
fn powerworld_status_vocabulary_is_closed() {
    let base = |status: &str| {
        format!(
            "DATA (Bus, [BusNum, BusName])\n{{\n1 \"A\"\n}}\n\
             DATA (Load, [BusNum, LoadID, LoadStatus, LoadSMW])\n{{\n1 \"1\" \"{status}\" 5.0\n}}\n"
        )
    };
    // Case insensitive on the documented vocabulary.
    assert!(!parse_powerworld(&base("open")).unwrap().loads[0].in_service);
    assert!(!parse_powerworld(&base("OPEN")).unwrap().loads[0].in_service);
    assert!(parse_powerworld(&base("closed")).unwrap().loads[0].in_service);
    // An unknown token must not silently mean energized.
    assert!(matches!(
        parse_powerworld(&base("Disconnected")),
        Err(Error::FormatRead { .. })
    ));
}

#[test]
fn powerworld_rejects_non_integer_bus_numbers() {
    // Float to integer casts saturate (NaN and negatives to 0), which would
    // silently rewire devices onto bus 0; the parse must error instead.
    for bad in ["-3", "NaN", "inf", "1.5"] {
        let aux = format!(
            "DATA (Bus, [BusNum, BusName])\n{{\n1 \"A\"\n}}\n\
             DATA (Load, [BusNum, LoadID, LoadSMW])\n{{\n{bad} \"1\" 5.0\n}}\n"
        );
        assert!(
            matches!(parse_powerworld(&aux), Err(Error::FormatRead { .. })),
            "bus number {bad:?} must reject"
        );
    }
}

#[test]
fn powerworld_reads_name_keyed_exports() {
    let aux = "\
DATA (Bus, [BusName_NomVolt, BusNum, BusName])\n{\n\"A_138.0\" 1 \"A\"\n}\n\
DATA (Load, [BusName_NomVolt, LoadID, LoadSMW])\n{\n\"A_138.0\" \"1\" 5.0\n}\n";
    let net = parse_powerworld(aux).unwrap();
    assert_eq!(net.loads[0].bus.0, 1);

    let unknown = aux.replace("A_138.0\" \"1\" 5.0", "B_138.0\" \"1\" 5.0");
    let err = parse_powerworld(&unknown).unwrap_err();
    assert!(
        err.to_string().contains("unknown BusName_NomVolt"),
        "error names the unresolved label: {err}"
    );
}

#[test]
fn powerworld_survives_multibyte_keyword_boundaries() {
    // Keyword detection slices at byte offsets; a multibyte character
    // straddling the boundary must read as "not the keyword", never panic.
    for text in [
        "SCRIPÄ x\n{\n}\n",
        "DATÄ(Bus, [BusNum])\n{\n1\n}\n",
        "DAT\u{00c4} (Bus, [BusNum])\n{\n1\n}\n",
    ] {
        let _ = parse_powerworld(text); // any Err is fine; a panic is the bug
    }
}
