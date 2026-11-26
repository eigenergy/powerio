"""Run external power flow solvers and collect their numerical results."""

import cmath
import csv
from functools import cache
import importlib.metadata
import json
import math
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
from tempfile import TemporaryDirectory
from xml.etree import ElementTree

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]


class Unavailable(RuntimeError):
    """A required solver or reference case is unavailable."""


def package(name):
    return f"{name} {importlib.metadata.version(name)}"


def command(args, *, timeout=180, **kwargs):
    result = subprocess.run(args, capture_output=True, text=True, timeout=timeout, **kwargs)
    if result.returncode:
        raise RuntimeError((result.stderr.strip() or result.stdout.strip()
                            or f"{Path(args[0]).name} exited with status {result.returncode}")[-3000:])
    return result.stdout


def executable(name):
    path = shutil.which(name)
    if path is None:
        raise Unavailable(f"{name} is not installed")
    return path


def points(rows):
    """Label, magnitude, angle in degrees. Drop disconnected rows with no voltage."""
    result = {}
    for label, magnitude, angle in rows:
        if math.isnan(magnitude) and math.isnan(angle):
            continue
        if label in result or not (math.isfinite(magnitude) and math.isfinite(angle)):
            raise ValueError(f"Duplicate label or invalid voltage at {label}")
        result[label] = [float(magnitude), float(angle)]
    if not result:
        raise ValueError("The solver returned no bus voltages")
    return result


def admittance(matrix, labels, basis, base_mva=None):
    from scipy.sparse import coo_matrix

    coo = coo_matrix(matrix)
    if coo.shape != (len(labels), len(labels)):
        raise ValueError("Admittance matrix dimensions do not match its labels")
    return {"basis": basis, "base_mva": base_mva, "labels": labels,
            "entries": [[int(i), int(j), float(v.real), float(v.imag)]
                        for i, j, v in zip(coo.row, coo.col, coo.data, strict=True)]}


def matpower_solutions(requests, project):
    checkout = os.environ.get("MATPOWER_PATH")
    if not checkout:
        raise Unavailable("Set MATPOWER_PATH to a MATPOWER 8.1 checkout")
    request = project / "matpower-requests.json"
    response = project / "matpower-responses.json"
    request.write_text(json.dumps({key: str(path) for key, path in requests.items()}))
    command([
        executable("octave-cli"), "--quiet", "--no-init-all", str(HERE / "matpower_pf.m"),
        str(Path(checkout).resolve()), str(request), str(response),
    ])
    return json.loads(response.read_text())


def powsybl_admittance(models, bus_ids, voltage, terminal_powers):
    from scipy.sparse import lil_matrix

    labels = list(voltage)
    index = {label: i for i, label in enumerate(labels)}
    ybus = lil_matrix((len(labels), len(labels)), dtype=complex)
    seen = set()
    checked = 0
    for model in models:
        nodes = {}
        for bus in model["buses"]:
            if bus.get("disabled"):
                continue
            configured = bus_ids.get(bus["id"], [])
            if len(configured) != 1 or configured[0] in seen:
                raise ValueError(f"No unique configured bus for PowSybl bus {bus['id']}")
            label = configured[0]
            seen.add(label)
            nodes[bus["num"]] = label
            for kind in ("shunt", "controllerShunt", "svcShunt"):
                shunt = bus.get(kind, {})
                if not shunt.get("disabled"):
                    ybus[index[label], index[label]] += complex(shunt.get("g", 0), shunt.get("b", 0))
        for branch in model["branches"]:
            if branch.get("disabled"):
                continue
            a, b = (nodes.get(branch.get(f"num{i}")) if branch.get(f"connectedSide{i}", True) else None
                    for i in (1, 2))
            if a is None and b is None:
                continue
            z = complex(branch.get("r", 0), branch.get("x", 0))
            if z == 0:
                raise ValueError(f"PowSybl treats {branch['id']} as an ideal connection")
            y = 1 / z
            ratio = branch.get("modifiedR1", branch.get("r1", 1))
            shift = branch.get("modifiedA1", branch.get("a1", 0))
            tap = cmath.rect(ratio, shift)
            aa = ratio**2 * (y + complex(branch.get("g1", 0), branch.get("b1", 0)))
            bb = y + complex(branch.get("g2", 0), branch.get("b2", 0))
            ab, ba = -y * tap.conjugate(), -y * tap
            if b is None:
                ybus[index[a], index[a]] += aa - ab * ba / bb
            elif a is None:
                ybus[index[b], index[b]] += bb - ba * ab / aa
            else:
                i, j = index[a], index[b]
                ybus[i, i] += aa
                ybus[i, j] += ab
                ybus[j, i] += ba
                ybus[j, j] += bb
                if branch["id"] in terminal_powers:
                    va, vb = (cmath.rect(voltage[label][0], math.radians(voltage[label][1]))
                              for label in (a, b))
                    calculated = (va * (aa * va + ab * vb).conjugate(),
                                  vb * (ba * va + bb * vb).conjugate())
                    # PowSybl's internal network uses a 100 MVA base. Check
                    # orientation and tap conventions against its solved flows.
                    actual = terminal_powers[branch["id"]]
                    if any(abs(p * 100 - q) > 1e-5 for p, q in zip(calculated, actual, strict=True)):
                        raise ValueError(f"Admittance stamp disagrees with PowSybl flows on {branch['id']}")
                    checked += 1
    if seen != set(labels):
        raise ValueError("The PowSybl debug model does not identify every solved bus")
    return {**admittance(ybus, labels, "network-pu", 100.0),
            "method": "Assembled from the PowSybl OpenLoadFlow debug model",
            "branch_stamps_checked_against_solved_flows": checked}


