//! Playlists: reading m3u files, editing them in memory, writing them on `:w`.

use std::path::{Path, PathBuf};


use crate::library::{self, Track};

use super::*;

impl App {
    /// `p`: puts the yanked tracks into the open playlist, after the cursor.
    ///
    /// Only into a playlist: vibox does not add files to a folder.
    pub fn paste_into_playlist(&mut self) {
        if self.playlist_view.is_none() {
            self.error("`p` adds to a playlist: open one with `gt` and enter");
            return;
        }
        if self.yank.is_empty() {
            self.error("nothing yanked");
            return;
        }

        self.checkpoint();
        let mut rows = Vec::new();
        for path in self.yank.clone() {
            if let Some(row) = self.tracks.iter().position(|t| t.path == path) {
                rows.push(row);
            }
        }
        let n = rows.len();
        let at = (self.cur + 1).min(self.playlist_rows.len());
        self.playlist_rows.splice(at..at, rows);
        self.playlist_dirty = true;
        self.rebuild_view();
        self.goto(at);
        let name = self.playlist_view.clone().unwrap_or_default();
        self.info(format!("put {n} track{} into `{name}`, `:w` saves", plural(n)));
    }

    /// `dd`: drops rows from the open playlist. The files stay on disk.
    pub fn remove_from_playlist(&mut self) {
        if self.playlist_view.is_none() {
            self.error("`dd` removes a track from a playlist, never from disk");
            return;
        }
        if self.playlist_rows.is_empty() {
            return;
        }
        self.checkpoint();
        let (a, b) = self.selection_range();
        let b = b.min(self.playlist_rows.len() - 1);

        // `dd` is a cut, as in vim: what comes out goes in the register, so a
        // `p` further down the playlist puts it back there.
        let taken: Vec<usize> = self.playlist_rows.drain(a..=b).collect();
        self.yank = taken
            .iter()
            .filter_map(|&i| self.tracks.get(i).map(|t| t.path.clone()))
            .collect();

        self.playlist_dirty = true;
        self.exit_visual();
        self.cur = a;
        self.rebuild_view();
        let n = b - a + 1;
        let name = self.playlist_view.clone().unwrap_or_default();
        self.info(format!(
            "cut {n} track{} from `{name}`, `p` puts them back, `:w` saves",
            plural(n)
        ));
    }

    /// Creates an empty playlist, ready for `p` to fill.
    pub fn create_playlist(&mut self, name: &str) {
        let name = name.trim();
        if name.is_empty() {
            self.error("`:playlist` needs a name: `:playlist late night`");
            return;
        }
        if name.contains('/') {
            self.error("a playlist name cannot contain `/`");
            return;
        }
        let Some(dir) = self.playlists_in() else {
            self.error("cannot find a data directory to save playlists in");
            return;
        };

        let path = dir.join(format!("{name}.m3u"));
        if path.exists() {
            self.error(format!("playlist `{name}` already exists"));
            return;
        }

        match library::write_m3u(&path, &[]) {
            Ok(()) => {
                self.reload_playlists();
                self.tab = Tab::Playlists;
                if let Some(row) = self.playlists.iter().position(|(n, _)| n == name) {
                    self.pl_cur = row;
                }
                self.info(format!("`{name}` created, `p` puts tracks in it"));
            }
            Err(e) => self.error(format!("{e}")),
        }
    }

    /// `dd` on a playlist row marks the m3u for deletion, and a second `dd`
    /// takes the mark off again. Nothing happens on disk until `:w`.
    pub fn delete_playlist(&mut self) {
        let Some((name, path)) = self.playlists.get(self.pl_cur).cloned() else {
            return;
        };

        self.checkpoint();
        if let Some(at) = self.doomed.iter().position(|p| *p == path) {
            self.doomed.remove(at);
            self.info(format!("`{name}` kept"));
        } else {
            self.doomed.push(path);
            self.info(format!("`{name}` will be deleted on `:w`, `dd` undoes it"));
        }
    }

    pub fn is_doomed(&self, path: &Path) -> bool {
        self.doomed.iter().any(|p| p == path)
    }

