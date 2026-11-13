#!/usr/bin/env python3
"""Validate powerio's PowerWorld .aux writer against PowerWorld Simulator.

Simulator is the one reader whose interpretation of an .aux file matters, and it
is the only independent oracle for the writer. Each case is parsed by powerio,
written as .aux by powerio, and loaded into a fresh Simulator case through the
SimAuto COM server. The harness then checks, per case:

- the load: Simulator's message log for the aux holds no undefined field, no
  object it could not create, and no skipped record;
- the counts: Simulator holds as many buses, loads, shunts, generators, and
  branches as powerio wrote;
- the readback: every written field of every object, keyed by Simulator's
  identity (bus number, bus number plus device ID, bus pair plus circuit),
  equals powerio's value;
- the power flow: Simulator's Newton-Raphson solution of the loaded case
  equals a MATPOWER style Newton-Raphson solution of powerio's own network
  (powerio's Y_bus, MATPOWER's bus type rules), bus by bus.

Recorded per the evals/README contract:

- Source software: PowerWorld Simulator through SimAuto; the version and build
  date print with each run (validated with Simulator 24, build 2026-05-20).
- Calculation settings: Simulator solves with generator Mvar limit checks,
  automatic tap, phase shifter, and switched shunt control, and generator MW
  limit enforcement disabled, a convergence tolerance of 1e-8 pu, so the
  solution is MATPOWER's runpf solution (which enforces none of them). The
  reference solves to a 1e-10 pu mismatch. Angles compare relative to the
  first slack bus. A network with no slack (the .pwb reader decodes none)
  skips the power flow. The zero impedance threshold is the one the written
  case states.
- Identity mapping: buses by number; loads, shunts, and generators by bus
  number and the ID the writer states; branches by bus pair and circuit.
  The case is written from its PowerIO IR round trip, so an .aux input goes
  through the writer rather than echoing its retained source.
- Tolerances: FIELD_RTOL/FIELD_ATOL for readback, VM_ATOL (pu) and VA_ATOL
  (degrees) for the solution, declared below with their reasons.
- Expected diagnostics: the writer's emission diagnostics print per case;
  they report what the aux cannot state (piecewise costs, dc lines, and so on)
  and those items are left out of the comparison. A substituted bus value
  (138 kV, area or zone 1) and an infinite value (the +-1e10 sentinel) are
  compared as Simulator holds them, and only when the writer reported them.

Last run (Simulator 24, build 2026-05-20): case9, case14, case30, case33bw,
case57, case118, t_case9_dcline, t_case9_oos, api_conformance,
angle_bounds_clamp, both PGLib cases, ACTIVSg200 from .aux, .m, .RAW, and
.pwb, and case2869pegase all mark ok.

    python evals/validation/validate_powerworld_simauto.py tests/data/case14.m [case ...]

Needs Windows, a Simulator license with the SimAuto add-on, pywin32, NumPy,
SciPy, and a powerio build from this tree. Per case the mark is `ok` or `FAIL`.
Appends `<stem>\tpowerworld\t<mark>` to $PIO_RESULTS_TSV when set. Exits
nonzero on any FAIL.
"""

from __future__ import annotations

import math
import os
import re
import sys
import tempfile
from pathlib import Path

import numpy as np
import scipy.sparse as sp
import scipy.sparse.linalg as spla

import powerio

# Simulator keeps per unit values in single precision: a GenMW of 829.8 reads
# back as 829.80003357, the float32 of 8.298 pu. Readback allows twice
# float32's relative step. The same rounding moves a large case's solution:
# case2869pegase lands 1.1e-6 pu and 1.8e-4 degrees from the reference while
# the cases up to 200 buses agree to 2e-7 pu and 2e-5 degrees, so the
# solution tolerances leave about five times pegase's difference.
FIELD_RTOL = 2.4e-7
FIELD_ATOL = 1e-7
VM_ATOL = 5e-6
VA_ATOL = 1e-3
# The nominal voltage Simulator 24 gives a bus written with none.
SUBSTITUTED_KV = 138.0
AUX_VALUE = re.compile(r'"([^"]*)"|(\S+)')
LOG_FAILURES = ("not defined", "unable to create", "skipped", "required field", "not all required")


