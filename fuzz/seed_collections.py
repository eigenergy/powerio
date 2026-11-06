"""Generate small synthetic inputs for the collection/archive fuzz smoke gate."""
import io
import json
from pathlib import Path
import zipfile


def archive(files, method=zipfile.ZIP_STORED):
    output = io.BytesIO()
    with zipfile.ZipFile(output, "w", compression=method) as zipped:
        for name, content in files:
            zipped.writestr(name, content)
    return output.getvalue()


def main():
    corpus = Path(__file__).parent / "corpus" / "collection_archive"
    corpus.mkdir(parents=True, exist_ok=True)
    child = archive([("master.dss", b"New Circuit.fuzz basekv=12.47\n")])
    seeds = {
        "empty": archive([]),
        "stored": child,
        "deflated": archive([("case.xml", b"<rdf:RDF/>"), ("empty/", b"")], zipfile.ZIP_DEFLATED),
        "nested": archive([("child.zip", child)]),
        "prefixed-native": b"prefix" + child,
        "collision": archive([("A/file", b"a"), ("a/file", b"b")]),
        "traversal": archive([("../escape", b"x")]),
        "manifest": json.dumps({"schema": "powerio-collection", "version": 1, "cases": [{
            "id": "opaque", "selection": {"primary": "opaque.bin"}
        }]}).encode(),
    }
    for name, data in seeds.items():
        (corpus / name).write_bytes(data)


if __name__ == "__main__":
    main()
