//! Moving, copying and deleting files.
//!
//! None of it happens when you press the key: `dd`, `d`+`p` and `y`+`p` queue
//! the work, `:ch` lists exactly what is waiting and `:w` is the only thing that
//! touches the disk. That staging is the safety, which is why the `:set danger`
//! this module used to sit behind is gone: an arming switch you set once and
//! forget protects nothing that a change you can read before writing does not
//! already protect. `:mkdir` is the one exception, made the moment it is typed,
//! since the name on the command line is the confirmation.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use super::*;

impl App {
    /// Whether `:w` will delete this file, marked on its own or with its folder.
    pub fn is_doomed_file(&self, path: &Path) -> bool {
        self.doomed_files.iter().any(|p| p == path) || self.is_doomed_dir(path)
    }

    pub fn is_cut(&self, path: &Path) -> bool {
        self.cut.iter().any(|p| p == path)
    }

    /// `dd` on a track: marks the files, and remembers them so a `p` somewhere
    /// else turns the deletion into a move, exactly like vim deleting text and
    /// putting it back. A rename waiting on one of them stays with it, so the
    /// `p` moves it under its new name.
    pub fn cut_tracks(&mut self) {
        if self.busy() {
            return;
        }
        let (a, b) = self.selection_range();
        let b = b.min(self.view.len().saturating_sub(1));
        let paths: Vec<PathBuf> = self.view[a..=b]
            .iter()
            .map(|&i| self.tracks[i].path.clone())
            .collect();
        if paths.is_empty() {
            return;
        }

        self.checkpoint();
        let n = paths.len();
        self.cut.clone_from(&paths);
        // One register, as in vim: the newest cut or yank is what `p` puts,
        // and it keeps the cut files after `u` puts them back.
        self.yank = self.register(&paths);
        for path in paths {
            if !self.doomed_files.contains(&path) {
                self.doomed_files.push(path);
            }
        }
        self.exit_visual();
        // On the row that took the place of the first one deleted.
        self.cur = a;
        self.rebuild_view();
        self.info(format!(
            "{n} file{} marked for deletion, `p` elsewhere moves them instead, `u` undoes",
            plural(n)
        ));
    }

    /// `p` in a folder with files cut: moves them into the folder in view,
    /// each under the name a rename waiting for it gives it.
    pub fn move_cut_here(&mut self) -> bool {
        if self.cut.is_empty() {
            return false;
        }
        if self.busy() {
            return true;
        }
        let Some(dir) = self.current_dir() else {
            self.error(self.nowhere("move"));
            return true;
        };
        let targets: Vec<(PathBuf, PathBuf)> = self
            .cut
            .iter()
            .filter_map(|from| Some((from.clone(), dir.join(self.put_name(from)?))))
            .collect();
        if let Err(why) = self.room_for(&dir, &targets, &self.cut) {
            self.error(why);
            return true;
        }

        self.checkpoint();
        let cut = std::mem::take(&mut self.cut);
        self.doomed_files.retain(|p| !cut.contains(p));
        // A name one of these rows is leaving is made room on by that row's
        // own step, so only the others are asked to make room.
        for (_, to) in &targets {
            if !cut.contains(to) {
                self.land_on(to);
            }
        }
        let n = self.move_rows(&targets);
        let landed: Vec<PathBuf> = targets.into_iter().map(|(_, to)| to).collect();
        self.rebuild_view();
        self.gather_at_cursor(&landed);
        // The register still holds what was put, as vim's does, so another
        // `p` copies it.
        self.yank = self.register(&landed);
        let shown = self.under_root(&dir);
        if n == 0 {
            self.info("put back where it was, nothing to move");
        } else {
            self.info(format!(
                "{n} file{} will move to {shown}, `:w` does it",
                plural(n)
            ));
        }
        true
    }

    /// Queues moving the row at `from` to `to`, folded into whatever already
    /// brought it to `from`: a pending copy lands at `to` instead, and a
    /// pending move starts where it did and ends at `to`, or is dropped when
    /// that is back where it started, which is the one time this answers false.
    /// Each file then has one step from the disk to where it ends, because a
    /// chain of them (there and back, or a copy moved on) claims a name twice
    /// and `:w` refuses the batch.
    pub(super) fn pend_move(&mut self, from: PathBuf, to: PathBuf) -> bool {
        // A yank names the row, and the row is going.
        for yanked in &mut self.yank {
            if yanked.path == from {
                yanked.path.clone_from(&to);
            }
        }
        let had = self.take_step(&from);
        self.queue_step(had, from, to)
    }

