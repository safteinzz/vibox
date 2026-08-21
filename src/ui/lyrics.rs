//! The lyrics pane and its popup: wrapping, the sync hint, and the window that
//! `:lyrics` opens over the track list.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::app::App;

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

/// The lyric lines, and which one is playing right now.
pub(super) fn lyric_lines(app: &App, width: usize, height: usize) -> Vec<Line<'static>> {
    let Some(track) = app.playing_track() else {
        return vec![Line::styled("  nothing playing", dim())];
    };

    let Some(found) = app.lyrics.get(&track.path) else {
        let waiting = if app.lyrics.is_loading(&track.path) {
            "  looking on lrclib..."
        } else {
            "  ..."
        };
        return vec![Line::styled(waiting, dim())];
    };

    match found {
        crate::lyrics::Lyrics::Missing(why) => vec![Line::styled(format!("  {why}"), dim())],
        crate::lyrics::Lyrics::Plain(lines) => lines
            .iter()
            .flat_map(|l| wrap_lyric(l, width))
            .map(Line::raw)
            .collect(),
        // `:set nokaraoke`: read them as words on a page. Timings that
        // do not fit the recording are worse than no timings, and chasing the
        // wrong line down the pane is the part that grates.
        crate::lyrics::Lyrics::Synced(lines) if !app.karaoke => lines
            .iter()
            .flat_map(|(_, words)| wrap_lyric(words, width))
            .map(Line::raw)
            .collect(),
        crate::lyrics::Lyrics::Synced(lines) => {
            // The per file correction shifts every timestamp, for rips whose
            // lead-in differs from whoever uploaded the lyrics.
            let now = app.elapsed().as_millis() as i64;
            let offset = app.lyrics.offset(&track.path);
            // The line being sung is the last one whose timestamp has passed.
            let current = lines
                .iter()
                .rposition(|(at, _)| at.as_millis() as i64 + offset <= now)
                .unwrap_or(usize::MAX);

            // Keep it centred rather than scrolling it off the top.
            let top = current
                .saturating_sub(height / 2)
                .min(lines.len().saturating_sub(height));

            lines
                .iter()
                .enumerate()
                .skip(if current == usize::MAX { 0 } else { top })
                .flat_map(|(i, (_, words))| {
                    let style = if i == current {
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        dim()
                    };
                    wrap_lyric(words, width)
                        .into_iter()
                        .map(move |part| Line::styled(part, style))
                })
                .collect()
        }
    }
}

pub(super) fn draw_lyrics(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::LEFT)
        .border_style(dim())
        .title(lyrics_title(app, area.width as usize));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    frame.render_widget(
        Paragraph::new(lyric_lines(
            app,
            inner.width as usize,
            inner.height as usize,
        )),
        inner,
    );
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
pub(super) fn draw_lyrics_popup(frame: &mut Frame, app: &App, area: Rect) {
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
    frame.render_widget(
        Paragraph::new(lyric_lines(
            app,
            inner.width as usize,
            inner.height as usize,
        )),
        inner,
    );
}
