"""Tests of the profiler's readings, without a profiler: `python3 -I scripts/dev/test_profile.py`."""
import importlib.machinery
import importlib.util
import pathlib
import tempfile
import unittest

HERE = pathlib.Path(__file__).resolve().parent
loader = importlib.machinery.SourceFileLoader("profile_tools", str(HERE / "profile"))
spec = importlib.util.spec_from_loader("profile_tools", loader)
profile = importlib.util.module_from_spec(spec)
loader.exec_module(profile)

T = "\t"


class PerfReportTest(unittest.TestCase):
    def test_a_reports_rows_are_read_by_its_fields_and_the_marks_are_dropped(self):
        text = (f" 12.07% {T} 67         {T}cupertino-launcher  {T}[.] cupertino_launcher::view::layers::button_at\n"
                f" 5.89%  {T} 33         {T}ld-linux-x86-64.so.2{T}[.] __tls_get_addr\n"
                f" 0.73%  {T} 10         {T}[unknown]           {T}[k] 0xffffffffab9168f3\n"
                "\n# a comment\n")
        rows = profile.parse_report(text, ["overhead", "sample", "dso", "sym"])
        self.assertEqual(len(rows), 3)
        self.assertEqual(rows[0], {"overhead": 12.07, "sample": 67, "dso": "cupertino-launcher",
                                   "sym": "cupertino_launcher::view::layers::button_at"})
        self.assertEqual(rows[2]["sym"], "0xffffffffab9168f3")

    def test_the_inclusive_report_reads_both_shares(self):
        text = f" 99.88% {T} 0.00%  {T}libc.so.6           {T}[.] __libc_start_main\n"
        (row,) = profile.parse_report(text, ["overhead_children", "overhead", "dso", "sym"])
        self.assertEqual((row["overhead_children"], row["overhead"], row["sym"]), (99.88, 0.0, "__libc_start_main"))

    def test_a_symbol_that_holds_the_separator_stays_whole(self):
        text = f" 1.00% {T} 3 {T}a.out{T}[.] weird{T}name\n"
        (row,) = profile.parse_report(text, ["overhead", "sample", "dso", "sym"])
        self.assertEqual(row["sym"], f"weird{T}name")

    def test_a_line_that_is_not_a_row_is_skipped(self):
        self.assertEqual(profile.parse_report("Error:\nUnknown --fields key\n", ["overhead", "sym"]), [])

    def test_threads_are_counted_to_their_process_and_the_busiest_process_comes_first(self):
        tasks = profile.parse_tasks(
            "#      pid      tid     ppid  comm\n"
            "         0        0       -1 |swapper\n"
            "       100      100       -1 | cupertino-serve\n"
            "       100      101      100 |  toolkit-program\n"
            "       200      200      100 |  cupertino-launc\n"
            "       200      201      200 |   gpu-warm-up\n")
        self.assertEqual(tasks[201], {"pid": 200, "ppid": 200, "comm": "gpu-warm-up"})
        threads = [{"pid": "100:cupertino-serve", "sample": 30}, {"pid": "101:toolkit-program", "sample": 5},
                   {"pid": "200:cupertino-launc", "sample": 20}, {"pid": "201:gpu-warm-up", "sample": 25}]
        procs = profile.processes(threads, tasks)
        self.assertEqual([(p["pid"], p["comm"], p["samples"]) for p in procs],
                         [(200, "cupertino-launc", 45), (100, "cupertino-serve", 35)])

    def test_a_process_name_cut_to_15_characters_is_the_program_it_begins(self):
        known = ["cupertino-launcher", "cupertino-files", "cupertino-server"]
        self.assertEqual(profile.program_name("cupertino-launc", known), "cupertino-launcher")
        self.assertEqual(profile.program_name("cupertino-files", known), "cupertino-files")
        self.assertEqual(profile.program_name("nvidia-smi", known), "nvidia-smi")

    def test_bare_addresses_of_one_object_are_summed_into_one_row(self):
        rows = [{"overhead": 17.0, "sample": 89, "dso": "libnvidia-eglcore.so", "sym": "0x97df4e"},
                {"overhead": 12.0, "sample": 65, "dso": "libnvidia-eglcore.so", "sym": "0x97df40"},
                {"overhead": 20.0, "sample": 100, "dso": "cupertino-launcher", "sym": "cupertino_launcher::main"},
                {"overhead": 1.0, "sample": 5, "dso": "[unknown]", "sym": "0xffffffffab9168f3"}]
        merged = profile.merge_unnamed(rows)
        self.assertEqual([(r["dso"], r["overhead"], r["sample"]) for r in merged],
                         [("libnvidia-eglcore.so", 29.0, 154), ("cupertino-launcher", 20.0, 100),
                          ("[unknown]", 1.0, 5)])
        self.assertEqual(merged[0]["sym"], "(code without symbols)")
        self.assertEqual(merged[2]["sym"], "(entering or leaving the kernel)")
        self.assertTrue(profile.unnamed("0000000000000000"))
        self.assertFalse(profile.unnamed("add"), "a short name of hexadecimal letters is a name")

    def test_the_summary_names_the_command_that_shows_the_process(self):
        prof = {"threads": [{"overhead": 100.0, "comm": "cupertino-launc"}],
                "self": [{"overhead": 3.5, "sym": "f", "dso": "a"}],
                "inclusive": [{"overhead_children": 50.0, "overhead": 3.5, "sym": "f", "dso": "a"}]}
        text = profile.cpu_text("cupertino-launcher", pathlib.Path("/x/s.perf.data"),
                                {"pid": 7, "samples": 1999}, prof, "cycles:u")
        self.assertIn("== cupertino-launcher (pid 7): 1999 samples of cycles:u, about 1.00 s of CPU", text)
        self.assertIn("perf report -i /x/s.perf.data --pid 7", text)
        self.assertIn("    3.50%  f  [a]", text)
        self.assertIn("   50.00%    3.50%  f  [a]", text)


