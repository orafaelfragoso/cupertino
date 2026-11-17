# cupertino-radios

The Wi-Fi and Bluetooth radios through whichever service owns them. The network is `org.cupertino.Network1`
(`cupertino-netd`) when it is on the system bus, else NetworkManager; the Bluetooth adapter is
`org.cupertino.Bluetooth1` (`cupertino-btd`), else BlueZ. Settings, the control panel and the menu bar find, read,
change and follow the radios through this crate, so the three show the same state. Layer: `foundation`.

## Public API

| Item | Contents |
|---|---|
| `Wifi`, `Network`, `KnownNetwork`, `Device`, `Vpn` | the network's reading in the words of `org.cupertino.Network1`: the radio, the devices, the networks in range (keyed by `ssid`, labelled by `name`) and the saved ones |
| `Link` | the radio and its first Wi-Fi device without the networks, and `network()`, the one it is connected to |
| `Adapter`, `BtDevice`, `LinkState` | the Bluetooth adapter (`available` is false without one) and its devices; `BtDevice::title` and `state` |
| `display_name`, `device_title`, `device_kind`, `MAX_NAME` | a name a radio supplies as a label (no control or bidirectional characters, cut to `MAX_NAME`); a device's label when it has no name |
| `detect`, `Found` (feature `dbus`) | which services are on the bus, running or activatable; the Cupertino daemons come first |
| `NetworkBackend` | `wifi`, `link`, `set_wireless_enabled`, `join` (an empty passphrase joins an open or a saved network), `disconnect`, `scan` |
| `BluetoothBackend` | `adapter`, `set_powered`, `connect`, `disconnect` (BlueZ answers a `connect` once the link is up or failed: call it from a thread that may wait) |
| `refusal` | the text a service sent with its refusal |
| `CALL_TIMEOUT`, `LINK_TIMEOUT`, `call_timeout`, `set_call_timeout`, `unanswered` | how long a reading or a switch waits for a service's answer (3 s; a process can set another wait), how long a call waits that answers once a link is up, down or removed (90 s), and whether an error says the service did not answer in time |
| `netd`, `btd`, `nm`, `bluez` | each service's readings and calls; `nm` also has the saved networks' Forget and Auto-Join and the VPN connections, `bluez` discovery, pairing and removal |
| `watch::rules`, `watch::classify`, `watch::concern`, `watch::shown`, `watch::follow` | the match rules of the signals that tell a reading is out of date (a service starting or stopping, the services' objects), which radio an object is about, which radio's reading a signal makes out of date (none for a change of the Bluetooth stack that no reading shows: a device's signal strength and advertisement, a player's position), and a follower that calls back with the radio after each such signal on a connection's own executor |
| `fake::FakeNetworkManager`, `fake::FakeBluez`, `fake::Silent` (feature `fake`) | stand-ins for the two services on a test bus, for the tests of the crates that use this one: they record what they were asked and how often they were read, send the services' signals, and can be changed from outside as the radio itself would change; `FakeNetworkManager::hang` and `Silent` keep a service's name and answer no reading |

## How it works

Every reading asks the service again; nothing is cached, so a reading taken after a change signal sees what the signal
announced. The blocking calls take a `zbus::blocking::Connection`; a caller that must not wait runs them on a thread
of its own. Every call gives up after a bounded wait, whatever the connection: a reading or a switch after
`call_timeout()`, a call that answers once a link is up, down or removed after `LINK_TIMEOUT`; only a pairing, which
waits for a person, has no bound. A service that does not answer fails the whole reading at its first call
(`unanswered`), and a list of objects waits once, not once per object. Without the feature `dbus` the crate is the
readings' types alone and has no dependency.

A NetworkManager reading works the daemon's records out of NetworkManager's objects, each device's and each access
point's properties in one call and the saved connections read once: one network per SSID with its strongest access
point, the security from the access point's flags, the signal as dBm. An SSID that is not text keeps
a key of its own (`nm::ssid_key`), so a Join reaches the network chosen. On BlueZ the adapter is the first by path and
a device's id is its object path; a call naming any other path is refused before it is sent.
