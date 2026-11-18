//! State of one browser window without a toolkit: the folder shown (a live [`DirectoryModel`]), the
//! back / forward [`History`], the [`Selection`] kept on the same items across changes of the listing,
//! the sort order and type-select. The window process feeds it input and draws what it holds.
//!
//! Behaviour (reference, unless noted): opening a folder replaces the window's folder; going
//! to the enclosing folder selects the folder the window came from; the folder of a window that is
//! deleted is replaced by its nearest existing ancestor (the design closes such windows
//! or shows the parent depending on the volume); opening a selection opens every selected file with
//! its default application and goes into the first selected folder (approximate: the design opens
//! further folders in new windows or tabs, which this window does not have).
//!
//! The list shows folders as an outline: expanding a folder lists its items under it one level
//! deeper, from a live model of its own opened only then (lazy, in batches like the window's). Collapsing
//! forgets the expanded folders inside it, and a selection inside moves to the collapsed folder
//! (the outline view convention); going to another folder or leaving the list collapses everything
//! (the design keeps expansion state per window, its persistence is not known).

use crate::history::History;
use crate::layout::list::RowSpec;
use crate::model::{DirectoryModel, ModelDelta, ModelOptions, Notify, DEFAULT_BATCH};
use crate::selection::{Modifiers, MouseDown, Selection};
use crate::typeselect::{Outcome, TypeSelect};
use cupertino_apps::MimeDatabase;
use cupertino_kit::files::entry::{Entry, Kind};
use cupertino_kit::files::recents;
use cupertino_kit::files::sort::{SortKey, SortSpec};
use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// What opening the selection does ([`Browser::activation`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Activation {
    /// The folder to go into.
    pub folder: Option<PathBuf>,
    /// Files for their default applications.
    pub files: Vec<PathBuf>,
}

/// What a [`Browser::poll`] brought.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Polled {
    /// The listing or the folder changed: draw again.
    pub changed: bool,
    /// Errors the model reported (reading the folder, the watch); the listing stays as it was.
    pub errors: Vec<String>,
}

pub struct Browser {
    history: History,
    model: DirectoryModel,
    selection: Selection,
    sort: SortSpec,
    /// Given to every model of the window: sets `queued`, then tells the window.
    notify: Notify,
    /// The model queued something since the last [`Browser::poll`].
    queued: Arc<AtomicBool>,
    type_select: TypeSelect,
    /// Name of an item to select once the listing has it: the folder a step to the parent came from.
    select_when_listed: Option<String>,
    /// An item the window made and waits for the listing to show: selected when it appears.
    awaited: Option<PathBuf>,
    /// The focus moved by keyboard or by the window itself, so the view should scroll it into sight.
    focus_moved: bool,
    /// The folders shown expanded, each with its own live model, by path.
    expanded: HashMap<PathBuf, DirectoryModel>,
    /// The listing with the items of expanded folders under them; empty while nothing is expanded, when
    /// [`Browser::entries`] is the folder's own listing.
    rows: Vec<Entry>,
    /// Level and disclosure state of every displayed item, in display order.
    outline: Vec<RowSpec>,
    /// The bookmark file the Recents folder ([`recents::RECENTS`]) lists.
    recents_file: PathBuf,
    /// The MIME database every model of the window types its items with.
    types: Option<Arc<MimeDatabase>>,
    /// The toolbar search, lower-cased: only the folder's items whose names contain it are shown, flat.
    filter: Option<String>,
    /// The selection to give back once the listing is complete ([`Browser::return_to`]).
    reselect: Option<SelectionPaths>,
    /// The folders to expand again as the listing reaches them ([`Browser::return_to`]), outermost first.
    reopen: Vec<PathBuf>,
    /// The item [`Browser::reveal`] puts the keyboard on, until the window takes it ([`Browser::take_revealed`]).
    revealing: Option<PathBuf>,
}

/// Where a window was, to go back to with [`Browser::return_to`]: its history, its expanded folders and its
/// selection, by path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Visit {
    history: History,
    selection: SelectionPaths,
    /// Outermost first.
    expanded: Vec<PathBuf>,
}

impl Visit {
    /// The folder shown.
    pub fn dir(&self) -> &Path {
        self.history.current()
    }
}

