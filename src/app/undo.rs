//! The undo stack: every mutation pushes what it takes to reverse it.

use std::path::PathBuf;

use crate::name::NameBuffer;

use super::*;

impl App {
    fn pending(&self) -> Pending {
        Pending {
            renames: self.renames.clone(),
            playlist_rows: self.playlist_rows.clone(),
            playlist_dirty: self.playlist_dirty,
            doomed: self.doomed.clone(),
            doomed_files: self.doomed_files.clone(),
            cut: self.cut.clone(),
            moves: self.moves.clone(),
            copies: self.copies.clone(),
            doomed_dirs: self.doomed_dirs.clone(),
            sidebar_name: self.edit.as_ref().map(|edit| edit.buf.text()),
        }
    }

    fn apply_pending(&mut self, p: Pending) {
        self.renames = p.renames;
        self.playlist_rows = p.playlist_rows;
        self.playlist_dirty = p.playlist_dirty;
        self.doomed = p.doomed;
        self.doomed_files = p.doomed_files;
        self.cut = p.cut;
        // Paths live on the tracks, so stepping between snapshots means
        // undoing the moves this one does not have and applying the ones it does.
        for (from, to) in &self.moves {
            if !p.moves.contains(&(from.clone(), to.clone()))
                && let Some(track) = self.tracks.iter_mut().find(|t| t.path == *to)
            {
                track.path = from.clone();
            }
        }
        for (from, to) in &p.moves {
            if !self.moves.contains(&(from.clone(), to.clone()))
                && let Some(track) = self.tracks.iter_mut().find(|t| t.path == *from)
            {
                track.path = to.clone();
            }
        }
        // A pending copy already shows in the list, so stepping between
        // snapshots adds and removes those rows too.
        let dropped: Vec<PathBuf> = self
            .copies
            .iter()
            .filter(|pair| !p.copies.contains(pair))
            .map(|(_, to)| to.clone())
            .collect();
        let added: Vec<(PathBuf, PathBuf)> = p
            .copies
            .iter()
            .filter(|pair| !self.copies.contains(pair))
            .cloned()
            .collect();
        self.copies = p.copies;
        if !dropped.is_empty() {
            self.forget_tracks(&dropped);
        }
        for (from, to) in added {
            if let Some(source) = self.tracks.iter().find(|t| t.path == from) {
                let mut copy = source.clone();
                copy.path = to;
                copy.file = copy
                    .path
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                self.tracks.push(copy);
            }
        }

        self.moves = p.moves;
        self.doomed_dirs = p.doomed_dirs;
        if let (Some(edit), Some(name)) = (self.edit.as_mut(), p.sidebar_name) {
            let col = edit.buf.col().min(name.chars().count());
            edit.buf = NameBuffer::new(&name);
            for _ in 0..col {
                edit.buf.right();
            }
        }
        self.rebuild_view();
    }

    /// Called before anything that changes what `:w` would do.
    pub fn checkpoint(&mut self) {
        self.undo_stack.push(self.pending());
        self.redo_stack.clear();
    }

    pub fn undo(&mut self) {
        let Some(previous) = self.undo_stack.pop() else {
            self.info("already at the oldest change");
            return;
        };
        self.redo_stack.push(self.pending());
        self.apply_pending(previous);
        self.info("undo");
    }

    pub fn redo(&mut self) {
        let Some(next) = self.redo_stack.pop() else {
            self.info("already at the newest change");
            return;
        };
        self.undo_stack.push(self.pending());
        self.apply_pending(next);
        self.info("redo");
    }
}
