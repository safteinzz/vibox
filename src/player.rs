//! Audio output, with no C library anywhere in the build.
//!
//! rodio decodes (it is symphonia underneath, pure rust), and the samples go
//! straight down the pulseaudio socket, which on any modern desktop is served
//! by pipewire. That keeps `cargo install vibox` from ever needing a system
//! header: no cpal, no alsa-sys, no pkg-config.
//!
//! One thread owns the socket and the decoder. The ui talks to it through a
//! channel and reads playback position out of a couple of atomics.
//!
//! Linux only, on purpose. If macos or windows ever matter, the way in is a
//! target gated dependency, not a rewrite: keep this file for
//! `cfg(target_os = "linux")` and add a cpal backed sibling behind
//! `cfg(not(target_os = "linux"))`, where cpal talks to coreaudio and wasapi
//! without needing any headers that the platform sdk does not already ship.
//! Everything above the `Audio` type is platform blind already.

use std::collections::HashMap;
use std::ffi::CString;
use std::fs::File;
use std::io::BufReader;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result, anyhow, bail};
use pulseaudio::protocol;
use rodio::{Decoder, Source};

type Track = Decoder<BufReader<File>>;

/// Server side buffer. Small enough that pause and seek feel immediate,
/// large enough not to underrun on a busy machine.
const BUFFER_SECS: f32 = 0.2;
const REQUEST_SECS: f32 = 0.05;

