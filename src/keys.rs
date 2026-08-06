//! Modal key handling. Normal mode owns motions and playback, the `:` and `/`
//! lines are just text editors that hand their contents off on Enter.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::app::{App, Mode, Pane, Tab};
use crate::excmd;

pub fn handle(app: &mut App, key: KeyEvent) {
    // Key release and repeat events arrive on terminals that speak the kitty
    // protocol; acting on them would double every motion.
    if key.kind != KeyEventKind::Press {
        return;
    }

    // A window that is up owns the keyboard, whatever mode opened it. `:ch`
    // from inside a rename used to draw the popup while h and l still edited
    // the name behind it.
    if app.show_info || app.show_changes || app.show_help {
        window_key(app, key);
        return;
    }

    match app.mode {
        Mode::Command | Mode::Search => line_mode(app, key),
        Mode::Edit | Mode::EditInsert => edit_mode(app, key),
        Mode::Normal | Mode::Visual => normal_mode(app, key),
    }
}

/// Keys for whichever window is open: info, changes, or help.
fn window_key(app: &mut App, key: KeyEvent) {
    // The info window takes the keyboard while it is up.
    if app.show_info {
        match key.code {
            KeyCode::Char('y') => {
                let path = app.current_track().map(|t| t.path.display().to_string());
                if let Some(path) = path {
                    app.copy_to_clipboard(&path);
                }
                app.show_info = false;
            }
            KeyCode::Char('q' | 'Q' | 'K') | KeyCode::Esc | KeyCode::Enter => {
                app.show_info = false;
            }
            _ => {}
        }
        return;
    }

    // The changes window pans sideways: a rename of two long paths is wider
    // than any popup, and the interesting part is usually the far end.
    if app.show_changes {
        let step = app.count.take().unwrap_or(1) * 4;
        match key.code {
            KeyCode::Char('l') | KeyCode::Right => app.changes_pan += step,
            KeyCode::Char('h') | KeyCode::Left => {
                app.changes_pan = app.changes_pan.saturating_sub(step);
            }
            KeyCode::Char('0') => app.changes_pan = 0,
            KeyCode::Char('$') => app.changes_pan = usize::MAX,
            KeyCode::Char('q' | 'Q') | KeyCode::Esc | KeyCode::Enter => {
                app.show_changes = false;
                app.changes_pan = 0;
            }
            _ => {}
        }
        return;
    }

    // The help window takes the keyboard while it is up, like a help buffer.
    if app.show_help {
        help_key(app, key);
    }
}

/// The `:` and `/` lines. Always insert, like vim's command line, but the
/// cursor can be moved and the usual readline keys work.
fn line_mode(app: &mut App, key: KeyEvent) {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Esc => leave_line(app),
        KeyCode::Char('c') if ctrl => leave_line(app),
        KeyCode::Backspace => {
            if app.line.is_empty() {
                // Backspacing off the start of the line leaves the mode, as in vim.
                leave_line(app);
            } else if app.line_cur > 0 {
                let mut text = chars(app);
                text.remove(app.line_cur - 1);
                app.line_cur -= 1;
                set_line(app, &text);
            }
        }
        KeyCode::Delete => {
            let mut text = chars(app);
            if app.line_cur < text.len() {
                text.remove(app.line_cur);
                set_line(app, &text);
            }
        }
        KeyCode::Left => app.line_cur = app.line_cur.saturating_sub(1),
        KeyCode::Right => app.line_cur = (app.line_cur + 1).min(chars(app).len()),
        KeyCode::Home => app.line_cur = 0,
        KeyCode::End => app.line_cur = chars(app).len(),
        KeyCode::Char('u') if ctrl => {
            app.line.clear();
            app.line_cur = 0;
        }
        KeyCode::Char('w') if ctrl => {
            let mut text = chars(app);
            let start = word_start(&text, app.line_cur);
            text.drain(start..app.line_cur);
            app.line_cur = start;
            set_line(app, &text);
        }
        KeyCode::Enter => {
            let line = std::mem::take(&mut app.line);
            let was_search = app.mode == Mode::Search;
            let backward = app.line_prefix == '?';
            app.line_cur = 0;
            // The edit buffer survives a `:` command, so `:w` can see the
            // pending renames.
            app.mode = if app.edit.is_some() {
                Mode::Edit
            } else {
                Mode::Normal
            };
            if was_search {
                app.last_search = line.clone();
                app.search_back = backward;
                if !app.search(&line, backward, app.cur) {
                    app.error(format!("pattern not found: {line}"));
                }
            } else {
                excmd::run(app, &line);
            }
        }
        KeyCode::Char(c) => {
            let mut text = chars(app);
            text.insert(app.line_cur.min(text.len()), c);
            app.line_cur += 1;
            set_line(app, &text);
        }
        _ => {}
    }
}

