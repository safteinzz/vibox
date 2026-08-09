//! Application state. One struct, mutated by key handlers and ex commands,
//! read by the renderer. No state lives anywhere else.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Result;

use crate::library::{self, SortKey, Track};
use crate::lyrics::Fetcher;
use crate::matrix::Matrix;
use crate::name::NameBuffer;
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
    /// Linewise visual, the only visual there is when rows are tracks.
    Visual,
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
            Mode::Visual => "VISUAL",
            Mode::Edit => "EDIT",
            Mode::EditInsert => "INSERT",
        }
    }
}

/// What the left pane is showing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tab {
    Folders,
    Playlists,
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

/// Everything waiting for a `:w`, kept in one place so undo is a snapshot of
/// it and `:w` is the only thing that touches the disk.
#[derive(Clone, Default)]
pub struct Pending {
    pub renames: std::collections::BTreeMap<Renaming, String>,
    pub playlist_rows: Vec<usize>,
    pub playlist_dirty: bool,
    /// Playlist files marked with `dd`.
    pub doomed: Vec<PathBuf>,
    /// Tracks marked for deletion, danger mode only.
    pub doomed_files: Vec<PathBuf>,
    /// Files cut with `dd`, waiting for a `p` to turn them into moves.
    pub cut: Vec<PathBuf>,
    pub moves: Vec<(PathBuf, PathBuf)>,
    pub copies: Vec<(PathBuf, PathBuf)>,
    /// Folders marked with `dd`, taken with everything inside them.
    pub doomed_dirs: Vec<PathBuf>,
    /// The name being typed in the sidebar, so `u` works there as well.
    pub sidebar_name: Option<String>,
}

/// One open view: what it shows, and where you left it. Snapshotted on the way
/// out of a tab and restored on the way back, the way vim keeps a tab page.
#[derive(Clone)]
pub struct ViewTab {
    pub playlist: Option<String>,
    pub folder: usize,
    pub cur: usize,
    pub top: usize,
    pub sort_key: SortKey,
    rows: Vec<usize>,
    dirty: bool,
}

/// What a rename is renaming. Tracks come from the list, folders and playlists
/// from the sidebar, and all three queue up in the same pending set.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Renaming {
    Track(usize),
    Folder(PathBuf),
    Playlist(String),
}

/// The name being typed right now: one buffer, and what it will rename.
///
/// Only one thing is ever being typed at a time, in whichever pane `c` opened.
/// Leaving it puts the result in `renames` with everything else waiting for
/// `:w`, so a rename is not a special case that needs writing on its own.
pub struct NameEdit {
    pub buf: NameBuffer,
    pub what: Renaming,
}

pub struct App {
    pub root: PathBuf,
    pub tracks: Vec<Track>,
    pub sort_key: SortKey,
    pub columns: Columns,
    /// The library opened when vibox is started with no path, from `:set music=`.
    pub music: Option<PathBuf>,
    pub show_lyrics: bool,
    pub matrix: Matrix,
    pub lyrics: Fetcher,
    /// Directories that hold tracks. Index 0 of the pane is the whole library,
    /// so a folder `i` in this vec is row `i + 1`.
    pub folders: Vec<(String, PathBuf)>,
    pub folder_cur: usize,
    /// The folder actually shown, as opposed to the one the cursor sits on.
    pub folder_open: usize,
    pub folder_top: usize,
    pub folder_h: usize,
    /// Which tab the left pane shows, and the m3u files it lists.
    pub tab: Tab,
    pub playlists: Vec<(String, PathBuf)>,
    pub pl_cur: usize,
    pub pl_top: usize,
    /// The playlist currently shown, if any. It is a view over the library, so
    /// the folders tab keeps browsing the whole library while it is open.
    pub playlist_view: Option<String>,
    playlist_rows: Vec<usize>,
    /// Open views, and which one you are looking at.
    pub tabs: Vec<ViewTab>,
    pub tab_idx: usize,

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
    pub visual_anchor: Option<usize>,
    /// Tracks picked up by `y`, waiting for a `p` into a playlist.
    pub yank: Vec<PathBuf>,
    /// Playlist rows changed since the last `:w`.
    pub playlist_dirty: bool,
    /// Playlists marked for deletion, written by `:w` like every other change.
    pub doomed: Vec<PathBuf>,
    /// `:set danger`: lets `dd`, `p` and `o` touch files, still only on `:w`.
    pub danger: bool,
    pub doomed_files: Vec<PathBuf>,
    pub cut: Vec<PathBuf>,
    pub moves: Vec<(PathBuf, PathBuf)>,
    /// Yanked files waiting to be copied into a folder, danger mode only.
    pub copies: Vec<(PathBuf, PathBuf)>,
    pub doomed_dirs: Vec<PathBuf>,
    undo_stack: Vec<Pending>,
    redo_stack: Vec<Pending>,
    /// Filename edits waiting for `:w`, and where the cursor is inside one.
    pub edit: Option<NameEdit>,
    /// Renames waiting for `:w`: tracks, folders and playlists together.
    pub renames: std::collections::BTreeMap<Renaming, String>,
    /// Set only while the sidebar is renaming something.
    /// Overridden only by tests; `None` means the user's data directory.
    pub playlists_dir: Option<PathBuf>,

    /// Playback order, snapshotted from the view when playback starts.
    pub queue: Vec<usize>,
    pub qpos: usize,
    /// Queue positions already played, so shuffle can go back the way it came.
    history: Vec<usize>,
    pub playing: Option<usize>,
    pub repeat: Repeat,
    pub shuffle: bool,

    pub audio: Option<Audio>,
    /// Absent on a machine with no session bus; media keys just do nothing then.
    pub mpris: Option<Mpris>,
    pub msg: Option<(String, bool)>,
    /// Where the real terminal cursor goes while inserting, set by the renderer.
    pub cursor_screen: Option<(u16, u16)>,
    pub show_info: bool,
    /// How far `:changes` is panned sideways, in cells.
    pub changes_pan: usize,
    pub show_changes: bool,
    pub show_help: bool,
    pub help_scroll: usize,
    pub quit: bool,
    rng: u64,
}

impl App {
    pub fn new(root: PathBuf, sort_key: SortKey, on: library::Report) -> Result<App> {
        let (audio, audio_err) = match Audio::new() {
            Ok(a) => (Some(a), None),
            Err(e) => (None, Some(format!("{e}"))),
        };

        let mut app = App {
            root,
            tracks: Vec::new(),
            sort_key,
            columns: Columns::default(),
            music: None,
            show_lyrics: false,
            matrix: Matrix::default(),
            lyrics: Fetcher::new(),
            folders: Vec::new(),
            folder_cur: 0,
            folder_open: 0,
            folder_top: 0,
            folder_h: 1,
            tab: Tab::Folders,
            playlists: Vec::new(),
            pl_cur: 0,
            pl_top: 0,
            playlist_view: None,
            playlist_rows: Vec::new(),
            tabs: Vec::new(),
            tab_idx: 0,
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
            visual_anchor: None,
            yank: Vec::new(),
            playlist_dirty: false,
            doomed: Vec::new(),
            danger: false,
            doomed_files: Vec::new(),
            cut: Vec::new(),
            moves: Vec::new(),
            copies: Vec::new(),
            doomed_dirs: Vec::new(),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            edit: None,
            renames: std::collections::BTreeMap::new(),
            playlists_dir: None,
            queue: Vec::new(),
            qpos: 0,
            history: Vec::new(),
            playing: None,
            repeat: Repeat::Off,
            shuffle: false,
            audio,
            mpris: mpris::start().ok(),
            msg: None,
            cursor_screen: None,
            show_info: false,
            changes_pan: 0,
            show_changes: false,
            show_help: false,
            help_scroll: 0,
            quit: false,
            rng: seed(),
        };

        app.reload_playlists();
        app.reload_reporting(on)?;
        app.tabs.push(app.snapshot());
        if let Some(e) = audio_err {
            app.error(e);
        }
        Ok(app)
    }

    // ---- library ----------------------------------------------------------

    /// Rescans the root from disk, keeping the cursor on the same track if it
    /// survived the rescan.
    pub fn reload(&mut self) -> Result<()> {
        self.reload_reporting(library::QUIET)
    }

