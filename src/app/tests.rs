//! Tests for `App`, which is the thing under test rather than any one file.

use super::*;

/// Types a whole name into whichever buffer `c` opened.
fn set_name(app: &mut App, name: &str) {
    if let Some(buf) = app.name_buffer() {
        buf.clear();
        for c in name.chars() {
            buf.insert(c);
        }
    }
}

/// A real library on disk, since `App` is the thing under test.
///
/// Everything a test can write lives under the returned directory, which is
/// deleted when it drops. Playlists included: a test that wrote into the
/// user's own playlist folder would leave rubbish on a contributor's
/// machine, and could overwrite a playlist they actually listen to.
fn library(files: &[&str]) -> (App, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    for name in files {
        let path = dir.path().join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"").unwrap();
    }

    let mut app = App::new(dir.path().to_path_buf(), SortKey::Path, library::QUIET).unwrap();
    app.playlists_dir = Some(dir.path().join(".playlists"));
    app.reload_playlists();
    (app, dir)
}

/// `:set` speaks vim's dialect, whatever the option is: bare turns on, `no`
/// turns off, `!` flips, and an unknown name is refused rather than
/// silently doing nothing.
#[test]
fn set_turns_options_on_off_and_over() {
    let (mut app, _dir) = library(&["a.mp3"]);

    crate::excmd::run(&mut app, "set nokaraoke");
    assert!(!app.karaoke);
    crate::excmd::run(&mut app, "set karaoke");
    assert!(app.karaoke);
    crate::excmd::run(&mut app, "set karaoke!");
    assert!(!app.karaoke);

    crate::excmd::run(&mut app, "set noartist");
    assert!(!app.columns.get("artist").unwrap());

    crate::excmd::run(&mut app, "set nonsense");
    assert!(
        app.msg.is_some(),
        "an option vibox does not have has to say so"
    );
}

/// The rule danger exists for: it is off on every start unless the rc file
/// asked for it, and quitting with it on must not be what asks.
#[test]
fn danger_is_never_on_because_a_previous_session_had_it_on() {
    let (mut app, _dir) = library(&["a.mp3"]);
    assert!(!app.danger, "off until it is asked for");

    crate::excmd::run(&mut app, "set danger");
    assert!(app.danger);

    // What quitting writes down, and a new process reads back on the way in.
    let restored = crate::excmd::saved_options(&app);
    assert!(
        !restored.contains("danger"),
        "danger reached the saved session as `{restored}`"
    );
}

#[test]
fn moving_the_folder_cursor_leaves_the_track_list_alone() {
    let (mut app, _dir) = library(&["a.mp3", "jazz/b.mp3"]);
    assert_eq!(app.view.len(), 2, "everything is shown to begin with");

    app.move_folder(1);
    assert_eq!(app.view.len(), 2, "the cursor moved, the view did not");

    app.open_folder();
    assert_eq!(app.view.len(), 1, "enter is what opens a folder");
    assert_eq!(app.focus, Pane::Tracks);
}

#[test]
fn a_move_onto_an_existing_file_stops_the_whole_batch() {
    let (mut app, dir) = library(&["a.mp3", "b.mp3", "jazz/a.mp3"]);
    let root = dir.path().to_path_buf();

    app.moves = vec![
        (root.join("b.mp3"), root.join("jazz/b.mp3")),
        (root.join("a.mp3"), root.join("jazz/a.mp3")),
    ];
    assert!(app.move_problem().is_some(), "the clash has to be caught");

    app.write_all();
    assert!(
        root.join("b.mp3").exists(),
        "the file with no clash must not have moved either"
    );
    assert_eq!(app.moves.len(), 2, "everything stays pending");
}

#[test]
fn two_files_moved_onto_the_same_name_are_refused() {
    let (mut app, dir) = library(&["one/x.mp3", "two/x.mp3"]);
    let root = dir.path().to_path_buf();
    std::fs::create_dir_all(root.join("all")).unwrap();

    app.moves = vec![
        (root.join("one/x.mp3"), root.join("all/x.mp3")),
        (root.join("two/x.mp3"), root.join("all/x.mp3")),
    ];
    assert!(app.move_problem().is_some());
}