def powsybl_run(pp, network):
    with TemporaryDirectory() as debug:
        options = pp.loadflow.Parameters(
            use_reactive_limits=False, distributed_slack=False, transformer_voltage_control_on=False,
            phase_shifter_regulation_on=False, shunt_compensator_voltage_control_on=False,
            component_mode=pp.loadflow.ComponentMode.ALL_CONNECTED,
            provider_parameters={"newtonRaphsonConvEpsPerEq": "1e-10", "debugDir": debug},
        )
        results = pp.loadflow.run_ac(network, options)
        models = [json.loads(item.read_text()) for item in sorted(Path(debug).glob("lfnetwork-*.json"))]
    return results, models


def powsybl_island_models(pp, network, results, models):
    # OpenLoadFlow names the dumps by time to millisecond precision. A later
    # island can overwrite an earlier one. Export missing islands separately
    # when keeping their voltage levels preserves all connected equipment.
    if len(models) >= len(results) or len(results) == 1:
        return models
    buses = network.get_buses()
    components = list(buses.groupby("connected_component"))
    level_components = buses.groupby("voltage_level_id")["connected_component"].nunique()
    if (len(components) != len(results) or (level_components > 1).any()
            or not network.get_hvdc_lines().empty or not network.get_3_windings_transformers().empty):
        raise ValueError("PowSybl's diagnostic dumps are incomplete and the islands cannot be isolated by voltage level")
    level_island = dict(zip(buses.voltage_level_id, buses.connected_component, strict=True))
    branches = network.get_branches()
    active = branches[branches.connected1 | branches.connected2]
    if any(level_island.get(row.voltage_level1_id) != level_island.get(row.voltage_level2_id)
           for row in active.itertuples()):
        raise ValueError("PowSybl's diagnostic dumps are incomplete and a branch crosses the island boundary")

    def bus_ids(model):
        return {bus["id"] for bus in model["buses"] if not bus.get("disabled")}

    present = {frozenset(bus_ids(model)) for model in models}
    original = network.save_to_string("XIIDM")
    completed = list(models)
    for _, island in components:
        expected = set(island.index)
        if frozenset(expected) in present:
            continue
        reduced = pp.network.load_from_string("island.xiidm", original)
        reduced.reduce_by_ids(island.voltage_level_id.unique().tolist())
        isolated, dumps = powsybl_run(pp, reduced)
        if (len(isolated) != 1 or isolated[0].status.name != "CONVERGED"
                or len(dumps) != 1 or bus_ids(dumps[0]) != expected):
            raise ValueError("PowSybl could not recover a complete diagnostic dump for the missing island")
        completed.extend(dumps)
    return completed


