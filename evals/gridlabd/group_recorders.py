"""Recorder declarations shared by feeder validation; no circuit interpretation."""


def group_recorders(properties):
    text = ""
    outputs = []
    for cls, fields in properties.items():
        for field in fields:
            filename = f"{cls}-{field}.csv"
            text += f'object group_recorder {{ group "class={cls}"; property {field}; file "{filename}"; interval 1; strict true; print_units false; }}\n'
            outputs.append(filename)
    return text, tuple(outputs)


def recorders(properties, objects, profile):
    if profile == "pnnl":
        text, outputs = group_recorders(properties)
        return text, properties, outputs
    if profile != "arras":
        raise ValueError("unknown recorder engine profile")
    # The pinned Arras group recorder truncates complex rows. Keep each
    # multi-recorder property list below its native string buffer capacity.
    chunks, chunk = [], []
    for name, obj in objects.items():
        for field in properties[obj["class"]]:
            value = f"{name}:{field}"
            if len(",".join(chunk + [value])) > 900:
                chunks.append(chunk)
                chunk = []
            chunk.append(value)
    if chunk:
        chunks.append(chunk)
    text = ""
    for index, fields in enumerate(chunks):
        text += f'object multi_recorder {{ property {",".join(fields)}; file "records/chunk{index}.csv"; interval 1; }}\n'
    return text, {"chunks": chunks}, ("records/.",)
