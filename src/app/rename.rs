//! Renaming files and folders in the edit buffer, applied together on `:w`.

use std::path::{Path, PathBuf};

use crate::name::NameBuffer;

use super::*;

impl App {
    /// Opens the list for editing. Nothing touches the disk until `:w`.
    pub fn begin_edit(&mut self) {
        if self.busy() {
            return;
        }
        if self.view.is_empty() {
            self.error("nothing here to rename");
            return;
        }
        if !self.columns.file {
            self.error("the file column is hidden: `:set file` to edit names");
            return;
        }
        let Some(&track_idx) = self.view.get(self.cur) else {
            return;
        };
        self.open_name(Renaming::Track(track_idx));
    }

    /// `j` and `k` while editing: the name you were typing joins the pending
    /// set and the next row opens, so a pass down the list is one edit each.
    pub fn edit_next_row(&mut self, delta: isize) {
        let col = self.edit.as_ref().map_or(0, |edit| edit.buf.col());
        self.commit_name();
        self.move_cursor(delta);
        if let Some(&track_idx) = self.view.get(self.cur) {
            self.open_name(Renaming::Track(track_idx));
            if let Some(buf) = self.name_buffer() {
                for _ in 0..col {
                    buf.right();
                }
            }
        }
    }

    /// `c` on a sidebar row: renames the folder or the playlist under it.
    pub fn begin_sidebar_edit(&mut self) {
        if self.busy() {
            return;
        }
        let what = match self.tab {
            Tab::Folders => {
                let folder = self.folder_cur.checked_sub(1);
                let Some((_, path)) = folder.and_then(|i| self.folders.get(i)).cloned() else {
                    self.error("that is the whole library, not a folder");
                    return;
                };
                // The `.` row is the library's own folder.
                if self.root_dir().starts_with(&path) {
                    self.error("that is the whole library, not a folder");
                    return;
                }
                if self.is_doomed_dir(&path) {
                    self.error("that folder is marked for deletion: `dd` on it takes the mark off");
                    return;
                }
                Renaming::Folder(path)
            }
            Tab::Playlists => {
                let Some((_, path)) = self.playlists.get(self.pl_cur).cloned() else {
                    self.error("no playlist here to rename");
                    return;
                };
                if self.is_doomed(&path) {
                    self.error(
                        "that playlist is marked for deletion: `dd` on it takes the mark off",
                    );
                    return;
                }
                Renaming::Playlist(path)
            }
        };
        self.open_name(what);
    }

    /// Opens a buffer on whatever is being renamed, starting from the name it
    /// has now, or from a rename already waiting to be written.
    fn open_name(&mut self, what: Renaming) {
        let name = self
            .renames
            .get(&what)
            .cloned()
            .unwrap_or_else(|| self.name_of(&what));

        // Cursor at the start, wherever the name lives.
        self.edit = Some(NameEdit {
            buf: NameBuffer::new(&name),
            what,
            depth: self.undo_stack.len(),
        });
        self.mode = Mode::Edit;
    }

    /// The name a thing has on disk right now.
    pub(super) fn name_of(&self, what: &Renaming) -> String {
        match what {
            Renaming::Track(idx) => self
                .tracks
                .get(*idx)
                .map(|t| t.file.clone())
                .unwrap_or_default(),
            Renaming::Folder(path) => path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            Renaming::Playlist(path) => playlist_label(path),
        }
    }

    /// The buffer being typed into, in whichever pane opened it.
    pub fn name_buffer(&mut self) -> Option<&mut NameBuffer> {
        self.edit.as_mut().map(|edit| &mut edit.buf)
    }

    /// Cursor position in the name being typed.
    pub fn name_col(&self) -> usize {
        self.edit.as_ref().map_or(0, |edit| edit.buf.col())
    }

    /// How far the name being typed has scrolled inside its column.
    pub fn name_scroll(&self) -> usize {
        self.edit.as_ref().map_or(0, |edit| edit.buf.scroll())
    }

    /// True while `v` has a selection up inside the name being typed.
    pub fn name_selecting(&self) -> bool {
        self.edit.as_ref().is_some_and(|edit| edit.buf.selecting())
    }

    /// The selected span of the name being typed, both ends included.
    pub fn name_selection(&self) -> Option<(usize, usize)> {
        self.edit.as_ref()?.buf.selection()
    }

    /// True while the name being typed belongs to a track row.
    pub fn renaming_a_track(&self) -> bool {
        matches!(self.renaming(), Some(Renaming::Track(_)))
    }

    /// What is being renamed right now, if anything.
    pub fn renaming(&self) -> Option<&Renaming> {
        self.edit.as_ref().map(|edit| &edit.what)
    }