def powsybl(path):
    import pypowsybl as pp

    cgmes_names = {}
    if path.is_dir():
        # CGMES consists of several XML profiles. PowSybl accepts their ZIP.
        for file in path.glob("*.xml"):
            for node in ElementTree.parse(file).getroot().findall("{*}TopologicalNode"):
                container = node.find("{*}TopologicalNode.ConnectivityNodeContainer")
                name = node.findtext("{*}IdentifiedObject.name")
                if container is not None and name:
                    rdf = "{http://www.w3.org/1999/02/22-rdf-syntax-ns#}"
                    mrid = container.get(rdf + "resource", "").removeprefix("#").removeprefix("_")
                    cgmes_names.setdefault(mrid, []).append(name.strip())
        zip_path = path.with_name(path.name + "-powsybl.zip")
        shutil.make_archive(str(zip_path.with_suffix("")), "zip", root_dir=path)
        path = zip_path
    if path.suffix == ".txt":
        network = pp.network.load_from_string("case.cdf", path.read_text())
    elif path.suffix.lower() in (".raw", ".rawx"):
        network = pp.network.load(str(path), {"psse.import.ignore-base-voltage": "true"})
    else:
        network = pp.network.load(str(path))
    results, models = powsybl_run(pp, network)
    if not results or any(item.status.name != "CONVERGED" for item in results):
        raise RuntimeError(f"PowSybl did not converge: {results}")
    nominal = network.get_voltage_levels()["nominal_v"]
    buses = network.get_bus_breaker_view_buses()
    voltage = points(
        (str(key), row.v_mag / nominal[row.voltage_level_id], row.v_angle)
        for key, row in buses.iterrows()
    )
    aliases = {str(key): str(row["name"]).strip() for key, row in buses.iterrows() if row["name"]}
    for key, row in buses.iterrows():
        names = cgmes_names.get(row.voltage_level_id, [])
        if len(names) == 1:
            aliases[str(key)] = names[0]
    if path.suffix.lower() in (".raw", ".rawx"):
        # PowSybl's PSS/E importer labels configured buses B<bus number>.
        aliases.update({label: label[1:] for label in voltage if label.startswith("B") and label[1:].isdigit()})
    bus_ids = {}
    for key, row in buses.iterrows():
        if str(key) in voltage:
            bus_ids.setdefault(row.bus_id, []).append(str(key))
    powers = []
    terminal_powers = {}
    for table in (network.get_lines(), network.get_2_windings_transformers()):
        for row in table.itertuples():
            a, b = bus_ids.get(row.bus1_id, []), bus_ids.get(row.bus2_id, [])
            values = (row.p1, row.q1, row.p2, row.q2)
            if len(a) == len(b) == 1 and all(math.isfinite(v) for v in values):
                powers.append([a[0], b[0], *values])
                terminal_powers[row.Index] = (complex(*values[:2]), complex(*values[2:]))
    answer = {"solver": package("pypowsybl"), "units": "pu", "voltage": voltage, "aliases": aliases,
              "branch_power": powers, "one_island": len(results) == 1}
    try:
        models = powsybl_island_models(pp, network, results, models)
        answer["admittance"] = powsybl_admittance(models, bus_ids, voltage, terminal_powers)
    except (ValueError, KeyError, ZeroDivisionError, pp.PyPowsyblError) as error:
        answer["admittance_detail"] = str(error)
    return answer


def pandapower(path, three_phase=False, lightsim=False):
    import pandapower as pp

    network = pp.from_json(str(path))
    if three_phase:
        pp.runpp_3ph(network, enforce_q_lims=False, tolerance_mva=1e-9)
        rows = (
            (f"{key}.{i}", row[f"vm_{phase}_pu"] * network.bus.at[key, "vn_kv"] * 1000 / math.sqrt(3),
             row[f"va_{phase}_degree"])
            for key, row in network.res_bus_3ph.iterrows() for i, phase in enumerate("abc", 1)
        )
    else:
        pp.runpp(network, enforce_q_lims=False, calculate_voltage_angles=True, tolerance_mva=1e-9,
                 numba=False, lightsim2grid=lightsim)
        rows = ((str(key), row.vm_pu, row.va_degree) for key, row in network.res_bus.iterrows())
    answer = {"solver": package("pandapower"), "units": "V" if three_phase else "pu", "voltage": points(rows)}
    if three_phase:
        answer["one_island"] = len(network.ext_grid[network.ext_grid.in_service]) == 1
    if lightsim:
        if not network._options["lightsim2grid"]:
            raise RuntimeError("pandapower did not use the LightSim2Grid solver")
        answer.update(solver=package("lightsim2grid"), reader=f"{package('pandapower')} file reader")
    if not three_phase:
        powers = []
        for devices, table, a, b, ports in (
            (network.line, network.res_line, "from_bus", "to_bus", ("from", "to")),
            (network.trafo, network.res_trafo, "hv_bus", "lv_bus", ("hv", "lv")),
        ):
            for key, row in devices.iterrows():
                if row.in_service:
                    values = [table.at[key, f"{kind}_{port}_{unit}"] for port in ports
                              for kind, unit in (("p", "mw"), ("q", "mvar"))]
                    if all(math.isfinite(v) for v in values):
                        powers.append([str(row[a]), str(row[b]), *values])
        answer["branch_power"] = powers
        y = network._ppc["internal"]["Ybus"]
        indices = network._pd2ppc_lookups["bus"]
        lookup = {int(indices[int(key)]): key for key in answer["voltage"] if indices[int(key)] >= 0}
        if not lightsim and set(lookup) == set(range(y.shape[0])) and len(lookup) == len(answer["voltage"]):
            answer["admittance"] = admittance(y, [lookup[i] for i in range(y.shape[0])], "network-pu", network.sn_mva)
        if lightsim:
            answer["admittance_detail"] = "The pandapower integration exposes its own Y-bus, shared by both solvers"
    return answer


