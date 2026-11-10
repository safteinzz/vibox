//! What a `:w` would do to the filesystem, checked as one thing.
//!
//! Renames, moves, copies and deletions were each validated on their own
//! before this existed, which meant none of them could see the others: a
//! rename onto a name a deletion was about to free was refused, and a rename
//! and a move claiming the same name were both allowed, the second silently
//! overwriting the first. `std::fs::rename` overwrites without asking, so that
//! second one ate files.
//!
//! The model here is a namespace rather than a list of checks:
//!
//! - **occupied** is what is on disk, asked of the filesystem and never of
//!   `app.tracks`. `tracks` holds only audio, so a tracks based check would
//!   happily overwrite `cover.jpg`; and on a case insensitive mount `song.mp3`
//!   and `SONG.mp3` are one file but two strings, where only the filesystem
//!   knows the truth.
//! - **vacated** is every path this batch empties: rename and move sources,
//!   and deleted files.
//! - **claimed** is every path it fills: rename, move and copy targets.
//!
//! A claim is a clash when the path is occupied and not vacated, when two
//! claims want the same path, or when it lies in a folder being deleted. The
//! filesystem is the only thing that can say "occupied", and the in-memory sets
//! can only ever excuse a clash, never invent one. That asymmetry is what keeps
//! a wrong set from costing a file.
//!
//! The order is worked out from what each step frees, takes and reads, across
//! every kind at once, and the write runs the steps exactly in that order.

use std::collections::{HashMap, HashSet};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

/// One thing to do, in the order the plan says to do it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// A file that goes, freeing its name for anything claiming it.
    Delete(PathBuf),
    /// A rename or a move: the same syscall, and the same hazard.
    Rename(PathBuf, PathBuf),
    Copy(PathBuf, PathBuf),
    /// A folder and everything under it.
    DeleteDir(PathBuf),
}

/// What a batch wants to do, before it is known to be possible.
#[derive(Default, Debug)]
pub struct Wanted {
    pub renames: Vec<(PathBuf, PathBuf)>,
    pub moves: Vec<(PathBuf, PathBuf)>,
    /// Each reads its source as the file there before anything runs.
    pub copies: Vec<(PathBuf, PathBuf)>,
    /// Each reads its source once a rename or a move in the batch has put a
    /// file there, which a copy back onto the name its file leaves needs.
    pub copies_after: Vec<(PathBuf, PathBuf)>,
    pub deletes: Vec<PathBuf>,
    pub delete_dirs: Vec<PathBuf>,
}

/// Why a batch cannot run, phrased for the message row.
#[derive(Debug)]
pub struct Refusal(pub String);

/// Checks a whole batch and puts it in an order that can actually run.
///
/// `exists` is the filesystem, taken as an argument so a test can hand over a
/// fake disk without writing one.
pub fn plan(wanted: &Wanted, exists: &dyn Fn(&Path) -> bool) -> Result<Vec<Step>, Refusal> {
    plan_with(wanted, exists, &|_, _| false, &|_| false)
}

