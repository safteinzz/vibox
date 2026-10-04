//! `/` and `?`, matching against the rows of the focused list.

use super::*;

impl App {
    /// Searches the list that has focus: folder labels, playlist names, or
    /// the columns of each track row. With the lyrics pane focused it searches
    /// the tracks.
    ///
    /// Vim smartcase: a lowercase pattern matches anything, an uppercase one
    /// is taken literally.
    pub fn search(&mut self, pattern: &str, backward: bool) -> bool {
        if pattern.is_empty() {
            return false;
        }
        let smart = pattern.chars().any(char::is_uppercase);
        let needle = if smart {
            pattern.to_string()
        } else {
            pattern.to_lowercase()
        };
        let hit = |hay: &str| {
            if smart {
                hay.contains(&needle)
            } else {
                hay.to_lowercase().contains(&needle)
            }
        };

        match (self.focus, self.tab) {
            (Pane::Folders, Tab::Folders) => {
                // Row 0 is `* everything`, which is not a folder to find.
                let found = find(self.folders.len() + 1, self.folder_cur, backward, |row| {
                    row > 0 && hit(&self.folders[row - 1].0)
                });
                found.inspect(|&row| self.move_folder(row as isize - self.folder_cur as isize))
            }
            (Pane::Folders, Tab::Playlists) => {
                let found = find(self.playlists.len(), self.pl_cur, backward, |row| {
                    hit(&self.playlists[row].0)
                });
                found.inspect(|&row| self.move_playlist(row as isize - self.pl_cur as isize))
            }
            (Pane::Tracks | Pane::Lyrics, _) => {
                let found = find(self.view.len(), self.cur, backward, |row| {
                    hit(&self.tracks[self.view[row]].haystack())
                });
                found.inspect(|&row| self.goto(row))
            }
        }
        .is_some()
    }
}

/// The first row after `from` (before it when `backward`) that `matches`,
/// wrapping around and trying `from` itself last.
fn find(n: usize, from: usize, backward: bool, matches: impl Fn(usize) -> bool) -> Option<usize> {
    (1..=n)
        .map(|step| {
            if backward {
                (from + n - (step % n)) % n
            } else {
                (from + step) % n
            }
        })
        .find(|&row| matches(row))
}
