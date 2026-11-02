# Roadmap

Where Cupertino stands at the end of 2026-11-01, when development was frozen for the first install. The work is grouped in phases. This page says what is done, what is started and what comes next, in that order. It is a plan and not a promise: nothing here has a date.

## Done

**Baseline and trust.** A registry of every Wayland global, D-Bus interface and shipped program, checked in CI. A layer graph of the crates. Golden image tests with input replay. Shell roles that the server grants and checks, and caller checks on the session-bus services.

**Transport and scene.** A Wayland client library that speaks only standard protocols, a scene description that the toolkit hands to the compositor, and a seam between the toolkit and the renderer.

**CupertinoKit core.** The toolkit's runtime, layout, text and input methods, the full list of standard controls with fixtures that check their pixels and what a screen reader hears, and accessibility over AT-SPI. Three example apps (Counter, Forms and Virtual List) run on Cupertino and on a second compositor.

## Started

**Apps and portals.** Settings, the on-screen display and the file manager's new window run on CupertinoKit; the other shell programs do not yet (see [What runs on CupertinoKit](apps-on-kit.md)). Kit apps put their menus in the menu bar. The portal backend serves settings, file choosing, screen sharing, the app chooser and screenshots, each with a window that asks first (see [Portals](portals.md)).

**Command line, diagnostics and SDK.** The `cupertino` command has its registry and JSON output but one subcommand ([The cupertino command](cupertino-command.md)). `cupertino-diagnose` checks the GPU, session, portals, services, crashes and logs ([diagnose](diagnose.md)). The C interface to the toolkit has its first library and header.

**Hardware and distribution.** You can install from a USB stick ([Install from a USB stick](install-usb.md)). A signed package repository, a keyring, and rollback to an earlier release exist ([Signed updates and rollback](updates-and-rollback.md)); the release workflow that signs packages still needs a signing key and a repository address. The render GPU can be chosen ([Displays and GPUs](displays-and-gpus.md)). A crash of the compositor logs its reason and a backtrace, and the login screen comes back.

## Next

In order of value for daily use:

- **Apps.** Move the rest of Settings and Files, and then the other shell programs, onto CupertinoKit. In the portals: colour picking, a permission store, retrying a capture, and each app's own icon in the app chooser. One session-bus connection for each app's menus. More screen-reader checks.
- **Command line and SDK.** `cupertino dev` and the rest of the C interface. More diagnose checks, and `diagnose` as a subcommand of `cupertino`.
- **Hardware and distribution.** A host for the signed repository and care of the signing key. Laptops with two graphics chips, and a security check that has to run on real hardware.
- **Daily-driver checks.** Recovery from a GPU reset, and ten checks to run on the installed machine over the first days.
- **Housekeeping.** Pin the keyboard tests to one keyboard-data version, a test that locks and unlocks in a loop, and a flaky pointer-focus test in Settings.
- **AI and voice.** Not started.
