<!--
AI-ONLY DOCUMENT. This file exists to give an AI agent the COMPLETE operating picture for this repo. Optimize for completeness and precision for the agent, not for human readability. Humans read README.md instead. Do not remove detail to make this nicer, err toward more explicit, not less. FORMAT: machine-read, not a formatted human doc. Do NOT hard-wrap lines to a column width for readability; put each rule/point on ONE line, however long.
-->
# AGENTS.md

## Hard rules
- **Commit, push, and publish only when the user says to ship.** They test interactively first; a mid-work commit is never the deliverable.
- Release flow, in this exact order: `cargo clippy` warning-clean (+ `cargo test` if a suite exists) → bump `version` in `Cargo.toml` → one commit (short conventional message, never co-authored) → `git push origin main` → `cargo publish` (dry-run first; publishing is irreversible) → **tag only after publish succeeds**: `git tag vX.Y.Z && git push origin --tags`. A tag must never point at a version that failed to publish.
- Commit messages: short conventional tags (`feat:`, `fix:`, ...). **Never** add a `Co-Authored-By` trailer.
- **No em-dashes** anywhere user-facing (README, --help, crate description, commit messages, prose) - they read as AI-generated text.
- **Never add a dependency that needs a C library, system headers, or an external binary.** `cargo install vibox` must succeed on a bare toolchain; a build that fails on a missing header is a broken product. This is why the audio output is written against the pulseaudio wire protocol instead of using cpal.
- **Test by running `./target/release/vibox` directly. Never `cargo install` to test**, it replaces the binary on PATH with a work-in-progress build; install only when the user asks.
- **The only write vibox performs on a music library is a rename the user typed in the edit buffer and confirmed with `:w`.** Everything else is read only, and no code path may add, delete, or retag a file.
- **Renames are validated as a batch before any of them runs** (no empty name, no `/`, no overwriting an existing file), so a bad name aborts the whole write instead of leaving a folder half renamed.
- Automated tests are deliberately absent while the interface is still moving; add them (ex command parsing, motions, m3u round trip) when the user says the design is settled.
- Fix the root cause. If a workaround must ship, say the word "workaround" out loud, so a silent patch never passes as a real fix. Same for lints: never `#[allow]` a warning away; delete or fix the code it points at.
- `TODO-LIST.md` (gitignored) holds one-line ideas; delete the line when the idea ships.

