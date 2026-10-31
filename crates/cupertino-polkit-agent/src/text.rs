//! Every string the prompt shows comes from another program (the action's message, its details, PAM's prompts), so
//! it is bounded and stripped of what could disguise it before it reaches the screen.

/// The most characters of an untrusted string the prompt shows; longer ones end in an ellipsis.
pub const MAX_CHARS: usize = 256;

/// `text` without control characters (line breaks and tabs become spaces) and without the bidirectional formatting
/// characters that could reorder what is shown, cut to [`MAX_CHARS`].
pub fn shown(text: &str) -> String {
    let mut out = String::new();
    for (count, c) in text
        .chars()
        .filter(|c| !bidi_control(*c))
        .map(|c| if c.is_control() { ' ' } else { c })
        .enumerate()
    {
        if count == MAX_CHARS {
            out.pop();
            out.push('…');
            break;
        }
        out.push(c);
    }
    out
}

fn bidi_control(c: char) -> bool {
    matches!(
        c,
        '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
    )
}

#[cfg(test)]
mod tests;
