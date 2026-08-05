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

use crate::app::{App, Mode, Pane};
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
    if app.show_help {
        draw_help(frame, app, frame.area());
    }
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
    let inner = block.inner(area);
    frame.render_widget(block, area);

    app.folder_h = inner.height as usize;
    if app.folder_cur < app.folder_top {
        app.folder_top = app.folder_cur;
    }
    if app.folder_cur >= app.folder_top + app.folder_h.max(1) {
        app.folder_top = app.folder_cur + 1 - app.folder_h.max(1);
    }

    let w = inner.width as usize;
    let mut lines: Vec<Line> = Vec::new();
    for row in app.folder_top..(app.folder_top + inner.height as usize) {
        if row > app.folders.len() {
            break;
        }
        let label = if row == 0 {
            format!("* everything ({})", app.tracks.len())
        } else {
            app.folders[row - 1].0.clone()
        };
        let style = if row == app.folder_cur {
            cursor_style(focused)
        } else {
            Style::default()
        };
        lines.push(Line::styled(truncate(&label, w), style));
    }

    frame.render_widget(Paragraph::new(lines), inner);
}

/// Name and width of each visible column, so the header and the rows agree.
///
/// The row is `sign, space, 3 digit gutter, space` (6), then every visible
/// column followed by a space, then the duration.
fn columns(app: &App, width: usize, dur_w: usize) -> Vec<(&'static str, usize)> {
    let shown = app.columns.shown();
    let rest = width.saturating_sub(6 + shown.len() + dur_w);
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
    // The header sticks: the rows scroll under it.
    let [head, area] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(area);
    app.track_h = area.height as usize;
    app.scroll_to_cursor();

    let dur_w = duration_width(app);
    let cols = columns(app, area.width as usize, dur_w);
    let mut header = String::from("      ");
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
        let root = app.root.display().to_string();
        let msg = vec![
            Line::styled(format!("  no tracks under {root}"), dim()),
            Line::from(""),
            Line::styled("  :e ~/Music     load a library", dim()),
            Line::styled("  :help          keys", dim()),
        ];
        frame.render_widget(Paragraph::new(msg), area);
        return;
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
            format!("{:<3}", row + 1)
        } else {
            format!("{:>3}", app.cur.abs_diff(row))
        };

        let editing_row =
            is_cursor && matches!(app.mode, Mode::Edit | Mode::EditInsert);

        let base = if editing_row {
            // A reversed row would swallow the cursor cell, so while editing the
            // row only gets a quiet background and the cursor does the work.
            Style::default().bg(Color::Indexed(236))
        } else if is_cursor {
            cursor_style(focused)
        } else if is_playing {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default()
        };

        // In edit mode the file column shows the pending name instead.
        let edited = app.edit.as_ref().and_then(|edit| edit.pending.get(&track_idx));
        let changed = edited.is_some_and(|name| *name != track.file);
        let sign = if changed { '~' } else { sign };

        let mut body = format!("{sign} {number} ");
        for (name, width) in &cols {
            let value = match *name {
                "file" => edited.unwrap_or(&track.file),
                "title" => &track.title,
                "artist" => &track.artist,
                _ => &track.album,
            };
            body.push_str(&truncate(value, *width));
            body.push(' ');
        }
        body.push_str(&format!("{:>dur_w$}", fmt_duration(track.duration)));

        if editing_row {
            // The cursor sits in the name itself, six cells in: sign, space,
            // three digit gutter, space.
            let col = app.edit.as_ref().map_or(0, |edit| edit.col);
            if app.mode == Mode::EditInsert {
                // Insert uses the terminal's own cursor, so the shape can be a
                // thin bar. Painting a cell here would show a block instead.
                let text: String = app
                    .edit_text(row)
                    .unwrap_or_default()
                    .chars()
                    .take(col)
                    .collect();
                let x = area.x + 6 + u16::try_from(text.width()).unwrap_or(0);
                cursor_at = Some((x.min(area.right().saturating_sub(1)), area.y + rows));
                lines.push(Line::styled(truncate(&body, w), base));
            } else {
                lines.push(row_with_cursor(&truncate(&body, w), 6 + col, base, app.mode));
            }
        } else {
            lines.push(Line::styled(truncate(&body, w), base));
        }
    }

    app.cursor_screen = cursor_at;
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
    let vol = format!(" vol {vol} ");

    // The track name is the only elastic part: everything else has to fit, or
    // a five digit track count gets its last digit shaved off.
    let fixed = left.chars().count() + vol.chars().count() + right.chars().count();
    let middle = truncate(
        &format!(" {now} "),
        (area.width as usize).saturating_sub(fixed),
    );

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(left, mode_style),
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
        "windows and modes",
        &[
            ("tab, ctrl-w h/l", "move between the folder pane and the tracks"),
            ("V", "visual line mode, esc leaves it"),
            (":", "the command line"),
            ("F1, :help", "this window, q or esc closes it"),
            (":q, ZZ", "quit"),
        ],
    ),
    (
        "movement",
        &[
            ("j k", "down, up, and with a count: 8j"),
            ("gg G, 12G", "first row, last row, row 12"),
            ("ctrl-d ctrl-u", "half a page down, up"),
            ("ctrl-f ctrl-b", "a page down, up"),
            ("H M L", "cursor to the top, middle, bottom of the window"),
            ("zz zt zb", "window around the cursor"),
            ("ctrl-e ctrl-y", "scroll the window, leave the cursor"),
            ("gp", "jump to whatever is playing"),
        ],
    ),
    (
        "playback",
        &[
            ("enter", "play this track, queue the rest of the view"),
            ("space", "pause, resume"),
            ("h l", "seek 5s back, forward, and 30l seeks 30s"),
            ("< >", "previous, next in the queue"),
            ("+ -", "volume, and 20+ raises it by 20"),
            ("m", "mute"),
            ("[ ]", "shift the lyrics earlier, later, kept per file"),
            ("r", "repeat: off, all, one"),
            ("s", "shuffle the queue on, off"),
        ],
    ),
    (
        "search",
        &[
            ("/ ?", "forward, backward: title, artist, album, path"),
            ("n N", "next match, previous match"),
        ],
    ),
    (
        "renaming files",
        &[
            ("c", "edit the names in the list, vi motions inside the row"),
            ("i a I A", "insert, and esc goes back to the motions"),
            ("cw cc dw x D C", "the operators, on the name under the cursor"),
            ("j k", "move to another row, still editing"),
            (":w", "rename every changed file at once"),
            (":e!", "throw the pending renames away"),
        ],
    ),
    (
        "the library",
        &[
            (":e ~/Music", "open a directory as the library"),
            (":e mix.m3u", "open an m3u playlist, in its own order"),
            (":reload", "rescan from disk"),
            (":sort artist", "path, title, artist, album, duration"),
            (":vol 70, :vol +5", "volume"),
            (":seek 1:30, :seek +30", "seek"),
            (":set artist!", "flip a column: file, title, artist, album"),
            (":set lyrics", "lyrics pane for the playing track, from lrclib"),
            (":mkrc", "keep the current :set options for next time"),
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

fn draw_help(frame: &mut Frame, app: &App, area: Rect) {
    let lines = help_lines();
    let w = 74.min(area.width.saturating_sub(4));
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

    let top = app.help_scroll.min(lines.len().saturating_sub(shown));
    frame.render_widget(Paragraph::new(lines[top..].to_vec()), inner);
}
