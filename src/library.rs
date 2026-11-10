//! The library: files on disk, their tags, and the sort orders we present them in.
//!
//! vibox never owns a database. A library is a directory, a track is a file, a
//! playlist is an m3u. Everything here is a pure function of what is on disk.

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use rayon::prelude::*;
use walkdir::WalkDir;

/// Extensions that make a file a track. Anything else in the tree is ignored.
///
/// Wider than what the decoder can play: an opus or wavpack file is still part
/// of the library you rename and move, and `player::open` says why it will not
/// play rather than the file vanishing from the list.
pub const AUDIO_EXTS: [&str; 8] = ["mp3", "flac", "ogg", "opus", "m4a", "aac", "wav", "wv"];

/// `path` made absolute against the working directory, with `.` and `..`
/// folded away, without asking the filesystem anything.
///
/// Not `canonicalize`: a library reached through a symlink keeps the path it
/// was opened by, which is the path its playlists name.
pub fn absolute(path: &Path) -> PathBuf {
    let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    tidy(&path)
}

/// `path` written relative to the folder `base`, climbing out with `..` where
/// it has to: how a relative m3u entry names a file outside the playlist's own
/// folder. Both are absolute and tidy.
pub fn relative_to(base: &Path, path: &Path) -> PathBuf {
    let base: Vec<Component> = base.components().collect();
    let parts: Vec<Component> = path.components().collect();
    let common = base.iter().zip(&parts).take_while(|(a, b)| a == b).count();
    let mut out = PathBuf::new();
    for _ in common..base.len() {
        out.push("..");
    }
    for part in &parts[common..] {
        out.push(part);
    }
    out
}

/// `a/./b/../c` as `a/c`, lexically.
fn tidy(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => match out.components().next_back() {
                Some(Component::Normal(_)) => {
                    out.pop();
                }
                // Above the root is the root.
                Some(Component::RootDir | Component::Prefix(_)) => {}
                _ => out.push(part),
            },
            other => out.push(other),
        }
    }
    out
}

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
/// A symlink to a file is a track like any other, but a symlink to a folder is
/// not walked into: a music tree with a loop in it should not hang the player.
/// Unreadable entries are skipped rather than failing the scan.
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
            .filter(|e| e.file_type().is_file() || (e.path_is_symlink() && e.path().is_file()))
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
    tracks.sort_by(|a, b| compare(a, b, key));
}

/// The order `sort` puts two tracks in, for sorting anything that refers to
/// tracks rather than the tracks themselves.
pub fn compare(a: &Track, b: &Track, key: SortKey) -> std::cmp::Ordering {
    match key {
        SortKey::Path => a.path.cmp(&b.path),
        SortKey::Title => (lower(&a.title), &a.path).cmp(&(lower(&b.title), &b.path)),
        SortKey::Artist => (lower(&a.artist), lower(&a.album), a.disc_no, a.track_no).cmp(&(
            lower(&b.artist),
            lower(&b.album),
            b.disc_no,
            b.track_no,
        )),
        SortKey::Album => (
            lower(&a.album_artist),
            lower(&a.album),
            a.disc_no,
            a.track_no,
        )
            .cmp(&(
                lower(&b.album_artist),
                lower(&b.album),
                b.disc_no,
                b.track_no,
            )),
        SortKey::Duration => (a.duration, &a.path).cmp(&(b.duration, &b.path)),
    }
}

/// The directories that actually contain tracks, as shown in the left pane.
///
/// `root` is the library as it was opened. When that is an m3u or a single
/// file, the folders are the ones its tracks sit in and the labels are relative
/// to the folder holding it; nothing else beside it is walked, since an m3u
/// kept in a home directory would otherwise list, and read, the whole home.
pub fn folders(tracks: &[Track], root: &Path) -> Vec<(String, PathBuf)> {
    let walked = !(is_playlist(root) || root.is_file());
    let root = if walked {
        root
    } else {
        root.parent().unwrap_or(root)
    };
    // Only the library itself: a playlist can name files from anywhere, and
    // those directories are not part of this library.
    let mut dirs: BTreeSet<PathBuf> = tracks
        .iter()
        .filter(|t| t.in_library)
        .map(|t| t.dir().to_path_buf())
        .collect();
    // Directories with no tracks in them count: a folder you just made has to
    // show up, or there is nowhere to move anything into.
    if walked {
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

    let mut out = b"#EXTM3U\n".to_vec();
    for t in tracks {
        out.extend_from_slice(&m3u_entry(t, t.path.as_path(), b"\n"));
    }

    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| {
            anyhow!(
                "cannot create `{}`: {}",
                dir.display(),
                crate::reason::of(&e)
            )
        })?;
    }
    std::fs::File::create(path)
        .and_then(|mut f| f.write_all(&out))
        .map_err(|e| {
            anyhow!(
                "cannot write playlist `{}`: {}",
                path.display(),
                crate::reason::of(&e)
            )
        })
}