/// `plan` on a disk that may spell one name two ways: `alias(a, b)` says
/// whether two different paths are one name there, which a disk that ignores
/// case says of `song.mp3` and `SONG.mp3`, and `folds(dir)` whether names in
/// that folder are told apart without their case. A name freed under one
/// spelling is then free under the other, and two claims that differ only in
/// case are one name there.
pub fn plan_with(
    wanted: &Wanted,
    exists: &dyn Fn(&Path) -> bool,
    alias: &dyn Fn(&Path, &Path) -> bool,
    folds: &dyn Fn(&Path) -> bool,
) -> Result<Vec<Step>, Refusal> {
    // Everything this batch empties. A deleted folder is not in here: nothing
    // may land in one, so the paths under it are never a free name.
    let mut vacated: HashSet<PathBuf> = HashSet::new();
    for (from, _) in wanted.renames.iter().chain(&wanted.moves) {
        vacated.insert(from.clone());
    }
    vacated.extend(wanted.deletes.iter().cloned());
    let doomed_dirs: Vec<&PathBuf> = wanted.delete_dirs.iter().collect();

    // A source that is being deleted as well as renamed is a contradiction,
    // not a free name: applying both in either order loses one of them. One
    // leaving a folder that is deleted is fine, since it goes first.
    for (from, _) in wanted.renames.iter().chain(&wanted.moves) {
        if wanted.deletes.contains(from) {
            return Err(Refusal(format!(
                "`{}` is marked for deletion and for renaming, nothing written: `u` takes one of them back",
                name_of(from)
            )));
        }
    }
    // Nor can one file leave by two steps: the second would find it gone.
    let mut leaving: HashSet<&PathBuf> = HashSet::new();
    for (from, _) in wanted.renames.iter().chain(&wanted.moves) {
        if !leaving.insert(from) {
            return Err(Refusal(format!(
                "`{}` would be renamed or moved twice, nothing written: `u` takes one of them back",
                name_of(from)
            )));
        }
    }

    // Every claim, checked against the disk and against the other claims,
    // each under the name its folder tells it apart by.
    let mut folding: HashMap<&Path, bool> = HashMap::new();
    let mut claims: HashSet<(PathBuf, String)> = HashSet::new();
    for to in wanted
        .renames
        .iter()
        .chain(&wanted.moves)
        .chain(&wanted.copies)
        .chain(&wanted.copies_after)
        .map(|(_, to)| to)
    {
        let dir = to.parent().unwrap_or(Path::new(""));
        let name = name_of(to);
        let folded = *folding.entry(dir).or_insert_with(|| folds(dir));
        let key = if folded { name.to_lowercase() } else { name };
        if !claims.insert((dir.to_path_buf(), key)) {
            return Err(Refusal(format!(
                "two files would both become `{}`, nothing written: rename one of them",
                name_of(to)
            )));
        }
        // Folders are deleted after everything else lands, so whatever lands
        // inside one would go with it. One landing on the folder's own name
        // lands once the folder is gone.
        if let Some(dir) = doomed_dirs
            .iter()
            .find(|dir| to.starts_with(dir.as_path()) && to.as_path() != dir.as_path())
        {
            return Err(Refusal(format!(
                "`{}` is marked for deletion, so nothing can be put in it, nothing written: `dd` on it takes the mark off",
                name_of(dir)
            )));
        }
        // The filesystem decides whether something is in the way; the batch
        // only gets to say it is about to move out of the way.
        let freed = vacated.contains(to)
            || vacated.iter().any(|v| alias(v, to))
            || doomed_dirs.contains(&to);
        if exists(to) && !freed {
            return Err(Refusal(format!(
                "`{}` already exists, nothing written: rename it or put it somewhere else",
                name_of(to)
            )));
        }
    }

    order(wanted, exists, alias)
}

/// Puts every step in an order where each one finds the disk the way it
/// expects: a name is free before anything lands on it, a copy is made before
/// its source goes, and whatever happens inside a folder happens before the
/// folder is renamed. A cycle is refused rather than given a temporary name.
///
/// Where nothing forces an order, deletions come first, then renames and
/// moves, then copies, then folders deepest first.
fn order(
    wanted: &Wanted,
    exists: &dyn Fn(&Path) -> bool,
    alias: &dyn Fn(&Path, &Path) -> bool,
) -> Result<Vec<Step>, Refusal> {
    let mut steps: Vec<Step> = wanted.deletes.iter().cloned().map(Step::Delete).collect();
    steps.extend(
        wanted
            .renames
            .iter()
            .chain(&wanted.moves)
            .map(|(from, to)| Step::Rename(from.clone(), to.clone())),
    );
    steps.extend(
        wanted
            .copies
            .iter()
            .map(|(from, to)| Step::Copy(from.clone(), to.clone())),
    );
    let reading_now = steps.len();
    steps.extend(
        wanted
            .copies_after
            .iter()
            .map(|(from, to)| Step::Copy(from.clone(), to.clone())),
    );
    let reading_after = steps.len();
    // Deepest first, so a folder inside a marked folder is already gone.
    let mut dirs = wanted.delete_dirs.clone();
    dirs.sort_by_key(|d| std::cmp::Reverse(d.components().count()));
    steps.extend(dirs.into_iter().map(Step::DeleteDir));

    let n = steps.len();
    let after = |i: usize| (reading_now..reading_after).contains(&i);
    let mut waiting: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut blockers = vec![0usize; n];
    for a in 0..n {
        for b in 0..n {
            if a != b && must_precede(&steps[a], after(a), &steps[b], after(b), exists, alias) {
                waiting[a].push(b);
                blockers[b] += 1;
            }
        }
    }

    let mut placed = vec![false; n];
    let mut out = Vec::with_capacity(n);
    while let Some(next) = (0..n).find(|&i| !placed[i] && blockers[i] == 0) {
        placed[next] = true;
        out.push(steps[next].clone());
        for &b in &waiting[next] {
            blockers[b] -= 1;
        }
    }

    // Everything left is waiting on something that is waiting on it. A swap
    // is the usual shape.
    let stuck: Vec<&Step> = (0..n).filter(|&i| !placed[i]).map(|i| &steps[i]).collect();
    if let Some(Step::Rename(from, to) | Step::Copy(from, to)) = stuck
        .iter()
        .find(|step| matches!(step, Step::Rename(..) | Step::Copy(..)))
    {
        return Err(Refusal(format!(
            "`{}` and `{}` would swap names, which takes two writes: undo one of them, `:w`, then do it again",
            name_of(from),
            name_of(to)
        )));
    }
    if let Some(step) = stuck.first() {
        return Err(Refusal(format!(
            "`{}` waits on a change that waits on it, nothing written: undo one of them, `:w`, then do it again",
            name_of(paths(step)[0])
        )));
    }
    Ok(out)
}

