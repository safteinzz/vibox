//! `:w`: every pending change, applied in the order `plan` worked out.
//!
//! The disk work runs on a thread of its own, so a write that copies
//! gigabytes never freezes the screen. That thread touches nothing but the
//! disk: it runs the steps in order and reports each one, and the input
//! thread brings memory up to date with every report as it lands
//! (`App::write_landed`).

use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Read, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::time::Instant;

use crate::library::{self, Fix, Track};
use crate::reason;

use super::jobs::{Job, WriteJob};
use super::plan::Step;
use super::*;

/// What one `:w` did, for the message row.
#[derive(Default)]
pub(super) struct Done {
    renamed: usize,
    moved: usize,
    copied: usize,
    deleted: usize,
    deleted_dirs: usize,
    saved: Vec<String>,
    dropped_playlists: usize,
    repaired: usize,
}

impl Done {
    fn summary(&self) -> String {
        let mut parts = Vec::new();
        if self.renamed > 0 {
            parts.push(format!("renamed {}", self.renamed));
        }
        if self.moved > 0 {
            parts.push(format!("moved {} file{}", self.moved, plural(self.moved)));
        }
        if self.copied > 0 {
            parts.push(format!(
                "copied {} file{}",
                self.copied,
                plural(self.copied)
            ));
        }
        match (self.deleted, self.deleted_dirs) {
            (0, 0) => {}
            (files, 0) => parts.push(format!("deleted {files} file{}", plural(files))),
            (0, dirs) => parts.push(format!("deleted {dirs} folder{}", plural(dirs))),
            (files, dirs) => parts.push(format!(
                "deleted {files} file{} and {dirs} folder{}",
                plural(files),
                plural(dirs)
            )),
        }
        match self.saved.as_slice() {
            [] => {}
            [one] => parts.push(format!("saved `{one}`")),
            many => parts.push(format!("saved {} playlists", many.len())),
        }
        if self.dropped_playlists > 0 {
            parts.push(format!(
                "deleted {} playlist{}",
                self.dropped_playlists,
                plural(self.dropped_playlists)
            ));
        }
        if self.repaired > 0 {
            parts.push(format!(
                "updated {} playlist{}",
                self.repaired,
                plural(self.repaired)
            ));
        }
        parts.join(", ")
    }
}

/// Everything the writing thread needs, made on the input thread so the
/// thread never touches `App`.
struct Work {
    steps: Vec<Step>,
    /// Every playlist to repair afterwards, the library's own m3u included.
    playlists: Vec<PathBuf>,
    /// Playlists edited and not saved, by file, as their rows stand now.
    saves: Vec<(PathBuf, Vec<Track>)>,
    /// Playlists marked with `dd`.
    drops: Vec<PathBuf>,
    /// The m3u the library was opened from, which names its own tracks, and
    /// the rows of the copies, so each copy that lands joins it.
    library_m3u: Option<PathBuf>,
    copied: Vec<Track>,
    /// What paths in a message are shown relative to.
    root: PathBuf,
}

/// What the writing thread reports, in the order it happens.
pub(super) enum Event {
    /// A step is done, with the file read again when it is a track that
    /// moved, or it failed, which ends the steps.
    Step(usize, Result<Option<Track>, String>),
    Repaired(usize, Vec<String>),
    /// A playlist saved, by the file it is in now.
    Saved(PathBuf),
    /// Something that went wrong after the files themselves were written.
    Noted(String),
    Dropped(PathBuf),
    Failed(String),
    Done,
}

