//! A renderer that touches no GPU consumes the scenes the compositor composes: every typed handle an item holds
//! resolves in the frame's resources (retained Snapshot sources are outside this fake's checks), and the frame has the items the scene calls for. It is the proof that the seam
//! (`cupertino-render-api`) and the scene frame are usable by an implementation that is not GLES.

mod common;

use std::time::Duration;

use common::desktop::Desktop;
use cupertino_render_api::{Completion, Failure, RenderFailure, RenderStatus, Renderer};
use cupertino_scene::{SceneFrame, SceneKind, Source};
use cupertino_server::backend::Backend;
use cupertino_server::render::compose::{Composed, FrameResources};
use cupertino_wm::Command;

/// What the fake saw of one frame.
#[derive(Debug, Default)]
struct Seen {
    items: usize,
    kinds: Vec<SceneKind>,
    surfaces: usize,
    images: usize,
    recipes: usize,
    snapshots: usize,
}

impl Completion for Seen {
    fn status(&self) -> RenderStatus {
        if self.items == 0 {
            RenderStatus::Empty
        } else {
            RenderStatus::Rendered
        }
    }
}

#[derive(Debug)]
struct Unresolved(String);

impl std::fmt::Display for Unresolved {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for Unresolved {}

impl RenderFailure for Unresolved {
    fn failure(&self) -> Failure {
        Failure::Transient
    }
}

struct Fake;

impl Renderer for Fake {
    type Resources<'a> = FrameResources;
    type Target<'a> = ();
    type Completion = Seen;
    type Error = Unresolved;

    fn render(
        &mut self,
        scene: &SceneFrame,
        resources: &mut FrameResources,
        (): (),
    ) -> Result<Seen, Unresolved> {
        if resources.frame != scene.id {
            return Err(Unresolved("the resources belong to another frame".into()));
        }
        let mut seen = Seen::default();
        for (_, item) in scene.items() {
            seen.items += 1;
            seen.kinds.push(item.id.kind);
            let slot_of = |slot: u64, generation: u64, held: usize, what: &str| {
                if generation == scene.id.0 && (slot as usize) < held {
                    Ok(())
                } else {
                    Err(Unresolved(format!(
                        "{what} handle {slot}/{generation} of {:?}",
                        item.id
                    )))
                }
            };
            match &item.source {
                Source::SurfaceTree { id, .. } => {
                    slot_of(
                        id.0.slot,
                        id.0.generation,
                        resources.surfaces.len(),
                        "surface",
                    )?;
                    seen.surfaces += 1;
                }
                Source::Image { id, .. } => {
                    slot_of(id.0.slot, id.0.generation, resources.images.len(), "image")?;
                    seen.images += 1;
                }
                Source::Material(pass) => {
                    slot_of(
                        pass.recipe.0.slot,
                        pass.recipe.0.generation,
                        resources.recipes.len(),
                        "recipe",
                    )?;
                    seen.recipes += 1;
                }
                Source::Glass(pass) => {
                    slot_of(
                        pass.glass.0.slot,
                        pass.glass.0.generation,
                        resources.glass.len(),
                        "glass",
                    )?;
                }
                Source::TexturedMesh { snapshot, .. } => {
                    slot_of(
                        snapshot.0.slot,
                        snapshot.0.generation,
                        resources.snapshots.len(),
                        "snapshot",
                    )?;
                    if resources.snapshots[snapshot.0.slot as usize].is_none() {
                        return Err(Unresolved("the frame snapshot is absent".into()));
                    }
                    seen.snapshots += 1;
                }
                Source::Snapshot { .. }
                | Source::RoundedRect(_)
                | Source::Shadow(_)
                | Source::Wallpaper { .. } => {}
            }
        }
        Ok(seen)
    }
}

/// A renderer whose every frame fails the same way.
struct Failing(Failure);

#[derive(Debug)]
struct Broken(Failure);

impl std::fmt::Display for Broken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "the device failed: {:?}", self.0)
    }
}

impl std::error::Error for Broken {}

impl RenderFailure for Broken {
    fn failure(&self) -> Failure {
        self.0
    }
}

