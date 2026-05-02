use std::io::{self, Stdout};

use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};

use crate::errors::FrameworkError;

pub type Term = Terminal<CrosstermBackend<Stdout>>;

pub fn enter() -> Result<Term, FrameworkError> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture).map_err(FrameworkError::from)?;
    let backend = CrosstermBackend::new(stdout);
    Terminal::new(backend).map_err(FrameworkError::from)
}

pub fn restore(terminal: &mut Term) -> Result<(), FrameworkError> {
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )
    .map_err(FrameworkError::from)?;
    terminal.show_cursor().map_err(FrameworkError::from)?;
    Ok(())
}
