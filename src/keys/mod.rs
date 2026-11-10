//! Modal key handling. Normal mode owns motions and playback, the `:` and `/`
//! lines are just text editors that hand their contents off on Enter.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::app::{App, Mode};

pub mod edit;
pub mod line;
pub mod normal;
pub mod text;
pub mod windows;

use edit::edit_mode;
use line::line_mode;
use normal::normal_mode;
use windows::window_key;

pub fn handle(app: &mut App, key: KeyEvent) {
    // Key release and repeat events arrive on terminals that speak the kitty
    // protocol; acting on them would double every motion.
    if key.kind != KeyEventKind::Press {
        return;
    }

    // ctrl-c cancels the `:` and `/` lines; anywhere else it only says how to
    // quit.
    let typing_a_line = matches!(app.mode, Mode::Command | Mode::Search);
    if key.code == KeyCode::Char('c')
        && key.modifiers.contains(KeyModifiers::CONTROL)
        && !typing_a_line
    {
        app.info("type  :q  to quit vibox");
        return;
    }

    // Windows get every key before the mode does: `:ch` opened from inside a
    // rename used to leave `h` and `l` editing the name behind it.
    if app.alert.is_some() || app.show_info || app.show_changes || app.show_help || app.show_history
    {
        window_key(app, key);
        return;
    }

    match app.mode {
        Mode::Command | Mode::Search => line_mode(app, key),
        Mode::Edit | Mode::EditInsert => edit_mode(app, key),
        Mode::Normal | Mode::Visual => normal_mode(app, key),
    }
}
