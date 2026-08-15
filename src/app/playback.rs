//! Starting playback, walking the queue and keeping mpris in step.

use std::path::{Path, PathBuf};
use std::time::Duration;


use crate::mpris::{self, Remote};
use crate::player::Audio;

use super::*;

impl App {
    /// Starts playback at the cursor, snapshotting the view as the queue.
    pub fn play_cursor(&mut self) {
        if self.view.is_empty() {
            self.error("nothing to play here");
            return;
        }
        self.queue = self.view.clone();
        self.qpos = self.cur;
        self.history.clear();
        self.play_queue_pos();
    }

    /// Where a track actually is on disk right now.
    ///
    /// A pending move has already changed the path the list shows, but the file
    /// itself does not move until `:w`, so playback follows it back.
    fn disk_path(&self, path: &Path) -> PathBuf {
        self.moves
            .iter()
            .find(|(_, to)| to == path)
            .map_or_else(|| path.to_path_buf(), |(from, _)| from.clone())
    }

    fn play_queue_pos(&mut self) {
        let Some(&track_idx) = self.queue.get(self.qpos) else {
            return;
        };
        let path = self.disk_path(&self.tracks[track_idx].path);
        let Some(audio) = self.audio.as_mut() else {
            self.error("no audio device: playback is disabled");
            return;
        };
        match audio.play(&path) {
            Ok(()) => {
                self.playing = Some(track_idx);
                self.msg = None;
                // The one place a track actually starts, so the one place the
                // session log grows.
                let track = &self.tracks[track_idx];
                self.played.push(if track.artist.is_empty() {
                    track.title.clone()
                } else {
                    format!("{} - {}", track.artist, track.title)
                });
            }
            Err(e) => {
                self.playing = None;
                self.error(format!("{e}"));
            }
        }
    }

    pub fn stop(&mut self) {
        if let Some(audio) = self.audio.as_mut() {
            audio.stop();
        }
        self.playing = None;
    }

    /// Moves through the queue. `auto` is a track ending on its own, which is
    /// the only case where `repeat one` replays instead of advancing.
    pub fn advance(&mut self, delta: isize, auto: bool) {
        if self.queue.is_empty() {
            return;
        }
        if auto && self.repeat == Repeat::One {
            self.play_queue_pos();
            return;
        }
        // Shuffle keeps a history, because "previous" has to mean the track you
        // just heard. Without it, going back walks the queue order instead, to
        // a track that was never played.
        if self.shuffle {
            if delta > 0 {
                self.history.push(self.qpos);
                self.qpos = self.next_random();
            } else if let Some(previous) = self.history.pop() {
                self.qpos = previous;
            } else {
                self.info("nothing played before this");
                return;
            }
            self.play_queue_pos();
            self.follow_playing();
            return;
        }

        let next = self.qpos as isize + delta;
        if next < 0 {
            self.qpos = 0;
        } else if next as usize >= self.queue.len() {
            if self.repeat == Repeat::All {
                self.qpos = 0;
            } else {
                self.stop();
                return;
            }
        } else {
            self.qpos = next as usize;
        }
        self.play_queue_pos();
        self.follow_playing();
    }

    /// Keeps the cursor on the track that is playing while it moves by itself.
    fn follow_playing(&mut self) {
        if let Some(p) = self.playing
            && let Some(row) = self.view.iter().position(|&i| i == p)
        {
            self.goto(row);
        }
    }

    /// Called every event loop turn: detects a track that ran out, and picks
    /// up anything the audio thread wants to say.
    pub fn tick(&mut self) {
        if let Some(e) = self.audio.as_ref().and_then(Audio::take_error) {
            self.error(e);
        }

        let finished = self
            .audio
            .as_ref()
            .is_some_and(|a| a.has_track() && a.finished() && !a.is_paused());
        if finished && self.playing.is_some() {
            self.advance(1, true);
        }

        self.remote_control();
        self.publish_status();
        self.lyrics_tick();
    }

