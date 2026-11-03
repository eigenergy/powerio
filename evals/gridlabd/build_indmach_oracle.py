"""Build the pinned upstream IndMach012a library for the native IEEE oracle.

Requires Free Pascal 3.2.2. The currently verified build is macOS arm64.
Source files stay outside the repository. Patches change paths, the unused
Windows resource reference, and Darwin export names; no model equations change.
The output manifest records every original/patched hash and the compiled binary.
"""

import argparse
import hashlib
import json
import platform
import re
import subprocess
import sys
import urllib.request
from pathlib import Path

from oracle import LOCK

EXPORTS = (
    "New", "Delete", "Select", "Init", "Calc", "Integrate", "Save", "Restore",
    "Edit", "UpdateModel", "NumVars", "GetAllVars", "GetVariable", "SetVariable", "GetVarName",
)


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def portable_source(name, data):
    text = data.decode("utf-8").replace("\r\n", "\n")
    if name == "IndMach012Model.pas":
        text = text.replace("{$INCLUDE ..\\Common\\DSSCallBackStructDef.pas}", "{$INCLUDE DSSCallBackStructDef.pas}")
    if name == "IndMach012a.dpr":
        text = re.sub(r"in '\.\.\\[^']*\\([^'\\]+)'", r"in '\1'", text)
        text = text.replace("{$R *.RES}", "")
        head, tail = text.split("Exports", 1)
        for symbol in EXPORTS:
            tail, count = re.subn(r"\b" + symbol + r"(?=[,;])", f"{symbol} name '_{symbol}'", tail)
            if count != 1:
                raise ValueError(f"missing or repeated upstream export {symbol}")
        text = head + "Exports" + tail
    return text.encode()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True, help="new external build directory")
    parser.add_argument("--fpc", default="fpc", help="Free Pascal compiler executable")
    args = parser.parse_args()
    pin = LOCK["indmach012a"]
    host = f"{platform.system()} {platform.machine()}"
    if host != pin["qualified_build_platform"]:
        raise ValueError(f"unverified build platform {host}; expected {pin['qualified_build_platform']}")
    version = subprocess.check_output([args.fpc, "-iV"], text=True).strip()
    if version != pin["compiler_version"]:
        raise ValueError(f"unpinned Free Pascal version {version}")
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    original, build = output / "original", output / "build"
    original.mkdir()
    build.mkdir()
    records = []
    for item in pin["files"]:
        url = f"https://raw.githubusercontent.com/tshort/OpenDSS/{pin['commit']}/{item['path']}"
        with urllib.request.urlopen(url, timeout=60) as response:
            data = response.read(1024 * 1024)
        git_hash = hashlib.sha1(f"blob {len(data)}\0".encode() + data).hexdigest()
        if len(data) != item["bytes"] or git_hash != item["git_blob_sha1"]:
            raise ValueError(f"upstream pin mismatch: {item['path']}")
        name = Path(item["path"]).name
        (original / name).write_bytes(data)
        patched = portable_source(name, data) if name != "License.txt" else data
        (build / name).write_bytes(patched)
        records.append(dict(item, url=url, sha256=sha256(data), build_sha256=sha256(patched)))
    command = [args.fpc, "-Mdelphi", "-Cg", "-O2", "IndMach012a.dpr"]
    result = subprocess.run(command, cwd=build, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, check=False)
    (output / "compiler.log").write_text(result.stdout)
    result.check_returncode()
    library = build / "libIndMach012a.dylib"
    # Check Darwin C symbol names in a separate process before engine loading.
    subprocess.run([sys.executable, "-c", "import ctypes,sys; lib=ctypes.CDLL(sys.argv[1]); [getattr(lib,n) for n in sys.argv[2:]]", str(library), *EXPORTS], check=True)
    (build / "IndMach012a").symlink_to(library.name)
    manifest = dict(repository=pin["repository"], commit=pin["commit"], platform=host,
                    compiler_version=version, command=command, files=records,
                    library=str(library), library_sha256=sha256(library.read_bytes()),
                    loader_directory=str(build), engine_version=LOCK["dss"]["version"],
                    scope="Native reference only. Loading this library does not establish a typed PowerIO motor law.")
    (output / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    print(json.dumps(manifest, indent=2))


if __name__ == "__main__":
    main()