/// The `#EXTINF` line and the path line of one track, the path written as
/// `shown` and each line ended with `eol`.
fn m3u_entry(t: &Track, shown: &Path, eol: &[u8]) -> Vec<u8> {
    let who = if t.artist.is_empty() {
        String::new()
    } else {
        format!("{} - ", t.artist)
    };
    let mut out = format!("#EXTINF:{},{}{}", t.duration.as_secs(), who, t.title).into_bytes();
    out.extend_from_slice(eol);
    // The path as the bytes it is on disk, which `display` would mangle for a
    // name that is not UTF-8.
    out.extend_from_slice(shown.as_os_str().as_bytes());
    out.extend_from_slice(eol);
    out
}

/// The line ending an m3u already uses, so lines added to it match.
fn eol_of(bytes: &[u8]) -> &'static [u8] {
    if bytes.windows(2).any(|w| w == b"\r\n") {
        b"\r\n"
    } else {
        b"\n"
    }
}

fn read_playlist(path: &Path) -> Result<Vec<u8>> {
    std::fs::read(path).map_err(|e| {
        anyhow!(
            "cannot read playlist `{}`: {}",
            path.display(),
            crate::reason::of(&e)
        )
    })
}

fn write_playlist(path: &Path, bytes: &[u8]) -> Result<()> {
    std::fs::write(path, bytes).map_err(|e| {
        anyhow!(
            "cannot write playlist `{}`: {}",
            path.display(),
            crate::reason::of(&e)
        )
    })
}

/// Saves an edited playlist over its file, changing only what the edit
/// changed. An entry that stays keeps its lines exactly, comments and lines
/// this program does not read stay where they are, an entry taken out goes
/// with its `#EXTINF` line, one that only moved takes its own lines along, and
/// a new one is written the way `write_m3u` writes. A playlist named by
/// hand is full of things nothing here understands, and an edit rebuilt from
/// the rows dropped all of them. A file that is not there yet is written new.
pub fn save_m3u(path: &Path, tracks: &[&Track]) -> Result<()> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return write_m3u(path, tracks),
        Err(_) => read_playlist(path)?,
    };
    let eol = eol_of(&bytes);
    let base = path.parent().unwrap_or(Path::new(""));
    let lines: Vec<&[u8]> = m3u_lines(&bytes).collect();

    // Every entry: its line, the `#EXTINF` line that introduces it, its file.
    let mut old: Vec<(usize, Option<usize>, PathBuf)> = Vec::new();
    let mut info = None;
    for (at, raw) in lines.iter().enumerate() {
        let line = raw.trim_ascii();
        if line.starts_with(b"#EXTINF:") {
            info = Some(at);
        } else if !line.is_empty() && !line.starts_with(b"#") {
            old.push((at, info.take(), entry_path(base, line)));
        }
    }
    let new: Vec<&Path> = tracks.iter().map(|t| t.path.as_path()).collect();
    let (kept_old, kept_new) =
        kept_in_order(&old.iter().map(|o| o.2.as_path()).collect::<Vec<_>>(), &new);

    // The lines of entries that leave their place, by file, so one that only
    // moved is written with its own lines where it went.
    let mut spare: std::collections::HashMap<&Path, std::collections::VecDeque<usize>> =
        std::collections::HashMap::new();
    let mut skip = vec![false; lines.len()];
    for (i, (at, info, file)) in old.iter().enumerate() {
        if kept_old[i].is_none() {
            spare.entry(file.as_path()).or_default().push_back(i);
            skip[*at] = true;
            if let Some(info) = info {
                skip[*info] = true;
            }
        }
    }
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    out.extend_from_slice(bom_of(&bytes));
    let push_line = |out: &mut Vec<u8>, raw: &[u8]| {
        out.extend_from_slice(raw);
        if !raw.ends_with(b"\n") {
            out.extend_from_slice(eol);
        }
    };
    let mut emit = |out: &mut Vec<u8>, j: usize| match spare
        .get_mut(new[j])
        .and_then(std::collections::VecDeque::pop_front)
    {
        Some(i) => {
            if let Some(info) = old[i].1 {
                push_line(out, lines[info]);
            }
            push_line(out, lines[old[i].0]);
        }
        None => out.extend_from_slice(&m3u_entry(tracks[j], new[j], eol)),
    };

    // Where each kept entry's lines start, so the rows that come before it in
    // the edit are written in front of them.
    let starts: std::collections::HashMap<usize, usize> = old
        .iter()
        .enumerate()
        .filter(|(i, _)| kept_old[*i].is_some())
        .map(|(i, (at, info, _))| (info.unwrap_or(*at), i))
        .collect();
    let ends: std::collections::HashMap<usize, usize> = old
        .iter()
        .enumerate()
        .filter(|(i, _)| kept_old[*i].is_some())
        .map(|(i, (at, _, _))| (*at, i))
        .collect();
    // With nothing kept, the new rows go where the first entry was.
    let first = old.first().map(|(at, info, _)| info.unwrap_or(*at));
    let nothing_kept = kept_old.iter().all(Option::is_none);
    let mut next = 0;
    for (at, raw) in lines.iter().enumerate() {
        if let Some(&i) = starts.get(&at)
            && let Some(j) = kept_old[i]
        {
            while next < j {
                emit(&mut out, next);
                next += 1;
            }
        }
        if nothing_kept && first == Some(at) {
            while next < new.len() {
                emit(&mut out, next);
                next += 1;
            }
        }
        if !skip[at] {
            push_line(&mut out, raw);
        }
        if let Some(&i) = ends.get(&at)
            && let Some(j) = kept_old[i]
        {
            next = j + 1;
            while next < new.len() && kept_new[next].is_none() {
                emit(&mut out, next);
                next += 1;
            }
        }
    }
    while next < new.len() {
        emit(&mut out, next);
        next += 1;
    }
    write_playlist(path, &out)
}

