//! The loop a component or a program of its own runs in (`shell-host.md#cupertino-kit-shell`): Kit's UI loop on the
//! component's own Wayland connection, which the host opened and the component's `cupertino_component_v1` lives on, or
//! on the connection the server gave the program. Its window system gives each window the role the program's
//! [`Program::surface`] names ([`roles::ShellRoles`]), carries out the requests the program queued before it waits,
//! and hands what the shell says to [`Program::event`].

mod capture;
mod globals;
mod protocols;
mod requests;
mod roles;

use std::any::Any;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::future::poll_fn;
use std::marker::PhantomData;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::task::Poll;
use std::time::Instant;

use cupertino_keyboard::modifiers::Profile;
use cupertino_kit::core::{Application, Command, RendererClass, Window, WindowKey};
use cupertino_ui_wayland::WaylandWindowSystem;
use cupertino_wayland_client::Connection;

use crate::bus::BusHub;
use crate::context::{self, Inbox};
use crate::{hosting, Component, Error, Event, Program, Surface};

pub(crate) use globals::check as check_globals;

/// What a host hands the loop of the component it starts.
pub(crate) struct Hosting {
    /// Set when the host asks the component to stop; the loop ends on its next turn.
    pub stop: Arc<AtomicBool>,
    /// When the loop left its wait, while it is out of it (`hosting::Running::busy_for`).
    pub busy: Arc<Mutex<Option<Instant>>>,
}

/// How a component's loop ended.
pub(crate) enum Ran {
    /// The host asked it to stop.
    Stopped,
    /// It ended without being asked (the component's `Command::Exit`).
    Exited,
}

/// What the program's wrapper and its window system's roles share on the UI thread.
#[derive(Default)]
pub(crate) struct Shared {
    /// How each window of the last view reaches the screen, by its key ([`Program::surface`]).
    pub surfaces: HashMap<WindowKey, Surface>,
    /// The persistent windows of the first view that have not presented a frame yet; `None` before the first view.
    /// `ready` goes out once it is empty.
    pub awaiting: Option<HashSet<WindowKey>>,
}

/// Whether the loop keeps running with no window open.
trait Residence {
    const RESIDENT: bool;
}

/// A component: it runs until its host stops it, with or without windows.
struct AsComponent;

impl Residence for AsComponent {
    const RESIDENT: bool = true;
}

/// A program of its own: it runs as its own [`Application::RESIDENT`] says.
struct AsProgram;

impl Residence for AsProgram {
    const RESIDENT: bool = false;
}

/// `P` as Kit runs it: its model, view and messages, with the surfaces of each view recorded for the roles and a task
/// that hands the shell's events to [`Program::event`].
struct Hosted<P, R>(PhantomData<(P, R)>);

/// The model of [`Hosted`]: the program's own, and what its loop shares.
struct HostedModel<P: Program> {
    /// The program's model, also held by the thread's [`STATE`], so a panic that unwinds the loop does not drop it.
    inner: Rc<RefCell<P::Model>>,
    shared: Rc<RefCell<Shared>>,
    feed: Feed,
}

/// Where the shell's events for the program wait: its inbox and, in a host, its queue on the host bus.
#[derive(Clone)]
struct Feed {
    inbox: Arc<Inbox>,
    bus: Option<(Arc<BusHub>, u64)>,
    /// A task waits for the next event that becomes a message.
    armed: Arc<AtomicBool>,
    /// Where the program was asked to panic (development builds).
    #[cfg(debug_assertions)]
    crash: Option<Arc<crate::crash::Crash>>,
}

impl Feed {
    /// The feed of the calling thread's program.
    fn of_thread() -> Feed {
        let (inbox, bus) = context::with_ctx(|ctx| {
            (
                Arc::clone(&ctx.inbox),
                ctx.bus.clone().zip(ctx.bus_subscriber_id),
            )
        })
        .unwrap_or_default();
        Feed {
            inbox,
            bus,
            armed: Arc::new(AtomicBool::new(false)),
            #[cfg(debug_assertions)]
            crash: context::with_ctx(|ctx| Arc::clone(&ctx.crash)),
        }
    }

    /// Adds the task that waits for the next event `P` turns into a message, unless one waits already.
    fn arm<P: Program>(&self, commands: &mut Vec<Command<P::Message>>) {
        if self.armed.swap(true, Ordering::SeqCst) {
            return;
        }
        let feed = self.clone();
        commands.push(Command::Perform(Box::pin(async move {
            loop {
                let event = poll_fn(|cx| {
                    #[cfg(debug_assertions)]
                    if feed
                        .crash
                        .as_ref()
                        .is_some_and(|crash| crash.take_dispatch())
                    {
                        panic!("the crash asked for as the component's loop dispatches");
                    }
                    if let Some((hub, id)) = &feed.bus {
                        if let Poll::Ready(published) = hub.poll_drain(*id, cx) {
                            for (from, message) in published {
                                feed.inbox.push(Event::Bus { from, message });
                            }
                        }
                    }
                    feed.inbox.poll_next(cx)
                })
                .await;
                if let Some(message) = P::event(event) {
                    feed.armed.store(false, Ordering::SeqCst);
                    return message;
                }
            }
        })));
    }
}