/// The list as a buffer of filenames: vi motions and operators on the name
/// under the cursor, `j` and `k` to move between rows, `:w` to write.
///
/// Whichever pane `c` opened hands over its own `NameBuffer`, and everything
/// below works on that. There is one implementation of the editing, not one
/// per pane.
fn edit_mode(app: &mut App, key: KeyEvent) {
    if app.mode == Mode::EditInsert {
        edit_insert(app, key);
        return;
    }

    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

    // `c` and `d` wait for their motion.
    if let Some(op) = app.pending.take() {
        edit_operator(app, op, key);
        return;
    }

    // Keys that are about the pane rather than the name it is editing.
    match key.code {
        // Leaving a name is not a write: it joins the pending set, like
        // marking a file does, and `:w` writes the lot.
        KeyCode::Esc => {
            app.commit_name();
            app.mode = Mode::Normal;
            return;
        }
        KeyCode::Char(':') => {
            app.mode = Mode::Command;
            app.line_prefix = ':';
            app.line.clear();
            app.line_cur = 0;
            return;
        }
        KeyCode::Char('u') => {
            app.undo();
            return;
        }
        KeyCode::Char('r') if ctrl => {
            app.redo();
            return;
        }
        KeyCode::Char('j') | KeyCode::Down if app.renaming_a_track() => {
            app.edit_next_row(1);
            return;
        }
        KeyCode::Char('k') | KeyCode::Up if app.renaming_a_track() => {
            app.edit_next_row(-1);
            return;
        }
        KeyCode::Char('c' | 'd') => {
            app.pending = Some(if key.code == KeyCode::Char('c') { 'c' } else { 'd' });
            return;
        }
        _ => {}
    }

    // Anything that changes the text is undoable on its own.
    if matches!(
        key.code,
        KeyCode::Char('i' | 'a' | 'I' | 'A' | 'x' | 'D' | 'C' | 'S') | KeyCode::Delete
    ) {
        app.checkpoint();
    }

    let insert = matches!(key.code, KeyCode::Char('i' | 'a' | 'I' | 'A' | 'C' | 'S'));
    let Some(buf) = app.name_buffer() else { return };

    match key.code {
        KeyCode::Char('h') | KeyCode::Left => buf.left(),
        KeyCode::Char('l') | KeyCode::Right => buf.right(),
        KeyCode::Char('0') => buf.jump_start(),
        KeyCode::Char('$') => buf.jump_end(),
        KeyCode::Char('w') if !ctrl => buf.jump_word_forward(),
        KeyCode::Char('b') => buf.jump_word_back(),
        KeyCode::Char('e') => buf.jump_word_end(),
        KeyCode::Char('i') => {}
        KeyCode::Char('a') => buf.append_here(),
        KeyCode::Char('I') => buf.jump_start(),
        KeyCode::Char('A') => buf.append_at_end(),
        KeyCode::Char('x') | KeyCode::Delete => buf.delete_here(),
        KeyCode::Char('D' | 'C') => buf.truncate_here(),
        KeyCode::Char('S') => buf.clear(),
        _ => return,
    }

    if insert {
        app.mode = Mode::EditInsert;
    }
}

