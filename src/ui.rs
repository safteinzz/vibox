//! Rendering. Two panes, a progress line, a statusline and the command line,
//! in that order, exactly like a neovim window with a lualine under it.
//!
//! No images, no glyphs outside plain box drawing: whatever font the terminal
//! is already using is the font vibox uses.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use unicode_width::UnicodeWidthStr;

use crate::app::{App, Mode, Pane, Renaming, Tab};
use crate::library::fmt_duration;

const FOLDER_W: u16 = 30;

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
    if app.show_help {
        draw_help(frame, app, frame.area());
    }
}

/// `K`: everything vibox knows about the track under the cursor.
fn draw_info(frame: &mut Frame, app: &App, area: Rect) {
    let Some(track) = app.current_track() else {
        return;
    };

    let year = track.year.map(|y| y.to_string()).unwrap_or_default();
    let number = match (track.disc_no, track.track_no) {
        (Some(disc), Some(no)) => format!("{disc}.{no}"),
        (_, Some(no)) => no.to_string(),
        _ => String::new(),
    };
    let fields = [
        ("path", track.path.display().to_string()),
        ("file", track.file.clone()),
        ("title", track.title.clone()),
        ("artist", track.artist.clone()),
        ("album", track.album.clone()),
        ("album artist", track.album_artist.clone()),
        ("track", number),
        ("year", year),
        ("genre", track.genre.clone()),
        ("length", fmt_duration(track.duration)),
    ];

    let body: Vec<Line> = fields
        .iter()
        .filter(|(_, value)| !value.is_empty())
        .map(|(label, value)| {
            Line::from(vec![
                Span::styled(format!(" {label:<13}"), dim()),
                Span::raw(value.clone()),
            ])
        })
        .collect();

    let widest = body.iter().map(Line::width).max().unwrap_or(20) + 2;
    let w = (widest as u16).min(area.width.saturating_sub(4));
    let h = (body.len() as u16 + 2).min(area.height.saturating_sub(2));
    let popup = Rect {
        x: (area.width.saturating_sub(w)) / 2,
        y: (area.height.saturating_sub(h)) / 2,
        width: w,
        height: h,
    };

    let block = Block::bordered().title(" y copies the path, q closes ");
    let inner = block.inner(popup);
    frame.render_widget(Clear, popup);
    frame.render_widget(block, popup);
    frame.render_widget(Paragraph::new(body), inner);
}

/// `:changes`: exactly what a `:w` would write, before you press it.
///
/// Two long paths and an arrow are wider than any popup, so the window pans
/// sideways with `h` and `l`, and says so along the bottom when there is more
/// to see.
fn draw_changes(frame: &mut Frame, app: &mut App, area: Rect) {
    let changes = app.pending_changes();
    let widest = changes.iter().map(|line| line.width()).max().unwrap_or(0);

    let w = 88.min(area.width.saturating_sub(4));
    let h = (changes.len().max(1) as u16 + 2).min(area.height.saturating_sub(2));
    let popup = Rect {
        x: (area.width.saturating_sub(w)) / 2,
        y: (area.height.saturating_sub(h)) / 2,
        width: w,
        height: h,
    };

    let block = Block::bordered().title(" :w would do this, q closes ");
    let inner = block.inner(popup);
    let view_w = inner.width as usize;
    app.changes_pan = app.changes_pan.min(widest.saturating_sub(view_w));
    let pan = app.changes_pan;

    let body: Vec<Line> = if changes.is_empty() {
        vec![Line::styled(" nothing to write", dim())]
    } else {
        changes
            .iter()
            .map(|line| {
                let shown: String = line.chars().skip(pan).collect();
                Line::styled(format!(" {shown}"), change_style(line))
            })
            .collect()
    };

    frame.render_widget(Clear, popup);
    frame.render_widget(block, popup);
    frame.render_widget(Paragraph::new(body), inner);

    if widest > view_w {
        draw_hscrollbar(frame, popup, widest, pan, view_w);
    }
}

/// Colour by what the line would do: green makes something, blue moves or
/// renames it, red takes it away.
fn change_style(line: &str) -> Style {
    let colour = match line.split_whitespace().next() {
        Some("save" | "copy") => Color::Green,
        Some("rename" | "move") => Color::Blue,
        Some("delete" | "DELETE") => Color::Red,
        _ => Color::Reset,
    };
    Style::default().fg(colour)
}

