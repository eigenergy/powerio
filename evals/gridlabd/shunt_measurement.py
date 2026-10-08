"""Measure generated shunts without inserting expected reference currents.

Arras 4.3 does not publish load measured_power/current_inj properties. A meter
between each generated load and its parent measures that load alone under FBS.
Arras NR ignores grandchild loads, so NR retains original parent relationships,
reads native impedance coefficients and solved voltages, and independently
checks root-meter source current. No link impedance is added. Instrumentation
is explicit in engine inputs; canonical output itself is not changed on disk.
"""

import re
from oracle import recorded

PAIRS = ("AB", "BC", "CA", "AN", "BN", "CN")


def loads(text):
    return re.findall(
        r'object load\s*\{\s*name\s+"([^"\n]+__glm_shunt_(?:from|to)(?:_\d+)?)";\s*parent\s+"([^"\n]+)";',
        text,
    )


def instrument(text, phases):
    outputs = []
    if "solver_method NR;" in text:
        # Arras NR does not propagate a grandchild load through a parented
        # meter. Keep original parent relationships. Read native impedance
        # coefficients and solved voltages, and independently witness total
        # source current with a root meter (no added electrical edges).
        text = text.replace(
            'object node { name "source";', 'object meter { name "source";'
        )
        text += 'object recorder { parent source; property measured_current_A,measured_current_B,measured_current_C; interval 1; file "source_meter.csv"; }\n'
        outputs.append("source_meter.csv")
        for name, _ in loads(text):
            props = ",".join(
                [f"voltage_{p}" for p in phases]
                + [f"constant_impedance_{p}" for p in PAIRS]
            )
            text += f'object recorder {{ parent "{name}"; property {props}; interval 1; file "{name}.csv"; }}\n'
            outputs.append(name + ".csv")
        return text, tuple(outputs)
    for name, bus in loads(text):
        meter = name + "__oracle_meter"
        if meter in text:
            raise ValueError("oracle meter identity collision")
        text = text.replace(
            f'name "{name}"; parent "{bus}";',
            f'name "{name}"; parent "{meter}";',
        )
        text += f'\nobject meter {{ name "{meter}"; parent "{bus}"; phases {phases}N; nominal_voltage 240; }}\n'
        props = ",".join(
            f"{q}_{p}" for p in phases for q in ("voltage", "measured_current")
        )
        text += f'object recorder {{ parent "{meter}"; property {props}; interval 1; file "{name}.csv"; }}\n'
        outputs.append(name + ".csv")
    return text, tuple(outputs)


def measurements(directory, phases):
    text = (directory / "model.glm").read_text()
    if "solver_method NR;" in text:
        for name, bus in loads(text):
            row = recorded(directory / (name + ".csv"))
            voltage = [row[f"voltage_{p}"] for p in phases]
            current = [0j] * len(phases)
            for pair in PAIRS:
                z = row[f"constant_impedance_{pair}"]
                if z == 0:
                    continue
                a = phases.index(pair[0])
                b = None if pair[1] == "N" else phases.index(pair[1])
                amps = (voltage[a] - (0 if b is None else voltage[b])) / z
                current[a] += amps
                if b is not None:
                    current[b] -= amps
            yield name, bus, voltage, current
        return
    for name, bus in re.findall(
        r'object meter\s*\{\s*name\s+"([^"\n]+)__oracle_meter";\s*parent\s+"([^"\n]+)";',
        text,
    ):
        row = recorded(directory / (name + ".csv"))
        current = [row[f"measured_current_{p}"] for p in phases]
        voltage = [row[f"voltage_{p}"] for p in phases]
        yield name, bus, voltage, current
