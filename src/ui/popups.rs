//! Windows drawn over the panes: `:info`, `:changes`, the play history and the
//! help screen. An open window owns the keyboard, which `keys` enforces.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::app::App;
use crate::library::fmt_duration;

use super::name_diff::{change_style, marked, split_change};
use super::name_diff::{chars_of, common};
use super::widgets::{Window, dim, hscrollbar, vscrollbar, window, window_body_width};

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
        .map(|(i, line)| format!("{:>4}  {line}", i + 1))
        .collect();
    let widest = numbered.iter().map(|l| l.width()).max().unwrap_or(0);

    let Window { frame: popup, body } = window(frame, area, "history", "q close", widest, total);
    let rows = body.height as usize;
    app.history_top = app.history_top.min(total.saturating_sub(rows));
    let top = app.history_top;

    // The last line is what is playing now, so it gets the colour the playing
    // row gets everywhere else.
    let lines: Vec<Line> = numbered
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
    frame.render_widget(Paragraph::new(lines), body);
    vscrollbar(frame, popup, total, top, rows);
}

/// Width of the label column in the `K` window, `album artist` and a space.
const LABEL: usize = 13;

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
    let fields: Vec<_> = fields.iter().filter(|(_, v)| !v.is_empty()).collect();

    // A path is wider than any box, and its far end is the part worth reading,
    // so a long value wraps under its own column instead of being cut off.
    let widest = fields.iter().map(|(_, v)| v.width()).max().unwrap_or(0);
    let room = window_body_width(area, LABEL + widest)
        .saturating_sub(LABEL)
        .max(1);
    let mut lines = Vec::new();
    for (label, value) in &fields {
        for (i, piece) in wrap_cells(value, room).into_iter().enumerate() {
            let label = if i == 0 { *label } else { "" };
            lines.push(Line::from(vec![
                Span::styled(format!("{label:<LABEL$}"), dim()),
                Span::raw(piece),
            ]));
        }
    }

    let body = window(
        frame,
        area,
        "track info",
        "y yank · q close",
        LABEL + widest,
        lines.len(),
    )
    .body;
    frame.render_widget(Paragraph::new(lines), body);
}

/// Cuts `text` into pieces of at most `width` terminal cells each.
fn wrap_cells(text: &str, width: usize) -> Vec<String> {
    let mut pieces = vec![String::new()];
    let mut used = 0;
    for ch in text.chars() {
        let w = ch.width().unwrap_or(0);
        if used + w > width && used > 0 {
            pieces.push(String::new());
            used = 0;
        }
        if let Some(last) = pieces.last_mut() {
            last.push(ch);
        }
        used += w;
    }
    pieces
}

