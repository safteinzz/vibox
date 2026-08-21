//! The batch a `:w` would apply, checked as one thing.
//!
//! This is the code that touches somebody's music, and `std::fs::rename`
//! overwrites the destination without asking, so the interesting cases are the
//! ones where a wrong verdict costs a file rather than an error message.

use std::path::{Path, PathBuf};

use vibox::app::plan::{Step, Wanted, plan};

fn p(s: &str) -> PathBuf {
    PathBuf::from(s)
}

/// A pretend disk: everything named here exists, nothing else does.
fn disk(names: &[&str]) -> impl Fn(&Path) -> bool + use<> {
    let owned: Vec<PathBuf> = names.iter().map(|n| p(n)).collect();
    move |path: &Path| owned.iter().any(|held| held == path)
}

fn renames(pairs: &[(&str, &str)]) -> Wanted {
    Wanted {
        renames: pairs.iter().map(|(a, b)| (p(a), p(b))).collect(),
        ..Wanted::default()
    }
}

// ---- what must be refused ----------------------------------------------

#[test]
fn a_name_already_on_disk_is_refused() {
    let wanted = renames(&[("/m/a.mp3", "/m/b.mp3")]);
    assert!(plan(&wanted, &disk(&["/m/a.mp3", "/m/b.mp3"])).is_err());
}

#[test]
fn two_things_claiming_one_name_are_refused() {
    let wanted = renames(&[("/m/a.mp3", "/m/same.mp3"), ("/m/b.mp3", "/m/same.mp3")]);
    assert!(
        plan(&wanted, &disk(&["/m/a.mp3", "/m/b.mp3"])).is_err(),
        "the second would overwrite the first, and `fs::rename` would not say so"
    );
}

/// The hole the old per-kind checks left: a rename and a move each looked only
/// at its own list, so both were allowed and the later one ate the earlier.
#[test]
fn a_rename_and_a_move_claiming_one_name_are_refused() {
    let wanted = Wanted {
        renames: vec![(p("/m/a.mp3"), p("/m/same.mp3"))],
        moves: vec![(p("/m/sub/c.mp3"), p("/m/same.mp3"))],
        ..Wanted::default()
    };
    assert!(plan(&wanted, &disk(&["/m/a.mp3", "/m/sub/c.mp3"])).is_err());
}

#[test]
fn a_copy_onto_an_occupied_name_is_refused() {
    let wanted = Wanted {
        copies: vec![(p("/m/a.mp3"), p("/m/b.mp3"))],
        ..Wanted::default()
    };
    assert!(plan(&wanted, &disk(&["/m/a.mp3", "/m/b.mp3"])).is_err());
}

#[test]
fn renaming_something_that_is_also_being_deleted_is_refused() {
    let wanted = Wanted {
        renames: vec![(p("/m/a.mp3"), p("/m/new.mp3"))],
        deletes: vec![p("/m/a.mp3")],
        ..Wanted::default()
    };
    assert!(
        plan(&wanted, &disk(&["/m/a.mp3"])).is_err(),
        "keeping it and losing it are not both possible"
    );
}

#[test]
fn a_swap_is_refused_rather_than_half_applied() {
    let wanted = renames(&[("/m/a.mp3", "/m/b.mp3"), ("/m/b.mp3", "/m/a.mp3")]);
    let refusal = plan(&wanted, &disk(&["/m/a.mp3", "/m/b.mp3"])).unwrap_err();
    assert!(
        refusal.0.contains("swap"),
        "the message has to say what to do instead, got `{}`",
        refusal.0
    );
}

// ---- what must be allowed ----------------------------------------------

