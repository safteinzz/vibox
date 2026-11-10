//! Opening and scanning a library, the folder list it produces, and keeping
//! every index into `tracks` pointing at the same track while it changes.

use std::collections::HashMap;
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
    /// the terminal is taken over, has anywhere to say it; every later one runs
    /// in the background (`App::start_scan`).
    pub fn reload_reporting(&mut self, on: library::Report) -> Result<()> {
        let tracks = library::scan_reporting(&self.root, on)?;
        self.adopt(tracks);
        Ok(())
    }

    /// Replaces the library in memory with `tracks`, a fresh scan of `root`.
    ///
    /// Every index into `tracks` means something else afterwards, so whatever
    /// holds one is remembered as paths and looked up again: what is playing
    /// and the queue it plays through, and the rows of every open playlist,
    /// whose files from outside the library are kept as rows of their own.
    /// Dropping them instead left the sound thread playing a track the
    /// statusline called `not playing`, so a playing track the new library
    /// does not have is kept the same way while its file is still there. Undo
    /// snapshots cannot be mapped like that, so they are dropped.
    pub(super) fn adopt(&mut self, tracks: Vec<Track>) {
        let path_of = |app: &App, i: usize| app.tracks.get(i).map(|t| t.path.clone());
        let under_cursor = self.current_track().map(|t| t.path.clone());
        let was_playing = self.playing_track().cloned();
        // A row made the playing one without starting the sound thread plays
        // its own file.
        let playing_file = self
            .playing_file
            .clone()
            .or_else(|| was_playing.as_ref().map(|t| self.origin_of(&t.path)));
        let queued: Vec<Option<PathBuf>> = self.queue.iter().map(|&i| path_of(self, i)).collect();
        // Rows outside the library keep their whole track, since a rescan of
        // the library will not bring them back.
        let rows_of = |app: &App, rows: &[usize]| -> Vec<Track> {
            rows.iter()
                .filter_map(|&i| app.tracks.get(i).cloned())
                .collect()
        };
        let open_rows = rows_of(self, &self.playlist_rows);
        let edited: Vec<(PathBuf, Vec<Track>)> = self
            .pl_edits
            .iter()
            .map(|(path, rows)| (path.clone(), rows_of(self, rows)))
            .collect();
        let typing = match self.renaming() {
            Some(Renaming::Track(i)) => path_of(self, *i),
            _ => None,
        };
        let renamed: Vec<(PathBuf, Renaming, String)> = self
            .renames
            .iter()
            .filter_map(|(what, name)| match what {
                Renaming::Track(i) => Some((path_of(self, *i)?, what.clone(), name.clone())),
                _ => None,
            })
            .collect();

        self.tracks = tracks;
        // `path` order means "the order the files came in", which for a
        // playlist is the order it was written, so leave that one alone.
        if !self.keeps_its_own_order() {
            library::sort(&mut self.tracks, self.sort_key);
        }
        self.undo_stack.clear();
        self.redo_stack.clear();

        let mut index: HashMap<PathBuf, usize> = self
            .tracks
            .iter()
            .enumerate()
            .map(|(i, t)| (t.path.clone(), i))
            .collect();
        let mut row_of = |app: &mut App, track: &Track| -> usize {
            if let Some(&i) = index.get(&track.path) {
                return i;
            }
            let mut stray = track.clone();
            stray.in_library = false;
            app.tracks.push(stray);
            index.insert(track.path.clone(), app.tracks.len() - 1);
            app.tracks.len() - 1
        };
        self.playlist_rows = open_rows.iter().map(|t| row_of(self, t)).collect();
        for (path, rows) in &edited {
            let mapped: Vec<usize> = rows.iter().map(|t| row_of(self, t)).collect();
            self.pl_edits.insert(path.clone(), mapped);
        }
        // The row of the file the sound thread plays, whatever name its row
        // showed it under. A file that is gone is not playing anything,
        // whatever the sound thread has left in its buffer.
        self.playing = match (was_playing, playing_file) {
            (Some(mut track), Some(file)) if file.exists() => {
                track.path = file;
                Some(row_of(self, &track))
            }
            _ => None,
        };
        if self.playing.is_none() {
            self.playing_file = None;
        }
        self.playing_gone = None;
        // A name being typed stays on its row, wherever the row is now.
        if let Some(path) = typing {
            match index.get(&path) {
                Some(&i) => {
                    if let Some(edit) = self.edit.as_mut() {
                        edit.what = Renaming::Track(i);
                    }
                }
                None => self.end_edit(),
            }
        }

        // The queue keeps its own order: it is a snapshot of the view as it
        // was when playback started, not of the view as it is now. A track
        // that left the library on this scan simply drops out of it.
        let mut queue = Vec::with_capacity(queued.len());
        let mut moved_to = Vec::with_capacity(queued.len());
        for path in &queued {
            match path.as_ref().and_then(|p| index.get(p)) {
                Some(&i) => {
                    moved_to.push(Some(queue.len()));
                    queue.push(i);
                }
                None => moved_to.push(None),
            }
        }
        self.queue = queue;
        self.follow_queue(&moved_to);
        self.history.clear();
        self.shuffle_bag.clear();
        self.renames
            .retain(|what, _| !matches!(what, Renaming::Track(_)));
        for (path, _, name) in renamed {
            if let Some(&i) = index.get(&path) {
                self.renames.insert(Renaming::Track(i), name);
            }
        }

        self.set_folders();
        self.rebuild_view();
        if let Some(path) = under_cursor
            && let Some(pos) = self.view.iter().position(|&i| self.tracks[i].path == path)
        {
            self.cur = pos;
        }
        self.clamp();
    }

    /// The directory the library lives in: the root itself, or the folder
    /// holding the m3u or the file it was opened from.
    pub fn root_dir(&self) -> PathBuf {
        if self.root.is_dir() {
            self.root.clone()
        } else {
            self.root
                .parent()
                .map_or_else(|| self.root.clone(), Path::to_path_buf)
        }
    }

    /// True when the library is an m3u in its own order, which no sort by
    /// path may undo.
    fn keeps_its_own_order(&self) -> bool {
        library::is_playlist(&self.root) && self.sort_key == SortKey::Path
    }

    /// Rebuilds the folder list, keeping the cursor, the open folder and every
    /// tab on the folder they were on rather than on the same row number.
    pub(super) fn set_folders(&mut self) {
        let fresh = library::folders(&self.tracks, &self.root);
        let path_at = |row: usize| -> Option<PathBuf> {
            row.checked_sub(1)
                .and_then(|i| self.folders.get(i))
                .map(|(_, p)| p.clone())
        };
        let row_now = |path: Option<PathBuf>, fallback: usize| -> usize {
            match path {
                None => 0,
                Some(path) => fresh
                    .iter()
                    .position(|(_, p)| *p == path)
                    .map_or(fallback.min(fresh.len()), |i| i + 1),
            }
        };
        let cur = row_now(path_at(self.folder_cur), self.folder_cur);
        let open = row_now(path_at(self.folder_open), 0);
        let tabs: Vec<usize> = self
            .tabs
            .iter()
            .map(|tab| row_now(path_at(tab.folder), 0))
            .collect();

        self.folders = fresh;
        self.folder_cur = cur;
        self.folder_open = open;
        for (tab, folder) in self.tabs.iter_mut().zip(tabs) {
            tab.folder = folder;
        }
    }

    pub fn set_sort(&mut self, key: SortKey) {
        self.sort_key = key;
        self.resort();
    }

    /// Repoints everything that holds an index into `tracks` after the vec was
    /// rearranged or shrunk. `remap[old]` is where that track is now, or `None`
    /// once it is gone.
    ///
    /// Every index lives here: the view, the queue and the positions into it,
    /// what is playing, the open playlist and every edited one, the pending
    /// renames and the name being typed, so a sort or a put can never leave a
    /// rename pointing at another file. Undo snapshots hold paths, not indices.
    pub(super) fn retarget(&mut self, remap: &[Option<usize>]) {
        let follow = |i: usize| remap.get(i).copied().flatten();
        let rows = |v: &[usize]| -> Vec<usize> { v.iter().filter_map(|&i| follow(i)).collect() };
        let renamed = |what: &Renaming| -> Option<Renaming> {
            match what {
                Renaming::Track(i) => follow(*i).map(Renaming::Track),
                other => Some(other.clone()),
            }
        };

        let under_cursor = self.view.get(self.cur).and_then(|&i| follow(i));

        // The queue keeps its order, and the positions into it (where we are,
        // the shuffle history and bag) follow the entries that survive.
        let mut queue = Vec::with_capacity(self.queue.len());
        let mut moved_to: Vec<Option<usize>> = Vec::with_capacity(self.queue.len());
        for &i in &self.queue {
            match follow(i) {
                Some(now) => {
                    moved_to.push(Some(queue.len()));
                    queue.push(now);
                }
                None => moved_to.push(None),
            }
        }
        self.queue = queue;
        self.follow_queue(&moved_to);
        self.playing = self.playing.and_then(follow);

        self.playlist_rows = rows(&self.playlist_rows);
        self.pl_edits = self
            .pl_edits
            .iter()
            .map(|(path, v)| (path.clone(), rows(v)))
            .collect();
        self.renames = self
            .renames
            .iter()
            .filter_map(|(what, name)| Some((renamed(what)?, name.clone())))
            .collect();
        if let Some(edit) = self.edit.as_mut() {
            match renamed(&edit.what) {
                Some(what) => edit.what = what,
                None => self.end_edit(),
            }
        }

        self.rebuild_view();
        if let Some(track) = under_cursor
            && let Some(row) = self.view.iter().position(|&i| i == track)
        {
            self.cur = row;
        }
        self.clamp();
    }

    /// Follows the positions into the queue once entries left it,
    /// `moved_to[p]` being where the entry at `p` is now. An entry that left
    /// hands its place to the one before it, so the next track is still the
    /// one after it; with none before it, `qpos` becomes that next track
    /// itself (`qpos_next`).
    fn follow_queue(&mut self, moved_to: &[Option<usize>]) {
        match moved_to.get(self.qpos).copied() {
            Some(Some(now)) => self.qpos = now,
            Some(None) => {
                let before = moved_to.iter().take(self.qpos).flatten().count();
                if self.qpos_next || before == 0 {
                    self.qpos = before;
                    self.qpos_next = true;
                } else {
                    self.qpos = before - 1;
                }
            }
            // Nothing was queued there.
            None => self.qpos = moved_to.iter().flatten().count().saturating_sub(1),
        }
        let positions = |v: &[usize]| -> Vec<usize> {
            v.iter()
                .filter_map(|&p| moved_to.get(p).copied().flatten())
                .collect()
        };
        self.history = positions(&self.history);
        self.shuffle_bag = positions(&self.shuffle_bag);
    }

    /// Rearranges `tracks` into `order`, taking every index that points into
    /// it along. `order[i]` is where the track that ends up at row `i` is now.
    pub(super) fn reorder(&mut self, order: &[usize]) {
        if order.len() != self.tracks.len() {
            return;
        }
        let mut remap = vec![None; order.len()];
        for (to, &from) in order.iter().enumerate() {
            remap[from] = Some(to);
        }
        let mut taken: Vec<Option<Track>> = self.tracks.drain(..).map(Some).collect();
        self.tracks = order
            .iter()
            .filter_map(|&from| taken[from].take())
            .collect();
        self.retarget(&remap);
    }

    /// Puts `moved` just below the cursor, where they were dropped, and the
    /// cursor on the first of them, which is where a put lands.
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
        // The row the cursor is on, unless it is one of the moved tracks.
        let anchor = self.view.get(self.cur).copied().filter(|&i| !is_moved[i]);
        let just_moved: Vec<usize> = is_moved
            .iter()
            .enumerate()
            .filter_map(|(i, moved)| moved.then_some(i))
            .collect();

        let mut order = Vec::with_capacity(self.tracks.len());
        let mut placed = false;
        for (i, moved) in is_moved.iter().enumerate() {
            if !moved {
                order.push(i);
            }
            if Some(i) == anchor {
                order.extend_from_slice(&just_moved);
                placed = true;
            }
        }
        if !placed {
            order.extend_from_slice(&just_moved);
        }

        let first = just_moved.first().copied();
        self.reorder(&order);
        // `reorder` keeps the cursor on its track; `p` moves it onto the put.
        if let Some(first) = first {
            let now = order.iter().position(|&from| from == first);
            if let Some(row) = now.and_then(|t| self.view.iter().position(|&i| i == t)) {
                self.cur = row;
                self.clamp();
            }
        }
    }

    /// Sorts the library again, taking everything that points into it along.
    ///
    /// A library opened from an m3u keeps its own order under the path sort,
    /// the same as a rescan leaves it.
    pub(super) fn resort(&mut self) {
        if self.keeps_its_own_order() {
            self.rebuild_view();
            self.clamp();
            return;
        }
        let key = self.sort_key;
        let mut order: Vec<usize> = (0..self.tracks.len()).collect();
        order.sort_by(|&a, &b| library::compare(&self.tracks[a], &self.tracks[b], key));
        self.reorder(&order);
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
            i => match self.folders.get(i - 1) {
                Some((_, dir)) => (0..self.tracks.len())
                    .filter(|&t| self.tracks[t].in_library && self.tracks[t].dir() == dir)
                    .collect(),
                None => Vec::new(),
            },
        };
        if !self.doomed_files.is_empty() || !self.doomed_dirs.is_empty() {
            let view = std::mem::take(&mut self.view);
            self.view = view
                .into_iter()
                .filter(|&t| !self.is_doomed_file(&self.tracks[t].path))
                .collect();
        }
        self.clamp();
    }

    pub fn current_track(&self) -> Option<&Track> {
        self.view.get(self.cur).map(|&i| &self.tracks[i])
    }

    pub fn playing_track(&self) -> Option<&Track> {
        self.playing
            .map(|i| &self.tracks[i])
            .or(self.playing_gone.as_ref())
    }

    /// Gives the playing track its row again once undo has brought the row
    /// back.
    pub(super) fn reattach_playing(&mut self) {
        if self.playing.is_none()
            && let Some(track) = &self.playing_gone
            && let Some(i) = self.row_at(&track.path)
            && self.tracks[i] == *track
        {
            self.playing = Some(i);
            self.playing_gone = None;
        }
    }
}
