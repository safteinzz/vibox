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
    assert!(app.write_plan().is_err(), "the clash has to be caught");

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
    assert!(app.write_plan().is_err());
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
        app.pending_changes()
            .iter()
            .any(|line| line.contains("roadtrip")),
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
    app.write_all();

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
    app.write_all();

    assert!(dir.path().join("Jazz/a.mp3").exists());
    assert!(
        dir.path().join("Jazz/live/b.mp3").exists(),
        "subfolders come too"
    );
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
    app.write_all();

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
    let order: Vec<String> = app
        .view
        .iter()
        .map(|&i| app.tracks[i].file.clone())
        .collect();
    assert_eq!(order, ["a", "b", "c"]);

    app.cur = 0;
    app.remove_from_playlist();
    assert_eq!(app.yank.len(), 1, "a cut fills the register");

    app.cur = 1;
    app.paste_into_playlist();
    let order: Vec<String> = app
        .view
        .iter()
        .map(|&i| app.tracks[i].file.clone())
        .collect();
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
    assert!(
        app.pending_changes().is_empty(),
        "`:changes` comes back empty"
    );
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

/// Sorting reorders `tracks`, and `playing`, the queue and a playlist's rows
/// are all positions in it. A reorder that forgets them repoints the player at
/// a different song, which is the one thing a sort must never do.
#[test]
fn sorting_keeps_every_index_on_the_track_it_pointed_at() {
    let (mut app, _dir) = library(&["c.mp3", "a.mp3", "b.mp3"]);

    // Set by hand rather than played: a test machine has no audio device, and
    // these are indices either way.
    app.queue = (0..app.tracks.len()).collect();
    app.playlist_rows = vec![2, 0];
    app.playing = Some(0);

    let playing_before = app.playing_track().map(|t| t.file.clone());
    let queue_before: Vec<String> = app
        .queue
        .iter()
        .map(|&i| app.tracks[i].file.clone())
        .collect();
    let rows_before: Vec<String> = app
        .playlist_rows
        .iter()
        .map(|&i| app.tracks[i].file.clone())
        .collect();

    app.set_sort(SortKey::Title);

    assert_eq!(
        app.playing_track().map(|t| t.file.clone()),
        playing_before,
        "the same song is still the one playing"
    );
    let queue_after: Vec<String> = app
        .queue
        .iter()
        .map(|&i| app.tracks[i].file.clone())
        .collect();
    assert_eq!(
        queue_after, queue_before,
        "the queue still names the same songs, in the order it snapshotted them"
    );

    let rows_after: Vec<String> = app
        .playlist_rows
        .iter()
        .map(|&i| app.tracks[i].file.clone())
        .collect();
    assert_eq!(
        rows_after, rows_before,
        "a playlist keeps its own order through a sort of the library"
    );
}

/// A rename changes where a row belongs in the sort, so `:w` puts the list
/// back in order instead of leaving it to a manual `:sort`.
#[test]
fn writing_a_rename_puts_the_list_back_in_order() {
    let (mut app, _dir) = library(&["a.mp3", "b.mp3", "c.mp3"]);

    // rename the first row so it sorts last
    app.cur = 0;
    app.begin_edit();
    set_name(&mut app, "zzz");
    app.commit_name();
    app.write_all();

    let order: Vec<String> = app
        .view
        .iter()
        .map(|&i| app.tracks[i].file.clone())
        .collect();
    assert_eq!(
        order,
        vec!["b".to_string(), "c".to_string(), "zzz".to_string()],
        "the renamed row moved to where its new name sorts"
    );
}

/// A move rewrites paths, and the list is in path order, so left alone the
/// pasted rows jump to wherever the new name sorts and read as lost. They stay
/// under the cursor until the next sort or `:w`.
#[test]
fn pasted_files_stay_where_they_were_dropped() {
    let (mut app, _dir) = library(&["aaa.mp3", "mmm.mp3", "zzz/keep.mp3"]);
    app.danger = true;

    // cut the row that would sort first, then put it in the `zzz` folder
    app.cur = app
        .view
        .iter()
        .position(|&i| app.tracks[i].file == "aaa")
        .unwrap();
    app.cut_tracks();

    let zzz = app
        .folders
        .iter()
        .position(|(label, _)| label.contains("zzz"))
        .unwrap();
    app.folder_cur = zzz + 1;
    app.open_folder();

    // land on `keep`, then put
    app.cur = 0;
    assert!(app.move_cut_here(), "the cut is put into this folder");

    let names: Vec<String> = app
        .view
        .iter()
        .map(|&i| app.tracks[i].file.clone())
        .collect();
    assert_eq!(
        names,
        vec!["aaa".to_string(), "keep".to_string()],
        "the pasted row is at the cursor, not sorted away to the end"
    );

    // and a sort is what puts it back in order
    app.set_sort(SortKey::Path);
    assert_eq!(app.moves.len(), 1, "the move is still only pending");
}

