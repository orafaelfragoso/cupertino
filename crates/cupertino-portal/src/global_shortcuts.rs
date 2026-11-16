//! `org.freedesktop.impl.portal.GlobalShortcuts`: `CreateSession`, `BindShortcuts` and `ListShortcuts`, the
//! `org.freedesktop.impl.portal.Session` object of each session, and the `Activated`, `Deactivated` and
//! `ShortcutsChanged` signals. A shortcut is a chord an application may use while another application has the keyboard,
//! such as a push-to-talk key.
//!
//! The first `BindShortcuts` of an app becomes an [`Ask`] for the dialog, which lists the shortcuts with their keys and
//! answers through its [`Reply`]; the answer is remembered for the app (`global-shortcuts.conf`,
//! [`cupertino_defaults::global_shortcuts`]) and every later call of the app follows it without asking. What is allowed
//! goes to the server as [`Keys`] requests (`cupertino_global_shortcuts_v1`), and the server's presses and releases come
//! back through [`Handle::fired`] as `Activated` and `Deactivated`.
//!
//! The clash rule ([`Registry::offer`]): a shortcut gets its key only when the key is readable, holds Control, Option
//! (Alt) or Command (Super) or is a function or media key, is none the system keeps for itself, and no other session holds it;
//! the first shortcut of a call that names a key keeps it. Any other shortcut is not bound: it is shown in the dialog
//! with the reason and left out of the answer, so the app learns which shortcuts it has.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use cupertino_defaults::desktop_control::Decision;
use cupertino_defaults::global_shortcuts::{is_rememberable, GlobalShortcutsConf};
use xkbcommon::xkb;
use zbus::message::Header;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};
use zbus::{interface, Connection};

use crate::consent::identify;
use crate::request::{answered, from_frontend, Exported, PortalError, Response};
use crate::session::Session;
use crate::PORTAL_PATH;

/// The interface version served.
pub const VERSION: u32 = 1;
/// The interface's name.
pub const INTERFACE: &str = "org.freedesktop.impl.portal.GlobalShortcuts";
/// At most this many sessions are open at once, and this many for one app id.
pub const MAX_SESSIONS: usize = 32;
pub const MAX_SESSIONS_PER_APP: usize = 4;
/// At most this many shortcuts in one call.
pub const MAX_SHORTCUTS: usize = 64;
/// Bytes of an app id, a shortcut id, a description, a trigger or a parent window.
pub const MAX_TEXT: usize = 1024;

/// The modifier bits of a [`Trigger`], as `cupertino_global_shortcuts_v1` carries them.
pub mod modifier {
    pub const SHIFT: u32 = 1;
    pub const CTRL: u32 = 2;
    pub const ALT: u32 = 4;
    pub const LOGO: u32 = 8;
}

/// A chord: the physical modifiers and a keysym, letters in lower case.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Trigger {
    pub modifiers: u32,
    pub keysym: u32,
}

/// `XKB_KEY_F1` and `XKB_KEY_F35`.
const F1: u32 = 0xffbe;
const F35: u32 = 0xffe0;
/// The keysyms of the media and special keys of a keyboard (`XF86…`).
const MEDIA: std::ops::RangeInclusive<u32> = 0x1008_ff00..=0x1008_ffff;

impl Trigger {
    /// The chord of a `preferred_trigger`: modifiers and a key name joined by `+` as the shortcuts specification writes
    /// them (`CTRL+ALT+k`, `LOGO+Return`), or GTK's accelerator form (`<Control><Alt>k`). Modifier names are read
    /// without regard to case; the key is an xkb keysym name or the character it types. `None` when it names no key.
    pub fn parse(text: &str) -> Option<Trigger> {
        let mut modifiers = 0;
        let mut rest = text.trim();
        // GTK's form: `<Name>` prefixes
        while let Some(tail) = rest.strip_prefix('<') {
            let (name, after) = tail.split_once('>')?;
            modifiers |= modifier_named(name)?;
            rest = after;
        }
        let mut parts: Vec<&str> = rest.split('+').collect();
        // `CTRL++` names the plus key
        if rest.ends_with("++") {
            parts.truncate(parts.len() - 2);
            parts.push("plus");
        }
        let key = parts.pop()?.trim();
        for part in parts {
            modifiers |= modifier_named(part.trim())?;
        }
        if key.is_empty() {
            return None;
        }
        let key = key_alias(key).unwrap_or(key);
        let mut keysym = xkb::keysym_from_name(key, xkb::KEYSYM_NO_FLAGS).raw();
        if keysym == 0 {
            keysym = xkb::keysym_from_name(key, xkb::KEYSYM_CASE_INSENSITIVE).raw();
        }
        if keysym == 0 {
            let mut chars = key.chars();
            if let (Some(c), None) = (chars.next(), chars.next()) {
                keysym = xkb::utf32_to_keysym(c as u32).raw();
            }
        }
        if keysym == 0 {
            return None;
        }
        Some(Trigger {
            modifiers,
            keysym: lower(keysym),
        })
    }

