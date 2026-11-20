//! What each screen's pictures are made from ([`Wants`]), from the same values its view is built from.

use cupertino_kit::geometry::{Rect, Size};

use super::{clock_parts, layout_of, name_box, strip_of, Metrics, PickerScreen, Screen, Wants};
use crate::lock_notices::{self, Notice};

impl Screen<'_> {
    /// What the screen's pictures are made from at the device `scale` of its output, for [`ScreenPictures::want`](super::ScreenPictures::want):
    /// its wallpaper and dimming, the material of the password field, of the help button and of whichever of the back
    /// button, the name field and the notification cards it shows, and the glass letters of its time.
    pub fn wants(&self, scale: f64) -> Wants {
        let layout = layout_of(self.size, self.metrics);
        let mut strips = vec![strip_of(layout.pill), strip_of(layout.help_button)];
        if self.back {
            strips.push(strip_of(layout.back_button));
        }
        if self.name.is_some() {
            strips.push(strip_of(name_box(&layout)));
        }
        Wants {
            size: self.size,
            scale,
            source: self.wallpaper.clone(),
            dim: self.dim,
            strips,
            notices: notices_strip(self.notices, self.size, self.metrics),
            glass: self
                .large_clock
                .then(|| clock_parts(self.clock, &layout, self.metrics).1),
        }
    }
}

impl PickerScreen<'_> {
    /// What the picker's pictures are made from at the device `scale` of its output, for [`ScreenPictures::want`](super::ScreenPictures::want):
    /// its wallpaper and dimming, the glass letters of its time, and the material of the password screen's field, name
    /// field and buttons, made while the picker rests so the password screen has it from its first frame.
    pub fn wants(&self, scale: f64) -> Wants {
        let layout = layout_of(self.size, self.metrics);
        Wants {
            size: self.size,
            scale,
            source: self.wallpaper.clone(),
            dim: self.dim,
            strips: vec![
                strip_of(layout.pill),
                strip_of(layout.help_button),
                strip_of(layout.back_button),
                strip_of(name_box(&layout)),
            ],
            notices: None,
            glass: self
                .large_clock
                .then(|| clock_parts(self.clock, &layout, self.metrics).1),
        }
    }
}

/// The strip of the screen the notification cards' material covers: the room the shown cards may take at most, made
/// once for all of them; `None` without notifications.
pub(super) fn notices_strip(notices: &[Notice], size: Size, metrics: Metrics) -> Option<Rect> {
    if notices.is_empty() {
        return None;
    }
    let (cards, _) = lock_notices::shown(notices);
    let column = lock_notices::column(size.width, size.height);
    let tallest =
        lock_notices::card_height(lock_notices::BODY_LINES, metrics.ascent, metrics.descent)
            + lock_notices::CARD_GAP;
    Some(Rect::new(
        column.x(),
        column.y(),
        column.width(),
        (tallest * cards.len() as f64).min(column.height()),
    ))
}
