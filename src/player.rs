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

/// Tags for the replies we have to match up ourselves.
const CREATE: u32 = 10;
const TIMING: u32 = 11;
const DRAIN: u32 = 12;

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
    let file = File::open(path).with_context(|| format!("cannot open `{}`", path.display()))?;
    let len = file
        .metadata()
        .with_context(|| format!("cannot stat `{}`", path.display()))?
        .len();

    Decoder::builder()
        .with_data(BufReader::new(file))
        .with_byte_len(len)
        .with_coarse_seek(coarse)
        .build()
        .with_context(|| format!("cannot decode `{}`", path.display()))
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
    /// Volume as a gain in thousandths, so it can live in an atomic.
    gain: AtomicU32,
    /// Loudness of the last chunk sent, in thousandths, for the visualiser.
    level: AtomicU32,
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
        shared.gain.store(800, Ordering::Relaxed);

        let (tx, rx) = channel();
        let thread_shared = Arc::clone(&shared);
        std::thread::Builder::new()
            .name("vibox-audio".into())
            .spawn(move || {
                if let Err(e) = pump(sock, version, &rx, &thread_shared) {
                    thread_shared.fail(format!("audio thread stopped: {e}"));
                }
            })
            .context("cannot start the audio thread")?;

        Ok(Audio {
            tx,
            shared,
            volume: 80,
            muted: false,
            current: None,
        })
    }

    /// Opens and decodes on the caller's side so a broken file is reported
    /// right here, instead of vanishing into the audio thread.
    pub fn play(&mut self, path: &Path) -> Result<()> {
        let decoder = open(path, false)?;

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
        let now = self.pos().as_secs() as i64;
        self.seek(Duration::from_secs((now + delta).max(0) as u64))
    }

    /// Anything the audio thread wants to tell the user, taken once.
    pub fn take_error(&self) -> Option<String> {
        self.shared.error.lock().ok()?.take()
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
            0
        } else {
            u32::from(self.volume) * 10
        };
        self.shared.gain.store(gain, Ordering::Relaxed);
    }

    fn send(&self, cmd: Cmd) -> Result<()> {
        self.tx
            .send(cmd)
            .map_err(|_| anyhow!("the audio thread is gone; restart vibox"))
    }
}

// ---- the socket ---------------------------------------------------------