    /// Deletes the playlists marked with `dd`. The tracks they named stay.
    fn apply_deletes(&mut self) -> usize {
        let mut gone = 0;
        for path in std::mem::take(&mut self.doomed) {
            let name = path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            match std::fs::remove_file(&path) {
                Ok(()) => {
                    if self.playlist_view.as_deref() == Some(name.as_str()) {
                        self.playlist_view = None;
                        self.playlist_dirty = false;
                        self.rebuild_view();
                    }
                    // A tab you are not looking at must not keep showing a
                    // playlist whose file no longer exists.
                    for tab in &mut self.tabs {
                        if tab.playlist.as_deref() == Some(name.as_str()) {
                            tab.playlist = None;
                            tab.rows.clear();
                            tab.dirty = false;
                        }
                    }
                    gone += 1;
                }
                Err(e) => self.error(format!("cannot delete `{name}`: {e}")),
            }
        }
        if gone > 0 {
            self.reload_playlists();
        }
        gone
    }

    /// Rewrites every playlist that names a file we just renamed, so a rename
    /// never leaves a playlist pointing at a path that is gone.
    pub(super) fn repair_playlists(&mut self, renamed: &[(PathBuf, PathBuf)]) -> usize {
        let mut fixed = 0;
        for (_, path) in self.playlists.clone() {
            let Ok(paths) = library::read_m3u(&path) else {
                continue;
            };
            if !paths.iter().any(|p| renamed.iter().any(|(old, _)| old == p)) {
                continue;
            }

            let updated: Vec<PathBuf> = paths
                .into_iter()
                .map(|p| {
                    renamed
                        .iter()
                        .find(|(old, _)| *old == p)
                        .map_or(p, |(_, new)| new.clone())
                })
                .collect();
            let tracks: Vec<&Track> = updated
                .iter()
                .filter_map(|p| self.tracks.iter().find(|t| t.path == *p))
                .collect();
            if library::write_m3u(&path, &tracks).is_ok() {
                fixed += 1;
            }
        }
        fixed
    }

    /// A path as `root/...`, since every change is inside the library anyway.
    /// Quoted when it has a space in it, so the arrow stays readable.
    pub fn under_root(&self, path: &Path) -> String {
        let shown = match path.strip_prefix(&self.root) {
            Ok(rel) => format!("root/{}", rel.display()),
            Err(_) => path.display().to_string(),
        };
        if shown.contains(' ') {
            format!("'{shown}'")
        } else {
            shown
        }
    }

    /// What `:w` would do right now, one line each, for `:changes`.
    ///
    /// Every line starts with its verb, which is also what colours it.
    pub fn pending_changes(&self) -> Vec<String> {
        let mut out = Vec::new();
        // Every rename, wherever it was typed, reads the same way.
        let mut queued = self.renames.clone();
        if let Some(edit) = self.edit.as_ref() {
            let now = edit.buf.text();
            if now.trim().is_empty() || now == self.name_of(&edit.what) {
                queued.remove(&edit.what);
            } else {
                queued.insert(edit.what.clone(), now);
            }
        }
        for (what, name) in &queued {
            out.push(match what {
                Renaming::Track(idx) => {
                    let path = &self.tracks[*idx].path;
                    format!(
                        "rename  {}  ->  {}",
                        self.under_root(path),
                        self.under_root(&path.with_file_name(name))
                    )
                }
                Renaming::Folder(path) => format!(
                    "rename  folder {}  ->  {}",
                    self.under_root(path),
                    self.under_root(&path.with_file_name(name))
                ),
                Renaming::Playlist(old) => {
                    format!("rename  playlist `{old}`  ->  `{name}`")
                }
            });
        }
        if self.playlist_dirty
            && let Some(name) = self.playlist_view.as_ref()
        {
            let n = self.view.len();
            out.push(format!("save    playlist `{name}`, {n} tracks"));
        }
        for (name, rows) in self.dirty_tabs() {
            let n = rows.len();
            out.push(format!("save    playlist `{name}`, {n} tracks"));
        }
        for path in &self.doomed {
            let name = path.file_stem().unwrap_or_default().to_string_lossy();
            out.push(format!("delete  playlist `{name}`, its tracks stay"));
        }
        for (from, to) in &self.moves {
            out.push(format!(
                "move    {}  ->  {}",
                self.under_root(from),
                self.under_root(to)
            ));
        }
        for (from, to) in &self.copies {
            out.push(format!(
                "copy    {}  ->  {}",
                self.under_root(from),
                self.under_root(to)
            ));
        }
        for path in &self.doomed_files {
            out.push(format!("DELETE  {}", self.under_root(path)));
        }
        for dir in &self.doomed_dirs {
            out.push(format!(
                "DELETE  folder {}, and anything else left in it",
                self.under_root(dir)
            ));
        }
        out
    }