def pypsa(path):
    import pypsa as ps

    network = ps.Network(str(path))
    snapshot = network.snapshots[:1]
    result = network.pf(snapshots=snapshot, x_tol=1e-9)
    if not result["converged"].size or not result["converged"].to_numpy().all():
        raise RuntimeError("PyPSA did not converge")
    time = snapshot[0]
    voltage = points(
        (str(key), vm, math.degrees(network.buses_t.v_ang.at[time, key]))
        for key, vm in network.buses_t.v_mag_pu.loc[time].items()
    )
    powers = []
    for devices, series in ((network.lines, network.lines_t), (network.transformers, network.transformers_t)):
        for key, row in devices.iterrows():
            values = [series[field].at[time, key] for field in ("p0", "q0", "p1", "q1")]
            if all(math.isfinite(v) for v in values):
                powers.append([row.bus0, row.bus1, *values])
    answer = {"solver": package("pypsa"), "units": "pu", "voltage": voltage, "branch_power": powers}
    if len(network.sub_networks) == 1:
        sub = network.sub_networks.obj.iloc[0]
        answer["admittance"] = admittance(sub.Y, list(sub.buses_o), "network-pu", 1.0)
    return answer


def egret(path):
    from egret.data.model_data import ModelData
    from egret.models.acpf import solve_acpf
    from pyomo.opt import TerminationCondition

    solver = executable("ipopt")
    network, result = solve_acpf(
        ModelData.read(str(path)), solver, solver_tee=False,
        return_results=True, options={"tol": 1e-10},
    )
    if result.solver.termination_condition not in (TerminationCondition.optimal, TerminationCondition.locallyOptimal):
        raise RuntimeError(f"Egret did not converge: {result.solver.termination_condition}")
    powers = [
        [row["from_bus"], row["to_bus"], *[row[field] for field in ("pf", "qf", "pt", "qt")]]
        for _, row in network.elements("branch") if all(field in row for field in ("pf", "qf", "pt", "qt"))
    ]
    answer = {"solver": f"{package('gridx-egret')}, {command([solver, '-v']).strip()}", "units": "pu",
              "voltage": points((label, row["vm"], row["va"]) for label, row in network.elements("bus")),
              "branch_power": powers}
    from egret.model_library.transmission.tx_calc import calculate_y_matrix_from_branch
    from egret.model_library.transmission.tx_utils import scale_ModelData_to_pu
    from scipy.sparse import lil_matrix

    per_unit = scale_ModelData_to_pu(network, inplace=False)
    labels = list(answer["voltage"])
    index = {name: i for i, name in enumerate(labels)}
    y = lil_matrix((len(labels), len(labels)), dtype=complex)
    for _, row in per_unit.elements("branch"):
        f, t = (index[row[key]] for key in ("from_bus", "to_bus"))
        entries = calculate_y_matrix_from_branch(row)
        for i, current in ((f, "if"), (t, "it")):
            for j, voltage in ((f, "vf"), (t, "vt")):
                y[i, j] += complex(entries[current + "r", voltage + "r"],
                                   entries[current + "j", voltage + "r"])
    for _, row in per_unit.elements("shunt"):
        i = index[row["bus"]]
        y[i, i] += complex(row["gs"], row["bs"])
    answer["admittance"] = admittance(y, labels, "network-pu", network.data["system"]["baseMVA"])
    return answer


