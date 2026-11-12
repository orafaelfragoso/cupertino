//! How every menu follows the pointer: the menu bar's, CupertinoKit's pop-up and context menus and the file manager's.
//!
//! * [`Tracking`] decides what the release of a press does: a press on a menu's title dragged onto a row chooses the
//!   row on release, while a quick click on the title leaves the menu open to choose with a second click.
//! * [`SubmenuAim`] tells when the pointer is on its way from a row into the submenu it opened, so the rows it crosses
//!   on the way neither take the highlight nor close the submenu. The pointer heads into the submenu while each move
//!   stays inside the triangle between where it was and the submenu's near edge, and only as long as it keeps moving:
//!   a move outside the triangle, or a rest of [`SUBMENU_AIM_PAUSE`], gives the submenu up to the row under the
//!   pointer, and the submenu then closes at once. A submenu opens [`SUBMENU_OPEN_DELAY`] after its row takes the
//!   highlight, so a pointer passing over the row on its way elsewhere opens nothing.
//!
//! Times are in seconds on any clock the caller keeps; positions are `(x, y)` in any coordinates the caller keeps, the
//! same for the pointer and the submenu.

/// A press on a menu's title held at least this long (seconds) is a drag: its release chooses the row under the
/// pointer, or closes the menu away from the menus. Released sooner on the title, it is a click and the menu stays
/// open.
pub const TRACKING_INTERVAL: f64 = 0.5;

/// How long a submenu's row stays highlighted before the submenu opens (seconds).
pub const SUBMENU_OPEN_DELAY: f64 = 0.2;

/// The grace of the way into an open submenu: how long the pointer may rest on it, over any row, before the row under
/// it takes over and the submenu closes (seconds). Off the way, another row takes over at once.
pub const SUBMENU_AIM_PAUSE: f64 = 0.375;

/// Where a press was let go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Under {
    /// A row that can be chosen and opens no submenu.
    Item,
    /// Anywhere else on an open menu: its padding, a separator, a disabled row or a submenu's row.
    Menu,
    /// The title or the control the menu opened from.
    Title,
    /// Away from the menus and their title.
    Outside,
}

/// What the release of a press does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Release {
    /// Choose the row under the pointer, which closes the menus.
    Choose,
    /// Leave the menus open, to choose with a click or the keyboard.
    Stay,
    /// Close the menus without choosing.
    Close,
}

/// A press held on a menu's title or in an open menu, until its release.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tracking {
    pressed_at: f64,
    reached: bool,
}

impl Tracking {
    /// A press at `at` on the title of a menu, which opened it: its release chooses a row once the pointer reached one.
    pub fn from_title(at: f64) -> Self {
        Tracking {
            pressed_at: at,
            reached: false,
        }
    }

    /// A press at `at` in an open menu: its release chooses the row it lands on, as a click does.
    pub fn in_menu(at: f64) -> Self {
        Tracking {
            pressed_at: at,
            reached: true,
        }
    }

    /// The pointer came onto a row that can be chosen, other than one it was on when the press opened the menu.
    pub fn reach(&mut self) {
        self.reached = true;
    }

    pub fn reached(&self) -> bool {
        self.reached
    }

    /// What letting go at `at` over `under` does.
    pub fn release(&self, at: f64, under: Under) -> Release {
        let held = at - self.pressed_at >= TRACKING_INTERVAL;
        match under {
            Under::Item if self.reached || held => Release::Choose,
            Under::Item | Under::Menu => Release::Stay,
            Under::Title if !held => Release::Stay,
            Under::Title | Under::Outside => Release::Close,
        }
    }
}

/// The near edge of an open submenu, where the pointer comes into it from the row that opened it: the vertical segment
/// at `x` from `top` to `bottom`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Edge {
    pub x: f64,
    pub top: f64,
    pub bottom: f64,
}

/// Whether the pointer is on its way into an open submenu. Feed it every move of the pointer over the menu the
/// submenu opened from; see the module documentation.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SubmenuAim {
    last: Option<(f64, f64)>,
    /// The pointer heads into the submenu until this time unless it moves on.
    until: Option<f64>,
}

impl SubmenuAim {
    /// The pointer moved to `to` at `at`; `submenu` is the near edge of the submenu open from this menu, if one is.
    /// `true` while the pointer heads into the submenu: the menu keeps its highlight and its submenu, and the row under
    /// the pointer waits.
    pub fn moved(&mut self, to: (f64, f64), at: f64, submenu: Option<Edge>) -> bool {
        let from = self.last.replace(to);
        if from == Some(to) {
            return self.until.is_some_and(|until| at <= until);
        }
        let in_time = self.until.is_none_or(|until| at <= until);
        let heading = in_time
            && match (from, submenu) {
                (Some(from), Some(edge)) => {
                    point_in_triangle(to, from, (edge.x, edge.top), (edge.x, edge.bottom))
                }
                _ => false,
            };
        self.until = heading.then_some(at + SUBMENU_AIM_PAUSE);
        heading
    }

