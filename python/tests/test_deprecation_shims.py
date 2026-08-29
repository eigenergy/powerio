"""1.0 carries no deprecated python names: the 0.8 aliases and their
module hooks are gone, and an unknown attribute raises plainly."""

import powerio.dist
import pytest

import powerio


def test_the_08_network_alias_is_gone():
    with pytest.raises(AttributeError, match="Network"):
        _ = powerio.Network


def test_the_08_dist_alias_is_gone():
    with pytest.raises(AttributeError, match="DistNetwork"):
        _ = powerio.dist.DistNetwork


def test_an_unknown_name_still_raises():
    with pytest.raises(AttributeError, match="no_such_name"):
        _ = powerio.no_such_name


def test_the_stored_module_name_is_gone():
    # StoredModule was the pre-0.10 name for PioModule; the rename carries no
    # alias.
    with pytest.raises(AttributeError, match="StoredModule"):
        _ = powerio.StoredModule