def surge(path):
    import surge as engine

    network = engine.load(str(path))
    options = engine.AcPfOptions()
    options.enforce_q_limits = False
    options.tolerance = 1e-9
    result = engine.solve_ac_pf(network, options)
    if not result.converged:
        raise RuntimeError(f"Surge did not converge: {result.status}")
    powers = [
        [str(row.from_bus), str(row.to_bus), row.pf_mw, row.qf_mvar, row.pt_mw, row.qt_mvar]
        for row in result.branches if row.in_service
    ]
    labels = [str(key) for key in network.bus_numbers]
    answer = {"solver": package("surge-py"), "units": "pu",
              "voltage": points(zip(labels, result.vm, result.va_deg, strict=True)),
              "branch_power": powers}
    ybus = network.ybus()
    answer["admittance"] = admittance(
        ybus.to_scipy(), [str(key) for key in ybus.bus_numbers], "network-pu", network.base_mva
    )
    return answer


def opendss(path):
    from opendssdirect import dss

    # A project written by PowerIO includes this entrypoint and the data files.
    if path.is_dir():
        path = path / "case.dss"
    cwd = Path.cwd()
    try:
        dss.Basic.ClearAll()
        dss(f'Redirect "{path}"')
        dss("Solve")
        if not dss.Solution.Converged():
            raise RuntimeError("OpenDSS did not converge")
        labels = dss.Circuit.AllNodeNames()
        values = dss.Circuit.AllBusVolts()
        rows = (
            (label.lower(), abs(complex(re, im)), math.degrees(math.atan2(im, re)))
            for label, re, im in zip(labels, values[::2], values[1::2], strict=True)
        )
        from scipy.sparse import csc_matrix

        answer = {"solver": dss.Basic.Version().strip(), "units": "V", "voltage": points(rows),
                  "one_island": dss.Vsources.Count() == 1}
        powers = []
        for group, names in (("Line", dss.Lines.AllNames()), ("Transformer", dss.Transformers.AllNames())):
            for name in names:
                if dss.Circuit.SetActiveElement(f"{group}.{name}") < 0:
                    continue
                element = dss.CktElement
                if not element.Enabled() or element.NumTerminals() != 2:
                    continue
                frm, to = (bus.split(".", 1)[0].lower() for bus in element.BusNames())
                n = element.NumConductors()
                nodes = element.NodeOrder()
                values = element.Powers()  # kW and kvar, both ends into the branch
                for i in range(n):
                    a, b = f"{frm}.{nodes[i]}", f"{to}.{nodes[n + i]}"
                    if a in answer["voltage"] and b in answer["voltage"]:
                        quantities = [values[2 * k + part] / 1000 for k in (i, n + i) for part in (0, 1)]
                        powers.append([a, b, *quantities])
        answer["branch_power"] = powers
        ylabels = [name.lower() for name in dss.Circuit.YNodeOrder()]
        values, indices, indptr = dss.YMatrix.getYsparse()
        matrix = csc_matrix((values, indices, indptr), shape=(len(ylabels), len(ylabels)))
        answer["admittance"] = admittance(matrix, ylabels, "OpenDSS SystemY after solving, in siemens")
        return answer
    finally:
        os.chdir(cwd)


def gridlabd(path):
    if path.is_dir():
        path = path / "case.glm"
    with TemporaryDirectory() as directory:
        run = Path(directory)
        output = run / "voltages.csv"
        model = run / "measure.glm"
        model.write_text(f'#include "{path}"\nobject voltdump {{ filename "{output}"; mode RECT; }}\n')
        if binary := os.environ.get("GRIDLABD_BIN"):
            version, source_count = gridlabd_cli(Path(binary).resolve(), run, model)
        else:
            version = package("gridlabd")
            text = command([sys.executable, __file__, str(model)], timeout=30, cwd=run)
            source_count = int(text.rpartition("reference_buses=")[2].strip())
        with output.open() as file:
            next(file)  # comments describing the snapshot
            rows = []
            for row in csv.DictReader(file):
                for i, phase in enumerate("ABC", start=1):
                    voltage = complex(float(row[f"volt{phase}_real"]), float(row[f"volt{phase}_imag"]))
                    # The phase dump includes zero placeholders for absent phases.
                    if voltage != 0:
                        rows.append((f"{row['node_name'].lower()}.{i}", abs(voltage),
                                     math.degrees(cmath.phase(voltage))))
    return {"solver": f"PNNL {version}", "units": "V", "voltage": points(rows),
            "one_island": source_count == 1, "admittance_detail": "The GLM voltage dump does not expose the Y-bus"}


