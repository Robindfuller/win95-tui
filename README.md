# win95-tui

A little desktop that runs in your terminal: pixel-art desktop icons, a taskbar
with an Apps menu, and overlapping windows you can drag, resize, minimise and
maximise, all in your Omarchy theme colours.

Apps inside it:

- **Terminal**: a real shell in a window. Anything runs in it (nvim, btop, lazygit, even this).
- **Notes**, **Mines**, **Files** (file browser).
- **Solitaire**: classic Klondike on green felt. Drag cards, or click one then
  click where it goes; the card you're over turns inverted when the move is
  allowed. Double-click sends a card up top, right-click sends everything that
  can go. Draw one or three, undo, Windows scoring, and the cards bounce off
  the table when you win. Keys: arrows move, Space picks up and puts down,
  Enter sends a card up top, U undoes, F2 deals again.

In Notes, drag, Shift+arrows or Shift+click to select (double-click for a
word, Ctrl+A for all), and Ctrl+C, Ctrl+X and Ctrl+V to copy, cut and paste.
Over SSH, copied text goes to your own terminal's clipboard instead, and you
paste with the terminal's paste key. File > Open Folder... puts a folder down
the left, like Sublime Text: click a folder to open it out, a file to open it
(in a tab of its own unless the one you're on is empty). Ctrl+E moves the keys
into the list and back. File > Close Folder takes it away again.

File > Open... in Notes, Paint and Media Player opens what you pick in that
same window.

In Files, F2 (or right-click > Rename) renames a file; typing replaces the
name but keeps its extension, Esc leaves it as it was.  Ctrl+C, Ctrl+X and Ctrl+V copy, cut and paste files using the
desktop clipboard, so they go between Files windows and to or from Nautilus.
Space or Ctrl+click picks several, Ctrl+A picks them all. A paste never
overwrites: a clash becomes "name (copy)". This needs wl-clipboard, so it
doesn't work over SSH.
- **Paint**: pencil, brush, eraser, fill, line and rectangle with the Windows
  95 colours; left and right click paint in two colours, Ctrl+Z undoes, and
  pictures save as .bmp. Each pixel is two cells wide so it comes out square.
- **Amp**: a Winamp-style music and internet radio player, played by mpv.
  The screen has a big clock, the scrolling title, bitrate and a spectrum
  (or scope) of what's playing; under it the seek bar, buttons, volume and
  balance, then a ten-band equaliser with presets, then the playlist and a
  Radio tab that searches radio-browser.info's free station list. The
  Winamp keys work (Z X C V B, arrows to skip), F favourites a station,
  D makes it one line, and EQ and PL fold the panels away. Drop files or
  folders on it, or open music from Files, and they join the playlist of
  the one Amp window. The playlist, favourites and settings are kept in
  ~/.config/win95-tui/amp. It needs mpv; the visualiser uses parec.
- **Browser**: the web, drawn the desktop's way. Each site keeps its logo,
  menu and colour, but the page is in your theme, with pictures in chunky
  pixels. Click links (middle-click for a new tab), type an address or a
  search after Ctrl+L, Tab and Enter to go link by link, Alt+arrows or
  Backspace for back and forward. Pages open in a hidden Chromium, so most
  sites work, including ones behind a Cloudflare check; without Chromium it
  fetches pages plainly and fewer sites let it in. Searches go to Brave.
- **Doom**: the real Doom, in chunky half-block pixels. It's played by
  tui-doom, a separate program kept apart because Doom's code is
  GPL; install that and Doom turns up in Apps > Programs. Arrows move, Ctrl or
  left click fires, Space or right click opens doors, Shift runs, Esc is the
  menu. Keys work best in a terminal with the kitty keyboard protocol (Ghostty,
  Kitty, Alacritty, foot), which tells it when a key is let go.
- **Media Player**: plays video (MPG, MP4, MKV, AVI and the rest) in chunky
  half-block pixels, laid out like the Windows 95 one with the track bar, its
  scale and the transport buttons. ffmpeg draws the picture and mpv plays the
  sound. Open a video from Files, or File > Open... in the player. Space plays
  and pauses, S stops, arrows step 5 seconds, I and O mark a selection.
- **Run...** opens any command in its own window.

Add your own with Apps > Add Program...: type the command that starts it and
paint it an icon (click or drag to paint, right-click rubs out). It goes in
Apps > Programs with a shortcut on the desktop, named after the command, and
Apps > Programs > Remove Program takes it off. They're kept in
~/.config/win95-tui/programs (`name = command` lines) and icons/. A command
that fails keeps its window open so you can read why.

Terminal, Files and Notes open more in tabs: File > New Tab or Ctrl+Shift+T,
Ctrl+Shift+W closes one, Ctrl+PgUp/PgDn switch. The tabs show along the top
once there are two; click × (or middle-click) to close one and + for another.
Right-click a folder in Files for Open in New Tab.

Files opens folders in Files, .bmp pictures in Paint, text in Notes and
anything else with the system's own app. Right-click a file for Create
Shortcut (or File > Create Shortcut) to put it on the desktop, where it opens
the same way.

Drag desktop icons around; they snap to a grid. Drag a box over the desktop
to select several (Ctrl+click adds or drops one) and drag them all at once.
Right-click one (or select them and press Delete) to take them off the desktop; right-click the desktop for Add Icon to put
any app back, and Arrange Icons to line them up by name or in their current
order. Where they are is kept in ~/.config/win95-tui/desktop.

Apps > Settings > Background (or right-click the desktop) picks a picture to
Fill, Fit or Stretch across the desktop, fades it towards a colour, and picks
that colour: the theme's, Windows teal, greys, the theme's colours or any hex.
Omarchy uses your current Omarchy wallpaper and follows theme switches, and
◂ ▸ step through the pictures beside it. Shade behind icons puts a patch of
the theme's background behind each icon so they stay readable. Changes show as
you make them; Cancel puts it back.

The speaker in the tray (on the top bar in the Omarchy desktop) opens a volume
slider with a Mute box; the mouse wheel over it changes the volume too. It
uses wpctl, so it shows when PipeWire is there.

Drag a window against the far left or right edge to fill that half of the
screen, into a corner for a quarter, or against the top to maximise. An outline
shows where it will land. Drag it off again and it goes back to its own size.

Keys: Ctrl+Esc or Alt+S for the Apps menu, Alt+Tab or Alt+` to switch, Alt+F4 to
close, Alt+Space for the window menu, Ctrl+Alt+Del for the task list,
Shift+PgUp/PgDn to scroll back in a terminal. With the Apps menu open, just
start typing to search it; Enter opens the top match, Esc clears the search.

## One app on its own

`win95 amp`, `win95 mplayer video.mpg`, `win95 notes todo.txt` and the like
run just that app, filling the terminal with no title bar, taskbar or session;
closing it (File > Exit, or Alt+F4) ends it. The names are amp, mplayer, notes,
paint, mines, solitaire, doom, browser and files.

From a running desktop, the ⊡ button on a window's title bar (or Just This App
on its right-click menu, or Alt+J) collapses everything down to that one app:
it fills the terminal under its title bar and the taskbar goes. Press ⊡ again,
double-click the title, or Ctrl+Esc to bring the desktop back. Opening another
app or closing this one brings it back too.

## Sessions

win95 keeps running in the background, like tmux or herdr. Close the terminal
and your windows and shells carry on; type `win95` again and you're back where
you were, at whatever size the new terminal is. Opening it in a second terminal
moves it there. Apps > Quit offers Detach (leave it running) or Quit (close
everything), and `win95 stop` quits it from outside. `win95 --here` runs one
that lives and dies with its terminal, the old way.

After installing a new win95, Quit the running one so the next start picks up
the new version; it tells you when there's a newer one waiting.

## Omarchy desktop

Apps > Settings > Omarchy Desktop swaps the overlapping windows for a tiling
desktop like Omarchy's own: a bar along the top (Apps menu, workspaces,
clock, battery, power), windows tiled Hyprland's dwindle way, and a dock that
slides up when the pointer reaches the bottom edge (it stays up on an empty
workspace). Dialogs and fixed-size apps like Mines and Solitaire float over the tiles.

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