/// Lay out `entries` at `level` with the items of the expanded folders under them, noting the expanded
/// folders reached.
fn flatten(
    entries: &[Entry],
    level: u32,
    expanded: &HashMap<PathBuf, DirectoryModel>,
    rows: &mut Vec<Entry>,
    outline: &mut Vec<RowSpec>,
    reached: &mut HashSet<PathBuf>,
) {
    for e in entries {
        let folder = e.kind == Kind::Directory;
        let children = expanded.get(&e.path).filter(|_| folder);
        outline.push(RowSpec {
            level,
            expandable: folder,
            expanded: children.is_some(),
        });
        rows.push(e.clone());
        if let Some(model) = children {
            reached.insert(e.path.clone());
            flatten(model.entries(), level + 1, expanded, rows, outline, reached);
        }
    }
}

/// Selected, anchor and focus items by path.
type SelectionPaths = (Vec<PathBuf>, Option<PathBuf>, Option<PathBuf>);

/// The options every folder of a window is opened with: hidden files off (default,
/// ), watched for live changes, each item's tags read (the sidebar lists them and the search finds by them), typed
/// with the window's MIME database `types`.
fn model_options(sort: SortSpec, types: &Option<Arc<MimeDatabase>>) -> ModelOptions {
    ModelOptions {
        batch_size: DEFAULT_BATCH,
        show_hidden: false,
        sort,
        watch: true,
        read_tags: true,
        types: types.clone(),
    }
}

/// A live model of `dir`, or of the Recents list `recents_file` when `dir` is [`recents::RECENTS`].
fn open_model(
    dir: &Path,
    sort: SortSpec,
    recents_file: &Path,
    types: &Option<Arc<MimeDatabase>>,
    notify: Notify,
) -> io::Result<DirectoryModel> {
    if dir == recents::location() {
        DirectoryModel::open_recents(
            recents_file.to_path_buf(),
            model_options(sort, types),
            notify,
        )
    } else if dir == cupertino_kit::files::applications::location() {
        DirectoryModel::open_applications(
            cupertino_kit::files::applications::application_dirs(),
            model_options(sort, types),
            notify,
        )
    } else {
        DirectoryModel::open_notifying(dir, model_options(sort, types), notify)
    }
}

impl Browser {
    /// A window on `start` listed in `sort` order, its Recents folder from the bookmark file `recents_file`, its items
    /// typed with the MIME database `types` ([`ModelOptions::types`]); `notify` runs on other threads whenever there is
    /// something to [`Browser::poll`].
    pub fn open(
        start: PathBuf,
        sort: SortSpec,
        recents_file: PathBuf,
        types: Option<Arc<MimeDatabase>>,
        notify: Notify,
    ) -> io::Result<Self> {
        let queued = Arc::new(AtomicBool::new(false));
        let flag = queued.clone();
        let notify: Notify = Arc::new(move || {
            flag.store(true, Ordering::SeqCst);
            notify();
        });
        let model = open_model(&start, sort, &recents_file, &types, notify.clone())?;
        Ok(Browser {
            history: History::new(start),
            model,
            selection: Selection::new(),
            sort,
            notify,
            queued,
            type_select: TypeSelect::default(),
            select_when_listed: None,
            awaited: None,
            focus_moved: false,
            expanded: HashMap::new(),
            rows: Vec::new(),
            outline: Vec::new(),
            recents_file,
            types,
            filter: None,
            reselect: None,
            reopen: Vec::new(),
            revealing: None,
        })
    }

    /// The folder is not a directory of the file system (Recents, Applications): it has no enclosing folder and no
    /// free space.
    pub fn is_virtual(&self) -> bool {
        self.dir() == recents::location()
            || self.dir() == cupertino_kit::files::applications::location()
    }

    /// Number of the folder's own items (not those of expanded folders), or of those the search finds.
    pub fn item_count(&self) -> usize {
        match self.filter {
            Some(_) => self.rows.len(),
            None => self.model.len(),
        }
    }

    /// A search is shown, the folder has been read and nothing matches it: the view says "No Results".
    pub fn no_results(&self) -> bool {
        self.filter.is_some() && self.item_count() == 0 && !self.model.is_loading()
    }

    /// How many items match the search shown once the folder has been read; `None` without a search or while it is read.
    pub fn search_results(&self) -> Option<usize> {
        (self.filter.is_some() && !self.model.is_loading()).then(|| self.item_count())
    }

