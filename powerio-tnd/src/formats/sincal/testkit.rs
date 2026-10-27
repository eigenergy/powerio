use powerio_dist::MulticonductorNetwork;
pub struct Parsed {
    pub network: MulticonductorNetwork,
}
pub fn parse_str(text: &str, from: &str) -> Result<Parsed, powerio_core::Error> {
    let source = powerio_core::Source::from_memory("test", text.as_bytes().to_vec())?
        .with_format(powerio_core::FormatId::new(from)?);
    powerio_dist::parse(source).map(|m| Parsed {
        network: m.value().clone(),
    })
}

impl std::ops::Deref for Parsed {
    type Target = MulticonductorNetwork;
    fn deref(&self) -> &Self::Target {
        &self.network
    }
}
pub struct TextEmission {
    pub text: String,
    pub diagnostics: Vec<powerio_core::Diagnostic>,
}
pub fn emit_value_text(
    net: &MulticonductorNetwork,
    format: powerio_dist::DistTargetFormat,
) -> TextEmission {
    let result = powerio_dist::emit(
        &powerio_core::PioModule::new(net.clone()),
        format,
        powerio_core::Destination::memory("output").unwrap(),
    )
    .unwrap();
    let diagnostics = result.diagnostics().to_vec();
    let powerio_core::EmittedOutput::Memory { artifacts } = result.into_output() else {
        panic!("expected memory output")
    };
    TextEmission {
        text: String::from_utf8(artifacts.into_iter().next().unwrap().into_bytes()).unwrap(),
        diagnostics,
    }
}

/// The SimBench fixture's native 250-table database, grown by a radial chain
/// of `lines` copies of its first cable (element 20). Each copy adds one node,
/// one element, one line and two terminals; the chain hangs off node 3. The
/// copies state explicit zero-sequence data (`Flag_Input` bit 0x4,
/// `Flag_Z0_Input = 2`), which the fixture's own lines lack, so each one maps
/// in the multiconductor reader.
pub fn grown_authentic_feeder(lines: usize) -> Vec<u8> {
    let archive = include_bytes!("../../../../tests/data/sincal/1-LV-rural1--0-sw.sinx");
    let bytes = super::storage::database_bytes(archive).unwrap();
    let file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(file.path(), &bytes).unwrap();
    let connection = rusqlite::Connection::open(file.path()).unwrap();
    connection
        .execute_batch(&format!(
            "CREATE TEMP TABLE seq (k INTEGER);
             WITH RECURSIVE n(k) AS (SELECT 1 UNION ALL SELECT k + 1 FROM n WHERE k < {lines})
             INSERT INTO seq SELECT k FROM n;
             CREATE TEMP TABLE grown_node AS SELECT Node.*, k FROM Node, seq WHERE Node_ID = 3;
             UPDATE grown_node SET Node_ID = 100000 + k, Name = 'grown bus ' || k;
             ALTER TABLE grown_node DROP COLUMN k;
             INSERT INTO Node SELECT * FROM grown_node;
             CREATE TEMP TABLE grown_element AS
                 SELECT Element.*, k FROM Element, seq WHERE Element_ID = 20;
             UPDATE grown_element SET Element_ID = 200000 + k, Name = 'grown line ' || k,
                 Flag_Input = 7;
             ALTER TABLE grown_element DROP COLUMN k;
             INSERT INTO Element SELECT * FROM grown_element;
             CREATE TEMP TABLE grown_line AS SELECT Line.*, k FROM Line, seq WHERE Element_ID = 20;
             UPDATE grown_line SET Element_ID = 200000 + k,
                 Flag_Z0_Input = 2, r0 = 0.8, x0 = 0.3, c0 = 500;
             ALTER TABLE grown_line DROP COLUMN k;
             INSERT INTO Line SELECT * FROM grown_line;
             CREATE TEMP TABLE grown_terminal AS
                 SELECT Terminal.*, k FROM Terminal, seq WHERE Element_ID = 20;
             UPDATE grown_terminal SET
                 Terminal_ID = 300000 + 2 * k + TerminalNo,
                 Element_ID = 200000 + k,
                 Node_ID = CASE WHEN TerminalNo = 2 THEN 100000 + k
                                WHEN k = 1 THEN 3 ELSE 100000 + k - 1 END;
             ALTER TABLE grown_terminal DROP COLUMN k;
             INSERT INTO Terminal SELECT * FROM grown_terminal;
             DROP TABLE seq; DROP TABLE grown_node; DROP TABLE grown_element;
             DROP TABLE grown_line; DROP TABLE grown_terminal;"
        ))
        .unwrap();
    connection.serialize("main").unwrap().to_vec()
}
