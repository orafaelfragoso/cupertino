//! The overview of the Cupertino desktop, a component of the shell host (`cupertino-shell`): a surface over the
//! output that shows its windows in holes (`cupertino_hole_v1`), laid out by the reference packing of
//! `cupertino_wm::overview` ([`layout`]), with the desktops band and the selected window's ring and name
//! ([`chrome`], on the parts of [`parts`]). The way in and out are transactions bound to the gestures that open and
//! close it, or started by the person's toggle ([`component`]).

#![forbid(unsafe_code)]

pub mod chrome;
pub mod component;
pub mod layout;
pub mod parts;

pub use component::{Message, Model, Overview};
