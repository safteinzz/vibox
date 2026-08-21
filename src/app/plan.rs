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
//!   deleted files, anything under a deleted folder.
//! - **claimed** is every path it fills: rename, move and copy targets.
//!
//! A claim is a clash when the path is occupied and not vacated, or when two
//! claims want the same path. The filesystem is the only thing that can say
//! "occupied", and the in-memory sets can only ever excuse a clash, never
//! invent one. That asymmetry is what keeps a wrong set from costing a file.

use std::collections::{HashMap, HashSet};
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
    pub copies: Vec<(PathBuf, PathBuf)>,
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
    // Everything this batch empties. A folder is vacated along with every path
    // under it, since removing it takes the lot.
    let mut vacated: HashSet<PathBuf> = HashSet::new();
    for (from, _) in wanted.renames.iter().chain(&wanted.moves) {
        vacated.insert(from.clone());
    }
    vacated.extend(wanted.deletes.iter().cloned());
    let doomed_dirs: Vec<&PathBuf> = wanted.delete_dirs.iter().collect();

    let under_doomed_dir =
        |path: &Path| doomed_dirs.iter().any(|dir| path.starts_with(dir.as_path()));

    // A source that is being deleted as well as renamed is a contradiction,
    // not a free name: applying both in either order loses one of them.
    for (from, to) in wanted.renames.iter().chain(&wanted.moves) {
        if wanted.deletes.contains(from) || under_doomed_dir(from) {
            return Err(Refusal(format!(
                "`{}` is marked for deletion and for renaming, nothing written",
                name_of(from)
            )));
        }
        if from == to {
            continue;
        }
    }

    // Every claim, checked against the disk and against the other claims.
    let mut claims: HashMap<&PathBuf, usize> = HashMap::new();
    for to in wanted
        .renames
        .iter()
        .chain(&wanted.moves)
        .chain(&wanted.copies)
        .map(|(_, to)| to)
    {
        *claims.entry(to).or_insert(0) += 1;
        if claims[to] > 1 {
            return Err(Refusal(format!(
                "two files would both become `{}`, nothing written",
                name_of(to)
            )));
        }
        // The filesystem decides whether something is in the way; the batch
        // only gets to say it is about to move out of the way.
        if exists(to) && !vacated.contains(to) && !under_doomed_dir(to) {
            return Err(Refusal(format!(
                "`{}` already exists, nothing written",
                name_of(to)
            )));
        }
    }

    // Renames run before moves, so a batch where one depends on the other
    // cannot be ordered by this pass. Rare, and refusing is honest where
    // guessing an order would silently pick the wrong one.
    let rename_sources: HashSet<&PathBuf> = wanted.renames.iter().map(|(from, _)| from).collect();
    let move_sources: HashSet<&PathBuf> = wanted.moves.iter().map(|(from, _)| from).collect();
    for (_, to) in &wanted.moves {
        if rename_sources.contains(to) {
            return Err(Refusal(format!(
                "`{}` is being renamed and moved onto in one write: do them one at a time",
                name_of(to)
            )));
        }
    }
    for (_, to) in &wanted.renames {
        if move_sources.contains(to) {
            return Err(Refusal(format!(
                "`{}` is being moved and renamed onto in one write: do them one at a time",
                name_of(to)
            )));
        }
    }

    order(wanted)
}

/// Puts renames and moves in an order where nothing lands on a name that is
/// still taken, and refuses a cycle rather than inventing a temporary name.
fn order(wanted: &Wanted) -> Result<Vec<Step>, Refusal> {
    let mut steps: Vec<Step> = wanted.deletes.iter().cloned().map(Step::Delete).collect();

    // A rename whose target is another one's source has to wait for it. With
    // no cycles this settles in as many passes as there are renames.
    // Renames first, then moves: each set ordered among itself, and a batch
    // that needs them interleaved was refused before we got here.
    let mut pending: Vec<(PathBuf, PathBuf)> = wanted.renames.clone();
    pending.extend(wanted.moves.iter().cloned());

    while !pending.is_empty() {
        let sources: HashSet<&PathBuf> = pending.iter().map(|(from, _)| from).collect();
        let ready: Vec<usize> = pending
            .iter()
            .enumerate()
            .filter(|(_, (from, to))| !sources.contains(to) || from == to)
            .map(|(i, _)| i)
            .collect();

        if ready.is_empty() {
            // Everything left is waiting on something else that is waiting on
            // it. A swap is the usual shape.
            let (from, to) = &pending[0];
            return Err(Refusal(format!(
                "`{}` and `{}` would swap names, which needs two writes: rename one aside first",
                name_of(from),
                name_of(to)
            )));
        }

        for i in ready.iter().rev() {
            let (from, to) = pending.remove(*i);
            steps.push(Step::Rename(from, to));
        }
    }

    steps.extend(
        wanted
            .copies
            .iter()
            .map(|(from, to)| Step::Copy(from.clone(), to.clone())),
    );

    // Deepest first, so a folder inside a marked folder is already gone.
    let mut dirs = wanted.delete_dirs.clone();
    dirs.sort_by_key(|d| std::cmp::Reverse(d.components().count()));
    steps.extend(dirs.into_iter().map(Step::DeleteDir));

    Ok(steps)
}

/// A path as the message row should name it: the file, not the whole tree.
fn name_of(path: &Path) -> String {
    path.file_name()
        .map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())
}

impl super::App {
    /// Everything this `:w` would do, checked as one batch.
    ///
    /// Runs before a single byte is written, so a refusal leaves every change
    /// pending exactly as it was. The name checks live here too: an empty name
    /// has to stop the batch before the deletions run, not after.
    pub fn write_plan(&self) -> Result<Vec<Step>, String> {
        let mut wanted = Wanted {
            moves: self.moves.clone(),
            copies: self.copies.clone(),
            deletes: self.doomed_files.clone(),
            delete_dirs: self.doomed_dirs.clone(),
            ..Wanted::default()
        };

        for (what, name) in &self.renames {
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

        plan(&wanted, &|path| path.exists()).map_err(|Refusal(why)| why)
    }
}
