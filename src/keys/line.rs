//! The `:` and `/` lines. Both are plain text editors that hand their contents
//! to `excmd` or to the search on Enter, and restore the previous mode on Esc.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::text::{chars, word_start};
use crate::app::{App, Mode};
use crate::excmd;

/// The `:` and `/` lines. Always insert, like vim's command line, but the
/// cursor can be moved and the usual readline keys work.
pub(super) fn line_mode(app: &mut App, key: KeyEvent) {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Esc => leave_line(app),
        KeyCode::Char('c') if ctrl => leave_line(app),
        KeyCode::Backspace => {
            if app.line.is_empty() {
                // Backspacing off the start of the line leaves the mode, as in vim.
                leave_line(app);
            } else if app.line_cur > 0 {
                let mut text = chars(app);
                text.remove(app.line_cur - 1);
                app.line_cur -= 1;
                set_line(app, &text);
            }
        }
        KeyCode::Delete => {
            let mut text = chars(app);
            if app.line_cur < text.len() {
                text.remove(app.line_cur);
                set_line(app, &text);
            }
        }
        KeyCode::Left => app.line_cur = app.line_cur.saturating_sub(1),
        KeyCode::Right => app.line_cur = (app.line_cur + 1).min(chars(app).len()),
        KeyCode::Home => app.line_cur = 0,
        KeyCode::End => app.line_cur = chars(app).len(),
        KeyCode::Char('u') if ctrl => {
            app.line.clear();
            app.line_cur = 0;
        }
        KeyCode::Char('w') if ctrl => {
            let mut text = chars(app);
            let start = word_start(&text, app.line_cur);
            text.drain(start..app.line_cur);
            app.line_cur = start;
            set_line(app, &text);
        }
        KeyCode::Enter => {
            let line = std::mem::take(&mut app.line);
            let was_search = app.mode == Mode::Search;
            let backward = app.line_prefix == '?';
            app.line_cur = 0;
            // The edit buffer survives a `:` command, so `:w` can see the
            // pending renames.
            app.mode = if app.edit.is_some() {
                Mode::Edit
            } else {
                Mode::Normal
            };
            if was_search {
                app.last_search = line.clone();
                app.search_back = backward;
                if !app.search(&line, backward, app.cur) {
                    app.error(format!("pattern not found: {line}"));
                }
            } else {
                excmd::run(app, &line);
            }
        }
        KeyCode::Char(c) => {
            let mut text = chars(app);
            text.insert(app.line_cur.min(text.len()), c);
            app.line_cur += 1;
            set_line(app, &text);
        }
        _ => {}
    }
}

pub(super) fn set_line(app: &mut App, chars: &[char]) {
    app.line = chars.iter().collect();
    app.line_cur = app.line_cur.min(app.line.chars().count());
}

pub(super) fn leave_line(app: &mut App) {
    app.line.clear();
    app.line_cur = 0;
    app.mode = if app.edit.is_some() {
        Mode::Edit
    } else {
        Mode::Normal
    };
}
