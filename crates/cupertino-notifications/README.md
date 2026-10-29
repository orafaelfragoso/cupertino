# cupertino-notifications

The notification daemon of the Cupertino desktop. It stands in for the design's banner controller and notification centre: an
`org.freedesktop.Notifications` server, a single banner slot with a waiting slot, grouping by application, notification-centre
history, do-not-disturb, hover hold, swipe-to-dismiss and CPU banner rendering.

The crate has no Smithay dependency and no threads of its own except the `zbus` executor of the optional bus server. Everything
that depends on time takes `now: f64` (seconds on the host's monotonic clock), so the compositor drives it from its frame clock.
Behavioural constants follow the design banner controller. Colours and typography come from `cupertino-theme`.

Features: `app` (the binary, implies `dbus`), `dbus` (zbus 5, default), `material` (the CPU material pipeline of `cupertino-glass`
for the banner background when a backdrop is supplied; without it the flat fallback fill is used).

## Running

Feature `app` builds the binary: a client of the display server that shows the banners and the notification centre list. The
server starts it from its `"shell"` configuration, restarts it after a failure, renders the banner backdrops requested through
`cupertino_shell_v1`, and calls `ToggleCentre` for the menu bar clock and the `notification-center` action. Screenshots:
`core/cupertino-server/scripts/capture-overlays.sh`.

```sh
cargo run -p cupertino-notifications --features app -- [--replace]
```

| Option | Effect |
|---|---|
| `--replace` | Take `org.freedesktop.Notifications` from a running daemon that allows replacement. Without it a taken name ends the process normally (status 0, with a message), so the server does not restart it; any other startup failure exits with failure and is restarted. |

Startup fails with a clear message when the server does not advertise `zwlr_layer_shell_v1`, `cupertino_shell_v1` (backdrop
materials, `cupertino-protocols`), `xdg_activation_v1`, `wl_shm` or a seat. It connects to `$WAYLAND_DISPLAY`. Fractional scale is
used when both `wp_fractional_scale_manager_v1` and `wp_viewporter` are offered, otherwise the integer `wl_output` scale. Any
connection or bus failure, or the server closing the surface, ends the process with a failure status so the server restarts it.

## How it works

- **Surface.** One overlay layer surface anchored top, right and bottom, namespace `cupertino-notifications`, exclusive zone -1.
  Its input region covers only the visible banners (the whole surface while the list is open), so clicks elsewhere reach the
  windows. Each banner and card gets one `cupertino_backdrop_v1` with material `Menu`; the process draws the shadow and content
  only.
- **Display options.** Reduce Motion from the session observer sets `Motion::reduce_motion` (banners fade in place). Reduce
  Transparency and Increase Contrast select the opaque material and the high-contrast catalog.
- **Input.** Hover pauses the timeout. A click runs the default action, a button its action, a link `xdg-open`. The close button
  dismisses, and a drag to the right swipes the banner away. Every action first fetches an `xdg_activation_v1` token, which goes
  out as `ActivationToken` before `ActionInvoked`. While the list is open the surface takes the keyboard and Escape closes it.
- **Session.** The appearance comes from `cupertino_session_v1`, read at start. On a change the renderer is rebuilt and the cards
  are drawn again. On `locked` an open list closes; banners expire on their own.
- **Banners.** One on screen, one waiting. A banner arriving while one is shown takes the waiting slot; a banner already waiting goes straight to the centre. Critical urgency and `expire_timeout = 0` are alerts without a timer; a waiting banner
  stays behind them. A client `expire_timeout` is honoured, capped at 60 s. A rightward release dismisses at speed
  `max(dx/100, 8) / 15`; a leftward one springs back.
- **Do not disturb.** Banners are suppressed and notifications kept in the centre. Critical urgency bypasses it
  (`critical_bypasses_dnd`), and `Until(t) ends by itself.
- **Rendering.** 36 pt icon (image-data > image-path > `app_icon` > the `Icon=` of the `desktop-entry` hint > the system's `application.generic`; `BannerRenderer::new` fails when the asset directory lacks it), semibold title, up to 3 body lines with
  bold/link markup (italic parsed, rendered upright), up to 3 action buttons (24 pt high, 12 pt radius and label padding), a hover
  close button, a 0.5 pt outline in the window-rim colour, continuous corners (`cupertino-geometry`) and the platter shadow
  (`PlatterShadow`). Text uses `cupertino-decorations`' text renderer (the `ui` font of the asset directory, `CUPERTINO_ASSET_DIR`) with the
  tracking table of the specification. An application's own icons come from an absolute path or the `hicolor` theme only (`cupertino-icons`), never a host theme. The optical size of the text follows the output scale (`TextRenderer::set_scale`, set before each layout and render).

## Protocols / D-Bus it speaks

The process owns `org.freedesktop.Notifications` on the session bus. It requests the name with `DoNotQueue | AllowReplacement`;
`replace_existing` adds `ReplaceExisting`. It also owns `org.cupertino.NotificationCenter1` at `/org/cupertino/NotificationCenter1`.

| Call | Behaviour |
|---|---|
| `Notify` | Hints `urgency`, `category`, `transient` (not kept in the centre), `resident` (stays after an action), `image-data` / `image_data` / `icon_data` (RGB/RGBA, 8 bit), `image-path`, `desktop-entry`, `suppress-sound`. Actions are `[key, label, …]`, and key `default` is the banner click action. `replaces_id` keeps the id and restarts the timeout without re-sliding. |
| `CloseNotification` | Removes the notification from stack and centre (`NotificationClosed`, reason 3; unknown ids are ignored). |
| `GetCapabilities` | Configurable: `actions body body-markup icon-static persistence`. |
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
| `platter` | `PlatterMaterial` (saturation / brightness tables, backdrop), `PlatterShadow`, `VibrantRim`, outline and inner-rim widths |
| `notification` | `NotifyInput`, `Notification`, `Hints`, `ImageData`, `Urgency`, `CloseReason`, `Expire` |
| `markup` | Body markup subset (`<b> <i> <u> <a href> <img alt>), entity decoding |
| `surface` | Layout of banners and list cards on the layer surface, input region, press routing (pure, used by the process) |
| `render` | `BannerStyle`, `BannerRenderer`, layout, hit testing, icon loading |
| `dbus` (feature `dbus`) | `Server`, `ServerOptions`, `parse_hints` |

