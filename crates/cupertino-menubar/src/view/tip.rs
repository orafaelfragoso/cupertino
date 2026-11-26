//! The tip of a status item: Kit's tooltip popup below the item's slot, with the item's tooltip text (its title, else),
//! while the controller says the pointer rested on it ([`MenuController::tooltip`]). Kit's tip is one line, so the
//! tooltip's lines (the title, then the description) are joined by [`LINE_JOIN`].

use super::*;
use cupertino_kit::controls::Tooltip;

/// What stands between the lines of an item's tooltip text on the one line of the tip.
pub const LINE_JOIN: &str = " \u{2014} ";

/// The key of the slot `key` of a bar, which a tip is anchored below.
pub(super) fn anchor(key: &SlotKey) -> Key {
    Key::from(format!("slot {key:?}"))
}

/// The text of the tip of the status item `id`: its tooltip text on one line; `None` for an item without one, or one
/// of the bar's own.
pub fn tip_text(shared: &Shared, id: &str) -> Option<String> {
    let status = shared.status.iter().find(|s| s.item.id == id)?;
    let text = status.item.tooltip_text();
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    (!lines.is_empty()).then(|| lines.join(LINE_JOIN))
}

/// The tip's window, when the controller shows one and its item has a text.
pub(super) fn window<M: Clone + 'static>(
    scene: &BarScene<'_>,
    input: fn(Input) -> M,
) -> Option<Window<M>> {
    let slot = scene.controller.tooltip()?;
    let SlotKey::Status(id) = slot else {
        return None;
    };
    let text = tip_text(scene.shared, id)?;
    let bar = scene.bar;
    let tip = Tooltip {
        text,
        anchor: Element::new(Spacer::default()),
        state: cupertino_kit::controls::TooltipState::default(),
        // the controller times the tip from the bar's pointer: the pointer reaching the tip, or its dismissal, puts
        // it away
        on_change: Box::new(move |_| input(Input::TipDismissed(bar))),
    };
    Some(tip.window(bar.tip(), bar.window(), anchor(slot)))
}
