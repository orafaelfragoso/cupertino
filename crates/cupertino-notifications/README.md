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
  bottom that takes the keyboard: the header stays at the top and the cards scroll below it in a Kit `ScrollView`, which ends
  `BOTTOM_MARGIN` (16 pt) above the screen's bottom edge. A replaced notification gets a new banner window (its height may
  change).
- **Display options.** Reduce Motion sets `Motion::reduce_motion` (banners fade in place); unfolding a body and scrolling have
  no animation of their own beyond the scroll view's. Kit follows the appearance, Reduce Transparency and Increase Contrast
  itself: under either, or without the compositor's effects, the theme's popover surface has no material and every card paints
  its opaque fill.
- **Input.** Hover pauses the timeout. A click runs the default action, a button its action, a link `xdg-open`. The close button
  dismisses, and a drag to the right swipes the banner away. Every action first fetches an `xdg_activation_v1` token, which goes
  out as `ActivationToken` before `ActionInvoked`. While the list is open the surface takes the keyboard and Escape, or a press
  outside the header, the cards and the Clear menu, closes it. The list scrolls with the wheel (40 pt a notch), the touchpad
  (by its distance, offsets kept on whole points) and the keyboard (Up, Down, Page Up, Page Down, Home, End); Tab moves
  between the cards and their buttons and the list follows the focus. On a focused card Return, keypad Enter and Space run its
  default action and Delete or Backspace clears it; the focus then goes to the card in its place (its group's next one, else the next group's, else the one before).
- **Session.** The host passes the compositor's lock state (`cupertino_appearance_v1`'s session lock event) to
  `set_screen_locked(locked, now)`. At the lock an open list closes and no window stays open until the unlock: the lock
  screen hides them, so assistive technology has nothing of them to read. The banner on screen stops counting down and
  keeps the rest of its lifetime; the waiting banner goes to the centre. While locked no banner is shown or counted
  down: a new notification goes straight to the centre (a transient one is closed, reason 4), and a replacement of the
  frozen banner updates it in place, frozen with a whole lifetime. At the unlock the frozen banner shows again and counts
  down what it had left, at least the minimum time on screen (1 s); it is quiet (`is_quiet`), so assistive technology
  does not hear it again, while its next update in place and every new arrival are heard.
- **Lock screen.** While locked, `lock_screen()` lists the notifications that arrived since the lock and are still in
  the centre, newest first, at most `LOCK_SCREEN_LIMIT` (20), from the applications whose policy allows the lock
  screen; a notification held back by Do Not Disturb is not listed. Each is its id, application name, title and body;
  unless the application's previews are Always, the title is "Notification" (`REDACTED_TITLE`) and the body empty, so
  the rest never leaves this process. The body is cut to `LOCK_SCREEN_BODY_CHARS` (256). The lock screen
  (`cupertino-login`) reads the list over the bus (`LockScreenNotifications`, below), which is empty while unlocked.
- **Policy.** `notifications.conf` (`cupertino_defaults::notifications`, written by Settings > Notifications) holds Show
  on lock screen (on by default) and Show previews (Always, When Unlocked by default, Never), each with a per-application
  override. The application id is the `desktop-entry` hint, else the application name (`Notification::group_key`). The
  daemon reads the file at start and follows its changes (`set_policy`). Never also hides the content of the
  application's banners and centre cards while unlocked (`shown`, `redacted`): its application, the title
  "Notification", no body, image or action buttons; a click still runs the default action. Changing the policy
  measures the cards again.
- **Bounds.** `Notify` cuts the application name to `MAX_APP_NAME_CHARS` (64), the summary to `MAX_SUMMARY_CHARS`
  (256) and the body to `MAX_BODY_CHARS` (8192) characters, each cut ending in an ellipsis (`notification::cap`), so the
  model, the bus replies and the lock screen never hold more. The cards cut again where they draw: the summary and
  application name by the same caps, the body at its byte bound and line limit, all ending in an ellipsis.