    /// The name to show for something: the one being typed, then any pending
    /// rename, then what it is called on disk.
    pub fn shown_name(&self, what: &Renaming) -> Option<String> {
        if let Some(edit) = self.edit.as_ref().filter(|edit| edit.what == *what) {
            return Some(edit.buf.text());
        }
        self.renames.get(what).cloned()
    }

    /// The name shown for a track row.
    pub fn edit_text(&self, row: usize) -> Option<String> {
        let track_idx = *self.view.get(row)?;
        Some(
            self.shown_name(&Renaming::Track(track_idx))
                .unwrap_or_else(|| self.tracks[track_idx].file.clone()),
        )
    }

    /// Puts the name being typed into the pending set, and closes the buffer.
    ///
    /// Nothing is written here. A rename queues up with the moves, copies and
    /// deletions so one `:w` does the lot and `u` takes any of it back, which
    /// is why leaving a name never demands a write of its own. A row that is
    /// already moving or being copied takes the name as part of where it goes,
    /// so the one step `:w` runs for it lands it under that name. A name that
    /// cannot be given is refused here, with the old one kept.
    pub fn commit_name(&mut self) -> bool {
        let Some(edit) = self.edit.take() else {
            return true;
        };
        let depth = edit.depth.min(self.undo_stack.len());
        let name = edit.buf.text().trim().to_string();
        let was = self.name_of(&edit.what);

        if name == was {
            if self.renames.contains_key(&edit.what) {
                self.one_undo_step(depth);
                self.renames.remove(&edit.what);
            } else {
                self.undo_stack.truncate(depth);
            }
            return true;
        }
        // The name it already has waiting changes nothing, so it is no step.
        if self.renames.get(&edit.what) == Some(&name) {
            self.undo_stack.truncate(depth);
            return true;
        }
        if let Some(why) = self.bad_name(&edit.what, &name, &was) {
            self.error(why);
            self.undo_stack.truncate(depth);
            return false;
        }
        if let Renaming::Track(i) = edit.what
            && let Some(from) = self.tracks.get(i).map(|t| t.path.clone())
            && self.is_staged(&from)
        {
            let Some(to) = self.rename_target(&edit.what, &name) else {
                self.undo_stack.truncate(depth);
                return false;
            };
            let kept = (0..self.tracks.len()).any(|j| {
                j != i
                    && !self.is_doomed_file(&self.tracks[j].path)
                    && self.shown_path(j).as_deref() == Some(to.as_path())
            });
            if kept {
                self.error(format!(
                    "`{}` is already there, `{was}` kept",
                    self.plain_under_root(&to)
                ));
                self.undo_stack.truncate(depth);
                return false;
            }
            if let Some(why) = self.holder_stuck(&to, std::slice::from_ref(&from), &[]) {
                self.error(why);
                self.undo_stack.truncate(depth);
                return false;
            }
            self.one_undo_step(depth);
            self.land_on(&to);
            if let Some(i) = self.row_at(&from) {
                self.relocate_row(i, &to);
            }
            self.pend_move(from, to);
            return true;
        }
        self.one_undo_step(depth);
        self.renames.insert(edit.what, name);
        true
    }

    /// Leaves exactly one undo step for a rename about to be queued: the state
    /// from before its first keystroke when the typing saved one, or now.
    fn one_undo_step(&mut self, depth: usize) {
        if self.undo_stack.len() > depth {
            self.undo_stack.truncate(depth + 1);
            self.redo_stack.clear();
        } else {
            self.checkpoint();
        }
    }

    /// Why `name` cannot be given to `what`, if it cannot.
    fn bad_name(&self, what: &Renaming, name: &str, was: &str) -> Option<String> {
        if name.is_empty() {
            return Some(format!("a name cannot be empty, `{was}` kept"));
        }
        if name.contains('/') {
            return Some(format!(
                "a name cannot contain `/`, this renames but never moves: `{was}` kept"
            ));
        }
        match what {
            Renaming::Playlist(path) => self
                .playlists
                .iter()
                .any(|(_, p)| p != path && self.playlist_named(p) == name)
                .then(|| format!("a playlist called `{name}` already exists, `{was}` kept")),
            // vibox does not list hidden files, so the row would vanish.
            _ if name.starts_with('.') => Some(format!(
                "a name starting with `.` is hidden and would drop out of the list: `{was}` kept"
            )),
            _ => None,
        }
    }

    /// The name a playlist has once `:w` has run its renames.
    pub(super) fn playlist_named(&self, path: &Path) -> String {
        let what = Renaming::Playlist(path.to_path_buf());
        match self.renames.get(&what) {
            Some(name) if !self.dormant(&what) => name.clone(),
            _ => playlist_label(path),
        }
    }

