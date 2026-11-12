"""Electrical oracle for the three-phase pandapower reader.

Per case a network is built in pandapower (no std types), solved with
`runpp_3ph`, written with `pandapower.to_json`, converted by the PowerIO CLI
(`powerio convert <json> --to dss`) and solved by OpenDSS. Three decks are
compared per phase, in per unit of the bus nominal line-to-neutral voltage:

  pp      res_bus_3ph of runpp_3ph
  powerio the deck the CLI writes (only solver settings are patched in)
  hand    a deck written here from the pandapower tables, with an ideal source
          like the reader's, and with the transformer zero-sequence T-model
          reproduced by independent formulas (pd2ppc_zero.py)
  oracle  the hand deck with a source that mirrors runpp_3ph (positive
          sequence ideal, Z2 and Z0 from the short-circuit data)

`hand` against `oracle` is the known ideal-source error of the reader, and
`oracle` against `pp` checks the hand deck itself. A case passes when

  powerio vs hand <= 1e-6 pu      (isolates reader and writer defects)
  powerio vs pp   <= (hand vs pp) + 1e-6 pu
  oracle vs pp    <= 5e-6 pu      (the hand deck is sound)

Angles use 1e-4 degrees in place of 1e-6 pu. A case marked `xfail` records a
known gap: it must fail, and the script reports XPASS if it stops failing.

    python evals/validation/validate_pandapower_3ph.py [--count] [--workdir DIR]
                                                       [--keep] [case ...]

The CLI is $POWERIO_BIN, else target/{release,debug}/powerio, else `powerio`
on PATH. Appends `pandapower-3ph/<case>\tpp3ph\t<mark>` to $PIO_RESULTS_TSV
when set. It needs pandapower 3.5.6 and opendssdirect.py from
evals/validation/requirements.txt.
pandapower 3.5 has the `type` of `sgen` and the zero-sequence columns of
`trafo`. run_validation.sh does not run this script.
"""

from __future__ import annotations

import argparse
import copy
import io
import json
import math
import os
import re
import shutil
import subprocess
import sys
import tempfile
import warnings
from contextlib import redirect_stdout
from pathlib import Path

import numpy as np
import pandas as pd

warnings.filterwarnings("ignore")
import pandapower as pp  # noqa: E402
import pandapower.networks as pn  # noqa: E402
from opendssdirect import dss  # noqa: E402

REPO = Path(__file__).resolve().parents[2]
VM_HAND = 1e-6  # PowerIO deck against the hand deck, pu
VM_SLACK = 1e-6  # added to the ideal-source error against pandapower, pu
VM_ORACLE = 5e-6  # hand deck with a pandapower-like source against pandapower, pu
VA_TOL = 1e-4  # degrees, in place of the pu bounds
VA_ORACLE = 5e-4  # degrees, for the hand deck against pandapower
# Looser hand-deck bound (pu, degrees) for a case whose hand deck is known to be inexact.
ORACLE_TOL = {
    # The OpenDSS hand deck acts like a no-load shunt at the LV terminal.
    # runpp_3ph puts the no-load branch in the middle of its T model.
    "dyn_noload_pfe_i0": (2e-4, 1e-2),
    # A 1 percent loss share moves the angle further.
    "dyn_noload_loss_above_i0": (2e-4, 2e-2),
    "std_type_trafo": (2e-4, 1e-2),  # the library type has pfe_kw and i0_percent
}
# Positive and negative sequence of a zero-sequence-only hand-deck shunt, ohm.
# 1e9 keeps the MV and HV errors below 1e-8 pu.
ZERO_SHUNT_OPEN = 1e9
# The first OpenDSS node of a YNyn LV neutral. Each transformer gets its own.
NEUTRAL_NODE = 10
# The runpp_3ph default switch_rx_ratio, which the JSON does not store.
SWITCH_RX_RATIO = 2.0
# runpp_3ph per-unitizes a switch on vn^2/sn while lines use vn^2/(3 sn), so the
# switch acts as z_ohm/3 per phase (build_branch._calc_switch_parameter). The
# pandapower docstring calls this path not tested with three phases. The oracle
# matches pandapower to 6e-9 with it.
SWITCH_Z_SCALE = 1 / 3

# Orders of the LV terminals that reproduce a Dyn shift_degree in OpenDSS.
DYN_ORDER = {30: "1.2.3", 150: "3.1.2", 270: "2.3.1", 330: "1.3.2", 210: "2.1.3", 90: "3.2.1"}
# Orders of the HV terminals. Swapping two terminals on both windings reverses
# the sequence the core sees, which gives the 330 degree family.
DYN_HV_ORDER = {30: "1.2.3", 150: "1.2.3", 270: "1.2.3", 330: "1.3.2", 210: "1.3.2", 90: "1.3.2"}
YNYN_ORDER = {0: "1.2.3", 120: "3.1.2", 240: "2.3.1"}


# --------------------------------------------------------------------------
# pandapower builders (std-type free)
# --------------------------------------------------------------------------


def new_net():
    return pp.create_empty_network(sn_mva=1.0, add_stdtypes=False)


def add_grid(net, bus, **kw):
    args = dict(vm_pu=1.0, va_degree=0.0, s_sc_max_mva=100.0, rx_max=0.1, x0x_max=1.2, r0x0_max=0.2)
    args.update(kw)
    pp.create_ext_grid(net, bus, **args)


def add_line(net, a, b, km, **kw):
    args = dict(
        r_ohm_per_km=0.207, x_ohm_per_km=0.08, c_nf_per_km=260.0, max_i_ka=0.3,
        r0_ohm_per_km=0.828, x0_ohm_per_km=0.32, c0_nf_per_km=150.0,
    )
    args.update(kw)
    return pp.create_line_from_parameters(net, a, b, length_km=km, **args)


def add_trafo(net, hv, lv, group="Dyn", shift=30.0, **kw):
    args = dict(
        sn_mva=0.4, vn_hv_kv=20.0, vn_lv_kv=0.4, vk_percent=6.0, vkr_percent=1.425,
        pfe_kw=0.0, i0_percent=0.0, vk0_percent=6.0, vkr0_percent=1.425,
        mag0_percent=1e9, mag0_rx=0.0, si0_hv_partial=0.9,
    )
    args.update(kw)
    return pp.create_transformer_from_parameters(
        net, hv, lv, shift_degree=shift, vector_group=group, **args
    )


def add_asym(net, bus, a, b, c, **kw):
    """Per-phase (p_mw, q_mvar) pairs."""
    return pp.create_asymmetric_load(
        net, bus, p_a_mw=a[0], q_a_mvar=a[1], p_b_mw=b[0], q_b_mvar=b[1],
        p_c_mw=c[0], q_c_mvar=c[1], **kw
    )


def feeder():
    """0.4 kV source, two lines, two asymmetric wye loads."""
    net = new_net()
    b = [pp.create_bus(net, vn_kv=0.4) for _ in range(3)]
    add_grid(net, b[0])
    add_line(net, b[0], b[1], 0.3)
    add_line(net, b[1], b[2], 0.2)
    add_asym(net, b[1], (0.010, 0.003), (0.004, 0.001), (0.0, 0.0))
    add_asym(net, b[2], (0.002, 0.0), (0.012, 0.004), (0.006, 0.002))
    return net