    /// `pend_move` for several rows at once, each row moved in memory and
    /// its rename made part of where it goes. Every row and the step that
    /// brought it there are found before any of them moves, since one can be
    /// landing on the name another is leaving, and a row or a step found by
    /// that name afterwards would be the other one. A row put back on its own
    /// name keeps its step. Answers how many will move.
    fn move_rows(&mut self, targets: &[(PathBuf, PathBuf)]) -> usize {
        let rows: Vec<Option<usize>> = targets.iter().map(|(from, _)| self.row_at(from)).collect();
        let had: Vec<Option<(bool, PathBuf)>> = targets
            .iter()
            .map(|(from, _)| self.take_step(from))
            .collect();
        let mut n = 0;
        for (((from, to), row), had) in targets.iter().zip(rows).zip(had) {
            if let Some(i) = row {
                self.renames.remove(&Renaming::Track(i));
            }
            if to == from {
                if had.is_some() {
                    self.queue_step(had, from.clone(), to.clone());
                }
                continue;
            }
            if let Some(i) = row {
                self.relocate_row(i, to);
            }
            if self.queue_step(had, from.clone(), to.clone()) {
                n += 1;
            }
        }
        n
    }

    /// Takes the step that brought the row at `path` there out of the queue:
    /// whether it is a copy, and where its file is on disk now.
    fn take_step(&mut self, path: &Path) -> Option<(bool, PathBuf)> {
        if let Some(at) = self.copies.iter().position(|(_, t)| t == path) {
            return Some((true, self.copies.remove(at).0));
        }
        let at = self.moves.iter().position(|(_, t)| t == path)?;
        Some((false, self.moves.remove(at).0))
    }

    /// Queues the row at `from` going to `to`, continuing `had`, the step
    /// `take_step` took for it: a copy lands at `to` instead, and a move ends
    /// at `to`, or is gone when that is where it started, which is the one
    /// time this answers false.
    fn queue_step(&mut self, had: Option<(bool, PathBuf)>, from: PathBuf, to: PathBuf) -> bool {
        match had {
            Some((true, origin)) => self.copies.push((origin, to)),
            Some((false, origin)) if origin == to => return false,
            Some((false, origin)) => self.moves.push((origin, to)),
            None => self.moves.push((from, to)),
        }
        true
    }

    /// The row a register entry names, while it is still that row: once a
    /// put has replaced it, its path names the row that landed there.
    pub(super) fn yanked_row(&self, yanked: &Yanked) -> Option<usize> {
        self.row_at(&yanked.path)
            .filter(|&i| self.origin_of(&self.tracks[i].path) == yanked.origin)
    }

    /// The register entries for the rows at `paths`, as they are now.
    pub(super) fn register(&self, paths: &[PathBuf]) -> Vec<Yanked> {
        paths
            .iter()
            .filter_map(|path| {
                Some(Yanked {
                    path: path.clone(),
                    origin: self.origin_of(path),
                    name: self.put_name(path)?,
                })
            })
            .collect()
    }

    /// Whether the row at `path` is moving or being copied there. Not the same
    /// as its file being somewhere else: a copy put back onto the name its
    /// file is leaving reads a file at its own path.
    pub(super) fn is_staged(&self, path: &Path) -> bool {
        self.moves
            .iter()
            .chain(&self.copies)
            .any(|(_, to)| to == path)
    }

    /// Where the file a row shows is on disk right now: a moved row's file is
    /// still where its move starts, and a copy's file is still its source.
    pub(super) fn origin_of(&self, path: &Path) -> PathBuf {
        self.moves
            .iter()
            .chain(&self.copies)
            .find(|(_, to)| to == path)
            .map_or_else(|| path.to_path_buf(), |(from, _)| from.clone())
    }

    /// The row a path is on, if one is.
    pub(super) fn row_at(&self, path: &Path) -> Option<usize> {
        self.tracks.iter().position(|t| t.path == path)
    }

    /// Where a row's file will be after `:w`: its path, with a rename waiting
    /// for it applied.
    pub(super) fn shown_path(&self, i: usize) -> Option<PathBuf> {
        let what = Renaming::Track(i);
        match self.renames.get(&what) {
            Some(name) => self.rename_target(&what, name),
            None => self.tracks.get(i).map(|t| t.path.clone()),
        }
    }