    /// The chord as the person reads it: the modifiers' symbols in their usual order, then the key (a letter in upper
    /// case, else its name).
    pub fn describe(&self) -> String {
        let mut text = String::new();
        for (bit, symbol) in [
            (modifier::CTRL, '⌃'),
            (modifier::ALT, '⌥'),
            (modifier::SHIFT, '⇧'),
            (modifier::LOGO, '⌘'),
        ] {
            if self.modifiers & bit != 0 {
                text.push(symbol);
            }
        }
        let typed = char::from_u32(xkb::keysym_to_utf32(xkb::Keysym::new(self.keysym)))
            .filter(|c| !c.is_control() && !c.is_whitespace() && *c != '\0');
        match typed {
            Some(c) => text.extend(c.to_uppercase()),
            None => text.push_str(&xkb::keysym_get_name(xkb::Keysym::new(self.keysym))),
        }
        text
    }

    /// Whether the chord holds a modifier that makes it a command rather than typing: Control, Option or Command; a
    /// function key or a media key needs none.
    fn commands(&self) -> bool {
        self.modifiers & (modifier::CTRL | modifier::ALT | modifier::LOGO) != 0
            || (F1..=F35).contains(&self.keysym)
            || MEDIA.contains(&self.keysym)
    }
}

/// The xkb name of a key some programs name otherwise (web key names, Electron's accelerators).
fn key_alias(key: &str) -> Option<&'static str> {
    Some(match key.to_ascii_lowercase().as_str() {
        "enter" => "Return",
        "esc" => "Escape",
        "up" | "arrowup" => "Up",
        "down" | "arrowdown" => "Down",
        "left" | "arrowleft" => "Left",
        "right" | "arrowright" => "Right",
        "pageup" => "Prior",
        "pagedown" => "Next",
        "backspace" => "BackSpace",
        "capslock" => "Caps_Lock",
        "numlock" => "Num_Lock",
        "scrolllock" => "Scroll_Lock",
        "printscreen" => "Print",
        "mediaplaypause" => "XF86AudioPlay",
        "mediastop" => "XF86AudioStop",
        "medianexttrack" => "XF86AudioNext",
        "mediaprevioustrack" => "XF86AudioPrev",
        "volumeup" | "audiovolumeup" => "XF86AudioRaiseVolume",
        "volumedown" | "audiovolumedown" => "XF86AudioLowerVolume",
        "volumemute" | "audiovolumemute" => "XF86AudioMute",
        _ => return None,
    })
}

fn modifier_named(name: &str) -> Option<u32> {
    Some(match name.to_ascii_lowercase().as_str() {
        "shift" => modifier::SHIFT,
        // the command key of a program written for several systems is Control here
        "ctrl" | "control" | "primary" | "command" | "cmd" | "commandorcontrol" | "cmdorctrl" => {
            modifier::CTRL
        }
        "alt" | "mod1" | "option" => modifier::ALT,
        "logo" | "super" | "meta" | "mod4" => modifier::LOGO,
        _ => return None,
    })
}

/// A letter keysym in lower case, the shifted tab as tab, as the server matches chords.
fn lower(keysym: u32) -> u32 {
    const ISO_LEFT_TAB: u32 = 0xfe20;
    const TAB: u32 = 0xff09;
    match keysym {
        ISO_LEFT_TAB => TAB,
        0x41..=0x5a => keysym + 0x20,
        _ => keysym,
    }
}

/// Why a shortcut gets no key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The app named no key, or one that is not a key.
    NoKey,
    /// The key holds no Control, Option or Command and is no function key: it would take typing away.
    Typing,
    /// The system keeps the key for itself.
    System,
    /// Another session holds it: the app id of its app.
    Taken(String),
    /// An earlier shortcut of the same call names it.
    Twice,
}

/// One shortcut as the dialog shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Offered {
    pub id: String,
    pub description: String,
    pub trigger: Option<Trigger>,
    /// `None` when the shortcut gets its key.
    pub refusal: Option<Refusal>,
}

/// What the dialog asks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ask {
    pub app_id: String,
    /// The name of the app's desktop entry, else its id; `None` when the id is empty.
    pub app_name: Option<String>,
    pub app_icon: Option<String>,
    /// `wayland:<handle>`, `x11:<xid>` or empty.
    pub parent_window: String,
    pub shortcuts: Vec<Offered>,
    /// The names of the apps holding the keys refused as [`Refusal::Taken`], by app id.
    pub holders: HashMap<String, String>,
}

