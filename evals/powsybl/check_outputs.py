#!/usr/bin/env python3
"""Check fresh PowerIO output against official PowSybl networks."""

from __future__ import annotations

import argparse
import contextlib
import hashlib
import io
import json
import math
import re
import shutil
import uuid
import xml.etree.ElementTree as ET
import zipfile
from collections import Counter
from collections.abc import Callable
from importlib.metadata import version as distribution_version
from pathlib import Path
from typing import Any

import pandas as pd
import pypowsybl as pp

EXPECTED_BUSES = 9
EXPECTED_BRANCHES = 9
EXPECTED_GENERATORS = 3
EXPECTED_LOADS = 3
EXPECTED_PYPOWSYBL_VERSION = "1.16.1"
EXPECTED_POWSYBL_CORE_VERSION = "7.3.0"
EXPECTED_POWSYBL_CORE_COMMIT = "0939bfcc2c0c094de907dc818dd688b4cbfb7281"
EXPECTED_RUNTIME_VERSIONS = {
    "pandas": "3.0.5",
    "numpy": "2.5.2",
    "networkx": "3.6.1",
    "prettytable": "3.18.0",
}

CGMES_2415_RELATIVE = Path(
    "cgmes/cgmes-conformity/src/main/resources/conformity/cas-2/MicroGrid/"
    "Type2_T2/CGMES_v2.4.15_MicroGridTestConfiguration_T2_Assembled_Complete_v2"
)
CGMES_30_RELATIVE = Path(
    "cgmes/cgmes-conformity/src/main/resources/conformity/cas-3-data-3.0.2/"
    "MicroGrid/BaseCase/MicroGrid-BaseCase-Merged"
)
TWO_SUBSTATIONS_RELATIVE = Path(
    "psse/psse-converter/src/test/resources/twoSubstations_rev35.rawx"
)
SWITCHED_SHUNT_RELATIVE = Path(
    "psse/psse-converter/src/test/resources/SwitchedShunt.raw"
)
NODE_BREAKER_RELATIVE = Path(
    "psse/psse-model-test/src/main/resources/five_bus_nodeBreaker_rev35.raw"
)
NODE_BREAKER_XIIDM_RELATIVE = Path(
    "psse/psse-converter/src/test/resources/five_bus_nodeBreaker_rev35.xiidm"
)
NODE_BREAKER_XIIDM_SHA256 = (
    "5c25f95d2abcd194c4ee5f797a32b59b0ee9fd075cb348056de0766149277214"
)
REMOTE_CONTROL_RELATIVE = Path(
    "psse/psse-converter/src/test/resources/remoteControl.xiidm"
)
TWO_TERMINAL_DC_RELATIVE = Path(
    "psse/psse-converter/src/test/resources/twoTerminalDc.xiidm"
)
XIIDM_VERSION_FIXTURES = {
    "1.12": (
        Path("iidm/iidm-serde/src/test/resources/V1_12/threeWindingsTransformerToBeEstimated.xiidm"),
        "0f2af9ff86338cd06caf6c0229c96b1e35f7ad61544aec5ae41b4709668b77df",
    ),
    "1.13": (
        Path("iidm/iidm-serde/src/test/resources/V1_13/threeWindingsTransformerToBeEstimated.xiidm"),
        "75bab6e318fd4a0d508a1dbc866bcb7feda9eecb2331e0b0d36f4cdd91bb5986",
    ),
    "1.14": (
        Path("iidm/iidm-serde/src/test/resources/V1_14/threeWindingsTransformerToBeEstimated.xiidm"),
        "b3ac37a6dd948424cf13acd26df628d60e6d6e58dc2da6ae130fbc2daaeb05d6",
    ),
    "1.15": (
        Path("iidm/iidm-serde/src/test/resources/V1_15/threeWindingsTransformerToBeEstimated.xiidm"),
        "5222c300d51783de02017223bc025365a73053ff26ce212801dc026afe8003a3",
    ),
    "1.16": (
        Path("iidm/iidm-serde/src/test/resources/V1_16/threeWindingsTransformerToBeEstimated.xiidm"),
        "900799a726702fa9d6e7ea840f4df73fcde6f16a2d44ab040ef174518ce96e5d",
    ),
    "1.17": (
        Path("iidm/iidm-serde/src/test/resources/V1_17/threeWindingsTransformerToBeEstimated.xiidm"),
        "bf8154c0815510125dc5f347caa41a587680e231b623c74e1160d956fb73116f",
    ),
}

CIM16 = "http://iec.ch/TC57/2013/CIM-schema-cim16#"
CIM100 = "http://iec.ch/TC57/CIM100#"
RDF = "http://www.w3.org/1999/02/22-rdf-syntax-ns#"
CGMES_UUID_NAMESPACE = uuid.uuid5(uuid.NAMESPACE_URL, "https://powerio.dev/cgmes")
CGMES_VALUE_SUBSTITUTED_PREFIX = "EMIT.CGMES.VALUE_SUBSTITUTED: "
SERIES_COMPENSATOR_ID = "df16b3dd-c905-4a6f-84ee-f067be86f5da"
REMOTE_GENERATOR_ID = "3a3b27be-b18b-4385-b557-6735d733baf0"
REMOTE_REGULATED_ELEMENT_ID = "a708c3bc-465d-4fe7-b6ef-6fa6408a62b0"
REMOTE_GENERATOR_SV_STATUS_ID = "1805f4e6-d146-49ed-952b-b4cf86d68a1a"
HVDC_LINE_ID = "d7693c6d-58bd-49da-bb24-973a63f9faf1"
VSC_TARGET_Q = {
    "0f05e270-37ea-471d-89fe-aee8a55b932b": 40.0,
    "76eeb38f-a3ef-4444-9c65-6cb46a7a94da": -40.0,
}

CGMES_30_SOURCE_COUNTS = {
    "buses": 10,
    "lines": 3,
    "two_winding_transformers": 6,
    "three_winding_transformers": 1,
    "generators": 5,
    "loads": 6,
    "shunts": 3,
    "static_var_compensators": 1,
    "tie_lines": 5,
    "boundary_lines": 10,
}
CGMES_2415_SOURCE_COUNTS = {
    "buses": 18,
    "lines": 4,
    "two_winding_transformers": 10,
    "three_winding_transformers": 1,
    "generators": 5,
    "loads": 6,
    "shunts": 3,
    "static_var_compensators": 0,
    "tie_lines": 1,
    "boundary_lines": 12,
}
CGMES_30_REACTIVE_CURVE = (
    (-100.0, -200.0, 200.0),
    (0.0, -300.0, 300.0),
    (200.0, -200.0, 200.0),
)
CGMES_2415_REACTIVE_CURVE = (
    (-100.0, -200.0, 200.0),
    (0.0, -300.0, 300.0),
    (100.0, -200.0, 200.0),
)
CGMES_30_SV_STATUS_COUNT = 52
CGMES_30_PAIRED_AUTHORITY_VOLTAGES = (
    (
        "cf29c76d-daaf-4f4c-b4c7-8a8c46089fbd",
        "1a4d0f02-6adb-4fa2-a828-b0366497ae5a",
        414.0277,
        339.5112,
    ),
    (
        "9408a842-1d22-49ef-abd2-d3cfa8b087b8",
        "eff86931-991c-4f6f-b5d6-75dad40e0a12",
        412.6039,
        339.568,
    ),
    (
        "721de4d4-4271-4ef6-8f8e-22b29bc213c3",
        "ebd2d2fd-3cfc-466d-a542-104ce0e8fd4d",
        223.2214,
        -9.052585,
    ),
    (
        "3c68ee89-7a4f-4aeb-8c95-9356398d204c",
        "994d58d1-48c9-477d-8ba4-ec143e714ba1",
        412.8738,
        339.5562,
    ),
    (
        "bf4d0111-51f6-4c69-934e-8ffaefe6ff15",
        "88f1e099-9930-4ad2-a477-f301e35ca9a4",
        223.2396,
        -11.03524,
    ),
)
CGMES_30_BOUNDARY_SV_OMISSIONS = (
    (
        "d32c84c9-3277-4579-9c51-c28d5eb6a027",
        "6aecb9ba-5835-4b70-89bc-96d687e45779",
        "a279a3dc-550b-426c-af3a-61b7be508dcc",
        "-103.7413",
        "11.31944",
    ),
    (
        "df359307-4034-4879-b636-048fde40252e",
        "5dfee914-a4fd-4bde-a5b4-c4caa6378d10",
        "a279a3dc-550b-426c-af3a-61b7be508dcc",
        "106.0121",
        "2.926857",
    ),
    (
        "69f01012-4ce8-4177-9a61-b169fd44fa85",
        "ae588863-b154-451d-978a-7ab08ac50fb6",
        "e8acf6b6-99cb-45ad-b8dc-16c7866a4ddc",
        "-27.0286",
        "120.7887",
    ),
    (
        "cefca669-270e-4e51-b953-5c94f58fe587",
        "757d4f50-707b-47a0-891c-cbaefd649631",
        "e8acf6b6-99cb-45ad-b8dc-16c7866a4ddc",
        "33.00131",
        "-131.1585",
    ),
    (
        "7d61f2c1-7988-4912-ab6f-8824d2922e74",
        "c557146e-dfc4-4020-9738-d592b188338b",
        "dad02278-bd25-476f-8f58-dbe44be72586",
        "15.82242",
        "-70.92072",
    ),
    (
        "b3e1a065-79f6-414a-b8c2-358718cf1ed9",
        "8f308117-8bbd-4798-b145-4e78f7d049e7",
        "dad02278-bd25-476f-8f58-dbe44be72586",
        "-8.953211",
        "67.2335",
    ),
    (
        "cb73a0dc-c910-4288-a930-eda39925c607",
        "653bc4b8-518b-4adc-9d68-012fb641fb1d",
        "8fdc7abd-3746-481a-a65e-3df56acd8b13",
        "84.71166",
        "3.317878",
    ),
    (
        "2a2b7b8e-77d3-498c-abc4-f0f194fda7bc",
        "57979d14-d1d8-4311-ae53-aa8890423885",
        "8fdc7abd-3746-481a-a65e-3df56acd8b13",
        "-83.18928",
        "1.501529",
    ),
    (
        "594eb33c-ddcd-48af-9810-85ea4744d200",
        "357e3e14-c38e-4a7e-9c95-b3dbe158f5f3",
        "7f43f508-2496-4b64-9146-0a40406cbe49",
        "19.19087",
        "-87.50481",
    ),
    (
        "e783e2c1-8497-4f89-b2ed-36eb2ea161db",
        "6b1cd30c-19ba-44e1-9447-d01db6b1ef9d",
        "7f43f508-2496-4b64-9146-0a40406cbe49",
        "-14.06748",
        "63.95825",
    ),
)
CGMES_30_EQUIPMENT_RECORD_COUNT = 57
CGMES_2415_EQUIPMENT_RECORD_COUNT = 82
CGMES_NUMERIC_REL_TOL = 1e-9
CGMES_NUMERIC_ABS_TOL = 1e-8
CGMES_ELECTRICAL_FIELDS = {
    "ordinary line": {
        "numeric": ("r", "x", "g1", "b1", "g2", "b2"),
        "exact": (
            "voltage_level1_id",
            "voltage_level2_id",
            "connected1",
            "connected2",
        ),
    },
    "2W transformer": {
        "numeric": (
            "r",
            "x",
            "g",
            "b",
            "rated_u1",
            "rated_u2",
            "rho",
            "alpha",
        ),
        "exact": (
            "voltage_level1_id",
            "voltage_level2_id",
            "connected1",
            "connected2",
        ),
    },
    "3W transformer": {
        "numeric": (
            "rated_u0",
            "r1",
            "x1",
            "g1",
            "b1",
            "rated_u1",
            "rho1",
            "alpha1",
            "r2",
            "x2",
            "g2",
            "b2",
            "rated_u2",
            "rho2",
            "alpha2",
            "r3",
            "x3",
            "g3",
            "b3",
            "rated_u3",
            "rho3",
            "alpha3",
        ),
        "exact": (
            "voltage_level1_id",
            "voltage_level2_id",
            "voltage_level3_id",
            "connected1",
            "connected2",
            "connected3",
        ),
    },
    "generator": {
        "numeric": (
            "target_p",
            "target_q",
            "min_p",
            "max_p",
            "min_q",
            "max_q",
            "rated_s",
            "target_v",
        ),
        "exact": (
            "energy_source",
            "reactive_limits_kind",
            "voltage_regulator_on",
            "regulated_element_id",
            "voltage_level_id",
            "connected",
        ),
    },
    "load": {
        "numeric": ("p0", "q0"),
        "exact": ("voltage_level_id", "connected"),
    },
    "shunt": {
        "numeric": (
            "g",
            "b",
            "section_count",
            "max_section_count",
            "target_v",
            "target_deadband",
        ),
        "exact": (
            "model_type",
            "voltage_regulation_on",
            "voltage_level_id",
            "connected",
        ),
    },
    "static var compensator": {
        "numeric": ("b_min", "b_max", "target_v", "target_q"),
        "exact": (
            "regulation_mode",
            "regulating",
            "regulated_element_id",
            "voltage_level_id",
            "connected",
        ),
    },
    "VSC converter": {
        "numeric": (
            "loss_factor",
            "min_q",
            "max_q",
            "target_v",
            "target_q",
        ),
        "exact": (
            "reactive_limits_kind",
            "voltage_regulator_on",
            "regulated_element_id",
            "voltage_level_id",
            "connected",
            "hvdc_line_id",
        ),
    },
    "HVDC line": {
        "numeric": ("target_p", "max_p", "nominal_v", "r"),
        "exact": (
            "converters_mode",
            "converter_station1_id",
            "converter_station2_id",
            "connected1",
            "connected2",
        ),
    },
}
CGMES_SOLUTION_FIELDS = {
    "bus solution": {
        "numeric": ("v_mag", "v_angle"),
        "exact": ("voltage_level_id",),
    },
    "ordinary line solution": {
        "numeric": ("p1", "q1", "p2", "q2"),
        "exact": (),
    },
    "2W transformer solution": {
        "numeric": ("p1", "q1", "p2", "q2"),
        "exact": (),
    },
    "3W transformer solution": {
        "numeric": ("p1", "q1", "p2", "q2", "p3", "q3"),
        "exact": (),
    },
    "generator solution": {
        "numeric": ("p", "q"),
        "exact": (),
    },
    "load solution": {
        "numeric": ("p", "q"),
        "exact": (),
    },
    "shunt solution": {
        "numeric": ("p", "q"),
        "exact": (),
    },
    "static var compensator solution": {
        "numeric": ("p", "q"),
        "exact": (),
    },
    "VSC converter solution": {
        "numeric": ("p", "q"),
        "exact": (),
    },
}
CGMES_UNAVAILABLE_SOLUTION_BUSES = {
    "CGMES 3.0": set(),
    "CGMES 2.4.15": {
        "bccf01d1-680c-4683-91b9-9d19748519cb_0",
        "d966db16-ec73-4c00-a9bc-8aae3aaa0573_0",
    },
}
CGMES_2415_TRANSFORMER_CONNECTION_PROJECTIONS = (
    (
        "d1494778-e194-4ee5-84ec-ac8024375e4f",
        "bccf01d1-680c-4683-91b9-9d19748519cb_0",
        "5cae357a-731f-4ab1-b133-4f90795206c7",
        "d74faa88-d996-4de3-be52-f5db28dd3fb8",
    ),
    (
        "b59c4282-a1f7-4ed2-a6de-2a5b97c03f64",
        "d966db16-ec73-4c00-a9bc-8aae3aaa0573_0",
        "f3c8e35e-744f-431a-8505-dfe3c275616a",
        "dde55045-9d3e-43b8-8d28-0585e30cd6b1",
    ),
)
CGMES_2415_JUNCTION_CONTAINER_PROJECTION = (
    "5249A78F-6642-4fc5-968F-06E2ED18FAB7",
    "Junction_XJ1",
    "f03d65b2a51049ffa533e433721145c1_X",
    "6cd9f4f5-8185-57c9-a8a3-53de5e0ddeb1",
    "TieLine_XWI_GY11",
)
CGMES_LIMIT_EXPECTATIONS = {
    "CGMES 3.0": (25, 0),
    "CGMES 2.4.15": (93, 31),
}
CGMES_BOUNDARY_LIMIT_EXPECTATIONS = {
    "CGMES 3.0": (20, 40),
    "CGMES 2.4.15": (24, 162),
}
CGMES_LIMIT_KIND_EXPECTATIONS = {
    "CGMES 3.0": (
        Counter({"patl": 2, "tatl": 2}),
        Counter({"patl": 1, "tatl": 1}),
    ),
    "CGMES 2.4.15": (
        Counter({"patl": 3, "patlt": 3, "tatl": 6, "tct": 3}),
        Counter({"patl": 1, "tatl": 3}),
    ),
}
CGMES_BOUNDARY_LINE_NO_DIRECT_SV = {
    "CGMES 3.0": {
        "a279a3dc-550b-426c-af3a-61b7be508dcc",
        "e8acf6b6-99cb-45ad-b8dc-16c7866a4ddc",
        "dad02278-bd25-476f-8f58-dbe44be72586",
        "8fdc7abd-3746-481a-a65e-3df56acd8b13",
        "7f43f508-2496-4b64-9146-0a40406cbe49",
    },
    "CGMES 2.4.15": {
        "8fdc7abd-3746-481a-a65e-3df56acd8b13",
        "7f43f508-2496-4b64-9146-0a40406cbe49",
        "882271f9-1793-4fc2-8b2e-12a37292f364",
        "e8acf6b6-99cb-45ad-b8dc-16c7866a4ddc",
        "dad02278-bd25-476f-8f58-dbe44be72586",
        "63f6d38b-56fb-4dae-804d-9777d5f30f80",
        "6052bacf-9eaa-4217-be91-4c7c89e92a52",
        "ed0c5d75-4a54-43c8-b782-b20d7431630b",
        "b18cd1aa-7808-49b9-a7cf-605eaf07b006",
        "78736387-5f60-4832-b3fe-d50daf81b0a6",
        "a16b4a6c-70b1-4abf-9a9d-bd0fa47f9fe4",
        "17086487-56ba-4979-b8de-064025a6b4da",
    },
}
CGMES_TAP_EXPECTATIONS = {
    "CGMES 3.0": {
        "ratio changers": 3,
        "ratio steps": 97,
        "phase changers": 4,
        "phase steps": 125,
    },
    "CGMES 2.4.15": {
        "ratio changers": 10,
        "ratio steps": 271,
        "phase changers": 1,
        "phase steps": 25,
    },
}
CGMES_TAP_FIELDS = {
    "ratio changer": {
        "numeric": ("target_v", "target_deadband"),
        "exact": (
            "tap",
            "low_tap",
            "high_tap",
            "step_count",
            "oltc",
            "regulating",
            "regulated_side",
        ),
    },
    "ratio step": {
        "numeric": ("rho", "r", "x", "g", "b"),
        "exact": (),
    },
    "phase changer": {
        "numeric": ("regulation_value", "target_deadband"),
        "exact": (
            "tap",
            "low_tap",
            "high_tap",
            "step_count",
            "oltc",
            "regulating",
            "regulation_mode",
            "regulated_side",
        ),
    },
    "phase step": {
        "numeric": ("rho", "alpha", "r", "x", "g", "b"),
        "exact": (),
    },
}
XIIDM_ELECTRICAL_FIELDS = {
    "voltage level": {
        "numeric": ("nominal_v", "high_voltage_limit", "low_voltage_limit"),
        "exact": ("name", "substation_id", "topology_kind"),
    },
    "bus": {
        "numeric": ("v_mag", "v_angle"),
        "exact": ("name", "voltage_level_id"),
    },
    "line": {
        "numeric": ("r", "x", "g1", "b1", "g2", "b2", "p1", "q1", "p2", "q2"),
        "exact": (
            "name",
            "voltage_level1_id",
            "voltage_level2_id",
            "bus1_id",
            "bus_breaker_bus1_id",
            "node1",
            "bus2_id",
            "bus_breaker_bus2_id",
            "node2",
            "connected1",
            "connected2",
        ),
    },
    "2W transformer": {
        "numeric": (
            "r",
            "x",
            "g",
            "b",
            "rated_u1",
            "rated_u2",
            "rated_s",
            "rho",
            "alpha",
            "p1",
            "q1",
            "p2",
            "q2",
        ),
        "exact": (
            "name",
            "voltage_level1_id",
            "voltage_level2_id",
            "bus1_id",
            "bus_breaker_bus1_id",
            "node1",
            "bus2_id",
            "bus_breaker_bus2_id",
            "node2",
            "connected1",
            "connected2",
        ),
    },
    "3W transformer": {
        "numeric": (
            "rated_u0",
            "r1",
            "x1",
            "g1",
            "b1",
            "rated_u1",
            "rated_s1",
            "rho1",
            "alpha1",
            "p1",
            "q1",
            "r2",
            "x2",
            "g2",
            "b2",
            "rated_u2",
            "rated_s2",
            "rho2",
            "alpha2",
            "p2",
            "q2",
            "r3",
            "x3",
            "g3",
            "b3",
            "rated_u3",
            "rated_s3",
            "rho3",
            "alpha3",
            "p3",
            "q3",
        ),
        "exact": (
            "name",
            "voltage_level1_id",
            "bus1_id",
            "bus_breaker_bus1_id",
            "node1",
            "connected1",
            "voltage_level2_id",
            "bus2_id",
            "bus_breaker_bus2_id",
            "node2",
            "connected2",
            "voltage_level3_id",
            "bus3_id",
            "bus_breaker_bus3_id",
            "node3",
            "connected3",
        ),
    },
    "generator": {
        "numeric": (
            "target_p",
            "target_q",
            "min_p",
            "max_p",
            "min_q",
            "max_q",
            "rated_s",
            "target_v",
            "p",
            "q",
        ),
        "exact": (
            "name",
            "energy_source",
            "reactive_limits_kind",
            "voltage_regulator_on",
            "regulated_element_id",
            "voltage_level_id",
            "bus_id",
            "bus_breaker_bus_id",
            "node",
            "condenser",
            "connected",
        ),
    },
    "load": {
        "numeric": ("p0", "q0", "p", "q"),
        "exact": (
            "name",
            "type",
            "voltage_level_id",
            "bus_id",
            "bus_breaker_bus_id",
            "node",
            "connected",
        ),
    },
    "shunt": {
        "numeric": (
            "g",
            "b",
            "max_section_count",
            "section_count",
            "solved_section_count",
            "target_v",
            "target_deadband",
            "p",
            "q",
        ),
        "exact": (
            "name",
            "model_type",
            "voltage_regulation_on",
            "regulating_bus_id",
            "voltage_level_id",
            "bus_id",
            "bus_breaker_bus_id",
            "node",
            "connected",
        ),
    },
    "LCC converter": {
        "numeric": ("power_factor", "loss_factor", "p", "q"),
        "exact": (
            "name",
            "voltage_level_id",
            "bus_id",
            "bus_breaker_bus_id",
            "node",
            "connected",
            "hvdc_line_id",
        ),
    },
    "HVDC line": {
        "numeric": ("target_p", "max_p", "nominal_v", "r"),
        "exact": (
            "name",
            "converters_mode",
            "converter_station1_id",
            "converter_station2_id",
            "connected1",
            "connected2",
        ),
    },
    "operational limit": {
        "numeric": ("value",),
        "exact": ("element_type", "name", "fictitious", "selected"),
    },
}
XIIDM_TAP_FIELDS = {
    "ratio changer": {
        "numeric": ("solved_tap_position", "target_v", "target_deadband"),
        "exact": (
            "tap",
            "low_tap",
            "high_tap",
            "step_count",
            "oltc",
            "regulating",
            "regulating_bus_id",
            "regulated_side",
        ),
    },
    "ratio step": {
        "numeric": ("rho", "r", "x", "g", "b"),
        "exact": (),
    },
}
GENERATED_CGMES_ELECTRICAL_FIELDS = {
    equipment: {
        "numeric": fields["numeric"],
        "exact": tuple(
            column
            for column in fields["exact"]
            if not re.fullmatch(r"node[123]?", column)
        ),
    }
    for equipment, fields in XIIDM_ELECTRICAL_FIELDS.items()
}
GENERATED_CGMES_ELECTRICAL_FIELDS.update(
    {
        "2W transformer": {
            **GENERATED_CGMES_ELECTRICAL_FIELDS["2W transformer"],
            "numeric": tuple(
                column
                for column in XIIDM_ELECTRICAL_FIELDS["2W transformer"]["numeric"]
                if column != "rated_s"
            ),
        },
        "3W transformer": {
            **GENERATED_CGMES_ELECTRICAL_FIELDS["3W transformer"],
            "numeric": tuple(
                column
                for column in XIIDM_ELECTRICAL_FIELDS["3W transformer"]["numeric"]
                if column not in {"rated_s1", "rated_s2", "rated_s3"}
            ),
        },
    }
)
PSSE_ELECTRICAL_FIELDS = {
    **XIIDM_ELECTRICAL_FIELDS,
    "3W transformer": {
        "numeric": (
            "rated_u0",
            "r1",
            "x1",
            "g1",
            "b1",
            "rated_u1",
            "rated_s1",
            "rho1",
            "alpha1",
            "p1",
            "q1",
            "r2",
            "x2",
            "g2",
            "b2",
            "rated_u2",
            "rated_s2",
            "rho2",
            "alpha2",
            "p2",
            "q2",
            "r3",
            "x3",
            "g3",
            "b3",
            "rated_u3",
            "rated_s3",
            "rho3",
            "alpha3",
            "p3",
            "q3",
        ),
        "exact": (
            "name",
            "voltage_level1_id",
            "voltage_level2_id",
            "voltage_level3_id",
            "bus1_id",
            "bus_breaker_bus1_id",
            "node1",
            "bus2_id",
            "bus_breaker_bus2_id",
            "node2",
            "bus3_id",
            "bus_breaker_bus3_id",
            "node3",
            "connected1",
            "connected2",
            "connected3",
        ),
    },
}
PSSE_EXPECTATIONS = {
    "PSS/E switched shunt": {
        "voltage levels": 6,
        "buses": 7,
        "lines": 2,
        "2W transformers": 2,
        "3W transformers": 1,
        "generators": 4,
        "loads": 5,
        "shunts": 2,
        "operational limits": 11,
        "ratio changers": 3,
        "ratio steps": 25,
        "switches": 0,
    },
    "PSS/E RAWX": {
        "voltage levels": 5,
        "buses": 5,
        "lines": 1,
        "2W transformers": 1,
        "3W transformers": 1,
        "generators": 2,
        "loads": 2,
        "shunts": 1,
        "operational limits": 7,
        "ratio changers": 4,
        "ratio steps": 4,
        "switches": 41,
    },
    "PSS/E node breaker": {
        "voltage levels": 5,
        "buses": 5,
        "lines": 1,
        "2W transformers": 1,
        "3W transformers": 1,
        "generators": 1,
        "loads": 4,
        "shunts": 2,
        "operational limits": 0,
        "ratio changers": 4,
        "ratio steps": 10,
        "switches": 21,
    },
}
XIIDM_EXPECTATIONS = {
    "XIIDM remote control": {
        "voltage levels": 8,
        "buses": 9,
        "lines": 7,
        "2W transformers": 4,
        "3W transformers": 0,
        "generators": 4,
        "loads": 7,
        "shunts": 2,
        "LCC converters": 0,
        "HVDC lines": 0,
        "operational limits": 22,
        "ratio changers": 4,
        "ratio steps": 14,
        "switches": 0,
    },
    "XIIDM HVDC": {
        "voltage levels": 9,
        "buses": 9,
        "lines": 8,
        "2W transformers": 2,
        "3W transformers": 0,
        "generators": 4,
        "loads": 7,
        "shunts": 2,
        "LCC converters": 2,
        "HVDC lines": 1,
        "operational limits": 20,
        "ratio changers": 2,
        "ratio steps": 2,
        "switches": 0,
    },
    "XIIDM node breaker": {
        "voltage levels": 5,
        "buses": 5,
        "lines": 1,
        "2W transformers": 1,
        "3W transformers": 1,
        "generators": 1,
        "loads": 4,
        "shunts": 2,
        "LCC converters": 2,
        "HVDC lines": 1,
        "operational limits": 0,
        "ratio changers": 4,
        "ratio steps": 10,
        "switches": 21,
    },
    **{
        f"XIIDM {version}": {
            "voltage levels": 3,
            "buses": 3,
            "lines": 0,
            "2W transformers": 0,
            "3W transformers": 1,
            "generators": 1,
            "loads": 2,
            "shunts": 0,
            "LCC converters": 0,
            "HVDC lines": 0,
            "operational limits": 0,
            "ratio changers": 2,
            "ratio steps": 6,
            "switches": 0,
        }
        for version in XIIDM_VERSION_FIXTURES
    },
}
ASSERTION_COUNT = [0]


