#!/usr/bin/env python3
"""Electrical checks for fresh native open-switch output; no SINCAL execution.
Independent MNA compares original typed circuits and native readbacks. OpenDSS
solves the original feeder with the tie disconnected (an open ideal tie has no
admittance). Deliberately closing each recovered tie must change the solution.
"""
import argparse
import copy
import hashlib
import json
from pathlib import Path

from check_multiconductor_writer import solve, opendss_reference, VOLTAGE_TOLERANCE, EPS_OHM, np, dss


def check(directory):
    cases = []
    reference, deck_hash = opendss_reference(False)
    expected = [['1'], ['2'], ['3'], ['1', '2'], ['2', '3'], ['3', '1'], ['1', '2', '3']]
    for i, phases in enumerate(expected):
        path = directory / f'case{i}.json'
        doc = json.loads(path.read_text())
        if doc['phases'] != phases:
            raise ValueError('wrong case coverage')
        original, residual = solve(doc['input'])
        recovered, reread_residual = solve(doc['fresh_readback'])
        pairs = [((bus, p), (str(native), p)) for bus, native in doc['bus_ids'].items()
                 for p in ['1', '2', '3'] if (bus, p) in original]
        error = max(abs(original[a] - recovered[b]) for a, b in pairs)
        engine_error = max(abs(original[k] - reference[k]) for k in reference)
        if error > 1e-9 or engine_error > VOLTAGE_TOLERANCE:
            raise ValueError('fresh native readback or independent engine mismatch')
        wrong = copy.deepcopy(doc['fresh_readback'])
        open_ports = [s for s in wrong['switches'] if s['open']]
        if len(open_ports) != 1 or open_ports[0]['terminal_map_from'] != phases:
            raise ValueError('wrong recovered open terminal')
        open_ports[0]['open'] = False
        corrupt, _ = solve(wrong)
        difference = max(abs(corrupt[b] - recovered[b]) for _, b in pairs)
        if difference < .01:
            raise ValueError('closed-tie counterexample was not detected')
        cases.append(dict(phases=phases, fresh_voltage_error_v=float(error),
                          opendss_voltage_error_v=float(engine_error),
                          closed_tie_counterexample_v=float(difference),
                          mna_residual=float(max(residual, reread_residual)),
                          export_sha256=hashlib.sha256(path.read_bytes()).hexdigest()))
    return dict(scope=__doc__, cases=cases, opendss_deck_sha256=deck_hash,
                tolerance_v=VOLTAGE_TOLERANCE, fresh_tolerance_v=1e-9,
                ideal_source_engine_approximation_ohms=EPS_OHM,
                numpy=np.__version__, engine=dss.Basic.Version(), native_execution=False, passed=True)


if __name__ == '__main__':
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('directory', type=Path)
    p.add_argument('report', type=Path)
    args = p.parse_args()
    report = check(args.directory)
    args.report.write_text(json.dumps(report, indent=2) + '\n')
    print(f"{len(report['cases'])} open-switch cases and closed-tie negative controls passed")