    /// True when a name has been changed but not written.
    pub fn edit_dirty(&self) -> bool {
        if self.renames.keys().any(|what| !self.dormant(what)) {
            return true;
        }
        self.edit
            .as_ref()
            .is_some_and(|edit| edit.buf.text() != self.name_of(&edit.what))
    }

    /// A rename of something marked for deletion, which `:w` does not run: a
    /// row's waits in case a `p` moves it, which takes the new name along, and
    /// any of them comes back when the mark comes off.
    pub(super) fn dormant(&self, what: &Renaming) -> bool {
        match what {
            Renaming::Track(i) => self
                .tracks
                .get(*i)
                .is_some_and(|t| self.is_doomed_file(&t.path)),
            Renaming::Folder(path) => self.is_doomed_dir(path),
            Renaming::Playlist(path) => self.is_doomed(path),
        }
    }

    pub fn end_edit(&mut self) {
        self.edit = None;
        if matches!(self.mode, Mode::Edit | Mode::EditInsert) {
            self.mode = Mode::Normal;
        }
    }

    /// `:e!`: throws away everything waiting for a `:w`.
    ///
    /// All of it, not just the name being typed: the point of the command is
    /// that `[+]` goes out and `:changes` comes back empty. Memory goes back
    /// to what the disk holds without reading it again: a pending move already
    /// shows the file where it is going and a pending copy as a row of its own,
    /// and both are taken back here.
    pub fn discard_changes(&mut self) {
        if self.busy() {
            return;
        }
        let had = self.unsaved();
        self.end_edit();
        self.renames.clear();
        // Copy rows go before any row goes back, since one can be going back
        // to the name a copy sits on.
        let copies: Vec<PathBuf> = std::mem::take(&mut self.copies)
            .into_iter()
            .map(|(_, to)| to)
            .collect();
        self.forget_tracks(&copies);
        // Every moved row is found before any goes back, since one can be
        // going back to a name another has just left.
        let back: Vec<(usize, PathBuf, PathBuf)> = std::mem::take(&mut self.moves)
            .into_iter()
            .filter_map(|(from, to)| Some((self.row_at(&to)?, to, from)))
            .collect();
        for yanked in &mut self.yank {
            if let Some((_, _, origin)) = back.iter().find(|(_, now, _)| *now == yanked.path) {
                yanked.path.clone_from(origin);
            }
        }
        for (i, _, origin) in &back {
            self.relocate_row(*i, origin);
        }
        // Rows a put replaced come back where their files are, once nothing
        // sits on those names, except a replaced copy row, which had no file.
        for Replaced { mut track, file } in std::mem::take(&mut self.replaced) {
            if let Some(file) = file {
                track.path = file;
                self.tracks.push(track);
            }
        }
        self.reattach_playing();
        self.doomed.clear();
        self.doomed_files.clear();
        self.doomed_dirs.clear();
        self.cut.clear();
        self.undo_stack.clear();
        self.redo_stack.clear();

        // Every edited playlist goes back to what its file says.
        self.pl_edits.clear();
        self.refresh_playlist_rows();

        // A put rearranged rows, which the sort puts back.
        self.reload_playlists();
        self.resort();
        self.rebuild_view();

        if had {
            self.info("changes thrown away");
        }
    }

    /// The path a rename would move away from, which is what makes it a
    /// vacated name for anything else in the same batch.
    pub(super) fn rename_source(&self, what: &Renaming) -> Option<PathBuf> {
        match what {
            Renaming::Track(idx) => Some(self.tracks.get(*idx)?.path.clone()),
            Renaming::Folder(path) | Renaming::Playlist(path) => Some(path.clone()),
        }
    }

    /// Where a rename would put something, for the checks and the write.
    pub(super) fn rename_target(&self, what: &Renaming, name: &str) -> Option<PathBuf> {
        match what {
            Renaming::Track(idx) => {
                let path = &self.tracks.get(*idx)?.path;
                // Built by hand: `set_extension` would eat everything after a
                // dot in a name like `Mr. Blue Sky`.
                let file_name = match path.extension().and_then(|e| e.to_str()) {
                    Some(ext) => format!("{name}.{ext}"),
                    None => name.to_string(),
                };
                Some(path.with_file_name(file_name))
            }
            Renaming::Folder(path) => Some(path.with_file_name(name)),
            // Same kind of file as before: an `.m3u8` stays one.
            Renaming::Playlist(from) => {
                let ext = from.extension().and_then(|e| e.to_str()).unwrap_or("m3u");
                Some(from.with_file_name(format!("{name}.{ext}")))
            }
        }
    }
}