def with_trafo(group="Dyn", shift=30.0, **kw):
    """20 kV source, one transformer, one 0.4 kV line with an unbalanced load."""
    net = new_net()
    hv, lv, b2 = (pp.create_bus(net, vn_kv=v) for v in (20.0, 0.4, 0.4))
    add_grid(net, hv)
    add_trafo(net, hv, lv, group, shift, **kw)
    add_line(net, lv, b2, 0.4)
    add_asym(net, b2, (0.030, 0.010), (0.010, 0.002), (0.005, 0.0))
    return net


def balanced_scaled():
    net = feeder()
    pp.create_load(net, 1, p_mw=0.012, q_mvar=0.004, scaling=1.5)
    pp.create_load(net, 2, p_mw=0.009, q_mvar=0.002, scaling=0.5, type="delta")
    pp.create_load(net, 2, p_mw=0.004, q_mvar=0.001, type="wye")
    return net


def delta_asym_load():
    net = feeder()
    add_asym(net, 2, (0.008, 0.002), (0.0, 0.0), (0.003, 0.001), type="delta")
    return net


def asym_sgen():
    net = feeder()
    pp.create_asymmetric_sgen(net, 1, p_a_mw=0.0, p_b_mw=0.004, p_c_mw=0.0, scaling=1.0)
    return net


def sgen_types():
    net = feeder()
    pp.create_sgen(net, 1, p_mw=0.006, q_mvar=0.001, type="wye", scaling=0.8)
    pp.create_sgen(net, 2, p_mw=0.003, q_mvar=-0.001, type="delta")
    return net


def shunt():
    net = feeder()
    pp.create_shunt(net, 1, q_mvar=0.004, p_mw=0.001, step=2)
    pp.create_shunt(net, 2, q_mvar=-0.002, p_mw=0.0, vn_kv=0.4)
    return net


def ring(closed, line_switch=None, z_ohm=0.0, **kw):
    """b0 feeds b1 and b2 over two lines. A bus-bus switch joins b1 and b2.
    With line_switch set to a bus, the closing line b1-b2 has a switch at that bus.
    `kw` passes name, type or in_ka to the switch."""
    net = new_net()
    b = [pp.create_bus(net, vn_kv=0.4) for _ in range(3)]
    add_grid(net, b[0])
    add_line(net, b[0], b[1], 0.3)
    add_line(net, b[0], b[2], 0.5)
    add_asym(net, b[1], (0.010, 0.003), (0.004, 0.001), (0.002, 0.0))
    add_asym(net, b[2], (0.002, 0.0), (0.012, 0.004), (0.006, 0.002))
    if line_switch is None:
        pp.create_switch(net, b[1], b[2], et="b", closed=closed, z_ohm=z_ohm, **kw)
    else:
        li = add_line(net, b[1], b[2], 0.2)
        pp.create_switch(net, line_switch, li, et="l", closed=closed, z_ohm=z_ohm, **kw)
    return net


def taps(side, pos=2, **kw):
    return with_trafo(
        "Dyn", 30.0, tap_changer_type="Ratio", tap_side=side, tap_neutral=0, tap_min=-4,
        tap_max=4, tap_pos=pos, tap_step_percent=2.5, **kw
    )


def tap2(side2, side="hv"):
    """A first changer on `side` and a second one on `side2`."""
    net = taps(side, mag0_percent=100.0)
    net.trafo["tap2_changer_type"] = "Ratio"
    net.trafo["tap2_side"] = side2
    net.trafo["tap2_neutral"] = 0
    net.trafo["tap2_pos"] = 3
    net.trafo["tap2_step_percent"] = 1.5
    return net


def parallel_lines():
    net = feeder()
    net.line.loc[1, "parallel"] = 2
    net.line.loc[0, "parallel"] = 3
    return net


def eulv():
    net = pn.ieee_european_lv_asymmetric()
    net.bus["name"] = [f"b{i}" for i in net.bus.index]
    return net


def out_of_service_load():
    net = feeder()
    add_asym(net, 2, (0.020, 0.005), (0.020, 0.005), (0.020, 0.005), in_service=False)
    return net


def tap_neutral_nan():
    """A ratio tap with tap_pos set and tap_neutral NaN, which pandapower applies as no tap."""
    net = taps("hv", mag0_percent=100.0)
    net.trafo["tap_neutral"] = float("nan")
    return net


def ideal_degree_shifter():
    """An Ideal changer with only a degree step, which the reader reports."""
    net = with_trafo("Dyn", 30.0)
    net.trafo["tap_changer_type"] = "Ideal"
    net.trafo["tap_side"] = "hv"
    net.trafo["tap_neutral"] = 0
    net.trafo["tap_pos"] = 2
    net.trafo["tap_step_percent"] = 0.0
    net.trafo["tap_step_degree"] = 5.0
    return net


def shunt_characteristic():
    """A shunt whose step reads its powers from the characteristic table."""
    net = feeder()
    pp.create_shunt(net, 1, q_mvar=0.004, p_mw=0.0, step=1, step_dependency_table=True,
                    id_characteristic_table=0)
    net["shunt_characteristic_table"] = pd.DataFrame(
        {"id_characteristic": [0, 0], "step": [1, 2], "q_mvar": [0.010, 0.020], "p_mw": [0.0, 0.0]})
    return net


def std_type_trafo():
    """A std-type Dyn transformer (mag0 100 from the library) and a std-type cable."""
    net = pp.create_empty_network(sn_mva=1.0)
    hv, lv, b2 = (pp.create_bus(net, vn_kv=v) for v in (20.0, 0.4, 0.4))
    add_grid(net, hv)
    pp.create_transformer(net, hv, lv, std_type="0.4 MVA 20/0.4 kV")
    # the library type has no zero-sequence data, which runpp_3ph needs
    net.trafo["vk0_percent"] = net.trafo["vk_percent"]
    net.trafo["vkr0_percent"] = net.trafo["vkr_percent"]
    net.trafo["mag0_percent"] = 100.0
    net.trafo["mag0_rx"] = 0.0
    net.trafo["si0_hv_partial"] = 0.9
    net.trafo["vector_group"] = "Dyn"  # the library says Dyn5, which runpp_3ph refuses
    add_line(net, lv, b2, 0.25)
    add_asym(net, b2, (0.030, 0.010), (0.010, 0.002), (0.005, 0.0))
    return net


def series_trafos():
    """110 kV source, YNyn 110/20 kV, a 20 kV line, Dyn 20/0.4 kV, a LV line, loads on all levels.
    The Dyn transformer has no rn_ohm or xn_ohm, so its cells are NaN."""
    net = new_net()
    b = [pp.create_bus(net, vn_kv=v) for v in (110.0, 20.0, 20.0, 0.4, 0.4)]
    add_grid(net, b[0], s_sc_max_mva=3000.0)
    add_trafo(net, b[0], b[1], "YNyn", 0.0, sn_mva=25.0, vn_hv_kv=110.0, vn_lv_kv=20.0,
              vk_percent=12.0, vkr_percent=0.5, vk0_percent=11.0, vkr0_percent=0.6,
              mag0_percent=100.0, mag0_rx=0.1, si0_hv_partial=0.5, xn_ohm=2.0)
    add_line(net, b[1], b[2], 5.0, r_ohm_per_km=0.161, x_ohm_per_km=0.117, c_nf_per_km=273.0,
             r0_ohm_per_km=0.644, x0_ohm_per_km=0.468, c0_nf_per_km=160.0)
    add_trafo(net, b[2], b[3], "Dyn", 150.0, mag0_percent=100.0)
    add_line(net, b[3], b[4], 0.3)
    add_asym(net, b[1], (0.4, 0.1), (0.1, 0.02), (0.2, 0.05))
    add_asym(net, b[4], (0.030, 0.010), (0.010, 0.002), (0.005, 0.0))
    pp.create_load(net, b[3], p_mw=0.02, q_mvar=0.005)
    return net


