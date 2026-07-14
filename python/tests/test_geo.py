"""Tests for the geographic layer surface and the AC OPF instance."""

from pathlib import Path

import pytest

import powerio as pio
from powerio.dist import parse_str as dist_parse_str

DATA = Path(__file__).resolve().parents[2] / "tests" / "data"

BUSCOORDS = "1, -89.6, 40.6\n2, -89.2, 39.8\n"

DSS_MASTER = (
    "New Circuit.c1 bus1=sourcebus basekv=12.47\n"
    "New Line.l1 bus1=sourcebus bus2=loadbus length=1 units=km\n"
)


def test_parse_geo_normalizes_a_buscoords_sidecar():
    parsed = pio.parse_geo(BUSCOORDS)
    doc = parsed["geojson"]
    assert doc["type"] == "FeatureCollection"
    assert doc["powerio_geo"]["space"] == "geographic"
    assert len(doc["features"]) == 2
    assert parsed["warnings"] == []


def test_parse_geo_rejects_input_without_coordinates():
    with pytest.raises(pio.PowerIOParseError):
        pio.parse_geo("not a geo file")


def test_network_apply_and_extract_round_trip():
    net = pio.parse_file(DATA / "case9.m")
    with pytest.raises(ValueError):
        net.geo_layer()

    placed, report = net.apply_geo_layer(BUSCOORDS)
    assert report["matched_buses"] == 2
    assert report["unmatched_features"] == 0
    # The input case is unchanged; the placed copy carries the layer.
    with pytest.raises(ValueError):
        net.geo_layer()
    layer = placed.geo_layer()
    assert len(layer["features"]) == 2


def test_dist_apply_returns_a_placed_copy():
    net = dist_parse_str(DSS_MASTER, "dss")
    placed, report = net.apply_geo_layer(
        "sourcebus, -89.6, 40.6\nloadbus, -89.2, 39.8\n"
    )
    assert report["matched_buses"] == 2
    layer = placed.geo_layer()
    assert len(layer["features"]) == 2


def test_acopf_instance_shape():
    net = pio.parse_file(DATA / "case9.m")
    instance = net.acopf_instance()
    assert instance["n_buses"] == 9
    assert instance["units"] == "PerUnit"
    assert len(instance["generators"]["c0"]) == 3
    assert len(instance["branches"]["g"]) == 9
    assert len(instance["buses"]["p_d"]) == 9

    native = net.acopf_instance(units="native")
    assert native["units"] == "Native"

    with pytest.raises(ValueError):
        net.acopf_instance(units="percent")
