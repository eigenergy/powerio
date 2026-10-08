use super::{read, schema::NativeDatabase, tests::database};
use crate::{Configuration, DistBus, MulticonductorNetwork};

pub(super) fn schema_sql() -> String {
    let mut columns = vec![
        "Element_ID INTEGER".to_owned(),
        "Variant_ID INTEGER".to_owned(),
    ];
    for field in [
        "Typ_ID",
        "Mpl_ID",
        "Macro_ID",
        "EnergyStorage_ID",
        "DayOpSer_ID",
        "WeekOpSer_ID",
        "YearOpSer_ID",
        "PowerLimit_ID",
        "TransformerTap_ID",
        "Qctrl_U_P_ID",
        "Qctrl_PF_U_ID",
        "Qctrl_PF_P_ID",
        "Qctrl_P_Q_ID",
        "Qctrl_U_Q_ID",
        "Node_ID",
        "MasterElm_ID",
        "HarCur_ID",
        "HarVolt_ID",
        "Flag_Typ_ID",
        "IslandOp",
        "Flag_Macro",
        "Flag_Converter",
        "Flag_LfLimit",
        "Flag_LimitType",
        "Flag_Pctrl",
        "Flag_Qctrl",
        "Flag_CtrlPrior",
        "Flag_ShdU",
        "Flag_ShdP",
    ] {
        columns.push(format!("{field} INTEGER DEFAULT 0"));
    }
    for field in [
        "Flag_Lf",
        "Flag_Connect",
        "Flag_DCtyp",
        "Flag_ChkType",
        "Flag_I",
    ] {
        columns.push(format!("{field} INTEGER DEFAULT 1"));
    }
    for field in ["Rlf", "Xlf", "Kr", "Ireg", "pk", "Sn_Inverter"] {
        columns.push(format!("{field} REAL DEFAULT 0"));
    }
    columns.extend(
        [
            "Flag_LA REAL",
            "Boost_Idc REAL",
            "P REAL DEFAULT 0.06",
            "Q REAL DEFAULT -0.03",
            "fP REAL DEFAULT 0.5",
            "fQ REAL DEFAULT 2",
        ]
        .map(str::to_owned),
    );
    format!("CREATE TABLE DCInfeeder ({});", columns.join(","))
}

fn native(edit: &str) -> NativeDatabase {
    let bytes = database(&format!(
        "UPDATE Element SET Type='DCInfeeder';
         ALTER TABLE Element ADD COLUMN Flag_Input INTEGER DEFAULT 2;
         ALTER TABLE Element ADD COLUMN Flag_State INTEGER DEFAULT 1;
         ALTER TABLE Element ADD COLUMN VoltLevel_ID INTEGER DEFAULT 1;
         CREATE TABLE VoltageLevel (VoltLevel_ID INTEGER, Variant_ID INTEGER, Flag_DCInfeeder INTEGER);
         INSERT INTO VoltageLevel VALUES (1,1,0);
         ALTER TABLE Terminal ADD COLUMN Flag_Terminal INTEGER DEFAULT 7;
         ALTER TABLE Terminal ADD COLUMN Flag_State INTEGER DEFAULT 1;
         DELETE FROM Terminal WHERE TerminalNo=2;
         {} INSERT INTO DCInfeeder (Element_ID,Variant_ID) VALUES (30,1); {edit}",
        schema_sql()
    ));
    NativeDatabase::decode(&bytes, None).unwrap()
}

fn bus() -> DistBus {
    DistBus::new("10", ["3", "n", "1", "2"].map(str::to_owned).to_vec())
}

