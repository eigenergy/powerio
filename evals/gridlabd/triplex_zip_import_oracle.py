"""Exercise triplex ZIP initialization through the service import oracle."""

import re

import split_phase_import_oracle as service

BASE_CASES = service.cases()


def cases():
    result = []
    profiles = {
        "ieee9500": "base_power_1 3376.29; power_fraction_1 1; power_pf_1 .97; base_power_2 3376.29; power_fraction_2 1; power_pf_2 .97;",
        "mixed": "base_power_1 1200; power_fraction_1 .2; current_fraction_1 .3; impedance_fraction_1 .5; power_pf_1 .85; current_pf_1 -.9; impedance_pf_1 0; base_power_2 800; current_fraction_2 .2; impedance_fraction_2 .3; power_pf_2 -.8; base_power_12 500; power_fraction_12 .4; current_fraction_12 .4; impedance_fraction_12 .2; current_pf_12 -.7;",
        "negative-and-override": "base_power_1 -300; power_pf_1 -.8; base_power_2 700; power_pf_2 0; base_power_12 200; current_fraction_12 1; constant_power_1 9999+8888j; constant_impedance_2 1+1j;",
        "zero-base-keeps-explicit": "base_power_1 0; power_fraction_1 .5; constant_power_1 500+100j; constant_impedance_2 20+6j; constant_current_12 1-.3j;",
    }
    for index, (name, fields) in enumerate(profiles.items()):
        _, phase, text, _ = BASE_CASES[index % 3]
        for solver in ("NR", "FBS"):
            text_solver = re.sub(
                r"solver_method (NR|FBS)", "solver_method " + solver, text
            )

            def replace(match, fields=fields):
                body = re.sub(
                    r"constant_(?:power|current|impedance)_\w+\s+[^;]+;", "", match[1]
                )
                return "object triplex_load {" + body + fields + "}"

            text_solver = re.sub(
                r"object triplex_load\s*\{([^{}]*)\}", replace, text_solver
            )
            text_solver = re.sub(r"//[^\n]*", "", text_solver).replace(";", ";\n")
            result.append((name + "-" + solver, phase, text_solver, False))
    return result


if __name__ == "__main__":
    service.cases = cases
    raise SystemExit(service.main())
