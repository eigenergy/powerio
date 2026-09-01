"""Install-and-import smoke for one built wheel (#325).

Runs in a clean environment with the wheel (and its `all` extra) installed:
imports powerio, parses a case, verifies the reported version and schema
identity, narrows a typed module, builds one matrix, and checks the stored
document writes deterministically.
"""

import json
import io

import powerio

CASE = """function mpc = smoke
mpc.version = '2';
mpc.baseMVA = 100;
mpc.bus = [1 3 0 0 0 0 1 1.0 0 230 1 1.1 0.9; 2 1 30 10 0 0 1 1.0 0 230 1 1.1 0.9; 3 1 20 5 0 0 1 1.0 0 230 1 1.1 0.9;];
mpc.gen = [1 60 0 50 -50 1.0 100 1 120 0;];
mpc.branch = [1 2 0.01 0.1 0 250 250 250 0 0 1 -30 30; 2 3 0.02 0.2 0 250 250 250 0 0 1 -30 30; 1 3 0.02 0.25 0 250 250 250 0 0 1 -30 30;];
"""


def main() -> None:
    versions = powerio.versions()
    assert powerio.__version__ == versions["powerio_version"], versions
    assert versions["module_schema"] == {"name": "powerio.module", "version": 1}, versions

    module = powerio.parse(
        io.StringIO(CASE),
        name="smoke.m",
        format="matpower",
    )
    assert isinstance(module.value, powerio.BalancedNetwork), type(module.value)
    net = module.value
    assert net.n_buses == 3 and net.n_branches == 3, (net.n_buses, net.n_branches)

    document = powerio.serialize(module).text
    assert document is not None
    decoded = json.loads(document)
    assert decoded["schema"] == "powerio.module" and decoded["version"] == 1
    # Deterministic release: the stored document is byte stable.
    assert (
        powerio.serialize(powerio.deserialize(document.encode())).text == document
    )

    incidence = net.calc_incidence_matrix()
    bus_susceptance = net.calc_bus_susceptance_matrix()
    assert incidence.shape == (3, 3), incidence.shape
    assert bus_susceptance.shape == (3, 3), bus_susceptance.shape

    # The matrix path, with the `all` extra installed.
    bprime = net.calc_bprime_matrix()
    assert bprime.shape == (3, 3), bprime.shape

    print(
        "wheel smoke OK:",
        versions["powerio_version"],
        f"module_schema={versions['module_schema']['name']}/{versions['module_schema']['version']}",
    )


if __name__ == "__main__":
    main()
