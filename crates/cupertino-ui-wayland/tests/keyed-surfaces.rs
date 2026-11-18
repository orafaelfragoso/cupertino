//! A role extension addresses the view's window and its actual layer surfaces before their frame reaches the wire.

#[path = "../../cupertino-wayland-client/tests/support/mod.rs"]
mod wire;

use cupertino_geometry::{Rect, Size};
use cupertino_keyboard::modifiers::Profile;
use cupertino_ui_core::platform::{
    DecorationMode, LayerChange, LayerList, LayerProperty, PlatformEvent, RaisedChange, Submission,
    Submitted, SubmittedLayer, WindowEvent, WindowRequest, WindowSpec, WindowSystem,
};
use cupertino_ui_core::WindowKey;
use cupertino_ui_render::{CpuConfig, CpuRenderer, DamageRegion};
use cupertino_ui_text::{FontIndex, GlyphRaster, TextSystem};
use cupertino_ui_wayland::{RoleEvent, SurfaceRoles, WaylandWindowSystem};
use cupertino_wayland_client::{Connection, Error, ExtensionEvent, SurfaceId};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use wire::{Harness, Offer};

#[derive(Default)]
struct Seen {
    opening: Option<WindowKey>,
    created: Vec<Option<WindowKey>>,
    frames: Vec<Frame>,
    state: Vec<Frame>,
}

struct Frame {
    key: WindowKey,
    surface: SurfaceId,
    layers: Vec<(u64, SurfaceId)>,
}

struct Roles(Rc<RefCell<Seen>>);

impl SurfaceRoles for Roles {
    fn opening(&mut self, key: Option<&WindowKey>) {
        self.0.borrow_mut().opening = key.cloned();
    }

    fn create(&mut self, _: &mut Connection, _: &WindowSpec) -> Option<Result<SurfaceId, Error>> {
        let mut seen = self.0.borrow_mut();
        let key = seen.opening.take();
        seen.created.push(key);
        None
    }

    fn event(&mut self, _: &ExtensionEvent) -> Option<RoleEvent> {
        None
    }

    fn before_commit(
        &mut self,
        conn: &mut Connection,
        key: &WindowKey,
        surface: SurfaceId,
        layers: &mut dyn Iterator<Item = (u64, SurfaceId)>,
        new_frame: bool,
    ) {
        let layers: Vec<_> = layers.collect();
        for (_, layer) in &layers {
            // A state-only commit marks the extension's request in the server's ordered record.
            conn.commit_state(*layer).unwrap();
        }
        let mut seen = self.0.borrow_mut();
        let commits = if new_frame {
            &mut seen.frames
        } else {
            &mut seen.state
        };
        commits.push(Frame {
            key: key.clone(),
            surface,
            layers,
        });
    }
}

fn frame() -> Submission {
    let text = Arc::new(TextSystem::new(FontIndex::new(), ""));
    let renderer = || {
        CpuRenderer::new(
            CpuConfig {
                max_pixels: 4096,
                image_cache_bytes: 0,
                glyph_cache_bytes: 0,
                font_smoothing: false,
            },
            GlyphRaster::new(text.clone()),
        )
    };
    Submission {
        renderer: Arc::new(Mutex::new(Box::new(renderer()))),
        list: Default::default(),
        resources: Default::default(),
        scale: 1.0,
        width: 32,
        height: 32,
        damage: DamageRegion::Full,
        raised: RaisedChange::Kept,
        surface_layers: Some(vec![SubmittedLayer {
            key: 42,
            rect: Rect::new(2.0, 2.0, 8.0, 8.0),
            look: Default::default(),
            layer: Some(LayerList {
                renderer: Arc::new(Mutex::new(Box::new(renderer()))),
                list: Default::default(),
                width: 8,
                height: 8,
                damage: DamageRegion::Full,
            }),
        }]),
    }
}

