//! The column view without a toolkit: the chain of folders it shows side by side, each column's width, and where the
//! chain scrolls to.
//!
//! The chain runs from a root folder to the folder the window shows (its [`crate::Browser`]'s folder), one column for
//! each; a single selected folder adds a column of its contents after them, and a single selected file a preview
//! column. The root stays while the window goes to folders inside it, so earlier columns stay to the left; a folder
//! outside it (a place of the sidebar, Back to an unrelated folder) becomes the new root. Going up past the root (Go ›
//! Enclosing Folder) makes the folder gone to the root.
//!
//! Each column keeps its own width while the window is open, by its place in the chain; a column without one takes the
//! default width. Resizing a column with Option held resizes every column and makes that width the default. The chain
//! scrolls horizontally in whole columns: the first column shown is the first from which the rest of the chain fits,
//! and the last column of folders widens to fill what the chain leaves of the view.

use std::path::{Path, PathBuf};

use crate::metrics::column_view;

/// The width a column starts at.
pub const DEFAULT_WIDTH: f64 = column_view::PREFERRED_WIDTH;
/// The narrowest a column can be made.
pub const MIN_WIDTH: f64 = column_view::MIN_WIDTH;
/// The widest a column can be made.
pub const MAX_WIDTH: f64 = 1000.0;
/// The width of the preview column at the end of the chain.
pub const PREVIEW_WIDTH: f64 = 240.0;
/// The height of a row: the 16 pt icon and the row's padding.
pub const ROW_HEIGHT: f64 = 16.0 + column_view::ROW_EXTRA;

/// The folders of the column view and their widths.
#[derive(Clone, Debug, PartialEq)]
pub struct ColumnChain {
    root: Option<PathBuf>,
    /// The width of each column that was resized, by its place in the chain.
    widths: Vec<Option<f64>>,
    default_width: f64,
}

impl Default for ColumnChain {
    fn default() -> Self {
        ColumnChain::new(DEFAULT_WIDTH)
    }
}

impl ColumnChain {
    /// A chain without a root yet, whose columns start `default_width` wide.
    pub fn new(default_width: f64) -> Self {
        ColumnChain {
            root: None,
            widths: Vec::new(),
            default_width: clamp(default_width),
        }
    }

    /// The width a column without its own width takes.
    pub fn default_width(&self) -> f64 {
        self.default_width
    }

    /// The folder the chain starts at.
    pub fn root(&self) -> Option<&Path> {
        self.root.as_deref()
    }

    /// The window shows `dir`: the root stays when `dir` is inside it or is it, else `dir` becomes the root.
    pub fn follow(&mut self, dir: &Path) {
        let keep = self
            .root
            .as_deref()
            .is_some_and(|root| dir.starts_with(root));
        if !keep {
            self.root = Some(dir.to_path_buf());
        }
    }

    /// The folders shown as columns while the window shows `dir`, from the root to `dir`; only `dir` when the root is
    /// not one of its ancestors.
    pub fn folders(&self, dir: &Path) -> Vec<PathBuf> {
        let Some(root) = self.root.as_deref().filter(|root| dir.starts_with(root)) else {
            return vec![dir.to_path_buf()];
        };
        let mut chain: Vec<PathBuf> = dir
            .ancestors()
            .take_while(|folder| folder.starts_with(root))
            .map(Path::to_path_buf)
            .collect();
        chain.reverse();
        chain
    }

    /// The width of the column at place `index` of the chain.
    pub fn width(&self, index: usize) -> f64 {
        self.widths
            .get(index)
            .copied()
            .flatten()
            .unwrap_or(self.default_width)
    }

    /// Make the column at `index` `width` wide, within [`MIN_WIDTH`] and [`MAX_WIDTH`]; with `all`, every column and
    /// the default.
    pub fn resize(&mut self, index: usize, width: f64, all: bool) {
        let width = clamp(width);
        if all {
            self.widths.clear();
            self.default_width = width;
            return;
        }
        if self.widths.len() <= index {
            self.widths.resize(index + 1, None);
        }
        self.widths[index] = Some(width);
    }

    /// The widths of the first `count` columns.
    pub fn widths(&self, count: usize) -> Vec<f64> {
        (0..count).map(|i| self.width(i)).collect()
    }
}

fn clamp(width: f64) -> f64 {
    if width.is_finite() {
        width.clamp(MIN_WIDTH, MAX_WIDTH)
    } else {
        DEFAULT_WIDTH
    }
}

