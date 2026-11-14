"""Tests of the bench's scripts and readings, without a session: `python3 -I scripts/dev/test_bench.py`."""
import importlib.machinery
import importlib.util
import json
import pathlib
import tempfile
import unittest

HERE = pathlib.Path(__file__).resolve().parent
loader = importlib.machinery.SourceFileLoader("bench", str(HERE / "bench"))
spec = importlib.util.spec_from_loader("bench", loader)
bench = importlib.util.module_from_spec(spec)
loader.exec_module(bench)

WINDOWS = {
    "files": {"app": "cupertino-files", "title": "home", "x": 180.0, "y": 98.0, "width": 920.0, "height": 436.0},
    "settings": {"app": "cupertino-settings", "title": "Account", "x": 270.0, "y": 33.0, "width": 740.0,
                 "height": 693.0, "buttons": [[296.0, 59.0], [336.0, 59.0]]},
}


def times(flags: list[str], name: str) -> list[float]:
    return [float(flags[i + 1].split(":")[0]) for i, f in enumerate(flags) if f == name]


class ScriptTest(unittest.TestCase):
    def test_every_scenario_acts_inside_the_session_after_it_settled_and_its_span_holds_its_input(self):
        for name in bench.BASELINE + bench.EXTRA:
            flags, (a, b) = bench.script(name, 60, WINDOWS, (1280, 800), 1.0, 15)
            self.assertGreaterEqual(a, bench.SETTLE, name)
            self.assertGreater(b, a, name)
            steps = [t for f in ("--pointer", "--pointer-on", "--press", "--release", "--click", "--action", "--key", "--type",
                                 "--scroll", "--right-click") for t in times(flags, f)]
            if name == "idle":
                self.assertEqual(steps, [], "an idle desktop gets no input")
                self.assertEqual(b - a, 15)
                continue
            self.assertTrue(steps, name)
            self.assertGreaterEqual(min(steps), bench.SETTLE, name)
            self.assertLessEqual(max(steps), b + 0.5, name)

    def test_the_drag_holds_settings_by_its_title_bar_and_lets_go_where_it_began(self):
        flags, _ = bench.script("drag", 60, WINDOWS, (1280, 800), 1.0, 15)
        press = flags[flags.index("--press") + 1].split(":")[1]
        release = flags[flags.index("--release") + 1].split(":")[1]
        self.assertEqual(press, release)
        x, y = (float(v) for v in press.split(","))
        s = WINDOWS["settings"]
        self.assertTrue(s["x"] < x < s["x"] + s["width"] and s["y"] < y < s["y"] + 28)

    def test_the_hover_crosses_every_window_button_of_settings_wherever_settings_is(self):
        flags, _ = bench.script("hover", 60, WINDOWS, (1280, 800), 1.0, 15)
        on = [flags[i + 1].split(":") for i, f in enumerate(flags) if f == "--pointer-on"]
        self.assertTrue(on and all(window == "settings" for _, window, _ in on))
        s = WINDOWS["settings"]
        # points of Settings' frame, as the layout session placed it
        points = [tuple(float(v) for v in p.split(",")) for _, _, p in on]
        points = [(s["x"] + x, s["y"] + y) for x, y in points]
        (close_x, row), (zoom_x, _) = s["buttons"]
        self.assertTrue(all(y == row for _, y in points))
        xs = [x for x, _ in points]
        self.assertLess(min(xs), close_x - 6)
        self.assertGreater(max(xs), zoom_x + 6)

    def test_a_point_files_shows_is_one_settings_does_not_cover(self):
        x, y = bench.visible_point(WINDOWS["files"], WINDOWS["settings"])
        self.assertTrue(bench.inside(WINDOWS["files"], x, y))
        self.assertFalse(bench.inside(WINDOWS["settings"], x, y))
        # where Settings covers Files' middle and right, the click lands below the sidebar's rows, not on one of them
        f = WINDOWS["files"]
        self.assertGreater(y, f["y"] + f["height"] - 30)

    def test_the_full_screen_files_scenarios_measure_only_after_files_fills_the_output(self):
        for name in ("files-full-repaint", "files-full-scroll"):
            flags, (a, b) = bench.script(name, 60, WINDOWS, (1280, 800), 1.0, 15)
            actions = [flags[i + 1].split(":") for i, f in enumerate(flags) if f == "--action"]
            self.assertEqual(actions[0][1], "toggle-fullscreen", name)
            self.assertLessEqual(float(actions[0][0]) + bench.FULL_SCREEN_SETTLE, a, name)
            if name == "files-full-repaint":
                self.assertEqual([what for _, what in actions[1:]], ["toggle-appearance"] * 3)
                self.assertTrue(all(a <= float(t) <= b for t, _ in actions[1:]))
            else:
                points = {flags[i + 1].split(":")[1] for i, f in enumerate(flags) if f == "--scroll"}
                self.assertEqual(points, {"768.0,400.0"}, "the middle of the list, which fills the output")

    def test_full_screen_files_lists_a_folder_long_enough_to_scroll_and_the_others_the_baselines_folder(self):
        self.assertEqual(bench.items_of("files-full-scroll"), bench.FULL_SCREEN_ITEMS)
        self.assertEqual(bench.items_of("files-full-repaint"), bench.FULL_SCREEN_ITEMS)
        for name in bench.BASELINE:
            self.assertEqual(bench.items_of(name), bench.ITEMS, name)
        # 3840 x 2160 at scale 1 shows 32 icons a row and 36 rows: the scroll must not reach the folder's end
        rows, row_pt, shown_pt = bench.FULL_SCREEN_ITEMS / 32, 59.0, 2160.0
        self.assertGreater(rows * row_pt - shown_pt, 1800.0)

    def test_a_full_screen_files_scenario_where_files_did_not_fill_the_output_is_flagged(self):
        def run(width):
            return {"windows": {"before": [], "after": [dict(WINDOWS["files"], width=width, height=800.0)]},
                    "span_s": (9.0, 15.0), "output": [1280.0, 800.0], "compositor": {"frames": 200},
                    "clients": {"cupertino-files": {"frames": 180}}}
        self.assertIsNone(bench.check("files-full-scroll", run(1280.0)))
        self.assertIsNotNone(bench.check("files-full-scroll", run(920.0)))
        self.assertIsNotNone(bench.check("files-full-repaint", run(920.0)))

    def test_typing_in_the_launcher_types_each_letter_and_erases_it(self):
        flags, _ = bench.script("launcher-typing", 60, WINDOWS, (1280, 800), 1.0, 15)
        typed = [flags[i + 1].split(":", 1)[1] for i, f in enumerate(flags) if f == "--type"]
        keys = [flags[i + 1].split(":", 1)[1] for i, f in enumerate(flags) if f == "--key"]
        self.assertEqual("".join(typed), "settings" * 2)
        self.assertEqual(keys.count("BackSpace"), 16)
        self.assertEqual(keys.count("Escape"), 2)


