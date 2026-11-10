//! Playlists: reading m3u files, editing them in memory, writing them on `:w`.

use std::path::{Path, PathBuf};

use crate::library::{self, Track};

use super::*;

impl App {
    /// `p`: puts the yanked tracks into the open playlist, after the cursor.
    ///
    /// Only into a playlist: vibox does not add files to a folder.
    pub fn paste_into_playlist(&mut self) {
        if self.busy() {
            return;
        }
        let Some(file) = self.playlist_view.clone() else {
            self.error("`p` adds to a playlist: open one with `gt` and enter");
            return;
        };
        if self.yank.is_empty() {
            self.error("nothing yanked");
            return;
        }
        // The row it was yanked from, or the row of the file it came from.
        let rows: Vec<usize> = self
            .yank
            .iter()
            .filter_map(|y| {
                self.yanked_row(y).or_else(|| {
                    self.row_at(&y.origin)
                        .filter(|&i| self.origin_of(&self.tracks[i].path) == y.origin)
                })
            })
            .collect();
        if rows.is_empty() {
            self.error("the yanked tracks are not in the list any more: yank them again");
            return;
        }

        self.checkpoint();
        let n = rows.len();
        let at = (self.cur + 1).min(self.playlist_rows.len());
        self.playlist_rows.splice(at..at, rows);
        self.pl_edits
            .insert(file.clone(), self.playlist_rows.clone());
        self.rebuild_view();
        self.goto(at);
        let name = playlist_label(&file);
        self.info(format!(
            "put {n} track{} into `{name}`, `:w` saves",
            plural(n)
        ));
    }

    /// `dd`: drops rows from the open playlist. The files stay on disk.
    pub fn remove_from_playlist(&mut self) {
        if self.busy() {
            return;
        }
        let Some(file) = self.playlist_view.clone() else {
            self.error("`dd` removes a track from a playlist, never from disk");
            return;
        };
        if self.playlist_rows.is_empty() {
            return;
        }
        self.checkpoint();
        let (a, b) = self.selection_range();
        let b = b.min(self.playlist_rows.len() - 1);

        // `dd` is a cut, as in vim: what comes out goes in the register, so a
        // `p` further down the playlist puts it back there.
        let taken: Vec<usize> = self.playlist_rows.drain(a..=b).collect();
        let paths: Vec<PathBuf> = taken
            .iter()
            .filter_map(|&i| self.tracks.get(i).map(|t| t.path.clone()))
            .collect();
        self.yank = self.register(&paths);
        // One register, as in vim: this is now what `p` puts.
        self.cut.clear();

        self.pl_edits
            .insert(file.clone(), self.playlist_rows.clone());
        self.exit_visual();
        self.cur = a;
        self.rebuild_view();
        let n = b - a + 1;
        let name = playlist_label(&file);
        self.info(format!(
            "cut {n} track{} from `{name}`, `p` puts them back, `:w` saves",
            plural(n)
        ));
    }