/// Which entries of `old` stay where they are as entries of `new`: the
/// longest run of them that keeps its order, as `kept_old[i] = Some(j)` and
/// `kept_new[j] = Some(i)`.
///
/// The shared start and end are matched first, which is all a `dd` or two
/// needs. The rest is matched from the pairs of entries that name one file
/// (`rising_pairs`) while there are about as many of those as entries, and by
/// Myers' diff (`shortest_edit`) once a file named many times on both sides
/// makes them grow with the square of the length, since a playlist may name
/// one track thousands of times.
fn kept_in_order(old: &[&Path], new: &[&Path]) -> (Vec<Option<usize>>, Vec<Option<usize>>) {
    let (mut a0, mut a1, mut b0, mut b1) = (0, old.len(), 0, new.len());
    let mut pairs = Vec::new();
    while a0 < a1 && b0 < b1 && old[a0] == new[b0] {
        pairs.push((a0, b0));
        a0 += 1;
        b0 += 1;
    }
    while a0 < a1 && b0 < b1 && old[a1 - 1] == new[b1 - 1] {
        a1 -= 1;
        b1 -= 1;
        pairs.push((a1, b1));
    }
    let (a, b) = (&old[a0..a1], &new[b0..b1]);
    let mut count: std::collections::HashMap<&Path, usize> = std::collections::HashMap::new();
    for file in a {
        *count.entry(file).or_default() += 1;
    }
    let matching: usize = b
        .iter()
        .map(|file| count.get(file).copied().unwrap_or(0))
        .sum();
    let middle = if matching <= 8 * (a.len() + b.len()) {
        rising_pairs(a, b)
    } else {
        shortest_edit(a, b)
    };
    pairs.extend(middle.into_iter().map(|(i, j)| (a0 + i, b0 + j)));

    let mut kept_old = vec![None; old.len()];
    let mut kept_new = vec![None; new.len()];
    for (i, j) in pairs {
        kept_old[i] = Some(j);
        kept_new[j] = Some(i);
    }
    (kept_old, kept_new)
}