def require(condition: bool, message: str) -> None:
    ASSERTION_COUNT[0] += 1
    if not condition:
        raise AssertionError(message)


def check_powsybl_version() -> None:
    require(
        pp.__version__ == EXPECTED_PYPOWSYBL_VERSION,
        f"PyPowSybl {pp.__version__}, expected {EXPECTED_PYPOWSYBL_VERSION}",
    )
    output = io.StringIO()
    with contextlib.redirect_stdout(output):
        pp.print_version()
    version_rows = {
        fields[0]: (fields[1], fields[3])
        for line in output.getvalue().splitlines()
        if len(fields := [field.strip() for field in line.split("|") if field.strip()]) >= 4
    }
    require(
        version_rows.get("powsybl-core")
        == (EXPECTED_POWSYBL_CORE_VERSION, EXPECTED_POWSYBL_CORE_COMMIT),
        "PyPowSybl does not contain the pinned PowSybl Core 7.3.0 build",
    )
    for distribution, expected in EXPECTED_RUNTIME_VERSIONS.items():
        actual = distribution_version(distribution)
        require(
            actual == expected,
            f"{distribution} {actual}, expected {expected}",
        )


def require_validation(network: pp.network.Network, label: str) -> None:
    validation_level = network.validate()
    expected = pp.network.ValidationLevel.STEADY_STATE_HYPOTHESIS
    require(
        validation_level == expected,
        f"{label}: IIDM validation is {validation_level.name}, expected {expected.name}",
    )


def count_branches(network: pp.network.Network) -> int:
    return sum(
        len(frame)
        for frame in (
            network.get_lines(),
            network.get_2_windings_transformers(),
            network.get_3_windings_transformers(),
            network.get_tie_lines(),
            network.get_boundary_lines(),
        )
    )


def check_references(network: pp.network.Network, label: str) -> None:
    identifiables = network.get_identifiables()
    require(not identifiables.index.has_duplicates, f"{label}: duplicate identifiable ids")
    identifiable_ids = set(identifiables.index)

    voltage_levels = network.get_voltage_levels()
    voltage_level_ids = set(voltage_levels.index)
    require(voltage_level_ids, f"{label}: no voltage levels")

    bus_breaker_buses = network.get_bus_breaker_view_buses()
    require(len(bus_breaker_buses) > 0, f"{label}: no bus breaker buses")
    require(
        set(bus_breaker_buses["voltage_level_id"]).issubset(voltage_level_ids),
        f"{label}: a bus references an unknown voltage level",
    )

    buses = network.get_buses()
    bus_ids = set(buses.index)
    require(bus_ids, f"{label}: no calculated buses")

    terminals = network.get_terminals()
    require(len(terminals) > 0, f"{label}: no terminals")
    require(
        set(terminals.index).issubset(identifiable_ids),
        f"{label}: a terminal references an unknown element",
    )
    require(
        set(terminals["voltage_level_id"]).issubset(voltage_level_ids),
        f"{label}: a terminal references an unknown voltage level",
    )

    connected = terminals[terminals["connected"]]
    require(
        not connected["bus_id"].isna().any(),
        f"{label}: a connected terminal has no bus",
    )
    require(
        set(connected["bus_id"]).issubset(bus_ids),
        f"{label}: a connected terminal references an unknown bus",
    )


def load_checked(path: Path, label: str) -> pp.network.Network:
    require(path.exists(), f"{label}: missing {path}")
    network = pp.network.load(path)
    require_validation(network, label)
    check_references(network, label)
    return network


def check_case9(path: Path, label: str) -> None:
    network = load_checked(path, label)
    bus_count = len(network.get_buses())
    bus_breaker_count = len(network.get_bus_breaker_view_buses())
    branch_count = count_branches(network)
    generator_count = len(network.get_generators())
    load_count = len(network.get_loads())

    require(bus_count == EXPECTED_BUSES, f"{label}: {bus_count} buses, expected 9")
    require(
        bus_breaker_count == EXPECTED_BUSES,
        f"{label}: {bus_breaker_count} bus breaker buses, expected 9",
    )
    require(
        branch_count == EXPECTED_BRANCHES,
        f"{label}: {branch_count} branches, expected 9",
    )
    require(
        generator_count == EXPECTED_GENERATORS,
        f"{label}: {generator_count} generators, expected 3",
    )
    require(load_count == EXPECTED_LOADS, f"{label}: {load_count} loads, expected 3")
    print(
        f"{label}: validation=STEADY_STATE_HYPOTHESIS, buses={bus_count}, "
        f"branches={branch_count}, generators={generator_count}, loads={load_count}"
    )


def check_pow_sybl_rejects_raw_revision_34(path: Path) -> None:
    try:
        pp.network.load(path)
    except pp.PyPowsyblError as error:
        require(
            "Unsupported file format or invalid file" in str(error),
            f"PowSybl rejected PSS/E RAW revision 34 for an unexpected reason: {error}",
        )
    else:
        raise AssertionError(
            "PowSybl Core 7.3.0 unexpectedly accepted PSS/E RAW revision 34; "
            "update its declared revision coverage and this check"
        )
    print(
        "case9 PSS/E RAW revision 34: PowerIO output written; "
        "PowSybl Core 7.3.0 does not support revision 34"
    )


def cgmes_counts(network: pp.network.Network) -> dict[str, int]:
    return {
        "buses": len(network.get_buses()),
        "lines": len(network.get_lines()),
        "two_winding_transformers": len(network.get_2_windings_transformers()),
        "three_winding_transformers": len(network.get_3_windings_transformers()),
        "generators": len(network.get_generators()),
        "loads": len(network.get_loads()),
        "shunts": len(network.get_shunt_compensators()),
        "static_var_compensators": len(network.get_static_var_compensators()),
        "tie_lines": len(network.get_tie_lines()),
        "boundary_lines": len(network.get_boundary_lines()),
    }


def element_ids(getter: Callable[..., Any]) -> set[str]:
    return set(getter().index)


def check_cgmes_projection(
    source: pp.network.Network,
    fresh: pp.network.Network,
    label: str,
    expected_source_counts: dict[str, int],
) -> None:
    source_counts = cgmes_counts(source)
    fresh_counts = cgmes_counts(fresh)
    require(
        source_counts == expected_source_counts,
        f"{label}: unexpected official source counts: {source_counts}",
    )

    # Each source folder joins profiles from several modeling authorities.
    # The PowerIO projection turns boundary and tie equipment into ordinary
    # network rows, so source and fresh topology counts must differ. Compare
    # the mRIDs of equipment shared by both representations instead.
    require(fresh_counts["tie_lines"] == 0, f"{label}: fresh output contains tie lines")
    require(
        fresh_counts["boundary_lines"] == 0,
        f"{label}: fresh output contains boundary lines",
    )
    require(
        fresh_counts["lines"] > source_counts["lines"],
        f"{label}: boundary and tie line projection did not add ordinary lines",
    )

    shared_getters = (
        (source.get_2_windings_transformers, fresh.get_2_windings_transformers, "2W"),
        (source.get_3_windings_transformers, fresh.get_3_windings_transformers, "3W"),
        (source.get_generators, fresh.get_generators, "generator"),
        (source.get_shunt_compensators, fresh.get_shunt_compensators, "shunt"),
        (
            source.get_static_var_compensators,
            fresh.get_static_var_compensators,
            "static var compensator",
        ),
    )
    for source_getter, fresh_getter, equipment in shared_getters:
        require(
            element_ids(source_getter) == element_ids(fresh_getter),
            f"{label}: {equipment} mRIDs changed during fresh emission",
        )
    require(
        element_ids(source.get_lines).issubset(element_ids(fresh.get_lines)),
        f"{label}: an ordinary line mRID is missing from fresh output",
    )
    require(
        element_ids(source.get_loads).issubset(element_ids(fresh.get_loads)),
        f"{label}: a source load mRID is missing from fresh output",
    )
    print(f"{label}: source counts={source_counts}; fresh projection counts={fresh_counts}")


def same_missing_value(source_value: Any, fresh_value: Any) -> bool:
    return bool(pd.isna(source_value)) and bool(pd.isna(fresh_value))


def numeric_values_match(
    source_value: Any,
    fresh_value: Any,
) -> bool:
    if same_missing_value(source_value, fresh_value):
        return True
    if bool(pd.isna(source_value)) or bool(pd.isna(fresh_value)):
        return False
    source_number = float(source_value)
    fresh_number = float(fresh_value)
    return math.isclose(
        source_number,
        fresh_number,
        rel_tol=CGMES_NUMERIC_REL_TOL,
        abs_tol=CGMES_NUMERIC_ABS_TOL,
    )


