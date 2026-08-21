//! Keys while a window is up. An open window owns the keyboard whatever mode
//! opened it, or a popup opened from inside a rename leaves its keys editing
//! the name behind it.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::App;

/// Keys for whichever window is open: info, changes, or help.
pub(super) fn window_key(app: &mut App, key: KeyEvent) {
    // The info window takes the keyboard while it is up.
    if app.show_info {
        match key.code {
            KeyCode::Char('y') => {
                let path = app.current_track().map(|t| t.path.display().to_string());
                if let Some(path) = path {
                    app.copy_to_clipboard(&path);
                }
                app.show_info = false;
            }
            KeyCode::Char('q' | 'Q' | 'K') | KeyCode::Esc | KeyCode::Enter => {
                app.show_info = false;
            }
            _ => {}
        }
        return;
    }

    // The changes window pans sideways: a rename of two long paths is wider
    // than any popup, and the interesting part is usually the far end.
    if app.show_changes {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let step = app.count.take().unwrap_or(1) * 4;
        match key.code {
            KeyCode::Char('l') | KeyCode::Right => app.changes_pan += step,
            KeyCode::Char('h') | KeyCode::Left => {
                app.changes_pan = app.changes_pan.saturating_sub(step);
            }
            KeyCode::Char('0') => app.changes_pan = 0,
            KeyCode::Char('$') => app.changes_pan = usize::MAX,
            // The whole list, not the row under a cursor: there is no cursor
            // here, and what you want to paste elsewhere is all of it.
            KeyCode::Char('y') => {
                let text = app.pending_changes().join("\n");
                app.copy_to_clipboard(&text);
                app.show_changes = false;
            }
            // A big batch outgrows any popup, so it scrolls like the history
            // window does.
            KeyCode::Char('j') | KeyCode::Down => app.changes_top += step / 4,
            KeyCode::Char('k') | KeyCode::Up => {
                app.changes_top = app.changes_top.saturating_sub(step / 4);
            }
            KeyCode::Char('g') | KeyCode::Home => app.changes_top = 0,
            KeyCode::Char('G') | KeyCode::End => app.changes_top = usize::MAX,
            KeyCode::Char('d') if ctrl => app.changes_top += 10,
            KeyCode::Char('u') if ctrl => app.changes_top = app.changes_top.saturating_sub(10),
            KeyCode::Char('q' | 'Q') | KeyCode::Esc | KeyCode::Enter => {
                app.show_changes = false;
                app.changes_pan = 0;
                app.changes_top = 0;
            }
            _ => {}
        }
        return;
    }

    // The history window scrolls, since a long session outgrows any popup.
    if app.show_history {
        let step = app.count.take().unwrap_or(1);
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => app.history_top += step,
            KeyCode::Char('k') | KeyCode::Up => {
                app.history_top = app.history_top.saturating_sub(step);
            }
            KeyCode::Char('g') | KeyCode::Home => app.history_top = 0,
            KeyCode::Char('G') | KeyCode::End => app.history_top = usize::MAX,
            KeyCode::Char('q' | 'Q') | KeyCode::Esc | KeyCode::Enter => {
                app.show_history = false;
            }
            _ => {}
        }
        return;
    }

    // The help window takes the keyboard while it is up, like a help buffer.
    if app.show_help {
        help_key(app, key);
    }
}

/// Keys while `:help` is up. It scrolls, and then it closes.
pub(super) fn help_key(app: &mut App, key: KeyEvent) {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let last = crate::ui::help_len().saturating_sub(1);
    let scroll = |app: &mut App, delta: isize| {
        app.help_scroll = (app.help_scroll as isize + delta).clamp(0, last as isize) as usize;
    };

    match key.code {
        KeyCode::Char('q' | 'Q') | KeyCode::Esc | KeyCode::F(1) => {
            app.show_help = false;
            app.help_scroll = 0;
        }
        KeyCode::Char('j') | KeyCode::Down => scroll(app, 1),
        KeyCode::Char('k') | KeyCode::Up => scroll(app, -1),
        KeyCode::Char('d') if ctrl => scroll(app, 10),
        KeyCode::Char('u') if ctrl => scroll(app, -10),
        KeyCode::Char('g') => app.help_scroll = 0,
        KeyCode::Char('G') => app.help_scroll = last,
        _ => {}
    }
}