/// A thumb along the bottom border showing how much is off to the sides.
fn draw_hscrollbar(frame: &mut Frame, popup: Rect, total: usize, pan: usize, view: usize) {
    let track = popup.width.saturating_sub(2) as usize;
    if track == 0 || total == 0 {
        return;
    }

    let thumb = (track * view / total).max(1).min(track);
    let at = if total > view {
        (track - thumb) * pan / (total - view)
    } else {
        0
    };

    let mut bar = String::new();
    for cell in 0..track {
        bar.push(if cell >= at && cell < at + thumb {
            '━'
        } else {
            '─'
        });
    }

    let row = Rect {
        x: popup.x + 1,
        y: popup.y + popup.height.saturating_sub(1),
        width: track as u16,
        height: 1,
    };
    frame.render_widget(
        Paragraph::new(Line::styled(bar, Style::default().fg(Color::Cyan))),
        row,
    );
}

/// Wraps one lyric at the pane width, indenting the runover so a long line
/// still reads as one line and not as two lyrics.
fn wrap_lyric(text: &str, width: usize) -> Vec<String> {
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
fn lyric_lines(app: &App, width: usize, height: usize) -> Vec<Line<'static>> {
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

fn draw_lyrics(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::LEFT)
        .border_style(dim())
        .title(" lyrics ");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    frame.render_widget(
        Paragraph::new(lyric_lines(app, inner.width as usize, inner.height as usize)),
        inner,
    );
}

/// Same content, for a terminal too narrow to give lyrics their own pane.
fn draw_lyrics_popup(frame: &mut Frame, app: &App, area: Rect) {
    let w = 50.min(area.width.saturating_sub(2));
    let h = (area.height * 3 / 4).max(3);
    let popup = Rect {
        x: (area.width.saturating_sub(w)) / 2,
        y: (area.height.saturating_sub(h)) / 2,
        width: w,
        height: h,
    };

    let block = Block::bordered().title(" lyrics: :set nolyrics to close ");
    let inner = block.inner(popup);
    frame.render_widget(Clear, popup);
    frame.render_widget(block, popup);
    frame.render_widget(
        Paragraph::new(lyric_lines(app, inner.width as usize, inner.height as usize)),
        inner,
    );
}

fn dim() -> Style {
    Style::default().fg(Color::DarkGray)
}

/// The row being renamed, in any pane.
///
/// Deliberately not the reversed cursor style: reversing a row that already
/// carries a coloured cursor block hides the block, which is how the cursor
/// went missing in the sidebar.
fn editing_style() -> Style {
    Style::default().bg(Color::Indexed(236))
}

/// Cursor line: reversed when the pane has focus, dimmed when it does not.
fn cursor_style(focused: bool) -> Style {
    if focused {
        Style::default().add_modifier(Modifier::REVERSED)
    } else {
        Style::default().bg(Color::Indexed(236))
    }
}

/// Pads or cuts to an exact number of terminal cells.
///
/// Measured in display width, not characters: a cjk glyph occupies two cells,
/// so counting characters would push every column after it out of line.
fn truncate(s: &str, width: usize) -> String {
    use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

    if width == 0 {
        return String::new();
    }
    let shown = s.width();
    if shown <= width {
        return format!("{s}{}", " ".repeat(width - shown));
    }

    let mut out = String::new();
    let mut used = 0;
    for ch in s.chars() {
        let w = ch.width().unwrap_or(0);
        if used + w > width - 1 {
            break;
        }
        out.push(ch);
        used += w;
    }
    out.push('…');
    used += 1;
    out.push_str(&" ".repeat(width.saturating_sub(used)));
    out
}

fn draw_folders(frame: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.focus == Pane::Folders;
    let block = Block::default()
        .borders(Borders::RIGHT)
        .border_style(if focused { Style::default() } else { dim() });
    let whole = block.inner(area);
    frame.render_widget(block, area);

    // Tab header, lit on the side you are looking at. `gt` switches.
    let [head, inner] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(whole);
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
            format!("* everything ({})", app.tracks.len())
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
        let doomed = row > 0 && app.folders.get(row - 1).is_some_and(|(_, p)| app.is_doomed_dir(p));
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
                lines.push(row_with_cursor(&truncate(&label, w), col, style, app.mode));
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
fn draw_playlists(frame: &mut Frame, app: &mut App, area: Rect, focused: bool) {
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
                style = style
                    .fg(Color::Red)
                    .add_modifier(Modifier::CROSSED_OUT);
            }

            let shown = renamed.unwrap_or_else(|| name.clone());
            if editing {
                let col = app.name_col();
                row_with_cursor(&truncate(&shown, w), col, style, app.mode)
            } else {
                Line::styled(truncate(&shown, w), style)
            }
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), area);
}

