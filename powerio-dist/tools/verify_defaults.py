"""Empirically dump OpenDSS constructor defaults for the phase A classes.

Usage: <python-with-opendssdirect> verify_defaults.py

Creates one bare object per class in a throwaway circuit and prints every
property value the engine reports. The Rust defaults table
(powerio-dist/src/dss/defaults.rs) is checked against this output; rerun it
when bumping the engine version.
"""

import sys


CASES = [
    ("Vsource", "source", None),  # the circuit's own source, all defaults
    ("Line", "l_def", "bus1=a bus2=b"),
    ("Linecode", "lc_def", ""),
    ("Load", "ld_def", "bus1=a"),
    ("Transformer", "t_def", "buses=(a, b)"),
    ("Capacitor", "c_def", "bus1=a"),
    ("Generator", "g_def", "bus1=a"),
]


def main():
    import opendssdirect as dss

    dss.Text.Command("Clear")
    dss.Text.Command("New Circuit.defaults_probe")
    for cls, name, props in CASES:
        if props is not None:
            dss.Text.Command(f"New {cls}.{name} {props}")
        full = f"{cls}.{name}"
        dss.Circuit.SetActiveElement(full)
        # Properties API works for general (non circuit) elements too.
        dss.Text.Command(f"? {full}.name")
        print(f"== {full}")
        for prop in dss.Element.AllPropertyNames():
            dss.Text.Command(f"? {full}.{prop}")
            print(f"  {prop} = {dss.Text.Result()}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