/// Second key of `cw`, `cc`, `dw`, `dd`.
fn edit_operator(app: &mut App, op: char, key: KeyEvent) {
    app.checkpoint();
    let Some(buf) = app.name_buffer() else { return };

    match (op, key.code) {
        ('c', KeyCode::Char('c')) | ('d', KeyCode::Char('d')) => buf.clear(),
        // vim's own quirk: `cw` changes to the end of the word, the way `ce`
        // does, instead of eating the space after it like `dw`.
        (_, KeyCode::Char('w' | 'e')) => buf.delete_word(op == 'c' || key.code == KeyCode::Char('e')),
        (_, KeyCode::Char('b')) => buf.delete_word_back(),
        (_, KeyCode::Char('$')) => buf.truncate_here(),
        _ => return,
    }

    if op == 'c' {
        app.mode = Mode::EditInsert;
    }
}

fn edit_insert(app: &mut App, key: KeyEvent) {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Esc => {
            if let Some(buf) = app.name_buffer() {
                buf.left();
            }
            app.mode = Mode::Edit;
            return;
        }
        KeyCode::Enter => {
            app.mode = Mode::Edit;
            return;
        }
        _ => {}
    }

    let Some(buf) = app.name_buffer() else { return };
    match key.code {
        KeyCode::Backspace => buf.backspace(),
        KeyCode::Delete => buf.delete_here(),
        KeyCode::Left => buf.left(),
        KeyCode::Right => buf.append_here(),
        KeyCode::Home => buf.jump_start(),
        KeyCode::End => buf.append_at_end(),
        KeyCode::Char('u') if ctrl => buf.clear(),
        KeyCode::Char('w') if ctrl => buf.delete_word_back(),
        KeyCode::Char(c) => buf.insert(c),
        _ => {}
    }
}

fn chars(app: &App) -> Vec<char> {
    app.line.chars().collect()
}

fn set_line(app: &mut App, chars: &[char]) {
    app.line = chars.iter().collect();
    app.line_cur = app.line_cur.min(app.line.chars().count());
}

/// What kind of run a character belongs to, the way vim splits a line: a word
/// of letters and digits, a run of punctuation, or whitespace between them.
///
/// This is why `w` on `ABBA - As` stops on the dash: punctuation is a word of
/// its own, not a separator to skip over.
#[derive(PartialEq, Eq, Clone, Copy)]
enum Class {
    Space,
    Word,
    Punct,
}

fn class(c: char) -> Class {
    if c.is_whitespace() {
        Class::Space
    } else if c.is_alphanumeric() || c == '_' {
        Class::Word
    } else {
        Class::Punct
    }
}

/// Start of the word at or before `at`, which is where `b` and `ctrl-w` land.
fn word_start(text: &[char], at: usize) -> usize {
    let mut i = at.min(text.len());
    while i > 0 && class(text[i - 1]) == Class::Space {
        i -= 1;
    }
    if i > 0 {
        let kind = class(text[i - 1]);
        while i > 0 && class(text[i - 1]) == kind {
            i -= 1;
        }
    }
    i
}

fn leave_line(app: &mut App) {
    app.line.clear();
    app.line_cur = 0;
    app.mode = if app.edit.is_some() {
        Mode::Edit
    } else {
        Mode::Normal
    };
}