/// The longest chain of pairs `(i, j)` with `a[i] == b[j]` that rises in
/// both, built from those pairs rather than from a table of every pair, so
/// it costs about as much as there are pairs.
fn rising_pairs(a: &[&Path], b: &[&Path]) -> Vec<(usize, usize)> {
    let mut at: std::collections::HashMap<&Path, Vec<usize>> = std::collections::HashMap::new();
    for (i, file) in a.iter().enumerate() {
        at.entry(file).or_default().push(i);
    }
    // `links[k]` is one pair `(i, j)` and the pair before it in its chain;
    // `ends[len]` is the link ending the chain of `len + 1` pairs whose last
    // `i` is smallest. Within one `j` the `i`s go highest first, so no chain
    // takes two pairs of one entry.
    let mut links: Vec<(usize, usize, Option<usize>)> = Vec::new();
    let mut ends: Vec<usize> = Vec::new();
    for (j, file) in b.iter().enumerate() {
        for &i in at.get(file).into_iter().flatten().rev() {
            let len = ends.partition_point(|&k| links[k].0 < i);
            links.push((i, j, len.checked_sub(1).map(|l| ends[l])));
            let link = links.len() - 1;
            match ends.get_mut(len) {
                Some(end) => *end = link,
                None => ends.push(link),
            }
        }
    }
    let mut pairs = Vec::new();
    let mut next = ends.last().copied();
    while let Some(k) = next {
        let (i, j, before) = links[k];
        pairs.push((i, j));
        next = before;
    }
    pairs
}

/// The pairs a shortest edit from `a` to `b` keeps, by Myers' diff in linear
/// space: the shared start and end are kept, the rest is split where the
/// edit crosses its middle (`middle`), and each half is done the same way.
/// Time grows with the length times the entries that differ, memory with the
/// length alone.
fn shortest_edit(a: &[&Path], b: &[&Path]) -> Vec<(usize, usize)> {
    let mut pairs = Vec::new();
    let mut todo = vec![(0, a.len(), 0, b.len())];
    while let Some((mut a0, mut a1, mut b0, mut b1)) = todo.pop() {
        while a0 < a1 && b0 < b1 && a[a0] == b[b0] {
            pairs.push((a0, b0));
            a0 += 1;
            b0 += 1;
        }
        while a0 < a1 && b0 < b1 && a[a1 - 1] == b[b1 - 1] {
            a1 -= 1;
            b1 -= 1;
            pairs.push((a1, b1));
        }
        if a0 == a1 || b0 == b1 {
            continue;
        }
        let (n, m) = (a1 - a0, b1 - b0);
        if let Some((x, y)) = middle(&a[a0..a1], &b[b0..b1])
            && x + y > 0
            && (x, y) != (n, m)
        {
            todo.push((a0, a0 + x, b0, b0 + y));
            todo.push((a0 + x, a1, b0 + y, b1));
        }
    }
    pairs
}

/// A point `(x, y)` on a shortest edit from `a` to `b` that splits it in
/// two, found by walking from both ends at once until the walks meet, or
/// `None` when the two share no entry at all. `a` and `b` are not empty and
/// differ at both ends.
fn middle(a: &[&Path], b: &[&Path]) -> Option<(usize, usize)> {
    let (n, m) = (a.len() as isize, b.len() as isize);
    let max_d = (n + m + 1) / 2;
    let offset = max_d;
    let size = (2 * max_d + 2) as usize;
    // `ahead[offset + k]` is how far along `a` the walk from the start gets on
    // diagonal `k` (`x - y`); `back` the same for the walk from the end.
    let mut ahead = vec![-1isize; size];
    let mut back = vec![-1isize; size];
    ahead[(offset + 1) as usize] = 0;
    back[(offset + 1) as usize] = 0;
    let delta = n - m;
    // With an odd difference in length the walk from the start is the one
    // that runs into the other.
    let front = delta % 2 != 0;
    let (mut k1start, mut k1end, mut k2start, mut k2end) = (0, 0, 0, 0);
    for d in 0..max_d {
        let mut k1 = -d + k1start;
        while k1 <= d - k1end {
            let at = (offset + k1) as usize;
            let mut x1 = if k1 == -d || (k1 != d && ahead[at - 1] < ahead[at + 1]) {
                ahead[at + 1]
            } else {
                ahead[at - 1] + 1
            };
            let mut y1 = x1 - k1;
            while x1 < n && y1 < m && a[x1 as usize] == b[y1 as usize] {
                x1 += 1;
                y1 += 1;
            }
            ahead[at] = x1;
            if x1 > n {
                k1end += 2;
            } else if y1 > m {
                k1start += 2;
            } else if front {
                let other = offset + delta - k1;
                if other >= 0
                    && (other as usize) < size
                    && back[other as usize] != -1
                    && x1 >= n - back[other as usize]
                {
                    return Some((x1 as usize, y1 as usize));
                }
            }
            k1 += 2;
        }
        let mut k2 = -d + k2start;
        while k2 <= d - k2end {
            let at = (offset + k2) as usize;
            let mut x2 = if k2 == -d || (k2 != d && back[at - 1] < back[at + 1]) {
                back[at + 1]
            } else {
                back[at - 1] + 1
            };
            let mut y2 = x2 - k2;
            while x2 < n && y2 < m && a[(n - x2 - 1) as usize] == b[(m - y2 - 1) as usize] {
                x2 += 1;
                y2 += 1;
            }
            back[at] = x2;
            if x2 > n {
                k2end += 2;
            } else if y2 > m {
                k2start += 2;
            } else if !front {
                let other = offset + delta - k2;
                if other >= 0 && (other as usize) < size && ahead[other as usize] != -1 {
                    let x1 = ahead[other as usize];
                    let y1 = offset + x1 - other;
                    if x1 >= n - x2 {
                        return Some((x1 as usize, y1 as usize));
                    }
                }
            }
            k2 += 2;
        }
    }
    None
}

