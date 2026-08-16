//! Renaming files and folders in the edit buffer, applied together on `:w`.

use std::path::PathBuf;


use crate::library::{self};
use crate::name::NameBuffer;

use super::*;

impl App {
    /// Opens the list for editing. Nothing touches the disk until `:w`.
    pub fn begin_edit(&mut self) {
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
        let what = match self.tab {
            Tab::Folders => {
                if self.folder_cur == 0 {
                    self.error("that is the whole library, not a folder");
                    return;
                }
                let Some((_, path)) = self.folders.get(self.folder_cur - 1).cloned() else {
                    return;
                };
                Renaming::Folder(path)
            }
            Tab::Playlists => {
                let Some((name, _)) = self.playlists.get(self.pl_cur).cloned() else {
                    self.error("no playlist here to rename");
                    return;
                };
                Renaming::Playlist(name)
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
            Renaming::Playlist(name) => name.clone(),
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
    /// is why leaving a name never demands a write of its own.
    pub fn commit_name(&mut self) {
        let Some(edit) = self.edit.take() else {
            return;
        };
        let name = edit.buf.text().trim().to_string();
        let was = self.name_of(&edit.what);

        if name == was {
            self.renames.remove(&edit.what);
        } else if !name.is_empty() {
            self.checkpoint();
            self.renames.insert(edit.what, name);
        }
    }

    /// True when a name has been changed but not written.
    pub fn edit_dirty(&self) -> bool {
        if !self.renames.is_empty() {
            return true;
        }
        self.edit
            .as_ref()
            .is_some_and(|edit| edit.buf.text() != self.name_of(&edit.what))
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
    /// that `[+]` goes out and `:changes` comes back empty.
    pub fn discard_changes(&mut self) {
        let had = self.unsaved();
        self.end_edit();
        self.renames.clear();
        self.doomed.clear();
        self.doomed_files.clear();
        self.doomed_dirs.clear();
        self.cut.clear();
        self.moves.clear();
        self.copies.clear();
        self.playlist_dirty = false;
        for tab in &mut self.tabs {
            tab.dirty = false;
        }
        self.undo_stack.clear();
        self.redo_stack.clear();

        // The list is showing cuts and moves that are no longer going to
        // happen, so it has to be rebuilt from the library as it stands.
        if let Err(e) = self.reload() {
            self.error(format!("{e}"));
            return;
        }
        self.reload_playlists();

        if had {
            self.info("changes thrown away");
        }
    }

    /// Renames every changed file. Names are checked first, so a bad one stops
    /// the whole write instead of leaving the batch half applied.
    pub fn apply_edits(&mut self) {
        self.commit_name();
        if self.renames.is_empty() {
            return;
        }

        // Everything is checked before anything runs: a bad name stops the
        // whole write rather than leaving half a folder renamed.
        let queued: Vec<(Renaming, String)> = self
            .renames
            .iter()
            .map(|(what, name)| (what.clone(), name.trim().to_string()))
            .collect();
        for (what, name) in &queued {
            if name.is_empty() {
                self.error("a name cannot be empty");
                return;
            }
            if name.contains('/') {
                self.error("a name cannot contain `/`, this renames but never moves");
                return;
            }
            if let Some(to) = self.rename_target(what, name)
                && to.exists()
            {
                self.error(format!("`{name}` already exists here"));
                return;
            }
        }

        let mut files = 0;
        let mut moved: Vec<(PathBuf, PathBuf)> = Vec::new();
        for (what, name) in queued {
            match what {
                Renaming::Track(idx) => {
                    let old = self.tracks[idx].path.clone();
                    let Some(new) = self.rename_target(&Renaming::Track(idx), &name) else {
                        continue;
                    };
                    match std::fs::rename(&old, &new) {
                        Ok(()) => {
                            self.tracks[idx].path = new.clone();
                            self.tracks[idx].file = name;
                            moved.push((old, new));
                            files += 1;
                        }
                        Err(e) => self.error(format!("cannot rename `{}`: {e}", old.display())),
                    }
                }
                Renaming::Folder(old) => {
                    let new = old.with_file_name(&name);
                    if let Err(e) = std::fs::rename(&old, &new) {
                        self.error(format!("cannot rename `{name}`: {e}"));
                        continue;
                    }
                    // One rename moves the folder; the tracks inside it just
                    // need their paths followed.
                    for track in &mut self.tracks {
                        if let Ok(rest) = track.path.strip_prefix(&old) {
                            let to = new.join(rest);
                            moved.push((track.path.clone(), to.clone()));
                            track.path = to;
                        }
                    }
                    files += 1;
                }
                Renaming::Playlist(old) => {
                    let Some(dir) = self.playlists_in() else {
                        continue;
                    };
                    let to = dir.join(format!("{name}.m3u"));
                    if let Err(e) = std::fs::rename(dir.join(format!("{old}.m3u")), &to) {
                        self.error(format!("cannot rename `{old}`: {e}"));
                        continue;
                    }
                    // A tab showing it keeps showing it, under the new name.
                    if self.playlist_view.as_deref() == Some(old.as_str()) {
                        self.playlist_view = Some(name.clone());
                    }
                    for tab in &mut self.tabs {
                        if tab.playlist.as_deref() == Some(old.as_str()) {
                            tab.playlist = Some(name.clone());
                        }
                    }
                    files += 1;
                }
            }
        }

        self.renames.clear();
        self.end_edit();
        let fixed = self.repair_playlists(&moved);
        self.folders = library::folders(&self.tracks, &self.root);
        self.reload_playlists();
        self.rebuild_view();

        if fixed > 0 {
            self.info(format!(
                "renamed {files}, updated {fixed} playlist{}",
                plural(fixed)
            ));
        } else {
            self.info(format!("renamed {files}"));
        }
    }

    /// Where a rename would put something, for the checks and the write.
    fn rename_target(&self, what: &Renaming, name: &str) -> Option<PathBuf> {
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
            Renaming::Playlist(_) => Some(self.playlists_in()?.join(format!("{name}.m3u"))),
        }
    }
}