/// How the dialog ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    /// Allow: remembered.
    Allowed,
    /// Don't Allow: remembered.
    Denied,
    /// Escape or the window closed: nothing is bound and nothing remembered.
    Dismissed,
}

/// The dialog's answer to one [`Ask`].
pub struct Reply(pub(crate) async_channel::Sender<Answer>);

impl Reply {
    pub fn send(self, answer: Answer) {
        let _ = self.0.try_send(answer);
    }
}

/// What the dialog side is told.
pub enum Event {
    Ask {
        handle: OwnedObjectPath,
        ask: Box<Ask>,
        reply: Reply,
    },
    /// The frontend closed the call at `handle`, or its session: take the window down without answering.
    Close { handle: OwnedObjectPath },
}

/// What the server is asked: route `trigger` to the shortcut known as `key`, or stop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Keys {
    Bind { key: u32, trigger: Trigger },
    Unbind { key: u32 },
}

/// A shortcut that has its key.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Bound {
    id: String,
    description: String,
    trigger: Trigger,
    /// The number the server knows it by.
    key: u32,
}

struct Entry {
    app_id: String,
    bound: Vec<Bound>,
    /// Ends a `BindShortcuts` waiting on the dialog when the session closes.
    closed: async_channel::Receiver<()>,
}

/// The sessions and their shortcuts, and the keys the system keeps.
#[derive(Default)]
pub struct Registry {
    sessions: HashMap<OwnedObjectPath, Entry>,
    reserved: HashSet<Trigger>,
    next_key: u32,
}

/// A shortcut as the bus carries it: its id and `description` and `trigger_description`.
pub type Shortcut = (String, HashMap<String, OwnedValue>);

fn shortcut(bound: &Bound) -> Shortcut {
    let mut properties = HashMap::new();
    for (key, value) in [
        ("description", bound.description.clone()),
        ("trigger_description", bound.trigger.describe()),
    ] {
        if let Ok(value) = OwnedValue::try_from(Value::from(value)) {
            properties.insert(key.to_owned(), value);
        }
    }
    (bound.id.clone(), properties)
}

impl Registry {
    /// The keys the system keeps; a shortcut bound on one of them loses it (the server would never route it). The
    /// sessions whose shortcuts changed, each with what it keeps, and what the server is asked.
    pub fn set_reserved(
        &mut self,
        reserved: impl IntoIterator<Item = Trigger>,
    ) -> (Vec<(OwnedObjectPath, Vec<Shortcut>)>, Vec<Keys>) {
        self.reserved = reserved.into_iter().collect();
        let (mut changed, mut keys) = (Vec::new(), Vec::new());
        for (path, entry) in &mut self.sessions {
            let before = entry.bound.len();
            entry.bound.retain(|b| {
                let keep = !self.reserved.contains(&b.trigger);
                if !keep {
                    keys.push(Keys::Unbind { key: b.key });
                }
                keep
            });
            if entry.bound.len() != before {
                changed.push((path.clone(), entry.bound.iter().map(shortcut).collect()));
            }
        }
        (changed, keys)
    }

    /// The shortcuts `asked` for `session` with the key each gets under the clash rule (see the module).
    pub fn offer(
        &self,
        session: &OwnedObjectPath,
        asked: Vec<(String, String, Option<String>)>,
    ) -> Vec<Offered> {
        let mut named: HashSet<Trigger> = HashSet::new();
        asked
            .into_iter()
            .map(|(id, description, preferred)| {
                let trigger = preferred.as_deref().and_then(Trigger::parse);
                let refusal = match trigger {
                    None => Some(Refusal::NoKey),
                    Some(t) if !t.commands() => Some(Refusal::Typing),
                    Some(t) if self.reserved.contains(&t) => Some(Refusal::System),
                    Some(t) => match self.holder(session, t) {
                        Some(app) => Some(Refusal::Taken(app)),
                        None if !named.insert(t) => Some(Refusal::Twice),
                        None => None,
                    },
                };
                Offered {
                    id,
                    description,
                    trigger,
                    refusal,
                }
            })
            .collect()
    }

    /// The app of another session that holds `trigger`.
    fn holder(&self, session: &OwnedObjectPath, trigger: Trigger) -> Option<String> {
        self.sessions
            .iter()
            .filter(|(path, _)| *path != session)
            .find(|(_, e)| e.bound.iter().any(|b| b.trigger == trigger))
            .map(|(_, e)| e.app_id.clone())
    }