def gridlabd_cli(binary, directory, model):
    root = binary.parent.parent
    env = os.environ.copy()
    for key, subdirectories in (("GLPATH", ("lib", "share")), ("LD_LIBRARY_PATH", ("lib",))):
        paths = [str(root / name) for name in subdirectories if (root / name).is_dir()]
        if env.get(key):
            paths.append(env[key])
        env[key] = os.pathsep.join(paths)
    state = directory / "state.glm"
    version = command([binary, "--version"], env=env, timeout=15).strip()
    command([binary, "--quiet", "--output", state, model], env=env, timeout=30, cwd=directory)
    source_count = len(re.findall(r"^\s*bustype\s+SWING\s*;", state.read_text(), re.MULTILINE))
    return version, source_count


def gridlabd_run(path):
    import gridlabd as engine

    simulation = engine.GridLabD()
    try:
        if any(f() for f in (simulation.setup_before_load, lambda: simulation.load_glm(path, quiet=True),
                             simulation.run)):
            errors = [message["message"] for message in simulation.get_messages() if message["type"] == "ERROR"]
            raise RuntimeError("GridLAB-D failed: " + "; ".join(errors[-4:]))
        slack = set()
        for kind in ("node", "meter", "load", "triplex_node", "triplex_meter", "triplex_load"):
            slack.update(name for name, value in simulation.get_properties_by_class(kind, "bustype").items()
                         if value == "SWING")
        print(f"reference_buses={len(slack)}")
    finally:
        simulation.stop()


@cache
def tellegen_version(binary):
    info = json.loads(command([binary, "describe"]))
    return f"Tellegen {info['tellegen_version']} with PowerIO {info['powerio_version']}"


def tellegen(request, bmopf=False):
    _, path, ir, operating_point = request
    binary = os.environ.get("TELLEGEN") or executable("tellegen")
    version = tellegen_version(binary)
    module = json.loads(ir.read_text()) if ir and ir.is_file() else None
    network = module["value"]["data"] if module else {}
    if bmopf:
        data = path.read_text()
        reader = "Tellegen BMOPF reader (PowerIO)"
    else:
        if not module:
            raise Unavailable("Tellegen needs a parsed PowerIO multiconductor model")
        for source in network.get("sources", []):
            # PowerIO preserves CIM identifiers in extras. They do not enter
            # the electrical model and Tellegen rejects unknown source extras.
            source.get("extras", {}).pop("ravens_mrid", None)
        if operating_point == "nominal":
            # This fixture's schedule is deliberately disabled for the static
            # test. Other controls remain subject to Tellegen's normal checks.
            network["untyped"] = [item for item in network.get("untyped", [])
                                  if (item["class"], item["name"]) != ("EnergyConsumerSchedule", "ls1")]
            for load in network.get("loads", []):
                for key in ("daily", "yearly"):
                    if load.get("extras", {}).get(key) == "ls1":
                        del load["extras"][key]
        data = json.dumps(module)
        reader = "PowerIO reads the case, Tellegen solves the parsed grid"
    settings = {"output": "tables", "tolerance": 1e-9}
    result = json.loads(command([binary, "solve-mc-bmopf" if bmopf else "solve-mc", json.dumps(settings)],
                                input=data))
    summary, table = result["summary"], result["tables"]
    if (not summary["converged"] or summary["load_model"]["status"] != "original"
            or table["excluded_islands"]):
        raise RuntimeError("Tellegen could not solve the full network with the original load models")
    strings = table["strings"]
    volts = table["terminal_voltage"]
    grounded = {(bus["id"].lower(), str(terminal))
                for bus in network.get("buses", []) for terminal in bus.get("grounded", [])}
    rows = []
    for i, (bus, terminal) in enumerate(zip(table["terminal_bus"], table["terminal_name"], strict=True)):
        v = complex(volts[2 * i], volts[2 * i + 1])
        labels = (strings[bus].lower(), strings[terminal])
        if labels not in grounded:
            rows.append((".".join(labels), abs(v), math.degrees(cmath.phase(v))))
    return {"solver": version, "reader": reader, "units": "V", "voltage": points(rows),
            "shared_powerio_reader": True,
            "one_island": len(set(table["source_name"])) == 1,
            "operating_point": operating_point,
            "solver_settings": summary["load_model"]["options"]}


