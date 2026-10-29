# Introduction

Cupertino is a Wayland compositor and desktop shell for Linux, written in Rust on Smithay. It is made of small processes around one compositor: a dock, a menu bar, a launcher, notifications, a control panel, a file manager, a settings app, a lock screen and a session manager.

Cupertino serves Wayland clients, and X11 programs through `xwayland-satellite` when it is installed. The project is early (0.x): the compositor runs nested in another Wayland session and on a TTY, and the TTY backend has run in a virtual machine but not yet on real hardware.

- [Trying Cupertino](trying.md): what works, the open p0/p1 defects, and how to report a bug.
- [Building and running](building.md): get a nested session up in a minute, then the TTY session.
- [Architecture](architecture.md): processes, layers and the rules between them.
- [Asset packs](asset-packs.md): swap fonts, icons, cursors and wallpapers, or build your own pack.

Each crate has a README with its API, files and tests. The repository root has the contribution rules (`CONTRIBUTING.md`), the [contributor licence agreement](https://github.com/orafaelfragoso/cupertino/blob/main/CLA.md), the security policy (`SECURITY.md`) and the licence (`LICENSE`, `GPL-3.0-or-later`; the library crates listed in the README carry their own `MPL-2.0` `LICENSE`).