/// Whether `a` has to run before `b`, with `exists` saying what is on disk
/// before anything runs. A copy marked `after` reads the file a rename or a
/// move in the batch puts at its source, never the one there now.
fn must_precede(
    a: &Step,
    a_after: bool,
    b: &Step,
    b_after: bool,
    exists: &dyn Fn(&Path) -> bool,
    alias: &dyn Fn(&Path, &Path) -> bool,
) -> bool {
    // `a` frees the name `b` lands on, which only matters while something is
    // there to free.
    let frees = |to: &Path| match a {
        Step::Delete(p) | Step::Rename(p, _) => p == to || alias(p, to),
        _ => vacates(a, to),
    };
    if let Some(to) = claim(b)
        && exists(to)
        && frees(to)
    {
        return true;
    }
    // Whatever starts inside a folder being deleted runs before it goes.
    if let Step::DeleteDir(dir) = b
        && source(a).is_some_and(|from| from.starts_with(dir))
    {
        return true;
    }
    // A copy reads its source, so it goes before whatever takes that away.
    if let Step::Copy(from, _) = a
        && !a_after
        && vacates(b, from)
    {
        return true;
    }
    // `a` puts in place the file `b` starts from, which is not on disk yet: a
    // rename or a deletion of a pending copy, or of a file that is being
    // moved, or a copy that reads a file after its move. A source that is
    // already there is otherwise the file `b` is about to move out of the way.
    if let Some(from) = source(b)
        && (b_after || !exists(from))
        && claim(a) == Some(from)
    {
        return true;
    }
    // A folder's rename comes after everything that names a path inside it,
    // since those paths stop existing the moment it runs.
    if let Step::Rename(folder, _) = b {
        return paths(a)
            .iter()
            .any(|p| p.starts_with(folder) && p.as_path() != folder.as_path());
    }
    false
}

fn claim(step: &Step) -> Option<&PathBuf> {
    match step {
        Step::Rename(_, to) | Step::Copy(_, to) => Some(to),
        Step::Delete(_) | Step::DeleteDir(_) => None,
    }
}

fn source(step: &Step) -> Option<&PathBuf> {
    match step {
        Step::Rename(from, _) | Step::Copy(from, _) | Step::Delete(from) => Some(from),
        Step::DeleteDir(_) => None,
    }
}

fn vacates(step: &Step, path: &Path) -> bool {
    match step {
        Step::Delete(p) | Step::Rename(p, _) => p == path,
        Step::DeleteDir(dir) => path.starts_with(dir),
        Step::Copy(..) => false,
    }
}

fn paths(step: &Step) -> Vec<&PathBuf> {
    match step {
        Step::Delete(p) | Step::DeleteDir(p) => vec![p],
        Step::Rename(from, to) | Step::Copy(from, to) => vec![from, to],
    }
}