#[test]
fn converter_connections_preserve_total_power_order_and_explicit_earth() {
    for (code, expected) in [
        (1, vec!["1", "0"]),
        (2, vec!["2", "0"]),
        (3, vec!["3", "0"]),
        (4, vec!["1", "2"]),
        (5, vec!["2", "3"]),
        (6, vec!["3", "1"]),
        (7, vec!["1", "2", "3", "0"]),
    ] {
        let db = native(&format!("UPDATE Terminal SET Flag_Terminal={code}"));
        let input = db.dc_infeeder_input(30).unwrap();
        let native_bus = bus();
        let circuit = input.circuit(&native_bus).unwrap();
        let generator = &circuit.generator;
        assert_eq!(generator.terminal_map, expected);
        assert_eq!(
            generator.configuration,
            if code == 7 {
                Configuration::Wye
            } else {
                Configuration::SinglePhase
            }
        );
        assert!((generator.p_nom.iter().sum::<f64>() - 30_000.0).abs() < 1e-9);
        assert!((generator.q_nom.iter().sum::<f64>() + 60_000.0).abs() < 1e-9);
        assert_eq!(generator.p_nom.len(), if code == 7 { 3 } else { 1 });
        assert_eq!(circuit.bus.grounded.is_empty(), (4..=6).contains(&code));
        assert_eq!(native_bus.grounded.as_slice(), []);
        assert!(
            !circuit
                .switch
                .terminal_map_from
                .iter()
                .any(|t| t == "n" || t == "0")
        );
        let mut net = MulticonductorNetwork::new();
        net.buses_mut().extend([native_bus, circuit.bus]);
        net.generators_mut().push(circuit.generator);
        net.switches_mut().push(circuit.switch);
        crate::require_electrical_readiness(&net).unwrap();
        let output = crate::convert::emit_value_text(&net, crate::DistTargetFormat::PmdJson);
        let parsed = crate::testkit::parse_str(&output.text, "pmd-json").unwrap();
        crate::require_electrical_readiness(&parsed).unwrap();
        assert_eq!(parsed.generators()[0].p_nom, net.generators()[0].p_nom);
        assert_eq!(parsed.generators()[0].q_nom, net.generators()[0].q_nom);
        // PMD uses a one-branch WYE for a single phase with the same two terminals.
        assert_eq!(parsed.generators()[0].configuration, Configuration::Wye);
        assert_eq!(
            parsed.generators()[0].terminal_map,
            net.generators()[0].terminal_map
        );
    }
}

#[test]
fn converter_absorption_and_open_port_keep_the_internal_device() {
    let closed = native("UPDATE DCInfeeder SET P=-0.06, Flag_DCtyp=3")
        .dc_infeeder_input(30)
        .unwrap()
        .circuit(&bus())
        .unwrap();
    let open =
        native("UPDATE DCInfeeder SET P=-0.06, Flag_DCtyp=3; UPDATE Terminal SET Flag_State=0")
            .dc_infeeder_input(30)
            .unwrap()
            .circuit(&bus())
            .unwrap();
    assert_eq!(closed.generator, open.generator);
    assert_eq!(closed.bus, open.bus);
    assert!(!closed.switch.open && open.switch.open);
    assert!(open.generator.p_nom.iter().all(|p| *p < 0.0));
    for kind in 1..=7 {
        assert!(
            native(&format!("UPDATE DCInfeeder SET Flag_DCtyp={kind}"))
                .dc_infeeder_input(30)
                .is_ok()
        );
    }
}