    /// The file name the row at `from` lands under when it is put somewhere.
    fn put_name(&self, from: &Path) -> Option<OsString> {
        let shown = self.row_at(from).and_then(|i| self.shown_path(i));
        shown
            .as_deref()
            .unwrap_or(from)
            .file_name()
            .map(ToOwned::to_owned)
    }

    /// Moves a row to `to` in memory, its name with it.
    pub(super) fn relocate_row(&mut self, i: usize, to: &Path) {
        if let Some(track) = self.tracks.get_mut(i) {
            track.path = to.to_path_buf();
            track.file = to
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
        }
    }

    /// Makes room at `to` for a row about to land there. A row on that name
    /// that is marked for deletion leaves the list and its file is deleted on
    /// `:w` first; one with a rename waiting takes its new name now. Either way
    /// two rows never share a path, which nothing that finds a row by its path
    /// could tell apart.
    pub(super) fn land_on(&mut self, to: &Path) {
        let Some(i) = self.row_at(to) else {
            return;
        };
        if self.doomed_files.iter().any(|p| p == to) {
            self.doomed_files.retain(|p| p != to);
            self.cut.retain(|p| p != to);
            // Its own step goes with it, and what is left is the file it showed.
            let file = if let Some(at) = self.copies.iter().position(|(_, t)| t == to) {
                self.copies.remove(at);
                None
            } else if let Some(at) = self.moves.iter().position(|(_, t)| t == to) {
                Some(self.moves.remove(at).0)
            } else {
                Some(to.to_path_buf())
            };
            self.replaced.push(Replaced {
                track: self.tracks[i].clone(),
                file,
            });
            self.forget_tracks(&[to.to_path_buf()]);
            return;
        }
        let what = Renaming::Track(i);
        if let Some(name) = self.renames.get(&what).cloned()
            && let Some(away) = self.rename_target(&what, &name)
        {
            // A row marked for deletion on the new name gives it up first,
            // which can take a row out of the list before this one.
            if self.doomed_files.contains(&away) {
                self.land_on(&away);
            }
            let Some(i) = self.row_at(to) else {
                return;
            };
            self.renames.remove(&Renaming::Track(i));
            self.relocate_row(i, &away);
            self.pend_move(to.to_path_buf(), away);
        }
    }

    /// Why the row on `to` cannot make room for one landing there, when it
    /// cannot: one with a rename waiting moves to that name as the other
    /// lands, so no other row may be on it, nor may this put `claim` it,
    /// except a row marked for deletion, which gives the name up. `moving`
    /// are the rows this put takes along, which leave anyway.
    pub(super) fn holder_stuck(
        &self,
        to: &Path,
        moving: &[PathBuf],
        claim: &[PathBuf],
    ) -> Option<String> {
        let h = self.row_at(to)?;
        let path = &self.tracks[h].path;
        if moving.contains(path) || self.doomed_files.contains(path) {
            return None;
        }
        let away = self.shown_path(h).filter(|away| away != to)?;
        let taken = claim.contains(&away)
            || (0..self.tracks.len()).any(|j| {
                let path = &self.tracks[j].path;
                let gives_up = self.doomed_files.contains(path) && !moving.contains(path);
                j != h
                    && ((*path == away && !gives_up)
                        || (!self.is_doomed_file(path)
                            && self.shown_path(j).as_deref() == Some(away.as_path())))
            });
        taken.then(|| {
            format!(
                "`{}` is waiting to be renamed to `{}`, which is taken: undo that rename first",
                self.plain_under_root(to),
                self.plain_under_root(&away)
            )
        })
    }

    /// `dd` on a folder row: marks the folder and everything under it,
    /// subfolders included, and a second `dd` takes the mark off again.
    ///
    /// Only the folder is marked: the files in it go because it does, so
    /// taking the mark off leaves a file you had marked on its own still marked.
    pub fn cut_folder(&mut self) {
        if self.busy() {
            return;
        }
        let whole = "that is the whole library, not a folder";
        let Some((label, dir)) = self.folders.get(self.folder_cur.wrapping_sub(1)).cloned() else {
            self.error(whole);
            return;
        };
        // The `.` row is the library's own folder, listed when tracks sit in it.
        if self.root_dir().starts_with(&dir) {
            self.error(whole);
            return;
        }

        if let Some(at) = self.doomed_dirs.iter().position(|p| *p == dir) {
            self.checkpoint();
            self.doomed_dirs.remove(at);
            self.rebuild_view();
            self.info(format!("`{label}` kept"));
            return;
        }
        // Whatever lands in a marked folder would go with it, which is never
        // what putting it there meant.
        if self
            .moves
            .iter()
            .chain(&self.copies)
            .any(|(_, to)| to.starts_with(&dir))
        {
            self.error(format!(
                "files are waiting to land in `{label}`: undo that first, then mark it"
            ));
            return;
        }

        self.checkpoint();
        let inside: Vec<PathBuf> = self
            .tracks
            .iter()
            .filter(|t| t.path.starts_with(&dir))
            .map(|t| t.path.clone())
            .collect();
        let n = inside.len();
        self.doomed_dirs.push(dir);
        self.rebuild_view();
        self.info(format!(
            "`{label}` and {n} track{} marked, `:w` deletes them, `dd` undoes it",
            plural(n)
        ));
    }

