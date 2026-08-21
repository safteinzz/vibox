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
    /// False for a track a playlist named from outside the library.
    ///
    /// Those are read in so the playlist can play them, but they are not part
    /// of the library: `everything` and the folder list must not grow a stray
    /// directory because a playlist mentioned one. Set where the track is
    /// born, so it does not depend on what `root` happens to be.
    pub in_library: bool,
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
        // Everything a scan finds is the library, whether the root is a
        // directory or an m3u. Only `App::open_playlist` clears this.
        in_library: true,
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
    track.year = tag.date().map(|d| u32::from(d.year)).or_else(|| {
        tag.get_string(ItemKey::RecordingDate)?
            .get(..4)?
            .parse()
            .ok()
    });

    track
}

/// How far along a scan is. A cold or large library takes seconds to walk and
/// tag, and the caller is the only one that knows where to say so.
#[derive(Clone, Copy, Debug)]
pub enum Scan {
    /// Walking the tree: audio files found so far. The total is not known yet.
    Walking(usize),
    /// Reading tags: files done, files found.
    Reading(usize, usize),
}

/// Where a scan reports its progress. Called from the scan's worker threads,
/// so a sink that prints has to serialise itself.
pub type Report<'a> = &'a (dyn Fn(Scan) + Sync);

/// A scan nobody is watching.
pub const QUIET: Report<'static> = &|_| {};

/// Walks `root` and reads every audio file under it.
///
/// Symlinks are not followed: a music tree with a loop in it should not hang
/// the player. Unreadable entries are skipped rather than failing the scan.
pub fn scan(root: &Path) -> Result<Vec<Track>> {
    scan_reporting(root, QUIET)
}

/// `scan`, saying how far along it is as it goes.
pub fn scan_reporting(root: &Path, on: Report) -> Result<Vec<Track>> {
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
        let mut found = Vec::new();
        for entry in WalkDir::new(root)
            .follow_links(false)
            .into_iter()
            .filter_entry(|e| e.depth() == 0 || !hidden(e))
            .filter_map(std::result::Result::ok)
            .filter(|e| e.file_type().is_file())
            .map(walkdir::DirEntry::into_path)
            .filter(|p| is_audio(p))
        {
            found.push(entry);
            on(Scan::Walking(found.len()));
        }
        found
    };

    let total = paths.len();
    let done = std::sync::atomic::AtomicUsize::new(0);
    let mut tracks: Vec<Track> = paths
        .par_iter()
        .map(|p| {
            let track = read_track(p);
            let n = done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            on(Scan::Reading(n, total));
            track
        })
        .collect();
    // A playlist already has an order; a directory tree does not.
    if !is_playlist(root) {
        sort(&mut tracks, SortKey::Path);
    }
    Ok(tracks)
}

/// Hidden entries are skipped: a library has no business showing `.git`,
/// `.stfolder` or a trash directory.
///
/// The root itself is exempt at the call site, since a library that lives in a
/// dotted directory would otherwise scan to nothing.
fn hidden(entry: &walkdir::DirEntry) -> bool {
    entry
        .file_name()
        .to_str()
        .is_some_and(|name| name.starts_with('.') && name != ".")
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
    // Only the library itself: a playlist can name files from anywhere, and
    // those directories are not part of this library.
    let mut dirs: BTreeSet<PathBuf> = tracks
        .iter()
        .filter(|t| t.in_library)
        .map(|t| t.dir().to_path_buf())
        .collect();
    // Directories with no tracks in them count: a folder you just made has to
    // show up, or there is nowhere to move anything into.
    for entry in WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| e.depth() == 0 || !hidden(e))
        .filter_map(std::result::Result::ok)
    {
        if entry.file_type().is_dir() && entry.path() != root {
            dirs.insert(entry.into_path());
        }
    }
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

