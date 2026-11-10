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
use std::time::{Duration, Instant};

use crate::library::Track;

/// Stamped on every cache entry. Bump it when what gets cached changes
/// meaning: entries without the current marker are refetched rather than
/// trusted, since an old one cannot say whether its timings were believed.
const CACHE_MARK: &str = "[vibox:5]";

/// Who the words came from, for the pane to credit. One provider today; when
/// there are two this moves onto `Lyrics` so each result carries its own.
pub const SOURCE: &str = "lrclib";

const AGENT: &str = concat!(
    "vibox/",
    env!("CARGO_PKG_VERSION"),
    " (https://github.com/safteinzz/vibox)"
);

/// How long one lookup may take, connecting included. There is one worker, so
/// a request that never comes back would leave every later track waiting.
const TIMEOUT: Duration = Duration::from_secs(15);

/// How long a lookup that failed for want of lrclib waits before the next play
/// asks again.
const RETRY: Duration = Duration::from_secs(60);

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

/// What the worker sends back for one track: the words or why there are none,
/// the correction kept with them, and whether the answer is lrclib's own or a
/// failure to ask it at all.
struct Answer {
    path: PathBuf,
    lyrics: Lyrics,
    offset: i64,
    settled: bool,
}

pub struct Fetcher {
    tx: Sender<Request>,
    rx: Receiver<Answer>,
    cache: HashMap<PathBuf, Lyrics>,
    /// Per file correction in milliseconds, positive meaning the words come
    /// later. A rip with a different lead-in needs this and no lookup can.
    offsets: HashMap<PathBuf, i64>,
    inflight: HashSet<PathBuf>,
    /// Tracks whose lookup could not reach lrclib, and when, so they are
    /// asked again later instead of being remembered as having no lyrics.
    unsettled: HashMap<PathBuf, Instant>,
}

