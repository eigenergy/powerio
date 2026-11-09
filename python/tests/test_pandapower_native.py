"""Native pandapower imports must accept canonical output with default options."""

import io

import pytest

import powerio


def native_network(pp):
    net = pp.create_empty_network(sn_mva=100, f_hz=50)
    for index, active in ((42, True), (7, True), (19, False)):
        pp.create_bus(net, 110, index=index, in_service=active)
    pp.create_ext_grid(net, 42, vm_pu=1.03, va_degree=2)
    pp.create_load(
        net, 7, p_mw=20, q_mvar=5, scaling=0.5,
        const_z_p_percent=30, const_i_p_percent=20,
        const_z_q_percent=50, const_i_q_percent=30,
    )
    pp.create_line_from_parameters(
        net, 42, 7, length_km=10, r_ohm_per_km=0.1,
        x_ohm_per_km=0.4, c_nf_per_km=0, max_i_ka=1,
    )
    return net


def canonical(pp, net):
    module = powerio.parse(io.StringIO(pp.to_json(net)), format="pandapower-json")
    restored = powerio.deserialize(io.StringIO(powerio.serialize(module).text))
    return powerio.emit(restored, "pandapower-json").text


def test_native_default_import_preserves_split_zip_load_columns():
    # Optional in the binding-only suite; Validation CI requires this import.
    pp = pytest.importorskip("pandapower")
    source = native_network(pp)
    output = canonical(pp, source)
    native = pp.from_json_string(output)
    for column in (
        "p_mw", "q_mvar", "scaling", "const_z_p_percent", "const_i_p_percent",
        "const_z_q_percent", "const_i_q_percent",
    ):
        assert native.load[column].tolist() == source.load[column].tolist()
    assert "const_z_percent" not in native.load
    assert "const_i_percent" not in native.load
    pp.runpp(native, init="flat", calculate_voltage_angles=True, numba=False)
    assert native.converged

