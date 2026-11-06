"""Strict supplement for the two fields emitted by upstream mpc2ravens.py.

The pinned UML/template omits these fields even though its own balanced
converter writes them. This does not relax the distribution schema or allow
arbitrary extra properties. Keep this distinction visible in reports.
"""

from copy import deepcopy


def closed_object(properties: dict) -> dict:
    return {
        "type": "object",
        "additionalProperties": False,
        "required": list(properties),
        "properties": properties,
    }


def balanced_schema(upstream: dict) -> dict:
    schema = deepcopy(upstream)
    nodes = schema["properties"]["ConnectivityNode"]["patternProperties"]["^.+$"]
    if (
        "ConnectivityNode.SvVoltage" in nodes["properties"]
        or "MySettings" in schema["properties"]
    ):
        raise ValueError(
            "upstream schema changed: review the balanced profile supplement"
        )
    nodes["properties"]["ConnectivityNode.SvVoltage"] = {
        "type": "array",
        "minItems": 1,
        "maxItems": 1,
        "items": closed_object(
            {
                "Ravens.cimObjectType": {"const": "SvVoltage"},
                "IdentifiedObject.mRID": {"type": "string"},
                "SvVoltage.v": {"type": "number"},
                "SvVoltage.angle": {"type": "number"},
            }
        ),
    }
    schema["properties"]["MySettings"] = closed_object(
        {
            "Ravens.cimObjectType": {"const": "AlgorithmSettings"},
            "IdentifiedObject.mRID": {"type": "string"},
            "IdentifiedObject.name": {"const": "MySettings"},
            "ApplicationSettings.Settings": {
                "type": "array",
                "minItems": 1,
                "maxItems": 1,
                "items": closed_object(
                    {
                        "Ravens.cimObjectType": {"const": "GenericApplicationSetting"},
                        "name": {"const": "baseMVA"},
                        "value": {"type": "number", "exclusiveMinimum": 0},
                        "type": {"const": "float"},
                    }
                ),
            },
        }
    )
    return schema
