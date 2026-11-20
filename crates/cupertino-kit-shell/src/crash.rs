//! Development builds only: a component made to panic where it is asked (`cupertino shell crash NAME`), to try how
//! the host contains it. The component's session bus connection (`bus::session`) serves [`INTERFACE`] at [`PATH`];
//! its `Crash(at)` makes the component panic
//!
//! - `dispatch`: as its loop next hands it the shell's events, on its UI thread;
//! - `paint`: as it next paints one of its windows;
//! - `helper`: on a helper thread started at once (`host::thread`), which fails the component.
//!
//! A release build has none of this.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use cupertino_kit::controls::Canvas;
use cupertino_kit::core::{Element, Window};

use crate::context::{self, Inbox};

/// The object the component's connection serves the interface at.
const PATH: &str = "/org/cupertino/ShellComponentDebug1";
/// Where the component was asked to panic and has not yet.
#[derive(Default)]
pub(crate) struct Crash {
    dispatch: AtomicBool,
    paint: AtomicBool,
}

impl Crash {
    /// Whether the loop panics now, as it dispatches; asked once.
    pub(crate) fn take_dispatch(&self) -> bool {
        self.dispatch.swap(false, Ordering::SeqCst)
    }
}

/// The crash of the calling thread's component.
fn of_thread() -> Option<Arc<Crash>> {
    context::with_ctx(|ctx| Arc::clone(&ctx.crash))
}

/// `windows` as the view gives them, each window's content under a canvas that panics as it paints while a paint
/// crash was asked for.
pub(crate) fn wrap<M: 'static>(windows: Vec<Window<M>>) -> Vec<Window<M>> {
    let Some(crash) = of_thread().filter(|crash| crash.paint.load(Ordering::SeqCst)) else {
        return windows;
    };
    windows
        .into_iter()
        .map(|mut window| {
            let crash = Arc::clone(&crash);
            let content = std::mem::replace(&mut window.content, Element::new(empty()));
            window.content = Element::new(Canvas {
                size: None,
                label: String::new(),
                paint: Box::new(move |_| {
                    if crash.paint.swap(false, Ordering::SeqCst) {
                        panic!("the crash asked for as the component paints");
                    }
                }),
                children: vec![content],
            });
            window
        })
        .collect()
}

/// A canvas that draws nothing.
fn empty<M>() -> Canvas<M> {
    Canvas {
        size: None,
        label: String::new(),
        paint: Box::new(|_| {}),
        children: Vec::new(),
    }
}

/// The interface on the component's connection.
struct Served {
    crash: Arc<Crash>,
    inbox: Arc<Inbox>,
}

#[zbus::interface(name = "org.cupertino.ShellComponentDebug1")]
impl Served {
    /// Makes the component panic `at` `dispatch`, `paint` or `helper`.
    fn crash(&self, at: &str) -> zbus::fdo::Result<()> {
        match at {
            "dispatch" => {
                self.crash.dispatch.store(true, Ordering::SeqCst);
                // the loop takes a turn, which hands it the shell's events
                self.inbox.wake();
            }
            "paint" => self.crash.paint.store(true, Ordering::SeqCst),
            "helper" => {
                // its task is never run: the helper's panic fails the component all the same
                drop(crate::host::thread(
                    "crash",
                    |_| -> () { panic!("the crash asked for on a helper thread") },
                    |()| (),
                ));
            }
            _ => {
                return Err(zbus::fdo::Error::InvalidArgs(format!(
                    "{at:?} is not dispatch, paint or helper"
                )))
            }
        }
        Ok(())
    }
}

/// What [`serve`] needs of the calling thread's component, taken on its thread.
pub(crate) struct Serving {
    crash: Arc<Crash>,
    inbox: Arc<Inbox>,
}

impl Serving {
    /// The calling thread's component's; `None` outside a component.
    pub(crate) fn of_thread() -> Option<Serving> {
        context::with_ctx(|ctx| Serving {
            crash: Arc::clone(&ctx.crash),
            inbox: Arc::clone(&ctx.inbox),
        })
    }
}

/// Serves [`INTERFACE`] on the component's session bus connection.
pub(crate) async fn serve(connection: &zbus::Connection, serving: Serving) {
    let served = Served {
        crash: serving.crash,
        inbox: serving.inbox,
    };
    if let Err(e) = connection.object_server().at(PATH, served).await {
        tracing::warn!(error = %e, "the component serves no crash interface");
    }
}
