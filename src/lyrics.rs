//! Lyrics from lrclib.net, fetched off the ui thread and cached on disk.
//!
//! lrclib needs no api key and matches on artist, title, album and duration,
//! which is exactly what a tagged file already carries. Synced lyrics come back
//! in lrc format, so the pane can follow the playback position.
//!
//! Nothing here ever blocks the ui: a miss queues a request and the pane says
//! so until the worker answers.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use crate::library::Track;

/// Stamped on every cache entry. Bump it when what gets cached changes
/// meaning: entries without the current marker are refetched rather than
/// trusted, since an old one cannot say whether its timings were believed.
const CACHE_MARK: &str = "[vibox:3]";

/// Who the words came from, for the pane to credit. One provider today; when
/// there are two this moves onto `Lyrics` so each result carries its own.
pub const SOURCE: &str = "lrclib";

const AGENT: &str = concat!(
    "vibox/",
    env!("CARGO_PKG_VERSION"),
    " (https://gitlab.com/safteinzz/vibox)"
);

pub enum Lyrics {
    /// Timestamped lines, in order.
    Synced(Vec<(Duration, String)>),
    Plain(Vec<String>),
    /// Nothing to show, and the reason to put in the pane.
    Missing(String),
}

/// What the worker needs to ask lrclib for one track.
struct Request {
    path: PathBuf,
    artist: String,
    title: String,
    album: String,
    duration: u64,
}

pub struct Fetcher {
    tx: Sender<Request>,
    rx: Receiver<(PathBuf, Lyrics, i64)>,
    cache: HashMap<PathBuf, Lyrics>,
    /// Per file correction in milliseconds, positive meaning the words come
    /// later. A rip with a different lead-in needs this and no lookup can.
    offsets: HashMap<PathBuf, i64>,
    inflight: HashSet<PathBuf>,
}

impl Fetcher {
    pub fn new() -> Fetcher {
        let (tx, jobs) = channel::<Request>();
        let (results, rx) = channel();

        // One worker: lyrics are not urgent and a queue keeps lrclib happy.
        let _ = std::thread::Builder::new()
            .name("vibox-lyrics".into())
            .spawn(move || {
                while let Ok(job) = jobs.recv() {
                    let (found, offset) = load_cached(&job.path).unwrap_or_else(|| {
                        let fetched = fetch(&job);
                        store_cached(&job.path, &fetched, 0);
                        (fetched, 0)
                    });
                    if results.send((job.path, found, offset)).is_err() {
                        return;
                    }
                }
            });

        Fetcher {
            tx,
            rx,
            cache: HashMap::new(),
            offsets: HashMap::new(),
            inflight: HashSet::new(),
        }
    }

    /// Queues a fetch unless the track is already cached or already queued.
    pub fn request(&mut self, track: &Track) {
        if self.cache.contains_key(&track.path) || self.inflight.contains(&track.path) {
            return;
        }
        if track.title.trim().is_empty() && track.artist.trim().is_empty() {
            self.cache.insert(
                track.path.clone(),
                Lyrics::Missing("no artist or title tag to search with".into()),
            );
            return;
        }

        self.inflight.insert(track.path.clone());
        let _ = self.tx.send(Request {
            path: track.path.clone(),
            artist: track.artist.clone(),
            title: track.title.clone(),
            album: track.album.clone(),
            duration: track.duration.as_secs(),
        });
    }

    /// Moves whatever the worker finished into the cache. Called every tick.
    pub fn poll(&mut self) {
        while let Ok((path, lyrics, offset)) = self.rx.try_recv() {
            self.inflight.remove(&path);
            self.offsets.insert(path.clone(), offset);
            self.cache.insert(path, lyrics);
        }
    }

    pub fn offset(&self, path: &Path) -> i64 {
        self.offsets.get(path).copied().unwrap_or(0)
    }

    /// Shifts this file's lyrics and writes the correction into its cache
    /// entry, so the track stays in sync every time it is played.
    pub fn nudge(&mut self, path: &Path, delta_ms: i64) -> i64 {
        let offset = self.offset(path) + delta_ms;
        self.offsets.insert(path.to_path_buf(), offset);
        if let Some(lyrics) = self.cache.get(path) {
            store_cached(path, lyrics, offset);
        }
        offset
    }