    /// Show only the items whose names contain `query`, or that carry a tag of that name, ignoring case, as a flat
    /// list; an empty query shows the folder again. approximate: the design's search field searches the whole computer by default; this one filters the
    /// folder shown. The selection stays on the items still shown.
    pub fn set_filter(&mut self, query: &str) {
        let filter = (!query.is_empty()).then(|| query.to_lowercase());
        if filter == self.filter {
            return;
        }
        let kept = self.selection_paths();
        self.filter = filter;
        self.expanded.clear();
        self.rebuild();
        self.restore_selection(kept);
    }

    /// The search shown, lower-cased.
    pub fn filter(&self) -> Option<&str> {
        self.filter.as_deref()
    }

    /// The listing is [`Browser::rows`]: folders are expanded or a search is shown.
    fn uses_rows(&self) -> bool {
        self.filter.is_some() || !self.expanded.is_empty()
    }

    /// Bytes an unprivileged user can still write on the folder's file system (`statvfs`: available blocks times the
    /// fragment size); `None` for a virtual folder.
    pub fn available_space(&self) -> io::Result<Option<u64>> {
        let Some(fs) = self.file_system()? else {
            return Ok(None);
        };
        // the fields are narrower on 32-bit targets
        #[allow(clippy::unnecessary_cast)]
        Ok(Some(fs.f_bavail as u64 * fs.f_frsize as u64))
    }

    /// Whether the folder's file system is mounted read-only; see [`mounted_read_only`]. Applications lists what is
    /// installed: nothing is made, renamed or moved there.
    pub fn read_only(&self) -> io::Result<bool> {
        if self.dir() == cupertino_kit::files::applications::location() {
            return Ok(true);
        }
        match self.is_virtual() {
            true => Ok(false),
            false => mounted_read_only(self.dir()),
        }
    }

    /// The folder's file system; `None` for a virtual folder.
    fn file_system(&self) -> io::Result<Option<libc::statvfs>> {
        match self.is_virtual() {
            true => Ok(None),
            false => file_system(self.dir()).map(Some),
        }
    }

    pub fn dir(&self) -> &Path {
        self.history.current()
    }

    pub fn history(&self) -> &History {
        &self.history
    }

    /// The items in display order, the items of expanded folders under them.
    pub fn entries(&self) -> &[Entry] {
        if self.uses_rows() {
            &self.rows
        } else {
            self.model.entries()
        }
    }

    /// Level and disclosure state of each item of [`Browser::entries`]: every folder (not a link to one) can be
    /// expanded.
    pub fn outline(&self) -> &[RowSpec] {
        &self.outline
    }

    /// Lay the displayed rows out again from the models, dropping the expanded folders no longer listed (removed,
    /// renamed, or inside a collapsed folder).
    fn rebuild(&mut self) {
        let (mut rows, mut outline, mut reached) = (Vec::new(), Vec::new(), HashSet::new());
        if let Some(query) = &self.filter {
            // search results are flat: no folder expands among them
            rows = self
                .model
                .entries()
                .iter()
                .filter(|e| matches(e, query))
                .cloned()
                .collect();
            outline = vec![
                RowSpec {
                    level: 0,
                    expandable: false,
                    expanded: false
                };
                rows.len()
            ];
        } else if self.expanded.is_empty() {
            outline = self
                .model
                .entries()
                .iter()
                .map(|e| RowSpec {
                    level: 0,
                    expandable: e.kind == Kind::Directory,
                    expanded: false,
                })
                .collect();
        } else {
            flatten(
                self.model.entries(),
                0,
                &self.expanded,
                &mut rows,
                &mut outline,
                &mut reached,
            );
            self.expanded.retain(|path, _| reached.contains(path));
            if self.expanded.is_empty() {
                rows.clear();
            }
        }
        self.rows = rows;
        self.outline = outline;
    }

    /// Expand folder row `index` (its disclosure triangle, or Right): its items load under it. `Ok(false)` when
    /// the row is not a folder or is expanded already.
    pub fn expand(&mut self, index: usize) -> io::Result<bool> {
        let Some(e) = self.entries().get(index).filter(|e| {
            self.filter.is_none()
                && e.kind == Kind::Directory
                && !self.expanded.contains_key(&e.path)
        }) else {
            return Ok(false);
        };
        let path = e.path.clone();
        let model = DirectoryModel::open_notifying(
            &path,
            model_options(self.sort, &self.types),
            self.notify.clone(),
        )?;
        let kept = self.selection_paths();
        self.expanded.insert(path, model);
        self.rebuild();
        self.restore_selection(kept);
        Ok(true)
    }

