//! Moving, copying and deleting files, all of it behind `:set danger`.

use std::path::{Path, PathBuf};


use crate::library::{self, Track};

use super::*;

impl App {
    pub fn is_doomed_file(&self, path: &Path) -> bool {
        self.doomed_files.iter().any(|p| p == path)
    }

    pub fn is_cut(&self, path: &Path) -> bool {
        self.cut.iter().any(|p| p == path)
    }

    /// `dd` on a track with danger on: marks the files, and remembers them so
    /// a `p` somewhere else turns the deletion into a move, exactly like vim
    /// deleting text and putting it back.
    pub fn cut_tracks(&mut self) {
        if !self.danger {
            self.error("`dd` here needs `:set danger`; it deletes files");
            return;
        }
        let (a, b) = self.selection_range();
        let b = b.min(self.view.len().saturating_sub(1));
        let paths: Vec<PathBuf> = self.view[a..=b]
            .iter()
            .map(|&i| self.tracks[i].path.clone())
            .collect();
        if paths.is_empty() {
            return;
        }

        self.checkpoint();
        let n = paths.len();
        self.cut.clone_from(&paths);
        self.doomed_files.extend(paths);
        self.exit_visual();
        self.cur = self.cur.min(self.view.len().saturating_sub(1));
        self.rebuild_view();
        self.info(format!(
            "{n} file{} marked for deletion, `p` elsewhere moves them instead, `u` undoes",
            plural(n)
        ));
    }

    /// `p` in a folder with files cut: moves them into the folder in view.
    pub fn move_cut_here(&mut self) -> bool {
        if self.cut.is_empty() {
            return false;
        }
        let Some(dir) = self.current_dir() else {
            self.error("no folder here to move into");
            return true;
        };

        self.checkpoint();
        let cut = std::mem::take(&mut self.cut);
        let n = cut.len();
        for from in cut {
            self.doomed_files.retain(|p| *p != from);
            let Some(name) = from.file_name() else { continue };
            let to = dir.join(name);
            if let Some(track) = self.tracks.iter_mut().find(|t| t.path == from) {
                track.path = to.clone();
            }
            self.moves.push((from, to));
        }
        self.rebuild_view();
        let shown = dir.display().to_string();
        self.info(format!("{n} file{} will move to {shown}, `:w` does it", plural(n)));
        true
    }

    /// `dd` on a folder row with danger on: marks the folder and everything
    /// under it, subfolders included.
    ///
    /// The tracks are marked one by one rather than hidden behind the folder,
    /// so `:changes` still lists every file that is about to go.
    pub fn cut_folder(&mut self) {
        if !self.danger {
            self.error("`dd` on a folder needs `:set danger`; it deletes the folder and its tracks");
            return;
        }
        let Some((label, dir)) = self.folders.get(self.folder_cur.wrapping_sub(1)).cloned() else {
            self.error("that is the whole library, not a folder");
            return;
        };

        if let Some(at) = self.doomed_dirs.iter().position(|p| *p == dir) {
            self.checkpoint();
            self.doomed_dirs.remove(at);
            self.doomed_files.retain(|f| !f.starts_with(&dir));
            self.rebuild_view();
            self.info(format!("`{label}` kept"));
            return;
        }

        self.checkpoint();
        let inside: Vec<PathBuf> = self
            .tracks
            .iter()
            .filter(|t| t.path.starts_with(&dir))
            .map(|t| t.path.clone())
            .collect();
        let n = inside.len();
        self.doomed_files.extend(inside);
        self.doomed_dirs.push(dir);
        self.rebuild_view();
        self.info(format!(
            "`{label}` and {n} track{} marked, `:w` deletes them, `dd` undoes it",
            plural(n)
        ));
    }

    pub fn is_doomed_dir(&self, path: &Path) -> bool {
        self.doomed_dirs.iter().any(|p| path.starts_with(p))
    }