impl App {
    /// `:w`: starts writing everything that is waiting, in the background.
    ///
    /// The batch is checked against the disk before a byte is written, so a
    /// clash leaves every change pending. A failure no check can see coming,
    /// a permission or a full disk, stops the write at the step that failed:
    /// what ran before it is done, and it and everything after it stay pending
    /// for another `:w`. The message says which is which.
    pub fn start_write(&mut self) {
        if self.busy() {
            return;
        }
        // A name refused there says why on the message row, which a write
        // starting would cover, so nothing is written.
        if !self.commit_name() {
            self.end_edit();
            if let Some((why, _)) = self.msg.take() {
                self.error(format!(
                    "{why}, nothing written: `:w` again writes the rest"
                ));
            }
            return;
        }
        // The name being typed is in the pending set now, so the buffer it was
        // typed in closes, whatever the write does next.
        self.end_edit();
        if !self.unsaved() {
            self.info("nothing to write");
            return;
        }
        let steps = match self.write_plan() {
            Ok(steps) => steps,
            Err(problem) => {
                self.error(problem);
                return;
            }
        };

        self.drop_no_ops();

        let mut playlists: Vec<PathBuf> = self.playlists.iter().map(|(_, p)| p.clone()).collect();
        // A library opened from an m3u names its own tracks.
        let library_m3u = library::is_playlist(&self.root).then(|| self.root.clone());
        for other in library_m3u.iter().chain(&self.elsewhere) {
            if !playlists.contains(other) {
                playlists.push(other.clone());
            }
        }
        // A playlist about to be deleted is not saved first, and one that is
        // names each file where this write leaves it.
        let saves = self
            .pl_edits
            .iter()
            .filter(|(path, _)| !self.is_doomed(path))
            .map(|(path, rows)| {
                let rows = rows
                    .iter()
                    .filter_map(|&i| {
                        let mut track = self.tracks.get(i)?.clone();
                        track.path = self.final_path(i)?;
                        Some(track)
                    })
                    .collect();
                (path.clone(), rows)
            })
            .collect();
        let copied = self
            .copies
            .iter()
            .filter_map(|(_, to)| self.row_at(to).map(|i| self.tracks[i].clone()))
            .collect();
        let work = Work {
            steps: steps.clone(),
            playlists,
            saves,
            drops: self.doomed.clone(),
            root: self.root_dir(),
            library_m3u,
            copied,
        };

        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = channel();
        let stop = Arc::clone(&cancel);
        let spawned = std::thread::Builder::new()
            .name("vibox-write".into())
            .spawn(move || write(work, &tx, &stop));
        let handle = match spawned {
            Ok(handle) => handle,
            Err(e) => {
                self.error(format!("cannot start writing: {}", reason::of(&e)));
                return;
            }
        };
        // Undo steps between states nothing has written yet; once the disk
        // has moved there is no stepping back over it.
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.job = Some(Job::Write(WriteJob {
            steps,
            rx,
            done: Done::default(),
            problems: Vec::new(),
            notes: Vec::new(),
            cancel,
            handle: Some(handle),
            landed: 0,
            started: Instant::now(),
        }));
    }

    /// `:w`, waited for, for a caller with no event loop to land it in.
    pub fn write_all(&mut self) {
        self.start_write();
        self.finish_job();
    }

    /// Brings memory up to date with one report from the writing thread.
    /// True once the write is over.
    pub(super) fn write_landed(&mut self, job: &mut WriteJob, event: Event) -> bool {
        match event {
            Event::Step(at, Ok(fresh)) => {
                job.landed += 1;
                if let Some(step) = job.steps.get(at).cloned() {
                    self.step_landed(&step, fresh, &mut job.done);
                }
            }
            Event::Step(_, Err(why)) | Event::Failed(why) => job.problems.push(why),
            Event::Repaired(fixed, failed) => {
                job.done.repaired = fixed;
                job.notes.extend(failed);
            }
            Event::Noted(why) => job.notes.push(why),
            Event::Saved(path) => {
                self.pl_edits.remove(&path);
                job.done.saved.push(playlist_label(&path));
            }
            Event::Dropped(path) => {
                self.doomed.retain(|p| *p != path);
                self.renames.remove(&Renaming::Playlist(path.clone()));
                self.pl_edits.remove(&path);
                if self.playlist_view.as_ref() == Some(&path) {
                    self.playlist_view = None;
                }
                // A tab you are not looking at must not keep showing a
                // playlist whose file no longer exists.
                for tab in &mut self.tabs {
                    if tab.playlist.as_ref() == Some(&path) {
                        tab.playlist = None;
                    }
                }
                job.done.dropped_playlists += 1;
            }
            Event::Done => {
                self.write_finished(job);
                return true;
            }
        }
        false
    }