def parallel_trafos(group, shift, **kw):
    """Two transformers with different data between the same buses."""
    net = with_trafo(group, shift, mag0_percent=100.0, **kw)
    add_trafo(net, 0, 1, group, shift, sn_mva=0.25, vk_percent=4.0, vkr_percent=1.2,
              vk0_percent=4.0, vkr0_percent=1.2, mag0_percent=80.0, si0_hv_partial=0.6, **kw)
    return net


def ynyn_lv_loads():
    """YNyn with a neutral impedance: loads of each kind on the LV bus with the neutral terminal."""
    net = with_trafo("YNyn", 0.0, mag0_percent=100.0, xn_ohm=0.5, rn_ohm=0.1)
    add_asym(net, 1, (0.020, 0.005), (0.004, 0.001), (0.010, 0.0))
    pp.create_load(net, 1, p_mw=0.015, q_mvar=0.004, type="wye")
    pp.create_load(net, 1, p_mw=0.012, q_mvar=0.003, type="delta")
    add_asym(net, 1, (0.006, 0.001), (0.0, 0.0), (0.003, 0.001), type="delta")
    pp.create_asymmetric_sgen(net, 1, p_a_mw=0.0, p_b_mw=0.0, p_c_mw=0.0)
    pp.create_shunt(net, 1, q_mvar=0.003, p_mw=0.0)
    return net


def ynyn_two_neutrals():
    """Two YNyn in parallel, both with a neutral impedance: two neutral terminals on one bus."""
    net = parallel_trafos("YNyn", 0.0, xn_ohm=0.4)
    add_asym(net, 1, (0.010, 0.002), (0.0, 0.0), (0.004, 0.001))
    return net


def dyn_mixed_loads():
    net = with_trafo("Dyn", 30.0, mag0_percent=100.0)
    for bus in (1, 2):
        pp.create_load(net, bus, p_mw=0.012, q_mvar=0.003, type="delta")
        pp.create_load(net, bus, p_mw=0.006, q_mvar=0.001, type="wye")
        add_asym(net, bus, (0.004, 0.001), (0.0, 0.0), (0.008, 0.002), type="delta")
    return net


def trafo_shunts(group, shift):
    """Shunts on both sides of a transformer. The HV shunt sits off the ext_grid bus.
    runpp_3ph overwrites a shunt at the ext_grid bus in the negative sequence (build_bus.py)."""
    net = with_trafo(group, shift, mag0_percent=100.0)
    hv2 = pp.create_bus(net, vn_kv=20.0)
    add_line(net, 0, hv2, 2.0)
    net.trafo.loc[0, "hv_bus"] = hv2
    pp.create_shunt(net, hv2, q_mvar=0.05, p_mw=0.002)
    pp.create_shunt(net, 1, q_mvar=-0.01, p_mw=0.001)
    pp.create_shunt(net, 2, q_mvar=0.004, p_mw=0.0, step=3)
    return net


def sgen_load_same_bus():
    net = feeder()
    pp.create_load(net, 2, p_mw=0.010, q_mvar=0.003, scaling=1.3)
    pp.create_sgen(net, 2, p_mw=0.008, q_mvar=0.001, type="wye", scaling=0.6)
    pp.create_sgen(net, 2, p_mw=0.004, q_mvar=0.0, type="delta", scaling=1.5)
    add_asym(net, 2, (0.001, 0.0), (0.003, 0.001), (0.002, 0.0), scaling=2.0)
    pp.create_asymmetric_sgen(net, 2, p_a_mw=0.002, p_b_mw=0.002, p_c_mw=0.002,
                              q_a_mvar=0.001, q_b_mvar=0.001, q_c_mvar=0.001, scaling=0.5)
    return net


def line_g():
    net = feeder()
    net.line["g_us_per_km"] = 300.0
    net.line.loc[0, "c0_nf_per_km"] = 400.0
    return net


def long_mv():
    """20 kV cable feeder of 15 and 10 km with sizeable charging, loads in MW."""
    net = new_net()
    b = [pp.create_bus(net, vn_kv=20.0) for _ in range(3)]
    add_grid(net, b[0], s_sc_max_mva=300.0)
    kw = dict(r_ohm_per_km=0.125, x_ohm_per_km=0.112, c_nf_per_km=406.0,
              r0_ohm_per_km=0.5, x0_ohm_per_km=0.45, c0_nf_per_km=406.0)
    add_line(net, b[0], b[1], 15.0, **kw)
    add_line(net, b[1], b[2], 10.0, **kw)
    add_asym(net, b[1], (0.8, 0.2), (0.3, 0.1), (0.5, 0.1))
    add_asym(net, b[2], (0.2, 0.0), (1.0, 0.3), (0.4, 0.1))
    return net


def hz60(group="Dyn", shift=30.0):
    net = with_trafo(group, shift, mag0_percent=100.0)
    net.f_hz = 60.0
    net.line["c_nf_per_km"] = 600.0
    net.line["c0_nf_per_km"] = 300.0
    return net


def two_ext_grids():
    net = feeder()
    add_grid(net, 2, vm_pu=1.01, va_degree=0.0)
    return net


def ynyn_tap(side, pos, **kw):
    return with_trafo(
        "YNyn", 0.0, mag0_percent=100.0, tap_changer_type="Ratio", tap_side=side,
        tap_neutral=0, tap_min=-4, tap_max=4, tap_pos=pos, tap_step_percent=2.5, **kw)


def ynyn_tap_upstream(side, pos, **kw):
    """A YNyn tap whose HV bus sits behind an 8 km line, so the HV correction acts."""
    net = new_net()
    b0, hv, lv, b3 = (pp.create_bus(net, vn_kv=v) for v in (20.0, 20.0, 0.4, 0.4))
    add_grid(net, b0)
    add_line(net, b0, hv, 8.0)
    add_trafo(net, hv, lv, "YNyn", 0.0, mag0_percent=100.0, si0_hv_partial=0.5,
              tap_changer_type="Ratio", tap_side=side, tap_neutral=0, tap_min=-4, tap_max=4,
              tap_pos=pos, tap_step_percent=2.5, **kw)
    add_line(net, lv, b3, 0.4)
    add_asym(net, b3, (0.030, 0.010), (0.010, 0.002), (0.005, 0.0))
    return net


def out_of_service_bad_trafo():
    """An out-of-service transformer whose vkr exceeds vk, next to a working one."""
    net = with_trafo("Dyn", 30.0, mag0_percent=100.0)
    add_trafo(net, 0, 1, "Dyn", 30.0, mag0_percent=100.0, vkr_percent=7.0, in_service=False)
    return net


def out_of_service_null_parallel_line():
    """An out-of-service line with a null `parallel`, next to a working feeder."""
    net = feeder()
    li = add_line(net, 0, 2, 0.4, in_service=False)
    net.line["parallel"] = net.line["parallel"].astype(float)
    net.line.loc[li, "parallel"] = np.nan
    return net