def cgmes_terminal_topology(
    directory: Path,
    namespace: str,
) -> dict[str, dict[int, tuple[str, str, str]]]:
    partial: dict[str, dict[str, Any]] = {}
    for terminal in xml_elements(directory, namespace, "Terminal"):
        terminal_id = rdf_id(terminal, "ID") or rdf_id(terminal, "about")
        record = partial.setdefault(terminal_id, {})
        fields = (
            ("equipment", "Terminal.ConductingEquipment"),
            ("connectivity_node", "Terminal.ConnectivityNode"),
            ("topological_node", "Terminal.TopologicalNode"),
        )
        for field, property_name in fields:
            value = terminal.find(f"{{{namespace}}}{property_name}")
            if value is not None:
                record[field] = rdf_id(value, "resource")
        sequence = terminal.find(f"{{{namespace}}}ACDCTerminal.sequenceNumber")
        if sequence is not None and sequence.text is not None:
            record["sequence"] = int(sequence.text)

    by_equipment: dict[str, dict[int, tuple[str, str, str]]] = {}
    for terminal_id, record in partial.items():
        if "equipment" not in record or "sequence" not in record:
            continue
        equipment_id = str(record["equipment"])
        sequence = int(record["sequence"])
        equipment = by_equipment.setdefault(equipment_id, {})
        require(
            sequence not in equipment,
            f"duplicate CGMES terminal sequence {equipment_id}/{sequence}",
        )
        equipment[sequence] = (
            terminal_id,
            str(record.get("connectivity_node", "")),
            str(record.get("topological_node", "")),
        )
    return by_equipment


def check_boundary_terminal_topology(
    boundary_ids: set[str],
    source_directory: Path,
    source_namespace: str,
    fresh_directory: Path,
    label: str,
) -> int:
    source = cgmes_terminal_topology(source_directory, source_namespace)
    fresh = cgmes_terminal_topology(fresh_directory, CIM100)
    synthesized_connectivity_nodes: dict[str, str] = {}
    for equipment_id in sorted(boundary_ids):
        require(
            set(source.get(equipment_id, {})) == {1, 2},
            f"{label}: official boundary line {equipment_id} terminal sides changed",
        )
        require(
            set(fresh.get(equipment_id, {})) == {1, 2},
            f"{label}: fresh boundary line {equipment_id} terminal sides changed",
        )
        for side in (1, 2):
            _, source_cn, source_tn = source[equipment_id][side]
            _, fresh_cn, fresh_tn = fresh[equipment_id][side]
            require(
                fresh_tn == source_tn,
                f"{label}: boundary line {equipment_id} side {side} changed "
                "TopologicalNode",
            )
            if source_cn:
                require(
                    fresh_cn == source_cn,
                    f"{label}: boundary line {equipment_id} side {side} changed "
                    "ConnectivityNode",
                )
            else:
                require(
                    bool(fresh_cn),
                    f"{label}: boundary line {equipment_id} side {side} has no "
                    "projected ConnectivityNode",
                )
                existing = synthesized_connectivity_nodes.setdefault(source_tn, fresh_cn)
                require(
                    fresh_cn == existing,
                    f"{label}: TopologicalNode {source_tn} projects to several "
                    "ConnectivityNodes",
                )
    comparisons = len(boundary_ids) * 2 * 3
    print(
        f"{label}: boundary terminal equipment/side, ConnectivityNode, and "
        f"TopologicalNode comparisons={comparisons}"
    )
    return comparisons


def sv_power_flows_by_terminal(
    directory: Path,
    namespace: str,
    label: str,
) -> dict[str, tuple[str, str]]:
    flows: dict[str, tuple[str, str]] = {}
    for flow in xml_elements(directory, namespace, "SvPowerFlow"):
        terminal = flow.find(f"{{{namespace}}}SvPowerFlow.Terminal")
        p = flow.find(f"{{{namespace}}}SvPowerFlow.p")
        q = flow.find(f"{{{namespace}}}SvPowerFlow.q")
        require(terminal is not None, f"{label}: SvPowerFlow has no terminal")
        require(p is not None and p.text is not None, f"{label}: SvPowerFlow has no p")
        require(q is not None and q.text is not None, f"{label}: SvPowerFlow has no q")
        terminal_id = rdf_id(terminal, "resource")
        require(terminal_id not in flows, f"{label}: duplicate SvPowerFlow terminal")
        flows[terminal_id] = (p.text, q.text)
    return flows


def check_boundary_sv_projection(
    label: str,
    source_lines: pd.DataFrame,
    source_directory: Path,
    source_namespace: str,
    fresh_directory: Path,
    ir_path: Path,
) -> int:
    no_direct_sv = CGMES_BOUNDARY_LINE_NO_DIRECT_SV[label]
    fresh_topology = cgmes_terminal_topology(fresh_directory, CIM100)
    fresh_flows = sv_power_flows_by_terminal(
        fresh_directory,
        CIM100,
        f"fresh {label}",
    )
    direct_terminals = {
        terminal_id
        for equipment_id in no_direct_sv
        for terminal_id, _, _ in fresh_topology[equipment_id].values()
    }
    require(
        direct_terminals.isdisjoint(fresh_flows),
        f"{label}: fresh SV contains power flow on a projected boundary line terminal",
    )

    comparisons = len(direct_terminals)
    if label == "CGMES 3.0":
        expected = {
            sv_id: (terminal_id, equipment_id, p, q)
            for sv_id, terminal_id, equipment_id, p, q in CGMES_30_BOUNDARY_SV_OMISSIONS
        }
        source_terminals = cgmes_terminal_topology(source_directory, source_namespace)
        terminal_equipment = {
            terminal_id: equipment_id
            for equipment_id, terminals in source_terminals.items()
            for terminal_id, _, _ in terminals.values()
        }
        source_records: dict[str, tuple[str, str, str, str]] = {}
        for flow in xml_elements(source_directory, source_namespace, "SvPowerFlow"):
            sv_id = rdf_id(flow, "ID") or rdf_id(flow, "about")
            if sv_id not in expected:
                continue
            terminal = flow.find(f"{{{source_namespace}}}SvPowerFlow.Terminal")
            p = flow.find(f"{{{source_namespace}}}SvPowerFlow.p")
            q = flow.find(f"{{{source_namespace}}}SvPowerFlow.q")
            require(terminal is not None, f"CGMES 3.0: SvPowerFlow {sv_id} has no terminal")
            require(p is not None and p.text is not None, f"CGMES 3.0: {sv_id} has no p")
            require(q is not None and q.text is not None, f"CGMES 3.0: {sv_id} has no q")
            terminal_id = rdf_id(terminal, "resource")
            source_records[sv_id] = (
                terminal_id,
                terminal_equipment.get(terminal_id, ""),
                p.text,
                q.text,
            )
        require(
            set(source_records) == set(expected),
            "CGMES 3.0: official cross-authority SvPowerFlow identities changed",
        )
        diagnostics = json.loads(ir_path.read_text())["diagnostics"]
        for sv_id, (terminal_id, equipment_id, p, q) in expected.items():
            source_record = source_records[sv_id]
            require(
                source_record[:2] == (terminal_id, equipment_id)
                and numeric_values_match(source_record[2], p)
                and numeric_values_match(source_record[3], q),
                f"CGMES 3.0: official SvPowerFlow {sv_id} changed: {source_record}",
            )
            expected_message = (
                f"SvPowerFlow `{sv_id}` for terminal `{terminal_id}` belongs to "
                "modelingAuthoritySet `http://elia.be/CGMES`, while conducting "
                f"equipment `{equipment_id}` belongs to `http://tennet.nl/CGMES`; "
                f"its p=Some({p}) MW and q=Some({q}) MVAr observations were not "
                "mapped to the other authority's equipment results"
            )
            matches = [
                diagnostic
                for diagnostic in diagnostics
                if diagnostic["code"] == "READ.CGMES.RECORD_UNMAPPED"
                and diagnostic["message"] == expected_message
            ]
            require(
                len(matches) == 1 and matches[0]["severity"] == "warning",
                f"CGMES 3.0: missing cross-authority SvPowerFlow diagnostic {sv_id}",
            )
        comparisons += len(expected) * 7

        print(
            "CGMES 3.0: exact cross-authority SvPowerFlow diagnostics="
            f"{len(expected)}, omitted p/q quantities={len(expected) * 2}, "
            f"comparisons={comparisons}"
        )
        return comparisons

    require(
        set(map(str, source_lines.index)) == no_direct_sv,
        "CGMES 2.4.15: projected boundary line identities changed",
    )
    source_topology = cgmes_terminal_topology(source_directory, source_namespace)
    source_flows = sv_power_flows_by_terminal(
        source_directory,
        source_namespace,
        "official CGMES 2.4.15",
    )
    relocated: list[tuple[str, str, str]] = []
    for line_id, row in source_lines.iterrows():
        injection_id = str(row["CGMES.EquivalentInjection"])
        require(
            set(source_topology.get(injection_id, {})) == {1},
            f"CGMES 2.4.15: official injection {injection_id} has no unique terminal",
        )
        source_terminal_id = source_topology[injection_id][1][0]
        require(
            source_terminal_id in source_flows,
            f"CGMES 2.4.15: official injection {injection_id} has no SvPowerFlow",
        )
        require(
            set(fresh_topology.get(injection_id, {})) == {1},
            f"CGMES 2.4.15: projected injection {injection_id} has no unique terminal",
        )
        terminal_id = fresh_topology[injection_id][1][0]
        require(
            terminal_id in fresh_flows,
            f"CGMES 2.4.15: projected injection {injection_id} has no fresh SvPowerFlow",
        )
        p, q = fresh_flows[terminal_id]
        source_p, source_q = source_flows[source_terminal_id]
        require(
            numeric_values_match(source_p, p),
            f"CGMES 2.4.15: projected injection {injection_id} changed SvPowerFlow.p",
        )
        require(
            numeric_values_match(source_q, q),
            f"CGMES 2.4.15: projected injection {injection_id} changed SvPowerFlow.q",
        )
        relocated.append((str(line_id), injection_id, terminal_id))
    comparisons += len(relocated) * 6

    print(
        "CGMES 2.4.15: boundary SvPowerFlow moved from line terminals to exact "
        f"projected EquivalentInjection load terminals={len(relocated)}, "
        f"comparisons={comparisons}"
    )
    return comparisons


def check_projected_boundary_lines(
    source: pp.network.Network,
    fresh: pp.network.Network,
    label: str,
    source_directory: Path,
    source_namespace: str,
    fresh_directory: Path,
    ir_path: Path,
) -> int:
    source_lines = source.get_boundary_lines(all_attributes=True)
    fresh_lines = fresh.get_lines(all_attributes=True)
    fresh_buses = fresh.get_buses()
    fresh_loads = fresh.get_loads(all_attributes=True)
    source_ids = set(map(str, source_lines.index))
    require(
        source_ids.issubset(set(map(str, fresh_lines.index))),
        f"{label}: projected boundary line mRIDs are missing from fresh lines",
    )

    source_ties = source.get_tie_lines(all_attributes=True)
    paired_ids: set[str] = set()
    tie_comparisons = 0
    for tie_id, tie in source_ties.iterrows():
        first_id = str(tie["boundary_line1_id"])
        second_id = str(tie["boundary_line2_id"])
        require(
            first_id in source_ids and second_id in source_ids,
            f"{label}: tie line {tie_id} references an unknown boundary line",
        )
        require(
            str(tie["pairing_key"])
            == str(source_lines.at[first_id, "pairing_key"])
            == str(source_lines.at[second_id, "pairing_key"]),
            f"{label}: tie line {tie_id} pairing key is inconsistent",
        )
        require(
            bool(tie["connected1"]) == bool(source_lines.at[first_id, "connected"])
            and bool(tie["connected2"])
            == bool(source_lines.at[second_id, "connected"]),
            f"{label}: tie line {tie_id} connection flags disagree with its ends",
        )
        paired_ids.update((first_id, second_id))
        tie_comparisons += 6
    require(
        {str(line_id) for line_id, row in source_lines.iterrows() if bool(row["paired"])}
        == paired_ids,
        f"{label}: paired boundary flags disagree with tie line membership",
    )
    projected_injection_ids = {
        str(injection_id) for injection_id in source_lines["CGMES.EquivalentInjection"]
    }
    source_load_ids = set(map(str, source.get_loads().index))
    require(
        set(map(str, fresh_loads.index)) - source_load_ids == projected_injection_ids,
        f"{label}: projected boundary injection load mRIDs changed",
    )

    no_direct_sv = CGMES_BOUNDARY_LINE_NO_DIRECT_SV[label]
    require(
        no_direct_sv <= source_ids,
        f"{label}: boundary equipment without direct fresh SV changed",
    )
    mismatches: list[str] = []
    for raw_line_id, source_line in source_lines.iterrows():
        line_id = str(raw_line_id)
        fresh_line = fresh_lines.loc[line_id]
        internal_sides = [
            side
            for side in (1, 2)
            if fresh_line[f"voltage_level{side}_id"]
            == source_line["voltage_level_id"]
        ]
        require(
            len(internal_sides) == 1,
            f"{label}: projected boundary line {line_id} has no unique internal side",
        )
        internal_side = internal_sides[0]
        boundary_side = 3 - internal_side
        boundary_bus_id = str(fresh_line[f"bus{boundary_side}_id"])
        require(
            boundary_bus_id in fresh_buses.index,
            f"{label}: projected boundary line {line_id} has no calculation bus",
        )
        injection_id = str(source_line["CGMES.EquivalentInjection"])
        injection = fresh_loads.loc[injection_id]
        omit_solution = line_id in no_direct_sv

        numeric_values: tuple[tuple[str, Any, Any], ...] = (
            ("r", source_line["r"], fresh_line["r"]),
            ("x", source_line["x"], fresh_line["x"]),
            ("g", source_line["g"], fresh_line["g1"] + fresh_line["g2"]),
            ("b", source_line["b"], fresh_line["b1"] + fresh_line["b2"]),
            (
                "p",
                math.nan if omit_solution else source_line["p"],
                fresh_line[f"p{internal_side}"],
            ),
            (
                "q",
                math.nan if omit_solution else source_line["q"],
                fresh_line[f"q{internal_side}"],
            ),
        )
        for column, source_value, fresh_value in numeric_values:
            if not numeric_values_match(source_value, fresh_value):
                mismatches.append(
                    f"{line_id}.{column}: source={source_value!r}, "
                    f"fresh={fresh_value!r}"
                )

        exact_values = (
            ("name", source_line["name"], fresh_line["name"]),
            (
                "voltage_level_id",
                source_line["voltage_level_id"],
                fresh_line[f"voltage_level{internal_side}_id"],
            ),
            ("bus_id", source_line["bus_id"], fresh_line[f"bus{internal_side}_id"]),
            (
                "connected",
                bool(source_line["connected"]),
                bool(fresh_line[f"connected{internal_side}"]),
            ),
            ("fictitious", bool(source_line["fictitious"]), bool(fresh_line["fictitious"])),
        )
        for column, source_value, fresh_value in exact_values:
            if source_value != fresh_value:
                mismatches.append(
                    f"{line_id}.{column}: source={source_value!r}, "
                    f"fresh={fresh_value!r}"
                )

        # p0/q0 are the EquivalentInjection assignment, not line terminal powers.
        boundary_solution_values = (
            (
                "p",
                math.nan if omit_solution else -source_line["p0"],
                fresh_line[f"p{boundary_side}"],
            ),
            (
                "q",
                math.nan if omit_solution else -source_line["q0"],
                fresh_line[f"q{boundary_side}"],
            ),
        )
        for column, source_value, fresh_value in boundary_solution_values:
            if not numeric_values_match(source_value, fresh_value):
                mismatches.append(
                    f"{line_id}.boundary_{column}: source={source_value!r}, "
                    f"fresh={fresh_value!r}"
                )

        injection_numeric_values: tuple[tuple[str, Any, Any], ...] = (
            ("p0", source_line["p0"], injection["p0"]),
            ("q0", source_line["q0"], injection["q0"]),
        )
        if not omit_solution:
            injection_numeric_values += (
                ("p", source_line["p0"], injection["p"]),
                ("q", source_line["q0"], injection["q"]),
            )
        for column, source_value, fresh_value in injection_numeric_values:
            if not numeric_values_match(source_value, fresh_value):
                mismatches.append(
                    f"{injection_id}.{column}: source boundary="
                    f"{source_value!r}, fresh={fresh_value!r}"
                )
        injection_exact_values = (
            ("voltage_level_id", fresh_line[f"voltage_level{boundary_side}_id"]),
            ("bus_id", boundary_bus_id),
            ("connected", True),
        )
        for column, expected in injection_exact_values:
            if injection[column] != expected:
                mismatches.append(
                    f"{injection_id}.{column}: expected={expected!r}, "
                    f"fresh={injection[column]!r}"
                )

    require(
        not mismatches,
        f"{label}: projected boundary lines have {len(mismatches)} changed values: "
        f"{mismatches[:12]}",
    )
    topology_comparisons = check_boundary_terminal_topology(
        source_ids,
        source_directory,
        source_namespace,
        fresh_directory,
        label,
    )
    sv_comparisons = check_boundary_sv_projection(
        label,
        source_lines,
        source_directory,
        source_namespace,
        fresh_directory,
        ir_path,
    )
    retained_solution_rows = len(source_lines) - len(no_direct_sv)
    line_comparisons = retained_solution_rows * 21 + len(no_direct_sv) * 19
    comparison_count = (
        line_comparisons
        + tie_comparisons
        + topology_comparisons
        + sv_comparisons
    )
    print(
        f"{label}: projected boundary lines={len(source_lines)}, "
        f"projected injection loads={len(projected_injection_ids)}, "
        f"tie lines={len(source_ties)}, comparisons={comparison_count}; "
        f"line terminals without direct fresh SV={len(no_direct_sv)}; "
        "retained direct boundary p/q use the opposite injection sign"
    )
    return comparison_count


def check_electrical_frame(
    source_frame: pd.DataFrame,
    fresh_frame: pd.DataFrame,
    equipment: str,
    label: str,
    field_sets: dict[str, dict[str, tuple[str, ...]]] = CGMES_ELECTRICAL_FIELDS,
    exact_ids: bool = False,
    excluded_cells: set[tuple[str, str]] | None = None,
) -> tuple[int, int]:
    field_set = field_sets[equipment]
    numeric_columns = field_set["numeric"]
    exact_columns = field_set["exact"]
    columns = numeric_columns + exact_columns
    excluded_cells = excluded_cells or set()
    require(
        all(equipment_id in source_frame.index and column in columns for equipment_id, column in excluded_cells),
        f"{label}: invalid excluded {equipment} cells {sorted(excluded_cells)}",
    )
    for column in columns:
        require(column in source_frame.columns, f"{label}: source {equipment} has no {column}")
        require(column in fresh_frame.columns, f"{label}: fresh {equipment} has no {column}")

    source_ids = set(source_frame.index)
    fresh_ids = set(fresh_frame.index)
    if exact_ids:
        require(
            source_ids == fresh_ids,
            f"{label}: {equipment} identities changed; "
            f"missing={sorted(source_ids - fresh_ids)[:5]}, "
            f"extra={sorted(fresh_ids - source_ids)[:5]}",
        )
    else:
        require(
            source_ids.issubset(fresh_ids),
            f"{label}: a source {equipment} mRID is missing from fresh output",
        )
    mismatches: list[str] = []
    for equipment_id in sorted(source_ids):
        for column in numeric_columns:
            if (str(equipment_id), column) in excluded_cells:
                continue
            source_value = source_frame.at[equipment_id, column]
            fresh_value = fresh_frame.at[equipment_id, column]
            if same_missing_value(source_value, fresh_value):
                continue
            if bool(pd.isna(source_value)) or bool(pd.isna(fresh_value)):
                mismatches.append(
                    f"{equipment_id}.{column}: source={source_value!r}, fresh={fresh_value!r}"
                )
                continue
            if not math.isclose(
                float(source_value),
                float(fresh_value),
                rel_tol=CGMES_NUMERIC_REL_TOL,
                abs_tol=CGMES_NUMERIC_ABS_TOL,
            ):
                mismatches.append(
                    f"{equipment_id}.{column}: source={source_value!r}, fresh={fresh_value!r}"
                )
        for column in exact_columns:
            if (str(equipment_id), column) in excluded_cells:
                continue
            source_value = source_frame.at[equipment_id, column]
            fresh_value = fresh_frame.at[equipment_id, column]
            if same_missing_value(source_value, fresh_value):
                continue
            if source_value != fresh_value:
                mismatches.append(
                    f"{equipment_id}.{column}: source={source_value!r}, fresh={fresh_value!r}"
                )
    require(
        not mismatches,
        f"{label}: {equipment} has {len(mismatches)} changed electrical values: "
        f"{mismatches[:8]}",
    )

    comparison_count = len(source_ids) * len(columns) - len(excluded_cells)
    print(
        f"{label}: {equipment} mRIDs={len(source_ids)}, comparisons={comparison_count}, "
        f"numeric={numeric_columns}, exact={exact_columns}"
    )
    return len(source_ids), comparison_count