/// Opens a file as a decoder.
///
/// `byte_len` is what makes seeking work: without it rodio marks the stream
/// unseekable, and formats with no timing header (mp3, vorbis) then refuse an
/// accurate seek outright. `coarse` trades exactness for a seek that lands
/// even on a variable bitrate file with no seek table.
fn open(path: &Path, coarse: bool) -> Result<Track> {
    let name = path
        .file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy();
    let file =
        File::open(path).map_err(|e| anyhow!("cannot open `{name}`: {}", crate::reason::of(&e)))?;
    let len = file
        .metadata()
        .map_err(|e| anyhow!("cannot read `{name}`: {}", crate::reason::of(&e)))?
        .len();

    Decoder::builder()
        .with_data(BufReader::new(file))
        .with_byte_len(len)
        .with_coarse_seek(coarse)
        .build()
        .map_err(|_| {
            let ext = path
                .extension()
                .map(|e| e.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            // Listed with the library so they can be renamed and moved, but
            // no decoder this build carries reads them.
            if matches!(ext.as_str(), "opus" | "wv") {
                anyhow!("cannot play `{name}`: vibox has no decoder for .{ext} files yet")
            } else {
                anyhow!("cannot play `{name}`: it is damaged, or in a format vibox does not read")
            }
        })
}

enum Cmd {
    Play(Box<Track>, PathBuf),
    Cork(bool),
    Seek(Duration),
    Stop,
}

#[derive(Default)]
struct Shared {
    pos_us: AtomicU64,
    /// Set when the server finished playing a track by itself.
    finished: AtomicBool,
    paused: AtomicBool,
    /// Volume as a gain, held as the bits of an `f32` so it can live in an
    /// atomic.
    gain: AtomicU32,
    /// Loudness of the last chunk sent, in thousandths, for the visualiser.
    level: AtomicU32,
    /// Set when the sound server took the stream away, from a mixer or
    /// because the output went, so the ui stops instead of waiting on it.
    killed: AtomicBool,
    error: Mutex<Option<String>>,
}

impl Shared {
    fn fail(&self, text: String) {
        if let Ok(mut slot) = self.error.lock() {
            *slot = Some(text);
        }
    }
}

pub struct Audio {
    tx: Sender<Cmd>,
    shared: Arc<Shared>,
    volume: u8,
    muted: bool,
    current: Option<PathBuf>,
}

impl Audio {
    pub fn new() -> Result<Audio> {
        let (sock, version) = connect()?;
        let shared = Arc::new(Shared::default());

        let (tx, rx) = channel();
        let thread_shared = Arc::clone(&shared);
        std::thread::Builder::new()
            .name("vibox-audio".into())
            .spawn(move || {
                if let Err(e) = pump(sock, version, &rx, &thread_shared) {
                    thread_shared.fail(format!(
                        "playback stopped: {}, restart vibox to play again",
                        crate::reason::chain(&e)
                    ));
                }
            })
            .context("cannot start the audio thread")?;

        let audio = Audio {
            tx,
            shared,
            volume: 80,
            muted: false,
            current: None,
        };
        audio.push_gain();
        Ok(audio)
    }

    /// Opens and decodes on the caller's side so a broken file is reported
    /// right here, instead of vanishing into the audio thread.
    pub fn play(&mut self, path: &Path) -> Result<()> {
        let decoder = open(path, false)?;
        let channels = decoder.channels().get();
        if channels > 2 {
            let name = path
                .file_name()
                .unwrap_or(path.as_os_str())
                .to_string_lossy();
            bail!(
                "cannot play `{name}`: it has {channels} channels, and vibox plays mono and stereo"
            );
        }

        self.shared.finished.store(false, Ordering::Relaxed);
        self.shared.paused.store(false, Ordering::Relaxed);
        self.shared.pos_us.store(0, Ordering::Relaxed);
        self.send(Cmd::Play(Box::new(decoder), path.to_path_buf()))?;
        self.current = Some(path.to_path_buf());
        Ok(())
    }

    pub fn stop(&mut self) {
        self.shared.level.store(0, Ordering::Relaxed);
        let _ = self.send(Cmd::Stop);
        self.shared.pos_us.store(0, Ordering::Relaxed);
        self.shared.paused.store(false, Ordering::Relaxed);
        self.current = None;
    }

    pub fn toggle_pause(&self) {
        if self.is_paused() {
            self.resume()
        } else {
            self.pause()
        }
    }

    pub fn pause(&self) {
        self.shared.paused.store(true, Ordering::Relaxed);
        let _ = self.send(Cmd::Cork(true));
    }

    pub fn resume(&self) {
        self.shared.paused.store(false, Ordering::Relaxed);
        let _ = self.send(Cmd::Cork(false));
    }

    pub fn is_paused(&self) -> bool {
        self.shared.paused.load(Ordering::Relaxed)
    }

    /// True once the server has played out everything we sent it.
    pub fn finished(&self) -> bool {
        self.shared.finished.load(Ordering::Relaxed)
    }

    pub fn has_track(&self) -> bool {
        self.current.is_some()
    }

    /// How loud the audio going out right now is, 0.0 to 1.0 ish.
    pub fn level(&self) -> f32 {
        self.shared.level.load(Ordering::Relaxed) as f32 / 1000.0
    }

    pub fn pos(&self) -> Duration {
        Duration::from_micros(self.shared.pos_us.load(Ordering::Relaxed))
    }

    pub fn seek(&self, to: Duration) -> Result<()> {
        self.send(Cmd::Seek(to))
    }

    /// Seeks relative to the current position, clamped at zero.
    pub fn seek_by(&self, delta: i64) -> Result<()> {
        self.seek_by_ms(delta.saturating_mul(1000))
    }

    /// `seek_by` to the millisecond, for a remote that asks in microseconds.
    pub fn seek_by_ms(&self, delta: i64) -> Result<()> {
        let now = i64::try_from(self.pos().as_millis()).unwrap_or(i64::MAX);
        let to = now.saturating_add(delta).max(0);
        self.seek(Duration::from_millis(to as u64))
    }

    /// Anything the audio thread wants to tell the user, taken once.
    pub fn take_error(&self) -> Option<String> {
        self.shared.error.lock().ok()?.take()
    }

    /// True once after the sound server took the stream away, at which point
    /// nothing is playing any more.
    pub fn take_killed(&mut self) -> bool {
        let killed = self.shared.killed.swap(false, Ordering::Relaxed);
        if killed {
            self.current = None;
        }
        killed
    }

    pub fn volume(&self) -> u8 {
        self.volume
    }

    pub fn set_volume(&mut self, v: u8) {
        self.volume = v.min(100);
        self.muted = false;
        self.push_gain();
    }

    pub fn nudge_volume(&mut self, delta: i32) {
        let v = (i32::from(self.volume) + delta).clamp(0, 100) as u8;
        self.set_volume(v);
    }

    pub fn muted(&self) -> bool {
        self.muted
    }

    pub fn toggle_mute(&mut self) {
        self.muted = !self.muted;
        self.push_gain();
    }

    fn push_gain(&self) {
        let gain = if self.muted {
            0.0
        } else {
            gain_of(self.volume)
        };
        self.shared.gain.store(gain.to_bits(), Ordering::Relaxed);
    }

    fn send(&self, cmd: Cmd) -> Result<()> {
        self.tx
            .send(cmd)
            .map_err(|_| anyhow!("the audio thread is gone; restart vibox"))
    }
}

/// The gain a volume percentage plays at.
///
/// Loudness is heard as a ratio, so a linear gain made each step at the bottom
/// a leap and each step at the top a nudge. The cube is the curve pulseaudio's
/// own percentages use, so 50 here sounds like 50 in a mixer.
fn gain_of(volume: u8) -> f32 {
    (f32::from(volume) / 100.0).powi(3)
}

// ---- the socket ---------------------------------------------------------

fn connect() -> Result<(BufReader<UnixStream>, u16)> {
    let path = pulseaudio::socket_path_from_env().context(
        "no pulseaudio socket: vibox plays through pipewire or pulseaudio, and neither is running",
    )?;
    let mut sock = BufReader::new(UnixStream::connect(&path).map_err(|e| {
        anyhow!(
            "cannot connect to the sound server at `{}`: {}",
            path.display(),
            crate::reason::of(&e)
        )
    })?);

    let (cookie, found) = match cookie() {
        Some(cookie) => (cookie, true),
        // A server that checks cookies refuses this either way, and one that
        // does not, pipewire, still wants one of the right length.
        None => (vec![0; COOKIE_LEN], false),
    };

    protocol::write_command_message(
        sock.get_mut(),
        0,
        &protocol::Command::Auth(protocol::AuthParams {
            version: protocol::MAX_VERSION,
            supports_shm: false,
            supports_memfd: false,
            cookie,
        }),
        protocol::MAX_VERSION,
    )
    .context("cannot talk to the sound server")?;

    let (_, reply) =
        protocol::read_reply_message::<protocol::AuthReply>(&mut sock, protocol::MAX_VERSION)
            .map_err(|_| {
                if found {
                    anyhow!("the sound server refused the connection")
                } else {
                    anyhow!(
                        "the sound server refused the connection: there is no pulseaudio cookie in `$XDG_CONFIG_HOME/pulse` or `~/.config/pulse`"
                    )
                }
            })?;
    let version = protocol::MAX_VERSION.min(reply.version);

    let mut props = protocol::Props::new();
    props.set(protocol::Prop::ApplicationName, CString::new("vibox")?);
    protocol::write_command_message(
        sock.get_mut(),
        1,
        &protocol::Command::SetClientName(props),
        version,
    )?;
    let _ = protocol::read_reply_message::<protocol::SetClientNameReply>(&mut sock, version)?;

    Ok((sock, version))
}

/// What a pulseaudio cookie is, in bytes.
const COOKIE_LEN: usize = 256;

/// The cookie the sound server handed out, looked for where its clients write
/// it: `$PULSE_COOKIE`, then the config dir, honouring `XDG_CONFIG_HOME`, then
/// the old dotfile.
fn cookie() -> Option<Vec<u8>> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| home.as_ref().map(|h| h.join(".config")));
    [
        std::env::var_os("PULSE_COOKIE").map(PathBuf::from),
        config.map(|c| c.join("pulse/cookie")),
        home.as_ref().map(|h| h.join(".config/pulse/cookie")),
        home.map(|h| h.join(".pulse-cookie")),
    ]
    .into_iter()
    .flatten()
    .find_map(|path| std::fs::read(path).ok().filter(|bytes| !bytes.is_empty()))
}