    /// Everything waiting to be written, for the marker on the statusline.
    pub fn unsaved(&self) -> bool {
        self.edit_dirty()
            || self.playlist_dirty
            || !self.doomed.is_empty()
            || !self.doomed_files.is_empty()
            || !self.moves.is_empty()
            || !self.copies.is_empty()
            || !self.doomed_dirs.is_empty()
            || self.dirty_tabs().next().is_some()
    }

    /// Playlists changed in a tab you are not looking at. `:w` writes
    /// everything, so `:changes` and the `[+]` marker have to see everything.
    fn dirty_tabs(&self) -> impl Iterator<Item = (String, Vec<usize>)> + '_ {
        self.tabs
            .iter()
            .enumerate()
            .filter(move |(i, tab)| *i != self.tab_idx && tab.dirty)
            .filter_map(|(_, tab)| tab.playlist.clone().map(|name| (name, tab.rows.clone())))
    }

    /// `:w`: renames the files you edited and saves the playlist you changed,
    /// in one press, and says what it did.
    pub fn write_all(&mut self) {
        // Checked before a single thing is written, so a clash leaves every
        // change pending instead of applying half of them.
        if let Some(problem) = self.move_problem() {
            self.error(problem);
            return;
        }

        self.undo_stack.clear();
        self.redo_stack.clear();
        let renames = self.edit_dirty();
        let playlist = self.playlist_dirty;
        let deletes = !self.doomed.is_empty();

        if renames {
            self.apply_edits();
            // A failed batch leaves the names pending; do not go on and write a
            // playlist as if everything were fine.
            if self.edit_dirty() {
                return;
            }
        }

        // Playlists changed in other tabs are written too: `:w` means all of it.
        for (name, rows) in self.dirty_tabs().collect::<Vec<_>>() {
            let tracks: Vec<&Track> = rows.iter().filter_map(|&i| self.tracks.get(i)).collect();
            if let Some(dir) = self.playlists_in()
                && library::write_m3u(&dir.join(format!("{name}.m3u")), &tracks).is_err()
            {
                self.error(format!("cannot save playlist `{name}`"));
                return;
            }
        }
        for (i, tab) in self.tabs.iter_mut().enumerate() {
            if i != self.tab_idx {
                tab.dirty = false;
            }
        }

        if playlist {
            let renamed = self.msg.take();
            self.save_open_playlist();
            if let (Some((before, false)), Some((after, false))) = (renamed, self.msg.clone()) {
                self.info(format!("{before}, {after}"));
            }
        } else if !renames && !deletes {
            self.save_open_playlist();
        }

        let (moved, copied, removed) = self.apply_file_ops();
        if moved + copied + removed > 0 {
            let before = self.msg.take();
            let mut parts = Vec::new();
            if moved > 0 {
                parts.push(format!("moved {moved} file{}", plural(moved)));
            }
            if copied > 0 {
                parts.push(format!("copied {copied} file{}", plural(copied)));
            }
            if removed > 0 {
                parts.push(format!("deleted {removed} file{}", plural(removed)));
            }
            let line = parts.join(", ");
            match before {
                Some((text, false)) => self.info(format!("{text}, {line}")),
                _ => self.info(line),
            }
        }

        if deletes {
            let gone = self.apply_deletes();
            if gone > 0 {
                let before = self.msg.take();
                let line = format!("deleted {gone} playlist{}", plural(gone));
                match before {
                    Some((text, false)) => self.info(format!("{text}, {line}")),
                    _ => self.info(line),
                }
            }
        }

        // A rename or a move changes where a row belongs in the sort, and a
        // move changes which folders exist. Put the list back in order here
        // rather than making the user follow every `:w` with a `:sort`.
        //
        // Deliberately not a rescan: `reload` throws away `playing` and the
        // queue, and the names on disk already match what is in memory, so
        // there is nothing to read back.
        if renames || moved + copied + removed > 0 {
            self.resort();
            let base = if self.root.is_dir() {
                self.root.clone()
            } else {
                self.root.parent().unwrap_or(&self.root).to_path_buf()
            };
            self.folders = library::folders(&self.tracks, &base);
            self.folder_cur = self.folder_cur.min(self.folders.len());
        }
    }

    /// Writes the open playlist back over its own file.
    pub fn save_open_playlist(&mut self) {
        let Some(name) = self.playlist_view.clone() else {
            self.error("`:w` needs a name: `:w late night`");
            return;
        };
        self.save_playlist(&name);
        self.playlist_dirty = false;
    }

    /// Where saved playlists live. State, not configuration, so it sits under
    /// the data dir next to the lyrics cache.
    /// Where this instance keeps playlists. A field rather than a constant so
    /// a test can point it at a scratch directory instead of writing into the
    /// playlists someone actually listens to.
    pub(super) fn playlists_in(&self) -> Option<PathBuf> {
        self.playlists_dir.clone().or_else(Self::playlist_dir)
    }

    pub fn playlist_dir() -> Option<PathBuf> {
        Some(dirs::data_dir()?.join("vibox/playlists"))
    }

    /// Rereads the playlist directory. Cheap: it lists names, it does not open
    /// the files.
    pub fn reload_playlists(&mut self) {
        let Some(dir) = self.playlists_in() else {
            return;
        };
        let mut found: Vec<(String, PathBuf)> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| library::is_playlist(path))
            .filter_map(|path| {
                let name = path.file_stem()?.to_string_lossy().into_owned();
                Some((name, path))
            })
            .collect();
        found.sort();
        self.playlists = found;
        self.pl_cur = self.pl_cur.min(self.playlists.len().saturating_sub(1));
    }

    pub fn move_playlist(&mut self, delta: isize) {
        if self.playlists.is_empty() {
            return;
        }
        let last = self.playlists.len() as isize - 1;
        self.pl_cur = (self.pl_cur as isize + delta).clamp(0, last) as usize;
        let h = self.folder_h.max(1);
        if self.pl_cur < self.pl_top {
            self.pl_top = self.pl_cur;
        }
        if self.pl_cur >= self.pl_top + h {
            self.pl_top = self.pl_cur + 1 - h;
        }
    }

    /// Shows the playlist under the cursor. The library, and so the folders
    /// tab, is left exactly as it was: a playlist is a view, not a new root.
    pub fn open_playlist(&mut self) {
        let Some((name, path)) = self.playlists.get(self.pl_cur).cloned() else {
            self.error("no playlists yet: `:w <name>` saves what you are looking at");
            return;
        };
        if self.jump_to_open(Some(&name), 0) {
            self.info(format!("`{name}` is already open"));
            return;
        }

        let wanted = match library::read_m3u(&path) {
            Ok(paths) => paths,
            Err(e) => {
                self.error(format!("{e}"));
                return;
            }
        };

        let mut rows = Vec::new();
        let mut missing = 0;
        for want in wanted {
            if let Some(row) = self.tracks.iter().position(|t| t.path == want) {
                rows.push(row);
            } else if want.is_file() {
                // A playlist may name tracks from outside the library; read
                // them in rather than dropping them, but mark them so they
                // stay reachable only through this playlist. Without that,
                // `everything` and the folder pane grow rows the library
                // never had.
                match library::scan(&want) {
                    Ok(mut found) if !found.is_empty() => {
                        let mut stray = found.remove(0);
                        stray.in_library = false;
                        self.tracks.push(stray);
                        rows.push(self.tracks.len() - 1);
                    }
                    _ => missing += 1,
                }
            } else {
                missing += 1;
            }
        }

        let n = rows.len();
        self.playlist_rows = rows;
        self.playlist_view = Some(name.clone());
        self.cur = 0;
        self.top = 0;
        self.rebuild_view();
        self.focus = Pane::Tracks;

        if missing > 0 {
            self.info(format!("{name}: {n} tracks, {missing} missing"));
        } else {
            self.info(format!("{name}: {n} tracks"));
        }
    }

    /// Writes the current view as a playlist. A bare name goes to the playlist
    /// directory, a name with a path in it goes exactly where it says.
    pub fn save_playlist(&mut self, name: &str) {
        let name = name.trim();
        if name.is_empty() {
            self.error("`:w` needs a name: `:w late night`");
            return;
        }

        let mut path = if name.contains('/') {
            PathBuf::from(name)
        } else {
            let Some(dir) = self.playlists_in() else {
                self.error("cannot find a data directory to save playlists in");
                return;
            };
            dir.join(name)
        };
        if path.extension().is_none() {
            path.set_extension("m3u");
        }

        let tracks: Vec<&Track> = self.view.iter().map(|&i| &self.tracks[i]).collect();
        if tracks.is_empty() {
            self.error("nothing to save");
            return;
        }

        let n = tracks.len();
        match library::write_m3u(&path, &tracks) {
            Ok(()) => {
                self.reload_playlists();
                self.info(format!("saved {n} tracks as {name}"));
            }
            Err(e) => self.error(format!("{e}")),
        }
    }
}
