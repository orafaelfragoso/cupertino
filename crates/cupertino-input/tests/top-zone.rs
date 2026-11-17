//! Pushing the pointer up against the top edge in a near-vertical stroke fires once after enough travel.

use cupertino_input::{TopZone, TopZoneConfig};

/// The indices of the `(x, dx, dy)` motions that fired.
fn fires(z: &mut TopZone, motions: &[(f64, f64, f64)]) -> Vec<usize> {
    motions
        .iter()
        .enumerate()
        .filter(|(_, &(x, dx, dy))| z.motion(x, dx, dy))
        .map(|(i, _)| i)
        .collect()
}

const UP: (f64, f64, f64) = (0.0, 0.0, -10.0);

#[test]
fn a_near_vertical_push_at_the_edge_fires_once_after_the_distance() {
    let up = |n: usize| vec![UP; n];
    for (name, motions, fired) in [
        (
            "64 points of travel is reached by the seventh 10 point step",
            up(20),
            vec![6],
        ),
        (
            "away from the top edge",
            vec![(5.0, 0.0, -10.0); 20],
            vec![],
        ),
        // atan(10 / 20) is well above the limit of a sixteenth of a half turn.
        ("too far off vertical", vec![(0.0, 10.0, -20.0); 50], vec![]),
        ("slightly slanted", vec![(0.0, 1.0, -20.0); 10], vec![3]),
        (
            "leaving the edge allows another fire",
            [up(8), vec![(30.0, 0.0, 0.0)], up(7)].concat(),
            vec![6, 15],
        ),
        (
            "motion down resets the push",
            [up(5), vec![(0.0, 0.0, 3.0)], up(1)].concat(),
            vec![],
        ),
        (
            "motion sideways resets the push",
            [up(5), vec![(0.0, 8.0, 0.0)], up(1)].concat(),
            vec![],
        ),
    ] {
        assert_eq!(fires(&mut TopZone::default(), &motions), fired, "{name}");
    }
}

#[test]
fn a_reset_clears_the_push_and_the_fired_state() {
    let mut z = TopZone::default();
    assert_eq!(fires(&mut z, &[UP; 7]), [6]);
    z.reset();
    assert_eq!(fires(&mut z, &[UP; 7]), [6]);
}

#[test]
fn the_distance_comes_from_the_configuration() {
    let mut z = TopZone::new(TopZoneConfig {
        distance: 15.0,
        ..TopZoneConfig::default()
    });
    assert_eq!(fires(&mut z, &[UP; 3]), [1]);
}
