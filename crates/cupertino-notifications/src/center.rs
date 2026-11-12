//! The notification model: the banner slot and its waiting slot, notification-centre history,
//! grouping, do-not-disturb, hover hold and swipe-to-dismiss.
//!
//! The model is pure and synchronous. Time is an absolute `f64` in seconds on the host's
//! monotonic clock, passed to every call that depends on it; nothing here reads a clock or spawns
//! a thread. The host drives it with [`NotificationCenter::tick`] and reads the animated banner
//! rectangles from [`NotificationCenter::placements`].
//!
//! Behaviour follows the design banner controller: one
//! banner on screen and one waiting, a 5 s lifetime, a 1 s minimum before a waiting banner takes
//! over, a hover hold polled every second, a slide by the banner width and a drag that dismisses
//! rightwards and springs back leftwards.
//!
//! While the session is locked ([`NotificationCenter::set_screen_locked`]) nothing counts down: the banner on screen
//! keeps the rest of its lifetime until the unlock, the waiting one goes to the centre, and every notification that
//! arrives goes straight to the centre. Those the policy allows ([`NotificationSettings`]) are listed for the lock
//! screen ([`NotificationCenter::lock_screen`]), with their title and body only when its preview setting is
//! [`Previews::Always`]. At the unlock the banner on screen comes back without being announced again
//! ([`NotificationCenter::is_quiet`]).

use crate::notification::{
    cap, CloseReason, Expire, Notification, NotifyInput, Urgency, DEFAULT_ACTION_KEY,
    MAX_APP_NAME_CHARS, MAX_SUMMARY_CHARS,
};
use cupertino_animation::{CubicBezier, Spring, SpringParams};
use cupertino_defaults::notifications::{NotificationSettings, Policy, Previews};
use cupertino_geometry::{Point, Rect, Size};
use cupertino_theme::Theme;
use std::borrow::Cow;
use std::collections::VecDeque;

/// What a notification without its preview says in place of its title.
pub const REDACTED_TITLE: &str = "Notification";

/// The most notifications listed for the lock screen, newest first.
pub const LOCK_SCREEN_LIMIT: usize = 20;
/// The most of a body listed for the lock screen, in characters (it shows two lines of it at most).
pub const LOCK_SCREEN_BODY_CHARS: usize = 256;

/// A notification as the lock screen may show it: its application always, its title and body only when the
/// application's previews are [`Previews::Always`], else [`REDACTED_TITLE`] and no body. Each text is cut to its cap.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LockItem {
    pub id: u32,
    pub app: String,
    pub title: String,
    pub body: String,
}

/// Banner lifetimes.
#[derive(Clone, Debug, PartialEq)]
pub struct TimeoutPolicy {
    /// Seconds a banner of that urgency stays when the client asks for the default; `None` = an
    /// alert that stays until dismissed. `low` and `normal`: 5.0 s, the single banner lifetime of
    /// the design (the design has no urgency levels, so both share it).
    pub low: Option<f64>,
    pub normal: Option<f64>,
    /// `None`: critical notifications are presented as alerts, which skip the timer
    /// (mapping critical urgency to the alert style is approximate).
    pub critical: Option<f64>,
    /// Honour an explicit client timeout on critical notifications (approximate, protocol only).
    pub client_timeout_for_critical: bool,
    /// Upper bound of a client-requested timeout (approximate, protocol only).
    pub max_client_timeout: f64,
    /// Lifetime of a banner shown while another one is already waiting: 1.0 s.
    pub pending_lifetime: f64,
    /// A visible banner gives way to a waiting one only after this long on screen: 1.0 s
    /// (constant, elapsed-time source).
    pub min_on_screen: f64,
    /// When the timer fires while the pointer is over the banner, expiry is checked again after
    /// this long: 1.0 s (constant, predicate).
    pub hover_recheck: f64,
}

impl Default for TimeoutPolicy {
    fn default() -> Self {
        Self {
            low: Some(5.0),
            normal: Some(5.0),
            critical: None,
            client_timeout_for_critical: false,
            max_client_timeout: 60.0,
            pending_lifetime: 1.0,
            min_on_screen: 1.0,
            hover_recheck: 1.0,
        }
    }
}

impl TimeoutPolicy {
    /// Seconds until the banner expires, `None` for a banner that stays until dismissed.
    pub fn duration(&self, urgency: Urgency, expire: Expire) -> Option<f64> {
        let by_urgency = match urgency {
            Urgency::Low => self.low,
            Urgency::Normal => self.normal,
            Urgency::Critical => self.critical,
        };
        match expire {
            Expire::Default => by_urgency,
            Expire::Never => None,
            Expire::Millis(ms) => {
                if urgency == Urgency::Critical && !self.client_timeout_for_critical {
                    by_urgency
                } else {
                    Some((ms as f64 / 1000.0).min(self.max_client_timeout))
                }
            }
        }
    }
}

/// Banner geometry in points.
#[derive(Clone, Debug, PartialEq)]
pub struct StackLayout {
    /// 344 pt by default (role); see [`StackLayout::with_width_setting`].
    pub width: f64,
    /// Distance from the top of the output to the banner: menu bar height plus the 16 pt top
    /// padding of the list (roles).
    pub top_inset: f64,
    /// Distance from the right edge of the output: the 16 pt padding per side of the banner row
    /// (Dock at the bottom).
    pub right_margin: f64,
    /// Height assumed for a banner the host did not measure (approximate).
    pub fallback_height: f64,
}

impl Default for StackLayout {
    fn default() -> Self {
        Self {
            width: 344.0,
            top_inset: 24.0 + Self::LIST_TOP_PADDING,
            right_margin: Self::ROW_SIDE_PADDING,
            fallback_height: 72.0,
        }
    }
}

impl StackLayout {
    /// Top padding of the notification list below the menu bar: 16 pt (role).
    const LIST_TOP_PADDING: f64 = 16.0;
    /// Padding per side of a banner row: 16 pt.
    const ROW_SIDE_PADDING: f64 = 16.0;

