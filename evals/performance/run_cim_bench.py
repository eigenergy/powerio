"""Sequential, pinned-image CIM bench run; never build during measurement."""

import argparse
import datetime
import hashlib
import json
import platform
import shlex
import subprocess
import time
import xml.etree.ElementTree as ET
from pathlib import Path

import yaml


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--cim-bench", type=Path, required=True)
    p.add_argument("--output", type=Path, required=True)
    p.add_argument("--services", nargs="*")
    p.add_argument("--baseline", action="store_true")
    p.add_argument("--rounds", type=int, default=3)
    p.add_argument("--reverse-order", action="store_true")
    p.add_argument("--timeout", type=int, default=900)
    a = p.parse_args()
    if a.rounds < 1:
        p.error("--rounds must be positive")
    root = a.cim_bench.resolve()
    out = a.output.resolve()
    out.mkdir(parents=True, exist_ok=True)
    if (out / "manifest.json").exists():
        p.error(
            "choose a new output directory; an existing run will not be overwritten"
        )
    services = yaml.safe_load((root / "docker/docker-compose.yml").read_text())[
        "services"
    ]
    if a.services:
        services = {k: services[k] for k in a.services}
    if a.baseline:
        for dataset in ["svedala", "realgrid"]:
            services[f"powerio-baseline-{dataset}"] = {
                "image": "cim-bench/powerio:baseline",
                "command": f"pytest powerio_{dataset}_benchmark.py --benchmark-only",
            }
    priority = ["powerio-svedala", "powerio-realgrid"]
    services = dict(
        sorted(
            services.items(),
            key=lambda item: (
                priority.index(item[0]) if item[0] in priority else len(priority)
            ),
        )
    )
    if a.reverse_order:
        services = dict(reversed(list(services.items())))
    report = {
        "started_utc": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "host": platform.platform(),
        "cim_bench_revision": subprocess.check_output(
            ["git", "-C", str(root), "rev-parse", "HEAD"], text=True
        ).strip(),
        "patch_sha256": hashlib.sha256(
            subprocess.check_output(["git", "-C", str(root), "diff", "HEAD"])
        ).hexdigest(),
        "runs": [],
    }
    for service, config in services.items():
        name = "cim-comparison-" + service
        test = shlex.split(config["command"])[1]
        print("RUN", service, flush=True)
        entry = {
            "service": service,
            "started_utc": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        }
        image = subprocess.run(
            ["podman", "image", "inspect", config["image"], "--format", "{{.Id}}"],
            capture_output=True,
            text=True,
            check=False,
        )
        if image.returncode:
            entry.update(status="image unavailable", detail=image.stderr.strip())
            report["runs"].append(entry)
            (out / "manifest.json").write_text(json.dumps(report, indent=2) + "\n")
            continue
        entry["image"] = image.stdout.strip()
        temp = out / service
        temp.mkdir(exist_ok=True)
        cmd = [
            "podman",
            "run",
            "--rm",
            "--name",
            name,
            "--network=none",
            "--cpus=8",
            "--memory=12g",
            "-v",
            f"{root}/benchmarks:/benchmarks:ro",
            "-v",
            f"{root}/parsers:/benchmarks/parsers:ro",
            "-v",
            f"{root}/data:/benchmarks/data:ro",
            "-v",
            f"{temp}:/benchmarks/temp",
            "-v",
            f"{out}:/output",
        ]
        # Historical 549c6ff baseline predates the shared 1 GiB defaults.
        if "baseline" in service:
            for var in ["PRIMARY", "REFERENCED", "CGMES"]:
                cmd += ["-e", f"POWERIO_MAX_{var}_BYTES=1073741824"]
        cmd += [
            entry["image"],
            "pytest",
            test,
            "--benchmark-only",
            f"--benchmark-min-rounds={a.rounds}",
            "--benchmark-max-time=1",
            "--benchmark-warmup=off",
            "-p",
            "no:cacheprovider",
            f"--benchmark-json=/output/{service}.json",
            "--junitxml=/output/" + service + ".xml",
        ]
        entry["command"] = cmd
        start = time.monotonic()
        with (out / (service + ".log")).open("w") as log:
            try:
                run = subprocess.run(
                    cmd,
                    stdout=log,
                    stderr=subprocess.STDOUT,
                    timeout=a.timeout,
                    check=False,
                )
                entry.update(
                    status="passed" if run.returncode == 0 else "failed",
                    exit_code=run.returncode,
                )
            except subprocess.TimeoutExpired:
                subprocess.run(
                    ["podman", "stop", "--time", "5", name],
                    stdout=log,
                    stderr=subprocess.STDOUT,
                    check=False,
                )
                entry.update(status="timeout", timeout_seconds=a.timeout)
        entry["elapsed_seconds"] = time.monotonic() - start
        junit = out / (service + ".xml")
        if junit.exists():
            suites = list(ET.parse(junit).getroot().iter("testsuite"))
            entry["tests"] = {
                k: sum(int(s.get(k, "0")) for s in suites)
                for k in ("tests", "failures", "errors", "skipped")
            }
            if (
                entry["status"] == "passed"
                and entry["tests"]["tests"] == entry["tests"]["skipped"]
            ):
                entry["status"] = "all skipped"
        result = out / (service + ".json")
        if result.exists():
            entry["benchmark_count"] = len(json.loads(result.read_text())["benchmarks"])
        report["runs"].append(entry)
        (out / "manifest.json").write_text(json.dumps(report, indent=2) + "\n")
        print(service, entry["status"], round(entry["elapsed_seconds"], 2), flush=True)


if __name__ == "__main__":
    main()
