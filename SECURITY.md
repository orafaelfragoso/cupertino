# Security policy

## Supported versions

Cupertino is pre-1.0. Only the latest release receives security fixes.

## Reporting a vulnerability

Do not open a public issue for a security problem.

Report it privately through the repository's **private vulnerability reporting** (Security tab, "Report a vulnerability"), or by email to rafael@workbits.io.

Include what you found, the version or commit, how to reproduce it and what an attacker gains. We aim to acknowledge a report within 7 days and to tell you the plan within 30 days. We credit reporters who want credit when the fix ships.

## Scope

In scope:

- `cupertino-sessiond`, the root-owned session manager: its login socket, PAM handling and session and terminal management.
- `cupertino-netd`, `cupertino-btd` and `cupertino-powerd`, the daemons on the system bus, and how they authorise callers.
- `cupertino-login` and the lock screen, including anything that lets a locked session be bypassed.
- The compositor's handling of untrusted Wayland clients (input isolation, clipboard, screen capture, session lock).
- The private shell protocol and the D-Bus interfaces under `org.cupertino.*`.

Out of scope: problems in third-party software that Cupertino runs or depends on (report those upstream), attacks that need root or the user's own session already compromised, and denial of service by a client that the user chose to run.

## Design notes that matter for reports

- Privileged work stays in the daemons. They authorise a caller by its D-Bus peer credentials and by whether it is the active seat user.
- Secrets typed into the login screen, the lock screen, the authentication prompt and the keyring's password prompt are zeroed after use and never printed in debug output. The processes that hold them (`cupertino-sessiond`, `cupertino-login`, `cupertino-polkit-agent`, `cupertino-keyring-prompter`) zero every block of memory they free.
- A process that acts on a D-Bus signal checks who sent it when it arrives: the bus daemon for `NameOwnerChanged`, the connection that owns the service's name for anything else. A match rule's `sender` alone keeps out nothing that is sent to a process's unique name.
- A failed password at the lock screen counts towards the account lockout: the lock screen authenticates through the distribution's shared PAM stack, where `pam_faillock` lives.
- Mutating calls on `org.cupertino.Session1` check the caller's process identity. Settings setters accept the trusted Settings executable, the supervised session agent or the server. Quit and force-quit calls require the session agent or server. Flatpak and Snap callers are always refused. A caller whose sandbox cannot be read is accepted only while it is a running supervised agent. Settings trust is based on the executable path. This is not a boundary against unsandboxed code running as the same user. The checks are in `crates/cupertino-server/src/caller.rs` and `quit.rs`; tests cover authorization and refusals.

## Disclosure

We prefer coordinated disclosure: a fix and a release first, details after. If you plan to publish, tell us the date so we can ship before it.
