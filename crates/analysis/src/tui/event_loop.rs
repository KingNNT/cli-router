use std::time::Duration;

use crossterm::event::{self, Event, KeyEventKind};

use crate::errors::FrameworkError;
use crate::tui::app_state::AppState;
use crate::tui::controllers::TuiController;
use crate::tui::renderer;
use crate::tui::terminal::Term;

pub fn run(
    terminal: &mut Term,
    controller: &TuiController,
    state: &mut AppState,
) -> Result<(), FrameworkError> {
    controller
        .warmup(state)
        .map_err(|e| FrameworkError::Terminal(e.to_string()))?;

    loop {
        terminal.draw(|f| renderer::draw(f, state))?;

        if event::poll(Duration::from_millis(100))? {
            match event::read()? {
                Event::Key(key) => {
                    if key.kind != KeyEventKind::Press {
                        continue;
                    }
                    controller
                        .handle(key, state)
                        .map_err(|e| FrameworkError::Terminal(e.to_string()))?;
                }
                Event::Mouse(mouse) => {
                    controller
                        .handle_mouse(mouse, state)
                        .map_err(|e| FrameworkError::Terminal(e.to_string()))?;
                }
                _ => {}
            }
            if state.should_quit {
                return Ok(());
            }
        }
    }
}