/// Adds tracks to the end of an m3u, written the way its other entries are:
/// relative to its folder when they are, absolute otherwise.
pub fn append_m3u(path: &Path, tracks: &[&Track]) -> Result<()> {
    let mut bytes = read_playlist(path)?;
    let eol = eol_of(&bytes);
    let base = path.parent().unwrap_or(Path::new("")).to_path_buf();
    let relative = m3u_lines(&bytes)
        .map(<[u8]>::trim_ascii)
        .find(|line| !line.is_empty() && !line.starts_with(b"#"))
        .is_some_and(|line| !Path::new(OsStr::from_bytes(line)).is_absolute());
    if !bytes.is_empty() && !bytes.ends_with(b"\n") {
        bytes.extend_from_slice(eol);
    }
    for t in tracks {
        let shown = if relative {
            relative_to(&base, &t.path)
        } else {
            t.path.clone()
        };
        bytes.extend_from_slice(&m3u_entry(t, &shown, eol));
    }
    write_playlist(path, &bytes)
}

/// One entry of an m3u: the file, and what its `#EXTINF` line said about it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub path: PathBuf,
    /// What followed `#EXTINF:`, as `seconds,artist - title`.
    pub info: Option<String>,
}

/// Reads the file paths out of an m3u, ignoring directives and comments.
pub fn read_m3u(path: &Path) -> Result<Vec<PathBuf>> {
    Ok(read_m3u_entries(path)?
        .into_iter()
        .map(|entry| entry.path)
        .collect())
}

/// The entries of an m3u, in order.
///
/// Read as bytes rather than as text, because a path on disk is bytes: an m3u
/// written on a Latin-1 system names its files in Latin-1, and decoding it
/// first would name files that do not exist. A relative entry is relative to
/// the folder the playlist is in, which is where every player that writes one
/// expects it to be read from.
pub fn read_m3u_entries(path: &Path) -> Result<Vec<Entry>> {
    let bytes = std::fs::read(path).map_err(|e| {
        anyhow!(
            "cannot read playlist `{}`: {}",
            path.display(),
            crate::reason::of(&e)
        )
    })?;
    let base = path.parent().unwrap_or(Path::new(""));
    let mut entries = Vec::new();
    let mut info = None;
    for line in m3u_lines(&bytes) {
        let line = line.trim_ascii();
        if let Some(rest) = line.strip_prefix(b"#EXTINF:") {
            info = Some(String::from_utf8_lossy(rest).into_owned());
        } else if !line.is_empty() && !line.starts_with(b"#") {
            entries.push(Entry {
                path: entry_path(base, line),
                info: info.take(),
            });
        }
    }
    Ok(entries)
}

/// The lines of an m3u with their line endings left on, and a UTF-8 byte
/// order mark taken off the first one.
fn m3u_lines(bytes: &[u8]) -> impl Iterator<Item = &[u8]> {
    bytes[bom_of(bytes).len()..].split_inclusive(|&b| b == b'\n')
}

/// The UTF-8 byte order mark an m3u starts with, if it starts with one, so a
/// file rewritten from its lines starts with it again.
fn bom_of(bytes: &[u8]) -> &'static [u8] {
    if bytes.starts_with(b"\xef\xbb\xbf") {
        b"\xef\xbb\xbf"
    } else {
        b""
    }
}