    /// Collapse folder row `index` and every folder expanded inside it; selected items inside give their selection to
    /// the folder. `false` when the row is not expanded.
    pub fn collapse(&mut self, index: usize) -> bool {
        let Some(path) = self
            .entries()
            .get(index)
            .filter(|e| self.expanded.contains_key(&e.path))
            .map(|e| e.path.clone())
        else {
            return false;
        };
        let (selected, anchor, focus) = self.selection_paths();
        let inside = |p: &PathBuf| p != &path && p.starts_with(&path);
        let mut kept: Vec<PathBuf> = selected.iter().filter(|p| !inside(p)).cloned().collect();
        if selected.iter().any(inside) && !kept.contains(&path) {
            kept.push(path.clone());
        }
        let up = |p: Option<PathBuf>| p.map(|p| if inside(&p) { path.clone() } else { p });
        let (anchor, focus) = (up(anchor), up(focus));
        self.expanded.retain(|p, _| !p.starts_with(&path));
        self.rebuild();
        self.restore_selection((kept, anchor, focus));
        true
    }

    /// Show only the folder's own items (another view than the list).
    pub fn collapse_all(&mut self) {
        if self.expanded.is_empty() {
            return;
        }
        let kept = self.selection_paths();
        self.expanded.clear();
        self.rebuild();
        self.restore_selection(kept);
    }

    pub fn selection(&self) -> &Selection {
        &self.selection
    }

    pub fn sort(&self) -> SortSpec {
        self.sort
    }

    /// Whether the focus moved since the last call in a way the view should scroll into sight.
    pub fn take_focus_moved(&mut self) -> bool {
        std::mem::take(&mut self.focus_moved)
    }

    /// Replace the model with one on `to`; the history is the caller's.
    fn show(&mut self, to: &Path) -> io::Result<()> {
        let model = open_model(
            to,
            self.sort,
            &self.recents_file,
            &self.types,
            self.notify.clone(),
        )?;
        self.replace_model(model);
        Ok(())
    }

    /// A live listing of `dir` opened as the window's own folders are, in its sort order and waking it when it changes:
    /// a column of the column view, which [`Browser::go_with`] can show without reading the folder again.
    pub fn open_listing(&self, dir: &Path) -> io::Result<DirectoryModel> {
        open_model(
            dir,
            self.sort,
            &self.recents_file,
            &self.types,
            self.notify.clone(),
        )
    }

    /// Show the folder `model` lists, a listing of [`Browser::open_listing`], as far as it has read it, and select the
    /// item named `select` once it is listed; the listing of the folder left is given back.
    pub fn go_with(&mut self, model: DirectoryModel, select: Option<&str>) -> DirectoryModel {
        let to = model.dir().to_path_buf();
        let left = self.replace_model(model);
        // what the listing queued before it was the window's own is applied at the next poll
        self.queued.store(true, Ordering::SeqCst);
        self.history.visit(to);
        if let Some(name) = select {
            self.select_when_listed = Some(name.to_owned());
            self.select_pending();
        }
        left
    }

    /// Put `model` in place of the folder's model, forgetting what belonged to the old one; the old model.
    fn replace_model(&mut self, model: DirectoryModel) -> DirectoryModel {
        let old = std::mem::replace(&mut self.model, model);
        self.expanded.clear();
        self.filter = None;
        self.rebuild();
        self.selection.clear();
        self.type_select.clear();
        self.select_when_listed = None;
        self.awaited = None;
        self.reselect = None;
        self.reopen.clear();
        self.revealing = None;
        old
    }

    /// Where the window is, with its history and its selection, for [`Browser::return_to`].
    pub fn visit(&self) -> Visit {
        let mut expanded: Vec<PathBuf> = self.expanded.keys().cloned().collect();
        expanded.sort_by_key(|p| p.components().count());
        Visit {
            history: self.history.clone(),
            selection: self.selection_paths(),
            expanded,
        }
    }

    /// Go back to `visit` as it was: its folder with its history, its folders expanded again as the listing reaches
    /// them, and its selection once they are listed. On an error the window stays where it is.
    pub fn return_to(&mut self, visit: Visit) -> io::Result<()> {
        if visit.dir() != self.dir() {
            self.show(visit.dir())?;
        }
        self.history = visit.history;
        self.reopen = visit.expanded;
        self.reselect = Some(visit.selection);
        self.reselect_pending();
        Ok(())
    }