impl Renderer for Failing {
    type Resources<'a> = FrameResources;
    type Target<'a> = ();
    type Completion = Seen;
    type Error = Broken;

    fn render(&mut self, _: &SceneFrame, _: &mut FrameResources, (): ()) -> Result<Seen, Broken> {
        Err(Broken(self.0))
    }
}

/// Compose output 0.
fn compose(d: &mut Desktop) -> Composed {
    let mut backend = d.state.backend.take().unwrap();
    #[allow(irrefutable_let_patterns)]
    let Backend::Headless(ref mut h) = backend
    else {
        panic!("headless backend required")
    };
    let composed = d
        .state
        .compose_output(&mut h.renderer, 0, true)
        .expect("render resources");
    d.state.backend = Some(backend);
    composed
}

/// One second of loop passes, 4 ms apart as the udev loop runs them, drawing output 0 with a renderer that always
/// fails: a pass draws while the session runs and the output is due, and a failed frame takes the backends' tail.
/// Returns how many frames were drawn.
fn fail_for_a_second(d: &mut Desktop, failure: Failure) -> u32 {
    let start = Duration::from(d.state.clock.now());
    let mut draws = 0;
    for pass in 0..250 {
        let now = start + Duration::from_millis(4) * pass;
        if !d.state.running || !d.state.retry_due(0, now) {
            continue;
        }
        let composed = compose(d);
        let mut resources = composed.resources;
        draws += 1;
        if let Err(e) = Failing(failure).render(&composed.frame, &mut resources, ()) {
            d.state
                .frame_failed(0, now, Duration::from_micros(16_667), &e);
        }
    }
    draws
}

/// Compose output 0 and let the fake consume it.
fn consume(d: &mut Desktop) -> Seen {
    let composed = compose(d);
    let mut resources = composed.resources;
    Fake.render(&composed.frame, &mut resources, ())
        .unwrap_or_else(|e| panic!("the fake could not resolve a handle: {e}"))
}

#[test]
fn a_desktop_with_windows_resolves_without_a_gpu() {
    let mut d = Desktop::new(1.5);
    d.window();
    d.window();
    d.at(3000);
    let seen = consume(&mut d);
    assert_eq!(seen.status(), RenderStatus::Rendered);
    assert!(seen.surfaces >= 2, "the window contents: {seen:?}");
    assert!(seen.images >= 1, "the pointer image: {seen:?}");
    assert!(seen.kinds.contains(&SceneKind::Shadow));
}

#[test]
fn a_locked_session_resolves_without_a_gpu() {
    let mut d = Desktop::new(1.0);
    d.window();
    d.at(3000);
    let _locker = d.lock();
    d.at(3100);
    let seen = consume(&mut d);
    assert!(seen.kinds.contains(&SceneKind::LockContent));
    assert!(!seen.kinds.contains(&SceneKind::WindowContent));
}

#[test]
fn a_material_pass_and_a_minimizing_window_resolve_without_a_gpu() {
    let mut config = cupertino_server::config::Config::default();
    config.effects.minimize = Some(cupertino_effects::MinimizeKind::Warp);
    let display = cupertino_server::backend::headless::HeadlessDisplay {
        name: "headless-1".into(),
        size: (1000, 700),
        physical_mm: (0, 0),
    };
    let mut d = Desktop::configured(config, std::slice::from_ref(&display), 1.0);
    d.window();
    d.at(3000);
    d.state.accessibility.settings.color_filter =
        Some(cupertino_defaults::accessibility::ColorFilter::Grayscale);
    let id = d.state.wm.key_window().unwrap();
    d.state.command(Command::Minimize {
        window: id,
        target: Some(cupertino_wm::Rect::new(440.0, 640.0, 64.0, 64.0)),
    });
    let t0 = d.state.now();
    common::set_time(&mut d.state, &mut d.event_loop, t0 + 0.25);
    d.state.tick();
    d.pump();
    let seen = consume(&mut d);
    assert!(
        seen.recipes >= 1,
        "the display filter is a material pass: {seen:?}"
    );
    assert!(
        seen.snapshots >= 1,
        "the minimizing window is a mesh over a snapshot: {seen:?}"
    );
}

