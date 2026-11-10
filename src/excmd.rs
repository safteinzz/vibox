//! Ex commands: everything typed after `:`.
//!
//! Names follow vim where vim has an opinion (`:q`, `:e`, `:w`, `:h`) and stay
//! spelled out where it does not (`:vol`, `:seek`, `:shuffle`).

use std::path::PathBuf;
use std::time::Duration;

use crate::app::jobs::ScanFor;
use crate::app::{App, Pane, Repeat};
use crate::library::SortKey;

pub struct Parsed<'a> {
    pub name: &'a str,
    pub bang: bool,
    pub args: &'a str,
}

pub fn parse(line: &str) -> Option<Parsed<'_>> {
    let line = line.trim_start();
    if line.is_empty() {
        return None;
    }
    let (head, args) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
    let bang = head.ends_with('!');
    Some(Parsed {
        name: head.trim_end_matches('!'),
        bang,
        args: args.trim(),
    })
}

pub fn run(app: &mut App, line: &str) {
    let Some(cmd) = parse(line) else {
        return;
    };

    // `:42` is a line number, same as vim.
    if let Ok(row) = cmd.name.parse::<usize>() {
        app.goto(row.saturating_sub(1));
        return;
    }

    match cmd.name {
        "q" | "qa" | "quit" | "qall" | "x" => {
            let all = cmd.name == "qa" || cmd.name == "qall";
            // Like vim: `:q` cares about the tab it is closing, `:qa` cares
            // about everything. Closing a clean tab is never blocked by changes
            // sitting in a different one.
            let closing_a_tab = !all && app.tabs.len() > 1;
            let blocked = if closing_a_tab {
                app.view_edited()
            } else {
                app.unsaved()
            };

            if blocked && !cmd.bang {
                app.error(
                    "no write since last change: `:ch` lists it, `:w` saves, `:q!` throws it away",
                );
            } else if all {
                app.quit = true;
            } else {
                app.close_tab();
            }
        }
        // `:e!` is vim's "reread and lose my edits", so it drops everything
        // waiting for a `:w`, not only the name being typed.
        "e" | "edit" | "cd" if cmd.bang && cmd.args.is_empty() => {
            if app.job.is_none() {
                app.discard_changes();
                // Then the disk, read again, for anything that changed under it.
                let root = app.root.clone();
                app.start_scan(root, ScanFor::Refresh);
            } else {
                app.discard_changes();
            }
        }
        "e" | "edit" | "cd" => edit(app, cmd.args, cmd.bang),
        "pwd" => {
            let root = app.root.display().to_string();
            app.info(root);
        }
        "reload" | "scan" => {
            if refused_while_unsaved(app, "reload") {
                return;
            }
            let root = app.root.clone();
            app.start_scan(root, ScanFor::Reload);
        }
        // One `:w` writes everything outstanding: the pending renames and the
        // playlist you changed. With a name it saves the view as a new one.
        "w" | "write" | "save" => {
            if !cmd.args.is_empty() {
                app.save_playlist(cmd.args, cmd.bang);
            } else {
                app.start_write();
            }
        }
        "vol" | "volume" => volume(app, cmd.args),
        "mute" => match app.audio.as_mut() {
            Some(audio) => {
                audio.toggle_mute();
                let muted = audio.muted();
                app.info(if muted { "muted" } else { "unmuted" });
            }
            None => app.error("no audio device: playback is disabled"),
        },
        "seek" => seek(app, cmd.args),
        "next" | "n" => app.advance(1, false),
        "prev" | "p" | "previous" => app.advance(-1, false),
        "play" => {
            if app
                .audio
                .as_ref()
                .is_some_and(super::player::Audio::is_paused)
            {
                if let Some(audio) = app.audio.as_ref() {
                    audio.resume();
                }
            } else {
                app.play_cursor();
            }
        }
        "pause" => match app.audio.as_ref() {
            Some(audio) => audio.pause(),
            None => app.error("no audio device: playback is disabled"),
        },
        "stop" => app.stop(),
        "repeat" => {
            app.repeat = match cmd.args {
                "" | "toggle" => app.repeat.next(),
                "off" | "none" => Repeat::Off,
                "all" => Repeat::All,
                "one" | "track" => Repeat::One,
                other => {
                    app.error(format!("bad repeat mode `{other}`, use off, all or one"));
                    return;
                }
            };
            let name = app.repeat.name();
            app.info(format!("repeat {name}"));
        }
        "shuffle" => {
            app.shuffle = match cmd.args {
                "" | "toggle" => !app.shuffle,
                "on" | "1" => true,
                "off" | "0" => false,
                other => {
                    app.error(format!("bad shuffle value `{other}`, use on or off"));
                    return;
                }
            };
            let on = app.shuffle;
            app.info(if on { "shuffle on" } else { "shuffle off" });
        }
        "sort" => match SortKey::parse(cmd.args) {
            Some(key) => {
                app.set_sort(key);
                let name = key.name();
                app.info(format!("sorted by {name}"));
            }
            None => app.error("bad sort key, use path, title, artist, album or duration"),
        },
        "set" | "se" => set(app, cmd.args),
        "mkrc" => mkrc(app, cmd.bang),
        // No bang: everything in here can be fetched again, so the worst a
        // stray `:clearcache` costs is one lookup per track.
        "clearcache" | "clearc" => match app.lyrics.clear() {
            Ok(0) => app.info("no cached lyrics to clear"),
            Ok(n) => app.info(format!("cleared {n} cached lyrics, they refetch on play")),
            Err(e) => app.error(format!(
                "cannot clear the lyrics cache: {}",
                crate::reason::of(&e)
            )),
        },
        "history" | "hist" => {
            if app.played.is_empty() {
                app.info("nothing played yet this session");
            } else {
                // Open on the newest, which is what you came to read.
                app.history_top = usize::MAX;
                app.show_history = true;
            }
        }
        "matrix" => {
            app.matrix.on = !app.matrix.on;
            if app.matrix.on {
                app.info("wake up...");
            } else {
                app.msg = None;
            }
        }
        "mkplaylist" | "mkpl" => app.create_playlist(cmd.args),
        "mkdir" => app.make_dir(cmd.args),
        "changes" | "ch" => app.show_changes = true,
        "h" | "help" => app.show_help = true,
        other => app.error(format!(
            "not a vibox command: `{other}`, `:help` lists them"
        )),
    }
}

