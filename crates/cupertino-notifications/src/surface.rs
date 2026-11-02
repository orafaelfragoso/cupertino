//! Geometry of the single layer surface that carries the banners and the notification centre
//! list, and the routing of presses on them. Pure functions: the `cupertino-notifications`
//! binary feeds them the model's placements and the pointer position.
//!
//! The surface is anchored to the top, right and bottom edges of the output. The banner column
//! sits at its right edge ([`StackLayout::right_margin`] from it); the room left of the column
//! holds the leftward swipe and the shadow.

use crate::center::{StackLayout, SwipeConfig};
use crate::render::BannerHit;
use crate::stack::GroupStack;
use cupertino_geometry::{Point, Rect, Size};

/// Space kept free below the last card of the list, in points (approximate).
pub const CENTRE_BOTTOM_MARGIN: f64 = 12.0;

/// Width of the surface in points: the banner column with its right margin, plus room on the
/// left for the leftward drag ([`SwipeConfig::left_limit`]) or the shadow, whichever is wider.
pub fn surface_width(layout: &StackLayout, swipe: &SwipeConfig, shadow_left: f64) -> f64 {
    layout.width + layout.right_margin + (-swipe.left_limit).max(shadow_left).ceil()
}

/// Title of the centre header (text, spelled as in the project glossary).
pub const CENTRE_TITLE: &str = "Notification Centre";
/// Title size, bold: 22 pt.
pub const CENTRE_TITLE_SIZE: f64 = 22.0;
/// Label of the Clear menu button (text).
pub const CLEAR_LABEL: &str = "Clear";
/// Height of the header row, which holds the 22 pt title and the 24 pt Clear button (approximate; the value is not known).
pub const CENTRE_HEADER_HEIGHT: f64 = 36.0;
/// Clear button: height 24 and 12 pt label padding like the banner action buttons (approximate role, values).
pub const CLEAR_BUTTON_HEIGHT: f64 = 24.0;
pub const CLEAR_BUTTON_PADDING: f64 = 12.0;
/// The Clear button sits as far inside the header's right edge as from its top and bottom: 6 pt, concentric with
/// the panel's corner.
pub const CLEAR_BUTTON_INSET: f64 = (CENTRE_HEADER_HEIGHT - CLEAR_BUTTON_HEIGHT) / 2.0;
/// Clear menu metrics: item height 22, edge padding 5, minimum width 150, radius 6 (section 1).
pub const MENU_ITEM_HEIGHT: f64 = 22.0;
pub const MENU_EDGE_PADDING: f64 = 5.0;
pub const MENU_MIN_WIDTH: f64 = 150.0;
pub const MENU_RADIUS: f64 = 6.0;
/// Horizontal padding of a menu label inside its item (approximate).
pub const MENU_LABEL_PADDING: f64 = 12.0;

/// Header row of the open list, a panel as wide as the cards: the title at the card padding from its left edge, the
/// Clear button [`CLEAR_BUTTON_INSET`] inside its right edge.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CentreHeader {
    /// Row rectangle.
    pub row: Rect,
    /// Clear button rectangle, vertically centred in the row.
    pub clear: Rect,
}

/// Header of the list for a surface, `clear_width` being the measured width of the button.
pub fn centre_header(layout: &StackLayout, surface: Size, clear_width: f64) -> CentreHeader {
    let x = surface.width - layout.right_margin - layout.width;
    let row = Rect::new(x, layout.top_inset, layout.width, CENTRE_HEADER_HEIGHT);
    let clear = Rect::new(
        row.max_x() - CLEAR_BUTTON_INSET - clear_width,
        row.y() + CLEAR_BUTTON_INSET,
        clear_width,
        CLEAR_BUTTON_HEIGHT,
    );
    CentreHeader { row, clear }
}

/// The open Clear menu: the panel below the button, right-aligned to it, and one row per entry.
#[derive(Clone, Debug, PartialEq)]
pub struct ClearMenu {
    pub panel: Rect,
    pub rows: Vec<Rect>,
}