    /// Give `session` the shortcuts of `offered` that get a key, in place of the ones it had. What the server is asked,
    /// and the session's shortcuts now; `None` when the session is gone.
    pub fn bind(
        &mut self,
        session: &OwnedObjectPath,
        offered: &[Offered],
    ) -> Option<(Vec<Keys>, Vec<Shortcut>)> {
        // the clash rule is applied again: a key another session took while the dialog was up stays its
        let offered = self.offer(
            session,
            offered
                .iter()
                .filter(|o| o.refusal.is_none())
                .map(|o| {
                    (
                        o.id.clone(),
                        o.description.clone(),
                        o.trigger.map(|t| format!("{}", Raw(t))),
                    )
                })
                .collect(),
        );
        let entry = self.sessions.get_mut(session)?;
        let mut keys: Vec<Keys> = entry
            .bound
            .drain(..)
            .map(|b| Keys::Unbind { key: b.key })
            .collect();
        for o in offered {
            let (None, Some(trigger)) = (&o.refusal, o.trigger) else {
                continue;
            };
            let key = self.next_key;
            self.next_key = self.next_key.wrapping_add(1);
            keys.push(Keys::Bind { key, trigger });
            entry.bound.push(Bound {
                id: o.id,
                description: o.description,
                trigger,
                key,
            });
        }
        Some((keys, entry.bound.iter().map(shortcut).collect()))
    }

    /// The shortcuts of `session`; `None` when it is not a session.
    pub fn list(&self, session: &OwnedObjectPath) -> Option<Vec<Shortcut>> {
        self.sessions
            .get(session)
            .map(|e| e.bound.iter().map(shortcut).collect())
    }

    /// End `session`: what the server is asked to give back.
    pub fn close(&mut self, session: &OwnedObjectPath) -> Vec<Keys> {
        self.sessions
            .remove(session)
            .map(|e| {
                e.bound
                    .iter()
                    .map(|b| Keys::Unbind { key: b.key })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The session and the shortcut id the server's `key` stands for.
    pub fn fired(&self, key: u32) -> Option<(OwnedObjectPath, String)> {
        self.sessions.iter().find_map(|(path, e)| {
            e.bound
                .iter()
                .find(|b| b.key == key)
                .map(|b| (path.clone(), b.id.clone()))
        })
    }
}

/// A trigger written back in the specification's form, which [`Trigger::parse`] reads as itself.
struct Raw(Trigger);

impl std::fmt::Display for Raw {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (bit, name) in [
            (modifier::CTRL, "CTRL"),
            (modifier::ALT, "ALT"),
            (modifier::SHIFT, "SHIFT"),
            (modifier::LOGO, "LOGO"),
        ] {
            if self.0.modifiers & bit != 0 {
                write!(f, "{name}+")?;
            }
        }
        write!(
            f,
            "{}",
            xkb::keysym_get_name(xkb::Keysym::new(self.0.keysym))
        )
    }
}

fn lock(registry: &Mutex<Registry>) -> std::sync::MutexGuard<'_, Registry> {
    registry.lock().unwrap_or_else(|e| e.into_inner())
}

type KeysOut = Arc<dyn Fn(Keys) + Send + Sync>;

/// What the server's side of the process reaches the sessions through.
#[derive(Clone)]
pub struct Handle {
    registry: Arc<Mutex<Registry>>,
    keys: KeysOut,
}

impl Handle {
    /// The server's shortcut `key` went down (`pressed`) or up at `time` (milliseconds): `Activated` or `Deactivated`
    /// for its session. A key no session holds any more sends nothing.
    pub fn fired(&self, bus: &zbus::blocking::Connection, key: u32, pressed: bool, time: u32) {
        let Some((session, id)) = lock(&self.registry).fired(key) else {
            return;
        };
        let signal = if pressed { "Activated" } else { "Deactivated" };
        let options: HashMap<&str, Value<'_>> = HashMap::new();
        if let Err(e) = bus.emit_signal(
            None::<&str>,
            PORTAL_PATH,
            INTERFACE,
            signal,
            &(session, id, u64::from(time), options),
        ) {
            eprintln!("cupertino-portal: {signal} could not be sent: {e}");
        }
    }