    /// Translation of the slide-in and slide-out: the list controller width, card width + 2 x 16 pt
    /// (translation), so the card starts fully beyond the column's right edge.
    pub fn slide_distance(&self) -> f64 {
        self.width + 2.0 * Self::ROW_SIDE_PADDING
    }

    /// Layout for the user's `width` preference: `clamp(setting, 344, 620), so an unset preference
    /// (0) gives 344 pt.
    pub fn with_width_setting(setting: i64) -> Self {
        Self {
            width: setting.clamp(344, 620) as f64,
            ..Self::default()
        }
    }
}

/// What the Clear menu of the centre header removes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClearScope {
    /// Everything except pinned notifications.
    BannersOnly,
    /// Everything, pinned included.
    All,
}

impl ClearScope {
    /// Menu label: / of the header (strings).
    pub fn label(self) -> &'static str {
        match self {
            ClearScope::BannersOnly => "Banners Only",
            ClearScope::All => "All Notifications",
        }
    }
}

/// Pinned notifications survive "Banners Only". The pinned role is as designed; mapping it
/// to critical urgency, the alert style of this daemon, is approximate (the bus has no pin hint).
fn is_pinned(n: &Notification) -> bool {
    n.urgency == Urgency::Critical
}

/// Swipe-to-dismiss (left-to-right layouts).
#[derive(Clone, Debug, PartialEq)]
pub struct SwipeConfig {
    /// Travel before a press becomes a drag: 5 pt (that this gesture is the
    /// banner's is ).
    pub min_distance: f64,
    /// Leftward travel is clamped at −16 pt.
    pub left_limit: f64,
    /// Drag value divisor for the spring-back velocity and the dismiss speed: 100.
    pub velocity_divisor: f64,
    /// Lower bound of the dismiss speed: 8.
    pub min_dismiss_speed: f64,
    /// Spring back after a leftward release: mass 1, stiffness 100, damping 100; the
    /// initial velocity (`dx / velocity_divisor`, fractions of the distance per second) is set on
    /// release. Damping ratio 5 is clamped to 1 (critically damped), as the design layer
    /// framework's spring does; that the banner's spring clamps the same way is.
    pub spring_back: SpringParams,
}

impl Default for SwipeConfig {
    fn default() -> Self {
        Self {
            min_distance: 5.0,
            left_limit: -16.0,
            velocity_divisor: 100.0,
            min_dismiss_speed: 8.0,
            spring_back: SpringParams {
                mass: 1.0,
                stiffness: 100.0,
                damping: 100.0,
                ..SpringParams::default()
            },
        }
    }
}

impl SwipeConfig {
    /// Speed handed to the dismissal after a rightward release: `max(dx / 100, 8).
    /// Timer and enqueue dismissals take no speed and run at [`Motion::duration`].
    pub fn dismiss_speed(&self, dx: f64) -> f64 {
        (dx / self.velocity_divisor).max(self.min_dismiss_speed)
    }
}

/// Banner transitions.
#[derive(Clone, Debug, PartialEq)]
pub struct Motion {
    /// 0.25 s, the default transaction duration (that the banner insert/remove
    /// uses this helper is). Every non-swipe slide-out uses it as is.
    pub duration: f64,
    /// Ease-in-out.
    pub curve: CubicBezier,
    /// The exit is complete `duration + 0.01` s after it started.
    pub completion_slack: f64,
    /// Fade instead of the horizontal slide (branch, fade opacities 0 → 1 ).
    pub reduce_motion: bool,
    /// Animation speed = dismiss speed / 15 (divisor, that the swipe dismiss
    /// speed feeds it is with low confidence).
    pub speed_divisor: f64,
}

impl Default for Motion {
    fn default() -> Self {
        Self {
            duration: 0.25,
            curve: CubicBezier::EASE_IN_EASE_OUT,
            completion_slack: 0.01,
            reduce_motion: false,
            speed_divisor: 15.0,
        }
    }
}

impl Motion {
    /// Duration of a transition played at `speed` (see [`SwipeConfig::dismiss_speed`]).
    pub fn duration_at_speed(&self, speed: f64) -> f64 {
        self.duration * self.speed_divisor / speed
    }
}

#[derive(Clone, Debug)]
pub struct CenterConfig {
    pub timeouts: TimeoutPolicy,
    pub layout: StackLayout,
    pub swipe: SwipeConfig,
    pub motion: Motion,
    pub history_limit: usize,
    /// Critical notifications are shown while do-not-disturb is on.
    pub critical_bypasses_dnd: bool,
    /// A swiped-away banner also leaves the notification centre.
    pub swipe_removes_from_centre: bool,
    /// Capabilities reported by `GetCapabilities`.
    pub capabilities: Vec<String>,
}

impl Default for CenterConfig {
    fn default() -> Self {
        Self {
            timeouts: TimeoutPolicy::default(),
            layout: StackLayout::default(),
            swipe: SwipeConfig::default(),
            motion: Motion::default(),
            history_limit: 200,
            critical_bypasses_dnd: true,
            swipe_removes_from_centre: false,
            capabilities: ["actions", "body", "body-markup", "persistence"]
                .map(String::from)
                .to_vec(),
        }
    }
}