#[allow(clippy::too_many_lines)] // it is a keymap: one arm per key reads better flat
fn normal_mode(app: &mut App, key: KeyEvent) {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let count = app.count.unwrap_or(1);
    let has_count = app.count.is_some();

    // A pending prefix (g, z, d, y, Z, ctrl-w) swallows the next key.
    if let Some(prefix) = app.pending.take() {
        pending_key(app, prefix, key, count, has_count);
        return;
    }

    // Counts: 0 only counts once a count is under way, otherwise it is a motion.
    if let KeyCode::Char(c @ '0'..='9') = key.code
        && !ctrl
        && (c != '0' || app.count.is_some())
    {
        let digit = c.to_digit(10).unwrap_or(0) as usize;
        app.count = Some(app.count.unwrap_or(0) * 10 + digit);
        return;
    }

    let mut clear_count = true;
    match key.code {
        // ---- modes -------------------------------------------------------
        KeyCode::Char(':') => {
            app.mode = Mode::Command;
            app.line_prefix = ':';
            app.line.clear();
        }
        KeyCode::Char('/') | KeyCode::Char('?') => {
            app.line_prefix = if key.code == KeyCode::Char('?') { '?' } else { '/' };
            app.mode = Mode::Search;
            app.line.clear();
        }
        KeyCode::Esc => {
            app.exit_visual();
            app.msg = None;
            app.show_help = false;
        }
        KeyCode::Char('v' | 'V') => {
            if app.mode == Mode::Visual {
                app.exit_visual();
            } else {
                app.mode = Mode::Visual;
                app.visual_anchor = Some(app.cur);
            }
        }
        KeyCode::Char('c') if ctrl => app.info("type  :q  to quit vibox"),
        KeyCode::F(1) => app.show_help = !app.show_help,
        // vim's `K`: tell me about the thing under the cursor.
        KeyCode::Char('K') if app.focus == Pane::Tracks => {
            if app.current_track().is_some() {
                app.show_info = true;
            } else {
                app.error("no track here");
            }
        }

        KeyCode::Char('q') if app.show_help => app.show_help = false,

        // ---- panes -------------------------------------------------------
        KeyCode::Tab => toggle_pane(app),
        KeyCode::Char('w') if ctrl => app.pending = Some('\u{17}'),

        // ---- motions -----------------------------------------------------
        KeyCode::Char('j') | KeyCode::Down => step(app, count as isize),
        KeyCode::Char('k') | KeyCode::Up => step(app, -(count as isize)),
        KeyCode::Char('g') => app.pending = Some('g'),
        KeyCode::Char('G') => {
            if app.focus == Pane::Tracks {
                if has_count {
                    app.goto(count - 1);
                } else {
                    app.goto(usize::MAX);
                }
            } else {
                app.move_folder(isize::MAX / 2);
            }
        }
        KeyCode::Char('z') => app.pending = Some('z'),
        KeyCode::Char(c @ ('H' | 'M' | 'L')) if app.focus == Pane::Tracks => {
            app.cursor_to_screen(c);
        }
        KeyCode::Char('d') if ctrl => step(app, (app.track_h / 2) as isize),
        KeyCode::Char('u') if ctrl => step(app, -((app.track_h / 2) as isize)),
        KeyCode::Char('f') if ctrl => step(app, app.track_h as isize),
        KeyCode::Char('b') if ctrl => step(app, -(app.track_h as isize)),
        KeyCode::PageDown => step(app, app.track_h as isize),
        KeyCode::PageUp => step(app, -(app.track_h as isize)),
        KeyCode::Char('e') if ctrl => scroll_view(app, count as isize),
        KeyCode::Char('y') if ctrl => scroll_view(app, -(count as isize)),

        // ---- search ------------------------------------------------------
        KeyCode::Char('n' | 'N') => {
            let back = app.search_back ^ (key.code == KeyCode::Char('N'));
            let pattern = app.last_search.clone();
            if pattern.is_empty() {
                app.error("no previous search");
            } else if !app.search(&pattern, back, app.cur) {
                app.error(format!("pattern not found: {pattern}"));
            }
        }

        // ---- playback ----------------------------------------------------
        KeyCode::Enter => {
            if app.focus == Pane::Folders {
                // Enter opens, on both tabs: moving the cursor never changes
                // what the track pane shows.
                match app.tab {
                    Tab::Playlists => app.open_playlist(),
                    Tab::Folders => app.open_folder(),
                }
            } else {
                app.play_cursor();
            }
        }
        KeyCode::Char(' ') => match app.audio.as_ref() {
            Some(audio) => {
                if audio.has_track() {
                    audio.toggle_pause();
                } else {
                    app.play_cursor();
                }
            }
            None => app.error("no audio device: playback is disabled"),
        },
        KeyCode::Char('h') | KeyCode::Left => seek(app, -(if has_count { count } else { 5 } as i64)),
        KeyCode::Char('l') | KeyCode::Right => seek(app, if has_count { count } else { 5 } as i64),
        KeyCode::Char('[' | ']') => {
            let step = 100 * count as i64;
            let delta = if key.code == KeyCode::Char('[') { -step } else { step };
            match app.playing_track().map(|t| t.path.clone()) {
                Some(path) => {
                    let offset = app.lyrics.nudge(&path, delta);
                    app.info(format!("lyrics {:+.1}s", offset as f64 / 1000.0));
                }
                None => app.error("nothing playing to shift the lyrics of"),
            }
        }
        KeyCode::Char('>') => app.advance(count as isize, false),
        KeyCode::Char('<') => app.advance(-(count as isize), false),
        KeyCode::Char('+' | '=') => volume(app, if has_count { count as i32 } else { 5 }),
        KeyCode::Char('-' | '_') => volume(app, -(if has_count { count as i32 } else { 5 })),
        KeyCode::Char('m') => match app.audio.as_mut() {
            Some(audio) => {
                audio.toggle_mute();
                let muted = audio.muted();
                app.info(if muted { "muted" } else { "unmuted" });
            }
            None => app.error("no audio device: playback is disabled"),
        },
        KeyCode::Char('r') if ctrl => app.redo(),
        KeyCode::Char('r') => {
            app.repeat = app.repeat.next();
            let name = app.repeat.name();
            app.info(format!("repeat {name}"));
        }
        KeyCode::Char('s') => {
            app.shuffle = !app.shuffle;
            let on = app.shuffle;
            app.info(if on { "shuffle on" } else { "shuffle off" });
        }

        KeyCode::Char('t') if app.focus == Pane::Folders => app.open_in_new_tab(),
        // A new playlist is a name, so it goes through the command line where
        // you can see and edit it before it exists.
        KeyCode::Char('o' | 'O') if app.focus == Pane::Folders => {
            app.mode = Mode::Command;
            app.line_prefix = ':';
            app.line = match app.tab {
                Tab::Playlists => "mkplaylist ".to_string(),
                Tab::Folders => "mkdir ".to_string(),
            };
            app.line_cur = app.line.chars().count();
        }

        // ---- playlists ---------------------------------------------------
        KeyCode::Char('y') => {
            if app.mode == Mode::Visual {
                app.yank_selection();
            } else {
                app.pending = Some('y');
            }
        }
        KeyCode::Char('d') => {
            if app.mode == Mode::Visual {
                delete_selection(app);
            } else {
                app.pending = Some('d');
            }
        }
        KeyCode::Char('x') if app.focus == Pane::Tracks => delete_selection(app),
        // `p` puts what you have: cut files move, yanked files copy, and in a
        // playlist the yank goes in as entries.
        KeyCode::Char('p') => {
            if !app.move_cut_here() && !app.copy_yank_here() {
                app.paste_into_playlist();
            }
        }
        KeyCode::Char('u') => app.undo(),

        // ---- editing -----------------------------------------------------
        KeyCode::Char('c') if app.focus == Pane::Tracks => app.begin_edit(),
        KeyCode::Char('c') if app.focus == Pane::Folders => app.begin_sidebar_edit(),
        KeyCode::Char('Z') => app.pending = Some('Z'),

        _ => clear_count = false,
    }

    if clear_count {
        app.count = None;
    }
}

