"""Tests of the soak verdict: `python3 -I ci/test_soak.py`."""
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))  # isolated mode (-I) does not add the script's directory
from soak import Sample, judge, pacing  # noqa: E402


def run(rss: list[int], fds: list[int]) -> dict:
    return judge([Sample(60.0 * i, r, f) for i, (r, f) in enumerate(zip(rss, fds))], warmup=120)


class JudgeTest(unittest.TestCase):
    def test_a_flat_run_passes(self):
        verdict = run([90_000, 95_000] + [100_000] * 10, [80] * 12)
        self.assertEqual(verdict["problems"], [])
        self.assertEqual(verdict["rss_growth"], 0.0)

    def test_growth_during_the_warm_up_does_not_count(self):
        self.assertTrue(run([50_000, 70_000] + [100_000] * 10, [40, 60] + [80] * 10)["passed"])

    def test_rss_growth_over_ten_percent_after_the_warm_up_fails(self):
        verdict = run([0, 0] + [100_000] * 5 + [110_100] * 5, [80] * 12)
        self.assertEqual(verdict["problems"], ["RSS grew 10.1% after the warm-up (100000 to 110100 KiB)"])

    def test_ten_percent_growth_passes(self):
        self.assertTrue(run([0, 0] + [100_000] * 5 + [110_000] * 5, [80] * 12)["passed"])

    def test_one_sample_more_than_five_percent_off_the_descriptor_count_fails(self):
        verdict = run([100_000] * 12, [80] * 8 + [85] + [80] * 3)
        self.assertEqual(verdict["problems"], ["open descriptors drifted 6.2% from 80 (85 at 480 s)"])

    def test_five_percent_off_the_descriptor_count_passes(self):
        self.assertTrue(run([100_000] * 12, [80] * 8 + [84] + [80] * 3)["passed"])

    def test_a_fall_in_descriptors_counts_as_drift(self):
        self.assertFalse(run([100_000] * 12, [80] * 11 + [75])["passed"])

    def test_too_few_samples_after_the_warm_up_fail(self):
        self.assertEqual(run([100_000] * 11, [80] * 11)["problems"], ["9 samples after the warm-up, 10 needed"])


class PacingTest(unittest.TestCase):
    def test_a_minute_of_frames_gives_its_median_and_99th_percentile(self):
        self.assertEqual(pacing([16_667] * 98 + [33_333, 50_000]), (16_667, 50_000.0))

    def test_a_minute_without_frames_has_no_pacing(self):
        self.assertEqual(pacing([]), (None, None))

    def test_the_verdict_reports_the_change_of_the_frame_median_without_judging_it(self):
        samples = [Sample(60.0 * i, 100_000, 80, 16_000 if i < 7 else 20_000, 17_000) for i in range(12)]
        verdict = judge(samples, warmup=120)
        self.assertTrue(verdict["passed"])
        self.assertEqual(verdict["frame_median"], {"first_us": 16_000, "last_us": 20_000, "change": 0.25})


if __name__ == "__main__":
    unittest.main()