    /// Expand again the folders [`Browser::return_to`] waits for that the listing shows; once every folder shown has
    /// been read, those it still does not show are gone and forgotten. `true` while any is waited for.
    fn reopen_pending(&mut self) -> bool {
        if self.reopen.is_empty() {
            return false;
        }
        if self.filter.is_some() {
            // a search shows no folder expanded
            self.reopen.clear();
            return false;
        }
        loop {
            let entries = self.entries();
            let shown = self.reopen.iter().position(|path| {
                entries
                    .iter()
                    .any(|e| &e.path == path && e.kind == Kind::Directory)
            });
            let Some(at) = shown else { break };
            let path = self.reopen.remove(at);
            let index = self.entries().iter().position(|e| e.path == path);
            if let Some(index) = index {
                if let Err(e) = self.expand(index) {
                    eprintln!("cupertino-files: cannot open {} again: {e}", path.display());
                }
            }
        }
        let reading =
            self.model.is_loading() || self.expanded.values().any(DirectoryModel::is_loading);
        if !reading {
            self.reopen.clear();
        }
        !self.reopen.is_empty() || reading
    }

    /// Select the items at `paths` once the folder has been read, the keyboard on the first, which the window then
    /// scrolls into sight ([`Browser::take_revealed`]). Items the listing does not show are left out.
    pub fn reveal(&mut self, paths: Vec<PathBuf>) {
        let first = paths.first().cloned();
        self.reselect = Some((paths, first.clone(), first.clone()));
        self.revealing = first;
        self.reselect_pending();
    }

    /// The item a [`Browser::reveal`] put the keyboard on, once its selection is made and while it is listed: given
    /// once.
    pub fn take_revealed(&mut self) -> Option<PathBuf> {
        if self.reselect.is_some() {
            return None;
        }
        let path = self.revealing.take()?;
        self.index_of_path(&path).map(|_| path)
    }

    /// Whether the folder is still being read.
    pub fn is_loading(&self) -> bool {
        self.model.is_loading()
    }

    fn reselect_pending(&mut self) {
        if self.model.is_loading() || self.reopen_pending() {
            return;
        }
        if let Some(kept) = self.reselect.take() {
            self.restore_selection(kept);
        }
    }

    /// Show the folder `to`. On an error (no such folder, no permission) the window stays where it is.
    pub fn go(&mut self, to: PathBuf) -> io::Result<()> {
        if to == self.dir() {
            return Ok(());
        }
        self.show(&to)?;
        self.history.visit(to);
        Ok(())
    }

    /// Go back; `false` when there is nothing to go back to.
    pub fn back(&mut self) -> io::Result<bool> {
        let Some(to) = self.history.back_target().map(Path::to_path_buf) else {
            return Ok(false);
        };
        self.show(&to)?;
        self.history.back();
        Ok(true)
    }

    /// Go forward; `false` when nothing was gone back from.
    pub fn forward(&mut self) -> io::Result<bool> {
        let Some(to) = self.history.forward_target().map(Path::to_path_buf) else {
            return Ok(false);
        };
        self.show(&to)?;
        self.history.forward();
        Ok(true)
    }

    /// Go to the enclosing folder and select the one the window came from; `false` at the root.
    pub fn parent(&mut self) -> io::Result<bool> {
        if self.is_virtual() {
            return Ok(false);
        }
        let here = self.dir().to_path_buf();
        let (Some(parent), Some(name)) = (here.parent(), here.file_name()) else {
            return Ok(false);
        };
        self.go(parent.to_path_buf())?;
        self.select_when_listed = Some(name.to_string_lossy().into_owned());
        self.select_pending();
        Ok(true)
    }

