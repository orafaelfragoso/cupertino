//! Geometry of a collapsed notification group in the notification centre: the newest card in
//! front, older cards peeking out below it, each narrower than the one in front.
//!
//! Values are as designed: card i behind the front one is scaled by
//! `(344 − 20·min(i, 3)) / 344` about its bottom edge, peeks out of an 8 pt frame and the group
//! has 6 pt of bottom padding. The roles of these values are. The arrangement is as designed
//! each card behind is a row 8 pt high, one peek below the card in front, and
//! at most 2 such rows are shown. Their height (the front card's) is approximate. Each card
//! behind is also darkened by `0.03·min(i, 3).

use cupertino_geometry::Rect;

#[derive(Clone, Debug, PartialEq)]
pub struct GroupStack {
    /// Width the scale is computed against: 344 pt.
    pub width: f64,
    /// Width lost per card behind the front one: 20 pt (10 pt per side).
    pub width_step: f64,
    /// Brightness lost per card behind the front one: 0.03 (role as a
    /// brightness amount).
    pub brightness_step: f64,
    /// Cards behind the front one, at most: 2.
    pub max_behind: usize,
    /// Depth the scale and brightness steps stop at: 3. Never reached, as at
    /// most [`Self::max_behind`] cards are shown behind.
    pub max_step: usize,
    /// Height of the visible part of each card behind: 8 pt.
    pub peek: f64,
    /// Space below the group: 6 pt.
    pub bottom_padding: f64,
}

impl Default for GroupStack {
    fn default() -> Self {
        Self { width: 344.0, width_step: 20.0, brightness_step: 0.03, max_behind: 2, max_step: 3, peek: 8.0, bottom_padding: 6.0 }
    }
}

/// A card behind the front one.
#[derive(Clone, Debug, PartialEq)]
pub struct StackCard {
    /// 1 for the card directly behind the front one; draw higher indices first.
    pub depth: usize,
    pub scale: f64,
    /// Brightness amount applied to the card: `−0.03·min(depth, 3)`.
    pub brightness: f64,
    /// Scaled frame of the card in the coordinates of `front`.
    pub rect: Rect,
}

impl GroupStack {
    /// Scale of the card at `depth` (0 = the front card).
    pub fn scale(&self, depth: usize) -> f64 {
        (self.width - self.width_step * depth.min(self.max_step) as f64) / self.width
    }

    /// Brightness amount of the card at `depth` (0 = the front card).
    pub fn brightness(&self, depth: usize) -> f64 {
        -self.brightness_step * depth.min(self.max_step) as f64
    }

    /// Cards drawn behind `front` for a group of `count` notifications, nearest first.
    pub fn cards(&self, front: Rect, count: usize) -> Vec<StackCard> {
        let behind = count.saturating_sub(1).min(self.max_behind);
        (1..=behind)
            .map(|depth| {
                let scale = self.scale(depth);
                let (w, h) = (front.width() * scale, front.height() * scale);
                let bottom = front.max_y() + self.peek * depth as f64;
                StackCard { depth, scale, brightness: self.brightness(depth), rect: Rect::new(front.x() + (front.width() - w) / 2.0, bottom - h, w, h) }
            })
            .collect()
    }

    /// Height of the whole group, peeking cards and bottom padding included.
    pub fn height(&self, front_height: f64, count: usize) -> f64 {
        self.visible_height(front_height, count) + self.bottom_padding
    }

    /// Height of the drawn cards of the group: the front card and its peeking cards.
    pub fn visible_height(&self, front_height: f64, count: usize) -> f64 {
        front_height + self.peek * count.saturating_sub(1).min(self.max_behind) as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cards_behind_shrink_by_20_pt_and_peek_8_pt_below_each_other() {
        let s = GroupStack::default();
        let front = Rect::new(0.0, 0.0, 344.0, 86.0);
        let cards = s.cards(front, 6);
        let scales: Vec<f64> = cards.iter().map(|c| c.scale).collect();
        assert_eq!(scales, [324.0 / 344.0, 304.0 / 344.0], "at most two behind");
        let r = cards[0].rect;
        assert_eq!((r.x(), r.width(), r.max_y()), (10.0, 324.0, 94.0));
        assert!((r.height() - 81.0).abs() < 1e-9, "86 · 324/344");
        assert_eq!((cards[1].rect.x(), cards[1].rect.width(), cards[1].rect.max_y()), (20.0, 304.0, 102.0));
        let brightness: Vec<f64> = cards.iter().map(|c| c.brightness).collect();
        assert_eq!(brightness, [-0.03, -0.06]);
    }

    #[test]
    fn a_collapsed_group_of_five_shows_two_rows_behind() {
        let s = GroupStack::default();
        assert_eq!(s.visible_height(86.0, 5), 86.0 + 16.0);
        assert_eq!(s.height(86.0, 5), 86.0 + 16.0 + 6.0);
        assert_eq!(s.height(86.0, 2), 86.0 + 8.0 + 6.0);
    }

    #[test]
    fn a_single_notification_has_no_cards_behind() {
        let s = GroupStack::default();
        assert!(s.cards(Rect::new(0.0, 0.0, 344.0, 60.0), 1).is_empty());
        assert_eq!(s.height(60.0, 1), 66.0);
        assert_eq!(s.scale(7), 284.0 / 344.0, "the formula stops at depth 3");
        assert_eq!((s.brightness(0), s.brightness(7)), (0.0, -0.09));
    }
}