    /// `p` in a folder with tracks yanked: copies them in, danger mode only.
    ///
    /// The move case is `dd` then `p`; this is the `y` then `p` case, and it is
    /// a copy for the same reason it is in vim, where the yanked text stays
    /// where it was.
    pub fn copy_yank_here(&mut self) -> bool {
        if self.yank.is_empty() || self.playlist_view.is_some() {
            return false;
        }
        if !self.danger {
            self.error("copying files here needs `:set danger`");
            return true;
        }
        let Some(dir) = self.current_dir() else {
            self.error("no folder here to copy into");
            return true;
        };

        self.checkpoint();
        let mut n = 0;
        for from in self.yank.clone() {
            let Some(name) = from.file_name() else { continue };
            let to = dir.join(name);
            if to == from {
                continue;
            }
            if let Some(source) = self.tracks.iter().find(|t| t.path == from) {
                let mut copy = source.clone();
                copy.path = to.clone();
                self.tracks.push(copy);
            }
            self.copies.push((from, to));
            n += 1;
        }

        self.rebuild_view();
        let shown = self.under_root(&dir);
        self.info(format!(
            "{n} file{} will be copied to {shown}, `:w` does it",
            plural(n)
        ));
        true
    }

    /// The folder the view is looking at, which is where a `p` puts files.
    fn current_dir(&self) -> Option<PathBuf> {
        if self.folder_cur > 0 {
            return self.folders.get(self.folder_cur - 1).map(|(_, p)| p.clone());
        }
        self.current_track().map(|t| t.dir().to_path_buf())
    }

    /// `:mkdir`: a new folder, made right away.
    ///
    /// Typing a command with a name in it is the confirmation, the way `:w
    /// file` writes in vim. Only the buffer edits wait for `:w`.
    pub fn make_dir(&mut self, name: &str) {
        if !self.danger {
            self.error("`:mkdir` needs `:set danger`; it writes to your library");
            return;
        }
        let name = name.trim();
        if name.is_empty() {
            self.error("`:mkdir` needs a name");
            return;
        }

        let path = self.root.join(name);
        if path.exists() {
            self.error(format!("`{name}` already exists"));
            return;
        }
        match std::fs::create_dir_all(&path) {
            Ok(()) => {
                self.folders = library::folders(&self.tracks, &self.root);
                let shown = self.under_root(&path);
                self.info(format!("{shown} created"));
            }
            Err(e) => self.error(format!("cannot create `{name}`: {e}")),
        }
    }

    /// Checks every pending move before any of them runs.
    ///
    /// A batch is all or nothing: moving three files into a folder where one of
    /// them already exists must not move the other two and leave you to work
    /// out which. Same rule as the rename batch.
    pub(super) fn move_problem(&self) -> Option<String> {
        let mut targets: Vec<&PathBuf> = Vec::new();
        for (_, to) in self.moves.iter().chain(self.copies.iter()) {
            if to.exists() {
                return Some(format!(
                    "`{}` already exists, nothing written",
                    self.under_root(to)
                ));
            }
            if targets.contains(&to) {
                return Some(format!(
                    "two files would both become `{}`, nothing written",
                    self.under_root(to)
                ));
            }
            targets.push(to);
        }
        None
    }

