//! Opening and scanning a library, and the folder list it produces.

use std::path::Path;

use anyhow::Result;

use crate::library::{self, SortKey, Track};

use super::*;

impl App {
    /// Rescans the root from disk, keeping the cursor on the same track if it
    /// survived the rescan.
    pub fn reload(&mut self) -> Result<()> {
        self.reload_reporting(library::QUIET)
    }

    /// `reload`, saying how far along the scan is. Only the first one, before
    /// the terminal is taken over, has anywhere to say it.
    pub fn reload_reporting(&mut self, on: library::Report) -> Result<()> {
        let under_cursor = self.current_track().map(|t| t.path.clone());

        self.tracks = library::scan_reporting(&self.root, on)?;
        // `path` order means "the order the files came in", which for a
        // playlist is the order it was written, so leave that one alone.
        if !(library::is_playlist(&self.root) && self.sort_key == SortKey::Path) {
            library::sort(&mut self.tracks, self.sort_key);
        }
        let base = if self.root.is_dir() {
            self.root.clone()
        } else {
            self.root.parent().unwrap_or(&self.root).to_path_buf()
        };
        self.folders = library::folders(&self.tracks, &base);
        self.folder_cur = self.folder_cur.min(self.folders.len());
        self.playing = None;
        self.queue.clear();
        self.rebuild_view();

        if let Some(path) = under_cursor
            && let Some(pos) = self.view.iter().position(|&i| self.tracks[i].path == path)
        {
            self.cur = pos;
        }
        self.clamp();
        Ok(())
    }

