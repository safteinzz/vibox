//! Windows drawn over the panes: `:info`, `:changes`, the play history and the
//! help screen. An open window owns the keyboard, which `keys` enforces.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};

use crate::app::App;
use crate::library::fmt_duration;

use super::name_diff::{change_style, marked, split_change};
use super::name_diff::{chars_of, common};
use super::widgets::{dim, draw_hscrollbar, draw_vscrollbar};

/// Width of the verb column in `:changes`: `renamed`, `deleted`, `save`.
pub(super) const VERB: usize = 8;

/// `:history`: what this session has played, oldest first.
///
/// Opens on the newest, because the question it answers is almost always "what
/// was that one two songs ago", and shuffle is the reason you cannot just look
/// at the list.
pub(super) fn draw_history(frame: &mut Frame, app: &mut App, area: Rect) {
    let total = app.played.len();
    let numbered: Vec<String> = app
        .played
        .iter()
        .enumerate()
        .map(|(i, line)| format!(" {:>4}  {line}", i + 1))
        .collect();

    let widest = numbered
        .iter()
        .map(|l| l.chars().count())
        .max()
        .unwrap_or(20)
        + 2;
    let w = (widest as u16).clamp(24, area.width.saturating_sub(4));
    let h = ((total as u16) + 2).min(area.height.saturating_sub(2));
    let popup = Rect {
        x: (area.width.saturating_sub(w)) / 2,
        y: (area.height.saturating_sub(h)) / 2,
        width: w,
        height: h,
    };

    let block = Block::bordered().title(format!(" played this session ({total}), q closes "));
    let inner = block.inner(popup);
    let rows = inner.height as usize;
    app.history_top = app.history_top.min(total.saturating_sub(rows));
    let top = app.history_top;

    // The last line is what is playing now, so it gets the colour the playing
    // row gets everywhere else.
    let body: Vec<Line> = numbered
        .iter()
        .enumerate()
        .skip(top)
        .take(rows)
        .map(|(i, line)| {
            if i + 1 == total && app.playing.is_some() {
                Line::styled(line.clone(), Style::default().fg(Color::Cyan))
            } else {
                Line::raw(line.clone())
            }
        })
        .collect();

    frame.render_widget(Clear, popup);
    frame.render_widget(block, popup);
    frame.render_widget(Paragraph::new(body), inner);
}

/// `K`: everything vibox knows about the track under the cursor.
pub(super) fn draw_info(frame: &mut Frame, app: &App, area: Rect) {
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
pub(super) fn draw_changes(frame: &mut Frame, app: &mut App, area: Rect) {
    let changes = app.pending_changes();

    // The whole window, not a popup floating over the track list. Two long
    // paths side by side need the width, and every row here is a change, so
    // there is nothing underneath worth keeping in view.
    let block = Block::bordered().title(" :w would do this, j k scroll, y copies, q closes ");
    let inner = block.inner(area);
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);

    if changes.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::styled(" nothing to write", dim())),
            inner,
        );
        return;
    }

    // A gutter for the verb, then the two sides of the diff with a divider
    // between them, the way vimdiff splits a window.
    let [gutter, before, divider, after] = Layout::horizontal([
        Constraint::Length(VERB as u16 + 1),
        Constraint::Min(10),
        Constraint::Length(1),
        Constraint::Min(10),
    ])
    .areas(inner);

    let rows = inner.height as usize;
    app.changes_top = app.changes_top.min(changes.len().saturating_sub(rows));
    let top = app.changes_top;

    // Both columns pan together, so a path longer than half the window can
    // still be read to its end without the two sides sliding apart.
    let widest = changes
        .iter()
        .map(|line| {
            let (_, old, new) = split_change(line);
            old.chars()
                .count()
                .max(new.map_or(0, |n| n.chars().count()))
        })
        .max()
        .unwrap_or(0);
    let column = before.width as usize;
    app.changes_pan = app.changes_pan.min(widest.saturating_sub(column));
    let pan = app.changes_pan;

    let shown = changes.iter().skip(top).take(rows);

    let (mut verbs, mut olds, mut news) = (Vec::new(), Vec::new(), Vec::new());
    for line in shown {
        let (verb, old, new) = split_change(line);
        verbs.push(Line::styled(format!(" {verb}"), change_style(line)));
        match new {
            // A rename: what goes on the left, what arrives on the right, and
            // only the characters that differ are marked.
            Some(new) => {
                let (kept_old, kept_new) = common(&chars_of(&old), &chars_of(&new));
                olds.push(marked(&old, &kept_old, Color::Red, pan));
                news.push(marked(&new, &kept_new, Color::Green, pan));
            }
            // A save or a delete names one thing, so it just sits on the left.
            None => {
                olds.push(Line::styled(
                    old.chars().skip(pan).collect::<String>(),
                    change_style(line),
                ));
                news.push(Line::raw(""));
            }
        }
    }

    frame.render_widget(Paragraph::new(verbs), gutter);
    frame.render_widget(Paragraph::new(olds), before);
    frame.render_widget(
        Paragraph::new(vec![Line::styled("│", dim()); rows]),
        divider,
    );
    frame.render_widget(Paragraph::new(news), after);

    if changes.len() > rows {
        draw_vscrollbar(frame, area, changes.len(), top, rows);
    }
    if widest > column {
        draw_hscrollbar(frame, area, widest, pan, column);
    }
}

type HelpSection = (&'static str, &'static [(&'static str, &'static str)]);

pub(super) const HELP: &[HelpSection] = &[
    (
        "windows and tabs",
        &[
            (
                "tab, ctrl-w h/l",
                "move between the side pane and the tracks",
            ),
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
            (
                "/ ?, n N",
                "search files, artists and albums, then repeat it",
            ),
            ("* #", "next, previous track by the artist under the cursor"),
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
            (
                "c",
                "rename what the cursor is on: a track, folder or playlist",
            ),
            (
                "i a I A",
                "insert while renaming, esc goes back to the motions",
            ),
            ("h l 0 ^ $ w b e", "move inside the name being renamed"),
            (
                "cw cc dw yw yy x D C",
                "the usual operators, inside the name",
            ),
            ("v", "while renaming, select inside the name; esc drops it"),
            ("d c y", "on that selection: cut, change, or copy it"),
            (
                "p P",
                "put what the last delete or copy took, after or before",
            ),
            (
                "j k",
                "while renaming, take the name and move to the next row",
            ),
            ("V, y", "select a run of tracks, yank the selection"),
            ("p", "put the yank in a playlist, or the cut in a folder"),
            ("dd x", "cut: a playlist entry, a playlist, or a file"),
            (
                "dd then p",
                "move a track: cut it, put it where it should go",
            ),
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
            (
                "dd",
                "cut tracks, or a folder and all of it; never put back, deleted",
            ),
            (
                "d then p",
                "put them somewhere else instead: a move, like vim",
            ),
            ("y then p", "copy them into another folder"),
            (":mkdir jazz", "a new folder under the library root"),
            (
                ":mkrc",
                "keeps danger on for good, if it is on when you write it",
            ),
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
            (
                ":set nokaraoke",
                "the words, without them following the song",
            ),
            (
                ":clearcache",
                "drop every cached lyric so they are fetched again",
            ),
            (":vol 70, :seek 1:30", "volume and position"),
            (":history", "every track played this session, newest last"),
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
pub(super) fn help_lines() -> Vec<Line<'static>> {
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

pub(super) fn draw_help(frame: &mut Frame, app: &mut App, area: Rect) {
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
