//! A frame copied to a display of another card never makes the compositor's loop wait for the render GPU without a
//! deadline: a sync point is reached, or the display's GPU waits for it, or the loop waits at most the copy's bound,
//! and a fence nothing can bound is never waited for. Two cards cannot be had here: one renderer stands for both.
#![cfg(feature = "udev")]

use std::os::fd::OwnedFd;
use std::time::{Duration, Instant};

use cupertino_server::backend::copy::{finished, settle, Settled, CPU_WAIT};
use cupertino_server::backend::headless::create_renderer;
use smithay::backend::egl::fence::EGLFence;
use smithay::backend::renderer::sync::{Fence, Interrupted, SyncPoint};

/// Well over the copy's bound, well under a hang.
const PROMPT: Duration = Duration::from_secs(1);

/// A fence that never signals and cannot be exported, as a GPU that hung: a wait for it would never end.
#[derive(Debug)]
struct Hung;

impl Fence for Hung {
    fn is_signaled(&self) -> bool {
        false
    }

    fn wait(&self) -> Result<(), Interrupted> {
        panic!("a fence was waited for without a deadline")
    }

    fn is_exportable(&self) -> bool {
        false
    }

    fn export(&self) -> Option<OwnedFd> {
        None
    }
}

#[test]
fn a_signalled_point_is_reached() {
    let mut renderer = create_renderer().expect("an offscreen renderer");
    let sync = SyncPoint::signaled();
    assert_eq!(settle(&mut renderer, &sync, CPU_WAIT), Settled::Reached);
    assert!(finished(&sync, CPU_WAIT));
}

#[test]
fn a_fence_nothing_can_bound_is_late_at_once() {
    let mut renderer = create_renderer().expect("an offscreen renderer");
    let sync = SyncPoint::from(Hung);
    let started = Instant::now();
    assert_eq!(settle(&mut renderer, &sync, CPU_WAIT), Settled::Late);
    assert!(!finished(&sync, CPU_WAIT));
    assert!(started.elapsed() < PROMPT, "took {:?}", started.elapsed());
}

#[test]
fn a_fence_of_finished_work_is_not_late() {
    let mut renderer = create_renderer().expect("an offscreen renderer");
    renderer
        .with_context(|gl| unsafe { gl.Finish() })
        .expect("the renderer's context");
    let fence = match EGLFence::create(renderer.egl_context().display()) {
        Ok(fence) => fence,
        Err(e) => {
            eprintln!("skipped: the display has no fences ({e})");
            return;
        }
    };
    renderer
        .with_context(|gl| unsafe { gl.Flush() })
        .expect("the renderer's context");
    let sync = SyncPoint::from(fence);
    let started = Instant::now();
    assert_ne!(settle(&mut renderer, &sync, CPU_WAIT), Settled::Late);
    assert!(finished(&sync, PROMPT));
    assert!(started.elapsed() < PROMPT, "took {:?}", started.elapsed());
}
