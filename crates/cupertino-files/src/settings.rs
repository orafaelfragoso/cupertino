//! Per-folder view settings and their persistence (the role of the per-folder metadata file of the
//! reference).
//!
//! Defaults and valid ranges are the measured ones of [`crate::metrics`]. [`ViewSettings::clamp`]
//! applies the integrity check of the design (out-of-range values are rejected there; this
//! implementation clamps instead, which is a deliberate simplification).

use crate::metrics::{column_view, icon_view, list_view};
use crate::sort::{SortKey, SortSpec};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

/// Four-character codes of the view styles as stored by the design..
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ViewStyle {
    Icon,
    List,
    Column,
    Gallery,
}

impl ViewStyle {
    pub fn code(self) -> &'static str {
        match self {
            ViewStyle::Icon => "icnv",
            ViewStyle::List => "Nlsv",
            ViewStyle::Column => "clmv",
            ViewStyle::Gallery => "glyv",
        }
    }
    pub fn from_code(c: &str) -> Option<Self> {
        [ViewStyle::Icon, ViewStyle::List, ViewStyle::Column, ViewStyle::Gallery].into_iter().find(|s| s.code() == c)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct IconViewSettings {
    pub icon_size: u32,
    pub grid_spacing: u32,
    pub text_size: u16,
    pub label_on_bottom: bool,
    pub show_item_info: bool,
    pub show_icon_preview: bool,
    pub keep_arranged_by: SortKey,
}

impl Default for IconViewSettings {
    fn default() -> Self {
        IconViewSettings {
            icon_size: icon_view::ICON_SIZE_DEFAULT,
            grid_spacing: icon_view::GRID_SPACING_DEFAULT,
            text_size: icon_view::TEXT_SIZE_DEFAULT,
            label_on_bottom: true,
            show_item_info: false,
            show_icon_preview: true,
            keep_arranged_by: SortKey::None,
        }
    }
}

/// Column descriptor of the list view. `width` is the default, `min` / `max` bound user resizing.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ColumnDesc {
    pub id: ColumnId,
    pub width: f64,
    pub min_width: f64,
    pub max_width: f64,
    pub visible: bool,
    pub ascending: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ColumnId {
    Name,
    DateModified,
    DateCreated,
    Size,
    Kind,
    Label,
    Version,
    Comments,
}

impl ColumnId {
    pub fn sort_key(self) -> SortKey {
        match self {
            ColumnId::Name => SortKey::Name,
            ColumnId::DateModified => SortKey::DateModified,
            ColumnId::DateCreated => SortKey::DateCreated,
            ColumnId::Size => SortKey::Size,
            ColumnId::Kind => SortKey::Kind,
            ColumnId::Label => SortKey::Tags,
            ColumnId::Version | ColumnId::Comments => SortKey::Name,
        }
    }
}

/// Default list columns, in order. (table): identifier,
/// default / minimum / maximum width and visibility. Name, Date Modified, Size, Kind are visible.
pub fn default_columns() -> Vec<ColumnDesc> {
    let c = |id, width, min_width, max_width, visible, ascending| ColumnDesc { id, width, min_width, max_width, visible, ascending };
    vec![
        c(ColumnId::Name, 300.0, 80.0, 5000.0, true, true),
        c(ColumnId::DateModified, 181.0, 80.0, 512.0, true, false),
        c(ColumnId::DateCreated, 181.0, 75.0, 512.0, false, false),
        c(ColumnId::Size, 97.0, 40.0, 192.0, true, false),
        c(ColumnId::Kind, 115.0, 80.0, 512.0, true, true),
        c(ColumnId::Label, 100.0, 40.0, 256.0, false, true),
        c(ColumnId::Version, 75.0, 50.0, 512.0, false, true),
        c(ColumnId::Comments, 300.0, 60.0, 7680.0, false, true),
    ]
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ListViewSettings {
    pub icon_size: u32,
    pub text_size: u16,
    pub use_relative_dates: bool,
    pub calculate_all_sizes: bool,
    pub show_icon_preview: bool,
    pub sort: SortSpec,
    pub columns: Vec<ColumnDesc>,
}

impl Default for ListViewSettings {
    fn default() -> Self {
        ListViewSettings {
            icon_size: list_view::ICON_SIZE_DEFAULT,
            text_size: list_view::TEXT_SIZE_DEFAULT,
            use_relative_dates: true,
            calculate_all_sizes: false,
            show_icon_preview: true,
            sort: SortSpec::default(),
            columns: default_columns(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ColumnViewSettings {
    pub preferred_column_width: f64,
    /// Text size is not known for this view; the list default is assumed (approximate).
    pub text_size: u16,
    pub show_icons: bool,
    pub show_preview_column: bool,
    pub show_icon_preview: bool,
}

impl Default for ColumnViewSettings {
    fn default() -> Self {
        ColumnViewSettings { preferred_column_width: column_view::PREFERRED_WIDTH, text_size: list_view::TEXT_SIZE_DEFAULT, show_icons: true, show_preview_column: true, show_icon_preview: true }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GalleryViewSettings {
    pub icon_size: u32,
    pub show_titles: bool,
    pub show_preview_pane: bool,
    pub preview_pane_width: f64,
}

impl Default for GalleryViewSettings {
    fn default() -> Self {
        GalleryViewSettings { icon_size: 64, show_titles: true, show_preview_pane: true, preview_pane_width: column_view::PREVIEW_PANE_WIDTH }
    }
}

/// Everything remembered for one folder.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ViewSettings {
    pub style: ViewStyle,
    pub icon: IconViewSettings,
    pub list: ListViewSettings,
    pub column: ColumnViewSettings,
    pub gallery: GalleryViewSettings,
    pub scroll_position: (f64, f64),
    pub window_size: Option<(f64, f64)>,
}

impl Default for ViewSettings {
    fn default() -> Self {
        ViewSettings {
            style: ViewStyle::Icon,
            icon: IconViewSettings::default(),
            list: ListViewSettings::default(),
            column: ColumnViewSettings::default(),
            gallery: GalleryViewSettings::default(),
            scroll_position: (0.0, 0.0),
            window_size: None,
        }
    }
}

impl ViewSettings {
    /// Bring every value into its valid range.
    pub fn clamp(&mut self) {
        self.icon.icon_size = self.icon.icon_size.clamp(icon_view::ICON_SIZE_MIN, icon_view::ICON_SIZE_MAX);
        self.icon.grid_spacing = self.icon.grid_spacing.clamp(icon_view::GRID_SPACING_MIN, icon_view::GRID_SPACING_MAX);
        self.icon.text_size = self.icon.text_size.clamp(icon_view::TEXT_SIZE_MIN, icon_view::TEXT_SIZE_MAX);
        self.list.icon_size = self.list.icon_size.clamp(list_view::ICON_SIZE_MIN, list_view::ICON_SIZE_MAX);
        self.list.text_size = self.list.text_size.clamp(list_view::TEXT_SIZE_MIN, list_view::TEXT_SIZE_MAX);
        self.column.text_size = self.column.text_size.clamp(list_view::TEXT_SIZE_MIN, list_view::TEXT_SIZE_MAX);
        self.column.preferred_column_width = self.column.preferred_column_width.max(column_view::MIN_WIDTH);
        self.gallery.icon_size = self.gallery.icon_size.clamp(icon_view::ICON_SIZE_MIN, icon_view::ICON_SIZE_MAX);
        for c in &mut self.list.columns {
            c.width = c.width.clamp(c.min_width, c.max_width);
        }
    }

    /// Resize one list column, respecting its bounds; returns the applied width.
    pub fn set_column_width(&mut self, id: ColumnId, width: f64) -> Option<f64> {
        let c = self.list.columns.iter_mut().find(|c| c.id == id)?;
        c.width = width.clamp(c.min_width, c.max_width);
        Some(c.width)
    }
}

/// Key of a folder: the canonical path plus device and inode, so that a renamed folder keeps its
/// settings when the inode is unchanged and a different folder at an old path does not inherit them.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct FolderKey {
    pub path: String,
    pub dev: u64,
    pub inode: u64,
}

impl FolderKey {
    pub fn of(path: &Path) -> io::Result<Self> {
        use std::os::unix::fs::MetadataExt;
        let canon = std::fs::canonicalize(path)?;
        let m = std::fs::metadata(&canon)?;
        Ok(FolderKey { path: canon.to_string_lossy().into_owned(), dev: m.dev(), inode: m.ino() })
    }
    fn storage_key(&self) -> String {
        format!("{}:{}", self.dev, self.inode)
    }
}

#[derive(Serialize, Deserialize, Default)]
struct StoreFile {
    version: u32,
    folders: BTreeMap<String, Stored>,
    defaults: Option<ViewSettings>,
}

#[derive(Serialize, Deserialize)]
struct Stored {
    path: String,
    settings: ViewSettings,
}

/// JSON file store of the per-folder settings plus the user defaults for folders without an entry.
/// Writes are atomic (temporary file + rename).
pub struct SettingsStore {
    file: PathBuf,
    data: StoreFile,
    dirty: bool,
}

impl SettingsStore {
    pub fn open(file: impl Into<PathBuf>) -> io::Result<Self> {
        let file = file.into();
        let data = match std::fs::read(&file) {
            Ok(b) => serde_json::from_slice(&b).unwrap_or_default(),
            Err(e) if e.kind() == io::ErrorKind::NotFound => StoreFile { version: 1, ..Default::default() },
            Err(e) => return Err(e),
        };
        Ok(SettingsStore { file, data, dirty: false })
    }

    /// Default location: `$XDG_DATA_HOME/cupertino-files/folders.json`.
    pub fn default_path() -> PathBuf {
        let base = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share"))).unwrap_or_else(|| PathBuf::from("."));
        base.join("cupertino-files").join("folders.json")
    }

    pub fn defaults(&self) -> ViewSettings {
        self.data.defaults.clone().unwrap_or_default()
    }

    pub fn set_defaults(&mut self, s: ViewSettings) {
        self.data.defaults = Some(s);
        self.dirty = true;
    }

    /// Settings of a folder, falling back to the defaults. The result is clamped.
    pub fn get(&self, key: &FolderKey) -> ViewSettings {
        let mut s = self.data.folders.get(&key.storage_key()).map(|x| x.settings.clone()).unwrap_or_else(|| self.defaults());
        s.clamp();
        s
    }

    pub fn has(&self, key: &FolderKey) -> bool {
        self.data.folders.contains_key(&key.storage_key())
    }

    pub fn set(&mut self, key: &FolderKey, mut s: ViewSettings) {
        s.clamp();
        self.data.folders.insert(key.storage_key(), Stored { path: key.path.clone(), settings: s });
        self.dirty = true;
    }

    pub fn remove(&mut self, key: &FolderKey) {
        if self.data.folders.remove(&key.storage_key()).is_some() {
            self.dirty = true;
        }
    }

    /// Drop entries whose folder no longer exists (same inode at the stored path).
    pub fn prune(&mut self) -> usize {
        let before = self.data.folders.len();
        self.data.folders.retain(|k, v| FolderKey::of(Path::new(&v.path)).map(|f| f.storage_key() == *k).unwrap_or(false));
        let n = before - self.data.folders.len();
        if n > 0 {
            self.dirty = true;
        }
        n
    }

    pub fn len(&self) -> usize {
        self.data.folders.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.folders.is_empty()
    }

    pub fn save(&mut self) -> io::Result<()> {
        if !self.dirty {
            return Ok(());
        }
        if let Some(dir) = self.file.parent() {
            std::fs::create_dir_all(dir)?;
        }
        self.data.version = 1;
        let tmp = self.file.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(&self.data).map_err(io::Error::other)?)?;
        std::fs::rename(&tmp, &self.file)?;
        self.dirty = false;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_the_measured_ones() {
        let s = ViewSettings::default();
        assert_eq!((s.icon.icon_size, s.icon.grid_spacing, s.icon.text_size), (64, 54, 12));
        assert_eq!((s.list.icon_size, s.list.text_size), (16, 13));
        assert_eq!(s.column.preferred_column_width, 245.0);
        let vis: Vec<ColumnId> = s.list.columns.iter().filter(|c| c.visible).map(|c| c.id).collect();
        assert_eq!(vis, [ColumnId::Name, ColumnId::DateModified, ColumnId::Size, ColumnId::Kind]);
    }

    #[test]
    fn style_codes_roundtrip() {
        for s in [ViewStyle::Icon, ViewStyle::List, ViewStyle::Column, ViewStyle::Gallery] {
            assert_eq!(ViewStyle::from_code(s.code()), Some(s));
        }
        assert_eq!(ViewStyle::Icon.code(), "icnv");
        assert_eq!(ViewStyle::Gallery.code(), "glyv");
    }

    #[test]
    fn clamp_applies_ranges() {
        let mut s = ViewSettings::default();
        s.icon.icon_size = 4000;
        s.icon.grid_spacing = 0;
        s.icon.text_size = 3;
        s.list.icon_size = 64;
        s.list.text_size = 99;
        s.clamp();
        assert_eq!((s.icon.icon_size, s.icon.grid_spacing, s.icon.text_size), (512, 1, 10));
        assert_eq!((s.list.icon_size, s.list.text_size), (32, 16));
    }

    #[test]
    fn column_resize_respects_bounds() {
        let mut s = ViewSettings::default();
        assert_eq!(s.set_column_width(ColumnId::Size, 10.0), Some(40.0));
        assert_eq!(s.set_column_width(ColumnId::Size, 1000.0), Some(192.0));
        assert_eq!(s.set_column_width(ColumnId::Name, 250.0), Some(250.0));
    }

    #[test]
    fn store_persists_per_folder_and_survives_rename() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("a");
        std::fs::create_dir(&folder).unwrap();
        let key = FolderKey::of(&folder).unwrap();
        let file = dir.path().join("store/folders.json");
        {
            let mut st = SettingsStore::open(&file).unwrap();
            assert!(!st.has(&key));
            let mut s = ViewSettings { style: ViewStyle::List, ..Default::default() };
            s.icon.icon_size = 128;
            st.set(&key, s);
            st.save().unwrap();
        }
        let st = SettingsStore::open(&file).unwrap();
        assert_eq!(st.get(&key).style, ViewStyle::List);
        assert_eq!(st.get(&key).icon.icon_size, 128);
        // a renamed folder has the same inode: same key
        let renamed = dir.path().join("b");
        std::fs::rename(&folder, &renamed).unwrap();
        let key2 = FolderKey::of(&renamed).unwrap();
        assert_eq!(st.get(&key2).style, ViewStyle::List);
        // an unknown folder gets the defaults
        let other = dir.path().join("c");
        std::fs::create_dir(&other).unwrap();
        assert_eq!(st.get(&FolderKey::of(&other).unwrap()).style, ViewStyle::Icon);
    }

    #[test]
    fn prune_removes_vanished_folders() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("a");
        std::fs::create_dir(&folder).unwrap();
        let key = FolderKey::of(&folder).unwrap();
        let mut st = SettingsStore::open(dir.path().join("s.json")).unwrap();
        st.set(&key, ViewSettings::default());
        std::fs::remove_dir(&folder).unwrap();
        assert_eq!(st.prune(), 1);
        assert!(st.is_empty());
    }
}