- **Accessibility.** A banner is a live region, polite or assertive when critical, so a screen reader reads it as it
  arrives without the banner taking the focus. The AT-SPI bridge announces a live node only as it appears or is
  renamed, and says its name alone, so the banner is named after the words it shows (`view::banner_speech`): its
  application, summary and body, in that order, each run of spaces and line breaks one space, the body cut where the
  card cuts it ("Mail: Lunch, Are you free at noon?"). Its buttons and close button keep their own names outside it.
  An update in place that changes the words renames it and is said again; one that changes nothing shown says nothing.
  Each banner a sighted user sees is said as it shows: a waiting banner as it takes the screen, and one a newer
  banner replaced in the waiting slot never, as it goes straight to the centre. A banner on screen whose window comes
  back (at the unlock, or as the centre closes) is said again; one sliding out is no longer live, so it is not said as
  it leaves. Nothing is said or readable while the session is locked, and nothing already said is said again at the
  unlock (see Session); an application whose previews are Never is said as its application and "Notification". The centre
  is a group named "Notification Centre" holding a list of its cards (list items with their position); its cards are
  not live. Each is named "app: summary" with its whole body as description, read as a reader moves to it, and a
  group's front card says how many more it holds. The first card takes the focus when the list opens. Every card, close button and action button takes assistive technology's click, and the disclosure button reports
  whether the body is unfolded.
- **Banners.** One on screen, one waiting. A banner arriving while one is shown takes the waiting slot; a banner already waiting goes straight to the centre. Critical urgency and `expire_timeout = 0` are alerts without a timer; a waiting banner
  stays behind them. A client `expire_timeout` is honoured, capped at 60 s. A rightward release dismisses at speed
  `max(dx/100, 8) / 15`; a leftward one springs back.
