# win95-tui

A little desktop that runs in your terminal: line-art desktop icons, a taskbar
with an Apps menu, and overlapping windows you can drag, resize, minimise and
maximise. No backgrounds by default, just lines in your Omarchy theme colours.

Apps inside it:

- **Terminal**: a real shell in a window. Anything runs in it (nvim, btop, lazygit, even this).
- **Notes**, **Mines**, **Files** (file browser).
- **Run...** opens any command in its own window.

Keys: Ctrl+Esc or Alt+S for the Apps menu, Alt+Tab or Alt+` to switch, Alt+F4 to
close, Alt+Space for the window menu, Ctrl+Alt+Del for the task list,
Shift+PgUp/PgDn to scroll back in a terminal.

Alt+L switches between the Lite look (default) and a chunkier Classic look with
filled windows. Also under Apps > Settings, and `--lite` / `--classic` on the
command line. The choice is saved in ~/.config/win95-tui/settings.

It's a plain terminal program, so it works the same over SSH.

    cargo build --release && cp target/release/win95 ~/.local/bin/