    /// Keeps the lyrics pane fed: results in, a request out for a new track.
    fn lyrics_tick(&mut self) {
        self.lyrics.poll();
        if !self.show_lyrics {
            return;
        }
        if let Some(track) = self.playing_track().cloned() {
            self.lyrics.request(&track);
        }
    }

    /// Media keys, `playerctl`, and whatever else is on the session bus.
    fn remote_control(&mut self) {
        let Some(commands) = self
            .mpris
            .as_ref()
            .map(|m| m.rx.try_iter().collect::<Vec<_>>())
        else {
            return;
        };

        for command in commands {
            match command {
                Remote::PlayPause => match self.audio.as_ref() {
                    Some(audio) if audio.has_track() => audio.toggle_pause(),
                    _ => self.play_cursor(),
                },
                Remote::Play => match self.audio.as_ref() {
                    Some(audio) if audio.has_track() => audio.resume(),
                    _ => self.play_cursor(),
                },
                Remote::Pause => {
                    if let Some(audio) = self.audio.as_ref() {
                        audio.pause();
                    }
                }
                Remote::Stop => self.stop(),
                Remote::Next => self.advance(1, false),
                Remote::Prev => self.advance(-1, false),
                Remote::Seek(offset_us) => {
                    if let Some(audio) = self.audio.as_ref()
                        && let Err(e) = audio.seek_by(offset_us / 1_000_000)
                    {
                        self.error(format!("{e}"));
                    }
                }
                Remote::Volume(v) => {
                    if let Some(audio) = self.audio.as_mut() {
                        audio.set_volume((v * 100.0).round() as u8);
                    }
                }
                Remote::Quit => self.quit = true,
            }
        }
    }

    /// Hands the desktop something to put in its media widget.
    fn publish_status(&self) {
        let Some(mpris) = self.mpris.as_ref() else {
            return;
        };
        let audio = self.audio.as_ref();
        let track = self.playing_track();

        mpris.publish(&mpris::Status {
            has_track: audio.is_some_and(Audio::has_track),
            paused: audio.is_some_and(Audio::is_paused),
            title: track.map(|t| t.title.clone()).unwrap_or_default(),
            artist: track.map(|t| t.artist.clone()).unwrap_or_default(),
            album: track.map(|t| t.album.clone()).unwrap_or_default(),
            path: track.map(|t| t.path.clone()).unwrap_or_default(),
            length_us: track.map_or(0, |t| t.duration.as_micros() as u64),
            pos_us: self.elapsed().as_micros() as u64,
            volume: audio.map_or(0.0, |a| f64::from(a.volume()) / 100.0),
            can_next: !self.queue.is_empty(),
            can_prev: !self.queue.is_empty(),
        });
    }

    pub fn elapsed(&self) -> Duration {
        self.audio.as_ref().map_or(Duration::ZERO, Audio::pos)
    }

    fn next_random(&mut self) -> usize {
        // xorshift64: a shuffle order does not need a crate.
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        (self.rng % self.queue.len() as u64) as usize
    }

    /// Copies text to the system clipboard with OSC 52, the escape sequence
    /// terminals answer for it.
    ///
    /// No clipboard crate: those pull in x11 or wayland C libraries, and the
    /// terminal already knows how to do this. tmux needs `set-clipboard on`.
    pub fn copy_to_clipboard(&mut self, text: &str) {
        use std::io::Write;

        const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let bytes = text.as_bytes();
        let mut encoded = String::new();
        for chunk in bytes.chunks(3) {
            let b = [
                chunk[0],
                chunk.get(1).copied().unwrap_or(0),
                chunk.get(2).copied().unwrap_or(0),
            ];
            let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
            for i in 0..4 {
                if i <= chunk.len() {
                    encoded.push(ALPHABET[(n >> (18 - i * 6)) as usize & 0x3F] as char);
                } else {
                    encoded.push('=');
                }
            }
        }

        let mut out = std::io::stdout();
        if write!(out, "\x1b]52;c;{encoded}\x07")
            .and_then(|()| out.flush())
            .is_ok()
        {
            self.info(format!("copied {text}"));
        } else {
            self.error("cannot reach the terminal clipboard");
        }
    }
}