fn connect() -> Result<(BufReader<UnixStream>, u16)> {
    let path = pulseaudio::socket_path_from_env().context(
        "no pulseaudio socket: vibox plays through pipewire or pulseaudio, neither is running",
    )?;
    let mut sock = BufReader::new(
        UnixStream::connect(&path)
            .with_context(|| format!("cannot connect to `{}`", path.display()))?,
    );

    let cookie = pulseaudio::cookie_path_from_env()
        .and_then(|p| std::fs::read(p).ok())
        .unwrap_or_default();

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
            .context("the sound server refused the connection")?;
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

/// One playback stream: a decoder, and the bookkeeping to turn what the server
/// has consumed back into a position in the track.
struct Stream {
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

/// The audio thread. Blocks on the command channel when there is nothing
/// playing (or while corked, when the server has stopped asking for data), and
/// otherwise lets the server's requests drive the decoding.
fn pump(
    mut sock: BufReader<UnixStream>,
    version: u16,
    rx: &Receiver<Cmd>,
    shared: &Arc<Shared>,
) -> Result<()> {
    let mut stream: Option<Stream> = None;
    let mut pending: Option<(Box<Track>, PathBuf)> = None;
    let mut buf: Vec<u8> = Vec::new();

    loop {
        // Commands that piled up while we were waiting on the server.
        loop {
            match rx.try_recv() {
                Ok(cmd) => command(&mut sock, version, cmd, &mut stream, &mut pending, shared)?,
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return Ok(()),
            }
        }

        // Nothing is being clocked by the server: wait for the ui instead.
        let idle = stream.is_none() && pending.is_none();
        if idle || shared.paused.load(Ordering::Relaxed) {
            match rx.recv() {
                Ok(cmd) => command(&mut sock, version, cmd, &mut stream, &mut pending, shared)?,
                Err(_) => return Ok(()),
            }
            continue;
        }

        let (seq, msg) = protocol::read_command_message(&mut sock, version)
            .context("lost the connection to the sound server")?;

        match msg {
            protocol::Command::Reply if seq == CREATE => {
                let mut ts = protocol::TagStructReader::new(&mut sock, version);
                let reply = ts.read::<protocol::CreatePlaybackStreamReply>()?;
                let Some((decoder, path)) = pending.take() else {
                    continue;
                };

                let channels = decoder.channels().get() as usize;
                let mut new = Stream {
                    channel: reply.channel,
                    rate: decoder.sample_rate().get(),
                    frame_bytes: 4 * channels,
                    decoder: *decoder,
                    path,
                    fed: 0,
                    base: 0.0,
                    draining: false,
                };
                let want = reply.requested_bytes as usize;
                feed(&mut sock, &mut new, want, &mut buf, shared, version)?;
                stream = Some(new);
            }
            protocol::Command::Request(req) => {
                let Some(s) = stream.as_mut() else { continue };
                if req.channel != s.channel || s.draining {
                    continue;
                }
                feed(&mut sock, s, req.length as usize, &mut buf, shared, version)?;

                // Ask what the server has actually played, for the progress bar.
                protocol::write_command_message(
                    sock.get_mut(),
                    TIMING,
                    &protocol::Command::GetPlaybackLatency(protocol::LatencyParams {
                        channel: s.channel,
                        now: SystemTime::now(),
                    }),
                    version,
                )?;
            }
            protocol::Command::Reply if seq == TIMING => {
                let mut ts = protocol::TagStructReader::new(&mut sock, version);
                let timing = ts.read::<protocol::PlaybackLatency>()?;
                if let Some(s) = stream.as_ref() {
                    let inflight = timing.write_offset - timing.read_offset;
                    shared.pos_us.store(s.pos_us(inflight), Ordering::Relaxed);
                }
            }
            // The server has played out the last of the track.
            protocol::Command::Reply if seq == DRAIN => {
                if let Some(s) = stream.take() {
                    let _ = protocol::write_command_message(
                        sock.get_mut(),
                        DRAIN + 100,
                        &protocol::Command::DeletePlaybackStream(s.channel),
                        version,
                    );
                }
                shared.finished.store(true, Ordering::Relaxed);
            }
            protocol::Command::Error(e) => {
                shared.fail(format!("sound server error: {e:?}"));
                stream = None;
                pending = None;
            }
            _ => {}
        }
    }
}

/// The sink to play into.
///
/// `@DEFAULT_SINK@` is resolved by the server, which knows nothing about who is
/// asking, so "send this app to the other card" cannot be answered there. Every
/// libpulse client answers it from `PULSE_SINK`, and that is the variable a
/// person reaches for (or a script sets, per app) to route one program at one
/// output. Talking the protocol directly means reading it here or not honouring
/// it at all.
///
/// A name the server does not know comes back as an error on the stream, which
/// the loop already turns into a message on the status row rather than a crash.
fn sink_name() -> CString {
    sink_from(std::env::var_os("PULSE_SINK"))
}

/// The decision on its own, so it can be checked without touching the
/// environment of a running process.
fn sink_from(var: Option<std::ffi::OsString>) -> CString {
    match var {
        // An interior nul cannot be a sink name, so treat it as unset rather
        // than failing playback over a malformed variable.
        Some(name) if !name.is_empty() => {
            CString::new(name.as_bytes()).unwrap_or_else(|_| protocol::DEFAULT_SINK.to_owned())
        }
        _ => protocol::DEFAULT_SINK.to_owned(),
    }
}

fn command(
    sock: &mut BufReader<UnixStream>,
    version: u16,
    cmd: Cmd,
    stream: &mut Option<Stream>,
    pending: &mut Option<(Box<Track>, PathBuf)>,
    shared: &Arc<Shared>,
) -> Result<()> {
    match cmd {
        Cmd::Play(decoder, path) => {
            drop_stream(sock, version, stream)?;
            let channels = decoder.channels().get();
            let rate = decoder.sample_rate().get();
            let channel_map = match channels {
                1 => protocol::ChannelMap::mono(),
                2 => protocol::ChannelMap::stereo(),
                n => bail!("{n} channel audio is not supported yet"),
            };

            let bytes_per_sec = rate * u32::from(channels) * 4;
            let params = protocol::PlaybackStreamParams {
                sample_spec: protocol::SampleSpec {
                    format: protocol::SampleFormat::Float32Le,
                    channels: channels as u8,
                    sample_rate: rate,
                },
                channel_map,
                cvolume: Some(protocol::ChannelVolume::norm(channels as u8)),
                sink_name: Some(sink_name()),
                buffer_attr: protocol::stream::BufferAttr {
                    max_length: u32::MAX,
                    target_length: (bytes_per_sec as f32 * BUFFER_SECS) as u32,
                    pre_buffering: u32::MAX,
                    minimum_request_length: (bytes_per_sec as f32 * REQUEST_SECS) as u32,
                    fragment_size: u32::MAX,
                },
                ..Default::default()
            };

            protocol::write_command_message(
                sock.get_mut(),
                CREATE,
                &protocol::Command::CreatePlaybackStream(params),
                version,
            )?;
            *pending = Some((decoder, path));
        }
        Cmd::Cork(cork) => {
            if let Some(s) = stream.as_ref() {
                protocol::write_command_message(
                    sock.get_mut(),
                    30,
                    &protocol::Command::CorkPlaybackStream(protocol::CorkStreamParams {
                        channel: s.channel,
                        cork,
                    }),
                    version,
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
                            .map_err(|e| anyhow!("cannot seek this file: {e}"))?;
                        Ok(fresh)
                    }) {
                        Ok(fresh) => s.decoder = fresh,
                        Err(e) => {
                            shared.fail(format!("{e}"));
                            return Ok(());
                        }
                    }
                }
                // Drop what the server has buffered, or the old audio plays on.
                protocol::write_command_message(
                    sock.get_mut(),
                    31,
                    &protocol::Command::FlushPlaybackStream(s.channel),
                    version,
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
            drop_stream(sock, version, stream)?;
            *pending = None;
        }
    }
    Ok(())
}

fn drop_stream(
    sock: &mut BufReader<UnixStream>,
    version: u16,
    stream: &mut Option<Stream>,
) -> Result<()> {
    if let Some(s) = stream.take() {
        protocol::write_command_message(
            sock.get_mut(),
            32,
            &protocol::Command::DeletePlaybackStream(s.channel),
            version,
        )?;
    }
    Ok(())
}

/// Decodes `want` bytes worth of samples and hands them to the server. A short
/// read means the track ended, so we ask the server to drain and tell us when
/// the last sample has actually been played.
fn feed(
    sock: &mut BufReader<UnixStream>,
    stream: &mut Stream,
    want: usize,
    buf: &mut Vec<u8>,
    shared: &Arc<Shared>,
    version: u16,
) -> Result<()> {
    let gain = shared.gain.load(Ordering::Relaxed) as f32 / 1000.0;
    buf.clear();
    buf.reserve(want);

    let mut energy = 0.0f32;
    let mut count = 0u32;
    while buf.len() + 4 <= want {
        let Some(sample) = stream.decoder.next() else {
            break;
        };
        let out = sample * gain;
        energy += out * out;
        count += 1;
        buf.extend_from_slice(&out.to_le_bytes());
    }

    if count > 0 {
        let rms = (energy / count as f32).sqrt();
        shared
            .level
            .store((rms * 1000.0).min(1000.0) as u32, Ordering::Relaxed);
    }

    if !buf.is_empty() {
        protocol::write_memblock(sock.get_mut(), stream.channel, buf, 0)?;
        stream.fed += (buf.len() / stream.frame_bytes) as u64;
    }

    if buf.len() + 4 <= want {
        protocol::write_command_message(
            sock.get_mut(),
            DRAIN,
            &protocol::Command::DrainPlaybackStream(stream.channel),
            version,
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
        let name = "alsa_output.usb-Generic_USB_Audio-00.HiFi__Speaker__sink";
        assert_eq!(
            sink_from(Some(OsString::from(name))).to_str().unwrap(),
            name
        );
    }

    #[test]
    fn unset_falls_back_to_the_server_default() {
        assert_eq!(sink_from(None), protocol::DEFAULT_SINK.to_owned());
    }

    #[test]
    fn an_empty_value_is_not_a_sink_name() {
        // `PULSE_SINK=` is how a shell unsets it in practice.
        assert_eq!(
            sink_from(Some(OsString::new())),
            protocol::DEFAULT_SINK.to_owned()
        );
    }

    #[test]
    fn an_interior_nul_falls_back_rather_than_failing_playback() {
        use std::os::unix::ffi::OsStringExt;
        let bad = OsString::from_vec(b"speak\0er".to_vec());
        assert_eq!(sink_from(Some(bad)), protocol::DEFAULT_SINK.to_owned());
    }
}