#[test]
fn converter_refuses_unresolved_models_references_and_invalid_powers() {
    for edit in [
        "UPDATE DCInfeeder SET Flag_Lf=2",
        "UPDATE DCInfeeder SET Flag_Connect=2",
        "UPDATE DCInfeeder SET Flag_DCtyp=8",
        "UPDATE DCInfeeder SET Typ_ID=5",
        "UPDATE DCInfeeder SET HarCur_ID=5",
        "UPDATE DCInfeeder SET DayOpSer_ID=5",
        "UPDATE DCInfeeder SET EnergyStorage_ID=5",
        "UPDATE DCInfeeder SET Flag_Qctrl=1",
        "UPDATE DCInfeeder SET Flag_Pctrl=1",
        "UPDATE DCInfeeder SET Flag_LfLimit=1",
        "UPDATE DCInfeeder SET Sn_Inverter=100",
        "UPDATE DCInfeeder SET Flag_ShdU=1",
        "UPDATE DCInfeeder SET Flag_ShdP=1",
        "UPDATE DCInfeeder SET Rlf=1",
        "UPDATE DCInfeeder SET Ireg=1",
        "UPDATE DCInfeeder SET Flag_I=0",
        "UPDATE DCInfeeder SET Flag_LA=1",
        "UPDATE DCInfeeder SET Boost_Idc=1",
        "UPDATE DCInfeeder SET Flag_Converter=1",
        "UPDATE DCInfeeder SET P=NULL",
        "UPDATE VoltageLevel SET Flag_DCInfeeder=1",
        "UPDATE VoltageLevel SET Variant_ID=2",
        "INSERT INTO VoltageLevel VALUES (1,1,0)",
        "UPDATE DCInfeeder SET P=1e308",
        "UPDATE DCInfeeder SET P=1e-300, fP=1e-300",
        "UPDATE Terminal SET Flag_Terminal=8",
        "UPDATE Element SET Flag_Input=1",
        "INSERT INTO DCInfeeder SELECT * FROM DCInfeeder",
        "DELETE FROM DCInfeeder",
    ] {
        assert!(
            native(edit).dc_infeeder_input(30).is_err(),
            "accepted {edit}"
        );
    }
    assert!(
        native("UPDATE Element SET Flag_State=0")
            .dc_infeeder_input(30)
            .unwrap()
            .circuit(&bus())
            .is_err()
    );
    let input = native("").dc_infeeder_input(30).unwrap();
    let mut missing = bus();
    missing.terminals.retain(|p| p != "2");
    assert!(input.circuit(&missing).is_err());
    missing.id = "99".into();
    assert!(input.circuit(&missing).is_err());
    let db = native("INSERT INTO DCInfeeder (Element_ID,Variant_ID,P) VALUES (30,2,999)");
    assert!((db.dc_infeeder_input(30).unwrap().watts - 30_000.0).abs() < 1e-9);
}

#[test]
fn authentic_converter_powers_match_the_four_paired_csv_generators() {
    let source = powerio_core::Source::from_memory(
        "case.sinx",
        &include_bytes!("../../../tests/data/sincal/1-LV-rural1--0-sw.sinx")[..],
    )
    .unwrap();
    let db = read(&source, None).unwrap();
    let topology = db.topology_draft().unwrap();
    let csv = include_str!("../../../tests/data/sincal/simbench-csv/RES.csv");
    let rows: Vec<_> = csv
        .lines()
        .skip(1)
        .map(|line| line.split(';').collect::<Vec<_>>())
        .collect();
    let mut count = 0;
    for (&id, kind) in &db.elements {
        if kind != "DCInfeeder" {
            continue;
        }
        let name: String = db
            .connection
            .query_row(
                "SELECT Name FROM Element WHERE Element_ID=?1 AND Variant_ID=1",
                [id],
                |r| r.get(0),
            )
            .unwrap();
        let row = rows.iter().find(|row| row[0] == name).unwrap();
        let input = db.dc_infeeder_input(id).unwrap();
        assert_eq!(
            topology.nodes[&input.terminal.node].name.as_deref(),
            Some(row[1])
        );
        let native_bus = topology
            .buses()
            .into_iter()
            .find(|b| b.id == input.terminal.node.to_string())
            .unwrap();
        let circuit = input.circuit(&native_bus).unwrap();
        assert!(
            (circuit.generator.p_nom.iter().sum::<f64>() - row[5].parse::<f64>().unwrap() * 1e6)
                .abs()
                < 1e-8
        );
        assert!(
            (circuit.generator.q_nom.iter().sum::<f64>() - row[6].parse::<f64>().unwrap() * 1e6)
                .abs()
                < 1e-8
        );
        count += 1;
    }
    assert_eq!(count, 4);
}
