#!/usr/bin/env python3
"""Build the disposable guest without host privilege or system configuration changes."""

import argparse
import fcntl
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import time
import traceback


GIB = 1024**3
ROOT = Path.home() / ".cache/cupertino-vm"
CONFIG = Path(__file__).resolve().parent


def allocated_bytes(root: Path) -> int:
    total = root.lstat().st_blocks * 512
    seen = set()
    for path in root.rglob("*"):
        if path.is_relative_to(ROOT / "workspace"):
            continue
        try:
            stat = path.lstat()
            identity = (stat.st_dev, stat.st_ino)
            if identity not in seen:
                seen.add(identity)
                total += stat.st_blocks * 512
        except FileNotFoundError:
            # mkosi removes staging files while the budget scan is in progress.
            continue
    return total


def check_space() -> None:
    stats = os.statvfs(ROOT)
    free = stats.f_bavail * stats.f_frsize
    if allocated_bytes(ROOT) > 4 * GIB or free < 30 * GIB:
        raise SystemExit("VM budget exceeded or free space fell below 30 GiB")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=["build"])
    parser.parse_args()
    if os.getuid() == 0:
        raise SystemExit("Run as the ordinary user; root execution is refused")
    ROOT.mkdir(mode=0o700, parents=True, exist_ok=True)
    check_space()
    with open("/dev/kvm", "rb", buffering=0) as kvm:
        if fcntl.ioctl(kvm, 0xAE00) != 12:
            raise SystemExit("KVM API version is unsupported")
    for name in ["output", "workspace", "packages", "cache", "logs"]:
        (ROOT / name).mkdir(exist_ok=True)
    (ROOT / "logs/status.json").write_text(json.dumps({
        "state": "building", "exit_status": None, "stderr": "",
    }) + "\n")
    command = [
        "mkosi", "-C", str(CONFIG), "--force",
        "--output-directory", str(ROOT / "output"),
        "--workspace-directory", str(ROOT / "workspace"),
        "--package-cache-directory", str(ROOT / "packages"),
        "--cache-directory", str(ROOT / "cache"), "build",
    ]
    print("Executing:", " ".join(command), flush=True)
    started = time.monotonic()
    with (ROOT / "logs/build.log").open("w") as log:
        process = subprocess.Popen(command, stdout=log, stderr=subprocess.STDOUT,
                                   start_new_session=True)
        try:
            while process.poll() is None:
                # Leave headroom for writes in flight before reaching the hard cap.
                if allocated_bytes(ROOT) > 3.75 * GIB:
                    usage = subprocess.run(
                        ["du", "-B1", "-d3", str(ROOT)],
                        capture_output=True, text=True, check=False,
                    )
                    (ROOT / "logs/budget-report.json").write_text(json.dumps({
                        "counted_bytes": allocated_bytes(ROOT),
                        "du_exit": usage.returncode,
                        "du_stdout": usage.stdout, "du_stderr": usage.stderr,
                    }) + "\n")
                    raise SystemExit("Build stopped at 3.75 GiB allocation; preserving headroom below the 4 GiB hard cap")
                check_space()
                time.sleep(0.25)
        finally:
            if process.poll() is None:
                os.killpg(process.pid, signal.SIGTERM)
                try:
                    process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    os.killpg(process.pid, signal.SIGKILL)
                    process.wait()
    print(f"Image build exit={process.returncode}, seconds={time.monotonic() - started:.3f}, "
          f"allocated_bytes={allocated_bytes(ROOT)}", flush=True)
    raise SystemExit(process.returncode)


if __name__ == "__main__":
    signal.signal(signal.SIGTERM, lambda number, frame: sys.exit(128 + number))
    try:
        main()
    except BaseException as error:
        # Preserve the controller failure as well as mkosi's child-process log.
        if (ROOT / "logs").is_dir():
            (ROOT / "logs/controller-result.json").write_text(json.dumps({
                "exception": type(error).__name__, "detail": str(error),
            }) + "\n")
            if not isinstance(error, SystemExit):
                (ROOT / "logs/controller-traceback.log").write_text(traceback.format_exc())
        raise
