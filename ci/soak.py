#!/usr/bin/env python3
"""The daily-driver soak: a browser playing video and scrolling, a terminal, Files, an Electron app and an X11 client run
in a Cupertino session while the server's RSS and open descriptors are sampled every minute, with the frame pacing
of a cupertino-testclient window that draws every frame (the time between its presented frames).

Usage: ci/soak.py [--hours H] [--warmup-minutes M] [--out DIR] [--without x11] [--headless]
  In a terminal of a running session (the hardware run), or with --headless in session.sh's headless desktop.
The run fails when the server's RSS at the end is more than 10% above its RSS after the warm-up (the median of five
samples at each end), when any sample after the warm-up has an open descriptor count more than 5% from the count after
the warm-up, when the server exits, or when a workload program exits. It writes samples.csv (one line a minute, as it
goes) and results.json into DIR and exits 1 on a failure. The frame pacing is reported, not judged.
"""

import argparse
import csv
import json
import os
import shutil
import signal
import statistics
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass, replace
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
ASSETS = ROOT / "ci/soak"
RSS_GROWTH = 0.10
FD_DRIFT = 0.05
EDGE = 5
WORKLOADS = ["browser", "terminal", "files", "electron", "x11", "pacing"]


@dataclass(frozen=True)
class Sample:
    seconds: float
    rss_kib: int
    fds: int
    frame_median_us: float | None = None
    frame_p99_us: float | None = None


def pacing(intervals: list[int]) -> tuple[float | None, float | None]:
    """The median and 99th percentile of a minute's frame intervals, in microseconds."""
    if not intervals:
        return None, None
    ordered = sorted(intervals)
    return statistics.median(ordered), float(ordered[min(len(ordered) - 1, int(len(ordered) * 0.99))])


def drift(values: list[float | None]) -> dict | None:
    """The median of the first five values against the median of the last five."""
    known = [v for v in values if v is not None]
    if len(known) < 2 * EDGE:
        return None
    first, last = statistics.median(known[:EDGE]), statistics.median(known[-EDGE:])
    return {"first_us": first, "last_us": last, "change": round(last / first - 1, 4)}


def judge(samples: list[Sample], warmup: float) -> dict:
    """The verdict on the samples of a run whose warm-up lasted `warmup` seconds."""
    after = [s for s in samples if s.seconds >= warmup]
    if len(after) < 2 * EDGE:
        return {"passed": False, "problems": [f"{len(after)} samples after the warm-up, {2 * EDGE} needed"]}
    rss_base = statistics.median(s.rss_kib for s in after[:EDGE])
    rss_end = statistics.median(s.rss_kib for s in after[-EDGE:])
    fd_base = statistics.median(s.fds for s in after[:EDGE])
    growth = rss_end / rss_base - 1
    worst = max(after, key=lambda s: abs(s.fds - fd_base))
    fd_change = abs(worst.fds - fd_base) / fd_base
    problems = []
    if rss_end > rss_base * (1 + RSS_GROWTH):
        problems.append(f"RSS grew {growth:.1%} after the warm-up ({rss_base:.0f} to {rss_end:.0f} KiB)")
    if abs(worst.fds - fd_base) > fd_base * FD_DRIFT:
        problems.append(f"open descriptors drifted {fd_change:.1%} from {fd_base:.0f} ({worst.fds} at {worst.seconds:.0f} s)")
    return {"passed": not problems, "problems": problems, "rss_base_kib": rss_base, "rss_end_kib": rss_end,
            "rss_growth": round(growth, 4), "rss_max_kib": max(s.rss_kib for s in after), "fd_base": fd_base,
            "fd_drift": round(fd_change, 4), "frame_median": drift([s.frame_median_us for s in after]),
            "frame_p99": drift([s.frame_p99_us for s in after])}


def sample(pid: int, start: float) -> Sample | None:
    try:
        status = Path(f"/proc/{pid}/status").read_text()
        fds = len(os.listdir(f"/proc/{pid}/fd"))
    except (FileNotFoundError, ProcessLookupError):
        return None
    rss = next(int(line.split()[1]) for line in status.splitlines() if line.startswith("VmRSS:"))
    return Sample(round(time.monotonic() - start, 1), rss, fds)


def exe(pid: int) -> str:
    try:
        return os.path.basename(os.readlink(f"/proc/{pid}/exe"))
    except OSError:
        return ""