def _simauto():
    import pythoncom
    import win32com.client

    pythoncom.CoInitialize()
    sa = win32com.client.dynamic.Dispatch("pwrworld.SimulatorAuto")
    sa.UIVisible = False
    return sa


def _check(result, what):
    error = result[0] if isinstance(result, tuple) else ""
    if error:
        raise RuntimeError(f"{what}: {error}")
    return result[1] if isinstance(result, tuple) and len(result) > 1 else None


def _table(sa, obj, fields):
    """Simulator's rows of `obj` as dicts of stripped strings."""
    result = sa.GetParametersMultipleElement(obj, fields, "")
    if result[0]:
        if "No data returned" in result[0]:
            return []
        raise RuntimeError(f"GetParametersMultipleElement {obj}: {result[0]}")
    columns = result[1]
    return [
        {field: (value or "").strip() for field, value in zip(fields, row, strict=True)}
        for row in zip(*columns, strict=True)
    ]


def _close(a, b):
    try:
        return math.isclose(float(a), float(b), rel_tol=FIELD_RTOL, abs_tol=FIELD_ATOL)
    except ValueError:
        return False


def _stated_rating(rating):
    """The MVA a Branch row states: PowerWorld writes no limit as 0, and the
    model holds no limit as infinity (or no value)."""
    if rating is None or math.isinf(rating):
        return 0.0
    return rating


def _yes(flag):
    return "YES" if flag else "NO"


def _status(on):
    return "Closed" if on else "Open"


def reference_solution(net):
    """A MATPOWER style Newton-Raphson power flow of powerio's network.

    Bus types follow MATPOWER's bustypes: a PV or slack bus keeps its type
    only with an in-service generator, and that generator's set point fixes
    the bus voltage. Isolated buses and buses out of reach of a slack are left
    out. Returns {bus id: (vm, va degrees)}.
    """
    buses = net.buses
    n = len(buses)
    index = {b["id"]: i for i, b in enumerate(buses)}
    base = net.base_mva
    ybus = sp.csr_matrix(net.calc_admittance_matrix())
    sbus = np.zeros(n, dtype=complex)
    vm = np.array([b["vm"] for b in buses], dtype=float)
    va = np.radians([b["va"] for b in buses])
    has_gen = np.zeros(n, dtype=bool)
    gen_vm = {}
    for g in net.generators:
        if not g["in_service"]:
            continue
        i = index[g["bus"]]
        sbus[i] += complex(g["pg"], g["qg"]) / base
        has_gen[i] = True
        gen_vm.setdefault(i, g["vg"])
    for load in net.loads:
        if load["in_service"]:
            sbus[index[load["bus"]]] -= complex(load["p"], load["q"]) / base
    kind = [b["kind"] for b in buses]
    ref = [i for i in range(n) if kind[i] == "REF" and has_gen[i]]
    pv = [i for i in range(n) if kind[i] == "PV" and has_gen[i]]
    for i in ref + pv:
        vm[i] = gen_vm[i]
    # Buses a slack reaches through in-service branches.
    reach = set(ref)
    adjacency = {i: [] for i in range(n)}
    for br in net.branches:
        if br["in_service"]:
            f, t = index[br["from_id"]], index[br["to_id"]]
            adjacency[f].append(t)
            adjacency[t].append(f)
    stack = list(ref)
    while stack:
        i = stack.pop()
        for j in adjacency[i]:
            if j not in reach and kind[j] != "ISOLATED":
                reach.add(j)
                stack.append(j)
    pv = [i for i in pv if i in reach]
    pq = [i for i in range(n) if i in reach and i not in ref and i not in pv]
    active = sorted(reach)
    pvpq = pv + pq
    v = vm * np.exp(1j * va)
    for _ in range(31):
        mis = v * np.conj(ybus @ v) - sbus
        f = np.concatenate([mis[pvpq].real, mis[pq].imag])
        if np.max(np.abs(f), initial=0.0) < 1e-10:
            break
        ibus = ybus @ v
        diag_v = sp.diags(v)
        diag_i = sp.diags(ibus)
        diag_vnorm = sp.diags(v / np.abs(v))
        # MATPOWER's dSbus_dV, polar form.
        ds_dvm = sp.csr_matrix(diag_v @ (ybus @ diag_vnorm).conj() + diag_i.conj() @ diag_vnorm)
        ds_dva = sp.csr_matrix(1j * diag_v @ (diag_i - ybus @ diag_v).conj())
        jac = sp.vstack(
            [
                sp.hstack([ds_dva[pvpq][:, pvpq].real, ds_dvm[pvpq][:, pq].real]),
                sp.hstack([ds_dva[pq][:, pvpq].imag, ds_dvm[pq][:, pq].imag]),
            ]
        ).tocsc()
        dx = spla.spsolve(jac, -f)
        va_new, vm_new = np.angle(v), np.abs(v)
        va_new[pvpq] += dx[: len(pvpq)]
        vm_new[pq] += dx[len(pvpq) :]
        v = vm_new * np.exp(1j * va_new)
    else:
        raise RuntimeError("the reference power flow did not converge")
    return {buses[i]["id"]: (abs(v[i]), math.degrees(np.angle(v[i]))) for i in active}


