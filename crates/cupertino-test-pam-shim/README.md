# cupertino-test-pam-shim

The shared object `cupertino-test-pam` loads into a process under test with `LD_PRELOAD`; never installed. The release
builds only the workspace's binaries (`cargo build --bins`), nothing depends on this library, and
`packaging/test_release.py` checks that no package ships it. It has no tests of its own (a test harness linked from it
would replace `connect`, `syslog` and `pam_start` in the test runner); `cupertino-test-pam`'s tests run it in a child
process.

In a process that loads it:

| Interface | What it does |
|---|---|
| `pam_start`, `pam_start_confdir` | start the transaction from the directory in `CUPERTINO_TEST_PAM_DIR`; without it, refuse with `PAM_ABORT` |
| `pam_sm_authenticate`, `pam_sm_setcred`, `pam_sm_acct_mgmt`, `pam_sm_open_session`, `pam_sm_close_session`, `pam_sm_chauthtok` | the policy's module: each step does what its line's arguments say (`record=`, `users=`, `password=`, `prompt=`, `lockout=`, `auth_info=`, `account_info=`, a forced result by step name, `env=`) and appends a line to the record |
| `openlog`, `syslog`, `vsyslog`, `__syslog_chk`, `__vsyslog_chk`, `closelog` | write to the file in `CUPERTINO_TEST_PAM_REPORT` and standard error, never to the journal |
| `audit_open` | fails as on a kernel without audit, so libpam sends no audit record |
| `connect`, `sendto`, `sendmsg` | refuse (`EACCES`) and report every address of the journal, syslog, the system bus and the users' session buses, by path and by the socket file's identity, and the path in `CUPERTINO_TEST_PAM_REFUSE` |
| `getpwnam_r`, `getpwuid_r`, `initgroups` | with `CUPERTINO_TEST_PAM_PASSWD`, answer from that file first; `initgroups` for the process's own group succeeds without root |