def server_pid() -> int:
    """The cupertino-server this process runs under, else the only one of this user."""
    pid = os.getppid()
    while pid > 1:
        if exe(pid) == "cupertino-server":
            return pid
        stat = Path(f"/proc/{pid}/stat").read_text()
        pid = int(stat.rsplit(")", 1)[1].split()[1])
    uid = os.getuid()
    found = [int(p) for p in os.listdir("/proc") if p.isdigit() and exe(int(p)) == "cupertino-server"
             and os.stat(f"/proc/{p}").st_uid == uid]
    if len(found) != 1:
        sys.exit(f"soak.py: {len(found)} cupertino-server processes of this user, not one; pass --server-pid")
    return found[0]


def program(name: str, bin_dir: Path) -> str | None:
    local = bin_dir / name
    return str(local) if os.access(local, os.X_OK) else shutil.which(name)


def workloads(scratch: Path, bin_dir: Path, without: set[str], frames: Path) -> tuple[dict[str, list[str]], list[str]]:
    """The command of each workload, and what is missing for the ones not left out."""
    page = scratch / "page.html"
    shutil.copy2(ASSETS / "page.html", page)
    shutil.copytree(ASSETS / "electron", scratch / "electron")
    wayland = ["--ozone-platform=wayland", "--no-first-run", "--no-default-browser-check", "--password-store=basic",
               "--autoplay-policy=no-user-gesture-required"]
    found = {name: program(name, bin_dir) for name in
             ["google-chrome-stable", "foot", "cupertino-files", "electron43", "glxgears", "xwayland-satellite",
              "cupertino-testclient"]}
    commands = {
        "browser": [found["google-chrome-stable"], *wayland, f"--user-data-dir={scratch / 'chrome'}", page.as_uri()],
        "terminal": [found["foot"], "sh", "-c", "while :; do date; ls -l /usr/bin | tail -n 40; sleep 0.2; done"],
        "files": [found["cupertino-files"]],
        "electron": [found["electron43"], *wayland, f"--user-data-dir={scratch / 'electron-data'}",
                     str(scratch / "electron"), page.as_uri()],
        "x11": [found["glxgears"]],
        "pacing": [found["cupertino-testclient"], "--app-id", "org.cupertino.SoakPacing", "--title", "Pacing",
                   "--output", os.devnull, "--intervals", str(frames)],
    }
    needs = {"browser": ["google-chrome-stable"], "terminal": ["foot"], "files": ["cupertino-files"],
             "electron": ["electron43"], "x11": ["glxgears", "xwayland-satellite"],
             "pacing": ["cupertino-testclient"]}
    missing = [f"{name} needs {need}" for name in commands if name not in without
               for need in needs[name] if found[need] is None]
    if "x11" not in without and not os.environ.get("DISPLAY"):
        missing.append("x11 needs DISPLAY (the server sets it when xwayland-satellite is installed)")
    return {name: command for name, command in commands.items() if name not in without}, missing


def video(scratch: Path) -> None:
    subprocess.run(["ffmpeg", "-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i",
                    "testsrc2=size=1280x720:rate=30", "-t", "20", "-c:v", "libvpx-vp9", "-deadline", "realtime",
                    "-cpu-used", "8", "-b:v", "2M", str(scratch / "video.webm")], check=True)


def say(text: str) -> None:
    try:
        print(text, flush=True)
    except OSError:
        pass