    /// Apply what the model's threads queued, keeping the selection on the same items.
    pub fn poll(&mut self) -> Polled {
        // cheap when nothing arrived: the window polls after every batch of input events
        if !self.queued.swap(false, Ordering::SeqCst) {
            return Polled::default();
        }
        let kept = self.selection_paths();
        let deltas = self.model.poll();
        let mut polled = Polled {
            changed: !deltas.is_empty(),
            errors: Vec::new(),
        };
        let mut reordered = false;
        let mut gone = false;
        for delta in deltas {
            match delta {
                ModelDelta::Reloaded { .. }
                | ModelDelta::Inserted { .. }
                | ModelDelta::Removed { .. }
                | ModelDelta::Moved { .. } => reordered = true,
                ModelDelta::LoadComplete { .. } | ModelDelta::Changed { .. } => {}
                ModelDelta::DirectoryGone => gone = true,
                ModelDelta::Error(e) => polled.errors.push(e),
            }
        }
        // an expanded folder that is gone also leaves its parent's listing, which drops it in the rebuild
        for (path, model) in &mut self.expanded {
            for delta in model.poll() {
                polled.changed = true;
                match delta {
                    ModelDelta::Reloaded { .. }
                    | ModelDelta::Inserted { .. }
                    | ModelDelta::Removed { .. }
                    | ModelDelta::Moved { .. } => reordered = true,
                    ModelDelta::LoadComplete { .. }
                    | ModelDelta::Changed { .. }
                    | ModelDelta::DirectoryGone => {}
                    ModelDelta::Error(e) => polled.errors.push(format!("{}: {e}", path.display())),
                }
            }
        }
        if polled.changed {
            self.rebuild();
        }
        if gone {
            match self.nearest_existing_ancestor() {
                Some(to) => match self.go(to) {
                    Ok(()) => return polled,
                    Err(e) => polled
                        .errors
                        .push(format!("cannot open the enclosing folder: {e}")),
                },
                None => polled
                    .errors
                    .push("the folder and every folder above it are gone".into()),
            }
        }
        if reordered {
            self.restore_selection(kept);
        }
        self.select_pending();
        self.select_awaited();
        self.reselect_pending();
        polled
    }

    fn nearest_existing_ancestor(&self) -> Option<PathBuf> {
        self.dir()
            .ancestors()
            .skip(1)
            .find(|p| p.is_dir())
            .map(Path::to_path_buf)
    }

    /// The paths of the selected, anchor and focus items: names repeat at different levels of the outline.
    fn selection_paths(&self) -> SelectionPaths {
        let entries = self.entries();
        let path = |i: usize| entries.get(i).map(|e| e.path.clone());
        (
            self.selection.indices().filter_map(path).collect(),
            self.selection.anchor().and_then(path),
            self.selection.focus().and_then(path),
        )
    }

    /// Select the items at these paths again at their new indexes; paths no longer displayed drop out.
    fn restore_selection(&mut self, (selected, anchor, focus): SelectionPaths) {
        let index: HashMap<&Path, usize> = self
            .entries()
            .iter()
            .enumerate()
            .map(|(i, e)| (e.path.as_path(), i))
            .collect();
        let at = |p: &PathBuf| index.get(p.as_path()).copied();
        let (selected, anchor, focus): (Vec<usize>, _, _) = (
            selected.iter().filter_map(at).collect(),
            anchor.as_ref().and_then(at),
            focus.as_ref().and_then(at),
        );
        self.selection.restore(selected, anchor, focus);
    }

    fn index_of_path(&self, path: &Path) -> Option<usize> {
        self.entries().iter().position(|e| e.path == path)
    }

    /// Select the item a step to the parent came from once it is listed; give up when the listing is complete
    /// without it.
    fn select_pending(&mut self) {
        let Some(name) = &self.select_when_listed else {
            return;
        };
        if let Some(i) = self.index_of_path(&self.dir().join(name)) {
            self.selection.set_single(i);
            self.focus_moved = true;
            self.select_when_listed = None;
        } else if !self.model.is_loading() {
            self.select_when_listed = None;
        }
    }

    /// Select the item at `path` as soon as the listing shows it, and scroll it into sight: the watcher reports the
    /// result of a rename or of a new folder a little after the change. A selection made in the meantime, or another folder,
    /// ends the wait.
    pub fn select_when_appears(&mut self, path: PathBuf) {
        self.awaited = Some(path);
        self.select_awaited();
    }

    fn select_awaited(&mut self) {
        let Some(i) = self
            .awaited
            .as_deref()
            .and_then(|path| self.index_of_path(path))
        else {
            return;
        };
        self.selection.set_single(i);
        self.focus_moved = true;
        self.awaited = None;
    }

    /// Order by `key`: the same key again reverses the order, another key starts in `ascending` order (the
    /// column's own default direction).
    pub fn sort_by(&mut self, key: SortKey, ascending: bool) {
        let ascending = if self.sort.key == key {
            !self.sort.ascending
        } else {
            ascending
        };
        self.set_sort(key, ascending);
    }

