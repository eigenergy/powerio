"""Read and solve the files themselves, without calling PowerIO."""

import importlib.metadata
import json
import math
import os
from pathlib import Path
import shutil
import subprocess

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]


class Unavailable(RuntimeError):
    """A required solver or reference case is unavailable."""


def package(name):
    return f"{name} {importlib.metadata.version(name)}"


def command(args, **kwargs):
    result = subprocess.run(args, capture_output=True, text=True, timeout=180, **kwargs)
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


def powsybl(path):
    import pypowsybl as pp

    if path.is_dir():
        # CGMES consists of several XML profiles. PowSybl accepts their ZIP.
        zip_path = path.with_name(path.name + "-powsybl.zip")
        shutil.make_archive(str(zip_path.with_suffix("")), "zip", root_dir=path)
        path = zip_path
    if path.suffix == ".txt":
        network = pp.network.load_from_string("case.cdf", path.read_text())
    elif path.suffix.lower() in (".raw", ".rawx"):
        network = pp.network.load(str(path), {"psse.import.ignore-base-voltage": "true"})
    else:
        network = pp.network.load(str(path))
    options = pp.loadflow.Parameters(
        use_reactive_limits=False, distributed_slack=False, transformer_voltage_control_on=False,
        phase_shifter_regulation_on=False, shunt_compensator_voltage_control_on=False,
        connected_component_mode=pp.loadflow.ConnectedComponentMode.ALL,
        provider_parameters={"newtonRaphsonConvEpsPerEq": "1e-10"},
    )
    results = pp.loadflow.run_ac(network, options)
    if not results or any(item.status.name != "CONVERGED" for item in results):
        raise RuntimeError(f"PowSybl did not converge: {results}")
    nominal = network.get_voltage_levels()["nominal_v"]
    buses = network.get_bus_breaker_view_buses()
    voltage = points(
        (str(key), row.v_mag / nominal[row.voltage_level_id], row.v_angle)
        for key, row in buses.iterrows()
    )
    aliases = {str(key): str(row["name"]).strip() for key, row in buses.iterrows() if row["name"]}
    if path.suffix.lower() in (".raw", ".rawx"):
        # PowSybl's PSS/E importer labels configured buses B<bus number>.
        aliases.update({label: label[1:] for label in voltage if label.startswith("B") and label[1:].isdigit()})
    bus_ids = {}
    for key, row in buses.iterrows():
        if str(key) in voltage:
            bus_ids.setdefault(row.bus_id, []).append(str(key))
    powers = []
    for table in (network.get_lines(), network.get_2_windings_transformers()):
        for row in table.itertuples():
            a, b = bus_ids.get(row.bus1_id, []), bus_ids.get(row.bus2_id, [])
            values = (row.p1, row.q1, row.p2, row.q2)
            if len(a) == len(b) == 1 and all(math.isfinite(v) for v in values):
                powers.append([a[0], b[0], *values])
    return {"solver": package("pypowsybl"), "units": "pu", "voltage": voltage, "aliases": aliases,
            "branch_power": powers, "one_island": len(results) == 1}


def pandapower(path, three_phase=False):
    import pandapower as pp

    network = pp.from_json(str(path))
    if three_phase:
        pp.runpp_3ph(network, enforce_q_lims=False, tolerance_mva=1e-9)
        rows = (
            (f"{key}.{phase}", row[f"vm_{phase}_pu"], row[f"va_{phase}_degree"])
            for key, row in network.res_bus_3ph.iterrows() for phase in "abc"
        )
    else:
        pp.runpp(network, enforce_q_lims=False, calculate_voltage_angles=True, tolerance_mva=1e-9,
                 numba=False)
        rows = ((str(key), row.vm_pu, row.va_degree) for key, row in network.res_bus.iterrows())
    answer = {"solver": package("pandapower"), "units": "pu", "voltage": points(rows)}
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
        if set(lookup) == set(range(y.shape[0])) and len(lookup) == len(answer["voltage"]):
            answer["admittance"] = admittance(y, [lookup[i] for i in range(y.shape[0])], "network-pu", network.sn_mva)
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
        ylabels = [name.lower() for name in dss.Circuit.YNodeOrder()]
        values, indices, indptr = dss.YMatrix.getYsparse()
        matrix = csc_matrix((values, indices, indptr), shape=(len(ylabels), len(ylabels)))
        answer["admittance"] = admittance(matrix, ylabels, "OpenDSS SystemY after solving, in siemens")
        return answer
    finally:
        os.chdir(cwd)


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
    "ieee-cdf": "powsybl", "powersystems-json": "powersystems",
}


def solve_many(requests, work):
    results = {}
    groups = {}
    for key, (format, path) in requests.items():
        solver = SOLVERS.get(format)
        if solver:
            groups.setdefault(solver, {})[key] = path
        else:
            results[key] = {"status": "unavailable", "detail": f"No independent solver configured for {format}"}
    for solver, cases in groups.items():
        if solver in ("matpower", "powermodels", "pmd", "powersystems"):
            try:
                batch = matpower_solutions(cases, work) if solver == "matpower" else julia_solutions(cases, solver, work)
                if batch.keys() != cases.keys():
                    raise RuntimeError(f"{solver} returned a different set of cases than requested")
                results.update(batch)
            except (Unavailable, OSError, RuntimeError, subprocess.TimeoutExpired) as error:
                for key in cases:
                    results[key] = {"status": "unavailable", "detail": str(error)}
            continue
        fn = {"powsybl": powsybl, "opendss": opendss,
              "egret": egret, "pypsa": pypsa, "pandapower": pandapower, "surge": surge,
              "pandapower-3ph": lambda p: pandapower(p, three_phase=True)}[solver]
        for key, path in cases.items():
            try:
                results[key] = {"status": "solved", **fn(path)}
            except (ImportError, Unavailable) as error:
                results[key] = {"status": "unavailable", "detail": str(error)}
            except Exception as error:
                results[key] = {"status": "solve_failed", "detail": str(error)[-3000:]}
    return results
