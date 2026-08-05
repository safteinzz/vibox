//! The library: files on disk, their tags, and the sort orders we present them in.
//!
//! vibox never owns a database. A library is a directory, a track is a file, a
//! playlist is an m3u. Everything here is a pure function of what is on disk.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use rayon::prelude::*;
use walkdir::WalkDir;

/// Extensions we hand to the decoder. Anything else in the tree is ignored.
pub const AUDIO_EXTS: [&str; 8] = ["mp3", "flac", "ogg", "opus", "m4a", "aac", "wav", "wv"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Track {
    pub path: PathBuf,
    /// Filename without its extension. This is what the track list shows; the
    /// tag title is kept for the statusline and for mpris.
    pub file: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub album_artist: String,
    pub track_no: Option<u32>,
    pub disc_no: Option<u32>,
    pub year: Option<u32>,
    pub genre: String,
    pub duration: Duration,
}

impl Track {
    /// Directory holding the file, which for a picarded library is the album.
    pub fn dir(&self) -> &Path {
        self.path.parent().unwrap_or(Path::new(""))
    }

    /// Haystack for `/` searches: the columns of the row, and nothing else.
    ///
    /// The directory is deliberately absent. Including the path made every
    /// track under `~/Music/TESTING` match `/testing`, so a search inside a
    /// folder matched the folder instead of anything in it.
    pub fn haystack(&self) -> String {
        format!(
            "{} {} {} {} {}",
            self.file, self.title, self.artist, self.album, self.album_artist
        )
    }
}

/// Reads tags for one file, falling back to the filename when a file is untagged.
fn read_track(path: &Path) -> Track {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();

    let mut track = Track {
        path: path.to_path_buf(),
        file: stem.clone(),
        title: stem.clone(),
        artist: String::new(),
        album: String::new(),
        album_artist: String::new(),
        track_no: None,
        disc_no: None,
        year: None,
        genre: String::new(),
        duration: Duration::ZERO,
    };

    let Ok(tagged) = lofty::read_from_path(path) else {
        return track;
    };

    use lofty::file::{AudioFile, TaggedFileExt};
    track.duration = tagged.properties().duration();

    let Some(tag) = tagged.primary_tag().or_else(|| tagged.first_tag()) else {
        return track;
    };

    use lofty::tag::{Accessor, ItemKey};
    // A tag that is present but blank is the same as no tag: a row with an
    // empty title is a file you cannot see.
    if let Some(v) = tag.title().filter(|v| !v.trim().is_empty()) {
        track.title = v.into_owned();
    }
    if let Some(v) = tag.artist().filter(|v| !v.trim().is_empty()) {
        track.artist = v.into_owned();
    }
    if let Some(v) = tag.album().filter(|v| !v.trim().is_empty()) {
        track.album = v.into_owned();
    }
    if let Some(v) = tag.genre().filter(|v| !v.trim().is_empty()) {
        track.genre = v.into_owned();
    }
    track.album_artist = tag
        .get_string(ItemKey::AlbumArtist)
        .filter(|v| !v.trim().is_empty())
        .unwrap_or(&track.artist)
        .to_string();
    track.track_no = tag.track();
    track.disc_no = tag.disk();
    // Picard writes a full date; only the year is worth a column.
    track.year = tag
        .date()
        .map(|d| u32::from(d.year))
        .or_else(|| tag.get_string(ItemKey::RecordingDate)?.get(..4)?.parse().ok());

    track
}

/// Walks `root` and reads every audio file under it.
///
/// Symlinks are not followed: a music tree with a loop in it should not hang
/// the player. Unreadable entries are skipped rather than failing the scan.
pub fn scan(root: &Path) -> Result<Vec<Track>> {
    if !root.exists() {
        bail!("no such path: `{}`", root.display());
    }

    let paths: Vec<PathBuf> = if is_playlist(root) {
        // An m3u is a library too: the tracks it names, in the order it names them.
        read_m3u(root)?
            .into_iter()
            .filter(|p| p.is_file())
            .collect()
    } else if root.is_file() {
        vec![root.to_path_buf()]
    } else {
        WalkDir::new(root)
            .follow_links(false)
            .into_iter()
            .filter_map(std::result::Result::ok)
            .filter(|e| e.file_type().is_file())
            .map(walkdir::DirEntry::into_path)
            .filter(|p| is_audio(p))
            .collect()
    };

    let mut tracks: Vec<Track> = paths.par_iter().map(|p| read_track(p)).collect();
    // A playlist already has an order; a directory tree does not.
    if !is_playlist(root) {
        sort(&mut tracks, SortKey::Path);
    }
    Ok(tracks)
}

pub fn is_playlist(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .is_some_and(|e| e == "m3u" || e == "m3u8")
}

pub fn is_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .is_some_and(|e| AUDIO_EXTS.contains(&e.as_str()))
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum SortKey {
    /// Directory order, which for a tagged library already is album order.
    #[default]
    Path,
    Title,
    Artist,
    Album,
    Duration,
}

impl SortKey {
    pub fn parse(s: &str) -> Option<SortKey> {
        match s {
            "path" | "p" | "file" => Some(SortKey::Path),
            "title" | "t" => Some(SortKey::Title),
            "artist" | "a" => Some(SortKey::Artist),
            "album" | "al" => Some(SortKey::Album),
            "duration" | "d" | "len" => Some(SortKey::Duration),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            SortKey::Path => "path",
            SortKey::Title => "title",
            SortKey::Artist => "artist",
            SortKey::Album => "album",
            SortKey::Duration => "duration",
        }
    }
}

fn lower(s: &str) -> String {
    s.to_lowercase()
}

pub fn sort(tracks: &mut [Track], key: SortKey) {
    match key {
        SortKey::Path => tracks.sort_by(|a, b| a.path.cmp(&b.path)),
        SortKey::Title => tracks.sort_by_key(|t| (lower(&t.title), t.path.clone())),
        SortKey::Artist => {
            tracks.sort_by_key(|t| (lower(&t.artist), lower(&t.album), t.disc_no, t.track_no));
        }
        SortKey::Album => {
            tracks.sort_by_key(|t| {
                (
                    lower(&t.album_artist),
                    lower(&t.album),
                    t.disc_no,
                    t.track_no,
                )
            });
        }
        SortKey::Duration => tracks.sort_by_key(|t| (t.duration, t.path.clone())),
    }
}

/// The directories that actually contain tracks, as shown in the left pane.
pub fn folders(tracks: &[Track], root: &Path) -> Vec<(String, PathBuf)> {
    let dirs: BTreeSet<PathBuf> = tracks.iter().map(|t| t.dir().to_path_buf()).collect();
    dirs.into_iter()
        .map(|d| {
            // Outside the root (an m3u can name anything), the full path is
            // noise, so fall back to the directory's own name.
            let label = match d.strip_prefix(root) {
                Ok(rel) if rel.as_os_str().is_empty() => ".".to_string(),
                Ok(rel) => rel.to_string_lossy().into_owned(),
                Err(_) => d
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| d.to_string_lossy().into_owned()),
            };
            (label, d)
        })
        .collect()
}

/// `4:03`, or `1:02:03` once a track runs past the hour.
pub fn fmt_duration(d: Duration) -> String {
    let secs = d.as_secs();
    let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// Reads the file paths out of an m3u, ignoring directives and comments.
pub fn read_m3u(path: &Path) -> Result<Vec<PathBuf>> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("cannot read playlist `{}`", path.display()))?;
    Ok(text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(PathBuf::from)
        .collect())
}