#[test]
fn a_clean_batch_of_moves_is_written() {
    let (mut app, dir) = library(&["a.mp3", "b.mp3"]);
    let root = dir.path().to_path_buf();
    std::fs::create_dir_all(root.join("jazz")).unwrap();

    app.moves = vec![
        (root.join("a.mp3"), root.join("jazz/a.mp3")),
        (root.join("b.mp3"), root.join("jazz/b.mp3")),
    ];
    app.write_all();

    assert!(root.join("jazz/a.mp3").exists());
    assert!(root.join("jazz/b.mp3").exists());
    assert!(!root.join("a.mp3").exists());
    assert!(app.moves.is_empty());
}

#[test]
fn deleting_a_track_repoints_everything_that_held_an_index() {
    let (mut app, dir) = library(&["a.mp3", "b.mp3", "c.mp3"]);
    let gone = dir.path().join("b.mp3");

    // The queue and an open playlist both hold indices into `tracks`.
    app.queue = vec![0, 1, 2];
    app.playlist_rows = vec![2, 1, 0];
    app.playing = Some(2);
    let last = app.tracks[2].path.clone();

    app.forget_tracks(&[gone]);

    assert_eq!(app.tracks.len(), 2);
    assert_eq!(app.queue.len(), 2, "the deleted track leaves the queue");
    assert_eq!(app.playlist_rows.len(), 2);
    assert_eq!(
        app.tracks[app.playing.unwrap()].path,
        last,
        "what is playing must still be the same file"
    );
    for &row in app.playlist_rows.iter().chain(app.queue.iter()) {
        assert!(row < app.tracks.len(), "no index may dangle");
    }
}

#[test]
fn changes_show_playlists_edited_in_another_tab() {
    let (mut app, _dir) = library(&["a.mp3"]);
    app.tabs.push(ViewTab {
        playlist: Some("roadtrip".into()),
        folder: 0,
        cur: 0,
        top: 0,
        sort_key: SortKey::Path,
        rows: vec![0],
        dirty: true,
    });

    assert!(app.unsaved(), "a change anywhere counts as unsaved");
    assert!(
        app.pending_changes().iter().any(|line| line.contains("roadtrip")),
        "`:changes` has to list what `:w` would write, in every tab"
    );
}

#[test]
fn a_playlist_is_a_view_and_does_not_become_the_library() {
    let (mut app, dir) = library(&["a.mp3", "jazz/b.mp3"]);
    let root = dir.path().to_path_buf();
    let before = app.tracks.len();

    app.create_playlist("mix");
    app.open_playlist();

    assert_eq!(app.root, root, "the library root never moves");
    assert_eq!(app.tracks.len(), before);
    assert!(!app.folders.is_empty(), "the folders tab still browses");
}

#[test]
fn danger_is_needed_before_a_key_can_delete_a_file() {
    let (mut app, _dir) = library(&["a.mp3"]);
    app.cut_tracks();
    assert!(app.doomed_files.is_empty(), "nothing marked without danger");
    assert!(app.msg.as_ref().is_some_and(|(_, error)| *error));

    app.danger = true;
    app.cut_tracks();
    assert_eq!(app.doomed_files.len(), 1);
    assert!(app.unsaved());
    assert!(app.view.is_empty(), "a marked file leaves the list at once");
}

#[test]
fn a_view_is_only_ever_open_in_one_tab() {
    let (mut app, _dir) = library(&["a.mp3", "jazz/b.mp3"]);
    app.create_playlist("mix");
    app.open_playlist();
    let tabs = app.tabs.len();

    // Asking for it again from somewhere else takes you back to it.
    app.tab = Tab::Playlists;
    app.open_in_new_tab();
    assert_eq!(app.tabs.len(), tabs, "no second tab of the same playlist");
    assert_eq!(app.playlist_view.as_deref(), Some("mix"));
}

#[test]
fn renaming_the_playing_track_keeps_playback_pointed_at_it() {
    let (mut app, _dir) = library(&["a.mp3", "b.mp3"]);
    app.queue = app.view.clone();
    app.qpos = 0;
    app.playing = Some(app.view[0]);

    app.begin_edit();
    set_name(&mut app, "renamed");
    app.apply_edits();

    let playing = app.playing.expect("still playing something");
    assert_eq!(app.tracks[playing].file, "renamed");
    assert_eq!(
        app.tracks[playing].path.file_name().unwrap(),
        "renamed.mp3",
        "the track that is playing has to follow its own rename"
    );
    assert!(app.tracks[playing].path.exists());
}