def table_neutral():
    """A ratio changer at its neutral position with a characteristic table."""
    net = with_trafo("Dyn", 30.0, mag0_percent=100.0)
    net.trafo["tap_changer_type"] = "Ratio"
    net.trafo["tap_side"] = "hv"
    net.trafo["tap_neutral"] = 0
    net.trafo["tap_pos"] = 0
    net.trafo["tap_step_percent"] = 2.5
    net.trafo["tap_dependency_table"] = True
    net.trafo["id_characteristic_table"] = 0
    net["trafo_characteristic_table"] = pd.DataFrame({
        "id_characteristic": [0, 0], "step": [0, 1], "voltage_ratio": [1.03, 1.05],
        "angle_deg": [0.0, 0.0], "vk_percent": [6.0, 6.0], "vkr_percent": [1.425, 1.425],
    })
    return net


def table_impedance(drop_type=False):
    """A Dyn changer whose characteristic table states vk and vkr unlike the row."""
    net = with_trafo("Dyn", 150.0, mag0_percent=100.0, vk_percent=6.0, vkr_percent=1.0,
                     vk0_percent=6.0, vkr0_percent=1.0)
    net.trafo["tap_changer_type"] = "Ratio"
    net.trafo["tap_side"] = "hv"
    net.trafo["tap_neutral"] = 0
    net.trafo["tap_pos"] = 0
    net.trafo["tap_step_percent"] = 0.0
    net.trafo["tap_step_degree"] = 0.0
    net.trafo["tap_dependency_table"] = True
    net.trafo["id_characteristic_table"] = 0
    net["trafo_characteristic_table"] = pd.DataFrame({
        "id_characteristic": [0, 0], "step": [0, 1], "voltage_ratio": [1.0, 1.0],
        "angle_deg": [0.0, 0.0], "vk_percent": [12.0, 12.0], "vkr_percent": [2.0, 2.0],
    })
    if drop_type:
        # pandapower applies the table impedance without a changer type too.
        net.trafo = net.trafo.drop(columns=["tap_changer_type"])
    return net


# name, builder, xfail reason or None
CASES = [
    ("wye_feeder_asym", feeder, None),
    ("balanced_load_scaling", balanced_scaled, None),
    ("delta_asym_load", delta_asym_load, None),
    ("asym_sgen", asym_sgen, "DSS writer sums an asymmetric generator over its phases"),
    ("sgen_wye_delta", sgen_types, None),
    ("dyn_30", lambda: with_trafo("Dyn", 30.0), None),
    ("dyn_150", lambda: with_trafo("Dyn", 150.0), None),
    ("dyn_270", lambda: with_trafo("Dyn", 270.0), None),
    ("dyn_90", lambda: with_trafo("Dyn", 90.0), None),
    ("dyn_210", lambda: with_trafo("Dyn", 210.0), None),
    ("dyn_330", lambda: with_trafo("Dyn", 330.0), None),
    ("ynyn_0", lambda: with_trafo("YNyn", 0.0), None),
    ("dyn_mag0_100", lambda: with_trafo("Dyn", 30.0, mag0_percent=100.0), None),
    ("dyn_vk0_ne_vk", lambda: with_trafo(
        "Dyn", 150.0, mag0_percent=100.0, vk0_percent=4.0, vkr0_percent=1.0), None),
    ("dyn_xn_ohm", lambda: with_trafo(
        "Dyn", 30.0, mag0_percent=100.0, mag0_rx=0.5, xn_ohm=0.01), None),
    ("dyn_xn_ohm_mag0_inf", lambda: with_trafo("Dyn", 30.0, xn_ohm=0.01), None),
    ("ynyn_mag0_100", lambda: with_trafo("YNyn", 0.0, mag0_percent=100.0), None),
    ("ynyn_vk0_ne_vk_mag0_100", lambda: with_trafo(
        "YNyn", 0.0, mag0_percent=100.0, vk0_percent=4.0, vkr0_percent=1.0),
     "the DSS writer states a negative neutral resistance, which OpenDSS reads as open"),
    ("ynyn_xn_ohm", lambda: with_trafo("YNyn", 0.0, mag0_percent=100.0, xn_ohm=0.5), None),
    ("ynyn_rn_xn_ohm", lambda: with_trafo(
        "YNyn", 0.0, mag0_percent=100.0, rn_ohm=0.3, xn_ohm=0.5), None),
    ("dyn_vk0_zero", lambda: with_trafo(
        "Dyn", 30.0, mag0_percent=100.0, vk0_percent=0.0, vkr0_percent=0.0), None),
    ("ynyn_vk0_zero", lambda: with_trafo(
        "YNyn", 0.0, mag0_percent=100.0, vk0_percent=0.0, vkr0_percent=0.0), None),
    ("dyn_parallel_2", lambda: with_trafo("Dyn", 150.0, parallel=2, mag0_percent=100.0), None),
    ("ynyn_parallel_2", lambda: with_trafo("YNyn", 0.0, parallel=2, mag0_percent=100.0), None),
    ("dyn_noload_pfe_i0", lambda: with_trafo("Dyn", 150.0, pfe_kw=1.0, i0_percent=0.3), None),
    ("dyn_noload_loss_above_i0", lambda: with_trafo("Dyn", 150.0, pfe_kw=4.0, i0_percent=0.3), None),
    ("shunt", shunt, None),
    ("bus_switch_closed", lambda: ring(True), None),
    ("bus_switch_closed_named", lambda: ring(True, name="S1", type="CB", in_ka=0.4), None),
    ("bus_switch_open", lambda: ring(False), None),
    ("line_switch_open_from", lambda: ring(False, line_switch=1), None),
    ("line_switch_open_to", lambda: ring(False, line_switch=2), None),
    ("line_switch_closed", lambda: ring(True, line_switch=1), None),
    ("trafo_tap_hv", lambda: taps("hv"), None),
    ("trafo_tap_hv_negative", lambda: taps("hv", pos=-3), None),
    ("trafo_tap_lv", lambda: taps("lv"), None),
    ("trafo_tap_hv_mag0_100", lambda: taps("hv", mag0_percent=100.0), None),
    ("trafo_tap_lv_mag0_100", lambda: taps("lv", mag0_percent=100.0), None),
    ("trafo_tap_side_none", lambda: taps(None, pos=3), None),
    ("trafo_tap2_lv", lambda: tap2("lv"), None),
    ("trafo_tap2_hv_tap_lv", lambda: tap2("hv", side="lv"), None),
    ("ynyn_tap_lv_mag0_100", lambda: with_trafo(
        "YNyn", 0.0, mag0_percent=100.0, tap_changer_type="Ratio", tap_side="lv",
        tap_neutral=0, tap_min=-4, tap_max=4, tap_pos=2, tap_step_percent=2.5), None),
    ("parallel_lines", parallel_lines, None),
    ("ieee_european_lv", eulv, None),
    ("trafo_tap_neutral_nan", tap_neutral_nan, None),
    ("shunt_characteristic", shunt_characteristic, None),
    ("std_type_trafo", std_type_trafo, None),
    ("series_ynyn_dyn", series_trafos, None),
    ("parallel_dyn_dyn", lambda: parallel_trafos("Dyn", 150.0), None),
    ("parallel_ynyn_ynyn", lambda: parallel_trafos("YNyn", 0.0), None),
    ("ynyn_two_neutrals", ynyn_two_neutrals, None),
    ("ynyn_lv_loads", ynyn_lv_loads, None),
    ("dyn_mixed_loads", dyn_mixed_loads, None),
    ("trafo_shunts_dyn", lambda: trafo_shunts("Dyn", 30.0), None),
    ("trafo_shunts_ynyn", lambda: trafo_shunts("YNyn", 0.0), None),
    ("sgen_load_same_bus", sgen_load_same_bus, None),
    ("long_mv", long_mv, None),
    ("hz60_dyn", hz60, None),
    ("hz60_ynyn", lambda: hz60("YNyn", 0.0), None),
    ("two_ext_grids", two_ext_grids, None),
    ("bus_switch_z_ohm", lambda: ring(True, z_ohm=0.05), None),
    ("line_switch_closed_z_ohm", lambda: ring(True, line_switch=1, z_ohm=0.05), None),
    ("ynyn_tap_hv_mag0_100", lambda: ynyn_tap("hv", 3), None),
    ("ynyn_tap_hv_xn", lambda: ynyn_tap("hv", -2, xn_ohm=0.5), None),
    ("ynyn_tap_hv_upstream", lambda: ynyn_tap_upstream("hv", 4), None),
    ("ynyn_tap_lv_upstream", lambda: ynyn_tap_upstream("lv", -4), None),
    ("ynyn_tap_hv_upstream_xn", lambda: ynyn_tap_upstream("hv", 4, xn_ohm=0.5), None),
    ("oos_bad_trafo", out_of_service_bad_trafo, None),
    ("oos_null_parallel_line", out_of_service_null_parallel_line, None),
    ("trafo_table_impedance", table_impedance, None),
    ("trafo_table_impedance_no_changer_type", lambda: table_impedance(drop_type=True), None),
    ("ynyn_offnominal_xn", lambda: with_trafo(
        "YNyn", 0.0, mag0_percent=100.0, xn_ohm=0.5, vn_hv_kv=21.0, vn_lv_kv=0.42), None),
    ("dyn_offnominal", lambda: with_trafo(
        "Dyn", 30.0, mag0_percent=100.0, vn_hv_kv=19.0, vn_lv_kv=0.41), None),
    ("dyn_offnominal_xn_tap_lv", lambda: with_trafo(
        "Dyn", 30.0, mag0_percent=100.0, xn_ohm=0.01, vn_hv_kv=19.0, vn_lv_kv=0.41,
        tap_changer_type="Ratio", tap_side="lv", tap_neutral=0, tap_min=-4, tap_max=4,
        tap_pos=2, tap_step_percent=2.5), None),
    # known gaps, recorded so that a fix shows up as XPASS
    ("ynyn_120_zero_seq_shift", lambda: with_trafo("YNyn", 120.0), "YNyn 120 rotates the zero sequence in pandapower"),
    ("ynyn_240_zero_seq_shift", lambda: with_trafo("YNyn", 240.0), "YNyn 240 rotates the zero sequence in pandapower"),
    ("out_of_service_load", out_of_service_load, "an out-of-service element stays energized"),
    ("line_g_c0", line_g, "the DSS writer drops a linecode's shunt conductance (EMIT.DSS.FIELD_DROPPED)"),
]


