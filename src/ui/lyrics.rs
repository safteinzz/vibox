//! The lyrics pane and its popup: wrapping, the sync hint, and the window that
//! `:lyrics` opens over the track list.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::app::{App, Pane};

use super::widgets::dim;

/// Wraps one lyric at the pane width, indenting the runover so a long line
/// still reads as one line and not as two lyrics.
pub(super) fn wrap_lyric(text: &str, width: usize) -> Vec<String> {
    let width = width.max(8);
    if text.chars().count() < width {
        return vec![format!(" {text}")];
    }

    let mut out: Vec<String> = Vec::new();
    let mut line = String::from(" ");
    for word in text.split_whitespace() {
        let indent = if out.is_empty() { 1 } else { 3 };
        if line.chars().count() > indent && line.chars().count() + 1 + word.chars().count() > width
        {
            out.push(std::mem::take(&mut line));
            line = "   ".to_string();
        }
        if line.chars().count() > indent {
            line.push(' ');
        }
        line.push_str(word);
    }
    if line.trim().is_empty() {
        if out.is_empty() {
            out.push(String::new());
        }
    } else {
        out.push(line);
    }
    out
}

/// Every lyric row, wrapped to the pane, and the row the line being sung
/// starts on when there is one.
///
/// Nothing is skipped here: which rows are on screen is the caller's job,
/// because the same words are followed by the song in one mode and scrolled by
/// hand in another, and only one of those knows where the window should sit.
pub(super) fn lyric_rows(app: &App, width: usize) -> (Vec<Line<'static>>, Option<usize>) {
    let Some(track) = app.playing_track() else {
        return (vec![Line::styled("  nothing playing", dim())], None);
    };

    let Some(found) = app.lyrics.get(&track.path) else {
        let waiting = if app.lyrics.is_loading(&track.path) {
            "  looking on lrclib..."
        } else {
            "  ..."
        };
        return (vec![Line::styled(waiting, dim())], None);
    };

    match found {
        crate::lyrics::Lyrics::Missing(why) => {
            (vec![Line::styled(format!("  {why}"), dim())], None)
        }
        crate::lyrics::Lyrics::Plain(lines) => (
            lines
                .iter()
                .flat_map(|l| wrap_lyric(l, width))
                .map(Line::raw)
                .collect(),
            None,
        ),
        // `:set nokaraoke`: read them as words on a page. Timings that
        // do not fit the recording are worse than no timings, and chasing the
        // wrong line down the pane is the part that grates.
        crate::lyrics::Lyrics::Synced(lines) if !app.karaoke => (
            lines
                .iter()
                .flat_map(|(_, words)| wrap_lyric(words, width))
                .map(Line::raw)
                .collect(),
            None,
        ),
        crate::lyrics::Lyrics::Synced(lines) => {
            // The per file correction shifts every timestamp, for rips whose
            // lead-in differs from whoever uploaded the lyrics.
            let now = app.elapsed().as_millis() as i64;
            let offset = app.lyrics.offset(&track.path);
            // The line being sung is the last one whose timestamp has passed.
            let current = lines
                .iter()
                .rposition(|(at, _)| at.as_millis() as i64 + offset <= now);

            let mut rows: Vec<Line<'static>> = Vec::new();
            let mut current_row = None;
            for (i, (_, words)) in lines.iter().enumerate() {
                let style = if Some(i) == current {
                    current_row = Some(rows.len());
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD)
                } else {
                    dim()
                };
                rows.extend(
                    wrap_lyric(words, width)
                        .into_iter()
                        .map(|part| Line::styled(part, style)),
                );
            }
            (rows, current_row)
        }
    }
}