def check_switch_frame(
    source_frame: pd.DataFrame,
    fresh_frame: pd.DataFrame,
    expected_count: int,
    label: str,
) -> int:
    columns = (
        "name",
        "kind",
        "open",
        "retained",
        "voltage_level_id",
        "bus_breaker_bus1_id",
        "bus_breaker_bus2_id",
        "node1",
        "node2",
        "fictitious",
    )
    require(
        len(source_frame) == expected_count,
        f"{label}: official source has {len(source_frame)} switches, "
        f"expected {expected_count}",
    )
    for column in columns:
        require(column in source_frame.columns, f"{label}: source switch has no {column}")
        require(column in fresh_frame.columns, f"{label}: fresh switch has no {column}")

    source_ids = set(source_frame.index)
    fresh_ids = set(fresh_frame.index)
    require(
        source_ids == fresh_ids,
        f"{label}: switch identities changed; "
        f"missing={sorted(source_ids - fresh_ids)[:5]}, "
        f"extra={sorted(fresh_ids - source_ids)[:5]}",
    )
    mismatches: list[str] = []
    for switch_id in sorted(source_ids):
        for column in columns:
            source_value = source_frame.at[switch_id, column]
            fresh_value = fresh_frame.at[switch_id, column]
            if same_missing_value(source_value, fresh_value):
                continue
            if source_value != fresh_value:
                mismatches.append(
                    f"{switch_id}.{column}: source={source_value!r}, "
                    f"fresh={fresh_value!r}"
                )
    require(
        not mismatches,
        f"{label}: switches have {len(mismatches)} changed values: {mismatches[:8]}",
    )
    comparison_count = len(source_ids) * len(columns)
    print(
        f"{label}: switch identities={len(source_ids)}, "
        f"comparisons={comparison_count}, fields={columns}"
    )
    return comparison_count


def terminal_bus_assignments(
    network: pp.network.Network,
    label: str,
) -> dict[tuple[str, str, str], str]:
    assignments: dict[tuple[str, str, str], str] = {}
    for element_id, row in network.get_terminals().iterrows():
        key = (
            str(element_id),
            str(row["element_side"]),
            str(row["voltage_level_id"]),
        )
        require(key not in assignments, f"{label}: duplicate terminal identity {key}")
        assignments[key] = str(row["bus_id"])
    return assignments


def mapped_solution_buses(
    source: pp.network.Network,
    fresh: pp.network.Network,
    label: str,
) -> tuple[pd.DataFrame, pd.DataFrame, set[str], set[str]]:
    source_buses = source.get_buses()
    fresh_buses = fresh.get_buses()
    source_assignments = terminal_bus_assignments(source, f"official {label}")
    fresh_assignments = terminal_bus_assignments(fresh, f"fresh {label}")
    shared_terminals = set(source_assignments) & set(fresh_assignments)

    def signatures(
        assignments: dict[tuple[str, str, str], str],
    ) -> dict[str, set[tuple[str, str, str]]]:
        result: dict[str, set[tuple[str, str, str]]] = {}
        for terminal in shared_terminals:
            bus_id = assignments[terminal]
            result.setdefault(bus_id, set()).add(terminal)
        return result

    source_signatures = signatures(source_assignments)
    fresh_signatures = signatures(fresh_assignments)
    mapped: dict[str, str] = {}
    unavailable: set[str] = set()
    for source_bus_id in source_buses.index:
        signature = source_signatures.get(str(source_bus_id), set())
        candidates = [
            fresh_bus_id
            for fresh_bus_id, fresh_signature in fresh_signatures.items()
            if signature and fresh_signature == signature
        ]
        if len(candidates) == 1:
            mapped[str(source_bus_id)] = candidates[0]
        else:
            unavailable.add(str(source_bus_id))
    require(
        unavailable == CGMES_UNAVAILABLE_SOLUTION_BUSES[label],
        f"{label}: unexpected buses without a shared-terminal solution mapping: "
        f"{sorted(unavailable)}",
    )
    require(
        len(set(mapped.values())) == len(mapped),
        f"{label}: more than one official bus maps to the same fresh bus",
    )
    unmatched_fresh = set(map(str, fresh_buses.index)) - set(mapped.values())
    unmatched_non_boundary = {
        bus_id
        for bus_id in unmatched_fresh
        if fresh_signatures.get(bus_id, set())
    }
    require(
        not unmatched_non_boundary,
        f"{label}: fresh non-boundary buses lack an official solution mapping: "
        f"{sorted(unmatched_non_boundary)}",
    )

    source_ids = sorted(mapped)
    source_mapped = source_buses.loc[source_ids].copy()
    fresh_mapped = fresh_buses.loc[[mapped[source_id] for source_id in source_ids]].copy()
    fresh_mapped.index = source_ids
    fresh_mapped.index.name = source_mapped.index.name
    return source_mapped, fresh_mapped, unavailable, unmatched_fresh


def check_cgmes_solution_values(
    source: pp.network.Network,
    fresh: pp.network.Network,
    label: str,
) -> int:
    source_buses, fresh_buses, unavailable, unmatched_fresh = mapped_solution_buses(
        source,
        fresh,
        label,
    )
    _, comparisons = check_electrical_frame(
        source_buses,
        fresh_buses,
        "bus solution",
        label,
        CGMES_SOLUTION_FIELDS,
        exact_ids=True,
    )
    frames = (
        ("ordinary line solution", source.get_lines, fresh.get_lines),
        (
            "2W transformer solution",
            source.get_2_windings_transformers,
            fresh.get_2_windings_transformers,
        ),
        (
            "3W transformer solution",
            source.get_3_windings_transformers,
            fresh.get_3_windings_transformers,
        ),
        ("generator solution", source.get_generators, fresh.get_generators),
        ("load solution", source.get_loads, fresh.get_loads),
        ("shunt solution", source.get_shunt_compensators, fresh.get_shunt_compensators),
        (
            "static var compensator solution",
            source.get_static_var_compensators,
            fresh.get_static_var_compensators,
        ),
        (
            "VSC converter solution",
            source.get_vsc_converter_stations,
            fresh.get_vsc_converter_stations,
        ),
    )
    for equipment, source_getter, fresh_getter in frames:
        _, compared = check_electrical_frame(
            source_getter(all_attributes=True),
            fresh_getter(all_attributes=True),
            equipment,
            label,
            CGMES_SOLUTION_FIELDS,
        )
        comparisons += compared
    print(
        f"{label}: SV comparisons={comparisons}, "
        f"unavailable source boundary buses={sorted(unavailable)}, "
        f"fresh boundary projection buses={len(unmatched_fresh)}"
    )
    if label == "CGMES 2.4.15":
        print(
            f"{label}: exact source buses unavailable after PowSybl disconnects "
            f"two transformer side TWO terminals={sorted(unavailable)}"
        )
    return comparisons


def tap_changer_rows(frame: pd.DataFrame, label: str) -> dict[tuple[str, str], pd.Series]:
    rows: dict[tuple[str, str], pd.Series] = {}
    for transformer_id, row in frame.iterrows():
        side = "" if bool(pd.isna(row["side"])) else str(row["side"])
        key = (str(transformer_id), side)
        require(key not in rows, f"{label}: duplicate tap changer {key}")
        rows[key] = row
    return rows


def tap_step_rows(
    frame: pd.DataFrame,
    label: str,
) -> dict[tuple[str, str, int], pd.Series]:
    rows: dict[tuple[str, str, int], pd.Series] = {}
    for (transformer_id, position), row in frame.iterrows():
        side = "" if bool(pd.isna(row["side"])) else str(row["side"])
        key = (str(transformer_id), side, int(position))
        require(key not in rows, f"{label}: duplicate tap step {key}")
        rows[key] = row
    return rows


def check_tap_rows(
    source_rows: dict[tuple[Any, ...], pd.Series],
    fresh_rows: dict[tuple[Any, ...], pd.Series],
    tap_kind: str,
    expected_rows: int,
    label: str,
    field_sets: dict[str, dict[str, tuple[str, ...]]] = CGMES_TAP_FIELDS,
) -> int:
    require(
        len(source_rows) == expected_rows,
        f"{label}: official source has {len(source_rows)} {tap_kind} rows, "
        f"expected {expected_rows}",
    )
    require(
        set(fresh_rows) == set(source_rows),
        f"{label}: {tap_kind} transformer, side, or position identities changed; "
        f"missing={sorted(set(source_rows) - set(fresh_rows))[:5]}, "
        f"extra={sorted(set(fresh_rows) - set(source_rows))[:5]}",
    )

    field_set = field_sets[tap_kind]
    numeric_columns = field_set["numeric"]
    exact_columns = field_set["exact"]
    source_columns = next(iter(source_rows.values())).index
    fresh_columns = next(iter(fresh_rows.values())).index
    for column in numeric_columns + exact_columns:
        require(column in source_columns, f"{label}: source {tap_kind} has no {column}")
        require(column in fresh_columns, f"{label}: fresh {tap_kind} has no {column}")
    mismatches: list[str] = []
    for row_id in sorted(source_rows):
        source_row = source_rows[row_id]
        fresh_row = fresh_rows[row_id]
        for column in numeric_columns:
            source_value = source_row[column]
            fresh_value = fresh_row[column]
            if same_missing_value(source_value, fresh_value):
                continue
            if bool(pd.isna(source_value)) or bool(pd.isna(fresh_value)):
                mismatches.append(
                    f"{row_id}.{column}: source={source_value!r}, fresh={fresh_value!r}"
                )
                continue
            if not math.isclose(
                float(source_value),
                float(fresh_value),
                rel_tol=CGMES_NUMERIC_REL_TOL,
                abs_tol=CGMES_NUMERIC_ABS_TOL,
            ):
                mismatches.append(
                    f"{row_id}.{column}: source={source_value!r}, fresh={fresh_value!r}"
                )
        for column in exact_columns:
            source_value = source_row[column]
            fresh_value = fresh_row[column]
            if same_missing_value(source_value, fresh_value):
                continue
            if source_value != fresh_value:
                mismatches.append(
                    f"{row_id}.{column}: source={source_value!r}, fresh={fresh_value!r}"
                )
    require(
        not mismatches,
        f"{label}: {tap_kind} has {len(mismatches)} changed values: {mismatches[:8]}",
    )

    comparison_count = len(source_rows) * (len(numeric_columns) + len(exact_columns))
    print(
        f"{label}: {tap_kind} rows={len(source_rows)}, comparisons={comparison_count}, "
        f"numeric={numeric_columns}, exact={exact_columns}"
    )
    return comparison_count


def check_cgmes_tap_changers(
    source: pp.network.Network,
    fresh: pp.network.Network,
    label: str,
) -> int:
    expected = CGMES_TAP_EXPECTATIONS[label]
    comparisons = 0
    comparisons += check_tap_rows(
        tap_changer_rows(source.get_ratio_tap_changers(all_attributes=True), label),
        tap_changer_rows(fresh.get_ratio_tap_changers(all_attributes=True), label),
        "ratio changer",
        expected["ratio changers"],
        label,
    )
    comparisons += check_tap_rows(
        tap_step_rows(source.get_ratio_tap_changer_steps(all_attributes=True), label),
        tap_step_rows(fresh.get_ratio_tap_changer_steps(all_attributes=True), label),
        "ratio step",
        expected["ratio steps"],
        label,
    )
    comparisons += check_tap_rows(
        tap_changer_rows(source.get_phase_tap_changers(all_attributes=True), label),
        tap_changer_rows(fresh.get_phase_tap_changers(all_attributes=True), label),
        "phase changer",
        expected["phase changers"],
        label,
    )
    comparisons += check_tap_rows(
        tap_step_rows(source.get_phase_tap_changer_steps(all_attributes=True), label),
        tap_step_rows(fresh.get_phase_tap_changer_steps(all_attributes=True), label),
        "phase step",
        expected["phase steps"],
        label,
    )
    print(f"{label}: tap changer comparisons={comparisons}")
    return comparisons


def check_operational_limits(
    source: pp.network.Network,
    fresh: pp.network.Network,
    label: str,
) -> int:
    source_branch_ids = set(source.get_lines().index)
    source_branch_ids.update(source.get_2_windings_transformers().index)
    source_branch_ids.update(source.get_3_windings_transformers().index)

    source_limits = source.get_operational_limits(all_attributes=True)
    source_limits = source_limits[
        source_limits.index.get_level_values("element_id").isin(source_branch_ids)
    ]
    fresh_limits = fresh.get_operational_limits(all_attributes=True)
    fresh_limits = fresh_limits[
        fresh_limits.index.get_level_values("element_id").isin(source_branch_ids)
    ]
    expected_source_rows, expected_extra_rows = CGMES_LIMIT_EXPECTATIONS[label]
    require(
        len(source_limits) == expected_source_rows,
        f"{label}: official source has {len(source_limits)} shared operational limits, "
        f"expected {expected_source_rows}",
    )
    missing = set(source_limits.index) - set(fresh_limits.index)
    extra = set(fresh_limits.index) - set(source_limits.index)
    require(not missing, f"{label}: fresh output lost operational limits: {sorted(missing)[:5]}")
    require(
        len(extra) == expected_extra_rows,
        f"{label}: fresh output has {len(extra)} additional shared operational limits, "
        f"expected {expected_extra_rows}",
    )

    numeric_columns = ("value",)
    exact_columns = ("element_type", "fictitious", "selected")
    mismatches: list[str] = []
    for limit_id in source_limits.index:
        for column in numeric_columns:
            source_value = source_limits.at[limit_id, column]
            fresh_value = fresh_limits.at[limit_id, column]
            if not math.isclose(
                float(source_value),
                float(fresh_value),
                rel_tol=CGMES_NUMERIC_REL_TOL,
                abs_tol=CGMES_NUMERIC_ABS_TOL,
            ):
                mismatches.append(
                    f"{limit_id}.{column}: source={source_value!r}, fresh={fresh_value!r}"
                )
        for column in exact_columns:
            source_value = source_limits.at[limit_id, column]
            fresh_value = fresh_limits.at[limit_id, column]
            if source_value != fresh_value:
                mismatches.append(
                    f"{limit_id}.{column}: source={source_value!r}, fresh={fresh_value!r}"
                )
    require(
        not mismatches,
        f"{label}: {len(mismatches)} operational limit values changed: "
        f"{mismatches[:8]}",
    )

    # The limit name is a display label. The CIM16 to CIM100 projection adds
    # an equipment prefix while retaining the group mRID in the row key.
    row_key = tuple(source_limits.index.names)
    comparison_count = len(source_limits) * (len(row_key) + 4)
    print(
        f"{label}: official operational limit rows={len(source_limits)}, "
        f"additional fresh rows={len(extra)}, comparisons={comparison_count}, "
        f"row key={row_key}, numeric={numeric_columns}, exact={exact_columns}"
    )
    return comparison_count


def check_cgmes_switches(
    source: pp.network.Network,
    fresh: pp.network.Network,
    label: str,
) -> int:
    source_switches = source.get_switches(all_attributes=True)
    fresh_switches = fresh.get_switches(all_attributes=True)
    source_physical = source_switches[~source_switches["fictitious"]]
    fresh_physical = fresh_switches[~fresh_switches["fictitious"]]
    require(
        set(source_physical.index) == set(fresh_physical.index),
        f"{label}: switch identities changed; "
        f"missing={sorted(set(source_physical.index) - set(fresh_physical.index))[:5]}, "
        f"extra={sorted(set(fresh_physical.index) - set(source_physical.index))[:5]}",
    )

    exact_columns = (
        "name",
        "kind",
        "open",
        "retained",
        "voltage_level_id",
        "fictitious",
        "CGMES.originalClass",
        "CGMES.normalOpen",
    )
    mismatches: list[str] = []
    available_columns = tuple(
        column
        for column in exact_columns
        if column in source_physical.columns and column in fresh_physical.columns
    )
    for switch_id in sorted(source_physical.index):
        for column in available_columns:
            source_value = source_physical.at[switch_id, column]
            fresh_value = fresh_physical.at[switch_id, column]
            if same_missing_value(source_value, fresh_value):
                continue
            if source_value != fresh_value:
                mismatches.append(
                    f"{switch_id}.{column}: source={source_value!r}, fresh={fresh_value!r}"
                )
    require(
        not mismatches,
        f"{label}: {len(mismatches)} switch values changed: {mismatches[:8]}",
    )

    # PowSybl synthesizes switches for disconnected terminals. Their IDs and
    # node numbers are derived during import, so compare their electrical role
    # rather than treating those generated values as source identities.
    synthetic_columns = ("kind", "open", "retained", "voltage_level_id", "fictitious")

    def synthetic_rows(frame: pd.DataFrame) -> Counter[tuple[Any, ...]]:
        synthetic = frame[frame["fictitious"]]
        return Counter(
            tuple(row[column] for column in synthetic_columns)
            for _, row in synthetic.iterrows()
        )

    require(
        synthetic_rows(source_switches) == synthetic_rows(fresh_switches),
        f"{label}: generated disconnected terminal switches changed electrical role",
    )
    comparison_count = len(source_physical) * len(available_columns)
    comparison_count += len(source_switches[source_switches["fictitious"]]) * len(
        synthetic_columns
    )
    print(
        f"{label}: physical switches={len(source_physical)}, "
        f"generated disconnected terminal switches="
        f"{len(source_switches[source_switches['fictitious']])}, "
        f"comparisons={comparison_count}, exact={available_columns}"
    )
    return comparison_count