    pub fn get(&self, path: &Path) -> Option<&Lyrics> {
        self.cache.get(path)
    }

    pub fn is_loading(&self, path: &Path) -> bool {
        self.inflight.contains(path)
    }

    /// Throws away every cached lyric, on disk and in memory, so the next play
    /// of each track asks lrclib again.
    ///
    /// Only `.lrc` files directly inside the cache directory are removed, and
    /// nothing else in the data directory is touched. Returns how many went.
    pub fn clear(&mut self) -> std::io::Result<usize> {
        self.cache.clear();
        self.offsets.clear();

        let Some(dir) = cache_dir() else {
            return Ok(0);
        };
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            // Never fetched anything yet: nothing to clear is not a failure.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(e) => return Err(e),
        };

        let mut gone = 0;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "lrc") && std::fs::remove_file(&path).is_ok() {
                gone += 1;
            }
        }
        Ok(gone)
    }
}

impl Default for Fetcher {
    fn default() -> Self {
        Fetcher::new()
    }
}

// ---- network ------------------------------------------------------------

fn fetch(job: &Request) -> Lyrics {
    let exact = format!(
        "https://lrclib.net/api/get?artist_name={}&track_name={}&album_name={}&duration={}",
        encode(&job.artist),
        encode(&job.title),
        encode(&job.album),
        job.duration
    );

    match get_json(&exact) {
        // lrclib matches duration loosely, so it will answer with a different
        // edit of the same song. Check the duration ourselves before believing
        // its timestamps.
        Ok(body) => {
            let trust = gap(&body, job.duration) <= SYNC_TOLERANCE;
            return from_json(&body, job.duration, trust);
        }
        // A miss on the exact match is normal: the duration or the album
        // rarely lines up with what someone else uploaded.
        Err(FetchError::NotFound) => {}
        Err(FetchError::Other(e)) => return Lyrics::Missing(e),
    }

    let search = format!(
        "https://lrclib.net/api/search?artist_name={}&track_name={}",
        encode(&job.artist),
        encode(&job.title)
    );
    match get_json(&search) {
        Ok(body) => match pick(&body, job) {
            Some(hit) => {
                let trust = gap(&hit, job.duration) <= SYNC_TOLERANCE;
                from_json(&hit, job.duration, trust)
            }
            None => Lyrics::Missing("no lyrics on lrclib for this track".into()),
        },
        Err(FetchError::NotFound) => Lyrics::Missing("no lyrics on lrclib for this track".into()),
        Err(FetchError::Other(e)) => Lyrics::Missing(e),
    }
}

/// How far a hit's duration may be from ours before its timestamps belong to
/// another edit. Seconds of difference show up as seconds of lag.
const SYNC_TOLERANCE: f64 = 2.0;

/// True when the last line is timed past the end of the track, allowing a few
/// seconds for a fade or a sloppy final timestamp.
fn runs_over(lines: &[(Duration, String)], ours: u64) -> bool {
    if ours == 0 {
        return false;
    }
    lines.last().is_some_and(|(at, _)| at.as_secs() > ours + 5)
}

/// How far a hit's artist or title may be from ours, as a share of the longer
/// of the two. A remaster suffix or a stray accent stays under it; a different
/// song does not.
const NAME_TOLERANCE: f64 = 0.3;

/// Picks the hit closest in duration, out of the ones that are actually this
/// song.
///
/// lrclib's search is a substring match on both fields, so asking for `Wax` and
/// `Destiny` also answers with `Nightmares on Wax - Date With Destiny`. Ranking
/// those on duration alone picks whichever wrong song happens to run about as
/// long as ours, which is how a confidently wrong lyric sheet gets synced to a
/// track. The name has to agree before the duration is worth consulting.
fn pick(body: &serde_json::Value, job: &Request) -> Option<serde_json::Value> {
    let hits = body.as_array()?;
    let ours = job.duration;
    hits.iter()
        .filter(|hit| has_lyrics(hit))
        // An empty tag cannot disagree with anything, so it does not get a say.
        .filter(|hit| job.artist.is_empty() || close(&text_of(hit, "artistName"), &job.artist))
        .filter(|hit| job.title.is_empty() || close(&text_of(hit, "trackName"), &job.title))
        .min_by(|a, b| gap(a, ours).total_cmp(&gap(b, ours)))
        .cloned()
}