def julia_solutions(requests, scheme, project):
    """Solve a batch in one Julia process; only the two package readers see these files."""
    request = project / f"{scheme}-requests.json"
    response = project / f"{scheme}-responses.json"
    request.write_text(json.dumps({key: str(path) for key, path in requests.items()}))
    environment = {
        "powermodels": ROOT / "evals/validation",
        "pmd": os.environ.get("PMD_PROJECT"),
        "powersystems": os.environ.get("POWERSYSTEMS_PROJECT"),
    }[scheme]
    if not environment:
        raise Unavailable(f"Set {scheme.upper()}_PROJECT to the Julia environment for this solver")
    command([executable("julia"), "--startup-file=no", f"--project={environment}",
             str(HERE / "julia_pf.jl"), scheme, str(request), str(response)])
    return json.loads(response.read_text())


SOLVERS = {
    "matpower": "matpower", "psse": "powsybl", "psse34": "powsybl", "psse35": "powsybl",
    "xiidm": "powsybl", "jiidm": "powsybl", "cgmes": "powsybl", "ucte": "powsybl",
    "powermodels-json": "powermodels", "pandapower-json": "pandapower", "pandapower-3ph": "pandapower-3ph",
    "pypsa-csv": "pypsa", "dss": "opendss", "pmd-json": "pmd", "pmd-math-json": "pmd",
    "egret-json": "egret", "surge-json": "surge", "pslf": "surge", "psse-rawx": "powsybl",
    "ieee-cdf": "powsybl", "powersystems-json": "powersystems", "dss-pmd": "pmd",
    "bmopf-json": "tellegen-bmopf", "ravens-distribution": "tellegen",
    "cim-dist": "tellegen", "gridlabd": "gridlabd", "gridlabd-arras": "gridlabd",
    "sincal-multiconductor": "tellegen",
    "powermodels-via-powerio": "powermodels", "pandapower-lightsim": "lightsim",
}


def solve_many(requests, work):
    results = {}
    groups = {}
    for key, request in requests.items():
        format = request[0]
        solver = SOLVERS.get(format)
        if solver:
            groups.setdefault(solver, {})[key] = request
        else:
            results[key] = {"status": "unavailable", "detail": f"No independent solver configured for {format}"}
    for solver, cases in groups.items():
        if solver in ("matpower", "powermodels", "pmd", "powersystems"):
            paths = {key: req[1] for key, req in cases.items()}
            try:
                batch = matpower_solutions(paths, work) if solver == "matpower" else julia_solutions(paths, solver, work)
                if batch.keys() != cases.keys():
                    raise RuntimeError(f"{solver} returned a different set of cases than requested")
                for key, result in batch.items():
                    if cases[key][0] == "powermodels-via-powerio":
                        result.update(reader="PowerIO reads the case and exports PowerModels JSON",
                                      shared_powerio_reader=True)
                results.update(batch)
            except (Unavailable, OSError, RuntimeError, subprocess.TimeoutExpired) as error:
                for key in cases:
                    results[key] = {"status": "unavailable", "detail": str(error)}
            continue
        for key, request in cases.items():
            try:
                if solver in ("tellegen", "tellegen-bmopf"):
                    solution = tellegen(request, bmopf=solver == "tellegen-bmopf")
                else:
                    fn = {"powsybl": powsybl, "opendss": opendss,
                          "egret": egret, "pypsa": pypsa, "pandapower": pandapower, "surge": surge,
                          "gridlabd": gridlabd,
                          "lightsim": lambda p: pandapower(p, lightsim=True),
                          "pandapower-3ph": lambda p: pandapower(p, three_phase=True)}[solver]
                    solution = fn(request[1])
                results[key] = {"status": "solved", **solution}
            except (ImportError, importlib.metadata.PackageNotFoundError, Unavailable) as error:
                results[key] = {"status": "unavailable", "detail": str(error)}
            except Exception as error:
                results[key] = {"status": "solve_failed", "detail": str(error)[-3000:]}
    return results


if __name__ == "__main__":
    gridlabd_run(sys.argv[1])