def cgmes_operational_limit_kinds(
    path: Path,
    namespace: str,
    label: str,
) -> Counter[str]:
    kinds: Counter[str] = Counter()
    seen: set[str] = set()
    for root in cgmes_xml_roots(path):
        for element in root.iter(f"{{{namespace}}}OperationalLimitType"):
            type_id = rdf_id(element, "ID") or rdf_id(element, "about")
            require(type_id, f"{label}: OperationalLimitType has no RDF identity")
            require(
                type_id not in seen,
                f"{label}: duplicate OperationalLimitType identity {type_id}",
            )
            seen.add(type_id)
            kind = ""
            for child in element:
                local_name = child.tag.rsplit("}", 1)[-1]
                if local_name not in (
                    "OperationalLimitType.kind",
                    "OperationalLimitType.limitType",
                ):
                    continue
                kind = (rdf_id(child, "resource") or (child.text or "")).rsplit(
                    ".", 1
                )[-1]
            require(kind, f"{label}: OperationalLimitType {type_id} has no limit kind")
            kinds[kind] += 1
    return kinds


def check_boundary_operational_limits(
    source: pp.network.Network,
    label: str,
    source_directory: Path,
    source_namespace: str,
    fresh_directory: Path,
    ir_path: Path,
) -> int:
    boundary_ids = set(map(str, source.get_boundary_lines().index))
    source_kinds = cgmes_operational_limit_kinds(
        source_directory, source_namespace, f"official {label}"
    )
    fresh_kinds = cgmes_operational_limit_kinds(
        fresh_directory, CIM100, f"fresh {label}"
    )
    expected_source_kinds, expected_fresh_kinds = CGMES_LIMIT_KIND_EXPECTATIONS[label]
    require(
        source_kinds == expected_source_kinds,
        f"{label}: official raw OperationalLimitType kinds changed: {source_kinds}",
    )
    require(
        fresh_kinds == expected_fresh_kinds,
        f"{label}: fresh raw OperationalLimitType kinds changed: {fresh_kinds}",
    )

    def terminal_owners(
        directory: Path,
        namespace: str,
    ) -> dict[str, tuple[str, int]]:
        partial: dict[str, dict[str, Any]] = {}
        for terminal in xml_elements(directory, namespace, "Terminal"):
            terminal_id = rdf_id(terminal, "ID") or rdf_id(terminal, "about")
            record = partial.setdefault(terminal_id, {})
            equipment = terminal.find(f"{{{namespace}}}Terminal.ConductingEquipment")
            sequence = terminal.find(f"{{{namespace}}}ACDCTerminal.sequenceNumber")
            if equipment is not None:
                record["equipment"] = rdf_id(equipment, "resource")
            if sequence is not None and sequence.text is not None:
                record["sequence"] = int(sequence.text)
        return {
            terminal_id: (str(record["equipment"]), int(record["sequence"]))
            for terminal_id, record in partial.items()
            if "equipment" in record and "sequence" in record
        }

    def xml_groups(
        directory: Path,
        namespace: str,
    ) -> dict[str, tuple[str, int]]:
        terminals = terminal_owners(directory, namespace)
        groups: dict[str, tuple[str, int]] = {}
        for element in xml_elements(directory, namespace, "OperationalLimitSet"):
            group_id = rdf_id(element, "ID") or rdf_id(element, "about")
            terminal = element.find(
                f"{{{namespace}}}OperationalLimitSet.Terminal"
            )
            terminal_id = rdf_id(terminal, "resource") if terminal is not None else ""
            if terminal_id not in terminals:
                continue
            owner = terminals[terminal_id]
            if owner[0] in boundary_ids:
                require(group_id not in groups, f"{label}: duplicate limit group {group_id}")
                groups[group_id] = owner
        return groups

    source_groups = xml_groups(source_directory, source_namespace)
    fresh_groups = xml_groups(fresh_directory, CIM100)
    ir = json.loads(ir_path.read_text())
    all_ir_groups = ir["value"]["data"]["detailed_connectivity"][
        "operational_limit_groups"
    ]
    ir_groups = {
        str(group["id"]): group
        for group in all_ir_groups
        if group["equipment"]["component_type"] == "branch"
        and str(group["equipment"]["local_id"]) in boundary_ids
    }
    expected_group_count, expected_limit_count = CGMES_BOUNDARY_LIMIT_EXPECTATIONS[
        label
    ]
    require(
        len(source_groups) == expected_group_count,
        f"{label}: official XML has {len(source_groups)} boundary limit groups, "
        f"expected {expected_group_count}",
    )
    require(
        set(ir_groups) == set(source_groups),
        f"{label}: PowerIO IR boundary limit group identities changed",
    )
    require(
        set(fresh_groups) == set(source_groups),
        f"{label}: fresh XML boundary limit group identities changed",
    )
    for group_id, group in ir_groups.items():
        expected_owner = (
            str(group["equipment"]["local_id"]),
            int(group["terminal"]),
        )
        require(
            source_groups[group_id] == expected_owner,
            f"{label}: official boundary limit group {group_id} owner changed",
        )
        require(
            fresh_groups[group_id] == expected_owner,
            f"{label}: fresh boundary limit group {group_id} owner changed",
        )
        require(
            not bool(group["selected"]),
            f"{label}: boundary limit group {group_id} unexpectedly selected",
        )

    type_records: dict[str, tuple[int | None, str]] = {}
    for element in xml_elements(fresh_directory, CIM100, "OperationalLimitType"):
        type_id = rdf_id(element, "ID") or rdf_id(element, "about")
        duration = element.find(
            f"{{{CIM100}}}OperationalLimitType.acceptableDuration"
        )
        direction = element.find(f"{{{CIM100}}}OperationalLimitType.direction")
        type_records[type_id] = (
            int(float(duration.text))
            if duration is not None and duration.text is not None
            else None,
            rdf_id(direction, "resource").rsplit(".", 1)[-1],
        )

    fresh_rows: dict[str, list[tuple[str, float, int | None, bool]]] = {
        group_id: [] for group_id in fresh_groups
    }
    for element in xml_elements(fresh_directory, CIM100, "CurrentLimit"):
        group = element.find(f"{{{CIM100}}}OperationalLimit.OperationalLimitSet")
        group_id = rdf_id(group, "resource") if group is not None else ""
        if group_id not in fresh_rows:
            continue
        name = element.find(f"{{{CIM100}}}IdentifiedObject.name")
        value = element.find(f"{{{CIM100}}}CurrentLimit.normalValue")
        limit_type = element.find(
            f"{{{CIM100}}}OperationalLimit.OperationalLimitType"
        )
        require(name is not None and name.text is not None, f"{label}: unnamed limit")
        require(value is not None and value.text is not None, f"{label}: valueless limit")
        type_id = rdf_id(limit_type, "resource") if limit_type is not None else ""
        require(type_id in type_records, f"{label}: limit has unknown type {type_id}")
        duration, direction = type_records[type_id]
        require(
            direction == "absoluteValue",
            f"{label}: boundary current limit direction is {direction}",
        )
        fresh_rows[group_id].append((name.text, float(value.text), duration, False))

    expected_rows: dict[str, list[tuple[str, float, int | None, bool]]] = {}
    for group_id, group in ir_groups.items():
        limits = group["current_limits"]
        rows = [
            (
                str(limits["permanent_limit_name"]),
                float(limits["permanent_limit"]),
                None,
                False,
            )
        ]
        rows.extend(
            (
                str(limit["name"]),
                float(limit["value"]),
                int(limit["acceptable_duration_seconds"]),
                bool(limit["fictitious"]),
            )
            for limit in limits["temporary_limits"]
        )
        expected_rows[group_id] = rows

    actual_limit_count = sum(map(len, expected_rows.values()))
    require(
        actual_limit_count == expected_limit_count,
        f"{label}: PowerIO IR has {actual_limit_count} retained boundary limits, "
        f"expected {expected_limit_count}",
    )
    mismatches: list[str] = []
    for group_id, expected in expected_rows.items():
        remaining = list(fresh_rows[group_id])
        for expected_row in expected:
            match = next(
                (
                    row
                    for row in remaining
                    if row[0] == expected_row[0]
                    and numeric_values_match(row[1], expected_row[1])
                    and row[2:] == expected_row[2:]
                ),
                None,
            )
            if match is None:
                mismatches.append(f"{group_id}: missing {expected_row}")
            else:
                remaining.remove(match)
        mismatches.extend(f"{group_id}: extra {row}" for row in remaining)
    require(
        not mismatches,
        f"{label}: retained boundary limits changed: {mismatches[:8]}",
    )
    kind_comparisons = sum(source_kinds.values()) + sum(fresh_kinds.values())
    comparisons = expected_group_count * 3 + expected_limit_count * 4 + kind_comparisons
    print(
        f"{label}: raw boundary limit groups={expected_group_count}, "
        f"retained current limits={expected_limit_count}, "
        f"raw limit kinds source={dict(source_kinds)}, fresh={dict(fresh_kinds)}, "
        f"comparisons={comparisons}; "
        "PowSybl's boundary limit frame is a collapsed view"
    )
    return comparisons


def check_cgmes_2415_transformer_connection_projection(
    source: pp.network.Network,
    fresh: pp.network.Network,
    ir_path: Path,
    emit_log_path: Path,
) -> int:
    source_transformers = source.get_2_windings_transformers(all_attributes=True)
    fresh_transformers = fresh.get_2_windings_transformers(all_attributes=True)
    terminals = json.loads(ir_path.read_text())["value"]["data"][
        "detailed_connectivity"
    ]["terminals"]
    log_lines = emit_log_path.read_text().splitlines()
    warning_prefix = (
        "EMIT.CGMES.RECORD_DROPPED: mixed topology projection preserves transformer "
    )
    actual_warnings = {line for line in log_lines if line.startswith(warning_prefix)}
    expected_warnings: set[str] = set()

    for transformer_id, source_bus_id, configured_bus_id, converter_id in (
        CGMES_2415_TRANSFORMER_CONNECTION_PROJECTIONS
    ):
        require(
            bool(source_transformers.at[transformer_id, "connected2"]),
            f"CGMES 2.4.15: official transformer {transformer_id} side TWO disconnected",
        )
        require(
            str(source_transformers.at[transformer_id, "bus2_id"]) == source_bus_id,
            f"CGMES 2.4.15: official transformer {transformer_id} side TWO bus changed",
        )
        require(
            not bool(fresh_transformers.at[transformer_id, "connected2"]),
            f"CGMES 2.4.15: PowSybl transformer {transformer_id} side TWO is connected",
        )
        retained = [
            terminal
            for terminal in terminals
            if terminal["equipment"]
            == {"component_type": "branch", "local_id": transformer_id}
            and terminal["terminal"] == 2
        ]
        require(
            len(retained) == 1,
            f"CGMES 2.4.15: PowerIO IR transformer {transformer_id} side TWO changed",
        )
        retained_terminal = retained[0]
        expected_bus = {"component_type": "bus", "local_id": configured_bus_id}
        require(
            retained_terminal["bus"] == expected_bus
            and retained_terminal["connectable_bus"] == expected_bus,
            f"CGMES 2.4.15: PowerIO IR transformer {transformer_id} bus changed",
        )
        require(
            retained_terminal["connected"] is True,
            f"CGMES 2.4.15: PowerIO IR transformer {transformer_id} connection changed",
        )
        expected_warnings.add(
            warning_prefix
            + f"`branch/{transformer_id}` terminal 2 as connected at configured bus "
            + f"`bus/{configured_bus_id}`, but its projected ConnectivityNode is shared "
            + "only with converter(s) "
            + f"[voltage_source_converter/{converter_id}] in a DC island containing "
            + "DCSeriesDevice; PowSybl 7.3.0 reports that source island as unsupported "
            + "BACK_TO_BACK, drops the converter(s), and reloads this transformer terminal "
            + "as disconnected; PowerIO retains the source connection"
        )

    require(
        actual_warnings == expected_warnings,
        "CGMES 2.4.15: exact PowSybl transformer projection diagnostics changed: "
        f"{sorted(actual_warnings ^ expected_warnings)}",
    )
    comparisons = len(CGMES_2415_TRANSFORMER_CONNECTION_PROJECTIONS) * 7
    print(
        "CGMES 2.4.15: exact external PowSybl transformer connection projections="
        f"{len(CGMES_2415_TRANSFORMER_CONNECTION_PROJECTIONS)}, "
        f"comparisons={comparisons}; PowerIO IR connections remain true"
    )
    return comparisons


def check_cgmes_electrical_values(
    source: pp.network.Network,
    fresh: pp.network.Network,
    label: str,
    source_directory: Path,
    source_namespace: str,
    fresh_directory: Path,
    ir_path: Path,
) -> None:
    frames = (
        ("ordinary line", source.get_lines, fresh.get_lines),
        (
            "2W transformer",
            source.get_2_windings_transformers,
            fresh.get_2_windings_transformers,
        ),
        (
            "3W transformer",
            source.get_3_windings_transformers,
            fresh.get_3_windings_transformers,
        ),
        ("generator", source.get_generators, fresh.get_generators),
        ("load", source.get_loads, fresh.get_loads),
        ("shunt", source.get_shunt_compensators, fresh.get_shunt_compensators),
        (
            "static var compensator",
            source.get_static_var_compensators,
            fresh.get_static_var_compensators,
        ),
        ("VSC converter", source.get_vsc_converter_stations, fresh.get_vsc_converter_stations),
        ("HVDC line", source.get_hvdc_lines, fresh.get_hvdc_lines),
    )
    equipment_count = 0
    comparison_count = 0
    for equipment, source_getter, fresh_getter in frames:
        excluded_cells: set[tuple[str, str]] = set()
        if label == "CGMES 2.4.15" and equipment == "2W transformer":
            excluded_cells = {
                (transformer_id, "connected2")
                for transformer_id, _, _, _ in (
                    CGMES_2415_TRANSFORMER_CONNECTION_PROJECTIONS
                )
            }
        compared_equipment, compared_values = check_electrical_frame(
            source_getter(all_attributes=True),
            fresh_getter(all_attributes=True),
            equipment,
            label,
            excluded_cells=excluded_cells,
        )
        equipment_count += compared_equipment
        comparison_count += compared_values
    comparison_count += check_projected_boundary_lines(
        source,
        fresh,
        label,
        source_directory,
        source_namespace,
        fresh_directory,
        ir_path,
    )
    comparison_count += check_cgmes_solution_values(source, fresh, label)
    comparison_count += check_cgmes_tap_changers(source, fresh, label)
    comparison_count += check_cgmes_switches(source, fresh, label)
    comparison_count += check_operational_limits(source, fresh, label)
    comparison_count += check_boundary_operational_limits(
        source,
        label,
        source_directory,
        source_namespace,
        fresh_directory,
        ir_path,
    )
    if label == "CGMES 2.4.15":
        comparison_count += check_cgmes_2415_transformer_connection_projection(
            source,
            fresh,
            ir_path,
            fresh_directory.parent / "cgmes-2415.emit.log",
        )
    print(
        f"{label}: shared electrical equipment={equipment_count}, "
        f"total comparisons={comparison_count}"
    )


def xml_elements(directory: Path, namespace: str, local_name: str) -> list[ET.Element]:
    result: list[ET.Element] = []
    for path in sorted(directory.glob("*.xml")):
        result.extend(ET.parse(path).getroot().iter(f"{{{namespace}}}{local_name}"))
    return result


def require_cim_namespace(directory: Path, namespace: str, label: str) -> None:
    require(
        any(xml_elements(directory, namespace, local) for local in ("BaseVoltage", "Terminal")),
        f"{label}: no equipment uses {namespace}",
    )


def check_series_compensator(network: pp.network.Network, directory: Path) -> None:
    lines = network.get_lines(all_attributes=True)
    require(SERIES_COMPENSATOR_ID in lines.index, "CGMES 3.0: SeriesCompensator mRID is missing")
    require(
        lines.at[SERIES_COMPENSATOR_ID, "CGMES.originalClass"] == "SeriesCompensator",
        "CGMES 3.0: series equipment was emitted as the wrong CIM class",
    )

    elements = xml_elements(directory, CIM100, "SeriesCompensator")
    definitions = [element for element in elements if rdf_id(element, "ID")]
    require(
        len(definitions) == 1,
        f"CGMES 3.0: found {len(definitions)} SeriesCompensator definitions",
    )
    element = definitions[0]

    def property_text(name: str) -> str:
        child = element.find(f"{{{CIM100}}}{name}")
        require(child is not None and child.text is not None, f"CGMES 3.0: missing {name}")
        return child.text

    require(
        rdf_id(element, "ID") == SERIES_COMPENSATOR_ID,
        "CGMES 3.0: SeriesCompensator mRID changed",
    )
    expected_numbers = {
        "SeriesCompensator.r": 0.0,
        "SeriesCompensator.x": -31.83099,
        "SeriesCompensator.r0": 0.0,
        "SeriesCompensator.x0": -31.83099,
        "SeriesCompensator.varistorRatedCurrent": 500.0,
        "SeriesCompensator.varistorVoltageThreshold": 250.0,
    }
    for name, expected in expected_numbers.items():
        actual = float(property_text(name))
        require(math.isclose(actual, expected, rel_tol=0.0, abs_tol=1e-9), f"{name}: {actual}")
    require(
        property_text("SeriesCompensator.varistorPresent").lower() == "true",
        "CGMES 3.0: SeriesCompensator varistorPresent is not true",
    )


def check_generator_regulation(
    network: pp.network.Network,
    generator_id: str,
    enabled: bool,
    regulated_element_id: str,
    label: str,
) -> None:
    generators = network.get_generators(all_attributes=True)
    require(generator_id in generators.index, f"{label}: missing generator {generator_id}")
    generator = generators.loc[generator_id]
    require(
        bool(generator["voltage_regulator_on"]) is enabled,
        f"{label}: wrong voltage regulation flag for {generator_id}",
    )
    require(
        generator["regulated_element_id"] == regulated_element_id,
        f"{label}: {generator_id} regulates {generator['regulated_element_id']}, "
        f"expected {regulated_element_id}",
    )


def check_synchronous_machine_curve(
    network: pp.network.Network,
    expected_points: tuple[tuple[float, float, float], ...],
    label: str,
) -> None:
    generators = network.get_generators(all_attributes=True)
    require(
        generators.at[REMOTE_GENERATOR_ID, "reactive_limits_kind"] == "CURVE",
        f"{label}: {REMOTE_GENERATOR_ID} does not use a reactive capability curve",
    )
    points = network.get_reactive_capability_curve_points(all_attributes=True)
    require(
        REMOTE_GENERATOR_ID in points.index.get_level_values("id"),
        f"{label}: {REMOTE_GENERATOR_ID} has no reactive capability curve points",
    )
    machine_points = points.loc[REMOTE_GENERATOR_ID]
    require(
        list(machine_points.index) == list(range(len(expected_points))),
        f"{label}: unexpected reactive capability curve point numbers",
    )
    actual_points = tuple(
        (float(row["p"]), float(row["min_q"]), float(row["max_q"]))
        for _, row in machine_points.iterrows()
    )
    require(
        actual_points == expected_points,
        f"{label}: reactive capability curve points are {actual_points}",
    )