#[test]
fn a_closing_ghost_and_the_input_hud_resolve_without_a_gpu() {
    let mut d = Desktop::new(1.0);
    d.state.fx.cfg.closing = true;
    d.window();
    d.at(3000);
    d.state.overlays.inputs.hud = Some(cupertino_server::kbd::HudView {
        order: vec!["us".into(), "de".into()],
        labels: vec!["ABC".into(), "Deutsch".into()],
        selection: 0,
        fading_since: None,
    });
    d.clients.clear();
    let deadline = std::time::Instant::now() + common::HANG_GUARD;
    while d.state.wm.windows().count() != 0 {
        d.pump();
        assert!(
            std::time::Instant::now() < deadline,
            "the window did not close"
        );
    }
    let seen = consume(&mut d);
    assert!(seen.kinds.contains(&SceneKind::ClosingGhost), "{seen:?}");
    assert!(seen.kinds.contains(&SceneKind::InputHud), "{seen:?}");
}

#[test]
fn a_handle_from_another_frame_is_refused() {
    let mut d = Desktop::new(1.0);
    d.window();
    d.at(3000);
    let mut backend = d.state.backend.take().unwrap();
    #[allow(irrefutable_let_patterns)]
    let Backend::Headless(ref mut h) = backend
    else {
        panic!("headless backend required")
    };
    let first = d.state.compose_output(&mut h.renderer, 0, true).unwrap();
    let second = d.state.compose_output(&mut h.renderer, 0, true).unwrap();
    d.state.backend = Some(backend);
    // the second frame's items name surfaces of the second frame's registry, not the first's
    let mut resources = first.resources;
    let result = Fake.render(&second.frame, &mut resources, ());
    assert!(
        result.is_err(),
        "handles of one frame do not resolve against another's resources"
    );
}

#[test]
fn a_failing_output_is_retried_at_the_display_rate_not_the_loop_rate() {
    let mut d = Desktop::new(1.0);
    d.window();
    d.at(3000);
    let draws = fail_for_a_second(&mut d, Failure::Transient);
    assert!(
        (1..=60).contains(&draws),
        "{draws} draws and frame callbacks in one second"
    );
    assert!(d.state.running);
    assert!(
        d.state.outputs[0].scheduler.damaged(),
        "a failed frame is drawn again"
    );
    d.state.frame_drawn(0);
    assert!(
        d.state.retry_due(0, Duration::ZERO),
        "a drawn frame ends the wait between retries"
    );
}

#[test]
fn a_lost_rendering_context_ends_the_session_after_one_draw() {
    let mut d = Desktop::new(1.0);
    d.window();
    d.at(3000);
    assert_eq!(fail_for_a_second(&mut d, Failure::ContextLost), 1);
    assert!(!d.state.running, "the session ends");
    assert!(
        d.state.rendering_lost.is_some(),
        "with an error, so the session command sees a failure"
    );
}

#[test]
fn a_lost_rendering_context_is_recorded_for_the_session_and_a_hung_teardown_is_cut_short() {
    const CHILD: &str = "CUPERTINO_TEST_CONTEXT_LOST_CHILD";
    if std::env::var_os(CHILD).is_some() {
        cupertino_server::logging::record_session_end(|key| std::env::var_os(key));
        let mut d = Desktop::new(1.0);
        d.window();
        d.at(3000);
        fail_for_a_second(&mut d, Failure::ContextLost);
        loop {
            std::thread::park();
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let end = dir.path().join("session-end");
    // timed from the loss, which writes the reason: starting a compositor with a window is not what is measured
    let (status, _) = common::spawn::run_child_test_after(
        "a_lost_rendering_context_is_recorded_for_the_session_and_a_hung_teardown_is_cut_short",
        &[
            (CHILD, std::ffi::OsStr::new("1")),
            (cupertino_server::logging::SESSION_END_ENV, end.as_os_str()),
        ],
        || end.exists(),
        cupertino_server::logging::TEARDOWN_LIMIT + Duration::from_secs(3),
    );
    assert_eq!(status.code(), Some(1));
    let reason = std::fs::read_to_string(&end).unwrap();
    assert!(
        reason.starts_with("the rendering context was lost ("),
        "{reason:?}"
    );
}