#[test]
fn a_folder_takes_everything_under_it_including_subfolders() {
    let (mut app, dir) = library(&["jazz/a.mp3", "jazz/live/b.mp3", "rock/c.mp3"]);
    app.danger = true;
    // row 0 is the whole library, so jazz is the first real folder
    app.folder_cur = 1 + app
        .folders
        .iter()
        .position(|(label, _)| label == "jazz")
        .unwrap();
    app.cut_folder();

    let listed = app.pending_changes().join("\n");
    assert!(listed.contains("a.mp3"), "every track is listed on its own");
    assert!(listed.contains("b.mp3"), "a subfolder's tracks come too");
    assert!(!listed.contains("c.mp3"), "another folder is untouched");

    app.write_all();
    assert!(!dir.path().join("jazz").exists(), "the folder is gone");
    assert!(dir.path().join("rock/c.mp3").exists());
}

#[test]
fn a_marked_folder_is_unmarked_by_pressing_dd_again() {
    let (mut app, _dir) = library(&["jazz/a.mp3"]);
    app.danger = true;
    app.folder_cur = 1;
    app.cut_folder();
    assert!(app.unsaved());

    app.cut_folder();
    assert!(!app.unsaved(), "the second dd puts it back");
    assert!(app.doomed_files.is_empty());
}

#[test]
fn renaming_a_folder_is_one_rename_and_the_tracks_follow() {
    let (mut app, dir) = library(&["jazz/a.mp3", "jazz/live/b.mp3"]);
    app.folder_cur = 1 + app
        .folders
        .iter()
        .position(|(label, _)| label == "jazz")
        .unwrap();

    app.begin_sidebar_edit();
    set_name(&mut app, "Jazz");
    assert!(app.edit_dirty());
    app.apply_edits();

    assert!(dir.path().join("Jazz/a.mp3").exists());
    assert!(dir.path().join("Jazz/live/b.mp3").exists(), "subfolders come too");
    assert!(!dir.path().join("jazz").exists());
    for track in &app.tracks {
        assert!(track.path.starts_with(dir.path().join("Jazz")));
    }
}

#[test]
fn renaming_a_playlist_keeps_the_tab_showing_it() {
    let (mut app, _dir) = library(&["a.mp3"]);
    app.create_playlist("mix");
    app.open_playlist();
    app.tab = Tab::Playlists;

    app.begin_sidebar_edit();
    set_name(&mut app, "roadtrip");
    app.apply_edits();

    assert_eq!(app.playlist_view.as_deref(), Some("roadtrip"));
    assert!(app.playlists.iter().any(|(name, _)| name == "roadtrip"));
    assert!(!app.playlists.iter().any(|(name, _)| name == "mix"));
}

#[test]
fn dd_in_a_playlist_cuts_so_p_can_put_it_back() {
    let (mut app, _dir) = library(&["a.mp3", "b.mp3", "c.mp3"]);
    // yank the three tracks from the library, then fill a playlist
    app.mode = Mode::Visual;
    app.visual_anchor = Some(0);
    app.cur = 2;
    app.yank_selection();

    app.create_playlist("mix");
    app.open_playlist();
    app.paste_into_playlist();
    let order: Vec<String> = app.view.iter().map(|&i| app.tracks[i].file.clone()).collect();
    assert_eq!(order, ["a", "b", "c"]);

    app.cur = 0;
    app.remove_from_playlist();
    assert_eq!(app.yank.len(), 1, "a cut fills the register");

    app.cur = 1;
    app.paste_into_playlist();
    let order: Vec<String> = app.view.iter().map(|&i| app.tracks[i].file.clone()).collect();
    assert_eq!(order, ["b", "c", "a"], "dd then p is how a track moves");
}

#[test]
fn deleting_a_playlist_leaves_every_track_alone() {
    let (mut app, dir) = library(&["a.mp3", "b.mp3"]);
    app.mode = Mode::Visual;
    app.visual_anchor = Some(0);
    app.cur = 1;
    app.yank_selection();

    app.create_playlist("mix");
    app.open_playlist();
    app.paste_into_playlist();
    app.write_all();

    app.tab = Tab::Playlists;
    app.pl_cur = 0;
    app.delete_playlist();
    app.write_all();

    assert!(app.playlists.is_empty(), "the m3u is gone");
    assert!(dir.path().join("a.mp3").exists(), "its tracks are not");
    assert!(dir.path().join("b.mp3").exists());
    assert_eq!(app.tracks.len(), 2);
}