    /// Where the row at `i` ends up once this write has run, or `None` when
    /// the write deletes it: its path with its rename, inside its folder's
    /// new name, the deepest folder renamed first as the write does it.
    fn final_path(&self, i: usize) -> Option<PathBuf> {
        let track = self.tracks.get(i)?;
        if self.is_doomed_file(&track.path) {
            return None;
        }
        let mut path = self.shown_path(i)?;
        let mut folders: Vec<(PathBuf, PathBuf)> = self
            .renames
            .iter()
            .filter(|(what, _)| matches!(what, Renaming::Folder(_)) && !self.dormant(what))
            .filter_map(|(what, name)| {
                Some((self.rename_source(what)?, self.rename_target(what, name)?))
            })
            .collect();
        folders.sort_by_key(|(from, _)| std::cmp::Reverse(from.components().count()));
        for (from, to) in folders {
            if let Ok(rest) = path.strip_prefix(&from) {
                path = to.join(rest);
            }
        }
        Some(path)
    }

    /// The write is over: the screen put back in order, and the message.
    pub(super) fn write_finished(&mut self, job: &mut WriteJob) {
        if let Some(handle) = job.handle.take() {
            let _ = handle.join();
        }
        self.settle();
        let summary = job.done.summary();
        if job.problems.is_empty() && job.notes.is_empty() {
            // Changes that undid each other leave the disk as it was.
            if summary.is_empty() {
                self.info("nothing on disk needed changing");
            } else {
                self.info(summary);
            }
            return;
        }
        if job.problems.is_empty() {
            let mut text = job.notes.join("\n");
            text.push_str(&format!("\nthe files themselves were written: {summary}"));
            self.fail("playlists not updated", text);
            return;
        }
        let mut text = job.problems.join("\n");
        if !summary.is_empty() {
            text.push_str(&format!("\nbefore that: {summary}"));
        }
        let left = self.pending_changes().len();
        if left > 0 {
            text.push_str(&format!(
                "\n{left} change{} still waiting: `:ch` lists them, `:w` tries again",
                plural(left)
            ));
        }
        self.fail("write stopped", text);
    }

    /// Memory after one step that the disk has done.
    fn step_landed(&mut self, step: &Step, fresh: Option<Track>, done: &mut Done) {
        match step {
            Step::Delete(path) => {
                // A row a put replaced is out of the list already, and the
                // row on its name now is the one that landed there. A moved
                // row's file is deleted where its move starts.
                let replaced = self.replaced.iter().any(|r| r.file.as_ref() == Some(path));
                self.replaced.retain(|r| r.file.as_ref() != Some(path));
                let row = match self.moves.iter().position(|(from, _)| from == path) {
                    Some(at) => self.moves.remove(at).1,
                    None => path.clone(),
                };
                self.doomed_files.retain(|p| *p != row);
                self.cut.retain(|p| *p != row);
                if !replaced {
                    self.forget_tracks(std::slice::from_ref(&row));
                }
                done.deleted += 1;
            }
            Step::Rename(from, to) => self.renamed(from, to, fresh, done),
            Step::Copy(_, to) => {
                // Copies have one each, so the target says which landed.
                self.copies.retain(|(_, t)| t != to);
                for yanked in &mut self.yank {
                    if yanked.path == *to {
                        yanked.origin.clone_from(to);
                    }
                }
                done.copied += 1;
            }
            Step::DeleteDir(dir) => {
                self.doomed_dirs.retain(|d| d != dir);
                self.doomed_files.retain(|p| !p.starts_with(dir));
                // Its renames waited for the mark to come off, which it never
                // will now.
                self.renames
                    .retain(|what, _| !matches!(what, Renaming::Folder(p) if p.starts_with(dir)));
                self.elsewhere.retain(|p| !p.starts_with(dir));
                let inside: Vec<PathBuf> = self
                    .tracks
                    .iter()
                    .filter(|t| t.path.starts_with(dir))
                    .map(|t| t.path.clone())
                    .collect();
                self.forget_tracks(&inside);
                done.deleted_dirs += 1;
            }
        }
    }

