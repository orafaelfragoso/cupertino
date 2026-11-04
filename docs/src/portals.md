# Portals

Cupertino ships `cupertino-portal`, an xdg-desktop-portal backend. Sandboxed and unsandboxed apps use it to ask for a file, a screen share, a screenshot or a choice of app. The server sets `XDG_CURRENT_DESKTOP=Cupertino`, so xdg-desktop-portal picks it. The crate's README (`crates/cupertino-portal/README.md`) has the protocol details; this page is the overview.

## What it serves

| Interface | What it does |
|---|---|
| Settings | Three appearance keys for apps that read them: the colour scheme (light or dark), contrast, and reduced motion. A change is announced to apps. |
| FileChooser | Open and Save As windows drawn with CupertinoKit. |
| ScreenCast | Shares one display or one window as a PipeWire video stream after you choose the source in a window. |
| AppChooser | Asks which app opens a link or a file, when xdg-desktop-portal's OpenURI needs you to choose. |
| Screenshot | Asks before it takes a screenshot of every display for an app. |

The routing file `cupertino-portals.conf` sends Settings and FileChooser to Cupertino first and then the GTK backend, sends ScreenCast, AppChooser and Screenshot to Cupertino only, and sends every other interface to the GTK backend.

## App chooser (OpenURI)

A window titled "Open “NAME” with:" lists the apps offered, by the name of their desktop entry. Return opens the selected app, Escape cancels. Nothing opens unless you choose an app. The window opens with the app the caller named as its last choice selected, if it is on the list. Each row shows the app's own icon, or a generic applications symbol when the app has none. The app you open comes to the front. Limits: an app whose icon is given as a file path shows the generic symbol.

## Screenshot

A window says "NAME wants to take a screenshot of your screen." Nothing is preselected, and no key takes the screenshot, so a key typed for the app cannot consent. Escape cancels. After you agree, the window gets 300 ms to leave the screen, then one frame of each display is joined into one PNG, without the pointer. The file goes to your Pictures folder (`XDG_PICTURES_DIR`, else `~/Pictures`) as `Screenshot DATE at TIME.png`, with ` (2)`, ` (3)` added when the name is taken. It is created new, never replaces a file, and only you can read it.

After consent, capture has at most three attempts, with a 100 ms delay before retrying a transient failure (#75). A permanent failure is returned to the requesting application.

Limits: the whole screen only (no window or area), no pointer in the image, no colour picker (`PickColor` always answers that it was cancelled), and a rotated display is not turned upright.

## Limits that apply to all of them

Only xdg-desktop-portal may call the backend. An app has at most four windows open at once and the whole backend 16; shown text loses control and bidirectional-override characters. ScreenCast and Screenshot are offered only when the server offers the image-capture protocols. For ScreenCast's own limits (shared memory only, no restore tokens, no audio), see the crate README.
