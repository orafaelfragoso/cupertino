# Introduction

Cupertino is an Arch-based Linux distribution with its own Wayland desktop, written in Rust on Smithay. You install it from a USB stick ([Install from a USB stick](install-usb.md)), and its updates are signed packages that can be rolled back ([Signed updates and rollback](updates-and-rollback.md)). The desktop is made of small processes around one compositor: a dock, a menu bar, a launcher, notifications, a control panel, a file manager, a settings app, a lock screen and a session manager.

Cupertino serves Wayland clients, and X11 programs through `xwayland-satellite` when it is installed. The project is early (0.x): the compositor runs nested in another Wayland session and on a TTY, and the TTY backend has run in a virtual machine but not yet on real hardware.

- [Trying Cupertino](trying.md): what works, the open p1 defects, and how to report a bug.
- [Using Cupertino every day](daily-use.md): sessions, shortcuts, screenshots, logs and what to do when something hangs.
- [The cupertino command](cupertino-command.md): the command line for diagnostics, the desktop, the shell processes and the SDK.
- [Building and running](building.md): get a nested session up in a minute, then the TTY session.
- [Architecture](architecture.md): processes, layers and the rules between them.
- [Asset packs](asset-packs.md): swap fonts, icons, cursors and wallpapers, or build your own pack.
- [Desktop quality and open issues](desktop-quality.md): what the installed session still gets wrong, and how each item is accepted.

Each crate has a README with its API, files and tests. The repository root has the contribution rules (`CONTRIBUTING.md`), the [contributor licence agreement](https://github.com/orafaelfragoso/cupertino/blob/main/CLA.md), the security policy (`SECURITY.md`) and the licence (`LICENSE`, `GPL-3.0-or-later`; the library crates listed in the README carry their own `MPL-2.0` `LICENSE`).