impl CenterConfig {
    /// Defaults with the top inset below the theme's menu bar.
    pub fn from_theme(theme: &Theme) -> Self {
        let layout = StackLayout {
            top_inset: theme.shapes.menu_bar_height + StackLayout::LIST_TOP_PADDING,
            ..StackLayout::default()
        };
        Self {
            layout,
            ..Self::default()
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DndMode {
    Off,
    On,
    /// On until the given time on the host clock.
    Until(f64),
}

/// Something the host has to report on the bus (or react to).
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Closed {
        id: u32,
        reason: CloseReason,
    },
    ActionInvoked {
        id: u32,
        key: String,
        activation_token: Option<String>,
    },
}

/// Measures banner heights; implemented by the renderer (text wrapping needs the font stack).
pub trait BannerMeasure {
    fn banner_height(&mut self, n: &Notification, width: f64) -> f64;
}

pub struct FixedHeight(pub f64);

impl BannerMeasure for FixedHeight {
    fn banner_height(&mut self, _: &Notification, _: f64) -> f64 {
        self.0
    }
}

/// One banner as it should be drawn this frame.
#[derive(Clone, Debug, PartialEq)]
pub struct Placement {
    pub id: u32,
    /// Output coordinates, slide and drag offsets included.
    pub rect: Rect,
    pub opacity: f64,
    pub hovered: bool,
    pub swiping: bool,
    pub leaving: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Wakeup {
    /// Something is animating: redraw on the next frame.
    Frame,
    /// Nothing animates; the next state change (expiry, hover re-check or removal) is at this time.
    At(f64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwipeOutcome {
    Dismissed,
    /// The banner springs back to rest.
    Cancelled,
    /// The press never travelled [`SwipeConfig::min_distance`]: the caller treats it as a click.
    Click,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActivateOutcome {
    /// The default action was invoked.
    ActionInvoked,
    /// No default action: the banner was dismissed.
    Dismissed,
    NotFound,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Group<'a> {
    pub key: String,
    pub app_name: &'a str,
    pub icon: &'a str,
    /// Newest first.
    pub notifications: Vec<&'a Notification>,
    pub unread: usize,
    pub latest: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    /// In the waiting slot.
    Pending,
    Visible {
        shown_at: f64,
    },
    Leaving {
        ends_at: f64,
    },
}

/// A scalar that is still, eased between two values, or settling with a spring.
#[derive(Clone, Copy, Debug)]
enum Track {
    Still(f64),
    Eased {
        from: f64,
        to: f64,
        start: f64,
        duration: f64,
        curve: CubicBezier,
    },
    Spring {
        from: f64,
        to: f64,
        start: f64,
        ends: f64,
        spring: Spring,
    },
}

impl Track {
    fn value(&self, now: f64) -> f64 {
        match *self {
            Track::Still(v) => v,
            Track::Eased {
                from,
                to,
                start,
                duration,
                curve,
            } => {
                let x = if duration <= 0.0 {
                    1.0
                } else {
                    ((now - start) / duration).clamp(0.0, 1.0)
                };
                from + (to - from) * curve.eval(x)
            }
            Track::Spring {
                from,
                to,
                start,
                ends,
                spring,
            } => {
                if now >= ends {
                    to
                } else {
                    from + (to - from) * spring.value(now - start)
                }
            }
        }
    }

    fn is_animating(&self, now: f64) -> bool {
        match *self {
            Track::Still(_) => false,
            Track::Eased {
                start, duration, ..
            } => now < start + duration,
            Track::Spring { ends, .. } => now < ends,
        }
    }

    /// Ease from the current value to `to`.
    fn ease_to(&mut self, to: f64, motion: &Motion, duration: f64, now: f64) {
        *self = Track::Eased {
            from: self.value(now),
            to,
            start: now,
            duration,
            curve: motion.curve,
        };
    }
}

#[derive(Clone, Copy, Debug)]
struct Drag {
    dx: f64,
    /// The pointer moved at least [`SwipeConfig::min_distance`].
    recognised: bool,
}

#[derive(Clone, Debug)]
struct Banner {
    phase: Phase,
    seq: u64,
    /// `None`: an alert without a timer.
    lifetime: Option<f64>,
    deadline: Option<f64>,
    /// The rest of the lifetime while the session is locked: nothing counts down until the unlock.
    paused: Option<f64>,
    /// The revision that came back on screen at the unlock: it was announced as it arrived, so it is not again.
    quiet_revision: Option<u32>,
    hovered: bool,
    drag: Option<Drag>,
    /// Transition offset (0 at rest, the banner width outside).
    slide: Track,
    opacity: Track,
    /// Drag offset, added to `slide`.
    offset: Track,
    height: f64,
    measured_revision: Option<u32>,
}

#[derive(Clone, Debug)]
struct Entry {
    n: Notification,
    in_centre: bool,
    read: bool,
    closed_signalled: bool,
    banner: Option<Banner>,
    /// It arrived, or was replaced, while the session was locked and do not disturb was off: the lock screen lists it.
    locked_arrival: bool,
}

pub struct NotificationCenter {
    pub config: CenterConfig,
    entries: Vec<Entry>,
    next_id: u32,
    next_seq: u64,
    dnd: DndMode,
    screen_locked: bool,
    policy: NotificationSettings,
    events: VecDeque<Event>,
    last_rects: Vec<(u32, Rect)>,
    revision: u64,
}

impl Default for NotificationCenter {
    fn default() -> Self {
        Self::new(CenterConfig::default())
    }
}

impl NotificationCenter {
    pub fn new(config: CenterConfig) -> Self {
        Self {
            config,
            entries: Vec::new(),
            next_id: 1,
            next_seq: 0,
            dnd: DndMode::Off,
            screen_locked: false,
            policy: NotificationSettings::UNSET,
            events: VecDeque::new(),
            last_rects: Vec::new(),
            revision: 0,
        }
    }

    /// Increments on every state change (cheap change detection for caches and redraw requests).
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn take_events(&mut self) -> Vec<Event> {
        self.events.drain(..).collect()
    }

    pub fn has_events(&self) -> bool {
        !self.events.is_empty()
    }

    fn touch(&mut self) {
        self.revision += 1;
    }

    fn alloc_id(&mut self) -> u32 {
        loop {
            let id = self.next_id;
            self.next_id = if self.next_id == u32::MAX {
                1
            } else {
                self.next_id + 1
            };
            if id != 0 && !self.entries.iter().any(|e| e.n.id == id) {
                return id;
            }
        }
    }

    fn index_of(&self, id: u32) -> Option<usize> {
        self.entries.iter().position(|e| e.n.id == id)
    }

    fn phase_of(&self, i: usize) -> Option<Phase> {
        self.entries[i].banner.as_ref().map(|b| b.phase)
    }

    fn find_phase(&self, pred: impl Fn(Phase) -> bool) -> Option<usize> {
        (0..self.entries.len()).find(|&i| self.phase_of(i).is_some_and(&pred))
    }

    fn visible_index(&self) -> Option<usize> {
        self.find_phase(|p| matches!(p, Phase::Visible { .. }))
    }

    fn pending_index(&self) -> Option<usize> {
        self.find_phase(|p| p == Phase::Pending)
    }

    /// Index of a notification that has not been closed yet (still in the centre or on screen
    /// and not already on its way out).
    fn live_index(&self, id: u32) -> Option<usize> {
        self.index_of(id).filter(|&i| {
            self.entries[i].in_centre
                || matches!(
                    self.phase_of(i),
                    Some(Phase::Pending | Phase::Visible { .. })
                )
        })
    }

    pub fn get(&self, id: u32) -> Option<&Notification> {
        self.entries.iter().find(|e| e.n.id == id).map(|e| &e.n)
    }

    /// The session was locked or unlocked at `now`. At the lock the banner on screen stops counting down (it keeps the
    /// rest of its lifetime), the waiting banner goes to the centre and a swipe or hover ends. At the unlock the banner
    /// on screen counts down again, at least [`TimeoutPolicy::min_on_screen`], and is quiet ([`Self::is_quiet`]); the
    /// lock screen's list empties.
    pub fn set_screen_locked(&mut self, locked: bool, now: f64) {
        if self.screen_locked == locked {
            return;
        }
        self.screen_locked = locked;
        if locked {
            if let Some(p) = self.pending_index() {
                self.drop_pending(p);
            }
            if let Some(i) = self.visible_index() {
                let b = self.entries[i].banner.as_mut().expect("visible banner");
                b.paused = b.deadline.take().map(|d| (d - now).max(0.0));
                b.hovered = false;
                if b.drag.take().is_some() {
                    b.offset = Track::Still(0.0);
                }
            }
            self.gc();
        } else {
            let min = self.config.timeouts.min_on_screen;
            for e in &mut self.entries {
                e.locked_arrival = false;
                let revision = e.n.revision;
                if let Some(b) = e.banner.as_mut() {
                    if let Some(rest) = b.paused.take() {
                        b.deadline = Some(now + rest.max(min));
                    }
                    if matches!(b.phase, Phase::Visible { .. }) {
                        b.quiet_revision = Some(revision);
                    }
                }
            }
            self.promote(now);
        }
        self.touch();
    }

    /// Whether the banner of `id` came back on screen at the unlock with the text it was announced with: it is not
    /// announced again. An update after the unlock is news, and is.
    pub fn is_quiet(&self, id: u32) -> bool {
        self.entries.iter().any(|e| {
            e.n.id == id
                && e.banner
                    .as_ref()
                    .is_some_and(|b| b.quiet_revision == Some(e.n.revision))
        })
    }

    /// Follow the settings of `notifications.conf`; the windows and the lock screen's list show them from now on.
    pub fn set_policy(&mut self, policy: NotificationSettings) {
        if self.policy != policy {
            self.policy = policy;
            // a banner with or without its preview is another height
            for b in self.entries.iter_mut().filter_map(|e| e.banner.as_mut()) {
                b.measured_revision = None;
            }
            self.touch();
        }
    }

    pub fn policy(&self) -> &NotificationSettings {
        &self.policy
    }

    /// The policy of `n`'s application, named by its [`Notification::group_key`].
    pub fn policy_of(&self, n: &Notification) -> Policy {
        self.policy.policy(&n.group_key())
    }

    /// `n` as the banners and the centre show it: whole, or without its preview when its application's previews are
    /// [`Previews::Never`] ([`redacted`]).
    pub fn shown<'a>(&self, n: &'a Notification) -> Cow<'a, Notification> {
        match self.policy_of(n).show_previews {
            Previews::Never => Cow::Owned(redacted(n)),
            Previews::Always | Previews::WhenUnlocked => Cow::Borrowed(n),
        }
    }

    /// The notifications the lock screen shows, newest first, at most [`LOCK_SCREEN_LIMIT`]: those that arrived while
    /// the session is locked, from applications allowed on the lock screen, redacted unless their previews are
    /// [`Previews::Always`]. Empty while the session is unlocked.
    pub fn lock_screen(&self) -> Vec<LockItem> {
        if !self.screen_locked {
            return Vec::new();
        }
        let mut listed: Vec<&Notification> = self
            .entries
            .iter()
            .filter(|e| e.locked_arrival && e.in_centre && !e.closed_signalled)
            .map(|e| &e.n)
            .filter(|n| self.policy_of(n).show_on_lock_screen)
            .collect();
        listed.sort_by(|a, b| b.timestamp.total_cmp(&a.timestamp).then(b.id.cmp(&a.id)));
        listed
            .into_iter()
            .take(LOCK_SCREEN_LIMIT)
            .map(|n| {
                let app = match n.app_name.trim() {
                    "" => REDACTED_TITLE.to_owned(),
                    app => cap(app, MAX_APP_NAME_CHARS).into_owned(),
                };
                let (title, body) = if self.policy_of(n).show_previews.shows_content(true) {
                    (
                        cap(&n.summary, MAX_SUMMARY_CHARS).into_owned(),
                        cap(&n.body_text(), LOCK_SCREEN_BODY_CHARS).into_owned(),
                    )
                } else {
                    (REDACTED_TITLE.to_owned(), String::new())
                };
                LockItem {
                    id: n.id,
                    app,
                    title,
                    body,
                }
            })
            .collect()
    }

    /// The applications of the notifications on screen and in the centre: their [`Notification::group_key`] and
    /// name, sorted by key, each once.
    pub fn applications(&self) -> Vec<(String, String)> {
        let mut apps: Vec<(String, String)> = Vec::new();
        for e in &self.entries {
            let key = e.n.group_key();
            if !apps.iter().any(|(k, _)| *k == key) {
                let name = match e.n.app_name.trim() {
                    "" => key.clone(),
                    name => cap(name, MAX_APP_NAME_CHARS).into_owned(),
                };
                apps.push((key, name));
            }
        }
        apps.sort();
        apps
    }

    /// Whether the screen is locked, as the host last said ([`Self::set_screen_locked`]).
    pub fn screen_locked(&self) -> bool {
        self.screen_locked
    }

    pub fn dnd(&self) -> DndMode {
        self.dnd
    }

    pub fn dnd_active(&self, now: f64) -> bool {
        match self.dnd {
            DndMode::Off => false,
            DndMode::On => true,
            DndMode::Until(t) => now < t,
        }
    }

    /// Switching do-not-disturb on drops the waiting banner; the banner on screen finishes
    /// normally. Everything stays in the notification centre.
    pub fn set_dnd(&mut self, mode: DndMode, now: f64) {
        self.dnd = mode;
        if self.dnd_active(now) {
            if let Some(i) = self.pending_index() {
                self.drop_pending(i);
            }
            self.gc();
        }
        self.touch();
    }

    fn new_banner(&self, n: &Notification) -> Banner {
        Banner {
            phase: Phase::Pending,
            seq: 0,
            lifetime: self.config.timeouts.duration(n.urgency, n.expire),
            deadline: None,
            paused: None,
            quiet_revision: None,
            hovered: false,
            drag: None,
            slide: Track::Still(0.0),
            opacity: Track::Still(1.0),
            offset: Track::Still(0.0),
            height: self.config.layout.fallback_height,
            measured_revision: None,
        }
    }

    /// Dismissal time of a banner (re)shown at `now`: its lifetime, or 1.0 s when another banner
    /// is already waiting. Alerts get none.
    fn arm(&self, lifetime: Option<f64>, now: f64) -> Option<f64> {
        let waiting = self.pending_index().is_some();
        lifetime.map(|l| {
            now + if waiting {
                l.min(self.config.timeouts.pending_lifetime)
            } else {
                l
            }
        })
    }

    /// Handle a `Notify` call. Returns the id of the notification (the replaced id when it exists).
    pub fn notify(&mut self, input: NotifyInput, now: f64) -> u32 {
        let existing = (input.replaces_id != 0)
            .then(|| self.live_index(input.replaces_id))
            .flatten();
        let suppressed = self.dnd_active(now)
            && !(self.config.critical_bypasses_dnd
                && self.policy.critical_during_dnd
                && input.hints.urgency == Some(Urgency::Critical));
        let id = match existing {
            Some(i) => self.entries[i].n.id,
            None => self.alloc_id(),
        };
        let mut n = Notification::from_input(id, input, now);
        let app_policy = self.policy_of(&n);
        // an application the person turned off is neither shown nor kept
        if !app_policy.allow {
            self.events.push_back(Event::Closed {
                id,
                reason: CloseReason::Undefined,
            });
            return id;
        }
        let in_centre = !n.transient;
        // while locked nothing new takes the screen: it goes to the centre, and the lock screen lists it; an
        // application without banners puts its notifications there too
        let locked = self.screen_locked;
        let held = suppressed
            || locked
            || app_policy.alert_style == cupertino_defaults::notifications::AlertStyle::None;
        let locked_arrival = locked && !suppressed;
        let mut queued = None;
        match existing {
            Some(i) => {
                n.revision = self.entries[i].n.revision + 1;
                let lifetime = self.config.timeouts.duration(n.urgency, n.expire);
                let deadline = self.arm(lifetime, now);
                let e = &mut self.entries[i];
                e.n = n;
                e.read = false;
                e.closed_signalled = false;
                e.in_centre = in_centre;
                e.locked_arrival |= locked_arrival;
                match &mut e.banner {
                    // shown again in place: the timer restarts, the banner does not slide again; a swipe in
                    // progress ends, as the banner's window may be made again for the new height
                    Some(b) if matches!(b.phase, Phase::Visible { .. }) => {
                        if b.drag.take().is_some() {
                            b.offset = Track::Still(0.0);
                        }
                        b.lifetime = lifetime;
                        // while locked the new lifetime waits for the unlock
                        if locked {
                            b.paused = lifetime;
                            b.deadline = None;
                        } else {
                            b.deadline = deadline;
                        }
                        b.phase = Phase::Visible { shown_at: now };
                    }
                    Some(b) if b.phase == Phase::Pending => b.lifetime = lifetime,
                    _ => {
                        e.banner = None;
                        if !held {
                            queued = Some(i);
                        } else if !in_centre {
                            // a transient update while held is never seen
                            self.signal_if_needed(i, CloseReason::Undefined);
                        }
                    }
                }
            }
            None => {
                if held && !in_centre {
                    self.events.push_back(Event::Closed {
                        id,
                        reason: CloseReason::Undefined,
                    });
                } else {
                    self.entries.push(Entry {
                        n,
                        in_centre,
                        read: false,
                        closed_signalled: false,
                        banner: None,
                        locked_arrival,
                    });
                    if !held {
                        queued = Some(self.entries.len() - 1);
                    }
                }
            }
        }
        if let Some(i) = queued {
            self.queue(i, now);
        }
        self.enforce_history_limit();
        self.gc();
        self.promote(now);
        self.touch();
        id
    }

    /// Put entry `i` into the waiting slot. A banner already waiting goes straight to the centre;
    /// the visible one gives way once it has been on screen for the minimum time.
    fn queue(&mut self, i: usize, now: f64) {
        if let Some(p) = self.pending_index() {
            self.drop_pending(p);
        }
        let b = self.new_banner(&self.entries[i].n);
        self.entries[i].banner = Some(b);
        let min_on_screen = self.config.timeouts.min_on_screen;
        if let Some(v) = self.visible_index() {
            let b = self.entries[v].banner.as_mut().expect("visible banner");
            if let (Phase::Visible { shown_at }, Some(d)) = (b.phase, b.deadline) {
                b.deadline = Some(d.min((shown_at + min_on_screen).max(now)));
            }
            self.expire_due(now);
        }
    }

    /// Drop the waiting banner; a transient notification disappears with it.
    fn drop_pending(&mut self, i: usize) {
        self.entries[i].banner = None;
        if !self.entries[i].in_centre {
            self.signal_if_needed(i, CloseReason::Undefined);
        }
    }

    fn enforce_history_limit(&mut self) {
        let mut over = self.history_len().saturating_sub(self.config.history_limit);
        let mut i = 0;
        while over > 0 && i < self.entries.len() {
            if self.entries[i].in_centre && self.entries[i].banner.is_none() {
                let e = self.entries.remove(i);
                if !e.closed_signalled {
                    self.events.push_back(Event::Closed {
                        id: e.n.id,
                        reason: CloseReason::Undefined,
                    });
                }
                over -= 1;
            } else {
                i += 1;
            }
        }
    }

    fn gc(&mut self) {
        self.entries.retain(|e| e.in_centre || e.banner.is_some());
    }

    /// Show the waiting banner when the slot is free.
    fn promote(&mut self, now: f64) {
        if self.screen_locked || self.visible_index().is_some() {
            return;
        }
        let Some(i) = self.pending_index() else {
            return;
        };
        self.next_seq += 1;
        let seq = self.next_seq;
        let m = self.config.motion.clone();
        let slide = self.config.layout.slide_distance();
        let b = self.entries[i].banner.as_mut().expect("pending banner");
        b.phase = Phase::Visible { shown_at: now };
        let lifetime = b.lifetime;
        let deadline = self.arm(lifetime, now);
        let b = self.entries[i].banner.as_mut().expect("visible banner");
        b.seq = seq;
        b.deadline = deadline;
        if m.reduce_motion {
            b.opacity = Track::Still(0.0);
            b.opacity.ease_to(1.0, &m, m.duration, now);
        } else {
            b.slide = Track::Still(slide);
            b.slide.ease_to(0.0, &m, m.duration, now);
        }
    }

    /// Start the exit transition of a banner (it is removed once the transition completed) and
    /// report the close reason once.
    fn start_leaving(&mut self, idx: usize, reason: CloseReason, duration: f64, now: f64) {
        let m = self.config.motion.clone();
        let slide = self.config.layout.slide_distance();
        let Some(b) = self.entries[idx].banner.as_mut() else {
            return;
        };
        if matches!(b.phase, Phase::Leaving { .. }) {
            return;
        }
        b.drag = None;
        b.hovered = false;
        b.deadline = None;
        b.offset = Track::Still(b.offset.value(now));
        b.phase = Phase::Leaving {
            ends_at: now + duration + m.completion_slack,
        };
        if m.reduce_motion {
            b.opacity.ease_to(0.0, &m, duration, now);
        } else {
            b.slide.ease_to(slide, &m, duration, now);
        }
        self.signal_if_needed(idx, reason);
    }

    fn signal_if_needed(&mut self, idx: usize, reason: CloseReason) {
        let e = &mut self.entries[idx];
        if !e.closed_signalled {
            e.closed_signalled = true;
            self.events.push_back(Event::Closed { id: e.n.id, reason });
        }
    }

    /// `CloseNotification`: removes the notification from the screen and the centre. Returns
    /// whether it existed.
    pub fn close(&mut self, id: u32, now: f64) -> bool {
        self.remove(id, CloseReason::Closed, now)
    }

    /// Remove from the notification centre (the user's clear button); reported as dismissed.
    pub fn clear(&mut self, id: u32, now: f64) -> bool {
        self.remove(id, CloseReason::Dismissed, now)
    }

    fn remove(&mut self, id: u32, reason: CloseReason, now: f64) -> bool {
        let Some(i) = self.live_index(id) else {
            return false;
        };
        self.entries[i].in_centre = false;
        match self.phase_of(i) {
            Some(Phase::Pending) => self.entries[i].banner = None,
            Some(_) => self.start_leaving(i, reason, self.config.motion.duration, now),
            None => {}
        }
        self.signal_if_needed(i, reason);
        self.gc();
        self.promote(now);
        self.touch();
        true
    }

    pub fn clear_group(&mut self, key: &str, now: f64) -> usize {
        let ids: Vec<u32> = self
            .entries
            .iter()
            .filter(|e| e.in_centre && e.n.group_key() == key)
            .map(|e| e.n.id)
            .collect();
        ids.iter().for_each(|id| {
            self.clear(*id, now);
        });
        ids.len()
    }

    /// Scopes the Clear menu offers now: "Banners Only" only while pinned notifications
    /// and others are both in the centre, "All Notifications" always. Empty-menu states do not occur.
    pub fn clear_scopes(&self) -> Vec<ClearScope> {
        let (pinned, others) =
            self.entries
                .iter()
                .filter(|e| e.in_centre)
                .fold((0, 0), |(p, o), e| {
                    if is_pinned(&e.n) {
                        (p + 1, o)
                    } else {
                        (p, o + 1)
                    }
                });
        let mut scopes = Vec::new();
        if pinned > 0 && others > 0 {
            scopes.push(ClearScope::BannersOnly);
        }
        scopes.push(ClearScope::All);
        scopes
    }

    /// Clear the notification centre by `scope`; returns how many notifications left.
    pub fn clear_scope(&mut self, scope: ClearScope, now: f64) -> usize {
        let ids: Vec<u32> = self
            .entries
            .iter()
            .filter(|e| e.in_centre && (scope == ClearScope::All || !is_pinned(&e.n)))
            .map(|e| e.n.id)
            .collect();
        ids.iter().for_each(|id| {
            self.clear(*id, now);
        });
        ids.len()
    }

    pub fn mark_read(&mut self, id: u32) {
        if let Some(e) = self.entries.iter_mut().find(|e| e.n.id == id) {
            e.read = true;
            self.touch();
        }
    }

    /// Invoke an action (`ActionInvoked`). Unless the notification is resident it is closed
    /// afterwards, as the specification requires. `activation_token` is forwarded to the client.
    pub fn invoke_action(
        &mut self,
        id: u32,
        key: &str,
        activation_token: Option<String>,
        now: f64,
    ) -> bool {
        let Some(i) = self.live_index(id) else {
            return false;
        };
        if !self.entries[i].n.actions.iter().any(|a| a.key == key) {
            return false;
        }
        self.events.push_back(Event::ActionInvoked {
            id,
            key: key.to_string(),
            activation_token,
        });
        self.entries[i].read = true;
        if self.entries[i].n.resident {
            self.dismiss_banner(id, now);
        } else {
            self.clear(id, now);
        }
        self.touch();
        true
    }

    /// Click on the banner body: invokes the default action, else dismisses the banner.
    pub fn activate(
        &mut self,
        id: u32,
        activation_token: Option<String>,
        now: f64,
    ) -> ActivateOutcome {
        let Some(i) = self.live_index(id) else {
            return ActivateOutcome::NotFound;
        };
        if self.entries[i].n.has_default_action() {
            self.invoke_action(id, DEFAULT_ACTION_KEY, activation_token, now);
            ActivateOutcome::ActionInvoked
        } else {
            self.entries[i].read = true;
            self.dismiss_banner(id, now);
            ActivateOutcome::Dismissed
        }
    }

    /// Dismiss only the banner (the entry stays in the notification centre).
    pub fn dismiss_banner(&mut self, id: u32, now: f64) {
        let Some(i) = self.index_of(id) else { return };
        match self.phase_of(i) {
            Some(Phase::Pending) => {
                self.drop_pending(i);
                self.gc();
            }
            Some(Phase::Visible { .. }) => {
                self.start_leaving(i, CloseReason::Dismissed, self.config.motion.duration, now)
            }
            _ => {}
        }
        self.promote(now);
        self.touch();
    }

    /// Pointer hover (`None` = no banner hovered). A change of the hovered banner re-arms the
    /// visible banner's timer to its full lifetime; while hovered it does not expire.
    pub fn set_hover(&mut self, id: Option<u32>, now: f64) {
        let Some(i) = self.visible_index() else {
            return;
        };
        let want = Some(self.entries[i].n.id) == id && !self.screen_locked;
        let b = self.entries[i].banner.as_mut().expect("visible banner");
        if b.hovered == want || b.paused.is_some() {
            return;
        }
        b.hovered = want;
        b.deadline = b.lifetime.map(|l| now + l);
        self.touch();
    }

    /// Press on the visible banner; it becomes a drag once it moved [`SwipeConfig::min_distance`].
    pub fn swipe_begin(&mut self, id: u32, _now: f64) -> bool {
        let Some(i) = self.visible_index().filter(|&i| self.entries[i].n.id == id) else {
            return false;
        };
        let b = self.entries[i].banner.as_mut().expect("visible banner");
        b.drag = Some(Drag {
            dx: 0.0,
            recognised: false,
        });
        true
    }

    /// `dx`: horizontal pointer travel since the press (positive = towards the screen edge).
    /// Leftward travel is clamped at [`SwipeConfig::left_limit`].
    pub fn swipe_update(&mut self, id: u32, dx: f64, _now: f64) {
        let cfg = self.config.swipe.clone();
        let Some(i) = self.visible_index().filter(|&i| self.entries[i].n.id == id) else {
            return;
        };
        let b = self.entries[i].banner.as_mut().expect("visible banner");
        let Some(drag) = b.drag.as_mut() else { return };
        drag.recognised |= dx.abs() >= cfg.min_distance;
        if drag.recognised {
            drag.dx = dx.max(cfg.left_limit);
            b.offset = Track::Still(drag.dx);
            self.touch();
        }
    }

    /// Release. A press that never became a drag is a `Click`. A leftward drag springs back; any other drag dismisses the banner, its exit played at the
    /// dismiss speed.
    pub fn swipe_end(&mut self, id: u32, now: f64) -> SwipeOutcome {
        let cfg = self.config.swipe.clone();
        let Some(i) = self.visible_index().filter(|&i| self.entries[i].n.id == id) else {
            return SwipeOutcome::Cancelled;
        };
        let b = self.entries[i].banner.as_mut().expect("visible banner");
        let Some(drag) = b.drag.take() else {
            return SwipeOutcome::Cancelled;
        };
        if !drag.recognised {
            return SwipeOutcome::Click;
        }
        if drag.dx < 0.0 {
            let spring = Spring::new(SpringParams {
                initial_velocity: drag.dx / cfg.velocity_divisor,
                ..cfg.spring_back
            });
            b.offset = Track::Spring {
                from: drag.dx,
                to: 0.0,
                start: now,
                ends: now + spring.settling_duration(),
                spring,
            };
            self.touch();
            return SwipeOutcome::Cancelled;
        }
        self.touch();
        if self.config.swipe_removes_from_centre {
            self.entries[i].in_centre = false;
        }
        let duration = self
            .config
            .motion
            .duration_at_speed(cfg.dismiss_speed(drag.dx));
        self.start_leaving(i, CloseReason::Dismissed, duration, now);
        self.gc();
        self.promote(now);
        SwipeOutcome::Dismissed
    }

    /// Expire the visible banner when its timer is due; a hovered or dragged banner is checked
    /// again after [`TimeoutPolicy::hover_recheck`].
    fn expire_due(&mut self, now: f64) -> bool {
        let Some(i) = self.visible_index() else {
            return false;
        };
        let recheck = self.config.timeouts.hover_recheck;
        let b = self.entries[i].banner.as_mut().expect("visible banner");
        if !b.deadline.is_some_and(|d| d <= now) {
            return false;
        }
        if b.hovered || b.drag.is_some() {
            b.deadline = Some(now + recheck);
        } else {
            self.start_leaving(i, CloseReason::Expired, self.config.motion.duration, now);
        }
        true
    }

    /// Advance timers: expire the banner, finish exit transitions, show the waiting banner. Call
    /// when [`Self::next_wakeup`] says so and before reading [`Self::placements`].
    pub fn tick(&mut self, now: f64) {
        let mut changed = self.expire_due(now);
        for e in &mut self.entries {
            if let Some(Phase::Leaving { ends_at }) = e.banner.as_ref().map(|b| b.phase) {
                if ends_at <= now {
                    e.banner = None;
                    changed = true;
                }
            }
        }
        if matches!(self.dnd, DndMode::Until(t) if now >= t) {
            self.dnd = DndMode::Off;
            changed = true;
        }
        if changed {
            self.gc();
            self.touch();
        }
        self.promote(now);
    }

    pub fn is_animating(&self, now: f64) -> bool {
        self.entries
            .iter()
            .filter_map(|e| e.banner.as_ref())
            .any(|b| {
                b.slide.is_animating(now)
                    || b.opacity.is_animating(now)
                    || b.offset.is_animating(now)
            })
    }

    pub fn next_wakeup(&self, now: f64) -> Option<Wakeup> {
        if self.is_animating(now) {
            return Some(Wakeup::Frame);
        }
        let mut t: Option<f64> = None;
        let mut take = |v: f64| t = Some(t.map_or(v, |c| c.min(v)));
        for b in self.entries.iter().filter_map(|e| e.banner.as_ref()) {
            if let Some(d) = b.deadline {
                take(d);
            }
            if let Phase::Leaving { ends_at } = b.phase {
                take(ends_at);
            }
        }
        if let DndMode::Until(u) = self.dnd {
            take(u);
        }
        t.map(Wakeup::At)
    }

    /// Number of banners on screen (leaving ones included).
    pub fn visible_count(&self) -> usize {
        self.visible_ids().len()
    }

    /// Number of banners waiting for the slot (0 or 1).
    pub fn queued_count(&self) -> usize {
        usize::from(self.pending_index().is_some())
    }

    /// Ids of the banners on screen: the visible one first, then leaving ones, newest first.
    pub fn visible_ids(&self) -> Vec<u32> {
        let mut v: Vec<(u32, &Banner)> = self
            .entries
            .iter()
            .filter_map(|e| e.banner.as_ref().map(|b| (e.n.id, b)))
            .filter(|(_, b)| b.phase != Phase::Pending)
            .collect();
        v.sort_by_key(|(_, b)| {
            (
                matches!(b.phase, Phase::Leaving { .. }),
                std::cmp::Reverse(b.seq),
            )
        });
        v.into_iter().map(|(id, _)| id).collect()
    }

    /// Resolve the animated rectangles of the banners on screen at time `now`. Remeasures banners
    /// whose notification changed and caches the result for [`Self::banner_at`].
    pub fn placements(
        &mut self,
        viewport: Size,
        measure: &mut dyn BannerMeasure,
        now: f64,
    ) -> Vec<Placement> {
        let layout = self.config.layout.clone();
        let mut out = Vec::new();
        for id in self.visible_ids() {
            let e = self
                .entries
                .iter_mut()
                .find(|e| e.n.id == id)
                .expect("visible id");
            let b = e.banner.as_mut().expect("banner");
            if b.measured_revision != Some(e.n.revision) {
                let shown = match self.policy.policy(&e.n.group_key()).show_previews {
                    Previews::Never => Cow::Owned(redacted(&e.n)),
                    _ => Cow::Borrowed(&e.n),
                };
                b.height = measure.banner_height(&shown, layout.width).max(1.0);
                b.measured_revision = Some(e.n.revision);
            }
            let x = viewport.width - layout.right_margin - layout.width
                + b.slide.value(now)
                + b.offset.value(now);
            out.push(Placement {
                id,
                rect: Rect::new(x, layout.top_inset, layout.width, b.height),
                opacity: b.opacity.value(now).clamp(0.0, 1.0),
                hovered: b.hovered,
                swiping: b.drag.is_some_and(|d| d.recognised),
                leaving: matches!(b.phase, Phase::Leaving { .. }),
            });
        }
        self.last_rects = out
            .iter()
            .filter(|p| !p.leaving)
            .map(|p| (p.id, p.rect))
            .collect();
        out
    }

    /// Non-leaving banner under `p` according to the last [`Self::placements`] call.
    pub fn banner_at(&self, p: Point) -> Option<u32> {
        self.last_rects
            .iter()
            .find(|(_, r)| r.contains(p))
            .map(|(id, _)| *id)
    }

    /// Notification-centre history, newest first.
    pub fn history(&self) -> Vec<&Notification> {
        let mut v: Vec<&Notification> = self
            .entries
            .iter()
            .filter(|e| e.in_centre)
            .map(|e| &e.n)
            .collect();
        v.sort_by(|a, b| b.timestamp.total_cmp(&a.timestamp).then(b.id.cmp(&a.id)));
        v
    }

    pub fn history_len(&self) -> usize {
        self.entries.iter().filter(|e| e.in_centre).count()
    }

    pub fn unread_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| e.in_centre && !e.read)
            .count()
    }

    pub fn is_read(&self, id: u32) -> bool {
        self.entries
            .iter()
            .find(|e| e.n.id == id)
            .is_some_and(|e| e.read)
    }

    /// Notification-centre history grouped by application, groups ordered by their newest
    /// notification, notifications newest first.
    pub fn groups(&self) -> Vec<Group<'_>> {
        let mut groups: Vec<Group<'_>> = Vec::new();
        for n in self.history() {
            let key = n.group_key();
            let unread = usize::from(!self.is_read(n.id));
            match groups.iter_mut().find(|g| g.key == key) {
                Some(g) => {
                    g.notifications.push(n);
                    g.unread += unread;
                    if g.icon.is_empty() {
                        g.icon = &n.icon;
                    }
                }
                None => groups.push(Group {
                    key,
                    app_name: &n.app_name,
                    icon: &n.icon,
                    notifications: vec![n],
                    unread,
                    latest: n.timestamp,
                }),
            }
        }
        groups
    }
}

/// `n` without its preview: its title [`REDACTED_TITLE`], no body, image or action buttons. Its default action stays,
/// so a click still opens the application.
pub fn redacted(n: &Notification) -> Notification {
    Notification {
        summary: REDACTED_TITLE.to_owned(),
        body: String::new(),
        spans: Vec::new(),
        actions: n
            .actions
            .iter()
            .filter(|a| a.key == DEFAULT_ACTION_KEY)
            .cloned()
            .collect(),
        image: None,
        image_path: None,
        ..n.clone()
    }
}

#[cfg(test)]
mod tests;