#[test]
fn keyed_roles_send_to_the_current_layers_before_the_window_commits() {
    let (server, conn) = Harness::start(Offer {
        layers: Some(6),
        releases: true,
        frames: true,
        ..Offer::NONE
    });
    let seen = Rc::new(RefCell::new(Seen::default()));
    let mut system =
        WaylandWindowSystem::new(conn, Profile::cupertino()).with_roles(Roles(seen.clone()));
    let spec = WindowSpec {
        app_id: "org.cupertino.tests.Layers".into(),
        title: "Window".into(),
        size: Size::new(32.0, 32.0),
        min_size: None,
        max_size: None,
        decorations: DecorationMode::None,
        foreign_parent: None,
        input_region: None,
    };
    let first = WindowKey("first".into());
    let second = WindowKey("second".into());
    system.opening(&first);
    let one = system.create_window(&spec).unwrap();
    system.opening(&second);
    let two = system.create_window(&spec).unwrap();
    system.create_window(&spec).unwrap();
    assert_eq!(
        seen.borrow().created,
        [Some(first), Some(second.clone()), None]
    );
    let mut frame = frame();
    assert_eq!(system.submit(two, &frame).unwrap(), Submitted::NotReady);
    assert!(
        seen.borrow().frames.is_empty(),
        "no role request for a frame waiting for configure"
    );
    system.connection().roundtrip().unwrap();
    system.dispatch(Some(Duration::ZERO), &mut vec![]).unwrap();
    let parent = system.surface(two).unwrap();
    assert_ne!(Some(parent), system.surface(one));
    let mut first_layer = None;
    for pass in 0..3 {
        if pass == 1 {
            frame.surface_layers = None;
        }
        if pass == 2 {
            frame.surface_layers = Some(vec![]);
        }
        let before = server.every_commit().len();
        assert_eq!(system.submit(two, &frame).unwrap(), Submitted::Taken);
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let mut events = Vec::new();
            system
                .dispatch(
                    Some(deadline.saturating_duration_since(Instant::now())),
                    &mut events,
                )
                .unwrap();
            if events.iter().any(|e| matches!(e, PlatformEvent::Window { window, event: WindowEvent::Shown(_) } if *window == two)) {
                break;
            }
            assert!(Instant::now() < deadline, "frame {pass} did not finish");
        }
        system.connection().roundtrip().unwrap();
        if pass == 1 {
            let start = server.every_commit().len();
            system
                .request(
                    two,
                    WindowRequest::Layer {
                        key: 42,
                        changes: vec![LayerChange::new(LayerProperty::Opacity, 0.5, None)],
                    },
                )
                .unwrap();
            system.connection().roundtrip().unwrap();
            let seen = seen.borrow();
            let [state] = seen.state.as_slice() else {
                panic!("expected one state-only commit")
            };
            assert_eq!((&state.key, state.surface), (&second, parent));
            assert_eq!(state.layers.as_slice(), [(42, first_layer.unwrap())]);
            assert_eq!(
                &server.every_commit()[start..],
                ["layer state", "window state"]
            );
        }
        let seen = seen.borrow();
        let Frame {
            key,
            surface,
            layers,
        } = &seen.frames[pass];
        assert_eq!((key, *surface), (&second, parent));
        let commits = server.every_commit();
        match pass {
            0 => {
                let [(42, layer)] = layers.as_slice() else {
                    panic!("wrong layer keys: {layers:?}")
                };
                assert_ne!(*layer, parent);
                first_layer = Some(*layer);
                assert_eq!(&commits[before..], ["layer state", "layer", "window"]);
            }
            1 => {
                assert_eq!(layers.as_slice(), [(42, first_layer.unwrap())]);
                assert_eq!(
                    &commits[before..],
                    ["layer state", "window", "layer state", "window state"]
                );
            }
            _ => {
                assert!(layers.is_empty());
                assert_eq!(&commits[before..], ["window"]);
            }
        }
    }
}
