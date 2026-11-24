# cupertino-shell-overview

The overview of the Cupertino desktop: every window of the output's current desktop laid out side by side, the desktops
band across the top, and the application overview of one application's windows.

The crate is a library: one component, `Overview`, which the shell host (`cupertino-shell`) runs as `overview` of
the server's component table. The layout (`layout`, on the packing of `cupertino_wm::overview`) is pure; the
surface (`chrome`, `parts`) is drawn by CupertinoKit; the component (`component`) speaks to the server only through
`cupertino-kit-shell`: the overview role's feed and requests, holes and transactions.

## Running

The server lists `overview` to the shell host, which runs it; nothing starts it on its own. `compositor.json` names it
among the `components` of its `shell` section (`docs/architecture/shell-host.md`), and `alone` runs it in a host of
its own (`cupertino-shell --only overview`). A development build of the host makes it panic on request
(`cupertino shell crash overview --at paint|dispatch|helper`).

```
cargo run -p cupertino-shell-overview --example overview_capture -- overview/rest --sheet   # light and dark
cargo run -p cupertino-shell-overview --example overview_capture -- overview/transition --progress 0.5
cargo run -p cupertino-shell-overview --example overview_capture -- overview/workspaces
```

A capture has no server: each hole is painted a flat colour where the server would draw the window or the desktop, and
the band's material, which the shell draws in a window of its own, is its veil without the blur.

## What a restart loses

A failed component is started again alone by its host (D16, D21). The overview's surface is transient: when its
component stops, the server dismisses it and the windows it claimed go back to their places along the server's return
motion (D19). Lost with it:

- an overview on screen, and the way in or out under way: the person opens it again; the host bus says the overview
  gone in its name as the component stops, so the menu bar comes back at once;
- a gesture under way: the server settles it back, and the rest of that touchpad gesture stays with it;
- the armed gestures: the new instance arms them again once it has the feed, so a swipe made before that reaches the
  dock, which runs its action through the server's toggle.

The windows, desktops and their order are the server's, so a restart changes none of them.

## How it works

**The way in.** For every output whose `dock.conf` gives the swipe up the `overview` action, and the swipe down
`app-overview`, the component keeps a transaction armed for that gesture (`bind_gesture`). Its surface is staged and
not shown: a hole per window, laid out at the window's own frame and claiming it, so the hole takes the window over with
no change on screen, and layer animations that take each hole to its place in the overview and fade the chrome in. The
server drives them with the fingers (start + progress x (target - start)) and settles the release along
`SpringPreset::OVERVIEW_OPEN` from its velocity: completed, the overview is open; reverted, the surface goes and the
gesture is armed again. The same transaction keeps the reverse gesture (`bind_reverse_gesture`), which takes the
overview back. When the windows change, the armed surface goes at once and comes back once they kept still for 250 ms
(`REARM_QUIET`): a swipe in between reaches the dock. The server's toggle (the shortcut, a hot corner, a named action,
the dock's Show All Windows) opens the same surface with a transaction of its own, critically damped over 0.25 s. The
server ends every transaction once its motion has run and says so (`finished`): the toggle's way in is open, and every
way out over, when it does, as a gesture's are when it comes to rest. An armed surface whose first frame comes late is
kept for the gesture all the same: it never shows by itself.

**Open.** The holes are laid out at their places, with the keyboard. A change of the windows or of the band moves a hole
to its new place along `SpringPreset::OVERVIEW_REFLOW`; a new window's hole shows at its place; a window that left the
set keeps its hole for 500 ms (`DEPARTED_GRACE`), in which the server fades the hole of a window that closed and says
so. The desktops band opens with the pointer at the top, and each desktop's picture there is a workspace hole. A
minimized window of the application overview shows in a thumbnail hole, which claims nothing. At most 64 holes show
(`MAX_HOLES`), the desktops' pictures first.

**The way out.** A pick (click, Return or Space on the selection), a desktop, the background or Escape starts a
transaction that takes every hole back to its window's frame and fades the chrome out; the picked window is activated,
or the desktop switched to, at its start (`kit_shell::overview`). At its end, once the holes are back and the switch or
the activation has run, the surface goes and the windows show in their places again. Dragging a window onto another
desktop of the band moves it there in a transaction of window changes alone, kept until it finished; dropped anywhere
else it goes back to its place.

**The band.** Nothing lies behind the windows: the desktop shows as it is, and only the desktops band across the top
has a backdrop, the desktop blurred (σ 30 pt) and saturated under a light veil (gray 0.9646 at 36 %, saturation 1.8) or
a dark one (gray 0.1569 at 40 %, saturation 1.6), the theme's `OverviewBand` material, with a shadow under it, a dark
hairline over a faint shade gone by about 20 pt. The pill under the current desktop's name is a material of its own
(`OverviewBandSelection`) that adds gray 0.1373 to the band in the dark appearance and takes 0.0902 away in the light
one; the names and the plus are white in the dark appearance and dark in the light one. A material lies under all of a
window, so the band and its pill are a second window of the component (`chrome::band_window`): a layer surface over
the output's top edge in the layer under the overview's, as tall as the open band and taking no input, so the
overview's window draws the desktops' pictures, the names and the rest over it. It shows from the start of the way in
and fades in and out with the chrome's own curve, from the moment the server applies the toggle's or the way out's
transaction, from a gesture's start, or when the fingers lift to take the overview back; like the menu bar's, its fade
does not follow the fingers' progress.

**The menu bar.** The component says on the host bus where an overview is on its way in or open (`OverviewShown`,
retained, one per output): from a gesture's `gesture_began` or the toggle, until the way out starts or the fingers of a
gesture lift to take it back (`gesture_ended` reverted). The menu bar of that output gives way to it meanwhile, fading
along the overview's own motion; it is not a part of the overview's transactions, so an armed way in never holds the
bar's commits, and it does not follow the fingers' progress, which the server alone knows.

**Session.** Under Reduce Motion the server drives only the opacity of a gesture's layers, and a toggle fades the
overview in and out over 0.2 s (`Timing::OVERVIEW_FADE`) with the holes at their end places. Kit follows the appearance;
with Reduce Transparency or Increase Contrast the band (gray 0.8548 / 0.1255 with Reduce Transparency, 0.7843 / 0.0588
with Increase Contrast) and the plates are opaque and the ring wider.

**Accessibility.** The window is a group named "Overview"; each window is a button named by its title ("Untitled Window"
without one), the selection its focus; each desktop is a button named "Desktop" or "Desktop n", with "Add Desktop" and
"Remove Desktop n".

The packing, the band's metrics and the springs are approximate and authored.