# --------------------------------------------------------------------------
# hand deck
# --------------------------------------------------------------------------


def tap_ratio(t, side):
    """The product of the ratio taps of both changers on `side` ("hv" or "lv")."""
    ratio = 1.0
    for c in ("tap", "tap2"):
        pos = t.get(f"{c}_pos", float("nan"))
        if t.get(f"{c}_changer_type") not in ("Ratio", "Symmetrical") or t.get(f"{c}_side") != side:
            continue
        # pandapower applies no tap when the step count or the step is NaN
        step = np.nan_to_num((pos - t.get(f"{c}_neutral", float("nan"))) * t.get(f"{c}_step_percent", 0.0))
        ratio *= 1.0 + step / 100.0
    return ratio


def short_circuit(net, t):
    """The vk and vkr runpp_3ph maps to the positive and negative sequence.

    With `tap_dependency_table` true they come from the
    `trafo_characteristic_table` row at `tap_pos`, else 1 for both
    (build_branch._get_vk_values_from_table).
    """
    flag = t.get("tap_dependency_table", False)
    if isinstance(flag, (bool, np.bool_)) and flag:
        c = net.trafo_characteristic_table
        hit = c[(c.id_characteristic == t.id_characteristic_table) & (c.step == t.tap_pos)]
        if hit.empty:
            return 1.0, 1.0
        return float(hit.vk_percent.iloc[0]), float(hit.vkr_percent.iloc[0])
    return t.vk_percent, t.vkr_percent


def zero_seq(t, vk, vkr):
    """pd2ppc_zero.py:194-356, in ohm at the LV side (one transformer per vector group)."""
    vn_lv = t.vn_lv_kv * tap_ratio(t, "lv")
    zb = vn_lv**2 / (t.sn_mva * (t.get("parallel", 1) or 1))
    # pandapower falls back to vk and vkr when the vk0 or vkr0 column is zero
    vk0 = t.vk0_percent if abs(t.vk0_percent) > 1e-8 else t.vk_percent
    vkr0 = t.vkr0_percent if abs(t.vkr0_percent) > 1e-8 else t.vkr_percent
    z0k = (vkr0 + 1j * math.sqrt(vk0**2 - vkr0**2)) / 100 * zb
    # pandapower fills a NaN rn_ohm or xn_ohm with 0 (pd2ppc_zero.py)
    zn = complex(np.nan_to_num(t.get("rn_ohm", 0.0) or 0.0), np.nan_to_num(t.get("xn_ohm", 0.0) or 0.0))
    if t.vector_group.lower() == "ynyn":  # referred to the HV voltage (pd2ppc_zero.py:316-329)
        zn *= (t.vn_lv_kv / t.vn_hv_kv) ** 2
    z0k += zn  # the neutral counts once
    zm = vk0 / 100 * zb * t.mag0_percent / 100
    xm = zm / math.sqrt(t.mag0_rx**2 + 1)
    z3 = xm * t.mag0_rx + 1j * xm
    z1, z2 = t.si0_hv_partial * z0k, (1 - t.si0_hv_partial) * z0k
    zt = z1 * z2 + z2 * z3 + z1 * z3
    # The winding model's own zero sequence uses the mapped vk and vkr. The
    # vk0 fallback above reads the row (pd2ppc_zero.py).
    zw = (vkr + 1j * math.sqrt(vk**2 - vkr**2)) / 100 * zb
    return z1, z2, z3, zt, zw


def zero_shunt(name, bus, y):
    """A zero-sequence-only grounded shunt of admittance y, as a Reactor matrix."""
    j = np.ones((3, 3))
    z = (1 / y) / 3 * j + 1j * ZERO_SHUNT_OPEN * (np.eye(3) - j / 3)

    def mat(m):
        return "[" + " | ".join(" ".join(f"{m[i, k]:.17g}" for k in range(i + 1)) for i in range(3)) + "]"

    return f"New Reactor.{name} phases=3 bus1={bus} Rmatrix={mat(z.real)} Xmatrix={mat(z.imag)}"


def fuses(sw):
    """Whether pandapower fuses the buses of a switch: closed bus-bus without a finite positive z_ohm."""
    z = sw.get("z_ohm", 0.0)
    return sw.et == "b" and bool(sw.closed) and not (np.isfinite(z) and z > 0)


