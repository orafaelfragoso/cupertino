# cupertino-diagnostics

Typed diagnostics of a Cupertino installation and the session it runs in, as text for a person or JSON for a script.
`cupertino diagnose` (crate `cupertino-cli`) runs it; `cupertino-diagnose` is the program it replaces. Layer: `tools`.

```
cupertino diagnose                       # every check, as text
cupertino --format json diagnose         # the report below as the envelope's data
cupertino-diagnose --json                # the report alone, as before
cupertino-diagnose --area gpu --area crash
```

`cupertino-diagnose` still works. In the text form it first prints one line on stderr,
``cupertino-diagnose: use `cupertino diagnose`; this command will be removed in a later release``; its `--json`
output is unchanged, with nothing on stderr. It will be removed in a later release.

It exits 0 once the checks ran, whatever they found: a script reads each check's `status` from `--json`. A wrong
argument exits 2, as `invalid_usage` does in the `cupertino` command's exit codes; it never exits 1. Nothing is changed
and no device is opened: the checks read sysfs, installed files, `/run/systemd/sessions`, the session's logs, and ask
systemd for unit states (`systemctl show`). It runs as the user, from the session or from a text console; checks that need a session are skipped
outside one.

## Checks

| Id | Status |
|---|---|
| `gpu.driver` | the primary GPU's driver, and what it needs before a compositor takes the display (`cupertino_devices::preflight`): `fail` for a problem (nvidia-drm without `modeset=1`, no GPU), `warn` for a warning (no nvidia GBM backend, no driver bound) |
| `gpu.renderer` | the last session's line in `session.log` saying it rendered with a software renderer (`cupertino_server::software_render`): `warn` when the last or the running session did, that is when no `starting` line of `cupertino-session` follows it; facts: its time and GPU; `skip` without a `session.log` |
| `session.logind` | logind's record of `XDG_SESSION_ID`: `ok` for an active wayland session; facts: class, PAM service, VT, seat |
| `session.desktop` | `XDG_CURRENT_DESKTOP` names Cupertino; `skip` otherwise |
| `session.wayland` | the compositor answers on `WAYLAND_DISPLAY` (a connect to the socket); `fail` when nothing does |
| `portals.backend` | `cupertino.portal` is installed and lists ScreenCast |
| `portals.conf` | the first `cupertino-portals.conf` xdg-desktop-portal reads (user, `/etc`, `/usr/share`) sends ScreenCast to `cupertino` |
| `portals.service` | `xdg-desktop-portal.service` (user) is active; in a session only |
| `services.login`, `services.network`, `services.power`, `services.bluetooth` | `cupertino-login`, `cupertino-netd`, `cupertino-powerd`, `cupertino-btd` (system): `ok` active, `fail` failed, `warn` inactive or not installed, `skip` when systemd cannot be asked. `cupertino-netd` and `cupertino-btd` are also `ok` in any other state (inactive with any unit file state, failed or not installed) while `NetworkManager.service` or `bluetooth.service` is active, or enabled and inactive (BlueZ stays inactive without an adapter), which does their job |
| `services.session_target`, `services.pipewire` | `cupertino-session.target` and `pipewire.service` (user); in a session only |
| `crash.last` | the last end in `session.log`: `warn` when it was not a normal end or a stop by the session; facts: how many ended badly, the last bad end, and the first panic or fatal-signal line of `server.log` and `server.log.1` |
| `crash.gpu_reset` | the GPU resets the server recorded in `session.log` (it makes a new renderer and the clients stay connected): `warn` when there was one; facts: how many, after how many it made a new renderer, and the last one's time, GPU, reason and outcome. Its `fix` says how to investigate one: the JSON form and the `server.log` lines of that time, for an AI assistant or the driver's bug tracker; not run without a `session.log` |
| `logs.paths` | where `session.log`, `server.log` and `server.log.1` are, with their sizes, and the journal unit |

## JSON

```json
{
  "schema": 1,
  "checks": [
    {
      "id": "gpu.driver",
      "area": "gpu",
      "status": "ok",
      "summary": "the primary GPU uses the nvidia driver",
      "facts": [{ "key": "driver", "value": "nvidia" }],
      "fix": "..."
    }
  ]
}
```

`schema` changes when a field is removed or changes meaning. Ids and fact keys are stable; `facts` and `fix` are left out
when empty. `status` is `skip`, `ok`, `warn` or `fail`; `area` is `gpu`, `session`, `portals`, `services`, `crash` or
`logs`.

## Library

`run(&Sources, &[Area]) -> Report`. `Sources` holds the sysfs root, the file system root, the environment and a function
that names a unit's state, so `tests/diagnose.rs` runs every area against a fixture tree. `Sources::system()` is this
machine.

## Known limitations

- The portal checks read the installed files and the user service; they do not call the backend on the bus.
- `crash.last` reads the logs of the user running it; another user's sessions are not seen.
- No GPU check of the second GPU of a hybrid machine, only the primary one.

## Tests

```
cargo test -p cupertino-diagnostics
```

`tests/exit-codes.rs` runs the program: its exit codes, the notice on stderr in the text form, and a `--json` stdout
that is the report alone with nothing on stderr.
