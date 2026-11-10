//! Starting playback, walking the queue and keeping mpris in step.

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
        self.qpos_next = false;
        self.history.clear();
        self.shuffle_bag.clear();
        self.play_queue_pos();
    }

    fn play_queue_pos(&mut self) {
        let Some(&track_idx) = self.queue.get(self.qpos) else {
            return;
        };
        // The file as it is on disk now: a pending move has not moved it yet,
        // and a pending copy plays the file it is made from.
        let path = self.origin_of(&self.tracks[track_idx].path);
        let Some(audio) = self.audio.as_mut() else {
            self.error("no audio device: playback is disabled");
            return;
        };
        match audio.play(&path) {
            Ok(()) => {
                self.playing = Some(track_idx);
                self.playing_gone = None;
                self.playing_file = Some(path);
                self.qpos_next = false;
                self.msg = None;
                // New words: whatever was pinned, or scrolled to, was for the
                // last song.
                self.unpin_lyrics();
                self.lyrics_top = 0;
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
                self.playing_gone = None;
                self.playing_file = None;
                self.error(crate::reason::chain(&e));
            }
        }
    }

    pub fn stop(&mut self) {
        if let Some(audio) = self.audio.as_mut() {
            audio.stop();
        }
        self.playing = None;
        self.playing_gone = None;
        self.playing_file = None;
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
        // Set when the entry that played has left the queue, so `qpos` was
        // never heard.
        let next_is_here = std::mem::take(&mut self.qpos_next);
        // Shuffle keeps a history, because "previous" has to mean the track you
        // just heard. Without it, going back walks the queue order instead, to
        // a track that was never played.
        if self.shuffle {
            if delta > 0 {
                if !next_is_here {
                    self.history.push(self.qpos);
                }
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

        let next = if next_is_here && delta > 0 {
            self.qpos as isize + delta - 1
        } else {
            self.qpos as isize + delta
        };
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
        self.poll_job();
        if let Some(e) = self.audio.as_ref().and_then(Audio::take_error) {
            self.error(e);
        }
        // The server took the stream away: nothing is playing, and nothing
        // should start playing on its own.
        if self.audio.as_mut().is_some_and(Audio::take_killed) {
            self.playing = None;
            self.playing_gone = None;
            self.playing_file = None;
        }

        let finished = self
            .audio
            .as_ref()
            .is_some_and(|a| a.has_track() && a.finished() && !a.is_paused());
        if finished && self.playing_track().is_some() {
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
                Remote::Seek(offset_us) => self.seek_by_ms(offset_us / 1000),
                Remote::SetPosition(position_us) => {
                    self.seek_to(Duration::from_micros(position_us.max(0) as u64));
                }
                Remote::Volume(v) => {
                    if let Some(audio) = self.audio.as_mut() {
                        audio.set_volume((v * 100.0).round() as u8);
                    }
                }
                // A desktop widget's quit is `:qa`, so it refuses while
                // anything is unsaved like every other way out.
                Remote::Quit => crate::excmd::run(self, "qa"),
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
            seeks: self.seeks,
        });
    }

    pub fn elapsed(&self) -> Duration {
        self.audio.as_ref().map_or(Duration::ZERO, Audio::pos)
    }

    /// Seeks by `delta_ms` from where the track is now, for `h`, `l`,
    /// `:seek +30` and a remote's relative seek alike.
    pub fn seek_by_ms(&mut self, delta_ms: i64) {
        let Some(audio) = self.audio.as_ref() else {
            self.error("no audio device: playback is disabled");
            return;
        };
        if !audio.has_track() {
            self.error("nothing playing");
            return;
        }
        match audio.seek_by_ms(delta_ms) {
            Ok(()) => self.seeks += 1,
            Err(e) => self.error(crate::reason::chain(&e)),
        }
    }

    /// Seeks to a point in the track, for `:seek 1:30` and a remote's
    /// `SetPosition`.
    pub fn seek_to(&mut self, to: Duration) {
        let Some(audio) = self.audio.as_ref() else {
            self.error("no audio device: playback is disabled");
            return;
        };
        if !audio.has_track() {
            self.error("nothing playing");
            return;
        }
        match audio.seek(to) {
            Ok(()) => self.seeks += 1,
            Err(e) => self.error(crate::reason::chain(&e)),
        }
    }

    /// Draws from a bag of not-yet-played queue positions, so shuffle covers
    /// every track once before any of them repeat. The bag is refilled, minus
    /// the track that is about to become "previous", whenever it runs dry.
    fn next_random(&mut self) -> usize {
        if self.shuffle_bag.is_empty() {
            self.shuffle_bag = (0..self.queue.len()).filter(|&p| p != self.qpos).collect();
        }
        if self.shuffle_bag.is_empty() {
            // Nothing but the track already playing: it is the only option.
            return self.qpos;
        }

        // xorshift64: a shuffle order does not need a crate.
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        let idx = (self.rng % self.shuffle_bag.len() as u64) as usize;
        self.shuffle_bag.swap_remove(idx)
    }

    /// Copies text to the system clipboard with OSC 52, the escape sequence
    /// terminals answer for it, and says it copied `what`.
    ///
    /// No clipboard crate: those pull in x11 or wayland C libraries, and the
    /// terminal already knows how to do this. tmux needs `set-clipboard on`.
    pub fn copy_to_clipboard(&mut self, text: &str, what: &str) {
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
            self.info(format!("copied {what}"));
        } else {
            self.error("cannot reach the terminal clipboard");
        }
    }
}