/// Cells before the first column: the sign, a space, the line number, a space.
///
/// The number grows with the library, so a four digit row on a library over a
/// thousand tracks widens the gutter instead of shoving the name sideways.
fn gutter(app: &App) -> usize {
    let digits = app.view.len().to_string().len().max(3);
    3 + digits
}

/// Name and width of each visible column, so the header and the rows agree.
///
/// Every visible column is followed by a space, and the duration closes the row.
fn columns(app: &App, width: usize, dur_w: usize) -> Vec<(&'static str, usize)> {
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
fn duration_width(app: &App) -> usize {
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

fn draw_tracks(frame: &mut Frame, app: &mut App, area: Rect) {
    // Tab bar on top when more than one view is open, then the sticky header.
    // Always shown, even with one tab: naming the view beats guessing it.
    let labels = app.tab_labels();
    let area = {
        let [bar, rest] =
            Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(area);
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
    let [head, area] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(area);
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
    let paused = app.audio.as_ref().is_some_and(crate::player::Audio::is_paused);
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
                lines.push(row_with_cursor(&truncate(&body, w), pad + col, base, app.mode));
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

fn draw_progress(frame: &mut Frame, app: &App, area: Rect) {
    let Some(track) = app.playing_track() else {
        frame.render_widget(Paragraph::new(Line::styled("  not playing", dim())), area);
        return;
    };

    let elapsed = app.elapsed();
    let total = track.duration;
    let bar_w = area.width.saturating_sub(18) as usize;
    let done = if total.as_secs_f64() > 0.0 {
        ((elapsed.as_secs_f64() / total.as_secs_f64()).clamp(0.0, 1.0) * bar_w as f64) as usize
    } else {
        0
    };

    let line = Line::from(vec![
        Span::raw(format!(" {:>6} ", fmt_duration(elapsed))),
        Span::styled("━".repeat(done), Style::default().fg(Color::Cyan)),
        Span::styled("─".repeat(bar_w.saturating_sub(done)), dim()),
        Span::raw(format!(" {:>6}", fmt_duration(total))),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

fn draw_status(frame: &mut Frame, app: &App, area: Rect) {
    let mode_style = Style::default()
        .add_modifier(Modifier::BOLD | Modifier::REVERSED)
        .fg(match app.mode {
            Mode::Normal => Color::Blue,
            Mode::Command | Mode::Search => Color::Yellow,
            Mode::Visual => Color::Magenta,
            Mode::Edit | Mode::EditInsert => Color::Red,
        });

    let now = app.playing_track().map_or_else(
        || "-".to_string(),
        |t| {
            if t.artist.is_empty() {
                t.title.clone()
            } else {
                format!("{} - {}", t.artist, t.title)
            }
        },
    );

    let vol = app.audio.as_ref().map_or_else(
        || "no audio".to_string(),
        |a| {
            if a.muted() {
                "mute".to_string()
            } else {
                format!("{}%", a.volume())
            }
        },
    );

    let right = format!(
        " {} {} {}  {}/{} ",
        app.sort_key.name(),
        match app.repeat {
            crate::app::Repeat::Off => "rep:-",
            crate::app::Repeat::All => "rep:all",
            crate::app::Repeat::One => "rep:one",
        },
        if app.shuffle { "shf" } else { "   " },
        if app.view.is_empty() { 0 } else { app.cur + 1 },
        app.view.len(),
    );

    let left = format!(" {} ", app.mode.label());
    // Which pane has the keyboard, so `dd` never surprises anyone.
    let where_ = match (app.focus, app.tab) {
        (Pane::Folders, Tab::Playlists) => " PLAYLISTS ",
        (Pane::Folders, Tab::Folders) => " FOLDERS ",
        (Pane::Tracks, _) => "",
    };
    // vim's modified marker: something is typed but not written.
    let dirty = if app.unsaved() { " [+] " } else { "" };
    let vol = format!(" vol {vol} ");

    // The track name is the only elastic part: everything else has to fit, or
    // a five digit track count gets its last digit shaved off.
    let fixed = left.chars().count()
        + where_.len()
        + dirty.len()
        + vol.chars().count()
        + right.chars().count();
    let middle = truncate(
        &format!(" {now} "),
        (area.width as usize).saturating_sub(fixed),
    );

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(left, mode_style),
            Span::styled(
                where_,
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::REVERSED | Modifier::BOLD),
            ),
            Span::styled(
                dirty,
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(middle, Style::default().add_modifier(Modifier::BOLD)),
            Span::styled(vol, Style::default().fg(Color::Green)),
            Span::styled(right, dim()),
        ])),
        area,
    );
}

fn draw_cmdline(frame: &mut Frame, app: &App, area: Rect) {
    let line = match app.mode {
        Mode::Command | Mode::Search => {
            let mut spans = vec![Span::raw(app.line_prefix.to_string())];
            spans.extend(with_cursor(app));
            Line::from(spans)
        }
        _ => match &app.msg {
            Some((text, true)) => Line::styled(text.clone(), Style::default().fg(Color::Red)),
            Some((text, false)) => Line::raw(text.clone()),
            None => Line::raw(""),
        },
    };

    frame.render_widget(Paragraph::new(line), area);

    // Pending count and half typed sequences sit bottom right, like vim.
    let pending = format!(
        "{}{}",
        app.count.map(|c| c.to_string()).unwrap_or_default(),
        match app.pending {
            Some('\u{17}') => "^W".to_string(),
            Some(c) => c.to_string(),
            None => String::new(),
        }
    );
    if !pending.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::styled(pending, dim())).alignment(Alignment::Right),
            area,
        );
    }
}

