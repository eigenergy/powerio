"""Build a tiny synthetic pandapower 3ph net, run runpp_3ph, write JSON.

Usage: python -X utf8 build_tiny_3ph.py
Writes tiny_3ph.json next to this script and asserts the run converged.
"""
import os
import warnings

import pandapower as pp
from pandapower.pf.runpp_3ph import runpp_3ph

warnings.filterwarnings("ignore")
HERE = os.path.dirname(os.path.abspath(__file__))


def build():
    # no std types: they add about 25 KB and the elements below use explicit parameters
    net = pp.create_empty_network(sn_mva=1.0, add_stdtypes=False)
    b_mv = pp.create_bus(net, vn_kv=20.0, name="mv")
    b_lv = pp.create_bus(net, vn_kv=0.4, name="lv")
    b2 = pp.create_bus(net, vn_kv=0.4, name="lv2")
    b3 = pp.create_bus(net, vn_kv=0.4, name="lv3")
    b4 = pp.create_bus(net, vn_kv=0.4, name="lv4")
    pp.create_ext_grid(net, b_mv, vm_pu=1.0, s_sc_max_mva=1000, rx_max=0.1,
                       x0x_max=1.0, r0x0_max=0.1)
    pp.create_transformer_from_parameters(
        net, b_mv, b_lv, sn_mva=0.4, vn_hv_kv=20.0, vn_lv_kv=0.4,
        vkr_percent=1.325, vk_percent=4.0, pfe_kw=0.95, i0_percent=0.2375,
        shift_degree=150, vector_group="Dyn", vk0_percent=4.0,
        vkr0_percent=1.325, mag0_percent=100, mag0_rx=0.0, si0_hv_partial=0.9)
    for fb, tb in ((b_lv, b2), (b2, b3)):
        pp.create_line_from_parameters(
            net, fb, tb, length_km=0.1, r_ohm_per_km=0.207, x_ohm_per_km=0.08,
            c_nf_per_km=261.0, max_i_ka=0.27, r0_ohm_per_km=0.828,
            x0_ohm_per_km=0.32, c0_nf_per_km=261.0)
    pp.create_asymmetric_load(net, b3, p_a_mw=0.02, p_b_mw=0.01, p_c_mw=0.005,
                              q_a_mvar=0.005, q_b_mvar=0.002, q_c_mvar=0.001)
    pp.create_asymmetric_sgen(net, b2, p_a_mw=0.0, p_b_mw=0.004, p_c_mw=0.0)
    # a balanced delta load, a shunt and a closed bus-bus switch to a wye load
    pp.create_load(net, b3, p_mw=0.006, q_mvar=0.002, type="delta")
    pp.create_shunt(net, b2, q_mvar=0.003, p_mw=0.0005)
    pp.create_switch(net, b3, b4, et="b", closed=True, name="S1", type="CB", in_ka=0.4)
    pp.create_load(net, b4, p_mw=0.003, q_mvar=0.001, type="wye")
    # an sgen without a type, which runpp_3ph leaves out of the solve
    pp.create_sgen(net, b2, p_mw=0.001)
    return net


if __name__ == "__main__":
    net = build()
    runpp_3ph(net)
    assert net.converged
    assert (net.res_bus_3ph.vm_a_pu.round(6) != net.res_bus_3ph.vm_b_pu.round(6)).any()
    path = os.path.join(HERE, "tiny_3ph.json")
    # LF line endings on every platform, as the repository requires
    with open(path, "w", encoding="utf-8", newline="\n") as f:
        f.write(pp.to_json(net))
    print(net.res_bus_3ph[["vm_a_pu", "vm_b_pu", "vm_c_pu", "unbalance_percent"]])
    with open(path, "rb") as f:
        data = f.read()
    print("bytes", len(data), "lines", data.count(b"\n"))
    back = pp.from_json(path)
    runpp_3ph(back)
    assert (back.res_bus_3ph.round(9) == net.res_bus_3ph.round(9)).all().all()
    print("from_json round trip reruns identically")
