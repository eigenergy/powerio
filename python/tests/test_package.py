"""Tests for the `powerio.Package` handle class."""

from pathlib import Path

import pytest

import powerio as pio

DATA = Path(__file__).resolve().parents[2] / "tests" / "data"

# Two buses, one producer, one consumer, two time periods; mirrors the GOC3
# fixture in powerio-pkg/tests/roundtrip.rs so the materialized values match
# the Rust assertions.
GOC3_SRC = """{
  "network": {
    "general": {"base_norm_mva": 100.0},
    "bus": [
      {"uid": "bus_00", "base_nom_volt": 230.0, "vm_lb": 0.95, "vm_ub": 1.05, "initial_status": {"vm": 1.0, "va": 0.0}},
      {"uid": "bus_01", "base_nom_volt": 115.0, "vm_lb": 0.9, "vm_ub": 1.1, "initial_status": {"vm": 1.0, "va": 0.0}}
    ],
    "simple_dispatchable_device": [
      {"uid": "prod", "bus": "bus_00", "device_type": "producer", "startup_cost": 5.0, "shutdown_cost": 6.0, "initial_status": {"on_status": 1, "p": 0.1, "q": 0.0}},
      {"uid": "load", "bus": "bus_01", "device_type": "consumer", "initial_status": {"on_status": 1, "p": 0.4, "q": 0.1}}
    ]
  },
  "time_series_input": {
    "general": {"time_periods": 2, "interval_duration": [1.0, 2.0]},
    "simple_dispatchable_device": [
      {"uid": "prod", "p_lb": [0.1, 0.2], "p_ub": [1.0, 0.8], "q_lb": [-0.2, -0.1], "q_ub": [0.4, 0.3], "cost": [[[10.0, 0.1]], [[20.0, 0.2]]]},
      {"uid": "load", "p_lb": [0.0, 0.0], "p_ub": [0.4, 0.3], "q_lb": [0.0, 0.0], "q_ub": [0.1, 0.2], "cost": [[[0.0, 0.4]], [[0.0, 0.3]]]}
    ]
  }
}"""


def test_package_from_file_balanced_roundtrip():
    pkg = pio.Package.from_file(DATA / "case9.m")
    assert pkg.model_kind == "balanced"
    assert pkg.as_balanced().n_buses == 9
    back = pio.Package.from_json(pkg.to_json())
    assert back.model_kind == "balanced"
    assert repr(pkg).startswith("Package(model_kind=balanced")


def test_package_operating_points_and_materialize():
    pkg = pio.Package.from_str(GOC3_SRC, from_="goc3-json")
    points = pkg.operating_points()
    assert points["time_axis"]["periods"] == 2
    assert points["time_axis"]["duration_hours"] == [1.0, 2.0]

    static_pkg = pkg.materialize_operating_point(1)
    assert static_pkg.operating_points() is None
    net = static_pkg.as_balanced()
    dense = net.to_dense()
    assert dense.gen.pmax[0] == pytest.approx(80.0)
    assert sum(dense.demand.pd) == pytest.approx(30.0)


def test_package_set_operating_points_attaches_and_clears():
    pkg = pio.Package.from_file(DATA / "case9.m")
    assert pkg.operating_points() is None

    series = {
        "time_axis": {"periods": 1, "duration_hours": [1.0]},
        "points": [
            {
                "index": 0,
                "updates": [
                    {
                        "element": {"table": "generators", "source_uid": "generators:0"},
                        "fields": {"pg": 1.5},
                    }
                ],
            }
        ],
    }
    pkg.set_operating_points(series)
    assert pkg.operating_points() == series
    assert pkg.validation()["status"] == "ok"

    static_pkg = pkg.materialize_operating_point(0)
    assert static_pkg.as_balanced().generators[0]["pg"] == pytest.approx(1.5)

    invalid_series = {
        **series,
        "points": [
            {
                "index": 0,
                "updates": [
                    {
                        "element": {
                            "table": "generators",
                            "source_uid": "missing",
                        },
                        "fields": {"pg": 1.5},
                    }
                ],
            }
        ],
    }
    pkg.set_operating_points(invalid_series)
    assert pkg.validation()["status"] == "error"

    pkg.set_operating_points(None)
    assert pkg.operating_points() is None
    assert pkg.validation()["status"] == "ok"