    pub fn is_doomed_dir(&self, path: &Path) -> bool {
        self.doomed_dirs.iter().any(|p| path.starts_with(p))
    }

    /// Whether `targets` can land in `dir` at all: not in a folder marked for
    /// deletion, not two of them on one name, and not onto a name another row
    /// keeps after `:w`. The last two are refused here rather than at `:w`,
    /// because two rows with one path cannot be told apart by anything that
    /// finds a row by its path. A row marked for deletion, or renamed away,
    /// gives its name up; `moving` are the rows this put takes along.
    fn room_for(
        &self,
        dir: &Path,
        targets: &[(PathBuf, PathBuf)],
        moving: &[PathBuf],
    ) -> Result<(), String> {
        if self.is_doomed_dir(dir) {
            return Err(format!(
                "`{}` is marked for deletion, `dd` on it takes the mark off",
                self.plain_under_root(dir)
            ));
        }
        for (at, (from, to)) in targets.iter().enumerate() {
            if targets[..at].iter().any(|(_, t)| t == to) {
                return Err(format!(
                    "two of them are called `{}`, nothing put",
                    to.file_name().unwrap_or_default().to_string_lossy()
                ));
            }
            // A row this put takes along, put back on its own name.
            if to == from && moving.contains(from) {
                continue;
            }
            let kept = (0..self.tracks.len()).any(|i| {
                let path = &self.tracks[i].path;
                !moving.contains(path)
                    && !self.is_doomed_file(path)
                    && self.shown_path(i).as_deref() == Some(to.as_path())
            });
            if kept {
                return Err(format!(
                    "`{}` is already there, nothing put",
                    self.plain_under_root(to)
                ));
            }
            let claim: Vec<PathBuf> = targets.iter().map(|(_, t)| t.clone()).collect();
            if let Some(why) = self.holder_stuck(to, moving, &claim) {
                return Err(why);
            }
        }
        Ok(())
    }

    /// `p` in a folder with tracks yanked: copies them in.
    ///
    /// The move case is `dd` then `p`; this is the `y` then `p` case, and it is
    /// a copy for the same reason it is in vim, where the yanked text stays
    /// where it was.
    pub fn copy_yank_here(&mut self) -> bool {
        if self.yank.is_empty() {
            return false;
        }
        if self.busy() {
            return true;
        }
        let Some(dir) = self.current_dir() else {
            self.error(self.nowhere("copy"));
            return true;
        };
        // What a yank copies is the file it was when it was yanked: the row's
        // file while the row is there, the file it came from once undo, `:e!`
        // or a put has taken the row away. Each carries its tags and the file
        // the copy reads, which is where that file is on disk now.
        // The last of each is where its file is after `:w`, a rename waiting
        // on its row included: a copy onto that name is already there.
        let mut sources: Vec<(PathBuf, PathBuf, Track, PathBuf)> = Vec::new();
        for yanked in &self.yank {
            let to = dir.join(&yanked.name);
            if let Some(i) = self.yanked_row(yanked) {
                let track = self.tracks[i].clone();
                let stays = self.shown_path(i).unwrap_or_else(|| track.path.clone());
                sources.push((yanked.origin.clone(), to, track, stays));
            } else if let Some(track) = self
                .replaced
                .iter()
                .find(|r| r.file.as_ref() == Some(&yanked.origin))
                .map(|r| r.track.clone())
                .or_else(|| {
                    crate::library::scan(&yanked.origin)
                        .ok()
                        .and_then(|mut found| found.pop())
                })
            {
                let stays = track.path.clone();
                sources.push((yanked.origin.clone(), to, track, stays));
            }
        }
        if sources.is_empty() {
            self.error("the yanked tracks are gone from the disk: yank them again");
            return true;
        }
        // A put lands every file it holds or none, as one onto a name another
        // row keeps does.
        if let Some((_, to, _, _)) = sources.iter().find(|(_, to, _, stays)| stays == to) {
            let why = if sources.iter().all(|(_, to, _, stays)| stays == to) {
                format!(
                    "the yanked tracks are already in `{}`",
                    self.plain_under_root(&dir)
                )
            } else {
                format!(
                    "`{}` is already there, nothing put",
                    self.plain_under_root(to)
                )
            };
            self.error(why);
            return true;
        }
        let targets: Vec<(PathBuf, PathBuf)> = sources
            .iter()
            .map(|(_, to, _, stays)| (stays.clone(), to.clone()))
            .collect();
        if let Err(why) = self.room_for(&dir, &targets, &[]) {
            self.error(why);
            return true;
        }

        self.checkpoint();
        let mut landed = Vec::new();
        for (source, to, mut copy, _) in sources {
            self.land_on(&to);
            copy.path.clone_from(&to);
            copy.file = to
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            self.tracks.push(copy);
            self.copies.push((source, to.clone()));
            landed.push(to);
        }

        let n = landed.len();
        self.rebuild_view();
        self.gather_at_cursor(&landed);
        let shown = self.under_root(&dir);
        self.info(format!(
            "{n} file{} will be copied to {shown}, `:w` does it",
            plural(n)
        ));
        true
    }