/// The whole point of the batch check: a `:w` that cannot run must leave the
/// disk exactly as it found it, including the deletions, which are the part
/// that cannot be taken back.
#[test]
fn a_refused_write_deletes_nothing() {
    let (mut app, dir) = library(&["a.mp3", "b.mp3", "gone.mp3"]);
    app.danger = true;

    // mark one for deletion, and rename another onto a name already taken
    app.cur = app
        .view
        .iter()
        .position(|&i| app.tracks[i].file == "gone")
        .unwrap();
    app.cut_tracks();
    app.cur = app
        .view
        .iter()
        .position(|&i| app.tracks[i].file == "a")
        .unwrap();
    app.begin_edit();
    set_name(&mut app, "b");
    app.commit_name();

    app.write_all();

    assert!(
        dir.path().join("gone.mp3").exists(),
        "a refused write deletes nothing"
    );
    assert!(dir.path().join("a.mp3").exists(), "and renames nothing");
    assert!(dir.path().join("b.mp3").exists());
    assert!(app.unsaved(), "it is all still pending");
}

/// The case that started this: the name you want is taken by a song you are
/// deleting in the same write.
#[test]
fn a_rename_onto_a_song_being_deleted_goes_through() {
    let (mut app, dir) = library(&["keep.mp3", "dupe.mp3"]);
    app.danger = true;

    app.cur = app
        .view
        .iter()
        .position(|&i| app.tracks[i].file == "dupe")
        .unwrap();
    app.cut_tracks();

    app.cur = app
        .view
        .iter()
        .position(|&i| app.tracks[i].file == "keep")
        .unwrap();
    app.begin_edit();
    set_name(&mut app, "dupe");
    app.commit_name();

    app.write_all();

    assert!(
        dir.path().join("dupe.mp3").exists(),
        "the name was freed and taken"
    );
    assert!(!dir.path().join("keep.mp3").exists(), "the rename happened");
    assert_eq!(app.tracks.len(), 1, "one file, not two");
    assert!(!app.unsaved(), "nothing left pending");
}

/// `fs::rename` overwrites silently, so a file vibox does not even list has to
/// be safe from a rename that would land on it.
#[test]
fn a_rename_never_overwrites_a_file_that_is_not_a_track() {
    let (mut app, dir) = library(&["song.mp3"]);
    std::fs::write(dir.path().join("cover.mp3"), b"artwork").unwrap();

    // vibox has not scanned it as a track, but the name is taken on disk
    app.cur = 0;
    app.begin_edit();
    set_name(&mut app, "cover");
    app.commit_name();
    app.write_all();

    assert_eq!(
        std::fs::read(dir.path().join("cover.mp3")).unwrap(),
        b"artwork",
        "the file that was already there is untouched"
    );
    assert!(
        dir.path().join("song.mp3").exists(),
        "and the rename did not happen"
    );
}

/// Two rows renamed to the same thing must not leave the second quietly
/// overwriting the first.
#[test]
fn two_rows_renamed_to_one_name_are_refused() {
    let (mut app, dir) = library(&["a.mp3", "b.mp3"]);

    app.cur = 0;
    app.begin_edit();
    set_name(&mut app, "same");
    app.edit_next_row(1);
    set_name(&mut app, "same");
    app.commit_name();

    app.write_all();

    assert!(dir.path().join("a.mp3").exists());
    assert!(dir.path().join("b.mp3").exists());
    assert!(app.unsaved(), "both renames are still pending");
}

/// The hole that cost files rather than patience: a rename and a move both
/// claiming one name. Each was checked against its own list only, so both were
/// allowed, and `fs::rename` overwrote the first with the second without a
/// word.
#[test]
fn a_rename_and_a_move_onto_the_same_name_are_refused() {
    let (mut app, dir) = library(&["a.mp3", "sub/x.mp3"]);
    let root = dir.path().to_path_buf();
    app.danger = true;

    // the move: sub/x.mp3 into the root
    app.moves = vec![(root.join("sub/x.mp3"), root.join("x.mp3"))];

    // and a rename claiming the same name
    app.cur = app
        .view
        .iter()
        .position(|&i| app.tracks[i].file == "a")
        .unwrap();
    app.begin_edit();
    set_name(&mut app, "x");
    app.commit_name();

    app.write_all();

    assert!(root.join("a.mp3").exists(), "the rename did not run");
    assert!(root.join("sub/x.mp3").exists(), "and neither did the move");
    assert!(
        !root.join("x.mp3").exists(),
        "nothing was written, so nothing could be overwritten"
    );
}