    /// Memory after a rename or a move: the track, the folder or the playlist
    /// it was, `fresh` being the file read again where it now is.
    fn renamed(&mut self, from: &Path, to: &Path, fresh: Option<Track>, done: &mut Done) {
        // A yank remembers the file it came from, which is now here, and so
        // does the sound thread.
        for yanked in &mut self.yank {
            if yanked.origin == from {
                yanked.origin = to.to_path_buf();
            }
        }
        if self.playing_file.as_deref() == Some(from) {
            self.playing_file = Some(to.to_path_buf());
        }
        if let Some(at) = self.moves.iter().position(|(f, t)| f == from && t == to) {
            // The list already shows a pending move where it is going. One
            // that stays in its folder is a rename, as `:ch` called it.
            self.moves.remove(at);
            self.lyrics.moved(from, to);
            if from.parent() == to.parent() {
                done.renamed += 1;
            } else {
                done.moved += 1;
            }
            return;
        }

        let Some(what) = self
            .renames
            .keys()
            .find(|what| self.rename_source(what).as_deref() == Some(from))
            .cloned()
        else {
            return;
        };
        self.renames.remove(&what);
        done.renamed += 1;
        match what {
            Renaming::Track(idx) => {
                // Read again where it now is: a file with no title tag shows
                // its name as the title, and the name just changed.
                let mut track = fresh.unwrap_or_else(|| self.tracks[idx].clone());
                track.path = to.to_path_buf();
                if track.file == self.tracks[idx].file {
                    track.file = to
                        .file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default();
                }
                track.in_library = self.tracks[idx].in_library;
                self.tracks[idx] = track;
                for yanked in &mut self.yank {
                    if yanked.path == from {
                        yanked.path = to.to_path_buf();
                    }
                }
                self.lyrics.moved(from, to);
            }
            Renaming::Folder(_) => {
                let mut followed = Vec::new();
                for track in &mut self.tracks {
                    if let Ok(rest) = track.path.strip_prefix(from) {
                        let now = to.join(rest);
                        followed.push((track.path.clone(), now.clone()));
                        track.path = now;
                    }
                }
                for (was, now) in followed {
                    self.lyrics.moved(&was, &now);
                }
                for yanked in &mut self.yank {
                    for path in [&mut yanked.path, &mut yanked.origin] {
                        if let Ok(rest) = path.strip_prefix(from) {
                            *path = to.join(rest);
                        }
                    }
                }
                // A playlist `:w <path>` wrote in it, the file playing, and the
                // folder list, which keeps the sidebar and the open view on
                // the folder under its new name.
                let inside = self
                    .elsewhere
                    .iter_mut()
                    .chain(self.playing_file.iter_mut())
                    .chain(self.folders.iter_mut().map(|(_, path)| path));
                for path in inside {
                    if let Ok(rest) = path.strip_prefix(from) {
                        *path = if rest.as_os_str().is_empty() {
                            to.to_path_buf()
                        } else {
                            to.join(rest)
                        };
                    }
                }
            }
            Renaming::Playlist(old) => {
                // A tab showing it keeps showing it, under the new name, and
                // an edit to it is saved there.
                if self.playlist_view.as_ref() == Some(&old) {
                    self.playlist_view = Some(to.to_path_buf());
                }
                for tab in &mut self.tabs {
                    if tab.playlist.as_ref() == Some(&old) {
                        tab.playlist = Some(to.to_path_buf());
                    }
                }
                if let Some(rows) = self.pl_edits.remove(&old) {
                    self.pl_edits.insert(to.to_path_buf(), rows);
                }
                self.reload_playlists();
            }
        }
    }

