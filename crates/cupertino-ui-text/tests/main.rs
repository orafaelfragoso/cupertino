//! The integration tests of `cupertino-ui-text`, one binary: each file beside this one is a module of it, so a
//! test's name starts with its file's.

#[path = "field-keys.rs"]
mod field_keys;
#[path = "font-data.rs"]
mod font_data;
#[path = "glyph-font.rs"]
mod glyph_font;
#[path = "paragraph-layout.rs"]
mod paragraph_layout;
#[path = "secure-buffer.rs"]
mod secure_buffer;
#[path = "shared-text-system.rs"]
mod shared_text_system;
#[path = "text-buffer.rs"]
mod text_buffer;