def source_impedances(net, eg, source):
    """The Z2 and Z0 of an ext_grid Vsource: ideal, or from the short-circuit data."""
    if source == "ideal":
        return "[1e-9 1e-9]", "[1e-9 1e-9]"
    vn = net.bus.vn_kv[eg.bus]
    zsc = 1.1 * vn**2 / eg.s_sc_max_mva  # pf_3ph: c = 1.1 on the short-circuit impedance
    x1 = zsc / math.sqrt(eg.rx_max**2 + 1)
    r1 = eg.rx_max * x1
    x0 = eg.x0x_max * x1
    r0 = eg.r0x0_max * x0
    return f"[{r1} {x1}]", f"[{r0} {x0}]"


def shunt_powers(net, r):
    """(p_mw, q_mvar) of a shunt at its step, from the characteristic table when it applies."""
    if getattr(r, "step_dependency_table", False) is True:
        t = net.shunt_characteristic_table
        hit = t[(t.id_characteristic == r.id_characteristic_table) & (t.step == r.step)]
        if hit.empty:
            return 0.0, 0.0
        return float(np.nan_to_num(hit.p_mw.iloc[0])), float(np.nan_to_num(hit.q_mvar.iloc[0]))
    return r.p_mw * r.step, r.q_mvar * r.step


def hand_dss(net, source="ideal") -> str:
    """The deck for `net`. Buses are b<root> after closed bus-bus switches fuse them."""
    root = {int(i): int(i) for i in net.bus.index}

    def find(i):
        while root[i] != i:
            i = root[i]
        return i

    for _, sw in net.switch.iterrows():
        if fuses(sw):
            root[find(int(sw.bus))] = find(int(sw.element))
    name = lambda i: f"b{find(int(i))}"  # noqa: E731
    eg = net.ext_grid.iloc[0]
    z2, z0 = source_impedances(net, eg, source)
    out = [
        "Clear",
        f"Set DefaultBaseFrequency={net.f_hz}",
        f"New Circuit.pp bus1={name(eg.bus)} basekv={net.bus.vn_kv[eg.bus]} pu={eg.vm_pu} "
        f"angle={eg.va_degree} Z1=[1e-9 1e-9] Z2={z2} Z0={z0}",
    ]
    for j in range(1, len(net.ext_grid)):
        e = net.ext_grid.iloc[j]
        z2e, z0e = source_impedances(net, e, source)
        out.append(f"New Vsource.eg{j} bus1={name(e.bus)} basekv={net.bus.vn_kv[e.bus]} pu={e.vm_pu} "
                   f"angle={e.va_degree} Z1=[1e-9 1e-9] Z2={z2e} Z0={z0e}")
    for i, sw in net.switch.iterrows():
        z = sw.get("z_ohm", 0.0)
        if sw.et == "b" and sw.closed and np.isfinite(z) and z > 0:
            zz = z * SWITCH_Z_SCALE
            root_rx = math.sqrt(1 + SWITCH_RX_RATIO**2)
            r, x = zz * SWITCH_RX_RATIO / root_rx, zz / root_rx
            out.append(f"New Line.sw{i} bus1={name(sw.bus)} bus2={name(sw.element)} phases=3 length=1 "
                       f"units=km r1={r} x1={x} r0={r} x0={x} c1=0 c0=0")
    open_end = {}  # (line, bus) -> auxiliary bus name
    for sw in net.switch.itertuples():
        if sw.et == "l" and not sw.closed:
            open_end[(int(sw.element), int(sw.bus))] = f"aux_l{sw.element}_{sw.bus}"
    for i, ln in net.line.iterrows():
        if not ln.in_service:
            continue
        par = ln.get("parallel", 1) or 1
        ends = [open_end.get((i, int(b)), name(b)) for b in (ln.from_bus, ln.to_bus)]
        out.append(
            f"New Line.l{i} bus1={ends[0]} bus2={ends[1]} phases=3 length={ln.length_km} units=km "
            f"r1={ln.r_ohm_per_km / par} x1={ln.x_ohm_per_km / par} r0={ln.r0_ohm_per_km / par} "
            f"x0={ln.x0_ohm_per_km / par} c1={ln.c_nf_per_km * par} c0={ln.c0_nf_per_km * par}"
        )
        # pandapower has no zero-sequence line conductance (pd2ppc_zero), so a delta
        # conductance reproduces it.
        g = (ln.get("g_us_per_km", 0.0) or 0.0) * 1e-6 * ln.length_km * par / 2
        if g:
            for e, bn in enumerate(ends):
                for (u, w) in ((1, 2), (2, 3), (3, 1)):
                    out.append(f"New Reactor.g{i}_{e}_{u}{w} phases=1 bus1={bn}.{u} bus2={bn}.{w} "
                               f"R={3 / g} X=0")
    extra = []
    for i, t in net.trafo.iterrows():
        if not t.in_service:
            continue
        hv, lv = name(t.hv_bus), name(t.lv_bus)
        par = t.get("parallel", 1) or 1
        group = t.vector_group.lower()
        shift = int(round(t.shift_degree)) % 360
        order = (DYN_ORDER if group == "dyn" else YNYN_ORDER)[shift]
        vk, vkr = short_circuit(net, t)
        xhl = math.sqrt(vk**2 - vkr**2)
        loss = t.pfe_kw / (t.sn_mva * 10)
        imag = math.sqrt(max(t.i0_percent**2 - loss**2, 0.0))
        taps = f"[{tap_ratio(t, 'hv')} {tap_ratio(t, 'lv')}]"
        z1, z2c, z3, zt, zw = zero_seq(t, vk, vkr)
        lv_nodes = f"{lv}.{order}"
        hv_nodes = f"{hv}.{DYN_HV_ORDER[shift]}" if group == "dyn" else hv
        rneut = ""
        if group == "ynyn":
            dz = (zt / z3 - zw) / 3
            if abs(dz) > 1e-6 * abs(zw):
                lv_nodes = f"{lv}.{order}.{NEUTRAL_NODE + i}"  # one floating neutral per transformer
                if dz.real >= 0:
                    rneut = f"Transformer.t{i}.wdg=2 Rneut={dz.real} Xneut={dz.imag}"
                else:  # OpenDSS reads a negative Rneut as an open neutral
                    rneut = (f"New Reactor.zn{i} phases=1 bus1={lv}.{NEUTRAL_NODE + i} bus2={lv}.0 "
                             f"R={dz.real} X={dz.imag}")
        out.append(
            f"New Transformer.t{i} phases=3 windings=2 buses=[{hv_nodes} {lv_nodes}] "
            f"conns={'[delta wye]' if group == 'dyn' else '[wye wye]'} "
            f"kvs=[{t.vn_hv_kv} {t.vn_lv_kv}] kvas=[{t.sn_mva * par * 1e3} {t.sn_mva * par * 1e3}] "
            f"%rs=[{vkr / 2} {vkr / 2}] xhl={xhl} taps={taps} "
            f"%noloadloss={loss} %imag={imag}"
        )
        if group == "dyn":
            yc = (z1 + z3) / zt - 1 / zw
            if abs(yc) > 1e-6 / abs(zw):
                extra.append(zero_shunt(f"z0c{i}", lv, yc))
        else:
            if rneut:
                extra.append(rneut)
            # the HV correction reflects through the tapped ratio
            ratio2 = (t.vn_hv_kv * tap_ratio(t, 'hv') / (t.vn_lv_kv * tap_ratio(t, 'lv'))) ** 2
            for tag, bus, y in (("h", hv, z2c / zt / ratio2), ("l", lv, z1 / zt)):
                if abs(y) > 1e-6 / abs(zw):
                    extra.append(zero_shunt(f"z0{tag}{i}", bus, y))
    delta = {"a": "1.2", "b": "2.3", "c": "3.1"}

    def injections(rows, sign, tag):
        for r in rows.itertuples():
            if not r.in_service:
                continue
            kind = getattr(r, "type", "wye")
            if kind not in ("wye", "delta"):
                continue
            vnb = net.bus.vn_kv[r.bus]
            sc = getattr(r, "scaling", 1.0)
            for k, ph in enumerate("abc", start=1):
                if hasattr(r, "p_a_mw"):
                    p, q = getattr(r, f"p_{ph}_mw"), getattr(r, f"q_{ph}_mvar")
                else:
                    p, q = r.p_mw / 3, r.q_mvar / 3
                p, q = sign * p * sc * 1e3, sign * q * sc * 1e3
                if p or q:
                    where = (f"{name(r.bus)}.{delta[ph]} conn=delta kv={vnb}" if kind == "delta"
                             else f"{name(r.bus)}.{k} kv={vnb / math.sqrt(3)}")
                    out.append(f"New Load.{tag}{r.Index}{ph} bus1={where} phases=1 kw={p} "
                               f"kvar={q} model=1 vminpu=0.5 vmaxpu=1.5")

    injections(net.load, 1.0, "ld")
    injections(net.asymmetric_load, 1.0, "al")
    injections(net.sgen, -1.0, "sg")
    injections(net.asymmetric_sgen, -1.0, "as")
    for r in net.shunt.itertuples():
        if not r.in_service:
            continue
        # constant impedance per phase, V_ln^2 / conj(S) with S drawn at vn_kv
        p, q = shunt_powers(net, r)
        if p or q:
            for k in (1, 2, 3):
                out.append(
                    f"New Reactor.sh{r.Index}_{k} phases=1 bus1={name(r.bus)}.{k} bus2={name(r.bus)}.0 "
                    f"R={r.vn_kv**2 * p / (p * p + q * q)} X={r.vn_kv**2 * q / (p * p + q * q)}"
                )
    out += extra
    bases = sorted(set(net.bus.vn_kv))
    out += [f"Set voltagebases=[{' '.join(str(v) for v in bases)}]", "Calcvoltagebases",
            "Set tolerance=1e-10 maxiterations=100"]
    return "\n".join(out) + "\n"