/// Menu for `entries` rows whose widest label measures `widest`.
pub fn clear_menu(clear: Rect, entries: usize, widest: f64) -> ClearMenu {
    let width = (widest + 2.0 * MENU_LABEL_PADDING + 2.0 * MENU_EDGE_PADDING).max(MENU_MIN_WIDTH);
    let height = entries as f64 * MENU_ITEM_HEIGHT + 2.0 * MENU_EDGE_PADDING;
    let panel = Rect::new(clear.max_x() - width, clear.max_y() + 4.0, width, height);
    let rows = (0..entries)
        .map(|i| {
            Rect::new(
                panel.x() + MENU_EDGE_PADDING,
                panel.y() + MENU_EDGE_PADDING + i as f64 * MENU_ITEM_HEIGHT,
                width - 2.0 * MENU_EDGE_PADDING,
                MENU_ITEM_HEIGHT,
            )
        })
        .collect();
    ClearMenu { panel, rows }
}

/// What a press does while the list is open and the Clear menu is closed or open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeaderPress {
    /// Open the menu (or close it when open).
    ToggleMenu,
    /// Run the scope at this index of the offered scopes, then close the menu.
    Choose(usize),
    /// A press outside the menu while it is open: only the menu closes.
    CloseMenu,
    /// Not on the header or the menu.
    Elsewhere,
}

/// Route a press at `p`. While the menu is open it takes every press.
pub fn header_press(header: &CentreHeader, menu: Option<&ClearMenu>, p: Point) -> HeaderPress {
    if let Some(m) = menu {
        if let Some(i) = m.rows.iter().position(|r| r.contains(p)) {
            return HeaderPress::Choose(i);
        }
        return if header.clear.contains(p) {
            HeaderPress::ToggleMenu
        } else {
            HeaderPress::CloseMenu
        };
    }
    if header.clear.contains(p) {
        HeaderPress::ToggleMenu
    } else {
        HeaderPress::Elsewhere
    }
}

/// Cards of the notification centre list in surface coordinates, newest first. `rows` yields
/// `(id, front card height, notifications in the row) lazily: 1 for an expanded card, the group
/// size for a collapsed group, whose cards behind come from [`GroupStack::cards`]. Rows follow
/// each other with no spacing, each [`GroupStack::height`] tall: the card,
/// 8 pt per stacked row and the 6 pt bottom padding of its last visible card (the 6 pt gap between cards and groups). Rows are placed below the
/// header panel, the same gap apart, until the next one's visible cards would pass the bottom margin.
pub fn centre_cards(
    rows: impl IntoIterator<Item = (u32, f64, usize)>,
    stack: &GroupStack,
    layout: &StackLayout,
    surface: Size,
) -> Vec<(u32, Rect)> {
    let x = surface.width - layout.right_margin - layout.width;
    let mut y = layout.top_inset + CENTRE_HEADER_HEIGHT + stack.bottom_padding;
    let mut cards = Vec::new();
    for (id, height, count) in rows {
        if y + stack.visible_height(height, count) > surface.height - CENTRE_BOTTOM_MARGIN {
            break;
        }
        cards.push((id, Rect::new(x, y, layout.width, height)));
        y += stack.height(height, count);
    }
    cards
}

/// Where the surface takes pointer input: only the banners, so clicks elsewhere reach the
/// windows below; while the list is open the whole surface, so a press outside the cards closes it.
pub fn input_region(banners: &[Rect], centre_open: bool, surface: Size) -> Vec<Rect> {
    if centre_open {
        vec![Rect::from_origin_size(Point::ZERO, surface)]
    } else {
        banners.to_vec()
    }
}

/// The card under `p`, if any.
pub fn card_at(cards: &[(u32, Rect)], p: Point) -> Option<(u32, Rect)> {
    cards.iter().find(|(_, r)| r.contains(p)).copied()
}

/// `p` relative to the top-left corner of `rect`, the coordinates of `BannerLayout::hit`.
pub fn local_to(rect: Rect, p: Point) -> Point {
    Point::new(p.x - rect.x(), p.y - rect.y())
}

/// What a press on a banner does. Buttons and links act on press; a press on the body starts a
/// potential swipe and acts on release.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BannerPress {
    Dismiss,
    Invoke(String),
    OpenLink(String),
    BeginSwipe,
}

