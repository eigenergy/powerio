"""Check generated phase-tap reactance with native PowSyBl (#829).

Run: python evals/powsybl/check_phase_tap_base.py /path/to/powerio
Requires pypowsybl (tested with 1.16.1). All input is synthetic and temporary.
This checks imported parameters and tap steps; it does not run a power flow.
"""

import io
import math
import subprocess
import sys
import tempfile
import xml.etree.ElementTree as ET
import zipfile
from pathlib import Path

import pypowsybl as pp


def load_profiles(profiles):
    buffer = io.BytesIO()
    with zipfile.ZipFile(buffer, "w") as archive:
        for name, content in profiles.items():
            archive.writestr(name, content)
    buffer.seek(0)
    return pp.network.load_from_binary_buffers([buffer])


def check(binary):
    cases = 0
    with tempfile.TemporaryDirectory() as temporary:
        root = Path(temporary)
        for kv2 in (20.0, 220.0):
            for tap in (0.9, 1.0, 1.05, 1.2):
                for shift in (-10.0, 10.0, 25.0):
                    source = root / "case.m"
                    source.write_text(f"""function mpc = case_phase
mpc.version = '2';
mpc.baseMVA = 100;
mpc.bus = [1 3 0 0 0 0 1 1 0 220 1 1.1 0.9;
           2 1 10 2 0 0 1 1 0 {kv2} 1 1.1 0.9];
mpc.gen = [1 10 2 100 -100 1 100 1 100 0];
mpc.branch = [1 2 0.01 0.1 0 150 150 150 {tap} {shift} 1 -360 360];
mpc.gencost = [2 0 0 2 1 0];
""")
                    directory = root / f"out-{cases}"
                    subprocess.run(
                        [binary, "convert", str(source), "--to", "cgmes", "-o", str(directory)],
                        check=True, capture_output=True, text=True,
                    )
                    profiles = {path.name: path.read_bytes() for path in directory.glob("*.xml")}
                    native = load_profiles(profiles)
                    transformers = native.get_2_windings_transformers()
                    assert len(transformers) == 1
                    transformer = transformers.iloc[0]
                    # IIDM refers transformer impedance to rated voltage 2.
                    assert math.isclose(transformer.x, 0.1 * kv2 ** 2 / 100, rel_tol=1e-10)
                    assert math.isclose(transformer.r, 0.01 * kv2 ** 2 / 100, rel_tol=1e-10)
                    assert math.isclose(transformer.rated_u1, 220 * tap, rel_tol=1e-10)
                    assert math.isclose(transformer.rated_u2, kv2, rel_tol=1e-10)
                    steps = native.get_phase_tap_changer_steps()
                    assert len(steps) == 3
                    for field in ("r", "x", "g", "b"):
                        assert (steps[field].abs() < 1e-9).all(), (tap, shift, steps)
                    assert sorted(steps.alpha) == sorted((0, -shift, -2 * shift))

                    # Recreate the old writer's voltage-base mismatch. The
                    # native importer must observe the resulting deviation.
                    if tap != 1.0 and shift > 0:
                        mutated = dict(profiles)
                        for name, content in profiles.items():
                            if not name.endswith("_EQ.xml"):
                                continue
                            xml = ET.fromstring(content)
                            for element in xml.iter():
                                if element.tag.rsplit("}", 1)[-1] in (
                                    "PhaseTapChangerLinear.xMin", "PhaseTapChangerLinear.xMax"
                                ):
                                    element.text = str(0.1 * 220 ** 2 / 100)
                            mutated[name] = ET.tostring(xml)
                        old_steps = load_profiles(mutated).get_phase_tap_changer_steps()
                        expected = 100 * (1 / tap ** 2 - 1)
                        assert all(math.isclose(value, expected, abs_tol=1e-9) for value in old_steps.x)
                    cases += 1
    print(f"PASS: {cases} CGMES cases, native PowSyBl {pp.__version__}; old-base negative controls detected")


if __name__ == "__main__":
    check(str(Path(sys.argv[1]).resolve()))