/// One playback stream: a decoder, and the bookkeeping to turn what the server
/// has consumed back into a position in the track.
struct Stream {
    /// Which play this is, so an answer meant for an earlier one is told apart.
    play: u64,
    channel: u32,
    decoder: Track,
    path: PathBuf,
    rate: u32,
    frame_bytes: usize,
    /// Frames handed to the server since the last create or seek.
    fed: u64,
    /// Where the last seek landed, in seconds.
    base: f64,
    /// The decoder ran out; we are waiting for the server to play the rest.
    draining: bool,
}

impl Stream {
    fn pos_us(&self, inflight_bytes: i64) -> u64 {
        let inflight = (inflight_bytes.max(0) as u64) / self.frame_bytes as u64;
        let frames = self.fed.saturating_sub(inflight) as f64;
        ((self.base + frames / f64::from(self.rate)) * 1_000_000.0).max(0.0) as u64
    }
}

/// A play waiting for the server to create its stream.
struct Pending {
    play: u64,
    decoder: Box<Track>,
    path: PathBuf,
}

/// What a request was, so its reply, which carries no kind of its own, is read
/// the right way and applied to the play it was for.
#[derive(Clone, Copy)]
enum Asked {
    Create,
    Timing,
    Drain,
    /// A cork, flush or delete: the reply is an empty acknowledgement.
    Plain,
}

