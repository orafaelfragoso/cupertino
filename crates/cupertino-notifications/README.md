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
- **Display options.** Reduce Motion sets `Motion::reduce_motion`: banners fade in place and a group expands and collapses at
  once; unfolding a body and scrolling have no animation of their own beyond the scroll view's. Kit follows the appearance,
  Reduce Transparency and Increase Contrast itself: under either the cards keep their glass, drawn with the compositor's recipe
  for it; without the compositor's effects every card paints the popover surface's opaque fill.
- **Input.** Hover pauses the timeout. A click runs the default action, a button its action, a link `xdg-open`. The close button
  dismisses, and a drag to the right swipes the banner away. Every action first fetches an `xdg_activation_v1` token, which goes
  out as `ActivationToken` before `ActionInvoked`. While the list is open the surface takes the keyboard and Escape, or a press
  outside the header, the cards and the Clear menu, closes it. In the list the pointer over a card, or the keyboard on it
  or in it with its focus ring showing, reveals its close button and its action buttons (the card grows to show them); the
  two are kept apart, so the pointer moving elsewhere never takes the buttons from under the keyboard and Tab goes on from
  the card into them; a banner shows its buttons always and its
  close button while hovered. A click (or Return, Space or assistive technology's click) on a collapsed group of more than
  one notification expands it: a header row with the application, Show Less and a clear button for the group, then each
  card on its own, 6 pt apart; the cards move out of the stack with a spring of 0.35 s perceptual duration and no bounce,
  their text fading in as they arrive, and Show Less moves them back. The header's clear button is a circle with a cross
  that grows into "Clear All" under the pointer or the keyboard; it clears the list, or opens the Clear menu when there are
  two scopes. The list scrolls with the wheel (40 pt a notch), the touchpad
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
  override. The application id is the `desktop-entry` hint, else the application name (`Notification::group_key`).
  Both are what the sender says of itself: nothing ties them to the sending process, so an application can name
  another's id and be shown with that id's policy. It gains no other application's content that way, only the
  showing of its own: with an id set to Always, its own title and body on the lock screen. That is the reach any sender
  already has through the application name it chooses, which the lock screen always shows; set Always only for an
  application whose words you are content to see there. The daemon reads the file at start and follows its changes
  (`set_policy`). Never also hides the content of the
  application's banners and centre cards while unlocked (`shown`, `redacted`): its application, the title
  "Notification", no body, image or action buttons; a click still runs the default action. Changing the policy
  measures the cards again.
- **Bounds.** `Notify` cuts the application name to `MAX_APP_NAME_CHARS` (64), the summary to `MAX_SUMMARY_CHARS`
  (256) and the body to `MAX_BODY_CHARS` (8192) characters, each cut ending in an ellipsis (`notification::cap`), so the
  model, the bus replies and the lock screen never hold more. A cut falls between grapheme clusters, so a letter keeps
  its marks and an emoji sequence or flag stays whole; text is cut in its logical order, right-to-left text included.
  The body is cut before its markup is parsed, and a cut inside a tag or an entity drops what is left of it
  (`cap_markup`), so no half tag shows as text. A `desktop-entry` hint longer than a file name (255 bytes) is dropped. The cards cut again where they draw: the summary and
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
  group's front card says how many more it holds and that it is collapsed (expanded state false). The first card takes the
  focus when the list opens, and keeps it while its group expands. Every card, close button and action button takes assistive technology's click, and the disclosure button reports
  whether the body is unfolded.
- **Banners.** One on screen, one waiting. A banner arriving while one is shown takes the waiting slot; a banner already waiting goes straight to the centre. Critical urgency and `expire_timeout = 0` are alerts without a timer; a waiting banner
  stays behind them. A client `expire_timeout` is honoured, capped at 60 s. A rightward release dismisses at speed
  `max(dx/100, 8) / 15`; a leftward one springs back.
- **Do not disturb.** Banners are suppressed and notifications kept in the centre. Critical urgency bypasses it
  (`critical_bypasses_dnd`), and `Until(t) ends by itself.
- **Drawing.** Kit controls on a card the `view` module paints: regular glass under the card (`PaintCx::glass`, cut to the
  part the list shows), a 0.5 pt separator outline, 18 pt corners and a drop shadow drawn only outside it, faded as one
  layer with its content. A collapsed group's front card and the sheets peeking below it join one glass container with a
  hard edge, so they read as one shape; each sheet's shadow, darkening and outline stay off the front card. The list keeps its glass
  within what one window shows (`MAX_MATERIALS`, counted over the part in view): every card keeps its glass, and the
  sheets of collapsed groups keep theirs a whole group at a time down the list while room is left; the sheets past it are
  opaque sheets. Where the glass is refused (no compositor effects, or more cards in view than the window may show) a card
  paints the popover surface's opaque fill instead. The Clear menu is always opaque, as glass would show through to the cards under it. The header has no
  panel: the title, bold 19 pt, sits over the cards' leading edge, the clear button at their trailing edge. A card holds a
  32 pt icon (the `app_icon` symbol, else `bell`) 13 pt from its leading edge and 14 pt before the text, its age at the top
  right ("now", "5m ago", "2h ago", "3d ago", 11 pt secondary), the summary semibold in up to `SUMMARY_LINES` (2) lines
  short of the age, the body in the label colour in up to `BODY_LINES` (4) lines with bold, italic and link markup
  (`RichText`, links are `xdg-open`ed), up to 3 action buttons (pills, sharing the row when their labels do not fit) and an
  18 pt close button centred on the top-leading corner, clear of the icon. A card is at least 58 pt tall. Text wraps at word boundaries and keeps the body's
  line breaks; text past the limit ends in an ellipsis and links past the cut are dropped. A centre card whose body is
  cut has a disclosure button beside its summary that unfolds the body to `UNFOLDED_BODY_LINES` (20); banners stay folded
  (the centre holds the whole message). Card heights come from `view::CardMetrics`, which measures the text with the
  windows' own text system and theme, so the model's stack geometry and the drawn cards agree; the disclosure button's
  side is the control size table's (`ControlFamily::Disclosure`) at the theme's control size, as Kit draws it.

## Protocols / D-Bus it speaks

The process owns `org.freedesktop.Notifications` on the session bus. It requests the name with `DoNotQueue | AllowReplacement`;
**Who may replace or close a notification.** The daemon asks the bus who sent each `Notify` and who calls
`CloseNotification` (`cupertino_peer::identify`, the process from the daemon's `ProcessFD`). An unsandboxed process of the
person may replace or close any notification: it can already act as the person, and the legitimate replacer cannot be
told apart from it (`notify-send --replace-id` starts a new process for each call, an application reconnects under a new
name). A Flatpak or Snap may replace or close only what its own sandbox instance sent (`cupertino_peer::same_instance`;
a Snap matches only its own process, so a restarted Snap cannot close what it sent before), and a caller the bus cannot
name nothing. A refused replacement is a new notification and a refused close does nothing, exactly as for an id that
does not exist, so a sandboxed application learns nothing of another's ids. The daemon keeps the sending process of
each live notification a sandboxed application sent (at most the centre's history), and forgets it with the
notification.

`replace_existing` adds `ReplaceExisting`. It also owns `org.cupertino.NotificationCenter1` at `/org/cupertino/NotificationCenter1`.

| Call | Behaviour |
|---|---|
| `Notify` | Hints `urgency`, `category`, `transient` (not kept in the centre), `resident` (stays after an action), `image-data` / `image_data` / `icon_data` (RGB/RGBA, 8 bit), `image-path`, `desktop-entry`, `suppress-sound`. Actions are `[key, label, …]`, and key `default` is the banner click action. `replaces_id` keeps the id and restarts the timeout without re-sliding, when the caller may change the notification (below); otherwise the call makes a new notification, as for an id that does not exist. |
| `CloseNotification` | Removes the notification from stack and centre (`NotificationClosed`, reason 3; unknown ids are ignored, and so are ids the caller may not change). |
| `GetCapabilities` | Configurable: `actions body body-markup persistence`. `icon-static` stays off until the cards draw `image-data`. |
| `GetServerInformation` | Name `cupertino-notifications`, spec version `1.2`. |
| `ToggleCentre | Opens the list or closes it. Same caller check as `SetDoNotDisturb`. |
| `SetDoNotDisturb(b) | Turns do not disturb on (until turned off) or off. The caller is checked (`cupertino-peer`): a Flatpak, a Snap or a caller the bus cannot name is refused with an `org.cupertino.Error.*` name and nothing changes; other processes of the user may call it. |
| `SetDoNotDisturbFor(s) (same caller check) | Turns it on for one of the `FocusLifetimes` rows (`a(ss): identifier and title, in menu order). |
| `DoNotDisturb` property | Tells whether it is in effect, with change signals, also when a duration runs out. The menu bar's Focus item shows these rows. |
| `CentreShown` property (`b`) | Whether the list is open (its surface exists), with change signals; the menu bar highlights the clock while it is. |
| `CentreOutput` property (`s`) | The connector name (`wl_output.name`) of the output the open list is on, with change signals; empty while it is closed or before its surface is told its output. The process's surface roles publish both before every wait of its loop (`CentrePublisher`). |
| `LockScreenNotifications` → `a(usss)` | The lock screen's list (`lock_screen()`): id, application, title and body of each, already redacted by the policy; empty while unlocked. Same caller check as `SetDoNotDisturb`: any unsandboxed process of the user may call it, a Flatpak or a Snap may not. With previews Always, such a process can read the titles and bodies listed while locked; it could already read every notification as it is sent, since a process of the user may monitor its own session bus, so the method opens nothing new. With When Unlocked or Never no reply holds a title, body, image or action. |
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
| centre header: a 44 pt row without a panel, 5 pt above the first card; title "Notification Centre" bold 19 pt 3 pt in from the cards' leading edge; an 18 pt clear button at their trailing edge, growing into "Clear All"; the open list replaces the banners on screen | |
| card: 18 pt corners, regular glass, icon 13 pt from the leading edge and 14 pt before the text, 16 pt trailing, 12 pt top and bottom, at least 58 pt tall; age 11 pt at the top right in 56 pt of room | |
| expanded group: a 30 pt header row (application 15 pt semibold, Show Less, clear), cards 6 pt apart; a spring of perceptual duration 0.35 s, no bounce | |
| Clear menu scopes: "Banners Only" (keeps pinned; offered only with both pinned and other notifications in the list) and "All Notifications" (always); panel metrics item 22, edge padding 5, minimum width 150, radius 6, row highlight radius 4 | |
| corner radius 14 pt, platter material `Medium`, all other banner metrics | |
| Focus rows: "For 1 hour" (3600 s), "Until this evening" from 04:00 to 17:59 else "Until tomorrow morning"; both end today 07:00 before 04:00, today 19:00 before 18:00, else tomorrow 07:00; durations clamped to 86400 s; row ids `one-hour`, `evening`, `morning` | |
| the location and calendar-event rows are not offered (no place or calendar source in the session) | |

## Open issues

- The centre holds notifications only: no widgets, as the session has no data source for them.
- A centre card's close and action buttons are in the accessibility tree while it is revealed (pointer, or the keyboard on it
  with its focus ring showing); a reader that browses without moving the focus reaches its default action and clearing
  through the card itself (click, Delete), and Tab onto the card reveals its buttons.

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
| `tests/dbus_integration.rs` | The server against a real client on a private `dbus-daemon`. Skipped when `dbus-daemon` is not installed. `Notify` caps the application name, summary and body; the lock screen gets only what the policy allows (redacted, full with Always, nothing for an application kept off it, empty unlocked), `LockScreenChanged` at each change, and `Applications`; a caller over anonymous TCP (no process the bus can name) neither closes nor replaces another's notification, while another unsandboxed process does both. The unit tests of `dbus.rs` cover the rest of `may_change`: a sandboxed caller changes only what its own instance sent. |
| `src/center/tests.rs` | The model: arrival, transient and Do Not Disturb while locked; a replacement while locked stays in the centre or updates the frozen banner; locking freezes the banner on screen and sends the waiting one to the centre; the unlock resumes it quietly for at least the minimum time, while an update is heard; a per-application override; each preview policy on the lock screen and the banners; the list bounded and capped; `applications`. |
| `tests/kit_goldens.rs` | The Kit windows on the headless window system against the Inspector dumps in `tests/goldens/`: a banner sliding in and at rest, a critical one taking over, with the screen-reader announcements (`banner`); the centre with a stacked group (`centre-stacked`) and with a group expanded (`centre-expanded`: a click expands it, its cards move out of the stack, Show Less collapses it); a swipe dragged 60 pt and mid-dismissal (`swipe-dismiss`). `CUPERTINO_UPDATE_GOLDENS=1` rewrites them. Geometry and interaction: a long banner body wraps inside its card and stops at its line limit; the centre scrolls with the wheel, the touchpad and the keyboard and a click hits the card shown there; a banner's close button stays under the pointer past the card's edge and dismisses it; the disclosure button unfolds and folds a body; the keyboard activates and clears cards, the focus following to the card in a cleared one's place, the close button clears one and Escape closes an empty centre; each card's material region is its card and Reduce Transparency paints fills; past the glass one window shows, every card keeps its glass and the lower groups' sheets go opaque; the keyboard reveals the card it is on and Tab reaches its buttons whatever the pointer does. Screen reader, through `cupertino-ui-test`'s stand-in reader, which announces as the AT-SPI bridge does (a live node's name, as it appears or is renamed): a new banner says its application, title and body and not its buttons; a banner updated in place is said again; nothing is said or readable while locked and nothing is said again at the unlock, while the next update of the banner back on screen is; an application without previews shows and says only its name and "Notification" (with `CUPERTINO_LOOK_DIR` set, capture `banner-without-previews`); banners arriving together are each said as they show; announcing never takes the focus; a reader's click on an action button invokes it; the centre's cards carry their whole body as description. |
| `src/view/tests.rs` | `CardMetrics` against the built-in font: heights follow the text, wrapping, line limits and ellipsis, kept line breaks, the body byte bound, links cut at the ellipsis, buttons sharing a row, the close button on the corner clear of the icon and in the banner's input region, a banner's speech (order, spacing, the cut body), the caps drawn with an ellipsis, no window while locked, the `apply` of scroll, unfold, clear and close, the age words, a centre card's buttons only while revealed, and a group's expansion spring, instant under Reduce Motion, and clearing a whole group. |