    /// Where a `p` puts files: the folder under the sidebar cursor when the
    /// sidebar has the keyboard, otherwise the folder the track pane shows, and
    /// in `everything` the folder of the track under the cursor. Nowhere while
    /// the sidebar shows playlists, or sits on `everything`, since no folder
    /// is under its cursor then.
    fn current_dir(&self) -> Option<PathBuf> {
        let folder = match (self.focus, self.tab) {
            (Pane::Folders, Tab::Folders) if self.folder_cur == 0 => return None,
            (Pane::Folders, Tab::Folders) => self.folder_cur,
            (Pane::Folders, Tab::Playlists) => return None,
            _ => self.folder_open,
        };
        if folder > 0 {
            return self.folders.get(folder - 1).map(|(_, p)| p.clone());
        }
        self.current_track().map(|t| t.dir().to_path_buf())
    }

    /// Why a `p` has no folder to put files in.
    fn nowhere(&self, verb: &str) -> String {
        match (self.focus, self.tab) {
            (Pane::Folders, Tab::Playlists) => format!(
                "the sidebar shows playlists: `gt` back to the folders to {verb} files into one"
            ),
            (Pane::Folders, Tab::Folders) => format!(
                "`everything` is not a folder: put the cursor on one to {verb} files into it"
            ),
            _ => format!("no folder here to {verb} into"),
        }
    }

    /// `:mkdir`: a new folder, made right away.
    ///
    /// Typing a command with a name in it is the confirmation, the way `:w
    /// file` writes in vim. Only the buffer edits wait for `:w`.
    pub fn make_dir(&mut self, name: &str) {
        if self.busy() {
            return;
        }
        let name = name.trim();
        if name.is_empty() {
            self.error("`:mkdir` needs a name");
            return;
        }

        let path = self.root_dir().join(name);
        if path.exists() {
            self.error(format!("`{name}` already exists"));
            return;
        }
        match std::fs::create_dir_all(&path) {
            Ok(()) => {
                self.set_folders();
                let shown = self.under_root(&path);
                self.info(format!("{shown} created"));
            }
            Err(e) => self.error(format!("cannot create `{name}`: {}", crate::reason::of(&e))),
        }
    }

    /// Drops deleted tracks out of the library and repoints everything that
    /// held an index into it.
    ///
    /// `view`, `queue`, the open playlist and every other tab all store indices
    /// into `tracks`, so removing an entry without remapping them would leave
    /// each of those pointing at whatever slid into the gap.
    pub(super) fn forget_tracks(&mut self, gone: &[PathBuf]) {
        if let Some(track) = self.playing_track()
            && gone.contains(&track.path)
        {
            self.playing_gone = Some(track.clone());
        }
        let mut remap: Vec<Option<usize>> = Vec::with_capacity(self.tracks.len());
        let mut next = 0;
        for track in &self.tracks {
            if gone.contains(&track.path) {
                remap.push(None);
            } else {
                remap.push(Some(next));
                next += 1;
            }
        }
        self.tracks.retain(|t| !gone.contains(&t.path));
        self.retarget(&remap);
    }
}
