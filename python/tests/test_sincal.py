"""Public selections using synthetic records and the existing licensed SimBench fixture."""
import hashlib
import io
import json
import sqlite3
from pathlib import Path

import pytest
from powerio.dist import SincalReadOptions

import powerio

SQL = (Path(__file__).resolve().parents[2] / "tests/data/sincal/synthetic-multiconductor.sql").read_text()
PROFILE = """
UPDATE Load SET Flag_Lf=15, fP=1, fQ=1, fS=1, DayOpSer_ID=7;
CREATE TABLE OpSer (OpSer_ID INTEGER, Variant_ID INTEGER, Flag_Variant INTEGER,
  Flag_Typ INTEGER, Flag_Ser INTEGER, BaseT REAL, Power_a1 REAL, Power_b1 REAL,
  Reduce_a2 REAL, Reduce_b2 REAL);
INSERT INTO OpSer VALUES (7,1,1,3,1,0,0,0,0,0);
CREATE TABLE OpSerVal (OpSerVal_ID INTEGER, OpSer_ID INTEGER, Variant_ID INTEGER,
  Flag_Variant INTEGER, OpTime REAL, Flag_Curve INTEGER, Factor REAL, P REAL, Q REAL, Op_ID INTEGER);
INSERT INTO OpSerVal VALUES (1,7,1,1,0,1,NULL,6,-3,NULL),(2,7,1,1,12,2,NULL,18,9,NULL);
"""


def acquisition():
    # Header-shaped bytes deliberately do not pretend to be a real MDB.
    original = b"\0\x01\0\0Standard Jet DB\0original synthetic source"
    with sqlite3.connect(":memory:") as db:
        db.executescript(SQL + PROFILE + "UPDATE Version SET Version_No=11.5;")
        tables = []
        names = [row[0] for row in db.execute("SELECT name FROM sqlite_master WHERE type='table'")]
        for name in names:
            cursor = db.execute(f'SELECT * FROM "{name}"')
            tables.append({"name": name, "columns": [
                {"name": column[0], "native_type": "synthetic"} for column in cursor.description
            ], "rows": cursor.fetchall()})
    return original, json.dumps({
        "format": "powerio-sincal-tables", "version": 1, "transport": "access-mdbtools",
        "source": {"name": "synthetic.mdb", "bytes": len(original),
                   "sha256": hashlib.sha256(original).hexdigest()},
        "tools": {tool: "synthetic" for tool in ["mdb-json", "mdb-schema", "mdb-tables"]},
        "tables": tables, "excluded_tables": [], "absent_requested_tables": [],
    }).encode()


def test_memory_companions_preserve_source_and_selected_snapshot():
    original, records = acquisition()
    for hour, expected in [(0, 2000), (6, 4000), (12, 6000), (24, 2000)]:
        module = powerio.parse(io.BytesIO(original), name="synthetic.mdb",
            format="sincal-multiconductor",
            sincal_multiconductor=SincalReadOptions(variant=1, snapshot_hours=hour, acquired_tables="records.json"),
            named_buffers={"records.json": memoryview(records)})
        assert isinstance(module.value, powerio.dist.MulticonductorNetwork)
        assert module.value.loads[0]["p_nom"] == [expected] * 3
        assert powerio.emit(module, "sincal").artifacts[0].data == original
        restored = powerio.deserialize(io.StringIO(powerio.serialize(module).text))
        assert restored.value.loads == module.value.loads
        with pytest.raises(powerio.PowerIOError):
            powerio.emit(restored, "sincal")


def test_path_acquisition_root_is_explicit(tmp_path):
    original, records = acquisition()
    (tmp_path / "case").mkdir()
    source = tmp_path / "case/original.mdb"
    source.write_bytes(original)
    (tmp_path / "records.json").write_bytes(records)
    selection = SincalReadOptions(snapshot_hours=6, acquired_tables="../records.json")
    with pytest.raises(powerio.PowerIOError):
        powerio.parse(source, format="sincal-multiconductor", sincal_multiconductor=selection)
    module = powerio.parse(source, format="sincal-multiconductor", sincal_multiconductor=selection,
                           acquisition_root=tmp_path)
    assert module.value.loads[0]["p_nom"] == [4000] * 3
    assert powerio.emit(module, "sincal").artifacts[0].data == original


