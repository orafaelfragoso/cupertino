//! The shell's protocols that `cupertino-shell-client` does not carry, on the transport's queue: the window manager and
//! the overview of each output, transactions and holes, and the list of windows on screen. What their objects receive
//! arrives in order with the transport's events, as [`Wire`] extension events, one for each group a `done` closes.

use std::collections::BTreeMap;
use std::sync::{Mutex, PoisonError};

use cupertino_kit::geometry::Rect;
use cupertino_kit::platform::Timestamp;
use cupertino_protocols::client::cupertino_keyboard_panels_v1::{
    self as panels, CupertinoKeyboardPanelsV1,
};
use cupertino_protocols::client::cupertino_shell_v1::CupertinoShellV1;
use cupertino_protocols::client::cupertino_window_manager_v1::{
    self as wm, CupertinoWindowManagerV1,
};
use cupertino_protocols::client::overview::cupertino_overview_manager_v1::CupertinoOverviewManagerV1;
use cupertino_protocols::client::overview::cupertino_overview_v1::{
    self as ov, CupertinoOverviewV1,
};
use cupertino_protocols::client::screenshot::cupertino_screenshot_v1::{
    self as shot, CupertinoScreenshotV1,
};
use cupertino_protocols::client::transaction::cupertino_hole_v1::{self as hole, CupertinoHoleV1};
use cupertino_protocols::client::transaction::cupertino_transaction_manager_v1::CupertinoTransactionManagerV1;
use cupertino_protocols::client::transaction::cupertino_transaction_v1::{
    self as tx, CupertinoTransactionV1,
};
use cupertino_wayland_client::State;
use wayland_client::protocol::wl_output::WlOutput;
use wayland_client::{Connection as WlConnection, Dispatch, QueueHandle, WEnum};

use crate::keyboard_panels::{self, Source, Update as PanelsUpdate};
use crate::outputs::OutputId;
use crate::transaction::{self, Gesture, Outcome};
use crate::{capture, overview, window_manager, ServerWindowId, WorkspaceId};

/// What the objects of this module received, one event for each group the server closed with `done`.
#[derive(Debug)]
pub(super) enum Wire {
    WindowManager(OutputId, window_manager::Update),
    Overview(OutputId, overview::Update),
    KeyboardPanels(keyboard_panels::Update),
    Transaction(transaction::Event),
    /// The window or workspace of the hole of this number is gone.
    HoleClosed(u64),
    Windows(Vec<capture::Window>),
}

/// User data of a global or an object whose events are none or handled elsewhere.
pub(super) struct Quiet;

/// User data of the window manager of an output: its windows as of the last group, and the group being sent.
pub(super) struct WindowManagerData {
    pub output: OutputId,
    feed: Mutex<WindowManagerFeed>,
}

#[derive(Default)]
struct WindowManagerFeed {
    usable_area: Option<Rect>,
    windows: BTreeMap<u32, window_manager::Window>,
    toggle_stage: bool,
}

impl WindowManagerData {
    pub(super) fn new(output: OutputId) -> Self {
        Self {
            output,
            feed: Mutex::default(),
        }
    }
}

/// User data of the overview of an output: its windows and workspaces as of the last group, and the group being sent.
pub(super) struct OverviewData {
    pub output: OutputId,
    feed: Mutex<OverviewFeed>,
}

#[derive(Default)]
struct OverviewFeed {
    area: Option<Rect>,
    windows: BTreeMap<u32, overview::Window>,
    workspaces: Vec<overview::Workspace>,
    /// The group being sent lists the workspaces anew.
    workspaces_listed: bool,
    toggle: Option<overview::Toggle>,
}

impl OverviewData {
    pub(super) fn new(output: OutputId) -> Self {
        Self {
            output,
            feed: Mutex::default(),
        }
    }
}

/// User data of the keyboard's panels: the entries of the list being sent.
#[derive(Default)]
pub(super) struct KeyboardPanelsData(Mutex<PanelsFeed>);

#[derive(Default)]
struct PanelsFeed {
    sources: Vec<Source>,
    accents: Vec<String>,
}

/// User data of a transaction part: the transaction it is a part of.
pub(super) struct TransactionData(pub transaction::Id);

/// User data of a hole: its number among the component's holes.
pub(super) struct HoleData(pub u64);

/// User data of the window list: the windows of the group being sent.
#[derive(Default)]
pub(super) struct WindowsData(Mutex<Vec<capture::Window>>);

fn rect(x: i32, y: i32, width: i32, height: i32) -> Rect {
    Rect::new(
        f64::from(x),
        f64::from(y),
        f64::from(width),
        f64::from(height),
    )
}

/// The wire's gesture of `gesture`.
pub(super) fn wire_gesture(gesture: Gesture) -> tx::Gesture {
    match gesture {
        Gesture::SwipeUp => tx::Gesture::SwipeUp,
        Gesture::SwipeDown => tx::Gesture::SwipeDown,
        Gesture::Pinch => tx::Gesture::Pinch,
        Gesture::Spread => tx::Gesture::Spread,
    }
}