    /// `reload`, saying how far along the scan is. Only the first one, before
    /// the terminal is taken over, has anywhere to say it.
    pub fn reload_reporting(&mut self, on: library::Report) -> Result<()> {
        let under_cursor = self.current_track().map(|t| t.path.clone());

        self.tracks = library::scan_reporting(&self.root, on)?;
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
        self.folder_open = 0;
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

    /// True for tracks that belong to the library itself.
    ///
    /// A playlist may name files anywhere on disk, and those are read in so it
    /// can play them, but they are not part of the library: `everything` and
    /// the folder list must not grow a stray directory because a playlist
    /// mentioned one.
    fn in_library(&self, path: &Path) -> bool {
        // With an m3u opened as the root, its tracks are the library.
        !self.root.is_dir() || path.starts_with(&self.root)
    }

    /// Recomputes the visible track list from the playlist or the folder.
    pub fn rebuild_view(&mut self) {
        if self.playlist_view.is_some() {
            self.view = self.playlist_rows.clone();
            self.clamp();
            return;
        }
        // A marked deletion is gone from the list straight away: this is a
        // buffer, so it should look like the edit already happened.
        self.view = match self.folder_open {
            0 => (0..self.tracks.len())
                .filter(|&t| self.in_library(&self.tracks[t].path))
                .collect(),
            i => {
                let dir = self.folders[i - 1].1.clone();
                (0..self.tracks.len())
                    .filter(|&t| self.tracks[t].dir() == dir)
                    .collect()
            }
        };
        if !self.doomed_files.is_empty() {
            self.view
                .retain(|&t| !self.doomed_files.contains(&self.tracks[t].path));
        }
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

    /// Moves the cursor in the folder list. The track pane does not follow: a
    /// folder is opened with enter, the same as a playlist.
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
    }

    /// Enter on a folder row: this is what actually changes the track pane.
    pub fn open_folder(&mut self) {
        if self.jump_to_open(None, self.folder_cur) {
            return;
        }
        self.playlist_view = None;
        self.folder_open = self.folder_cur;
        self.cur = 0;
        self.top = 0;
        self.rebuild_view();
        self.focus = Pane::Tracks;
    }

    // ---- undo -------------------------------------------------------------

    fn pending(&self) -> Pending {
        Pending {
            renames: self.renames.clone(),
            playlist_rows: self.playlist_rows.clone(),
            playlist_dirty: self.playlist_dirty,
            doomed: self.doomed.clone(),
            doomed_files: self.doomed_files.clone(),
            cut: self.cut.clone(),
            moves: self.moves.clone(),
            copies: self.copies.clone(),
            doomed_dirs: self.doomed_dirs.clone(),
            sidebar_name: self.edit.as_ref().map(|edit| edit.buf.text()),
        }
    }

    fn apply_pending(&mut self, p: Pending) {
        self.renames = p.renames;
        self.playlist_rows = p.playlist_rows;
        self.playlist_dirty = p.playlist_dirty;
        self.doomed = p.doomed;
        self.doomed_files = p.doomed_files;
        self.cut = p.cut;
        // Paths live on the tracks, so stepping between snapshots means
        // undoing the moves this one does not have and applying the ones it does.
        for (from, to) in &self.moves {
            if !p.moves.contains(&(from.clone(), to.clone()))
                && let Some(track) = self.tracks.iter_mut().find(|t| t.path == *to)
            {
                track.path = from.clone();
            }
        }
        for (from, to) in &p.moves {
            if !self.moves.contains(&(from.clone(), to.clone()))
                && let Some(track) = self.tracks.iter_mut().find(|t| t.path == *from)
            {
                track.path = to.clone();
            }
        }
        // A pending copy already shows in the list, so stepping between
        // snapshots adds and removes those rows too.
        let dropped: Vec<PathBuf> = self
            .copies
            .iter()
            .filter(|pair| !p.copies.contains(pair))
            .map(|(_, to)| to.clone())
            .collect();
        let added: Vec<(PathBuf, PathBuf)> = p
            .copies
            .iter()
            .filter(|pair| !self.copies.contains(pair))
            .cloned()
            .collect();
        self.copies = p.copies;
        if !dropped.is_empty() {
            self.forget_tracks(&dropped);
        }
        for (from, to) in added {
            if let Some(source) = self.tracks.iter().find(|t| t.path == from) {
                let mut copy = source.clone();
                copy.path = to;
                copy.file = copy
                    .path
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                self.tracks.push(copy);
            }
        }

        self.moves = p.moves;
        self.doomed_dirs = p.doomed_dirs;
        if let (Some(edit), Some(name)) = (self.edit.as_mut(), p.sidebar_name) {
            let col = edit.buf.col().min(name.chars().count());
            edit.buf = NameBuffer::new(&name);
            for _ in 0..col {
                edit.buf.right();
            }
        }
        self.rebuild_view();
    }

    /// Called before anything that changes what `:w` would do.
    pub fn checkpoint(&mut self) {
        self.undo_stack.push(self.pending());
        self.redo_stack.clear();
    }

    pub fn undo(&mut self) {
        let Some(previous) = self.undo_stack.pop() else {
            self.info("already at the oldest change");
            return;
        };
        self.redo_stack.push(self.pending());
        self.apply_pending(previous);
        self.info("undo");
    }

    pub fn redo(&mut self) {
        let Some(next) = self.redo_stack.pop() else {
            self.info("already at the newest change");
            return;
        };
        self.undo_stack.push(self.pending());
        self.apply_pending(next);
        self.info("redo");
    }

    // ---- danger mode ------------------------------------------------------

    pub fn is_doomed_file(&self, path: &Path) -> bool {
        self.doomed_files.iter().any(|p| p == path)
    }

    pub fn is_cut(&self, path: &Path) -> bool {
        self.cut.iter().any(|p| p == path)
    }

    /// `dd` on a track with danger on: marks the files, and remembers them so
    /// a `p` somewhere else turns the deletion into a move, exactly like vim
    /// deleting text and putting it back.
    pub fn cut_tracks(&mut self) {
        if !self.danger {
            self.error("`dd` here needs `:set danger`; it deletes files");
            return;
        }
        let (a, b) = self.selection_range();
        let b = b.min(self.view.len().saturating_sub(1));
        let paths: Vec<PathBuf> = self.view[a..=b]
            .iter()
            .map(|&i| self.tracks[i].path.clone())
            .collect();
        if paths.is_empty() {
            return;
        }

        self.checkpoint();
        let n = paths.len();
        self.cut.clone_from(&paths);
        self.doomed_files.extend(paths);
        self.exit_visual();
        self.cur = self.cur.min(self.view.len().saturating_sub(1));
        self.rebuild_view();
        self.info(format!(
            "{n} file{} marked for deletion, `p` elsewhere moves them instead, `u` undoes",
            plural(n)
        ));
    }

    /// `p` in a folder with files cut: moves them into the folder in view.
    pub fn move_cut_here(&mut self) -> bool {
        if self.cut.is_empty() {
            return false;
        }
        let Some(dir) = self.current_dir() else {
            self.error("no folder here to move into");
            return true;
        };

        self.checkpoint();
        let cut = std::mem::take(&mut self.cut);
        let n = cut.len();
        for from in cut {
            self.doomed_files.retain(|p| *p != from);
            let Some(name) = from.file_name() else { continue };
            let to = dir.join(name);
            if let Some(track) = self.tracks.iter_mut().find(|t| t.path == from) {
                track.path = to.clone();
            }
            self.moves.push((from, to));
        }
        self.rebuild_view();
        let shown = dir.display().to_string();
        self.info(format!("{n} file{} will move to {shown}, `:w` does it", plural(n)));
        true
    }

    /// `dd` on a folder row with danger on: marks the folder and everything
    /// under it, subfolders included.
    ///
    /// The tracks are marked one by one rather than hidden behind the folder,
    /// so `:changes` still lists every file that is about to go.
    pub fn cut_folder(&mut self) {
        if !self.danger {
            self.error("`dd` on a folder needs `:set danger`; it deletes the folder and its tracks");
            return;
        }
        let Some((label, dir)) = self.folders.get(self.folder_cur.wrapping_sub(1)).cloned() else {
            self.error("that is the whole library, not a folder");
            return;
        };

        if let Some(at) = self.doomed_dirs.iter().position(|p| *p == dir) {
            self.checkpoint();
            self.doomed_dirs.remove(at);
            self.doomed_files.retain(|f| !f.starts_with(&dir));
            self.rebuild_view();
            self.info(format!("`{label}` kept"));
            return;
        }

        self.checkpoint();
        let inside: Vec<PathBuf> = self
            .tracks
            .iter()
            .filter(|t| t.path.starts_with(&dir))
            .map(|t| t.path.clone())
            .collect();
        let n = inside.len();
        self.doomed_files.extend(inside);
        self.doomed_dirs.push(dir);
        self.rebuild_view();
        self.info(format!(
            "`{label}` and {n} track{} marked, `:w` deletes them, `dd` undoes it",
            plural(n)
        ));
    }

    pub fn is_doomed_dir(&self, path: &Path) -> bool {
        self.doomed_dirs.iter().any(|p| path.starts_with(p))
    }

    /// `p` in a folder with tracks yanked: copies them in, danger mode only.
    ///
    /// The move case is `dd` then `p`; this is the `y` then `p` case, and it is
    /// a copy for the same reason it is in vim, where the yanked text stays
    /// where it was.
    pub fn copy_yank_here(&mut self) -> bool {
        if self.yank.is_empty() || self.playlist_view.is_some() {
            return false;
        }
        if !self.danger {
            self.error("copying files here needs `:set danger`");
            return true;
        }
        let Some(dir) = self.current_dir() else {
            self.error("no folder here to copy into");
            return true;
        };

        self.checkpoint();
        let mut n = 0;
        for from in self.yank.clone() {
            let Some(name) = from.file_name() else { continue };
            let to = dir.join(name);
            if to == from {
                continue;
            }
            if let Some(source) = self.tracks.iter().find(|t| t.path == from) {
                let mut copy = source.clone();
                copy.path = to.clone();
                self.tracks.push(copy);
            }
            self.copies.push((from, to));
            n += 1;
        }

        self.rebuild_view();
        let shown = self.under_root(&dir);
        self.info(format!(
            "{n} file{} will be copied to {shown}, `:w` does it",
            plural(n)
        ));
        true
    }

    /// The folder the view is looking at, which is where a `p` puts files.
    fn current_dir(&self) -> Option<PathBuf> {
        if self.folder_cur > 0 {
            return self.folders.get(self.folder_cur - 1).map(|(_, p)| p.clone());
        }
        self.current_track().map(|t| t.dir().to_path_buf())
    }

    /// `:mkdir`: a new folder, made right away.
    ///
    /// Typing a command with a name in it is the confirmation, the way `:w
    /// file` writes in vim. Only the buffer edits wait for `:w`.
    pub fn make_dir(&mut self, name: &str) {
        if !self.danger {
            self.error("`:mkdir` needs `:set danger`; it writes to your library");
            return;
        }
        let name = name.trim();
        if name.is_empty() {
            self.error("`:mkdir` needs a name");
            return;
        }

        let path = self.root.join(name);
        if path.exists() {
            self.error(format!("`{name}` already exists"));
            return;
        }
        match std::fs::create_dir_all(&path) {
            Ok(()) => {
                self.folders = library::folders(&self.tracks, &self.root);
                let shown = self.under_root(&path);
                self.info(format!("{shown} created"));
            }
            Err(e) => self.error(format!("cannot create `{name}`: {e}")),
        }
    }

    /// Checks every pending move before any of them runs.
    ///
    /// A batch is all or nothing: moving three files into a folder where one of
    /// them already exists must not move the other two and leave you to work
    /// out which. Same rule as the rename batch.
    fn move_problem(&self) -> Option<String> {
        let mut targets: Vec<&PathBuf> = Vec::new();
        for (_, to) in self.moves.iter().chain(self.copies.iter()) {
            if to.exists() {
                return Some(format!(
                    "`{}` already exists, nothing written",
                    self.under_root(to)
                ));
            }
            if targets.contains(&to) {
                return Some(format!(
                    "two files would both become `{}`, nothing written",
                    self.under_root(to)
                ));
            }
            targets.push(to);
        }
        None
    }

    /// The file operations, run by `:w` after the renames and playlists.
    ///
    /// Playlists are repaired afterwards for the same reason a rename repairs
    /// them: a moved file must keep playing from its playlists, and a deleted
    /// one must not sit there as an entry that points nowhere.
    fn apply_file_ops(&mut self) -> (usize, usize, usize) {
        let (mut moved, mut copied, mut gone) = (0, 0, 0);
        let mut done: Vec<(PathBuf, PathBuf)> = Vec::new();
        let mut removed: Vec<PathBuf> = Vec::new();

        for (from, to) in std::mem::take(&mut self.moves) {
            match std::fs::rename(&from, &to) {
                Ok(()) => {
                    done.push((from, to));
                    moved += 1;
                }
                Err(e) => self.error(format!("cannot move `{}`: {e}", from.display())),
            }
        }
        for (from, to) in std::mem::take(&mut self.copies) {
            if let Err(e) = std::fs::copy(&from, &to) {
                self.error(format!("cannot copy `{}`: {e}", self.under_root(&from)));
            } else {
                copied += 1;
            }
        }
        for path in std::mem::take(&mut self.doomed_files) {
            match std::fs::remove_file(&path) {
                Ok(()) => {
                    removed.push(path);
                    gone += 1;
                }
                Err(e) => self.error(format!("cannot delete `{}`: {e}", path.display())),
            }
        }

        // Deepest first, so a folder inside a marked folder is already gone.
        let mut dirs = std::mem::take(&mut self.doomed_dirs);
        dirs.sort_by_key(|d| std::cmp::Reverse(d.components().count()));
        for dir in dirs {
            match std::fs::remove_dir_all(&dir) {
                Ok(()) => gone += 1,
                Err(e) => self.error(format!("cannot delete `{}`: {e}", self.under_root(&dir))),
            }
        }
        if !done.is_empty() {
            self.repair_playlists(&done);
        }
        if !removed.is_empty() {
            self.drop_from_playlists(&removed);
            self.forget_tracks(&removed);
        }

        self.cut.clear();
        self.folders = library::folders(&self.tracks, &self.root);
        self.folder_cur = self.folder_cur.min(self.folders.len());
        self.folder_open = self.folder_open.min(self.folders.len());
        self.rebuild_view();
        (moved, copied, gone)
    }

    /// Drops deleted tracks out of the library and repoints everything that
    /// held an index into it.
    ///
    /// `view`, `queue`, the open playlist and every other tab all store indices
    /// into `tracks`, so removing an entry without remapping them would leave
    /// each of those pointing at whatever slid into the gap.
    fn forget_tracks(&mut self, gone: &[PathBuf]) {
        let mut remap: Vec<Option<usize>> = Vec::with_capacity(self.tracks.len());
        let mut next = 0;
        for track in &self.tracks {
            if gone.contains(&track.path) {
                remap.push(None);
            } else {
                remap.push(Some(next));
                next += 1;
            }
        }

        self.tracks.retain(|t| !gone.contains(&t.path));
        let follow = |rows: &[usize]| -> Vec<usize> {
            rows.iter().filter_map(|&i| remap[i]).collect()
        };

        self.playlist_rows = follow(&self.playlist_rows);
        self.queue = follow(&self.queue);
        self.qpos = self.qpos.min(self.queue.len().saturating_sub(1));
        self.playing = self.playing.and_then(|i| remap[i]);
        for tab in &mut self.tabs {
            tab.rows = tab.rows.iter().filter_map(|&i| remap[i]).collect();
        }
        self.renames = std::mem::take(&mut self.renames)
            .into_iter()
            .filter_map(|(what, name)| match what {
                Renaming::Track(i) => remap[i].map(|new| (Renaming::Track(new), name)),
                other => Some((other, name)),
            })
            .collect();
    }

    /// Takes deleted files out of every playlist that named them.
    fn drop_from_playlists(&mut self, gone: &[PathBuf]) -> usize {
        let mut fixed = 0;
        for (_, path) in self.playlists.clone() {
            let Ok(paths) = library::read_m3u(&path) else {
                continue;
            };
            if !paths.iter().any(|p| gone.contains(p)) {
                continue;
            }

            let tracks: Vec<&Track> = paths
                .iter()
                .filter(|p| !gone.contains(p))
                .filter_map(|p| self.tracks.iter().find(|t| t.path == *p))
                .collect();
            if library::write_m3u(&path, &tracks).is_ok() {
                fixed += 1;
            }
        }
        fixed
    }

    // ---- tabs -------------------------------------------------------------

    fn snapshot(&self) -> ViewTab {
        ViewTab {
            playlist: self.playlist_view.clone(),
            folder: self.folder_open,
            cur: self.cur,
            top: self.top,
            sort_key: self.sort_key,
            rows: self.playlist_rows.clone(),
            dirty: self.playlist_dirty,
        }
    }

    fn restore(&mut self, tab: &ViewTab) {
        self.playlist_view = tab.playlist.clone();
        self.playlist_rows = tab.rows.clone();
        self.playlist_dirty = tab.dirty;
        self.folder_open = tab.folder;
        self.folder_cur = tab.folder;
        if self.sort_key != tab.sort_key {
            self.set_sort(tab.sort_key);
        }
        self.rebuild_view();
        self.cur = tab.cur.min(self.view.len().saturating_sub(1));
        self.top = tab.top;
        self.clamp();
    }

    /// Name for each tab, and whether it has unwritten changes.
    pub fn tab_labels(&self) -> Vec<(String, bool)> {
        self.tabs
            .iter()
            .enumerate()
            .map(|(i, tab)| {
                let tab = if i == self.tab_idx {
                    &self.snapshot()
                } else {
                    tab
                };
                let name = match (&tab.playlist, tab.folder) {
                    (Some(name), _) => name.clone(),
                    (None, 0) => "everything".to_string(),
                    (None, folder) => self
                        .folders
                        .get(folder - 1)
                        .map_or_else(|| "everything".to_string(), |(label, _)| label.clone()),
                };
                (name, tab.dirty)
            })
            .collect()
    }

    pub fn cycle_tab(&mut self, delta: isize) {
        if self.tabs.len() < 2 {
            self.info("only one tab: `t` on a folder or playlist opens another");
            return;
        }
        self.tabs[self.tab_idx] = self.snapshot();
        let len = self.tabs.len() as isize;
        self.tab_idx = ((self.tab_idx as isize + delta).rem_euclid(len)) as usize;
        let tab = self.tabs[self.tab_idx].clone();
        self.restore(&tab);
    }

    /// Switches to the tab already showing this view, if one is.
    ///
    /// A view is only ever open once: two tabs of the same playlist would drift
    /// apart and the later `:w` would quietly win.
    fn jump_to_open(&mut self, playlist: Option<&str>, folder: usize) -> bool {
        self.tabs[self.tab_idx] = self.snapshot();
        let found = self.tabs.iter().position(|tab| match (playlist, &tab.playlist) {
            (Some(name), Some(open)) => open == name,
            (None, None) => tab.folder == folder,
            _ => false,
        });

        let Some(i) = found else { return false };
        if i != self.tab_idx {
            self.tab_idx = i;
            let tab = self.tabs[i].clone();
            self.restore(&tab);
        }
        self.focus = Pane::Tracks;
        true
    }

    /// `t`: opens whatever the sidebar cursor is on in a tab of its own, or
    /// takes you to the tab it is already in.
    pub fn open_in_new_tab(&mut self) {
        let already = match self.tab {
            Tab::Playlists => {
                let name = self.playlists.get(self.pl_cur).map(|(name, _)| name.clone());
                name.is_some_and(|name| self.jump_to_open(Some(&name), 0))
            }
            Tab::Folders => self.jump_to_open(None, self.folder_cur),
        };
        if already {
            self.info("already open");
            return;
        }

        let fresh = self.snapshot();
        self.tabs.insert(self.tab_idx + 1, fresh);
        self.tab_idx += 1;

        match self.tab {
            Tab::Playlists => self.open_playlist(),
            Tab::Folders => self.open_folder(),
        }
    }

    /// `:q` closes the tab. The last one closes vibox.
    pub fn close_tab(&mut self) {
        if self.tabs.len() < 2 {
            self.quit = true;
            return;
        }
        self.tabs.remove(self.tab_idx);
        self.tab_idx = self.tab_idx.min(self.tabs.len() - 1);
        let tab = self.tabs[self.tab_idx].clone();
        self.restore(&tab);
    }

    // ---- selecting --------------------------------------------------------

    /// Inclusive row range under the cursor, or the whole visual selection.
    pub fn selection_range(&self) -> (usize, usize) {
        match self.visual_anchor {
            Some(anchor) if self.mode == Mode::Visual => {
                (anchor.min(self.cur), anchor.max(self.cur))
            }
            _ => (self.cur, self.cur),
        }
    }

    pub fn exit_visual(&mut self) {
        self.visual_anchor = None;
        if self.mode == Mode::Visual {
            self.mode = Mode::Normal;
        }
    }

    /// `y`: remembers the selected tracks so `p` can put them in a playlist.
    pub fn yank_selection(&mut self) {
        if self.view.is_empty() {
            self.error("nothing here to yank");
            return;
        }
        let (a, b) = self.selection_range();
        let b = b.min(self.view.len() - 1);
        self.yank = self.view[a..=b]
            .iter()
            .map(|&i| self.tracks[i].path.clone())
            .collect();
        let n = self.yank.len();
        self.exit_visual();
        self.info(format!("yanked {n} track{}", plural(n)));
    }

    // ---- playlists --------------------------------------------------------

    /// `p`: puts the yanked tracks into the open playlist, after the cursor.
    ///
    /// Only into a playlist: vibox does not add files to a folder.
    pub fn paste_into_playlist(&mut self) {
        if self.playlist_view.is_none() {
            self.error("`p` adds to a playlist: open one with `gt` and enter");
            return;
        }
        if self.yank.is_empty() {
            self.error("nothing yanked");
            return;
        }

        self.checkpoint();
        let mut rows = Vec::new();
        for path in self.yank.clone() {
            if let Some(row) = self.tracks.iter().position(|t| t.path == path) {
                rows.push(row);
            }
        }
        let n = rows.len();
        let at = (self.cur + 1).min(self.playlist_rows.len());
        self.playlist_rows.splice(at..at, rows);
        self.playlist_dirty = true;
        self.rebuild_view();
        self.goto(at);
        let name = self.playlist_view.clone().unwrap_or_default();
        self.info(format!("put {n} track{} into `{name}`, `:w` saves", plural(n)));
    }

    /// `dd`: drops rows from the open playlist. The files stay on disk.
    pub fn remove_from_playlist(&mut self) {
        if self.playlist_view.is_none() {
            self.error("`dd` removes a track from a playlist, never from disk");
            return;
        }
        if self.playlist_rows.is_empty() {
            return;
        }
        self.checkpoint();
        let (a, b) = self.selection_range();
        let b = b.min(self.playlist_rows.len() - 1);

        // `dd` is a cut, as in vim: what comes out goes in the register, so a
        // `p` further down the playlist puts it back there.
        let taken: Vec<usize> = self.playlist_rows.drain(a..=b).collect();
        self.yank = taken
            .iter()
            .filter_map(|&i| self.tracks.get(i).map(|t| t.path.clone()))
            .collect();

        self.playlist_dirty = true;
        self.exit_visual();
        self.cur = a;
        self.rebuild_view();
        let n = b - a + 1;
        let name = self.playlist_view.clone().unwrap_or_default();
        self.info(format!(
            "cut {n} track{} from `{name}`, `p` puts them back, `:w` saves",
            plural(n)
        ));
    }

    /// Creates an empty playlist, ready for `p` to fill.
    pub fn create_playlist(&mut self, name: &str) {
        let name = name.trim();
        if name.is_empty() {
            self.error("`:playlist` needs a name: `:playlist late night`");
            return;
        }
        if name.contains('/') {
            self.error("a playlist name cannot contain `/`");
            return;
        }
        let Some(dir) = self.playlists_in() else {
            self.error("cannot find a data directory to save playlists in");
            return;
        };

        let path = dir.join(format!("{name}.m3u"));
        if path.exists() {
            self.error(format!("playlist `{name}` already exists"));
            return;
        }

        match library::write_m3u(&path, &[]) {
            Ok(()) => {
                self.reload_playlists();
                self.tab = Tab::Playlists;
                if let Some(row) = self.playlists.iter().position(|(n, _)| n == name) {
                    self.pl_cur = row;
                }
                self.info(format!("`{name}` created, `p` puts tracks in it"));
            }
            Err(e) => self.error(format!("{e}")),
        }
    }

    /// `dd` on a playlist row marks the m3u for deletion, and a second `dd`
    /// takes the mark off again. Nothing happens on disk until `:w`.
    pub fn delete_playlist(&mut self) {
        let Some((name, path)) = self.playlists.get(self.pl_cur).cloned() else {
            return;
        };

        self.checkpoint();
        if let Some(at) = self.doomed.iter().position(|p| *p == path) {
            self.doomed.remove(at);
            self.info(format!("`{name}` kept"));
        } else {
            self.doomed.push(path);
            self.info(format!("`{name}` will be deleted on `:w`, `dd` undoes it"));
        }
    }

    pub fn is_doomed(&self, path: &Path) -> bool {
        self.doomed.iter().any(|p| p == path)
    }

    /// Deletes the playlists marked with `dd`. The tracks they named stay.
    fn apply_deletes(&mut self) -> usize {
        let mut gone = 0;
        for path in std::mem::take(&mut self.doomed) {
            let name = path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            match std::fs::remove_file(&path) {
                Ok(()) => {
                    if self.playlist_view.as_deref() == Some(name.as_str()) {
                        self.playlist_view = None;
                        self.playlist_dirty = false;
                        self.rebuild_view();
                    }
                    // A tab you are not looking at must not keep showing a
                    // playlist whose file no longer exists.
                    for tab in &mut self.tabs {
                        if tab.playlist.as_deref() == Some(name.as_str()) {
                            tab.playlist = None;
                            tab.rows.clear();
                            tab.dirty = false;
                        }
                    }
                    gone += 1;
                }
                Err(e) => self.error(format!("cannot delete `{name}`: {e}")),
            }
        }
        if gone > 0 {
            self.reload_playlists();
        }
        gone
    }

    /// Rewrites every playlist that names a file we just renamed, so a rename
    /// never leaves a playlist pointing at a path that is gone.
    fn repair_playlists(&mut self, renamed: &[(PathBuf, PathBuf)]) -> usize {
        let mut fixed = 0;
        for (_, path) in self.playlists.clone() {
            let Ok(paths) = library::read_m3u(&path) else {
                continue;
            };
            if !paths.iter().any(|p| renamed.iter().any(|(old, _)| old == p)) {
                continue;
            }

            let updated: Vec<PathBuf> = paths
                .into_iter()
                .map(|p| {
                    renamed
                        .iter()
                        .find(|(old, _)| *old == p)
                        .map_or(p, |(_, new)| new.clone())
                })
                .collect();
            let tracks: Vec<&Track> = updated
                .iter()
                .filter_map(|p| self.tracks.iter().find(|t| t.path == *p))
                .collect();
            if library::write_m3u(&path, &tracks).is_ok() {
                fixed += 1;
            }
        }
        fixed
    }

    /// A path as `root/...`, since every change is inside the library anyway.
    /// Quoted when it has a space in it, so the arrow stays readable.
    pub fn under_root(&self, path: &Path) -> String {
        let shown = match path.strip_prefix(&self.root) {
            Ok(rel) => format!("root/{}", rel.display()),
            Err(_) => path.display().to_string(),
        };
        if shown.contains(' ') {
            format!("'{shown}'")
        } else {
            shown
        }
    }

    /// What `:w` would do right now, one line each, for `:changes`.
    ///
    /// Every line starts with its verb, which is also what colours it.
    pub fn pending_changes(&self) -> Vec<String> {
        let mut out = Vec::new();
        // Every rename, wherever it was typed, reads the same way.
        let mut queued = self.renames.clone();
        if let Some(edit) = self.edit.as_ref() {
            let now = edit.buf.text();
            if now.trim().is_empty() || now == self.name_of(&edit.what) {
                queued.remove(&edit.what);
            } else {
                queued.insert(edit.what.clone(), now);
            }
        }
        for (what, name) in &queued {
            out.push(match what {
                Renaming::Track(idx) => {
                    let path = &self.tracks[*idx].path;
                    format!(
                        "rename  {}  ->  {}",
                        self.under_root(path),
                        self.under_root(&path.with_file_name(name))
                    )
                }
                Renaming::Folder(path) => format!(
                    "rename  folder {}  ->  {}",
                    self.under_root(path),
                    self.under_root(&path.with_file_name(name))
                ),
                Renaming::Playlist(old) => {
                    format!("rename  playlist `{old}`  ->  `{name}`")
                }
            });
        }
        if self.playlist_dirty
            && let Some(name) = self.playlist_view.as_ref()
        {
            let n = self.view.len();
            out.push(format!("save    playlist `{name}`, {n} tracks"));
        }
        for (name, rows) in self.dirty_tabs() {
            let n = rows.len();
            out.push(format!("save    playlist `{name}`, {n} tracks"));
        }
        for path in &self.doomed {
            let name = path.file_stem().unwrap_or_default().to_string_lossy();
            out.push(format!("delete  playlist `{name}`, its tracks stay"));
        }
        for (from, to) in &self.moves {
            out.push(format!(
                "move    {}  ->  {}",
                self.under_root(from),
                self.under_root(to)
            ));
        }
        for (from, to) in &self.copies {
            out.push(format!(
                "copy    {}  ->  {}",
                self.under_root(from),
                self.under_root(to)
            ));
        }
        for path in &self.doomed_files {
            out.push(format!("DELETE  {}", self.under_root(path)));
        }
        for dir in &self.doomed_dirs {
            out.push(format!(
                "DELETE  folder {}, and anything else left in it",
                self.under_root(dir)
            ));
        }
        out
    }

    /// Everything waiting to be written, for the marker on the statusline.
    pub fn unsaved(&self) -> bool {
        self.edit_dirty()
            || self.playlist_dirty
            || !self.doomed.is_empty()
            || !self.doomed_files.is_empty()
            || !self.moves.is_empty()
            || !self.copies.is_empty()
            || !self.doomed_dirs.is_empty()
            || self.dirty_tabs().next().is_some()
    }

    /// Playlists changed in a tab you are not looking at. `:w` writes
    /// everything, so `:changes` and the `[+]` marker have to see everything.
    fn dirty_tabs(&self) -> impl Iterator<Item = (String, Vec<usize>)> + '_ {
        self.tabs
            .iter()
            .enumerate()
            .filter(move |(i, tab)| *i != self.tab_idx && tab.dirty)
            .filter_map(|(_, tab)| tab.playlist.clone().map(|name| (name, tab.rows.clone())))
    }

