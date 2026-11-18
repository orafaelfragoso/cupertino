//! A surface's presenter on the transport: when it says to draw, and what of the last frame a frame keeps.

#[path = "../../cupertino-wayland-client/tests/support/mod.rs"]
mod wire;

use cupertino_geometry::Rect;
use cupertino_ui_wayland::{Ready, ShmPresenter};
use cupertino_wayland_client::{Connection, PixelRect};
use std::time::Duration;
use wire::{window, Harness, Offer};

#[test]
fn a_presenter_draws_once_configured_and_asked_and_then_waits_for_its_frame_callback() {
    let (_server, mut conn) = Harness::start(Offer {
        releases: true,
        ..Offer::NONE
    });
    let window = window(&mut conn);
    let mut presenter = ShmPresenter::new(window.surface());
    assert_eq!(presenter.size(), None);
    assert!(
        matches!(presenter.poll(&conn), Ok(Ready::Idle)),
        "nothing is drawn before a configure"
    );

    presenter.configured((64, 48));
    assert_eq!(presenter.size(), Some((64, 48)));
    assert!(matches!(presenter.poll(&conn), Ok(Ready::Draw)));
    presenter
        .draw(&mut conn, None, |pixmap, _| {
            pixmap.fill(tiny_skia::Color::WHITE)
        })
        .unwrap();
    presenter.request_redraw();
    assert!(
        matches!(presenter.poll(&conn), Ok(Ready::Idle)),
        "no second frame while the first is away"
    );
    presenter.frame_done();
    assert!(matches!(presenter.poll(&conn), Ok(Ready::Draw)));
    presenter.draw(&mut conn, None, |_, _| ()).unwrap();
    assert!(
        matches!(presenter.poll(&conn), Ok(Ready::Idle)),
        "a frame nobody asked for is not drawn"
    );
}

#[test]
fn a_frame_with_damage_clears_and_repaints_only_it_and_any_other_frame_is_whole() {
    let (_server, mut conn) = Harness::start(Offer {
        releases: true,
        ..Offer::NONE
    });
    let window = window(&mut conn);
    let mut presenter = ShmPresenter::new(window.surface());
    presenter.configured((64, 48));
    let mut cleared = Vec::new();
    let frame = |presenter: &mut ShmPresenter, conn: &mut Connection, damage: Option<&[Rect]>| {
        let mut seen = None;
        presenter.frame_done();
        // the server releases the buffer of the frame before; the connection learns it when it is dispatched
        for _ in 0..100 {
            if conn.buffer_free(presenter.surface()).unwrap() {
                break;
            }
            conn.wait(Some(Duration::from_millis(50))).unwrap();
        }
        presenter
            .draw_with(conn, damage, |pixmap, _, rects| {
                pixmap.fill(tiny_skia::Color::from_rgba8(10, 20, 30, 255));
                seen = Some(rects.map(<[PixelRect]>::to_vec));
            })
            .unwrap();
        seen.unwrap()
    };
    let hole = [Rect::new(10.2, 5.5, 20.0, 10.0)];
    cleared.push((
        "the first frame",
        frame(&mut presenter, &mut conn, Some(&hole)),
    ));
    cleared.push((
        "the next, with damage",
        frame(&mut presenter, &mut conn, Some(&hole)),
    ));
    cleared.push((
        "damage past the corner",
        frame(
            &mut presenter,
            &mut conn,
            Some(&[Rect::new(60.0, 40.0, 30.0, 30.0)]),
        ),
    ));
    cleared.push((
        "damage wholly outside",
        frame(
            &mut presenter,
            &mut conn,
            Some(&[Rect::new(100.0, 100.0, 5.0, 5.0)]),
        ),
    ));
    cleared.push(("no damage given", frame(&mut presenter, &mut conn, None)));
    presenter.configured((64, 48));
    cleared.push((
        "after a configure",
        frame(&mut presenter, &mut conn, Some(&hole)),
    ));
    cleared.push((
        "the frame after that",
        frame(&mut presenter, &mut conn, Some(&hole)),
    ));
    let px = |x, y, width, height| {
        Some(vec![PixelRect {
            x,
            y,
            width,
            height,
        }])
    };
    let expected = [
        ("the first frame", None),
        ("the next, with damage", px(10, 5, 21, 11)),
        ("damage past the corner", px(60, 40, 4, 8)),
        ("damage wholly outside", Some(vec![])),
        ("no damage given", None),
        ("after a configure", None),
        ("the frame after that", px(10, 5, 21, 11)),
    ];
    assert_eq!(cleared, expected);
}