/// Deleting a row you had already renamed drops the rename: you meant to be
/// rid of the file, and `:w` cannot both rename and delete one path.
#[test]
fn deleting_a_row_you_renamed_drops_the_rename() {
    let (mut app, dir) = library(&["a.mp3", "b.mp3"]);
    app.danger = true;

    app.cur = app
        .view
        .iter()
        .position(|&i| app.tracks[i].file == "a")
        .unwrap();
    app.begin_edit();
    set_name(&mut app, "renamed");
    app.commit_name();
    assert!(app.edit_dirty(), "the rename is pending");

    app.cut_tracks();
    assert!(!app.edit_dirty(), "and deleting the row takes it back");

    app.write_all();

    assert!(!dir.path().join("a.mp3").exists(), "the file is gone");
    assert!(
        !dir.path().join("renamed.mp3").exists(),
        "and was never renamed"
    );
    assert!(
        dir.path().join("b.mp3").exists(),
        "the other one is untouched"
    );
    assert!(!app.unsaved(), "nothing left pending");
}

/// Deleting one row must not take another row's pending rename with it.
#[test]
fn deleting_a_row_leaves_other_pending_renames_alone() {
    let (mut app, dir) = library(&["a.mp3", "b.mp3"]);
    app.danger = true;

    app.cur = app
        .view
        .iter()
        .position(|&i| app.tracks[i].file == "a")
        .unwrap();
    app.begin_edit();
    set_name(&mut app, "renamed");
    app.commit_name();

    // delete the other one
    app.cur = app
        .view
        .iter()
        .position(|&i| app.tracks[i].file == "b")
        .unwrap();
    app.cut_tracks();
    app.write_all();

    assert!(
        dir.path().join("renamed.mp3").exists(),
        "the rename still happened"
    );
    assert!(
        !dir.path().join("b.mp3").exists(),
        "and the deletion did too"
    );
}

/// A folder is the same rule one level up: deleting it drops the renames
/// pending on it and on everything inside it.
#[test]
fn deleting_a_folder_drops_the_renames_inside_it() {
    let (mut app, dir) = library(&["jazz/a.mp3", "keep.mp3"]);
    app.danger = true;

    // rename a track inside the folder
    let jazz = app
        .folders
        .iter()
        .position(|(label, _)| label == "jazz")
        .unwrap();
    app.folder_cur = jazz + 1;
    app.open_folder();
    app.cur = 0;
    app.begin_edit();
    set_name(&mut app, "renamed");
    app.commit_name();
    assert!(app.edit_dirty());

    // then delete the folder out from under it
    app.focus = Pane::Folders;
    app.folder_cur = jazz + 1;
    app.cut_folder();
    assert!(!app.edit_dirty(), "the rename inside went with the folder");

    app.write_all();
    assert!(!dir.path().join("jazz").exists(), "the folder is gone");
    assert!(dir.path().join("keep.mp3").exists());
}

/// And renaming the folder itself, then deleting it.
#[test]
fn deleting_a_folder_drops_the_rename_of_the_folder() {
    let (mut app, dir) = library(&["jazz/a.mp3"]);
    app.danger = true;

    let jazz = app
        .folders
        .iter()
        .position(|(label, _)| label == "jazz")
        .unwrap();
    app.folder_cur = jazz + 1;
    app.tab = Tab::Folders;
    app.begin_sidebar_edit();
    set_name(&mut app, "Jazz");
    app.commit_name();
    assert!(app.edit_dirty());

    app.cut_folder();
    assert!(!app.edit_dirty(), "the folder rename went with the folder");

    app.write_all();
    assert!(!dir.path().join("jazz").exists());
    assert!(
        !dir.path().join("Jazz").exists(),
        "and it was never renamed on the way out"
    );
}

/// Playlists are the third thing that can be renamed and deleted at once.
#[test]
fn deleting_a_playlist_drops_its_pending_rename() {
    let (mut app, _dir) = library(&["a.mp3"]);
    app.create_playlist("mix");
    app.tab = Tab::Playlists;
    app.pl_cur = 0;

    app.begin_sidebar_edit();
    set_name(&mut app, "roadtrip");
    app.commit_name();
    assert!(app.edit_dirty());

    app.delete_playlist();
    assert!(!app.edit_dirty(), "the rename went with the playlist");

    app.write_all();
    assert!(app.playlists.is_empty(), "the playlist is gone");
}

/// Undo has to put the rename back with the deletion it was dropped for.
#[test]
fn undoing_a_deletion_brings_its_rename_back() {
    let (mut app, _dir) = library(&["a.mp3", "b.mp3"]);
    app.danger = true;

    app.cur = app
        .view
        .iter()
        .position(|&i| app.tracks[i].file == "a")
        .unwrap();
    app.begin_edit();
    set_name(&mut app, "renamed");
    app.commit_name();

    app.cut_tracks();
    assert!(!app.edit_dirty());

    app.undo();
    assert!(app.edit_dirty(), "the rename is pending again");
    assert!(
        app.doomed_files.is_empty(),
        "and nothing is marked for deletion"
    );
}
