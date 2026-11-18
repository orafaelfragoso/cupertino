//! CupertinoKit's file family, which programs name as `cupertino_kit::files`.
//!
//! * [`entry`]: an item of a folder; [`natural`], [`sort`]: the order of names and of items;
//! * [`mime`], [`tags`]: an item's type and its tags; [`applications`], [`recents`]: the two virtual folders;
//! * [`layout`]: the icon layout, with the measured metrics of [`metrics`]; [`geom`]: the direction a key moves in it;
//! * [`rename`], [`name_edit`]: name checks, free names and the name being edited; [`open`]: default applications and
//!   the Open With list; [`operations`]: new folders, renames, aliases, copies and moves, as blocking functions;
//! * [`item_art`]: the art of the common items.

pub mod applications;
pub mod entry;
pub mod geom;
pub mod item_art;
pub mod layout;
pub mod metrics;
pub mod mime;
pub mod name_edit;
pub mod natural;
pub mod open;
pub mod operations;
pub mod recents;
pub mod rename;
pub mod sort;
pub mod tags;