/// Splits a rendered row so one cell carries the cursor.
fn row_with_cursor(row: &str, at: usize, base: Style, mode: Mode) -> Line<'static> {
    let text: Vec<char> = row.chars().collect();
    let at = at.min(text.len().saturating_sub(1));
    let before: String = text[..at].iter().collect();
    let under = text.get(at).copied().unwrap_or(' ');
    let after: String = text.get(at + 1..).unwrap_or(&[]).iter().collect();

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

    Line::from(vec![
        Span::styled(before, base),
        Span::styled(under.to_string(), cursor),
        Span::styled(after, base),
    ])
}

/// The line being edited, with a block where the cursor is. A hollow looking
/// block in insert and a solid one otherwise, the way a terminal vi shows it.
fn with_cursor(app: &App) -> Vec<Span<'static>> {
    let text: Vec<char> = app.line.chars().collect();
    let at = app.line_cur.min(text.len());
    let before: String = text[..at].iter().collect();
    let under = text.get(at).copied().unwrap_or(' ');
    let after: String = text.get(at + 1..).unwrap_or(&[]).iter().collect();

    let cursor = if app.line_insert {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::UNDERLINED)
    } else {
        Style::default().add_modifier(Modifier::REVERSED)
    };

    vec![
        Span::raw(before),
        Span::styled(under.to_string(), cursor),
        Span::raw(after),
    ]
}

type HelpSection = (&'static str, &'static [(&'static str, &'static str)]);

