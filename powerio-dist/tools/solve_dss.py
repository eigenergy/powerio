"""Solve a .dss case with the OpenDSS engine and print node voltages as JSON.

Usage: <python-with-opendssdirect> solve_dss.py case.dss

Output: {"converged": bool, "voltages": {"<bus>.<node>": [re, im]}, ...} with
voltages in volts. Run it under an interpreter that has opendssdirect
installed; the test harness locates one via the PIO_DSS_PYTHON env var.
"""

import json
import sys


def solve(path):
    import opendssdirect as dss

    dss.Text.Command("Clear")
    dss.Text.Command(f'Redirect "{path}"')
    dss.Text.Command("Solve")

    volts = {}
    for bus in dss.Circuit.AllBusNames():
        dss.Circuit.SetActiveBus(bus)
        nodes = dss.Bus.Nodes()
        raw = dss.Bus.Voltages()  # interleaved re, im per node
        for k, node in enumerate(nodes):
            volts[f"{bus}.{node}"] = [raw[2 * k], raw[2 * k + 1]]

    return {
        "case": path,
        "converged": bool(dss.Solution.Converged()),
        "iterations": dss.Solution.Iterations(),
        "voltages": volts,
    }


def main():
    if len(sys.argv) != 2:
        print(__doc__, file=sys.stderr)
        return 2
    print(json.dumps(solve(sys.argv[1]), indent=1, sort_keys=True))
    return 0


if __name__ == "__main__":
    sys.exit(main())