def validate(path, sa, workdir):
    # PowerIO IR carries no retained source bytes, so after the round trip an
    # .aux input is written by the writer too rather than echoed byte for byte.
    module = powerio.deserialize(powerio.serialize(powerio.parse(path)).artifacts[0].data)
    net = module.value
    emitted = powerio.emit(module, "powerworld")
    codes = sorted({d.code for d in emitted.diagnostics})
    aux = Path(workdir) / (Path(path).stem + ".aux")
    aux.write_bytes(emitted.artifacts[0].data)
    problems = []

    # The load.
    log = Path(workdir) / (Path(path).stem + ".log")
    _check(sa.RunScriptCommand("NewCase;"), "NewCase")
    _check(sa.RunScriptCommand("LogClear;"), "LogClear")
    _check(sa.ProcessAuxFile(str(aux)), "ProcessAuxFile")
    _check(sa.RunScriptCommand(f'LogSave("{log}", NO);'), "LogSave")
    for line in log.read_text(encoding="utf-8", errors="replace").splitlines():
        if any(k in line.lower() for k in LOG_FAILURES):
            problems.append(f"log: {line.strip()}")

    # Readback, keyed by Simulator's identities: the device IDs and circuits
    # are the ones the written text states.
    sbase = _check(sa.GetParametersSingleElement("Sim_Solution_Options", ["SBase"], [""]), "SBase")[0]
    if not _close(sbase, net.base_mva):
        problems.append(f"SBase {sbase} != {net.base_mva}")

    def compare(obj, fields, key_fields, expected_rows):
        got = {tuple(r[k] for k in key_fields): r for r in _table(sa, obj, key_fields + fields)}
        if len(got) != len(expected_rows):
            problems.append(f"{obj}: Simulator holds {len(got)}, powerio wrote {len(expected_rows)}")
        for key, expected in expected_rows.items():
            row = got.get(key)
            if row is None:
                problems.append(f"{obj} {key}: missing in Simulator")
                continue
            for field, want in expected.items():
                have = row[field]
                # The writer states an infinite value as a +-1e10 sentinel and
                # reports it (EMIT.POWERWORLD.NOT_A_NUMBER).
                if isinstance(want, float) and math.isinf(want):
                    if "EMIT.POWERWORLD.NOT_A_NUMBER" not in codes:
                        problems.append(f"{obj} {key} {field}: infinite value not reported")
                    want = math.copysign(1e10, want)
                ok = _close(have, want) if isinstance(want, (int, float)) else have.upper() == str(want).upper()
                if not ok:
                    problems.append(f"{obj} {key} {field}: Simulator {have!r}, powerio {want!r}")

    # Simulator substitutes 138 kV for a nominal voltage that is not positive
    # and area or zone 1 for 0. The writer reports both, and the readback
    # expects the substituted value, which checks what the report claims.
    substituted = [b for b in net.buses if b["base_kv"] <= 0 or b["area"] == 0 or b["zone"] == 0]
    if substituted and "EMIT.POWERWORLD.VALUE_DEFAULTED" not in codes:
        problems.append(f"{len(substituted)} substituted bus value(s) not reported")
    compare(
        "Bus",
        ["BusNomVolt", "BusSlack", "AreaNum", "ZoneNum", "BusVoltLimHigh:1", "BusVoltLimLow:1"],
        ["BusNum"],
        {
            (str(b["id"]),): {
                "BusNomVolt": b["base_kv"] if b["base_kv"] > 0 else SUBSTITUTED_KV,
                "BusSlack": _yes(b["kind"] == "REF"),
                "AreaNum": b["area"] or 1,
                "ZoneNum": b["zone"] or 1,
                "BusVoltLimHigh:1": b["vmax"],
                "BusVoltLimLow:1": b["vmin"],
            }
            for b in net.buses
        },
    )
    ids = _aux_ids(emitted.artifacts[0].data.decode())
    compare(
        "Load",
        ["LoadSMW", "LoadSMVR", "LoadStatus"],
        ["BusNum", "LoadID"],
        {
            (str(load["bus"]), ids["Load"][i]): {
                "LoadSMW": load["p"],
                "LoadSMVR": load["q"],
                "LoadStatus": _status(load["in_service"]),
            }
            for i, load in enumerate(net.loads)
        },
    )
    compare(
        "Shunt",
        ["SSNMW", "SSNMVR", "SSStatus"],
        ["BusNum", "ShuntID"],
        {
            (str(s["bus"]), ids["Shunt"][i]): {"SSNMW": s["g"], "SSNMVR": s["b"], "SSStatus": _status(s["in_service"])}
            for i, s in enumerate(net.shunts)
        },
    )
    kinds = {b["id"]: b["kind"] for b in net.buses}
    compare(
        "Gen",
        ["GenMWSetPoint", "GenMWMax", "GenMWMin", "GenMVRMax", "GenMVRMin", "GenVoltSet", "GenMVABase", "GenStatus", "GenAVRAble"],
        ["BusNum", "GenID"],
        {
            (str(g["bus"]), ids["Gen"][i]): {
                "GenMWSetPoint": g["pg"],
                "GenMWMax": g["pmax"],
                "GenMWMin": g["pmin"],
                "GenMVRMax": g["qmax"],
                "GenMVRMin": g["qmin"],
                "GenVoltSet": g["vg"],
                "GenMVABase": g["mbase"],
                "GenStatus": _status(g["in_service"]),
                "GenAVRAble": _yes(g["voltage_regulation_on"] and kinds[g["bus"]] in ("PV", "REF")),
            }
            for i, g in enumerate(net.generators)
        },
    )
    expected_branches = {}
    for i, br in enumerate(net.branches):
        xf = br["tap"] != 0.0 or br["shift"] != 0.0
        row = {
            "LineR": br["r"],
            "LineX": br["x"],
            "LineC": br["b"],
            "LineAMVA": _stated_rating(br["rate_a"]),
            "LineAMVA:1": _stated_rating(br["rate_b"]),
            "LineAMVA:2": _stated_rating(br["rate_c"]),
            "LinePhase": br["shift"],
            "LineStatus": _status(br["in_service"]),
        }
        if xf:
            row["LineTap"] = br["tap"] if br["tap"] != 0.0 else 1.0
        expected_branches[(str(br["from_id"]), str(br["to_id"]), ids["Branch"][i])] = row
    compare(
        "Branch",
        ["LineR", "LineX", "LineC", "LineAMVA", "LineAMVA:1", "LineAMVA:2", "LineTap", "LinePhase", "LineStatus"],
        ["BusNum", "BusNum:1", "LineCircuit"],
        expected_branches,
    )

    # The power flow. A network with no slack (the .pwb reader decodes none)
    # leaves Simulator to pick one, so there is no common problem to compare.
    if not any(b["kind"] == "REF" for b in net.buses):
        return problems, codes, float("nan"), float("nan"), 0
    reference = reference_solution(net)
    options = {
        "DisableGenMVRCheck": "YES",
        "ChkTaps": "NO",
        "ChkPhaseShifters": "NO",
        "ChkShunts": "NO",
        "ChkSVCs": "NO",
        "EnforceGenMWLimits": "NO",
        "ConvergenceTol": "1e-8",
    }
    for name, value in options.items():
        _check(sa.ChangeParametersSingleElement("Sim_Solution_Options", [name], [value]), f"set {name}")
    _check(sa.RunScriptCommand("SolvePowerFlow(RECTNEWT);"), "SolvePowerFlow")
    solved = {int(r["BusNum"]): (float(r["BusPUVolt"]), float(r["BusAngle"])) for r in _table(sa, "Bus", ["BusNum", "BusPUVolt", "BusAngle"])}
    slack = next(b["id"] for b in net.buses if b["kind"] == "REF" and b["id"] in reference)
    dvm = max(abs(solved[i][0] - vm) for i, (vm, _) in reference.items())
    dva = max(
        abs((solved[i][1] - solved[slack][1]) - (va - reference[slack][1])) for i, (_, va) in reference.items()
    )
    if dvm > VM_ATOL or dva > VA_ATOL:
        problems.append(f"power flow: max |dVm| {dvm:.2e} pu, max |dVa| {dva:.2e} deg")
    return problems, codes, dvm, dva, len(reference)