- **Do not disturb.** Banners are suppressed and notifications kept in the centre. Critical urgency bypasses it
  (`critical_bypasses_dnd`), and `Until(t) ends by itself.
- **Drawing.** Kit controls on a card the `view` module paints: the compositor's material of the theme's popover surface under
  the card (`PaintCx::material`, cut to the part the list shows), a 0.5 pt separator outline, 16 pt corners and a drop shadow
  around it, faded as one layer with its content. Where the material is refused (reduced transparency, increased contrast, no
  compositor effects, more materials than the window may show) the card paints the surface's opaque fill instead. The cards
  behind a group's front card and the Clear menu are always opaque, as a material would show through to the card in front. A
  32 pt icon (the `app_icon` symbol, else `bell`), the summary semibold in up to `SUMMARY_LINES` (2) lines, the body in up to
  `BODY_LINES` (4) lines with bold, italic and link markup (`RichText`, links are `xdg-open`ed), up to 3 action buttons
  (pills, sharing the row when their labels do not fit) and an 18 pt close button centred on the top-leading corner,
  clear of the icon (a banner's while hovered, always on a centre card). Text wraps at word boundaries and keeps the body's
  line breaks; text past the limit ends in an ellipsis and links past the cut are dropped. A centre card whose body is
  cut has a disclosure button beside its summary that unfolds the body to `UNFOLDED_BODY_LINES` (20); banners stay folded
  (the centre holds the whole message). Card heights come from `view::CardMetrics`, which measures the text with the
  windows' own text system and theme, so the model's stack geometry and the drawn cards agree.

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
| `LockScreenNotifications` → `a(usss)` | The lock screen's list (`lock_screen()`): id, application, title and body of each, already redacted by the policy; empty while unlocked. Same caller check as `SetDoNotDisturb`. |
| `Applications` → `a(ss)` | The applications of the notifications on screen and in the centre: the id their settings are kept under and their name, for Settings > Notifications. Same caller check. |
| `LockScreenChanged` signal | The lock screen's list changed (an arrival, a replacement, a close, the lock or the unlock, a policy change). It carries nothing, so no content goes out to every listener on the bus: the lock screen asks for the list again, and again whenever the daemon's name changes owner. |

Close reasons: expired 1, dismissed 2 (swipe, click without default action, clear, action invocation), closed 3, undefined 4
(history trimmed or dropped while do-not-disturb).

## Modules

| Module | Content |
|---|---|
| `center` | `NotificationCenter` (pure model), `CenterConfig` (`TimeoutPolicy`, `StackLayout`, `SwipeConfig`, `Motion`), the lock state and the lock screen's list (`LockItem`), the policy of `notifications.conf` |
| `focus` | The Focus durations: `offered` (by local hour), `end` (when a row started now ends), `LocalCalendar` (`SystemCalendar` with feature `dbus`) |
| `stack` | `GroupStack`, `StackCard`: geometry of a collapsed group in the notification centre |
| `notification` | `NotifyInput`, `Notification`, `Hints`, `ImageData`, `Urgency`, `CloseReason`, `Expire`, the caps of the application name, summary and body (`cap`, `NotifyInput::capped`) |
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

// in its view: the banners on screen, or the open centre, measured with the windows' theme and text system
let metrics = view::CardMetrics::new(options.theme.clone(), options.text.clone()); // once, from RuntimeOptions
view::windows(&mut server.center(), &ui, &metrics, server.now(), Message::Input)
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

- A card scrolled partly under the header is cut by the list's edge, but its material keeps rounded corners there: the
  compositor's backdrop shapes are rounded rectangles of one radius.
- Application icons are looked up as system symbols; image-data, image-path and desktop-entry icons are not drawn yet.

## Tests

```
cargo run -p cupertino-notifications --example notification-daemon        # prints notifications from the session bus
cargo test -p cupertino-notifications
```

| Test | Proves |
|---|---|
| `tests/dbus_integration.rs` | The server against a real client on a private `dbus-daemon`. Skipped when `dbus-daemon` is not installed. `Notify` caps the application name, summary and body; the lock screen gets only what the policy allows (redacted, full with Always, nothing for an application kept off it, empty unlocked), `LockScreenChanged` at each change, and `Applications`. |
| `src/center/tests.rs` | The model: arrival, transient and Do Not Disturb while locked; a replacement while locked stays in the centre or updates the frozen banner; locking freezes the banner on screen and sends the waiting one to the centre; the unlock resumes it quietly for at least the minimum time, while an update is heard; a per-application override; each preview policy on the lock screen and the banners; the list bounded and capped; `applications`. |
| `tests/kit_goldens.rs` | The Kit windows on the headless window system against the Inspector dumps in `tests/goldens/`: a banner sliding in and at rest, a critical one taking over, with the screen-reader announcements (`banner`); the centre with a stacked group (`centre-stacked`); a swipe dragged 60 pt and mid-dismissal (`swipe-dismiss`). `CUPERTINO_UPDATE_GOLDENS=1` rewrites them. Geometry and interaction: a long banner body wraps inside its card and stops at its line limit; the centre scrolls with the wheel, the touchpad and the keyboard and a click hits the card shown there; a banner's close button stays under the pointer past the card's edge and dismisses it; the disclosure button unfolds and folds a body; the keyboard activates and clears cards, the focus following to the card in a cleared one's place, the close button clears one and Escape closes an empty centre; each card's material region is its card and Reduce Transparency paints fills. Screen reader, through `cupertino-ui-test`'s stand-in reader, which announces as the AT-SPI bridge does (a live node's name, as it appears or is renamed): a new banner says its application, title and body and not its buttons; a banner updated in place is said again; nothing is said or readable while locked and nothing is said again at the unlock, while the next update of the banner back on screen is; an application without previews shows and says only its name and "Notification"; banners arriving together are each said as they show; announcing never takes the focus; a reader's click on an action button invokes it; the centre's cards carry their whole body as description. |
| `src/view/tests.rs` | `CardMetrics` against the built-in font: heights follow the text, wrapping, line limits and ellipsis, kept line breaks, the body byte bound, links cut at the ellipsis, buttons sharing a row, the close button on the corner clear of the icon and in the banner's input region, a banner's speech (order, spacing, the cut body), the caps drawn with an ellipsis, no window while locked, and the `apply` of scroll, unfold, clear and close. |
