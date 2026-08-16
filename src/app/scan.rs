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
        let under_cursor = self.current_track().map(|t| t.path.clone());
        library::sort(&mut self.tracks, key);
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