# --------------------------------------------------------------------------
# solving and comparing
# --------------------------------------------------------------------------


def solve_dss(text: str, path: Path) -> dict[str, complex]:
    """Volts per node name (`bus.k`) of an OpenDSS solve."""
    path.write_text(text, encoding="utf-8")
    dss.Basic.ClearAll()
    dss(f'Redirect "{path}"')
    dss("Solve")
    if not dss.Solution.Converged():
        raise RuntimeError("OpenDSS did not converge")
    names = list(dss.Circuit.AllNodeNames())
    v = list(dss.Circuit.AllBusVolts())
    return {n: complex(v[2 * j], v[2 * j + 1]) for j, n in enumerate(names)}


def patch_solver(text: str) -> str:
    """Solver settings only: a tight tolerance before the final Solve."""
    head, _, tail = text.rpartition("\nSolve")
    if not _:
        raise RuntimeError("no Solve in the deck")
    return head + "\nSet Tolerance=1e-10\nSet MaxIterations=100\nSolve" + tail


def find_cli() -> str:
    exe = ".exe" if os.name == "nt" else ""
    cands = [os.environ.get("POWERIO_BIN", "")]
    cands += [str(REPO / "target" / mode / f"powerio{exe}") for mode in ("release", "debug")]
    for c in cands:
        if c and Path(c).is_file():
            return c
    found = shutil.which("powerio")
    if not found:
        raise SystemExit("no powerio binary: cargo build -p powerio-cli, or set POWERIO_BIN")
    return found


def convert(cli: str, src: Path, dst: Path, to: str = "dss") -> list[dict]:
    # The CLI refuses to overwrite. A network with bus locations also writes buscoords.csv.
    for stale in (dst, dst.parent / "buscoords.csv"):
        stale.unlink(missing_ok=True)
    r = subprocess.run(
        [cli, "convert", str(src), "--to", to, "-o", str(dst), "--diagnostics-format", "json"],
        capture_output=True, text=True, encoding="utf-8",
    )
    if r.returncode != 0:
        raise RuntimeError(f"powerio convert failed ({r.returncode}): {r.stderr.strip()[:600]}")
    return json.loads(r.stderr or "[]")


def pu_table(net, volts, key):
    """(vm_pu, va_degree) per (bus, phase) from node volts. `key(bus, k)` names the node."""
    out = {}
    for b in net.bus.index:
        vbase = net.bus.vn_kv[b] * 1e3 / math.sqrt(3)
        for k in (1, 2, 3):
            v = volts.get(key(int(b), k))
            out[(int(b), k)] = None if v is None else (abs(v) / vbase, math.degrees(np.angle(v)))
    return out


def pp_table(net):
    out = {}
    for b, row in net.res_bus_3ph.iterrows():
        for k, ph in enumerate("abc", start=1):
            vm, va = row[f"vm_{ph}_pu"], row[f"va_{ph}_degree"]
            out[(int(b), k)] = None if not (np.isfinite(vm) and np.isfinite(va)) else (vm, va)
    return out


def distance(a, b):
    """Worst (dVm, dVa, where) over the keys both tables hold. Keys only one table holds are listed."""
    wm = wa = 0.0
    where = ""
    missing = []
    for key in a:
        x, y = a[key], b.get(key)
        if x is None:
            continue
        if y is None:
            missing.append(key)
            continue
        dm = abs(x[0] - y[0])
        da = abs((x[1] - y[1] + 180) % 360 - 180)
        if dm > wm:
            wm, where = dm, f"bus {key[0]} phase {'abc'[key[1] - 1]}"
        wa = max(wa, da)
    return wm, wa, where, missing


