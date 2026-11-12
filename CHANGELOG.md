# Changelog

Notable changes to Cupertino, newest first. The project is pre-1.0: interfaces and configuration files can change between candidates.

## Unreleased

Changes on `main` since `v0.1.0-rc4`.

### Phase assessment

- The roadmap records all seven phases and the separate daily-driver gate. Phases 1–3 have recorded completion audits; phases 4–6 remain incomplete, and AI and voice have not started.
- Settings About uses Kit, and the old Settings painter and window modules are deleted (#69, #92). The Files Kit window adds file operations (#70, #71, #73), multiple windows (#76) and an icon view (#95).
- Portal app icons, activation and screenshot retry are merged (#74, #91, #75). Kit menus share one session-bus connection per app (#72), and app and portal accessibility checks were expanded (#93).
- Release builds sign the ISO checksum and reject a public key that differs from the checked-in primary fingerprint. Local ISO builds still write an unsigned checksum. Public-key distribution remains open.

### Apps and portals

- Every Cupertino app and shell process draws with CupertinoKit. The Dock ([#109](https://github.com/orafaelfragoso/cupertino/issues/109)), the application switcher and the overview's chrome ([#112](https://github.com/orafaelfragoso/cupertino/issues/112)), the login and lock screens ([#110](https://github.com/orafaelfragoso/cupertino/issues/110)) and the screenshot tool ([#111](https://github.com/orafaelfragoso/cupertino/issues/111)) moved to Kit, and Files starts its Kit window; the old Files window is deleted ([#46](https://github.com/orafaelfragoso/cupertino/issues/46)).
- Focus Dock (`Ctrl+F3`, `Super+F3` under the classic PC profile) moves the keyboard to the Dock, and every tile has an accessible name and actions ([#109](https://github.com/orafaelfragoso/cupertino/issues/109)).
- The overview lays its windows out below the desktops strip, balanced and never overlapping, with room under each for its title ([#67](https://github.com/orafaelfragoso/cupertino/issues/67)).
- Files: removable media mount, unlock and eject from the sidebar, and a pulled device sends its windows home; the encrypted-device and policy checks on hardware remain ([#65](https://github.com/orafaelfragoso/cupertino/issues/65)). The rubber band, disclosure triangles, click-to-rename, Ctrl+click menus, a clickable path bar and the View, Tags and Action pop-ups are in, and Copy, Paste Item and moves run off the window's thread with progress and Stop; a move across devices never loses or half-writes an item ([#108](https://github.com/orafaelfragoso/cupertino/issues/108)).
- Files: items dragged out carry an image of the item, the row or a stack with its count ([#68](https://github.com/orafaelfragoso/cupertino/issues/68)), and files dropped from any application land on folders, sidebar places, the Trash or the folder shown. Items show their type's icon from the icon theme, and PNG, JPEG and WebP pictures their thumbnail.
- Kit apps take drops and tell the source whether the drop copied or moved. A Kit toolbar moves its window by drag and zooms it on double click, and moves, resizes and window menus need the pointer press that asked for them.
- Notification cards sit on a material with wrapped bodies, and the notification centre scrolls ([#115](https://github.com/orafaelfragoso/cupertino/issues/115)). A screen reader speaks a banner's app, title and body ([#30](https://github.com/orafaelfragoso/cupertino/issues/30)).
- A click on empty menu bar space closes the open menu and gives the keyboard back to the window ([#113](https://github.com/orafaelfragoso/cupertino/issues/113)). Control Center closes when its display is unplugged and opens on a remaining one ([#114](https://github.com/orafaelfragoso/cupertino/issues/114)).
- The log out, restart and shut down panels have a "Reopen windows" checkbox, and the apps open again at the next login ([#37](https://github.com/orafaelfragoso/cupertino/issues/37)).
- The password field shows when an attempt is being checked and when the last one failed, and takes at most 512 bytes. The power-button panel focuses Cancel, so one Space press shuts nothing down, and a login process restarted after a crash takes over the lock screen instead of leaving a black screen ([#110](https://github.com/orafaelfragoso/cupertino/issues/110)).
- Portals: colour picking on a frozen screen with a loupe, screen-sharing consent that names the app and takes no answer until it has been on screen for 500 ms, displays an app may share again without asking (listed in Settings > Privacy > Screen Sharing with Forget), a menu bar item with Stop Sharing while a share runs, and captures of rotated displays that come out upright ([#116](https://github.com/orafaelfragoso/cupertino/issues/116)).
- gnome-keyring is unlocked through PAM at login and at the lock screen, the Secret portal goes to it, and its collections lock when the session ends ([#117](https://github.com/orafaelfragoso/cupertino/issues/117)). The authentication prompt names the application that asks. The keyring's password questions (unlocking a keyring, a new keyring's password) appear in the same prompt: `cupertino-keyring-prompter` owns the keyring's prompter names on the session bus and returns the password through the keyring's secret exchange.
- Settings > Accessibility > Screen reader, and `Super+Alt+S`, turn the screen reader on and off. Every Kit surface is checked against an accessibility audit in the tests, and the OSD's level, the Files Tab order and the keyboard's return after menus and renames were fixed on the way ([#30](https://github.com/orafaelfragoso/cupertino/issues/30)). Long row labels and section footnotes in Settings wrap ([#35](https://github.com/orafaelfragoso/cupertino/issues/35)).
- Files and Settings repaint only the nodes that changed. At 3840×2160 and scales 2 and 1.5, selection, hover, the caret, renaming and Settings' switches and sliders stay within 8 ms; scrolling, opening a large folder and the search that fills the Files list still take about 60 ms ([#40](https://github.com/orafaelfragoso/cupertino/issues/40)).

### Input

- Apps such as games and virtual machines can lock or confine the pointer, also when they draw in a subsurface. A notice shows while an app holds the pointer, and `Super+Escape` releases it for the whole app.
- An app can ask to receive the system shortcuts. A release chord that no configuration can remove gives them back, and a notice stays visible while the app holds them.
- Two modifier profiles, Cupertino and classic PC. System shortcuts match the physical key, with a table for each profile.
- Under the classic PC profile, tapping `Super` alone opens the launcher (a gesture or a mouse-keys key cancels the tap), `Super+Down` restores a tiled or zoomed window before it minimizes, `Ctrl+Tab` moves between key views in text views, the menu bar shows PC key names, and `Ctrl` is the command key in Settings and in text fields.
- Dead keys and the compose key type their characters, and key repeat skips a key that typed inside a compose sequence. The keyboard layout comes from the one localed holds at login, and a compose key can be set in `keyboard.conf`.
- Touch input from `wl_touch` reaches the window the touch went down on.
- Natural scrolling can be set per device, and the mouse wheel has its own setting. A pointer button carries the modifiers held at the press.
- A Kit window publishes its menus to the desktop's menu bar, and an item chosen there sends its message back to the app.

### Text

- A character the first font lacks is drawn from another loaded font, one character at a time.
- Emoji written with the emoji presentation selector are drawn from a colour font, and colour glyphs are cached once per size.
- Mixed-direction lines are laid out in UAX 9 order, each hard line of a paragraph takes its own base direction, and the arrows and `Option+Left` and `Option+Right` move through text as it is drawn.
- A line that is cut can end in an ellipsis, and an installed font answers only with a face that has the character or family asked for.
- Caret and word moves lay out only the caret's line and its neighbours.

### Menus

- The system menu has its items, live shortcuts, Recent Items and About This Computer. Lock Screen is always in it, and it closes when the session agent goes away ([#43](https://github.com/orafaelfragoso/cupertino/issues/43)).
- Session options the session agent doesn't offer are shown disabled. If they can't be read, every option stays offered, and a failed later read keeps the previous ones ([#42](https://github.com/orafaelfragoso/cupertino/issues/42)).
- An app's exported menus are merged with the system menus, and an app without menus gets standard fallback menus. Edit menu items go to the key window.
- Files and Settings publish their own menus.
- Menus follow the focused surface only. GTK menus follow their app's menu and action changes live and keep a literal `&`, and a menu that failed to load is loaded again. The app menu services are offered only while a menu bar runs.
- Help searches every enabled menu item, and `Shift+Super+/` opens it with the search field focused.
- Holding `Alt` in the Window menu offers Minimize All, Zoom All, Arrange in Front and Close All, and the menu bar offers Force Quit for the current app.
- Menu shortcuts are drawn as symbols the embedded font has, Escape included.
- The menu bar text stays readable over the wallpaper under it, the built-in procedural wallpaper included.
- The menu bar draws a scrim under its text that brings it to 4.5:1 over mid-tone wallpapers, and the wallpaper worker measures the strip under the bar.

- The Dock and the window switcher show the icon a window sets for itself.

### Settings, Files and theme

- An About This Computer window in Settings. Long values wrap, and each display shows its name, diagonal, mode and scale.
- Settings keeps running on a compositor without window activation.
- Files opens files through the shared app launcher and file typing.
- Files and Settings redraw only what changed, including Files' content rows, drop outline and selection rectangle ([#40](https://github.com/orafaelfragoso/cupertino/issues/40)).
- Files highlights a sidebar place as a drop target, with an outline when Increase Contrast is on.
- Neutral palettes reach 4.5:1 for text and 3:1 for borders and the focus ring. Switches use the accent of the buttons beside them, scroll bars stay visible on text backgrounds, and high-contrast buttons with a fill always draw their border.
- Reduce Transparency uses the opaque neutral palette.
- Choosing the theme that is already set changes nothing.
- Cupertino no longer writes GTK and Qt themes or reads the GTK icon-theme setting.
- Settings has Wi-Fi, Network and Bluetooth panes on NetworkManager and BlueZ, hidden when the service is missing. The Sound pane picks the output device among the PipeWire sinks, and a Privacy & Security pane has Clipboard Access.
- Files lists removable media in the sidebar through udisks2. The sidebar works from the keyboard (`Ctrl+Tab`, the arrows, `Return` and `Primary+E`) and shows its places and eject buttons to assistive technology. Failures show as notifications, and the Trash place is followed.
- Settings opens as a CupertinoKit window: the sidebar, a toolbar with back, forward and search, the panes' sections as Kit controls, sheets as dialogs, and its menus on the menu bar.
- Files has a browser window on CupertinoKit: places, toolbar, the file list as a table with sort and selection, path and status bars, the menu bar (actions it cannot do yet are disabled) and live folder updates. A sidebar item can carry a description, a busy state and a trailing button such as eject.
- The on-screen display for volume, brightness and keyboard backlight is a CupertinoKit window on a layer surface.
- Control Center now draws through CupertinoKit with module toggles, tiles, a battery meter and level sliders (#88). Launcher (#90), menu bar (#96), and notification banners and centre (#89) also use Kit.

### Shell and session

- X11 apps run through xwayland-satellite, started on first use and supervised. A quick exit backs off before the next start. Toolkits are asked to prefer Wayland, and logging out removes only the hints Cupertino set.
- A session started from the display manager is supervised, and a crash returns to the login screen. The session sets up its activation environment and its own session target, and restarts back off.
- Starting from a text console checks the hardware first, writes a log file and runs under a watchdog script.
- Explicit synchronization on the text-console backend, with bounded waits.
- After a VT switch, a frame lost during the switch is dropped, and the lock screen counts as shown only once a frame is on screen.
- Each monitor is paced to its own refresh rate and redraws on its own.
- When idle, the compositor sleeps until its next real deadline instead of waking every 4 ms, and child processes wake it only when they exit.
- The CPU renderer resizes a window and opens the window switcher within one frame at 4K.
- A failed frame keeps apps paced. On a display, a GPU reset makes a new renderer and the windows and clients stay; each reset is a line in `session.log` and `cupertino-diagnose` reports it as `crash.gpu_reset`. A fourth reset within a minute, or a renderer that cannot be made, ends the session with an error that names the lost context. This has run only through the offscreen test hook, not on hardware.
- A buffer that is already ready is used without a wait or a log line.
- A new window takes the keyboard only by the focus policy, never while you type or while the session is locked. Windows move and resize only from a real press on themselves. A process's right to focus its first window ends when its last connection closes ([#20](https://github.com/orafaelfragoso/cupertino/issues/20), [#44](https://github.com/orafaelfragoso/cupertino/issues/44)).
- An app can bring itself forward only while it has focus or an activation token. Otherwise its dock tile bounces.
- A window whose app stops answering shows the busy cursor, and the dock marks it Not Responding. Force Quit is `Super+Alt+Escape` ([#20](https://github.com/orafaelfragoso/cupertino/issues/20)).
- A window that can't zoom or has a fixed size refuses full screen, the shortcut always leaves full screen, and View > Exit Full Screen works.
- Apps can disable their minimize and zoom buttons. The buttons dim and the matching menu items are greyed out.
- Logging out reaches the login process over the server's private connection. A log out that reaches nobody shows a notice, and Lock is disabled when there is no login process.
- The launcher and the login screen honour Reduce Motion. A failed password keeps its outline until you type again.
- A machine-wide accessibility file, `/etc/cupertino/accessibility.conf`, is read by every session and by the login screen. An administrator at the console can set it through the session manager.
- Animated theme cursors play, and stop when the pointer leaves, hides or changes shape, theme or seat.
- A rotated monitor shows its wallpaper upright, and its screenshots and screen captures come out upright.
- Screenshots are written in one step and never replace an existing file, and a failed write, for example on a full disk, leaves no partial file.
- The network daemon starts without rfkill access and reports the real error. The Bluetooth daemon keeps running on a machine without Bluetooth.
- When every `wayland-N` socket name is taken, the server uses a name of its own.
- The authentication prompt: `cupertino-polkit-agent` shows a password panel over the session, starts its helper when nothing answers on polkit's socket, and tells assistive technology what it shows. It stays out of screen captures and keeps the keyboard under the lock.
- `cupertino-portal` is the xdg-desktop-portal backend, with the appearance settings and a file chooser drawn with CupertinoKit. A portal request opens as a child of the window it names, Save As opens with its name field focused, and a filter pattern, the number of open dialogs and app text are bounded. Screen sharing works through the portal's ScreenCast: an application shares one display or one window as a PipeWire video stream after you pick the source in a chooser (shared memory only, with no audio and no region source). A `portals.conf` sends Settings and FileChooser to Cupertino first and then the GTK backend, ScreenCast, AppChooser and Screenshot to Cupertino only, and every other portal interface to the GTK backend.
- The session starts the user's default terminal, and the portal and the polkit agent start as built-in processes with one role each. Before every start of a built-in program, the session checks the file and refuses links and a shared bin directory.
- A bad line in `login.conf` no longer stops the login manager, and `cupertino-sessiond --check-config` names ignored keys and unreadable files.
- The volume overlay, the control panel and the Sound pane set the volume of the default PipeWire sink. The microphone mute key toggles the default source and shows the microphone overlay.
- The displays of every DRM card are shown, and the frames of the render GPU are copied to the other cards. A display that cannot show its mode falls back, and an output is announced only once its backend shows it.
- A crash or an abort in the server leaves its reason and a backtrace in the log, a crash in the teardown keeps the first reason, and the login screen comes back when the compositor ends.
- The server supports `wp_content_type_manager_v1`, which keeps the content type a window sets for itself.
- A pinned desktops strip in the overview reserves its expanded height, so windows clear it.
- Data control is offered only to the clipboard role and to applications the person allows.
- The server supports `xdg-foreign-unstable-v2`, `xdg_toplevel_icon_manager_v1` and `wp_single_pixel_buffer_manager_v1`. A frame holds the client buffers it was drawn from until its flip is done, and renderers create no GL fences.
- The lock screen publishes its tree to assistive technology, with the answer to an attempt announced. The password field is protected: assistive technology gets neither its text nor its length and can't type into it. The appearance observer (`cupertino_app_effects_v1` version 5) tells whether the session is locked.
- `CUPERTINO_DRM_DEVICE` picks the render GPU by card or render node. A card that is plugged in is read from sysfs once.
- `cupertino-diagnose` exits 0 once its checks ran, whatever they found, and a wrong argument exits 2. A disabled `cupertino-netd` or `cupertino-btd` counts as ok while NetworkManager or BlueZ does its job.
- The portal asks before it acts: the app chooser asks which app opens a link or a file, and the screenshot portal asks before it captures the screen.

### Security

- Shell roles are bound to the process the session started, granted only by the root-owned system configuration, and checked on every request, also after the holder has exited.
- Session-bus services check their callers. On a bus older than dbus 1.15 the caller is identified by its process id, and the log says so ([#51](https://github.com/orafaelfragoso/cupertino/issues/51)).
- Sandboxed apps get no clipboard control, output management, layer shell or window list. While the session is locked, clipboard changes are refused.
- The notification centre refuses sandboxed and unnamed callers.
- Locking the screen stops screen captures, and only a process with the capture role may capture.
- The login process clears the memory it frees. The login and lock screens can't be dumped, and a login screen killed in the middle of an attempt ends that attempt.
- A lock request reaches the lock screen even when another process holds the login bus name.
- Force Quit acts only on the process the kernel named when it connected.
- The server checks that each announced menu belongs to the window's app, and only the verified menu bar may read or use an app's menus.
- About and Settings hand a launch only to their own program, and claim their bus name again if its owner left.
- Size limits before data is used: a keymap larger than its file is refused, text from apps for the dock, window manager and menu bar is cut to 1024 bytes, menu announcements are capped, and cursor theme files and glyph images are checked before anything is allocated.
- At most 64 backdrops per surface and 128 per app ([#48](https://github.com/orafaelfragoso/cupertino/issues/48)).

### Command line

- The `cupertino` command: a typed registry with help, version and JSON output, and one subcommand, `appearance assets check`, which replaces `cupertino-assets check`. Exit codes 2 to 8 name the kind of failure.
- A development folder for working on the desktop from inside its session: `sudo cupertino dev-folder enable` declares it once in `/etc/cupertino/compositor.json`, and the dock, menu bar, launcher, notifications, control panel, OSD and window manager then start a build placed there instead of their installed program. `cupertino shell list` shows what each shell process runs, `cupertino shell restart NAME` ends one and starts it again at once, and `cupertino shell use-installed NAME` goes back. The lock screen, capture, the authentication prompt, data control and desktop control never start from the folder.

### CupertinoKit

- CupertinoKit, the toolkit for apps, and everything it depends on are MPL-2.0, and a check keeps it so. The desktop stays GPL-3.0-or-later.
- `cupertino-wayland-client` talks to the compositor over standard Wayland protocols only, with text input, presentation feedback, popups that can move, and theme cursors when a compositor has no cursor-shape support.
- Cupertino's own shell processes and apps run on that client.
- The toolkit has a runtime, a Wayland window system, two-pass layout (stacks, grids, scrolling, right-to-left, virtual lists), a CPU renderer, text shaping and paragraph layout, and theme tokens with light, dark, neutral and high-contrast palettes.
- Controls: Toggle, Button, TextField, SecureField and Text. The Counter and Forms examples run on Cupertino and on a second compositor.
- Hidden and sensitive text stays protected: secure text buffers and input hints are kept out of logs and recordings.
- Apps start in the session's light or dark appearance and follow it, and a switch redraws every part of the window.
- The text caret blinks and stops after 10 seconds idle, and double and triple click select text.
- A held key press is matched by the physical key, and leaving a window cancels it.
- Clipboard reads don't block the app: each waits up to a second for more data and at most 10 seconds in all.
- Drag and drop (the transport only so far).
- Screen readers see the app's accessibility tree, including live regions that announce changes. Password text never leaves the app.
- Standard menus, with a menu model that apps export to the menu bar.
- Colour roles for scroll bars, dialog backdrops, links and page dots, and surface materials.
- A window can leave out its plain background, so a menu or popover can paint its own shape.
- A headless test window system that records and replays input, with protected text left out.
- More controls: Selection, ListView and TableView (virtualised rows, keyboard and click selection, type-select, sortable and keyboard-focusable column headers), TreeView, ScrollView with overlay and always-shown scroll bars, SplitView and NavigationStack, Stepper, DisclosureButton, Badge, PageDots, Tabs, Sidebar, Tooltip, Menu, MenuItem, ContextMenu and PopUpButton, Modal, Dialog and Alert, TextBox, a multi-line TextEditor, Popover and popup windows, and WindowGroup (one window per instance).
- Assistive technology sees list and table sizes and each row's place, a table's header and body as one table, tree levels, and the actions of its nodes. A tab controls its panel, and a tooltip describes its anchor.
- Controls lay out on their visible chrome, and a widget's hit target can reach past its rect (the minimum target is hit-only). On a selected row, the focus ring and a tree's disclosure triangle take the selected text colour, and a list row's focus ring stays inside the row.
- Focus: an inactive window shows no focus ring, a focus left in a hidden subtree moves to the next Tab stop, and `Escape` goes to dismissible nodes first. A sheet hides the content behind it and focuses its least destructive button.
- Right-to-left text and password fields keep their text and caret at the leading edge, and a text field takes the environment's direction.
- Applications start on the neutral theme without the desktop's theme data. The Editor example runs on Cupertino and on a second compositor.
- Kit: a split view follows the window's size, a scroll view whose content fits is no Tab stop, a popover with no control focuses its dialog, an element can fill what its parent and siblings leave, and a key right after a move of the roving focus goes to the item it moved to (Return after Down in an open menu chooses the next item). An update can ask for an activation token for another application.
- `SurfaceRoles` lets a window system give a Kit window roles from protocols it does not speak, and `kit::app::run_on` runs an app on it.
- Assistive technology hears a table's sort order on its column header, and gets a text field's caret and selection, in characters. A selection set from the assistive technology reaches the field. `cupertino-ui-test` lists what a screen reader could not announce in a tree (`semantic_gaps`).
- A first C interface: `libcupertinokit.so.1` with its header, symbol list and pinned struct layouts. It is a skeleton.

### Packaging and release

- The Adwaita icon theme is a required dependency.
- The default asset pack uses the built-in cursors.
- Package licence fields and the shipped `THIRD-PARTY-LICENSES` cover every linked licence.
- Debian packages ship stripped binaries.
- An upgrade test installs one version and then the next for deb and rpm.
- All three formats recommend xwayland-satellite and Xwayland. Debian does not package xwayland-satellite.
- The Arch packages can be signed. The `cupertino-keyring` package carries the signing key, `cupertino-rollback` installs an earlier release from `/var/lib/cupertino/previous`, the package cache or an archive, a pacman hook keeps the last release, and `build-repo` writes the signed repository tree (it copies archived packages into the output and verifies the copy, pins their primary key and signs `SHA256SUMS`). The release workflow signs the packages and attaches the signed repository to the draft release; it needs the signing secret and a repository address. Nothing is published yet.
- Every package format requires dbus 1.16 or newer and PipeWire 1.4 or newer.
- Machines installed from the USB image update from their own signed repository: `packaging/keyring/local-release` builds and signs a release, then `sudo pacman -Syu` installs it. pacman refuses an unsigned, tampered or foreign-signed package.
- Arch is the only target. The Debian and Fedora packages, their PAM files, builder images, install tests and release builds are removed; a final release builds the Arch packages and the ISO. `release.py check-metadata` checks the third-party notices, the shipped licence texts and the PKGBUILD licence lists.

### Install image

- `cupertino-install` is the installer on the live USB. It erases one disk you confirm by typing its path, installs Arch Linux with the Cupertino session started by `cupertino-login`, and uses ext4 without encryption. It installs NVIDIA drivers on Turing and newer cards, Chromium, `git`, `base-devel`, `rustup` and `gh`. It fails if the new system keeps anything of the live USB.
- An archiso profile and `scripts/build-iso.sh` build the live and install medium.
- The boot menu has "Cupertino" and "Cupertino (recovery: text login)", and tty2 to tty6 give text logins.

### Tests and CI

- Golden image tests of headless scenes in a pinned renderer image, with input replay. The login screen, launcher, power panel and overview have goldens, the CPU renderer has its own, and `ci/goldens.sh --update` checks each test's result.
- A registry of every Wayland global, D-Bus interface and shipped binary, checked at run time and in CI.
- Crate manifests declare each crate's layer, and one check enforces the layer rules.
- CI also runs the tests in the package build environment ([#41](https://github.com/orafaelfragoso/cupertino/issues/41)).
- Every shipped binary is started in the tests, and the server starts headless in CI.
- A test that needs EGL or D-Bus fails instead of skipping when the backend is missing.
- Tests use private D-Bus daemons and never change the process environment.
- A render timing matrix covers the window switcher and window resizing.
- Timing-sensitive tests count work instead of measuring time.
- A debug-only test clock for the time shown in the login screen, dock and launcher.
- `ci/capture.sh` draws Settings, Files and Kit surfaces headless for look checks, and `ci/demo-video.sh` records a scripted desktop at 60 frames per second.
- The vendored xkbcommon-rs tests build again.
- A gate check fails when a library's wayland-scanner XML is not embedded in its crate. The build now regenerates protocol code when the XML changes, the layer graph is regenerated, and the contracts registry covers the portal, the polkit agent, the pairing agent and `cupertino-diagnose` (the gate runs the contracts check).
- The lock screen's AT-SPI test subscribes to its monitor before the lock screen can send.
- The check job fetches the crates before the layer rules read the dependency graph offline, and runs on Ubuntu 26.04, where the package-environment tests pass.
- The goldens drop the power panel scene before the lock scene, so a lock reaches the new login process.
- The dock golden shows window icons at their resting and magnified sizes.
- CI builds, lints and tests in an Arch container (`archlinux:base-devel`, pinned by digest) as an unprivileged user; the Ubuntu runner only hosts it. `ci/arch-ci.sh` lists the packages, for the workflows and for contributors.
- The golden renderer image is Arch, with every package from one Arch Linux Archive date. Its baselines must be re-recorded on it; the Record goldens workflow does that and uploads them for review.

### Documentation

- The contributor agreement and how to accept it.
- X11 support and its limits.
- The local changes to the vendored xkbcommon-rs.
- Pages in the book: install from a USB stick, the `cupertino` command, `cupertino-diagnose`, portals, signed updates and rollback, displays and GPUs, what runs on CupertinoKit, and a roadmap. The daily-use guide says how a USB-installed machine starts the session and links the install page, and the CupertinoKit guide lists the published caret and selection.

## v0.1.0-rc4 — 2026-10-27

Tagged on `da978f9`. This entry covers the changes since `v0.1.0-rc3`.

### Menus

- A system menu at the left end of the menu bar holds the session actions, and app shortcuts are drawn as symbols ([#19](https://github.com/orafaelfragoso/cupertino/issues/19)).

### Files

- Files search uses the shared text field. While a search runs the field is wider, and an empty result shows No Results ([#39](https://github.com/orafaelfragoso/cupertino/issues/39)).
- The launcher copies through the app clipboard.

### Shell and session

- Sleep and a power dialog with Restart, Sleep, Cancel and Shut Down. The log-out shortcut asks first. While the login process restarts after a crash, the shortcut ends the server without asking ([#19](https://github.com/orafaelfragoso/cupertino/issues/19)).
- Live reload reads `/etc/cupertino/compositor.json` under the user's own file ([#33](https://github.com/orafaelfragoso/cupertino/issues/33)).

### Tests and CI

- The Settings popup tests pass in the package build.
- `ci/replace.py` fails when a pattern matches nothing, and writes every file or none.

### Documentation

- The release-candidate hardware checklist covers suspend, the lid, monitor hotplug and the wallpaper.

## v0.1.0-rc3 — 2026-10-27

Tagged on `c47d32a`. This entry covers the changes since the initial import (`cf9f5af`).

### Input

- Pointer acceleration is designed on physical speed and can be turned off, so a mouse moves predictably at every speed ([#1](https://github.com/orafaelfragoso/cupertino/issues/1)).
- One wheel notch scrolls three lines ([#2](https://github.com/orafaelfragoso/cupertino/issues/2)).
- The natural-scrolling setting applies live to wheels and touchpads ([#3](https://github.com/orafaelfragoso/cupertino/issues/3)).
- Settings > Mouse shows a resolution row per connected mouse, read from the session, with a typed value in a sheet.
- A press outside a popup chain dismisses it and is consumed; popup subsurfaces take input and belong to the chain; a session lock ends the grab ([#5](https://github.com/orafaelfragoso/cupertino/issues/5)).

### Text

- One shared single-line text field provides a caret, selection, clipboard, undo and a per-field length limit. Settings search, the launcher and Files rename use it ([#16](https://github.com/orafaelfragoso/cupertino/issues/16), [#4](https://github.com/orafaelfragoso/cupertino/issues/4)).
- Pasted and composed text is sanitised; a paste is capped at 1 MiB and served off the event loop.

### Menus

- Open menus in Settings and Files work from the keyboard: arrow keys, Return, Space, Escape, `Super+.` and type-to-select ([#18](https://github.com/orafaelfragoso/cupertino/issues/18)).
- Settings closes an open menu on an outside press, a pane change or an appearance change ([#5](https://github.com/orafaelfragoso/cupertino/issues/5)).

### Settings

- A Wallpaper pane with a grid of the packs' and added folders' pictures, a Current preview, and an Add Folder and Add Photo chooser with folder and picture icons, hover and wheel scrolling ([#7](https://github.com/orafaelfragoso/cupertino/issues/7)).
- Image wallpapers are chosen per appearance and decoded once on a worker; the procedural fallback is unchanged, and a decoded picture is released when no output wants it.
- A picture's title and photographer come from the pack's `credits.txt`.
- Folder and picture listings run on threads and show Loading; one listing per folder runs at a time, and a listing that cannot start releases its folder.
- Relative wallpaper paths are refused, the credits file is capped, and thumbnails have size and picture limits; the shared thumbnail cache is used by Files and Settings.
- The content and toolbar background matches Files ([#8](https://github.com/orafaelfragoso/cupertino/issues/8)).
- Failed bus and file operations show plain text, with the system error in the log; search matches every word in any order; accessibility controls show the values the server holds.

### Shell and session

- An installed session starts the full shell: `/etc/cupertino/compositor.json` supplies the process list, under the user's own `compositor.json` when there is one ([#17](https://github.com/orafaelfragoso/cupertino/issues/17)).
- `run-cupertino.sh` starts the compositor and every shell process from a text console, with a fresh session bus and a log at `~/cupertino-tty.log`.
- `capture.sh` renders the desktop in a fixture home and never starts the login agent or touches the host session ([#10](https://github.com/orafaelfragoso/cupertino/issues/10)).
- The lock agent is started in the installed and TTY sessions.

### Packaging and release

- One manifest builds Arch, Debian and RPM packages. `cupertino-data` carries the default asset pack, and desktop entries ship for Cupertino's own apps.
- `/etc/cupertino/compositor.json` is installed as a config file in all three formats.
- `v<version>-rc<N>` tags build a draft prerelease with packages, a source archive and `SHA256SUMS`.
- Arch packages build without debug information so the check build fits a hosted runner.

### Tests and CI

- The suite runs under nextest with a two-minute limit per test, and the hanging menu bar D-Bus test is fixed at its cause ([#11](https://github.com/orafaelfragoso/cupertino/issues/11)).
- Keyboard-map tests use pinned keymap data instead of the host's ([#14](https://github.com/orafaelfragoso/cupertino/issues/14)).
- The session settings setter test no longer caches a failed first read ([#15](https://github.com/orafaelfragoso/cupertino/issues/15)).
- CI builds the docs book with mdBook 0.5.4 and checks its internal links with lychee 0.24.2 ([#12](https://github.com/orafaelfragoso/cupertino/issues/12)).
- The test build carries no debug information, keeping the CI job inside the runner's disk.

### Documentation

- Public README, CONTRIBUTING (CLA process; the CLA text link is still to be set), SECURITY, Code of Conduct, and a docs book covering architecture, building and running, asset packs and trying Cupertino.
- Bug report and feature request forms.
- A release-candidate hardware checklist.

### Known missing

Open p0 and p1 issues at this tag: [#6](https://github.com/orafaelfragoso/cupertino/issues/6), [#19](https://github.com/orafaelfragoso/cupertino/issues/19), [#20](https://github.com/orafaelfragoso/cupertino/issues/20), [#21](https://github.com/orafaelfragoso/cupertino/issues/21), [#22](https://github.com/orafaelfragoso/cupertino/issues/22), [#23](https://github.com/orafaelfragoso/cupertino/issues/23), [#24](https://github.com/orafaelfragoso/cupertino/issues/24), [#25](https://github.com/orafaelfragoso/cupertino/issues/25), [#26](https://github.com/orafaelfragoso/cupertino/issues/26), [#27](https://github.com/orafaelfragoso/cupertino/issues/27), [#28](https://github.com/orafaelfragoso/cupertino/issues/28). The release-candidate tracking issue is [#13](https://github.com/orafaelfragoso/cupertino/issues/13).

### Install or upgrade

From a checkout, on a text console (`Ctrl+Alt+F3`):

```bash
./run-cupertino.sh
```

For an installed session, download the Arch packages `cupertino` and `cupertino-data` from the `v0.1.0-rc3` release and install both from the same candidate:

```bash
sudo pacman -U cupertino-0*.pkg.tar.zst cupertino-data-*.pkg.tar.zst
```

Both packages are replaced together. `/etc/cupertino/compositor.json` is a package config file, and files under `~/.config/cupertino/` are read as before.