/// Where the columns lie in the chain's content: each column's leading edge and width, then the width of the whole
/// content and how far it scrolls.
#[derive(Clone, Debug, PartialEq)]
pub struct ChainLayout {
    pub columns: Vec<(f64, f64)>,
    pub content: f64,
    pub offset: f64,
}

/// Lay out columns of `widths`, the last `fixed` of which keep their width (the preview column), in a view `viewport`
/// wide: scrolled in whole columns so the end of the chain shows, the last column that is not fixed widened to fill
/// what is left of the view.
pub fn layout(widths: &[f64], fixed: usize, viewport: f64) -> ChainLayout {
    let mut widths = widths.to_vec();
    let total: f64 = widths.iter().sum();
    // the first column from which the rest of the chain fits the view; the last column alone when none does
    let first = (0..widths.len())
        .find(|&i| widths[i..].iter().sum::<f64>() <= viewport + 1e-9)
        .unwrap_or(widths.len().saturating_sub(1));
    let shown: f64 = widths[first..].iter().sum();
    let spare = (viewport - shown).max(0.0);
    let stretch = widths.len().checked_sub(fixed + 1).filter(|&i| i >= first);
    if let Some(i) = stretch {
        widths[i] += spare;
    }
    let mut x = 0.0;
    let columns: Vec<(f64, f64)> = widths
        .iter()
        .map(|&w| {
            let column = (x, w);
            x += w;
            column
        })
        .collect();
    let offset = columns.get(first).map_or(0.0, |c| c.0);
    let content = if stretch.is_some() {
        x
    } else {
        total.max(offset + viewport)
    };
    ChainLayout {
        columns,
        content,
        offset,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_root_stays_inside_it_and_moves_to_a_folder_outside() {
        let mut chain = ColumnChain::default();
        chain.follow(Path::new("/home/ada"));
        chain.follow(Path::new("/home/ada/Documents/Notes"));
        assert_eq!(
            chain.folders(Path::new("/home/ada/Documents/Notes")),
            [
                PathBuf::from("/home/ada"),
                PathBuf::from("/home/ada/Documents"),
                PathBuf::from("/home/ada/Documents/Notes")
            ]
        );
        // up past the root: the folder gone to is the root
        chain.follow(Path::new("/home"));
        assert_eq!(chain.root(), Some(Path::new("/home")));
        // another place
        chain.follow(Path::new("/tmp/x"));
        assert_eq!(
            chain.folders(Path::new("/tmp/x")),
            [PathBuf::from("/tmp/x")]
        );
    }

    #[test]
    fn a_column_keeps_its_width_and_option_resizes_them_all() {
        let mut chain = ColumnChain::default();
        assert_eq!(chain.widths(3), [245.0, 245.0, 245.0]);
        chain.resize(1, 300.0, false);
        assert_eq!(chain.widths(3), [245.0, 300.0, 245.0]);
        chain.resize(0, 20.0, false);
        assert_eq!(chain.width(0), MIN_WIDTH);
        chain.resize(2, 180.0, true);
        assert_eq!(chain.widths(3), [180.0, 180.0, 180.0]);
        assert_eq!(chain.default_width(), 180.0);
        assert_eq!(ColumnChain::new(f64::NAN).default_width(), DEFAULT_WIDTH);
    }

    #[test]
    fn a_short_chain_widens_its_last_folder_column_to_the_view() {
        let l = layout(&[245.0, 245.0], 0, 700.0);
        assert_eq!(l.columns, [(0.0, 245.0), (245.0, 455.0)]);
        assert_eq!((l.content, l.offset), (700.0, 0.0));
        // the preview column keeps its width; the folder column before it takes the rest
        let l = layout(&[245.0, PREVIEW_WIDTH], 1, 690.0);
        assert_eq!(l.columns, [(0.0, 450.0), (450.0, 240.0)]);
    }

    #[test]
    fn a_long_chain_scrolls_in_whole_columns_to_its_end() {
        // three folders and the preview: only the last folder and the preview fit
        let l = layout(&[245.0, 245.0, 245.0, PREVIEW_WIDTH], 1, 690.0);
        assert_eq!(l.offset, 490.0);
        assert_eq!(l.columns[2], (490.0, 450.0));
        assert_eq!(l.content, 490.0 + 690.0);
        // a single column wider than the view shows from its start
        let l = layout(&[900.0], 0, 600.0);
        assert_eq!((l.offset, l.content), (0.0, 900.0));
    }
}
