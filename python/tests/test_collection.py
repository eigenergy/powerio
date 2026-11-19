import io
import zipfile

import pytest
from powerio.dist import SincalReadOptions

import powerio as pio


def files():
    return {
        "masters/day.dss": b"New Circuit.day basekv=12.47 bus1=source\nRedirect ../shared/topology.inc\n",
        "masters/night.dss": b"New Circuit.night basekv=12.47 bus1=source\nRedirect ../shared/topology.inc\n",
        "shared/topology.inc": b"New Line.l bus1=source bus2=load phases=3 r1=0.1 x1=0.2 r0=0.3 x0=0.4 length=1\n",
    }


def test_shared_collection_selection_and_module_lifetime():
    with pio.Collection.from_files(files()) as collection:
        candidates = collection.inspect()["candidates"]
        assert len(candidates) == 2
        for index in [0, 1, 1, 0]:
            parsed = collection.parse(candidates[index]["selection"])
            assert parsed.dependencies == ("shared/topology.inc",)
            assert not parsed.incomplete
    assert parsed.module.value is not None
    with pytest.raises(ValueError, match="closed"):
        collection.inspect()


def test_zip_folder_and_memory_inspection_match(tmp_path):
    buffer = io.BytesIO()
    with zipfile.ZipFile(buffer, "w", zipfile.ZIP_DEFLATED) as archive:
        for path, data in files().items():
            archive.writestr(path, data)
            target = tmp_path / path
            target.parent.mkdir(exist_ok=True)
            target.write_bytes(data)
    expected = pio.Collection.from_files(files()).inspect()
    assert pio.Collection.from_zip(buffer.getvalue()).inspect() == expected
    assert pio.Collection.open(tmp_path).inspect() == expected
    with pytest.raises(ValueError):
        pio.Collection.from_files(files(), max_bytes=1)
    with pytest.raises((ValueError, pio.PowerIOError)):
        pio.Collection.from_files(files()).parse({"primary": "../outside.dss"})


def test_selection_metadata_is_bounded():
    with pio.Collection.from_files({"case.dss": b"New Circuit.case basekv=12.47\n"}) as collection:
        with pytest.raises(ValueError, match="selection metadata exceeds 1 MiB"):
            collection.parse({"primary": "x" * (1 << 20)})


@pytest.mark.parametrize(
    "kwargs, message",
    [
        (
            {"sincal_multiconductor": pio.SincalBalancedReadOptions()},
            "sincal_multiconductor must be dist.SincalReadOptions",
        ),
        (
            {"sincal_balanced": SincalReadOptions()},
            "sincal_balanced must be SincalBalancedReadOptions",
        ),
        (
            {"sincal_multiconductor": object()},
            "sincal_multiconductor must be dist.SincalReadOptions",
        ),
        (
            {"sincal_balanced": object()},
            "sincal_balanced must be SincalBalancedReadOptions",
        ),
    ],
)
def test_collection_rejects_wrong_sincal_option_types(kwargs, message):
    collection = pio.Collection(object())
    with pytest.raises(TypeError, match=message):
        collection.parse({}, **kwargs)

