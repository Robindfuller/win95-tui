mod apps;
mod assoc;
mod clip;
mod desktop;
mod draw;
mod icons;
mod menu;
mod programs;
mod session;
mod theme;
mod volume;
mod wallpaper;
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
    let args: Vec<String> = std::env::args().collect();
    let has = |a: &str| args.iter().any(|x| x == a);
    let tiling = if has("--windows") {
        Some(false)
    } else if has("--omarchy") {
        Some(true)
    } else {
        None
    };
    // win95 keeps running in the background unless --here asks for it to
    // live and die with this terminal
    if has("--server") {
        return session::serve();
    }
    if has("stop") {
        return session::stop();
    }
    // `win95 amp` and the like run that one app on its own, filling the terminal
    let solo = args.get(1).filter(|a| apps::dialogs::SOLO.contains(&a.to_lowercase().as_str())).map(|_| apps::dialogs::parse_run(&args[1..].join(" ")));
    if solo.is_none() && !has("--here") {
        return session::attach(tiling);
    }
    let mut terminal = ratatui::init();
    execute!(stdout(), EnableMouseCapture, EnableBracketedPaste)?;
    let kb = crossterm::terminal::supports_keyboard_enhancement().unwrap_or(false);
    if kb {
        execute!(stdout(), PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES))?;
    }
    let size = terminal.size()?;
    let mut desk = wm::Desktop::new(size.width, size.height);
    match solo {
        Some(l) => {
            desk.set_tiling(false, false);
            desk.solo(l);
        }
        None => desk.set_tiling(tiling.unwrap_or_else(theme::saved_tiling), false),
    }
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