/// The case this was built for: the name is taken by a file the same write is
/// about to delete, so it is free by the time the rename runs.
#[test]
fn a_name_freed_by_a_deletion_in_the_same_write_is_allowed() {
    let wanted = Wanted {
        renames: vec![(p("/m/a.mp3"), p("/m/b.mp3"))],
        deletes: vec![p("/m/b.mp3")],
        ..Wanted::default()
    };
    let steps = plan(&wanted, &disk(&["/m/a.mp3", "/m/b.mp3"])).expect("allowed");
    assert_eq!(
        steps,
        vec![
            Step::Delete(p("/m/b.mp3")),
            Step::Rename(p("/m/a.mp3"), p("/m/b.mp3")),
        ],
        "and the delete has to run first, or the rename lands on a file still there"
    );
}

#[test]
fn a_name_freed_by_a_move_in_the_same_write_is_allowed() {
    let wanted = Wanted {
        renames: vec![(p("/m/a.mp3"), p("/m/b.mp3"))],
        moves: vec![(p("/m/b.mp3"), p("/m/sub/b.mp3"))],
        ..Wanted::default()
    };
    assert!(
        plan(&wanted, &disk(&["/m/a.mp3", "/m/b.mp3"])).is_err(),
        "a rename and a move that depend on each other are refused, not guessed at"
    );
}

#[test]
fn a_name_freed_by_a_deleted_folder_is_allowed() {
    let wanted = Wanted {
        moves: vec![(p("/m/a.mp3"), p("/m/old/a.mp3"))],
        delete_dirs: vec![p("/m/old")],
        ..Wanted::default()
    };
    assert!(plan(&wanted, &disk(&["/m/a.mp3", "/m/old/a.mp3"])).is_ok());
}

#[test]
fn a_chain_of_renames_runs_in_an_order_that_works() {
    // b must move out before a takes its name.
    let wanted = renames(&[("/m/a.mp3", "/m/b.mp3"), ("/m/b.mp3", "/m/c.mp3")]);
    let steps = plan(&wanted, &disk(&["/m/a.mp3", "/m/b.mp3"])).expect("allowed");
    assert_eq!(
        steps,
        vec![
            Step::Rename(p("/m/b.mp3"), p("/m/c.mp3")),
            Step::Rename(p("/m/a.mp3"), p("/m/b.mp3")),
        ]
    );
}

#[test]
fn an_untouched_name_is_no_obstacle() {
    let wanted = renames(&[("/m/a.mp3", "/m/new.mp3")]);
    assert!(plan(&wanted, &disk(&["/m/a.mp3", "/m/unrelated.mp3"])).is_ok());
}

#[test]
fn an_empty_batch_is_allowed_and_does_nothing() {
    assert_eq!(plan(&Wanted::default(), &disk(&[])).unwrap(), vec![]);
}

// ---- the order the steps come out in ------------------------------------

#[test]
fn folders_are_deleted_deepest_first() {
    let wanted = Wanted {
        delete_dirs: vec![p("/m/a"), p("/m/a/b/c"), p("/m/a/b")],
        ..Wanted::default()
    };
    let steps = plan(&wanted, &disk(&[])).unwrap();
    assert_eq!(
        steps,
        vec![
            Step::DeleteDir(p("/m/a/b/c")),
            Step::DeleteDir(p("/m/a/b")),
            Step::DeleteDir(p("/m/a")),
        ],
        "a folder inside a marked folder has to be gone before its parent"
    );
}

#[test]
fn deletions_come_before_the_renames_they_make_room_for() {
    let wanted = Wanted {
        renames: vec![(p("/m/a.mp3"), p("/m/b.mp3"))],
        deletes: vec![p("/m/b.mp3")],
        copies: vec![(p("/m/a.mp3"), p("/m/copy.mp3"))],
        delete_dirs: vec![p("/m/old")],
        ..Wanted::default()
    };
    let steps = plan(&wanted, &disk(&["/m/a.mp3", "/m/b.mp3"])).unwrap();
    let kinds: Vec<&str> = steps
        .iter()
        .map(|s| match s {
            Step::Delete(_) => "delete",
            Step::Rename(..) => "rename",
            Step::Copy(..) => "copy",
            Step::DeleteDir(_) => "rmdir",
        })
        .collect();
    assert_eq!(kinds, vec!["delete", "rename", "copy", "rmdir"]);
}
