//! Normal mode: motions, counts, playback and the multi-key sequences (`g`,
//! `z`, `d`, `y`, `Z`, ctrl-w) that `app.pending` collects.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::{App, Mode, Pane, Tab};

#[allow(clippy::too_many_lines)] // it is a keymap: one arm per key reads better flat
pub(super) fn normal_mode(app: &mut App, key: KeyEvent) {
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
            app.line_prefix = if key.code == KeyCode::Char('?') {
                '?'
            } else {
                '/'
            };
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
        // vim's `*` and `#`: the thing under the cursor becomes the search.
        // Here that is the artist, since "what else do I have by them" is the
        // question you ask while browsing.
        KeyCode::Char(c @ ('*' | '#')) if app.focus == Pane::Tracks => {
            let artist = app
                .current_track()
                .map(|t| t.artist.trim().to_string())
                .unwrap_or_default();
            if artist.is_empty() {
                app.error("no artist on this track");
            } else {
                let back = c == '#';
                app.last_search = artist.clone();
                app.search_back = back;
                if app.search(&artist, back, app.cur) {
                    app.info(format!("searching `{artist}`, `n` for the next one"));
                } else {
                    app.error(format!("pattern not found: {artist}"));
                }
            }
        }
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
        KeyCode::Char('h') | KeyCode::Left => {
            seek(app, -(if has_count { count } else { 5 } as i64))
        }
        KeyCode::Char('l') | KeyCode::Right => seek(app, if has_count { count } else { 5 } as i64),
        KeyCode::Char('[' | ']') => {
            let step = 100 * count as i64;
            let delta = if key.code == KeyCode::Char('[') {
                -step
            } else {
                step
            };
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

/// Second key of a two key sequence.
pub(super) fn pending_key(
    app: &mut App,
    prefix: char,
    key: KeyEvent,
    count: usize,
    has_count: bool,
) {
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
pub(super) fn delete_selection(app: &mut App) {
    if app.playlist_view.is_some() {
        app.remove_from_playlist();
    } else {
        app.cut_tracks();
    }
}

pub(super) fn toggle_pane(app: &mut App) {
    app.focus = match app.focus {
        Pane::Folders => Pane::Tracks,
        Pane::Tracks => Pane::Folders,
    };
}

pub(super) fn step(app: &mut App, delta: isize) {
    match (app.focus, app.tab) {
        (Pane::Tracks, _) => app.move_cursor(delta),
        (Pane::Folders, Tab::Folders) => app.move_folder(delta),
        (Pane::Folders, Tab::Playlists) => app.move_playlist(delta),
    }
}

/// `ctrl-e` and `ctrl-y`: the view moves, the cursor follows only when pushed.
pub(super) fn scroll_view(app: &mut App, delta: isize) {
    let max_top = app.view.len().saturating_sub(app.track_h.max(1));
    app.top = (app.top as isize + delta).clamp(0, max_top as isize) as usize;
    let cur = app
        .cur
        .clamp(app.top, app.top + app.track_h.saturating_sub(1));
    app.goto(cur);
}

pub(super) fn seek(app: &mut App, delta: i64) {
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

pub(super) fn volume(app: &mut App, delta: i32) {
    let Some(audio) = app.audio.as_mut() else {
        app.error("no audio device: playback is disabled");
        return;
    };
    audio.nudge_volume(delta);
    let v = audio.volume();
    app.info(format!("volume {v}%"));
}
