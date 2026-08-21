//! The three fixed lines under the panes: the progress bar, the statusline and
//! the command line. Only the now-playing segment of the statusline may stretch.

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::{App, Mode, Pane, Tab};
use crate::library::fmt_duration;

use super::widgets::{dim, truncate};

pub(super) fn draw_progress(frame: &mut Frame, app: &App, area: Rect) {
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

pub(super) fn draw_status(frame: &mut Frame, app: &App, area: Rect) {
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

    // A selection inside a name is still `Mode::Edit`, but saying EDIT while
    // half a filename is highlighted is a lie about what the next key does.
    let left = if app.name_selecting() {
        " VISUAL ".to_string()
    } else {
        format!(" {} ", app.mode.label())
    };
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

pub(super) fn draw_cmdline(frame: &mut Frame, app: &App, area: Rect) {
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

/// The line being edited, with a block where the cursor is. A hollow looking
/// block in insert and a solid one otherwise, the way a terminal vi shows it.
pub(super) fn with_cursor(app: &App) -> Vec<Span<'static>> {
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