fn entry_path(base: &Path, line: &[u8]) -> PathBuf {
    let path = Path::new(OsStr::from_bytes(line));
    if path.is_absolute() {
        tidy(path)
    } else {
        tidy(&base.join(path))
    }
}

/// What a repair does to one entry of a playlist.
pub enum Fix {
    Keep,
    /// The file is now here.
    Moved(PathBuf),
    /// The file is gone, so the entry goes with it.
    Drop,
}

/// Rewrites the entries of the m3u at `path` that `fix` says changed, and
/// leaves every other byte of the file exactly as it was.
///
/// Nothing else is regenerated: a playlist names files that are not in this
/// library, files on a drive that is not mounted and lines this program does not
/// understand, and a repair rebuilt from what vibox has loaded would drop all of
/// them. An entry that goes takes its `#EXTINF` line with it. Returns whether
/// anything changed; the file is only written then.
pub fn repair_m3u(path: &Path, fix: &dyn Fn(&Path) -> Fix) -> Result<bool> {
    let bytes = std::fs::read(path).map_err(|e| {
        anyhow!(
            "cannot read playlist `{}`: {}",
            path.display(),
            crate::reason::of(&e)
        )
    })?;
    let base = path.parent().unwrap_or(Path::new(""));

    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    out.extend_from_slice(bom_of(&bytes));
    // The lines since the last entry, held back until we know whether the
    // entry they introduce stays.
    let mut held: Vec<&[u8]> = Vec::new();
    let mut changed = false;
    for raw in m3u_lines(&bytes) {
        let line = raw.trim_ascii();
        if line.is_empty() || line.starts_with(b"#") {
            held.push(raw);
            continue;
        }
        match fix(&entry_path(base, line)) {
            Fix::Keep => {
                held.drain(..).for_each(|l| out.extend_from_slice(l));
                out.extend_from_slice(raw);
            }
            Fix::Moved(to) => {
                held.drain(..).for_each(|l| out.extend_from_slice(l));
                let shown = if Path::new(OsStr::from_bytes(line)).is_absolute() {
                    to.clone()
                } else {
                    relative_to(base, &to)
                };
                out.extend_from_slice(shown.as_os_str().as_bytes());
                out.extend_from_slice(&raw[raw.trim_ascii_end().len()..]);
                changed = true;
            }
            Fix::Drop => {
                held.drain(..)
                    .filter(|l| !l.trim_ascii().starts_with(b"#EXTINF"))
                    .for_each(|l| out.extend_from_slice(l));
                changed = true;
            }
        }
    }
    held.drain(..).for_each(|l| out.extend_from_slice(l));

    if changed {
        std::fs::write(path, out).map_err(|e| {
            anyhow!(
                "cannot write playlist `{}`: {}",
                path.display(),
                crate::reason::of(&e)
            )
        })?;
    }
    Ok(changed)
}

/// A track for an entry whose file cannot be read, built from what the
/// playlist said about it, so a playlist keeps it through an edit and a save.
pub fn missing(entry: &Entry) -> Track {
    let stem = entry
        .path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let (seconds, said) = entry
        .info
        .as_deref()
        .and_then(|info| info.split_once(','))
        .unwrap_or(("0", ""));
    let (artist, title) = said.split_once(" - ").unwrap_or(("", said));
    Track {
        path: entry.path.clone(),
        in_library: false,
        file: stem.clone(),
        title: if title.trim().is_empty() {
            stem
        } else {
            title.trim().to_string()
        },
        artist: artist.trim().to_string(),
        album: String::new(),
        album_artist: artist.trim().to_string(),
        track_no: None,
        disc_no: None,
        year: None,
        genre: String::new(),
        duration: Duration::from_secs(seconds.trim().parse().unwrap_or(0)),
    }
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
        let hay = track("Ember", "Paper Coast", 150, "/m/TESTING/Ember.mp3").haystack();
        assert!(hay.contains("Ember"), "the row's own words are searched");
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
            track("a", "x", 1, "/m/Paper Coast/Shore/01.flac"),
            track("b", "x", 1, "/m/Paper Coast/Shore/02.flac"),
            track("c", "y", 1, "/m/Lumen Drift/Tide Charts/01.flac"),
        ];
        let labels: Vec<String> = folders(&tracks, Path::new("/m"))
            .into_iter()
            .map(|(label, _)| label)
            .collect();
        assert_eq!(labels, vec!["Lumen Drift/Tide Charts", "Paper Coast/Shore"]);
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
