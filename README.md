# win95-tui

A little desktop that runs in your terminal: line-art desktop icons, a taskbar
with an Apps menu, and overlapping windows you can drag, resize, minimise and
maximise. No backgrounds by default, just lines in your Omarchy theme colours.

Apps inside it:

- **Terminal**: a real shell in a window. Anything runs in it (nvim, btop, lazygit, even this).
- **Notes**, **Mines**, **Files** (file browser).
- **Web**: lynx (or w3m) in a window, when one is installed.
- **Run...** opens any command in its own window.

Add your own with Apps > Add Program...: type the command that starts it and
paint it an icon (click or drag to paint, right-click rubs out). It goes at the
top of the Apps menu and on the desktop, named after the command, and Apps >
Programs > Remove Program takes it off. They're kept in
~/.config/win95-tui/programs (`name = command` lines) and icons/. A command
that fails keeps its window open so you can read why.

Drag a window against the far left or right edge to fill that half of the
screen, into a corner for a quarter, or against the top to maximise. An outline
shows where it will land. Drag it off again and it goes back to its own size.

Keys: Ctrl+Esc or Alt+S for the Apps menu, Alt+Tab or Alt+` to switch, Alt+F4 to
close, Alt+Space for the window menu, Ctrl+Alt+Del for the task list,
Shift+PgUp/PgDn to scroll back in a terminal.

Alt+L switches between the Lite look (default) and a chunkier Classic look with
filled windows. Also under Apps > Settings, and `--lite` / `--classic` on the
command line. The choice is saved in ~/.config/win95-tui/settings.

## Omarchy desktop

Apps > Settings > Omarchy Desktop swaps the overlapping windows for a tiling
desktop like Omarchy's own: a bar along the top (Apps menu, workspaces,
clock, battery, power), windows tiled Hyprland's dwindle way, and a dock that
slides up when the pointer reaches the bottom edge (it stays up on an empty
workspace). Dialogs and fixed-size apps like Mines float over the tiles.

Keys use Alt, or Super where your terminal passes it on (Hyprland keeps its
own Super bindings for itself):

    Alt+Enter      Terminal             Alt+W          Close window
    Alt+Space      Launcher             Alt+F          Fullscreen
    Alt+1..9       Go to workspace      Alt+T          Float or tile
    Alt+Shift+1..9 Send window there    Alt+Arrows     Focus (Shift swaps)

With the mouse: drag the line between two tiles to resize them, drag a
title onto another window to swap them, double-click a title for
fullscreen. In the dock, click an app to start it or bring its window up
(again for its next one), right-click for a new one. Dots show what's open.

`--omarchy` / `--windows` pick one for a single run; the menu choice is saved
in ~/.config/win95-tui/settings alongside the look.

It's a plain terminal program, so it works the same over SSH.

    cargo build --release && cp target/release/win95 ~/.local/bin/