#[test]
fn shuffle_goes_back_to_what_was_actually_played() {
    let (mut app, _dir) = library(&["a.mp3", "b.mp3", "c.mp3", "d.mp3", "e.mp3"]);
    app.shuffle = true;
    app.queue = app.view.clone();
    app.qpos = 0;
    app.history.clear();

    let mut heard = vec![app.qpos];
    for _ in 0..3 {
        app.advance(1, false);
        heard.push(app.qpos);
    }

    // Walking back has to retrace those steps, not the queue order.
    for expected in heard.iter().rev().skip(1) {
        app.advance(-1, false);
        assert_eq!(app.qpos, *expected);
    }
}

#[test]
fn e_bang_throws_away_every_kind_of_pending_change() {
    let (mut app, dir) = library(&["a.mp3", "b.mp3", "jazz/c.mp3"]);
    app.danger = true;

    // a rename, a cut file, and a playlist edit, all waiting
    app.begin_edit();
    set_name(&mut app, "renamed");
    app.commit_name();

    app.cur = 1;
    app.cut_tracks();

    app.mode = Mode::Visual;
    app.visual_anchor = Some(0);
    app.cur = 0;
    app.yank_selection();
    app.create_playlist("mix");
    app.open_playlist();
    app.paste_into_playlist();

    assert!(app.unsaved());
    assert!(!app.pending_changes().is_empty());

    app.discard_changes();

    assert!(!app.unsaved(), "nothing is waiting any more");
    assert!(app.pending_changes().is_empty(), "`:changes` comes back empty");
    assert!(app.renames.is_empty());
    assert!(app.doomed_files.is_empty());
    assert!(!app.playlist_dirty);

    // and the disk was never touched by any of it
    assert!(dir.path().join("a.mp3").exists());
    assert!(dir.path().join("b.mp3").exists());
    assert_eq!(app.tracks.len(), 3, "the cut track is back in the list");
}

#[test]
fn a_playlist_of_outside_tracks_never_joins_the_library() {
    let (mut app, dir) = library(&["a.mp3", "b.mp3"]);

    // a playlist naming a file that lives outside the library root
    let outside = tempfile::tempdir().unwrap();
    let far = outside.path().join("HOLA/far.mp3");
    std::fs::create_dir_all(far.parent().unwrap()).unwrap();
    std::fs::write(&far, b"").unwrap();
    let m3u = app.playlists_dir.clone().unwrap().join("test.m3u");
    std::fs::create_dir_all(m3u.parent().unwrap()).unwrap();
    std::fs::write(&m3u, format!("#EXTM3U\n{}\n", far.display())).unwrap();
    app.reload_playlists();

    app.tab = Tab::Playlists;
    app.pl_cur = 0;
    app.open_playlist();
    assert_eq!(app.view.len(), 1, "the playlist shows its track");

    // back to everything, exactly as pressing gt and enter does
    app.tab = Tab::Folders;
    app.folder_cur = 0;
    app.open_folder();

    assert_eq!(
        app.view.len(),
        2,
        "everything is the library, not the library plus a playlist's strays"
    );
    assert_eq!(
        app.library_len(),
        app.view.len(),
        "the count beside `everything` counts what `everything` shows"
    );
    assert!(
        !app.folders.iter().any(|(label, _)| label.contains("HOLA")),
        "a folder outside the root has no business in the folder list"
    );
    assert!(dir.path().join("a.mp3").exists());
}

/// An m3u opened as the root is the opposite case: its tracks live wherever
/// they like, and all of them are the library, because that is what the user
/// asked to open.
#[test]
fn an_m3u_opened_as_the_root_makes_its_tracks_the_library() {
    let dir = tempfile::tempdir().unwrap();
    let far = dir.path().join("somewhere/else/far.mp3");
    std::fs::create_dir_all(far.parent().unwrap()).unwrap();
    std::fs::write(&far, b"").unwrap();

    let m3u = dir.path().join("list.m3u");
    std::fs::write(&m3u, format!("#EXTM3U\n{}\n", far.display())).unwrap();

    let app = App::new(m3u, SortKey::Path, library::QUIET).unwrap();

    assert_eq!(app.view.len(), 1, "the m3u's track is shown");
    assert_eq!(
        app.library_len(),
        1,
        "a track named by the root m3u is the library, not a stray"
    );
}

#[test]
fn undo_takes_back_a_pending_deletion() {
    let (mut app, _dir) = library(&["a.mp3", "b.mp3"]);
    app.danger = true;
    app.cut_tracks();
    assert_eq!(app.view.len(), 1);

    app.undo();
    assert!(app.doomed_files.is_empty());
    assert_eq!(app.view.len(), 2, "the row comes back");
    assert!(!app.unsaved());
}
