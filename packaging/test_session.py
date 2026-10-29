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
trap 'touch "$OUT/term"; exit "${EXIT_ON_TERM:-143}"' TERM
printf '%s\\n' "$@" >"$OUT/args"
tr '\\0' ' ' </proc/$PPID/cmdline >"$OUT/parent"
if [ -n "${HANG:-}" ]; then
    sleep 30 &
    wait
    wait
fi
exit "${EXIT:-0}"
"""

# A session bus launcher that ends on TERM and leaves what it started running, as dbus-run-session does.
FAKE_DBUS = """#!/bin/sh
trap 'exit 0' TERM
shift
"$@" &
wait
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

    def wait_for(self, name):
        for _ in range(200):
            if (self.out / name).exists():
                return True
            time.sleep(0.05)
        return False

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
        self.assertTrue(self.wait_for('parent'))
        process.send_signal(signal.SIGTERM)
        self.assertEqual(process.wait(timeout=10), 143)
        self.assertTrue((self.out / 'term').exists())
        self.assertIn('status 143', self.log())

    def test_a_signal_reaches_the_server_through_a_bus_launcher_that_does_not_pass_it_on(self):
        launcher = self.root / 'bin'
        launcher.mkdir()
        (launcher / 'dbus-run-session').write_text(FAKE_DBUS)
        (launcher / 'dbus-run-session').chmod(0o755)
        env = self.env(HANG='1', PATH=f"{launcher}:{os.environ['PATH']}")
        del env['DBUS_SESSION_BUS_ADDRESS']
        process = subprocess.Popen(['sh', str(SESSION)], env=env)
        self.assertTrue(self.wait_for('parent'))
        process.send_signal(signal.SIGTERM)
        process.wait(timeout=10)
        self.assertTrue(self.wait_for('term'), 'the server was left running')

    def test_a_state_directory_that_cannot_be_written_does_not_keep_the_server_from_starting(self):
        blocker = self.root / 'not-a-directory'
        blocker.write_text('')
        result = self.run_session(XDG_STATE_HOME=str(blocker), EXIT='3')
        self.assertEqual(result.returncode, 3)
        self.assertEqual(self.args(), ['--udev'])
        self.assertEqual(result.stderr, '')

    def test_leading_zeros_do_not_count_towards_the_digit_limit(self):
        (self.root / 'config/cupertino/session-limit').write_text('00000060\n')
        result = self.run_session()
        self.assertEqual(result.returncode, 0)
        self.assertEqual(self.args(), ['--udev', '--run-for', '60'])

    def test_a_stop_by_the_session_is_logged_as_that_not_as_the_watchdog(self):
        process = subprocess.Popen(['sh', str(SESSION)], env=self.env(HANG='1', EXIT_ON_TERM='137'))
        self.assertTrue(self.wait_for('parent'))
        process.send_signal(signal.SIGTERM)
        self.assertEqual(process.wait(timeout=10), 137)
        self.assertIn('stopped by the session', self.log())
        self.assertNotIn('watchdog', self.log())

    def test_the_session_target_is_stopped_when_the_compositor_ends(self):
        tools = self.root / 'tools'
        tools.mkdir()
        (tools / 'systemctl').write_text(
            '#!/bin/sh\necho "$@" >>"$OUT/systemctl"\n[ "$2" = is-active ] && exit 0\nexit 0\n'
        )
        (tools / 'systemctl').chmod(0o755)
        result = self.run_session(PATH=f"{tools}:{os.environ['PATH']}")
        self.assertEqual(result.returncode, 0)
        calls = (self.out / 'systemctl').read_text()
        self.assertIn('--user stop cupertino-session.target', calls)
        self.assertIn('--user unset-environment WAYLAND_DISPLAY XDG_CURRENT_DESKTOP XDG_SESSION_TYPE', calls)

    def test_an_inactive_session_target_is_left_alone(self):
        tools = self.root / 'tools'
        tools.mkdir()
        (tools / 'systemctl').write_text('#!/bin/sh\necho "$@" >>"$OUT/systemctl"\n[ "$2" = is-active ] && exit 3\nexit 0\n')
        (tools / 'systemctl').chmod(0o755)
        self.run_session(PATH=f"{tools}:{os.environ['PATH']}")
        self.assertEqual((self.out / 'systemctl').read_text().split('\n')[:-1], ['--user is-active --quiet cupertino-session.target'])

    def test_a_limit_of_more_than_seven_digits_is_ignored(self):
        (self.root / 'config/cupertino/session-limit').write_text('12345678\n')
        result = self.run_session()
        self.assertEqual(result.returncode, 0)
        self.assertEqual(self.args(), ['--udev'])
        self.assertIn('more than 7 digits', self.log())


if __name__ == '__main__':
    unittest.main()
