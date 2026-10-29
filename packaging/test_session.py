"""Tests of the session command: `python3 -I packaging/test_session.py`."""
import os
import signal
import subprocess
import tempfile
import time
import unittest
from pathlib import Path

SESSION = Path(__file__).resolve().parent / 'files/usr/bin/cupertino-session'

FAKE_SERVER = """#!/bin/sh
printf '%s\\n' "$@" >"$OUT/args"
tr '\\0' ' ' </proc/$PPID/cmdline >"$OUT/parent"
if [ -n "${HANG:-}" ]; then
    sleep 30 &
    wait
fi
exit "${EXIT:-0}"
"""


class SessionTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.server = self.root / 'server'
        self.server.write_text(FAKE_SERVER)
        self.server.chmod(0o755)
        self.out = self.root / 'out'
        self.out.mkdir()
        (self.root / 'config/cupertino').mkdir(parents=True)

    def env(self, **extra):
        env = {
            'PATH': os.environ['PATH'],
            'HOME': str(self.root),
            'XDG_CONFIG_HOME': str(self.root / 'config'),
            'XDG_STATE_HOME': str(self.root / 'state'),
            'CUPERTINO_SESSION_SERVER': str(self.server),
            'DBUS_SESSION_BUS_ADDRESS': 'unix:path=/nonexistent',
            'OUT': str(self.out),
        }
        env.update(extra)
        return env

    def run_session(self, **extra):
        return subprocess.run(['sh', str(SESSION)], env=self.env(**extra), capture_output=True, text=True, timeout=30)

    def log(self):
        return (self.root / 'state/cupertino/session.log').read_text()

    def args(self):
        return (self.out / 'args').read_text().split()

    def test_a_normal_end_has_no_limit_and_passes_status_zero(self):
        result = self.run_session()
        self.assertEqual(result.returncode, 0)
        self.assertEqual(self.args(), ['--udev'])
        self.assertNotIn('timeout', (self.out / 'parent').read_text())
        self.assertIn('ended normally', self.log())

    def test_a_crash_ends_the_session_with_the_servers_status_and_is_logged(self):
        result = self.run_session(EXIT='139')
        self.assertEqual(result.returncode, 139)
        self.assertIn('status 139', self.log())
        self.assertIn('server.log', self.log())

    def test_a_limit_file_bounds_the_server_and_leading_zeros_are_decimal(self):
        (self.root / 'config/cupertino/session-limit').write_text('0700\n')
        result = self.run_session()
        self.assertEqual(result.returncode, 0)
        self.assertEqual(self.args(), ['--udev', '--run-for', '700'])
        self.assertTrue((self.out / 'parent').read_text().startswith('timeout -k 10 730 '))

    def test_a_limit_that_is_not_a_whole_number_is_ignored_and_logged(self):
        (self.root / 'config/cupertino/session-limit').write_text('15m\n')
        result = self.run_session()
        self.assertEqual(result.returncode, 0)
        self.assertEqual(self.args(), ['--udev'])
        self.assertIn("ignoring the session limit '15m'", self.log())

    def test_a_signal_to_the_session_reaches_the_server(self):
        process = subprocess.Popen(['sh', str(SESSION)], env=self.env(HANG='1'))
        for _ in range(100):
            if (self.out / 'parent').exists():
                break
            time.sleep(0.05)
        process.send_signal(signal.SIGTERM)
        self.assertEqual(process.wait(timeout=10), 143)
        self.assertIn('status 143', self.log())


if __name__ == '__main__':
    unittest.main()
