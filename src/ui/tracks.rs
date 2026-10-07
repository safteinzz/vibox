//! The track pane: the column layout, the sticky header and the rows, including
//! a name being edited in place. `columns()` and the header string must stay in
//! agreement or the header drifts off its data.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use unicode_width::UnicodeWidthStr;

use crate::app::{App, Mode, Pane, Renaming};
use crate::library::fmt_duration;

use super::widgets::{cursor_style, dim, editing_style, truncate};

/// Cells before the first column: the sign, a space, the line number, a space.
///
/// The number grows with the library, so a four digit row on a library over a
/// thousand tracks widens the gutter instead of shoving the name sideways.
pub(super) fn gutter(app: &App) -> usize {
    let digits = app.view.len().to_string().len().max(3);
    3 + digits
}

/// Name and width of each visible column, so the header and the rows agree.
///
/// Every visible column is followed by a space, and the duration closes the row.
pub(super) fn columns(app: &App, width: usize, dur_w: usize) -> Vec<(&'static str, usize)> {
    let shown = app.columns.shown();
    let rest = width.saturating_sub(gutter(app) + shown.len() + dur_w);
    let total: usize = shown.iter().map(|(_, weight)| weight).sum();
    if total == 0 {
        return Vec::new();
    }

    let mut out: Vec<(&'static str, usize)> = Vec::with_capacity(shown.len());
    let mut used = 0;
    for (i, (name, weight)) in shown.iter().enumerate() {
        // The last column takes the rounding leftovers.
        let w = if i + 1 == shown.len() {
            rest.saturating_sub(used)
        } else {
            rest * weight / total
        };
        used += w;
        out.push((name, w));
    }
    out
}

/// Hour long rips need `1:13:24`, everything else fits in `4:03`.
pub(super) fn duration_width(app: &App) -> usize {
    if app
        .view
        .iter()
        .any(|&i| app.tracks[i].duration.as_secs() >= 3600)
    {
        7
    } else {
        5
    }
}

pub(super) fn draw_tracks(frame: &mut Frame, app: &mut App, area: Rect) {
    // Tab bar on top when more than one view is open, then the sticky header.
    // Always shown, even with one tab: naming the view beats guessing it.
    let labels = app.tab_labels();
    let area = {
        let [bar, rest] = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(area);
        let mut spans = Vec::new();
        for (i, (name, dirty)) in labels.iter().enumerate() {
            let style = if i == app.tab_idx {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                dim()
            };
            spans.push(Span::styled(
                format!(" {name}{} ", if *dirty { "*" } else { "" }),
                style,
            ));
            spans.push(Span::styled("│", dim()));
        }
        frame.render_widget(Paragraph::new(Line::from(spans)), bar);
        rest
    };

    // The header sticks: the rows scroll under it.
    let [head, area] = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(area);
    app.track_h = area.height as usize;
    app.scroll_to_cursor();

    let dur_w = duration_width(app);
    let cols = columns(app, area.width as usize, dur_w);
    let pad = gutter(app);
    let num_w = pad - 3;
    let mut header = " ".repeat(pad);
    for (name, w) in &cols {
        header.push_str(&truncate(name, *w));
        header.push(' ');
    }
    header.push_str(&format!("{:>dur_w$}", "time"));

    frame.render_widget(
        Paragraph::new(Line::styled(
            header,
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )),
        head,
    );

    if app.view.is_empty() {
        // An empty playlist is waiting to be filled; an empty library is a
        // different problem, so they get different instructions.
        let msg = if let Some(name) = app.playlist_view.clone() {
            vec![
                Line::styled(format!("  `{name}` is empty"), dim()),
                Line::from(""),
                Line::styled("  gt then t     open a folder in its own tab", dim()),
                Line::styled("  V then y      pick the tracks you want", dim()),
                Line::styled("  gt then p     bring them back here", dim()),
                Line::styled("  :w            save it", dim()),
            ]
        } else {
            let root = app.root.display().to_string();
            vec![
                Line::styled(format!("  no tracks under {root}"), dim()),
                Line::from(""),
                Line::styled("  :set root=~/Music   make that your library", dim()),
                Line::styled("  :e ~/Music          open one just for now", dim()),
                Line::styled("  :help               keys", dim()),
            ]
        };
        frame.render_widget(Paragraph::new(msg), area);
        return;
    }

    // Slide the file column so the cursor stays on screen while editing. The
    // buffer keeps its own window, so this is one call.
    if app.renaming_a_track()
        && let Some(file_w) = cols.iter().find(|(n, _)| *n == "file").map(|(_, w)| *w)
        && let Some(buf) = app.name_buffer()
    {
        buf.follow(file_w);
    }

    let focused = app.focus == Pane::Tracks;

    let w = area.width as usize;
    let paused = app
        .audio
        .as_ref()
        .is_some_and(crate::player::Audio::is_paused);
    let mut lines: Vec<Line> = Vec::new();
    let mut cursor_at = None;

    let visible = app.top..(app.top + area.height as usize).min(app.view.len());
    for (rows, row) in (0_u16..).zip(visible) {
        let track_idx = app.view[row];
        let track = &app.tracks[track_idx];
        let is_cursor = row == app.cur;
        let is_playing = app.playing == Some(track_idx);

        let sign = if is_playing {
            if paused { '|' } else { '>' }
        } else {
            ' '
        };
        // Relative numbers, so 8j lands where you counted it would.
        let number = if is_cursor {
            format!("{:<num_w$}", row + 1)
        } else {
            format!("{:>num_w$}", app.cur.abs_diff(row))
        };

        // Only when the tracks are what `c` opened: a sidebar rename must not
        // put a cursor in this pane as well.
        let editing_row = is_cursor
            && matches!(app.mode, Mode::Edit | Mode::EditInsert)
            && app.renaming_a_track();

        let doomed_file = app.is_doomed_file(&track.path) || app.is_cut(&track.path);
        let (sel_a, sel_b) = app.selection_range();
        let base = if app.mode == Mode::Visual && row >= sel_a && row <= sel_b {
            Style::default().bg(Color::Indexed(238))
        } else if editing_row {
            editing_style()
        } else if is_cursor {
            cursor_style(focused)
        } else if doomed_file {
            Style::default()
                .fg(Color::Red)
                .add_modifier(Modifier::CROSSED_OUT)
        } else if is_playing {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default()
        };

        // The name a rename would give it, typed now or waiting for `:w`.
        let edited: Option<String> = app.shown_name(&Renaming::Track(track_idx));
        let changed = edited.as_ref().is_some_and(|name| *name != track.file);
        let sign = if changed { '~' } else { sign };

        // The row being edited shows the name from its own scroll offset, so a
        // long one slides under the cursor instead of ending in an ellipsis.
        let scrolled: Option<String> = editing_row.then(|| {
            let scroll = app.name_scroll();
            edited
                .clone()
                .unwrap_or_else(|| track.file.clone())
                .chars()
                .skip(scroll)
                .collect()
        });

        let mut body = format!("{sign} {number} ");
        for (name, width) in &cols {
            let value = match *name {
                "file" => scrolled.as_ref().or(edited.as_ref()).unwrap_or(&track.file),
                "title" => &track.title,
                "artist" => &track.artist,
                _ => &track.album,
            };
            body.push_str(&truncate(value, *width));
            body.push(' ');
        }
        body.push_str(&format!("{:>dur_w$}", fmt_duration(track.duration)));

        if editing_row {
            // The cursor sits in the name itself, just past the gutter.
            let scroll = app.name_scroll();
            let col = app.name_col().saturating_sub(scroll);
            if app.mode == Mode::EditInsert {
                // Insert uses the terminal's own cursor, so the shape can be a
                // thin bar. Painting a cell here would show a block instead.
                let text: String = app
                    .edit_text(row)
                    .unwrap_or_default()
                    .chars()
                    .skip(scroll)
                    .take(col)
                    .collect();
                let x = area.x
                    + u16::try_from(pad).unwrap_or(6)
                    + u16::try_from(text.width()).unwrap_or(0);
                cursor_at = Some((x.min(area.right().saturating_sub(1)), area.y + rows));
                lines.push(Line::styled(truncate(&body, w), base));
            } else {
                let picked = name_selection_at(app, pad, scroll);
                lines.push(row_with_cursor(
                    &truncate(&body, w),
                    pad + col,
                    base,
                    app.mode,
                    picked,
                ));
            }
        } else {
            lines.push(Line::styled(truncate(&body, w), base));
        }
    }

    if cursor_at.is_some() {
        app.cursor_screen = cursor_at;
    }
    frame.render_widget(Paragraph::new(lines), area);
}

/// Splits a rendered row so one cell carries the cursor.
/// The name's selection moved into the coordinates of the row it is drawn in:
/// shifted right by whatever sits in front of the name, and left by however
/// far the name has scrolled inside its column.
pub(super) fn name_selection_at(app: &App, offset: usize, scroll: usize) -> Option<(usize, usize)> {
    let (lo, hi) = app.name_selection()?;
    if hi < scroll {
        return None;
    }
    Some((
        offset + lo.saturating_sub(scroll),
        offset + hi.saturating_sub(scroll),
    ))
}

pub(super) fn row_with_cursor(
    row: &str,
    at: usize,
    base: Style,
    mode: Mode,
    selection: Option<(usize, usize)>,
) -> Line<'static> {
    let text: Vec<char> = row.chars().collect();
    let at = at.min(text.len().saturating_sub(1));

    // Solid colour, not a modifier: reversing an already reversed row is
    // invisible, which is exactly how the cursor got lost before.
    let cursor = if mode == Mode::EditInsert {
        Style::default().bg(Color::Cyan).fg(Color::Black)
    } else {
        Style::default()
            .bg(Color::Yellow)
            .fg(Color::Black)
            .add_modifier(Modifier::BOLD)
    };
    let picked = Style::default().bg(Color::Magenta).fg(Color::Black);

    // One span per character would be correct and wasteful, so runs of the
    // same style are glued back together as we go.
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut run = String::new();
    let mut run_style = None;

    for (i, c) in text.iter().enumerate() {
        let style = if i == at {
            cursor
        } else if selection.is_some_and(|(lo, hi)| i >= lo && i <= hi) {
            picked
        } else {
            base
        };
        if run_style != Some(style) && !run.is_empty() {
            spans.push(Span::styled(
                std::mem::take(&mut run),
                run_style.unwrap_or(base),
            ));
        }
        run_style = Some(style);
        run.push(*c);
    }
    if !run.is_empty() {
        spans.push(Span::styled(run, run_style.unwrap_or(base)));
    }

    Line::from(spans)
}