/// `:set`, spelled the way vim spells it: `:set artist`, `:set noartist`,
/// `:set artist!` to flip it and `:set artist?` to ask.
fn set(app: &mut App, args: &str) {
    if args.is_empty() {
        app.info(list_options(app));
        return;
    }

    // `root=<path>` is the one option with a value: the library vibox opens
    // when it is started with no path.
    if let Some(rest) = args.strip_prefix("root=") {
        let rest = rest.trim();
        if rest.is_empty() {
            app.error("`:set root=` needs a path: `:set root=~/Music`");
            return;
        }
        let path = crate::library::absolute(&expand(rest));
        // Already open, as when the rc names the library this run started on:
        // reading it a second time only makes the start twice as slow.
        if path == app.root {
            let shown = path.display().to_string();
            app.music = Some(path);
            app.info(format!("library is {shown}, `:mkrc` keeps it"));
            return;
        }
        if refused_while_unsaved(app, "open another library") {
            return;
        }
        app.start_scan(path, ScanFor::Root);
        return;
    }
    if args.trim() == "root?" {
        let shown = app
            .music
            .clone()
            .unwrap_or_else(|| app.root.clone())
            .display()
            .to_string();
        app.info(format!("root={shown}"));
        return;
    }

    let mut touched: Vec<String> = Vec::new();
    let mut unknown: Vec<&str> = Vec::new();
    for word in args.split_whitespace() {
        let (name, action) = match word {
            w if w.ends_with('?') => (w.trim_end_matches('?'), '?'),
            w if w.ends_with('!') => (w.trim_end_matches('!'), '!'),
            w if w.starts_with("no") && option(app, &w[2..]).is_some() => (&w[2..], '0'),
            w => (w, '1'),
        };

        // An option we do not have is skipped rather than fatal, so one stale
        // name in an rc file (`set danger`, which used to exist) cannot cost
        // you the options written beside it. Typed by hand it still says so;
        // `load_rc` clears the line when it is the file talking.
        let Some(current) = option(app, name) else {
            unknown.push(name);
            continue;
        };

        match action {
            '?' => {
                let no = if current { "" } else { "no" };
                app.info(format!("{no}{name}"));
                return;
            }
            '!' => set_option(app, name, !current),
            '0' => set_option(app, name, false),
            _ => set_option(app, name, true),
        }

        // Report back only what was touched. Listing every option on a toggle
        // buries the answer to the question that was actually asked.
        let now = option(app, name).unwrap_or(false);
        touched.push(format!("{}{name}", if now { "" } else { "no" }));
    }

    if !unknown.is_empty() {
        let names = unknown
            .iter()
            .map(|n| format!("`{n}`"))
            .collect::<Vec<_>>()
            .join(" ");
        let set = if touched.is_empty() {
            String::new()
        } else {
            format!("; set {}", touched.join(" "))
        };
        app.error(format!(
            "unknown option {names}, the options are {}{set}",
            OPTIONS.join(" ")
        ));
        return;
    }
    // An option only lasts the session until `:mkrc` writes it, so say so
    // every time rather than expecting anyone to remember.
    app.info(format!("{} | `:mkrc` keeps these", touched.join(" ")));
}