def soak(args: argparse.Namespace) -> int:
    out = args.out.resolve()
    out.mkdir(parents=True, exist_ok=True)
    signal.signal(signal.SIGHUP, signal.SIG_IGN)
    pid = args.server_pid or server_pid()
    scratch = Path(tempfile.mkdtemp(prefix="cupertino-soak."))
    frames = out / "frames.txt"
    frames.touch()
    commands, missing = workloads(scratch, args.bin_dir, set(args.without), frames)
    if missing:
        sys.exit("soak.py: " + "; ".join(missing) + " (--without NAME leaves a workload out)")
    video(scratch)
    log = (out / "workloads.log").open("w")
    running = {name: subprocess.Popen(command, stdout=log, stderr=subprocess.STDOUT, cwd=scratch,
                                      start_new_session=True)
               for name, command in commands.items()}
    say(f"soak: server {pid}, {', '.join(running)} started; {args.hours} h, samples in {out / 'samples.csv'}")
    start = time.monotonic()
    end = start + args.hours * 3600
    samples: list[Sample] = []
    events: list[str] = []
    with (out / "samples.csv").open("w", newline="") as file, frames.open("rb") as intervals:
        writer = csv.writer(file)
        writer.writerow(["seconds", "rss_kib", "fds", "frame_median_us", "frame_p99_us"])
        while True:
            now = sample(pid, start)
            if now is None:
                events.append(f"the server exited after {time.monotonic() - start:.0f} s")
                break
            lines = intervals.read().split(b"\n")
            intervals.seek(-len(lines[-1]), os.SEEK_CUR)
            now = replace(now, **dict(zip(["frame_median_us", "frame_p99_us"], pacing([int(v) for v in lines[:-1]]))))
            samples.append(now)
            writer.writerow([now.seconds, now.rss_kib, now.fds, now.frame_median_us, now.frame_p99_us])
            file.flush()
            for name, process in running.items():
                if process.poll() is not None and not any(e.startswith(f"{name} ") for e in events):
                    events.append(f"{name} exited with {process.returncode} after {now.seconds:.0f} s")
            if time.monotonic() >= end:
                break
            time.sleep(max(0.0, min(args.interval, end - time.monotonic())))
    for process in running.values():
        try:
            os.killpg(process.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
    shutil.rmtree(scratch, ignore_errors=True)
    verdict = judge(samples, args.warmup_minutes * 60)
    verdict["problems"] = events + verdict["problems"]
    verdict["passed"] = not verdict["problems"]
    result = {**verdict, "complete": not args.without, "without": args.without, "hours": args.hours,
              "warmup_minutes": args.warmup_minutes, "samples": len(samples), "server_pid": pid,
              "workloads": {name: commands[name] for name in running}}
    (out / "results.json").write_text(json.dumps(result, indent=1) + "\n")
    say(("PASSED" if result["passed"] else "FAILED: " + "; ".join(result["problems"])) + f" ({out / 'results.json'})")
    return 0 if result["passed"] else 1


def headless(args: argparse.Namespace) -> int:
    """The same run as a client of session.sh's headless desktop, which ends once the inner run has written results."""
    out = args.out.resolve()
    bin_dir = args.bin_dir.resolve()
    out.mkdir(parents=True, exist_ok=True)
    (out / "results.json").unlink(missing_ok=True)
    inner = [sys.executable, "-I", str(Path(__file__).resolve()), "--out", str(out), "--bin-dir", str(bin_dir),
             "--hours", str(args.hours), "--warmup-minutes", str(args.warmup_minutes), "--interval", str(args.interval),
             *[a for name in args.without for a in ("--without", name)]]
    if any(" " in a for a in inner):
        sys.exit("soak.py: --headless needs paths without spaces (the server splits --spawn at whitespace)")
    env = {**os.environ, "BIN_DIR": str(bin_dir), "SIZE": "1920x1080"}
    with (out / "server.log").open("w") as log:
        session = subprocess.Popen([str(ROOT / "crates/cupertino-server/scripts/session.sh"),
                                    "--run-for", str(int(args.hours * 3600) + 600), "--spawn", " ".join(inner),
                                    "--shot", f"180:{out / 'desktop.png'}"],
                                   stdout=log, stderr=subprocess.STDOUT, env=env, start_new_session=True)
        while session.poll() is None and not (out / "results.json").exists():
            time.sleep(5)
        status = session.poll()
        if status is None:
            time.sleep(2)
            os.killpg(session.pid, signal.SIGTERM)
            session.wait()
    if not (out / "results.json").exists():
        say(f"FAILED: the headless session exited with {status} before the soak ended ({out / 'server.log'})")
        return 1
    result = json.loads((out / "results.json").read_text())
    return 0 if result["passed"] else 1


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--hours", type=float, default=8)
    parser.add_argument("--warmup-minutes", type=float, default=60)
    parser.add_argument("--interval", type=float, default=60, help="seconds between samples")
    parser.add_argument("--out", type=Path, default=Path(f"soak-{time.strftime('%Y%m%d-%H%M%S')}"))
    parser.add_argument("--without", action="append", default=[], choices=WORKLOADS)
    parser.add_argument("--bin-dir", type=Path, default=Path(os.environ.get("BIN_DIR", ROOT / "target/release")),
                        help="the release binaries (default $BIN_DIR, else target/release); cupertino-files is looked "
                             "for here before PATH")
    parser.add_argument("--server-pid", type=int)
    parser.add_argument("--headless", action="store_true", help="run in session.sh's headless desktop")
    args = parser.parse_args()
    if args.hours * 60 <= args.warmup_minutes:
        parser.error("--hours must be longer than the warm-up")
    return headless(args) if args.headless else soak(args)


if __name__ == "__main__":
    sys.exit(main())
