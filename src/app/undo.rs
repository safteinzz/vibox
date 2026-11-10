//! The undo stack: every mutation pushes what it takes to reverse it.

use std::path::PathBuf;

use crate::name::NameBuffer;

use super::*;

impl App {
    fn pending(&self) -> Pending {
        let held = |what: &Renaming| -> Option<Held> {
            match what {
                Renaming::Track(i) => Some(Held::Track(self.tracks.get(*i)?.path.clone())),
                other => Some(Held::Other(other.clone())),
            }
        };
        let paths = |rows: &[usize]| -> Vec<PathBuf> {
            rows.iter()
                .filter_map(|&i| self.tracks.get(i).map(|t| t.path.clone()))
                .collect()
        };
        Pending {
            renames: self
                .renames
                .iter()
                .filter_map(|(what, name)| Some((held(what)?, name.clone())))
                .collect(),
            pl_edits: self
                .pl_edits
                .iter()
                .map(|(path, rows)| (path.clone(), paths(rows)))
                .collect(),
            doomed: self.doomed.clone(),
            doomed_files: self.doomed_files.clone(),
            replaced: self.replaced.clone(),
            cut: self.cut.clone(),
            moves: self.moves.clone(),
            copies: self.copies.clone(),
            copy_rows: self
                .copies
                .iter()
                .filter_map(|(_, to)| self.row_at(to).map(|i| self.tracks[i].clone()))
                .collect(),
            doomed_dirs: self.doomed_dirs.clone(),
            typing: self
                .edit
                .as_ref()
                .and_then(|edit| Some((held(&edit.what)?, edit.buf.text()))),
        }
    }

    /// Steps memory to the state `p` recorded. Paths and rows live on the
    /// tracks, so the step is the difference between the two, taken in an
    /// order where no row lands on a name another row still has: rows that
    /// leave go first (the ones `p` has replaced, and copy rows `p` does not
    /// have), then moves are undone or done again, then copy rows and replaced
    /// rows come back. Every row is then where it was when `p` was taken, so
    /// the paths in `p` name the rows they named then.
    fn apply_pending(&mut self, p: Pending) {
        self.doomed = p.doomed;
        self.doomed_files = p.doomed_files;
        self.cut = p.cut;
        self.doomed_dirs = p.doomed_dirs;

        let mut leaving: Vec<PathBuf> = p
            .replaced
            .iter()
            .filter(|r| !self.replaced.contains(r))
            .map(|r| r.track.path.clone())
            .collect();
        leaving.extend(
            self.copies
                .iter()
                .filter(|pair| !p.copies.contains(pair))
                .map(|(_, to)| to.clone()),
        );
        if !leaving.is_empty() {
            self.forget_tracks(&leaving);
        }

        // Every row is found before any of them moves: one step can free a
        // name and fill it, and a row found by a name another row has just
        // taken would be the wrong one.
        let mut steps: Vec<(usize, PathBuf, PathBuf)> = Vec::new();
        for (origin, now) in &self.moves {
            let then = p
                .moves
                .iter()
                .find(|(o, _)| o == origin)
                .map_or(origin, |(_, t)| t);
            if then != now
                && let Some(i) = self.row_at(now)
            {
                steps.push((i, now.clone(), then.clone()));
            }
        }
        for (origin, then) in &p.moves {
            if !self.moves.iter().any(|(o, _)| o == origin)
                && let Some(i) = self.row_at(origin)
            {
                steps.push((i, origin.clone(), then.clone()));
            }
        }
        for yanked in &mut self.yank {
            if let Some((_, _, to)) = steps.iter().find(|(_, from, _)| *from == yanked.path) {
                yanked.path.clone_from(to);
            }
        }
        for (i, _, to) in &steps {
            self.relocate_row(*i, to);
        }
        self.moves = p.moves;

        // By the row missing, not by the pair: a put can replace a copy row
        // with a copy of the same file onto the same name.
        let added: Vec<Track> = p
            .copies
            .iter()
            .filter(|(_, to)| self.row_at(to).is_none())
            .filter_map(|(_, to)| p.copy_rows.iter().find(|t| t.path == *to).cloned())
            .collect();
        self.tracks.extend(added);
        self.copies = p.copies;
        // A replaced copy row came back above, as the copy it was.
        let returning: Vec<Track> = self
            .replaced
            .iter()
            .filter(|r| r.file.is_some() && !p.replaced.contains(r))
            .map(|r| r.track.clone())
            .collect();
        self.tracks.extend(returning);
        self.replaced = p.replaced;

        let row = |app: &App, held: &Held| -> Option<Renaming> {
            match held {
                Held::Track(path) => app.row_at(path).map(Renaming::Track),
                Held::Other(what) => Some(what.clone()),
            }
        };
        self.renames = p
            .renames
            .iter()
            .filter_map(|(held, name)| Some((row(self, held)?, name.clone())))
            .collect();
        self.pl_edits = p
            .pl_edits
            .iter()
            .map(|(path, rows)| {
                let rows = rows.iter().filter_map(|r| self.row_at(r)).collect();
                (path.clone(), rows)
            })
            .collect();
        // Only into the buffer the text was typed in: a snapshot taken on
        // another row would otherwise rename this one to that row's name.
        let typed = p
            .typing
            .and_then(|(held, name)| Some((row(self, &held)?, name)));
        if let (Some(edit), Some((what, name))) = (self.edit.as_mut(), typed)
            && edit.what == what
        {
            let col = edit.buf.col().min(name.chars().count());
            edit.buf = NameBuffer::new(&name);
            for _ in 0..col {
                edit.buf.right();
            }
        }
        self.reattach_playing();
        self.refresh_playlist_rows();
        self.rebuild_view();
    }

    /// Called before anything that changes what `:w` would do.
    pub fn checkpoint(&mut self) {
        self.undo_stack.push(self.pending());
        self.redo_stack.clear();
    }

    pub fn undo(&mut self) {
        if self.busy() {
            return;
        }
        let Some(previous) = self.undo_stack.pop() else {
            self.info("already at the oldest change");
            return;
        };
        self.redo_stack.push(self.pending());
        self.apply_pending(previous);
        self.info("undo");
    }

    pub fn redo(&mut self) {
        if self.busy() {
            return;
        }
        let Some(next) = self.redo_stack.pop() else {
            self.info("already at the newest change");
            return;
        };
        self.undo_stack.push(self.pending());
        self.apply_pending(next);
        self.info("redo");
    }
}