/// `:changes`: exactly what a `:w` would do, before you press it.
///
/// Two long paths and an arrow are wider than any box, so the window pans
/// sideways with `h` and `l`, and its bottom border says so when there is more
/// to see.
pub(super) fn draw_changes(frame: &mut Frame, app: &mut App, area: Rect) {
    let changes = app.pending_changes();

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

    let empty = "nothing to write";
    // The verb gutter, both sides of the diff and the divider between them.
    let natural = if changes.is_empty() {
        empty.len()
    } else {
        VERB + 1 + widest * 2 + 1
    };
    let Window { frame: popup, body } = window(
        frame,
        area,
        "changes",
        "y yank · q close",
        natural,
        changes.len().max(1),
    );

    if changes.is_empty() {
        frame.render_widget(Paragraph::new(Line::styled(empty, dim())), body);
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
    .areas(body);

    let rows = body.height as usize;
    app.changes_top = app.changes_top.min(changes.len().saturating_sub(rows));
    let top = app.changes_top;

    // Both columns pan together, so a path longer than half the window can
    // still be read to its end without the two sides sliding apart.
    let column = before.width as usize;
    app.changes_pan = app.changes_pan.min(widest.saturating_sub(column));
    let pan = app.changes_pan;

    let shown = changes.iter().skip(top).take(rows);

    let (mut verbs, mut olds, mut news) = (Vec::new(), Vec::new(), Vec::new());
    for line in shown {
        let (verb, old, new) = split_change(line);
        verbs.push(Line::styled(verb, change_style(line)));
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

    vscrollbar(frame, popup, changes.len(), top, rows);
    hscrollbar(frame, popup, widest, pan, column);
}

type HelpSection = (&'static str, &'static [(&'static str, &'static str)]);

pub(super) const HELP: &[HelpSection] = &[
    (
        "normal: panes and tabs",
        &[
            ("tab, ctrl-w w", "the next pane: side pane, tracks, lyrics"),
            ("ctrl-w h l", "the pane to the left, to the right"),
            ("gt gT", "next, previous tab in the focused pane"),
            ("↵", "open the folder or playlist under the cursor"),
            ("t", "open it in a tab of its own instead"),
            ("o", "a new folder or playlist, named on the : line"),
        ],
    ),
    (
        "normal: moving",
        &[
            ("j k, 8j", "down, up, and with a count"),
            ("gg G, 12G", "first row, last row, row 12"),
            ("ctrl-d ctrl-u", "half a page down, up"),
            ("ctrl-f ctrl-b", "a page down, up"),
            ("H M L", "cursor to the top, middle, bottom"),
            ("zz zt zb", "window around the cursor"),
            ("ctrl-e ctrl-y", "scroll the window, leave the cursor"),
            ("gp", "jump to whatever is playing"),
            ("/ ?, n N", "search the focused list forward, back; repeat"),
            ("* #", "next, previous track by this artist"),
            ("K", "what vibox knows about this track"),
        ],
    ),
    (
        "normal: playing",
        &[
            ("↵", "play this track, queue the rest of the view"),
            ("space", "pause, resume"),
            ("h l, 30l", "seek 5s back, forward, or 30s"),
            ("< >", "previous, next in the queue"),
            ("+ -, 20+", "volume by 5, or by the count"),
            ("m", "mute, unmute"),
            ("r", "repeat: off, all, one"),
            ("s", "shuffle the queue on, off"),
            ("[ ]", "shift the lyrics earlier, later, kept per file"),
        ],
    ),
    (
        "normal: changing things",
        &[
            ("", "nothing reaches the disk until :w"),
            ("c", "rename the track, folder or playlist here"),
            ("dd x", "cut: a playlist entry, a playlist, a file"),
            ("yy", "yank tracks"),
            ("p", "put the yank in a playlist or a folder"),
            ("dd then p", "move: cut it, put it where it goes"),
            ("u, ctrl-r", "undo, redo anything still waiting"),
        ],
    ),
    (
        "visual: v V",
        &[
            ("j k, gg G", "grow the selection"),
            ("y", "yank the selection"),
            ("d x", "cut the selection"),
            ("v V, esc", "back to normal"),
        ],
    ),
    (
        "renaming: c",
        &[
            ("h l 0 ^ $ w b e", "move inside the name"),
            ("i a I A", "insert"),
            ("cw cc dw dd yw yy", "the operators, with their motions"),
            ("x D", "cut a letter, cut to the end"),
            ("s C S", "change a letter, to the end, the whole name"),
            (
                "v, then d c y",
                "select inside the name, then cut, change, yank",
            ),
            ("p P", "put after, before the cursor"),
            ("j k", "keep this name, rename the next row"),
            ("u, ctrl-r", "undo, redo"),
            ("esc", "keep the name for :w, back to normal"),
        ],
    ),
    (
        "renaming: insert",
        &[
            ("← → home end", "move"),
            ("backspace delete", "delete a letter"),
            ("ctrl-w ctrl-u", "delete a word, the whole name"),
            ("esc, ↵", "back to renaming"),
        ],
    ),
    (
        "the : and / lines",
        &[
            ("← → home end", "move"),
            (
                "backspace delete",
                "delete a letter; backspace on nothing leaves",
            ),
            ("ctrl-w ctrl-u", "delete a word, the whole line"),
            ("↵", "run it"),
            ("esc, ctrl-c", "drop the line"),
        ],
    ),
    (
        "windows: K, :ch, :history, :help",
        &[
            ("j k, ctrl-d ctrl-u", "scroll"),
            ("gg G", "top, bottom"),
            ("h l 0 $", "pan :changes sideways"),
            ("y", "yank the path in K, the whole list in :ch"),
            ("q, esc", "close"),
        ],
    ),
    (
        "leaving",
        &[
            ("ctrl-c", "never quits: it says to type :q"),
            (":q", "close the tab, and the last one closes vibox"),
            (":qa, ZZ", "quit, whatever is open"),
            (":qa!, ZQ", "quit and throw away what :w has not written"),
            ("", ":q and :qa refuse while anything is unsaved"),
        ],
    ),
    (
        ":commands",
        &[
            (":w", "write it all; :w mix saves the view as a playlist"),
            (":ch", "list exactly what :w would do"),
            (":e!", "throw away what is waiting"),
            (":mkdir jazz", "a new folder under the library root"),
            (":set root=~/Music", "the library vibox opens on its own"),
            (":e ~/Music", "open a directory or an m3u for now"),
            (":reload", "rescan from disk"),
            (":sort artist", "path, title, artist, album, duration"),
            (":set artist!", "flip a column: file, title, artist, album"),
            (":set lyrics", "lyrics for the playing track, from lrclib"),
            (
                ":set nokaraoke",
                "the words, without them following the song",
            ),
            (":clearcache", "drop every cached lyric so they refetch"),
            (":vol 70, :seek 1:30", "volume and position"),
            (":history", "every track played this session"),
            (":mkrc", "keep the options you have set"),
            (":42", "jump to row 42"),
            (":help", "this window"),
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

/// Width of the key column in `:help`, the widest key and a space.
const HELP_KEYS: usize = 20;

/// Header, entries and the blank line between sections, flattened for scrolling.
pub(super) fn help_lines() -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for (section, entries) in HELP {
        if !lines.is_empty() {
            lines.push(Line::raw(""));
        }
        lines.push(Line::styled(
            *section,
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
                Span::styled(
                    format!("  {keys:<HELP_KEYS$}"),
                    Style::default().fg(Color::Yellow),
                ),
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
    let widest = lines.iter().map(Line::width).max().unwrap_or(0);
    let win = window(frame, area, "help", "q close", widest, lines.len());
    let body = win.body;

    // Clamped here, where the height is known: scrolling past the end would
    // otherwise pile up invisibly and take as many presses to undo.
    let shown = body.height as usize;
    app.help_scroll = app.help_scroll.min(lines.len().saturating_sub(shown));
    let top = app.help_scroll;
    frame.render_widget(Paragraph::new(lines[top..].to_vec()), body);
    vscrollbar(frame, win.frame, lines.len(), top, shown);
}
