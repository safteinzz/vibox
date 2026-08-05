//! Application state. One struct, mutated by key handlers and ex commands,
//! read by the renderer. No state lives anywhere else.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Result;

use crate::library::{self, SortKey, Track};
use crate::lyrics::Fetcher;
use crate::matrix::Matrix;
use crate::mpris::{self, Mpris, Remote};
use crate::player::Audio;

/// Rows kept between the cursor and the edge of the track pane.
pub const SCROLLOFF: usize = 3;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Normal,
    /// The `:` line has focus.
    Command,
    /// The `/` or `?` line has focus.
    Search,
    /// The track list is a buffer of filenames being edited in place.
    Edit,
    /// Typing inside that buffer.
    EditInsert,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Mode::Normal => "NORMAL",
            Mode::Command => "COMMAND",
            Mode::Search => "SEARCH",
            Mode::Edit => "EDIT",
            Mode::EditInsert => "INSERT",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Pane {
    Folders,
    Tracks,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Repeat {
    #[default]
    Off,
    All,
    One,
}

impl Repeat {
    pub fn next(self) -> Repeat {
        match self {
            Repeat::Off => Repeat::All,
            Repeat::All => Repeat::One,
            Repeat::One => Repeat::Off,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Repeat::Off => "off",
            Repeat::All => "all",
            Repeat::One => "one",
        }
    }
}

/// Which tag columns the track list shows, toggled with `:set`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Columns {
    pub file: bool,
    pub title: bool,
    pub artist: bool,
    pub album: bool,
}

impl Default for Columns {
    fn default() -> Self {
        // The title repeats the filename on a tagged library, so it stays off.
        Columns {
            file: true,
            title: false,
            artist: true,
            album: true,
        }
    }
}

impl Columns {
    pub fn get(&self, name: &str) -> Option<bool> {
        match name {
            "file" => Some(self.file),
            "title" => Some(self.title),
            "artist" => Some(self.artist),
            "album" => Some(self.album),
            _ => None,
        }
    }

    /// False when the name is not an option, so the caller can complain.
    pub fn set(&mut self, name: &str, on: bool) -> bool {
        match name {
            "file" => self.file = on,
            "title" => self.title = on,
            "artist" => self.artist = on,
            "album" => self.album = on,
            _ => return false,
        }
        true
    }

    /// Visible columns with the share of the width each one gets.
    pub fn shown(self) -> Vec<(&'static str, usize)> {
        [
            ("file", self.file, 40),
            ("title", self.title, 34),
            ("artist", self.artist, 26),
            ("album", self.album, 26),
        ]
        .into_iter()
        .filter(|(_, on, _)| *on)
        .map(|(name, _, weight)| (name, weight))
        .collect()
    }
}

/// Filename edits held in memory until `:w` writes them.
#[derive(Default)]
pub struct Edit {
    /// Track index to its edited name, without the extension.
    pub pending: std::collections::BTreeMap<usize, String>,
    /// Cursor position inside the name being edited, in characters.
    pub col: usize,
}

pub struct App {
    pub root: PathBuf,
    pub tracks: Vec<Track>,
    pub sort_key: SortKey,
    pub columns: Columns,
    pub show_lyrics: bool,
    pub matrix: Matrix,
    pub lyrics: Fetcher,
    /// Directories that hold tracks. Index 0 of the pane is the whole library,
    /// so a folder `i` in this vec is row `i + 1`.
    pub folders: Vec<(String, PathBuf)>,
    pub folder_cur: usize,
    pub folder_top: usize,
    pub folder_h: usize,

    /// Indices into `tracks`, in display order.
    pub view: Vec<usize>,
    pub cur: usize,
    pub top: usize,
    pub track_h: usize,

    pub focus: Pane,
    pub mode: Mode,
    /// Text of the `:` or `/` line being typed, without its leading char.
    pub line: String,
    pub line_prefix: char,
    /// Cursor position in `line`, counted in characters.
    pub line_cur: usize,
    /// False puts the line in its own normal mode, where hjkl and w b move.
    pub line_insert: bool,
    pub count: Option<usize>,
    /// Half typed multi key sequence: `g`, `z`, `d`, `y` or ctrl-w.
    pub pending: Option<char>,

    pub last_search: String,
    pub search_back: bool,
    /// Filename edits waiting for `:w`, and where the cursor is inside one.
    pub edit: Option<Edit>,