/// The socket and the requests on it still waiting for an answer.
///
/// Every request gets a tag of its own. The server answers in order but
/// several answers can be in flight at once, as when `n` is pressed twice
/// before the first stream exists, and a reply matched by a fixed tag bound the
/// newer decoder to the older stream, at that stream's rate.
struct Wire {
    sock: BufReader<UnixStream>,
    version: u16,
    next: u32,
    asked: HashMap<u32, (Asked, u64)>,
}

impl Wire {
    fn send(&mut self, asked: Asked, play: u64, cmd: &protocol::Command) -> Result<()> {
        let tag = self.next;
        self.next = self.next.wrapping_add(1).max(100);
        self.asked.insert(tag, (asked, play));
        protocol::write_command_message(self.sock.get_mut(), tag, cmd, self.version)?;
        Ok(())
    }
}

/// The audio thread. Blocks on the command channel when there is nothing
/// playing (or while corked, when the server has stopped asking for data), and
/// otherwise lets the server's requests drive the decoding.
fn pump(
    sock: BufReader<UnixStream>,
    version: u16,
    rx: &Receiver<Cmd>,
    shared: &Arc<Shared>,
) -> Result<()> {
    let mut wire = Wire {
        sock,
        version,
        next: 100,
        asked: HashMap::new(),
    };
    let mut stream: Option<Stream> = None;
    let mut pending: Option<Pending> = None;
    let mut plays: u64 = 0;
    let mut buf: Vec<u8> = Vec::new();

    loop {
        // Commands that piled up while we were waiting on the server.
        loop {
            match rx.try_recv() {
                Ok(cmd) => command(
                    &mut wire,
                    cmd,
                    &mut stream,
                    &mut pending,
                    &mut plays,
                    shared,
                )?,
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return Ok(()),
            }
        }

        // Nothing is being clocked by the server: wait for the ui instead. A
        // corked stream gets no requests, and a drain does not finish while
        // corked, so neither is worth reading the socket for; every other
        // answer comes promptly and is read first.
        let answer_due = wire
            .asked
            .values()
            .any(|(asked, _)| !matches!(asked, Asked::Drain));
        let idle = stream.is_none() && pending.is_none() && !answer_due;
        let corked = shared.paused.load(Ordering::Relaxed) && !answer_due;
        if idle || corked {
            match rx.recv() {
                Ok(cmd) => command(
                    &mut wire,
                    cmd,
                    &mut stream,
                    &mut pending,
                    &mut plays,
                    shared,
                )?,
                Err(_) => return Ok(()),
            }
            continue;
        }

        let (tag, msg) = protocol::read_command_message(&mut wire.sock, wire.version)
            .context("lost the connection to the sound server")?;

        match msg {
            protocol::Command::Reply => {
                let Some((asked, play)) = wire.asked.remove(&tag) else {
                    continue;
                };
                match asked {
                    Asked::Create => {
                        let mut ts = protocol::TagStructReader::new(&mut wire.sock, wire.version);
                        let reply = ts.read::<protocol::CreatePlaybackStreamReply>()?;
                        // A stream for a play that has since been replaced is
                        // nobody's, and would sit in the mixer until exit.
                        let Some(p) = pending.take_if(|p| p.play == play) else {
                            wire.send(
                                Asked::Plain,
                                play,
                                &protocol::Command::DeletePlaybackStream(reply.channel),
                            )?;
                            continue;
                        };
                        let channels = p.decoder.channels().get() as usize;
                        let mut new = Stream {
                            play,
                            channel: reply.channel,
                            rate: p.decoder.sample_rate().get(),
                            frame_bytes: 4 * channels,
                            decoder: *p.decoder,
                            path: p.path,
                            fed: 0,
                            base: 0.0,
                            draining: false,
                        };
                        let want = reply.requested_bytes as usize;
                        feed(&mut wire, &mut new, want, &mut buf, shared)?;
                        stream = Some(new);
                    }
                    Asked::Timing => {
                        let mut ts = protocol::TagStructReader::new(&mut wire.sock, wire.version);
                        let timing = ts.read::<protocol::PlaybackLatency>()?;
                        if let Some(s) = stream.as_ref().filter(|s| s.play == play) {
                            let inflight = timing.write_offset - timing.read_offset;
                            shared.pos_us.store(s.pos_us(inflight), Ordering::Relaxed);
                        }
                    }
                    // The server has played out the last of the track.
                    Asked::Drain => {
                        if let Some(s) = stream.take_if(|s| s.play == play) {
                            wire.send(
                                Asked::Plain,
                                play,
                                &protocol::Command::DeletePlaybackStream(s.channel),
                            )?;
                            shared.finished.store(true, Ordering::Relaxed);
                        }
                    }
                    Asked::Plain => {}
                }
            }
            protocol::Command::Request(req) => {
                let Some(s) = stream.as_mut() else { continue };
                if req.channel != s.channel || s.draining {
                    continue;
                }
                feed(&mut wire, s, req.length as usize, &mut buf, shared)?;

                // Ask what the server has actually played, for the progress bar.
                let channel = s.channel;
                let play = s.play;
                wire.send(
                    Asked::Timing,
                    play,
                    &protocol::Command::GetPlaybackLatency(protocol::LatencyParams {
                        channel,
                        now: SystemTime::now(),
                    }),
                )?;
            }
            // An error answers one request: only one about the play going on
            // now has anything to say. Deleting a stream that was draining
            // fails its drain, and that is not news.
            protocol::Command::Error(e) => {
                let Some((asked, play)) = wire.asked.remove(&tag) else {
                    continue;
                };
                match asked {
                    Asked::Create if pending.as_ref().is_some_and(|p| p.play == play) => {
                        pending = None;
                        shared.fail(format!(
                            "the sound server would not start the stream: {}",
                            server_error(e)
                        ));
                    }
                    Asked::Drain if stream.as_ref().is_some_and(|s| s.play == play) => {
                        stream = None;
                        shared.finished.store(true, Ordering::Relaxed);
                    }
                    _ => {}
                }
            }
            protocol::Command::PlaybackStreamKilled(channel)
                if stream.as_ref().is_some_and(|s| s.channel == channel) =>
            {
                stream = None;
                shared.killed.store(true, Ordering::Relaxed);
                shared.fail("the sound server stopped the stream, nothing is playing".into());
            }
            _ => {}
        }
    }
}