/// Refuses a command that would throw away what `:w` has not written, since
/// pending changes point at this library's rows. True when it refused.
fn refused_while_unsaved(app: &mut App, what: &str) -> bool {
    // A write still running says so, rather than asking for the write.
    if app.busy() {
        return true;
    }
    if !app.unsaved() {
        return false;
    }
    app.error(format!(
        "no write since last change: `:w` first, or `:e!` to throw it away, then {what}"
    ));
    true
}

/// Where the last session is remembered. This is state, not configuration, so
/// it lives in the data dir and never touches the rc file the user edits.
fn state_path() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("vibox/state"))
}

/// Writes volume, shuffle, repeat and the options back out on quit, as ex
/// commands, so restoring them is just running them.
pub fn save_state(app: &App) {
    let Some(path) = state_path() else {
        return;
    };
    if let Some(dir) = path.parent()
        && std::fs::create_dir_all(dir).is_err()
    {
        return;
    }

    // A session with no sound server never knew the volume, so the one the
    // last session saved stands rather than a made up default.
    let volume = match app.audio.as_ref() {
        Some(audio) => audio.volume(),
        None => saved_volume().unwrap_or(80),
    };
    let body = format!(
        "\" last session, overwritten on quit. edit viboxrc instead\nset {}\nvol {volume}\nshuffle {}\nrepeat {}\n",
        saved_options(app),
        if app.shuffle { "on" } else { "off" },
        app.repeat.name(),
    );
    let _ = std::fs::write(path, body);
}

/// The volume the last session saved, if it saved one.
fn saved_volume() -> Option<u8> {
    let text = std::fs::read_to_string(state_path()?).ok()?;
    text.lines()
        .find_map(|line| line.trim().strip_prefix("vol "))
        .and_then(|v| v.trim().parse().ok())
}

/// Restores the last session. Runs before the rc file, so a line the user put
/// in `viboxrc` always wins over what they happened to leave behind.
pub fn load_state(app: &mut App) {
    let Some(path) = state_path() else {
        return;
    };
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    for line in text.lines() {
        let line = line.trim();
        if !line.is_empty() && !line.starts_with('"') {
            run(app, line);
        }
    }
    app.msg = None;
}

/// `~/.config/vibox/viboxrc`, a file of ex commands run at startup.
pub fn rc_path() -> Option<PathBuf> {
    Some(dirs::config_dir()?.join("vibox/viboxrc"))
}

/// The library named by `set root=` in the rc file, read before any state
/// exists so startup can open it. Read with the same parser `load_rc` runs the
/// line with, so `se root=` names it too.
pub fn configured_music() -> Option<PathBuf> {
    let text = std::fs::read_to_string(rc_path()?).ok()?;
    text.lines()
        .map(str::trim)
        .filter(|line| !line.starts_with('"'))
        .filter_map(|line| rc_root(line.trim_start_matches(':')))
        .map(str::trim)
        .rfind(|value| !value.is_empty())
        .map(|value| crate::library::absolute(&expand(value)))
}

