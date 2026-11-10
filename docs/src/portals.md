# Portals

Cupertino ships `cupertino-portal`, an xdg-desktop-portal backend. Sandboxed and unsandboxed apps use it to ask for a file, a screen share, a screenshot or a choice of app. The server sets `XDG_CURRENT_DESKTOP=Cupertino`, so xdg-desktop-portal picks it. The crate's README (`crates/cupertino-portal/README.md`) has the protocol details; this page is the overview.

## What it serves

| Interface | What it does |
|---|---|
| Settings | Three appearance keys for apps that read them: the colour scheme (light or dark), contrast, and reduced motion. A change is announced to apps. |
| FileChooser | Open and Save As windows drawn with CupertinoKit. |
| ScreenCast | Shares one display or one window as a PipeWire video stream after you choose the source in a window; it can remember your choice for that app. |
| AppChooser | Asks which app opens a link or a file, when xdg-desktop-portal's OpenURI needs you to choose. |
| Screenshot | Asks before it takes a screenshot of every display for an app, and lets you pick a colour on screen for an app. |

The routing file `cupertino-portals.conf` sends Settings and FileChooser to Cupertino first and then the GTK backend, sends ScreenCast, AppChooser and Screenshot to Cupertino only, sends Secret (the saved passwords of sandboxed apps) to gnome-keyring, and sends every other interface to the GTK backend. The packages install it as `packaging/files/usr/share/xdg-desktop-portal/cupertino-portals.conf`, and the portal crate keeps the same file for a manual install (`crates/cupertino-portal/data/cupertino-portals.conf`); a test checks both against the interfaces the backend serves. Saved passwords themselves are described in [Using Cupertino every day](daily-use.md#saved-passwords).

## App chooser (OpenURI)

A window titled "Open “NAME” with:" lists the apps offered, by the name of their desktop entry. Return opens the selected app, Escape cancels. Nothing opens unless you choose an app. The window opens with the app the caller named as its last choice selected, if it is on the list. Each row shows the app's own icon, or a generic applications symbol when the app has none. The app you open comes to the front. Limits: an app whose icon is given as a file path shows the generic symbol.

## Screen sharing

External recorders such as OBS can consume this portal using Screen Capture (PipeWire); the Cupertino screenshot tool itself has no video recorder. Commit `fe28a392` fixes OBS capture failing after source selection with PipeWire's `error alloc buffers: Invalid argument`: the producer now negotiates memory types as flags instead of a fixed integer, so its shared-memory offer intersects OBS's supported-type mask. The regression consumer fails with the old code and receives frames with the fix; a locally rebuilt and restarted portal produced the 2026-11-10 review recording. This is evidence for that installed OBS path, not completion of all browser, output-transform, performance, audio or packaging checks. See the [capture regression record](desktop-quality.md#closed-screen-capture-regression).

A window says "“NAME” wants to share your screen. Choose a display or a window." with the app's name and icon from its desktop entry. Nothing is selected. The keyboard focus starts on Cancel, and a screen reader reads Cancel first. For half a second after the window first appears on screen nothing you click or type can share, and Return and Space press no button, so a key meant for the app neither shares nor cancels; a key held down from before never shares: Return acts only on a fresh press. After that, Return presses the focused button (with the focus in the list, Share). Escape or closing the window cancels. When the app asks to keep the choice, a check box offers "Let “NAME” share it again without asking"; it starts unticked. A remembered window lasts until the app quits; a remembered display lasts until you forget it in Settings › Privacy › Screen Sharing. A remembered choice is for that app only, as xdg-desktop-portal identifies it.

While anything is shared, a Screen Sharing item in the menu bar says which app shares what and has Stop Sharing. Stop Sharing ends the share but keeps a remembered display: only Forget in Settings takes that back. A remembered window ends with the share. The server's privacy dot shows besides. If the app quits, the share and its item end with it.

## Screenshot

A window says "NAME wants to take a screenshot of your screen." The keyboard focus starts on Cancel, and the Take Screenshot button works only half a second after the window first appears on screen; until then Return and Space press no button, so a key or a click meant for the app cannot consent. After that, Return presses the focused button: Tab to Take Screenshot first. Escape or closing the window cancels. After you agree, the window gets 300 ms to leave the screen, then one frame of each display is joined into one PNG, without the pointer. The file goes to your Pictures folder (`XDG_PICTURES_DIR`, else `~/Pictures`) as `Screenshot DATE at TIME.png`, with ` (2)`, ` (3)` added when the name is taken. It is created new, never replaces a file, and only you can read it.

After consent, capture has at most three attempts, with a 100 ms delay before retrying a transient failure (#75). A permanent failure is returned to the requesting application.

A rotated or flipped display is turned upright, and displays at different scales are joined where they sit.

## Colour picker

When an app asks for a colour, the screen freezes under an overlay with a crosshair and a loupe that magnifies the pixels round the pointer. Click a pixel to give the app its colour (red, green and blue from 0 to 1); Escape cancels. Picking is the consent: no window asks first, and the app learns only the one colour you clicked. A click in the first half second after the overlay appears on that display, or a button held from before the overlay appeared, picks nothing. Plugging in, unplugging or moving a display during a pick ends it without a colour, and only one pick runs at a time.

Limits: the whole screen only (no window or area), no pointer in the image, and a picked colour ignores the display's colour profile.

## Limits that apply to all of them

Only xdg-desktop-portal may call the backend. An app has at most four windows open at once and the whole backend 16; shown text loses control and bidirectional-override characters. ScreenCast and Screenshot are offered only when the server offers the image-capture protocols. For ScreenCast's own limits (shared memory only, no audio), see the crate README.