const HELP: &[HelpSection] = &[
    (
        "windows and tabs",
        &[
            ("tab, ctrl-w h/l", "move between the side pane and the tracks"),
            ("gt gT", "switch tabs in whichever pane has the keyboard"),
            ("enter", "open the folder or playlist under the cursor"),
            ("t", "open it in a tab of its own instead"),
            (":q", "close the tab, and the last one closes vibox"),
            (":qa, ZZ", "quit, whatever is open"),
            ("F1, :help", "this window, q or esc closes it"),
        ],
    ),
    (
        "moving",
        &[
            ("j k", "down, up, and with a count: 8j"),
            ("gg G, 12G", "first row, last row, row 12"),
            ("ctrl-d ctrl-u", "half a page down, up"),
            ("ctrl-f ctrl-b", "a page down, up"),
            ("H M L", "cursor to the top, middle, bottom of the window"),
            ("zz zt zb", "window around the cursor"),
            ("ctrl-e ctrl-y", "scroll the window, leave the cursor"),
            ("gp", "jump to whatever is playing"),
            ("K", "what vibox knows about this track; y copies its path"),
            ("/ ?, n N", "search files, artists and albums, then repeat it"),
        ],
    ),
    (
        "playing",
        &[
            ("enter", "play this track, queue the rest of the view"),
            ("space", "pause, resume"),
            ("h l", "seek 5s back, forward, and 30l seeks 30s"),
            ("< >", "previous, next in the queue"),
            ("+ -, m", "volume, and 20+ raises it by 20; mute"),
            ("r", "repeat: off, all, one"),
            ("s", "shuffle the queue on, off"),
        ],
    ),
    (
        "changing things",
        &[
            ("", "nothing reaches the disk until `:w`"),
            ("c", "rename what the cursor is on: a track, folder or playlist"),
            ("i a I A", "insert while renaming, esc goes back to the motions"),
            ("cw cc dw x D C", "the usual operators, inside the name"),
            ("j k", "while renaming, take the name and move to the next row"),
            ("V, y", "select a run of tracks, yank the selection"),
            ("p", "put the yank in a playlist, or the cut in a folder"),
            ("dd x", "cut: a playlist entry, a playlist, or a file"),
            ("dd then p", "move a track: cut it, put it where it should go"),
            ("o", "new playlist or folder, by name"),
            ("u, ctrl-r", "undo, redo anything still waiting"),
            (":ch", "list exactly what `:w` would do; h and l pan it"),
            (":w", "do all of it; `:w mix` saves the view as a playlist"),
            (":e!", "throw away what is waiting"),
        ],
    ),
    (
        "danger mode, off by default",
        &[
            (":set danger", "let vibox move, copy and delete your files"),
            ("dd", "cut tracks, or a folder and all of it; never put back, deleted"),
            ("d then p", "put them somewhere else instead: a move, like vim"),
            ("y then p", "copy them into another folder"),
            (":mkdir jazz", "a new folder under the library root"),
        ],
    ),
    (
        "the library",
        &[
            (":set root=~/Music", "the library vibox opens on its own"),
            (":e ~/Music", "open a directory or an m3u for now"),
            (":reload", "rescan from disk"),
            (":sort artist", "path, title, artist, album, duration"),
            (":set artist!", "flip a column: file, title, artist, album"),
            (":set lyrics", "lyrics for the playing track, from lrclib"),
            ("[ ]", "shift those lyrics earlier, later, kept per file"),
            (":vol 70, :seek 1:30", "volume and position"),
            (":mkrc", "keep the options you have set"),
            (":42", "jump to row 42"),
        ],
    ),
    (
        "from outside",
        &[
            ("media keys", "play, pause, next, previous, over mpris"),
            ("playerctl", "the same, from a script or a status bar"),
        ],
    ),
];

/// Header, entries and the blank line between sections, flattened for scrolling.
fn help_lines() -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for (section, entries) in HELP {
        if !lines.is_empty() {
            lines.push(Line::raw(""));
        }
        lines.push(Line::styled(
            format!(" {section}"),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ));
        for (keys, what) in *entries {
            // An entry with no key is a note about the section itself.
            if keys.is_empty() {
                lines.push(Line::styled(format!("  {what}"), dim()));
                continue;
            }
            lines.push(Line::from(vec![
                Span::styled(format!("  {keys:<22}"), Style::default().fg(Color::Yellow)),
                Span::raw(*what),
            ]));
        }
    }
    lines
}

pub fn help_len() -> usize {
    help_lines().len()
}

fn draw_help(frame: &mut Frame, app: &mut App, area: Rect) {
    let lines = help_lines();
    let w = 88.min(area.width.saturating_sub(4));
    let h = (lines.len() as u16 + 2).min(area.height.saturating_sub(2));
    let popup = Rect {
        x: (area.width.saturating_sub(w)) / 2,
        y: (area.height.saturating_sub(h)) / 2,
        width: w,
        height: h,
    };

    let shown = (h as usize).saturating_sub(2);
    let more = lines.len() > shown;
    let title = if more {
        " vibox: j k to scroll, q to close "
    } else {
        " vibox: q or esc to close "
    };

    let block = Block::bordered().title(title);
    let inner = block.inner(popup);
    frame.render_widget(Clear, popup);
    frame.render_widget(block, popup);

    // Clamped here, where the height is known: scrolling past the end would
    // otherwise pile up invisibly and take as many presses to undo.
    app.help_scroll = app.help_scroll.min(lines.len().saturating_sub(shown));
    let top = app.help_scroll;
    frame.render_widget(Paragraph::new(lines[top..].to_vec()), inner);
}
