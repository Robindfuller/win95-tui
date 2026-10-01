mod apps;
mod draw;
mod icons;
mod menu;
mod theme;
mod wm;

use crossterm::{
    event::{self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags},
    execute,
};
use std::{
    io::stdout,
    time::{Duration, Instant},
};

fn main() -> anyhow::Result<()> {
    let mut terminal = ratatui::init();
    execute!(stdout(), EnableMouseCapture, EnableBracketedPaste)?;
    let kb = crossterm::terminal::supports_keyboard_enhancement().unwrap_or(false);
    if kb {
        execute!(stdout(), PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES))?;
    }
    let size = terminal.size()?;
    let mut desk = wm::Desktop::new(size.width, size.height);
    let res = run(&mut terminal, &mut desk);
    if kb {
        let _ = execute!(stdout(), PopKeyboardEnhancementFlags);
    }
    let _ = execute!(stdout(), DisableMouseCapture, DisableBracketedPaste);
    ratatui::restore();
    res
}

fn run(terminal: &mut ratatui::DefaultTerminal, desk: &mut wm::Desktop) -> anyhow::Result<()> {
    let frame = Duration::from_millis(16);
    let mut dirty = true;
    let mut last = Instant::now() - frame;
    loop {
        if desk.quit {
            return Ok(());
        }
        if dirty && last.elapsed() >= frame {
            terminal.draw(|f| desk.render(f))?;
            last = Instant::now();
            dirty = false;
        }
        let wait = if dirty { frame.saturating_sub(last.elapsed()) } else { Duration::from_millis(25) };
        if event::poll(wait)? {
            loop {
                desk.event(event::read()?);
                dirty = true;
                if !event::poll(Duration::ZERO)? {
                    break;
                }
            }
        }
        dirty |= desk.tick();
    }
}