impl Fetcher {
    pub fn new() -> Fetcher {
        let (tx, jobs) = channel::<Request>();
        let (results, rx) = channel();

        // One worker: lyrics are not urgent and a queue keeps lrclib happy.
        let _ = std::thread::Builder::new()
            .name("vibox-lyrics".into())
            .spawn(move || {
                let agent = ureq::Agent::config_builder()
                    .timeout_global(Some(TIMEOUT))
                    .build()
                    .new_agent();
                while let Ok(job) = jobs.recv() {
                    let answer = match load_cached(&job.path) {
                        Some((lyrics, offset)) => Answer {
                            path: job.path,
                            lyrics,
                            offset,
                            settled: true,
                        },
                        None => {
                            // Only lrclib's own answer is worth keeping: a
                            // lookup that never reached it says nothing about
                            // whether the track has words.
                            let (lyrics, settled) = match fetch(&agent, &job) {
                                Ok(lyrics) => {
                                    store_cached(&job.path, &lyrics, 0);
                                    (lyrics, true)
                                }
                                Err(why) => (Lyrics::Missing(why), false),
                            };
                            Answer {
                                path: job.path,
                                lyrics,
                                offset: 0,
                                settled,
                            }
                        }
                    };
                    if results.send(answer).is_err() {
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
            unsettled: HashMap::new(),
        }
    }

    /// Queues a fetch unless the track is already cached or already queued.
    ///
    /// A track whose last lookup could not reach lrclib is asked again once
    /// `RETRY` has passed.
    pub fn request(&mut self, track: &Track) {
        if let Some(when) = self.unsettled.get(&track.path)
            && when.elapsed() >= RETRY
        {
            self.unsettled.remove(&track.path);
            self.cache.remove(&track.path);
        }
        if self.cache.contains_key(&track.path) || self.inflight.contains(&track.path) {
            return;
        }
        // An untagged file's title is its filename, and a search by filename
        // with no artist to check the hits against is how a wrong sheet gets
        // synced to a track.
        if track.artist.trim().is_empty() && track.title == track.file {
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
            // Rounded, not truncated: lrclib's exact lookup allows two seconds
            // either side, and truncating 318.98 to 318 spends most of one of
            // them before the request leaves.
            duration: track.duration.as_secs_f64().round() as u64,
        });
    }

    /// Moves whatever the worker finished into the cache. Called every tick.
    pub fn poll(&mut self) {
        while let Ok(answer) = self.rx.try_recv() {
            self.inflight.remove(&answer.path);
            if !answer.settled {
                self.unsettled.insert(answer.path.clone(), Instant::now());
            }
            self.offsets.insert(answer.path.clone(), answer.offset);
            self.cache.insert(answer.path, answer.lyrics);
        }
    }

    /// Follows a file that was renamed or moved, so its words and its `[ ]`
    /// correction stay with it instead of being fetched again under a name the
    /// cache has never seen.
    pub fn moved(&mut self, from: &Path, to: &Path) {
        if let Some(lyrics) = self.cache.remove(from) {
            self.cache.insert(to.to_path_buf(), lyrics);
        }
        if let Some(offset) = self.offsets.remove(from) {
            self.offsets.insert(to.to_path_buf(), offset);
        }
        if let Some(when) = self.unsettled.remove(from) {
            self.unsettled.insert(to.to_path_buf(), when);
        }
        if let (Some(old), Some(new)) = (cache_path(from), cache_path(to)) {
            let _ = std::fs::rename(old, new);
        }
    }

    pub fn offset(&self, path: &Path) -> i64 {
        self.offsets.get(path).copied().unwrap_or(0)
    }

    /// Shifts this file's lyrics and writes the correction into its cache
    /// entry, so the track stays in sync every time it is played.
    ///
    /// `None` when there are no timestamps to shift, because an offset applies
    /// to those and nothing else: reporting a shift that cannot move anything
    /// on screen is a status line saying something worked when it did not.
    pub fn nudge(&mut self, path: &Path, delta_ms: i64) -> Option<i64> {
        if !matches!(self.cache.get(path), Some(Lyrics::Synced(_))) {
            return None;
        }
        let offset = self.offset(path) + delta_ms;
        self.offsets.insert(path.to_path_buf(), offset);
        if let Some(lyrics) = self.cache.get(path) {
            store_cached(path, lyrics, offset);
        }
        Some(offset)
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
        self.unsettled.clear();

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

/// lrclib's answer for one track, or why it could not be asked.
fn fetch(agent: &ureq::Agent, job: &Request) -> Result<Lyrics, String> {
    let exact = format!(
        "https://lrclib.net/api/get?artist_name={}&track_name={}&album_name={}&duration={}",
        encode(&job.artist),
        encode(&job.title),
        encode(&job.album),
        job.duration
    );

    match get_json(agent, &exact) {
        Ok(body) => return Ok(from_json(&body, job.duration)),
        // A miss on the exact match is normal: the duration or the album
        // rarely lines up with what someone else uploaded.
        Err(FetchError::NotFound) => {}
        Err(FetchError::Unreachable(why)) => return Err(why),
    }

    let search = format!(
        "https://lrclib.net/api/search?artist_name={}&track_name={}",
        encode(&job.artist),
        encode(&job.title)
    );
    match get_json(agent, &search) {
        Ok(body) => Ok(match pick(&body, job) {
            Some(hit) => from_json(&hit, job.duration),
            None => Lyrics::Missing("no lyrics on lrclib for this track".into()),
        }),
        Err(FetchError::NotFound) => {
            Ok(Lyrics::Missing("no lyrics on lrclib for this track".into()))
        }
        Err(FetchError::Unreachable(why)) => Err(why),
    }
}

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
/// Ranking is all the duration is good for. An lrclib entry's duration is the
/// length of whoever uploaded it, not of the sheet: the same twenty timestamped
/// lines come back claiming anything from 4:23 to 6:58, so a hit being seven
/// seconds out is no evidence at all against its timestamps.
///
/// lrclib's search is a substring match on both fields, so asking for `Wax` and
/// `Harbour` also answers with `Tides of Glass - Night Harbour Lights`. Ranking
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

/// Lowercase, without bracketed asides or punctuation: `Harbour (Original
/// Version)` and `harbour` are the same title written twice.
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
    /// lrclib answered, and it has nothing for these words.
    NotFound,
    /// lrclib was not reached or did not answer, said for the pane.
    Unreachable(String),
}

/// What the pane says when lrclib could not be asked.
fn unreachable(why: &str) -> FetchError {
    FetchError::Unreachable(format!(
        "cannot reach lrclib ({why}), vibox asks again on a later play"
    ))
}

fn get_json(agent: &ureq::Agent, url: &str) -> Result<serde_json::Value, FetchError> {
    let response = agent
        .get(url)
        .header("User-Agent", AGENT)
        .call()
        .map_err(|e| match e {
            // A lookup lrclib cannot make with what we sent, such as a blank
            // field, is a miss like any other, so the search still gets a go.
            ureq::Error::StatusCode(400 | 404) => FetchError::NotFound,
            ureq::Error::StatusCode(code) => unreachable(&format!("it answered {code}")),
            ureq::Error::Timeout(_) => unreachable("it did not answer in time"),
            ureq::Error::HostNotFound => unreachable("no network, or no name server"),
            ureq::Error::Io(io) => unreachable(&crate::reason::of(&io)),
            _ => unreachable("the connection failed"),
        })?;

    response
        .into_body()
        .read_to_string()
        .map_err(|_| unreachable("the answer was cut short"))
        .and_then(|text| {
            serde_json::from_str(&text).map_err(|_| unreachable("it sent something unreadable"))
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

/// Timestamps survive unless they cannot be about this track at all.
fn from_json(body: &serde_json::Value, ours: u64) -> Lyrics {
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
            // entry can say 2:44 and carry timings out to 3:10. This is the
            // one thing that ever costs a sheet its timestamps.
            Lyrics::Synced(lines) if runs_over(&lines, ours) => {
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
        match split_timestamps(line) {
            // A chorus written once with every time it comes round is sung at
            // each of them.
            Some((stamps, words)) => {
                timed.extend(stamps.into_iter().map(|at| (at, words.to_string())));
            }
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

/// `[01:23.45] words`, or `[00:12.00][00:45.00] words`, into every time the
/// line is sung and the words.
fn split_timestamps(line: &str) -> Option<(Vec<Duration>, &str)> {
    let mut rest = line;
    let mut stamps = Vec::new();
    while let Some(after) = rest.strip_prefix('[') {
        let Some((stamp, tail)) = after.split_once(']') else {
            break;
        };
        let Some(at) = timestamp(stamp) else {
            break;
        };
        stamps.push(at);
        rest = tail;
    }
    (!stamps.is_empty()).then(|| (stamps, rest.trim()))
}

/// `01:23.45` as a time. `None` for anything that is not one, such as an
/// `ar:` tag, and for a value no time can be, since this text comes from
/// whoever uploaded it and `[00:nan]` parses as a float.
fn timestamp(stamp: &str) -> Option<Duration> {
    let (minutes, seconds) = stamp.split_once(':')?;
    let minutes: u64 = minutes.trim().parse().ok()?;
    let seconds: f64 = seconds.trim().parse().ok()?;
    Duration::try_from_secs_f64(minutes as f64 * 60.0 + seconds).ok()
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
    // The newline after the mark goes with it, or every plain page grows a
    // blank first row the moment it comes back off the disk.
    let text = text
        .strip_prefix(CACHE_MARK)?
        .strip_prefix('\n')
        .unwrap_or("");

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
            hit("Tides of Glass", "Night Harbour Lights", 308.0),
            hit("Glass", "Call It Harbour", 243.0),
        ];
        assert_eq!(chosen(&hits, &asking("Glass", "Harbour", 306)), None);
    }

    #[test]
    fn the_song_we_asked_for_beats_a_closer_running_time() {
        let hits = [
            hit("Tides of Glass", "Night Harbour Lights", 306.0),
            hit("Glass", "Harbour", 280.0),
        ];
        assert_eq!(
            chosen(&hits, &asking("Glass", "Harbour", 306)).as_deref(),
            Some("Harbour")
        );
    }

    #[test]
    fn case_punctuation_and_whatever_is_in_the_brackets_do_not_matter() {
        let hits = [hit("LMNO", "Slowly But Still", 287.0)];
        let job = asking("lmno", "Slowly but still (Original Mix)", 288);
        assert!(chosen(&hits, &job).is_some());
    }

    #[test]
    fn the_closest_running_time_wins_among_the_ones_that_are_the_song() {
        let hits = [
            hit("Lumen Drift", "The Night Before The Tide", 360.0),
            hit("Lumen Drift", "The Night Before The Tide", 349.0),
            hit("Lumen Drift", "The Night Before The Tide", 320.0),
        ];
        let body = serde_json::Value::Array(hits.to_vec());
        let got = pick(
            &body,
            &asking("Lumen Drift", "The Night Before The Tide", 350),
        )
        .unwrap();
        assert_eq!(got.get("duration").unwrap().as_f64(), Some(349.0));
    }

    #[test]
    fn an_entry_with_no_words_in_it_is_never_the_answer() {
        let empty = serde_json::json!({
            "artistName": "Lumen Drift",
            "trackName": "Low Water",
            "duration": 166.0,
        });
        assert_eq!(
            chosen(&[empty], &asking("Lumen Drift", "Low Water", 166)),
            None
        );
    }

    /// A file with no artist tag still has a title to go on, and refusing every
    /// hit because a tag is blank would leave it with nothing.
    #[test]
    fn a_tag_we_do_not_have_does_not_get_a_vote() {
        let hits = [hit("Paper Coast", "Always Young", 227.0)];
        assert!(chosen(&hits, &asking("", "Always Young", 227)).is_some());
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

    /// The rule the discard broke: an entry's duration is the length of
    /// whoever uploaded it, so a hit minutes out still keeps its timestamps
    /// as long as they land inside our track. `[` and `]` are what fix the
    /// difference, and they need something to shift.
    #[test]
    fn a_hit_whose_duration_disagrees_keeps_its_timestamps() {
        let mut entry = hit("Low Ceiling", "The Rhyme Made Me Do It", 326.0);
        entry["syncedLyrics"] = serde_json::json!("[00:03.41] first\n[04:26.78] last");
        entry["plainLyrics"] = serde_json::json!("first\nlast");

        match from_json(&entry, 319) {
            Lyrics::Synced(lines) => assert_eq!(lines.len(), 2),
            _ => panic!("timestamps that fit inside the track were thrown away"),
        }
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