    /// Puts the screen back in order after a write: a rename moves a row in
    /// the sort and a move changes which folders exist. Not a rescan, which
    /// would throw away what is playing and the queue.
    fn settle(&mut self) {
        // A cut that was never put somewhere was a deletion, and has run.
        let doomed = &self.doomed_files;
        self.cut.retain(|p| doomed.contains(p));
        self.reload_playlists();
        self.resort();
        self.set_folders();
        self.rebuild_view();
    }
}

/// The writing thread: the steps in order, then the playlists. Stops at the
/// first step that fails, or when `cancel` is set, and still repairs the
/// playlists for whatever ran, so none is left naming a path that is gone.
fn write(work: Work, tx: &Sender<Event>, cancel: &AtomicBool) {
    let mut ran: Vec<&Step> = Vec::new();
    let mut copied: Vec<&Track> = Vec::new();
    let mut whole = true;
    for (at, step) in work.steps.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            whole = false;
            break;
        }
        let result = on_disk(step, &work.root, cancel);
        if result.is_ok() {
            ran.push(step);
            if let Step::Copy(_, to) = step {
                copied.extend(work.copied.iter().find(|t| t.path == *to));
            }
        }
        let failed = result.is_err();
        let _ = tx.send(Event::Step(at, result));
        if failed {
            whole = false;
            break;
        }
    }

    // A playlist renamed in this write is repaired where it is now.
    let playlists: Vec<PathBuf> = work
        .playlists
        .iter()
        .filter_map(|p| follow(p, &ran))
        .collect();
    let (fixed, unfixed) = repair_all(&playlists, &ran);
    let _ = tx.send(Event::Repaired(fixed, unfixed));
    if let Some(own) = work
        .library_m3u
        .as_deref()
        .and_then(|own| follow(own, &ran))
        && !copied.is_empty()
        && let Err(e) = library::append_m3u(&own, &copied)
    {
        let _ = tx.send(Event::Noted(reason::chain(&e)));
    }

    if whole {
        // Each row already names where this write leaves its file.
        for (path, rows) in work.saves {
            let Some(path) = follow(&path, &ran) else {
                continue;
            };
            let rows: Vec<&Track> = rows.iter().collect();
            match library::save_m3u(&path, &rows) {
                Ok(()) => {
                    let _ = tx.send(Event::Saved(path));
                }
                Err(e) => {
                    let _ = tx.send(Event::Failed(reason::chain(&e)));
                    whole = false;
                    break;
                }
            }
        }
    }
    if whole {
        for path in work.drops {
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == ErrorKind::NotFound => {}
                Err(e) => {
                    let name = path.file_stem().unwrap_or_default().to_string_lossy();
                    let _ = tx.send(Event::Failed(format!(
                        "cannot delete the playlist `{name}`: {}",
                        reason::of(&e)
                    )));
                    break;
                }
            }
            let _ = tx.send(Event::Dropped(path));
        }
    }
    let _ = tx.send(Event::Done);
}

