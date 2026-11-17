//! The scroll counter turns the speed of a vertical scroll gesture into a repeat count for the shell.

use cupertino_input::{ScrollCounter, ScrollPhase};

use ScrollPhase::{Began, Cancelled, Changed, Ended};

#[test]
fn the_count_follows_the_speed_and_rhythm_of_the_gesture() {
    // Each step: (phase, dx, dy, time in ms, the count it returns; None leaves it unchecked).
    type Step = (ScrollPhase, f64, f64, f64, Option<u32>);
    let fast_then_ended: [Step; 2] = [
        (Changed, 0.0, 40.0, 0.0, Some(1)),
        (Ended, 0.0, 0.0, 10.0, None),
    ];
    let slow: Vec<Step> = (0..20)
        .map(|i| (Changed, 0.0, 10.0, f64::from(i) * 10.0, Some(0)))
        .collect();
    let with = |rest: &[Step]| [&fast_then_ended[..], rest].concat();
    for (name, steps) in [
        ("a slow scroll never starts a count", slow),
        (
            "a new gesture within the window raises it, then motion adds speed / 2.5",
            with(&[
                (Began, 0.0, 40.0, 100.0, Some(2)),
                (Ended, 0.0, 0.0, 110.0, None),
                (Began, 0.0, 40.0, 200.0, Some(3)),
                (Changed, 0.0, 50.0, 210.0, Some(23)),
            ]),
        ),
        (
            "a gesture after the window starts over",
            with(&[(Began, 0.0, 40.0, 610.0, Some(0))]),
        ),
        (
            "a began phase raises the count once per gesture",
            with(&[
                (Began, 0.0, 40.0, 100.0, Some(2)),
                (Began, 0.0, 40.0, 110.0, Some(2)),
            ]),
        ),
        (
            "a pause in the motion ends the count",
            vec![
                (Changed, 0.0, 40.0, 0.0, Some(1)),
                (Changed, 0.0, 5.0, 200.0, Some(1)),
                (Changed, 0.0, 5.0, 300.0, Some(0)),
            ],
        ),
        (
            "a fast update renews the sustain time",
            vec![
                (Changed, 0.0, 40.0, 0.0, Some(1)),
                (Changed, 0.0, 30.0, 200.0, None),
                (Changed, 0.0, 5.0, 400.0, Some(1)),
            ],
        ),
        (
            "reversing direction restarts the count",
            vec![
                (Changed, 0.0, 40.0, 0.0, Some(1)),
                (Changed, 0.0, -40.0, 10.0, Some(1)),
                (Changed, 0.0, -5.0, 20.0, Some(1)),
            ],
        ),
        (
            "cancelling clears everything",
            vec![
                (Changed, 0.0, 40.0, 0.0, Some(1)),
                (Cancelled, 0.0, 0.0, 10.0, Some(0)),
                (Began, 0.0, 40.0, 20.0, Some(0)),
            ],
        ),
        (
            "a horizontal scroll does not change the direction",
            vec![
                (Changed, 0.0, 40.0, 0.0, Some(1)),
                (Changed, 100.0, 0.0, 10.0, Some(1)),
            ],
        ),
    ] {
        let mut c = ScrollCounter::default();
        for (i, (phase, dx, dy, t, expected)) in steps.into_iter().enumerate() {
            let count = c.update(phase, dx, dy, t);
            if let Some(expected) = expected {
                assert_eq!(count, expected, "{name}: step {i}");
            }
        }
    }
}

#[test]
fn the_count_is_bounded() {
    let mut c = ScrollCounter::default();
    c.update(Changed, 0.0, 40.0, 0.0);
    c.update(Ended, 0.0, 0.0, 1.0);
    c.update(Began, 0.0, 40.0, 2.0);
    c.update(Ended, 0.0, 0.0, 3.0);
    c.update(Began, 0.0, 40.0, 4.0);
    let mut last = 0;
    for i in 0..200 {
        last = c.update(Changed, 0.0, 5000.0, 10.0 + f64::from(i));
    }
    assert_eq!(last, 2000);
}
