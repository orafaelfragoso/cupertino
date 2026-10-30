"""The demo video's script: the server flags that drive the desktop through its shots (ci/demo-video.sh runs it).

Every input is a real input of the server's scripted path (`--pointer`, `--click`, `--press`, `--release`, `--key`,
`--type`, `--action`); a pointer move is a glide of one position per video frame on an ease-in-out curve. Times are
seconds of the recording; the recording starts `LEAD` seconds after the server, once the shell processes have drawn.
Coordinates are points of a 1280x720 output (2560x1440 at scale 2).

Usage: python3 -I ci/demo_video.py [--frames DIR]   (one flag or value per line; --frames adds a PNG of each shot's
look-check moment)
"""
import sys

LEAD = 2.5
FPS = 60
DOCK_FILES = (581, 684)
DOCK_SETTINGS = (633, 684)


class Script:
    def __init__(self, frames: str | None):
        self.frames = frames
        self.t = 0.0
        self.at = (640.0, 400.0)
        self.flags: list[str] = ["--pointer", self.point(-1.0, self.at)]
        if not frames:
            self.flags += ["--record-after", f"{LEAD}"]
        self.shots: list[str] = []

    def point(self, t: float, xy: tuple[float, float]) -> str:
        return f"{LEAD + t:.4f}:{xy[0]:.1f},{xy[1]:.1f}"

    def flag(self, name: str, value: str) -> None:
        self.flags += [name, value]

    def wait(self, seconds: float) -> None:
        self.t += seconds

    def glide(self, x: float, y: float, seconds: float = 0.5) -> None:
        (x0, y0), steps = self.at, max(1, round(seconds * FPS))
        for i in range(1, steps + 1):
            u = i / steps
            e = 4 * u**3 if u < 0.5 else 1 - (-2 * u + 2) ** 3 / 2
            self.flag("--pointer", self.point(self.t + seconds * u, (x0 + (x - x0) * e, y0 + (y - y0) * e)))
        self.at = (x, y)
        self.t += seconds

    def click(self) -> None:
        self.flag("--click", self.point(self.t, self.at))
        self.t += 0.15

    def press(self) -> None:
        self.flag("--press", self.point(self.t, self.at))

    def release(self) -> None:
        self.flag("--release", self.point(self.t, self.at))

    def key(self, chord: str) -> None:
        self.flag("--key", f"{LEAD + self.t:.4f}:{chord}")
        self.t += 0.1

    def type(self, text: str, per_key: float = 0.12) -> None:
        for c in text:
            self.flag("--type", f"{LEAD + self.t:.4f}:{c}")
            self.t += per_key

    def modifier(self, name: str, down: bool) -> None:
        self.flag("--modifier-down" if down else "--modifier-up", f"{LEAD + self.t:.4f}:{name}")

    def action(self, name: str) -> None:
        self.flag("--action", f"{LEAD + self.t:.4f}:{name}")

    def shot(self, name: str) -> None:
        if self.frames:
            self.flag("--shot", f"{LEAD + self.t:.4f}:{self.frames}/{name}.png")

    def end(self) -> list[str]:
        return self.flags + ["--run-for", f"{LEAD + self.t:.4f}"]


def demo(s: Script) -> None:
    # 1 the desktop at rest; the pointer drifts to the dock
    s.wait(3.5)
    s.shot("01-desktop")
    s.glide(*DOCK_FILES, 0.6)
    s.wait(1.5)

    # 2 Files from the dock, then Documents in the sidebar
    s.click()
    s.wait(2.8)
    s.shot("02-files-home")
    s.glide(250, 250)
    s.click()
    s.wait(2.2)
    s.shot("03-files-documents")

    # 3 a file dragged onto a sidebar place, then list and icon views
    s.glide(440, 166)
    s.wait(0.3)
    s.press()
    s.glide(300, 210, 0.5)
    s.glide(243, 222, 0.4)
    s.wait(0.2)
    s.shot("04-files-drop-target")
    s.wait(0.6)
    s.release()
    s.wait(1.2)
    s.key("Super+2")
    s.wait(2.2)
    s.shot("05-files-list")
    s.key("Super+1")
    s.wait(1.8)

    # 4 the system menu and About This Computer
    s.glide(19, 11, 0.6)
    s.click()
    s.wait(0.6)
    s.glide(60, 70, 0.4)
    s.glide(80, 40, 0.4)
    s.wait(0.5)
    s.shot("06-system-menu")
    s.click()
    s.wait(3.0)
    s.shot("07-about")
    s.key("Super+w")
    s.wait(0.8)

    # 5 Settings from the dock, Appearance to Dark
    s.glide(*DOCK_SETTINGS, 0.6)
    s.click()
    s.wait(2.0)
    s.glide(360, 281)
    s.click()
    s.wait(1.2)
    s.shot("08-appearance-light")
    s.glide(930, 114, 0.6)
    s.click()
    s.wait(3.0)
    s.shot("09-appearance-dark")

    # 6 Accessibility, then the Wallpaper pane
    s.glide(360, 253)
    s.click()
    s.wait(2.8)
    s.shot("10-accessibility")
    s.glide(354, 393)
    s.click()
    s.wait(2.8)
    s.shot("11-wallpaper")

    # 7 the Kit Forms example: a name typed, Remember me switched on
    s.key("Super+k")
    s.wait(2.0)
    s.glide(640, 215)
    s.click()
    s.type("Avery")
    s.wait(0.4)
    s.glide(636, 279)
    s.click()
    s.wait(2.2)
    s.shot("12-forms")

    # 8 the app switcher across the three applications, released on Files
    s.modifier("super", True)
    s.key("Super+Tab")
    s.wait(1.1)
    s.key("Super+Tab")
    s.wait(1.1)
    s.shot("13-switcher")
    s.wait(0.8)
    s.modifier("super", False)
    s.wait(2.0)

    # 9 the overview; Settings picked
    s.action("overview")
    s.wait(2.4)
    s.shot("14-overview")
    s.glide(520, 490, 0.6)
    s.wait(0.6)
    s.click()
    s.wait(2.2)

    # 10 the launcher, "fil" typed, Escape; the dark desktop as the end card
    s.action("launcher")
    s.wait(1.0)
    s.type("files", 0.18)
    s.wait(1.8)
    s.shot("15-launcher")
    s.key("Escape")
    s.wait(0.3)
    s.key("Escape")
    s.wait(0.8)
    s.action("show-desktop")
    s.glide(1180, 600, 0.6)
    s.wait(2.6)
    s.shot("16-end")
    s.wait(0.4)


def main(args: list[str]) -> int:
    frames = args[1] if args[:1] == ["--frames"] and len(args) == 2 else None
    if args and frames is None:
        print(__doc__.strip().splitlines()[-2], file=sys.stderr)
        return 2
    s = Script(frames)
    demo(s)
    print("\n".join(s.end()))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
