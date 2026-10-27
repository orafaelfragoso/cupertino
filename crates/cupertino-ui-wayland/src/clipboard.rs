//! Text on the clipboard.

use cupertino_wayland_client::{ClipboardOffer, Connection, Error};

/// The MIME types of text on the clipboard, in order of preference.
pub const TEXT_TYPES: [&str; 3] = ["text/plain;charset=utf-8", "text/plain", "UTF8_STRING"];

/// Put `text` on the clipboard under every text type, as the answer to the latest input event.
pub fn set_text(conn: &mut Connection, text: &str) -> Result<(), Error> {
    let contents = TEXT_TYPES
        .iter()
        .map(|mime| (mime.to_string(), text.as_bytes().to_vec()))
        .collect();
    conn.set_selection(contents)
}

/// The text of `offer`, `None` when it holds no text, was replaced while reading or its client did not answer in
/// time.
pub fn text(conn: &mut Connection, offer: &ClipboardOffer) -> Result<Option<String>, Error> {
    let Some(mime) = TEXT_TYPES
        .iter()
        .find(|t| offer.mime_types().iter().any(|m| m == *t))
    else {
        return Ok(None);
    };
    match conn.receive(offer, mime) {
        Ok(bytes) => Ok(Some(text_of(&bytes))),
        Err(Error::Cancelled) => Ok(None),
        Err(e) => Err(e),
    }
}

/// The clipboard as one client sees it: the selection the server reported last, or the text this client put there until
/// the server reports a selection again.
#[derive(Default)]
pub struct Clipboard {
    offer: Option<ClipboardOffer>,
    own: Option<String>,
}

impl Clipboard {
    /// The server reported another selection, or none (`Event::Selection`).
    pub fn selection_changed(&mut self, offer: Option<ClipboardOffer>) {
        self.offer = offer;
        self.own = None;
    }

    /// Put `text` on the clipboard, as [`set_text`] does.
    pub fn set_text(&mut self, conn: &mut Connection, text: String) -> Result<(), Error> {
        set_text(conn, &text)?;
        self.own = Some(text);
        Ok(())
    }

    /// The clipboard's text for a paste, as [`text`] reads it; `None` when it holds none.
    pub fn text(&self, conn: &mut Connection) -> Result<Option<String>, Error> {
        match &self.offer {
            Some(offer) => text(conn, offer),
            None => Ok(self.own.clone()),
        }
    }
}

/// `bytes` as text: invalid sequences replaced, a character cut by the read limit dropped.
fn text_of(bytes: &[u8]) -> String {
    let whole = match std::str::from_utf8(bytes) {
        Err(e) if e.error_len().is_none() => &bytes[..e.valid_up_to()],
        _ => bytes,
    };
    String::from_utf8_lossy(whole).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_character_cut_by_the_read_limit_is_dropped() {
        assert_eq!(text_of("aé".as_bytes()), "aé");
        assert_eq!(text_of(&"aé".as_bytes()[..2]), "a");
        assert_eq!(text_of(b"a\xFFb"), "a\u{FFFD}b");
    }
}