def test_package_study_and_materialize_commit():
    import json

    pkg = pio.Package.from_file(DATA / "case9.m")
    doc = json.loads(pkg.to_json())
    doc["study"] = {
        "label": "binding study",
        "commits": [
            {
                "label": "load step",
                "edits": [
                    {
                        "kind": "demand_delta",
                        "bus": {"table": "buses", "source_uid": "buses:0"},
                        "p_mw": 7.0,
                        "q_mvar": 3.0,
                    }
                ],
            }
        ],
    }
    pkg = pio.Package.from_json(json.dumps(doc))

    assert pkg.study()["label"] == "binding study"
    static_pkg = pkg.materialize_study_commit(0)
    assert static_pkg.study() is None
    assert static_pkg.operating_points() is None
    loads = static_pkg.as_balanced().loads
    assert any(
        load["uid"] == "study:load:buses:0"
        and load["p"] == pytest.approx(7.0)
        and load["q"] == pytest.approx(3.0)
        for load in loads
    )


def test_package_without_operating_points():
    pkg = pio.Package.from_file(DATA / "case9.m")
    assert pkg.operating_points() is None
    with pytest.raises(ValueError):
        pkg.materialize_operating_point(0)


def test_package_validate_and_diagnostics():
    pkg = pio.Package.from_file(DATA / "case9.m")
    pkg.validate()
    assert isinstance(pkg.validation()["status"], str)
    assert isinstance(pkg.diagnostics(), list)


def test_package_multiconductor_kind_and_lowering():
    pkg = pio.Package.from_file(DATA / "dist" / "micro" / "fourwire_linecode.dss")
    assert pkg.model_kind == "multiconductor"
    assert pkg.as_multiconductor().n_buses > 0
    with pytest.raises(ValueError):
        pkg.as_balanced()

    report = pkg.multiconductor_to_balanced_preflight()
    assert isinstance(report, dict)

    lowered = pkg.lower_multiconductor_to_balanced()
    assert lowered.model_kind == "balanced"
    assert lowered.as_balanced().n_buses > 0


def test_package_from_network_constructors():
    net = pio.parse_file(DATA / "case9.m")
    pkg = pio.Package.from_balanced(net, include_solver_metadata=True)
    assert pkg.model_kind == "balanced"

    dn = pio.dist.parse_file(DATA / "dist" / "micro" / "fourwire_linecode.dss")
    dpkg = pio.Package.from_multiconductor(dn)
    assert dpkg.model_kind == "multiconductor"
    # Preflight on a balanced package names the wrong model kind.
    with pytest.raises(ValueError):
        pkg.multiconductor_to_balanced_preflight()


def test_package_invalid_json_raises_value_error():
    with pytest.raises(ValueError):
        pio.Package.from_json("{}")


def test_package_declares_payload_schema_and_row_identity():
    import json

    pkg = pio.Package.from_file(DATA / "case9.m")
    doc = json.loads(pkg.to_json())
    assert doc["schema_version"] == "0.1.1"
    assert doc["payload_schema"].endswith("/pio-payload-balanced/1")
    assert doc["payload_schema_version"] == "1.2.0"
    # Every payload row carries an identity; case9 has no source uids, so they
    # are synthesized from the build position.
    assert doc["model"]["balanced_network"]["generators"][0]["uid"] == "generators:0"
    assert pkg.as_balanced().generators[0]["uid"] == "generators:0"


def test_package_materialize_rejects_unknown_identity():
    import json

    pkg = pio.Package.from_str(GOC3_SRC, from_="goc3-json")
    doc = json.loads(pkg.to_json())
    update = doc["operating_points"]["points"][0]["updates"][0]
    update["element"]["source_uid"] = "no-such-uid"
    del update["element"]["row"]
    broken = pio.Package.from_json(json.dumps(doc))
    with pytest.raises(ValueError, match="unknown identity"):
        broken.materialize_operating_point(0)