/// Keys while `:help` is up. It scrolls, and then it closes.
fn help_key(app: &mut App, key: KeyEvent) {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let last = crate::ui::help_len().saturating_sub(1);
    let scroll = |app: &mut App, delta: isize| {
        app.help_scroll = (app.help_scroll as isize + delta).clamp(0, last as isize) as usize;
    };

    match key.code {
        KeyCode::Char('q' | 'Q') | KeyCode::Esc | KeyCode::F(1) => {
            app.show_help = false;
            app.help_scroll = 0;
        }
        KeyCode::Char('j') | KeyCode::Down => scroll(app, 1),
        KeyCode::Char('k') | KeyCode::Up => scroll(app, -1),
        KeyCode::Char('d') if ctrl => scroll(app, 10),
        KeyCode::Char('u') if ctrl => scroll(app, -10),
        KeyCode::Char('g') => app.help_scroll = 0,
        KeyCode::Char('G') => app.help_scroll = last,
        _ => {}
    }
}

/// Second key of a two key sequence.
fn pending_key(app: &mut App, prefix: char, key: KeyEvent, count: usize, has_count: bool) {
    app.count = None;
    match (prefix, key.code) {
        ('g', KeyCode::Char('g')) => {
            if app.focus == Pane::Tracks {
                app.goto(if has_count { count - 1 } else { 0 });
            } else {
                app.move_folder(isize::MIN / 2);
            }
        }
        // `gp` jumps to whatever is playing, wherever it scrolled off to.
        ('g', KeyCode::Char('p')) => {
            if let Some(playing) = app.playing
                && let Some(row) = app.view.iter().position(|&i| i == playing)
            {
                app.focus = Pane::Tracks;
                app.goto(row);
            } else {
                app.error("nothing playing in this view");
            }
        }
        // vim's own tab keys, acting on whichever pane has the keyboard.
        ('g', KeyCode::Char(c @ ('t' | 'T'))) => {
            if app.focus == Pane::Folders {
                app.tab = match app.tab {
                    Tab::Folders => Tab::Playlists,
                    Tab::Playlists => Tab::Folders,
                };
                if app.tab == Tab::Playlists {
                    app.reload_playlists();
                }
            } else {
                app.cycle_tab(if c == 't' { 1 } else { -1 });
            }
        }
        ('z', KeyCode::Char(c @ ('z' | 't' | 'b'))) => app.scroll_cursor_to(c),
        ('y', KeyCode::Char('y')) => app.yank_selection(),
        ('d', KeyCode::Char('d')) => match (app.focus, app.tab) {
            (Pane::Folders, Tab::Playlists) => app.delete_playlist(),
            (Pane::Folders, Tab::Folders) => app.cut_folder(),
            (Pane::Tracks, _) => delete_selection(app),
        },
        ('Z', KeyCode::Char('Z' | 'Q')) => app.quit = true,
        ('\u{17}', KeyCode::Char('h' | 'k')) => app.focus = Pane::Folders,
        ('\u{17}', KeyCode::Char('l' | 'j')) => app.focus = Pane::Tracks,
        ('\u{17}', KeyCode::Char('w')) => toggle_pane(app),
        _ => {}
    }
}