class ReadingTest(unittest.TestCase):
    def frames(self) -> pathlib.Path:
        tmp = pathlib.Path(tempfile.mkdtemp())
        path = tmp / "frames.txt"
        path.write_text(
            "# renderer test\n# refresh_us 16666\n# start_monotonic_ns 1000000000\n"
            "# shown_us compose_us render_us glass_us blur_us blurred reused late_us missed input_lat_us app_lat_us "
            "allocated passes gpu_us deco_rasters hud_rasters deco_uploads deco_upload_bytes hud_uploads "
            "hud_upload_bytes blur_passes blur_texels pointer_lat_us submit_us\n"
            "10000000 300 1700 0 0 1 0 0 0 4000 - 0 4 1500 1 0 1 2048 0 0 4 6000 3000 500\n"
            "10016000 300 19000 0 0 0 1 0 1 - 9000 0 0 - 0 0 0 0 0 0 0 0 - 1700\n"
            "30000000 300 1000 0 0 0 0 0 0 - - 0 0 900 0 0 0 0 0 0 0 0 - 600\n")
        return path

    def test_the_frame_lines_are_read_by_their_header_and_a_span_takes_its_frames(self):
        head, rows = bench.read_frames(self.frames())
        self.assertEqual(head["start_monotonic_ns"], "1000000000")
        self.assertEqual(rows[0]["gpu_us"], 1500)
        self.assertIsNone(rows[1]["gpu_us"])
        c = bench.compositor(rows, 16.666, (9.0, 11.0), cpu_s=0.5)
        self.assertEqual(c["frames"], 2)
        self.assertEqual(c["missed"], 1)
        self.assertEqual(c["over_budget"], 1, "19.3 ms of work is over a 60 Hz refresh")
        self.assertEqual(c["decoration_rasters"], 1)
        self.assertEqual(c["decoration_upload_kib"], 2.0)
        self.assertEqual(c["blur_passes_per_frame"], 2.0)
        self.assertEqual(c["blur_texels_per_frame"], 3000.0)
        self.assertEqual(c["gpu_p99_ms"], 1.5)
        self.assertEqual(c["submit_p50_ms"], 0.8, "composing and issuing, without the wait for the GPU")
        self.assertEqual(c["cpu_ms_per_s"], 250.0)
        self.assertEqual(c["cpu_ms_per_frame"], 250.0)
        self.assertEqual(c["input_latency_p95_ms"], 4.0)
        self.assertEqual(c["pointer_latency_p95_ms"], 3.0, "pointer motion alone, to the next frame shown")
        # a file of a server that writes no pointer column reads as no pointer figure
        old = [{k: v for k, v in r.items() if k != "pointer_lat_us"} for r in rows]
        self.assertIsNone(bench.compositor(old, 16.666, (9.0, 11.0), cpu_s=None)["pointer_latency_p95_ms"])

    def test_client_frames_are_grouped_by_program_and_those_longer_than_a_refresh_are_slow(self):
        path = pathlib.Path(tempfile.mkdtemp()) / "timing.jsonl"
        line = lambda program, start, total: json.dumps(
            {"pid": 1, "program": program, "start": start, "update": 1000, "view": 0, "layout": 0, "paint": 0,
             "raster": total - 1000, "copy": 0, "commit": 0, "total": total, "frames": 1})
        start_ns = 1_000_000_000
        path.write_text("\n".join([
            line("cupertino-dock", start_ns + 10_000_000_000, 2_000_000),
            line("cupertino-dock", start_ns + 10_500_000_000, 20_000_000),
            line("cupertino-files --desktop", start_ns + 10_600_000_000, 1_000_000),
            line("cupertino-dock", start_ns + 20_000_000_000, 90_000_000),  # after the span
        ]) + "\n")
        c = bench.clients(path, start_ns, (9.0, 11.0), 16.666)
        self.assertEqual(sorted(c), ["cupertino-dock", "cupertino-files --desktop"])
        self.assertEqual(c["cupertino-dock"]["frames"], 2)
        self.assertEqual(c["cupertino-dock"]["slow_frames"], 1)
        self.assertEqual(c["cupertino-dock"]["frame_max_ms"], 20.0)
        self.assertEqual(c["cupertino-dock"]["update_p50_ms"], 0.001)

    def test_a_clients_update_view_and_layout_are_summed_per_frame(self):
        path = pathlib.Path(tempfile.mkdtemp()) / "timing.jsonl"
        start_ns = 1_000_000_000
        # one frame heavy in view, the other in layout: the worst frame spends 5 ms, though the phases' p99s add to 8
        frames = [(1_000_000, 4_000_000, 0), (1_000_000, 0, 3_000_000)]
        path.write_text("".join(json.dumps(
            {"pid": 1, "program": "cupertino-files", "start": start_ns + 10_000_000_000 + i, "update": u, "view": v,
             "layout": l, "paint": 0, "raster": 0, "copy": 0, "commit": 0, "total": u + v + l, "frames": 1}) + "\n"
            for i, (u, v, l) in enumerate(frames)))
        c = bench.clients(path, start_ns, (9.0, 11.0), 16.666)["cupertino-files"]
        self.assertEqual(c["logic_p50_ms"], 4.0)
        self.assertEqual(c["logic_p99_ms"], 5.0)

    def test_a_sleeping_thread_shows_its_call_and_timeout(self):
        self.assertEqual(bench.sleep_of("232 0x5 0x7ffd 0x400 0xffffffffffffffff 0x0 0x0 0x7ffd 0x7f"),
                         "epoll_wait forever")
        self.assertEqual(bench.sleep_of("232 0x5 0x7ffd 0x400 0x3e8 0x0 0x0 0x7ffd 0x7f"), "epoll_wait 1000 ms")
        self.assertEqual(bench.sleep_of("7 0x7ffd 0x2 0x1f4 0x0 0x0 0x0 0x7ffd 0x7f"), "poll 500 ms")
        self.assertEqual(bench.sleep_of("202 0x7f 0x80 0x0 0x0 0x0 0x0 0x7ffd 0x7f"), "futex forever")
        self.assertEqual(bench.sleep_of("202 0x7f 0x80 0x0 0x7ffd 0x0 0x0 0x7ffd 0x7f"), "futex with a timeout")
        self.assertIsNone(bench.sleep_of("running"))

    def test_a_scenario_that_missed_what_it_drives_is_flagged(self):
        def run(frames, deco, files):
            return {"windows": {"before": [], "after": []}, "span_s": (9.0, 39.0),
                    "compositor": {"frames": frames, "decoration_rasters": deco},
                    "clients": {"cupertino-files": {"frames": files}} if files else {}}
        self.assertIsNone(bench.check("hover", run(200, 30, 0)))
        self.assertIsNotNone(bench.check("hover", run(200, 0, 0)))
        self.assertIsNone(bench.check("files-scroll", run(180, 0, 170)))
        self.assertIsNotNone(bench.check("files-scroll", run(180, 0, 0)))
        self.assertIsNone(bench.check("idle", run(2, 0, 0)), "the clock's minute")
        self.assertIsNotNone(bench.check("idle", run(30, 0, 0)))

    def test_the_summary_is_the_median_of_the_runs_with_their_spread(self):
        runs = [{"rate": 60.0, "scenario": "drag", "compositor": {"frames": f}, "clients": {}, "processes": {}}
                for f in (170, 180, 178)]
        s = bench.summarise(runs)["60 Hz drag"]["compositor.frames"]
        self.assertEqual((s["median"], s["spread"], s["runs"]), (178, 10, 3))


if __name__ == "__main__":
    unittest.main()