def rdf_id(element: ET.Element, attribute: str) -> str:
    return element.get(f"{{{RDF}}}{attribute}", "").lstrip("#_")


def check_multi_authority_boundary_voltages(
    source: pp.network.Network,
    source_directory: Path,
    fresh_directory: Path,
    ir_path: Path,
) -> None:
    expected = {
        sv_id: (node_id, magnitude, angle)
        for sv_id, node_id, magnitude, angle in CGMES_30_PAIRED_AUTHORITY_VOLTAGES
    }
    source_records: dict[str, tuple[str, float, float]] = {}
    for element in xml_elements(source_directory, CIM100, "SvVoltage"):
        sv_id = rdf_id(element, "about") or rdf_id(element, "ID")
        if sv_id not in expected:
            continue
        node = element.find(f"{{{CIM100}}}SvVoltage.TopologicalNode")
        magnitude = element.find(f"{{{CIM100}}}SvVoltage.v")
        angle = element.find(f"{{{CIM100}}}SvVoltage.angle")
        require(node is not None, f"CGMES 3.0: SvVoltage {sv_id} has no node")
        require(
            magnitude is not None and magnitude.text is not None,
            f"CGMES 3.0: SvVoltage {sv_id} has no magnitude",
        )
        require(
            angle is not None and angle.text is not None,
            f"CGMES 3.0: SvVoltage {sv_id} has no angle",
        )
        require(
            sv_id not in source_records,
            f"CGMES 3.0: duplicate paired-authority SvVoltage {sv_id}",
        )
        source_records[sv_id] = (
            rdf_id(node, "resource"),
            float(magnitude.text),
            float(angle.text),
        )
    require(
        source_records == expected,
        f"CGMES 3.0: paired-authority voltage observations changed: {source_records}",
    )

    boundary_nodes = {
        str(node_id)
        for node_id in source.get_boundary_lines(all_attributes=True)[
            "CGMES.TopologicalNode_Boundary"
        ]
        if isinstance(node_id, str) and node_id
    }
    expected_nodes = {record[0] for record in expected.values()}
    require(
        boundary_nodes == expected_nodes,
        f"CGMES 3.0: paired boundary node mRIDs changed: {sorted(boundary_nodes)}",
    )

    fresh_nodes = {
        rdf_id(node, "resource")
        for element in xml_elements(fresh_directory, CIM100, "SvVoltage")
        if (node := element.find(f"{{{CIM100}}}SvVoltage.TopologicalNode"))
        is not None
    }
    require(
        expected_nodes.isdisjoint(fresh_nodes),
        "CGMES 3.0: a different-authority boundary voltage was emitted into fresh SV",
    )

    diagnostics = json.loads(ir_path.read_text())["diagnostics"]
    boundary_diagnostics = [
        diagnostic
        for diagnostic in diagnostics
        if diagnostic["code"] == "READ.CGMES.RECORD_UNMAPPED"
        and "for boundary TopologicalNode" in diagnostic["message"]
    ]
    require(
        len(boundary_diagnostics) == len(expected),
        "CGMES 3.0: expected exactly five boundary voltage projection diagnostics",
    )
    for sv_id, (node_id, magnitude, angle) in expected.items():
        expected_message = (
            f"SvVoltage `{sv_id}` for boundary TopologicalNode `{node_id}` belongs to "
            "modelingAuthoritySet `http://elia.be/CGMES` and supplies "
            f"v={magnitude} kV and angle={angle} degrees; the node is shared by "
            "conducting equipment from modelingAuthoritySets "
            "[`http://elia.be/CGMES`, `http://tennet.nl/CGMES`]. PowerIO maps the "
            "shared node to one boundary bus, so fresh CGMES omits this observation "
            "because it cannot reproduce distinct per-authority PowSybl boundary "
            "bus voltages"
        )
        matches = [
            diagnostic
            for diagnostic in boundary_diagnostics
            if diagnostic["message"] == expected_message
        ]
        require(
            len(matches) == 1 and matches[0]["severity"] == "warning",
            f"CGMES 3.0: no structured authority projection diagnostic for {sv_id}",
        )
    print(
        "CGMES 3.0: paired-authority boundary SvVoltage observations="
        f"{list(CGMES_30_PAIRED_AUTHORITY_VOLTAGES)}; fresh SV omissions=5"
    )


def check_powsybl_equipment_names(
    source: pp.network.Network,
    fresh: pp.network.Network,
    label: str,
) -> None:
    equipment_frames = (
        ("ordinary line", source.get_lines(), fresh.get_lines()),
        (
            "2W transformer",
            source.get_2_windings_transformers(),
            fresh.get_2_windings_transformers(),
        ),
        (
            "3W transformer",
            source.get_3_windings_transformers(),
            fresh.get_3_windings_transformers(),
        ),
        ("generator", source.get_generators(), fresh.get_generators()),
        ("load", source.get_loads(), fresh.get_loads()),
        ("shunt", source.get_shunt_compensators(), fresh.get_shunt_compensators()),
        (
            "static var compensator",
            source.get_static_var_compensators(),
            fresh.get_static_var_compensators(),
        ),
    )
    for equipment, source_frame, fresh_frame in equipment_frames:
        source_ids = set(source_frame.index)
        require(
            source_ids.issubset(set(fresh_frame.index)),
            f"{label}: a named source {equipment} is missing from fresh output",
        )
        changed = {
            equipment_id: (
                source_frame.at[equipment_id, "name"],
                fresh_frame.at[equipment_id, "name"],
            )
            for equipment_id in source_ids
            if source_frame.at[equipment_id, "name"]
            != fresh_frame.at[equipment_id, "name"]
        }
        require(
            not changed,
            f"{label}: {equipment} names changed: {changed}",
        )


def cgmes_equipment_records(
    directory: Path,
    namespace: str,
    label: str,
) -> dict[str, tuple[str, str, str]]:
    records: dict[str, tuple[str, str, str]] = {}
    for path in sorted(directory.glob("*.xml")):
        for element in ET.parse(path).getroot():
            container = element.find(f"{{{namespace}}}Equipment.EquipmentContainer")
            if container is None:
                continue
            mrid_element = element.find(f"{{{namespace}}}IdentifiedObject.mRID")
            mrid = (
                mrid_element.text.strip()
                if mrid_element is not None and mrid_element.text
                else rdf_id(element, "ID")
            )
            name_element = element.find(f"{{{namespace}}}IdentifiedObject.name")
            name = name_element.text if name_element is not None else None
            container_mrid = rdf_id(container, "resource")
            require(mrid, f"{label}: equipment with a container has no mRID")
            require(name is not None, f"{label}: equipment {mrid} has no name")
            require(container_mrid, f"{label}: equipment {mrid} has no container mRID")
            require(mrid not in records, f"{label}: duplicate equipment mRID {mrid}")
            records[mrid] = (
                element.tag.rsplit("}", 1)[-1],
                name,
                container_mrid,
            )
    return records


def cgmes_identified_names(
    directory: Path,
    namespace: str,
    class_name: str,
    label: str,
) -> dict[str, str]:
    records: dict[str, str] = {}
    for element in xml_elements(directory, namespace, class_name):
        mrid_element = element.find(f"{{{namespace}}}IdentifiedObject.mRID")
        mrid = (
            mrid_element.text.strip()
            if mrid_element is not None and mrid_element.text
            else rdf_id(element, "ID") or rdf_id(element, "about")
        )
        name_element = element.find(f"{{{namespace}}}IdentifiedObject.name")
        require(mrid, f"{label}: {class_name} has no mRID")
        require(
            name_element is not None and name_element.text is not None,
            f"{label}: {class_name} {mrid} has no name",
        )
        require(mrid not in records, f"{label}: duplicate {class_name} mRID {mrid}")
        records[mrid] = name_element.text
    return records


def check_cgmes_equipment_metadata(
    source_directory: Path,
    source_namespace: str,
    fresh_directory: Path,
    expected_source_count: int,
    label: str,
) -> None:
    source = cgmes_equipment_records(
        source_directory,
        source_namespace,
        f"official {label}",
    )
    fresh = cgmes_equipment_records(fresh_directory, CIM100, f"fresh {label}")
    require(
        len(source) == expected_source_count,
        f"{label}: official source has {len(source)} equipment records, "
        f"expected {expected_source_count}",
    )
    missing = set(source) - set(fresh)
    require(
        not missing,
        f"{label}: fresh equipment records are missing: {sorted(missing)}",
    )

    mismatches = [
        (mrid, source[mrid], fresh[mrid])
        for mrid in sorted(source)
        if source[mrid][1:] != fresh[mrid][1:]
    ]
    normalized_relationships = 0
    if label == "CGMES 2.4.15":
        (
            junction_id,
            junction_name,
            source_container_id,
            fresh_container_id,
            container_name,
        ) = CGMES_2415_JUNCTION_CONTAINER_PROJECTION
        expected_mismatch = (
            junction_id,
            ("Junction", junction_name, source_container_id),
            ("Junction", junction_name, fresh_container_id),
        )
        require(
            expected_mismatch in mismatches,
            "CGMES 2.4.15: Junction container normalization changed",
        )
        mismatches.remove(expected_mismatch)
        source_lines = cgmes_identified_names(
            source_directory,
            source_namespace,
            "Line",
            f"official {label}",
        )
        fresh_lines = cgmes_identified_names(
            fresh_directory,
            CIM100,
            "Line",
            f"fresh {label}",
        )
        require(
            source_lines.get(source_container_id) == container_name,
            "CGMES 2.4.15: source Junction Line relationship changed",
        )
        require(
            fresh_lines.get(fresh_container_id) == container_name,
            "CGMES 2.4.15: fresh Junction Line relationship changed",
        )
        normalized_relationships = 1
    require(
        not mismatches,
        f"{label}: equipment name or container changed: {mismatches[:5]}",
    )
    print(
        f"{label}: equipment names and container relationships="
        f"{len(source)}/{len(source)}, raw container mRIDs exact="
        f"{len(source) - normalized_relationships}/{len(source)}, "
        f"non-UUID Line container mRIDs normalized={normalized_relationships}"
    )


def check_sv_status_service(
    source_directory: Path,
    fresh: pp.network.Network,
    fresh_directory: Path,
) -> None:
    def statuses_by_equipment(directory: Path) -> dict[str, bool]:
        statuses: dict[str, bool] = {}
        for element in xml_elements(directory, CIM100, "SvStatus"):
            equipment = element.find(f"{{{CIM100}}}SvStatus.ConductingEquipment")
            in_service = element.find(f"{{{CIM100}}}SvStatus.inService")
            require(equipment is not None, "CGMES 3.0: SvStatus has no equipment")
            require(in_service is not None, "CGMES 3.0: SvStatus has no service flag")
            equipment_id = rdf_id(equipment, "resource")
            require(equipment_id, "CGMES 3.0: SvStatus has an empty equipment mRID")
            require(
                equipment_id not in statuses,
                f"CGMES 3.0: duplicate SvStatus for {equipment_id}",
            )
            require(
                in_service.text in ("true", "false"),
                f"CGMES 3.0: invalid service flag for {equipment_id}",
            )
            statuses[equipment_id] = in_service.text == "true"
        return statuses

    source_statuses = statuses_by_equipment(source_directory)
    fresh_statuses = statuses_by_equipment(fresh_directory)
    require(
        len(source_statuses) == CGMES_30_SV_STATUS_COUNT,
        f"CGMES 3.0: official source has {len(source_statuses)} SvStatus records, "
        f"expected {CGMES_30_SV_STATUS_COUNT}",
    )
    require(
        fresh_statuses == source_statuses,
        "CGMES 3.0: fresh SvStatus equipment references or service flags changed",
    )

    matching_status = [
        element
        for element in xml_elements(source_directory, CIM100, "SvStatus")
        if rdf_id(element, "ID") == REMOTE_GENERATOR_SV_STATUS_ID
    ]
    require(len(matching_status) == 1, "CGMES 3.0: generator SvStatus mRID changed")
    status = matching_status[0]
    in_service = status.find(f"{{{CIM100}}}SvStatus.inService")
    equipment = status.find(f"{{{CIM100}}}SvStatus.ConductingEquipment")
    require(
        in_service is not None and in_service.text == "true",
        "CGMES 3.0: generator SvStatus.inService is not true",
    )
    require(
        equipment is not None and rdf_id(equipment, "resource") == REMOTE_GENERATOR_ID,
        "CGMES 3.0: generator SvStatus references the wrong conducting equipment",
    )

    generators = fresh.get_generators(all_attributes=True)
    require(
        bool(generators.at[REMOTE_GENERATOR_ID, "connected"]),
        "CGMES 3.0: the SvStatus in-service generator is disconnected",
    )
    fresh_service_values = [
        child.text
        for path in sorted(fresh_directory.glob("*.xml"))
        for element in ET.parse(path).getroot()
        if rdf_id(element, "about") == REMOTE_GENERATOR_ID
        for child in element
        if child.tag == f"{{{CIM100}}}Equipment.inService"
    ]
    require(
        fresh_service_values == ["true"],
        f"CGMES 3.0: fresh generator in-service values are {fresh_service_values}",
    )
    fresh_status = [
        element
        for element in xml_elements(fresh_directory, CIM100, "SvStatus")
        if (
            equipment := element.find(f"{{{CIM100}}}SvStatus.ConductingEquipment")
        )
        is not None
        and rdf_id(equipment, "resource") == REMOTE_GENERATOR_ID
    ]
    require(
        len(fresh_status) == 1,
        "CGMES 3.0: fresh SV has no unique status for the official generator",
    )
    fresh_in_service = fresh_status[0].find(f"{{{CIM100}}}SvStatus.inService")
    require(
        fresh_in_service is not None and fresh_in_service.text == "true",
        "CGMES 3.0: fresh generator SvStatus.inService is not true",
    )
    print(
        f"CGMES 3.0: SvStatus equipment references and service flags="
        f"{len(source_statuses)}"
    )


def check_cgmes_2415_dc(network: pp.network.Network) -> None:
    hvdc = network.get_hvdc_lines(all_attributes=True)
    require(set(hvdc.index) == {HVDC_LINE_ID}, "CGMES 2.4.15: HVDC line mRID changed")
    line = hvdc.loc[HVDC_LINE_ID]
    for column, expected in (
        ("target_p", 150.0),
        ("max_p", 180.0),
        ("nominal_v", 160.0),
        ("r", 2.5),
    ):
        require(
            math.isclose(float(line[column]), expected, rel_tol=0.0, abs_tol=1e-9),
            f"CGMES 2.4.15: HVDC {column} is {line[column]}, expected {expected}",
        )
    require(
        line["converters_mode"] == "SIDE_1_INVERTER_SIDE_2_RECTIFIER",
        "CGMES 2.4.15: HVDC converter mode changed",
    )
    require(
        line["converter_station1_id"] == "0f05e270-37ea-471d-89fe-aee8a55b932b"
        and line["converter_station2_id"] == "76eeb38f-a3ef-4444-9c65-6cb46a7a94da",
        "CGMES 2.4.15: HVDC converter terminal order changed",
    )
    converters = network.get_vsc_converter_stations(all_attributes=True)
    require(set(converters.index) == VSC_TARGET_Q.keys(), "CGMES 2.4.15: VSC mRIDs changed")
    require(
        all(
            math.isclose(
                float(converters.at[converter_id, "target_q"]),
                expected,
                rel_tol=0.0,
                abs_tol=1e-9,
            )
            for converter_id, expected in VSC_TARGET_Q.items()
        ),
        "CGMES 2.4.15: VSC target Q values changed",
    )
    require(
        not converters["voltage_regulator_on"].any(),
        "CGMES 2.4.15: a VSC voltage regulator was enabled",
    )


def deterministic_cgmes_id(kind: str, name: str) -> str:
    return str(uuid.uuid5(CGMES_UUID_NAMESPACE, f"{kind}:{name}"))


def cgmes_xml_roots(path: Path) -> list[ET.Element]:
    if path.is_dir():
        return [ET.parse(xml_path).getroot() for xml_path in sorted(path.glob("*.xml"))]
    with zipfile.ZipFile(path) as archive:
        names = sorted(
            name for name in archive.namelist() if name.lower().endswith(".xml")
        )
        return [ET.fromstring(archive.read(name)) for name in names]


def cgmes_definition_ids(path: Path, namespace: str, class_name: str) -> set[str]:
    result: set[str] = set()
    for root in cgmes_xml_roots(path):
        for element in root.iter(f"{{{namespace}}}{class_name}"):
            identifier = rdf_id(element, "ID")
            if identifier:
                result.add(identifier)
    return result


def is_uuid(value: str) -> bool:
    try:
        uuid.UUID(value)
    except ValueError:
        return False
    return True


def check_generated_cgmes_substitution_diagnostics(
    ir_path: Path,
    emit_log_path: Path,
    label: str,
) -> dict[str, Any]:
    stored = json.loads(ir_path.read_text(encoding="utf-8"))
    detailed = stored["value"]["data"]["detailed_connectivity"]
    expected_lines: list[str] = []
    identity_rows: list[tuple[str, str]] = []
    for metadata in detailed["component_metadata"]:
        component = metadata["component"]
        component_name = f"{component['component_type']}/{component['local_id']}"
        for identifier in metadata["external_identifiers"]:
            if (identifier.get("authority") or "").lower() != "cgmes":
                continue
            source_id = str(identifier["value"])
            if is_uuid(source_id):
                continue
            expected_lines.append(
                CGMES_VALUE_SUBSTITUTED_PREFIX
                + f"component `{component_name}` has non-UUID CGMES identifier "
                + f"`{source_id}`; fresh CGMES uses a deterministic UUID"
            )
            identity_rows.append((component_name, source_id))

    actual_lines = [
        line
        for line in emit_log_path.read_text(encoding="utf-8").splitlines()
        if line.startswith(CGMES_VALUE_SUBSTITUTED_PREFIX)
    ]
    expected_counter = Counter(expected_lines)
    actual_counter = Counter(actual_lines)
    require(
        actual_counter == expected_counter,
        f"{label}: exact non-UUID substitution diagnostics changed; "
        f"missing={list((expected_counter - actual_counter).elements())[:3]}, "
        f"extra={list((actual_counter - expected_counter).elements())[:3]}",
    )
    require(
        len(identity_rows) == len(set(identity_rows)),
        f"{label}: duplicate non-UUID CGMES identity metadata",
    )
    digest = hashlib.sha256(
        "\n".join(
            sorted(
                f"{component}\t{source_id}" for component, source_id in identity_rows
            )
        ).encode()
    ).hexdigest()
    print(
        f"{label}: non-UUID identity diagnostics={len(identity_rows)}, "
        f"identity inventory SHA-256={digest}"
    )
    return detailed


def add_identity_mapping(
    source_to_fresh: dict[str, str],
    fresh_to_source: dict[str, str],
    source_id: Any,
    fresh_id: Any,
    label: str,
) -> None:
    source_text = str(source_id)
    fresh_text = str(fresh_id)
    previous_fresh = source_to_fresh.setdefault(source_text, fresh_text)
    previous_source = fresh_to_source.setdefault(fresh_text, source_text)
    require(
        previous_fresh == fresh_text and previous_source == source_text,
        f"{label}: identity mapping is not one-to-one for {source_text} -> {fresh_text}",
    )


