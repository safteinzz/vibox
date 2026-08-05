//! Modal key handling. Normal mode owns motions and playback, the `:` and `/`
//! lines are just text editors that hand their contents off on Enter.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::app::{App, Mode, Pane};
use crate::excmd;

pub fn handle(app: &mut App, key: KeyEvent) {
    // Key release and repeat events arrive on terminals that speak the kitty
    // protocol; acting on them would double every motion.
    if key.kind != KeyEventKind::Press {
        return;
    }

    match app.mode {
        Mode::Command | Mode::Search => line_mode(app, key),
        Mode::Edit | Mode::EditInsert => edit_mode(app, key),
        Mode::Normal => normal_mode(app, key),
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
fn edit_mode(app: &mut App, key: KeyEvent) {
    if app.mode == Mode::EditInsert {
        edit_insert(app, key);
        return;
    }

    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let mut text = edit_chars(app);
    let last = text.len().saturating_sub(1);
    let col = edit_col(app).min(last);

    // `c` and `d` wait for their motion.
    if let Some(op) = app.pending.take() {
        edit_operator(app, op, key);
        return;
    }

    match key.code {
        KeyCode::Esc => {
            if app.edit_dirty() {
                app.error("no write since last change: `:w` renames, `:e!` throws it away");
            } else {
                app.end_edit();
            }
        }
        KeyCode::Char(':') => {
            app.mode = Mode::Command;
            app.line_prefix = ':';
            app.line.clear();
            app.line_cur = 0;
        }
        KeyCode::Char('j') | KeyCode::Down => {
            app.move_cursor(1);
            set_edit_col(app, col);
        }
        KeyCode::Char('k') | KeyCode::Up => {
            app.move_cursor(-1);
            set_edit_col(app, col);
        }
        KeyCode::Char('h') | KeyCode::Left => set_edit_col(app, col.saturating_sub(1)),
        KeyCode::Char('l') | KeyCode::Right => set_edit_col(app, (col + 1).min(last)),
        KeyCode::Char('0') => set_edit_col(app, 0),
        KeyCode::Char('$') => set_edit_col(app, last),
        KeyCode::Char('w') if !ctrl => {
            set_edit_col(app, word_forward(&text, col).min(last));
        }
        KeyCode::Char('b') => set_edit_col(app, word_start(&text, col)),
        KeyCode::Char('e') => set_edit_col(app, word_end(&text, col)),
        KeyCode::Char('i') => app.mode = Mode::EditInsert,
        KeyCode::Char('a') => {
            set_edit_col(app, (col + 1).min(text.len()));
            app.mode = Mode::EditInsert;
        }
        KeyCode::Char('I') => {
            set_edit_col(app, 0);
            app.mode = Mode::EditInsert;
        }
        KeyCode::Char('A') => {
            set_edit_col(app, text.len());
            app.mode = Mode::EditInsert;
        }
        KeyCode::Char('x') | KeyCode::Delete => {
            if col < text.len() {
                text.remove(col);
                set_edit_col(app, col.min(text.len().saturating_sub(1)));
                app.set_edit_text(text.iter().collect());
            }
        }
        KeyCode::Char('D') => {
            text.truncate(col);
            app.set_edit_text(text.iter().collect());
        }
        KeyCode::Char('C') => {
            text.truncate(col);
            app.set_edit_text(text.iter().collect());
            app.mode = Mode::EditInsert;
        }
        KeyCode::Char('S') => {
            app.set_edit_text(String::new());
            set_edit_col(app, 0);
            app.mode = Mode::EditInsert;
        }
        KeyCode::Char('c' | 'd') => {
            app.pending = Some(if key.code == KeyCode::Char('c') { 'c' } else { 'd' });
        }
        _ => {}
    }
}

/// Second key of `cw`, `cc`, `dw`, `dd`.
fn edit_operator(app: &mut App, op: char, key: KeyEvent) {
    let mut text = edit_chars(app);
    let col = edit_col(app).min(text.len());

    match (op, key.code) {
        ('c', KeyCode::Char('c')) | ('d', KeyCode::Char('d')) => {
            app.set_edit_text(String::new());
            set_edit_col(app, 0);
            if op == 'c' {
                app.mode = Mode::EditInsert;
            }
        }
        (_, KeyCode::Char('w' | 'e')) => {
            let to = if key.code == KeyCode::Char('e') {
                word_end(&text, col) + 1
            } else {
                word_forward(&text, col).max(col)
            };
            let to = to.min(text.len());
            text.drain(col..to);
            app.set_edit_text(text.iter().collect());
            if op == 'c' {
                app.mode = Mode::EditInsert;
            }
        }
        (_, KeyCode::Char('b')) => {
            let from = word_start(&text, col);
            text.drain(from..col);
            set_edit_col(app, from);
            app.set_edit_text(text.iter().collect());
            if op == 'c' {
                app.mode = Mode::EditInsert;
            }
        }
        (_, KeyCode::Char('$')) => {
            text.truncate(col);
            app.set_edit_text(text.iter().collect());
            if op == 'c' {
                app.mode = Mode::EditInsert;
            }
        }
        _ => {}
    }
}

fn edit_insert(app: &mut App, key: KeyEvent) {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let mut text = edit_chars(app);
    let col = edit_col(app).min(text.len());

    match key.code {
        KeyCode::Esc => {
            set_edit_col(app, col.saturating_sub(1));
            app.mode = Mode::Edit;
        }
        KeyCode::Enter => app.mode = Mode::Edit,
        KeyCode::Backspace => {
            if col > 0 {
                text.remove(col - 1);
                set_edit_col(app, col - 1);
                app.set_edit_text(text.iter().collect());
            }
        }
        KeyCode::Delete => {
            if col < text.len() {
                text.remove(col);
                app.set_edit_text(text.iter().collect());
            }
        }
        KeyCode::Left => set_edit_col(app, col.saturating_sub(1)),
        KeyCode::Right => set_edit_col(app, (col + 1).min(text.len())),
        KeyCode::Home => set_edit_col(app, 0),
        KeyCode::End => set_edit_col(app, text.len()),
        KeyCode::Char('u') if ctrl => {
            app.set_edit_text(String::new());
            set_edit_col(app, 0);
        }
        KeyCode::Char('w') if ctrl => {
            let start = word_start(&text, col);
            text.drain(start..col);
            set_edit_col(app, start);
            app.set_edit_text(text.iter().collect());
        }
        KeyCode::Char(c) => {
            text.insert(col, c);
            set_edit_col(app, col + 1);
            app.set_edit_text(text.iter().collect());
        }
        _ => {}
    }
}

fn edit_chars(app: &App) -> Vec<char> {
    app.edit_text(app.cur).unwrap_or_default().chars().collect()
}

fn edit_col(app: &App) -> usize {
    app.edit.as_ref().map_or(0, |edit| edit.col)
}

fn set_edit_col(app: &mut App, col: usize) {
    if let Some(edit) = app.edit.as_mut() {
        edit.col = col;
    }
}

fn chars(app: &App) -> Vec<char> {
    app.line.chars().collect()
}

fn set_line(app: &mut App, chars: &[char]) {
    app.line = chars.iter().collect();
    app.line_cur = app.line_cur.min(app.line.chars().count());
}

/// Start of the word at or before `at`, which is where `b` and `ctrl-w` land.
fn word_start(text: &[char], at: usize) -> usize {
    let mut i = at.min(text.len());
    while i > 0 && !text[i - 1].is_alphanumeric() {
        i -= 1;
    }
    while i > 0 && text[i - 1].is_alphanumeric() {
        i -= 1;
    }
    i
}

/// Start of the next word, or the end of the text when there is none.
///
/// Not clamped to the last character: `cw` on a single word has to delete that
/// word entirely, while the `w` motion clamps at the call site.
fn word_forward(text: &[char], at: usize) -> usize {
    let mut i = at;
    while i < text.len() && text[i].is_alphanumeric() {
        i += 1;
    }
    while i < text.len() && !text[i].is_alphanumeric() {
        i += 1;
    }
    i
}

/// End of the current word, which is where `e` lands.
fn word_end(text: &[char], at: usize) -> usize {
    let mut i = at + 1;
    while i < text.len() && !text[i].is_alphanumeric() {
        i += 1;
    }
    while i + 1 < text.len() && text[i + 1].is_alphanumeric() {
        i += 1;
    }
    i.min(text.len().saturating_sub(1))
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
    // The help window takes the keyboard while it is up, like a help buffer.
    if app.show_help {
        help_key(app, key);
        return;
    }

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
            app.msg = None;
            app.show_help = false;
        }
        KeyCode::Char('c') if ctrl => app.info("type  :q  to quit vibox"),
        KeyCode::F(1) => app.show_help = !app.show_help,
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
                app.focus = Pane::Tracks;
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

        // ---- editing -----------------------------------------------------
        KeyCode::Char('c') if app.focus == Pane::Tracks => app.begin_edit(),
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
        ('z', KeyCode::Char(c @ ('z' | 't' | 'b'))) => app.scroll_cursor_to(c),
        ('Z', KeyCode::Char('Z' | 'Q')) => app.quit = true,
        ('\u{17}', KeyCode::Char('h' | 'k')) => app.focus = Pane::Folders,
        ('\u{17}', KeyCode::Char('l' | 'j')) => app.focus = Pane::Tracks,
        ('\u{17}', KeyCode::Char('w')) => toggle_pane(app),
        _ => {}
    }
}

fn toggle_pane(app: &mut App) {
    app.focus = match app.focus {
        Pane::Folders => Pane::Tracks,
        Pane::Tracks => Pane::Folders,
    };
}

fn step(app: &mut App, delta: isize) {
    match app.focus {
        Pane::Tracks => app.move_cursor(delta),
        Pane::Folders => app.move_folder(delta),
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
