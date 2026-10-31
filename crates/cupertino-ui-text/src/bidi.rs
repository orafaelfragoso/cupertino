use crate::{Affinity, Direction, TextPosition};
use unicode_bidi::{BidiInfo, Level, ParagraphBidiInfo};
use unicode_segmentation::UnicodeSegmentation;

/// The base direction of `text`: `direction`, else the first strong character's, else left to right.
pub(crate) fn base_direction(text: &str, direction: Option<Direction>) -> Direction {
    direction.unwrap_or_else(|| match unicode_bidi::get_base_direction_full(text) {
        unicode_bidi::Direction::Rtl => Direction::Rtl,
        unicode_bidi::Direction::Ltr | unicode_bidi::Direction::Mixed => Direction::Ltr,
    })
}

pub(crate) fn base_level(direction: Direction) -> Level {
    match direction {
        Direction::Ltr => Level::ltr(),
        Direction::Rtl => Level::rtl(),
    }
}

/// The caret positions of the single-line `text` with the edge each sits on, counted from the left, as
/// `ParagraphLayout` places them: downstream at the start edge of the grapheme after it (the end edge of the last at the
/// end), upstream at the end edge of the grapheme before it where that is another edge.
pub(crate) fn caret_edges(text: &str, direction: Option<Direction>) -> Vec<(TextPosition, usize)> {
    let base = base_level(base_direction(text, direction));
    let graphemes: Vec<(usize, &str)> = text.grapheme_indices(true).collect();
    let Some(last) = graphemes.len().checked_sub(1) else {
        return vec![(TextPosition::default(), 0)];
    };
    let bidi = ParagraphBidiInfo::new(text, Some(base));
    // UAX 9 L1: the spaces that end the line take the paragraph's level
    let visible = graphemes
        .iter()
        .rposition(|(_, grapheme)| !grapheme.chars().all(char::is_whitespace))
        .map_or(0, |index| index + 1);
    let levels: Vec<Level> = graphemes
        .iter()
        .enumerate()
        .map(|(index, (start, _))| {
            if index < visible {
                bidi.levels[*start]
            } else {
                base
            }
        })
        .collect();
    let mut place = vec![0; graphemes.len()];
    for (visual, logical) in BidiInfo::reorder_visual(&levels).into_iter().enumerate() {
        place[logical] = visual;
    }
    let start_edge = |index: usize| place[index] + usize::from(levels[index].is_rtl());
    let end_edge = |index: usize| place[index] + usize::from(!levels[index].is_rtl());
    let mut edges = Vec::with_capacity(graphemes.len() * 2 + 1);
    for (index, (start, _)) in graphemes.iter().enumerate() {
        let downstream = start_edge(index);
        if index > 0 && end_edge(index - 1) != downstream {
            edges.push((upstream(*start), end_edge(index - 1)));
        }
        edges.push((downstream_at(*start), downstream));
    }
    edges.push((downstream_at(text.len()), end_edge(last)));
    edges
}

fn downstream_at(byte: usize) -> TextPosition {
    TextPosition {
        byte,
        affinity: Affinity::Downstream,
    }
}

fn upstream(byte: usize) -> TextPosition {
    TextPosition {
        byte,
        affinity: Affinity::Upstream,
    }
}

/// The position whose caret is the nearest one right (or left) of the caret at `at`; `None` at that edge of the line.
pub(crate) fn visual_neighbour(
    edges: &[(TextPosition, usize)],
    at: TextPosition,
    rightward: bool,
) -> Option<TextPosition> {
    let from = edge_at(edges, at);
    let beyond = edges.iter().filter(|(_, edge)| {
        if rightward {
            *edge > from
        } else {
            *edge < from
        }
    });
    if rightward {
        beyond.min_by_key(|(_, edge)| *edge)
    } else {
        beyond.max_by_key(|(_, edge)| *edge)
    }
    .map(|(position, _)| *position)
}

/// The edge of the caret at `at`; an upstream position with no edge of its own sits where the downstream one does.
pub(crate) fn edge_at(edges: &[(TextPosition, usize)], at: TextPosition) -> usize {
    edges
        .iter()
        .find(|(position, _)| *position == at)
        .or_else(|| edges.iter().find(|(position, _)| position.byte >= at.byte))
        .or(edges.last())
        .map_or(0, |(_, edge)| *edge)
}