/// One step on disk, guarded immediately before the call, since the plan is a
/// moment old and `fs::rename` replaces whatever is there.
fn on_disk(step: &Step, root: &Path, cancel: &AtomicBool) -> Result<Option<Track>, String> {
    let shown = |path: &Path| match path.strip_prefix(root) {
        Ok(rel) => rel.display().to_string(),
        Err(_) => path.display().to_string(),
    };
    match step {
        Step::Delete(path) => match std::fs::remove_file(path) {
            // Already gone is the state we wanted.
            Ok(()) => Ok(None),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!(
                "cannot delete `{}`: {}",
                shown(path),
                reason::of(&e)
            )),
        },
        Step::Rename(from, to) => {
            // A name the source itself holds is not in the way: that is a
            // rename changing only the case on a disk that ignores case.
            let same = case_alias(from, to);
            if std::fs::symlink_metadata(to).is_ok() && !same {
                return Err(format!(
                    "`{}` is in the way, so it was left alone",
                    shown(to)
                ));
            }
            let verb = if from.parent() == to.parent() {
                "rename"
            } else {
                "move"
            };
            let moved = if same {
                rename_through_spare(from, to)
            } else {
                std::fs::rename(from, to)
            };
            moved.map_err(|e| {
                format!(
                    "cannot {verb} `{}` to `{}`: {}",
                    shown(from),
                    shown(to),
                    reason::of(&e)
                )
            })?;
            Ok(read_again(to))
        }
        Step::Copy(from, to) => match copy(from, to, cancel) {
            Ok(()) => Ok(None),
            Err(e) if e.kind() == ErrorKind::AlreadyExists => Err(format!(
                "`{}` is in the way, so it was left alone",
                shown(to)
            )),
            Err(e) => Err(format!(
                "cannot copy `{}` to `{}`: {}",
                shown(from),
                shown(to),
                reason::of(&e)
            )),
        },
        Step::DeleteDir(dir) => match std::fs::remove_dir_all(dir) {
            Ok(()) => Ok(None),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!(
                "cannot delete the folder `{}`: {}",
                shown(dir),
                reason::of(&e)
            )),
        },
    }
}

/// A track's tags read again at its new path, when it is a track.
fn read_again(path: &Path) -> Option<Track> {
    if !path.is_file() || !library::is_audio(path) {
        return None;
    }
    library::scan(path).ok().and_then(|mut found| found.pop())
}

/// A rename that changes only the case of a name, on a disk that ignores
/// case. The two names are one file there, and renaming a file onto itself
/// does nothing, so it goes through a spare name in the same folder.
fn rename_through_spare(from: &Path, to: &Path) -> std::io::Result<()> {
    let dir = from.parent().unwrap_or(Path::new("."));
    let name = from.file_name().unwrap_or_default().to_string_lossy();
    let spare = (0..)
        .map(|n| dir.join(format!(".{name}.vibox{n}")))
        .find(|p| std::fs::symlink_metadata(p).is_err())
        .unwrap_or_else(|| dir.join(format!(".{name}.vibox")));
    std::fs::rename(from, &spare)?;
    std::fs::rename(&spare, to).inspect_err(|_| {
        let _ = std::fs::rename(&spare, from);
    })
}

/// Copies `from` to `to`, which must not exist yet, a chunk at a time so
/// `cancel` can stop it, and takes the half made copy away when it stops or
/// fails: half a track looks exactly like a whole one in a list.
fn copy(from: &Path, to: &Path, cancel: &AtomicBool) -> std::io::Result<()> {
    let mut source = File::open(from)?;
    let permissions = source.metadata()?.permissions();
    let mut target = OpenOptions::new().write(true).create_new(true).open(to)?;
    let copied = (|| {
        let mut chunk = vec![0u8; 1 << 20];
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(std::io::Error::new(
                    ErrorKind::Interrupted,
                    "stopped before it finished",
                ));
            }
            let n = source.read(&mut chunk)?;
            if n == 0 {
                break;
            }
            target.write_all(&chunk[..n])?;
        }
        std::fs::set_permissions(to, permissions)
    })();
    if copied.is_err() {
        let _ = std::fs::remove_file(to);
    }
    copied
}

/// Where the file at `path` before the write is after the steps that `ran`,
/// in the order they ran, or `None` once one of them deleted it. A renamed
/// folder takes everything under it along.
fn follow(path: &Path, ran: &[&Step]) -> Option<PathBuf> {
    let mut now = path.to_path_buf();
    for step in ran {
        match step {
            Step::Rename(from, to) if now == *from => now.clone_from(to),
            Step::Rename(from, to) => {
                if let Ok(rest) = now.strip_prefix(from) {
                    now = to.join(rest);
                }
            }
            Step::Delete(gone) if *gone == now => return None,
            Step::DeleteDir(dir) if now.starts_with(dir) => return None,
            _ => {}
        }
    }
    Some(now)
}

