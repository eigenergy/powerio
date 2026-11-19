"""Native pandapower imports must accept canonical output with default options."""

import io
import json

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


def test_native_bus_voltages_survive_ir_without_a_convergence_claim():
    pp = pytest.importorskip("pandapower")
    source = native_network(pp)
    pp.runpp(source, calculate_voltage_angles=True, numba=False)
    output = canonical(pp, source)
    raw = json.loads(output)["_object"]
    assert "converged" not in raw
    assert json.loads(raw["res_bus"]["_object"])["columns"] == ["vm_pu", "va_degree"]
    native = pp.from_json_string(output)
    assert not native.get("converged", False)
    assert native.bus.index.tolist() == source.bus.index.tolist()
    assert native.bus.in_service.tolist() == source.bus.in_service.tolist()
    for index in (42, 7):
        for column in ("vm_pu", "va_degree"):
            assert native.res_bus.at[index, column] == pytest.approx(
                source.res_bus.at[index, column], abs=1e-12
            )


@pytest.mark.parametrize("start", [-10.0, 0.0, 10.0])
@pytest.mark.parametrize("element_type", ["gen", "ext_grid"])
def test_native_piecewise_objective_preserves_nonzero_first_power(start, element_type):
    pp = pytest.importorskip("pandapower")
    import numpy as np
    from pandapower.converter.pypower import to_ppc
    from pandapower.pypower.idx_cost import NCOST
    from pandapower.pypower.totcost import totcost

    source = native_network(pp)
    if element_type == "gen":
        element = pp.create_gen(source, 7, p_mw=10, vm_pu=1,
                                min_p_mw=-100, max_p_mw=100,
                                min_q_mvar=-100, max_q_mvar=100, controllable=True)
    else:
        element = source.ext_grid.index[0]
    pp.create_pwl_cost(source, element, element_type,
                       [[start, start + 10, 5], [start + 10, start + 20, 8]])
    module = powerio.parse(io.StringIO(pp.to_json(source)), format="pandapower-json")
    costs = [g["cost"] for g in module.value.generators if g["cost"] is not None]
    assert len(costs) == 1
    cost = costs[0]
    typed_curve = np.array([[cost["model"], cost["startup"], cost["shutdown"],
                             cost["ncost"], *cost["coeffs"]]])
    output = pp.from_json_string(canonical(pp, source))
    for network in (source, output):
        ppc = to_ppc(network, init="flat", mode="opf")
        # The only curve with three points is the synthetic cost above.
        curve = ppc["gencost"][ppc["gencost"][:, NCOST] == 3]
        assert curve.shape[0] == 1
        for power, expected in ((start, 5 * start),
                                (start + 5, 5 * start + 25),
                                (start + 15, 5 * start + 90)):
            assert totcost(curve, np.array([power]))[0] == pytest.approx(expected)
            assert totcost(typed_curve, np.array([power]))[0] == pytest.approx(expected)

