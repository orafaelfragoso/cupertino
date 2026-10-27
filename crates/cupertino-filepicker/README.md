# cupertino-filepicker

The model of a file chooser, with no drawing, for every window that lets the user pick a folder or a picture: the
Settings wallpaper pane today, the file-chooser portal later.

- `Picker::open(start, mode)` lists a folder; `Mode::Folder` shows folders, `Mode::Photo` shows folders and PNG and JPEG
  files. Folders come first, then files, each by name without regard to case. Names that start with a dot are hidden
  until `set_hidden(true)`.
- `Picker::key(key, now)` takes the keys of the chooser: Up, Down, Home and End move the highlight (no wrap); Return
  enters the highlighted folder or chooses the highlighted file; Super+Return (`Key::Choose`) chooses (in `Folder`
  mode the highlighted folder, else the open one); Super+Up (`Key::Parent`) opens the parent folder; Escape cancels; a
  typed character jumps to the next entry whose name starts with the text typed so far (`cupertino-typeahead`).
- A folder that cannot be read leaves the picker where it was and sets `unreadable`, which the view words plainly.
- The result is `Outcome::Choose(path)`, `Cancel`, `Changed` (redraw) or `Ignore`.
