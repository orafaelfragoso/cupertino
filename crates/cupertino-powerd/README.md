# cupertino-powerd

The power and backlight daemon, replacing UPower and the backlight helpers. It reads `/sys/class/power_supply`,
`/sys/class/backlight` and `/sys/class/leds`, follows the lid switch over evdev, and speaks the subset of
`org.freedesktop.UPower` that apps call. Pure Rust over `zbus` (blocking API).

## Service

| | |
|---|---|
| Bus names | `org.freedesktop.UPower` (subset) and `org.cupertino.Power1` (display backlight); system bus by default |
| Runs as | root |
| Unit file | [`cupertino-powerd.service`]: runs `/opt/cupertino/bin/cupertino-powerd` after `dbus.socket` |
| D-Bus policy | [`org.cupertino.conf`]: root owns both names, everyone may call them |

## Public API

| Item | Contents |
|---|---|
| `supply` | pure model of one supply: `Supply::parse(name, &Attributes)`, `Battery` (percentage, state, `time_to_empty`, `time_to_full`, health, energy in Wh) |
| `view` | `DeviceView::of_supply`, `DeviceView::display` (all batteries as one), `on_battery`: the values of a UPower device |
| `backlight` | `Backlight`, `BacklightType`, `select` (firmware, then platform, then raw), `KeyboardBacklight` |
| `sysfs` | `Snapshot::read(sysfs_root, lid)`, `read_supplies`, `read_backlights`, `read_keyboard_backlights`, `write_brightness` |
| `lid` | `LidSwitch::find(sysfs_root)`: the input device that reports `SW_LID`, found with `cupertino-devices` and read with `cupertino-evdev` |
| `events` | `concerns_power(&DeviceEvent): which uevents mean "read sysfs again" |
| `warning` | `Warnings::update(now, Reading)`: the low-battery level (none, early, final), `Level::upower |
| `auth` (feature `dbus`) | `Authorizer`, `LogindAuthorizer` (the real check), `AllowAll` |
| `daemon` (feature `dbus`) | `Daemon::start(Config)`, `refresh`, `set_lid; diffs two snapshots into signals |
| `interfaces` (feature `dbus`) | the D-Bus objects and their errors |

Features: `dbus` (default; zbus and `cupertino-seat`), `app` (the `cupertino-powerd` binary, implies `dbus`).

## Running the daemon

```sh
cargo run -p cupertino-powerd --features app -- [--session-bus [--no-auth]] [--sysfs-root PATH]
```

- **System bus by default**, as root: it writes sysfs and opens `/dev/input/event*.
- **`--session-bus`** serves on the session bus. With **`--sysfs-root`** it runs unprivileged against a fake tree, which
  still needs `class/input` (empty is fine). **`--no-auth`** skips the session check and needs `--session-bus`.
- **Events.** It waits on the kernel uevent socket (`cupertino_devices::Monitor`) and the lid device. A `power_supply`,
  `backlight` or `leds` uevent, a queue overflow, or 30 s without an event (approximate: drivers do not announce every
  change of charge or rate) makes it read sysfs again, as does the end of the warning rules' ignore window.
- **Exit.** A startup failure, a name held by another daemon (UPower), or a failing uevent socket ends the process.

## D-Bus

Names, paths and types follow <https://upower.freedesktop.org/docs/>.

