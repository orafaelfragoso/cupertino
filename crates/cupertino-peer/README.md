# cupertino-peer

Who is on the other end of a session bus call. Every client of the session bus runs as the same user, so the user id says
nothing; the caller is the process behind its connection.

* `identify(connection, sender) -> Caller`: asks the bus daemon for the connection's credentials and takes the process from
  the `ProcessFD` it gives. The descriptor is held as a `ProcessHandle` (a pidfd) while the call is judged, so a process id
  that is reused meanwhile is never mistaken for the caller. A daemon older than 1.15 (Ubuntu 24.04 ships 1.14) gives no
  descriptor, so caller identity there is pid-based: the pidfd is opened for its `ProcessID` (logged at debug level each time), which
  leaves a window for a reused process id that a descriptor does not. A caller with no process at all, with a process id that
  disagrees with the descriptor, that cannot be classified or that has exited is `Caller::Anonymous` and trusted with nothing. A process whose `/proc/<pid>/root` the kernel refuses (one that made itself non-dumpable) is `Caller::Unclassified`: its process id is known, but `unsandboxed()` and `require_relation` refuse it, so nothing trusts it by sandbox or relation; only a caller that knows the process by other means may accept it.
* Sandboxes are told apart the way the desktop portal does it: a process with `/proc/<pid>/root/.flatpak-info` is a Flatpak,
  one whose `LinuxSecurityLabel` starts with `snap.` a Snap. `Caller::unsandboxed` refuses both and the anonymous caller
  (`Denial`).
* `Relation` and `Caller::require_relation`: a service of the shell answers its own process and its parent, the server that
  started it. Other shell processes ask the server to call for them. A process with the same parent is no relation: the
  server starts the applications too (`spawn_prepared`), so "started by the server" would let every application in.
* `same_instance(a, b, proc_root)`: whether two identified processes are one sandbox instance. The same process (equal
  pids, neither pidfd readable, so a reused pid never counts) with the same sandbox class matches; so do two Flatpak
  processes whose `/.flatpak-info` files name the same application id and `instance-id` (an application and its
  `xdg-dbus-proxy`). A Snap has no instance to compare (no mount namespace comparison), so a Snap matches only its own
  process, as an unsandboxed process does; a sandboxed and an unsandboxed process never match, and anything that cannot
  be read is no match. An unsandboxed process could make a `/.flatpak-info` of its own in a user namespace, but it can
  already reach every bus name directly, so passing for a Flatpak instance gains it nothing (accepted).
* `Caller::from_process_fd`: like `from_credentials`, but only the daemon's `ProcessFD` names the process; a bare
  `ProcessID` gives `Anonymous`. `Caller::of_process` classifies a process held otherwise, by its own AppArmor label
  (`/proc/<pid>/attr/apparmor/current`, else `attr/current`; `EINVAL` means no labelling module is active) and its
  root. `ProcessHandle::of_peer` holds the process that connected a socket (`SO_PEERPIDFD`, Linux 6.5).
* `check_call`: the one call a method of such a service makes first. A refusal is logged once with the interface, the method
  and the process id, never the arguments, and returns a `CallerError`, whose names are `org.cupertino.Error.AnonymousCaller`,
  `FlatpakCaller`, `SnapCaller` and `UntrustedCaller`.

Users: `cupertino-server` (`org.cupertino.Session1`, the call to `org.cupertino.Login1` and the owner check of a bus name
a Wayland client announces), `cupertino-login`
(`org.cupertino.Login1`) and `cupertino-osd` (`org.cupertino.Osd1`).

## Known limitations

* The pidfd comes from the bus daemon; a caller in another pid namespace has none that names a visible process and is
  anonymous.
* An unsandboxed process can connect to the bus, pass the connection's descriptor on and then `exec` a program: the daemon
  reads the process at the time of the call, so a process that then runs `/usr/bin/cupertino-settings` counts as the settings
  application. It gains nothing it could not do by writing the person's files.
* Properties are not checked.

## Tests

`cargo test -p cupertino-peer`: anonymous (no descriptor, a disagreeing process id, an unclassifiable root), host, Flatpak
(a scratch `/proc`) and Snap (the label), the relations, the parent from `status`; `same_instance` (one process, a
process that exited, the same Flatpak instance through a proxy, another instance or application id, a missing or
incomplete `.flatpak-info`, sandboxed against unsandboxed both ways, two Snap processes), credentials without a
`ProcessFD`, the peer of a socket, and a Wayland client classified by its label and root.