    /// `:w`: renames the files you edited and saves the playlist you changed,
    /// in one press, and says what it did.
    pub fn write_all(&mut self) {
        // Checked before a single thing is written, so a clash leaves every
        // change pending instead of applying half of them.
        if let Some(problem) = self.move_problem() {
            self.error(problem);
            return;
        }

        self.undo_stack.clear();
        self.redo_stack.clear();
        let renames = self.edit_dirty();
        let playlist = self.playlist_dirty;
        let deletes = !self.doomed.is_empty();

        if renames {
            self.apply_edits();
            // A failed batch leaves the names pending; do not go on and write a
            // playlist as if everything were fine.
            if self.edit_dirty() {
                return;
            }
        }

        // Playlists changed in other tabs are written too: `:w` means all of it.
        for (name, rows) in self.dirty_tabs().collect::<Vec<_>>() {
            let tracks: Vec<&Track> = rows.iter().filter_map(|&i| self.tracks.get(i)).collect();
            if let Some(dir) = self.playlists_in()
                && library::write_m3u(&dir.join(format!("{name}.m3u")), &tracks).is_err()
            {
                self.error(format!("cannot save playlist `{name}`"));
                return;
            }
        }
        for (i, tab) in self.tabs.iter_mut().enumerate() {
            if i != self.tab_idx {
                tab.dirty = false;
            }
        }

        if playlist {
            let renamed = self.msg.take();
            self.save_open_playlist();
            if let (Some((before, false)), Some((after, false))) = (renamed, self.msg.clone()) {
                self.info(format!("{before}, {after}"));
            }
        } else if !renames && !deletes {
            self.save_open_playlist();
        }

        let (moved, copied, removed) = self.apply_file_ops();
        if moved + copied + removed > 0 {
            let before = self.msg.take();
            let mut parts = Vec::new();
            if moved > 0 {
                parts.push(format!("moved {moved} file{}", plural(moved)));
            }
            if copied > 0 {
                parts.push(format!("copied {copied} file{}", plural(copied)));
            }
            if removed > 0 {
                parts.push(format!("deleted {removed} file{}", plural(removed)));
            }
            let line = parts.join(", ");
            match before {
                Some((text, false)) => self.info(format!("{text}, {line}")),
                _ => self.info(line),
            }
        }

        if deletes {
            let gone = self.apply_deletes();
            if gone > 0 {
                let before = self.msg.take();
                let line = format!("deleted {gone} playlist{}", plural(gone));
                match before {
                    Some((text, false)) => self.info(format!("{text}, {line}")),
                    _ => self.info(line),
                }
            }
        }
    }

