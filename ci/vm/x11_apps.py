#!/usr/bin/env python3
"""Runs a Cupertino session at scale 2 in a disposable guest with an X11 program (xterm, through xwayland-satellite)
and a Wayland test window, and checks that X11 programs get the session's scale (the XSETTINGS Xft/DPI and
Gdk/WindowScalingFactor and the Xft.dpi resource, and an xterm twice its size at scale 1) and that the clipboard goes
from the Wayland window to X11 and from X11 to the Wayland window. Writes a screenshot, the logs and a results file to
--out; exits 1 when a check fails. Build the image first: `ci/vm/build-image.sh build`.

    ci/vm/x11_apps.py --binaries "$CARGO_TARGET_DIR/debug" --out /tmp/x11-apps

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
from daily_driver import SERVER_LOG, SESSION_LOG, Run  # noqa: E402
from guest import ROOT, Guest  # noqa: E402

FROM_WAYLAND = "copied-in-wayland"
FROM_X11 = "copied-in-x11"
CLIP_INPUT = "/home/qa/clip.input"
CLIP_CLIPBOARD = "/home/qa/clip.clipboard"
CLIP = (f"/usr/bin/cupertino-testclient --app-id qa-clip --title clip --output {CLIP_INPUT} --copy {FROM_WAYLAND} "
        f"--clipboard {CLIP_CLIPBOARD}")
# DejaVu Sans Mono at 11 pt is 7 pixels a column at 96 dpi; 80 columns at 192 dpi are well over 1000 pixels
# no shell in it, so the title stays
TERM = "xterm -title x11-term -fa 'DejaVu Sans Mono' -fs 11 -geometry 80x24 -e sleep infinity"
MIN_TERM_WIDTH = 1000
SWITCHES = 12


class X11Run(Run):
    def x11(self, command: str, seconds: float = 30) -> tuple[int, str]:
        """Runs `command` as qa on the session's X display. Its output is ended with a newline, as the shell's marker needs,
        and its status kept."""
        return self.guest.sh("a=$(ls /run/user/$(id -u qa)/cupertino-xauth-* | head -n 1); "
                             f"runuser -u qa -- env DISPLAY=:${{a##*-}} XAUTHORITY=$a {command}; s=$?; echo; (exit $s)",
                             seconds)

    def file(self, path: str) -> str:
        return self.guest.sh(f"cat {path} 2>/dev/null; echo")[1].strip()

    def switch(self, taps: int) -> None:
        """Holds the primary key over `taps` presses of Tab: the app switcher goes to the `taps`-th recent app."""
        key = lambda name, down: {"type": "key", "data": {"down": down, "key": {"type": "qcode", "data": name}}}
        events = [key("meta_l", True)]
        for _ in range(taps):
            events += [key("tab", True), key("tab", False)]
        self.guest.qmp("input-send-event", events=events)
        time.sleep(0.3)
        self.guest.qmp("input-send-event", events=[key("meta_l", False)])
        time.sleep(0.5)

    def focus_until(self, reached, before=lambda: None) -> bool:
        """Switches apps, pressing z in each, until `reached()`."""
        for attempt in range(SWITCHES):
            self.switch(1 + attempt % 4)
            self.send_keys("z")
            before()
            if self.settle(reached, True, 2):
                return True
        return False

    def record(self, step: str, passed: bool, **details) -> None:
        result = {"step": step, "passed": passed, **details}
        self.results.append(result)
        print(json.dumps(result), flush=True)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binaries", type=Path, required=True, help="directory with cupertino-server, "
                        "cupertino-testclient and cupertino-screenshot")
    parser.add_argument("--out", type=Path, required=True)
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
    with (Path.home() / "OpenBot/Shared/cupertino/gate.lock").open("a") as gate:
        fcntl.flock(gate, fcntl.LOCK_EX)
        with Guest(run_dir) as guest:
            run = X11Run(guest, args.out)
            try:
                guest.set_head(0, 0, True)
                run.settle(lambda: "Virtual-1" in run.connected(), True, 10)
                status, text = guest.sh(f"mount -o ro /dev/vdb1 /mnt && bash /mnt/guest-session.sh --scale 2 "
                                        f"--spawn '{CLIP}'", 60)
                if status != 0:
                    raise RuntimeError(f"guest setup failed: {text}")
                run.settle(run.outputs, {"Virtual-1"}, 60)
                run.settle(lambda: guest.sh(f"[ -e {CLIP_INPUT} ]")[0] == 0, True, 30)

                # the first X11 connection starts the satellite, which publishes the scale once it has the outputs
                dpi = lambda: re.search(r"^Xft\.dpi:\s*(\d+)$", run.x11("xrdb -query")[1], re.M)
                found = run.settle(lambda: (m := dpi()) and m.group(1), "192", 30)
                settings = run.x11("dump_xsettings")[1]
                xft = re.search(r"^Xft/DPI\s+(\d+)", settings, re.M)
                gdk = re.search(r"^Gdk/WindowScalingFactor\s+(\d+)", settings, re.M)
                run.record("1-x11-scale", found == "192" and bool(xft) and xft.group(1) == str(192 * 1024)
                           and bool(gdk) and gdk.group(1) == "2", xft_dpi_resource=found,
                           xsettings=settings.strip().splitlines()[-8:])

                run.x11(f"setsid -f {TERM} >/dev/null 2>&1")
                width = lambda: (m := re.search(r"Width:\s*(\d+)", run.x11("xwininfo -name x11-term")[1])) and \
                    int(m.group(1))
                term_width = run.settle(lambda: (w := width()) and w > MIN_TERM_WIDTH, True, 30) and width()
                run.record("2-x11-window-at-scale", bool(term_width), xterm_width=term_width,
                           screenshots=run.screenshots("2-x11-window-at-scale"))

                clip_focused = run.focus_until(lambda: "z" in run.file(CLIP_INPUT))
                pasted = lambda: run.x11("timeout 2 xclip -o -selection clipboard", 5)[1].strip() == FROM_WAYLAND
                term_focused = run.focus_until(pasted)
                run.record("3-wayland-to-x11", clip_focused and term_focused, copied=clip_focused,
                           x11_clipboard=run.x11("timeout 2 xclip -o -selection clipboard", 5)[1].strip())

                run.send_keys("z")
                run.x11(f"sh -c 'printf %s {FROM_X11} | xclip -selection clipboard -i'")
                received = run.focus_until(lambda: run.file(CLIP_CLIPBOARD) == FROM_X11)
                run.record("4-x11-to-wayland", received, wayland_clipboard=run.file(CLIP_CLIPBOARD),
                           screenshots=run.screenshots("4-x11-to-wayland"))
            except (OSError, RuntimeError, TimeoutError) as error:
                run.results.append({"step": "harness", "passed": False, "error": str(error)})
            finally:
                for name, path in [("session.log", SESSION_LOG), ("server.log", SERVER_LOG)]:
                    try:
                        guest.fetch(path, args.out / name)
                    except (OSError, RuntimeError, TimeoutError) as error:
                        (args.out / name).write_text(f"unreadable: {error}\n")
                shutil.copy2(run_dir / "console.log", args.out / "console.log")
    (args.out / "results.json").write_text(json.dumps(run.results, indent=2) + "\n")
    failed = [r["step"] for r in run.results if not r["passed"]]
    if failed:
        print("FAILED: " + ", ".join(failed))
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
