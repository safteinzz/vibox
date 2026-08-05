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
            return from_json(&body, trust);
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
        Ok(body) => match pick(&body, job.duration) {
            Some(hit) => {
                let trust = gap(&hit, job.duration) <= SYNC_TOLERANCE;
                from_json(&hit, trust)
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

/// Picks the hit closest in duration, which is the likeliest to be the same
/// recording.
fn pick(body: &serde_json::Value, ours: u64) -> Option<serde_json::Value> {
    let hits = body.as_array()?;
    hits.iter()
        .filter(|hit| has_lyrics(hit))
        .min_by(|a, b| gap(a, ours).total_cmp(&gap(b, ours)))
        .cloned()
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
            serde_json::from_str(&text).map_err(|e| FetchError::Other(format!("lrclib sent something unreadable: {e}")))
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
fn from_json(body: &serde_json::Value, trust_timing: bool) -> Lyrics {
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
            Lyrics::Synced(lines) if !trust_timing => {
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

fn cache_path(track: &Path) -> Option<PathBuf> {
    let id = track.to_string_lossy().bytes().fold(0u64, |acc, b| {
        acc.wrapping_mul(31).wrapping_add(u64::from(b))
    });
    Some(dirs::data_dir()?.join("vibox/lyrics").join(format!("{id:016x}.lrc")))
}

/// An empty cache file means "asked lrclib, it has nothing", so a track with
/// no lyrics is not looked up again on every play.
fn load_cached(track: &Path) -> Option<(Lyrics, i64)> {
    let text = std::fs::read_to_string(cache_path(track)?).ok()?;
    if text.trim().is_empty() {
        return Some((
            Lyrics::Missing("no lyrics on lrclib for this track".into()),
            0,
        ));
    }
    Some(parse_with_offset(&text))
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
    if body.is_empty() {
        let _ = std::fs::write(path, body);
    } else {
        let _ = std::fs::write(path, format!("{head}{body}"));
    }
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
