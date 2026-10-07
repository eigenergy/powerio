"""Public selections and companion acquisition; all input data here is synthetic."""
import hashlib
import io
import json
import sqlite3
from pathlib import Path

import pytest

import powerio
from powerio.dist import SincalReadOptions

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