    /// The keys the system keeps changed: a shortcut on one of them loses it, and its session is told with
    /// `ShortcutsChanged`.
    pub fn reserved(&self, bus: &zbus::blocking::Connection, reserved: Vec<Trigger>) {
        let (changed, keys) = lock(&self.registry).set_reserved(reserved);
        for k in keys {
            (self.keys)(k);
        }
        for (session, shortcuts) in changed {
            if let Err(e) = bus.emit_signal(
                None::<&str>,
                PORTAL_PATH,
                INTERFACE,
                "ShortcutsChanged",
                &(session, shortcuts),
            ) {
                eprintln!("cupertino-portal: ShortcutsChanged could not be sent: {e}");
            }
        }
    }
}

/// End the session at `path`: its keys go back to the server, and its object leaves the bus after this call.
fn end(
    connection: &Connection,
    registry: &Mutex<Registry>,
    keys: &KeysOut,
    path: &OwnedObjectPath,
) {
    for k in lock(registry).close(path) {
        keys(k);
    }
    let (connection, path) = (connection.clone(), path.clone());
    connection
        .executor()
        .clone()
        .spawn(
            async move {
                let _ = connection.object_server().remove::<Session, _>(&path).await;
            },
            "cupertino-portal-shortcuts-close",
        )
        .detach();
}

type Options = HashMap<String, OwnedValue>;

/// The object on the bus.
pub struct GlobalShortcuts {
    registry: Arc<Mutex<Registry>>,
    keys: KeysOut,
    events: Box<dyn Fn(Event) + Send + Sync>,
    applications: Vec<PathBuf>,
    /// `global-shortcuts.conf`; `None` remembers nothing.
    answers: Option<PathBuf>,
}

impl GlobalShortcuts {
    /// `events` reaches the dialog side and `keys` the server's side. Nothing is remembered until
    /// [`GlobalShortcuts::with_answers`] names a file, and apps are shown by their ids until
    /// [`GlobalShortcuts::with_applications`] names where their desktop entries are.
    pub fn new(
        events: impl Fn(Event) + Send + Sync + 'static,
        keys: impl Fn(Keys) + Send + Sync + 'static,
    ) -> Self {
        Self {
            registry: Arc::default(),
            keys: Arc::new(keys),
            events: Box::new(events),
            applications: Vec::new(),
            answers: None,
        }
    }

    pub fn with_applications(mut self, applications: Vec<PathBuf>) -> Self {
        self.applications = applications;
        self
    }

    pub fn with_answers(mut self, file: Option<PathBuf>) -> Self {
        self.answers = file;
        self
    }

    /// What the server's side reaches the sessions through.
    pub fn handle(&self) -> Handle {
        Handle {
            registry: Arc::clone(&self.registry),
            keys: Arc::clone(&self.keys),
        }
    }

    /// Serve the interface at [`PORTAL_PATH`]. When the frontend leaves the bus every session ends, so no key stays
    /// taken for an app nobody can close the session of.
    pub fn serve(self, connection: &zbus::blocking::Connection) -> zbus::Result<()> {
        let connection = connection.inner().clone();
        let (registry, keys) = (Arc::clone(&self.registry), Arc::clone(&self.keys));
        zbus::block_on(async {
            let mut owners = zbus::fdo::DBusProxy::new(&connection)
                .await?
                .receive_name_owner_changed_with_args(&[(0, crate::request::FRONTEND)])
                .await?;
            connection.object_server().at(PORTAL_PATH, self).await?;
            let watcher = connection.clone();
            connection
                .executor()
                .spawn(
                    async move {
                        use futures_lite::StreamExt;
                        while let Some(change) = owners.next().await {
                            if change.args().is_ok_and(|a| a.old_owner().is_some()) {
                                let open: Vec<OwnedObjectPath> =
                                    lock(&registry).sessions.keys().cloned().collect();
                                for path in open {
                                    end(&watcher, &registry, &keys, &path);
                                }
                            }
                        }
                    },
                    "cupertino-portal-shortcuts-frontend",
                )
                .detach();
            Ok(())
        })
    }

    fn remembered(&self, app_id: &str) -> Option<Decision> {
        let path = self.answers.as_ref()?;
        match GlobalShortcutsConf::load(path) {
            Ok((conf, _)) => conf.decision(app_id),
            Err(e) => {
                eprintln!("cupertino-portal: {} cannot be read: {e}", path.display());
                None
            }
        }
    }

    fn remember(&self, app_id: &str, decision: Decision) {
        let Some(path) = &self.answers else { return };
        if !is_rememberable(app_id) {
            return;
        }
        if let Err(e) = GlobalShortcutsConf::update(path, |c| c.set(app_id, decision)) {
            eprintln!("cupertino-portal: the answer for {app_id} was not remembered: {e}");
        }
    }

