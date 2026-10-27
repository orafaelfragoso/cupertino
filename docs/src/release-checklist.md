# Check a release candidate on hardware

An rc passes when hosted CI is green for its exact commit and Rafael's PC passes this checklist on a real TTY. A nested or headless run does not replace the hardware checks. Record each result as **pass**, **fail**, or **blocked**, with the observed result. An unchecked or blocked item does not pass. Link any exception to Orion's explicit release decision.

Use the same candidate for the checkout and installed Arch packages. Have a working Wayland terminal available. Use disposable files, save other work before quitting, and record settings before changing them so you can restore them afterward. `Super` means the keyboard's primary desktop modifier.

## Scope of this gate

The following known-missing features are outside this rc gate. Their absence is not a failed check or a claim that the feature works.

- The session power and logout menu ([#19](https://github.com/orafaelfragoso/cupertino/issues/19)), background-window focus protection, hung-app detection, and validation of unsolicited move or resize requests ([#20](https://github.com/orafaelfragoso/cupertino/issues/20)).
- Audio-server device selection and media controls ([#21](https://github.com/orafaelfragoso/cupertino/issues/21)), and Bluetooth headset audio ([#26](https://github.com/orafaelfragoso/cupertino/issues/26)).
- Multiple-monitor arrangement and per-output scaling ([#22](https://github.com/orafaelfragoso/cupertino/issues/22)), and multi-GPU outputs and explicit synchronization ([#27](https://github.com/orafaelfragoso/cupertino/issues/27)).
- Portals, administrator prompts, and credential storage ([#23](https://github.com/orafaelfragoso/cupertino/issues/23)), and restrictions on privileged interfaces ([#28](https://github.com/orafaelfragoso/cupertino/issues/28)). This gate does not establish application isolation.
- External input methods and a menu-bar input-source chooser ([#24](https://github.com/orafaelfragoso/cupertino/issues/24)), and pointer constraints and shortcut inhibition ([#25](https://github.com/orafaelfragoso/cupertino/issues/25)). Check 9 covers the existing keyboard-layout HUD only.

## Run the checks

Run the checks in order. Keep the original text-console login open for the final log collection.

1. Record the candidate and hosted CI result. From the candidate checkout on a text console, run:

   ```sh
   report=$(mktemp -d "$HOME/cupertino-rc.XXXXXX")
   started=$(date --iso-8601=seconds)
   git rev-parse HEAD > "$report/commit.txt"
   uname -a > "$report/kernel.txt"
   df -h /home > "$report/disk.txt"
   tty > "$report/source-vt.txt"
   lspci -k > "$report/hardware.txt"
   pacman -Q cupertino cupertino-data > "$report/packages.txt"
   printf 'Report directory: %s\n' "$report"
   ```

   Open the candidate's commit on GitHub and record the hosted CI run URL in the report. Expected: every required CI job succeeds for that SHA, both installed packages come from the same candidate, and `/home` has at least 30 GB free before the source build. A green run for an earlier commit does not pass. Guards [#13](https://github.com/orafaelfragoso/cupertino/issues/13) and [#14](https://github.com/orafaelfragoso/cupertino/issues/14).

2. Start the checkout from the logged-in text console with `./run-cupertino.sh`. Expected: the compositor takes the display and shows the dock, menu bar, Files, and Settings. Both windows accept input. The script prints `~/cupertino-tty.log`; a build failure or a blank desktop fails this check. Record display resolution, refresh rate, and scale. Guards [#9](https://github.com/orafaelfragoso/cupertino/issues/9).

3. Switch to a spare console with `Ctrl+Alt+F4`, log in as the same user, and run `ps -u "$USER" -o pid=,args= | grep '[c]upertino-'`. Return to the source VT recorded in step 1. Expected: `cupertino-server` and the eight shell processes `cupertino-dock`, `cupertino-menubar`, `cupertino-launcher`, `cupertino-notifications`, `cupertino-controlcenter`, `cupertino-osd`, `cupertino-windowmanager`, and `cupertino-login` are present for this session. Files and Settings are also present. Repeated process exits or restarts fail. Guards [#17](https://github.com/orafaelfragoso/cupertino/issues/17).

4. Move the mouse slowly onto a window-close button without clicking it, then move quickly across the display and back. Expected: small corrections remain controllable, fast motion covers useful distance, and the pointer does not jump or drift after the mouse stops. Record the mouse model, hardware DPI, and display scale. Guards [#1](https://github.com/orafaelfragoso/cupertino/issues/1).

5. Open **Settings > Mouse**. Set **Mouse resolution** to the mouse's known hardware DPI, using the named device row when more than one mouse is attached. Compare a fixed physical movement at two known hardware DPI settings, with the matching resolution selected each time. Expected: matching DPI preserves comparable pointer travel and the selected resolution remains visible after leaving and reopening the pane. If the hardware DPI cannot be established, record this comparison as blocked. Guards [#1](https://github.com/orafaelfragoso/cupertino/issues/1).

6. In **Mouse**, move **Tracking speed** to a lower stop, then a higher stop. Toggle **Pointer acceleration** off and on. Restore the original values. Expected: speed changes immediately; with acceleration off, equal physical distances give comparable pointer travel at slow and fast speeds. With acceleration on, fast movement travels farther without losing slow precision. Guards [#1](https://github.com/orafaelfragoso/cupertino/issues/1).

7. Open a Files folder with enough entries to scroll, then scroll the Accessibility pane in Settings. Turn the wheel one notch at a time. Expected: a notch moves a useful amount, about three text lines where applicable; both apps scroll consistently and stop when input stops. Guards [#2](https://github.com/orafaelfragoso/cupertino/issues/2).

8. In **Mouse**, turn **Natural scrolling** off, scroll Files, then turn it on and repeat the same wheel movement. Restore the original value. Expected: direction reverses immediately without restarting either app. If a trackpad is available, repeat with its **Natural scrolling** control and a two-finger gesture; record the trackpad result separately. Guards [#3](https://github.com/orafaelfragoso/cupertino/issues/3).

9. Record the current `input_sources` entry in `~/.config/cupertino/keyboard.conf`. Set only that entry to `input_sources = us, de`. Focus Settings search and use `Ctrl+Space` to switch sources. Press the physical key immediately right of **T** in each source. Expected: the input-source switcher HUD changes its selected source, the key produces `y` in US and `z` in German, and switching requires no app restart. Restore the original entry after checking both sources.

10. Press `Super+Space`, type `settings`, and press Escape. Hold `Super+Tab`, select Files, and release Super. Press `Super+W` to close that Files window, then reopen Files from the dock. Expected: the launcher receives the text and closes on Escape; the switcher activates the chosen app on release; the close shortcut affects only the focused window.

11. In Settings search, type `alpha beta`. Press `Ctrl+A`, type `gamma`, move the caret with Left and Right, then click between two characters and type `X`. Repeat selection and replacement in the launcher and the Files rename editor. Expected: the caret is visible, the selected range is highlighted, typing replaces the selection, and clicks place the caret at the clicked character boundary. Cancel the rename. Guards [#4](https://github.com/orafaelfragoso/cupertino/issues/4) and [#16](https://github.com/orafaelfragoso/cupertino/issues/16).

12. Open **Settings > Accessibility > Colour filter**. Click the Appearance sidebar row once while the menu is open. Expected: the menu closes, the selected filter stays unchanged, and the pane remains Accessibility. A second click opens Appearance. Reopen the menu and navigate with `Super+[` or `Super+]` where history is available. Expected: a pane change leaves no popup from the previous pane. Guards [#5](https://github.com/orafaelfragoso/cupertino/issues/5).

13. Reopen **Colour filter**. Use Down and Up, type the beginning of an item name, and press Escape. Reopen it and select an item with Return, then restore the original value. Expected: highlighting follows the keys, typing selects a matching item, Escape cancels, Return chooses, and the pane beneath receives none of those keys. Guards [#18](https://github.com/orafaelfragoso/cupertino/issues/18).

14. In a terminal opened with `Super+Return`, run `mktemp -d "$HOME/cupertino-files-rc.XXXXXX"` and record the printed path. Open that empty folder in Files. Create a folder, rename it to `rc-renamed`, open it, and go back. Expected: names and location update immediately, back navigation returns to the parent, and the selection remains visible. Use only these disposable items.

15. Focus Files, then Settings, by clicking their windows. Open each app's name menu and its **Window** menu. Expected: the menu bar identifies the focused app, each opened menu responds, and dismissal returns keyboard input to that app. Record missing app-specific commands against [#6](https://github.com/orafaelfragoso/cupertino/issues/6); do not treat a visible label as proof that a command works.

16. Open **Settings > Wi-Fi**, then **Bluetooth**. Expected when `cupertino-netd` and `cupertino-btd` are not running: each pane says "Wi-Fi is not available" or "Bluetooth is not available" respectively. These daemons are not installed in this test setup; those messages pass, and connecting or pairing is outside this gate. Then search for `natural scrolling`, choose the Mouse result, change the setting, leave the pane, and return. Restore the value. Expected: the result reaches Mouse, the stored control agrees with Files, and navigation leaves no search popup. Raw bus errors, a frozen control, or a value that silently resets fail. Guards [#3](https://github.com/orafaelfragoso/cupertino/issues/3) and [#5](https://github.com/orafaelfragoso/cupertino/issues/5).

17. Place Files and Settings side by side. Change **Settings > Appearance** from Light to Dark and back. Repeat after opening a Colour filter popup. Expected: both apps, the menu bar, and dock update live; Files and Settings share the content-background treatment in each appearance; text remains readable; no old popup survives. Guards [#8](https://github.com/orafaelfragoso/cupertino/issues/8) and [#5](https://github.com/orafaelfragoso/cupertino/issues/5).

18. Drag a window by its title bar and resize it from a corner. Release the mouse, then move the pointer away. Expected: the window follows while held, stops on release, and the pointer remains free. Typing after focusing the other app reaches that app. This checks ordinary held-button dragging only. The missing request validation, background-focus protection, and hung-app detection in [#20](https://github.com/orafaelfragoso/cupertino/issues/20) remain outside this gate.

19. Press `Super+Ctrl+Q`. Try typing and clicking before unlocking, then enter one incorrect password followed by the correct password. Expected: the lock screen covers the desktop, input never reaches the apps, the incorrect password keeps the lock, and the correct password restores the same windows and usable input. Guards [#17](https://github.com/orafaelfragoso/cupertino/issues/17).

20. Switch to the spare text console with `Ctrl+Alt+F4` and back to the recorded source VT. Repeat once while locked, then unlock. Expected: the console and desktop both redraw, the pointer and keyboard work after each return, and a locked session stays locked. Black output, stale frames, or lost input fail. Guards [#9](https://github.com/orafaelfragoso/cupertino/issues/9).

21. Save any work and press `Ctrl+Alt+Backspace` to end the checkout run. On the original console, run `cp "$HOME/cupertino-tty.log" "$report/source-tty.log"`. Expected: the script returns to that console without a crash, and the compositor and its shell children have exited. Check with the process command from step 3. This shortcut tests compositor exit, not a confirmed user logout.

22. Return to the machine's existing display manager, choose its **Cupertino** session, and log in. Do not enable or replace a display manager for this check. Expected: the packaged session entry starts `/usr/bin/cupertino-session` and reaches the desktop without `run-cupertino.sh` or a checkout on `PATH`. Open Files and Settings from the dock or launcher. If there is no configured session chooser, mark this check blocked rather than substituting the checkout script. Guards [#13](https://github.com/orafaelfragoso/cupertino/issues/13) and [#17](https://github.com/orafaelfragoso/cupertino/issues/17).

23. In the packaged session, repeat the process inventory from step 3, the launcher and switcher actions from step 10, and lock, unlock, and VT return from steps 19–20. Record each result separately. Expected: all eight packaged shell processes run, their executable paths resolve to installed binaries, and the desktop remains usable after unlock and VT return. Do not accept the checkout run as evidence for this path. Guards [#17](https://github.com/orafaelfragoso/cupertino/issues/17) and [#9](https://github.com/orafaelfragoso/cupertino/issues/9).

24. Save work in the packaged session and press `Ctrl+Alt+Backspace`. Expected: the compositor and its shell processes exit, the display-manager session ends, and its login screen returns. Verify from the spare console with the process command in step 3. No in-session Log Out, Restart, or Shut Down menu is expected: that UI is known-missing under [#19](https://github.com/orafaelfragoso/cupertino/issues/19), not a failure of this check.

25. On the original text console, collect the packaged-session and login-manager journal since the recorded start time:

   ```sh
   sudo journalctl -b --since "$started" _UID="$(id -u)" > "$report/user-journal.log"
   sudo journalctl -b --since "$started" \
     -u display-manager.service -u cupertino-login.service > "$report/login-journal.log"
   ```

   Expected: the report contains the exact SHA, hosted CI URL, package versions, hardware details, per-check results, and `source-tty.log`. For each failure, attach the relevant log and include its check number, timestamp, expected result, observed result, and reproducible action. If the display manager redirects session stderr to a file instead of the journal, attach that configured file too; an empty journal is not proof of a clean run. Include both startup warnings from [#9](https://github.com/orafaelfragoso/cupertino/issues/9) if present. Review logs for private data before sharing. Restore the recorded input and appearance settings. Record who ran the gate and the date; leave unexecuted checks marked blocked.
