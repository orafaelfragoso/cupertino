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

/// The base direction of one hard line: `direction`, else the line's first strong character's, else `text_direction`
/// (the whole text's), so a line of digits or an empty line stays with the text around it.
pub(crate) fn line_direction(
    line: &str,
    direction: Option<Direction>,
    text_direction: Direction,
) -> Direction {
    direction.unwrap_or_else(|| match unicode_bidi::get_base_direction(line) {
        unicode_bidi::Direction::Rtl => Direction::Rtl,
        unicode_bidi::Direction::Ltr => Direction::Ltr,
        unicode_bidi::Direction::Mixed => text_direction,
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

/// One hard line of a text: its bytes without the separator, and its caret positions in the line's own direction, as
/// `caret_edges` gives them.
pub(crate) struct CaretLine {
    start: usize,
    end: usize,
    rtl: bool,
    edges: Vec<(TextPosition, usize)>,
}

/// The hard lines of `text`, each laid out in its own direction as `ParagraphLayout` does (`line_direction`).
pub(crate) fn caret_lines(text: &str, direction: Option<Direction>) -> Vec<CaretLine> {
    let text_direction = base_direction(text, direction);
    let mut lines = Vec::new();
    let mut start = 0;
    loop {
        let (end, next) = crate::system::next_line(text, start);
        let line = &text[start..end];
        let line_direction = line_direction(line, direction, text_direction);
        let edges = caret_edges(line, Some(line_direction))
            .into_iter()
            .map(|(position, edge)| {
                (
                    TextPosition {
                        byte: start + position.byte,
                        ..position
                    },
                    edge,
                )
            })
            .collect();
        lines.push(CaretLine {
            start,
            end,
            rtl: line_direction == Direction::Rtl,
            edges,
        });
        match next {
            Some(next) => start = next,
            None => return lines,
        }
    }
}

/// The index of the hard line `byte` is on.
fn line_at(lines: &[CaretLine], byte: usize) -> usize {
    lines
        .iter()
        .rposition(|line| line.start <= byte)
        .unwrap_or(0)
}

/// The caret one stop right (or left) of the caret at `at`. Past that edge of its line it goes on in the line's reading
/// order: rightward from the right edge of a left-to-right line to the start of the next, of a right-to-left line to
/// the end of the one before. `None` at the end of the text in that direction.
pub(crate) fn visual_neighbour_in(
    lines: &[CaretLine],
    at: TextPosition,
    rightward: bool,
) -> Option<TextPosition> {
    let index = line_at(lines, at.byte);
    let line = &lines[index];
    visual_neighbour(&line.edges, at, rightward).or_else(|| {
        if rightward != line.rtl {
            lines.get(index + 1).map(|next| downstream_at(next.start))
        } else {
            index
                .checked_sub(1)
                .map(|before| downstream_at(lines[before].end))
        }
    })
}

/// The line and the edge of the caret at `at`, for ordering carets on screen.
pub(crate) fn place_of(lines: &[CaretLine], at: TextPosition) -> (usize, usize) {
    let index = line_at(lines, at.byte);
    (index, edge_at(&lines[index].edges, at))
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
