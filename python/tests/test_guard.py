"""Public wrappers convert native panics into ordinary coded exceptions."""

import pytest
from powerio._powerio import PanicException

import powerio


class _PanickingNetwork:
    @property
    def n_buses(self):
        raise PanicException("test delegated property panic")

    def to_normalized(self):
        raise PanicException("test method panic")


@pytest.mark.parametrize("entry_point", ["property", "method"])
def test_balanced_network_preserves_panic_error_boundary(entry_point):
    network = powerio.BalancedNetwork(_PanickingNetwork())
    with pytest.raises(powerio.PowerIOError) as raised:
        if entry_point == "property":
            _ = network.n_buses
        else:
            network.to_normalized()
    assert raised.value.code == "BIND.PY.PANIC"
    assert isinstance(raised.value, Exception)