/// A path as the message row should name it: the file, not the whole tree.
fn name_of(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

impl super::App {
    /// What `:w` does to the files, worked out from the rows rather than from
    /// the order things were done in, so each file gets one step to where it
    /// ends: a moved row's file goes where the row is, a copy row's file is
    /// copied there from where its source is now, and a row marked for
    /// deletion deletes the file it shows, which for a moved row is still
    /// where its move starts and for a copy row is not made yet. A copy back
    /// onto the very name its file is moving away from reads the file once
    /// it has moved, and one whose file is deleted instead (its moved row
    /// marked, or a row it replaced) just leaves the file where it is.
    /// Renames are not in it.
    pub(super) fn file_batch(&self) -> Wanted {
        self.file_batch_with(None)
    }

    /// `file_batch`, with the row at `typed.0` going to `typed.1` instead:
    /// the name being typed on a row that is moving or being copied, which
    /// leaving the buffer makes part of where it goes.
    pub(super) fn file_batch_with(&self, typed: Option<(&Path, &Path)>) -> Wanted {
        let doomed = |path: &PathBuf| self.doomed_files.contains(path);
        let lands = |to: &PathBuf| match typed {
            Some((row, now)) if row == to.as_path() => now.to_path_buf(),
            _ => to.clone(),
        };
        let mut wanted = Wanted {
            delete_dirs: self.doomed_dirs.clone(),
            ..Wanted::default()
        };
        for (from, to) in &self.moves {
            if doomed(to) {
                wanted.deletes.push(from.clone());
            } else if lands(to) != *from {
                wanted.moves.push((from.clone(), lands(to)));
            }
        }
        wanted.deletes.extend(
            self.doomed_files
                .iter()
                .filter(|path| {
                    !self
                        .moves
                        .iter()
                        .chain(&self.copies)
                        .any(|(_, to)| to == *path)
                })
                .cloned(),
        );
        wanted
            .deletes
            .extend(self.replaced.iter().filter_map(|r| r.file.clone()));
        for (from, to) in &self.copies {
            if doomed(to) {
                continue;
            }
            let to = lands(to);
            if *from != to {
                wanted.copies.push((from.clone(), to));
            } else if let Some((_, now)) = wanted.moves.iter().find(|(f, _)| f == from) {
                wanted.copies_after.push((now.clone(), to));
            } else {
                wanted.deletes.retain(|p| *p != to);
            }
        }
        wanted
    }

    /// The pending changes `file_batch` turns into nothing at all, taken out
    /// of memory when a write starts, since no step will land them: a copy
    /// row marked for deletion, whether or not a put has replaced it since,
    /// a moved row marked for deletion whose file a copy put back where it
    /// was, and a copy put back onto its own name over a row a put replaced.
    /// That copy is then the file there.
    pub(super) fn drop_no_ops(&mut self) {
        self.replaced.retain(|r| r.file.is_some());
        let mut forget: Vec<PathBuf> = Vec::new();
        for (from, to) in self.copies.clone() {
            if self.doomed_files.contains(&to) {
                self.copies.retain(|(_, t)| *t != to);
                self.doomed_files.retain(|p| *p != to);
                self.cut.retain(|p| *p != to);
                forget.push(to);
            } else if from == to
                && let Some(at) = self
                    .moves
                    .iter()
                    .position(|(f, t)| *f == from && self.doomed_files.contains(t))
            {
                let (_, moved) = self.moves.remove(at);
                self.doomed_files.retain(|p| *p != moved);
                self.cut.retain(|p| *p != moved);
                self.copies.retain(|(_, t)| *t != to);
                forget.push(moved);
            } else if from == to && !self.moves.iter().any(|(f, _)| *f == from) {
                // Copied back onto its own name with nothing moving it away,
                // so the file stays, and so does the row on it.
                self.copies.retain(|(_, t)| *t != to);
                self.replaced.retain(|r| r.file.as_ref() != Some(&from));
            }
        }
        if !forget.is_empty() {
            self.forget_tracks(&forget);
        }
    }

    /// Everything this `:w` would do, checked as one batch.
    ///
    /// Runs before a single byte is written, so a refusal leaves every change
    /// pending exactly as it was. The name checks live here too: an empty name
    /// has to stop the batch before the deletions run, not after.
    pub fn write_plan(&self) -> Result<Vec<Step>, String> {
        let mut wanted = self.file_batch();

        for (what, name) in &self.renames {
            if self.dormant(what) {
                continue;
            }
            let name = name.trim();
            if name.is_empty() {
                return Err("a name cannot be empty".into());
            }
            if name.contains('/') {
                return Err("a name cannot contain `/`, this renames but never moves".into());
            }
            let (Some(from), Some(to)) = (self.rename_source(what), self.rename_target(what, name))
            else {
                continue;
            };
            if from != to {
                wanted.renames.push((from, to));
            }
        }

        // A rename cannot carry a file to another disk, so a move that would
        // fails halfway through the write unless it is refused here.
        for (from, to) in wanted.renames.iter().chain(&wanted.moves) {
            let dev = |path: &Path| std::fs::symlink_metadata(path).ok().map(|m| m.dev());
            if let (Some(a), Some(b)) = (dev(from), to.parent().and_then(dev))
                && a != b
            {
                return Err(format!(
                    "`{}` would move to another disk, which a move cannot do, nothing written: copy it there with `yy` `p`, then `dd` it here",
                    name_of(from)
                ));
            }
        }

        // Asked without following symlinks: a link whose target is gone is
        // still a file someone made, and `exists` would call its name free.
        let occupied = |path: &Path| std::fs::symlink_metadata(path).is_ok();

        // A file that went away since it was listed would stop the write at
        // its step, after the ones before it had run, unless a step in the
        // batch is what puts it there.
        let claimed: Vec<&PathBuf> = wanted
            .renames
            .iter()
            .chain(&wanted.moves)
            .chain(&wanted.copies)
            .chain(&wanted.copies_after)
            .map(|(_, to)| to)
            .collect();
        for (from, _) in wanted
            .renames
            .iter()
            .chain(&wanted.moves)
            .chain(&wanted.copies)
        {
            if !occupied(from) && !claimed.contains(&from) {
                return Err(format!(
                    "`{}` is not there any more, nothing written: undo the change to it, then `:w`",
                    name_of(from)
                ));
            }
        }
        plan_with(
            &wanted,
            &occupied,
            &super::write::case_alias,
            &super::write::folds_case,
        )
        .map_err(|Refusal(why)| why)
    }
}