/// Where the window sits when the pane is following rather than pinned.
///
/// Only a highlighted line moves it, because only a timestamp knows where we
/// are. Words with no timings stay where they are put and are scrolled by hand
/// (`ctrl-w l`, then `j` and `k`): guessing the position from the clock reads
/// well for one song and drifts by the length of the intro on the next, and a
/// page that moves on its own for a reason the user cannot see is worse than
/// one that sits still.
fn follow_top(rows: usize, current: Option<usize>, height: usize) -> usize {
    let max_top = rows.saturating_sub(height);
    match current {
        Some(row) => row.saturating_sub(height / 2).min(max_top),
        None => 0,
    }
}

/// The rows to draw and how far down them to start, and the place the pane's
/// measurements are written back for the scrolling keys to use.
fn lyrics_window(app: &mut App, width: usize, height: usize) -> (Vec<Line<'static>>, u16) {
    let (rows, current) = lyric_rows(app, width);
    app.lyrics_rows = rows.len();
    app.lyrics_h = height.max(1);
    // A pin lives only as long as the keyboard is in the pane: leaving it is
    // how you say you have stopped reading, so the words follow the song again
    // without a key of their own to remember.
    if app.focus != Pane::Lyrics {
        app.unpin_lyrics();
    }
    if !app.lyrics_pinned {
        app.lyrics_top = follow_top(rows.len(), current, height.max(1));
    }
    // A pin taken on a longer set of words, or before a resize, can be past
    // the end of these ones.
    app.lyrics_top = app.lyrics_top.min(rows.len().saturating_sub(height.max(1)));
    (rows, app.lyrics_top as u16)
}

pub(super) fn draw_lyrics(frame: &mut Frame, app: &mut App, area: Rect) {
    let block = Block::default()
        .borders(Borders::LEFT)
        .border_style(if app.focus == Pane::Lyrics {
            Style::default()
        } else {
            dim()
        })
        .title(lyrics_title(app, area.width as usize));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let (rows, top) = lyrics_window(app, inner.width as usize, inner.height as usize);
    frame.render_widget(Paragraph::new(rows).scroll((top, 0)), inner);
}

/// `lyrics (lrclib)`, crediting whoever wrote the words, and how to shift them
/// when the timings sit a second out. The hint is dropped rather than clipped
/// on a pane too narrow to hold it.
pub(super) fn lyrics_title(app: &App, width: usize) -> String {
    let base = format!(" lyrics ({}) ", crate::lyrics::SOURCE);
    let Some(hint) = sync_hint(app) else {
        return base;
    };
    let full = format!("{base}{hint} ");
    if full.chars().count() <= width {
        full
    } else {
        base
    }
}

/// `use [ ] to sync`, becoming the correction itself once there is one. Only
/// for lyrics that are actually following: nudging a page of plain words does
/// nothing, and neither does nudging with the following turned off.
pub(super) fn sync_hint(app: &App) -> Option<String> {
    if !app.karaoke {
        return None;
    }
    let track = app.playing_track()?;
    match app.lyrics.get(&track.path)? {
        crate::lyrics::Lyrics::Synced(_) => {
            let offset = app.lyrics.offset(&track.path);
            Some(if offset == 0 {
                "use [ ] to sync".to_string()
            } else {
                format!("[ ] sync {:+.1}s", offset as f64 / 1000.0)
            })
        }
        _ => None,
    }
}

/// Same content, for a terminal too narrow to give lyrics their own pane.
pub(super) fn draw_lyrics_popup(frame: &mut Frame, app: &mut App, area: Rect) {
    let w = 50.min(area.width.saturating_sub(2));
    let h = (area.height * 3 / 4).max(3);
    let popup = Rect {
        x: (area.width.saturating_sub(w)) / 2,
        y: (area.height.saturating_sub(h)) / 2,
        width: w,
        height: h,
    };

    let block = Block::bordered().title(format!(
        " lyrics ({}): :set nolyrics to close ",
        crate::lyrics::SOURCE
    ));
    let inner = block.inner(popup);
    frame.render_widget(Clear, popup);
    frame.render_widget(block, popup);
    let (rows, top) = lyrics_window(app, inner.width as usize, inner.height as usize);
    frame.render_widget(Paragraph::new(rows).scroll((top, 0)), inner);
}