/// Points every playlist at where its files went, and takes out the ones that
/// were deleted. Only the entries that changed are touched; see
/// `library::repair_m3u`. How many were rewritten, and what could not be.
fn repair_all(playlists: &[PathBuf], ran: &[&Step]) -> (usize, Vec<String>) {
    if !ran.iter().any(|step| !matches!(step, Step::Copy(..))) {
        return (0, Vec::new());
    }
    let fix = |path: &Path| -> Fix {
        match follow(path, ran) {
            None => Fix::Drop,
            Some(now) if now == path => Fix::Keep,
            Some(now) => Fix::Moved(now),
        }
    };
    let (mut fixed, mut failed) = (0, Vec::new());
    // A link to a playlist that is gone names nothing to repair.
    for list in playlists.iter().filter(|p| p.exists()) {
        match library::repair_m3u(list, &fix) {
            Ok(true) => fixed += 1,
            Ok(false) => {}
            Err(e) => failed.push(reason::chain(&e)),
        }
    }
    (fixed, failed)
}

/// Whether `a` and `b` are one name spelled two ways on a disk that ignores
/// case: the same folder, names equal but for case, and the same file. A hard
/// link has a name of its own, so it is never one: the folder lists both
/// names, where a folder that ignores case lists only the spelling it keeps.
pub(super) fn case_alias(a: &Path, b: &Path) -> bool {
    let folded = |p: &Path| p.file_name().map(|n| n.to_string_lossy().to_lowercase());
    a != b
        && a.parent() == b.parent()
        && folded(a).is_some()
        && folded(a) == folded(b)
        && same_file(a, b)
        && !both_listed(a, b)
}

/// Whether the folder `a` and `b` share lists each of their names, spelled
/// exactly so. A folder that cannot be read counts as listing both, which
/// only ever keeps two names apart.
fn both_listed(a: &Path, b: &Path) -> bool {
    let dir = a.parent().unwrap_or(Path::new("."));
    let Ok(entries) = std::fs::read_dir(dir) else {
        return true;
    };
    let names: Vec<std::ffi::OsString> = entries.flatten().map(|e| e.file_name()).collect();
    [a, b]
        .iter()
        .all(|p| p.file_name().is_some_and(|n| names.iter().any(|m| m == n)))
}

/// Whether the folder `dir` tells names apart without their case, asked of the
/// disk: a name in it spelled with its case flipped finds the same file, and
/// the folder does not list that spelling as a name of its own. A folder with
/// no name to ask with is asked about through its own name in its parent.
pub(super) fn folds_case(dir: &Path) -> bool {
    let flipped = |name: &str| -> String {
        name.chars()
            .flat_map(|c| {
                if c.is_lowercase() {
                    c.to_uppercase().collect::<Vec<_>>()
                } else {
                    c.to_lowercase().collect::<Vec<_>>()
                }
            })
            .collect()
    };
    let ask = |dir: &Path, name: &std::ffi::OsStr| -> Option<bool> {
        let name = name.to_str()?;
        let other = flipped(name);
        (other != name).then(|| {
            let (a, b) = (dir.join(name), dir.join(&other));
            same_file(&a, &b) && !both_listed(&a, &b)
        })
    };
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            if let Some(answer) = ask(dir, &entry.file_name()) {
                return answer;
            }
        }
    }
    match (dir.parent(), dir.file_name()) {
        (Some(parent), Some(name)) => ask(parent, name).unwrap_or(false),
        _ => false,
    }
}

/// Whether two paths are one file on disk.
pub(super) fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::symlink_metadata(a), std::fs::symlink_metadata(b)) {
        (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
        _ => false,
    }
}