    /// Writes the open playlist back over its own file.
    pub fn save_open_playlist(&mut self) {
        let Some(name) = self.playlist_view.clone() else {
            self.error("`:w` needs a name: `:w late night`");
            return;
        };
        self.save_playlist(&name);
        self.playlist_dirty = false;
    }

    /// Where saved playlists live. State, not configuration, so it sits under
    /// the data dir next to the lyrics cache.
    /// Where this instance keeps playlists. A field rather than a constant so
    /// a test can point it at a scratch directory instead of writing into the
    /// playlists someone actually listens to.
    fn playlists_in(&self) -> Option<PathBuf> {
        self.playlists_dir.clone().or_else(Self::playlist_dir)
    }

    pub fn playlist_dir() -> Option<PathBuf> {
        Some(dirs::data_dir()?.join("vibox/playlists"))
    }

    /// Rereads the playlist directory. Cheap: it lists names, it does not open
    /// the files.
    pub fn reload_playlists(&mut self) {
        let Some(dir) = self.playlists_in() else {
            return;
        };
        let mut found: Vec<(String, PathBuf)> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| library::is_playlist(path))
            .filter_map(|path| {
                let name = path.file_stem()?.to_string_lossy().into_owned();
                Some((name, path))
            })
            .collect();
        found.sort();
        self.playlists = found;
        self.pl_cur = self.pl_cur.min(self.playlists.len().saturating_sub(1));
    }

    pub fn move_playlist(&mut self, delta: isize) {
        if self.playlists.is_empty() {
            return;
        }
        let last = self.playlists.len() as isize - 1;
        self.pl_cur = (self.pl_cur as isize + delta).clamp(0, last) as usize;
        let h = self.folder_h.max(1);
        if self.pl_cur < self.pl_top {
            self.pl_top = self.pl_cur;
        }
        if self.pl_cur >= self.pl_top + h {
            self.pl_top = self.pl_cur + 1 - h;
        }
    }

    /// Shows the playlist under the cursor. The library, and so the folders
    /// tab, is left exactly as it was: a playlist is a view, not a new root.
    pub fn open_playlist(&mut self) {
        let Some((name, path)) = self.playlists.get(self.pl_cur).cloned() else {
            self.error("no playlists yet: `:w <name>` saves what you are looking at");
            return;
        };
        if self.jump_to_open(Some(&name), 0) {
            self.info(format!("`{name}` is already open"));
            return;
        }

        let wanted = match library::read_m3u(&path) {
            Ok(paths) => paths,
            Err(e) => {
                self.error(format!("{e}"));
                return;
            }
        };

        let mut rows = Vec::new();
        let mut missing = 0;
        for want in wanted {
            if let Some(row) = self.tracks.iter().position(|t| t.path == want) {
                rows.push(row);
            } else if want.is_file() {
                // A playlist may name tracks from outside the library; read
                // them in rather than dropping them.
                match library::scan(&want) {
                    Ok(mut found) if !found.is_empty() => {
                        self.tracks.push(found.remove(0));
                        rows.push(self.tracks.len() - 1);
                    }
                    _ => missing += 1,
                }
            } else {
                missing += 1;
            }
        }

        let n = rows.len();
        self.playlist_rows = rows;
        self.playlist_view = Some(name.clone());
        self.cur = 0;
        self.top = 0;
        self.rebuild_view();
        self.focus = Pane::Tracks;

        if missing > 0 {
            self.info(format!("{name}: {n} tracks, {missing} missing"));
        } else {
            self.info(format!("{name}: {n} tracks"));
        }
    }

    /// Writes the current view as a playlist. A bare name goes to the playlist
    /// directory, a name with a path in it goes exactly where it says.
    pub fn save_playlist(&mut self, name: &str) {
        let name = name.trim();
        if name.is_empty() {
            self.error("`:w` needs a name: `:w late night`");
            return;
        }

        let mut path = if name.contains('/') {
            PathBuf::from(name)
        } else {
            let Some(dir) = self.playlists_in() else {
                self.error("cannot find a data directory to save playlists in");
                return;
            };
            dir.join(name)
        };
        if path.extension().is_none() {
            path.set_extension("m3u");
        }

        let tracks: Vec<&Track> = self.view.iter().map(|&i| &self.tracks[i]).collect();
        if tracks.is_empty() {
            self.error("nothing to save");
            return;
        }

        let n = tracks.len();
        match library::write_m3u(&path, &tracks) {
            Ok(()) => {
                self.reload_playlists();
                self.info(format!("saved {n} tracks as {name}"));
            }
            Err(e) => self.error(format!("{e}")),
        }
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
        let Some(&track_idx) = self.view.get(self.cur) else {
            return;
        };
        self.open_name(Renaming::Track(track_idx));
    }

    /// `j` and `k` while editing: the name you were typing joins the pending
    /// set and the next row opens, so a pass down the list is one edit each.
    pub fn edit_next_row(&mut self, delta: isize) {
        let col = self.edit.as_ref().map_or(0, |edit| edit.buf.col());
        self.commit_name();
        self.move_cursor(delta);
        if let Some(&track_idx) = self.view.get(self.cur) {
            self.open_name(Renaming::Track(track_idx));
            if let Some(buf) = self.name_buffer() {
                for _ in 0..col {
                    buf.right();
                }
            }
        }
    }

    /// `c` on a sidebar row: renames the folder or the playlist under it.
    pub fn begin_sidebar_edit(&mut self) {
        let what = match self.tab {
            Tab::Folders => {
                if self.folder_cur == 0 {
                    self.error("that is the whole library, not a folder");
                    return;
                }
                let Some((_, path)) = self.folders.get(self.folder_cur - 1).cloned() else {
                    return;
                };
                Renaming::Folder(path)
            }
            Tab::Playlists => {
                let Some((name, _)) = self.playlists.get(self.pl_cur).cloned() else {
                    self.error("no playlist here to rename");
                    return;
                };
                Renaming::Playlist(name)
            }
        };
        self.open_name(what);
    }

    /// Opens a buffer on whatever is being renamed, starting from the name it
    /// has now, or from a rename already waiting to be written.
    fn open_name(&mut self, what: Renaming) {
        let name = self
            .renames
            .get(&what)
            .cloned()
            .unwrap_or_else(|| self.name_of(&what));

        // Cursor at the start, wherever the name lives.
        self.edit = Some(NameEdit {
            buf: NameBuffer::new(&name),
            what,
        });
        self.mode = Mode::Edit;
    }

    /// The name a thing has on disk right now.
    fn name_of(&self, what: &Renaming) -> String {
        match what {
            Renaming::Track(idx) => self
                .tracks
                .get(*idx)
                .map(|t| t.file.clone())
                .unwrap_or_default(),
            Renaming::Folder(path) => path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            Renaming::Playlist(name) => name.clone(),
        }
    }

    /// The buffer being typed into, in whichever pane opened it.
    pub fn name_buffer(&mut self) -> Option<&mut NameBuffer> {
        self.edit.as_mut().map(|edit| &mut edit.buf)
    }

    /// Cursor position in the name being typed.
    pub fn name_col(&self) -> usize {
        self.edit.as_ref().map_or(0, |edit| edit.buf.col())
    }

    /// How far the name being typed has scrolled inside its column.
    pub fn name_scroll(&self) -> usize {
        self.edit.as_ref().map_or(0, |edit| edit.buf.scroll())
    }

    /// True while the name being typed belongs to a track row.
    pub fn renaming_a_track(&self) -> bool {
        matches!(self.renaming(), Some(Renaming::Track(_)))
    }

    /// What is being renamed right now, if anything.
    pub fn renaming(&self) -> Option<&Renaming> {
        self.edit.as_ref().map(|edit| &edit.what)
    }

    /// The name to show for something: the one being typed, then any pending
    /// rename, then what it is called on disk.
    pub fn shown_name(&self, what: &Renaming) -> Option<String> {
        if let Some(edit) = self.edit.as_ref().filter(|edit| edit.what == *what) {
            return Some(edit.buf.text());
        }
        self.renames.get(what).cloned()
    }

    /// The name shown for a track row.
    pub fn edit_text(&self, row: usize) -> Option<String> {
        let track_idx = *self.view.get(row)?;
        Some(
            self.shown_name(&Renaming::Track(track_idx))
                .unwrap_or_else(|| self.tracks[track_idx].file.clone()),
        )
    }

    /// Puts the name being typed into the pending set, and closes the buffer.
    ///
    /// Nothing is written here. A rename queues up with the moves, copies and
    /// deletions so one `:w` does the lot and `u` takes any of it back, which
    /// is why leaving a name never demands a write of its own.
    pub fn commit_name(&mut self) {
        let Some(edit) = self.edit.take() else {
            return;
        };
        let name = edit.buf.text().trim().to_string();
        let was = self.name_of(&edit.what);

        if name == was {
            self.renames.remove(&edit.what);
        } else if !name.is_empty() {
            self.checkpoint();
            self.renames.insert(edit.what, name);
        }
    }

    /// True when a name has been changed but not written.
    pub fn edit_dirty(&self) -> bool {
        if !self.renames.is_empty() {
            return true;
        }
        self.edit
            .as_ref()
            .is_some_and(|edit| edit.buf.text() != self.name_of(&edit.what))
    }

    pub fn end_edit(&mut self) {
        self.edit = None;
        if matches!(self.mode, Mode::Edit | Mode::EditInsert) {
            self.mode = Mode::Normal;
        }
    }

    /// `:e!`: throws away everything waiting for a `:w`.
    ///
    /// All of it, not just the name being typed: the point of the command is
    /// that `[+]` goes out and `:changes` comes back empty.
    pub fn discard_changes(&mut self) {
        let had = self.unsaved();
        self.end_edit();
        self.renames.clear();
        self.doomed.clear();
        self.doomed_files.clear();
        self.doomed_dirs.clear();
        self.cut.clear();
        self.moves.clear();
        self.copies.clear();
        self.playlist_dirty = false;
        for tab in &mut self.tabs {
            tab.dirty = false;
        }
        self.undo_stack.clear();
        self.redo_stack.clear();

        // The list is showing cuts and moves that are no longer going to
        // happen, so it has to be rebuilt from the library as it stands.
        if let Err(e) = self.reload() {
            self.error(format!("{e}"));
            return;
        }
        self.reload_playlists();

        if had {
            self.info("changes thrown away");
        }
    }

    /// Renames every changed file. Names are checked first, so a bad one stops
    /// the whole write instead of leaving the batch half applied.
    pub fn apply_edits(&mut self) {
        self.commit_name();
        if self.renames.is_empty() {
            return;
        }

        // Everything is checked before anything runs: a bad name stops the
        // whole write rather than leaving half a folder renamed.
        let queued: Vec<(Renaming, String)> = self
            .renames
            .iter()
            .map(|(what, name)| (what.clone(), name.trim().to_string()))
            .collect();
        for (what, name) in &queued {
            if name.is_empty() {
                self.error("a name cannot be empty");
                return;
            }
            if name.contains('/') {
                self.error("a name cannot contain `/`, this renames but never moves");
                return;
            }
            if let Some(to) = self.rename_target(what, name)
                && to.exists()
            {
                self.error(format!("`{name}` already exists here"));
                return;
            }
        }

        let mut files = 0;
        let mut moved: Vec<(PathBuf, PathBuf)> = Vec::new();
        for (what, name) in queued {
            match what {
                Renaming::Track(idx) => {
                    let old = self.tracks[idx].path.clone();
                    let Some(new) = self.rename_target(&Renaming::Track(idx), &name) else {
                        continue;
                    };
                    match std::fs::rename(&old, &new) {
                        Ok(()) => {
                            self.tracks[idx].path = new.clone();
                            self.tracks[idx].file = name;
                            moved.push((old, new));
                            files += 1;
                        }
                        Err(e) => self.error(format!("cannot rename `{}`: {e}", old.display())),
                    }
                }
                Renaming::Folder(old) => {
                    let new = old.with_file_name(&name);
                    if let Err(e) = std::fs::rename(&old, &new) {
                        self.error(format!("cannot rename `{name}`: {e}"));
                        continue;
                    }
                    // One rename moves the folder; the tracks inside it just
                    // need their paths followed.
                    for track in &mut self.tracks {
                        if let Ok(rest) = track.path.strip_prefix(&old) {
                            let to = new.join(rest);
                            moved.push((track.path.clone(), to.clone()));
                            track.path = to;
                        }
                    }
                    files += 1;
                }
                Renaming::Playlist(old) => {
                    let Some(dir) = self.playlists_in() else {
                        continue;
                    };
                    let to = dir.join(format!("{name}.m3u"));
                    if let Err(e) = std::fs::rename(dir.join(format!("{old}.m3u")), &to) {
                        self.error(format!("cannot rename `{old}`: {e}"));
                        continue;
                    }
                    // A tab showing it keeps showing it, under the new name.
                    if self.playlist_view.as_deref() == Some(old.as_str()) {
                        self.playlist_view = Some(name.clone());
                    }
                    for tab in &mut self.tabs {
                        if tab.playlist.as_deref() == Some(old.as_str()) {
                            tab.playlist = Some(name.clone());
                        }
                    }
                    files += 1;
                }
            }
        }

        self.renames.clear();
        self.end_edit();
        let fixed = self.repair_playlists(&moved);
        self.folders = library::folders(&self.tracks, &self.root);
        self.reload_playlists();
        self.rebuild_view();

        if fixed > 0 {
            self.info(format!(
                "renamed {files}, updated {fixed} playlist{}",
                plural(fixed)
            ));
        } else {
            self.info(format!("renamed {files}"));
        }
    }

    /// Where a rename would put something, for the checks and the write.
    fn rename_target(&self, what: &Renaming, name: &str) -> Option<PathBuf> {
        match what {
            Renaming::Track(idx) => {
                let path = &self.tracks.get(*idx)?.path;
                // Built by hand: `set_extension` would eat everything after a
                // dot in a name like `Mr. Blue Sky`.
                let file_name = match path.extension().and_then(|e| e.to_str()) {
                    Some(ext) => format!("{name}.{ext}"),
                    None => name.to_string(),
                };
                Some(path.with_file_name(file_name))
            }
            Renaming::Folder(path) => Some(path.with_file_name(name)),
            Renaming::Playlist(_) => Some(self.playlists_in()?.join(format!("{name}.m3u"))),
        }
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
        self.history.clear();
        self.play_queue_pos();
    }

    /// Where a track actually is on disk right now.
    ///
    /// A pending move has already changed the path the list shows, but the file
    /// itself does not move until `:w`, so playback follows it back.
    fn disk_path(&self, path: &Path) -> PathBuf {
        self.moves
            .iter()
            .find(|(_, to)| to == path)
            .map_or_else(|| path.to_path_buf(), |(from, _)| from.clone())
    }

    fn play_queue_pos(&mut self) {
        let Some(&track_idx) = self.queue.get(self.qpos) else {
            return;
        };
        let path = self.disk_path(&self.tracks[track_idx].path);
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
        // Shuffle keeps a history, because "previous" has to mean the track you
        // just heard. Without it, going back walks the queue order instead, to
        // a track that was never played.
        if self.shuffle {
            if delta > 0 {
                self.history.push(self.qpos);
                self.qpos = self.next_random();
            } else if let Some(previous) = self.history.pop() {
                self.qpos = previous;
            } else {
                self.info("nothing played before this");
                return;
            }
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

    /// Copies text to the system clipboard with OSC 52, the escape sequence
    /// terminals answer for it.
    ///
    /// No clipboard crate: those pull in x11 or wayland C libraries, and the
    /// terminal already knows how to do this. tmux needs `set-clipboard on`.
    pub fn copy_to_clipboard(&mut self, text: &str) {
        use std::io::Write;

        const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let bytes = text.as_bytes();
        let mut encoded = String::new();
        for chunk in bytes.chunks(3) {
            let b = [
                chunk[0],
                chunk.get(1).copied().unwrap_or(0),
                chunk.get(2).copied().unwrap_or(0),
            ];
            let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
            for i in 0..4 {
                if i <= chunk.len() {
                    encoded.push(ALPHABET[(n >> (18 - i * 6)) as usize & 0x3F] as char);
                } else {
                    encoded.push('=');
                }
            }
        }

        let mut out = std::io::stdout();
        if write!(out, "\x1b]52;c;{encoded}\x07")
            .and_then(|()| out.flush())
            .is_ok()
        {
            self.info(format!("copied {text}"));
        } else {
            self.error("cannot reach the terminal clipboard");
        }
    }

    // ---- messages ---------------------------------------------------------

    pub fn info(&mut self, text: impl Into<String>) {
        self.msg = Some((text.into(), false));
    }

    pub fn error(&mut self, text: impl Into<String>) {
        self.msg = Some((text.into(), true));
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

fn seed() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0x2545_F491_4F6C_DD1D, |d| d.as_nanos() as u64)
        | 1
}


#[cfg(test)]
mod tests {
    use super::*;

    /// Types a whole name into whichever buffer `c` opened.
    fn set_name(app: &mut App, name: &str) {
        if let Some(buf) = app.name_buffer() {
            buf.clear();
            for c in name.chars() {
                buf.insert(c);
            }
        }
    }

    /// A real library on disk, since `App` is the thing under test.
    ///
    /// Everything a test can write lives under the returned directory, which is
    /// deleted when it drops. Playlists included: a test that wrote into the
    /// user's own playlist folder would leave rubbish on a contributor's
    /// machine, and could overwrite a playlist they actually listen to.
    fn library(files: &[&str]) -> (App, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        for name in files {
            let path = dir.path().join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, b"").unwrap();
        }

        let mut app = App::new(dir.path().to_path_buf(), SortKey::Path, library::QUIET).unwrap();
        app.playlists_dir = Some(dir.path().join(".playlists"));
        app.reload_playlists();
        (app, dir)
    }

    #[test]
    fn moving_the_folder_cursor_leaves_the_track_list_alone() {
        let (mut app, _dir) = library(&["a.mp3", "jazz/b.mp3"]);
        assert_eq!(app.view.len(), 2, "everything is shown to begin with");

        app.move_folder(1);
        assert_eq!(app.view.len(), 2, "the cursor moved, the view did not");

        app.open_folder();
        assert_eq!(app.view.len(), 1, "enter is what opens a folder");
        assert_eq!(app.focus, Pane::Tracks);
    }

    #[test]
    fn a_move_onto_an_existing_file_stops_the_whole_batch() {
        let (mut app, dir) = library(&["a.mp3", "b.mp3", "jazz/a.mp3"]);
        let root = dir.path().to_path_buf();

        app.moves = vec![
            (root.join("b.mp3"), root.join("jazz/b.mp3")),
            (root.join("a.mp3"), root.join("jazz/a.mp3")),
        ];
        assert!(app.move_problem().is_some(), "the clash has to be caught");

        app.write_all();
        assert!(
            root.join("b.mp3").exists(),
            "the file with no clash must not have moved either"
        );
        assert_eq!(app.moves.len(), 2, "everything stays pending");
    }

    #[test]
    fn two_files_moved_onto_the_same_name_are_refused() {
        let (mut app, dir) = library(&["one/x.mp3", "two/x.mp3"]);
        let root = dir.path().to_path_buf();
        std::fs::create_dir_all(root.join("all")).unwrap();

        app.moves = vec![
            (root.join("one/x.mp3"), root.join("all/x.mp3")),
            (root.join("two/x.mp3"), root.join("all/x.mp3")),
        ];
        assert!(app.move_problem().is_some());
    }

    #[test]
    fn a_clean_batch_of_moves_is_written() {
        let (mut app, dir) = library(&["a.mp3", "b.mp3"]);
        let root = dir.path().to_path_buf();
        std::fs::create_dir_all(root.join("jazz")).unwrap();

        app.moves = vec![
            (root.join("a.mp3"), root.join("jazz/a.mp3")),
            (root.join("b.mp3"), root.join("jazz/b.mp3")),
        ];
        app.write_all();

        assert!(root.join("jazz/a.mp3").exists());
        assert!(root.join("jazz/b.mp3").exists());
        assert!(!root.join("a.mp3").exists());
        assert!(app.moves.is_empty());
    }

    #[test]
    fn deleting_a_track_repoints_everything_that_held_an_index() {
        let (mut app, dir) = library(&["a.mp3", "b.mp3", "c.mp3"]);
        let gone = dir.path().join("b.mp3");

        // The queue and an open playlist both hold indices into `tracks`.
        app.queue = vec![0, 1, 2];
        app.playlist_rows = vec![2, 1, 0];
        app.playing = Some(2);
        let last = app.tracks[2].path.clone();

        app.forget_tracks(&[gone]);

        assert_eq!(app.tracks.len(), 2);
        assert_eq!(app.queue.len(), 2, "the deleted track leaves the queue");
        assert_eq!(app.playlist_rows.len(), 2);
        assert_eq!(
            app.tracks[app.playing.unwrap()].path,
            last,
            "what is playing must still be the same file"
        );
        for &row in app.playlist_rows.iter().chain(app.queue.iter()) {
            assert!(row < app.tracks.len(), "no index may dangle");
        }
    }

    #[test]
    fn changes_show_playlists_edited_in_another_tab() {
        let (mut app, _dir) = library(&["a.mp3"]);
        app.tabs.push(ViewTab {
            playlist: Some("roadtrip".into()),
            folder: 0,
            cur: 0,
            top: 0,
            sort_key: SortKey::Path,
            rows: vec![0],
            dirty: true,
        });

        assert!(app.unsaved(), "a change anywhere counts as unsaved");
        assert!(
            app.pending_changes().iter().any(|line| line.contains("roadtrip")),
            "`:changes` has to list what `:w` would write, in every tab"
        );
    }

    #[test]
    fn a_playlist_is_a_view_and_does_not_become_the_library() {
        let (mut app, dir) = library(&["a.mp3", "jazz/b.mp3"]);
        let root = dir.path().to_path_buf();
        let before = app.tracks.len();

        app.create_playlist("mix");
        app.open_playlist();

        assert_eq!(app.root, root, "the library root never moves");
        assert_eq!(app.tracks.len(), before);
        assert!(!app.folders.is_empty(), "the folders tab still browses");
    }

    #[test]
    fn danger_is_needed_before_a_key_can_delete_a_file() {
        let (mut app, _dir) = library(&["a.mp3"]);
        app.cut_tracks();
        assert!(app.doomed_files.is_empty(), "nothing marked without danger");
        assert!(app.msg.as_ref().is_some_and(|(_, error)| *error));

        app.danger = true;
        app.cut_tracks();
        assert_eq!(app.doomed_files.len(), 1);
        assert!(app.unsaved());
        assert!(app.view.is_empty(), "a marked file leaves the list at once");
    }

    #[test]
    fn a_view_is_only_ever_open_in_one_tab() {
        let (mut app, _dir) = library(&["a.mp3", "jazz/b.mp3"]);
        app.create_playlist("mix");
        app.open_playlist();
        let tabs = app.tabs.len();

        // Asking for it again from somewhere else takes you back to it.
        app.tab = Tab::Playlists;
        app.open_in_new_tab();
        assert_eq!(app.tabs.len(), tabs, "no second tab of the same playlist");
        assert_eq!(app.playlist_view.as_deref(), Some("mix"));
    }

    #[test]
    fn renaming_the_playing_track_keeps_playback_pointed_at_it() {
        let (mut app, _dir) = library(&["a.mp3", "b.mp3"]);
        app.queue = app.view.clone();
        app.qpos = 0;
        app.playing = Some(app.view[0]);

        app.begin_edit();
        set_name(&mut app, "renamed");
        app.apply_edits();

        let playing = app.playing.expect("still playing something");
        assert_eq!(app.tracks[playing].file, "renamed");
        assert_eq!(
            app.tracks[playing].path.file_name().unwrap(),
            "renamed.mp3",
            "the track that is playing has to follow its own rename"
        );
        assert!(app.tracks[playing].path.exists());
    }

    #[test]
    fn a_folder_takes_everything_under_it_including_subfolders() {
        let (mut app, dir) = library(&["jazz/a.mp3", "jazz/live/b.mp3", "rock/c.mp3"]);
        app.danger = true;
        // row 0 is the whole library, so jazz is the first real folder
        app.folder_cur = 1 + app
            .folders
            .iter()
            .position(|(label, _)| label == "jazz")
            .unwrap();
        app.cut_folder();

        let listed = app.pending_changes().join("\n");
        assert!(listed.contains("a.mp3"), "every track is listed on its own");
        assert!(listed.contains("b.mp3"), "a subfolder's tracks come too");
        assert!(!listed.contains("c.mp3"), "another folder is untouched");

        app.write_all();
        assert!(!dir.path().join("jazz").exists(), "the folder is gone");
        assert!(dir.path().join("rock/c.mp3").exists());
    }

    #[test]
    fn a_marked_folder_is_unmarked_by_pressing_dd_again() {
        let (mut app, _dir) = library(&["jazz/a.mp3"]);
        app.danger = true;
        app.folder_cur = 1;
        app.cut_folder();
        assert!(app.unsaved());

        app.cut_folder();
        assert!(!app.unsaved(), "the second dd puts it back");
        assert!(app.doomed_files.is_empty());
    }

    #[test]
    fn renaming_a_folder_is_one_rename_and_the_tracks_follow() {
        let (mut app, dir) = library(&["jazz/a.mp3", "jazz/live/b.mp3"]);
        app.folder_cur = 1 + app
            .folders
            .iter()
            .position(|(label, _)| label == "jazz")
            .unwrap();

        app.begin_sidebar_edit();
        set_name(&mut app, "Jazz");
        assert!(app.edit_dirty());
        app.apply_edits();

        assert!(dir.path().join("Jazz/a.mp3").exists());
        assert!(dir.path().join("Jazz/live/b.mp3").exists(), "subfolders come too");
        assert!(!dir.path().join("jazz").exists());
        for track in &app.tracks {
            assert!(track.path.starts_with(dir.path().join("Jazz")));
        }
    }

    #[test]
    fn renaming_a_playlist_keeps_the_tab_showing_it() {
        let (mut app, _dir) = library(&["a.mp3"]);
        app.create_playlist("mix");
        app.open_playlist();
        app.tab = Tab::Playlists;

        app.begin_sidebar_edit();
        set_name(&mut app, "roadtrip");
        app.apply_edits();

        assert_eq!(app.playlist_view.as_deref(), Some("roadtrip"));
        assert!(app.playlists.iter().any(|(name, _)| name == "roadtrip"));
        assert!(!app.playlists.iter().any(|(name, _)| name == "mix"));
    }

    #[test]
    fn dd_in_a_playlist_cuts_so_p_can_put_it_back() {
        let (mut app, _dir) = library(&["a.mp3", "b.mp3", "c.mp3"]);
        // yank the three tracks from the library, then fill a playlist
        app.mode = Mode::Visual;
        app.visual_anchor = Some(0);
        app.cur = 2;
        app.yank_selection();

        app.create_playlist("mix");
        app.open_playlist();
        app.paste_into_playlist();
        let order: Vec<String> = app.view.iter().map(|&i| app.tracks[i].file.clone()).collect();
        assert_eq!(order, ["a", "b", "c"]);

        app.cur = 0;
        app.remove_from_playlist();
        assert_eq!(app.yank.len(), 1, "a cut fills the register");

        app.cur = 1;
        app.paste_into_playlist();
        let order: Vec<String> = app.view.iter().map(|&i| app.tracks[i].file.clone()).collect();
        assert_eq!(order, ["b", "c", "a"], "dd then p is how a track moves");
    }

    #[test]
    fn deleting_a_playlist_leaves_every_track_alone() {
        let (mut app, dir) = library(&["a.mp3", "b.mp3"]);
        app.mode = Mode::Visual;
        app.visual_anchor = Some(0);
        app.cur = 1;
        app.yank_selection();

        app.create_playlist("mix");
        app.open_playlist();
        app.paste_into_playlist();
        app.write_all();

        app.tab = Tab::Playlists;
        app.pl_cur = 0;
        app.delete_playlist();
        app.write_all();

        assert!(app.playlists.is_empty(), "the m3u is gone");
        assert!(dir.path().join("a.mp3").exists(), "its tracks are not");
        assert!(dir.path().join("b.mp3").exists());
        assert_eq!(app.tracks.len(), 2);
    }

    #[test]
    fn shuffle_goes_back_to_what_was_actually_played() {
        let (mut app, _dir) = library(&["a.mp3", "b.mp3", "c.mp3", "d.mp3", "e.mp3"]);
        app.shuffle = true;
        app.queue = app.view.clone();
        app.qpos = 0;
        app.history.clear();

        let mut heard = vec![app.qpos];
        for _ in 0..3 {
            app.advance(1, false);
            heard.push(app.qpos);
        }

        // Walking back has to retrace those steps, not the queue order.
        for expected in heard.iter().rev().skip(1) {
            app.advance(-1, false);
            assert_eq!(app.qpos, *expected);
        }
    }

    #[test]
    fn e_bang_throws_away_every_kind_of_pending_change() {
        let (mut app, dir) = library(&["a.mp3", "b.mp3", "jazz/c.mp3"]);
        app.danger = true;

        // a rename, a cut file, and a playlist edit, all waiting
        app.begin_edit();
        set_name(&mut app, "renamed");
        app.commit_name();

        app.cur = 1;
        app.cut_tracks();

        app.mode = Mode::Visual;
        app.visual_anchor = Some(0);
        app.cur = 0;
        app.yank_selection();
        app.create_playlist("mix");
        app.open_playlist();
        app.paste_into_playlist();

        assert!(app.unsaved());
        assert!(!app.pending_changes().is_empty());

        app.discard_changes();

        assert!(!app.unsaved(), "nothing is waiting any more");
        assert!(app.pending_changes().is_empty(), "`:changes` comes back empty");
        assert!(app.renames.is_empty());
        assert!(app.doomed_files.is_empty());
        assert!(!app.playlist_dirty);

        // and the disk was never touched by any of it
        assert!(dir.path().join("a.mp3").exists());
        assert!(dir.path().join("b.mp3").exists());
        assert_eq!(app.tracks.len(), 3, "the cut track is back in the list");
    }

    #[test]
    fn a_playlist_of_outside_tracks_never_joins_the_library() {
        let (mut app, dir) = library(&["a.mp3", "b.mp3"]);

        // a playlist naming a file that lives outside the library root
        let outside = tempfile::tempdir().unwrap();
        let far = outside.path().join("HOLA/far.mp3");
        std::fs::create_dir_all(far.parent().unwrap()).unwrap();
        std::fs::write(&far, b"").unwrap();
        let m3u = app.playlists_dir.clone().unwrap().join("test.m3u");
        std::fs::create_dir_all(m3u.parent().unwrap()).unwrap();
        std::fs::write(&m3u, format!("#EXTM3U\n{}\n", far.display())).unwrap();
        app.reload_playlists();

        app.tab = Tab::Playlists;
        app.pl_cur = 0;
        app.open_playlist();
        assert_eq!(app.view.len(), 1, "the playlist shows its track");

        // back to everything, exactly as pressing gt and enter does
        app.tab = Tab::Folders;
        app.folder_cur = 0;
        app.open_folder();

        assert_eq!(
            app.view.len(),
            2,
            "everything is the library, not the library plus a playlist's strays"
        );
        assert!(
            !app.folders.iter().any(|(label, _)| label.contains("HOLA")),
            "a folder outside the root has no business in the folder list"
        );
        assert!(dir.path().join("a.mp3").exists());
    }

    #[test]
    fn undo_takes_back_a_pending_deletion() {
        let (mut app, _dir) = library(&["a.mp3", "b.mp3"]);
        app.danger = true;
        app.cut_tracks();
        assert_eq!(app.view.len(), 1);

        app.undo();
        assert!(app.doomed_files.is_empty());
        assert_eq!(app.view.len(), 2, "the row comes back");
        assert!(!app.unsaved());
    }
}
