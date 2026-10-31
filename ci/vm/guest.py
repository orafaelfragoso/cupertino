"""A disposable QEMU guest: its root console, QMP, and the heads of its virtio-gpu devices over a private D-Bus."""

import json
import os
from pathlib import Path
import re
import socket
import subprocess
import threading
import time

ROOT = Path.home() / ".cache/cupertino-vm"
IMAGE = ROOT / "output/guest"
GIB = 1024**3
ESCAPES = re.compile(rb"\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)|\x1b\[[0-?]*[ -/]*[@-~]|\x1b[@-_]|\r")


def check_space() -> None:
    free = os.statvfs(ROOT).f_bavail * os.statvfs(ROOT).f_frsize
    if free < 30 * GIB:
        raise SystemExit(f"/home has {free / GIB:.1f} GiB free, under the 30 GiB floor")


class Guest:
    """Boots the image with `gpus` virtio-gpu devices of `heads` outputs each; every head starts at `size`. QEMU allows
    one virgl device, so with `gl` only the first renders and the others are display-only."""

    def __init__(self, run: Path, gpus: int = 1, heads: int = 3, size: tuple[int, int] = (1280, 800), gl: bool = True):
        self.run, self.gpus, self.heads, self.size, self.gl = run, gpus, heads, size, gl
        run.mkdir(parents=True, exist_ok=True)
        for name in ["console.sock", "files.sock", "qmp.sock", "bus"]:
            (run / name).unlink(missing_ok=True)
        self.output = bytearray()
        self.lock = threading.Lock()
        self.marks = 0
        self.probes = 0
        self.events: list[dict] = []

    def __enter__(self) -> "Guest":
        check_space()
        subprocess.run(["cp", "--reflink=always", f"{IMAGE}.raw", self.run / "boot.raw"], check=True)
        try:
            self.start()
        except BaseException:
            self.__exit__()
            raise
        return self

    def start(self) -> None:
        self.bus = f"unix:path={self.run / 'bus'}"
        self.dbus = subprocess.Popen(["dbus-daemon", "--session", "--nofork", f"--address={self.bus}"],
                                     stdout=subprocess.DEVNULL)
        self.wait_for(lambda: (self.run / "bus").exists(), 10, "the private D-Bus")
        devices = []
        for gpu in range(self.gpus):
            model = "virtio-gpu-gl-pci" if self.gl and gpu == 0 else "virtio-gpu-pci"
            devices += ["-device", f"pcie-root-port,id=port{gpu},chassis={gpu + 1}",
                        "-device", f"{model},id=gpu{gpu},bus=port{gpu},max_outputs={self.heads},"
                        f"xres={self.size[0]},yres={self.size[1]},x-pcie-pm-no-soft-reset=on"]
        command = [
            "qemu-system-x86_64", "-enable-kvm", "-M", "q35", "-vga", "none", "-m", "2048", "-smp", "2",
            "-global", "ICH9-LPC.disable_s3=0",
            "-display", "dbus,gl=on,rendernode=/dev/dri/renderD128" if self.gl else "dbus", *devices,
            "-kernel", f"{IMAGE}.vmlinuz", "-initrd", f"{IMAGE}.initrd",
            "-append", "root=LABEL=root-x86-64 rw console=hvc0 systemd.mask=systemd-firstboot.service",
            "-drive", f"file={self.run / 'boot.raw'},format=raw,if=virtio",
            "-drive", f"file=fat:ro:{self.run / 'payload'},format=raw,if=virtio,readonly=on",
            "-device", "virtio-serial",
            "-chardev", f"socket,id=console,path={self.run / 'console.sock'},server=on,wait=off",
            "-device", "virtconsole,chardev=console",
            "-chardev", f"socket,id=files,path={self.run / 'files.sock'},server=on,wait=off",
            "-device", "virtserialport,chardev=files,name=files",
            "-qmp", f"unix:{self.run / 'qmp.sock'},server=on,wait=off", "-serial", "none", "-no-reboot",
            "-d", "cpu_reset,guest_errors", "-D", str(self.run / "qemu-debug.log"),
        ]
        (self.run / "command.json").write_text(json.dumps(command, indent=1) + "\n")
        self.qemu = subprocess.Popen(command, env={**os.environ, "DBUS_SESSION_BUS_ADDRESS": self.bus},
                                     stdout=(self.run / "qemu.log").open("w"), stderr=subprocess.STDOUT)
        self.wait_for(lambda: (self.run / "qmp.sock").exists(), 30, "QEMU")
        self.console = socket.socket(socket.AF_UNIX)
        self.console.connect(str(self.run / "console.sock"))
        threading.Thread(target=self.read_console, daemon=True).start()
        qmp = socket.socket(socket.AF_UNIX)
        qmp.connect(str(self.run / "qmp.sock"))
        self.qmp_file = qmp.makefile("rw")
        self.qmp_file.readline()
        self.qmp("qmp_capabilities")
        self.files = socket.socket(socket.AF_UNIX)
        self.files.connect(str(self.run / "files.sock"))
        self.wait_for(lambda: b"]# " in self.output, 180, "the root console")

    def __exit__(self, *_) -> None:
        for process in [getattr(self, "qemu", None), getattr(self, "dbus", None)]:
            if process:
                process.terminate()
                process.wait(timeout=30)
        (self.run / "boot.raw").unlink(missing_ok=True)

    def read_console(self) -> None:
        pending = b""
        with (self.run / "console.log").open("wb") as log:
            while chunk := self.console.recv(65536):
                log.write(chunk)
                log.flush()
                pending += chunk
                cut = pending.rfind(b"\x1b")
                ready, pending = (pending[:cut], pending[cut:]) if cut != -1 and not ESCAPES.match(pending, cut) \
                    else (pending, b"")
                with self.lock:
                    self.output += ESCAPES.sub(b"", ready)

    def wait_for(self, ready, seconds: float, what: str) -> None:
        deadline = time.monotonic() + seconds
        while not ready():
            if getattr(self, "qemu", None) and self.qemu.poll() is not None:
                raise RuntimeError(f"QEMU exited ({self.qemu.returncode}) waiting for {what}")
            if time.monotonic() > deadline:
                raise TimeoutError(f"{what} did not come within {seconds} s")
            time.sleep(0.1)

    def sh(self, command: str, seconds: float = 30) -> tuple[int, str]:
        """Runs `command` on the root console and returns its status and output."""
        self.marks += 1
        mark = f"__done{self.marks}__"
        with self.lock:
            start = len(self.output)
        self.console.sendall(f"{command}; echo {mark}=$?\n".encode())
        pattern = re.compile(rf"^{mark}=(\d+)$".encode(), re.M)
        self.wait_for(lambda: pattern.search(self.output, start), seconds, f"`{command}`")
        with self.lock:
            found = pattern.search(self.output, start)
            text = self.output[start:found.start()].decode(errors="replace")
        return int(found.group(1)), text.split("\n", 1)[1] if "\n" in text else ""

    def fetch(self, path: str, destination: Path, seconds: float = 30) -> None:
        """Copies the guest's file at `path` to `destination` over a virtio-serial port of its own: the console is a tty
        and loses long output, and virtio-9p does not survive S3."""
        status, text = self.sh(f"stat -c size=%s '{path}'")
        if status != 0 or not (found := re.search(r"^size=(\d+)$", text, re.M)):
            raise RuntimeError(f"reading {path}: {text}")
        size = int(found.group(1))
        self.console.sendall(f"cat '{path}' >/dev/virtio-ports/files\n".encode())
        data = bytearray()
        self.files.settimeout(seconds)
        while len(data) < size:
            if not (chunk := self.files.recv(min(65536, size - len(data)))):
                raise RuntimeError(f"the file port closed after {len(data)} of {size} bytes of {path}")
            data += chunk
        destination.write_bytes(data)

    def qmp(self, command: str, **arguments) -> dict:
        self.qmp_file.write(json.dumps({"execute": command, "arguments": arguments}) + "\n")
        self.qmp_file.flush()
        while True:
            if not (line := self.qmp_file.readline()):
                raise RuntimeError(f"QMP {command}: QEMU closed the socket")
            reply = json.loads(line)
            if "event" in reply:
                self.events.append(reply)
                continue
            if "error" in reply:
                raise RuntimeError(f"QMP {command}: {reply['error']['desc']}")
            return reply["return"]

    def console_path(self, gpu: int, head: int) -> str:
        return f"/org/qemu/Display1/Console_{gpu * self.heads + head}"

    def set_head(self, gpu: int, head: int, connected: bool, millimetres: int = 300) -> None:
        """Connects or disconnects a head: virtio-gpu reports a head with a size of 0x0 as disconnected."""
        width, height = self.size if connected else (0, 0)
        subprocess.run(["busctl", f"--address={self.bus}", "call", "org.qemu", self.console_path(gpu, head),
                        "org.qemu.Display1.Console", "SetUIInfo", "qqiiuu", str(millimetres), "190", "0", "0",
                        str(width), str(height)], check=True)

    def reprobe(self, gpu: int, head: int, connected: bool) -> None:
        """Sends a display event without changing what `head` shows, so the guest reads the heads of `gpu` again:
        virtio-gpu drops the event of a head changed while the guest slept. The physical width alternates, since QEMU
        sends no event for an unchanged head; the event of a connected head re-initialises it."""
        self.probes += 1
        self.set_head(gpu, head, connected, 300 + self.probes % 2)