pub fn banner_press(hit: BannerHit) -> BannerPress {
    match hit {
        BannerHit::Close => BannerPress::Dismiss,
        BannerHit::Button { key, .. } => BannerPress::Invoke(key),
        BannerHit::Link(href) => BannerPress::OpenLink(href),
        BannerHit::Body | BannerHit::None => BannerPress::BeginSwipe,
    }
}

/// What a press on a card of the list does; every press acts at once.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CardPress {
    /// Remove the card from the notification centre.
    Clear,
    Invoke(String),
    OpenLink(String),
    /// Run the default action and close the list.
    Activate,
}

pub fn card_press(hit: BannerHit) -> CardPress {
    match hit {
        BannerHit::Close => CardPress::Clear,
        BannerHit::Button { key, .. } => CardPress::Invoke(key),
        BannerHit::Link(href) => CardPress::OpenLink(href),
        BannerHit::Body | BannerHit::None => CardPress::Activate,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn surface_is_column_plus_margin_plus_the_wider_left_slack() {
        let (layout, swipe) = (StackLayout::default(), SwipeConfig::default());
        // 344 + 16 + max(16, 10) and 344 + 16 + max(16, 20.2 -> 21)
        assert_eq!(surface_width(&layout, &swipe, 10.0), 376.0);
        assert_eq!(surface_width(&layout, &swipe, 20.2), 381.0);
    }

    #[test]
    fn cards_stack_from_the_top_inset_until_the_bottom_margin() {
        let (layout, stack) = (StackLayout::default(), GroupStack::default());
        let cards = centre_cards(
            [(7, 100.0, 1), (6, 100.0, 1), (5, 100.0, 1)],
            &stack,
            &layout,
            Size::new(376.0, 330.0),
        );
        // 82 + 100 = 182; next 188 + 100 = 288 <= 318; third 294 + 100 = 394 > 318
        assert_eq!(
            cards,
            vec![
                (7, Rect::new(16.0, 82.0, 344.0, 100.0)),
                (6, Rect::new(16.0, 188.0, 344.0, 100.0))
            ]
        );
    }

    #[test]
    fn expanded_cards_are_6_pt_apart() {
        let (layout, stack) = (StackLayout::default(), GroupStack::default());
        let cards = centre_cards(
            [(2, 60.0, 1), (1, 80.0, 1)],
            &stack,
            &layout,
            Size::new(376.0, 800.0),
        );
        assert_eq!(
            cards,
            vec![
                (2, Rect::new(16.0, 82.0, 344.0, 60.0)),
                (1, Rect::new(16.0, 148.0, 344.0, 80.0))
            ]
        );
    }

    #[test]
    fn a_collapsed_group_takes_8_pt_per_stacked_row_plus_the_6_pt_padding() {
        let (layout, stack) = (StackLayout::default(), GroupStack::default());
        let cards = centre_cards(
            [(3, 86.0, 3), (1, 60.0, 1)],
            &stack,
            &layout,
            Size::new(376.0, 800.0),
        );
        // the group: 86 + 2 · 8 + 6 = 108 tall
        assert_eq!(
            cards,
            vec![
                (3, Rect::new(16.0, 82.0, 344.0, 86.0)),
                (1, Rect::new(16.0, 190.0, 344.0, 60.0))
            ]
        );
        // its stacked rows end 82 + 86 + 16 = 184, inside the 800 − 12 limit; at a 195 pt surface they do not fit
        assert_eq!(
            centre_cards([(3, 86.0, 3)], &stack, &layout, Size::new(376.0, 195.0)),
            Vec::new()
        );
        assert_eq!(
            centre_cards([(3, 86.0, 3)], &stack, &layout, Size::new(376.0, 196.0)).len(),
            1
        );
    }

    #[test]
    fn cards_are_measured_lazily() {
        let (layout, stack) = (StackLayout::default(), GroupStack::default());
        let mut measured = Vec::new();
        let rows = (1..=5).map(|id| {
            measured.push(id);
            (id, 100.0, 1)
        });
        centre_cards(rows, &stack, &layout, Size::new(376.0, 330.0));
        assert_eq!(measured, vec![1, 2, 3]);
    }

    #[test]
    fn input_covers_banners_only_until_the_list_opens() {
        let banners = [Rect::new(16.0, 32.0, 344.0, 72.0)];
        let surface = Size::new(372.0, 800.0);
        assert_eq!(input_region(&banners, false, surface), banners.to_vec());
        assert_eq!(input_region(&[], false, surface), Vec::<Rect>::new());
        assert_eq!(
            input_region(&banners, true, surface),
            vec![Rect::new(0.0, 0.0, 372.0, 800.0)]
        );
    }

    #[test]
    fn card_lookup_uses_half_open_rectangles() {
        let cards = [
            (1, Rect::new(16.0, 32.0, 344.0, 100.0)),
            (2, Rect::new(16.0, 140.0, 344.0, 100.0)),
        ];
        assert_eq!(
            card_at(&cards, Point::new(16.0, 32.0)).map(|c| c.0),
            Some(1)
        );
        assert_eq!(card_at(&cards, Point::new(100.0, 132.0)), None);
        assert_eq!(
            card_at(&cards, Point::new(100.0, 150.0)).map(|c| c.0),
            Some(2)
        );
        assert_eq!(
            local_to(cards[1].1, Point::new(26.0, 150.0)),
            Point::new(10.0, 10.0)
        );
    }

    #[test]
    fn banner_presses_act_on_buttons_and_start_swipes_on_the_body() {
        assert_eq!(banner_press(BannerHit::Close), BannerPress::Dismiss);
        assert_eq!(
            banner_press(BannerHit::Button {
                index: 1,
                key: "reply".into()
            }),
            BannerPress::Invoke("reply".into())
        );
        assert_eq!(
            banner_press(BannerHit::Link("https://example.org".into())),
            BannerPress::OpenLink("https://example.org".into())
        );
        assert_eq!(banner_press(BannerHit::Body), BannerPress::BeginSwipe);
        assert_eq!(banner_press(BannerHit::None), BannerPress::BeginSwipe);
    }

    #[test]
    fn card_presses_clear_invoke_or_activate() {
        assert_eq!(card_press(BannerHit::Close), CardPress::Clear);
        assert_eq!(
            card_press(BannerHit::Button {
                index: 0,
                key: "open".into()
            }),
            CardPress::Invoke("open".into())
        );
        assert_eq!(card_press(BannerHit::Body), CardPress::Activate);
    }

    #[test]
    fn the_clear_button_sits_6_pt_inside_the_header_and_its_menu_drops_below_it() {
        let header = centre_header(&StackLayout::default(), Size::new(376.0, 800.0), 60.0);
        assert_eq!(header.row, Rect::new(16.0, 40.0, 344.0, 36.0));
        assert_eq!(header.clear, Rect::new(294.0, 46.0, 60.0, 24.0));
        let menu = clear_menu(header.clear, 2, 120.0);
        // 120 + 24 + 10 = 154 wide, 2 * 22 + 10 = 54 high, right edges aligned
        assert_eq!(menu.panel, Rect::new(200.0, 74.0, 154.0, 54.0));
        assert_eq!(
            menu.rows,
            vec![
                Rect::new(205.0, 79.0, 144.0, 22.0),
                Rect::new(205.0, 101.0, 144.0, 22.0)
            ]
        );
    }

    #[test]
    fn an_open_menu_takes_every_press() {
        let header = centre_header(&StackLayout::default(), Size::new(376.0, 800.0), 60.0);
        let menu = clear_menu(header.clear, 2, 120.0);
        let on_button = Point::new(330.0, 58.0);
        assert_eq!(
            header_press(&header, None, on_button),
            HeaderPress::ToggleMenu
        );
        assert_eq!(
            header_press(&header, None, Point::new(100.0, 200.0)),
            HeaderPress::Elsewhere
        );
        assert_eq!(
            header_press(&header, Some(&menu), on_button),
            HeaderPress::ToggleMenu
        );
        assert_eq!(
            header_press(&header, Some(&menu), Point::new(250.0, 110.0)),
            HeaderPress::Choose(1)
        );
        assert_eq!(
            header_press(&header, Some(&menu), Point::new(100.0, 200.0)),
            HeaderPress::CloseMenu
        );
    }
}
