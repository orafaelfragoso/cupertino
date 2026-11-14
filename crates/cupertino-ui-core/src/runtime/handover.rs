//! A popup's transitions handed to the compositor ([`Capabilities::present_popups`]): a popover's growth out of its
//! anchor and its shrinking back ([`super::morph`]) and a closing menu's fade ([`super::fade`]) become changes of the
//! popup's presentation, on the same curves, so the compositor runs them on its clock and the popup draws no frame for
//! them. The presentation scales about the popup's centre; growing out of another point `p` moves it by
//! `(p - c) (1 - s)` along the same curve, which keeps `p` where it is.
//!
//! [`Capabilities::present_popups`]: cupertino_ui_platform::Capabilities::present_popups

use cupertino_animation::{CubicBezier, LayerCurve};
use cupertino_geometry::{Point, Size};
use cupertino_ui_platform::{PresentChange, PresentProperty};

use super::morph;

/// `1 - (1 - t)²`, the progress of a popover shrinking into its anchor as a cubic Bézier.
const SHRINK: CubicBezier = CubicBezier::new(1.0 / 3.0, 2.0 / 3.0, 2.0 / 3.0, 1.0);
/// `1 - (1 - t)³`, the progress of a closing menu's fade as a cubic Bézier.
const MENU_FADE: CubicBezier = CubicBezier::new(1.0 / 3.0, 1.0, 2.0 / 3.0, 1.0);

fn change(property: PresentProperty, to: f64, curve: Option<LayerCurve>) -> PresentChange {
    PresentChange {
        property,
        to,
        curve,
        velocity: 0.0,
    }
}

/// How far the presentation moves a popup of `size` scaled by `s` about its centre so that `origin` stays put.
fn offset(origin: Point, size: Size, s: f64) -> (f64, f64) {
    (
        (origin.x - size.width / 2.0) * (1.0 - s),
        (origin.y - size.height / 2.0) * (1.0 - s),
    )
}

/// A popover of `size` growing out of `origin` (its coordinates): from [`morph::OPEN_START_SCALE`] to its size on the
/// opening spring.
pub(super) fn grow(origin: Point, size: Size) -> Vec<PresentChange> {
    let spring = Some(LayerCurve::Spring {
        response: morph::OPEN_RESPONSE,
        damping: morph::OPEN_DAMPING,
    });
    let (x, y) = offset(origin, size, morph::OPEN_START_SCALE);
    vec![
        change(PresentProperty::Scale, morph::OPEN_START_SCALE, None),
        change(PresentProperty::OffsetX, x, None),
        change(PresentProperty::OffsetY, y, None),
        change(PresentProperty::Scale, 1.0, spring),
        change(PresentProperty::OffsetX, 0.0, spring),
        change(PresentProperty::OffsetY, 0.0, spring),
    ]
}

/// A popup of `size` closing: a popover shrinks into `into` and fades as it does, over [`morph::CLOSE_NS`]; a menu
/// fades over [`super::fade::MENU_FADE_NS`].
pub(super) fn close(into: Option<Point>, size: Size) -> Vec<PresentChange> {
    let timed = |ns: u64, bezier| {
        Some(LayerCurve::Timed {
            duration: ns as f64 / 1e9,
            bezier,
        })
    };
    let Some(into) = into else {
        return vec![change(
            PresentProperty::Opacity,
            0.0,
            timed(super::fade::MENU_FADE_NS, MENU_FADE),
        )];
    };
    let curve = timed(morph::CLOSE_NS, SHRINK);
    let (x, y) = offset(into, size, 0.0);
    vec![
        change(PresentProperty::Scale, 0.0, curve),
        change(PresentProperty::Opacity, 0.0, curve),
        change(PresentProperty::OffsetX, x, curve),
        change(PresentProperty::OffsetY, y, curve),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use cupertino_animation::LayerProperty;

    /// The values the compositor shows for `changes` (sets, then animations started at 0) at `t` seconds, by property.
    fn shown(changes: &[PresentChange], rest: [f64; 4], t: f64) -> [f64; 4] {
        let index = |p| match p {
            PresentProperty::OffsetX => 0,
            PresentProperty::OffsetY => 1,
            PresentProperty::Opacity => 2,
            PresentProperty::Scale => 3,
        };
        let mut props = rest.map(LayerProperty::new);
        for c in changes.iter().filter(|c| c.curve.is_none()) {
            props[index(c.property)].set(c.to);
        }
        for c in changes {
            if let Some(curve) = c.curve {
                props[index(c.property)].animate(c.to, curve, 0.0, c.velocity);
            }
        }
        props.map(|p| p.value(t))
    }

    #[test]
    fn the_compositor_grows_a_popover_on_the_spring_it_draws_and_out_of_the_same_point() {
        let (size, origin) = (Size::new(240.0, 300.0), Point::new(120.0, 0.0));
        let changes = grow(origin, size);
        for ms in (0..=350).step_by(5) {
            let t = f64::from(ms) / 1000.0;
            let [x, y, opacity, s] = shown(&changes, [0.0, 0.0, 1.0, 1.0], t);
            let drawn = morph::open_scale(t);
            assert!((s - drawn).abs() < 1e-6, "{ms} ms: {s} against {drawn}");
            assert_eq!(opacity, 1.0);
            // the point it grows from stays put: scaled about the centre and moved
            let c = (size.width / 2.0, size.height / 2.0);
            let at = (
                c.0 + s * (origin.x - c.0) + x,
                c.1 + s * (origin.y - c.1) + y,
            );
            assert!((at.0 - origin.x).abs() < 1e-9 && (at.1 - origin.y).abs() < 1e-9);
        }
    }

    #[test]
    fn the_compositor_shrinks_a_popover_and_fades_a_menu_on_the_curves_they_draw() {
        let (size, into) = (Size::new(240.0, 300.0), Point::new(120.0, 0.0));
        let popover = close(Some(into), size);
        let menu = close(None, Size::new(180.0, 220.0));
        for step in 0..=40 {
            let ns = morph::CLOSE_NS * step / 40;
            let [x, _, opacity, s] = shown(&popover, [0.0, 0.0, 1.0, 1.0], ns as f64 / 1e9);
            let drawn = morph::close_scale(ns);
            assert!((s - drawn).abs() < 1e-6, "{ns}: {s} against {drawn}");
            assert!((opacity - drawn).abs() < 1e-6);
            assert!((x - (into.x - size.width / 2.0) * (1.0 - s)).abs() < 1e-9);
            let ns = super::super::fade::MENU_FADE_NS * step / 40;
            let [_, _, opacity, s] = shown(&menu, [0.0, 0.0, 1.0, 1.0], ns as f64 / 1e9);
            let drawn = f64::from(super::super::fade::opacity_at(ns));
            assert!(
                (opacity - drawn).abs() < 1e-6,
                "{ns}: {opacity} against {drawn}"
            );
            assert_eq!(s, 1.0, "a menu only fades");
        }
    }
}
