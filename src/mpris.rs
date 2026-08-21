//! MPRIS, the d-bus interface every linux desktop drives its media keys with.
//!
//! Claiming `org.mpris.MediaPlayer2.vibox` is what makes XF86AudioPlay,
//! XF86AudioNext and friends reach vibox while it sits in some other tmux
//! window, and it is the same interface `playerctl` speaks.
//!
//! Async is quarantined in here: one thread owns the bus connection, the ui
//! talks to it through a channel and a snapshot behind a mutex.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use zbus::interface;
use zbus::zvariant::{ObjectPath, OwnedValue, Value};

const PATH: &str = "/org/mpris/MediaPlayer2";

/// Something the desktop asked us to do.
#[derive(Debug, Clone, Copy)]
pub enum Remote {
    PlayPause,
    Play,
    Pause,
    Stop,
    Next,
    Prev,
    /// Relative seek, in microseconds, as the spec sends it.
    Seek(i64),
    Volume(f64),
    Quit,
}

/// What the desktop is allowed to know about us. Refreshed every tick.
#[derive(Clone, Default, PartialEq)]
pub struct Status {
    pub has_track: bool,
    pub paused: bool,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub path: PathBuf,
    pub length_us: u64,
    pub pos_us: u64,
    /// 0.0 to 1.0, as MPRIS wants it.
    pub volume: f64,
    pub can_next: bool,
    pub can_prev: bool,
}

/// The handle the ui keeps: commands coming in, status going out.
pub struct Mpris {
    pub rx: Receiver<Remote>,
    status: Arc<Mutex<Status>>,
}

impl Mpris {
    pub fn publish(&self, status: &Status) {
        if let Ok(mut slot) = self.status.lock()
            && *slot != *status
        {
            slot.clone_from(status);
        }
    }
}

/// Connects to the session bus and serves the two MPRIS interfaces. Returns an
/// error when there is no session bus, which is normal on a bare tty.
pub fn start() -> Result<Mpris> {
    let (tx, rx) = channel();
    let status = Arc::new(Mutex::new(Status::default()));

    // A second vibox cannot claim the plain name, and the spec has it park
    // under an instance name instead of failing.
    let conn = match serve("org.mpris.MediaPlayer2.vibox", &tx, &status) {
        Ok(conn) => conn,
        Err(_) => serve(
            &format!(
                "org.mpris.MediaPlayer2.vibox.instance{}",
                std::process::id()
            ),
            &tx,
            &status,
        )
        .context("no d-bus session bus, so no media key support")?,
    };

    // Desktop widgets update off PropertiesChanged, so watch the snapshot and
    // announce the transitions that matter.
    let watched = Arc::clone(&status);
    std::thread::Builder::new()
        .name("vibox-mpris".into())
        .spawn(move || {
            let Ok(iface) = conn.object_server().interface::<_, Player>(PATH) else {
                return;
            };
            let mut last = Status::default();
            loop {
                std::thread::sleep(Duration::from_millis(400));
                let now = match watched.lock() {
                    Ok(s) => s.clone(),
                    Err(_) => return,
                };
                let emitter = iface.signal_emitter();
                let guard = iface.get();
                if now.has_track != last.has_track || now.paused != last.paused {
                    let _ = zbus::block_on(guard.playback_status_changed(emitter));
                }
                if now.path != last.path {
                    let _ = zbus::block_on(guard.metadata_changed(emitter));
                }
                if (now.volume - last.volume).abs() > f64::EPSILON {
                    let _ = zbus::block_on(guard.volume_changed(emitter));
                }
                if now.can_next != last.can_next || now.can_prev != last.can_prev {
                    let _ = zbus::block_on(guard.can_go_next_changed(emitter));
                    let _ = zbus::block_on(guard.can_go_previous_changed(emitter));
                }
                drop(guard);
                last = now;
            }
        })
        .context("cannot start the mpris thread")?;

    Ok(Mpris { rx, status })
}

fn serve(
    name: &str,
    tx: &Sender<Remote>,
    status: &Arc<Mutex<Status>>,
) -> zbus::Result<zbus::blocking::Connection> {
    zbus::blocking::connection::Builder::session()?
        .name(name)?
        .serve_at(PATH, Root { tx: tx.clone() })?
        .serve_at(
            PATH,
            Player {
                tx: tx.clone(),
                status: Arc::clone(status),
            },
        )?
        .build()
}

/// `org.mpris.MediaPlayer2`: the application itself.
struct Root {
    tx: Sender<Remote>,
}

#[interface(name = "org.mpris.MediaPlayer2")]
impl Root {
    fn raise(&self) {}

    fn quit(&self) {
        let _ = self.tx.send(Remote::Quit);
    }

