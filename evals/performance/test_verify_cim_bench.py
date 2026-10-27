"""Checks for the difference records in verify_cim_bench.py.

These need only the standard library and pytest:
`python -m pytest evals/performance/test_verify_cim_bench.py`.
"""

import importlib.util
import json
import math
from pathlib import Path

_SPEC = importlib.util.spec_from_file_location(
    "verify_cim_bench", Path(__file__).with_name("verify_cim_bench.py")
)
verify_cim_bench = importlib.util.module_from_spec(_SPEC)
_SPEC.loader.exec_module(verify_cim_bench)
differences = verify_cim_bench.differences


def _writable(changes):
    # The packet is written with allow_nan=False; a record must survive that.
    return json.loads(json.dumps(changes, allow_nan=False))


def test_nan_on_both_sides_is_unchanged():
    assert differences(math.nan, math.nan) == []
    assert differences({"b": {"vm": math.nan}}, {"b": {"vm": float("nan")}}) == []
    assert differences([1.0, math.nan], [1.0, math.nan]) == []


def test_nan_against_a_number_is_reported_as_a_string():
    changes = differences({"b": {"vm": math.nan}}, {"b": {"vm": 1.0}})
    assert changes == [{"path": "/b/vm", "before": "NaN", "after": 1.0}]
    assert _writable(changes) == changes
    assert differences(2, math.nan, "/x") == [
        {"path": "/x", "before": 2, "after": "NaN"}
    ]


def test_nan_against_a_missing_value_is_reported():
    changes = differences({"b": {"va": math.nan}}, {"b": {}})
    assert changes == [{"path": "/b/va", "before": "NaN", "after": None}]
    assert _writable(changes) == changes
    assert differences(None, math.nan, "/y") == [
        {"path": "/y", "before": None, "after": "NaN"}
    ]


def test_opposite_infinities_are_reported_as_strings():
    changes = differences({"rate_a": math.inf}, {"rate_a": -math.inf})
    assert changes == [{"path": "/rate_a", "before": "Infinity", "after": "-Infinity"}]
    assert _writable(changes) == changes
    assert differences(math.inf, math.inf) == []


def test_non_finite_values_inside_a_reported_value_are_encoded():
    changes = differences({"q": [math.nan, math.inf]}, {"q": [1.0]})
    assert changes == [{"path": "/q", "before": ["NaN", "Infinity"], "after": [1.0]}]
    assert _writable(changes) == changes


def test_an_encoded_rating_is_an_unexpected_difference():
    changes = {
        "branches": differences({"l1": {"rate_a": math.nan}}, {"l1": {"rate_a": 5.0}})
    }
    accepted, unexpected = verify_cim_bench.classify_differences(
        "svedala_igm_cgmes_3", changes, []
    )
    assert accepted == []
    assert unexpected == [
        {"table": "branches", "path": "/l1/rate_a", "before": "NaN", "after": 5.0}
    ]