def _aux_ids(text):
    """The device IDs and circuits the writer stated, per object, in row order.

    Aux values quote only with double quotes, so an apostrophe in a name is
    data, not a quote.
    """
    ids = {}
    current = None
    key = {"Load": "LoadID", "Shunt": "ShuntID", "Gen": "GenID", "Branch": "LineCircuit"}
    for line in text.splitlines():
        stripped = line.strip()
        if stripped.startswith("DATA ("):
            obj = stripped[6:].split(",", 1)[0]
            fields = [f.strip() for f in stripped.split("[", 1)[1].rsplit("]", 1)[0].split(",")]
            current = (obj, fields.index(key[obj])) if obj in key else None
            if current:
                ids[obj] = []
        elif stripped in ("{", "}") or not stripped or stripped.startswith("//"):
            if stripped == "}":
                current = None
        elif current:
            values = [quoted or bare for quoted, bare in AUX_VALUE.findall(stripped)]
            ids[current[0]].append(values[current[1]].strip())
    for obj in key:
        ids.setdefault(obj, [])
    return ids


def main(paths):
    sa = _simauto()
    info = sa.ProgramInformation
    print(f"PowerWorld Simulator {info[0][1]} (build {info[0][2]:%Y-%m-%d}), powerio {powerio.__version__}")
    failed = False
    tsv = os.environ.get("PIO_RESULTS_TSV")
    with tempfile.TemporaryDirectory() as workdir:
        for path in paths:
            stem = Path(path).stem
            try:
                problems, codes, dvm, dva, n = validate(path, sa, workdir)
            except Exception as error:  # a case that cannot run is a FAIL
                problems, codes, dvm, dva, n = [f"error: {error!r}"], [], float("nan"), float("nan"), 0
            mark = "FAIL" if problems else "ok"
            failed |= bool(problems)
            print(f"{mark:4s} {stem}: {n} buses solved, max |dVm| {dvm:.1e} pu, max |dVa| {dva:.1e} deg; emission {codes}")
            for problem in problems[:20]:
                print(f"       {problem}")
            if len(problems) > 20:
                print(f"       ... {len(problems) - 20} more")
            if tsv:
                with open(tsv, "a", encoding="utf-8") as out:
                    out.write(f"{stem}\tpowerworld\t{mark}\n")
    _check(sa.CloseCase(), "CloseCase")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
