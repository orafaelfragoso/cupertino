#!/usr/bin/env python3
"""Runs a Cupertino session in a disposable guest on three virtio-gpu heads and checks that it follows the connected
heads through an unplug and replug, a VT round trip with a head unplugged while away, and an S3 cycle with a head
unplugged while asleep. Writes a screenshot of every output after each step, the session log and a results file to
--out; exits 1 when a step fails. Build the image first: `ci/vm/build-image.sh build`.

    ci/vm/daily_driver.py --binaries "$CARGO_TARGET_DIR/debug" --out /tmp/daily-driver [--gpus 2]

--binaries needs `cargo build -p cupertino-server -p cupertino-testclient -p cupertino-screenshot
--features cupertino-screenshot/app`."""

import argparse
import fcntl
import json
from pathlib import Path
import re
import shutil
import sys
import time

sys.path.insert(0, str(Path(__file__).resolve().parent))
from guest import ROOT, Guest  # noqa: E402

SERVER_LOG = "/home/qa/.local/state/cupertino/server.log"
SESSION_LOG = "/home/qa/session.log"
SHOTS = "/home/qa/shots"
EVENTS = r"display (dis)?connected|output initialised|session (paused|resumed)|cannot show its mode|fallback mode|CRTC"


class Run:
    def __init__(self, guest: Guest, out: Path):
        self.guest, self.out, self.results = guest, out, []
        self.step_start = 0

    def events(self) -> list[str]:
        status, text = self.guest.sh(f"[ ! -e {SERVER_LOG} ] || cat {SERVER_LOG}")
        if status != 0:
            raise RuntimeError(f"reading the server log: {text}")
        return [line for line in text.splitlines() if re.search(EVENTS, line)]

    def flip_failures(self) -> list[str]:
        """The outputs of the server's flip failures, one per failure, from its warnings on standard error."""
        _, text = self.guest.sh(f"[ ! -e {SESSION_LOG} ] || cat {SESSION_LOG}")
        text = re.sub(r"\x1b\[[0-9;]*m", "", text)
        return re.findall(r"queueing the frame failed.* output=(\S+)", text)

    def outputs(self) -> set[str]:
        """The connectors the server has connected and not disconnected, by its log."""
        current = set()
        for line in self.events():
            if found := re.search(r"display (connected|disconnected) connector=(\S+)", line):
                (current.add if found.group(1) == "connected" else current.discard)(found.group(2))
        return current

    def connected(self) -> set[str]:
        _, text = self.guest.sh("for c in /sys/class/drm/card*-*; do "
                                "[ \"$(cat $c/status)\" = connected ] && echo ${c##*/card?-}; done")
        return set(text.split())

    def processes(self) -> tuple[int, int]:
        """How many servers and test clients run."""
        _, running = self.guest.sh("readlink /proc/[0-9]*/exe")
        return tuple(running.split().count(f"/usr/bin/cupertino-{name}") for name in ["server", "testclient"])

    def settle(self, read, expected, seconds: float = 20):
        """Reads until the value is `expected` or `seconds` pass, and returns the last value read."""
        deadline = time.monotonic() + seconds
        while (value := read()) != expected and time.monotonic() < deadline:
            time.sleep(0.5)
        return value

    def check(self, step: str, heads: set[str]) -> bool:
        connected = self.settle(self.connected, heads)
        outputs = self.settle(self.outputs, heads)
        server, clients = self.settle(self.processes, (1, 3))
        _, cards = self.guest.sh("ls -d /sys/class/drm/card[0-9]")
        shots = self.screenshots(step)
        failures = self.flip_failures()
        flips = sorted(set(failures[self.step_start:]))
        self.step_start = len(failures)
        passed = (connected == heads and outputs == heads and server == 1 and clients == 3 and len(shots) == len(heads)
                  and len(cards.split()) == self.guest.gpus and not flips)
        result = {"step": step, "passed": passed, "expected": sorted(heads), "guest_connected": sorted(connected),
                  "server_outputs": sorted(outputs), "servers": server, "clients": clients,
                  "cards": len(cards.split()), "flip_failures": flips, "screenshots": shots}
        self.results.append(result)
        print(json.dumps(result), flush=True)
        return passed

    def screenshots(self, step: str) -> list[str]:
        """Takes a screenshot of every output with the session's shortcut and copies the files out: QMP cannot read the
        scanout of a virgl resource."""
        self.guest.sh(f"rm -rf {SHOTS}")
        self.send_keys("shift", "meta_l", "3")
        listing = lambda: self.guest.sh(f"[ ! -d {SHOTS} ] || ls -1 --quoting-style=literal {SHOTS}")[1].split("\n")
        names = self.settle(lambda: len([n for n in listing() if n.endswith(".png")]) == len(self.outputs()), True, 10)
        number = lambda name: int(found.group(1)) if (found := re.search(r" \((\d+)\)\.png$", name)) else 1
        files = sorted((n for n in listing() if n.endswith(".png")), key=number) if names else []
        shots = []
        for index, name in enumerate(files, 1):
            path = self.out / f"{step}-{index}.png"
            self.guest.fetch(f"{SHOTS}/{name}", path)
            shots.append(path.name)
        return shots

    def send_keys(self, *keys: str) -> None:
        self.guest.qmp("send-key", keys=[{"type": "qcode", "data": key} for key in keys])

    def wait_log(self, text: str, after: int, seconds: float = 20) -> bool:
        return self.settle(lambda: any(text in line for line in self.events()[after:]), True, seconds)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binaries", type=Path, required=True, help="directory with cupertino-server, "
                        "cupertino-testclient and cupertino-screenshot")
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--fail-flip", metavar="OUTPUT", help="write a flip failure of OUTPUT among the server's "
                        "warnings during step 2, which must then fail")
    parser.add_argument("--gpus", type=int, default=1, help="virtio-gpu devices; the steps use the first, the others have no head connected")
    args = parser.parse_args()
    config = Path(__file__).resolve().parent
    args.out.mkdir(parents=True, exist_ok=True)
    run_dir = ROOT / "run"
    payload = run_dir / "payload"
    shutil.rmtree(payload, ignore_errors=True)
    payload.mkdir(parents=True)
    for name in ["cupertino-server", "cupertino-testclient", "cupertino-screenshot"]:
        shutil.copy2(args.binaries / name, payload / name)
    shutil.copy2(config / "guest-session.sh", payload / "guest-session.sh")
    shutil.copytree(config.parents[1] / "data/theme", payload / "theme")
    all_heads = {f"Virtual-{n}" for n in range(1, 4)}
    with (Path.home() / "OpenBot/Shared/cupertino/gate.lock").open("a") as gate:
        fcntl.flock(gate, fcntl.LOCK_EX)
        with Guest(run_dir, gpus=args.gpus) as guest:
            run = Run(guest, args.out)
            try:
                for gpu in range(1, args.gpus):
                    guest.set_head(gpu, 0, False)
                for head in range(3):
                    guest.set_head(0, head, True)
                    run.settle(lambda: f"Virtual-{head + 1}" in run.connected(), True, 10)
                status, text = guest.sh("mount -o ro /dev/vdb1 /mnt && bash /mnt/guest-session.sh", 60)
                if status != 0:
                    raise RuntimeError(f"guest setup failed: {text}")
                run.settle(run.outputs, all_heads, 60)
                run.check("1-three-heads", all_heads)

                guest.set_head(0, 1, False)
                if args.fail_flip:
                    guest.sh(f"echo '{time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime())}  WARN cupertino_server::backend::"
                             f"udev: queueing the frame failed output={args.fail_flip} error=--fail-flip' >>{SESSION_LOG}")
                run.check("2-unplugged", all_heads - {"Virtual-2"})
                guest.set_head(0, 1, True)
                run.check("3-replugged", all_heads)

                before = len(run.events())
                run.send_keys("ctrl", "alt", "f2")
                paused = run.wait_log("session paused", before)
                guest.set_head(0, 1, False)
                time.sleep(2)
                run.send_keys("ctrl", "alt", "f1")
                resumed = run.wait_log("session resumed", before)
                if not (paused and resumed):
                    run.results.append({"step": "4-vt", "passed": False,
                                        "error": f"session paused={paused} resumed={resumed}"})
                run.check("4-back-from-vt-without-Virtual-2", all_heads - {"Virtual-2"})
                guest.set_head(0, 1, True)
                run.check("5-replugged-after-vt", all_heads)

                guest.sh("systemctl suspend")
                status = lambda: guest.qmp("query-status")["status"]
                asleep = run.settle(status, "suspended", 30) == "suspended"
                guest.set_head(0, 2, False)
                guest.qmp("system_wakeup")
                run.settle(status, "running", 30)
                expected = all_heads - {"Virtual-3"}
                for _ in range(5):
                    guest.reprobe(0, 2, False)
                    if run.settle(run.connected, expected, 2) == expected:
                        break
                if not asleep:
                    run.results.append({"step": "6-suspend", "passed": False, "error": "the guest never suspended"})
                run.check("6-resumed-without-Virtual-3", expected)
                guest.set_head(0, 2, True)
                run.check("7-replugged-after-resume", all_heads)
            except (OSError, RuntimeError, TimeoutError) as error:
                run.results.append({"step": "harness", "passed": False, "error": str(error)})
            finally:
                resets = [event for event in guest.events if event["event"] in ["RESET", "SHUTDOWN", "GUEST_PANICKED"]]
                if resets:
                    run.results.append({"step": "guest", "passed": False, "error": "the guest reset", "events": resets})
                for name, path in [("session.log", SESSION_LOG), ("server.log", SERVER_LOG)]:
                    try:
                        guest.fetch(path, args.out / name)
                    except (OSError, RuntimeError, TimeoutError) as error:
                        (args.out / name).write_text(f"unreadable: {error}\n")
                shutil.copy2(run_dir / "console.log", args.out / "console.log")
                shutil.copy2(run_dir / "command.json", args.out / "qemu-command.json")
                shutil.copy2(run_dir / "qemu-debug.log", args.out / "qemu-debug.log")
                (args.out / "results.json").write_text(json.dumps(run.results, indent=1) + "\n")
    failed = [r["step"] for r in run.results if not r["passed"]]
    print("FAILED: " + ", ".join(failed) if failed else f"PASSED: {len(run.results)} steps", flush=True)
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
