# cupertino-peer

Who is on the other end of a session bus call. Every client of the session bus runs as the same user, so the user id says
nothing; the caller is the process behind its connection.

* `identify(connection, sender) -> Caller`: asks the bus daemon for the connection's credentials and takes the process from
  the `ProcessFD` it gives. The descriptor is held as a `ProcessHandle` (a pidfd) while the call is judged, so a process id
  that is reused meanwhile is never mistaken for the caller. A daemon older than 1.15 (Ubuntu 24.04 ships 1.14) gives no
  descriptor; the pidfd is then opened for its `ProcessID`. A caller with no process at all, with a process id that
  disagrees with the descriptor, that cannot be classified or that has exited is `Caller::Anonymous` and trusted with nothing.
* Sandboxes are told apart the way the desktop portal does it: a process with `/proc/<pid>/root/.flatpak-info` is a Flatpak,
  one whose `LinuxSecurityLabel` starts with `snap.` a Snap. `Caller::unsandboxed` refuses both and the anonymous caller
  (`Denial`).
* `Relation` and `Caller::require_relation`: a service of the shell answers its own process and its parent, the server that
  started it. Other shell processes ask the server to call for them. A process with the same parent is no relation: the
  server starts the applications too (`spawn_prepared`), so "started by the server" would let every application in.
* `check_call`: the one call a method of such a service makes first. A refusal is logged once with the interface, the method
  and the process id, never the arguments, and returns a `CallerError`, whose names are `org.cupertino.Error.AnonymousCaller`,
  `FlatpakCaller`, `SnapCaller` and `UntrustedCaller`.

Users: `cupertino-server` (`org.cupertino.Session1` and the call to `org.cupertino.Login1`), `cupertino-login`
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
(a scratch `/proc`) and Snap (the label), the relations, the parent from `status`.
