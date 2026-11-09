# cupertino-test-pam

A private PAM policy for the workspace's tests, as a dev-dependency only. With it, a test that starts
`cupertino-login` (the lock screen) or `cupertino-sessiond` (the session manager and its worker), or runs their PAM
code, never reaches the machine's PAM stack, accounts, journal or buses. The production binaries are unchanged: they
call `pam_start` for their `/etc/pam.d` service as installed (`packaging/pam`).

The process under test loads `cupertino-test-pam-shim` with `LD_PRELOAD`. In that process:

- every PAM transaction starts from the policy's directory (`pam_start_confdir`), whose only module is the shim; without
  a policy the start is refused (`PAM_ABORT`) and nothing of `/etc/pam.d` runs;
- syslog writes go to the policy's report, libaudit is absent, and every connection or datagram to the journal's and
  syslog's sockets, the system bus and the users' session buses is refused (`EACCES`) and reported, by path and by the
  socket file's identity;
- with an account of the policy, the password database lookups find it first, and the session leader's `initgroups`
  for the process's own group succeeds without root.

## Public API

| Item | Contents |
|---|---|
| `Script` | what each step does: the password authentication accepts (`PASSWORD` by default; `None` asks nothing), the accounts the stack knows (any other is `user_unknown`), the prompt's style (`Prompt::Hidden`, `Echo`, `Radio`), a lock-out message said before the prompt (authentication then fails), messages after authentication and after the account check, a forced result (`Code`) for `authenticate`, `account`, `setcred`, `open_session` and `close_session`, and the entries the session's opening puts in the PAM environment |
| `Pam::new(&script)` | the policy in a temporary directory, for every service of `SERVICES` and `other`; `with_account(login, home)` adds an account of the test's own user and group |
| `Pam::isolate(&mut command)`, `PamIsolated::pam_isolated(&pam)` | the shim and the policy in a `Command`'s environment, without `XDG_SESSION_ID` and `JOURNAL_STREAM`, with the policy's own `HOME` (a test sets its own after) |
| `Pam::env_words()` | the same variables as `NAME=value` words for a command vector run through `env` |
| `Pam::calls()`, `Pam::steps()` | what each step did: `STEP service=S user=U tty=T flags=0xN [answer=right\|wrong] -> RESULT`, or just `STEP -> RESULT` |
| `Pam::reached()`, `take_reached()` | what a process of the policy tried to reach and was refused, and its syslog lines; a `Pam` panics on drop when this is not empty |
| `Pam::child(test, case)` | this test binary again, running only `test` under the shim with `case` in `CHILD_VARIABLE`, for PAM code a test runs in its own process; the child calls `require_loaded()` first |
| `shared()` | the default policy in the fixture session's directory, for the shell processes of the compositor's tests |
| `shim()`, `shim_loaded()`, `require_loaded()` | the shim's path as Cargo built it, and whether it is loaded in this process |

`crates/cupertino-server/tests/pam-isolation.rs` checks that every test of the workspace that starts one of the two
binaries starts it under the shim, and that a test running PAM in its own process checks the shim is loaded.

## Tests

```
python3 ci/build-test-fixtures.py --locked -- cargo nextest run --locked -E 'package(cupertino-test-pam)'
```

The shim is a fixture of `ci/build-test-fixtures.py`: the tests need its run.