    /// Bind what `offered` allows for `session`; the answer the call returns.
    fn bind(
        &self,
        session: &OwnedObjectPath,
        offered: &[Offered],
    ) -> (u32, HashMap<String, OwnedValue>) {
        // the server is asked under the lock, so the requests of two calls never interleave
        let mut registry = lock(&self.registry);
        let Some((keys, shortcuts)) = registry.bind(session, offered) else {
            return (Response::Other as u32, HashMap::new());
        };
        for k in keys {
            (self.keys)(k);
        }
        (Response::Success as u32, results(shortcuts))
    }
}

fn results(shortcuts: Vec<Shortcut>) -> HashMap<String, OwnedValue> {
    let mut results = HashMap::new();
    if let Ok(value) = OwnedValue::try_from(Value::from(shortcuts)) {
        results.insert("shortcuts".to_owned(), value);
    }
    results
}

fn invalid(why: impl Into<String>) -> PortalError {
    PortalError::InvalidArgs(why.into())
}

fn text(what: &str, value: &str) -> Result<(), PortalError> {
    if value.len() > MAX_TEXT {
        return Err(invalid(format!("{what} is longer than {MAX_TEXT} bytes")));
    }
    Ok(())
}

/// The string property `key` of a shortcut, if it has one.
fn property(properties: &Options, key: &str) -> Result<Option<String>, PortalError> {
    let Some(value) = properties.get(key) else {
        return Ok(None);
    };
    let value = value
        .try_clone()
        .ok()
        .and_then(|v| String::try_from(v).ok())
        .ok_or_else(|| invalid(format!("{key} is not a string")))?;
    text(key, &value)?;
    Ok(Some(value))
}

#[interface(name = "org.freedesktop.impl.portal.GlobalShortcuts")]
impl GlobalShortcuts {
    async fn create_session(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
        _handle: OwnedObjectPath,
        session_handle: OwnedObjectPath,
        app_id: String,
        _options: Options,
    ) -> Result<(u32, HashMap<String, OwnedValue>), PortalError> {
        let app = app_id.clone();
        let result = async {
            from_frontend(&header, connection).await?;
            text("app_id", &app_id)?;
            let (closed, closed_receiver) = async_channel::bounded(1);
            {
                let mut registry = lock(&self.registry);
                if registry.sessions.contains_key(&session_handle) {
                    return Err(invalid(format!("{session_handle} is already a session")));
                }
                let mine = registry
                    .sessions
                    .values()
                    .filter(|e| e.app_id == app_id)
                    .count();
                if registry.sessions.len() >= MAX_SESSIONS || mine >= MAX_SESSIONS_PER_APP {
                    return Err(PortalError::LimitsExceeded(
                        "too many shortcut sessions are open".to_owned(),
                    ));
                }
                registry.sessions.insert(
                    session_handle.clone(),
                    Entry {
                        app_id,
                        bound: Vec::new(),
                        closed: closed_receiver,
                    },
                );
            }
            let (registry, keys, path) = (
                Arc::clone(&self.registry),
                Arc::clone(&self.keys),
                session_handle.clone(),
            );
            let session = Session::new(closed, move |connection| {
                end(connection, &registry, &keys, &path)
            });
            let added = connection
                .object_server()
                .at(&session_handle, session)
                .await
                .unwrap_or(false);
            if !added {
                lock(&self.registry).sessions.remove(&session_handle);
                return Err(invalid(format!("{session_handle} is already in use")));
            }
            Ok((Response::Success as u32, HashMap::new()))
        }
        .await;
        answered(INTERFACE, "CreateSession", Some(&app), result)
    }

    // the D-Bus method's arguments, as the interface names them
    #[allow(clippy::too_many_arguments)]
    async fn bind_shortcuts(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
        handle: OwnedObjectPath,
        session_handle: OwnedObjectPath,
        shortcuts: Vec<(String, Options)>,
        parent_window: String,
        _options: Options,
    ) -> Result<(u32, HashMap<String, OwnedValue>), PortalError> {
        let result = async {
            from_frontend(&header, connection).await?;
            text("parent_window", &parent_window)?;
            if shortcuts.len() > MAX_SHORTCUTS {
                return Err(PortalError::LimitsExceeded(format!(
                    "at most {MAX_SHORTCUTS} shortcuts are bound at once"
                )));
            }
            let mut asked = Vec::with_capacity(shortcuts.len());
            let mut ids = HashSet::new();
            for (id, properties) in shortcuts {
                text("a shortcut id", &id)?;
                if id.is_empty() || !ids.insert(id.clone()) {
                    return Err(invalid(format!("shortcut id {id:?} is empty or twice")));
                }
                let description = property(&properties, "description")?.unwrap_or_default();
                let preferred = property(&properties, "preferred_trigger")?;
                asked.push((id, crate::file_chooser::plain(&description), preferred));
            }
            let (app_id, offered, closed) = {
                let registry = lock(&self.registry);
                let entry = registry
                    .sessions
                    .get(&session_handle)
                    .ok_or_else(|| invalid(format!("{session_handle} is no session")))?;
                (
                    entry.app_id.clone(),
                    registry.offer(&session_handle, asked),
                    entry.closed.clone(),
                )
            };
            // nothing to allow: no window
            if offered.iter().all(|o| o.refusal.is_some()) {
                return Ok(self.bind(&session_handle, &offered));
            }
            match self.remembered(&app_id) {
                Some(Decision::Allow) => return Ok(self.bind(&session_handle, &offered)),
                Some(Decision::Deny) => return Ok((Response::Cancelled as u32, HashMap::new())),
                None => {}
            }
            let request = Exported::at(connection, handle.as_ref()).await?;
            let identity = identify(&self.applications, &app_id);
            let holders = offered
                .iter()
                .filter_map(|o| match &o.refusal {
                    Some(Refusal::Taken(app)) => Some(app.clone()),
                    _ => None,
                })
                .map(|app| {
                    let name = identify(&self.applications, &app)
                        .name
                        .unwrap_or_else(|| app.clone());
                    (app, name)
                })
                .collect();
            let (reply, answer) = async_channel::bounded(1);
            (self.events)(Event::Ask {
                handle: handle.clone(),
                ask: Box::new(Ask {
                    app_name: identity.name,
                    app_icon: identity.icon,
                    app_id: app_id.clone(),
                    parent_window,
                    shortcuts: offered.clone(),
                    holders,
                }),
                reply: Reply(reply),
            });
            let answer =
                futures_lite::future::or(async { Some(answer.recv().await.ok()) }, async {
                    futures_lite::future::or(request.closed(), async {
                        let _ = closed.recv().await;
                    })
                    .await;
                    None
                })
                .await;
            let answered = match answer {
                Some(Some(Answer::Allowed)) => {
                    self.remember(&app_id, Decision::Allow);
                    self.bind(&session_handle, &offered)
                }
                Some(Some(Answer::Denied)) => {
                    self.remember(&app_id, Decision::Deny);
                    (Response::Cancelled as u32, HashMap::new())
                }
                Some(Some(Answer::Dismissed)) => (Response::Cancelled as u32, HashMap::new()),
                Some(None) => (Response::Other as u32, HashMap::new()),
                None => {
                    (self.events)(Event::Close { handle });
                    (Response::Other as u32, HashMap::new())
                }
            };
            request.remove().await;
            Ok(answered)
        }
        .await;
        answered(INTERFACE, "BindShortcuts", None, result)
    }