def map_named_identities(
    source_frame: pd.DataFrame,
    fresh_frame: pd.DataFrame,
    source_to_fresh: dict[str, str],
    fresh_to_source: dict[str, str],
    label: str,
    object_name: str,
    deterministic_kind: str | None = None,
    component_type: str | None = None,
) -> int:
    require(
        len(source_frame) == len(fresh_frame),
        f"{label}: {object_name} count changed from {len(source_frame)} to {len(fresh_frame)}",
    )
    require(
        "name" in source_frame.columns and "name" in fresh_frame.columns,
        f"{label}: {object_name} names are unavailable",
    )
    source_names = {
        str(name): str(index) for index, name in source_frame["name"].items()
    }
    fresh_names = {str(name): str(index) for index, name in fresh_frame["name"].items()}
    require(
        len(source_names) == len(source_frame) and len(fresh_names) == len(fresh_frame),
        f"{label}: {object_name} names are not unique",
    )
    require(
        source_names.keys() == fresh_names.keys(),
        f"{label}: {object_name} names changed; "
        f"missing={sorted(source_names.keys() - fresh_names.keys())[:5]}, "
        f"extra={sorted(fresh_names.keys() - source_names.keys())[:5]}",
    )
    for name, source_id in source_names.items():
        fresh_id = fresh_names[name]
        if deterministic_kind is not None:
            seed = (
                source_id if component_type is None else f"{component_type}/{source_id}"
            )
            require(
                fresh_id == deterministic_cgmes_id(deterministic_kind, seed),
                f"{label}: {object_name} {source_id} has unexpected deterministic mRID "
                f"{fresh_id}",
            )
        add_identity_mapping(
            source_to_fresh,
            fresh_to_source,
            source_id,
            fresh_id,
            label,
        )
    return len(source_frame)


def remap_generated_cgmes_frame(
    frame: pd.DataFrame,
    fresh_to_source: dict[str, str],
) -> pd.DataFrame:
    def remap(value: Any) -> Any:
        return fresh_to_source.get(value, value) if isinstance(value, str) else value

    result = frame.copy()
    if isinstance(result.index, pd.MultiIndex):
        result.index = pd.MultiIndex.from_tuples(
            [tuple(remap(value) for value in row) for row in result.index],
            names=result.index.names,
        )
    else:
        result.index = pd.Index(
            [remap(value) for value in result.index], name=result.index.name
        )
    for column in result.columns:
        result[column] = result[column].map(remap)
    return result


def check_generated_cgmes_terminal_rows(
    source: pp.network.Network,
    fresh: pp.network.Network,
    fresh_to_source: dict[str, str],
    label: str,
) -> int:
    source_frame = source.get_terminals(all_attributes=True)
    fresh_frame = remap_generated_cgmes_frame(
        fresh.get_terminals(all_attributes=True),
        fresh_to_source,
    )

    def rows(frame: pd.DataFrame) -> dict[tuple[str, str], tuple[Any, ...]]:
        result: dict[tuple[str, str], tuple[Any, ...]] = {}
        for equipment_id, row in frame.iterrows():
            key = (str(equipment_id), str(row["element_side"]))
            require(key not in result, f"{label}: duplicate terminal {key}")
            result[key] = (
                row["voltage_level_id"],
                row["bus_id"],
                bool(row["connected"]),
            )
        return result

    source_rows = rows(source_frame)
    fresh_rows = rows(fresh_frame)
    require(
        source_rows == fresh_rows,
        f"{label}: terminal equipment, side, voltage level, calculation bus, or "
        "connection changed",
    )
    comparisons = len(source_rows) * 5
    print(f"{label}: equipment terminal comparisons={comparisons}")
    return comparisons


def check_generated_cgmes_xml_identities(
    source_path: Path,
    fresh_path: Path,
    detailed: dict[str, Any],
    namespace: str,
    label: str,
) -> int:
    comparisons = 0
    for field, class_name in (
        ("bus_breaker_buses", "TopologicalNode"),
        ("terminals", "Terminal"),
        ("connectivity_nodes", "ConnectivityNode"),
    ):
        source_ids = cgmes_definition_ids(source_path, namespace, class_name)
        fresh_ids = cgmes_definition_ids(fresh_path, CIM100, class_name)
        records = detailed[field]
        require(
            len(source_ids) == len(records),
            f"{label}: source {class_name} definition count changed from "
            f"{len(records)} to {len(source_ids)}",
        )
        require(
            len(fresh_ids) == len(records),
            f"{label}: fresh {class_name} definition count changed from "
            f"{len(records)} to {len(fresh_ids)}",
        )
        for record in records:
            component = record["component"]
            source_id = str(component["local_id"])
            expected_fresh_id = deterministic_cgmes_id(
                str(component["component_type"]),
                f"{component['component_type']}/{source_id}",
            )
            require(
                source_id in source_ids,
                f"{label}: source {class_name} {source_id} is absent",
            )
            require(
                expected_fresh_id in fresh_ids,
                f"{label}: fresh {class_name} for {source_id} has the wrong mRID",
            )
            comparisons += 2
    print(
        f"{label}: calculation node, terminal, and connectivity node mRID mappings="
        f"{len(detailed['bus_breaker_buses']) + len(detailed['terminals']) + len(detailed['connectivity_nodes'])}"
    )
    return comparisons


def check_generated_cgmes_equivalence(
    source: pp.network.Network,
    fresh: pp.network.Network,
    source_path: Path,
    fresh_path: Path,
    ir_path: Path,
    emit_log_path: Path,
    namespace: str,
    label: str,
) -> None:
    expected = XIIDM_EXPECTATIONS["XIIDM remote control"]
    detailed = check_generated_cgmes_substitution_diagnostics(
        ir_path, emit_log_path, label
    )
    source_to_fresh: dict[str, str] = {}
    fresh_to_source: dict[str, str] = {}

    identity_frames = (
        (
            "substation",
            source.get_substations,
            fresh.get_substations,
            "substation",
            "substation",
        ),
        (
            "voltage level",
            source.get_voltage_levels,
            fresh.get_voltage_levels,
            "voltage_level",
            "voltage_level",
        ),
        (
            "bus breaker bus",
            source.get_bus_breaker_view_buses,
            fresh.get_bus_breaker_view_buses,
            None,
            None,
        ),
        ("calculated bus", source.get_buses, fresh.get_buses, None, None),
        ("line", source.get_lines, fresh.get_lines, "branch", None),
        (
            "2W transformer",
            source.get_2_windings_transformers,
            fresh.get_2_windings_transformers,
            "branch",
            None,
        ),
        (
            "3W transformer",
            source.get_3_windings_transformers,
            fresh.get_3_windings_transformers,
            "transformer_3w",
            None,
        ),
        ("generator", source.get_generators, fresh.get_generators, "generator", None),
        ("load", source.get_loads, fresh.get_loads, "load", None),
        (
            "shunt",
            source.get_shunt_compensators,
            fresh.get_shunt_compensators,
            "shunt",
            None,
        ),
        (
            "LCC converter",
            source.get_lcc_converter_stations,
            fresh.get_lcc_converter_stations,
            None,
            None,
        ),
        ("HVDC line", source.get_hvdc_lines, fresh.get_hvdc_lines, None, None),
    )
    for (
        object_name,
        source_getter,
        fresh_getter,
        kind,
        component_type,
    ) in identity_frames:
        map_named_identities(
            source_getter(all_attributes=True),
            fresh_getter(all_attributes=True),
            source_to_fresh,
            fresh_to_source,
            label,
            object_name,
            kind,
            component_type,
        )

    source_limits = source.get_operational_limits(all_attributes=True)
    fresh_limits = fresh.get_operational_limits(all_attributes=True)
    require(
        len(source_limits) == expected["operational limits"],
        f"{label}: source operational limit count changed",
    )
    require(
        len(source_limits) == len(fresh_limits),
        f"{label}: operational limit count changed",
    )
    source_limit_kinds = cgmes_operational_limit_kinds(
        source_path, namespace, f"source {label}"
    )
    fresh_limit_kinds = cgmes_operational_limit_kinds(
        fresh_path, CIM100, f"fresh {label}"
    )
    expected_limit_kinds = Counter({"patl": 1})
    require(
        source_limit_kinds == expected_limit_kinds,
        f"{label}: source raw OperationalLimitType kinds changed: {source_limit_kinds}",
    )
    require(
        fresh_limit_kinds == expected_limit_kinds,
        f"{label}: fresh raw OperationalLimitType kinds changed: {fresh_limit_kinds}",
    )
    fresh_limit_keys = set(fresh_limits.index)
    for equipment_id, side, limit_kind, duration, group_id in source_limits.index:
        fresh_equipment = source_to_fresh[str(equipment_id)]
        fresh_group = deterministic_cgmes_id("source_limit_set", str(group_id))
        fresh_key = (fresh_equipment, side, limit_kind, duration, fresh_group)
        require(
            fresh_key in fresh_limit_keys,
            f"{label}: operational limit identity {equipment_id}/{side}/{group_id} changed",
        )
        add_identity_mapping(
            source_to_fresh,
            fresh_to_source,
            group_id,
            fresh_group,
            label,
        )

    frames = (
        (
            "voltage level",
            "voltage levels",
            source.get_voltage_levels,
            fresh.get_voltage_levels,
        ),
        ("bus", "buses", source.get_buses, fresh.get_buses),
        ("line", "lines", source.get_lines, fresh.get_lines),
        (
            "2W transformer",
            "2W transformers",
            source.get_2_windings_transformers,
            fresh.get_2_windings_transformers,
        ),
        (
            "3W transformer",
            "3W transformers",
            source.get_3_windings_transformers,
            fresh.get_3_windings_transformers,
        ),
        ("generator", "generators", source.get_generators, fresh.get_generators),
        ("load", "loads", source.get_loads, fresh.get_loads),
        (
            "shunt",
            "shunts",
            source.get_shunt_compensators,
            fresh.get_shunt_compensators,
        ),
        (
            "LCC converter",
            "LCC converters",
            source.get_lcc_converter_stations,
            fresh.get_lcc_converter_stations,
        ),
        ("HVDC line", "HVDC lines", source.get_hvdc_lines, fresh.get_hvdc_lines),
    )
    comparisons = 0
    for equipment, count_name, source_getter, fresh_getter in frames:
        source_frame = source_getter(all_attributes=True)
        require(
            len(source_frame) == expected[count_name],
            f"{label}: source has {len(source_frame)} {count_name}, "
            f"expected {expected[count_name]}",
        )
        _, compared = check_electrical_frame(
            source_frame,
            remap_generated_cgmes_frame(
                fresh_getter(all_attributes=True),
                fresh_to_source,
            ),
            equipment,
            label,
            GENERATED_CGMES_ELECTRICAL_FIELDS,
            exact_ids=True,
        )
        comparisons += compared

    _, compared = check_electrical_frame(
        source_limits,
        remap_generated_cgmes_frame(fresh_limits, fresh_to_source),
        "operational limit",
        label,
        XIIDM_ELECTRICAL_FIELDS,
        exact_ids=True,
    )
    comparisons += compared
    comparisons += sum(source_limit_kinds.values()) + sum(fresh_limit_kinds.values())
    comparisons += check_tap_rows(
        tap_changer_rows(source.get_ratio_tap_changers(all_attributes=True), label),
        tap_changer_rows(
            remap_generated_cgmes_frame(
                fresh.get_ratio_tap_changers(all_attributes=True),
                fresh_to_source,
            ),
            label,
        ),
        "ratio changer",
        expected["ratio changers"],
        label,
        XIIDM_TAP_FIELDS,
    )
    comparisons += check_tap_rows(
        tap_step_rows(source.get_ratio_tap_changer_steps(all_attributes=True), label),
        tap_step_rows(
            remap_generated_cgmes_frame(
                fresh.get_ratio_tap_changer_steps(all_attributes=True),
                fresh_to_source,
            ),
            label,
        ),
        "ratio step",
        expected["ratio steps"],
        label,
        XIIDM_TAP_FIELDS,
    )
    comparisons += check_generated_cgmes_terminal_rows(
        source,
        fresh,
        fresh_to_source,
        label,
    )
    comparisons += check_generated_cgmes_xml_identities(
        source_path,
        fresh_path,
        detailed,
        namespace,
        label,
    )

    source_switches = source.get_switches(all_attributes=True)
    fresh_switches = fresh.get_switches(all_attributes=True)
    require(
        len(source_switches) == len(fresh_switches) == 0,
        f"{label}: the PowSybl generated reference case unexpectedly contains switches",
    )
    identity_digest = hashlib.sha256(
        "\n".join(
            f"{source_id}\t{fresh_id}"
            for source_id, fresh_id in sorted(source_to_fresh.items())
        ).encode()
    ).hexdigest()
    print(
        f"{label}: mapped retained identities={len(source_to_fresh)}, "
        f"mapping SHA-256={identity_digest}, comparisons={comparisons}; "
        f"raw limit kinds source={dict(source_limit_kinds)}, "
        f"fresh={dict(fresh_limit_kinds)}; switches=0"
    )


def check_xiidm_equivalence(
    source: pp.network.Network,
    fresh: pp.network.Network,
    label: str,
    expectation_key: str | None = None,
) -> None:
    expected = XIIDM_EXPECTATIONS[expectation_key or label]
    frames = (
        (
            "voltage level",
            "voltage levels",
            source.get_voltage_levels,
            fresh.get_voltage_levels,
        ),
        ("bus", "buses", source.get_buses, fresh.get_buses),
        ("line", "lines", source.get_lines, fresh.get_lines),
        (
            "2W transformer",
            "2W transformers",
            source.get_2_windings_transformers,
            fresh.get_2_windings_transformers,
        ),
        (
            "3W transformer",
            "3W transformers",
            source.get_3_windings_transformers,
            fresh.get_3_windings_transformers,
        ),
        ("generator", "generators", source.get_generators, fresh.get_generators),
        ("load", "loads", source.get_loads, fresh.get_loads),
        ("shunt", "shunts", source.get_shunt_compensators, fresh.get_shunt_compensators),
        (
            "LCC converter",
            "LCC converters",
            source.get_lcc_converter_stations,
            fresh.get_lcc_converter_stations,
        ),
        ("HVDC line", "HVDC lines", source.get_hvdc_lines, fresh.get_hvdc_lines),
    )
    comparisons = 0
    for equipment, count_name, source_getter, fresh_getter in frames:
        source_frame = source_getter(all_attributes=True)
        fresh_frame = fresh_getter(all_attributes=True)
        require(
            len(source_frame) == expected[count_name],
            f"{label}: official source has {len(source_frame)} {count_name}, "
            f"expected {expected[count_name]}",
        )
        _, compared = check_electrical_frame(
            source_frame,
            fresh_frame,
            equipment,
            label,
            XIIDM_ELECTRICAL_FIELDS,
            exact_ids=True,
        )
        comparisons += compared

    source_limits = source.get_operational_limits(all_attributes=True)
    fresh_limits = fresh.get_operational_limits(all_attributes=True)
    require(
        len(source_limits) == expected["operational limits"],
        f"{label}: official source has {len(source_limits)} operational limits, "
        f"expected {expected['operational limits']}",
    )
    _, compared = check_electrical_frame(
        source_limits,
        fresh_limits,
        "operational limit",
        label,
        XIIDM_ELECTRICAL_FIELDS,
        exact_ids=True,
    )
    comparisons += compared

    comparisons += check_tap_rows(
        tap_changer_rows(source.get_ratio_tap_changers(all_attributes=True), label),
        tap_changer_rows(fresh.get_ratio_tap_changers(all_attributes=True), label),
        "ratio changer",
        expected["ratio changers"],
        label,
        XIIDM_TAP_FIELDS,
    )
    comparisons += check_tap_rows(
        tap_step_rows(source.get_ratio_tap_changer_steps(all_attributes=True), label),
        tap_step_rows(fresh.get_ratio_tap_changer_steps(all_attributes=True), label),
        "ratio step",
        expected["ratio steps"],
        label,
        XIIDM_TAP_FIELDS,
    )
    for tap_kind, source_frame, fresh_frame in (
        (
            "phase changer",
            source.get_phase_tap_changers(all_attributes=True),
            fresh.get_phase_tap_changers(all_attributes=True),
        ),
        (
            "phase step",
            source.get_phase_tap_changer_steps(all_attributes=True),
            fresh.get_phase_tap_changer_steps(all_attributes=True),
        ),
    ):
        require(len(source_frame) == 0, f"{label}: official source has a {tap_kind}")
        require(len(fresh_frame) == 0, f"{label}: fresh output added a {tap_kind}")
    comparisons += check_switch_frame(
        source.get_switches(all_attributes=True),
        fresh.get_switches(all_attributes=True),
        expected["switches"],
        label,
    )
    print(f"{label}: strict source/fresh comparisons={comparisons}")


def psse_canonical_frame(frame: pd.DataFrame) -> pd.DataFrame:
    def strip_padding(value: Any) -> Any:
        return value.rstrip(" ") if isinstance(value, str) else value

    result = frame.copy()
    if isinstance(result.index, pd.MultiIndex):
        result.index = pd.MultiIndex.from_tuples(
            [
                tuple(
                    strip_padding(value) if position == 0 else value
                    for position, value in enumerate(row)
                )
                for row in result.index
            ],
            names=result.index.names,
        )
    else:
        result.index = pd.Index(
            [strip_padding(value) for value in result.index],
            name=result.index.name,
        )
    for column in result.columns:
        result[column] = result[column].map(strip_padding)
    return result