    /// The file operations, run by `:w` after the renames and playlists.
    ///
    /// Playlists are repaired afterwards for the same reason a rename repairs
    /// them: a moved file must keep playing from its playlists, and a deleted
    /// one must not sit there as an entry that points nowhere.
    pub(super) fn apply_file_ops(&mut self) -> (usize, usize, usize) {
        let (mut moved, mut copied, mut gone) = (0, 0, 0);
        let mut done: Vec<(PathBuf, PathBuf)> = Vec::new();
        let mut removed: Vec<PathBuf> = Vec::new();

        for (from, to) in std::mem::take(&mut self.moves) {
            match std::fs::rename(&from, &to) {
                Ok(()) => {
                    done.push((from, to));
                    moved += 1;
                }
                Err(e) => self.error(format!("cannot move `{}`: {e}", from.display())),
            }
        }
        for (from, to) in std::mem::take(&mut self.copies) {
            if let Err(e) = std::fs::copy(&from, &to) {
                self.error(format!("cannot copy `{}`: {e}", self.under_root(&from)));
            } else {
                copied += 1;
            }
        }
        for path in std::mem::take(&mut self.doomed_files) {
            match std::fs::remove_file(&path) {
                Ok(()) => {
                    removed.push(path);
                    gone += 1;
                }
                Err(e) => self.error(format!("cannot delete `{}`: {e}", path.display())),
            }
        }

        // Deepest first, so a folder inside a marked folder is already gone.
        let mut dirs = std::mem::take(&mut self.doomed_dirs);
        dirs.sort_by_key(|d| std::cmp::Reverse(d.components().count()));
        for dir in dirs {
            match std::fs::remove_dir_all(&dir) {
                Ok(()) => gone += 1,
                Err(e) => self.error(format!("cannot delete `{}`: {e}", self.under_root(&dir))),
            }
        }
        if !done.is_empty() {
            self.repair_playlists(&done);
        }
        if !removed.is_empty() {
            self.drop_from_playlists(&removed);
            self.forget_tracks(&removed);
        }

        self.cut.clear();
        self.folders = library::folders(&self.tracks, &self.root);
        self.folder_cur = self.folder_cur.min(self.folders.len());
        self.folder_open = self.folder_open.min(self.folders.len());
        self.rebuild_view();
        (moved, copied, gone)
    }

    /// Drops deleted tracks out of the library and repoints everything that
    /// held an index into it.
    ///
    /// `view`, `queue`, the open playlist and every other tab all store indices
    /// into `tracks`, so removing an entry without remapping them would leave
    /// each of those pointing at whatever slid into the gap.
    pub(super) fn forget_tracks(&mut self, gone: &[PathBuf]) {
        let mut remap: Vec<Option<usize>> = Vec::with_capacity(self.tracks.len());
        let mut next = 0;
        for track in &self.tracks {
            if gone.contains(&track.path) {
                remap.push(None);
            } else {
                remap.push(Some(next));
                next += 1;
            }
        }

        self.tracks.retain(|t| !gone.contains(&t.path));
        let follow = |rows: &[usize]| -> Vec<usize> {
            rows.iter().filter_map(|&i| remap[i]).collect()
        };

        self.playlist_rows = follow(&self.playlist_rows);
        self.queue = follow(&self.queue);
        self.qpos = self.qpos.min(self.queue.len().saturating_sub(1));
        self.playing = self.playing.and_then(|i| remap[i]);
        for tab in &mut self.tabs {
            tab.rows = tab.rows.iter().filter_map(|&i| remap[i]).collect();
        }
        self.renames = std::mem::take(&mut self.renames)
            .into_iter()
            .filter_map(|(what, name)| match what {
                Renaming::Track(i) => remap[i].map(|new| (Renaming::Track(new), name)),
                other => Some((other, name)),
            })
            .collect();
    }

    /// Takes deleted files out of every playlist that named them.
    fn drop_from_playlists(&mut self, gone: &[PathBuf]) -> usize {
        let mut fixed = 0;
        for (_, path) in self.playlists.clone() {
            let Ok(paths) = library::read_m3u(&path) else {
                continue;
            };
            if !paths.iter().any(|p| gone.contains(p)) {
                continue;
            }

            let tracks: Vec<&Track> = paths
                .iter()
                .filter(|p| !gone.contains(p))
                .filter_map(|p| self.tracks.iter().find(|t| t.path == *p))
                .collect();
            if library::write_m3u(&path, &tracks).is_ok() {
                fixed += 1;
            }
        }
        fixed
    }
}