## Invariants and gotchas
- **Visual mode, yank, queue keys and playlist writing were deliberately removed before the first release** and are recorded in `TODO-LIST.md`; they come back with playlists, so do not re-add them piecemeal.
- **When adding a key:** every binding lives in `keys.rs`. Multi-key sequences go through `app.pending` (one of `g`, `z`, `d`, `y`, `Z`, ctrl-w) and are consumed before counts are read; the `KeyEventKind::Press` filter at the top of `handle` is mandatory, because terminals speaking the kitty protocol also send release events and every motion doubles without it.
- **When adding a key, do not require a prefix for anything ordinary.** vibox is expected to run inside tmux, so a binding that needs a leader or a `ctrl-b` style chord fights the multiplexer; single presses and vi sequences only.
- **When touching audio:** `player.rs` runs one thread that owns both the pulseaudio socket and the decoder. The ui sends `Cmd`s over a channel and reads position, paused and finished out of atomics. Never do blocking audio work on the ui thread.
- **When changing pause:** pause is `CorkPlaybackStream`, and a corked stream gets no `Request` messages from the server, so the thread must block on the command channel and not on the socket; reading the socket while corked deadlocks until the user resumes.
- **When changing seek or the progress bar:** position is `base + (fed - inflight) / rate`, where `inflight` is `write_offset - read_offset` from the timing reply. A seek sends `FlushPlaybackStream`, which drops the server buffer, so `fed` resets to 0 and `base` becomes the seek target; forgetting either makes the clock lie.
- **When touching decoding:** rodio is `default-features = false` with decoder features only. Enabling its `playback` feature pulls in cpal and alsa-sys and breaks the no-system-headers rule.
- **When reading tags:** a tag that is present but blank counts as absent, otherwise the row renders empty and the file looks missing. The filename without its extension is the fallback and is stored in `Track::file`.
- **The track list shows `Track::file`, not `Track::title`.** Tags fill the artist and album columns, the statusline and the mpris metadata; the filename is the identity in the list.
- **When changing the track row:** `ui::columns()` and the header string must stay in agreement, and the fixed width is 10 columns plus whatever `duration_width()` returns (7 once anything in view runs past an hour, 5 otherwise).
- **When changing the statusline:** only the now-playing segment may stretch. Everything else is fixed width and must be subtracted first, or a large library gets the last digit of its track count clipped.
- **When changing the layout:** `app.track_h` is written by the renderer every frame and the paging keys depend on it; the sticky header is a separate one row area and must be subtracted from it.
- **`view` holds indices into `tracks`; `queue` is a snapshot of `view` taken when playback starts.** Filtering or sorting therefore never disturbs what is playing, and code that assumes `queue` matches the current view is wrong.
- **When touching mpris:** `mpris.rs` claims `org.mpris.MediaPlayer2.vibox` and falls back to an instance suffixed name so a second process still starts. The ui publishes a snapshot from `App::tick`; a watcher thread diffs that snapshot and emits PropertiesChanged, because desktop widgets update off the signal and never poll.
- **Degrade, never abort:** `App::audio` and `App::mpris` are both `Option`. No audio device or no session bus means vibox still browses, and every affected key reports one line on the message row.
- **Never enable mouse capture.** It takes selection and scrollback away from the terminal and from tmux.
- **Playlists are read only for now:** `:e` opens an m3u and it keeps its own order. `library::scan` skips the path sort for playlists and `App::reload` skips re-sorting when the sort key is the default, so a rewrite of either must preserve that.
- **When touching search:** the haystack is the columns of the row only. Including the path made every track under a directory match that directory's name, so `/testing` inside `TESTING/` matched all of them.
- Errors reaching the user are lowercase, name things in backticks, and say what to do next. A raw OS error is a bug: the terminal check in `main` exists because ratatui otherwise panics with `Os { code: 6 }` when stdout is not a tty.

## Build / lint / test
- `cargo build --release`
- `cargo clippy --release` must be warning-clean before any release.
- Run the release binary against a scratch directory of audio files: `./target/release/vibox <dir>`.
- **Do not drive the interface to test it.** Build it, say what changed and what to look at, and let the user run it; they see the screen instantly and an agent driving a pty is slow and wrong about what it looks like. Logic that is not visual (scanning, tag reading, search matching, ex command parsing) can still be checked directly, for example behind a temporary env var branch in `main` that prints results and is removed afterwards.
- Check the mpris surface while it runs: `busctl --user call org.mpris.MediaPlayer2.vibox /org/mpris/MediaPlayer2 org.mpris.MediaPlayer2.Player PlayPause`, and `busctl --user get-property org.mpris.MediaPlayer2.vibox /org/mpris/MediaPlayer2 org.mpris.MediaPlayer2.Player Metadata`.

## Overview
`vibox` is a Rust TUI music player, AGPL-3.0-only: a jukebox you exit with `:q`. The library is a directory (or an m3u), a track is a file, and the interface is modal in the vi sense: normal, visual, search and ex command modes, vi motions with counts, `/` and `:` on the last screen line, and a two pane layout of folders and tracks. `src/app.rs` holds all state and the playback queue, `keys.rs` the keymap, `excmd.rs` the `:` commands, `ui.rs` the rendering, `library.rs` the scan and the tag reading, `player.rs` decoding and pulseaudio output, `mpris.rs` the d-bus interface that makes media keys work. Linux only for now; the output layer is the only platform specific part.

## Self-repair
If this file contradicts the code, **the code wins** - fix AGENTS.md in the same session you notice.