fn gesture(wire: WEnum<tx::Gesture>) -> Option<Gesture> {
    Some(match wire {
        WEnum::Value(tx::Gesture::SwipeUp) => Gesture::SwipeUp,
        WEnum::Value(tx::Gesture::SwipeDown) => Gesture::SwipeDown,
        WEnum::Value(tx::Gesture::Pinch) => Gesture::Pinch,
        WEnum::Value(tx::Gesture::Spread) => Gesture::Spread,
        _ => return None,
    })
}

fn outcome(wire: WEnum<tx::Outcome>) -> Outcome {
    match wire {
        WEnum::Value(tx::Outcome::Completed) => Outcome::Completed,
        WEnum::Value(tx::Outcome::Reverted) => Outcome::Reverted,
        _ => Outcome::Cancelled,
    }
}

/// The objects with no events of their own.
macro_rules! quiet {
    ($($interface:ty),* $(,)?) => {$(
        impl Dispatch<$interface, Quiet> for State {
            fn event(
                _: &mut Self,
                _: &$interface,
                _: <$interface as wayland_client::Proxy>::Event,
                _: &Quiet,
                _: &WlConnection,
                _: &QueueHandle<Self>,
            ) {
            }
        }
    )*};
}

quiet!(
    CupertinoShellV1,
    CupertinoOverviewManagerV1,
    CupertinoTransactionManagerV1
);

impl Dispatch<CupertinoWindowManagerV1, WindowManagerData> for State {
    fn event(
        state: &mut Self,
        _: &CupertinoWindowManagerV1,
        event: wm::Event,
        data: &WindowManagerData,
        _: &WlConnection,
        _: &QueueHandle<Self>,
    ) {
        let mut feed = data.feed.lock().unwrap_or_else(PoisonError::into_inner);
        match event {
            wm::Event::UsableArea {
                x,
                y,
                width,
                height,
            } => feed.usable_area = Some(rect(x, y, width, height)),
            wm::Event::Window {
                id,
                app_id,
                x,
                y,
                width,
                height,
                state: flags,
            } => {
                let active = match flags {
                    WEnum::Value(flags) => flags.contains(wm::WindowState::Active),
                    WEnum::Unknown(bits) => bits & 1 != 0,
                };
                feed.windows.insert(
                    id,
                    window_manager::Window {
                        id: ServerWindowId(id),
                        app_id,
                        frame: rect(x, y, width, height),
                        active,
                    },
                );
            }
            wm::Event::WindowClosed { id } => {
                feed.windows.remove(&id);
            }
            wm::Event::ToggleStage => feed.toggle_stage = true,
            wm::Event::Done => {
                let update = window_manager::Update {
                    usable_area: feed.usable_area,
                    windows: feed.windows.values().cloned().collect(),
                    toggle_stage: std::mem::take(&mut feed.toggle_stage),
                };
                drop(feed);
                state.emit(Wire::WindowManager(data.output, update));
            }
            // the generated enum is non-exhaustive: a message a later version adds is not read here
            _ => {}
        }
    }
}

impl Dispatch<CupertinoOverviewV1, OverviewData> for State {
    fn event(
        state: &mut Self,
        _: &CupertinoOverviewV1,
        event: ov::Event,
        data: &OverviewData,
        _: &WlConnection,
        _: &QueueHandle<Self>,
    ) {
        let mut feed = data.feed.lock().unwrap_or_else(PoisonError::into_inner);
        match event {
            ov::Event::Area {
                x,
                y,
                width,
                height,
            } => feed.area = Some(rect(x, y, width, height)),
            ov::Event::Window {
                id,
                app_id,
                title,
                x,
                y,
                width,
                height,
                state: flags,
            } => {
                let bits = match flags {
                    WEnum::Value(flags) => flags.bits(),
                    WEnum::Unknown(bits) => bits,
                };
                feed.windows.insert(
                    id,
                    overview::Window {
                        id: ServerWindowId(id),
                        app_id,
                        title,
                        frame: rect(x, y, width, height),
                        minimized: bits & ov::WindowState::Minimized.bits() != 0,
                        active: bits & ov::WindowState::Active.bits() != 0,
                    },
                );
            }
            ov::Event::WindowClosed { id } => {
                feed.windows.remove(&id);
            }
            ov::Event::Workspace { id, state: flags } => {
                let bits = match flags {
                    WEnum::Value(flags) => flags.bits(),
                    WEnum::Unknown(bits) => bits,
                };
                if !std::mem::replace(&mut feed.workspaces_listed, true) {
                    feed.workspaces.clear();
                }
                feed.workspaces.push(overview::Workspace {
                    id: WorkspaceId(id),
                    current: bits & ov::WorkspaceState::Current.bits() != 0,
                    full_screen: bits & ov::WorkspaceState::FullScreen.bits() != 0,
                });
            }
            ov::Event::Toggle { mode, app_id } => {
                let mode = match mode {
                    WEnum::Value(ov::Mode::App) => overview::Mode::App,
                    _ => overview::Mode::All,
                };
                feed.toggle = Some(overview::Toggle { mode, app_id });
            }
            ov::Event::Done => {
                feed.workspaces_listed = false;
                let update = overview::Update {
                    area: feed.area,
                    windows: feed.windows.values().cloned().collect(),
                    workspaces: feed.workspaces.clone(),
                    toggle: feed.toggle.take(),
                };
                drop(feed);
                state.emit(Wire::Overview(data.output, update));
            }
            _ => {}
        }
    }
}