    /// Playback order, snapshotted from the view when playback starts.
    pub queue: Vec<usize>,
    pub qpos: usize,
    pub playing: Option<usize>,
    pub repeat: Repeat,
    pub shuffle: bool,

    pub audio: Option<Audio>,
    /// Absent on a machine with no session bus; media keys just do nothing then.
    pub mpris: Option<Mpris>,
    pub msg: Option<(String, bool)>,
    /// Where the real terminal cursor goes while inserting, set by the renderer.
    pub cursor_screen: Option<(u16, u16)>,
    pub show_help: bool,
    pub help_scroll: usize,
    pub quit: bool,
    rng: u64,
}

impl App {
    pub fn new(root: PathBuf, sort_key: SortKey) -> Result<App> {
        let (audio, audio_err) = match Audio::new() {
            Ok(a) => (Some(a), None),
            Err(e) => (None, Some(format!("{e}"))),
        };

        let mut app = App {
            root,
            tracks: Vec::new(),
            sort_key,
            columns: Columns::default(),
            show_lyrics: false,
            matrix: Matrix::default(),
            lyrics: Fetcher::new(),
            folders: Vec::new(),
            folder_cur: 0,
            folder_top: 0,
            folder_h: 1,
            view: Vec::new(),
            cur: 0,
            top: 0,
            track_h: 1,
            focus: Pane::Tracks,
            mode: Mode::Normal,
            line: String::new(),
            line_prefix: ':',
            line_cur: 0,
            line_insert: true,
            count: None,
            pending: None,
            last_search: String::new(),
            search_back: false,
            edit: None,
            queue: Vec::new(),
            qpos: 0,
            playing: None,
            repeat: Repeat::Off,
            shuffle: false,
            audio,
            mpris: mpris::start().ok(),
            msg: None,
            cursor_screen: None,
            show_help: false,
            help_scroll: 0,
            quit: false,
            rng: seed(),
        };

        app.reload()?;
        if let Some(e) = audio_err {
            app.error(e);
        }
        Ok(app)
    }

    // ---- library ----------------------------------------------------------

    /// Rescans the root from disk, keeping the cursor on the same track if it
    /// survived the rescan.
    pub fn reload(&mut self) -> Result<()> {
        let under_cursor = self.current_track().map(|t| t.path.clone());

        self.tracks = library::scan(&self.root)?;
        // `path` order means "the order the files came in", which for a
        // playlist is the order it was written, so leave that one alone.
        if !(library::is_playlist(&self.root) && self.sort_key == SortKey::Path) {
            library::sort(&mut self.tracks, self.sort_key);
        }
        let base = if self.root.is_dir() {
            self.root.clone()
        } else {
            self.root.parent().unwrap_or(&self.root).to_path_buf()
        };
        self.folders = library::folders(&self.tracks, &base);
        self.folder_cur = self.folder_cur.min(self.folders.len());
        self.playing = None;
        self.queue.clear();
        self.rebuild_view();

        if let Some(path) = under_cursor
            && let Some(pos) = self.view.iter().position(|&i| self.tracks[i].path == path)
        {
            self.cur = pos;
        }
        self.clamp();
        Ok(())
    }