    /// A press on item `index` with `mods` held; `true` when the selection changed.
    pub fn press(&mut self, index: usize, mods: Modifiers) -> bool {
        self.type_select.clear();
        self.awaited = None;
        self.selection.mouse_down(index, mods) != MouseDown::Unchanged
    }

    /// The button went up without a drag; `true` when the selection changed.
    pub fn release(&mut self) -> bool {
        self.selection.mouse_up(false)
    }

    /// The button went up after a drag of the pressed items: the selection stays as it is.
    pub fn end_drag(&mut self) {
        self.selection.mouse_up(true);
    }

    /// A press on empty space; `true` when the selection changed.
    pub fn press_empty(&mut self, mods: Modifiers) -> bool {
        self.type_select.clear();
        self.awaited = None;
        self.selection.click_empty(mods)
    }

    /// Order by `key` in the given direction (a menu choice: unlike [`Browser::sort_by`] it never reverses).
    pub fn set_sort(&mut self, key: SortKey, ascending: bool) {
        let kept = self.selection_paths();
        self.sort = SortSpec {
            key,
            ascending,
            ..self.sort
        };
        self.model.set_sort(self.sort);
        for model in self.expanded.values_mut() {
            model.set_sort(self.sort);
        }
        self.rebuild();
        self.restore_selection(kept);
    }

    /// Keyboard focus to `index`; `extend` (Shift) selects the range from the anchor.
    pub fn move_focus(&mut self, index: usize, extend: bool) {
        if index < self.entries().len() {
            self.awaited = None;
            self.selection.move_focus(index, extend);
            self.focus_moved = true;
        }
    }

    pub fn select_all(&mut self) {
        self.selection.select_all(self.entries().len());
    }

    /// Select the items at `indices` as a list view chose them; indexes past the listing are dropped.
    pub fn select(
        &mut self,
        indices: impl IntoIterator<Item = usize>,
        anchor: Option<usize>,
        focus: Option<usize>,
    ) {
        let len = self.entries().len();
        let within = |i: &usize| *i < len;
        self.awaited = None;
        self.selection.restore(
            indices.into_iter().filter(within),
            anchor.filter(within),
            focus.filter(within),
        );
    }

    /// Typed text at `now` (seconds on a monotonic clock): selects the first item whose name starts with
    /// what was typed recently. `true` when the selection changed.
    pub fn type_text(&mut self, text: &str, now: f64) -> bool {
        // the fields, not `entries()`: type-select is borrowed mutably beside them
        let entries = if self.filter.is_some() || !self.expanded.is_empty() {
            &self.rows
        } else {
            self.model.entries()
        };
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        match self.type_select.type_text(text, now, &names) {
            Outcome::Select(i) => {
                self.move_focus(i, false);
                true
            }
            Outcome::Ignored | Outcome::NoMatch => false,
        }
    }

    /// What opening the selected items does: the first selected folder to go into, every selected file
    /// to open.
    pub fn activation(&self) -> Activation {
        let entries = self.entries();
        let mut out = Activation::default();
        for e in self.selection.indices().filter_map(|i| entries.get(i)) {
            match (e.is_dir, &out.folder) {
                (true, None) => out.folder = Some(e.path.clone()),
                (true, Some(_)) => {}
                (false, _) => out.files.push(e.path.clone()),
            }
        }
        out
    }
}

/// Whether the file system of `dir` is mounted read-only, whatever the medium under it says: a writable stick can
/// hold a file system mounted so.
pub fn mounted_read_only(dir: &Path) -> io::Result<bool> {
    Ok(file_system(dir)?.f_flag & libc::ST_RDONLY != 0)
}

fn file_system(dir: &Path) -> io::Result<libc::statvfs> {
    let dir = std::ffi::CString::new(dir.as_os_str().as_encoded_bytes())
        .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    // SAFETY: a zeroed statvfs is a valid out value; the path is NUL-terminated and outlives the call
    let mut fs: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(dir.as_ptr(), &mut fs) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(fs)
}

/// Whether `entry` is a result of the search `query` (lower case): its name contains it, or one of its tags is named it.
fn matches(entry: &Entry, query: &str) -> bool {
    entry.name.to_lowercase().contains(query)
        || entry.tags.iter().any(|t| t.to_lowercase() == query)
}
