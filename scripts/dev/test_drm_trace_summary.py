"""Tests of the DRM trace summary, on logs written here: `python3 -I scripts/dev/test_drm_trace_summary.py`."""
import contextlib
import importlib.machinery
import importlib.util
import io
import pathlib
import tempfile
import unittest

HERE = pathlib.Path(__file__).resolve().parent
loader = importlib.machinery.SourceFileLoader("summary", str(HERE / "drm-trace-summary"))
spec = importlib.util.spec_from_loader("summary", loader)
summary = importlib.util.module_from_spec(spec)
loader.exec_module(summary)

UDEV = "cupertino_server::backend::udev"
TRACE = "cupertino_server::drm_trace"


def log(late_latch: bool, minutes: float, latencies_us: list[int], pointer_us: list[int], flips: int, missed: int,
        delay_us: int | None = None) -> pathlib.Path:
    """A server log as the session writes it, `minutes` of trace."""
    def at(seconds: float) -> str:
        total = int(seconds * 1e6)
        h, rest = divmod(total, 3_600_000_000)
        m, rest = divmod(rest, 60_000_000)
        s, us = divmod(rest, 1_000_000)
        return f"2026-11-14T{10 + h:02}:{m:02}:{s:02}.{us:06}Z  INFO "

    lines = [
        at(0) + f"{UDEV}: starting the TTY backend version=\"0.1.0\"",
        at(0) + f"{TRACE}: on: presentation-time branch, missed flips, planes follow",
        at(0) + f"{UDEV}: the primary planes' depth asked for primary_plane_bits=8 late_latch={str(late_latch).lower()}",
        at(0.5) + f"{TRACE}: presentation-time output=DP-1 branch=flip-timestamp",
        at(0.5) + f"{TRACE}: planes output=DP-1 direct_scanout=false cursor_plane=true overlays=0",
        at(0.6) + f"{UDEV}: output initialised output=DP-1 width=3840 height=2160",
    ]
    if delay_us is not None:
        lines.append(at(0.7) + f"{TRACE}: start-delay output=DP-1 delay_us={delay_us}")
    for i, latency in enumerate(latencies_us):
        lines.append(at(1 + i) + f"{TRACE}: input-to-flip output=DP-1 latency_us={latency}")
    for i, latency in enumerate(pointer_us):
        lines.append(at(1.5 + i) + f"{TRACE}: pointer-to-flip output=DP-1 latency_us={latency}")
    if missed:
        lines.append(at(2) + f"{TRACE}: missed output=DP-1 late_periods={missed} flips=100 missed_total={missed}")
    lines.append(at(minutes * 60) + f"{TRACE}: count flips={flips} missed_total={missed}")
    path = pathlib.Path(tempfile.mkdtemp()) / "server.log"
    path.write_text("\n".join(lines) + "\n")
    return path


class SummaryTest(unittest.TestCase):
    def test_a_log_gives_its_branch_planes_counts_delays_and_latencies(self):
        s = summary.read(log(True, 61, [20_000, 10_000, 30_000], [8_000, 12_000], flips=216_000, missed=3,
                             delay_us=8333))
        self.assertTrue(s["late_latch"])
        self.assertAlmostEqual(s["seconds"], 61 * 60, delta=1)
        self.assertEqual(s["branches"], {"DP-1": "flip-timestamp"})
        self.assertIn("cursor_plane=true", s["planes"]["DP-1"])
        self.assertEqual((s["flips"], s["missed"]), (216_000, 3))
        self.assertAlmostEqual(s["missed_per_1000"], 3000 / 216_000)
        self.assertEqual(s["delays_us"], [8333])
        self.assertEqual(s["input_p50_ms"], 20.0)
        self.assertEqual(s["input_p99_ms"], 30.0)
        self.assertEqual(s["pointer_p95_ms"], 12.0)
        text = summary.report(s)
        self.assertIn("late start on", text)
        self.assertIn("pointer-to-flip", text)

    def test_lower_latency_and_no_more_missed_refreshes_pass(self):
        base = summary.read(log(False, 60, [25_000, 27_000, 30_000], [24_000], flips=216_000, missed=4))
        run = summary.read(log(True, 60, [15_000, 17_000, 22_000], [14_000], flips=216_000, missed=4))
        self.assertEqual(summary.compare(base, run), (True, []))

    def test_more_missed_refreshes_or_no_lower_latency_fail(self):
        base = summary.read(log(False, 60, [25_000, 27_000, 30_000], [], flips=216_000, missed=4))
        missing = summary.read(log(True, 60, [15_000, 17_000, 22_000], [], flips=216_000, missed=5))
        ok, why = summary.compare(base, missing)
        self.assertFalse(ok)
        self.assertIn("missed refreshes per 1000 flips", why[0])
        slower = summary.read(log(True, 60, [25_000, 28_000, 31_000], [], flips=216_000, missed=0))
        ok, why = summary.compare(base, slower)
        self.assertFalse(ok)
        self.assertTrue(any("p50" in w for w in why), why)

    def test_a_trace_under_an_hour_fails(self):
        base = summary.read(log(False, 60, [25_000], [], flips=216_000, missed=0))
        short = summary.read(log(True, 5, [5_000], [], flips=18_000, missed=0))
        ok, why = summary.compare(base, short)
        self.assertFalse(ok)
        self.assertIn("under an hour", why[0])

    def test_the_command_prints_both_sessions_and_exits_with_the_verdict(self):
        base = log(False, 60, [25_000, 27_000], [], flips=216_000, missed=1)
        run = log(True, 60, [12_000, 14_000], [], flips=216_000, missed=1)
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            status = summary.main(["--baseline", str(base), str(run)])
        self.assertEqual(status, 0)
        self.assertTrue(out.getvalue().rstrip().endswith("pass"))
        self.assertIn("late start off", out.getvalue())
        with contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(summary.main(["--baseline", str(run), str(base)]), 1)
            self.assertEqual(summary.main([str(run)]), 0, "one log: figures only")


if __name__ == "__main__":
    unittest.main()