/// Runs the rc file. Every line is an ex command without its `:`, and `"`
/// starts a comment, the way a vimrc reads.
///
/// `argv_root` is true when a library was named on the command line. That one
/// wins for this session, so the rc's `set root=` is remembered for `:mkrc`
/// and for the next launch without it, but must not reopen over the library
/// the user actually asked for. `:set root=` typed by hand still opens, since
/// that is someone saying so now.
pub fn load_rc(app: &mut App, argv_root: bool) {
    let Some(path) = rc_path() else {
        return;
    };
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('"') {
            continue;
        }
        let line = line.trim_start_matches(':');
        match rc_root(line) {
            Some(rest) if argv_root => {
                app.music = Some(crate::library::absolute(&expand(rest.trim())));
            }
            _ => run(app, line),
        }
    }
    app.msg = None;
}

/// The library an rc line names, if that is what it does: `set root=<path>`.
fn rc_root(line: &str) -> Option<&str> {
    let cmd = parse(line)?;
    if matches!(cmd.name, "set" | "se") {
        cmd.args.strip_prefix("root=")
    } else {
        None
    }
}

/// `:mkrc`: writes the current options back out.
fn mkrc(app: &mut App, bang: bool) {
    let Some(path) = rc_path() else {
        app.error("cannot find a config directory to write to");
        return;
    };
    if path.exists() && !bang {
        let shown = path.display();
        app.error(format!("`{shown}` exists, `:mkrc!` overwrites it"));
        return;
    }

    let music = app
        .music
        .clone()
        .unwrap_or_else(|| app.root.clone())
        .display()
        .to_string();
    let body = format!(
        "\" written by :mkrc\nset root={music}\nset {}\n",
        saved_options(app)
    );
    let wrote = path
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(&path, body));

    match wrote {
        Ok(()) => app.info(format!("wrote {}", path.display())),
        Err(e) => {
            let shown = path.display();
            app.fail(
                "rc file not written",
                format!("cannot write `{shown}`: {}", crate::reason::of(&e)),
            );
        }
    }
}

/// Every `:set` option: the tag columns, plus the panes that can be turned off.
const OPTIONS: [&str; 6] = ["file", "title", "artist", "album", "lyrics", "karaoke"];

fn option(app: &App, name: &str) -> Option<bool> {
    match name {
        "lyrics" => Some(app.show_lyrics),
        "karaoke" => Some(app.karaoke),
        other => app.columns.get(other),
    }
}

fn set_option(app: &mut App, name: &str, on: bool) {
    if name == "karaoke" {
        app.karaoke = on;
    } else if name == "lyrics" {
        app.show_lyrics = on;
        // The pane cannot keep the keyboard once it is gone.
        if !on && app.focus == Pane::Lyrics {
            app.focus = Pane::Tracks;
        }
    } else {
        app.columns.set(name, on);
    }
}

