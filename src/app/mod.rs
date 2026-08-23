//! Application state. One struct, mutated by key handlers and ex commands,
//! read by the renderer. No state lives anywhere else.

use std::path::PathBuf;

use anyhow::Result;

use crate::library::{self, SortKey, Track};
use crate::lyrics::Fetcher;
use crate::matrix::Matrix;
use crate::mpris::{self, Mpris};
use crate::name::NameBuffer;
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
    /// Whether the lyrics pane follows the song. Off leaves the words still and
    /// unstyled, which is what you want when lrclib's timings do not fit your
    /// recording.
    pub karaoke: bool,
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
    /// What the last delete or yank inside a name took, for `p` to put back.
    ///
    /// Deliberately not the same register as the one `y` and `p` use on track
    /// rows: that one holds files, and pasting a file into a filename is not
    /// a thing anyone means.
    pub name_reg: String,

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
    /// Queue positions shuffle has not played yet this cycle. Refilled, minus
    /// whatever is currently playing, once it runs dry, so every track is
    /// heard once before any of them repeat.
    shuffle_bag: Vec<usize>,
    pub playing: Option<usize>,
    pub repeat: Repeat,
    pub shuffle: bool,

    pub audio: Option<Audio>,
    /// Absent on a machine with no session bus; media keys just do nothing then.
    pub mpris: Option<Mpris>,
    pub msg: Option<(String, bool)>,
    /// Where the real terminal cursor goes while inserting, set by the renderer.
    pub cursor_screen: Option<(u16, u16)>,
    /// Everything played this session, oldest first, one line each. Repeats are
    /// kept: playing a track twice happened twice. It is a log rather than a
    /// list of indices, so a delete or a rename cannot make it point at the
    /// wrong track later.
    pub played: Vec<String>,
    pub show_history: bool,
    /// First line of the log on screen, clamped by the renderer.
    pub history_top: usize,
    pub show_info: bool,
    /// How far `:changes` is panned sideways, in cells.
    pub changes_pan: usize,
    /// First row shown, for a batch taller than the popup.
    pub changes_top: usize,
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
            karaoke: true,
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
            name_reg: String::new(),
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
            shuffle_bag: Vec::new(),
            playing: None,
            repeat: Repeat::Off,
            shuffle: false,
            audio,
            mpris: mpris::start().ok(),
            msg: None,
            cursor_screen: None,
            played: Vec::new(),
            show_history: false,
            history_top: 0,
            show_info: false,
            changes_pan: 0,
            changes_top: 0,
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

// The rest of `impl App`, split by what each part deals with. Several impl
// blocks for one type are ordinary rust; the split is so a reader can find
// things, and so each area only reaches what it needs.
mod cursor;
mod danger;
pub mod plan;
mod playback;
mod playlists;
mod rename;
mod scan;
mod search;
mod select;
mod tabs;
mod undo;

#[cfg(test)]
mod tests;
