"""The 0.8 python names alias their successors for one release with a
DeprecationWarning; both the dict and the hook go away at 1.0.0."""

import warnings

import powerio.dist
import pytest

import powerio


def test_the_08_network_name_warns_and_aliases():
    with warnings.catch_warnings(record=True) as caught:
        warnings.simplefilter("always")
        assert powerio.Network is powerio.BalancedNetwork
    assert any(issubclass(w.category, DeprecationWarning) for w in caught)


def test_the_08_dist_name_warns_and_aliases():
    with warnings.catch_warnings(record=True) as caught:
        warnings.simplefilter("always")
        assert powerio.dist.DistNetwork is powerio.dist.MulticonductorNetwork
    assert any(issubclass(w.category, DeprecationWarning) for w in caught)


def test_an_unknown_name_still_raises():
    with pytest.raises(AttributeError, match="no_such_name"):
        _ = powerio.no_such_name
