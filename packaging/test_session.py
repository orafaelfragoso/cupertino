"""Tests of the session command: `python3 -I packaging/test_session.py`."""
import os
import shutil
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
[ -z "${REASON:-}" ] || printf '%s\\n' "$REASON" >"$CUPERTINO_SESSION_END"
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

# A user manager whose session target is active, so the end of the session cleans up after it.
ACTIVE_SYSTEMCTL = '#!/bin/sh\necho "$@" >>"$OUT/systemctl"\n[ "$2" = is-active ] && exit 0\nexit 0\n'

# busctl that records its calls and answers the list of collections with $COLLECTIONS (no provider runs when unset).
FAKE_BUSCTL = """#!/bin/sh
echo "$@" >>"$OUT/busctl"
case " $* " in
*' get-property '*) [ -n "${COLLECTIONS:-}" ] || exit 1; printf '%s\\n' "$COLLECTIONS" ;;
*' call '*) printf 'aoo 0 "/"\\n' ;;
esac
"""

# A session bus with no service directories: nothing on it is started on demand.
BUS_CONFIG = """<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:dir={dir}</listen>
  <auth>EXTERNAL</auth>
  <policy context="default">
    <allow send_destination="*"/>
    <allow receive_sender="*"/>
    <allow own="*"/>
  </policy>
</busconfig>
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
        self.assertIn('ended by signal 11 (status 139)', self.log())
        self.assertIn('server.log', self.log())

    def test_the_reason_the_compositor_wrote_is_logged_on_one_line_and_removed(self):
        result = self.run_session(EXIT='101', REASON="panic: panicked at src/frame.rs:1:2:\x07 no output")
        self.assertEqual(result.returncode, 101)
        self.assertRegex(self.log(), r'ended with status 101 after \d+ s: panic: panicked at src/frame.rs:1:2: no output\n')
        self.assertFalse((self.root / 'state/cupertino/session-end').exists())

    def test_a_reason_left_by_an_earlier_run_is_not_reported(self):
        (self.root / 'state/cupertino').mkdir(parents=True)
        (self.root / 'state/cupertino/session-end').write_text('the rendering context was lost (reset)\n')
        result = self.run_session()
        self.assertEqual(result.returncode, 0)
        self.assertIn('ended normally', self.log())
        self.assertNotIn('rendering context', self.log())

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
        self.assertIn('--user --no-block stop cupertino-session.target', calls)
        self.assertIn('--user unset-environment WAYLAND_DISPLAY XDG_CURRENT_DESKTOP XDG_SESSION_TYPE DISPLAY XAUTHORITY', calls)

    def test_the_defaults_the_compositor_set_are_removed_and_nothing_else(self):
        tools = self.root / 'tools'
        tools.mkdir()
        (tools / 'systemctl').write_text(
            '#!/bin/sh\necho "$@" >>"$OUT/systemctl"\n[ "$2" = is-active ] && exit 0\nexit 0\n'
        )
        (tools / 'systemctl').chmod(0o755)
        runtime = self.root / 'runtime'
        runtime.mkdir()
        record = runtime / 'cupertino-session-defaults'
        record.write_text('QT_QPA_PLATFORM\nELECTRON_OZONE_PLATFORM_HINT\nnot;a name\n1ABC\n')
        result = self.run_session(PATH=f"{tools}:{os.environ['PATH']}", XDG_RUNTIME_DIR=str(runtime))
        self.assertEqual(result.returncode, 0)
        calls = (self.out / 'systemctl').read_text()
        self.assertIn(
            '--user unset-environment WAYLAND_DISPLAY XDG_CURRENT_DESKTOP XDG_SESSION_TYPE DISPLAY XAUTHORITY'
            ' QT_QPA_PLATFORM ELECTRON_OZONE_PLATFORM_HINT\n',
            calls,
        )
        self.assertFalse(record.exists())

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

    # The end of a session locks the Secret Service's collections (cupertino-sessiond's README, "Secret Service").

    def tools(self, systemctl=ACTIVE_SYSTEMCTL, busctl=FAKE_BUSCTL):
        tools = self.root / 'tools'
        tools.mkdir(exist_ok=True)
        for name, text in (('systemctl', systemctl), ('busctl', busctl)):
            if text is not None:
                (tools / name).write_text(text)
                (tools / name).chmod(0o755)
        return f"{tools}:{os.environ['PATH']}"

    def busctl(self):
        path = self.out / 'busctl'
        return path.read_text().splitlines() if path.exists() else []

    def test_logout_locks_every_collection_without_starting_a_provider(self):
        listed = 'ao 2 "/org/freedesktop/secrets/collection/login" "/org/freedesktop/secrets/collection/session"'
        result = self.run_session(PATH=self.tools(), COLLECTIONS=listed)
        self.assertEqual(result.returncode, 0)
        prefix = '--user --auto-start=no --timeout=2 '
        self.assertEqual(self.busctl(), [
            prefix + 'get-property org.freedesktop.secrets /org/freedesktop/secrets org.freedesktop.Secret.Service '
            'Collections',
            prefix + 'call org.freedesktop.secrets /org/freedesktop/secrets org.freedesktop.Secret.Service Lock ao 2 '
            '/org/freedesktop/secrets/collection/login /org/freedesktop/secrets/collection/session',
        ])
        self.assertIn('locked the keyring (2 collections)', self.log())

    def test_logout_leaves_a_provider_that_is_not_running_alone(self):
        result = self.run_session(PATH=self.tools())
        self.assertEqual(result.returncode, 0)
        self.assertEqual(len(self.busctl()), 1)
        self.assertNotIn('keyring', self.log())

    def test_logout_never_passes_on_a_path_that_is_not_a_collection(self):
        for listed in ('ao 1 "/org/freedesktop/secrets/collection/x;touch${IFS}gone"',
                       'ao 2 "/org/freedesktop/secrets/collection/login" "/etc/passwd"',
                       'ao 1 "/org/freedesktop/secrets/collection/"',
                       'as 1 "/org/freedesktop/secrets/collection/login"'):
            with self.subTest(listed):
                (self.out / 'busctl').unlink(missing_ok=True)
                result = self.run_session(PATH=self.tools(), COLLECTIONS=listed)
                self.assertEqual(result.returncode, 0)
                self.assertEqual(len(self.busctl()), 1, 'Lock was called')
                self.assertIn('could not lock the keyring: an unexpected', self.log())
        self.assertFalse((self.root / 'gone').exists())

    def test_a_session_that_did_not_own_the_user_manager_does_not_lock_at_logout(self):
        inactive = '#!/bin/sh\necho "$@" >>"$OUT/systemctl"\n[ "$2" = is-active ] && exit 3\nexit 0\n'
        listed = 'ao 1 "/org/freedesktop/secrets/collection/login"'
        self.run_session(PATH=self.tools(systemctl=inactive), COLLECTIONS=listed)
        self.assertEqual(self.busctl(), [])

    def test_the_keyring_daemon_is_locked_at_logout(self):
        """gnome-keyring-daemon on a private bus with no activatable services, unlocked as PAM unlocks it at login."""
        missing = [tool for tool in ('dbus-daemon', 'gnome-keyring-daemon', 'busctl') if not shutil.which(tool)]
        if missing:
            self.skipTest(f"{', '.join(missing)} not installed")
        # under /tmp: the bus's and the daemon's sockets must fit in a socket path
        run = Path(tempfile.mkdtemp(prefix='kr', dir='/tmp'))
        self.addCleanup(shutil.rmtree, run, True)
        (run / 'bus.conf').write_text(BUS_CONFIG.format(dir=run))
        bus = subprocess.Popen(['dbus-daemon', f'--config-file={run}/bus.conf', '--nofork', '--print-address=1'],
                               stdout=subprocess.PIPE, text=True)
        self.addCleanup(lambda: (bus.kill(), bus.wait(), bus.stdout.close()))
        address = bus.stdout.readline().strip()
        home = self.root / 'home'
        home.mkdir()
        env = {'PATH': '/usr/bin:/bin', 'HOME': str(home), 'XDG_RUNTIME_DIR': str(run),
               'DBUS_SESSION_BUS_ADDRESS': address}
        daemon = subprocess.Popen(['gnome-keyring-daemon', '--foreground', '--components=secrets',
                                   f'--control-directory={run}/keyring', '--unlock'],
                                  env=env, stdin=subprocess.PIPE, stdout=subprocess.DEVNULL,
                                  stderr=subprocess.DEVNULL)
        self.addCleanup(lambda: (daemon.kill(), daemon.wait()))
        daemon.stdin.write(b'login password')
        daemon.stdin.close()

        def locked():
            got = subprocess.run(['busctl', '--user', 'get-property', 'org.freedesktop.secrets',
                                  '/org/freedesktop/secrets/collection/login', 'org.freedesktop.Secret.Collection',
                                  'Locked'], env=env, capture_output=True, text=True)
            return got.stdout.strip() if got.returncode == 0 else None

        for _ in range(100):
            if locked() is not None:
                break
            time.sleep(0.05)
        self.assertEqual(locked(), 'b false', 'the login password did not unlock the keyring')
        result = self.run_session(PATH=self.tools(busctl=None), DBUS_SESSION_BUS_ADDRESS=address)
        self.assertEqual(result.returncode, 0)
        self.assertEqual(locked(), 'b true')
        self.assertIn('locked the keyring (2 collections)', self.log())


if __name__ == '__main__':
    unittest.main()