    pub fn open(&mut self, path: &Path) -> Result<()> {
        let root = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.root.join(path)
        };
        let root = root.canonicalize().unwrap_or(root);
        self.root = root;
        self.folder_cur = 0;
        self.folder_open = 0;
        self.cur = 0;
        self.top = 0;
        self.reload()
    }

    pub fn set_sort(&mut self, key: SortKey) {
        self.sort_key = key;
        self.resort();
    }

    /// Rearranges `tracks` into `order`, taking every index that points into
    /// it along.
    ///
    /// `order[i]` is where the track that ends up at row `i` is now. Same
    /// bookkeeping as `resort`, which is the other thing allowed to move rows
    /// around.
    pub(super) fn reorder(&mut self, order: &[usize]) {
        if order.len() != self.tracks.len() {
            return;
        }
        let under_cursor = self.current_track().map(|t| t.path.clone());

        let mut remap = vec![0; order.len()];
        for (to, &from) in order.iter().enumerate() {
            remap[from] = to;
        }

        let mut taken: Vec<Option<Track>> = self.tracks.drain(..).map(Some).collect();
        self.tracks = order
            .iter()
            .filter_map(|&from| taken[from].take())
            .collect();

        self.playlist_rows = self.playlist_rows.iter().map(|&i| remap[i]).collect();
        self.queue = self.queue.iter().map(|&i| remap[i]).collect();
        self.playing = self.playing.map(|i| remap[i]);
        for tab in &mut self.tabs {
            tab.rows = tab.rows.iter().map(|&i| remap[i]).collect();
        }

        self.rebuild_view();
        if let Some(path) = under_cursor
            && let Some(pos) = self.view.iter().position(|&i| self.tracks[i].path == path)
        {
            self.cur = pos;
        }
        self.clamp();
    }

    /// Puts `moved` back next to the cursor, where they were just dropped.
    ///
    /// A move rewrites each track's path, and the list is in path order, so
    /// left alone the rows jump to wherever the new name sorts, which reads as
    /// "they vanished". They stay put until the next sort or `:w`.
    pub(super) fn gather_at_cursor(&mut self, moved: &[std::path::PathBuf]) {
        if moved.is_empty() {
            return;
        }
        let is_moved: Vec<bool> = self
            .tracks
            .iter()
            .map(|t| moved.contains(&t.path))
            .collect();

        // Where they go: in front of the row the cursor is on, or where that
        // row was if the cursor was sitting on one of the moved tracks.
        let anchor = self.view.get(self.cur).copied();

        let just_moved: Vec<usize> = is_moved
            .iter()
            .enumerate()
            .filter_map(|(i, moved)| moved.then_some(i))
            .collect();

        let mut order = Vec::with_capacity(self.tracks.len());
        let mut placed = false;
        for (i, moved) in is_moved.iter().enumerate() {
            if Some(i) == anchor && !placed {
                order.extend_from_slice(&just_moved);
                placed = true;
            }
            if !moved {
                order.push(i);
            }
        }
        if !placed {
            order.extend_from_slice(&just_moved);
        }

        self.reorder(&order);
    }

    /// Sorts the library again, taking everything that points into it along.
    ///
    /// `queue`, `playing`, `playlist_rows` and each tab's rows are positions
    /// in `tracks`, so reordering the vec on its own silently repoints all of
    /// them at other songs. This is the same bookkeeping `forget_tracks` does
    /// for a deletion, and every reorder needs it.
    pub(super) fn resort(&mut self) {
        let under_cursor = self.current_track().map(|t| t.path.clone());

        let before: Vec<PathBuf> = self.tracks.iter().map(|t| t.path.clone()).collect();
        library::sort(&mut self.tracks, self.sort_key);

        // Where each track ended up. Paths are unique, which is what makes
        // this a safe way to reuse `library::sort` rather than repeating its
        // comparison here.
        let now: std::collections::HashMap<&Path, usize> = self
            .tracks
            .iter()
            .enumerate()
            .map(|(i, t)| (t.path.as_path(), i))
            .collect();
        let Some(remap) = before
            .iter()
            .map(|p| now.get(p.as_path()).copied())
            .collect::<Option<Vec<usize>>>()
        else {
            // Two tracks sharing a path would make the mapping ambiguous.
            // Nothing should produce that, and guessing is worse than leaving
            // the indices as they are.
            self.rebuild_view();
            self.clamp();
            return;
        };

        self.playlist_rows = self.playlist_rows.iter().map(|&i| remap[i]).collect();
        self.queue = self.queue.iter().map(|&i| remap[i]).collect();
        self.playing = self.playing.map(|i| remap[i]);
        for tab in &mut self.tabs {
            tab.rows = tab.rows.iter().map(|&i| remap[i]).collect();
        }

        self.rebuild_view();
        if let Some(path) = under_cursor
            && let Some(pos) = self.view.iter().position(|&i| self.tracks[i].path == path)
        {
            self.cur = pos;
        }
        self.clamp();
    }

    /// How many tracks `everything` actually shows.
    ///
    /// Not `tracks.len()`: a playlist naming files from outside the library
    /// reads them in so it can play them, and those sit in `tracks` without
    /// ever being part of the library.
    pub fn library_len(&self) -> usize {
        self.tracks.iter().filter(|t| t.in_library).count()
    }

    /// Recomputes the visible track list from the playlist or the folder.
    pub fn rebuild_view(&mut self) {
        if self.playlist_view.is_some() {
            self.view = self.playlist_rows.clone();
            self.clamp();
            return;
        }
        // A marked deletion is gone from the list straight away: this is a
        // buffer, so it should look like the edit already happened.
        self.view = match self.folder_open {
            0 => (0..self.tracks.len())
                .filter(|&t| self.tracks[t].in_library)
                .collect(),
            i => {
                let dir = self.folders[i - 1].1.clone();
                (0..self.tracks.len())
                    .filter(|&t| self.tracks[t].dir() == dir)
                    .collect()
            }
        };
        if !self.doomed_files.is_empty() {
            self.view
                .retain(|&t| !self.doomed_files.contains(&self.tracks[t].path));
        }
        self.clamp();
    }

    pub fn current_track(&self) -> Option<&Track> {
        self.view.get(self.cur).map(|&i| &self.tracks[i])
    }

    pub fn playing_track(&self) -> Option<&Track> {
        self.playing.map(|i| &self.tracks[i])
    }
}