@pytest.mark.parametrize("selection", [
    SincalReadOptions(acquired_tables="records.json"),  # no implicit snapshot
    SincalReadOptions(variant=999, snapshot_hours=6, acquired_tables="records.json"),
    SincalReadOptions(snapshot_hours=float("nan"), acquired_tables="records.json"),
    SincalReadOptions(snapshot_hours=6, acquired_tables="missing.json"),
    SincalReadOptions(snapshot_hours=6, acquired_tables="../records.json"),
])
def test_invalid_selections_and_missing_companions_are_not_ignored(selection):
    original, records = acquisition()
    with pytest.raises(powerio.PowerIOError):
        powerio.parse(original, format="sincal-multiconductor", sincal_multiconductor=selection,
                      named_buffers={"records.json": records})


def test_conflicting_family_and_wrong_original_are_rejected():
    original, records = acquisition()
    selection = SincalReadOptions(snapshot_hours=6, acquired_tables="records.json")
    for format in [None, "sincal-balanced", "dss"]:
        with pytest.raises(powerio.PowerIOError) as error:
            powerio.parse(original, format=format, sincal_multiconductor=selection,
                          named_buffers={"records.json": records})
        assert error.value.code == "REQUEST.PARSE.SINCAL_OPTIONS_PROFILE"
    with pytest.raises(powerio.PowerIOError, match="do not match"):
        powerio.parse(original + b"changed", format="sincal-multiconductor", sincal_multiconductor=selection,
                      named_buffers={"records.json": records})


def test_public_option_types_and_source_modes(tmp_path):
    for kwargs in [{"variant": True}, {"variant": 1.5}, {"snapshot_hours": "6"}, {"acquired_tables": b"x"}]:
        with pytest.raises(TypeError):
            SincalReadOptions(**kwargs)
    with pytest.raises(TypeError):
        powerio.parse(b"", sincal_multiconductor={"snapshot_hours": 6})
    with pytest.raises(TypeError):
        powerio.parse(b"", named_buffers={"x": 12})
    with pytest.raises(ValueError):
        powerio.parse(b"", acquisition_root=tmp_path)
    with pytest.raises(ValueError):
        powerio.parse(tmp_path / "case", named_buffers={})


def test_compatibility_is_opt_in_and_assumptions_are_structured():
    original, encoded = acquisition()
    records = json.loads(encoded)
    records['excluded_tables'] = ['ULFNodeResult']
    table = next(t for t in records['tables'] if t['name'] == 'Infeeder')
    fields = ['Flag_LfLimit', 'Flag_LfCtrl', 'Flag_Qctrl', 'Flag_Macro', 'Kr']
    for field in fields:
        table['rows'][0][next(i for i, c in enumerate(table['columns']) if c['name'] == field)] = None
    options = dict(format='sincal-multiconductor', named_buffers={'records.json': json.dumps(records).encode()})
    with pytest.raises(powerio.PowerIOError):
        powerio.parse(original, sincal_multiconductor=SincalReadOptions(snapshot_hours=6, acquired_tables='records.json'), **options)
    module = powerio.parse(original, sincal_multiconductor=SincalReadOptions(snapshot_hours=6,
        acquired_tables='records.json', assume_inactive_source_controls=True), **options)
    assumption = next(d for d in powerio.diagnostic_records(module.diagnostics)
                      if d['code'] == 'READ.DIST.SINCAL_ASSUMED_INACTIVE_SOURCE_CONTROLS')
    assert set(assumption['details']['fields']) == set(fields)
    assert assumption['details']['assumed_value'] == 0
    assert assumption['details']['native_semantics_verified'] is False
    excluded = next(d['details'] for d in powerio.diagnostic_records(module.diagnostics) if 'excluded_tables' in d.get('details', {}))
    assert excluded['row_count'] is None
    assert excluded['inventory_complete'] is False
    restored = powerio.deserialize(io.StringIO(powerio.serialize(module).text))
    assert [d.get('details') for d in powerio.diagnostic_records(restored.diagnostics)] == [d.get('details') for d in powerio.diagnostic_records(module.diagnostics)]
    assert powerio.emit(module, 'sincal').artifacts[0].data == original
    with pytest.raises(TypeError):
        SincalReadOptions(assume_inactive_source_controls=1)


