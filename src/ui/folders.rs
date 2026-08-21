//! The sidebar: the folder tree and the playlist list, one tab each.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use unicode_width::UnicodeWidthStr;

use crate::app::{App, Mode, Pane, Renaming, Tab};

use super::tracks::{name_selection_at, row_with_cursor};
use super::widgets::{cursor_style, dim, editing_style, truncate};

pub(super) const FOLDER_W: u16 = 30;
pub(super) fn draw_folders(frame: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.focus == Pane::Folders;
    let block = Block::default()
        .borders(Borders::RIGHT)
        .border_style(if focused { Style::default() } else { dim() });
    let whole = block.inner(area);
    frame.render_widget(block, area);

    // Which library, then the tab header. The root goes above rather than into
    // the list, because "where am I" is the one thing that must not scroll
    // away, and it is the slot a remote source would name itself in.
    let [source, head, inner] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(0),
    ])
    .areas(whole);

    let width = source.width as usize;
    frame.render_widget(
        Paragraph::new(Line::styled(
            format!(
                " {}",
                crate::boot::short(&app.root, width.saturating_sub(2))
            ),
            Style::default().fg(Color::Green),
        )),
        source,
    );
    let lit = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let (folders_style, playlists_style) = match app.tab {
        Tab::Folders => (lit, dim()),
        Tab::Playlists => (dim(), lit),
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" folders", folders_style),
            Span::styled(" | ", dim()),
            Span::styled("playlists", playlists_style),
        ])),
        head,
    );

    if app.tab == Tab::Playlists {
        draw_playlists(frame, app, inner, focused);
        return;
    }

    app.folder_h = inner.height as usize;
    if app.folder_cur < app.folder_top {
        app.folder_top = app.folder_cur;
    }
    if app.folder_cur >= app.folder_top + app.folder_h.max(1) {
        app.folder_top = app.folder_cur + 1 - app.folder_h.max(1);
    }

    let w = inner.width as usize;
    let mut lines: Vec<Line> = Vec::new();
    let mut cursor_at = None;
    let rows = app.folder_top..(app.folder_top + inner.height as usize);
    for (shown, row) in (0_u16..).zip(rows) {
        if row > app.folders.len() {
            break;
        }
        let label = if row == 0 {
            format!("* everything ({})", app.library_len())
        } else {
            app.folders[row - 1].0.clone()
        };
        // The name a rename would give it, whether typed now or waiting for `:w`.
        let renamed = app
            .folders
            .get(row.wrapping_sub(1))
            .and_then(|(_, path)| app.shown_name(&Renaming::Folder(path.clone())));
        let editing = row == app.folder_cur
            && matches!(app.mode, Mode::Edit | Mode::EditInsert)
            && matches!(app.renaming(), Some(Renaming::Folder(_)));
        let label = renamed.unwrap_or(label);
        let doomed = row > 0
            && app
                .folders
                .get(row - 1)
                .is_some_and(|(_, p)| app.is_doomed_dir(p));
        let mut style = if editing {
            editing_style()
        } else if row == app.folder_cur {
            cursor_style(focused)
        } else {
            Style::default()
        };
        if doomed {
            style = style.fg(Color::Red).add_modifier(Modifier::CROSSED_OUT);
        }
        if editing {
            let col = app.name_col();
            if app.mode == Mode::EditInsert {
                let before: String = label.chars().take(col).collect();
                let x = inner.x + u16::try_from(before.width()).unwrap_or(0);
                cursor_at = Some((x.min(inner.right().saturating_sub(1)), inner.y + shown));
                lines.push(Line::styled(truncate(&label, w), style));
            } else {
                let picked = name_selection_at(app, 0, 0);
                lines.push(row_with_cursor(
                    &truncate(&label, w),
                    col,
                    style,
                    app.mode,
                    picked,
                ));
            }
        } else {
            lines.push(Line::styled(truncate(&label, w), style));
        }
    }

    if cursor_at.is_some() {
        app.cursor_screen = cursor_at;
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

/// The saved playlists, listed by name.
pub(super) fn draw_playlists(frame: &mut Frame, app: &mut App, area: Rect, focused: bool) {
    app.folder_h = area.height as usize;
    let w = area.width as usize;

    if app.playlists.is_empty() {
        frame.render_widget(
            Paragraph::new(vec![
                Line::styled(" no playlists yet", dim()),
                Line::from(""),
                Line::styled(" :w late night", dim()),
                Line::styled(" saves this view", dim()),
            ]),
            area,
        );
        return;
    }

    let lines: Vec<Line> = (app.pl_top..(app.pl_top + area.height as usize))
        .filter_map(|row| app.playlists.get(row).map(|(name, _)| (row, name)))
        .map(|(row, name)| {
            let doomed = app
                .playlists
                .get(row)
                .is_some_and(|(_, path)| app.is_doomed(path));
            let renamed = app.shown_name(&Renaming::Playlist(name.clone()));
            let editing = row == app.pl_cur
                && matches!(app.mode, Mode::Edit | Mode::EditInsert)
                && matches!(app.renaming(), Some(Renaming::Playlist(_)));
            let mut style = if editing {
                editing_style()
            } else if row == app.pl_cur {
                cursor_style(focused)
            } else {
                Style::default()
            };
            if doomed {
                style = style.fg(Color::Red).add_modifier(Modifier::CROSSED_OUT);
            }

            let shown = renamed.unwrap_or_else(|| name.clone());
            if editing {
                let col = app.name_col();
                row_with_cursor(
                    &truncate(&shown, w),
                    col,
                    style,
                    app.mode,
                    name_selection_at(app, 0, 0),
                )
            } else {
                Line::styled(truncate(&shown, w), style)
            }
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), area);
}