impl Dispatch<CupertinoKeyboardPanelsV1, KeyboardPanelsData> for State {
    fn event(
        state: &mut Self,
        _: &CupertinoKeyboardPanelsV1,
        event: panels::Event,
        data: &KeyboardPanelsData,
        _: &WlConnection,
        _: &QueueHandle<Self>,
    ) {
        let mut feed = data.0.lock().unwrap_or_else(PoisonError::into_inner);
        // the program's id of an output the server named, `None` for one this connection has not bound
        let output = |state: &Self, wl: Option<WlOutput>| {
            wl.and_then(|wl| state.output_id(&wl))
                .map(|id| OutputId::from_raw(id.registry_name()))
        };
        let update = match event {
            panels::Event::SourceEntry { id, name } => {
                feed.sources.push(Source { id, name });
                return;
            }
            panels::Event::SourcesChanged {
                output: wl,
                selected,
            } => PanelsUpdate::Sources {
                output: output(state, wl),
                sources: std::mem::take(&mut feed.sources),
                selected: selected as usize,
            },
            panels::Event::SourcesClosed => {
                feed.sources.clear();
                PanelsUpdate::SourcesClosed
            }
            panels::Event::AccentEntry { text } => {
                feed.accents.push(text);
                return;
            }
            panels::Event::AccentsChanged {
                serial,
                output: wl,
                x,
                y,
                width,
                height,
                top,
                focused,
            } => PanelsUpdate::Accents {
                serial,
                output: output(state, wl),
                anchor: rect(x, y, width, height),
                top: f64::from(top.max(0)),
                candidates: std::mem::take(&mut feed.accents),
                focused: usize::try_from(focused).ok(),
            },
            panels::Event::AccentsClosed { serial } => {
                feed.accents.clear();
                PanelsUpdate::AccentsClosed { serial }
            }
            _ => return,
        };
        drop(feed);
        state.emit(Wire::KeyboardPanels(update));
    }
}

impl Dispatch<CupertinoTransactionV1, TransactionData> for State {
    fn event(
        state: &mut Self,
        _: &CupertinoTransactionV1,
        event: tx::Event,
        data: &TransactionData,
        _: &WlConnection,
        _: &QueueHandle<Self>,
    ) {
        let id = data.0;
        let event = match event {
            tx::Event::Started {
                tv_sec_hi,
                tv_sec_lo,
                tv_nsec,
            } => {
                let seconds = (u64::from(tv_sec_hi) << 32) | u64::from(tv_sec_lo);
                let at = Timestamp(
                    seconds
                        .saturating_mul(1_000_000_000)
                        .saturating_add(u64::from(tv_nsec)),
                );
                transaction::Event::Started { id, at }
            }
            tx::Event::GestureBegan { gesture: wire } => match gesture(wire) {
                Some(gesture) => transaction::Event::GestureBegan { id, gesture },
                None => return,
            },
            tx::Event::GestureEnded { outcome: wire } => transaction::Event::GestureEnded {
                id,
                outcome: outcome(wire),
            },
            tx::Event::Finished { outcome: wire } => transaction::Event::Finished {
                id,
                outcome: outcome(wire),
            },
            _ => return,
        };
        state.emit(Wire::Transaction(event));
    }
}

impl Dispatch<CupertinoHoleV1, HoleData> for State {
    fn event(
        state: &mut Self,
        _: &CupertinoHoleV1,
        event: hole::Event,
        data: &HoleData,
        _: &WlConnection,
        _: &QueueHandle<Self>,
    ) {
        if let hole::Event::SourceClosed = event {
            state.emit(Wire::HoleClosed(data.0));
        }
    }
}

impl Dispatch<CupertinoScreenshotV1, WindowsData> for State {
    fn event(
        state: &mut Self,
        _: &CupertinoScreenshotV1,
        event: shot::Event,
        data: &WindowsData,
        _: &WlConnection,
        _: &QueueHandle<Self>,
    ) {
        let mut windows = data.0.lock().unwrap_or_else(PoisonError::into_inner);
        match event {
            shot::Event::Window {
                identifier,
                x,
                y,
                width,
                height,
            } => windows.push(capture::Window {
                identifier,
                frame: rect(x, y, width, height),
            }),
            shot::Event::Done => {
                let list = std::mem::take(&mut *windows);
                drop(windows);
                state.emit(Wire::Windows(list));
            }
            _ => {}
        }
    }
}