    #[zbus(property)]
    fn can_quit(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn can_raise(&self) -> bool {
        // vibox lives in whatever terminal started it; we cannot summon it.
        false
    }

    #[zbus(property)]
    fn has_track_list(&self) -> bool {
        false
    }

    #[zbus(property)]
    fn identity(&self) -> String {
        "vibox".into()
    }

    #[zbus(property)]
    fn supported_uri_schemes(&self) -> Vec<String> {
        vec!["file".into()]
    }

    #[zbus(property)]
    fn supported_mime_types(&self) -> Vec<String> {
        [
            "audio/mpeg",
            "audio/flac",
            "audio/ogg",
            "audio/mp4",
            "audio/wav",
        ]
        .iter()
        .map(|s| (*s).to_string())
        .collect()
    }
}

/// `org.mpris.MediaPlayer2.Player`: the transport controls.
struct Player {
    tx: Sender<Remote>,
    status: Arc<Mutex<Status>>,
}

impl Player {
    fn status(&self) -> Status {
        self.status.lock().map(|s| s.clone()).unwrap_or_default()
    }

    fn send(&self, remote: Remote) {
        let _ = self.tx.send(remote);
    }
}

#[interface(name = "org.mpris.MediaPlayer2.Player")]
impl Player {
    fn next(&self) {
        self.send(Remote::Next);
    }

    fn previous(&self) {
        self.send(Remote::Prev);
    }

    fn pause(&self) {
        self.send(Remote::Pause);
    }

    fn play_pause(&self) {
        self.send(Remote::PlayPause);
    }

    fn stop(&self) {
        self.send(Remote::Stop);
    }

    fn play(&self) {
        self.send(Remote::Play);
    }

    fn seek(&self, offset_us: i64) {
        self.send(Remote::Seek(offset_us));
    }

    fn set_position(&self, _track: ObjectPath<'_>, position_us: i64) {
        let now = self.status().pos_us as i64;
        self.send(Remote::Seek(position_us - now));
    }

    fn open_uri(&self, _uri: String) {}

    #[zbus(property)]
    fn playback_status(&self) -> String {
        let s = self.status();
        match (s.has_track, s.paused) {
            (false, _) => "Stopped",
            (true, true) => "Paused",
            (true, false) => "Playing",
        }
        .into()
    }

    #[zbus(property)]
    fn metadata(&self) -> HashMap<String, OwnedValue> {
        let s = self.status();
        let mut meta = HashMap::new();

        // The track id has to be a valid object path, so hash the file into one.
        let id = s.path.to_string_lossy().bytes().fold(0u64, |acc, b| {
            acc.wrapping_mul(31).wrapping_add(u64::from(b))
        });
        if let Ok(path) = ObjectPath::try_from(format!("/org/mpris/vibox/track/{id}"))
            && let Ok(v) = OwnedValue::try_from(Value::from(path))
        {
            meta.insert("mpris:trackid".into(), v);
        }
        if let Ok(v) = OwnedValue::try_from(Value::from(s.length_us as i64)) {
            meta.insert("mpris:length".into(), v);
        }
        if let Ok(v) = OwnedValue::try_from(Value::from(s.title.clone())) {
            meta.insert("xesam:title".into(), v);
        }
        if let Ok(v) = OwnedValue::try_from(Value::from(vec![s.artist.clone()])) {
            meta.insert("xesam:artist".into(), v);
        }
        if let Ok(v) = OwnedValue::try_from(Value::from(s.album.clone())) {
            meta.insert("xesam:album".into(), v);
        }
        if let Ok(v) = OwnedValue::try_from(Value::from(file_uri(&s.path))) {
            meta.insert("xesam:url".into(), v);
        }
        meta
    }

    #[zbus(property)]
    fn volume(&self) -> f64 {
        self.status().volume
    }

    #[zbus(property)]
    fn set_volume(&self, volume: f64) {
        self.send(Remote::Volume(volume.clamp(0.0, 1.0)));
    }

    #[zbus(property)]
    fn position(&self) -> i64 {
        self.status().pos_us as i64
    }

    #[zbus(property)]
    fn rate(&self) -> f64 {
        1.0
    }

    #[zbus(property)]
    fn minimum_rate(&self) -> f64 {
        1.0
    }

    #[zbus(property)]
    fn maximum_rate(&self) -> f64 {
        1.0
    }

    #[zbus(property)]
    fn can_go_next(&self) -> bool {
        self.status().can_next
    }

    #[zbus(property)]
    fn can_go_previous(&self) -> bool {
        self.status().can_prev
    }

    #[zbus(property)]
    fn can_play(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn can_pause(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn can_seek(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn can_control(&self) -> bool {
        true
    }
}

/// `file:///home/me/a b.mp3`, with the characters that break url parsers escaped.
fn file_uri(path: &std::path::Path) -> String {
    let mut out = String::from("file://");
    for byte in path.to_string_lossy().bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}
