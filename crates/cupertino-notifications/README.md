# cupertino-notifications

The notification daemon of the Cupertino desktop. It stands in for the design's banner controller and notification centre: an
`org.freedesktop.Notifications` server, a single banner slot with a waiting slot, grouping by application, notification-centre
history, do-not-disturb, hover hold, swipe-to-dismiss, and the banners and notification centre as CupertinoKit windows.

The crate has no Smithay dependency and no threads of its own except the `zbus` executor of the optional bus server. Everything
that depends on time takes `now: f64` (seconds on the host's monotonic clock), so the compositor drives it from its frame clock.
Behavioural constants follow the design banner controller. Colours and typography come from `cupertino-theme`.

Features: `app` (the binary, implies `dbus`), `dbus` (zbus 5, default). The windows (`view`) are always built, on `cupertino-kit`.

## Running

Feature `app` builds the binary: a CupertinoKit application on layer surfaces that shows the banners and the notification centre
list. The server starts it from its `"shell"` configuration, restarts it after a failure, and calls `ToggleCentre` for the menu bar clock and the `notification-center` action. Screenshots:
`core/cupertino-server/scripts/capture-overlays.sh`.

```sh
cargo run -p cupertino-notifications --features app -- [--replace]
```

| Option | Effect |
|---|---|
| `--replace` | Take `org.freedesktop.Notifications` from a running daemon that allows replacement. Without it a taken name ends the process normally (status 0, with a message), so the server does not restart it; any other startup failure exits with failure and is restarted. |

Startup fails with a clear message when the server does not advertise `zwlr_layer_shell_v1`; the rest of the connection
(scale, activation, appearance, accessibility) is CupertinoKit's. Any connection or bus failure, or the server closing a
surface, ends the process with a failure status so the server restarts it.

## How it works

- **Surfaces.** Every window is a Kit window on an overlay layer surface (namespace `cupertino-notifications`, exclusive zone
  -1) that `NotificationRoles` gives it. Each banner on screen is its own window at the top right, as wide as the card plus its
  right margin, the leftward swipe travel and the shadow (`view::strip_width`), and as tall as the card plus its shadow; its input
  region is the card at rest, so clicks elsewhere reach the windows under it, and a swipe keeps the pointer by its implicit grab.
  The card moves inside its window by the model's slide and drag offsets. The open centre is one window anchored top, right and
  bottom that takes the keyboard. A replaced notification gets a new banner window (its height may change).
- **Display options.** Reduce Motion sets `Motion::reduce_motion` (banners fade in place). Kit follows the appearance, Reduce
  Transparency and Increase Contrast itself.
- **Input.** Hover pauses the timeout. A click runs the default action, a button its action, a link `xdg-open`. The close button
  dismisses, and a drag to the right swipes the banner away. Every action first fetches an `xdg_activation_v1` token, which goes
  out as `ActivationToken` before `ActionInvoked`. While the list is open the surface takes the keyboard and Escape closes it.
- **Session.** On `locked` an open list closes; banners expire on their own.
- **Accessibility.** A banner is a live region named "app: summary" with its body as description: polite, assertive when
  critical, so a screen reader reads it as it arrives. The centre's cards are not live; a group's front card says how many more
  it holds. Every card and close button takes assistive technology's click.
- **Banners.** One on screen, one waiting. A banner arriving while one is shown takes the waiting slot; a banner already waiting goes straight to the centre. Critical urgency and `expire_timeout = 0` are alerts without a timer; a waiting banner
  stays behind them. A client `expire_timeout` is honoured, capped at 60 s. A rightward release dismisses at speed
  `max(dx/100, 8) / 15`; a leftward one springs back.
- **Do not disturb.** Banners are suppressed and notifications kept in the centre. Critical urgency bypasses it
  (`critical_bypasses_dnd`), and `Until(t) ends by itself.
- **Drawing.** Kit controls on a card the `view` module paints: the theme's popover fill with a 0.5 pt separator outline,
  16 pt corners and a drop shadow, faded as one layer with its content. A 32 pt icon (the `app_icon` symbol, else `bell`), the
  summary semibold on one line, the body on one line with bold, italic and link markup (`RichText`, links are `xdg-open`ed), up
  to 3 action buttons (pills) and an 18 pt close button (a banner's while hovered, always on a centre card). Card heights come
  from `view::CardMetrics`, which the model measures with.

## Protocols / D-Bus it speaks

The process owns `org.freedesktop.Notifications` on the session bus. It requests the name with `DoNotQueue | AllowReplacement`;
`replace_existing` adds `ReplaceExisting`. It also owns `org.cupertino.NotificationCenter1` at `/org/cupertino/NotificationCenter1`.

| Call | Behaviour |
|---|---|
| `Notify` | Hints `urgency`, `category`, `transient` (not kept in the centre), `resident` (stays after an action), `image-data` / `image_data` / `icon_data` (RGB/RGBA, 8 bit), `image-path`, `desktop-entry`, `suppress-sound`. Actions are `[key, label, …]`, and key `default` is the banner click action. `replaces_id` keeps the id and restarts the timeout without re-sliding. |
| `CloseNotification` | Removes the notification from stack and centre (`NotificationClosed`, reason 3; unknown ids are ignored). |
| `GetCapabilities` | Configurable: `actions body body-markup persistence`. `icon-static` stays off until the cards draw `image-data`. |
| `GetServerInformation` | Name `cupertino-notifications`, spec version `1.2`. |
| `ToggleCentre | Opens the list or closes it. Same caller check as `SetDoNotDisturb`. |
| `SetDoNotDisturb(b) | Turns do not disturb on (until turned off) or off. The caller is checked (`cupertino-peer`): a Flatpak, a Snap or a caller the bus cannot name is refused with an `org.cupertino.Error.*` name and nothing changes; other processes of the user may call it. |
| `SetDoNotDisturbFor(s) (same caller check) | Turns it on for one of the `FocusLifetimes` rows (`a(ss): identifier and title, in menu order). |
| `DoNotDisturb` property | Tells whether it is in effect, with change signals, also when a duration runs out. The menu bar's Focus item shows these rows. |

Close reasons: expired 1, dismissed 2 (swipe, click without default action, clear, action invocation), closed 3, undefined 4
(history trimmed or dropped while do-not-disturb).

## Modules

| Module | Content |
|---|---|
| `center` | `NotificationCenter` (pure model), `CenterConfig` (`TimeoutPolicy`, `StackLayout`, `SwipeConfig`, `Motion`) |
| `focus` | The Focus durations: `offered` (by local hour), `end` (when a row started now ends), `LocalCalendar` (`SystemCalendar` with feature `dbus`) |
| `stack` | `GroupStack`, `StackCard`: geometry of a collapsed group in the notification centre |
| `notification` | `NotifyInput`, `Notification`, `Hints`, `ImageData`, `Urgency`, `CloseReason`, `Expire` |
| `markup` | Body markup subset (`<b> <i> <u> <a href> <img alt>), entity decoding |
| `view` | The banner and centre windows (`windows`), the user's `Input` and `apply` carrying it out on the model, `CardMetrics`, `strip_width` |
| `dbus` (feature `dbus`) | `Server`, `ServerOptions`, `parse_hints` |

## Integration API

```rust
use cupertino_notifications::{dbus::{Server, ServerOptions}, view::{self, Input, Ui}, CenterConfig, Wakeup};
use cupertino_theme::Theme;

let theme = Theme::load()?;
let server = Server::start(ServerOptions {
    config: CenterConfig::from_theme(&theme),          // top inset = menu bar height + gap, spec springs
    wake: Some(Box::new(move || ping.ping())),         // runs on the zbus thread after Notify/Close
    ..Default::default()
})?;                                                    // Err(NameTaken) if another daemon runs

// in a CupertinoKit application's update: carry out what the windows reported, then expire and animate
let action = view::apply(&mut server.center(), &mut ui, input, server.now()); // activation, invocation, link
server.center().tick(server.now());
server.flush_events()?;                                 // NotificationClosed / ActionInvoked signals
// timers: server.center().next_wakeup(now) -> Frame | At(t) | None

// in its view: the banners on screen, or the open centre
view::windows(&mut server.center(), &ui, server.now(), Message::Input)
```

The model's calls, for a host of its own (all take `now`):

| Pointer event | Call |
|---|---|
| move | `set_hover(Some(id) / None, now) (re-arms the 5 s timer; a hovered banner does not expire) |
| click | `activate(id, token, now)` (default action or dismiss), `invoke_action(id, key, token, now)`, `dismiss_banner(id, now)` or `clear(id, now) |
| drag | `swipe_begin(id, now)` on press, `swipe_update(id, dx, now)` (a drag after 5 pt; leftward travel clamped at −16 pt), `swipe_end(id, now)` → `Dismissed` (dx ≥ 0) / `Cancelled` (leftward, spring back) / `Click` (a press that never moved 5 pt) |
| notification-centre UI | `history`, `groups` (by desktop entry or app name, newest first), `unread_count`, `mark_read`, `clear_group`, `clear_all`, `set_dnd(DndMode::{Off,On,Until(t)}, now) |

`activation_token` is the xdg-activation token the compositor minted for the click. It is sent as the `ActivationToken` signal
(spec 1.3) before `ActionInvoked`. Without the bus (the compositor's own messages, tests), build a `NotificationCenter`, call
`notify(NotifyInput, now)` and `take_events()` yourself.


| Value | ID |
|---|---|
| lifetime 5.0 s, 1.0 s while another waits | |
| one visible + one waiting slot, minimum 1.0 s on screen | |
| hover re-check 1.0 s, re-arm 5.0 s on hover change | |
| drag: 5 pt minimum, clamp −16 pt, spring back m 1 / k 100 / c 100 / v dx/100, dismiss speed max(dx/100, 8) | |
| slide by the list controller width (card width + 32), no clip, fade under reduce motion, speed divisor 15 | |
| 0.25 s ease-in-out, completion +0.01 s | |
| width 344 pt | |
| group stack: scale (344 − 20·min(i,3))/344 at the bottom, 8 pt peek, 6 pt padding; at most 2 cards behind, each an 8 pt row one peek below the card in front | |
| centre list: rows 0 pt apart, each card with 6 pt bottom padding (a collapsed group only on its last visible card), stacked rows 8 pt; cards and groups therefore 6 pt apart | |
| action buttons 24 pt high, radius 12 pt, padding 12 pt; outline 0.5 pt | |
| backdrop scale 1/8, blur 30, saturation / brightness tables | |
| shadow opacity 0.20 / 0.25, radius 4, no offset; vibrant rim; 1 pt white inner rim | |
| alerts skip the timer (critical urgency → alert mapping is ours) | |
| top inset (menu bar + 16 pt), right margin 16 pt (Dock at the bottom) | |
| centre header: a panel of the cards' material, corner radius and rim, 6 pt above the first card; title "Notification Centre" bold 22 at the card padding (12 pt) from its left edge, Clear menu button 6 pt inside its right edge, top and bottom; the open list replaces the banners on screen | |
| Clear menu scopes: "Banners Only" (keeps pinned; offered only with both pinned and other notifications in the list) and "All Notifications" (always); panel metrics item 22, edge padding 5, minimum width 150, radius 6, row highlight radius 4 | |
| corner radius 14 pt, platter material `Medium`, all other banner metrics | |
| Focus rows: "For 1 hour" (3600 s), "Until this evening" from 04:00 to 17:59 else "Until tomorrow morning"; both end today 07:00 before 04:00, today 19:00 before 18:00, else tomorrow 07:00; durations clamped to 86400 s; row ids `one-hour`, `evening`, `morning` | |
| the location and calendar-event rows are not offered (no place or calendar source in the session) | |

## Open issues

- The cards paint the theme's opaque popover fill: a Kit window has one compositor material for the whole window, so the
  per-card blur of the earlier CPU renderer is not drawn until Kit offers a material per node.
- The body shows one line and the summary one line (no wrapping), as `CardMetrics` measures without the text system.
- Application icons are looked up as system symbols; image-data, image-path and desktop-entry icons are not drawn yet.

## Tests

```
cargo run -p cupertino-notifications --example notification-daemon        # prints notifications from the session bus
cargo test -p cupertino-notifications
```

| Test | Proves |
|---|---|
| `tests/dbus_integration.rs` | The server against a real client on a private `dbus-daemon`. Skipped when `dbus-daemon` is not installed. |
| `tests/kit_goldens.rs` | The Kit windows on the headless window system against the Inspector dumps in `tests/goldens/`: a banner sliding in and at rest, a critical one taking over, with the screen-reader announcements (`banner`); the centre with a stacked group (`centre-stacked`); a swipe dragged 60 pt and mid-dismissal (`swipe-dismiss`). `CUPERTINO_UPDATE_GOLDENS=1` rewrites them. |
