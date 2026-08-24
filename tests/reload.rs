//! A rescan must not lose what is playing.
//!
//! `view`, `queue` and `playing` are all indices into `tracks`, and a rescan
//! rebuilds `tracks` from the disk, so every one of them means something else
//! afterwards. Dropping them was worse than repointing them: the sound thread
//! carried on while the statusline said nothing was playing, and `gp` had
//! nowhere to jump to.

use std::fs;
use std::path::{Path, PathBuf};

use vibox::app::App;
use vibox::library::{QUIET, SortKey};

/// An empty file with an audio extension is a track: the scan falls back to
/// the filename when there are no tags to read.
fn touch(dir: &Path, name: &str) {
    fs::write(dir.join(name), b"").expect("the temp dir should be writable");
}

fn paths(app: &App, rows: &[usize]) -> Vec<PathBuf> {
    rows.iter().map(|&i| app.tracks[i].path.clone()).collect()
}

#[test]
fn a_rescan_keeps_the_playing_track_and_its_queue() {
    let dir = tempfile::tempdir().unwrap();
    for name in ["b.mp3", "c.mp3", "d.mp3"] {
        touch(dir.path(), name);
    }

    let mut app = App::new(dir.path().to_path_buf(), SortKey::Path, QUIET).unwrap();
    // What `play_cursor` does: the view as it stands becomes the queue.
    app.queue = app.view.clone();
    app.qpos = 2;
    app.playing = app.view.get(2).copied();

    let playing = app
        .playing_track()
        .expect("row 3 should be a track")
        .path
        .clone();
    let queued = paths(&app, &app.queue.clone());

    // A file that sorts in front of every other one, so every index that was
    // right a moment ago is now off by one.
    touch(dir.path(), "a.mp3");
    app.reload().unwrap();

    assert_eq!(
        app.tracks.len(),
        4,
        "the new file should have been picked up"
    );
    assert_eq!(
        app.playing_track().map(|t| t.path.clone()),
        Some(playing),
        "the track that is playing should still be the one `playing` points at"
    );
    assert_eq!(
        paths(&app, &app.queue.clone()),
        queued,
        "the queue is a snapshot and keeps its own contents and order"
    );
    assert_eq!(app.qpos, 2, "and the position in it should not move");
}

#[test]
fn a_playing_track_that_left_the_library_stops_being_the_playing_track() {
    let dir = tempfile::tempdir().unwrap();
    for name in ["a.mp3", "b.mp3"] {
        touch(dir.path(), name);
    }

    let mut app = App::new(dir.path().to_path_buf(), SortKey::Path, QUIET).unwrap();
    app.queue = app.view.clone();
    app.qpos = 1;
    app.playing = app.view.get(1).copied();

    fs::remove_file(dir.path().join("b.mp3")).unwrap();
    app.reload().unwrap();

    assert!(
        app.playing_track().is_none(),
        "a track that is gone cannot be the one playing"
    );
    assert_eq!(
        paths(&app, &app.queue.clone()),
        vec![dir.path().join("a.mp3")],
        "and it drops out of the queue rather than shifting it"
    );
}
