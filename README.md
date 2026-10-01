# win95-tui

A Windows 95 style desktop that runs in your terminal. Desktop icons, a taskbar
with a Start menu, and overlapping windows you can drag, resize, minimise and
maximise. Colours come from the current Omarchy theme and follow theme switches.

Apps inside it:

- **MS-DOS Prompt**: a real shell in a window. Anything runs in it (nvim, btop, lazygit, even win95 itself).
- **Notepad**, **Minesweeper**, **Windows Explorer** (file browser).
- **Run...** opens any command in its own window.

Keys: Ctrl+Esc or Alt+S for Start, Alt+Tab or Alt+` to switch, Alt+F4 to close,
Alt+Space for the window menu, Ctrl+Alt+Del for Close Program,
Shift+PgUp/PgDn to scroll back in a prompt.

It's a plain terminal program, so it works the same over SSH.

    cargo build --release && cp target/release/win95 ~/.local/bin/