def check_psse_equivalence(
    source: pp.network.Network,
    fresh: pp.network.Network,
    label: str,
) -> None:
    expected = PSSE_EXPECTATIONS[label]
    frames = (
        (
            "voltage level",
            "voltage levels",
            source.get_voltage_levels,
            fresh.get_voltage_levels,
        ),
        ("bus", "buses", source.get_buses, fresh.get_buses),
        ("line", "lines", source.get_lines, fresh.get_lines),
        (
            "2W transformer",
            "2W transformers",
            source.get_2_windings_transformers,
            fresh.get_2_windings_transformers,
        ),
        (
            "3W transformer",
            "3W transformers",
            source.get_3_windings_transformers,
            fresh.get_3_windings_transformers,
        ),
        ("generator", "generators", source.get_generators, fresh.get_generators),
        ("load", "loads", source.get_loads, fresh.get_loads),
        ("shunt", "shunts", source.get_shunt_compensators, fresh.get_shunt_compensators),
        (
            "operational limit",
            "operational limits",
            source.get_operational_limits,
            fresh.get_operational_limits,
        ),
    )
    comparisons = 0
    for equipment, count_name, source_getter, fresh_getter in frames:
        source_frame = psse_canonical_frame(source_getter(all_attributes=True))
        fresh_frame = psse_canonical_frame(fresh_getter(all_attributes=True))
        require(
            len(source_frame) == expected[count_name],
            f"{label}: official source has {len(source_frame)} {count_name}, "
            f"expected {expected[count_name]}",
        )
        _, compared = check_electrical_frame(
            source_frame,
            fresh_frame,
            equipment,
            label,
            PSSE_ELECTRICAL_FIELDS,
            exact_ids=True,
        )
        comparisons += compared

    comparisons += check_tap_rows(
        tap_changer_rows(
            psse_canonical_frame(source.get_ratio_tap_changers(all_attributes=True)),
            label,
        ),
        tap_changer_rows(
            psse_canonical_frame(fresh.get_ratio_tap_changers(all_attributes=True)),
            label,
        ),
        "ratio changer",
        expected["ratio changers"],
        label,
        XIIDM_TAP_FIELDS,
    )
    comparisons += check_tap_rows(
        tap_step_rows(
            psse_canonical_frame(source.get_ratio_tap_changer_steps(all_attributes=True)),
            label,
        ),
        tap_step_rows(
            psse_canonical_frame(fresh.get_ratio_tap_changer_steps(all_attributes=True)),
            label,
        ),
        "ratio step",
        expected["ratio steps"],
        label,
        XIIDM_TAP_FIELDS,
    )
    for tap_kind, source_frame, fresh_frame in (
        (
            "phase changer",
            source.get_phase_tap_changers(all_attributes=True),
            fresh.get_phase_tap_changers(all_attributes=True),
        ),
        (
            "phase step",
            source.get_phase_tap_changer_steps(all_attributes=True),
            fresh.get_phase_tap_changer_steps(all_attributes=True),
        ),
    ):
        require(len(source_frame) == 0, f"{label}: official source has a {tap_kind}")
        require(len(fresh_frame) == 0, f"{label}: fresh output added a {tap_kind}")
    comparisons += check_switch_frame(
        psse_canonical_frame(source.get_switches(all_attributes=True)),
        psse_canonical_frame(fresh.get_switches(all_attributes=True)),
        expected["switches"],
        label,
    )
    print(f"{label}: strict source/fresh comparisons={comparisons}")


def check_xiidm_remote_control(network: pp.network.Network) -> None:
    check_generator_regulation(network, "B6-G1", True, "B5-L1", "XIIDM remote control")
    generators = network.get_generators(all_attributes=True)
    require(
        not bool(generators.at["B1-G1", "voltage_regulator_on"]),
        "XIIDM remote control: B1-G1 voltage regulation was enabled",
    )


def check_xiidm_hvdc(network: pp.network.Network, path: Path) -> None:
    converters = network.get_lcc_converter_stations(all_attributes=True)
    require(len(converters) == 2, f"XIIDM HVDC: found {len(converters)} LCC stations")
    require(converters["p"].isna().all(), "XIIDM HVDC: absent terminal p became a number")
    require(converters["q"].isna().all(), "XIIDM HVDC: absent terminal q became a number")

    root = ET.parse(path).getroot()
    namespace = root.tag.removesuffix("network").strip("{}")
    elements = list(root.iter(f"{{{namespace}}}lccConverterStation"))
    require(len(elements) == 2, f"XIIDM HVDC XML: found {len(elements)} LCC stations")
    require(
        all("p" not in element.attrib and "q" not in element.attrib for element in elements),
        "XIIDM HVDC XML: an absent terminal p/q attribute was written",
    )


def check_xiidm_version_fixture(
    version: str,
    source_path: Path,
    expected_sha256: str,
    fresh_path: Path,
    ir_path: Path,
) -> None:
    label = f"XIIDM {version}"
    require(
        hashlib.sha256(source_path.read_bytes()).hexdigest() == expected_sha256,
        f"{label}: official PowSybl fixture checksum changed",
    )
    source = load_checked(source_path, f"official {label}")
    fresh = load_checked(fresh_path, f"fresh {label}")
    check_xiidm_equivalence(source, fresh, label)

    root = ET.parse(fresh_path).getroot()
    require(
        root.tag == "{http://www.powsybl.org/schema/iidm/1_17}network",
        f"{label}: fresh output is not XIIDM 1.17",
    )
    diagnostics = json.loads(ir_path.read_text(encoding="utf-8"))["diagnostics"]
    expected_extension_message = (
        "XIIDM extension element `threeWindingsTransformerToBeEstimated` "
        "on `3WT` is retained only by exact same format emission"
    )
    require(
        sum(
            diagnostic["code"] == "READ.XIIDM.ELEMENT_UNMAPPED"
            and diagnostic["message"] == expected_extension_message
            for diagnostic in diagnostics
        )
        == 1,
        f"{label}: missing exact diagnostic for the unsupported PowSybl extension",
    )
    expected_compatibility = 0 if version == "1.17" else 1
    require(
        sum(
            diagnostic["code"] == "READ.XIIDM.VERSION.COMPATIBILITY"
            for diagnostic in diagnostics
        )
        == expected_compatibility,
        f"{label}: wrong legacy version diagnostic count",
    )
    print(
        f"{label}: official fixture SHA-256={expected_sha256}; "
        "source read and fresh XIIDM 1.17 reload passed"
    )


def check_switched_shunts(network: pp.network.Network) -> None:
    shunts = network.get_shunt_compensators(all_attributes=True)
    require(set(shunts.index) == {"B2-SwSH1", "B3-SwSH1"}, "PSS/E shunt ids changed")
    require(
        (shunts["section_count"] == 3).all(),
        f"PSS/E switched-shunt section counts are {shunts['section_count'].to_dict()}",
    )
    require(
        (shunts["max_section_count"] == 5).all(),
        f"PSS/E switched-shunt maximum sections are {shunts['max_section_count'].to_dict()}",
    )


def check_named_equipment(network: pp.network.Network) -> None:
    expected = (
        (network.get_lines(), "L-1-2-3", "LINE-S1-220-S2-220", "line"),
        (network.get_2_windings_transformers(), "T-2-3-3", "T2D-S2-220-15", "2W"),
        (network.get_3_windings_transformers(), "T-2-4-5-2", "T3D-S2-220-45-10", "3W"),
    )
    for frame, equipment_id, name, kind in expected:
        require(equipment_id in frame.index, f"RAWX: missing {kind} {equipment_id}")
        require(frame.at[equipment_id, "name"] == name, f"RAWX: wrong {kind} name")


def rawx_subnode_voltages(path: Path) -> dict[tuple[int, int], tuple[Any, Any]]:
    root = json.loads(path.read_text(encoding="utf-8"))
    table = root["network"]["subnode"]
    fields = table["fields"]
    indices = {field: fields.index(field) for field in ("isub", "inode", "vm", "va")}
    return {
        (int(row[indices["isub"]]), int(row[indices["inode"]])): (
            row[indices["vm"]],
            row[indices["va"]],
        )
        for row in table["data"]
    }


def check_rawx_nulls(source_path: Path, fresh_path: Path) -> None:
    source = rawx_subnode_voltages(source_path)
    fresh = rawx_subnode_voltages(fresh_path)
    require(source.keys() == fresh.keys(), "RAWX: substation node keys changed")
    null_rows = {key for key, values in source.items() if values == (None, None)}
    require(null_rows, "RAWX source: no explicit null vm/va rows")
    require(
        all(fresh[key] == (None, None) for key in null_rows),
        "RAWX: an explicit null substation node vm/va became a number",
    )


def check_node_breaker(network: pp.network.Network) -> None:
    generator_id = "B1-G1"
    check_generator_regulation(
        network,
        generator_id,
        False,
        "VL1-Busbar-2",
        "PSS/E node breaker",
    )
    generators = network.get_generators(all_attributes=True)
    require(
        int(generators.at[generator_id, "node"]) == 7,
        "PSS/E node breaker: generator node changed",
    )

    transformer_id = "T-1-3-4-1"
    taps = network.get_ratio_tap_changers(all_attributes=True)
    require(transformer_id in taps.index, "PSS/E node breaker: missing 3W ratio tap changer")
    transformer_taps = taps.loc[[transformer_id]]
    require(
        set(transformer_taps["side"]) == {"ONE", "TWO", "THREE"},
        "PSS/E node breaker: 3W tap changer sides changed",
    )
    regulated = transformer_taps[transformer_taps["side"] == "THREE"]
    require(len(regulated) == 1, "PSS/E node breaker: side THREE tap changer is not unique")
    row = regulated.iloc[0]
    require(int(row["tap"]) == 1, "PSS/E node breaker: side THREE tap changed")
    require(bool(row["oltc"]), "PSS/E node breaker: side THREE lost load tap capability")
    require(bool(row["regulating"]), "PSS/E node breaker: side THREE regulation is off")
    require(row["regulating_bus_id"] == "VL4_1", "PSS/E node breaker: wrong regulated bus")
    require(
        math.isclose(float(row["target_v"]), 21.105, rel_tol=0.0, abs_tol=1e-9),
        "PSS/E node breaker: side THREE target voltage changed",
    )


def cgmes_archive(directory: Path, output_dir: Path, stem: str) -> Path:
    return Path(shutil.make_archive(str(output_dir / stem), "zip", root_dir=directory))


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("output_dir", type=Path)
    parser.add_argument("powsybl_core", type=Path)
    args = parser.parse_args()
    check_powsybl_version()

    output_dir = args.output_dir.resolve()
    powsybl_core = args.powsybl_core.resolve()
    case9_cgmes_zip = cgmes_archive(output_dir / "case9-cgmes", output_dir, "case9-cgmes")

    for path, label in (
        (output_dir / "case9.xiidm", "case9 XIIDM"),
        (case9_cgmes_zip, "case9 CGMES"),
        (output_dir / "case9-psse33.raw", "case9 PSS/E RAW revision 33"),
        (output_dir / "case9-psse35.raw", "case9 PSS/E RAW revision 35"),
        (output_dir / "case9.rawx", "case9 PSS/E RAWX"),
    ):
        check_case9(path, label)
    check_pow_sybl_rejects_raw_revision_34(output_dir / "case9-psse34.raw")

    cgmes_30_source_path = powsybl_core / CGMES_30_RELATIVE
    cgmes_30_source = load_checked(cgmes_30_source_path, "official CGMES 3.0")
    cgmes_30_fresh = load_checked(output_dir / "cgmes-30", "fresh CGMES 3.0")
    require_cim_namespace(cgmes_30_source_path, CIM100, "official CGMES 3.0")
    require_cim_namespace(output_dir / "cgmes-30", CIM100, "fresh CGMES 3.0")
    check_multi_authority_boundary_voltages(
        cgmes_30_source,
        cgmes_30_source_path,
        output_dir / "cgmes-30",
        output_dir / "cgmes-30.pio.json",
    )
    check_cgmes_projection(
        cgmes_30_source,
        cgmes_30_fresh,
        "CGMES 3.0",
        CGMES_30_SOURCE_COUNTS,
    )
    check_powsybl_equipment_names(cgmes_30_source, cgmes_30_fresh, "CGMES 3.0")
    check_cgmes_electrical_values(
        cgmes_30_source,
        cgmes_30_fresh,
        "CGMES 3.0",
        cgmes_30_source_path,
        CIM100,
        output_dir / "cgmes-30",
        output_dir / "cgmes-30.pio.json",
    )
    check_cgmes_equipment_metadata(
        cgmes_30_source_path,
        CIM100,
        output_dir / "cgmes-30",
        CGMES_30_EQUIPMENT_RECORD_COUNT,
        "CGMES 3.0",
    )
    check_series_compensator(cgmes_30_fresh, output_dir / "cgmes-30")
    check_synchronous_machine_curve(
        cgmes_30_source,
        CGMES_30_REACTIVE_CURVE,
        "official CGMES 3.0",
    )
    check_generator_regulation(
        cgmes_30_fresh,
        REMOTE_GENERATOR_ID,
        True,
        REMOTE_REGULATED_ELEMENT_ID,
        "CGMES 3.0",
    )
    check_synchronous_machine_curve(
        cgmes_30_fresh,
        CGMES_30_REACTIVE_CURVE,
        "CGMES 3.0",
    )
    check_sv_status_service(
        cgmes_30_source_path,
        cgmes_30_fresh,
        output_dir / "cgmes-30",
    )

    cgmes_2415_source_path = powsybl_core / CGMES_2415_RELATIVE
    cgmes_2415_source = load_checked(cgmes_2415_source_path, "official CGMES 2.4.15")
    cgmes_2415_fresh = load_checked(output_dir / "cgmes-2415", "fresh CGMES 2.4.15 input")
    require_cim_namespace(cgmes_2415_source_path, CIM16, "official CGMES 2.4.15")
    require_cim_namespace(output_dir / "cgmes-2415", CIM100, "fresh CGMES 2.4.15 input")
    check_cgmes_projection(
        cgmes_2415_source,
        cgmes_2415_fresh,
        "CGMES 2.4.15",
        CGMES_2415_SOURCE_COUNTS,
    )
    check_powsybl_equipment_names(
        cgmes_2415_source,
        cgmes_2415_fresh,
        "CGMES 2.4.15",
    )
    check_cgmes_electrical_values(
        cgmes_2415_source,
        cgmes_2415_fresh,
        "CGMES 2.4.15",
        cgmes_2415_source_path,
        CIM16,
        output_dir / "cgmes-2415",
        output_dir / "cgmes-2415.pio.json",
    )
    check_cgmes_equipment_metadata(
        cgmes_2415_source_path,
        CIM16,
        output_dir / "cgmes-2415",
        CGMES_2415_EQUIPMENT_RECORD_COUNT,
        "CGMES 2.4.15",
    )
    check_synchronous_machine_curve(
        cgmes_2415_source,
        CGMES_2415_REACTIVE_CURVE,
        "official CGMES 2.4.15",
    )
    check_generator_regulation(
        cgmes_2415_fresh,
        REMOTE_GENERATOR_ID,
        True,
        REMOTE_REGULATED_ELEMENT_ID,
        "CGMES 2.4.15",
    )
    check_synchronous_machine_curve(
        cgmes_2415_fresh,
        CGMES_2415_REACTIVE_CURVE,
        "CGMES 2.4.15",
    )
    check_cgmes_2415_dc(cgmes_2415_fresh)

    remote_control_source = load_checked(
        powsybl_core / REMOTE_CONTROL_RELATIVE,
        "official XIIDM remote control",
    )
    remote_control = load_checked(
        output_dir / "remote-control.xiidm",
        "fresh XIIDM remote control",
    )
    check_xiidm_equivalence(
        remote_control_source,
        remote_control,
        "XIIDM remote control",
    )
    check_xiidm_remote_control(remote_control)

    two_terminal_dc_source = load_checked(
        powsybl_core / TWO_TERMINAL_DC_RELATIVE,
        "official XIIDM HVDC",
    )
    two_terminal_dc = load_checked(
        output_dir / "two-terminal-dc.xiidm",
        "fresh XIIDM HVDC",
    )
    check_xiidm_equivalence(
        two_terminal_dc_source,
        two_terminal_dc,
        "XIIDM HVDC",
    )
    check_xiidm_hvdc(two_terminal_dc, output_dir / "two-terminal-dc.xiidm")

    node_breaker_xiidm_path = powsybl_core / NODE_BREAKER_XIIDM_RELATIVE
    require(
        hashlib.sha256(node_breaker_xiidm_path.read_bytes()).hexdigest()
        == NODE_BREAKER_XIIDM_SHA256,
        "XIIDM node breaker: official PowSybl fixture checksum changed",
    )
    node_breaker_xiidm_source = load_checked(
        node_breaker_xiidm_path,
        "official XIIDM node breaker",
    )
    node_breaker_xiidm = load_checked(
        output_dir / "five-bus-node-breaker.xiidm",
        "fresh XIIDM node breaker",
    )
    check_xiidm_equivalence(
        node_breaker_xiidm_source,
        node_breaker_xiidm,
        "XIIDM node breaker",
    )
    print(
        "XIIDM node breaker: official fixture SHA-256="
        f"{NODE_BREAKER_XIIDM_SHA256}"
    )

    powsybl_inputs = output_dir / "powsybl-inputs"
    for version in range(12, 18):
        source = load_checked(
            powsybl_inputs / f"powsybl-xiidm-1-{version}.xiidm",
            f"PowSybl generated XIIDM 1.{version}",
        )
        fresh = load_checked(
            output_dir / f"powsybl-xiidm-1-{version}.xiidm",
            f"fresh PowSybl XIIDM 1.{version}",
        )
        check_xiidm_equivalence(
            source,
            fresh,
            f"PowSybl XIIDM 1.{version}",
            expectation_key="XIIDM remote control",
        )

    for version in ("2415", "30"):
        source_path = powsybl_inputs / f"powsybl-cgmes-{version}.zip"
        fresh_path = output_dir / f"powsybl-cgmes-{version}"
        source = load_checked(
            source_path,
            f"PowSybl generated CGMES {version}",
        )
        fresh = load_checked(
            fresh_path,
            f"fresh PowSybl CGMES {version}",
        )
        check_generated_cgmes_equivalence(
            source,
            fresh,
            source_path,
            fresh_path,
            output_dir / f"powsybl-cgmes-{version}.pio.json",
            output_dir / f"powsybl-cgmes-{version}.emit.log",
            CIM16 if version == "2415" else CIM100,
            f"PowSybl CGMES {version}",
        )

    for version, (relative_path, expected_sha256) in XIIDM_VERSION_FIXTURES.items():
        stem = version.replace(".", "-")
        check_xiidm_version_fixture(
            version,
            powsybl_core / relative_path,
            expected_sha256,
            output_dir / f"xiidm-v{stem}.xiidm",
            output_dir / f"xiidm-v{stem}.pio.json",
        )

    switched_shunt_source = load_checked(
        powsybl_core / SWITCHED_SHUNT_RELATIVE,
        "official PSS/E switched shunt",
    )
    switched_shunt = load_checked(
        output_dir / "switched-shunt.raw",
        "fresh PSS/E switched shunt",
    )
    check_psse_equivalence(
        switched_shunt_source,
        switched_shunt,
        "PSS/E switched shunt",
    )
    check_switched_shunts(switched_shunt)

    two_substations_source = load_checked(
        powsybl_core / TWO_SUBSTATIONS_RELATIVE,
        "official PSS/E RAWX",
    )
    two_substations = load_checked(
        output_dir / "two-substations.rawx",
        "fresh PSS/E RAWX",
    )
    check_psse_equivalence(
        two_substations_source,
        two_substations,
        "PSS/E RAWX",
    )
    check_named_equipment(two_substations)
    check_rawx_nulls(powsybl_core / TWO_SUBSTATIONS_RELATIVE, output_dir / "two-substations.rawx")

    node_breaker_source = load_checked(
        powsybl_core / NODE_BREAKER_RELATIVE,
        "official PSS/E node breaker",
    )
    node_breaker = load_checked(
        output_dir / "five-bus-node-breaker.raw",
        "fresh PSS/E node breaker",
    )
    check_psse_equivalence(
        node_breaker_source,
        node_breaker,
        "PSS/E node breaker",
    )
    check_node_breaker(node_breaker)
    print(
        "official PowSybl reference cases: fresh emission checks passed; "
        f"assertions={ASSERTION_COUNT[0]}"
    )


if __name__ == "__main__":
    main()