class HeaptrackTest(unittest.TestCase):
    def test_the_totals_are_read_from_the_closing_lines(self):
        totals = profile.parse_heap_totals(
            "MOST CALLS TO ALLOCATION FUNCTIONS\n\ntotal runtime: 0.05s.\n"
            "calls to allocation functions: 11071 (235553/s)\ntemporary memory allocations: 17 (361/s)\n"
            "peak heap memory consumption: 372.41K\npeak RSS (including heaptrack overhead): 5.93M\n"
            "total memory leaked: 9.29K\n")
        self.assertEqual(totals, {"runtime": "0.05s.", "allocations": "11071 (235553/s)", "temporary": "17 (361/s)",
                                  "peak_heap": "372.41K", "peak_rss": "5.93M", "leaked": "9.29K"})
        self.assertEqual(profile.heap_bytes("372.41K"), 372410.0)
        self.assertEqual(profile.heap_bytes("12B"), 12.0)

    def test_rusts_names_lose_their_crates_hashes(self):
        self.assertEqual(profile.demangle("main;cupertino_icons[f4bde7af3c6ae92c]::load_theme; 3\n"),
                         "main;cupertino_icons::load_theme; 3\n")

    def test_a_folded_stack_keeps_a_semicolon_inside_brackets(self):
        (frames, value), = profile.parse_folded("main;<[u8; 4] as Foo>::bar;malloc; 128\n")
        self.assertEqual(frames, ["main", "<[u8; 4] as Foo>::bar", "malloc"])
        self.assertEqual(value, 128)

    def test_a_site_is_the_first_frame_outside_the_allocator_and_the_containers(self):
        stacks = profile.parse_folded(
            "main;app::run;app::layout::measure;alloc::raw_vec::RawVecInner::finish_grow;__rust_alloc;malloc; 40\n"
            "main;app::run;app::layout::measure;<alloc::vec::Vec<u8> as core::clone::Clone>::clone;malloc; 2\n"
            "main;app::paint;hashbrown::raw::RawTable::reserve_rehash;malloc; 30\n"
            "main;std::thread::spawn;malloc; 5\n"
            "0x7f00;0x7f01; 9\n")
        found = profile.sites(stacks)
        self.assertEqual([(s["site"], s["value"]) for s in found],
                         [("app::layout::measure", 42), ("app::paint", 30), ("0x7f01", 9), ("main", 5)])
        self.assertEqual(found[0]["via"], "app::run < main")

    def test_the_heap_summary_lists_both_orders(self):
        site = [{"site": "app::paint", "value": 4096, "via": "main"}]
        text = profile.heap_text("cupertino-files", pathlib.Path("/x/f.1.heaptrack.zst"),
                                 {"peak_heap": "1.5M", "peak_rss": "40M"}, site, site)
        self.assertIn("== cupertino-files: peak heap 1.5M, peak RSS 40M", text)
        self.assertIn("      4096  app::paint  (main)", text)
        self.assertIn("   4.0 KiB  app::paint  (main)", text)


class RunTest(unittest.TestCase):
    def test_perfs_buffers_fit_in_half_of_what_a_process_may_lock(self):
        for cpus, memlock in ((16, 8 << 20), (4, 8 << 20), (64, 8 << 20), (128, 64 << 10)):
            pages = profile.buffer_pages(cpus, memlock)
            self.assertEqual(pages & (pages - 1), 0, "a power of two")
            self.assertTrue(pages == 8 or (pages + 1) * 4096 * cpus <= memlock // 2, (cpus, pages))
        self.assertEqual(profile.buffer_pages(16, 8 << 20), 32)

    def test_a_file_names_its_program_before_its_pid(self):
        self.assertEqual(profile.tag_program(pathlib.Path("cupertino-files-desktop.42.heaptrack.zst"),
                                             ".heaptrack.zst"), "cupertino-files-desktop")

    def test_the_wrapper_keeps_the_programs_path_and_records_the_tools_pid(self):
        profile._EVENT[:] = ["cycles:u"]
        out = pathlib.Path(tempfile.mkdtemp())
        text = profile.wrapper("perf", pathlib.Path("/b/cupertino-server"), out)
        env = profile.shutil.which("env") or "/usr/bin/env"
        self.assertIn(f'-- {env} PATH="$PATH" /b/cupertino-server "$@"', text)
        self.assertIn('echo "$child" > "$out/.pids/$tag"', text)
        self.assertIn("--user-callchains", text)
        heap = profile.wrapper("heaptrack", pathlib.Path("/b/cupertino-files"), out)
        self.assertIn('DUMP_HEAPTRACK_OUTPUT="$pipe" /b/cupertino-files "$@"', heap)
        self.assertIn('"$out/$tag.heaptrack.zst"', heap)

    def test_waiting_for_tools_that_ended_returns_at_once(self):
        pids = pathlib.Path(tempfile.mkdtemp())
        (pids / "gone.1").write_text("999999999\n")
        (pids / "x.fifo").write_text("")
        profile.wait_for_tools(pids, 0.0)


if __name__ == "__main__":
    unittest.main()
