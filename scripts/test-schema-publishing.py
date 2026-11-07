"""Check archived schema bytes and redirects at every published path depth."""

import json
import subprocess
import tempfile
import textwrap
from pathlib import Path
from urllib.parse import urljoin

ROOT = Path(__file__).resolve().parents[1]
workflow = (ROOT / ".github/workflows/docs.yml").read_text()
marker = "    - name: PowerIO IR schema history\n      run: |\n"
script = textwrap.dedent(workflow.split(marker, 1)[1].split("    - uses:", 1)[0])
cases = {
    "pio-ir/0.12.0": "pio-ir/0.12.0",
    "pio-ir/2": "pio-ir/2",
    "pio-ir/2/0.11.1": "pio-ir/2/0.11.1",
    "pio-ir/1": "pio-module/1",
    "pio-ir/0.1": "pio-package/0.1",
    "pio-ir/2/nested/example": "pio-ir/alias",
}
with tempfile.TemporaryDirectory(prefix="powerio-schema-publishing-") as temporary:
    root = Path(temporary)
    guide = root / "target/doc/guide/pio-json-schema.html"
    guide.parent.mkdir(parents=True)
    guide.write_text('<h1 id="pio-ir">PowerIO IR</h1>\n')
    documents = {}
    for archive, served in cases.items():
        schema = root / "docs/schema" / archive / "schema.json"
        schema.parent.mkdir(parents=True)
        identifier = f"https://powerio.dev/schema/{served}"
        if archive != "pio-ir/0.1":
            identifier += "/schema.json"
        data = (json.dumps({"$id": identifier, "archive": archive}) + "\n").encode()
        schema.write_bytes(data)
        documents[archive] = data
    subprocess.run(["bash", "-euo", "pipefail", "-c", script], cwd=root, check=True)
    for archive, served in cases.items():
        for path in {archive, served}:
            published = root / "target/doc/schema" / path
            assert (published / "schema.json").read_bytes() == documents[archive], path
            redirect = (published / "index.html").read_text()
            target = redirect.split('content="0; url=', 1)[1].split('"', 1)[0]
            resolved = urljoin(f"https://powerio.dev/schema/{path}/", target)
            assert resolved == "https://powerio.dev/guide/pio-json-schema.html#pio-ir", resolved
print("Schema publishing preserves nested catalogs, archived bytes, and redirects.")