/// The options shared by `:mkrc` and the session state.
pub(crate) fn saved_options(app: &App) -> String {
    OPTIONS
        .iter()
        .map(|name| {
            let on = option(app, name).unwrap_or(false);
            format!("{}{name}", if on { "" } else { "no" })
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn list_options(app: &App) -> String {
    OPTIONS
        .iter()
        .map(|name| {
            let on = option(app, name).unwrap_or(false);
            format!("{}{name}", if on { "" } else { "no" })
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// `:e <path>`, `:e! <path>` and a bare `:e`, which rereads the library.
///
/// It will not throw away what `:w` has not written: `:e!` drops it first,
/// and a plain `:e` refuses while anything is pending.
fn edit(app: &mut App, args: &str, bang: bool) {
    if app.job.is_some() {
        // Says what is running, and changes nothing.
        app.discard_changes();
        return;
    }
    if bang {
        app.discard_changes();
    } else if refused_while_unsaved(app, "open it") {
        return;
    }
    let target = if args.is_empty() {
        app.root.clone()
    } else {
        crate::library::absolute(&app.root_dir().join(expand(args)))
    };
    app.start_scan(target, ScanFor::Open);
}

fn volume(app: &mut App, args: &str) {
    let Some(audio) = app.audio.as_mut() else {
        app.error("no audio device: playback is disabled");
        return;
    };
    if args.is_empty() {
        let v = audio.volume();
        app.info(format!("volume {v}%"));
        return;
    }
    match parse_delta(args) {
        Some(Delta::Relative(d)) => audio.nudge_volume(d as i32),
        Some(Delta::Absolute(v)) => audio.set_volume(v.clamp(0, 100) as u8),
        None => {
            app.error(format!("bad volume `{args}`, use `:vol 70` or `:vol +5`"));
            return;
        }
    }
    let v = app.audio.as_ref().map_or(0, super::player::Audio::volume);
    app.info(format!("volume {v}%"));
}

fn seek(app: &mut App, args: &str) {
    match parse_seek(args) {
        Some(Delta::Relative(d)) => app.seek_by_ms(d.saturating_mul(1000)),
        Some(Delta::Absolute(s)) => app.seek_to(Duration::from_secs(s.max(0) as u64)),
        None => app.error(format!(
            "bad position `{args}`, use `:seek 1:30` or `:seek +30`"
        )),
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Delta {
    Absolute(i64),
    Relative(i64),
}

/// `70`, `+5`, `-5`.
pub fn parse_delta(s: &str) -> Option<Delta> {
    let s = s.trim();
    if let Some(rest) = s.strip_prefix('+') {
        return rest.parse().ok().map(Delta::Relative);
    }
    if let Some(rest) = s.strip_prefix('-') {
        return rest.parse::<i64>().ok().map(|n| Delta::Relative(-n));
    }
    s.parse().ok().map(Delta::Absolute)
}

/// Same as [`parse_delta`] but also accepts `m:ss` and plain seconds.
pub fn parse_seek(s: &str) -> Option<Delta> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if let Some(d) = parse_delta(s) {
        return Some(d);
    }
    if let Some((m, sec)) = s.split_once(':') {
        let m: i64 = m.trim().parse().ok()?;
        let sec: i64 = sec.trim().parse().ok()?;
        if !(0..60).contains(&sec) {
            return None;
        }
        return Some(Delta::Absolute(m * 60 + sec));
    }
    s.parse().ok().map(Delta::Absolute)
}

/// Expands a leading `~`, alone or as `~/`. Everything else is left to the
/// filesystem.
pub(crate) fn expand(arg: &str) -> PathBuf {
    if let Some(home) = dirs::home_dir() {
        if arg == "~" {
            return home;
        }
        if let Some(rest) = arg.strip_prefix("~/") {
            return home.join(rest);
        }
    }
    PathBuf::from(arg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bang_is_split_off_the_command_name() {
        let cmd = parse("q!").unwrap();
        assert_eq!(cmd.name, "q");
        assert!(cmd.bang);
        assert_eq!(cmd.args, "");
    }

    #[test]
    fn arguments_keep_the_spaces_inside_them() {
        let cmd = parse("  w  late night mix  ").unwrap();
        assert_eq!(cmd.name, "w");
        assert_eq!(cmd.args, "late night mix");
    }

    #[test]
    fn an_empty_line_is_not_a_command() {
        assert!(parse("   ").is_none());
        assert!(parse("").is_none());
    }

    #[test]
    fn volume_takes_absolute_and_relative_values() {
        assert_eq!(parse_delta("70"), Some(Delta::Absolute(70)));
        assert_eq!(parse_delta("+5"), Some(Delta::Relative(5)));
        assert_eq!(parse_delta("-15"), Some(Delta::Relative(-15)));
        assert_eq!(parse_delta("loud"), None);
    }

    #[test]
    fn seek_accepts_clock_time_as_well_as_seconds() {
        assert_eq!(parse_seek("1:30"), Some(Delta::Absolute(90)));
        assert_eq!(parse_seek("0:07"), Some(Delta::Absolute(7)));
        assert_eq!(parse_seek("90"), Some(Delta::Absolute(90)));
        assert_eq!(parse_seek("+30"), Some(Delta::Relative(30)));
        assert_eq!(parse_seek("1:75"), None, "seconds past 59 are a typo");
        assert_eq!(parse_seek(""), None);
    }

    #[test]
    fn sort_keys_have_the_short_forms_a_vi_user_would_try() {
        assert_eq!(SortKey::parse("t"), Some(SortKey::Title));
        assert_eq!(SortKey::parse("al"), Some(SortKey::Album));
        assert_eq!(SortKey::parse("duration"), Some(SortKey::Duration));
        assert_eq!(SortKey::parse("nope"), None);
    }
}