    pub fn open(&mut self, path: &Path) -> Result<()> {
        let root = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.root.join(path)
        };
        let root = root.canonicalize().unwrap_or(root);
        self.root = root;
        self.folder_cur = 0;
        self.cur = 0;
        self.top = 0;
        self.reload()
    }

    pub fn set_sort(&mut self, key: SortKey) {
        self.sort_key = key;
        let under_cursor = self.current_track().map(|t| t.path.clone());
        library::sort(&mut self.tracks, key);
        self.rebuild_view();
        if let Some(path) = under_cursor
            && let Some(pos) = self.view.iter().position(|&i| self.tracks[i].path == path)
        {
            self.cur = pos;
        }
        self.clamp();
    }

    /// Recomputes the visible track list from the selected folder.
    pub fn rebuild_view(&mut self) {
        self.view = match self.folder_cur {
            0 => (0..self.tracks.len()).collect(),
            i => {
                let dir = self.folders[i - 1].1.clone();
                (0..self.tracks.len())
                    .filter(|&t| self.tracks[t].dir() == dir)
                    .collect()
            }
        };
        self.clamp();
    }

    pub fn current_track(&self) -> Option<&Track> {
        self.view.get(self.cur).map(|&i| &self.tracks[i])
    }

    pub fn playing_track(&self) -> Option<&Track> {
        self.playing.map(|i| &self.tracks[i])
    }

    // ---- cursor -----------------------------------------------------------

    pub fn clamp(&mut self) {
        if self.view.is_empty() {
            self.cur = 0;
            self.top = 0;
            return;
        }
        self.cur = self.cur.min(self.view.len() - 1);
        self.scroll_to_cursor();
    }

    pub fn move_cursor(&mut self, delta: isize) {
        if self.view.is_empty() {
            return;
        }
        let last = self.view.len() as isize - 1;
        self.cur = (self.cur as isize + delta).clamp(0, last) as usize;
        self.scroll_to_cursor();
    }

    pub fn goto(&mut self, row: usize) {
        if self.view.is_empty() {
            return;
        }
        self.cur = row.min(self.view.len() - 1);
        self.scroll_to_cursor();
    }

    /// Keeps the viewport around the cursor, honouring [`SCROLLOFF`].
    pub fn scroll_to_cursor(&mut self) {
        let h = self.track_h.max(1);
        let pad = SCROLLOFF.min(h.saturating_sub(1) / 2);
        if self.cur < self.top + pad {
            self.top = self.cur.saturating_sub(pad);
        }
        if self.cur + pad >= self.top + h {
            self.top = self.cur + pad + 1 - h;
        }
        let max_top = self.view.len().saturating_sub(h);
        self.top = self.top.min(max_top);
    }

    /// `zz`, `zt`, `zb`: move the view, not the cursor.
    pub fn scroll_cursor_to(&mut self, where_: char) {
        let h = self.track_h.max(1);
        self.top = match where_ {
            't' => self.cur,
            'b' => self.cur + 1 - h.min(self.cur + 1),
            _ => self.cur.saturating_sub(h / 2),
        };
        let max_top = self.view.len().saturating_sub(h);
        self.top = self.top.min(max_top);
    }

    /// `H`, `M`, `L`: move the cursor inside the visible window.
    pub fn cursor_to_screen(&mut self, where_: char) {
        let h = self.track_h.max(1);
        let last_visible = (self.top + h - 1).min(self.view.len().saturating_sub(1));
        let row = match where_ {
            'H' => (self.top + SCROLLOFF.min(h / 2)).min(last_visible),
            'L' => last_visible.saturating_sub(SCROLLOFF.min(h / 2)).max(self.top),
            _ => (self.top + last_visible) / 2,
        };
        self.cur = row;
        self.scroll_to_cursor();
    }

    pub fn move_folder(&mut self, delta: isize) {
        let last = self.folders.len() as isize; // row 0 is the whole library
        self.folder_cur = (self.folder_cur as isize + delta).clamp(0, last) as usize;
        let h = self.folder_h.max(1);
        if self.folder_cur < self.folder_top {
            self.folder_top = self.folder_cur;
        }
        if self.folder_cur >= self.folder_top + h {
            self.folder_top = self.folder_cur + 1 - h;
        }
        self.cur = 0;
        self.top = 0;
        self.rebuild_view();
    }

    // ---- renaming ---------------------------------------------------------

    /// Opens the list for editing. Nothing touches the disk until `:w`.
    pub fn begin_edit(&mut self) {
        if self.view.is_empty() {
            self.error("nothing here to rename");
            return;
        }
        if !self.columns.file {
            self.error("the file column is hidden: `:set file` to edit names");
            return;
        }
        if self.edit.is_none() {
            self.edit = Some(Edit::default());
        }
        self.mode = Mode::Edit;
    }

    /// The name shown for a row: the edited one if it has been touched.
    pub fn edit_text(&self, row: usize) -> Option<String> {
        let track_idx = *self.view.get(row)?;
        Some(
            self.edit
                .as_ref()
                .and_then(|edit| edit.pending.get(&track_idx))
                .cloned()
                .unwrap_or_else(|| self.tracks[track_idx].file.clone()),
        )
    }

    pub fn set_edit_text(&mut self, text: String) {
        let Some(&track_idx) = self.view.get(self.cur) else {
            return;
        };
        if let Some(edit) = self.edit.as_mut() {
            edit.pending.insert(track_idx, text);
        }
    }

    /// True when a name has been changed but not written.
    pub fn edit_dirty(&self) -> bool {
        self.edit.as_ref().is_some_and(|edit| {
            edit.pending
                .iter()
                .any(|(idx, name)| self.tracks[*idx].file != *name)
        })
    }

    pub fn end_edit(&mut self) {
        self.edit = None;
        if matches!(self.mode, Mode::Edit | Mode::EditInsert) {
            self.mode = Mode::Normal;
        }
    }

    /// Renames every changed file. Names are checked first, so a bad one stops
    /// the whole write instead of leaving the batch half applied.
    pub fn apply_edits(&mut self) {
        let Some(edit) = self.edit.as_ref() else {
            return;
        };

        let mut jobs: Vec<(usize, PathBuf, String)> = Vec::new();
        for (&idx, name) in &edit.pending {
            let name = name.trim();
            let old = self.tracks[idx].path.clone();
            if name == self.tracks[idx].file {
                continue;
            }
            if name.is_empty() {
                self.error("a filename cannot be empty");
                return;
            }
            if name.contains('/') {
                self.error("a filename cannot contain `/`, this renames but never moves");
                return;
            }

            // Built by hand: `set_extension` would eat everything after a dot
            // in a name like `Mr. Blue Sky`.
            let file_name = match old.extension().and_then(|e| e.to_str()) {
                Some(ext) => format!("{name}.{ext}"),
                None => name.to_string(),
            };
            let new = old.with_file_name(&file_name);
            if new.exists() {
                self.error(format!("`{file_name}` already exists here"));
                return;
            }
            jobs.push((idx, new, name.to_string()));
        }

        if jobs.is_empty() {
            self.info("no changes");
            self.end_edit();
            return;
        }

        let mut done = 0;
        for (idx, new, name) in jobs {
            let old = self.tracks[idx].path.clone();
            match std::fs::rename(&old, &new) {
                Ok(()) => {
                    self.tracks[idx].path = new;
                    self.tracks[idx].file = name;
                    done += 1;
                }
                Err(e) => {
                    self.error(format!("cannot rename `{}`: {e}", old.display()));
                    self.edit = None;
                    return;
                }
            }
        }

        self.end_edit();
        self.info(format!("renamed {done} file{}", if done == 1 { "" } else { "s" }));
    }

    // ---- search -----------------------------------------------------------

    /// Vim smartcase: a lowercase pattern matches anything, an uppercase one
    /// is taken literally.
    pub fn search(&mut self, pattern: &str, backward: bool, from: usize) -> bool {
        if pattern.is_empty() || self.view.is_empty() {
            return false;
        }
        let smart = pattern.chars().any(char::is_uppercase);
        let needle = if smart {
            pattern.to_string()
        } else {
            pattern.to_lowercase()
        };
        let n = self.view.len();

        for step in 1..=n {
            let row = if backward {
                (from + n - (step % n)) % n
            } else {
                (from + step) % n
            };
            let hay = self.tracks[self.view[row]].haystack();
            let hay = if smart { hay } else { hay.to_lowercase() };
            if hay.contains(&needle) {
                self.goto(row);
                return true;
            }
        }
        false
    }

    // ---- playback ---------------------------------------------------------

    /// Starts playback at the cursor, snapshotting the view as the queue.
    pub fn play_cursor(&mut self) {
        if self.view.is_empty() {
            self.error("nothing to play here");
            return;
        }
        self.queue = self.view.clone();
        self.qpos = self.cur;
        self.play_queue_pos();
    }

    fn play_queue_pos(&mut self) {
        let Some(&track_idx) = self.queue.get(self.qpos) else {
            return;
        };
        let path = self.tracks[track_idx].path.clone();
        let Some(audio) = self.audio.as_mut() else {
            self.error("no audio device: playback is disabled");
            return;
        };
        match audio.play(&path) {
            Ok(()) => {
                self.playing = Some(track_idx);
                self.msg = None;
            }
            Err(e) => {
                self.playing = None;
                self.error(format!("{e}"));
            }
        }
    }

    pub fn stop(&mut self) {
        if let Some(audio) = self.audio.as_mut() {
            audio.stop();
        }
        self.playing = None;
    }

    /// Moves through the queue. `auto` is a track ending on its own, which is
    /// the only case where `repeat one` replays instead of advancing.
    pub fn advance(&mut self, delta: isize, auto: bool) {
        if self.queue.is_empty() {
            return;
        }
        if auto && self.repeat == Repeat::One {
            self.play_queue_pos();
            return;
        }
        if self.shuffle && delta > 0 {
            self.qpos = self.next_random();
            self.play_queue_pos();
            self.follow_playing();
            return;
        }

        let next = self.qpos as isize + delta;
        if next < 0 {
            self.qpos = 0;
        } else if next as usize >= self.queue.len() {
            if self.repeat == Repeat::All {
                self.qpos = 0;
            } else {
                self.stop();
                return;
            }
        } else {
            self.qpos = next as usize;
        }
        self.play_queue_pos();
        self.follow_playing();
    }

    /// Keeps the cursor on the track that is playing while it moves by itself.
    fn follow_playing(&mut self) {
        if let Some(p) = self.playing
            && let Some(row) = self.view.iter().position(|&i| i == p)
        {
            self.goto(row);
        }
    }

    /// Called every event loop turn: detects a track that ran out, and picks
    /// up anything the audio thread wants to say.
    pub fn tick(&mut self) {
        if let Some(e) = self.audio.as_ref().and_then(Audio::take_error) {
            self.error(e);
        }

        let finished = self
            .audio
            .as_ref()
            .is_some_and(|a| a.has_track() && a.finished() && !a.is_paused());
        if finished && self.playing.is_some() {
            self.advance(1, true);
        }

        self.remote_control();
        self.publish_status();
        self.lyrics_tick();
    }

    /// Keeps the lyrics pane fed: results in, a request out for a new track.
    fn lyrics_tick(&mut self) {
        self.lyrics.poll();
        if !self.show_lyrics {
            return;
        }
        if let Some(track) = self.playing_track().cloned() {
            self.lyrics.request(&track);
        }
    }

    /// Media keys, `playerctl`, and whatever else is on the session bus.
    fn remote_control(&mut self) {
        let Some(commands) = self
            .mpris
            .as_ref()
            .map(|m| m.rx.try_iter().collect::<Vec<_>>())
        else {
            return;
        };

        for command in commands {
            match command {
                Remote::PlayPause => match self.audio.as_ref() {
                    Some(audio) if audio.has_track() => audio.toggle_pause(),
                    _ => self.play_cursor(),
                },
                Remote::Play => match self.audio.as_ref() {
                    Some(audio) if audio.has_track() => audio.resume(),
                    _ => self.play_cursor(),
                },
                Remote::Pause => {
                    if let Some(audio) = self.audio.as_ref() {
                        audio.pause();
                    }
                }
                Remote::Stop => self.stop(),
                Remote::Next => self.advance(1, false),
                Remote::Prev => self.advance(-1, false),
                Remote::Seek(offset_us) => {
                    if let Some(audio) = self.audio.as_ref()
                        && let Err(e) = audio.seek_by(offset_us / 1_000_000)
                    {
                        self.error(format!("{e}"));
                    }
                }
                Remote::Volume(v) => {
                    if let Some(audio) = self.audio.as_mut() {
                        audio.set_volume((v * 100.0).round() as u8);
                    }
                }
                Remote::Quit => self.quit = true,
            }
        }
    }

    /// Hands the desktop something to put in its media widget.
    fn publish_status(&self) {
        let Some(mpris) = self.mpris.as_ref() else {
            return;
        };
        let audio = self.audio.as_ref();
        let track = self.playing_track();

        mpris.publish(&mpris::Status {
            has_track: audio.is_some_and(Audio::has_track),
            paused: audio.is_some_and(Audio::is_paused),
            title: track.map(|t| t.title.clone()).unwrap_or_default(),
            artist: track.map(|t| t.artist.clone()).unwrap_or_default(),
            album: track.map(|t| t.album.clone()).unwrap_or_default(),
            path: track.map(|t| t.path.clone()).unwrap_or_default(),
            length_us: track.map_or(0, |t| t.duration.as_micros() as u64),
            pos_us: self.elapsed().as_micros() as u64,
            volume: audio.map_or(0.0, |a| f64::from(a.volume()) / 100.0),
            can_next: !self.queue.is_empty(),
            can_prev: !self.queue.is_empty(),
        });
    }

    pub fn elapsed(&self) -> Duration {
        self.audio.as_ref().map_or(Duration::ZERO, Audio::pos)
    }

    fn next_random(&mut self) -> usize {
        // xorshift64: a shuffle order does not need a crate.
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        (self.rng % self.queue.len() as u64) as usize
    }

    // ---- messages ---------------------------------------------------------

    pub fn info(&mut self, text: impl Into<String>) {
        self.msg = Some((text.into(), false));
    }

    pub fn error(&mut self, text: impl Into<String>) {
        self.msg = Some((text.into(), true));
    }
}

fn seed() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0x2545_F491_4F6C_DD1D, |d| d.as_nanos() as u64)
        | 1
}

