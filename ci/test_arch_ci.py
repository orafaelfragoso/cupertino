#!/usr/bin/env python3
"""Tests of ci/arch-ci.sh without root, pacman or a container: `python3 -I ci/test_arch_ci.py`."""
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parent.parent
SCRIPT = ROOT / 'ci/arch-ci.sh'


def listed(*groups):
    return subprocess.run(['bash', str(SCRIPT), 'list', *groups], capture_output=True, text=True,
                          check=True).stdout.split()


def recipe_array(name):
    text = (ROOT / 'packaging/arch/cupertino/PKGBUILD').read_text()
    return [re.sub(r'[<>=].*', '', p) for p in re.findall(r"'([^']+)'", re.search(rf'^{name}=\((.*?)\)', text, re.M | re.S)[1])]


class Groups(unittest.TestCase):
    def test_the_build_group_holds_the_recipe_makedepends(self):
        # rustup provides cargo and installs the pinned toolchain
        wanted = {'rustup' if p == 'cargo' else p for p in recipe_array('makedepends')}
        self.assertLessEqual(wanted, set(listed('build')))

    def test_the_test_group_starts_the_daemons_the_tests_need(self):
        packages = listed('test')
        self.assertLessEqual(set(listed('build')), set(packages))
        # dbus-daemon, the PipeWire daemon, gnome-keyring-daemon and secret-tool
        self.assertLessEqual({'dbus', 'pipewire', 'gnome-keyring', 'libsecret'}, set(packages))

    def test_groups_combine_without_repeats(self):
        packages = listed('build', 'test', 'weston')
        self.assertEqual(len(packages), len(set(packages)))
        self.assertEqual(packages[-1], 'weston')

    def test_an_unknown_group_fails(self):
        result = subprocess.run(['bash', str(SCRIPT), 'list', 'debian'], capture_output=True, text=True)
        self.assertEqual(result.returncode, 2)
        self.assertIn('unknown group: debian', result.stderr)


class Commands(unittest.TestCase):
    """install and user with the system commands replaced by stubs that log their arguments."""

    def setUp(self):
        scratch = tempfile.TemporaryDirectory()
        self.addCleanup(scratch.cleanup)
        self.root = Path(scratch.name)
        self.bin = self.root / 'bin'
        self.bin.mkdir()
        self.log = self.root / 'log'
        self.runner_temp = self.root / 'runner-temp'
        self.runner_temp.mkdir()
        self.workspace = self.root / 'workspace'
        self.workspace.mkdir()
        self.env_file = self.root / 'github-env'
        for name in ('pacman', 'groupadd', 'chown', 'install', 'mkdir', 'runuser'):
            self.stub(name, 'exit 0')
        self.stub('getent', 'exit 2')  # no such group
        # the user exists once useradd made it, or from the start with CI_EXISTS
        self.stub('useradd', f'touch {self.root}/made')
        self.stub('id', f'[ -n "$CI_EXISTS" ] || [ -e {self.root}/made ]')
        self.stub('stat', 'case $2 in %u) echo "${OWNER_UID:-1001}";; %g) echo 118;; esac')

    def stub(self, name, body):
        path = self.bin / name
        path.write_text(f'#!/bin/sh\necho "{name} $*" >> "$TEST_LOG"\n{body}\n')
        path.chmod(0o700)

    def run_script(self, *args, **env):
        environment = dict(os.environ, PATH=f'{self.bin}{os.pathsep}{os.environ["PATH"]}', TEST_LOG=str(self.log),
                           RUNNER_TEMP=str(self.runner_temp), GITHUB_WORKSPACE=str(self.workspace),
                           GITHUB_ENV=str(self.env_file), **env)
        for name in ('CARGO_HOME', 'RUSTUP_HOME'):
            environment.pop(name, None)
        return subprocess.run(['bash', str(SCRIPT), *args], env=environment, capture_output=True, text=True)

    def calls(self):
        return self.log.read_text().splitlines() if self.log.exists() else []

    def test_install_updates_the_system_and_installs_each_package_once(self):
        result = self.run_script('install', 'build', 'test')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.calls(), ['pacman -Syu --noconfirm --needed ' + ' '.join(listed('build', 'test'))])

    def test_user_takes_the_runner_uid_and_exports_the_rust_homes_and_runtime_directory(self):
        result = self.run_script('user')
        self.assertEqual(result.returncode, 0, result.stderr)
        calls = self.calls()
        self.assertIn('groupadd --gid 118 ci', calls)
        self.assertIn('useradd --create-home --uid 1001 --gid 118 ci', calls)
        self.assertIn(f'chown -R ci:118 {self.workspace} /home/ci/.cargo /home/ci/.rustup', calls)
        self.assertIn('install -d -m 700 -o ci -g 118 /run/user/1001', calls)
        self.assertIn(f'runuser -u ci -- git config --global --add safe.directory {self.workspace}', calls)
        self.assertEqual(self.env_file.read_text().splitlines(),
                         ['CARGO_HOME=/home/ci/.cargo', 'RUSTUP_HOME=/home/ci/.rustup', 'XDG_RUNTIME_DIR=/run/user/1001'])

    def test_user_again_only_gives_back_ownership(self):
        result = self.run_script('user', CI_EXISTS='1')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse([call for call in self.calls() if call.startswith('useradd')])
        self.assertIn(f'chown -R ci:118 {self.workspace} /home/ci/.cargo /home/ci/.rustup', self.calls())

    def test_user_refuses_to_run_the_tests_as_root(self):
        result = self.run_script('user', OWNER_UID='0')
        self.assertEqual(result.returncode, 1)
        self.assertIn('cannot run as root', result.stderr)
        self.assertFalse([call for call in self.calls() if call.startswith(('useradd', 'chown'))])

    def test_user_outside_a_ci_job_fails(self):
        result = subprocess.run(['bash', str(SCRIPT), 'user'], capture_output=True, text=True,
                                env={'PATH': os.environ['PATH']})
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('CI container job only', result.stderr)


if __name__ == '__main__':
    unittest.main()