/// A pulseaudio error as the end of a sentence.
fn server_error(e: protocol::PulseError) -> String {
    match e {
        protocol::PulseError::NoEntity => {
            "it has no sink by that name, check `PULSE_SINK`".to_string()
        }
        other => {
            let said = other.to_string();
            let mut chars = said.chars();
            chars
                .next()
                .map(|c| c.to_lowercase().chain(chars).collect())
                .unwrap_or(said)
        }
    }
}

/// The sink to play into, or `None` to leave the stream to the server.
///
/// Any name becomes the stream's `target.object`, `@DEFAULT_SINK@` included,
/// and wireplumber never restores the output picked for an app over a stream's
/// own target, so each new track went back to the default after vibox was
/// moved to another output. A stream that names nothing is routed like any
/// other app's.
///
/// `PULSE_SINK` is named anyway, because pinning is what it asks for. Every
/// libpulse client reads it, and it is the variable a person reaches for (or a
/// script sets, per app) to route one program at one output. Talking the
/// protocol directly means reading it here or not honouring it at all.
///
/// A name the server does not know comes back as an error on the stream, which
/// the loop already turns into a message on the status row rather than a crash.
fn sink_name() -> Option<CString> {
    sink_from(std::env::var_os("PULSE_SINK"))
}

/// The decision on its own, so it can be checked without touching the
/// environment of a running process.
fn sink_from(var: Option<std::ffi::OsString>) -> Option<CString> {
    // An interior nul cannot be a sink name, so treat it as unset rather than
    // failing playback over a malformed variable.
    var.filter(|name| !name.is_empty())
        .and_then(|name| CString::new(name.as_bytes()).ok())
}

