# Checking a machine with cupertino-diagnose

`cupertino-diagnose` checks this machine and the session it runs in for what Cupertino needs: the GPU and its driver, the session, the desktop portal, the services, how the last sessions ended, and where the logs are. It changes nothing and opens no device. It reads sysfs, installed files, the session's logs and the states of systemd units. It runs as your user, from the session or from a text console; the checks that need a session are skipped outside one.

This page describes the command as it is on `main` today. The `cupertino` command, with its registry and `--format` option, has [its own page](cupertino-command.md). It does not have a `diagnose` subcommand yet, so `cupertino-diagnose` stays the way to run these checks.

## Usage

```sh
cupertino-diagnose                       # every check, as text
cupertino-diagnose --json                # the same as JSON
cupertino-diagnose --area gpu --area crash
```

| Flag | Meaning |
|---|---|
| `--json` | Print the report as JSON. |
| `--area AREA` | Run only this area. Repeat it for more. `AREA` is `gpu`, `session`, `portals`, `services`, `crash` or `logs`. With no `--area`, every area runs, in that order. |
| `-h`, `--help` | Print the usage and exit 0. |

## Exit codes

| Code | Meaning |
|---|---|
| 0 | The checks ran. A failed check does not change the exit code. |
| 2 | The command line was wrong: an unknown flag, or `--area` with a missing or unknown name. The usage goes to standard error. |

The command never exits 1. A script reads each check's `status` from `--json`.

## Text output

One line for each check: the status (`ok`, `warn`, `FAIL` or `skip`), the check id and a one-line summary. Facts and the fix follow, indented. This is the output from the machine this page was checked on, a session-less text run with the NVIDIA driver (it shows what the format looks like, not what your machine will print):

```text
ok    gpu.driver          the primary GPU uses the nvidia driver
                          driver: nvidia
                          note: primary GPU card1 uses the nvidia driver
skip  session.logind      not run from a logind session
warn  portals.conf        no cupertino-portals.conf: xdg-desktop-portal picks the backends on its own
                          fix: reinstall the cupertino package
```

## JSON output

```json
{
  "schema": 1,
  "checks": [
    {
      "id": "gpu.driver",
      "area": "gpu",
      "status": "ok",
      "summary": "the primary GPU uses the nvidia driver",
      "facts": [{ "key": "driver", "value": "nvidia" }]
    }
  ]
}
```

| Field | Content |
|---|---|
| `schema` | The version of the JSON form, 1 today. It changes when a field is removed or changes meaning. |
| `checks[].id` | A stable dotted name such as `gpu.driver`. |
| `checks[].area` | `gpu`, `session`, `portals`, `services`, `crash` or `logs`. |
| `checks[].status` | `skip`, `ok`, `warn` or `fail`. |
| `checks[].summary` | One line for a person. |
| `checks[].facts` | A list of `key` and `value` pairs. Left out when empty. A key can repeat (`note`, `also`). |
| `checks[].fix` | What to do when the status is `warn` or `fail`. Left out when there is none. |

`skip` means there was nothing to check: not in a session, or the tool the check needs is missing.

## The checks

| Id | What it checks |
|---|---|
| `gpu.driver` | The primary GPU's driver, and what it needs before a compositor takes the display. `fail` for a problem (for example `nvidia-drm` without `modeset=1`, or no GPU), `warn` for a warning (for example no NVIDIA GBM backend, or no driver bound). |
| `session.logind` | logind's record of the session in `XDG_SESSION_ID`; `skip` when there is none. |
| `session.desktop` | `XDG_CURRENT_DESKTOP` names Cupertino; `skip` otherwise. |
| `session.wayland` | The compositor answers on `WAYLAND_DISPLAY`. `fail` when nothing answers or `XDG_RUNTIME_DIR` is not set. |
| `portals.backend` | `cupertino.portal` is installed and lists ScreenCast; `warn` otherwise. |
| `portals.conf` | The first `cupertino-portals.conf` that xdg-desktop-portal reads sends ScreenCast to `cupertino`; `warn` otherwise. |
| `portals.service` | `xdg-desktop-portal.service` (user) is active. Runs only in a session. |
| `services.login`, `services.network`, `services.power`, `services.bluetooth` | The system units `cupertino-login`, `cupertino-netd`, `cupertino-powerd` and `cupertino-btd`. |
| `services.session_target`, `services.pipewire` | The user units `cupertino-session.target` and `pipewire.service`. Run only in a session. |
| `crash.last` | How the last session in `session.log` ended. `warn` when it was not a normal end or a stop by the session. Facts name how many ended badly, the last bad end, and the first panic or fatal-signal line of `server.log` and `server.log.1`. |
| `logs.paths` | Where `session.log`, `server.log` and `server.log.1` are, with their sizes, and the journal command for the login unit. |

For a unit check, the status follows the unit's state: `ok` when active, `fail` when failed, `warn` when inactive, not installed or in another state, and `skip` when systemd cannot be asked. The `fix` names `systemctl status` and `journalctl` for the unit, or says to install the cupertino package.

`services.network` and `services.bluetooth` also give `ok` when `cupertino-netd` or `cupertino-btd` is disabled (or masked) and inactive while `NetworkManager.service` or `bluetooth.service` is active or enabled, because that service does the same job. The summary then reads `... is disabled: managed by NetworkManager` (or `BlueZ`). A machine installed from the [USB image](install-usb.md) is set up this way. BlueZ may be enabled but stopped on a machine without a Bluetooth adapter, and that still counts.

## Known limits

- The portal checks read installed files and the user service. They do not call the backend on the bus.
- `crash.last` reads the logs of the user who runs the command; another user's sessions are not seen.
- Only the primary GPU is checked, not the second GPU of a hybrid machine.