## Integration API

```rust
use cupertino_notifications::{dbus::{Server, ServerOptions}, render::*, CenterConfig, Wakeup};
use cupertino_theme::{Appearance, Theme};

let theme = Theme::load()?;
let server = Server::start(ServerOptions {
    config: CenterConfig::from_theme(&theme),          // top inset = menu bar height + gap, spec springs
    clock: Some(Box::new(move || frame_clock_seconds())),   // same clock as `now` below
    wake: Some(Box::new(move || ping.ping())),         // runs on the zbus thread after Notify/Close
    ..Default::default()
})?;                                                    // Err(NameTaken) if another daemon runs
let mut renderer = BannerRenderer::new(BannerStyle::from_theme(&theme, Appearance::Light, false), &assets)?;

// every frame (or when the wake ping / next_wakeup timer fires)
let now = frame_clock_seconds();
let placements = {
    let mut c = server.center();                        // MutexGuard<NotificationCenter>
    c.tick(now);                                        // expiry, exit animations, queue promotion
    c.placements(output_size_pt, &mut renderer, now)    // animated rects, newest banner on top
};
server.flush_events()?;                                 // NotificationClosed / ActionInvoked signals
for p in &placements {
    let n = server.center().get(p.id).cloned().unwrap();
    let layout = renderer.layout_cached(&n, p.rect.width()).clone();
    let img = renderer.render(&n, &layout, &RenderOptions { scale, hovered: p.hovered, ..Default::default() });
    // upload img.pixmap (premultiplied RGBA8) as a texture at p.rect, honour p.opacity
}
// timers: server.center().next_wakeup(now) -> Frame | At(t) | None
```

Input handling (all calls take `now`):

| Pointer event | Call |
|---|---|
| move | `banner_at(point)` → `set_hover(Some(id) / None, now) (re-arms the 5 s timer; a hovered banner does not expire) |
| click | `layout.hit(local_point, hovered)` → `BannerHit::{Body, Close, Button{key,..}, Link(href)}`; then `activate(id, token, now)` (default action or dismiss), `invoke_action(id, key, token, now)`, `dismiss_banner(id, now)` or `clear(id, now) |
| drag | `swipe_begin(id, now)` on press, `swipe_update(id, dx, now)` (a drag after 5 pt; leftward travel clamped at −16 pt), `swipe_end(id, now)` → `Dismissed` (dx ≥ 0) / `Cancelled` (leftward, spring back, or a press that never moved 5 pt) |
| notification-centre UI | `history`, `groups` (by desktop entry or app name, newest first), `unread_count`, `mark_read`, `clear_group`, `clear_all`, `set_dnd(DndMode::{Off,On,Until(t)}, now) |

`activation_token` is the xdg-activation token the compositor minted for the click. It is sent as the `ActivationToken` signal
(spec 1.3) before `ActionInvoked`. Without the bus (the compositor's own messages, tests), build a `NotificationCenter`, call
`notify(NotifyInput, now)` and `take_events()` yourself.

**Compositor with its own material pass.** `RenderOptions { layers: Layers::CONTENT_ONLY, .. } draws only icon, text, buttons
and rim. Draw shadow and background with the GPU glass pass using `BannerStyle::material` (the `Recipe`: fill layers of the
`"Menu"` preset, approximate choice, with the platter backdrop of `BannerStyle::platter_material`) and `BannerStyle::shadow`,
corner radius `style.corner_radius` with `style.corner_style`. The CPU path (`Backdrop { image, banner_origin }) runs the same
recipe through `cupertino-glass`'s CPU reference for tools and screenshots.


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
| centre header: title "Notification Centre" bold 22 at the cards' left edge, Clear menu button at their right edge; the open list replaces the banners on screen | |
| Clear menu scopes: "Banners Only" (keeps pinned; offered only with both pinned and other notifications in the list) and "All Notifications" (always); panel metrics item 22, edge padding 5, minimum width 150, radius 6, row highlight radius 4 | |
| corner radius 14 pt, platter material `Medium`, all other banner metrics | |
| Focus rows: "For 1 hour" (3600 s), "Until this evening" from 04:00 to 17:59 else "Until tomorrow morning"; both end today 07:00 before 04:00, today 19:00 before 18:00, else tomorrow 07:00; durations clamped to 86400 s; row ids `one-hour`, `evening`, `morning` | |
| the location and calendar-event rows are not offered (no place or calendar source in the session) | |

## Open issues

None.

## Tests

```
cargo run -p cupertino-notifications --release --example notifications_samples   # output/*.png
cargo run -p cupertino-notifications --example notification-daemon        # prints notifications from the session bus
cargo test -p cupertino-notifications
```

| Test | Proves |
|---|---|
| `tests/dbus_integration.rs` | The server against a real client on a private `dbus-daemon`. Skipped when `dbus-daemon` is not installed. |
| `output/ | `banners_light.png`, `banners_dark.png` (three samples, one per row: hovered close button, wrapped text, link, buttons, the generic application icon) and `banner_slide_in.png` (the 0.25 s entry at quarter steps). |