def test_balanced_selections_use_the_balanced_family(tmp_path):
    import zipfile
    archive = Path(__file__).resolve().parents[2] / 'tests/data/sincal/1-LV-rural1--0-sw.sinx'
    native = powerio.parse(archive, format='sincal-balanced',
                          sincal_balanced=powerio.SincalBalancedReadOptions(variant=1))
    assert isinstance(native.value, powerio.BalancedNetwork)
    assert native.value.n_buses == 15
    # Runtime derivative of the existing licensed SimBench fixture, not new vendored data.
    with zipfile.ZipFile(archive) as z:
        database = z.read('1-LV-rural1--0-sw_files/database.db')
    path = tmp_path / 'legacy.db'
    path.write_bytes(database)
    with sqlite3.connect(path) as db:
        db.executescript('''UPDATE Version SET Version_No=11.5;
          UPDATE CalcParameter SET Temp_Cond=20; UPDATE VoltageLevel SET Temp_Cable=NULL,Flag_Volt=NULL;
          UPDATE Load SET DayOpSer_ID=NULL,WeekOpSer_ID=NULL,YearOpSer_ID=NULL,IncrSer_ID=NULL;
          UPDATE Load SET DayOpSer_ID=901,Flag_Lf=1,Flag_LoadType=2,fP=2,fQ=3 WHERE Element_ID=1;
          DROP TABLE IF EXISTS OpSerVal; DROP TABLE IF EXISTS OpSer;
          CREATE TABLE OpSer(OpSer_ID INTEGER,Variant_ID INTEGER,Flag_Ser INTEGER,Flag_Typ INTEGER,
            BaseT REAL,Power_a1 REAL,Power_b1 REAL,Reduce_a2 REAL,Reduce_b2 REAL);
          INSERT INTO OpSer VALUES(901,1,1,3,24,0,0,0,0);
          CREATE TABLE OpSerVal(OpSerVal_ID INTEGER,Variant_ID INTEGER,OpSer_ID INTEGER,OpTime REAL,
            Flag_Curve INTEGER,Op_ID INTEGER,P REAL,Q REAL);
          INSERT INTO OpSerVal VALUES(801,1,901,0,1,NULL,12,-4),(802,1,901,12,1,NULL,24,8);
        ''')
        original, encoded = acquisition()
        records = json.loads(encoded)
        tables = []
        for (name,) in db.execute("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'"):
            cursor = db.execute('SELECT * FROM "' + name.replace('"', '""') + '"')
            # Only acquired electrical tables are required; native blobs/results are outside this synthetic transport.
            rows = cursor.fetchall()
            if any(isinstance(v, bytes) for row in rows for v in row):
                continue
            tables.append({'name': name, 'columns': [{'name': c[0], 'native_type': 'synthetic'} for c in cursor.description], 'rows': rows})
        records['tables'] = tables
    encoded = json.dumps(records).encode()
    for hours, expected in [(0, .024), (6, .036), (12, .048), (24, .024)]:
        selection = powerio.SincalBalancedReadOptions(variant=1, snapshot_hours=hours, acquired_tables='records.json')
        module = powerio.parse(original, format='sincal-balanced', sincal_balanced=selection,
                               named_buffers={'records.json': encoded})
        assert isinstance(module.value, powerio.BalancedNetwork)
        assert powerio.emit(module, 'sincal').artifacts[0].data == original
        wire = json.loads(powerio.serialize(module).text)
        loads = wire['value']['data']['loads']
        load = next(x for x in loads if x.get('uid') == 'sincal:element:1')
        assert load['p'] == pytest.approx(expected)
        assert any(d['code'] == 'READ.SINCAL.VALUE_DEFAULTED' and 'default' in d['details'] for d in powerio.diagnostic_records(module.diagnostics))
    with pytest.raises(powerio.PowerIOError):
        powerio.parse(original, format='sincal-multiconductor', sincal_balanced=selection,
                      named_buffers={'records.json': encoded})
    with pytest.raises(TypeError):
        powerio.SincalBalancedReadOptions(variant=True)