/// Two names for the same song, allowing for case, punctuation and whatever is
/// in the brackets.
fn close(theirs: &str, ours: &str) -> bool {
    let (theirs, ours) = (normalize(theirs), normalize(ours));
    if theirs.is_empty() || ours.is_empty() {
        return false;
    }
    let longest = theirs.chars().count().max(ours.chars().count());
    distance(&theirs, &ours) as f64 / longest as f64 <= NAME_TOLERANCE
}

/// Lowercase, without bracketed asides or punctuation: `Destiny (Original
/// Version)` and `destiny` are the same title written twice.
fn normalize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut depth = 0usize;
    let mut space = true;
    for c in s.chars() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth = depth.saturating_sub(1),
            _ if depth > 0 => {}
            _ if c.is_alphanumeric() => {
                out.extend(c.to_lowercase());
                space = false;
            }
            _ if !space => {
                out.push(' ');
                space = true;
            }
            _ => {}
        }
    }
    out.trim_end().to_string()
}

/// Levenshtein distance, two rows at a time. Titles are short, so the naive
/// version is free.
fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0; b.len() + 1];
    for (i, ca) in a.chars().enumerate() {
        cur[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != *cb);
            cur[j + 1] = (prev[j + 1] + 1).min(cur[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

fn gap(hit: &serde_json::Value, ours: u64) -> f64 {
    hit.get("duration")
        .and_then(serde_json::Value::as_f64)
        .map_or(f64::MAX, |theirs| (theirs - ours as f64).abs())
}

enum FetchError {
    NotFound,
    Other(String),
}

fn get_json(url: &str) -> Result<serde_json::Value, FetchError> {
    let response = ureq::get(url)
        .header("User-Agent", AGENT)
        .call()
        .map_err(|e| match e {
            ureq::Error::StatusCode(404) => FetchError::NotFound,
            other => FetchError::Other(format!("lrclib: {other}")),
        })?;

    response
        .into_body()
        .read_to_string()
        .map_err(|e| FetchError::Other(format!("lrclib: {e}")))
        .and_then(|text| {
            serde_json::from_str(&text)
                .map_err(|e| FetchError::Other(format!("lrclib sent something unreadable: {e}")))
        })
}

fn has_lyrics(hit: &serde_json::Value) -> bool {
    !text_of(hit, "syncedLyrics").is_empty() || !text_of(hit, "plainLyrics").is_empty()
}

fn text_of(value: &serde_json::Value, key: &str) -> String {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// `trust_timing` false means the words are right but the timestamps belong to
/// another release, so they are shown without a following highlight.
fn from_json(body: &serde_json::Value, ours: u64, trust_timing: bool) -> Lyrics {
    if body
        .get("instrumental")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
    {
        return Lyrics::Missing("instrumental".into());
    }

    let synced = text_of(body, "syncedLyrics");
    if !synced.trim().is_empty() {
        return match parse(&synced) {
            // Lyrics that run past the end of the track are from a longer
            // recording, whatever the entry claims its duration is: an lrclib
            // entry can say 2:44 and carry timings out to 3:10.
            Lyrics::Synced(lines) if !trust_timing || runs_over(&lines, ours) => {
                Lyrics::Plain(lines.into_iter().map(|(_, words)| words).collect())
            }
            other => other,
        };
    }
    let plain = text_of(body, "plainLyrics");
    if plain.trim().is_empty() {
        Lyrics::Missing("no lyrics on lrclib for this track".into())
    } else {
        parse(&plain)
    }
}

// ---- lrc ----------------------------------------------------------------

/// Parses lrc if the text is timestamped, and falls back to plain lines.
fn parse(text: &str) -> Lyrics {
    parse_with_offset(text).0
}

/// Also reads the `[offset:ms]` tag written by a nudge.
fn parse_with_offset(text: &str) -> (Lyrics, i64) {
    let mut timed: Vec<(Duration, String)> = Vec::new();
    let mut plain: Vec<String> = Vec::new();
    let mut offset = 0;

    for line in text.lines() {
        if let Some(rest) = line.trim().strip_prefix("[offset:")
            && let Some(value) = rest.strip_suffix(']')
        {
            offset = value.trim().trim_start_matches('+').parse().unwrap_or(0);
            continue;
        }
        match split_timestamp(line) {
            Some((at, words)) => timed.push((at, words.to_string())),
            None => plain.push(line.trim_end().to_string()),
        }
    }

    let lyrics = if timed.is_empty() {
        Lyrics::Plain(plain)
    } else {
        timed.sort_by_key(|(at, _)| *at);
        Lyrics::Synced(timed)
    };
    (lyrics, offset)
}

/// `[01:23.45] words` into the offset and the words.
fn split_timestamp(line: &str) -> Option<(Duration, &str)> {
    let rest = line.strip_prefix('[')?;
    let (stamp, words) = rest.split_once(']')?;
    let (minutes, seconds) = stamp.split_once(':')?;
    let minutes: u64 = minutes.trim().parse().ok()?;
    let seconds: f64 = seconds.trim().parse().ok()?;
    Some((
        Duration::from_secs_f64(minutes as f64 * 60.0 + seconds),
        words.trim(),
    ))
}

// ---- disk cache ---------------------------------------------------------

fn cache_dir() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("vibox/lyrics"))
}

fn cache_path(track: &Path) -> Option<PathBuf> {
    let id = track.to_string_lossy().bytes().fold(0u64, |acc, b| {
        acc.wrapping_mul(31).wrapping_add(u64::from(b))
    });
    Some(cache_dir()?.join(format!("{id:016x}.lrc")))
}

/// An empty cache file means "asked lrclib, it has nothing", so a track with
/// no lyrics is not looked up again on every play.
fn load_cached(track: &Path) -> Option<(Lyrics, i64)> {
    let text = std::fs::read_to_string(cache_path(track)?).ok()?;
    // Written by an older vibox: refetch rather than believe its timestamps.
    let text = text.strip_prefix(CACHE_MARK)?;

    if text.trim().is_empty() {
        return Some((
            Lyrics::Missing("no lyrics on lrclib for this track".into()),
            0,
        ));
    }
    Some(parse_with_offset(text))
}

fn store_cached(track: &Path, lyrics: &Lyrics, offset: i64) {
    let Some(path) = cache_path(track) else {
        return;
    };
    if let Some(dir) = path.parent()
        && std::fs::create_dir_all(dir).is_err()
    {
        return;
    }

    let head = if offset == 0 {
        String::new()
    } else {
        format!("[offset:{offset}]\n")
    };
    let body = match lyrics {
        Lyrics::Synced(lines) => lines
            .iter()
            .map(|(at, words)| {
                let secs = at.as_secs_f64();
                format!("[{:02}:{:05.2}] {words}", secs as u64 / 60, secs % 60.0)
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Lyrics::Plain(lines) => lines.join("\n"),
        Lyrics::Missing(_) => String::new(),
    };
    let _ = std::fs::write(path, format!("{CACHE_MARK}\n{head}{body}"));
}

/// Percent encodes everything a query string cannot carry literally.
fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One lrclib search result, as the fields `pick` reads them.
    fn hit(artist: &str, track: &str, seconds: f64) -> serde_json::Value {
        serde_json::json!({
            "artistName": artist,
            "trackName": track,
            "duration": seconds,
            "syncedLyrics": "[00:01.00] words",
        })
    }

    /// The track we are asking about.
    fn asking(artist: &str, title: &str, seconds: u64) -> Request {
        Request {
            path: PathBuf::from("/m/track.mp3"),
            artist: artist.into(),
            title: title.into(),
            album: String::new(),
            duration: seconds,
        }
    }

    fn chosen(hits: &[serde_json::Value], job: &Request) -> Option<String> {
        let body = serde_json::Value::Array(hits.to_vec());
        pick(&body, job).map(|hit| text_of(&hit, "trackName"))
    }

    /// lrclib answers on substrings, so a search can come back entirely full of
    /// other people's songs. Length alone cannot tell them apart.
    #[test]
    fn a_search_that_found_only_other_songs_picks_nothing() {
        let hits = [
            hit("Nightmares on Wax", "Date With Destiny", 308.0),
            hit("Wax", "Call It Destiny", 243.0),
        ];
        assert_eq!(chosen(&hits, &asking("Wax", "Destiny", 306)), None);
    }

    #[test]
    fn the_song_we_asked_for_beats_a_closer_running_time() {
        let hits = [
            hit("Nightmares on Wax", "Date With Destiny", 306.0),
            hit("Wax", "Destiny", 280.0),
        ];
        assert_eq!(
            chosen(&hits, &asking("Wax", "Destiny", 306)).as_deref(),
            Some("Destiny")
        );
    }

    #[test]
    fn case_punctuation_and_whatever_is_in_the_brackets_do_not_matter() {
        let hits = [hit("SFDK", "Despacito Pero Voy", 287.0)];
        let job = asking("sfdk", "Despacito pero voy (Original Mix)", 288);
        assert!(chosen(&hits, &job).is_some());
    }

    #[test]
    fn the_closest_running_time_wins_among_the_ones_that_are_the_song() {
        let hits = [
            hit("ABBA", "The Day Before You Came", 360.0),
            hit("ABBA", "The Day Before You Came", 349.0),
            hit("ABBA", "The Day Before You Came", 320.0),
        ];
        let body = serde_json::Value::Array(hits.to_vec());
        let got = pick(&body, &asking("ABBA", "The Day Before You Came", 350)).unwrap();
        assert_eq!(got.get("duration").unwrap().as_f64(), Some(349.0));
    }

    #[test]
    fn an_entry_with_no_words_in_it_is_never_the_answer() {
        let empty = serde_json::json!({
            "artistName": "ABBA",
            "trackName": "Waterloo",
            "duration": 166.0,
        });
        assert_eq!(chosen(&[empty], &asking("ABBA", "Waterloo", 166)), None);
    }

    /// A file with no artist tag still has a title to go on, and refusing every
    /// hit because a tag is blank would leave it with nothing.
    #[test]
    fn a_tag_we_do_not_have_does_not_get_a_vote() {
        let hits = [hit("Alphaville", "Forever Young", 227.0)];
        assert!(chosen(&hits, &asking("", "Forever Young", 227)).is_some());
    }

    fn timed(seconds: &[u64]) -> Vec<(Duration, String)> {
        seconds
            .iter()
            .map(|s| (Duration::from_secs(*s), format!("line at {s}")))
            .collect()
    }

    /// The case this rule exists for: an lrclib entry claiming 2:44 whose
    /// timings run to 3:10, because they came from the original recording.
    #[test]
    fn lyrics_timed_past_the_end_of_the_track_are_not_trusted() {
        assert!(runs_over(&timed(&[14, 120, 190]), 164));
    }

    #[test]
    fn a_last_line_inside_the_track_is_fine() {
        assert!(!runs_over(&timed(&[14, 120, 160]), 164));
    }

    #[test]
    fn a_few_seconds_over_is_allowed_for_a_fade() {
        assert!(!runs_over(&timed(&[160, 166]), 164));
    }

    #[test]
    fn an_unknown_duration_never_rejects_anything() {
        assert!(!runs_over(&timed(&[190]), 0));
    }

    #[test]
    fn lrc_timestamps_parse_to_their_offsets() {
        let (lyrics, offset) = parse_with_offset("[offset:250]\n[01:30.50] words\n");
        assert_eq!(offset, 250);
        match lyrics {
            Lyrics::Synced(lines) => {
                assert_eq!(lines[0].0, Duration::from_secs_f64(90.5));
                assert_eq!(lines[0].1, "words");
            }
            _ => panic!("timestamped lines are synced lyrics"),
        }
    }
}