fn command(
    wire: &mut Wire,
    cmd: Cmd,
    stream: &mut Option<Stream>,
    pending: &mut Option<Pending>,
    plays: &mut u64,
    shared: &Arc<Shared>,
) -> Result<()> {
    match cmd {
        Cmd::Play(decoder, path) => {
            drop_stream(wire, stream)?;
            let channels = decoder.channels().get();
            let rate = decoder.sample_rate().get();
            let channel_map = match channels {
                1 => protocol::ChannelMap::mono(),
                2 => protocol::ChannelMap::stereo(),
                // `Audio::play` refuses these before they get here.
                n => {
                    shared.fail(format!("{n} channel audio is not supported yet"));
                    *pending = None;
                    return Ok(());
                }
            };

            let bytes_per_sec = rate * u32::from(channels) * 4;
            let params = protocol::PlaybackStreamParams {
                sample_spec: protocol::SampleSpec {
                    format: protocol::SampleFormat::Float32Le,
                    channels: channels as u8,
                    sample_rate: rate,
                },
                channel_map,
                // No volume of our own: the gain is applied to the samples, and
                // a volume set here is one the server restores over, so a
                // mixer's setting for vibox would reset on every track. A
                // server older than protocol 14 has no way to say "unset".
                cvolume: (wire.version < 14).then(|| protocol::ChannelVolume::norm(channels as u8)),
                sink_name: sink_name(),
                buffer_attr: protocol::stream::BufferAttr {
                    max_length: u32::MAX,
                    target_length: (bytes_per_sec as f32 * BUFFER_SECS) as u32,
                    pre_buffering: u32::MAX,
                    minimum_request_length: (bytes_per_sec as f32 * REQUEST_SECS) as u32,
                    fragment_size: u32::MAX,
                },
                ..Default::default()
            };

            *plays += 1;
            wire.send(
                Asked::Create,
                *plays,
                &protocol::Command::CreatePlaybackStream(params),
            )?;
            *pending = Some(Pending {
                play: *plays,
                decoder,
                path,
            });
        }
        Cmd::Cork(cork) => {
            if let Some(s) = stream.as_ref() {
                wire.send(
                    Asked::Plain,
                    s.play,
                    &protocol::Command::CorkPlaybackStream(protocol::CorkStreamParams {
                        channel: s.channel,
                        cork,
                    }),
                )?;
            }
        }
        Cmd::Seek(to) => {
            if let Some(s) = stream.as_mut() {
                if s.decoder.try_seek(to).is_err() {
                    // Some files reject an accurate seek. Reopen and land close
                    // instead, which beats refusing to move at all.
                    match open(&s.path, true).and_then(|mut fresh| {
                        fresh
                            .try_seek(to)
                            .map_err(|_| anyhow!("cannot seek in this file"))?;
                        Ok(fresh)
                    }) {
                        Ok(fresh) => s.decoder = fresh,
                        Err(e) => {
                            shared.fail(crate::reason::chain(&e));
                            return Ok(());
                        }
                    }
                }
                // Drop what the server has buffered, or the old audio plays on.
                let (channel, play) = (s.channel, s.play);
                wire.send(
                    Asked::Plain,
                    play,
                    &protocol::Command::FlushPlaybackStream(channel),
                )?;
                s.base = to.as_secs_f64();
                s.fed = 0;
                s.draining = false;
                shared
                    .pos_us
                    .store(to.as_micros() as u64, Ordering::Relaxed);
            }
        }
        Cmd::Stop => {
            drop_stream(wire, stream)?;
            *pending = None;
        }
    }
    Ok(())
}