impl<P: Program> HostedModel<P> {
    /// The program's model from its [`Application::init`], on the calling thread's context.
    fn start(shared: Rc<RefCell<Shared>>) -> (Self, Vec<Command<P::Message>>) {
        let (inner, mut commands) = P::init();
        let inner = Rc::new(RefCell::new(inner));
        STATE.with(|state| *state.borrow_mut() = Some(Rc::clone(&inner) as Rc<dyn Any>));
        let feed = Feed::of_thread();
        feed.arm::<P>(&mut commands);
        (
            Self {
                inner,
                shared,
                feed,
            },
            commands,
        )
    }
}

impl<P: Program, R: Residence + 'static> Application for Hosted<P, R> {
    type Model = HostedModel<P>;
    type Message = P::Message;
    const RESIDENT: bool = R::RESIDENT || P::RESIDENT;
    const RENDERER: RendererClass = P::RENDERER;

    fn init() -> (Self::Model, Vec<Command<Self::Message>>) {
        HostedModel::start(Rc::default())
    }

    fn update(model: &mut Self::Model, message: Self::Message) -> Vec<Command<Self::Message>> {
        let mut commands = P::update(&mut model.inner.borrow_mut(), message);
        model.feed.arm::<P>(&mut commands);
        commands
    }

    fn view(model: &Self::Model) -> Vec<Window<Self::Message>> {
        let inner = model.inner.borrow();
        let windows = P::view(&inner);
        #[cfg(debug_assertions)]
        let windows = crate::crash::wrap(windows);
        let surfaces: HashMap<WindowKey, Surface> = windows
            .iter()
            .map(|w| (w.key.clone(), P::surface(&inner, &w.key)))
            .collect();
        let mut shared = model.shared.borrow_mut();
        if shared.awaiting.is_none() {
            shared.awaiting = Some(
                surfaces
                    .iter()
                    .filter(|(_, s)| s.persistent.is_some())
                    .map(|(key, _)| key.clone())
                    .collect(),
            );
        }
        shared.surfaces = surfaces;
        windows
    }
}

thread_local! {
    /// The model of the program the thread runs, outside the frames a panic of its loop unwinds
    /// (`shell-host.md#the-host-programs-side`): its host drops it with [`take_state`] once the loop ended, inside
    /// `catch_unwind`, so a panic in its teardown is caught and leaks what was left of it.
    static STATE: RefCell<Option<Rc<dyn Any>>> = const { RefCell::new(None) };
}

/// The model of the program the calling thread ran, for its host to drop.
pub(crate) fn take_state() -> Option<Rc<dyn Any>> {
    STATE.with(|state| state.borrow_mut().take())
}

/// Runs component `C` on `connection` until its host stops it or its loop ends; the calling thread is the component's,
/// with its context installed.
pub(crate) fn run_component<C: Component>(
    hosting: &Hosting,
    connection: hosting::Connection,
) -> Result<Ran, Error> {
    mark_busy(&hosting.busy);
    let wayland = connection
        .wayland()
        .cloned()
        .ok_or_else(|| Error::Protocol("the component's connection is not open".into()))?;
    let mut conn = Connection::from_wayland(wayland)
        .map_err(|e| Error::Protocol(format!("component connection failed: {e}")))?;
    let shared = Rc::new(RefCell::new(Shared::default()));
    let roles = roles::ShellRoles::bind(
        &mut conn,
        roles::Binding {
            component: Some(C::NAME),
            shared: Rc::clone(&shared),
            busy: Some(Arc::clone(&hosting.busy)),
        },
    );
    let system =
        WaylandWindowSystem::new_as(conn, Profile::cupertino(), C::RENDERER).with_roles(roles);
    let stop = Arc::clone(&hosting.stop);
    let ran = cupertino_kit::app::run_on_with_until::<Hosted<C, AsComponent>>(
        system,
        |_| HostedModel::start(shared),
        Box::new(move || stop.load(Ordering::SeqCst)),
    );
    connection.shutdown();
    match ran {
        Ok(()) if hosting.stop.load(Ordering::SeqCst) => Ok(Ran::Stopped),
        Ok(()) => Ok(Ran::Exited),
        Err(e) => Err(Error::Protocol(format!("the component's loop failed: {e}"))),
    }
}

/// Runs program `P` on the connection the server gave the process until it exits; `lock_screen` keeps its
/// accessibility tree heard while the session is locked.
pub(crate) fn run_program<P: Program>(lock_screen: bool) -> Result<(), Error> {
    let mut conn = Connection::connect()
        .map_err(|e| Error::Protocol(format!("cannot connect to the server: {e}")))?;
    let shared = Rc::new(RefCell::new(Shared::default()));
    let roles = roles::ShellRoles::bind(
        &mut conn,
        roles::Binding {
            component: None,
            shared: Rc::clone(&shared),
            busy: None,
        },
    );
    let system =
        WaylandWindowSystem::new_as(conn, Profile::cupertino(), P::RENDERER).with_roles(roles);
    let start = |_: &_| HostedModel::start(shared);
    let ran = if lock_screen {
        cupertino_kit::app::run_lock_screen_on_with::<Hosted<P, AsProgram>>(system, start)
    } else {
        cupertino_kit::app::run_on_with::<Hosted<P, AsProgram>>(system, start)
    };
    ran.map_err(|e| Error::Protocol(format!("the program's loop failed: {e}")))
}

/// The loop is out of its wait from now on.
fn mark_busy(busy: &Mutex<Option<Instant>>) {
    *busy.lock().unwrap_or_else(PoisonError::into_inner) = Some(Instant::now());
}