/// Writes an extended m3u. Paths stay absolute so the file still resolves when
/// it is opened from somewhere else.
pub fn write_m3u(path: &Path, tracks: &[&Track]) -> Result<()> {
    use std::io::Write;

    let mut out = String::from("#EXTM3U\n");
    for t in tracks {
        let who = if t.artist.is_empty() {
            String::new()
        } else {
            format!("{} - ", t.artist)
        };
        out.push_str(&format!(
            "#EXTINF:{},{}{}\n{}\n",
            t.duration.as_secs(),
            who,
            t.title,
            t.path.display()
        ));
    }

    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("cannot create `{}`", dir.display()))?;
    }
    std::fs::File::create(path)
        .and_then(|mut f| f.write_all(out.as_bytes()))
        .with_context(|| format!("cannot write playlist `{}`", path.display()))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn track(file: &str, artist: &str, secs: u64, path: &str) -> Track {
        Track {
            path: PathBuf::from(path),
            in_library: true,
            file: file.into(),
            title: file.into(),
            artist: artist.into(),
            album: "Album".into(),
            album_artist: artist.into(),
            track_no: None,
            disc_no: None,
            year: None,
            genre: String::new(),
            duration: Duration::from_secs(secs),
        }
    }

    #[test]
    fn durations_grow_an_hour_field_only_when_they_need_one() {
        assert_eq!(fmt_duration(Duration::from_secs(3)), "0:03");
        assert_eq!(fmt_duration(Duration::from_secs(243)), "4:03");
        assert_eq!(fmt_duration(Duration::from_secs(3723)), "1:02:03");
    }

    #[test]
    fn only_known_audio_extensions_are_scanned() {
        assert!(is_audio(Path::new("/m/a.FLAC")));
        assert!(is_audio(Path::new("/m/a.mp3")));
        assert!(!is_audio(Path::new("/m/cover.jpg")));
        assert!(!is_audio(Path::new("/m/notes")));
        assert!(is_playlist(Path::new("/m/set.m3u")));
        assert!(!is_playlist(Path::new("/m/set.mp3")));
    }

    #[test]
    fn a_search_never_matches_the_directory_a_track_sits_in() {
        let hay = track("Bonfire", "Knife Party", 150, "/m/TESTING/Bonfire.mp3").haystack();
        assert!(hay.contains("Bonfire"));
        assert!(!hay.to_lowercase().contains("testing"));
    }

    #[test]
    fn a_written_playlist_reads_back_to_the_same_paths() {
        let dir = tempfile::tempdir().unwrap();
        let m3u = dir.path().join("set.m3u");
        let tracks = [
            track("a", "x", 61, "/m/a.mp3"),
            track("b", "y", 2, "/m/b.mp3"),
        ];
        let refs: Vec<&Track> = tracks.iter().collect();
        write_m3u(&m3u, &refs).unwrap();
        assert_eq!(
            read_m3u(&m3u).unwrap(),
            vec![PathBuf::from("/m/a.mp3"), PathBuf::from("/m/b.mp3")]
        );
    }

    #[test]
    fn an_empty_playlist_is_written_and_reads_back_empty() {
        let dir = tempfile::tempdir().unwrap();
        let m3u = dir.path().join("empty.m3u");
        write_m3u(&m3u, &[]).unwrap();
        assert!(read_m3u(&m3u).unwrap().is_empty());
    }

    #[test]
    fn folder_labels_are_relative_to_the_library_root() {
        let tracks = vec![
            track("a", "x", 1, "/m/Bjork/Post/01.flac"),
            track("b", "x", 1, "/m/Bjork/Post/02.flac"),
            track("c", "y", 1, "/m/Air/Moon/01.flac"),
        ];
        let labels: Vec<String> = folders(&tracks, Path::new("/m"))
            .into_iter()
            .map(|(label, _)| label)
            .collect();
        assert_eq!(labels, vec!["Air/Moon", "Bjork/Post"]);
    }

    #[test]
    fn sorting_by_title_ignores_case() {
        let mut tracks = vec![
            track("zeta", "a", 1, "/m/1.mp3"),
            track("Alpha", "b", 2, "/m/2.mp3"),
        ];
        sort(&mut tracks, SortKey::Title);
        assert_eq!(tracks[0].title, "Alpha");
    }

    #[test]
    fn a_scan_of_a_missing_root_names_the_path() {
        let err = scan(Path::new("/nope/nowhere")).unwrap_err().to_string();
        assert!(err.contains("/nope/nowhere"), "{err}");
    }

    #[test]
    fn hidden_directories_are_not_scanned() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".hidden")).unwrap();
        std::fs::write(dir.path().join(".hidden/x.mp3"), b"").unwrap();
        std::fs::write(dir.path().join("y.mp3"), b"").unwrap();
        let found = scan(dir.path()).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].file, "y");
    }
}