| Where | What |
|---|---|
| `org.freedesktop.UPower` at `/org/freedesktop/UPower` | `EnumerateDevices`, `EnumerateKbdBacklights`, `GetDisplayDevice`, `DeviceAdded`, `DeviceRemoved`; properties `DaemonVersion`, `OnBattery`, `LidIsClosed`, `LidIsPresent` |
| `org.freedesktop.UPower.Device` at `.../devices/<kind>_<name>` and `.../devices/DisplayDevice` | `NativePath`, `Vendor`, `Model`, `Serial`, `Type`, `PowerSupply`, `Online`, `Energy`, `EnergyEmpty`, `EnergyFull`, `EnergyFullDesign`, `EnergyRate`, `Voltage`, `TimeToEmpty`, `TimeToFull`, `Percentage`, `IsPresent`, `State`, `IsRechargeable`, `Capacity`, `IconName`; `WarningLevel`; `PropertiesChanged` for the ones that changed |
| `org.freedesktop.UPower.KbdBacklight` at `.../KbdBacklight` (the first one) and `.../KbdBacklight/<name> | `GetBrightness`, `GetMaxBrightness`, `SetBrightness`, `BrightnessChanged`, `BrightnessChangedWithSource` (`external` for a call, `internal` for a hardware change), `NativePath`; error `org.freedesktop.UPower.GeneralError` |
| `org.cupertino.Power1` at `/org/cupertino/Power1` | display backlight: `GetBrightness`, `GetMaxBrightness`, `SetBrightness(u)`, signal `BrightnessChanged(u)`; errors `NoBacklight`, `OutOfRange`, `NotAuthorized`, `Failed` |

Not served: `GetCriticalAction`, `Technology`, `BatteryLevel`, `UpdateTime`, history and statistics, charge thresholds.

## How it works

- **Reading is open, writing is not.** `SetBrightness` (both interfaces) needs the caller's process (from
  `GetConnectionCredentials`) to belong to a logind session (`GetSessionByPID`) that is `Active` and has a seat. No
  polkit. The check sits behind the `Authorizer` trait so tests control it.
- **Percentage** is `now / full` of the amounts the battery reports (UPower's method), falling back to the firmware's
  `capacity` only when there are no amounts. `charge_*` batteries need `voltage_now` for energy in Wh, not for percentage or
  time.
- **State.** `Not charging` is "pending charge"; `Discharging` at 0 percent is "empty"; nothing yields "pending discharge".
- **Time estimates** exist only while discharging (to empty) or charging (to full) and drop to 0 for a rate under 0.01 W
  (or A) or an estimate over 20 h (both approximate, modelled on UPower).
- **Display device.** One virtual battery over the present batteries with `scope` `System`: energies and rates add up, the
  percentage is total over total (or the batteries' mean when one has no energy figure), and charging beats discharging
  beats fully charged. A battery of an attached device (`scope` `Device`, such as a wireless mouse) is listed with
  `PowerSupply` false and left out of the display device and of `OnBattery`.
- **`OnBattery`** is true when no line power is online and a power-supply battery discharges.
- **Icons** use freedesktop icon names; the percentage steps between them are approximate.
- **Backlights.** With several display backlights the daemon controls the best `type` (firmware, platform, raw), the
  lowest name among equals, and reports `actual_brightness` when the driver has it. Brightness is in device units.
- **Sysfs reads.** A missing attribute file, `ENODATA` or `ENODEV` (the driver cannot report it now) means absent. Any other failure, an unknown `type` or
  `status`, or a non-numeric value is an error naming the supply. A missing class directory means no devices.
- **Low-battery warning** (`WarningLevel`, the design's power daemon rules). The percent is
  `lround` of the display device's percentage, clamped to 1 … 100, and does not rise while the battery discharges (only
  while it charges or during the ignore window). On battery, 3 to 10 percent is the early warning (`WarningLevel` 3, low)
  and 2 or less the final one (4, critical). The level only rises, and external power resets it to none (1). A change of
  the charging or full state opens a 60 s window with no warning. The display device and every power-supply battery report
  the machine's level; other devices (line power, a mouse's battery) report 1. The session agent of `cupertino-login`
  turns the level into notifications.

## Diagnostic events

The daemon records versioned diagnostic events to the system journal (`CUPERTINO_DIAG_SCHEMA=1`,
`SYSLOG_IDENTIFIER=cupertino-powerd`), each beside the line or error it already gives; `main` installs the recorder
before anything else. Suspend and inhibitor locks are not this daemon's: they are logind's, asked by the session
manager and the server, so it records none of their failures.

| Event | Severity | Fields | Where |
|---|---|---|---|
| `power.backend.start_failed` | error | `step` (`lid`, `logind`, `uevent`, `bus`), `kind` (the error's class), `error` | a step of the start fails |
| `power.request.failed` | warn | `interface`, `method` (`SetBrightness`), `error` (the D-Bus error name), `backlight` (the keyboard backlight's name) | a brightness change is answered with an error |
| `power.exit_error` | error | `error` | the daemon ends with an error, or the arguments are wrong |
| `power.panic` | fatal | `panic`, `location` | any panic |

## Tests

```
cargo test -p cupertino-powerd --features app
```

`tests/fake-sysfs.rs` reads fake trees (a battery with `energy_*`, one with `charge_*` and voltage, two batteries plus a
device battery, mains online with a full battery, several backlights, a keyboard LED). `tests/dbus.rs` runs on a private
`dbus-daemon` (skipped when missing): display device `Percentage` and `State`, `PropertiesChanged`, `DeviceAdded` and
`DeviceRemoved`, `SetBrightness` writing the fake file, `WarningLevel` from low to critical and back to none on external
power, a refused caller, and `LogindAuthorizer` against a stand-in logind (including the nested call from inside a served
method). Unit tests cover the parsers and the estimate bounds. The lid device needs `/dev/input` and is not covered.