    /// When the pointer, resting, stops heading into the submenu, if it heads there now.
    pub fn deadline(&self) -> Option<f64> {
        self.until
    }

    /// `true` once `now` passed the deadline: the pointer rested and the row under it takes over.
    pub fn expired(&mut self, now: f64) -> bool {
        if self.until.is_some_and(|until| now >= until) {
            self.until = None;
            true
        } else {
            false
        }
    }

    /// The pointer left the menu, or the submenu closed.
    pub fn reset(&mut self) {
        *self = SubmenuAim::default();
    }
}

fn cross(o: (f64, f64), a: (f64, f64), b: (f64, f64)) -> f64 {
    (a.0 - o.0) * (b.1 - o.1) - (a.1 - o.1) * (b.0 - o.0)
}

/// Whether `p` lies inside (or on the edge of) the triangle `(t0, t1, t2)`.
pub fn point_in_triangle(p: (f64, f64), t0: (f64, f64), t1: (f64, f64), t2: (f64, f64)) -> bool {
    let d1 = cross(p, t0, t1);
    let d2 = cross(p, t1, t2);
    let d3 = cross(p, t2, t0);
    let neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
    let pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
    !(neg && pos)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_quick_click_on_the_title_stays_open_a_held_one_closes_and_a_drag_onto_a_row_chooses_it() {
        let click = Tracking::from_title(1.0);
        assert_eq!(click.release(1.2, Under::Title), Release::Stay);
        assert_eq!(
            click.release(1.0 + TRACKING_INTERVAL - 0.01, Under::Title),
            Release::Stay,
            "just inside the interval"
        );
        assert_eq!(
            click.release(1.0 + TRACKING_INTERVAL, Under::Title),
            Release::Close,
            "held without reaching a row"
        );
        assert_eq!(click.release(1.4, Under::Outside), Release::Close);
        let mut drag = Tracking::from_title(1.0);
        drag.reach();
        assert_eq!(
            drag.release(1.2, Under::Item),
            Release::Choose,
            "a fast drag"
        );
        assert_eq!(drag.release(1.2, Under::Title), Release::Stay);
        assert_eq!(
            drag.release(1.6, Under::Title),
            Release::Close,
            "dragged out and back"
        );
        assert_eq!(drag.release(1.6, Under::Menu), Release::Stay);
        assert_eq!(drag.release(1.2, Under::Outside), Release::Close);
    }

    #[test]
    fn a_menu_opened_under_the_pointer_chooses_the_row_there_only_after_the_interval() {
        let press = Tracking::from_title(0.0);
        assert_eq!(press.release(0.3, Under::Item), Release::Stay);
        assert_eq!(
            press.release(TRACKING_INTERVAL, Under::Item),
            Release::Choose
        );
        assert_eq!(
            Tracking::in_menu(0.0).release(0.1, Under::Item),
            Release::Choose
        );
    }

    /// A submenu whose near edge is at x = 100, from y = 0 to 200; its row is around y = 10.
    const EDGE: Edge = Edge {
        x: 100.0,
        top: 0.0,
        bottom: 200.0,
    };

    #[test]
    fn moving_towards_the_submenu_heads_into_it_and_moving_away_does_not() {
        let mut aim = SubmenuAim::default();
        assert!(!aim.moved((50.0, 10.0), 0.0, Some(EDGE)), "a first point");
        assert!(aim.moved((60.0, 30.0), 0.02, Some(EDGE)), "diagonally down");
        assert_eq!(aim.deadline(), Some(0.02 + SUBMENU_AIM_PAUSE));
        assert!(aim.moved((70.0, 60.0), 0.04, Some(EDGE)));
        assert!(
            !aim.moved((40.0, 70.0), 0.06, Some(EDGE)),
            "back to the left"
        );
        assert_eq!(aim.deadline(), None);
        assert!(!aim.moved((50.0, 80.0), 0.08, None), "no submenu open");
    }

    #[test]
    fn a_rest_or_a_slow_path_gives_the_submenu_up() {
        let mut aim = SubmenuAim::default();
        aim.moved((50.0, 10.0), 0.0, Some(EDGE));
        assert!(aim.moved((55.0, 20.0), 0.1, Some(EDGE)));
        assert!(!aim.expired(0.2));
        assert!(aim.expired(0.1 + SUBMENU_AIM_PAUSE), "resting");
        assert!(!aim.expired(1.0), "once");
        let mut slow = SubmenuAim::default();
        slow.moved((50.0, 10.0), 0.0, Some(EDGE));
        assert!(slow.moved((55.0, 20.0), 0.1, Some(EDGE)));
        assert!(
            !slow.moved((60.0, 30.0), 0.1 + SUBMENU_AIM_PAUSE + 0.1, Some(EDGE)),
            "the next step came after the pause"
        );
    }

    /// A diagonal from the submenu's row down to its lower rows crosses three rows of the menu below it, 24 points
    /// each: the submenu stays all the way, and once the pointer stops on a row for the grace, that row takes over.
    #[test]
    fn a_diagonal_across_other_rows_keeps_the_submenu_and_a_rest_for_the_grace_hands_it_over() {
        assert_eq!(SUBMENU_AIM_PAUSE, 0.375);
        let mut aim = SubmenuAim::default();
        aim.moved((40.0, 12.0), 0.0, Some(EDGE));
        let mut t = 0.0;
        for step in 1..=6 {
            t += 0.016;
            let to = (40.0 + 9.0 * step as f64, 12.0 + 12.0 * step as f64);
            assert!(
                aim.moved(to, t, Some(EDGE)),
                "row {} crossed",
                (to.1 / 24.0) as u32
            );
        }
        // resting on the row at y = 84, the fourth one, until just before the timer
        assert!(!aim.expired(t + SUBMENU_AIM_PAUSE - 0.01));
        assert!(
            aim.moved((94.0, 84.0), t + SUBMENU_AIM_PAUSE - 0.01, Some(EDGE)),
            "still resting"
        );
        assert!(
            aim.expired(t + SUBMENU_AIM_PAUSE),
            "the grace at rest: the row takes over"
        );
    }

    /// Straight down or up from the submenu's row onto the next one is not the way into the submenu, however short the
    /// step: nothing waits, and the row the pointer reaches takes over at once.
    #[test]
    fn straight_down_or_up_to_the_next_row_is_not_the_way_into_the_submenu() {
        let mut aim = SubmenuAim::default();
        aim.moved((60.0, 12.0), 0.0, Some(EDGE));
        assert!(!aim.moved((60.0, 36.0), 0.016, Some(EDGE)), "down");
        assert_eq!(aim.deadline(), None);
        let mut aim = SubmenuAim::default();
        aim.moved((60.0, 12.0), 0.0, Some(EDGE));
        assert!(!aim.moved((60.0, 2.0), 0.016, Some(EDGE)), "up");
        assert_eq!(aim.deadline(), None);
    }

    #[test]
    fn a_submenu_opens_after_a_short_dwell_and_the_way_into_it_holds_for_the_grace() {
        assert_eq!(SUBMENU_OPEN_DELAY, 0.2);
        assert_eq!(SUBMENU_AIM_PAUSE, 0.375);
        let mut aim = SubmenuAim::default();
        aim.moved((60.0, 12.0), 0.0, Some(EDGE));
        assert!(aim.moved((70.0, 20.0), 0.016, Some(EDGE)));
        assert!(!aim.expired(0.016 + SUBMENU_AIM_PAUSE - 0.001));
        assert!(aim.expired(0.016 + SUBMENU_AIM_PAUSE));
    }

    #[test]
    fn the_same_point_again_keeps_the_aim_until_its_deadline() {
        let mut aim = SubmenuAim::default();
        aim.moved((50.0, 10.0), 0.0, Some(EDGE));
        assert!(aim.moved((60.0, 20.0), 0.1, Some(EDGE)));
        assert!(aim.moved((60.0, 20.0), 0.2, Some(EDGE)));
        assert!(!aim.moved((60.0, 20.0), 0.7, Some(EDGE)));
    }

    #[test]
    fn a_submenu_on_the_left_is_headed_into_leftwards_and_an_empty_one_along_its_line() {
        // flipped beside the screen edge, or in right-to-left: the near edge is the submenu's right side
        let left = Edge {
            x: 0.0,
            top: 0.0,
            bottom: 200.0,
        };
        let mut aim = SubmenuAim::default();
        aim.moved((50.0, 10.0), 0.0, Some(left));
        assert!(
            aim.moved((40.0, 30.0), 0.02, Some(left)),
            "down to the left"
        );
        assert!(!aim.moved((60.0, 40.0), 0.04, Some(left)), "to the right");
        // no height: the triangle is a segment, and a move off it never heads there
        let flat = Edge {
            x: 100.0,
            top: 10.0,
            bottom: 10.0,
        };
        let mut aim = SubmenuAim::default();
        aim.moved((50.0, 10.0), 0.0, Some(flat));
        assert!(aim.moved((70.0, 10.0), 0.02, Some(flat)), "along the line");
        assert!(!aim.moved((80.0, 11.0), 0.04, Some(flat)), "off it");
    }

    #[test]
    fn triangle_geometry() {
        let (a, b, c) = ((0.0, 0.0), (10.0, 0.0), (0.0, 10.0));
        assert!(point_in_triangle((2.0, 2.0), a, b, c));
        assert!(point_in_triangle((0.0, 0.0), a, b, c));
        assert!(!point_in_triangle((8.0, 8.0), a, b, c));
        assert!(!point_in_triangle((-1.0, 1.0), a, b, c));
        assert!(
            point_in_triangle((2.0, 2.0), a, c, b),
            "orientation independent"
        );
    }
}