    /// Creates an empty playlist, ready for `p` to fill.
    pub fn create_playlist(&mut self, name: &str) {
        if self.busy() {
            return;
        }
        let name = name.trim();
        if name.is_empty() {
            self.error("`:mkplaylist` needs a name: `:mkplaylist late night`");
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

        // Under either extension, and under a name a rename is waiting to
        // give one: two files with one name would be one row in the sidebar
        // that means two things.
        let path = dir.join(format!("{name}.m3u"));
        if path.exists()
            || self
                .playlists
                .iter()
                .any(|(n, p)| n == name || self.playlist_named(p) == name)
        {
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
            Err(e) => self.fail("playlist not created", crate::reason::chain(&e)),
        }
    }

    /// `dd` on a playlist row marks the m3u for deletion, and a second `dd`
    /// takes the mark off again. Nothing happens on disk until `:w`.
    pub fn delete_playlist(&mut self) {
        if self.busy() {
            return;
        }
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

    /// A path as `root/...`, since every change is inside the library anyway.
    /// Quoted when it has a space in it, so the arrow stays readable.
    pub fn under_root(&self, path: &Path) -> String {
        let shown = self.plain_under_root(path);
        if shown.contains(' ') {
            format!("'{shown}'")
        } else {
            shown
        }
    }

    /// The same without the quotes and without the `root/` prefix.
    ///
    /// `under_root` quotes a name that contains a space, which is right for a
    /// one line message and wrong for `:changes`: the two names sit in their
    /// own columns there, and a quote that only one of them earns shows up as
    /// a change the rename never made. `root/` goes for the same reason it is
    /// there at all: on one line it says which tree this is, and on a screen
    /// where every row starts with it, it only costs width the names want.
    pub fn plain_under_root(&self, path: &Path) -> String {
        match path.strip_prefix(self.root_dir()) {
            Ok(rel) => rel.display().to_string(),
            Err(_) => path.display().to_string(),
        }
    }

    /// What `:w` would do right now, one line each, for `:changes`.
    ///
    /// Every line starts with its verb, which is also what colours it.
    pub fn pending_changes(&self) -> Vec<String> {
        let mut out = Vec::new();
        // Every rename, wherever it was typed, reads the same way.
        let mut queued = self.renames.clone();
        // The name being typed, the way leaving it will queue it: trimmed,
        // and an empty one keeps whatever was queued before. On a row that is
        // moving or being copied it changes where that row goes instead.
        let mut typed: Option<(PathBuf, PathBuf)> = None;
        if let Some(edit) = self.edit.as_ref() {
            let now = edit.buf.text().trim().to_string();
            let staged = match &edit.what {
                Renaming::Track(i) => self
                    .tracks
                    .get(*i)
                    .map(|t| t.path.clone())
                    .filter(|path| self.is_staged(path)),
                _ => None,
            };
            if now == self.name_of(&edit.what) {
                queued.remove(&edit.what);
            } else if !now.is_empty() {
                match staged {
                    Some(path) => {
                        typed = self.rename_target(&edit.what, &now).map(|to| (path, to));
                    }
                    None => {
                        queued.insert(edit.what.clone(), now);
                    }
                }
            }
        }
        // In the order `:w` runs them, so the list reads top to bottom: a copy
        // reads its file before anything moves it, a deletion frees a name
        // before a rename takes it, a copy back onto the name its file is
        // leaving reads it once moved, and folders go last.
        let batch = self.file_batch_with(
            typed
                .as_ref()
                .map(|(row, to)| (row.as_path(), to.as_path())),
        );
        for (from, to) in &batch.copies {
            out.push(format!(
                "copy    {}  ->  {}",
                self.plain_under_root(from),
                self.plain_under_root(to)
            ));
        }
        for path in &batch.deletes {
            out.push(format!("DELETE  {}", self.plain_under_root(path)));
        }
        let mut playlists = Vec::new();
        for (what, name) in &queued {
            // The target comes from `rename_target`, the same thing the write
            // uses, or `:changes` shows a name `:w` would never produce: it
            // puts the extension back, and building the path here by hand
            // dropped it.
            let Some(to) = self.rename_target(what, name) else {
                continue;
            };
            match what {
                _ if self.dormant(what) => continue,
                Renaming::Track(idx) => out.push(format!(
                    "rename  {}  ->  {}",
                    self.plain_under_root(&self.tracks[*idx].path),
                    self.plain_under_root(&to)
                )),
                Renaming::Folder(path) => out.push(format!(
                    "rename  folder {}  ->  {}",
                    self.plain_under_root(path),
                    self.plain_under_root(&to)
                )),
                Renaming::Playlist(old) => playlists.push(format!(
                    "rename  playlist `{}`  ->  `{name}`",
                    playlist_label(old)
                )),
            }
        }
        for (from, to) in &batch.moves {
            // A move that stays in its folder only changes the name.
            let verb = if from.parent() == to.parent() {
                "rename"
            } else {
                "move  "
            };
            out.push(format!(
                "{verb}  {}  ->  {}",
                self.plain_under_root(from),
                self.plain_under_root(to)
            ));
        }
        for (from, to) in &batch.copies_after {
            out.push(format!(
                "copy    {}  ->  {}",
                self.plain_under_root(from),
                self.plain_under_root(to)
            ));
        }
        // A marked folder lists every track in it, so `:changes` shows each
        // file that is about to go and not only the folder they are in.
        for dir in &self.doomed_dirs {
            for track in self
                .tracks
                .iter()
                .filter(|t| t.path.starts_with(dir) && !self.doomed_files.contains(&t.path))
            {
                out.push(format!("DELETE  {}", self.plain_under_root(&track.path)));
            }
            out.push(format!(
                "DELETE  folder {}, and anything else left in it",
                self.plain_under_root(dir)
            ));
        }
        out.extend(playlists);
        // A playlist about to be deleted is not saved first.
        for (path, rows) in self.pl_edits.iter().filter(|(p, _)| !self.is_doomed(p)) {
            let n = rows.len();
            out.push(format!(
                "save    playlist `{}`, {n} track{}",
                playlist_label(path),
                plural(n)
            ));
        }
        for path in &self.doomed {
            let name = path.file_stem().unwrap_or_default().to_string_lossy();
            out.push(format!("delete  playlist `{name}`, its tracks stay"));
        }
        out
    }

    /// Everything waiting to be written, for the marker on the statusline.
    pub fn unsaved(&self) -> bool {
        self.edit_dirty()
            || !self.pl_edits.is_empty()
            || !self.doomed.is_empty()
            || !self.doomed_files.is_empty()
            || !self.replaced.is_empty()
            || !self.moves.is_empty()
            || !self.copies.is_empty()
            || !self.doomed_dirs.is_empty()
    }

    /// Whether the playlist in view has an edit `:w` has not saved.
    pub fn view_edited(&self) -> bool {
        self.playlist_view
            .as_ref()
            .is_some_and(|path| self.pl_edits.contains_key(path))
    }

    /// The name of the playlist in view, the way the sidebar lists it.
    pub fn playlist_name(&self) -> Option<String> {
        self.playlist_view.as_deref().map(playlist_label)
    }

    /// Where this instance keeps playlists: the data dir next to the lyrics
    /// cache, since they are state rather than configuration, unless a test
    /// pointed it at a scratch directory.
    pub(super) fn playlists_in(&self) -> Option<PathBuf> {
        self.playlists_dir.clone().or_else(Self::playlist_dir)
    }

    pub fn playlist_dir() -> Option<PathBuf> {
        Some(dirs::data_dir()?.join("vibox/playlists"))
    }

    /// The file a playlist lives in: the one listed under that name, whatever
    /// its extension, or a new `.m3u` beside the others.
    pub(super) fn playlist_path(&self, name: &str) -> Option<PathBuf> {
        if let Some((_, path)) = self.playlists.iter().find(|(n, _)| n == name) {
            return Some(path.clone());
        }
        Some(self.playlists_in()?.join(format!("{name}.m3u")))
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
        if self.jump_to_open(Some(&path), 0) {
            self.info(format!("`{name}` is already open"));
            return;
        }

        // An edit `:w` has not saved yet is what the playlist is now.
        let rows = match self.pl_edits.get(&path) {
            Some(rows) => rows.clone(),
            None => {
                let entries = match library::read_m3u_entries(&path) {
                    Ok(entries) => entries,
                    Err(e) => {
                        self.error(crate::reason::chain(&e));
                        return;
                    }
                };
                entries.iter().map(|entry| self.row_for(entry)).collect()
            }
        };
        let missing = rows
            .iter()
            .filter(|&&i| !self.origin_of(&self.tracks[i].path).exists())
            .count();

        let n = rows.len() - missing;
        self.playlist_rows = rows;
        self.playlist_view = Some(path);
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

    /// The row of `tracks` an m3u entry points at, read in when the library
    /// does not have it.
    ///
    /// A playlist may name a file from outside the library, which is read and
    /// marked so it stays reachable only through the playlist, or a file that
    /// is not there at all, which gets a row built from what the playlist said
    /// about it, so that saving the playlist does not drop it.
    pub(super) fn row_for(&mut self, entry: &library::Entry) -> usize {
        // A file that is moving is on the row it is moving to, whatever row
        // sits on its old name now.
        let moving = self
            .moves
            .iter()
            .find(|(from, _)| *from == entry.path)
            .and_then(|(_, to)| self.row_at(to));
        if let Some(row) = moving.or_else(|| self.row_at(&entry.path)) {
            return row;
        }
        let read = if entry.path.is_file() {
            library::scan(&entry.path)
                .ok()
                .and_then(|mut found| found.pop())
        } else {
            None
        };
        let mut track = read.unwrap_or_else(|| library::missing(entry));
        track.in_library = false;
        self.tracks.push(track);
        self.tracks.len() - 1
    }

    /// The rows of a playlist as its file has them now.
    pub(super) fn playlist_rows_on_disk(&mut self, path: &Path) -> Option<Vec<usize>> {
        let entries = library::read_m3u_entries(path).ok()?;
        Some(entries.iter().map(|entry| self.row_for(entry)).collect())
    }

    /// Fills the rows of the playlist in view from its edit, or from its file
    /// when it has none, after something changed which of the two holds.
    pub(super) fn refresh_playlist_rows(&mut self) {
        let Some(path) = self.playlist_view.clone() else {
            return;
        };
        self.playlist_rows = match self.pl_edits.get(&path) {
            Some(rows) => rows.clone(),
            None => self.playlist_rows_on_disk(&path).unwrap_or_default(),
        };
    }

    /// Writes the current view as a playlist. A bare name goes to the playlist
    /// directory, a name with a path in it goes exactly where it says.
    ///
    /// It will not replace a file that is already there unless `overwrite`
    /// says so, which is what `:w!` is for, but writing the open playlist back
    /// to its own name is just saving it.
    pub fn save_playlist(&mut self, name: &str, overwrite: bool) {
        if self.busy() {
            return;
        }
        let name = name.trim();
        if name.is_empty() {
            self.error("`:w` needs a name: `:w late night`");
            return;
        }

        // The playlist in view first, when that is the name: two files can
        // share it under two extensions.
        let open = self
            .playlist_view
            .clone()
            .filter(|path| playlist_label(path) == name);
        let mut path = if name.contains('/') {
            library::absolute(&crate::excmd::expand(name))
        } else if let Some(open) = open {
            open
        } else if let Some(listed) = self.playlist_path(name)
            && listed.extension().is_some()
            && self.playlists.iter().any(|(n, _)| n == name)
        {
            listed
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
        let own = self.playlist_view.as_ref() == Some(&path);
        if !overwrite && !own && std::fs::symlink_metadata(&path).is_ok() {
            self.error(format!("`{name}` exists: `:w! {name}` replaces it"));
            return;
        }

        // A moved file where it is on disk now, so the `:w` that moves it
        // repairs this playlist along with the others. A pending copy is named
        // where it is going, since that copy is the file the row is.
        let tracks: Vec<Track> = self
            .view
            .iter()
            .map(|&i| {
                let mut track = self.tracks[i].clone();
                if let Some((from, _)) = self.moves.iter().find(|(_, to)| *to == track.path) {
                    track.path.clone_from(from);
                }
                track
            })
            .collect();
        let tracks: Vec<&Track> = tracks.iter().collect();
        if tracks.is_empty() {
            self.error("nothing to save");
            return;
        }

        let n = tracks.len();
        // The open playlist saved under its own name keeps every line of its
        // file the edit did not touch; anything else is written afresh.
        let saved = if own {
            library::save_m3u(&path, &tracks)
        } else {
            library::write_m3u(&path, &tracks)
        };
        match saved {
            Ok(()) => {
                if own {
                    self.pl_edits.remove(&path);
                }
                self.reload_playlists();
                if !self.playlists.iter().any(|(_, p)| *p == path)
                    && !self.elsewhere.contains(&path)
                {
                    self.elsewhere.push(path);
                }
                self.info(format!("saved {n} track{} as `{name}`", plural(n)));
            }
            Err(e) => self.fail("playlist not saved", crate::reason::chain(&e)),
        }
    }
}
