//! Modal key handling. Normal mode owns motions and playback, the `:` and `/`
//! lines are just text editors that hand their contents off on Enter.

use ratatui::crossterm::event::{KeyEvent, KeyEventKind};

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

    // A window that is up owns the keyboard, whatever mode opened it. `:ch`
    // from inside a rename used to draw the popup while h and l still edited
    // the name behind it.
    if app.show_info || app.show_changes || app.show_help || app.show_history {
        window_key(app, key);
        return;
    }

    match app.mode {
        Mode::Command | Mode::Search => line_mode(app, key),
        Mode::Edit | Mode::EditInsert => edit_mode(app, key),
        Mode::Normal | Mode::Visual => normal_mode(app, key),
    }
}
