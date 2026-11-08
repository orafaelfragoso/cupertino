//! Spring-loaded folders: a drag held over a folder opens it in the same window, so a drop can go further down.
//!
//! * The drag dwells on one folder for [`SPRING_DELAY`]; the folder flashes for the last [`SPRING_FLASH`] of it and
//!   then opens. Moving onto another folder starts the wait again; moving off every folder stops it.
//! * The window remembers what it showed when the drag first opened a folder. The drag leaving the
//!   window, or being cancelled, takes it back there; a drop keeps the folder it opened.
//!
//! The model is driven by timers it asks for ([`Timer`]): each carries the generation of the dwell that asked, and a
//! timer of a dwell that has ended since does nothing.

use std::time::Duration;

/// How long a drag holds still over a folder before the folder opens, in seconds.
pub const SPRING_DELAY: f64 = 0.668;
/// How long the folder flashes just before it opens, in seconds: the end of [`SPRING_DELAY`].
pub const SPRING_FLASH: f64 = 0.1;

/// What a timer of the model does when it is due.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// The folder starts flashing.
    Flash,
    /// The folder opens.
    Open,
}

/// A timer the model asks for: call [`SpringLoad::due`] with `generation` and `step` once `after` has passed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timer {
    pub generation: u64,
    pub step: Step,
    pub after: Duration,
}

/// What a due timer did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Sprung<T> {
    /// The folder `T` flashes; the next timer opens it.
    Flash(T, Timer),
    /// Open the folder `T`.
    Open(T),
}

/// The dwell of a drag over the folders of one window.
#[derive(Clone, Debug)]
pub struct SpringLoad<T: PartialEq + Clone> {
    /// The folder the drag dwells on.
    target: Option<T>,
    /// Counts the dwells, so a timer of an earlier one is told apart.
    generation: u64,
    /// The folder flashing now.
    flashing: Option<T>,
}

impl<T: PartialEq + Clone> Default for SpringLoad<T> {
    fn default() -> Self {
        SpringLoad {
            target: None,
            generation: 0,
            flashing: None,
        }
    }
}

fn seconds(s: f64) -> Duration {
    Duration::from_secs_f64(s.max(0.0))
}

impl<T: PartialEq + Clone> SpringLoad<T> {
    /// The drag is over folder `target`, or over no folder. A new folder starts the wait and asks for its first timer;
    /// the folder already waited on changes nothing.
    pub fn hover(&mut self, target: Option<T>) -> Option<Timer> {
        if target.is_some() && target == self.target {
            return None;
        }
        self.generation += 1;
        self.flashing = None;
        self.target = target;
        self.target.as_ref()?;
        Some(Timer {
            generation: self.generation,
            step: Step::Flash,
            after: seconds(SPRING_DELAY - SPRING_FLASH),
        })
    }

    /// The drag ended or left: nothing waits any more.
    pub fn cancel(&mut self) {
        self.hover(None);
    }

    /// The timer of `generation` for `step` is due.
    pub fn due(&mut self, generation: u64, step: Step) -> Option<Sprung<T>> {
        if generation != self.generation {
            return None;
        }
        let target = self.target.clone()?;
        match step {
            Step::Flash => {
                self.flashing = Some(target.clone());
                Some(Sprung::Flash(
                    target,
                    Timer {
                        generation,
                        step: Step::Open,
                        after: seconds(SPRING_FLASH),
                    },
                ))
            }
            Step::Open => {
                // the folder opened: the drag waits on whatever it is over in it next
                self.target = None;
                self.flashing = None;
                self.generation += 1;
                Some(Sprung::Open(target))
            }
        }
    }

    /// The folder flashing now, if any.
    pub fn flashing(&self) -> Option<&T> {
        self.flashing.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(s: &mut SpringLoad<u32>, timer: Timer) -> Option<Sprung<u32>> {
        s.due(timer.generation, timer.step)
    }

    #[test]
    fn a_folder_flashes_and_then_opens_once_the_delay_has_passed() {
        let mut s = SpringLoad::default();
        let flash = s.hover(Some(7)).unwrap();
        assert_eq!(flash.step, Step::Flash);
        let Some(Sprung::Flash(7, open)) = run(&mut s, flash) else {
            panic!("the flash")
        };
        assert_eq!(s.flashing(), Some(&7));
        assert_eq!(open.step, Step::Open);
        let total = flash.after + open.after;
        assert!((total.as_secs_f64() - SPRING_DELAY).abs() < 1e-9);
        assert_eq!(run(&mut s, open), Some(Sprung::Open(7)));
        assert_eq!(s.flashing(), None);
        assert_eq!(run(&mut s, open), None, "once");
    }

    #[test]
    fn holding_on_the_same_folder_asks_for_nothing_and_another_folder_starts_again() {
        let mut s = SpringLoad::default();
        let first = s.hover(Some(1)).unwrap();
        assert_eq!(s.hover(Some(1)), None);
        let second = s.hover(Some(2)).unwrap();
        assert_eq!(run(&mut s, first), None, "the first folder's timer is stale");
        assert!(matches!(run(&mut s, second), Some(Sprung::Flash(2, _))));
    }

    #[test]
    fn leaving_every_folder_or_cancelling_stops_the_wait_and_the_flash() {
        let mut s = SpringLoad::default();
        let flash = s.hover(Some(1)).unwrap();
        let Some(Sprung::Flash(_, open)) = run(&mut s, flash) else {
            panic!("the flash")
        };
        assert_eq!(s.hover(None), None);
        assert_eq!(s.flashing(), None);
        assert_eq!(run(&mut s, open), None);

        let flash = s.hover(Some(3)).unwrap();
        s.cancel();
        assert_eq!(run(&mut s, flash), None);
    }

    #[test]
    fn after_a_folder_opens_the_same_folder_starts_a_new_wait() {
        let mut s = SpringLoad::default();
        let flash = s.hover(Some(4)).unwrap();
        let Some(Sprung::Flash(_, open)) = run(&mut s, flash) else {
            panic!("the flash")
        };
        run(&mut s, open);
        // the drag rests over the folder of the same name in the folder that opened
        assert!(s.hover(Some(4)).is_some());
    }
}