    async fn list_shortcuts(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
        _handle: OwnedObjectPath,
        session_handle: OwnedObjectPath,
    ) -> Result<(u32, HashMap<String, OwnedValue>), PortalError> {
        let result = async {
            from_frontend(&header, connection).await?;
            let shortcuts = lock(&self.registry)
                .list(&session_handle)
                .ok_or_else(|| invalid(format!("{session_handle} is no session")))?;
            Ok((Response::Success as u32, results(shortcuts)))
        }
        .await;
        answered(INTERFACE, "ListShortcuts", None, result)
    }

    #[zbus(signal)]
    async fn activated(
        emitter: &SignalEmitter<'_>,
        session_handle: OwnedObjectPath,
        shortcut_id: String,
        timestamp: u64,
        options: HashMap<String, OwnedValue>,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn deactivated(
        emitter: &SignalEmitter<'_>,
        session_handle: OwnedObjectPath,
        shortcut_id: String,
        timestamp: u64,
        options: HashMap<String, OwnedValue>,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn shortcuts_changed(
        emitter: &SignalEmitter<'_>,
        session_handle: OwnedObjectPath,
        shortcuts: Vec<Shortcut>,
    ) -> zbus::Result<()>;

    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        VERSION
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(modifiers: u32, keysym: u32) -> Trigger {
        Trigger { modifiers, keysym }
    }

    #[test]
    fn triggers_are_read_in_both_forms_and_letters_in_lower_case() {
        let ctrl_alt_k = t(modifier::CTRL | modifier::ALT, 0x6b);
        for text in [
            "CTRL+ALT+k",
            "ctrl+alt+K",
            "<Control><Alt>k",
            "Alt+Control+k",
            " CTRL + ALT + k ",
        ] {
            assert_eq!(Trigger::parse(text), Some(ctrl_alt_k), "{text}");
        }
        assert_eq!(
            Trigger::parse("LOGO+Return"),
            Some(t(modifier::LOGO, 0xff0d))
        );
        assert_eq!(Trigger::parse("SHIFT+F5"), Some(t(modifier::SHIFT, 0xffc2)));
        assert_eq!(Trigger::parse("CTRL++"), Some(t(modifier::CTRL, 0x2b)));
        assert_eq!(Trigger::parse("CTRL+space"), Some(t(modifier::CTRL, 0x20)));
        assert_eq!(Trigger::parse("CTRL+1"), Some(t(modifier::CTRL, 0x31)));
        // the names other programs use
        assert_eq!(
            Trigger::parse("CommandOrControl+Shift+K"),
            Some(t(modifier::CTRL | modifier::SHIFT, 0x6b))
        );
        assert_eq!(
            Trigger::parse("Option+ArrowUp"),
            Some(t(modifier::ALT, 0xff52))
        );
        assert_eq!(
            Trigger::parse("CTRL+Enter"),
            Some(t(modifier::CTRL, 0xff0d))
        );
        assert_eq!(Trigger::parse("MediaPlayPause"), Some(t(0, 0x1008_ff14)));
        for none in ["", "CTRL+", "HYPER+k", "CTRL+NoSuchKey", "<Control>"] {
            assert_eq!(Trigger::parse(none), None, "{none:?}");
        }
    }

    #[test]
    fn a_trigger_reads_as_symbols_and_writes_back_as_itself() {
        let chord = t(
            modifier::CTRL | modifier::ALT | modifier::SHIFT | modifier::LOGO,
            0x6b,
        );
        assert_eq!(chord.describe(), "⌃⌥⇧⌘K");
        assert_eq!(t(modifier::LOGO, 0xff0d).describe(), "⌘Return");
        assert_eq!(t(0, 0xffc2).describe(), "F5");
        for trigger in [chord, t(modifier::LOGO, 0xff0d), t(modifier::CTRL, 0x2b)] {
            assert_eq!(Trigger::parse(&Raw(trigger).to_string()), Some(trigger));
        }
    }

    fn path(n: u32) -> OwnedObjectPath {
        OwnedObjectPath::try_from(format!("/org/freedesktop/portal/desktop/session/1_7/s{n}"))
            .unwrap()
    }

    fn open(registry: &mut Registry, n: u32, app: &str) {
        let (_, closed) = async_channel::bounded(1);
        registry.sessions.insert(
            path(n),
            Entry {
                app_id: app.to_owned(),
                bound: Vec::new(),
                closed,
            },
        );
    }

    fn asked(id: &str, trigger: Option<&str>) -> (String, String, Option<String>) {
        (
            id.to_owned(),
            format!("{id} it"),
            trigger.map(str::to_owned),
        )
    }

    #[test]
    fn the_clash_rule_refuses_typing_the_systems_keys_taken_keys_and_doubles() {
        let mut registry = Registry::default();
        registry.set_reserved([t(modifier::LOGO, 0xff09)]);
        open(&mut registry, 1, "org.example.Meeting");
        open(&mut registry, 2, "org.example.Chat");
        let first = registry.offer(&path(1), vec![asked("talk", Some("CTRL+ALT+t"))]);
        let (keys, bound) = registry.bind(&path(1), &first).unwrap();
        assert_eq!(
            keys,
            [Keys::Bind {
                key: 0,
                trigger: t(6, 0x74)
            }]
        );
        assert_eq!(bound.len(), 1);

        let offered = registry.offer(
            &path(2),
            vec![
                asked("talk", Some("CTRL+ALT+t")),
                asked("switch", Some("LOGO+Tab")),
                asked("letter", Some("SHIFT+a")),
                asked("none", None),
                asked("bad", Some("CTRL+Nonsense")),
                asked("f13", Some("F13")),
                asked("again", Some("F13")),
                asked("play", Some("MediaPlayPause")),
                asked("up", Some("Up")),
                asked("open", Some("CTRL+SHIFT+o")),
            ],
        );
        let refusals: Vec<Option<Refusal>> = offered.iter().map(|o| o.refusal.clone()).collect();
        assert_eq!(
            refusals,
            [
                Some(Refusal::Taken("org.example.Meeting".into())),
                Some(Refusal::System),
                Some(Refusal::Typing),
                Some(Refusal::NoKey),
                Some(Refusal::NoKey),
                None,
                Some(Refusal::Twice),
                None,
                Some(Refusal::Typing),
                None,
            ]
        );
        let (_, bound) = registry.bind(&path(2), &offered).unwrap();
        let ids: Vec<&str> = bound.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(ids, ["f13", "play", "open"]);
        // a session that binds again gives its earlier keys back first, and keeps its own key
        let again = registry.offer(&path(1), vec![asked("talk", Some("CTRL+ALT+t"))]);
        assert_eq!(again[0].refusal, None);
        let (keys, _) = registry.bind(&path(1), &again).unwrap();
        assert_eq!(keys[0], Keys::Unbind { key: 0 });
        assert!(matches!(keys[1], Keys::Bind { .. }));
    }

    #[test]
    fn a_key_the_system_takes_later_is_given_back_and_the_session_told() {
        let mut registry = Registry::default();
        open(&mut registry, 1, "org.example.Meeting");
        let offered = registry.offer(
            &path(1),
            vec![
                asked("talk", Some("CTRL+ALT+t")),
                asked("mute", Some("CTRL+ALT+m")),
            ],
        );
        registry.bind(&path(1), &offered).unwrap();
        let (changed, keys) = registry.set_reserved([t(6, 0x74)]);
        assert_eq!(keys, [Keys::Unbind { key: 0 }]);
        assert_eq!(changed.len(), 1);
        assert_eq!(changed[0].1.len(), 1);
        assert_eq!(changed[0].1[0].0, "mute");
        assert_eq!(registry.fired(1), Some((path(1), "mute".to_owned())));
        assert_eq!(registry.fired(0), None);
        assert_eq!(registry.close(&path(1)), [Keys::Unbind { key: 1 }]);
        assert_eq!(registry.fired(1), None);
    }
}
