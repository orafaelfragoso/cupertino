use crate::{Affinity, Direction, TextPosition};
use std::ops::Range;
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
    edges_of(&drawn(text, direction), text.len())
}

fn edges_of(drawn: &Drawn, len: usize) -> Vec<(TextPosition, usize)> {
    let Drawn {
        graphemes,
        levels,
        place,
    } = drawn;
    let Some(last) = graphemes.len().checked_sub(1) else {
        return vec![(TextPosition::default(), 0)];
    };
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
    edges.push((downstream_at(len), end_edge(last)));
    edges
}

/// The graphemes of a single-line text in logical order, the level of each and the place it is drawn at from the left.
struct Drawn<'a> {
    graphemes: Vec<(usize, &'a str)>,
    levels: Vec<Level>,
    place: Vec<usize>,
}

fn drawn(text: &str, direction: Option<Direction>) -> Drawn<'_> {
    let base = base_level(base_direction(text, direction));
    let graphemes: Vec<(usize, &str)> = text.grapheme_indices(true).collect();
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
    Drawn {
        graphemes,
        levels,
        place,
    }
}

/// Where a word move from `at` lands: past the spaces and punctuation right (or left) of its caret, then past the
/// word there to its drawn edge, so the caret only moves in that direction on screen. Past no word it goes to that end
/// of the line; `None` when it is already there. `words` are the byte ranges of the words, in logical order.
pub(crate) fn visual_word_neighbour(
    text: &str,
    direction: Option<Direction>,
    words: &[Range<usize>],
    at: TextPosition,
    rightward: bool,
) -> Option<TextPosition> {
    let drawn = drawn(text, direction);
    let edges = edges_of(&drawn, text.len());
    let Drawn {
        graphemes, place, ..
    } = drawn;
    let mut word_at_place = vec![None; graphemes.len()];
    for (index, (start, _)) in graphemes.iter().enumerate() {
        let word = words.partition_point(|word| word.end <= *start);
        word_at_place[place[index]] = words.get(word).filter(|w| w.start <= *start).map(|_| word);
    }
    let beside = |edge: usize| {
        if rightward {
            word_at_place.get(edge).copied()
        } else {
            edge.checked_sub(1).map(|slot| word_at_place[slot])
        }
    };
    let step = |edge: usize| if rightward { edge + 1 } else { edge - 1 };
    let from = edge_at(&edges, at);
    let mut edge = from;
    while beside(edge) == Some(None) {
        edge = step(edge);
    }
    let word = beside(edge).flatten();
    if let Some(word) = word {
        while beside(edge) == Some(Some(word)) {
            edge = step(edge);
        }
    }
    if edge == from {
        return None;
    }
    let landing = edges.iter().filter(|(_, at_edge)| *at_edge == edge);
    word.and_then(|word| {
        landing.clone().find(|(position, _)| {
            position.byte == words[word].start || position.byte == words[word].end
        })
    })
    .or_else(|| landing.clone().next())
    .map(|(position, _)| *position)
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

/// The hard line `at` is on and the lines on either side of it, each laid out in its own direction as
/// `ParagraphLayout` does (`line_direction`).
pub(crate) fn caret_lines_around(
    text: &str,
    direction: Option<Direction>,
    at: usize,
) -> Vec<CaretLine> {
    let text_direction = base_direction(text, direction);
    let (line, separator) = line_start(text, at);
    let mut start = separator.map_or(line, |separator| line_start(text, separator).0);
    let mut lines = Vec::with_capacity(3);
    loop {
        let (end, next) = crate::system::next_line(text, start);
        lines.push(caret_line(text, start..end, direction, text_direction));
        match next {
            Some(next) if start <= line => start = next,
            _ => return lines,
        }
    }
}

/// The start of the hard line `at` is on, and where the separator before it starts.
fn line_start(text: &str, at: usize) -> (usize, Option<usize>) {
    let Some(last) = text[..at].rfind(crate::system::LINE_SEPARATORS) else {
        return (0, None);
    };
    let separator = if text[..last].ends_with('\r') && text[last..].starts_with('\n') {
        last - 1
    } else {
        last
    };
    let start = last + text[last..].chars().next().map_or(0, char::len_utf8);
    (start, Some(separator))
}

fn caret_line(
    text: &str,
    bytes: Range<usize>,
    direction: Option<Direction>,
    text_direction: Direction,
) -> CaretLine {
    let line = &text[bytes.clone()];
    let line_direction = line_direction(line, direction, text_direction);
    let edges = caret_edges(line, Some(line_direction))
        .into_iter()
        .map(|(position, edge)| {
            (
                TextPosition {
                    byte: bytes.start + position.byte,
                    ..position
                },
                edge,
            )
        })
        .collect();
    CaretLine {
        start: bytes.start,
        end: bytes.end,
        rtl: line_direction == Direction::Rtl,
        edges,
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
    visual_neighbour(&lines[index].edges, at, rightward)
        .or_else(|| past_line(lines, index, rightward))
}

/// [`visual_word_neighbour`] within the hard line of `at`, in that line's direction, with the `words` of that line;
/// past that edge of the line it goes on as [`visual_neighbour_in`] does.
pub(crate) fn visual_word_neighbour_in(
    text: &str,
    lines: &[CaretLine],
    words: impl FnOnce(&str) -> Vec<Range<usize>>,
    at: TextPosition,
    rightward: bool,
) -> Option<TextPosition> {
    let index = line_at(lines, at.byte);
    let line = &lines[index];
    let words = words(&text[line.start..line.end]);
    let direction = if line.rtl {
        Direction::Rtl
    } else {
        Direction::Ltr
    };
    let at = TextPosition {
        byte: at.byte - line.start,
        ..at
    };
    visual_word_neighbour(
        &text[line.start..line.end],
        Some(direction),
        &words,
        at,
        rightward,
    )
    .map(|to| TextPosition {
        byte: line.start + to.byte,
        ..to
    })
    .or_else(|| past_line(lines, index, rightward))
}

/// The caret past the right (or left) edge of line `index`, in reading order: the start of the next line when that
/// edge is the line's end, else the end of the line before.
fn past_line(lines: &[CaretLine], index: usize, rightward: bool) -> Option<TextPosition> {
    if rightward != lines[index].rtl {
        lines.get(index + 1).map(|next| downstream_at(next.start))
    } else {
        index
            .checked_sub(1)
            .map(|before| downstream_at(lines[before].end))
    }
}

/// The start of the hard line of `at` and the edge of its caret there, for ordering carets on screen.
pub(crate) fn place_of(
    text: &str,
    direction: Option<Direction>,
    at: TextPosition,
) -> (usize, usize) {
    let (start, _) = line_start(text, at.byte);
    let (end, _) = crate::system::next_line(text, start);
    let line = caret_line(text, start..end, direction, base_direction(text, direction));
    (start, edge_at(&line.edges, at))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lines_around_a_caret_are_its_own_and_the_ones_on_either_side() {
        let text = "ab\r\ncd\u{2028}ef\ngh";
        let around = |at| {
            caret_lines_around(text, None, at)
                .iter()
                .map(|line| line.start..line.end)
                .collect::<Vec<_>>()
        };
        assert_eq!(around(4), [0..2, 4..6, 9..11]);
        assert_eq!(around(9), [4..6, 9..11, 12..14]);
        assert_eq!(around(0), [0..2, 4..6]);
        assert_eq!(around(14), [9..11, 12..14]);
    }
}
