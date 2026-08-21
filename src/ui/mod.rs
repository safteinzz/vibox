//! Rendering. Two panes, a progress line, a statusline and the command line,
//! in that order, exactly like a neovim window with a lualine under it.
//!
//! No images, no glyphs outside plain box drawing: whatever font the terminal
//! is already using is the font vibox uses.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};

use crate::app::{App, Mode};

pub mod folders;
pub mod lyrics;
pub mod name_diff;
pub mod popups;
pub mod status;
pub mod tracks;
pub mod widgets;

pub use popups::help_len;

use folders::FOLDER_W;
use folders::draw_folders;
use lyrics::{draw_lyrics, draw_lyrics_popup};
use popups::{draw_changes, draw_help, draw_history, draw_info};
use status::{draw_cmdline, draw_progress, draw_status};
use tracks::draw_tracks;

pub fn draw(frame: &mut Frame, app: &mut App) {
    let [main, progress, status, cmdline] = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    // Claimed by whichever pane is inserting, so the terminal draws its own
    // thin bar there instead of a painted block.
    app.cursor_screen = None;

    let folder_w = FOLDER_W.min(main.width / 2);
    // Lyrics only take a pane when the track list can still breathe; on a
    // narrow terminal they go in a popup instead.
    let lyrics_w = if app.show_lyrics {
        let want = (main.width / 3).clamp(24, 48);
        if main.width.saturating_sub(folder_w + want) >= 34 {
            want
        } else {
            0
        }
    } else {
        0
    };

    let [folders, tracks, lyrics] = Layout::horizontal([
        Constraint::Length(folder_w),
        Constraint::Min(10),
        Constraint::Length(lyrics_w),
    ])
    .areas(main);

    draw_folders(frame, app, folders);
    draw_tracks(frame, app, tracks);
    if lyrics_w > 0 {
        draw_lyrics(frame, app, lyrics);
    }
    draw_progress(frame, app, progress);
    draw_status(frame, app, status);
    draw_cmdline(frame, app, cmdline);

    if app.mode == Mode::EditInsert
        && let Some(pos) = app.cursor_screen
    {
        frame.set_cursor_position(pos);
    }

    if app.matrix.on {
        let level = app
            .audio
            .as_ref()
            .filter(|a| !a.is_paused())
            .map_or(0.0, crate::player::Audio::level);
        crate::matrix::overlay(frame, &mut app.matrix, level);
    }
    if app.show_lyrics && lyrics_w == 0 {
        draw_lyrics_popup(frame, app, frame.area());
    }
    if app.show_info {
        draw_info(frame, app, frame.area());
    }
    if app.show_changes {
        let area = frame.area();
        draw_changes(frame, app, area);
    }
    if app.show_history {
        let area = frame.area();
        draw_history(frame, app, area);
    }
    if app.show_help {
        draw_help(frame, app, frame.area());
    }
}