fn drop_stream(wire: &mut Wire, stream: &mut Option<Stream>) -> Result<()> {
    if let Some(s) = stream.take() {
        wire.send(
            Asked::Plain,
            s.play,
            &protocol::Command::DeletePlaybackStream(s.channel),
        )?;
    }
    Ok(())
}

/// Decodes `want` bytes worth of samples and hands them to the server. A short
/// read means the track ended, so we ask the server to drain and tell us when
/// the last sample has actually been played.
fn feed(
    wire: &mut Wire,
    stream: &mut Stream,
    want: usize,
    buf: &mut Vec<u8>,
    shared: &Arc<Shared>,
) -> Result<()> {
    let gain = f32::from_bits(shared.gain.load(Ordering::Relaxed));
    // The visualiser moves with the volume as a percentage, the cube root of
    // the gain, as it did before the gain followed loudness: on the cube
    // itself the rain all but stopped at an everyday volume.
    let shown = gain.cbrt();
    buf.clear();
    buf.reserve(want);

    let mut energy = 0.0f32;
    let mut count = 0u32;
    while buf.len() + 4 <= want {
        let Some(sample) = stream.decoder.next() else {
            break;
        };
        energy += (sample * shown) * (sample * shown);
        count += 1;
        buf.extend_from_slice(&(sample * gain).to_le_bytes());
    }

    if count > 0 {
        let rms = (energy / count as f32).sqrt();
        shared
            .level
            .store((rms * 1000.0).min(1000.0) as u32, Ordering::Relaxed);
    }

    if !buf.is_empty() {
        protocol::write_memblock(wire.sock.get_mut(), stream.channel, buf, 0)?;
        stream.fed += (buf.len() / stream.frame_bytes) as u64;
    }

    if buf.len() + 4 <= want {
        let (channel, play) = (stream.channel, stream.play);
        wire.send(
            Asked::Drain,
            play,
            &protocol::Command::DrainPlaybackStream(channel),
        )?;
        stream.draining = true;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    #[test]
    fn a_named_sink_is_used() {
        let name = "alsa_output.example-card.analog-stereo";
        assert_eq!(
            sink_from(Some(OsString::from(name))),
            Some(CString::new(name).unwrap())
        );
    }

    #[test]
    fn unset_names_no_sink_so_the_server_routes_the_stream() {
        assert_eq!(sink_from(None), None);
    }

    #[test]
    fn an_empty_value_is_not_a_sink_name() {
        // `PULSE_SINK=` is how a shell unsets it in practice.
        assert_eq!(sink_from(Some(OsString::new())), None);
    }

    #[test]
    fn an_interior_nul_falls_back_rather_than_failing_playback() {
        use std::os::unix::ffi::OsStringExt;
        let bad = OsString::from_vec(b"speak\0er".to_vec());
        assert_eq!(sink_from(Some(bad)), None);
    }
}