/// `dd`, `d` in visual, and `x`: what they delete depends on what you are
/// looking at. In a playlist it is the entry, in a folder it is the file, and
/// the file case needs danger on.
fn delete_selection(app: &mut App) {
    if app.playlist_view.is_some() {
        app.remove_from_playlist();
    } else {
        app.cut_tracks();
    }
}

fn toggle_pane(app: &mut App) {
    app.focus = match app.focus {
        Pane::Folders => Pane::Tracks,
        Pane::Tracks => Pane::Folders,
    };
}

fn step(app: &mut App, delta: isize) {
    match (app.focus, app.tab) {
        (Pane::Tracks, _) => app.move_cursor(delta),
        (Pane::Folders, Tab::Folders) => app.move_folder(delta),
        (Pane::Folders, Tab::Playlists) => app.move_playlist(delta),
    }
}

/// `ctrl-e` and `ctrl-y`: the view moves, the cursor follows only when pushed.
fn scroll_view(app: &mut App, delta: isize) {
    let max_top = app.view.len().saturating_sub(app.track_h.max(1));
    app.top = (app.top as isize + delta).clamp(0, max_top as isize) as usize;
    let cur = app.cur.clamp(app.top, app.top + app.track_h.saturating_sub(1));
    app.goto(cur);
}

fn seek(app: &mut App, delta: i64) {
    let Some(audio) = app.audio.as_ref() else {
        app.error("no audio device: playback is disabled");
        return;
    };
    if !audio.has_track() {
        app.error("nothing playing");
        return;
    }
    if let Err(e) = audio.seek_by(delta) {
        app.error(format!("{e}"));
    }
}

fn volume(app: &mut App, delta: i32) {
    let Some(audio) = app.audio.as_mut() else {
        app.error("no audio device: playback is disabled");
        return;
    };
    audio.nudge_volume(delta);
    let v = audio.volume();
    app.info(format!("volume {v}%"));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `ctrl-w` on the command line, which is the only motion left in here now
    /// that names are edited through `NameBuffer`.
    #[test]
    fn ctrl_w_deletes_back_to_the_start_of_the_word() {
        let text: Vec<char> = "late night mix".chars().collect();
        assert_eq!(word_start(&text, text.len()), 11);
        assert_eq!(word_start(&text, 11), 5);
        assert_eq!(word_start(&text, 0), 0);
    }
}