def run_case(cli, name, build, xfail, work: Path, keep: bool):
    net = build()
    pp.runpp_3ph(net, tolerance_mva=1e-12, max_iteration=100)
    if not net.converged:
        raise RuntimeError("runpp_3ph did not converge")
    ref = pp_table(net)
    src = work / f"{name}.json"
    pp.to_json(net, str(src))
    dst = work / f"{name}.dss"
    dst.unlink(missing_ok=True)  # the CLI refuses to overwrite
    diags = convert(cli, src, dst)
    deck = dst.read_text(encoding="utf-8")
    # PowerIO names a bus by its pandapower index
    got = pu_table(net, solve_dss(patch_solver(deck), work / f"{name}_powerio.dss"),
                   lambda b, k: f"{b}.{k}")
    roots = {}
    for _, sw in net.switch.iterrows():
        if fuses(sw):
            roots[int(sw.bus)] = int(sw.element)

    def find(i):
        while i in roots:
            i = roots[i]
        return i

    hand_key = lambda b, k: f"b{find(b)}.{k}"  # noqa: E731
    hand = pu_table(net, solve_dss(hand_dss(net, "ideal"), work / f"{name}_hand.dss"), hand_key)
    orac = pu_table(net, solve_dss(hand_dss(net, "oracle"), work / f"{name}_oracle.dss"), hand_key)
    if not keep:
        for f in (src, dst):
            f.unlink(missing_ok=True)
    m_hand = distance(hand, got)
    m_ref = distance(ref, got)
    m_ideal = distance(ref, hand)
    m_orac = distance(ref, orac)
    # a node the reference has and a deck lacks is a failure, not a skip
    problems = []
    for tag, m in (("powerio", m_hand), ("powerio-vs-pp", m_ref), ("hand-vs-pp", m_ideal), ("oracle-vs-pp", m_orac)):
        if m[3]:
            problems.append(f"{tag}: nodes missing {m[3][:4]}")
    if m_hand[0] > VM_HAND or m_hand[1] > VA_TOL:
        problems.append(f"powerio vs hand {m_hand[0]:.2e} pu {m_hand[1]:.2e} deg at {m_hand[2]}")
    if m_ref[0] > m_ideal[0] + VM_SLACK or m_ref[1] > m_ideal[1] + VA_TOL:
        problems.append(f"powerio vs pp {m_ref[0]:.2e} pu {m_ref[1]:.2e} deg at {m_ref[2]} "
                        f"(ideal-source error {m_ideal[0]:.2e} pu {m_ideal[1]:.2e} deg)")
    tol_vm, tol_va = ORACLE_TOL.get(name, (VM_ORACLE, VA_ORACLE))
    if m_orac[0] > tol_vm or m_orac[1] > tol_va:
        problems.append(f"hand deck with the pandapower source vs pp {m_orac[0]:.2e} pu "
                        f"{m_orac[1]:.2e} deg at {m_orac[2]}")
    codes = sorted({d["code"] for d in diags})
    line = (f"vs hand {m_hand[0]:.2e} pu {m_hand[1]:.1e} deg | vs pp {m_ref[0]:.2e} pu "
            f"{m_ref[1]:.1e} deg | ideal-source {m_ideal[0]:.2e} | oracle {m_orac[0]:.2e}")
    return problems, line, codes


# --------------------------------------------------------------------------
# diagnostics
# --------------------------------------------------------------------------

APPROX = "READ.DIST.PANDAPOWER_APPROXIMATED"
UNSUP = "READ.DIST.PANDAPOWER_ELEMENT_UNSUPPORTED"
OOS = "READ.DIST.PANDAPOWER_OUT_OF_SERVICE"
TABLE = "READ.DIST.PANDAPOWER_TABLE_UNSUPPORTED"
NONPASSIVE = "READ.DIST.PANDAPOWER_NONPASSIVE_CORRECTION"
BALANCED = "READ.PANDAPOWER.TABLE_UNSUPPORTED"

# name, builder, [(code, substring of the message)], optional target token (default dss)
DIAGNOSTICS = [
    ("sgen without type", lambda: (lambda n: (pp.create_sgen(n, 1, p_mw=0.001), n)[1])(feeder()),
     [(UNSUP, "sgen_0")]),
    # A transformer without a winding mapping keeps the balanced reading.
    ("Yzn", lambda: with_trafo("Yzn", 150.0), [(BALANCED, "runpp_3ph")], "matpower"),
    ("Dyn shift 0", lambda: with_trafo("Dyn", 0.0), [(APPROX, "trafo_0"), (APPROX, "0")]),
    ("YNyn shift 120", lambda: with_trafo("YNyn", 120.0), [(APPROX, "trafo_0"), (APPROX, "120")]),
    ("ext_grid", feeder, [(APPROX, "ext_grid_0")]),
    ("out of service load", out_of_service_load, [(OOS, "asymmetric_load")]),
    ("Dyn xn_ohm", lambda: with_trafo("Dyn", 30.0, xn_ohm=0.01),
     [(NONPASSIVE, "trafo_0")]),
    ("ward and impedance", lambda: (lambda n: (
        pp.create_ward(n, 1, 0.001, 0.0, 0.0, 0.0),
        pp.create_impedance(n, 0, 1, 0.1, 0.1, 1.0),
        n)[2])(feeder()), [(TABLE, "ward"), (TABLE, "impedance")]),
    ("closed bus switch with z_ohm", lambda: ring(True, z_ohm=0.05), [(APPROX, "switch_0")]),
    ("Ideal degree-only shifter", ideal_degree_shifter, [(APPROX, "`tap` changer")]),
    ("no-load data", lambda: with_trafo("Dyn", 150.0, pfe_kw=1.0, i0_percent=0.3), [(APPROX, "T model")]),
    ("characteristic table at the neutral tap", table_neutral,
     [(APPROX, "trafo_characteristic_table")]),
    ("out-of-service trafo with vkr above vk", out_of_service_bad_trafo, [(UNSUP, "trafo_1")]),
]


def run_diagnostics(cli, work: Path):
    fails = []
    for label, build, want, *target in DIAGNOSTICS:
        to = target[0] if target else "dss"
        net = build()
        src = work / "diag.json"
        pp.to_json(net, str(src))
        try:
            diags = convert(cli, src, work / f"diag.{to}", to)
        except Exception as err:  # noqa: BLE001
            fails.append(f"diagnostics `{label}`: {err}")
            continue
        for code, text in want:
            if not any(d["code"] == code and text in d["message"] for d in diags):
                have = sorted({d["code"] for d in diags if d["code"].startswith("READ")})
                fails.append(f"diagnostics `{label}`: no {code} naming `{text}` (READ codes: {have})")
    return fails


# --------------------------------------------------------------------------


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--count", action="store_true", help="print the number of validated cases")
    ap.add_argument("--workdir", help="where the generated JSON and decks go")
    ap.add_argument("--keep", action="store_true", help="keep the generated JSON and decks")
    ap.add_argument("cases", nargs="*", help="case names (default: all)")
    args = ap.parse_args()
    if args.count:
        print(len(CASES) + len(DIAGNOSTICS))
        return 0

    cli = find_cli()
    work = Path(args.workdir) if args.workdir else Path(tempfile.mkdtemp(prefix="pp3ph_"))
    work.mkdir(parents=True, exist_ok=True)
    out = os.environ.get("PIO_RESULTS_TSV")
    failures = []
    for name, build, xfail in CASES:
        if args.cases and name not in args.cases:
            continue
        try:
            with redirect_stdout(io.StringIO()):
                problems, line, codes = run_case(cli, name, build, xfail, work, args.keep)
        except Exception as err:  # noqa: BLE001
            problems, line, codes = [f"{type(err).__name__}: {err}"], "", []
        if xfail:
            mark = "xfail" if problems else "XPASS"
            if not problems:
                failures.append(f"{name}: expected to fail ({xfail}) but passed")
        else:
            mark = "FAIL" if problems else "ok"
            if problems:
                failures.append(f"{name}: {len(problems)} mismatch(es)")
        print(f"{name}: {mark}  {line}")
        for p in problems[:6]:
            print(f"    {p}")
        if args.cases:
            print(f"    codes: {codes}")
        if out:
            with open(out, "a", encoding="utf-8") as fh:
                fh.write(f"pandapower-3ph/{name}\tpp3ph\t{mark}\n")
    if not args.cases:
        for f in run_diagnostics(cli, work):
            print(f"FAIL {f}")
            failures.append(f)
    if failures:
        print("\npandapower three-phase oracle failed:")
        for f in failures:
            print(f"  {f}")
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
